use crate::frame::{can_frame_default, CanFrame};
use socketcan_raw::{
    as_bytes_mut, raw_open_socket, AsPtr, CanAddr, Error, IoResult, Result, ShouldRetry as _,
    Socket, SocketOptions,
};
use std::{
    io::{Read, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd},
};

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
pub struct CanSocket(pub(crate) socket2::Socket);

impl CanSocket {
    /// Reads a low-level libc `can_frame` from the socket.
    pub fn read_raw_frame(&self) -> IoResult<libc::can_frame> {
        let mut frame = can_frame_default();
        self.as_raw_socket().read_exact(as_bytes_mut(&mut frame))?;
        Ok(frame)
    }
}

impl Socket for CanSocket {
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

    /// CanSocket reads/writes classic CAN 2.0 frames.
    type FrameType = CanFrame;

    /// Reads a normal CAN 2.0 frame from the socket.
    fn read_frame(&self) -> IoResult<CanFrame> {
        let frame = self.read_raw_frame()?;
        Ok(frame.into())
    }

    /// Writes a normal CAN 2.0 frame to the socket.
    fn write_frame<F>(&self, frame: &F) -> IoResult<()>
    where
        F: Into<Self::FrameType> + AsPtr,
    {
        self.as_raw_socket().write_all(frame.as_bytes())
    }
}

// ===== embedded_can I/O traits =====

impl embedded_can::blocking::Can for CanSocket {
    type Frame = CanFrame;
    type Error = Error;

    /// Blocking transmit of a frame to the bus.
    fn transmit(&mut self, frame: &Self::Frame) -> Result<()> {
        self.write_frame_insist(frame)?;
        Ok(())
    }

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
}

impl SocketOptions for CanSocket {}

impl embedded_can::nb::Can for CanSocket {
    type Frame = CanFrame;
    type Error = Error;

    /// Non-blocking transmit of a frame to the bus.
    fn transmit(&mut self, frame: &Self::Frame) -> nb::Result<Option<Self::Frame>, Self::Error> {
        match self.write_frame(frame) {
            Ok(_) => Ok(None),
            Err(err) if err.should_retry() => Err(nb::Error::WouldBlock),
            Err(err) => Err(Error::from(err).into()),
        }
    }

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
