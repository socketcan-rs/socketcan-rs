//! socketCAN support.
//!
//! The Linux kernel supports using CAN-devices through a network-like API
//! (see https://www.kernel.org/doc/Documentation/networking/can.txt). This
//! crate allows easy access to this functionality without having to wrestle
//! libc calls.
//!
//! # An introduction to CAN
//!
//! The CAN bus was originally designed to allow microcontrollers inside a
//! vehicle to communicate over a single shared bus. Messages called
//! *frames* are multicast to all devices on the bus.
//!
//! Every frame consists of an ID and a payload of up to 8 bytes. If two
//! devices attempt to send a frame at the same time, the device with the
//! higher ID will notice the conflict, stop sending and reattempt to sent its
//! frame in the next time slot. This means that the lower the ID, the higher
//! the priority. Since most devices have a limited buffer for outgoing frames,
//! a single device with a high priority (== low ID) can block communication
//! on that bus by sending messages too fast.
//!
//! The Linux socketcan subsystem makes the CAN bus available as a regular
//! networking device. Opening an network interface allows receiving all CAN
//! messages received on it. A device CAN be opened multiple times, every
//! client will receive all CAN frames simultaneously.
//!
//! Similarly, CAN frames can be sent to the bus by multiple client
//! simultaneously as well.
//!
//! # Hardware and more information
//!
//! More information on CAN [can be found on Wikipedia](). When not running on
//! an embedded platform with already integrated CAN components,
//! [Thomas Fischl's USBtin](http://www.fischl.de/usbtin/) (see
//! [section 2.4](http://www.fischl.de/usbtin/#socketcan)) is one of many ways
//! to get started.
//!
//! # RawFd
//!
//! Raw access to the underlying file descriptor and construction through
//! is available through the `AsRawFd`, `IntoRawFd` and `FromRawFd`
//! implementations.

mod err;
pub use crate::err::{CanError, CanErrorDecodingFailure};
pub mod dump;

#[cfg(test)]
mod tests;

use libc::{c_int, c_short, c_void, c_uint, socket, SOCK_RAW, close, bind, sockaddr, read, write,
           setsockopt, SOL_SOCKET, SO_RCVTIMEO, timeval, EINPROGRESS, SO_SNDTIMEO, time_t,
           suseconds_t, fcntl, F_GETFL, F_SETFL, O_NONBLOCK};
use itertools::Itertools;
use std::convert::TryFrom;
use std::{error, fmt, io, time};
use std::mem::size_of;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use nix::net::if_::if_nametoindex;


/// Check an error return value for timeouts.
///
/// Due to the fact that timeouts are reported as errors, calling `read_frame`
/// on a socket with a timeout that does not receive a frame in time will
/// result in an error being returned. This trait adds a `should_retry` method
/// to `Error` and `Result` to check for this condition.
pub trait ShouldRetry {
    /// Check for timeout
    ///
    /// If `true`, the error is probably due to a timeout.
    fn should_retry(&self) -> bool;
}

impl ShouldRetry for io::Error {
    fn should_retry(&self) -> bool {
        match self.kind() {
            // EAGAIN, EINPROGRESS and EWOULDBLOCK are the three possible codes
            // returned when a timeout occurs. the stdlib already maps EAGAIN
            // and EWOULDBLOCK os WouldBlock
            io::ErrorKind::WouldBlock => true,
            // however, EINPROGRESS is also valid
            io::ErrorKind::Other => {
                if let Some(i) = self.raw_os_error() {
                    i == EINPROGRESS
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

impl<E: fmt::Debug> ShouldRetry for io::Result<E> {
    fn should_retry(&self) -> bool {
        if let &Err(ref e) = self {
            e.should_retry()
        } else {
            false
        }
    }
}

// constants stolen from C headers
const AF_CAN: c_int = 29;
const PF_CAN: c_int = 29;
const CAN_RAW: c_int = 1;
const SOL_CAN_BASE: c_int = 100;
const SOL_CAN_RAW: c_int = SOL_CAN_BASE + CAN_RAW;
const CAN_RAW_FILTER: c_int = 1;
const CAN_RAW_ERR_FILTER: c_int = 2;
const CAN_RAW_FD_FRAMES: c_int = 5;

/// if set, indicate 29 bit extended format
pub const EFF_FLAG: u32 = 0x80000000;

/// remote transmission request flag
pub const RTR_FLAG: u32 = 0x40000000;

/// error flag
pub const ERR_FLAG: u32 = 0x20000000;

/// valid bits in standard frame id
pub const SFF_MASK: u32 = 0x000007ff;

/// valid bits in extended frame id
pub const EFF_MASK: u32 = 0x1fffffff;

/// valid bits in error frame
pub const ERR_MASK: u32 = 0x1fffffff;

/// 'legacy' CAN frame
const CAN_MTU: usize = 16;
const CAN_DATA_LEN_MAX: usize = 8;

/// CAN FD frame
const CANFD_MTU: usize = 72;
const CANFD_DATA_LEN_MAX: usize = 64;

/// CAN FD flags
const CANFD_BRS: u8 = 0x01; /* bit rate switch (second bitrate for payload data) */
const CANFD_ESI: u8 = 0x02; /* error state indicator of the transmitting node */


fn c_timeval_new(t: time::Duration) -> timeval {
    timeval {
        tv_sec: t.as_secs() as time_t,
        tv_usec: (t.subsec_nanos() / 1000) as suseconds_t,
    }
}

#[derive(Debug)]
#[repr(C)]
struct CanAddr {
    _af_can: c_short,
    if_index: c_int, // address familiy,
    rx_id: u32,
    tx_id: u32,
}

#[derive(Debug)]
/// Errors opening socket
pub enum CanSocketOpenError {
    /// Device could not be found
    LookupError(nix::Error),

    /// System error while trying to look up device name
    IOError(io::Error),
}

impl fmt::Display for CanSocketOpenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CanSocketOpenError::LookupError(ref e) => write!(f, "CAN Device not found: {}", e),
            CanSocketOpenError::IOError(ref e) => write!(f, "IO: {}", e),
        }
    }
}

#[allow(deprecated)]
impl error::Error for CanSocketOpenError {
    fn description(&self) -> &str {
        match *self {
            CanSocketOpenError::LookupError(_) => "can device not found",
            CanSocketOpenError::IOError(ref e) => e.description(),
        }
    }

    fn cause(&self) -> Option<&dyn error::Error> {
        match *self {
            CanSocketOpenError::LookupError(ref e) => Some(e),
            CanSocketOpenError::IOError(ref e) => Some(e),
        }
    }
}


#[derive(Debug, Copy, Clone)]
/// Error that occurs when creating CAN packets
pub enum ConstructionError {
    /// CAN ID was outside the range of valid IDs
    IDTooLarge,
    /// More than 8 Bytes of payload data were passed in
    TooMuchData,
}

impl fmt::Display for ConstructionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ConstructionError::IDTooLarge => write!(f, "CAN ID too large"),
            ConstructionError::TooMuchData => {
                write!(f, "Payload is larger than CAN maximum of 8 bytes")
            }
        }
    }
}

impl error::Error for ConstructionError {
    fn description(&self) -> &str {
        match *self {
            ConstructionError::IDTooLarge => "can id too large",
            ConstructionError::TooMuchData => "too much data",
        }
    }
}

impl From<nix::Error> for CanSocketOpenError {
    fn from(e: nix::Error) -> CanSocketOpenError {
        CanSocketOpenError::LookupError(e)
    }
}

impl From<io::Error> for CanSocketOpenError {
    fn from(e: io::Error) -> CanSocketOpenError {
        CanSocketOpenError::IOError(e)
    }
}

/// A socket for a CAN device.
///
/// Will be closed upon deallocation. To close manually, use std::drop::Drop.
/// Internally this is just a wrapped file-descriptor.
#[derive(Debug)]
pub struct CanNormalSocket {
    fd: c_int,
}

#[derive(Debug)]
pub struct CanFdSocket {
    fd: c_int,
}


fn raw_open_socket(if_index: c_uint) -> Result<i32, CanSocketOpenError> {
    let addr = CanAddr {
        _af_can: AF_CAN as c_short,
        if_index: if_index as c_int,
        rx_id: 0, // ?
        tx_id: 0, // ?
    };

    let sock_fd = unsafe {
        socket(PF_CAN, SOCK_RAW, CAN_RAW)
    };

    if sock_fd == -1 {
        return Err(CanSocketOpenError::from(io::Error::last_os_error()));
    }

    let bind_rv = unsafe {
        let sockaddr_ptr = &addr as *const CanAddr;
        bind(sock_fd,
             sockaddr_ptr as *const sockaddr,
             size_of::<CanAddr>() as u32)
    };

    if bind_rv == -1 {
        let e = io::Error::last_os_error();
        unsafe {
            close(sock_fd);
        }
        return Err(CanSocketOpenError::from(e));
    }

    Ok(sock_fd)
}

fn set_fd_mode(socket_fd: c_int, fd_mode_enable: bool) -> io::Result<c_int> {
    let fd_mode_enable = fd_mode_enable as c_int;
    let opt_ptr = &fd_mode_enable as *const c_int;
    let rv = unsafe {
        setsockopt(socket_fd, SOL_CAN_RAW, CAN_RAW_FD_FRAMES, opt_ptr as *const c_void, size_of::<c_int>() as u32)
    };

    if rv == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket_fd)
}

fn raw_write_frame<T>(socket_fd: c_int, frame: &T) -> io::Result<()> {
    let write_rv = unsafe {
        let frame_ptr = frame as *const T;
        write(socket_fd, frame_ptr as *const c_void, size_of::<T>())
    };

    if write_rv as usize != size_of::<T>() {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

pub trait CanSocket: AsRawFd {
    type FrameType;

    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "vcan0" or "socan0".
    fn open(ifname: &str) -> Result<Self, CanSocketOpenError>
        where Self: Sized
    {
        let if_index = if_nametoindex(ifname)?;
        Self::open_if(if_index)
    }

    /// Open CAN device by interface number.
    ///
    /// Opens a CAN device by kernel interface number.
    fn open_if(if_index: c_uint) -> Result<Self, CanSocketOpenError> where Self: Sized;

    fn close(&mut self) -> io::Result<()> {
        unsafe {
            if close(self.as_raw_fd()) == -1 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Blocking read a single can frame.
    fn read_frame(&self) -> io::Result<Self::FrameType>;

    /// Write a single can frame.
    ///
    /// Note that this function can fail with an `EAGAIN` error or similar.
    /// Use `write_frame_insist` if you need to be sure that the message got
    /// sent or failed.
    fn write_frame(&self, frame: &Self::FrameType) -> io::Result<()>;

    /// Blocking write a single can frame, retrying until it gets sent
    /// successfully.
    fn write_frame_insist(&self, frame: &Self::FrameType) -> io::Result<()> {
        loop {
            match self.write_frame(frame) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if !e.should_retry() {
                        return Err(e);
                    }
                }
            }
        }
    }

    /// Change socket to non-blocking mode
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        // retrieve current flags
        let oldfl = unsafe { fcntl(self.as_raw_fd(), F_GETFL) };

        if oldfl == -1 {
            return Err(io::Error::last_os_error());
        }

        let newfl = if nonblocking {
            oldfl | O_NONBLOCK
        } else {
            oldfl & !O_NONBLOCK
        };

        let rv = unsafe { fcntl(self.as_raw_fd(), F_SETFL, newfl) };

        if rv != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Sets the read timeout on the socket
    ///
    /// For convenience, the result value can be checked using
    /// `ShouldRetry::should_retry` when a timeout is set.
    fn set_read_timeout(&self, duration: time::Duration) -> io::Result<()> {
        let rv = unsafe {
            let tv = c_timeval_new(duration);
            let tv_ptr: *const timeval = &tv as *const timeval;
            setsockopt(self.as_raw_fd(),
                       SOL_SOCKET,
                       SO_RCVTIMEO,
                       tv_ptr as *const c_void,
                       size_of::<timeval>() as u32)
        };

        if rv != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    /// Sets the write timeout on the socket
    fn set_write_timeout(&self, duration: time::Duration) -> io::Result<()> {
        let rv = unsafe {
            let tv = c_timeval_new(duration);
            let tv_ptr: *const timeval = &tv as *const timeval;
            setsockopt(self.as_raw_fd(),
                       SOL_SOCKET,
                       SO_SNDTIMEO,
                       tv_ptr as *const c_void,
                       size_of::<timeval>() as u32)
        };

        if rv != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    /// Sets the filter mask on the socket.
    fn set_filter(&self, filters: &[CanFilter]) -> io::Result<()> {

        // TODO: Handle different *_FILTER sockopts.

        let rv = if filters.len() < 1 {
            // clears all filters
            unsafe { setsockopt(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_FILTER, 0 as *const c_void, 0) }
        } else {
            unsafe {
                let filters_ptr = &filters[0] as *const CanFilter;
                setsockopt(self.as_raw_fd(),
                           SOL_CAN_RAW,
                           CAN_RAW_FILTER,
                           filters_ptr as *const c_void,
                           (size_of::<CanFilter>() * filters.len()) as u32)
            }
        };

        if rv != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    /// Disable reception of CAN frames.
    ///
    /// Sets a completely empty filter; disabling all CAN frame reception.
    #[inline(always)]
    fn filter_drop_all(&self) -> io::Result<()> {
        self.set_filter(&[])
    }

    /// Accept all frames, disabling any kind of filtering.
    ///
    /// Replace the current filter with one containing a single rule that
    /// acceps all CAN frames.
    fn filter_accept_all(&self) -> io::Result<()> {
        // safe unwrap: 0, 0 is a valid mask/id pair
        self.set_filter(&[CanFilter::new(0, 0).unwrap()])
    }

    #[inline(always)]
    fn set_error_filter(&self, mask: u32) -> io::Result<()> {
        let rv = unsafe {
            setsockopt(self.as_raw_fd(),
                       SOL_CAN_RAW,
                       CAN_RAW_ERR_FILTER,
                       (&mask as *const u32) as *const c_void,
                       size_of::<u32>() as u32)
        };

        if rv != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())

    }

    #[inline(always)]
    fn error_filter_drop_all(&self) -> io::Result<()> {
        self.set_error_filter(0)
    }

    #[inline(always)]
    fn error_filter_accept_all(&self) -> io::Result<()> {
        self.set_error_filter(ERR_MASK)
    }
}

impl CanSocket for CanNormalSocket {
    type FrameType = CanNormalFrame;

    fn open_if(if_index: c_uint) -> Result<Self, CanSocketOpenError> {
        raw_open_socket(if_index).map(|sock_fd| Self { fd: sock_fd })
    }

    fn write_frame(&self, frame: &CanNormalFrame) -> io::Result<()> {
        raw_write_frame(self.fd, frame)
    }

    fn read_frame(&self) -> io::Result<CanNormalFrame> {
        let mut frame = Self::FrameType::default();

        let read_rv = unsafe {
            let frame_ptr = &mut frame as *mut CanNormalFrame;
            read(self.fd, frame_ptr as *mut c_void, size_of::<CanNormalFrame>())
        };

        if read_rv as usize != size_of::<CanNormalFrame>() {
            return Err(io::Error::last_os_error());
        }

        Ok(frame)
    }
}

impl CanSocket for CanFdSocket {
    type FrameType = CanAnyFrame;

    fn open_if(if_index: c_uint) -> Result<Self, CanSocketOpenError> {
        raw_open_socket(if_index)
            .and_then(|sock_fd| set_fd_mode(sock_fd, true)
                      .map_err(|io_err| CanSocketOpenError::IOError(io_err))
                     )
            .map(|sock_fd| Self { fd: sock_fd })
    }

    fn write_frame(&self, frame: &CanAnyFrame) -> io::Result<()> {
        match frame {
            CanAnyFrame::Normal(frame) => {
                raw_write_frame(self.fd, frame)
            },
            CanAnyFrame::Fd(fd_frame) => {
                raw_write_frame(self.fd, fd_frame)
            }
        }
    }

    fn read_frame(&self) -> io::Result<CanAnyFrame> {
        let mut frame = CanFdFrame::default();

        let read_rv = unsafe {
            let frame_ptr = &mut frame as *mut CanFdFrame;
            read(self.fd, frame_ptr as *mut c_void, size_of::<CanFdFrame>())
        };
        match read_rv as usize {
            CAN_MTU => CanNormalFrame::try_from(frame)
                .map(|frame| frame.into())
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "BUG in read_frame: cannot convert to CanNormalFrame")),

            CANFD_MTU => Ok(frame.into()), // Ok(CanAnyFrame::from(frame)),

            _ => Err(io::Error::last_os_error()),
        }
    }
}


impl AsRawFd for CanNormalSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl FromRawFd for CanNormalSocket {
    unsafe fn from_raw_fd(fd: RawFd) -> CanNormalSocket {
        CanNormalSocket { fd: fd }
    }
}

impl IntoRawFd for CanNormalSocket {
    fn into_raw_fd(self) -> RawFd {
        self.fd
    }
}

impl Drop for CanNormalSocket {
    fn drop(&mut self) {
        self.close().ok(); // ignore result
    }
}

impl AsRawFd for CanFdSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl FromRawFd for CanFdSocket {
    unsafe fn from_raw_fd(fd: RawFd) -> CanFdSocket {
        CanFdSocket { fd: fd }
    }
}

impl IntoRawFd for CanFdSocket {
    fn into_raw_fd(self) -> RawFd {
        self.fd
    }
}

impl Drop for CanFdSocket {
    fn drop(&mut self) {
        self.close().ok(); // ignore result
    }
}
/// CanNormalFrame
///
/// Uses the same memory layout as the underlying kernel struct for performance
/// reasons.
#[derive(Debug, Copy, Clone)]
#[repr(C, align(8))]
pub struct CanNormalFrame {
    /// 32 bit CAN_ID + EFF/RTR/ERR flags
    _id: u32,

    /// data length. Bytes beyond are not valid
    _data_len: u8,

    /// padding
    _pad: u8,

    /// reserved
    _res0: u8,

    /// reserved
    _res1: u8,

    /// buffer for data
    _data: [u8; CAN_DATA_LEN_MAX],
}

/// CanFdFrame
///
/// Uses the same memory layout as the underlying kernel struct for performance
/// reasons.
#[derive(Debug, Copy, Clone)]
#[repr(C, align(8))]
pub struct CanFdFrame {
    /// 32 bit CAN_ID + EFF/RTR/ERR flags
    _id: u32,

    /// data length. Bytes beyond are not valid
    _data_len: u8,

    /// flags for CAN FD
    _flags: u8,

    /// reserved
    _res0: u8,

    /// reserved
    _res1: u8,

    /// buffer for data
    _data: [u8; CANFD_DATA_LEN_MAX]
}

pub enum CanAnyFrame {
    Normal(CanNormalFrame),
    Fd(CanFdFrame),
}

impl fmt::Debug for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Normal(frame) => {
                write!(f, "CAN Frame {:?}", frame )
            }

            Self::Fd(frame) => {
                write!(f, "CAN FD Frame {:?}", frame )
            }
        }
    }
}

impl Default for CanNormalFrame {
    fn default() -> Self {
        CanNormalFrame {
            _id: 0,
            _data_len: 0,
            _pad: 0,
            _res0: 0,
            _res1: 0,
            _data: [0; CAN_DATA_LEN_MAX],
        }
    }
}

impl Default for CanFdFrame {
    fn default() -> Self {
        CanFdFrame {
            _id: 0,
            _data_len: 0,
            _flags: 0,
            _res0: 0,
            _res1: 0,
            _data: [0; CANFD_DATA_LEN_MAX],
        }
    }
}

impl TryFrom<CanFdFrame> for CanNormalFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanFdFrame) -> Result<Self, Self::Error> {
        if frame._data_len > CAN_DATA_LEN_MAX as u8 {
            return Err(ConstructionError::TooMuchData)
        }

        CanNormalFrame::new(
            frame.id(),
            &frame.data()[..(frame._data_len as usize)],
            frame.is_rtr(),
            frame.is_error())
    }
}

impl From<CanNormalFrame> for CanFdFrame {
    fn from(frame: CanNormalFrame) -> Self {
        CanFdFrame {
            _id: frame._id,
            _data_len: frame.data().len() as u8,
            _flags: 0,
            _res0: 0,
            _res1: 0,
            _data: slice_to_array::<CANFD_DATA_LEN_MAX>(frame.data()),
        }
    }
}

impl From<CanNormalFrame> for CanAnyFrame {
    fn from(frame: CanNormalFrame) -> Self {
        CanAnyFrame::Normal(frame)
    }
}

impl From<CanFdFrame> for CanAnyFrame {
    fn from(frame: CanFdFrame) -> Self {
        CanAnyFrame::Fd(frame)
    }
}

pub trait CanFrame {
    /// Data fields accessors
    fn _id(&self) -> u32;

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    /// for normal frames and <= 64 bytes for FD frames
    fn data(&self) -> &[u8];

    /// Return the actual CAN ID (without EFF/RTR/ERR flags)
    #[inline(always)]
    fn id(&self) -> u32 {
        if self.is_extended() {
            self._id() & EFF_MASK
        } else {
            self._id() & SFF_MASK
        }
    }

    /// Return the error message
    #[inline(always)]
    fn err(&self) -> u32 {
        self._id() & ERR_MASK
    }

    /// Check if frame uses 29 bit extended frame format
    #[inline(always)]
    fn is_extended(&self) -> bool {
        self._id() & EFF_FLAG != 0
    }

    /// Check if frame is an error message
    #[inline(always)]
    fn is_error(&self) -> bool {
        self._id() & ERR_FLAG != 0
    }

    /// Check if frame is a remote transmission request
    #[inline(always)]
    fn is_rtr(&self) -> bool {
        self._id() & RTR_FLAG != 0
    }

    #[inline(always)]
    fn error(&self) -> Result<CanError, CanErrorDecodingFailure> where Self: Sized {
        CanError::from_frame(self)
    }
}

impl CanFrame for CanNormalFrame {
    fn _id(&self) -> u32 {
        self._id
    }

    #[inline(always)]
    fn data(&self) -> &[u8] {
        &self._data[..(self._data_len as usize)]
    }
}

impl CanFrame for CanFdFrame {
    fn _id(&self) -> u32 {
        self._id
    }

    #[inline(always)]
    fn data(&self) -> &[u8] {
        &self._data[..(self._data_len as usize)]
    }
}

fn can_frame_init_id(id: u32, rtr: bool, err: bool) -> Result<u32, ConstructionError> {
    let mut _id = id;

    if id > EFF_MASK {
        return Err(ConstructionError::IDTooLarge);
    }

    // set EFF_FLAG on large message
    if id > SFF_MASK {
        _id |= EFF_FLAG;
    }

    if rtr {
        _id |= RTR_FLAG;
    }

    if err {
        _id |= ERR_FLAG;
    }

    Ok(_id)
}

fn slice_to_array<const S: usize>(data: &[u8]) -> [u8;S] {
    let mut array = [0;S];

    for (i, b) in data.iter().enumerate() {
        array[i] = *b;
    }
    array
}

impl CanNormalFrame {

    /// constructor for a normal CAN frame
    pub fn new(id: u32, data: &[u8], rtr: bool, err: bool) -> Result<CanNormalFrame, ConstructionError> {
        let _id = can_frame_init_id(id, rtr, err)?;

        if data.len() > CAN_DATA_LEN_MAX {
            return Err(ConstructionError::TooMuchData);
        }

        Ok(CanNormalFrame {
            _id: _id,
            _data_len: data.len() as u8,
            _pad: 0,
            _res0: 0,
            _res1: 0,
            _data: slice_to_array::<CAN_DATA_LEN_MAX>(data),
        })
    }

}

impl CanFdFrame {
    /// constructor for a new CAN FD frame
    pub fn new(id: u32, data: &[u8], rtr: bool, err: bool, brs: bool, esi: bool) -> Result<CanFdFrame, ConstructionError> {
        let _id = can_frame_init_id(id, rtr, err)?;

        if data.len() > CANFD_DATA_LEN_MAX {
            return Err(ConstructionError::TooMuchData);
        }

        let mut flags: u8 = 0;
        if brs {
            flags = flags | CANFD_BRS;
        }
        if esi {
            flags = flags | CANFD_ESI;
        }

        Ok(CanFdFrame {
            _id: _id,
            _data_len: data.len() as u8,
            _flags: flags,
            _res0: 0,
            _res1: 0,
            _data: slice_to_array::<CANFD_DATA_LEN_MAX>(data),
        })
    }

    pub fn is_brs(&self) -> bool {
        self._flags & CANFD_BRS == CANFD_BRS
    }
    pub fn is_esi(&self) -> bool {
        self._flags & CANFD_ESI == CANFD_ESI
    }
}

impl fmt::UpperHex for CanNormalFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self.id(), "#")?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}
impl fmt::UpperHex for CanFdFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self.id(), "##")?;
        write!(f, "{} ", self._flags)?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}

impl fmt::UpperHex for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal(frame) => frame.fmt(f),
            Self::Fd(frame) => frame.fmt(f),
        }
    }
}

/// CANFilter
///
/// Uses the same memory layout as the underlying kernel struct for performance
/// reasons.
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct CanFilter {
    _id: u32,
    _mask: u32,
}

impl CanFilter {
    pub fn new(id: u32, mask: u32) -> Result<CanFilter, ConstructionError> {
        Ok(CanFilter {
               _id: id,
               _mask: mask,
           })
    }
}
