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
    CanAnyFrame, CanFdFrame, CanFrame, CanRawFrame, Error, IoError, IoErrorKind, IoResult, Result,
    as_bytes, as_bytes_mut,
    frame::{AsPtr, can_frame_default, canfd_frame_default},
    id::CAN_ERR_MASK,
    timestamp::CanTimestamps,
};
pub use embedded_can::{
    self, ExtendedId, Frame as EmbeddedFrame, Id, StandardId, blocking::Can as BlockingCan,
    nb::Can as NonBlockingCan,
};
use libc::{AF_CAN, EINPROGRESS, SOL_SOCKET, canid_t, socklen_t};
use socket2::SockAddr;
use std::{
    fmt,
    io::{Read, Write},
    mem::{size_of, size_of_val, zeroed},
    os::{
        raw::{c_int, c_void},
        unix::io::{AsFd, AsRawFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd},
    },
    ptr,
    time::{Duration, SystemTime},
};

pub use libc::{
    CAN_MTU, CAN_RAW, CAN_RAW_ERR_FILTER, CAN_RAW_FD_FRAMES, CAN_RAW_FILTER, CAN_RAW_JOIN_FILTERS,
    CAN_RAW_LOOPBACK, CAN_RAW_RECV_OWN_MSGS, CANFD_MTU, SOL_CAN_BASE, SOL_CAN_RAW,
};

// TODO: This can be removed on the next major version update
pub use crate::CanAddr;

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

impl ShouldRetry for IoError {
    fn should_retry(&self) -> bool {
        match self.kind() {
            // EAGAIN, EINPROGRESS and EWOULDBLOCK are the three possible codes
            // returned when a timeout occurs. the stdlib already maps EAGAIN
            // and EWOULDBLOCK os WouldBlock
            IoErrorKind::WouldBlock => true,
            // however, EINPROGRESS is also valid
            IoErrorKind::Other => {
                matches!(self.raw_os_error(), Some(errno) if errno == EINPROGRESS)
            }
            _ => false,
        }
    }
}

impl<E: fmt::Debug> ShouldRetry for IoResult<E> {
    fn should_retry(&self) -> bool {
        match *self {
            Err(ref e) => e.should_retry(),
            _ => false,
        }
    }
}

// ===== Private local helper functions =====

/// Tries to open the CAN socket by the interface number.
fn raw_open_socket(addr: &CanAddr) -> IoResult<socket2::Socket> {
    let af_can = socket2::Domain::from(AF_CAN);
    let can_raw = socket2::Protocol::from(CAN_RAW);

    let sock = socket2::Socket::new_raw(af_can, socket2::Type::RAW, Some(can_raw))?;
    sock.bind(&SockAddr::from(*addr))?;
    Ok(sock)
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
#[deprecated(since = "3.4.0", note = "Moved into `SocketOptions` trait")]
#[inline]
pub fn set_socket_option<T>(fd: c_int, level: c_int, name: c_int, val: &T) -> IoResult<()> {
    let ret = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            val as *const _ as *const c_void,
            size_of::<T>() as socklen_t,
        )
    };

    match ret {
        0 => Ok(()),
        _ => Err(IoError::last_os_error()),
    }
}

/// Sets a collection of multiple socket options with one call.
#[deprecated(since = "3.4.0", note = "Moved into `SocketOptions` trait")]
pub fn set_socket_option_mult<T>(
    fd: c_int,
    level: c_int,
    name: c_int,
    values: &[T],
) -> IoResult<()> {
    let ret = if values.is_empty() {
        // can't pass in a ptr to a 0-len slice, pass a null ptr instead
        unsafe { libc::setsockopt(fd, level, name, ptr::null(), 0) }
    } else {
        unsafe {
            libc::setsockopt(
                fd,
                level,
                name,
                values.as_ptr().cast(),
                size_of_val(values) as socklen_t,
            )
        }
    };

    match ret {
        0 => Ok(()),
        _ => Err(IoError::last_os_error()),
    }
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
    fn open(ifname: &str) -> IoResult<Self>
    where
        Self: Sized,
    {
        let addr = CanAddr::from_iface(ifname)?;
        Self::open_addr(&addr)
    }

    /// Open CAN device by interface number.
    ///
    /// Opens a CAN device by kernel interface number.
    fn open_iface(ifindex: u32) -> IoResult<Self>
    where
        Self: Sized,
    {
        let addr = CanAddr::new(ifindex);
        Self::open_addr(&addr)
    }

    /// Open a CAN socket by address.
    fn open_addr(addr: &CanAddr) -> IoResult<Self>
    where
        Self: Sized;

    /// Gets a shared reference to the underlying socket object
    fn as_raw_socket(&self) -> &socket2::Socket;

    /// Gets a mutable reference to the underlying socket object
    fn as_raw_socket_mut(&mut self) -> &mut socket2::Socket;

    /// Determines if the socket is currently in nonblocking mode.
    fn nonblocking(&self) -> IoResult<bool> {
        self.as_raw_socket().nonblocking()
    }

    /// Change socket to non-blocking mode or back to blocking mode.
    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()> {
        self.as_raw_socket().set_nonblocking(nonblocking)
    }

    /// The type of CAN frame that can be read and written by the socket.
    ///
    /// This is typically distinguished by the size of the supported frame,
    /// with the primary difference between a `CanFrame` and a `CanFdFrame`.
    type FrameType;

    /// Gets the read timeout on the socket, if any.
    fn read_timeout(&self) -> IoResult<Option<Duration>> {
        self.as_raw_socket().read_timeout()
    }

    /// Sets the read timeout on the socket
    ///
    /// For convenience, the result value can be checked using
    /// `ShouldRetry::should_retry` when a timeout is set.
    ///
    /// If the duration is set to `None` then write calls will block
    /// indefinitely.
    fn set_read_timeout<D>(&self, duration: D) -> IoResult<()>
    where
        D: Into<Option<Duration>>,
    {
        self.as_raw_socket().set_read_timeout(duration.into())
    }

    /// Gets the write timeout on the socket, if any.
    fn write_timeout(&self) -> IoResult<Option<Duration>> {
        self.as_raw_socket().write_timeout()
    }

    /// Sets the write timeout on the socket
    ///
    /// If the duration is set to `None` then write calls will block
    /// indefinitely.
    fn set_write_timeout<D>(&self, duration: D) -> IoResult<()>
    where
        D: Into<Option<Duration>>,
    {
        self.as_raw_socket().set_write_timeout(duration.into())
    }

    /// Blocking read a single can frame.
    ///
    /// Concurrent readers: each `recvmsg()` consumes one frame from the
    /// socket's kernel receive queue. If two tasks call `read_frame*` on the
    /// same socket concurrently (via shared references), each will receive a
    /// disjoint subset of frames, but no two will ever observe the same
    /// frame. The frames are not duplicated and the call is safe, but the
    /// per-reader stream is not deterministic — design with that in mind.
    fn read_frame(&self) -> IoResult<Self::FrameType>;

    /// Blocking read a single can frame with timeout.
    fn read_frame_timeout(&self, timeout: Duration) -> IoResult<Self::FrameType> {
        use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
        let pollfd = PollFd::new(
            unsafe { BorrowedFd::borrow_raw(self.as_raw_fd()) },
            PollFlags::POLLIN,
        );

        match poll(
            &mut [pollfd],
            timeout.try_into().unwrap_or(PollTimeout::MAX),
        )? {
            0 => Err(IoErrorKind::TimedOut.into()),
            _ => self.read_frame(),
        }
    }

    /// Blocking read a CAN frame and its socket-layer arrival timestamp.
    ///
    /// Requires [`SocketOptions::set_recv_timestamp`] to be called with `true`
    /// before this method. Returns an `InvalidData` error if no
    /// `SO_TIMESTAMPNS` control message was delivered.
    fn read_frame_with_timestamp(&self) -> IoResult<(Self::FrameType, SystemTime)> {
        Err(IoError::from_raw_os_error(libc::ENOSYS))
    }

    /// Blocking read a CAN frame and its raw hardware clock timestamp.
    ///
    /// Requires [`SocketOptions::set_timestamping`] to be called with
    /// `SOF_TIMESTAMPING_RX_HARDWARE | SOF_TIMESTAMPING_OPT_CMSG` (and any
    /// other desired flags) before this method. Returns an `InvalidData` error
    /// if no hardware timestamp was delivered.
    fn read_frame_with_hw_timestamp(&self) -> IoResult<(Self::FrameType, Duration)> {
        Err(IoError::from_raw_os_error(libc::ENOSYS))
    }

    /// Blocking read a CAN frame and all available timestamps.
    ///
    /// Populates whichever [`CanTimestamps`] fields correspond to the
    /// `SO_TIMESTAMPNS` and/or `SO_TIMESTAMPING` modes that were enabled on
    /// the socket before the call. Fields for disabled modes are `None`.
    fn read_frame_with_timestamps(&self) -> IoResult<(Self::FrameType, CanTimestamps)> {
        Err(IoError::from_raw_os_error(libc::ENOSYS))
    }

    /// Writes a normal CAN 2.0 frame to the socket.
    fn write_frame<F>(&self, frame: &F) -> IoResult<()>
    where
        F: Into<Self::FrameType> + AsPtr;

    /// Blocking write a single can frame, retrying until it gets sent
    /// successfully.
    fn write_frame_insist<F>(&self, frame: &F) -> IoResult<()>
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

/// Traits for setting CAN socket options.
///
/// These are blocking calls, even when implemented on asynchronous sockets.
pub trait SocketOptions: AsRawFd {
    /// Sets an option on the socket.
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
    /// sock.set_socket_option(SOL_TCP, TCP_NO_DELAY, 1 as c_int)
    /// ```
    ///
    /// Note that the `val` parameter must be specified correctly; if an option
    /// expects an integer, it is advisable to pass in a `c_int`, not the default
    /// of `i32`.
    fn set_socket_option<T>(&self, level: c_int, name: c_int, val: &T) -> IoResult<()> {
        let ret = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                level,
                name,
                val as *const _ as *const c_void,
                size_of::<T>() as socklen_t,
            )
        };

        match ret {
            0 => Ok(()),
            _ => Err(IoError::last_os_error()),
        }
    }

    /// Sets a collection of multiple socket options with one call.
    fn set_socket_option_mult<T>(&self, level: c_int, name: c_int, values: &[T]) -> IoResult<()> {
        let ret = if values.is_empty() {
            // can't pass in a ptr to a 0-len slice, pass a null ptr instead
            unsafe { libc::setsockopt(self.as_raw_fd(), level, name, ptr::null(), 0) }
        } else {
            unsafe {
                libc::setsockopt(
                    self.as_raw_fd(),
                    level,
                    name,
                    values.as_ptr().cast(),
                    size_of_val(values) as socklen_t,
                )
            }
        };

        match ret {
            0 => Ok(()),
            _ => Err(IoError::last_os_error()),
        }
    }

    /// Sets CAN ID filters on the socket.
    ///
    /// CAN packages received by SocketCAN are matched against these filters,
    /// only matching packets are returned by the interface.
    ///
    /// See `CanFilter` for details on how filtering works. By default, all
    /// single filter matching all incoming frames is installed.
    fn set_filters<F>(&self, filters: &[F]) -> IoResult<()>
    where
        F: Into<CanFilter> + Copy,
    {
        let filters: Vec<CanFilter> = filters.iter().map(|f| (*f).into()).collect();
        self.set_socket_option_mult(SOL_CAN_RAW, CAN_RAW_FILTER, &filters)
    }

    /// Disable reception of CAN frames.
    ///
    /// Sets a completely empty filter; disabling all CAN frame reception.
    fn set_filter_drop_all(&self) -> IoResult<()> {
        let filters: &[CanFilter] = &[];
        self.set_socket_option_mult(SOL_CAN_RAW, CAN_RAW_FILTER, filters)
    }

    /// Accept all frames, disabling any kind of filtering.
    ///
    /// Replace the current filter with one containing a single rule that
    /// acceps all CAN frames.
    fn set_filter_accept_all(&self) -> IoResult<()> {
        // safe unwrap: 0, 0 is a valid mask/id pair
        self.set_filters(&[(0, 0)])
    }

    /// Sets the error mask on the socket.
    ///
    /// By default (`ERR_MASK_NONE`) no error conditions are reported as
    /// special error frames by the socket. Enabling error conditions by
    /// setting `ERR_MASK_ALL` or another non-empty error mask causes the
    /// socket to receive notification about the specified conditions.
    fn set_error_filter(&self, mask: u32) -> IoResult<()> {
        self.set_socket_option(SOL_CAN_RAW, CAN_RAW_ERR_FILTER, &mask)
    }

    /// Sets the error mask on the socket to reject all errors.
    #[inline(always)]
    fn set_error_filter_drop_all(&self) -> IoResult<()> {
        self.set_error_filter(0)
    }

    /// Sets the error mask on the socket to accept all errors.
    #[inline(always)]
    fn set_error_filter_accept_all(&self) -> IoResult<()> {
        self.set_error_filter(CAN_ERR_MASK)
    }

    /// Sets the error mask on the socket.
    ///
    /// By default (`ERR_MASK_NONE`) no error conditions are reported as
    /// special error frames by the socket. Enabling error conditions by
    /// setting `ERR_MASK_ALL` or another non-empty error mask causes the
    /// socket to receive notification about the specified conditions.
    fn set_error_mask(&self, mask: u32) -> IoResult<()> {
        self.set_socket_option(SOL_CAN_RAW, CAN_RAW_ERR_FILTER, &mask)
    }

    /// Enable or disable loopback.
    ///
    /// By default, loopback is enabled, causing other applications that open
    /// the same CAN bus to see frames emitted by different applications on
    /// the same system.
    fn set_loopback(&self, enabled: bool) -> IoResult<()> {
        let loopback = c_int::from(enabled);
        self.set_socket_option(SOL_CAN_RAW, CAN_RAW_LOOPBACK, &loopback)
    }

    /// Enable or disable receiving of own frames.
    ///
    /// When loopback is enabled, this settings controls if CAN frames sent
    /// are received back immediately by sender. Default is off.
    fn set_recv_own_msgs(&self, enabled: bool) -> IoResult<()> {
        let recv_own_msgs = c_int::from(enabled);
        self.set_socket_option(SOL_CAN_RAW, CAN_RAW_RECV_OWN_MSGS, &recv_own_msgs)
    }

    /// Enable or disable join filters.
    ///
    /// By default a frame is accepted if it matches any of the filters set
    /// with `set_filters`. If join filters is enabled, a frame has to match
    /// _all_ filters to be accepted.
    fn set_join_filters(&self, enabled: bool) -> IoResult<()> {
        let join_filters = c_int::from(enabled);
        self.set_socket_option(SOL_CAN_RAW, CAN_RAW_JOIN_FILTERS, &join_filters)
    }

    /// Enable or disable `SO_TIMESTAMPNS` on the socket.
    ///
    /// When enabled, `recvmsg()` delivers a `SCM_TIMESTAMPNS` control message
    /// containing the socket-layer arrival time as a `timespec`. Call this
    /// before using [`Socket::read_frame_with_timestamp`].
    ///
    /// This option is independent of [`set_timestamping`]; both can be
    /// enabled simultaneously and the resulting timestamps land in
    /// separate fields of [`CanTimestamps`].
    ///
    /// [`set_timestamping`]: Self::set_timestamping
    /// [`CanTimestamps`]: crate::CanTimestamps
    fn set_recv_timestamp(&self, enable: bool) -> IoResult<()> {
        let val = c_int::from(enable);
        self.set_socket_option(SOL_SOCKET, libc::SO_TIMESTAMPNS, &val)
    }

    /// Set `SO_TIMESTAMPING` flags on the socket.
    ///
    /// `flags` is a bitmask of `SOF_TIMESTAMPING_*` constants. Each
    /// timestamp source needs two flags — one to select **when** it is
    /// taken, and one to request that it be **reported** in the ancillary
    /// data:
    ///
    /// | When (selector)                  | Report (in ancillary data)            |
    /// |----------------------------------|---------------------------------------|
    /// | [`SOF_TIMESTAMPING_RX_SOFTWARE`] | [`SOF_TIMESTAMPING_SOFTWARE`]         |
    /// | [`SOF_TIMESTAMPING_RX_HARDWARE`] | [`SOF_TIMESTAMPING_RAW_HARDWARE`]     |
    ///
    /// Setting only a selector flag silently delivers no timestamps;
    /// setting only a reporter flag captures nothing to report.
    ///
    /// In addition, [`SOF_TIMESTAMPING_OPT_CMSG`] is required for RX
    /// timestamps to actually appear in the cmsg returned by `recvmsg()`
    /// on non-IP sockets (which includes CAN raw).
    ///
    /// Call this before using [`Socket::read_frame_with_timestamps`] or
    /// [`Socket::read_frame_with_hw_timestamp`].
    ///
    /// [`SOF_TIMESTAMPING_OPT_CMSG`]: crate::SOF_TIMESTAMPING_OPT_CMSG
    /// [`SOF_TIMESTAMPING_RX_SOFTWARE`]: crate::SOF_TIMESTAMPING_RX_SOFTWARE
    /// [`SOF_TIMESTAMPING_SOFTWARE`]: crate::SOF_TIMESTAMPING_SOFTWARE
    /// [`SOF_TIMESTAMPING_RX_HARDWARE`]: crate::SOF_TIMESTAMPING_RX_HARDWARE
    /// [`SOF_TIMESTAMPING_RAW_HARDWARE`]: crate::SOF_TIMESTAMPING_RAW_HARDWARE
    fn set_timestamping(&self, flags: u32) -> IoResult<()> {
        let val = flags as c_int;
        self.set_socket_option(SOL_SOCKET, libc::SO_TIMESTAMPING, &val)
    }
}

// ===== Private helpers =====

/// Returns true if the interface bound to `fd` reports RX hardware timestamp support.
///
/// Issues a `SIOCETHTOOL` / `ETHTOOL_GET_TS_INFO` ioctl and checks the
/// `SOF_TIMESTAMPING_RX_HARDWARE` bit.
///
/// Returns `false` on any error (unbound socket, unsupported ioctl,
/// unknown interface, etc).
fn hw_timestamps_supported(fd: RawFd) -> bool {
    use crate::timestamp::{ETHTOOL_GET_TS_INFO, EthtoolTsInfo, SOF_TIMESTAMPING_RX_HARDWARE};
    // Ioctl is u64 in glibc and i32 in musl this ensures the correct type is used for both
    const SIOCETHTOOL: libc::Ioctl = libc::SIOCETHTOOL as libc::Ioctl;

    // Retrieve the interface index from the bound socket address.
    let ifindex = unsafe {
        let mut addr: libc::sockaddr_can = zeroed();
        let mut addrlen = size_of::<libc::sockaddr_can>() as socklen_t;
        let ret = libc::getsockname(fd, &mut addr as *mut _ as *mut libc::sockaddr, &mut addrlen);
        if ret != 0 || addr.can_ifindex <= 0 {
            return false;
        }
        addr.can_ifindex as libc::c_uint
    };

    // Convert interface index to a name string.
    let mut ifname = [0 as libc::c_char; libc::IF_NAMESIZE];
    if unsafe { libc::if_indextoname(ifindex, ifname.as_mut_ptr()) }.is_null() {
        return false;
    }

    // Query hardware timestamping capabilities via SIOCETHTOOL.
    let mut ts_info = EthtoolTsInfo {
        cmd: ETHTOOL_GET_TS_INFO,
        so_timestamping: 0,
        phc_index: 0,
        tx_types: 0,
        tx_reserved: [0; 3],
        rx_filters: 0,
        rx_reserved: [0; 3],
    };

    let ret = unsafe {
        let mut ifr: libc::ifreq = zeroed();
        ifr.ifr_name.copy_from_slice(&ifname);
        ifr.ifr_ifru.ifru_data = (&mut ts_info as *mut EthtoolTsInfo).cast();
        libc::ioctl(fd, SIOCETHTOOL, &mut ifr)
    };

    ret == 0 && ts_info.so_timestamping & SOF_TIMESTAMPING_RX_HARDWARE != 0
}

/// Size of the `recvmsg()` ancillary control buffer.
///
/// Comfortably larger than what we need today:
/// `CMSG_SPACE(sizeof(timespec))`            — `SO_TIMESTAMPNS` cmsg
/// `+ CMSG_SPACE(3 * sizeof(timespec))`      — `SO_TIMESTAMPING` cmsg
/// ≈ 80 bytes on 64-bit Linux. 256 leaves headroom for future cmsg types.
const CTRL_BUF_SIZE: usize = 256;

/// Properly-aligned backing storage for the `recvmsg()` ancillary buffer.
///
/// `CMSG_FIRSTHDR`/`CMSG_NXTHDR` interpret the buffer as a sequence of
/// `cmsghdr` structures, which require `usize` alignment on Linux. A raw
/// `[u8; N]` has alignment 1; aligning to 8 bytes satisfies the contract
/// on all supported architectures.
#[repr(C, align(8))]
struct CtrlBuf([u8; CTRL_BUF_SIZE]);

/// Issues `recvmsg()` on `fd`, writing frame bytes into `frame_buf`, and
/// parses any `SOL_SOCKET` timestamp control messages into a [`CanTimestamps`].
///
/// Returns `(bytes_received, timestamps)`. The returned byte count is the
/// *real* packet size on the wire (via `MSG_TRUNC`), even if it exceeds
/// `frame_buf.len()`; callers should compare against `CAN_MTU`/`CANFD_MTU`
/// before trusting the buffer contents. Timestamp fields are `None` when
/// the corresponding socket option was not enabled before the call.
///
/// Returns `InvalidData` if the kernel sets `MSG_CTRUNC`, indicating the
/// ancillary buffer was too small to hold all delivered cmsgs.
fn recvmsg_with_ctrl(fd: RawFd, frame_buf: &mut [u8]) -> IoResult<(usize, CanTimestamps)> {
    use crate::timestamp::{timespec_to_duration, timespec_to_system_time};

    let mut iov = libc::iovec {
        iov_base: frame_buf.as_mut_ptr() as *mut libc::c_void,
        iov_len: frame_buf.len(),
    };

    let mut ctrl = CtrlBuf([0u8; CTRL_BUF_SIZE]);
    let mut msg: libc::msghdr = unsafe { zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl.0.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = ctrl.0.len() as _;

    // MSG_TRUNC: return the real packet length even if the iov was too small,
    // so callers can distinguish classic vs. FD frames by byte count rather
    // than silently truncating an FD frame into a classic-sized buffer.
    let n = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_TRUNC) };
    if n < 0 {
        return Err(IoError::last_os_error());
    }

    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(IoError::new(
            IoErrorKind::InvalidData,
            "recvmsg ancillary control buffer overflowed (MSG_CTRUNC)",
        ));
    }

    // Minimum cmsg_len for the two payload types we accept. A shorter cmsg
    // means the payload is truncated; skip rather than reading past it.
    let ns_min = unsafe { libc::CMSG_LEN(size_of::<libc::timespec>() as u32) } as usize;
    let scm_min = unsafe { libc::CMSG_LEN((3 * size_of::<libc::timespec>()) as u32) } as usize;

    let mut ts = CanTimestamps::default();
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };

    while !cmsg.is_null() {
        let (level, typ, len) = unsafe {
            (
                (*cmsg).cmsg_level,
                (*cmsg).cmsg_type,
                (*cmsg).cmsg_len as usize,
            )
        };
        let data = unsafe { libc::CMSG_DATA(cmsg) };
        match (level, typ) {
            (SOL_SOCKET, libc::SO_TIMESTAMPNS) if len >= ns_min => {
                let timespec = unsafe { ptr::read_unaligned(data.cast::<libc::timespec>()) };
                ts.socket = Some(timespec_to_system_time(timespec));
            }
            (SOL_SOCKET, libc::SO_TIMESTAMPING) if len >= scm_min => {
                // scm_timestamping: [timespec; 3]
                // [0] = RX_SOFTWARE (sw), [1] deprecated (zero), [2] = HW
                let tss = unsafe { ptr::read_unaligned(data.cast::<[libc::timespec; 3]>()) };
                if tss[0].tv_sec != 0 || tss[0].tv_nsec != 0 {
                    ts.sw = Some(timespec_to_system_time(tss[0]));
                }
                if tss[2].tv_sec != 0 || tss[2].tv_nsec != 0 {
                    ts.hw = Some(timespec_to_duration(tss[2]));
                }
            }
            _ => {}
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }

    Ok((n as usize, ts))
}

// ===== CanSocket =====

/// A socket for classic CAN 2.0 devices.
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
pub struct CanSocket(socket2::Socket);

impl CanSocket {
    /// Reads a low-level libc `can_frame` from the socket.
    pub fn read_raw_frame(&self) -> IoResult<libc::can_frame> {
        let mut frame = can_frame_default();
        // SAFETY: `frame` is fully zero-initialised by `can_frame_default`.
        self.as_raw_socket()
            .read_exact(unsafe { as_bytes_mut(&mut frame) })?;
        Ok(frame)
    }

    /// Returns `true` if the bound interface supports hardware receive timestamps.
    ///
    /// Returns `false` if the socket is unbound, the interface does not exist,
    /// or the driver does not implement the ethtool timestamp query.
    pub fn has_hw_timestamps(&self) -> bool {
        hw_timestamps_supported(self.as_raw_fd())
    }
}

impl Socket for CanSocket {
    /// CanSocket reads/writes classic CAN 2.0 frames.
    type FrameType = CanFrame;

    /// Opens the socket by interface index.
    fn open_addr(addr: &CanAddr) -> IoResult<Self> {
        let sock = raw_open_socket(addr)?;
        Ok(Self(sock))
    }

    /// Gets a shared reference to the underlying socket object
    fn as_raw_socket(&self) -> &socket2::Socket {
        &self.0
    }

    /// Gets a mutable reference to the underlying socket object
    fn as_raw_socket_mut(&mut self) -> &mut socket2::Socket {
        &mut self.0
    }

    /// Writes a normal CAN 2.0 frame to the socket.
    fn write_frame<F>(&self, frame: &F) -> IoResult<()>
    where
        F: Into<CanFrame> + AsPtr,
    {
        // SAFETY: the frame's inner `can_frame`/`canfd_frame` is fully
        // initialised — constructors zero the struct via `*_default()`
        // (`mem::zeroed`) before writing fields — so reading every byte
        // (including padding) is sound.
        self.as_raw_socket().write_all(unsafe { frame.as_bytes() })
    }

    /// Reads a normal CAN 2.0 frame from the socket.
    fn read_frame(&self) -> IoResult<CanFrame> {
        let frame = self.read_raw_frame()?;
        Ok(frame.into())
    }

    fn read_frame_with_timestamp(&self) -> IoResult<(CanFrame, SystemTime)> {
        let (frame, ts) = self.read_frame_with_timestamps()?;
        let timestamp = ts.socket.ok_or_else(|| {
            IoError::new(
                IoErrorKind::InvalidData,
                "no SO_TIMESTAMPNS control message received",
            )
        })?;
        Ok((frame, timestamp))
    }

    fn read_frame_with_hw_timestamp(&self) -> IoResult<(CanFrame, Duration)> {
        let (frame, ts) = self.read_frame_with_timestamps()?;
        let hw_ts = ts.hw.ok_or_else(|| {
            IoError::new(
                IoErrorKind::InvalidData,
                "no SO_TIMESTAMPING hardware timestamp received",
            )
        })?;
        Ok((frame, hw_ts))
    }

    fn read_frame_with_timestamps(&self) -> IoResult<(CanFrame, CanTimestamps)> {
        let mut frame = can_frame_default();
        // SAFETY: `frame` is fully zero-initialised by `can_frame_default`.
        let buf = unsafe { as_bytes_mut(&mut frame) };
        let (n, ts) = recvmsg_with_ctrl(self.as_raw_fd(), buf)?;
        if n != CAN_MTU {
            return Err(IoError::from(IoErrorKind::InvalidData));
        }
        Ok((CanFrame::from(frame), ts))
    }
}

// ===== embedded_can I/O traits =====

impl embedded_can::blocking::Can for CanSocket {
    type Frame = CanFrame;
    type Error = Error;

    /// Blocking call to receive the next frame from the bus.
    ///
    /// This block and wait for the next frame to be received from the bus.
    /// If an error frame is received, it will be converted to a `CanError`
    /// and returned as an error.
    fn receive(&mut self) -> Result<Self::Frame> {
        match self.read_frame() {
            Ok(CanFrame::Error(frame)) => Err(frame.into_error().into()),
            Ok(frame) => Ok(frame),
            Err(e) => Err(e.into()),
        }
    }

    /// Blocking transmit of a frame to the bus.
    fn transmit(&mut self, frame: &Self::Frame) -> Result<()> {
        self.write_frame_insist(frame)?;
        Ok(())
    }
}

impl SocketOptions for CanSocket {}

impl embedded_can::nb::Can for CanSocket {
    type Frame = CanFrame;
    type Error = Error;

    /// Non-blocking call to receive the next frame from the bus.
    ///
    /// If an error frame is received, it will be converted to a `CanError`
    /// and returned as an error.
    /// If no frame is available, it returns a `WouldBlck` error.
    fn receive(&mut self) -> nb::Result<Self::Frame, Self::Error> {
        match self.read_frame() {
            Ok(CanFrame::Error(frame)) => Err(Error::from(frame.into_error()).into()),
            Ok(frame) => Ok(frame),
            Err(err) if err.should_retry() => Err(nb::Error::WouldBlock),
            Err(err) => Err(Error::from(err).into()),
        }
    }

    /// Non-blocking transmit of a frame to the bus.
    fn transmit(&mut self, frame: &Self::Frame) -> nb::Result<Option<Self::Frame>, Self::Error> {
        match self.write_frame(frame) {
            Ok(_) => Ok(None),
            Err(err) if err.should_retry() => Err(nb::Error::WouldBlock),
            Err(err) => Err(Error::from(err).into()),
        }
    }
}

// Has no effect: #[deprecated(since = "3.1", note = "Use AsFd::as_fd() instead.")]
impl AsRawFd for CanSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl From<OwnedFd> for CanSocket {
    fn from(fd: OwnedFd) -> Self {
        Self(socket2::Socket::from(fd))
    }
}

impl IntoRawFd for CanSocket {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl AsFd for CanSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl Read for CanSocket {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.0.read(buf)
    }
}

impl Write for CanSocket {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> IoResult<()> {
        self.0.flush()
    }
}

// ===== CanFdSocket =====

/// A socket for CAN FD devices.
///
/// This can transmit and receive CAN 2.0 frames with up to 8-bytes of data,
/// or CAN Flexible Data (FD) frames with up to 64-bytes of data.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct CanFdSocket(socket2::Socket);

impl CanFdSocket {
    // Enable or disable FD mode on a socket.
    fn set_fd_mode(sock: socket2::Socket, enable: bool) -> IoResult<socket2::Socket> {
        let enable = enable as c_int;

        let ret = unsafe {
            libc::setsockopt(
                sock.as_raw_fd(),
                SOL_CAN_RAW,
                CAN_RAW_FD_FRAMES,
                &enable as *const _ as *const c_void,
                size_of::<c_int>() as u32,
            )
        };

        match ret {
            0 => Ok(sock),
            _ => Err(IoError::last_os_error()),
        }
    }

    // Figures out the type of frame from the MTU len read in.
    // This assumes a socket read a raw packet into a `canfd_frame` and
    // now needs to figure out which type it is by the MTU size.
    fn convert_raw_frame(mtu_len: usize, raw_frame: libc::canfd_frame) -> IoResult<CanAnyFrame> {
        match mtu_len {
            CAN_MTU => {
                let mut frame = can_frame_default();
                // SAFETY: `frame` is zero-initialised; `raw_frame` was either
                // filled by the kernel or zero-initialised before partial fill,
                // so all its bytes are valid for read.
                unsafe {
                    as_bytes_mut(&mut frame)[..CAN_MTU]
                        .copy_from_slice(&as_bytes(&raw_frame)[..CAN_MTU]);
                }
                Ok(CanFrame::from(frame).into())
            }
            CANFD_MTU => Ok(CanFdFrame::from(raw_frame).into()),
            _ => Err(IoError::from(IoErrorKind::InvalidData)),
        }
    }

    /// Returns `true` if the bound interface supports hardware receive timestamps.
    ///
    /// Returns `false` if the socket is unbound, the interface does not exist,
    /// or the driver does not implement the ethtool timestamp query.
    pub fn has_hw_timestamps(&self) -> bool {
        hw_timestamps_supported(self.as_raw_fd())
    }

    /// Reads a raw CAN frame from the socket.
    ///
    /// This might be either type of CAN frame, a classic CAN 2.0 frame
    /// or an FD frame.
    pub fn read_raw_frame(&self) -> IoResult<CanRawFrame> {
        let mut fdframe = canfd_frame_default();

        // SAFETY: `fdframe` is fully zero-initialised by `canfd_frame_default`.
        let buf = unsafe { as_bytes_mut(&mut fdframe) };
        match self.as_raw_socket().read(buf)? {
            // If we only get 'can_frame' number of bytes, then the return is,
            // by definition, a can_frame, so we just copy the bytes into the
            // proper type.
            CAN_MTU => {
                let mut frame = can_frame_default();
                // SAFETY: `frame` zero-initialised; `fdframe` likewise (and
                // possibly partially overwritten by the kernel above).
                unsafe {
                    as_bytes_mut(&mut frame)[..CAN_MTU]
                        .copy_from_slice(&as_bytes(&fdframe)[..CAN_MTU]);
                }
                Ok(frame.into())
            }
            CANFD_MTU => Ok(fdframe.into()),
            _ => Err(IoError::last_os_error()),
        }
    }
}

impl Socket for CanFdSocket {
    /// CanFdSocket can read/write classic CAN 2.0 or FD frames.
    type FrameType = CanAnyFrame;

    /// Opens the FD socket by interface index.
    fn open_addr(addr: &CanAddr) -> IoResult<Self> {
        raw_open_socket(addr)
            .and_then(|sock| Self::set_fd_mode(sock, true))
            .map(Self)
    }

    /// Gets a shared reference to the underlying socket object
    fn as_raw_socket(&self) -> &socket2::Socket {
        &self.0
    }

    /// Gets a mutable reference to the underlying socket object
    fn as_raw_socket_mut(&mut self) -> &mut socket2::Socket {
        &mut self.0
    }

    /// Writes any type of CAN frame to the socket.
    fn write_frame<F>(&self, frame: &F) -> IoResult<()>
    where
        F: Into<Self::FrameType> + AsPtr,
    {
        // SAFETY: the frame's inner `can_frame`/`canfd_frame` is fully
        // initialised — constructors zero the struct via `*_default()`
        // (`mem::zeroed`) before writing fields — so reading every byte
        // (including padding) is sound.
        self.as_raw_socket().write_all(unsafe { frame.as_bytes() })
    }

    /// Reads either type of CAN frame from the socket.
    fn read_frame(&self) -> IoResult<CanAnyFrame> {
        let mut fdframe = canfd_frame_default();

        // SAFETY: `fdframe` is fully zero-initialised by `canfd_frame_default`.
        let n = self
            .as_raw_socket()
            .read(unsafe { as_bytes_mut(&mut fdframe) })?;
        Self::convert_raw_frame(n, fdframe)
    }

    fn read_frame_with_timestamp(&self) -> IoResult<(CanAnyFrame, SystemTime)> {
        let (frame, ts) = self.read_frame_with_timestamps()?;
        let sw_ts = ts.socket.ok_or_else(|| {
            IoError::new(
                IoErrorKind::InvalidData,
                "no SO_TIMESTAMPNS control message received",
            )
        })?;
        Ok((frame, sw_ts))
    }

    fn read_frame_with_hw_timestamp(&self) -> IoResult<(CanAnyFrame, Duration)> {
        let (frame, ts) = self.read_frame_with_timestamps()?;
        let hw_ts = ts.hw.ok_or_else(|| {
            IoError::new(
                IoErrorKind::InvalidData,
                "no SO_TIMESTAMPING hardware timestamp received",
            )
        })?;
        Ok((frame, hw_ts))
    }

    fn read_frame_with_timestamps(&self) -> IoResult<(CanAnyFrame, CanTimestamps)> {
        let mut fdframe = canfd_frame_default();
        // SAFETY: `fdframe` is fully zero-initialised by `canfd_frame_default`.
        let buf = unsafe { as_bytes_mut(&mut fdframe) };
        let (n, ts) = recvmsg_with_ctrl(self.as_raw_fd(), buf)?;
        let any_frame = Self::convert_raw_frame(n, fdframe)?;
        Ok((any_frame, ts))
    }
}

impl SocketOptions for CanFdSocket {}

impl embedded_can::blocking::Can for CanFdSocket {
    type Frame = CanAnyFrame;
    type Error = Error;

    /// Blocking call to receive the next frame from the bus.
    ///
    /// This block and wait for the next frame to be received from the bus.
    /// If an error frame is received, it will be converted to a `CanError`
    /// and returned as an error.
    fn receive(&mut self) -> Result<Self::Frame> {
        match self.read_frame() {
            Ok(CanAnyFrame::Error(frame)) => Err(frame.into_error().into()),
            Ok(frame) => Ok(frame),
            Err(e) => Err(e.into()),
        }
    }

    /// Blocking transmit of a frame to the bus.
    fn transmit(&mut self, frame: &Self::Frame) -> Result<()> {
        self.write_frame_insist(frame)?;
        Ok(())
    }
}

impl embedded_can::nb::Can for CanFdSocket {
    type Frame = CanAnyFrame;
    type Error = Error;

    /// Non-blocking call to receive the next frame from the bus.
    ///
    /// If an error frame is received, it will be converted to a `CanError`
    /// and returned as an error.
    /// If no frame is available, it returns a `WouldBlck` error.
    fn receive(&mut self) -> nb::Result<Self::Frame, Self::Error> {
        match self.read_frame() {
            Ok(CanAnyFrame::Error(frame)) => Err(Error::from(frame.into_error()).into()),
            Ok(frame) => Ok(frame),
            Err(err) if err.should_retry() => Err(nb::Error::WouldBlock),
            Err(err) => Err(Error::from(err).into()),
        }
    }

    /// Non-blocking transmit of a frame to the bus.
    fn transmit(&mut self, frame: &Self::Frame) -> nb::Result<Option<Self::Frame>, Self::Error> {
        match self.write_frame(frame) {
            Ok(_) => Ok(None),
            Err(err) if err.should_retry() => Err(nb::Error::WouldBlock),
            Err(err) => Err(Error::from(err).into()),
        }
    }
}

// Has no effect: #[deprecated(since = "3.1", note = "Use AsFd::as_fd() instead.")]
impl AsRawFd for CanFdSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl From<OwnedFd> for CanFdSocket {
    fn from(fd: OwnedFd) -> CanFdSocket {
        Self(socket2::Socket::from(fd))
    }
}

impl TryFrom<CanSocket> for CanFdSocket {
    type Error = IoError;

    fn try_from(sock: CanSocket) -> std::result::Result<Self, Self::Error> {
        let CanSocket(sock2) = sock;
        let sock = CanFdSocket::set_fd_mode(sock2, true)?;
        Ok(CanFdSocket(sock))
    }
}

impl IntoRawFd for CanFdSocket {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl AsFd for CanFdSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl Read for CanFdSocket {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.0.read(buf)
    }
}

impl Write for CanFdSocket {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> IoResult<()> {
        self.0.flush()
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
