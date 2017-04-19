//! SocketCAN support.
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



// clippy: do not warn about things like "SocketCAN" inside the docs
#![cfg_attr(feature = "cargo-clippy", allow(doc_markdown))]

extern crate byte_conv;
extern crate futures;
extern crate hex;
extern crate itertools;
extern crate libc;
extern crate mio;
extern crate netlink_rs;
extern crate nix;
extern crate tokio_core;
extern crate try_from;

mod err;
pub use err::{CanError, CanErrorDecodingFailure};
pub mod dump;
mod nl;
mod util;

#[cfg(test)]
mod tests;

use libc::{c_int, c_short, c_void, c_uint, c_ulong, socket, SOCK_RAW, close, bind, connect,
           sockaddr, read, write, SOL_SOCKET, SO_RCVTIMEO, timespec, timeval, EINPROGRESS,
           SO_SNDTIMEO, time_t, suseconds_t, fcntl, F_GETFL, F_SETFL, O_NONBLOCK};
use itertools::Itertools;
use mio::{Evented, Ready, Poll, PollOpt, Token};
use mio::unix::EventedFd;
use nix::net::if_::if_nametoindex;
pub use nl::CanInterface;
use std::{error, fmt, io, slice, time};
use std::io::{Error, ErrorKind};
use std::mem::{size_of, uninitialized};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use tokio_core::reactor::{Handle, PollEvented};
use util::{set_socket_option, set_socket_option_mult};

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
        if let Err(ref e) = *self {
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
const CAN_BCM: c_int = 2;
const SOL_CAN_BASE: c_int = 100;
const SOL_CAN_RAW: c_int = SOL_CAN_BASE + CAN_RAW;
const CAN_RAW_FILTER: c_int = 1;
const CAN_RAW_ERR_FILTER: c_int = 2;
const CAN_RAW_LOOPBACK: c_int = 3;
const CAN_RAW_RECV_OWN_MSGS: c_int = 4;
// unused:
// const CAN_RAW_FD_FRAMES: c_int = 5;
const CAN_RAW_JOIN_FILTERS: c_int = 6;

/// datagram (conn.less) socket
const SOCK_DGRAM: c_int = 2;

// get timestamp in a struct timeval (us accuracy)
// const SIOCGSTAMP: c_int = 0x8906;

// get timestamp in a struct timespec (ns accuracy)
const SIOCGSTAMPNS: c_int = 0x8907;

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


/// an error mask that will cause SocketCAN to report all errors
pub const ERR_MASK_ALL: u32 = ERR_MASK;

/// an error mask that will cause SocketCAN to silently drop all errors
pub const ERR_MASK_NONE: u32 = 0;

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

impl error::Error for CanSocketOpenError {
    fn description(&self) -> &str {
        match *self {
            CanSocketOpenError::LookupError(_) => "can device not found",
            CanSocketOpenError::IOError(ref e) => e.description(),
        }
    }

    fn cause(&self) -> Option<&error::Error> {
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
pub struct CanSocket {
    fd: c_int,
}

impl CanSocket {
    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "vcan0" or "socan0".
    pub fn open(ifname: &str) -> Result<CanSocket, CanSocketOpenError> {
        let if_index = if_nametoindex(ifname)?;
        CanSocket::open_if(if_index)
    }

    /// Open CAN device by interface number.
    ///
    /// Opens a CAN device by kernel interface number.
    pub fn open_if(if_index: c_uint) -> Result<CanSocket, CanSocketOpenError> {
        let addr = CanAddr {
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
            return Err(CanSocketOpenError::from(io::Error::last_os_error()));
        }

        // bind it
        let bind_rv;
        unsafe {
            let sockaddr_ptr = &addr as *const CanAddr;
            bind_rv = bind(
                sock_fd,
                sockaddr_ptr as *const sockaddr,
                size_of::<CanAddr>() as u32,
            );
        }

        // FIXME: on fail, close socket (do not leak socketfds)
        if bind_rv == -1 {
            let e = io::Error::last_os_error();
            unsafe {
                close(sock_fd);
            }
            return Err(CanSocketOpenError::from(e));
        }

        Ok(CanSocket { fd: sock_fd })
    }

    fn close(&mut self) -> io::Result<()> {
        unsafe {
            let rv = close(self.fd);
            if rv != -1 {
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

    /// Sets the read timeout on the socket
    ///
    /// For convenience, the result value can be checked using
    /// `ShouldRetry::should_retry` when a timeout is set.
    pub fn set_read_timeout(&self, duration: time::Duration) -> io::Result<()> {
        set_socket_option(self.fd, SOL_SOCKET, SO_RCVTIMEO, &c_timeval_new(duration))
    }

    /// Sets the write timeout on the socket
    pub fn set_write_timeout(&self, duration: time::Duration) -> io::Result<()> {
        set_socket_option(self.fd, SOL_SOCKET, SO_SNDTIMEO, &c_timeval_new(duration))
    }

    /// Blocking read a single can frame.
    pub fn read_frame(&self) -> io::Result<CanFrame> {
        let mut frame = CanFrame {
            _id: 0,
            _data_len: 0,
            _pad: 0,
            _res0: 0,
            _res1: 0,
            _data: [0; 8],
        };

        let read_rv = unsafe {
            let frame_ptr = &mut frame as *mut CanFrame;
            read(self.fd, frame_ptr as *mut c_void, size_of::<CanFrame>())
        };

        if read_rv as usize != size_of::<CanFrame>() {
            return Err(io::Error::last_os_error());
        }

        Ok(frame)
    }

    /// Blocking read a single can frame with timestamp
    ///
    /// Note that reading a frame and retrieving the timestamp requires two
    /// consecutive syscalls. To avoid race conditions, exclusive access
    /// to the socket is enforce through requiring a `mut &self`.
    pub fn read_frame_with_timestamp(&mut self) -> io::Result<(CanFrame, time::SystemTime)> {
        let frame = self.read_frame()?;

        let mut ts: timespec;
        let rval = unsafe {
            // we initialize tv calling ioctl, passing this responsibility on
            ts = uninitialized();
            libc::ioctl(self.fd, SIOCGSTAMPNS as c_ulong, &mut ts as *mut timespec)
        };

        if rval == -1 {
            return Err(io::Error::last_os_error());
        }

        Ok((frame, util::system_time_from_timespec(ts)))
    }

    /// Write a single can frame.
    ///
    /// Note that this function can fail with an `EAGAIN` error or similar.
    /// Use `write_frame_insist` if you need to be sure that the message got
    /// sent or failed.
    pub fn write_frame(&self, frame: &CanFrame) -> io::Result<()> {
        // not a mutable reference needed (see std::net::UdpSocket) for
        // a comparison
        // debug!("Sending: {:?}", frame);

        let write_rv = unsafe {
            let frame_ptr = frame as *const CanFrame;
            write(self.fd, frame_ptr as *const c_void, size_of::<CanFrame>())
        };

        if write_rv as usize != size_of::<CanFrame>() {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    /// Blocking write a single can frame, retrying until it gets sent
    /// successfully.
    pub fn write_frame_insist(&self, frame: &CanFrame) -> io::Result<()> {
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

    /// Sets filters on the socket.
    ///
    /// CAN packages received by SocketCAN are matched against these filters,
    /// only matching packets are returned by the interface.
    ///
    /// See `CanFilter` for details on how filtering works. By default, all
    /// single filter matching all incoming frames is installed.
    pub fn set_filters(&self, filters: &[CanFilter]) -> io::Result<()> {
        set_socket_option_mult(self.fd, SOL_CAN_RAW, CAN_RAW_FILTER, filters)
    }

    /// Sets the error mask on the socket.
    ///
    /// By default (`ERR_MASK_NONE`) no error conditions are reported as
    /// special error frames by the socket. Enabling error conditions by
    /// setting `ERR_MASK_ALL` or another non-empty error mask causes the
    /// socket to receive notification about the specified conditions.
    #[inline]
    pub fn set_error_mask(&self, mask: u32) -> io::Result<()> {
        set_socket_option(self.fd, SOL_CAN_RAW, CAN_RAW_ERR_FILTER, &mask)
    }

    /// Enable or disable loopback.
    ///
    /// By default, loopback is enabled, causing other applications that open
    /// the same CAN bus to see frames emitted by different applications on
    /// the same system.
    #[inline]
    pub fn set_loopback(&self, enabled: bool) -> io::Result<()> {
        let loopback: c_int = if enabled { 1 } else { 0 };
        set_socket_option(self.fd, SOL_CAN_RAW, CAN_RAW_LOOPBACK, &loopback)
    }

    /// Enable or disable receiving of own frames.
    ///
    /// When loopback is enabled, this settings controls if CAN frames sent
    /// are received back immediately by sender. Default is off.
    pub fn set_recv_own_msgs(&self, enabled: bool) -> io::Result<()> {
        let recv_own_msgs: c_int = if enabled { 1 } else { 0 };
        set_socket_option(self.fd, SOL_CAN_RAW, CAN_RAW_RECV_OWN_MSGS, &recv_own_msgs)
    }

    /// Enable or disable join filters.
    ///
    /// By default a frame is accepted if it matches any of the filters set
    /// with `set_filters`. If join filters is enabled, a frame has to match
    /// _all_ filters to be accepted.
    pub fn set_join_filters(&self, enabled: bool) -> io::Result<()> {
        let join_filters: c_int = if enabled { 1 } else { 0 };
        set_socket_option(self.fd, SOL_CAN_RAW, CAN_RAW_JOIN_FILTERS, &join_filters)
    }
}

impl AsRawFd for CanSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl FromRawFd for CanSocket {
    unsafe fn from_raw_fd(fd: RawFd) -> CanSocket {
        CanSocket { fd: fd }
    }
}

impl IntoRawFd for CanSocket {
    fn into_raw_fd(self) -> RawFd {
        self.fd
    }
}

impl Drop for CanSocket {
    fn drop(&mut self) {
        self.close().ok(); // ignore result
    }
}

/// CanFrame
///
/// Uses the same memory layout as the underlying kernel struct for performance
/// reasons.
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct CanFrame {
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
    _data: [u8; 8],
}

impl CanFrame {
    pub fn new(id: u32, data: &[u8], rtr: bool, err: bool) -> Result<CanFrame, ConstructionError> {
        let mut _id = id;

        if data.len() > 8 {
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

        let mut full_data = [0; 8];

        // not cool =/
        for (n, c) in data.iter().enumerate() {
            full_data[n] = *c;
        }

        Ok(CanFrame {
            _id: _id,
            _data_len: data.len() as u8,
            _pad: 0,
            _res0: 0,
            _res1: 0,
            _data: full_data,
        })
    }

    /// Return the actual CAN ID (without EFF/RTR/ERR flags)
    #[inline]
    pub fn id(&self) -> u32 {
        if self.is_extended() {
            self._id & EFF_MASK
        } else {
            self._id & SFF_MASK
        }
    }

    /// Return the error message
    #[inline]
    pub fn err(&self) -> u32 {
        self._id & ERR_MASK
    }

    /// Check if frame uses 29 bit extended frame format
    #[inline]
    pub fn is_extended(&self) -> bool {
        self._id & EFF_FLAG != 0
    }

    /// Check if frame is an error message
    #[inline]
    pub fn is_error(&self) -> bool {
        self._id & ERR_FLAG != 0
    }

    /// Check if frame is a remote transmission request
    #[inline]
    pub fn is_rtr(&self) -> bool {
        self._id & RTR_FLAG != 0
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    #[inline]
    pub fn data(&self) -> &[u8] {
        &self._data[..(self._data_len as usize)]
    }

    /// Read error from message and transform it into a `CanError`.
    ///
    /// SocketCAN errors are indicated using the error bit and coded inside
    /// id and data payload. Call `error()` converts these into usable
    /// `CanError` instances.
    ///
    /// If the frame is malformed, this may fail with a
    /// `CanErrorDecodingFailure`.
    #[inline]
    pub fn error(&self) -> Result<CanError, CanErrorDecodingFailure> {
        CanError::from_frame(self)
    }
}

impl fmt::UpperHex for CanFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}#", self.id())?;

        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));

        let sep = if f.alternate() { " " } else { "" };
        write!(f, "{}", parts.join(sep))
    }
}

/// CanFilter
///
/// Contains an internal id and mask. Packets are considered to be matched by
/// a filter if `received_id & mask == filter_id & mask` holds true.
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct CanFilter {
    _id: u32,
    _mask: u32,
}

impl CanFilter {
    /// Construct a new CAN filter.
    pub fn new(id: u32, mask: u32) -> Result<CanFilter, ConstructionError> {
        Ok(CanFilter {
            _id: id,
            _mask: mask,
        })
    }
}

pub const MAX_NFRAMES: u32 = 256;

/// OpCodes
///
/// create (cyclic) transmission task
pub const TX_SETUP: u32 = 1;
/// remove (cyclic) transmission task
pub const TX_DELETE: u32 = 2;
/// read properties of (cyclic) transmission task
pub const TX_READ: u32 = 3;
/// send one CAN frame
pub const TX_SEND: u32 = 4;
/// create RX content filter subscription
pub const RX_SETUP: u32 = 5;
/// remove RX content filter subscription
pub const RX_DELETE: u32 = 6;
/// read properties of RX content filter subscription
pub const RX_READ: u32 = 7;
/// reply to TX_READ request
pub const TX_STATUS: u32 = 8;
/// notification on performed transmissions (count=0)
pub const TX_EXPIRED: u32 = 9;
/// reply to RX_READ request
pub const RX_STATUS: u32 = 10;
/// cyclic message is absent
pub const RX_TIMEOUT: u32 = 11;
/// sent if the first or a revised CAN message was received
pub const RX_CHANGED: u32 = 12;

/// Flags
///
/// set the value of ival1, ival2 and count
pub const SETTIMER: u32 = 0x0001;
/// start the timer with the actual value of ival1, ival2 and count.
/// Starting the timer leads simultaneously to emit a can_frame.
pub const STARTTIMER: u32 = 0x0002;
/// create the message TX_EXPIRED when count expires
pub const TX_COUNTEVT: u32 = 0x0004;
/// A change of data by the process is emitted immediatly.
/// (Requirement of 'Changing Now' - BAES)
pub const TX_ANNOUNCE: u32 = 0x0008;
/// Copies the can_id from the message header to each subsequent frame
/// in frames. This is intended only as usage simplification.
pub const TX_CP_CAN_ID: u32 = 0x0010;
/// Filter by can_id alone, no frames required (nframes=0)
pub const RX_FILTER_ID: u32 = 0x0020;
/// A change of the DLC leads to an RX_CHANGED.
pub const RX_CHECK_DLC: u32 = 0x0040;
/// If the timer ival1 in the RX_SETUP has been set equal to zero, on receipt
/// of the CAN message the timer for the timeout monitoring is automatically
/// started. Setting this flag prevents the automatic start timer.
pub const RX_NO_AUTOTIMER: u32 = 0x0080;
/// refers also to the time-out supervision of the management RX_SETUP.
/// By setting this flag, when an RX-outs occours, a RX_CHANGED will be
/// generated when the (cyclic) receive restarts. This will happen even if the
/// user data have not changed.
pub const RX_ANNOUNCE_RESUM: u32 = 0x0100;
/// forces a reset of the index counter from the update to be sent by multiplex
/// message even if it would not be necessary because of the length.
pub const TX_RESET_MULTI_ID: u32 = 0x0200;
/// the filter passed is used as CAN message to be sent when receiving an RTR frame.
pub const RX_RTR_FRAME: u32 = 0x0400;
pub const CAN_FD_FRAME: u32 = 0x0800;

/// BcmMsgHead
///
/// Head of messages to and from the broadcast manager
#[repr(C)]
pub struct BcmMsgHead {
    _opcode: u32,
    _flags: u32,
    /// number of frames to send before changing interval
    _count: u32,
    /// interval for the first count frames
    _ival1: timeval,
    /// interval for the following frames
    _ival2: timeval,
    _can_id: u32,
    /// number of can frames appended to the message head
    _nframes: u32,
    // TODO figure out how why C adds a padding here?
    #[cfg(all(target_pointer_width = "32"))]
    _pad: u32,
    // TODO figure out how to allocate only nframes instead of MAX_NFRAMES
    /// buffer of CAN frames
    _frames: [CanFrame; MAX_NFRAMES as usize],
}

/// BcmMsgHeadFrameLess
///
/// Head of messages to and from the broadcast manager see _pad fields for differences
/// to BcmMsgHead
#[repr(C)]
pub struct BcmMsgHeadFrameLess {
    _opcode: u32,
    _flags: u32,
    /// number of frames to send before changing interval
    _count: u32,
    /// interval for the first count frames
    _ival1: timeval,
    /// interval for the following frames
    _ival2: timeval,
    _can_id: u32,
    /// number of can frames appended to the message head
    _nframes: u32,
    // Workaround Rust ZST has a size of 0 for frames, in
    // C the BcmMsgHead struct contains an Array that although it has
    // a length of zero still takes n (4) bytes.
    #[cfg(all(target_pointer_width = "32"))]
    _pad: usize,
}

#[repr(C)]
pub struct TxMsg {
    _msg_head: BcmMsgHeadFrameLess,
    _frames: [CanFrame; MAX_NFRAMES as usize],
}

impl BcmMsgHead {
    pub fn can_id(&self) -> u32 {
        self._can_id
    }

    #[inline]
    pub fn frames(&self) -> &[CanFrame] {
        return unsafe { slice::from_raw_parts(self._frames.as_ptr(), self._nframes as usize) };
    }
}

/// A socket for a CAN device, specifically for broadcast manager operations.
#[derive(Debug)]
pub struct CanBCMSocket {
    pub fd: c_int,
}

impl CanBCMSocket {
    /// Open a named CAN device non blocking.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "vcan0" or "socan0".
    pub fn open_nb(ifname: &str) -> Result<CanBCMSocket, CanSocketOpenError> {
        let if_index = if_nametoindex(ifname)?;
        CanBCMSocket::open_if_nb(if_index)
    }

    /// Open CAN device by interface number non blocking.
    ///
    /// Opens a CAN device by kernel interface number.
    pub fn open_if_nb(if_index: c_uint) -> Result<CanBCMSocket, CanSocketOpenError> {

        // open socket
        let sock_fd;
        unsafe {
            sock_fd = socket(PF_CAN, SOCK_DGRAM, CAN_BCM);
        }

        if sock_fd == -1 {
            return Err(CanSocketOpenError::from(io::Error::last_os_error()));
        }

        let fcntl_resp = unsafe { fcntl(sock_fd, F_SETFL, O_NONBLOCK) };

        if fcntl_resp == -1 {
            return Err(CanSocketOpenError::from(io::Error::last_os_error()));
        }

        let addr = CanAddr {
            _af_can: AF_CAN as c_short,
            if_index: if_index as c_int,
            rx_id: 0, // ?
            tx_id: 0, // ?
        };

        let sockaddr_ptr = &addr as *const CanAddr;

        let connect_res;
        unsafe {
            connect_res = connect(
                sock_fd,
                sockaddr_ptr as *const sockaddr,
                size_of::<CanAddr>() as u32,
            );
        }

        if connect_res != 0 {
            return Err(CanSocketOpenError::from(io::Error::last_os_error()));
        }

        Ok(CanBCMSocket { fd: sock_fd })
    }

    fn close(&mut self) -> io::Result<()> {
        unsafe {
            let rv = close(self.fd);
            if rv != -1 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Create a content filter subscription, filtering can frames by can_id.
    pub fn filter_id(
        &self,
        can_id: c_uint,
        ival1: time::Duration,
        ival2: time::Duration,
    ) -> io::Result<()> {
        let _ival1 = c_timeval_new(ival1);
        let _ival2 = c_timeval_new(ival2);

        let frames = [CanFrame::new(0x0, &[], false, false).unwrap(); MAX_NFRAMES as usize];
        let msg = BcmMsgHeadFrameLess {
            _opcode: RX_SETUP,
            _flags: SETTIMER | RX_FILTER_ID,
            _count: 0,
            #[cfg(all(target_pointer_width = "32"))]
            _pad: 0,
            _ival1: _ival1,
            _ival2: _ival2,
            _can_id: can_id | EFF_FLAG,
            _nframes: 0,
        };

        let tx_msg = &TxMsg {
            _msg_head: msg,
            _frames: frames,
        };

        let write_rv = unsafe {
            let tx_msg_ptr = tx_msg as *const TxMsg;
            write(self.fd, tx_msg_ptr as *const c_void, size_of::<TxMsg>())
        };

        if write_rv < 0 {
            return Err(Error::new(ErrorKind::WriteZero, io::Error::last_os_error()));
        }

        Ok(())
    }

    /// Remove a content filter subscription.
    pub fn filter_delete(&self, can_id: c_uint) -> io::Result<()> {
        let frames = [CanFrame::new(0x0, &[], false, false).unwrap(); MAX_NFRAMES as usize];

        let msg = &BcmMsgHead {
            _opcode: RX_DELETE,
            _flags: 0,
            _count: 0,
            _ival1: c_timeval_new(time::Duration::new(0, 0)),
            _ival2: c_timeval_new(time::Duration::new(0, 0)),
            _can_id: can_id,
            _nframes: 0,
            #[cfg(all(target_pointer_width = "32"))]
            _pad: 0,
            _frames: frames,
        };

        let write_rv = unsafe {
            let msg_ptr = msg as *const BcmMsgHead;
            write(self.fd, msg_ptr as *const c_void, size_of::<BcmMsgHead>())
        };

        let expected_size = size_of::<BcmMsgHead>() - size_of::<[CanFrame; MAX_NFRAMES as usize]>();
        if write_rv as usize != expected_size {
            let msg = format!("Wrote {} but expected {}", write_rv, expected_size);
            return Err(Error::new(ErrorKind::WriteZero, msg));
        }

        Ok(())
    }

    /// Read a single can frame.
    pub fn read_msg(&self) -> io::Result<BcmMsgHead> {

        let ival1 = c_timeval_new(time::Duration::from_millis(0));
        let ival2 = c_timeval_new(time::Duration::from_millis(0));
        let frames = [CanFrame::new(0x0, &[], false, false).unwrap(); MAX_NFRAMES as usize];
        let mut msg = BcmMsgHead {
            _opcode: 0,
            _flags: 0,
            _count: 0,
            _ival1: ival1,
            _ival2: ival2,
            _can_id: 0,
            _nframes: 0,
            #[cfg(all(target_pointer_width = "32"))]
            _pad: 0,
            _frames: frames,
        };

        let msg_ptr = &mut msg as *mut BcmMsgHead;
        let count = unsafe {
            read(
                self.fd.clone(),
                msg_ptr as *mut c_void,
                size_of::<BcmMsgHead>(),
            )
        };

        let last_error = io::Error::last_os_error();
        if count < 0 { Err(last_error) } else { Ok(msg) }
    }
}

impl Evented for CanBCMSocket {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.fd).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.fd).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        EventedFd(&self.fd).deregister(poll)
    }
}

impl Drop for CanBCMSocket {
    fn drop(&mut self) {
        self.close().ok(); // ignore result
    }
}

pub struct BcmListener {
    io: PollEvented<CanBCMSocket>,
}

impl BcmListener {
    pub fn from(bcm_socket: CanBCMSocket, handle: &Handle) -> io::Result<BcmListener> {
        let io = try!(PollEvented::new(bcm_socket, handle));
        Ok(BcmListener { io: io })
    }
}

impl futures::stream::Stream for BcmListener {
    type Item = BcmMsgHead;
    type Error = io::Error;
    fn poll(&mut self) -> futures::Poll<Option<Self::Item>, Self::Error> {
        if let futures::Async::NotReady = self.io.poll_read() {
            return Ok(futures::Async::NotReady);
        }

        match self.io.get_ref().read_msg() {
            Ok(n) => Ok(futures::Async::Ready(Some(n))),
            Err(e) => {
                if e.kind() == io::ErrorKind::WouldBlock {
                    self.io.need_read();
                    return Ok(futures::Async::NotReady);
                }
                return Err(e);
            }
        }
    }
}
