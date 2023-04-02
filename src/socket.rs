// socketcan/src/socket.rs
//
// Implements sockets for CANbus 2.0 and FD for SocketCAN on Linux.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Implementation of sockets for CANbus 2.0 and FD for SocketCAN on Linux.

use crate::{frame::CAN_ERR_MASK, CanAnyFrame, CanFdFrame, CanFrame, CanSocketOpenError};
use libc::{
    can_frame, canid_t, fcntl, read, sa_family_t, setsockopt, sockaddr, sockaddr_can, socklen_t,
    suseconds_t, time_t, timeval, write, EINPROGRESS, F_GETFL, F_SETFL, O_NONBLOCK, SOCK_RAW,
    SOL_SOCKET, SO_RCVTIMEO, SO_SNDTIMEO,
};
use nix::net::if_::if_nametoindex;
use std::{
    convert::TryFrom,
    fmt, io, mem,
    os::{
        raw::{c_int, c_uint, c_void},
        unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
    },
    ptr, time,
};

pub use libc::{
    AF_CAN, CANFD_MTU, CAN_MTU, CAN_RAW, CAN_RAW_ERR_FILTER, CAN_RAW_FD_FRAMES, CAN_RAW_FILTER,
    CAN_RAW_JOIN_FILTERS, CAN_RAW_LOOPBACK, CAN_RAW_RECV_OWN_MSGS, PF_CAN, SOL_CAN_BASE,
    SOL_CAN_RAW,
};

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

// ===== CanAddr =====

/// CAN socket address.
///
/// This is based on and compatible with the
/// [ref](https://docs.rs/libc/latest/libc/struct.sockaddr_can.html)
#[derive(Clone, Copy)]
struct CanAddr(sockaddr_can);

impl CanAddr {
    /// Creates a new CAN socket address for the specified interface by index.
    pub fn new(ifindex: c_uint) -> Self {
        let mut addr = Self::default();
        addr.0.can_ifindex = ifindex as c_int;
        addr
    }

    /// Gets the address of the structure as a sockaddr_can.
    pub fn as_ptr(&self) -> *const sockaddr_can {
        &self.0
    }
}

impl Default for CanAddr {
    fn default() -> Self {
        let mut addr: sockaddr_can = unsafe { mem::zeroed() };
        addr.can_family = AF_CAN as sa_family_t;
        Self(addr)
    }
}

impl fmt::Debug for CanAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "CanAddr {{ can_family: {}, can_ifindex: {} }}",
            self.0.can_family, self.0.can_ifindex
        )
    }
}

impl From<sockaddr_can> for CanAddr {
    fn from(addr: sockaddr_can) -> Self {
        Self(addr)
    }
}

impl AsRef<sockaddr_can> for CanAddr {
    fn as_ref(&self) -> &sockaddr_can {
        &self.0
    }
}

// ===== Private local helper functions =====

fn c_timeval_new(t: time::Duration) -> timeval {
    timeval {
        tv_sec: t.as_secs() as time_t,
        tv_usec: t.subsec_micros() as suseconds_t,
    }
}

/// Tries to open the CAN socket by the interface number.
fn raw_open_socket(ifindex: c_uint) -> Result<c_int, CanSocketOpenError> {
    let sock_fd = unsafe { libc::socket(PF_CAN, SOCK_RAW, CAN_RAW) };

    if sock_fd == -1 {
        return Err(CanSocketOpenError::from(io::Error::last_os_error()));
    }

    let addr = CanAddr::new(ifindex);

    let bind_rv = unsafe {
        libc::bind(
            sock_fd,
            addr.as_ptr() as *const sockaddr,
            mem::size_of::<CanAddr>() as u32,
        )
    };

    if bind_rv == -1 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(sock_fd) };
        return Err(CanSocketOpenError::from(e));
    }

    Ok(sock_fd)
}

fn set_fd_mode(socket_fd: c_int, fd_mode_enable: bool) -> io::Result<c_int> {
    let fd_mode_enable = fd_mode_enable as c_int;
    let rv = unsafe {
        setsockopt(
            socket_fd,
            SOL_CAN_RAW,
            CAN_RAW_FD_FRAMES,
            &fd_mode_enable as *const _ as *const c_void,
            mem::size_of::<c_int>() as u32,
        )
    };

    if rv == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket_fd)
}

fn raw_write_frame<T>(socket_fd: c_int, frame_ptr: *const T) -> io::Result<()> {
    let ret = unsafe { write(socket_fd, frame_ptr as *const c_void, mem::size_of::<T>()) };

    if ret as usize != mem::size_of::<T>() {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

/// `setsockopt` wrapper
///
/// The libc `setsockopt` function is set to set various options on a socket.
/// `set_socket_option` offers a somewhat type-safe wrapper that does not
/// require messing around with `*const c_void`s.
///
/// A proper `std::io::Error` will be returned on failure.
///
/// Example use:
///
/// ```text
/// let fd = ...;  // some file descriptor, this will be stdout
/// set_socket_option(fd, SOL_TCP, TCP_NO_DELAY, 1 as c_int)
/// ```
///
/// Note that the `val` parameter must be specified correctly; if an option
/// expects an integer, it is advisable to pass in a `c_int`, not the default
/// of `i32`.
#[inline]
pub fn set_socket_option<T>(fd: c_int, level: c_int, name: c_int, val: &T) -> io::Result<()> {
    let rv = unsafe {
        setsockopt(
            fd,
            level,
            name,
            val as *const _ as *const c_void,
            mem::size_of::<T>() as socklen_t,
        )
    };

    if rv != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

/// Sets a collection of multiple socke options with one call.
pub fn set_socket_option_mult<T>(
    fd: c_int,
    level: c_int,
    name: c_int,
    values: &[T],
) -> io::Result<()> {
    let rv = if values.is_empty() {
        // can't pass in a ptr to a 0-len slice, pass a null ptr instead
        unsafe { setsockopt(fd, level, name, ptr::null(), 0) }
    } else {
        unsafe {
            setsockopt(
                fd,
                level,
                name,
                values.as_ptr() as *const c_void,
                (mem::size_of::<T>() * values.len()) as socklen_t,
            )
        }
    };

    if rv != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

// ===== Common 'Socket' trait =====

/// Common trait for SocketCAN sockets.
///
/// Note that a socket it created by opening it, and then closed by
/// dropping it.
pub trait Socket: AsRawFd {
    /// The type of CAN frame that can be read and written by the socket.
    ///
    /// This is typically distinguished by the size of the supported frame,
    /// with the primary difference between a `CanFrame` and a `CanFdFrame`.
    type FrameType;

    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "vcan0" or "socan0".
    fn open(ifname: &str) -> Result<Self, CanSocketOpenError>
    where
        Self: Sized,
    {
        let if_index = if_nametoindex(ifname)?;
        Self::open_iface(if_index)
    }

    /// Open CAN device by interface number.
    ///
    /// Opens a CAN device by kernel interface number.
    fn open_iface(if_index: c_uint) -> Result<Self, CanSocketOpenError>
    where
        Self: Sized;

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

    /// Change socket to non-blocking mode or back to blocking mode.
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
        set_socket_option(
            self.as_raw_fd(),
            SOL_SOCKET,
            SO_RCVTIMEO,
            &c_timeval_new(duration),
        )
    }

    /// Sets the write timeout on the socket
    fn set_write_timeout(&self, duration: time::Duration) -> io::Result<()> {
        set_socket_option(
            self.as_raw_fd(),
            SOL_SOCKET,
            SO_SNDTIMEO,
            &c_timeval_new(duration),
        )
    }

    /// Sets CAN ID filters on the socket.
    ///
    /// CAN packages received by SocketCAN are matched against these filters,
    /// only matching packets are returned by the interface.
    ///
    /// See `CanFilter` for details on how filtering works. By default, all
    /// single filter matching all incoming frames is installed.
    fn set_filters<F>(&self, filters: &[F]) -> io::Result<()>
    where
        F: Into<CanFilter> + Copy,
    {
        let filters: Vec<CanFilter> = filters.iter().map(|f| (*f).into()).collect();
        set_socket_option_mult(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_FILTER, &filters)
    }

    /// Disable reception of CAN frames.
    ///
    /// Sets a completely empty filter; disabling all CAN frame reception.
    fn filter_drop_all(&self) -> io::Result<()> {
        let filters: &[CanFilter] = &[];
        set_socket_option_mult(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_FILTER, filters)
    }

    /// Accept all frames, disabling any kind of filtering.
    ///
    /// Replace the current filter with one containing a single rule that
    /// acceps all CAN frames.
    fn filter_accept_all(&self) -> io::Result<()> {
        // safe unwrap: 0, 0 is a valid mask/id pair
        self.set_filters(&[(0, 0)])
    }

    /// Sets the error mask on the socket.
    ///
    /// By default (`ERR_MASK_NONE`) no error conditions are reported as
    /// special error frames by the socket. Enabling error conditions by
    /// setting `ERR_MASK_ALL` or another non-empty error mask causes the
    /// socket to receive notification about the specified conditions.
    fn set_error_filter(&self, mask: u32) -> io::Result<()> {
        set_socket_option(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_ERR_FILTER, &mask)
    }

    /// Sets the error mask on the socket to reject all errors.
    #[inline(always)]
    fn error_filter_drop_all(&self) -> io::Result<()> {
        self.set_error_filter(0)
    }

    /// Sets the error mask on the socket to accept all errors.
    #[inline(always)]
    fn error_filter_accept_all(&self) -> io::Result<()> {
        self.set_error_filter(CAN_ERR_MASK)
    }

    /// Sets the error mask on the socket.
    ///
    /// By default (`ERR_MASK_NONE`) no error conditions are reported as
    /// special error frames by the socket. Enabling error conditions by
    /// setting `ERR_MASK_ALL` or another non-empty error mask causes the
    /// socket to receive notification about the specified conditions.
    fn set_error_mask(&self, mask: u32) -> io::Result<()> {
        set_socket_option(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_ERR_FILTER, &mask)
    }

    /// Enable or disable loopback.
    ///
    /// By default, loopback is enabled, causing other applications that open
    /// the same CAN bus to see frames emitted by different applications on
    /// the same system.
    fn set_loopback(&self, enabled: bool) -> io::Result<()> {
        let loopback = c_int::from(enabled);
        set_socket_option(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_LOOPBACK, &loopback)
    }

    /// Enable or disable receiving of own frames.
    ///
    /// When loopback is enabled, this settings controls if CAN frames sent
    /// are received back immediately by sender. Default is off.
    fn set_recv_own_msgs(&self, enabled: bool) -> io::Result<()> {
        let recv_own_msgs = c_int::from(enabled);
        set_socket_option(
            self.as_raw_fd(),
            SOL_CAN_RAW,
            CAN_RAW_RECV_OWN_MSGS,
            &recv_own_msgs,
        )
    }

    /// Enable or disable join filters.
    ///
    /// By default a frame is accepted if it matches any of the filters set
    /// with `set_filters`. If join filters is enabled, a frame has to match
    /// _all_ filters to be accepted.
    fn set_join_filters(&self, enabled: bool) -> io::Result<()> {
        let join_filters = c_int::from(enabled);
        set_socket_option(
            self.as_raw_fd(),
            SOL_CAN_RAW,
            CAN_RAW_JOIN_FILTERS,
            &join_filters,
        )
    }
}

// TODO: We need to restore this, but preferably with TIMESTAMPING

/*
impl CanSocket {

    /// Blocking read a single can frame with timestamp
    ///
    /// Note that reading a frame and retrieving the timestamp requires two
    /// consecutive syscalls. To avoid race conditions, exclusive access
    /// to the socket is enforce through requiring a `mut &self`.
    pub fn read_frame_with_timestamp(&mut self) -> io::Result<(CanFrame, time::SystemTime)> {
        let frame = self.read_frame()?;

        let mut ts = timespec { tv_sec: 0, tv_nsec: 0 };
        let rval = unsafe {
            libc::ioctl(self.fd, SIOCGSTAMPNS as c_ulong, &mut ts as *mut timespec)
        };

        if rval == -1 {
            return Err(io::Error::last_os_error());
        }

        Ok((frame, system_time_from_timespec(ts)))
    }

}
*/

// ===== CanSocket =====

/// A socket for a CAN 2.0 device.
///
/// Will be closed upon deallocation. To close manually, use std::drop::Drop.
/// Internally this is just a wrapped file-descriptor.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct CanSocket {
    /// The raw file descriptor
    fd: c_int,
}

impl Socket for CanSocket {
    type FrameType = CanFrame;

    /// Opens the socket by interface index.
    fn open_iface(if_index: c_uint) -> Result<Self, CanSocketOpenError> {
        raw_open_socket(if_index).map(|sock_fd| Self { fd: sock_fd })
    }

    /// Writes a normal CAN 2.0 frame to the socket.
    fn write_frame(&self, frame: &CanFrame) -> io::Result<()> {
        raw_write_frame(self.fd, frame.as_ptr())
    }

    /// Reads a normal CAN 2.0 frame from the socket.
    fn read_frame(&self) -> io::Result<CanFrame> {
        let mut frame: can_frame = unsafe { mem::zeroed() };
        let n = mem::size_of::<can_frame>();

        let read_rv = unsafe { read(self.fd, &mut frame as *mut _ as *mut c_void, n) };

        if read_rv as usize != n {
            return Err(io::Error::last_os_error());
        }

        Ok(frame.into())
    }
}

impl AsRawFd for CanSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl FromRawFd for CanSocket {
    unsafe fn from_raw_fd(fd: RawFd) -> CanSocket {
        CanSocket { fd }
    }
}

impl IntoRawFd for CanSocket {
    fn into_raw_fd(self) -> RawFd {
        self.fd
    }
}

impl Drop for CanSocket {
    fn drop(&mut self) {
        unsafe { libc::close(self.as_raw_fd()) };
    }
}

// ===== CanFdSocket =====

/// A socket for CAN FD devices.
///
/// This can transmit and receive CAN 2.0 frames with up to 8-bytes of data,
/// or CAN Flexible Data (FD) frames with up to 64-bytes of data.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct CanFdSocket {
    /// The raw file descriptor
    fd: c_int,
}

impl Socket for CanFdSocket {
    type FrameType = CanAnyFrame;

    /// Opens the FD socket by interface index.
    fn open_iface(if_index: c_uint) -> Result<Self, CanSocketOpenError> {
        raw_open_socket(if_index)
            .and_then(|sock_fd| set_fd_mode(sock_fd, true).map_err(CanSocketOpenError::IOError))
            .map(|sock_fd| Self { fd: sock_fd })
    }

    /// Writes either type of CAN frame to the socket.
    fn write_frame(&self, frame: &CanAnyFrame) -> io::Result<()> {
        match frame {
            CanAnyFrame::Normal(frame) => raw_write_frame(self.fd, frame.as_ptr()),
            CanAnyFrame::Fd(fd_frame) => raw_write_frame(self.fd, fd_frame.as_ptr()),
        }
    }

    /// Reads either type of CAN frame from the socket.
    fn read_frame(&self) -> io::Result<CanAnyFrame> {
        let mut frame = CanFdFrame::default();

        let read_rv = unsafe {
            let frame_ptr = frame.as_mut_ptr();
            read(
                self.fd,
                frame_ptr as *mut c_void,
                mem::size_of::<CanFdFrame>(),
            )
        };
        match read_rv as usize {
            CAN_MTU => CanFrame::try_from(frame)
                .map(|frame| frame.into())
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        "BUG in read_frame: cannot convert to CanFrame",
                    )
                }),

            CANFD_MTU => Ok(frame.into()), // Ok(CanAnyFrame::from(frame)),

            _ => Err(io::Error::last_os_error()),
        }
    }
}

impl AsRawFd for CanFdSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl FromRawFd for CanFdSocket {
    unsafe fn from_raw_fd(fd: RawFd) -> CanFdSocket {
        CanFdSocket { fd }
    }
}

impl IntoRawFd for CanFdSocket {
    fn into_raw_fd(self) -> RawFd {
        self.fd
    }
}

impl Drop for CanFdSocket {
    fn drop(&mut self) {
        unsafe { libc::close(self.as_raw_fd()) };
    }
}

// ===== CanFilter =====

/// The CAN filter defines which ID's can be accepted on a socket.
///
/// Each filter contains an internal id and mask. Packets are considered to be matched
/// by a filter if `received_id & mask == filter_id & mask` holds true.
///
/// A socket can be given multiple filters, and each one can be inverted
/// ([ref](https://docs.kernel.org/networking/can.html#raw-protocol-sockets-with-can-filters-sock-raw))
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct CanFilter(libc::can_filter);

impl CanFilter {
    /// Construct a new CAN filter.
    pub fn new(id: canid_t, mask: canid_t) -> Self {
        Self(libc::can_filter {
            can_id: id,
            can_mask: mask,
        })
    }

    /// Construct a new inverted CAN filter.
    pub fn new_inverted(id: canid_t, mask: canid_t) -> Self {
        Self::new(id | libc::CAN_INV_FILTER, mask)
    }
}

impl From<libc::can_filter> for CanFilter {
    fn from(filt: libc::can_filter) -> Self {
        Self(filt)
    }
}

impl From<(u32, u32)> for CanFilter {
    fn from(filt: (u32, u32)) -> Self {
        CanFilter::new(filt.0, filt.1)
    }
}

impl AsRef<libc::can_filter> for CanFilter {
    fn as_ref(&self) -> &libc::can_filter {
        &self.0
    }
}
