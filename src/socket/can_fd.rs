use libc::{CANFD_MTU, CAN_MTU, CAN_RAW_FD_FRAMES, SOL_CAN_RAW};
use socketcan_raw::{
    as_bytes, as_bytes_mut, raw_open_socket, AsPtr, CanAddr, Error, IoError, IoResult, Result,
    ShouldRetry as _, Socket, SocketOptions,
};
use std::{
    io::{Read, Write},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd},
        raw::{c_int, c_void},
    },
};

use crate::{
    frame::{
        can_frame_default, canfd_frame_default, CanAnyFrame, CanFdFrame, CanFrame, CanRawFrame,
    },
    socket::can::CanSocket,
};

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

    /// Reads a raw CAN frame from the socket.
    ///
    /// This might be either type of CAN frame, a classic CAN 2.0 frame
    /// or an FD frame.
    pub fn read_raw_frame(&self) -> IoResult<CanRawFrame> {
        let mut fdframe = canfd_frame_default();

        match self.as_raw_socket().read(as_bytes_mut(&mut fdframe))? {
            // If we only get 'can_frame' number of bytes, then the return is,
            // by definition, a can_frame, so we just copy the bytes into the
            // proper type.
            CAN_MTU => {
                let mut frame = can_frame_default();
                as_bytes_mut(&mut frame)[..CAN_MTU].copy_from_slice(&as_bytes(&fdframe)[..CAN_MTU]);
                Ok(frame.into())
            }
            CANFD_MTU => Ok(fdframe.into()),
            _ => Err(IoError::last_os_error()),
        }
    }
}

impl Socket for CanFdSocket {
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

    /// CanFdSocket can read/write classic CAN 2.0 or FD frames.
    type FrameType = CanAnyFrame;

    /// Reads either type of CAN frame from the socket.
    fn read_frame(&self) -> IoResult<CanAnyFrame> {
        let mut fdframe = canfd_frame_default();

        match self.as_raw_socket().read(as_bytes_mut(&mut fdframe))? {
            // If we only get 'can_frame' number of bytes, then the return is,
            // by definition, a can_frame, so we just copy the bytes into the
            // proper type.
            CAN_MTU => {
                let mut frame = can_frame_default();
                as_bytes_mut(&mut frame)[..CAN_MTU].copy_from_slice(&as_bytes(&fdframe)[..CAN_MTU]);
                Ok(CanFrame::from(frame).into())
            }
            CANFD_MTU => Ok(CanFdFrame::from(fdframe).into()),
            _ => Err(IoError::last_os_error()),
        }
    }

    /// Writes any type of CAN frame to the socket.
    fn write_frame<F>(&self, frame: &F) -> IoResult<()>
    where
        F: Into<Self::FrameType> + AsPtr,
    {
        self.as_raw_socket().write_all(frame.as_bytes())
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
