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

use crate::{
    frame::{can_frame_default, canfd_frame_default, AsPtr, CAN_ERR_MASK},
    CanAnyFrame, CanFdFrame, CanFrame,
};
use libc::{
    can_frame, canid_t, fcntl, read, sa_family_t, setsockopt, sockaddr, sockaddr_can,
    sockaddr_storage, socklen_t, suseconds_t, time_t, timeval, write, EINPROGRESS, F_GETFL,
    F_SETFL, O_NONBLOCK, SOCK_RAW, SOL_SOCKET, SO_RCVTIMEO, SO_SNDTIMEO,
};
use nix::net::if_::if_nametoindex;
use std::{
    fmt, io, mem,
    os::{
        raw::{c_int, c_void},
        unix::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    },
    ptr,
    time::Duration,
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
                matches!(self.raw_os_error(), Some(errno) if errno == EINPROGRESS)
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
/// This is the address for use with CAN sockets. It is simply an addres to
/// the SocketCAN host interface. It can be created by looking up the name
/// of the interface, like "can0", "vcan0", etc, or an interface index can
/// be specified directly, if known. An index of zero can be used to read
/// frames from all interfaces.
///
/// This is based on, and compatible with, the `sockaddr_can` struct from
/// libc.
/// [ref](https://docs.rs/libc/latest/libc/struct.sockaddr_can.html)
#[derive(Clone, Copy)]
pub struct CanAddr(sockaddr_can);

impl CanAddr {
    /// Creates a new CAN socket address for the specified interface by index.
    /// An index of zero can be used to read from all interfaces.
    pub fn new(ifindex: u32) -> Self {
        let mut addr = Self::default();
        addr.0.can_ifindex = ifindex as c_int;
        addr
    }

    /// Try to create an address from an interface name.
    pub fn from_iface(ifname: &str) -> io::Result<Self> {
        let ifindex = if_nametoindex(ifname)?;
        Ok(Self::new(ifindex))
    }

    /// Gets the address of the structure as a `sockaddr_can` pointer.
    pub fn as_ptr(&self) -> *const sockaddr_can {
        &self.0
    }

    /// Gets the address of the structure as a `sockaddr` pointer.
    pub fn as_sockaddr_ptr(&self) -> *const sockaddr {
        self.as_ptr().cast()
    }

    /// Gets the size of the address structure.
    pub fn len() -> usize {
        mem::size_of::<sockaddr_can>()
    }

    /// Converts the address into a `sockaddr_storage` type.
    /// This is a generic socket address container with enough space to hold
    /// any address type in the system.
    pub fn into_storage(self) -> (sockaddr_storage, socklen_t) {
        let len = Self::len();
        let mut storage: sockaddr_storage = unsafe { mem::zeroed() };
        unsafe {
            ptr::copy_nonoverlapping(
                &self.0 as *const _ as *const sockaddr_storage,
                &mut storage,
                len,
            );
        }
        (storage, len as socklen_t)
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

fn c_timeval_new(t: Duration) -> timeval {
    timeval {
        tv_sec: t.as_secs() as time_t,
        tv_usec: t.subsec_micros() as suseconds_t,
    }
}

/// Tries to open the CAN socket by the interface number.
fn raw_open_socket(addr: &CanAddr) -> io::Result<c_int> {
    let fd = unsafe { libc::socket(PF_CAN, SOCK_RAW, CAN_RAW) };

    if fd == -1 {
        return Err(io::Error::last_os_error());
    }

    let ret = unsafe { libc::bind(fd, addr.as_sockaddr_ptr(), CanAddr::len() as u32) };

    if ret == -1 {
        let err = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        Err(err)
    } else {
        Ok(fd)
    }
}

// Enable or disable FD mode on the socket, fd.
fn set_fd_mode(fd: c_int, enable: bool) -> io::Result<c_int> {
    let enable = enable as c_int;

    let ret = unsafe {
        setsockopt(
            fd,
            SOL_CAN_RAW,
            CAN_RAW_FD_FRAMES,
            &enable as *const _ as *const c_void,
            mem::size_of::<c_int>() as u32,
        )
    };

    if ret == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

// Write a single frame of any type to the socket, fd.
fn raw_write_frame<T>(fd: c_int, frame_ptr: *const T, n: usize) -> io::Result<()> {
    let ret = unsafe { write(fd, frame_ptr.cast(), n) };

    if ret as usize == n {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
    let ret = unsafe {
        setsockopt(
            fd,
            level,
            name,
            val as *const _ as *const c_void,
            mem::size_of::<T>() as socklen_t,
        )
    };

    if ret != 0 {
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
    let ret = if values.is_empty() {
        // can't pass in a ptr to a 0-len slice, pass a null ptr instead
        unsafe { setsockopt(fd, level, name, ptr::null(), 0) }
    } else {
        unsafe {
            setsockopt(
                fd,
                level,
                name,
                values.as_ptr().cast(),
                mem::size_of_val(values) as socklen_t,
            )
        }
    };

    if ret != 0 {
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
    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "can0", "vcan0", or "socan0".
    fn open(ifname: &str) -> io::Result<Self>
    where
        Self: Sized,
    {
        let addr = CanAddr::from_iface(ifname)?;
        Self::open_addr(&addr)
    }

    /// Open CAN device by interface number.
    ///
    /// Opens a CAN device by kernel interface number.
    fn open_iface(ifindex: u32) -> io::Result<Self>
    where
        Self: Sized,
    {
        let addr = CanAddr::new(ifindex);
        Self::open_addr(&addr)
    }

    /// Open a CAN socket by address.
    fn open_addr(addr: &CanAddr) -> io::Result<Self>
    where
        Self: Sized;

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

        let ret = unsafe { fcntl(self.as_raw_fd(), F_SETFL, newfl) };

        if ret != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The type of CAN frame that can be read and written by the socket.
    ///
    /// This is typically distinguished by the size of the supported frame,
    /// with the primary difference between a `CanFrame` and a `CanFdFrame`.
    type FrameType;

    /// Sets the read timeout on the socket
    ///
    /// For convenience, the result value can be checked using
    /// `ShouldRetry::should_retry` when a timeout is set.
    fn set_read_timeout(&self, duration: Duration) -> io::Result<()> {
        set_socket_option(
            self.as_raw_fd(),
            SOL_SOCKET,
            SO_RCVTIMEO,
            &c_timeval_new(duration),
        )
    }

    /// Sets the write timeout on the socket
    fn set_write_timeout(&self, duration: Duration) -> io::Result<()> {
        set_socket_option(
            self.as_raw_fd(),
            SOL_SOCKET,
            SO_SNDTIMEO,
            &c_timeval_new(duration),
        )
    }

    /// Blocking read a single can frame.
    fn read_frame(&self) -> io::Result<Self::FrameType>;

    /// Blocking read a single can frame with timeout.
    fn read_frame_timeout(&self, timeout: Duration) -> io::Result<Self::FrameType> {
        use nix::poll::{poll, PollFd, PollFlags};
        let pollfd = PollFd::new(self.as_raw_fd(), PollFlags::POLLIN);

        match poll(&mut [pollfd], timeout.as_millis() as c_int)? {
            0 => Err(io::ErrorKind::TimedOut.into()),
            _ => self.read_frame(),
        }
    }

    /// Write a single can frame.
    ///
    /// Note that this function can fail with an `EAGAIN` error or similar.
    /// Use `write_frame_insist` if you need to be sure that the message got
    /// sent or failed.
    //fn write_frame(&self, frame: &Self::FrameType) -> io::Result<()>;

    /// Writes a normal CAN 2.0 frame to the socket.
    fn write_frame<F>(&self, frame: &F) -> io::Result<()>
    where
        F: Into<Self::FrameType> + AsPtr;

    /// Blocking write a single can frame, retrying until it gets sent
    /// successfully.
    fn write_frame_insist<F>(&self, frame: &F) -> io::Result<()>
    where
        F: Into<Self::FrameType> + AsPtr,
    {
        loop {
            match self.write_frame(frame) {
                Ok(v) => return Ok(v),
                Err(e) if e.should_retry() => (),
                Err(e) => return Err(e),
            }
        }
    }
}

/// Traits for setting CAN socket options
pub trait SocketOptions: AsRawFd {
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
    fn set_filter_drop_all(&self) -> io::Result<()> {
        let filters: &[CanFilter] = &[];
        set_socket_option_mult(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_FILTER, filters)
    }

    /// Accept all frames, disabling any kind of filtering.
    ///
    /// Replace the current filter with one containing a single rule that
    /// acceps all CAN frames.
    fn set_filter_accept_all(&self) -> io::Result<()> {
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
    fn set_error_filter_drop_all(&self) -> io::Result<()> {
        self.set_error_filter(0)
    }

    /// Sets the error mask on the socket to accept all errors.
    #[inline(always)]
    fn set_error_filter_accept_all(&self) -> io::Result<()> {
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
        let ret = unsafe {
            libc::ioctl(self.fd, SIOCGSTAMPNS as c_ulong, &mut ts as *mut timespec)
        };

        if ret == -1 {
            return Err(io::Error::last_os_error());
        }

        Ok((frame, system_time_from_timespec(ts)))
    }

}
*/

// ===== CanSocket =====

/// A socket for a classic CAN 2.0 device.
///
/// This provides an interface to read and write classic CAN 2.0 frames to
/// the bus, with up to 8 bytes of data per frame. It wraps a Linux socket
/// descriptor to a Raw SocketCAN socket.
///
/// The socket is automatically closed when the object is dropped. To close
/// manually, use std::drop::Drop. Internally this is just a wrapped socket
/// (file) descriptor.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct CanSocket {
    /// The raw file descriptor
    fd: OwnedFd,
}

impl Socket for CanSocket {
    /// CanSocket reads/writes classic CAN 2.0 frames.
    type FrameType = CanFrame;

    /// Opens the socket by interface index.
    fn open_addr(addr: &CanAddr) -> io::Result<Self> {
        raw_open_socket(addr).map(|fd| Self {
            /// SAFETY: We just obtained this FD and no else has seen it.
            /// Hence, it is fine to take exclusive ownership.
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    /// Writes a normal CAN 2.0 frame to the socket.
    fn write_frame<F>(&self, frame: &F) -> io::Result<()>
    where
        F: Into<CanFrame> + AsPtr,
    {
        raw_write_frame(self.fd.as_raw_fd(), frame.as_ptr(), frame.size())
    }

    /// Reads a normal CAN 2.0 frame from the socket.
    fn read_frame(&self) -> io::Result<CanFrame> {
        let mut frame = can_frame_default();
        let n = mem::size_of::<can_frame>();

        let rd = unsafe { read(self.fd.as_raw_fd(), &mut frame as *mut _ as *mut c_void, n) };

        if rd as usize == n {
            Ok(frame.into())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

impl SocketOptions for CanSocket {}

// Has no effect: #[deprecated(since = "3.1", note = "Use AsFd::as_fd() instead.")]
impl AsRawFd for CanSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl FromRawFd for CanSocket {
    unsafe fn from_raw_fd(fd: RawFd) -> CanSocket {
        CanSocket {
            /// Safety: The caller asserts that we may take ownership of this FD by passing it into
            /// from_raw_fd().
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        }
    }
}

impl IntoRawFd for CanSocket {
    fn into_raw_fd(self) -> RawFd {
        self.fd.into_raw_fd()
    }
}

impl AsFd for CanSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
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
    fd: OwnedFd,
}

impl Socket for CanFdSocket {
    /// CanFdSocket can read/write classic CAN 2.0 or FD frames.
    type FrameType = CanAnyFrame;

    /// Opens the FD socket by interface index.
    fn open_addr(addr: &CanAddr) -> io::Result<Self> {
        raw_open_socket(addr)
            .and_then(|fd| set_fd_mode(fd, true))
            .map(|fd| Self {
                /// SAFETY: We just obtained this FD and no else has seen it.
                /// Hence, it is fine to take exclusive ownership.
                fd: unsafe { OwnedFd::from_raw_fd(fd) },
            })
    }

    /// Writes any type of CAN frame to the socket.
    fn write_frame<F>(&self, frame: &F) -> io::Result<()>
    where
        F: Into<Self::FrameType> + AsPtr,
    {
        raw_write_frame(self.fd.as_raw_fd(), frame.as_ptr(), frame.size())
    }

    /// Reads either type of CAN frame from the socket.
    fn read_frame(&self) -> io::Result<CanAnyFrame> {
        let mut fdframe = canfd_frame_default();

        let rd = unsafe {
            read(
                self.fd.as_raw_fd(),
                &mut fdframe as *mut _ as *mut c_void,
                CANFD_MTU,
            )
        };
        match rd as usize {
            // If we only get 'can_frame' number of bytes, then the return is,
            // by definition, a can_frame, so we just copy the bytes into the
            // proper type.
            CAN_MTU => {
                let mut frame = can_frame_default();
                unsafe {
                    ptr::copy_nonoverlapping(
                        &fdframe as *const _ as *const can_frame,
                        &mut frame,
                        CAN_MTU,
                    );
                }
                Ok(CanFrame::from(frame).into())
            }
            CANFD_MTU => Ok(CanFdFrame::from(fdframe).into()),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

impl SocketOptions for CanFdSocket {}

// Has no effect: #[deprecated(since = "3.1", note = "Use AsFd::as_fd() instead.")]
impl AsRawFd for CanFdSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl FromRawFd for CanFdSocket {
    unsafe fn from_raw_fd(fd: RawFd) -> CanFdSocket {
        CanFdSocket {
            /// Safety: The caller asserts that we may take ownership of this FD by passing it into
            /// from_raw_fd().
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        }
    }
}

impl IntoRawFd for CanFdSocket {
    fn into_raw_fd(self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl AsFd for CanFdSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

// ===== CanFilter =====

/// The CAN filter defines which ID's can be accepted on a socket.
///
/// Each filter contains an internal id and mask. Packets are considered to
/// be matched by a filter if `received_id & mask == filter_id & mask` holds
/// true.
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
