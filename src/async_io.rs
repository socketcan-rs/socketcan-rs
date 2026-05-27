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

use crate::{
    frame::AsPtr, timestamp::CanTimestamps, CanAddr, CanAnyFrame, CanFrame, Error, Socket,
    SocketOptions,
};
use futures::{ready, sink::Sink, stream::Stream};
use std::{
    io,
    os::unix::io::{AsRawFd, RawFd},
    pin::Pin,
    task::{Context, Poll},
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

    /// Open a CAN device by kernel interface number.
    pub fn open_if(ifindex: u32) -> io::Result<Self> {
        crate::CanSocket::open_iface(ifindex)?.try_into()
    }

    /// Open a CAN socket bound to a specific address.
    ///
    /// Useful for J1939 / ISO-TP variants of [`CanAddr`].
    pub fn open_addr(addr: &CanAddr) -> io::Result<Self> {
        crate::CanSocket::open_addr(addr)?.try_into()
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
        self.0
            .read_with(|fd| fd.read_frame_with_hw_timestamp())
            .await
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

impl Stream for CanSocket {
    type Item = crate::Result<CanFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Loop because `poll_readable` can spuriously report readiness without
        // a frame actually being available (e.g. after a sibling reader
        // consumed the queued frame); in that case the inner `read_frame`
        // returns `WouldBlock` and we re-arm by polling again.
        loop {
            ready!(self.0.poll_readable(cx))?;
            match self.0.get_ref().read_frame() {
                Ok(frame) => return Poll::Ready(Some(Ok(frame))),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Poll::Ready(Some(Err(e.into()))),
            }
        }
    }
}

impl Sink<CanFrame> for CanSocket {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        ready!(self.0.poll_writable(cx))?;
        Poll::Ready(Ok(()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        // Nothing to flush; the underlying fd closes on drop.
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: CanFrame) -> crate::Result<()> {
        // `poll_ready` already cleared write-readiness, so a single
        // non-blocking write is sufficient.
        self.0.get_ref().write_frame(&item)?;
        Ok(())
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

    /// Open a CAN device by kernel interface number.
    pub fn open_if(ifindex: u32) -> io::Result<Self> {
        crate::CanFdSocket::open_iface(ifindex)?.try_into()
    }

    /// Open a CAN socket bound to a specific address.
    ///
    /// Useful for J1939 / ISO-TP variants of [`CanAddr`].
    pub fn open_addr(addr: &CanAddr) -> io::Result<Self> {
        crate::CanFdSocket::open_addr(addr)?.try_into()
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
        self.0
            .read_with(|fd| fd.read_frame_with_hw_timestamp())
            .await
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

impl Stream for CanFdSocket {
    type Item = crate::Result<CanAnyFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            ready!(self.0.poll_readable(cx))?;
            match self.0.get_ref().read_frame() {
                Ok(frame) => return Poll::Ready(Some(Ok(frame))),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Poll::Ready(Some(Err(e.into()))),
            }
        }
    }
}

impl Sink<CanAnyFrame> for CanFdSocket {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        ready!(self.0.poll_writable(cx))?;
        Poll::Ready(Ok(()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
        // Nothing to flush; the underlying fd closes on drop.
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: CanAnyFrame) -> crate::Result<()> {
        // `poll_ready` already cleared write-readiness, so a single
        // non-blocking write is sufficient.
        self.0.get_ref().write_frame(&item)?;
        Ok(())
    }
}
