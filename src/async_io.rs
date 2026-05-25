// socketcan/src/socket/async_io.rs
//
// Implements sockets for CANbus 2.0 and FD for SocketCAN on Linux.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Bindings to async-io for CANbus 2.0 and FD sockets using SocketCAN on Linux.

use crate::{frame::AsPtr, timestamp::CanTimestamps, CanAnyFrame, CanFrame, Socket, SocketOptions};
use std::{
    io,
    os::unix::io::{AsRawFd, RawFd},
    time::{Duration, SystemTime},
};

#[cfg(any(feature = "async-io", feature = "async-std"))]
use async_io::Async;

#[cfg(all(
    feature = "smol",
    not(any(feature = "async-io", feature = "async-std"))
))]
use smol::Async;

/////////////////////////////////////////////////////////////////////////////

/// An asynchronous CAN socket for use with `async-io`.
#[derive(Debug)]
pub struct CanSocket(Async<crate::CanSocket>);

impl CanSocket {
    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "can0", "vcan0", or "socan0".
    pub fn open(ifname: &str) -> io::Result<Self> {
        crate::CanSocket::open(ifname)?.try_into()
    }

    /// Writes a frame to the socket asynchronously.
    pub async fn write_frame<F>(&self, frame: &F) -> io::Result<()>
    where
        F: Into<CanFrame> + AsPtr,
    {
        self.0.write_with(|fd| fd.write_frame(frame)).await
    }

    /// Reads a frame from the socket asynchronously.
    pub async fn read_frame(&self) -> io::Result<CanFrame> {
        self.0.read_with(|fd| fd.read_frame()).await
    }

    /// Returns `true` if the bound interface supports hardware receive timestamps.
    pub fn has_hw_timestamps(&self) -> bool {
        self.0.get_ref().has_hw_timestamps()
    }

    /// Read a CAN frame and its socket-layer arrival timestamp asynchronously.
    ///
    /// Requires [`SocketOptions::set_recv_timestamp`] to be called first.
    pub async fn read_frame_with_timestamp(&self) -> io::Result<(CanFrame, SystemTime)> {
        self.0.read_with(|fd| fd.read_frame_with_timestamp()).await
    }

    /// Read a CAN frame and all available timestamps asynchronously.
    ///
    /// Timestamp fields are `None` for modes not enabled on the socket.
    pub async fn read_frame_with_timestamps(&self) -> io::Result<(CanFrame, CanTimestamps)> {
        self.0.read_with(|fd| fd.read_frame_with_timestamps()).await
    }

    /// Read a CAN frame and its raw hardware clock timestamp asynchronously.
    ///
    /// Requires [`SocketOptions::set_timestamping`] with
    /// `SOF_TIMESTAMPING_RX_HARDWARE | SOF_TIMESTAMPING_OPT_CMSG` to be called first.
    pub async fn read_frame_with_hw_timestamp(&self) -> io::Result<(CanFrame, Duration)> {
        self.0.read_with(|fd| fd.read_frame_with_hw_timestamp()).await
    }
}

impl SocketOptions for CanSocket {}

impl TryFrom<crate::CanSocket> for CanSocket {
    type Error = io::Error;

    fn try_from(sock: crate::CanSocket) -> Result<Self, Self::Error> {
        Ok(Self(Async::new(sock)?))
    }
}

impl AsRawFd for CanSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

/////////////////////////////////////////////////////////////////////////////

/// An asynchronous CAN socket for use with `async-io`.
#[derive(Debug)]
pub struct CanFdSocket(Async<crate::CanFdSocket>);

impl CanFdSocket {
    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "can0", "vcan0", or "socan0".
    pub fn open(ifname: &str) -> io::Result<Self> {
        crate::CanFdSocket::open(ifname)?.try_into()
    }

    /// Writes a frame to the socket asynchronously.
    pub async fn write_frame<F>(&self, frame: &F) -> io::Result<()>
    where
        F: Into<CanAnyFrame> + AsPtr,
    {
        self.0.write_with(|fd| fd.write_frame(frame)).await
    }

    /// Reads a frame from the socket asynchronously.
    pub async fn read_frame(&self) -> io::Result<CanAnyFrame> {
        self.0.read_with(|fd| fd.read_frame()).await
    }

    /// Returns `true` if the bound interface supports hardware receive timestamps.
    pub fn has_hw_timestamps(&self) -> bool {
        self.0.get_ref().has_hw_timestamps()
    }

    /// Read a CAN frame and its socket-layer arrival timestamp asynchronously.
    ///
    /// Requires [`SocketOptions::set_recv_timestamp`] to be called first.
    pub async fn read_frame_with_timestamp(&self) -> io::Result<(CanAnyFrame, SystemTime)> {
        self.0.read_with(|fd| fd.read_frame_with_timestamp()).await
    }

    /// Read a CAN frame and all available timestamps asynchronously.
    ///
    /// Timestamp fields are `None` for modes not enabled on the socket.
    pub async fn read_frame_with_timestamps(&self) -> io::Result<(CanAnyFrame, CanTimestamps)> {
        self.0.read_with(|fd| fd.read_frame_with_timestamps()).await
    }

    /// Read a CAN frame and its raw hardware clock timestamp asynchronously.
    ///
    /// Requires [`SocketOptions::set_timestamping`] with
    /// `SOF_TIMESTAMPING_RX_HARDWARE | SOF_TIMESTAMPING_OPT_CMSG` to be called first.
    pub async fn read_frame_with_hw_timestamp(&self) -> io::Result<(CanAnyFrame, Duration)> {
        self.0.read_with(|fd| fd.read_frame_with_hw_timestamp()).await
    }
}

impl SocketOptions for CanFdSocket {}

impl TryFrom<crate::CanFdSocket> for CanFdSocket {
    type Error = io::Error;

    fn try_from(sock: crate::CanFdSocket) -> Result<Self, Self::Error> {
        Ok(Self(Async::new(sock)?))
    }
}

impl AsRawFd for CanFdSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}
