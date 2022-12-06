use crate::{
    err::{CanSocketOpenError, ConstructionError},
    frame::ERR_MASK,
    util::{set_socket_option, set_socket_option_mult},
    CanAnyFrame, CanFdFrame, CanNormalFrame,
};
use libc::{
    bind, close, fcntl, read, setsockopt, sockaddr, socket, suseconds_t, time_t, timeval, write,
    EINPROGRESS, F_GETFL, F_SETFL, O_NONBLOCK, SOCK_RAW, SOL_SOCKET, SO_RCVTIMEO, SO_SNDTIMEO,
};
use nix::net::if_::if_nametoindex;
use std::{
    convert::TryFrom,
    fmt, io,
    mem::size_of,
    os::{
        raw::{c_int, c_short, c_uint, c_void},
        unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
    },
    time,
};

// constants stolen from C headers
const AF_CAN: c_int = 29;
const PF_CAN: c_int = 29;
const CAN_RAW: c_int = 1;
const SOL_CAN_BASE: c_int = 100;
const SOL_CAN_RAW: c_int = SOL_CAN_BASE + CAN_RAW;

const CAN_RAW_FILTER: c_int = 1;
const CAN_RAW_ERR_FILTER: c_int = 2;
const CAN_RAW_LOOPBACK: c_int = 3;
const CAN_RAW_RECV_OWN_MSGS: c_int = 4;
const CAN_RAW_FD_FRAMES: c_int = 5;
const CAN_RAW_JOIN_FILTERS: c_int = 6;

// CAN normal frame
pub const CAN_MTU: usize = 16;

/// CAN FD frame
pub const CANFD_MTU: usize = 72;

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

fn c_timeval_new(t: time::Duration) -> timeval {
    timeval {
        tv_sec: t.as_secs() as time_t,
        tv_usec: t.subsec_micros() as suseconds_t,
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

/*
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
            bind_rv = bind(sock_fd,
                           sockaddr_ptr as *const sockaddr,
                           size_of::<CanAddr>() as u32);
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
        let mut frame = CanFrame::default();

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

        let mut ts = timespec { tv_sec: 0, tv_nsec: 0 };
        let rval = unsafe {
            libc::ioctl(self.fd, SIOCGSTAMPNS as c_ulong, &mut ts as *mut timespec)
        };

        if rval == -1 {
            return Err(io::Error::last_os_error());
        }

        Ok((frame, system_time_from_timespec(ts)))
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
        CanSocket { fd, }
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
*/

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

    let sock_fd = unsafe { socket(PF_CAN, SOCK_RAW, CAN_RAW) };

    if sock_fd == -1 {
        return Err(CanSocketOpenError::from(io::Error::last_os_error()));
    }

    let bind_rv = unsafe {
        let sockaddr_ptr = &addr as *const CanAddr;
        bind(
            sock_fd,
            sockaddr_ptr as *const sockaddr,
            size_of::<CanAddr>() as u32,
        )
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
        setsockopt(
            socket_fd,
            SOL_CAN_RAW,
            CAN_RAW_FD_FRAMES,
            opt_ptr as *const c_void,
            size_of::<c_int>() as u32,
        )
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
    where
        Self: Sized,
    {
        let if_index = if_nametoindex(ifname)?;
        Self::open_if(if_index)
    }

    /// Open CAN device by interface number.
    ///
    /// Opens a CAN device by kernel interface number.
    fn open_if(if_index: c_uint) -> Result<Self, CanSocketOpenError>
    where
        Self: Sized;

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
            setsockopt(
                self.as_raw_fd(),
                SOL_SOCKET,
                SO_RCVTIMEO,
                tv_ptr as *const c_void,
                size_of::<timeval>() as u32,
            )
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
            setsockopt(
                self.as_raw_fd(),
                SOL_SOCKET,
                SO_SNDTIMEO,
                tv_ptr as *const c_void,
                size_of::<timeval>() as u32,
            )
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
            unsafe {
                setsockopt(
                    self.as_raw_fd(),
                    SOL_CAN_RAW,
                    CAN_RAW_FILTER,
                    0 as *const c_void,
                    0,
                )
            }
        } else {
            unsafe {
                let filters_ptr = &filters[0] as *const CanFilter;
                setsockopt(
                    self.as_raw_fd(),
                    SOL_CAN_RAW,
                    CAN_RAW_FILTER,
                    filters_ptr as *const c_void,
                    (size_of::<CanFilter>() * filters.len()) as u32,
                )
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
            setsockopt(
                self.as_raw_fd(),
                SOL_CAN_RAW,
                CAN_RAW_ERR_FILTER,
                (&mask as *const u32) as *const c_void,
                size_of::<u32>() as u32,
            )
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

    /// Sets filters on the socket.
    ///
    /// CAN packages received by SocketCAN are matched against these filters,
    /// only matching packets are returned by the interface.
    ///
    /// See `CanFilter` for details on how filtering works. By default, all
    /// single filter matching all incoming frames is installed.
    fn set_filters(&self, filters: &[CanFilter]) -> io::Result<()> {
        set_socket_option_mult(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_FILTER, filters)
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
        let loopback: c_int = if enabled { 1 } else { 0 };
        set_socket_option(self.as_raw_fd(), SOL_CAN_RAW, CAN_RAW_LOOPBACK, &loopback)
    }

    /// Enable or disable receiving of own frames.
    ///
    /// When loopback is enabled, this settings controls if CAN frames sent
    /// are received back immediately by sender. Default is off.
    fn set_recv_own_msgs(&self, enabled: bool) -> io::Result<()> {
        let recv_own_msgs: c_int = if enabled { 1 } else { 0 };
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
        let join_filters: c_int = if enabled { 1 } else { 0 };
        set_socket_option(
            self.as_raw_fd(),
            SOL_CAN_RAW,
            CAN_RAW_JOIN_FILTERS,
            &join_filters,
        )
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
            read(
                self.fd,
                frame_ptr as *mut c_void,
                size_of::<CanNormalFrame>(),
            )
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
            .and_then(|sock_fd| {
                set_fd_mode(sock_fd, true).map_err(|io_err| CanSocketOpenError::IOError(io_err))
            })
            .map(|sock_fd| Self { fd: sock_fd })
    }

    fn write_frame(&self, frame: &CanAnyFrame) -> io::Result<()> {
        match frame {
            CanAnyFrame::Normal(frame) => raw_write_frame(self.fd, frame),
            CanAnyFrame::Fd(fd_frame) => raw_write_frame(self.fd, fd_frame),
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
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        "BUG in read_frame: cannot convert to CanNormalFrame",
                    )
                }),

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

// ===== CanFilter =====

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
