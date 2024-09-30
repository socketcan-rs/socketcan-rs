//! # tokio-socketcan
//!
//! Connective plumbing between the socketcan crate
//! and the tokio asynchronous I/O system
//!
//! # Usage
//!
//! The [socketcan](https://docs.rs/socketcan/1.7.0/socketcan/)
//! crate's documentation is valuable as the api used by
//! tokio-socketcan is largely identical to the socketcan one.
//!
//! An example echo server:
//!
//! ```no_run
//! use futures_util::stream::StreamExt;
//! use socketcan::{Error, tokio::CanSocket};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Error> {
//!     let mut socket_rx = CanSocket::open("vcan0")?;
//!     let socket_tx = CanSocket::open("vcan0")?;
//!
//!     while let Some(Ok(frame)) = socket_rx.next().await {
//!         socket_tx.write_frame(frame).await;
//!     }
//!     Ok(())
//! }
//! ```
use crate::{
    socket::TimestampingMode, CanAddr, CanAnyFrame, CanFdFrame, CanFrame, Error, IoResult, Result,
    Socket, SocketOptions,
};
use futures::{prelude::*, ready, task::Context};
use std::{
    io::{Read, Write},
    os::unix::{
        io::{AsRawFd, OwnedFd},
        prelude::RawFd,
    },
    pin::Pin,
    task::Poll,
    time::SystemTime,
};
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// An asynchronous I/O wrapped CanSocket
#[derive(Debug)]
pub struct AsyncCanSocket<T: Socket>(AsyncFd<T>);

impl<T: Socket + From<OwnedFd>> AsyncCanSocket<T> {
    /// Open a named CAN device such as "can0, "vcan0", etc
    pub fn open(ifname: &str) -> IoResult<Self> {
        let sock = T::open(ifname)?;
        sock.set_nonblocking(true)?;
        Ok(Self(AsyncFd::new(sock)?))
    }

    /// Open CAN device by kernel interface number
    pub fn open_if(ifindex: u32) -> IoResult<Self> {
        let sock = T::open_iface(ifindex)?;
        sock.set_nonblocking(true)?;
        Ok(Self(AsyncFd::new(sock)?))
    }

    /// Open a CAN socket by address
    pub fn open_addr(addr: &CanAddr) -> IoResult<Self> {
        let sock = T::open_addr(addr)?;
        sock.set_nonblocking(true)?;
        Ok(Self(AsyncFd::new(sock)?))
    }
}

impl<T: Socket> SocketOptions for AsyncCanSocket<T> {}

impl<T: Socket> AsRawFd for AsyncCanSocket<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

/// Asynchronous Can Socket
pub type CanSocket = AsyncCanSocket<crate::CanSocket>;

impl CanSocket {
    /// Write a CAN frame to the socket asynchronously
    pub async fn write_frame(&self, frame: CanFrame) -> IoResult<()> {
        self.0
            .async_io(Interest::WRITABLE, |inner| inner.write_frame(&frame))
            .await
    }

    /// Read a CAN frame from the socket asynchronously
    pub async fn read_frame(&self) -> IoResult<CanFrame> {
        self.0
            .async_io(Interest::READABLE, |inner| inner.read_frame())
            .await
    }
}

impl Stream for CanSocket {
    type Item = Result<CanFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            let mut ready_guard = ready!(self.0.poll_read_ready(cx))?;
            match ready_guard.try_io(|inner| inner.get_ref().read_frame()) {
                Ok(result) => return Poll::Ready(Some(result.map_err(|e| e.into()))),
                Err(_would_block) => continue,
            }
        }
    }
}

impl Sink<CanFrame> for CanSocket {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let _ = ready!(self.0.poll_write_ready(cx))?;
        Poll::Ready(Ok(()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut ready_guard = ready!(self.0.poll_write_ready(cx))?;
        ready_guard.clear_ready();
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: CanFrame) -> Result<()> {
        self.0.get_ref().write_frame_insist(&item)?;
        Ok(())
    }
}

impl AsyncRead for CanSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready_mut(cx))?;

            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|inner| inner.get_mut().read(unfilled)) {
                Ok(Ok(len)) => {
                    buf.advance(len);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for CanSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Asynchronous Can Socket with timestamps
pub type CanSocketTimestamp = AsyncCanSocket<crate::CanSocketTimestamp>;

impl CanSocketTimestamp {
    /// Opens a socket with the specified [CanAddr] and [TimestampingMode]
    ///
    /// This is the same like `open_addr` but allows specifing a `mode`.
    pub fn open_with_timestamping_mode(addr: &CanAddr, mode: TimestampingMode) -> IoResult<Self> {
        let sock = crate::CanSocketTimestamp::open_with_timestamping_mode(addr, mode)?;
        Ok(Self(AsyncFd::new(sock)?))
    }

    /// Write a CAN frame to the socket asynchronously
    pub async fn write_frame(&self, frame: CanFrame) -> IoResult<()> {
        self.0
            .async_io(Interest::WRITABLE, |inner| inner.write_frame(&frame))
            .await
    }

    /// Read a CAN frame from the socket asynchronously
    pub async fn read_frame(&self) -> IoResult<(CanFrame, Option<SystemTime>)> {
        self.0
            .async_io(Interest::READABLE, |inner| inner.read_frame())
            .await
    }
}

impl Stream for CanSocketTimestamp {
    type Item = Result<(CanFrame, Option<SystemTime>)>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            let mut ready_guard = ready!(self.0.poll_read_ready(cx))?;
            match ready_guard.try_io(|inner| inner.get_ref().read_frame()) {
                Ok(result) => return Poll::Ready(Some(result.map_err(|e| e.into()))),
                Err(_would_block) => continue,
            }
        }
    }
}

impl Sink<CanFrame> for CanSocketTimestamp {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let _ = ready!(self.0.poll_write_ready(cx))?;
        Poll::Ready(Ok(()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut ready_guard = ready!(self.0.poll_write_ready(cx))?;
        ready_guard.clear_ready();
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: CanFrame) -> Result<()> {
        self.0.get_ref().write_frame_insist(&item)?;
        Ok(())
    }
}

impl AsyncRead for CanSocketTimestamp {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready_mut(cx))?;

            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|inner| inner.get_mut().read(unfilled)) {
                Ok(Ok(len)) => {
                    buf.advance(len);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for CanSocketTimestamp {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }
}

/// An Asynchronous CAN FD Socket
pub type CanFdSocket = AsyncCanSocket<crate::CanFdSocket>;

impl CanFdSocket {
    /// Write a CAN FD frame to the socket asynchronously
    pub async fn write_frame(&self, frame: CanFdFrame) -> IoResult<()> {
        self.0
            .async_io(Interest::WRITABLE, |inner| inner.write_frame(&frame))
            .await
    }

    /// Reads a CAN FD frame from the socket asynchronously
    pub async fn read_frame(&self) -> IoResult<CanAnyFrame> {
        self.0
            .async_io(Interest::READABLE, |inner| inner.read_frame())
            .await
    }
}

impl Stream for CanFdSocket {
    type Item = Result<CanAnyFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            let mut ready_guard = ready!(self.0.poll_read_ready(cx))?;
            match ready_guard.try_io(|inner| inner.get_ref().read_frame()) {
                Ok(result) => return Poll::Ready(Some(result.map_err(|e| e.into()))),
                Err(_would_block) => continue,
            }
        }
    }
}

impl Sink<CanFdFrame> for CanFdSocket {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let _ = ready!(self.0.poll_write_ready(cx))?;
        Poll::Ready(Ok(()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut ready_guard = ready!(self.0.poll_write_ready(cx))?;
        ready_guard.clear_ready();
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: CanFdFrame) -> Result<()> {
        self.0.get_ref().write_frame_insist(&item)?;
        Ok(())
    }
}

impl AsyncRead for CanFdSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready_mut(cx))?;

            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|inner| inner.get_mut().read(unfilled)) {
                Ok(Ok(len)) => {
                    buf.advance(len);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for CanFdSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }
}

/// An Asynchronous CAN FD Socket with timestamps
pub type CanFdSocketTimestamp = AsyncCanSocket<crate::CanFdSocketTimestamp>;

impl CanFdSocketTimestamp {
    /// Opens a socket with the specified [CanAddr] and [TimestampingMode]
    ///
    /// This is the same like `open_addr` but allows specifing a `mode`.
    pub fn open_with_timestamping_mode(addr: &CanAddr, mode: TimestampingMode) -> IoResult<Self> {
        let sock = crate::CanFdSocketTimestamp::open_with_timestamping_mode(addr, mode)?;
        Ok(Self(AsyncFd::new(sock)?))
    }

    /// Write a CAN FD frame to the socket asynchronously
    pub async fn write_frame(&self, frame: CanFdFrame) -> IoResult<()> {
        self.0
            .async_io(Interest::WRITABLE, |inner| inner.write_frame(&frame))
            .await
    }

    /// Reads a CAN FD frame from the socket asynchronously
    pub async fn read_frame(&self) -> IoResult<(CanAnyFrame, Option<SystemTime>)> {
        self.0
            .async_io(Interest::READABLE, |inner| inner.read_frame())
            .await
    }
}

impl Stream for CanFdSocketTimestamp {
    type Item = Result<(CanAnyFrame, Option<SystemTime>)>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            let mut ready_guard = ready!(self.0.poll_read_ready(cx))?;
            match ready_guard.try_io(|inner| inner.get_ref().read_frame()) {
                Ok(result) => return Poll::Ready(Some(result.map_err(|e| e.into()))),
                Err(_would_block) => continue,
            }
        }
    }
}

impl Sink<CanFdFrame> for CanFdSocketTimestamp {
    type Error = Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let _ = ready!(self.0.poll_write_ready(cx))?;
        Poll::Ready(Ok(()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut ready_guard = ready!(self.0.poll_write_ready(cx))?;
        ready_guard.clear_ready();
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: CanFdFrame) -> Result<()> {
        self.0.get_ref().write_frame_insist(&item)?;
        Ok(())
    }
}

impl AsyncRead for CanFdSocketTimestamp {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready_mut(cx))?;

            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|inner| inner.get_mut().read(unfilled)) {
                Ok(Ok(len)) => {
                    buf.advance(len);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for CanFdSocketTimestamp {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready_mut(cx))?;

            match guard.try_io(|inner| inner.get_mut().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Poll::Ready(Ok(()))
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(feature = "vcan_tests")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        frame::{can_frame_default, AsPtr},
        CanFrame, Frame, IoErrorKind, StandardId,
    };
    use embedded_can::Frame as EmbeddedFrame;
    use futures::{select, try_join};
    use futures_timer::Delay;
    use serial_test::serial;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TIMEOUT: Duration = Duration::from_millis(100);

    /// Receive a frame from the CanSocket
    async fn recv_frame(socket: CanSocket) -> Result<CanSocket> {
        select!(
            frame = socket.read_frame().fuse() => if let Ok(_frame) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(TIMEOUT).fuse() => Err(IoErrorKind::TimedOut.into()),
        )
    }

    /// Receive a frame from the CanSocket using the `Stream` trait
    async fn recv_frame_with_stream(mut socket: CanSocket) -> Result<CanSocket> {
        select!(
            frame = socket.next().fuse() => if let Some(_frame) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(TIMEOUT).fuse() => Err(IoErrorKind::TimedOut.into()),
        )
    }

    /// Receive a frame from the CanSocket using the `tokio::io::AsyncRead` trait
    async fn recv_frame_with_async_read(mut socket: CanSocket) -> Result<CanSocket> {
        let mut frame = can_frame_default();
        select!(
            frame = socket.read_exact(crate::as_bytes_mut(&mut frame)).fuse() => if let Ok(_bytes_read) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(TIMEOUT).fuse() => Err(IoErrorKind::TimedOut.into()),
        )
    }

    /// Write a test frame to the CanSocket
    async fn write_frame(socket: &CanSocket) -> Result<()> {
        let test_frame = CanFrame::new(StandardId::new(0x1).unwrap(), &[0]).unwrap();
        socket.write_frame(test_frame).await?;
        Ok(())
    }

    /// Write a test frame to the CanSocket using the `tokio::io::AsyncWrite` trait
    async fn write_frame_with_async_write(socket: &mut CanSocket) -> Result<()> {
        let test_frame = CanFrame::new(StandardId::new(0x1).unwrap(), &[0]).unwrap();
        socket.write(test_frame.as_bytes()).await?;
        Ok(())
    }

    /// Receive a frame from the CanFdSocket
    async fn recv_frame_fd(socket: CanFdSocket) -> Result<CanFdSocket> {
        select!(
            frame = socket.read_frame().fuse() => if let Ok(_frame) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(TIMEOUT).fuse() => Err(IoErrorKind::TimedOut.into()),
        )
    }

    /// Receive a frame from the CanFdSocket using the `Stream` trait
    async fn recv_frame_fd_with_stream(mut socket: CanFdSocket) -> Result<CanFdSocket> {
        select!(
            frame = socket.next().fuse() => if let Some(_frame) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(TIMEOUT).fuse() => Err(IoErrorKind::TimedOut.into()),
        )
    }

    /// Receive a frame from the CanFdSocket using the `tokio::io::AsyncWrite` trait
    async fn recv_frame_fd_with_async_read(mut socket: CanFdSocket) -> Result<CanFdSocket> {
        let mut frame = can_frame_default();
        select!(
            frame = socket.read_exact(crate::as_bytes_mut(&mut frame)).fuse() => if let Ok(_bytes_read) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(TIMEOUT).fuse() => Err(IoErrorKind::TimedOut.into()),
        )
    }

    /// Write a test frame to the CanSocket
    async fn write_frame_fd(socket: &CanFdSocket) -> Result<()> {
        let test_frame =
            CanFdFrame::new(StandardId::new(0x1).unwrap(), &[0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        socket.write_frame(test_frame).await?;
        Ok(())
    }

    /// Write a test frame to the CanSocket using the `tokio::io::AsyncWrite` trait
    async fn write_frame_fd_with_async_write(socket: &mut CanFdSocket) -> Result<()> {
        let test_frame = CanFdFrame::new(StandardId::new(0x1).unwrap(), &[0]).unwrap();
        socket.write(test_frame.as_bytes()).await?;
        Ok(())
    }

    #[serial]
    #[tokio::test]
    async fn test_receive() -> Result<()> {
        let socket1 = CanSocket::open("vcan0").unwrap();
        let socket2 = CanSocket::open("vcan0").unwrap();

        let send_frames = future::try_join(write_frame(&socket1), write_frame(&socket1));

        let recv_frames = async {
            let socket2 = recv_frame(socket2).await?;
            let _socket2 = recv_frame(socket2).await;
            Ok(())
        };

        try_join!(recv_frames, send_frames)?;

        Ok(())
    }

    #[serial]
    #[tokio::test]
    async fn test_receive_with_stream() -> Result<()> {
        let socket1 = CanSocket::open("vcan0").unwrap();
        let socket2 = CanSocket::open("vcan0").unwrap();

        let send_frames = future::try_join(write_frame(&socket1), write_frame(&socket1));

        let recv_frames = async {
            let socket2 = recv_frame_with_stream(socket2).await?;
            let _socket2 = recv_frame_with_stream(socket2).await;
            Ok(())
        };

        try_join!(recv_frames, send_frames)?;

        Ok(())
    }

    #[serial]
    #[tokio::test]
    async fn test_asyncread_and_asyncwrite() -> Result<()> {
        let mut socket1 = CanSocket::open("vcan0").unwrap();
        let socket2 = CanSocket::open("vcan0").unwrap();

        let send_frames = write_frame_with_async_write(&mut socket1);

        let recv_frames = async {
            let _socket2 = recv_frame_with_async_read(socket2).await?;
            Ok(())
        };

        try_join!(recv_frames, send_frames)?;

        Ok(())
    }

    #[serial]
    #[tokio::test]
    async fn test_receive_can_fd() -> Result<()> {
        let socket1 = CanFdSocket::open("vcan0").unwrap();
        let socket2 = CanFdSocket::open("vcan0").unwrap();

        let send_frames = future::try_join(write_frame_fd(&socket1), write_frame_fd(&socket1));

        let recv_frames = async {
            let socket2 = recv_frame_fd(socket2).await?;
            let _socket2 = recv_frame_fd(socket2).await;
            Ok(())
        };

        try_join!(recv_frames, send_frames)?;

        Ok(())
    }

    #[serial]
    #[tokio::test]
    async fn test_receive_can_fd_with_stream() -> Result<()> {
        let socket1 = CanFdSocket::open("vcan0").unwrap();
        let socket2 = CanFdSocket::open("vcan0").unwrap();

        let send_frames = future::try_join(write_frame_fd(&socket1), write_frame_fd(&socket1));

        let recv_frames = async {
            let socket2 = recv_frame_fd_with_stream(socket2).await?;
            let _socket2 = recv_frame_fd_with_stream(socket2).await;
            Ok(())
        };

        try_join!(recv_frames, send_frames)?;

        Ok(())
    }

    #[serial]
    #[tokio::test]
    async fn test_asyncread_and_asyncwrite_fd() -> Result<()> {
        let mut socket1 = CanFdSocket::open("vcan0").unwrap();
        let socket2 = CanFdSocket::open("vcan0").unwrap();

        let send_frames = write_frame_fd_with_async_write(&mut socket1);

        let recv_frames = async {
            let _socket2 = recv_frame_fd_with_async_read(socket2).await?;
            Ok(())
        };

        try_join!(recv_frames, send_frames)?;

        Ok(())
    }

    #[serial]
    #[tokio::test]
    async fn test_sink_stream() -> Result<()> {
        let socket1 = CanSocket::open("vcan0").unwrap();
        let socket2 = CanSocket::open("vcan0").unwrap();

        let frame_id_1 = CanFrame::from_raw_id(0x01, &[0u8]).unwrap();
        let frame_id_2 = CanFrame::from_raw_id(0x02, &[0u8]).unwrap();
        let frame_id_3 = CanFrame::from_raw_id(0x03, &[0u8]).unwrap();

        let (mut sink, _stream) = socket1.split();
        let (_sink, stream) = socket2.split();

        let count_ids_less_than_3 = stream
            .map(|x| x.unwrap())
            .take_while(|frame| future::ready(frame.raw_id() < 3))
            .fold(0u8, |acc, _frame| async move { acc + 1 });

        let send_frames = async {
            let _frame_1 = sink.send(frame_id_1).await?;
            let _frame_2 = sink.send(frame_id_2).await?;
            let _frame_3 = sink.send(frame_id_3).await?;
            println!("Sent 3 frames");
            Ok::<(), Error>(())
        };

        let (x, frame_send_r) = future::join(count_ids_less_than_3, send_frames).await;
        frame_send_r?;

        assert_eq!(x, 2);

        Ok(())
    }

    #[serial]
    #[tokio::test]
    async fn test_sink_stream_fd() -> Result<()> {
        let socket1 = CanFdSocket::open("vcan0").unwrap();
        let socket2 = CanFdSocket::open("vcan0").unwrap();

        let frame_id_1 = CanFdFrame::from_raw_id(0x01, &[0u8]).unwrap();
        let frame_id_2 = CanFdFrame::from_raw_id(0x02, &[0u8]).unwrap();
        let frame_id_3 = CanFdFrame::from_raw_id(0x03, &[0u8]).unwrap();

        let (mut sink, _stream) = socket1.split();
        let (_sink, stream) = socket2.split();

        let count_ids_less_than_3 = stream
            .map(|x| x.unwrap())
            .take_while(|frame| {
                if let CanAnyFrame::Fd(frame) = frame {
                    future::ready(frame.raw_id() < 3)
                } else {
                    future::ready(false)
                }
            })
            .fold(0u8, |acc, _frame| async move { acc + 1 });

        let send_frames = async {
            let _frame_1 = sink.send(frame_id_1).await?;
            let _frame_2 = sink.send(frame_id_2).await?;
            let _frame_3 = sink.send(frame_id_3).await?;
            println!("Sent 3 frames");
            Ok::<(), Error>(())
        };

        let (x, frame_send_r) = future::join(count_ids_less_than_3, send_frames).await;
        frame_send_r?;

        assert_eq!(x, 2);

        Ok(())
    }
}
