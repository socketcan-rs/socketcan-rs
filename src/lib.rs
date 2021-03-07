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
pub use crate::err::{CANError, CANErrorDecodingFailure};
pub mod dump;

#[cfg(test)]
mod tests;

use libc::{c_int, c_short, c_void, c_uint, socket, SOCK_RAW, close, bind, sockaddr, read, write,
           setsockopt, SOL_SOCKET, SO_RCVTIMEO, timeval, EINPROGRESS, SO_SNDTIMEO, time_t,
           suseconds_t, fcntl, F_GETFL, F_SETFL, O_NONBLOCK};
use itertools::Itertools;
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
#[cfg(feature = "can_fd")]
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

/// CAN FD frame
#[cfg(feature = "can_fd")]
const CANFD_MTU: usize = 72;

#[cfg(feature = "can_fd")]
const CAN_FRAME_DATA_LEN_MAX: usize = 64;
#[cfg(not(feature = "can_fd"))]
const CAN_FRAME_DATA_LEN_MAX: usize = 8;


fn c_timeval_new(t: time::Duration) -> timeval {
    timeval {
        tv_sec: t.as_secs() as time_t,
        tv_usec: (t.subsec_nanos() / 1000) as suseconds_t,
    }
}

#[derive(Debug)]
#[repr(C)]
struct CANAddr {
    _af_can: c_short,
    if_index: c_int, // address familiy,
    rx_id: u32,
    tx_id: u32,
}

#[derive(Debug)]
/// Errors opening socket
pub enum CANSocketOpenError {
    /// Device could not be found
    LookupError(nix::Error),

    /// System error while trying to look up device name
    IOError(io::Error),
}

impl fmt::Display for CANSocketOpenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CANSocketOpenError::LookupError(ref e) => write!(f, "CAN Device not found: {}", e),
            CANSocketOpenError::IOError(ref e) => write!(f, "IO: {}", e),
        }
    }
}

#[allow(deprecated)]
impl error::Error for CANSocketOpenError {
    fn description(&self) -> &str {
        match *self {
            CANSocketOpenError::LookupError(_) => "can device not found",
            CANSocketOpenError::IOError(ref e) => e.description(),
        }
    }

    fn cause(&self) -> Option<&dyn error::Error> {
        match *self {
            CANSocketOpenError::LookupError(ref e) => Some(e),
            CANSocketOpenError::IOError(ref e) => Some(e),
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

impl From<nix::Error> for CANSocketOpenError {
    fn from(e: nix::Error) -> CANSocketOpenError {
        CANSocketOpenError::LookupError(e)
    }
}

impl From<io::Error> for CANSocketOpenError {
    fn from(e: io::Error) -> CANSocketOpenError {
        CANSocketOpenError::IOError(e)
    }
}

/// A socket for a CAN device.
///
/// Will be closed upon deallocation. To close manually, use std::drop::Drop.
/// Internally this is just a wrapped file-descriptor.
#[derive(Debug)]
pub struct CANSocket {
    fd: c_int,
}

impl CANSocket {
    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "vcan0" or "socan0".
    pub fn open(ifname: &str) -> Result<CANSocket, CANSocketOpenError> {
        let if_index = if_nametoindex(ifname)?;
        CANSocket::open_if(if_index)
    }

    /// Open CAN device by interface number.
    ///
    /// Opens a CAN device by kernel interface number.
    pub fn open_if(if_index: c_uint) -> Result<CANSocket, CANSocketOpenError> {
        let addr = CANAddr {
            _af_can: AF_CAN as c_short,
            if_index: if_index as c_int,
            rx_id: 0, // ?
            tx_id: 0, // ?
        };

        // open socket
        let sock_fd;
        unsafe {
            sock_fd = socket(PF_CAN, SOCK_RAW, CAN_RAW);
        }

        if sock_fd == -1 {
            return Err(CANSocketOpenError::from(io::Error::last_os_error()));
        }

        // bind it
        let bind_rv;
        unsafe {
            let sockaddr_ptr = &addr as *const CANAddr;
            bind_rv = bind(sock_fd,
                           sockaddr_ptr as *const sockaddr,
                           size_of::<CANAddr>() as u32);
        }

        if bind_rv == -1 {
            let e = io::Error::last_os_error();
            unsafe {
                close(sock_fd);
            }
            return Err(CANSocketOpenError::from(e));
        }

        Ok(CANSocket { fd: sock_fd })
    }

    fn close(&mut self) -> io::Result<()> {
        unsafe {
            let rv = close(self.fd);
            if rv == -1 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Change socket to non-blocking mode
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        // retrieve current flags
        let oldfl = unsafe { fcntl(self.fd, F_GETFL) };

        if oldfl == -1 {
            return Err(io::Error::last_os_error());
        }

        let newfl = if nonblocking {
            oldfl | O_NONBLOCK
        } else {
            oldfl & !O_NONBLOCK
        };

        let rv = unsafe { fcntl(self.fd, F_SETFL, newfl) };

        if rv != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Control Flexible Data Rate
    #[cfg(feature = "can_fd")]
    pub fn set_fd_frames(&self, fd_frames_enable: bool) -> io::Result<()> {
        let fd_frames_enable = fd_frames_enable as c_int;
        let opt_ptr = &fd_frames_enable as *const c_int;
        let rv = unsafe {
            setsockopt(self.fd, SOL_CAN_RAW, CAN_RAW_FD_FRAMES, opt_ptr as *const c_void, size_of::<c_int>() as u32)
        };

        if rv == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Sets the read timeout on the socket
    ///
    /// For convenience, the result value can be checked using
    /// `ShouldRetry::should_retry` when a timeout is set.
    pub fn set_read_timeout(&self, duration: time::Duration) -> io::Result<()> {
        let rv = unsafe {
            let tv = c_timeval_new(duration);
            let tv_ptr: *const timeval = &tv as *const timeval;
            setsockopt(self.fd,
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
    pub fn set_write_timeout(&self, duration: time::Duration) -> io::Result<()> {
        let rv = unsafe {
            let tv = c_timeval_new(duration);
            let tv_ptr: *const timeval = &tv as *const timeval;
            setsockopt(self.fd,
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

    /// Blocking read a single can frame.
    pub fn read_frame(&self) -> io::Result<CANFrame> {
        let mut frame = CANFrame {
            tag: CANFrameType::Normal,

            /// NOTE: Can't use `std::mem::MaybeUninit::uninit().assume_init()`, which
            /// would be very appropriate here, because that would result in undefined
            /// behavior, according to `std::mem::MaybeUninit` docs (1.36.0):
            ///
            /// > [...] uninitialized memory is special in that the compiler knows that it
            /// > does not have a fixed value. This makes it undefined behavior to have
            /// > uninitialized data in a variable even if that variable has an integer type,
            /// > which otherwise can hold any fixed bit pattern [...]
            ///
            s: unsafe { std::mem::MaybeUninit::zeroed().assume_init() }
        };

        let read_rv = unsafe {
            let frame_ptr = &mut frame.s as *mut CANFrameStruct;
            read(self.fd, frame_ptr as *mut c_void, size_of::<CANFrameStruct>())
        };

        frame.tag = match read_rv as usize {
            CAN_MTU => CANFrameType::Normal,

            #[cfg(feature = "can_fd")]
            CANFD_MTU => CANFrameType::Fd,

            _ => return Err(io::Error::last_os_error()),
        };

        Ok(frame)
    }

    /// Write a single can frame.
    ///
    /// Note that this function can fail with an `EAGAIN` error or similar.
    /// Use `write_frame_insist` if you need to be sure that the message got
    /// sent or failed.
    pub fn write_frame(&self, frame: &CANFrame) -> io::Result<()> {
        // not a mutable reference needed (see std::net::UdpSocket) for
        // a comparison
        // debug!("Sending: {:?}", frame);

        let frame_size = match frame.tag {
            CANFrameType::Normal => CAN_MTU,

            #[cfg(feature = "can_fd")]
            CANFrameType::Fd => CANFD_MTU
        };

        let write_rv = unsafe {
            let frame_ptr = &frame.s as *const CANFrameStruct;
            write(self.fd, frame_ptr as *const c_void, frame_size)
        };

        if write_rv as usize != frame_size {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    /// Blocking write a single can frame, retrying until it gets sent
    /// successfully.
    pub fn write_frame_insist(&self, frame: &CANFrame) -> io::Result<()> {
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

    /// Sets the filter mask on the socket.
    pub fn set_filter(&self, filters: &[CANFilter]) -> io::Result<()> {

        // TODO: Handle different *_FILTER sockopts.

        let rv = if filters.len() < 1 {
            // clears all filters
            unsafe { setsockopt(self.fd, SOL_CAN_RAW, CAN_RAW_FILTER, 0 as *const c_void, 0) }
        } else {
            unsafe {
                let filters_ptr = &filters[0] as *const CANFilter;
                setsockopt(self.fd,
                           SOL_CAN_RAW,
                           CAN_RAW_FILTER,
                           filters_ptr as *const c_void,
                           (size_of::<CANFilter>() * filters.len()) as u32)
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
    pub fn filter_drop_all(&self) -> io::Result<()> {
        self.set_filter(&[])
    }

    /// Accept all frames, disabling any kind of filtering.
    ///
    /// Replace the current filter with one containing a single rule that
    /// acceps all CAN frames.
    pub fn filter_accept_all(&self) -> io::Result<()> {
        // safe unwrap: 0, 0 is a valid mask/id pair
        self.set_filter(&[CANFilter::new(0, 0).unwrap()])
    }

    #[inline(always)]
    pub fn set_error_filter(&self, mask: u32) -> io::Result<()> {
        let rv = unsafe {
            setsockopt(self.fd,
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
    pub fn error_filter_drop_all(&self) -> io::Result<()> {
        self.set_error_filter(0)
    }

    #[inline(always)]
    pub fn error_filter_accept_all(&self) -> io::Result<()> {
        self.set_error_filter(ERR_MASK)
    }
}

impl AsRawFd for CANSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl FromRawFd for CANSocket {
    unsafe fn from_raw_fd(fd: RawFd) -> CANSocket {
        CANSocket { fd: fd }
    }
}

impl IntoRawFd for CANSocket {
    fn into_raw_fd(self) -> RawFd {
        self.fd
    }
}

impl Drop for CANSocket {
    fn drop(&mut self) {
        self.close().ok(); // ignore result
    }
}

/// CANFrameStruct
///
/// Uses the same memory layout as the underlying kernel struct for performance
/// reasons.
#[derive(Debug, Copy, Clone)]
#[repr(C)]
struct CANFrameStruct {
    /// 32 bit CAN_ID + EFF/RTR/ERR flags
    _id: u32,

    /// frame payload length in bytes (0 .. 8) for classical frame,
    /// (0 .. 64) for FD frame
    _data_len: u8,

    /// padding for classical frame, additional
    /// flags for CAN FD
    _pad_flags: u8,

    /// reserved
    _res0: u8,

    /// reserved
    _res1: u8,

    /// buffer for data (classical or FD frame)
    _data: [u8; CAN_FRAME_DATA_LEN_MAX]
}

enum CANFrameType {
    Normal,

    #[cfg(feature = "can_fd")]
    Fd
}

pub struct CANFrame {
    tag: CANFrameType,
    s: CANFrameStruct,
}

impl fmt::Debug for CANFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.tag {
            CANFrameType::Normal => {
                write!(f, "CAN Classical Frame {:?}", self.s )
            }

            #[cfg(feature = "can_fd")]
            CANFrameType::Fd => {
                write!(f, "CAN FD Frame {:?}", self.s )
            }
        }
    }
}

impl CANFrame {
    /// constructor for a classical CAN frame
    pub fn new(id: u32, data: &[u8], rtr: bool, err: bool) -> Result<CANFrame, ConstructionError> {
        CANFrame::new_common(CANFrameType::Normal, id, data, rtr, err)
    }

    #[cfg(feature = "can_fd")]
    /// constructor for a new CAN FD frame
    pub fn new_fd(id: u32, data: &[u8], rtr: bool, err: bool) -> Result<CANFrame, ConstructionError> {
        CANFrame::new_common(CANFrameType::Fd, id, data, rtr, err)
    }

    fn new_common(frame_type: CANFrameType, id: u32, data: &[u8], rtr: bool, err: bool) -> Result<CANFrame, ConstructionError> {
        let mut _id = id;

        let max_valid_data_len = match frame_type {
            CANFrameType::Normal => 8,

            #[cfg(feature = "can_fd")]
            CANFrameType::Fd => 64
        };

        if data.len() > max_valid_data_len {
            return Err(ConstructionError::TooMuchData);
        }

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

        let mut full_data = [0; CAN_FRAME_DATA_LEN_MAX];

        // not cool =/
        for (n, c) in data.iter().enumerate() {
            full_data[n] = *c;
        }

        Ok(CANFrame {
            tag: frame_type,
            s: CANFrameStruct {
                _id: _id,
                _data_len: data.len() as u8,
                _pad_flags: 0,
                _res0: 0,
                _res1: 0,
                _data: full_data,
            }
        })
    }

    #[cfg(feature = "can_fd")]
    pub fn is_fd(&self) -> bool {
        if let CANFrameType::Fd = self.tag { true } else { false }
    }

    /// Return the actual CAN ID (without EFF/RTR/ERR flags)
    #[inline(always)]
    pub fn id(&self) -> u32 {
        if self.is_extended() {
            self.s._id & EFF_MASK
        } else {
            self.s._id & SFF_MASK
        }
    }

    /// Return the error message
    #[inline(always)]
    pub fn err(&self) -> u32 {
        self.s._id & ERR_MASK
    }

    /// Check if frame uses 29 bit extended frame format
    #[inline(always)]
    pub fn is_extended(&self) -> bool {
        self.s._id & EFF_FLAG != 0
    }

    /// Check if frame is an error message
    #[inline(always)]
    pub fn is_error(&self) -> bool {
        self.s._id & ERR_FLAG != 0
    }

    /// Check if frame is a remote transmission request
    #[inline(always)]
    pub fn is_rtr(&self) -> bool {
        self.s._id & RTR_FLAG != 0
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    /// for normal frames and <= 64 bytes for FD frames
    #[inline(always)]
    pub fn data(&self) -> &[u8] {
        &self.s._data[..(self.s._data_len as usize)]
    }

    #[inline(always)]
    pub fn error(&self) -> Result<CANError, CANErrorDecodingFailure> {
        CANError::from_frame(self)
    }
}

impl fmt::UpperHex for CANFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self.id(),
               match self.tag {
                   CANFrameType::Normal => "#",

                   #[cfg(feature = "can_fd")]
                   CANFrameType::Fd => "##"
               })?;

        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));

        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}

/// CANFilter
///
/// Uses the same memory layout as the underlying kernel struct for performance
/// reasons.
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct CANFilter {
    _id: u32,
    _mask: u32,
}

impl CANFilter {
    pub fn new(id: u32, mask: u32) -> Result<CANFilter, ConstructionError> {
        Ok(CANFilter {
               _id: id,
               _mask: mask,
           })
    }
}
