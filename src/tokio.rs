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
//!         socket_tx.write_frame(frame)?.await;
//!     }
//!     Ok(())
//! }
//! ```
use crate::{
    CanAddr, CanAnyFrame, CanFdFrame, CanFrame, Error, IoResult, Result, Socket, SocketOptions,
};
use futures::{prelude::*, ready, task::Context};
use mio::{event, unix::SourceFd, Interest, Registry, Token};
use std::{
    future::Future,
    os::unix::{
        io::{AsRawFd, FromRawFd, OwnedFd},
        prelude::RawFd,
    },
    pin::Pin,
    task::Poll,
};
use tokio::io::unix::AsyncFd;

/// A Future representing the eventual writing of a CanFrame to the socket.
///
/// Created by the CanSocket.write_frame() method
#[derive(Debug)]
pub struct CanWriteFuture {
    socket: CanSocket,
    frame: CanFrame,
}

impl Future for CanWriteFuture {
    type Output = IoResult<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let _ = ready!(self.socket.0.poll_write_ready(cx))?;
        match self.socket.0.get_ref().0.write_frame_insist(&self.frame) {
            Ok(_) => Poll::Ready(Ok(())),
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}

/// A CanSocket wrapped for mio eventing
/// to allow it be integrated in turn into tokio
#[derive(Debug)]
pub struct EventedCanSocket<T: Socket = crate::CanSocket>(T);

impl<T: Socket> EventedCanSocket<T> {
    fn get_ref(&self) -> &T {
        &self.0
    }
}

impl<T: Socket> AsRawFd for EventedCanSocket<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl<T: Socket> event::Source for EventedCanSocket<T> {
    fn register(&mut self, registry: &Registry, token: Token, interests: Interest) -> IoResult<()> {
        SourceFd(&self.0.as_raw_fd()).register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> IoResult<()> {
        SourceFd(&self.0.as_raw_fd()).reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> IoResult<()> {
        SourceFd(&self.0.as_raw_fd()).deregister(registry)
    }
}

/// An asynchronous I/O wrapped CanSocket
#[derive(Debug)]
pub struct AsyncCanSocket<T: Socket>(AsyncFd<EventedCanSocket<T>>);

impl<T: Socket + From<OwnedFd>> AsyncCanSocket<T> {
    /// Open a named CAN device such as "can0, "vcan0", etc
    pub fn open(ifname: &str) -> IoResult<Self> {
        let sock = T::open(ifname)?;
        sock.set_nonblocking(true)?;
        Ok(Self(AsyncFd::new(EventedCanSocket(sock))?))
    }

    /// Open CAN device by kernel interface number
    pub fn open_if(ifindex: u32) -> IoResult<Self> {
        let sock = T::open_iface(ifindex)?;
        sock.set_nonblocking(true)?;
        Ok(Self(AsyncFd::new(EventedCanSocket(sock))?))
    }

    /// Open a CAN socket by address
    pub fn open_addr(addr: &CanAddr) -> IoResult<Self> {
        let sock = T::open_addr(addr)?;
        sock.set_nonblocking(true)?;
        Ok(Self(AsyncFd::new(EventedCanSocket(sock))?))
    }

    /// Clone the Async Socket by using the `dup` syscall to get another
    /// file descriptor. This method makes clones fairly cheap and
    /// avoids complexity around ownership
    fn try_clone(&self) -> Result<Self> {
        let fd = self.as_raw_fd();
        unsafe {
            // essentially we're cheating and making it cheaper/easier
            // to manage multiple references to the socket by relying
            // on the posix behaviour of `dup()` which essentially lets
            // the kernel worry about keeping track of references;
            // as long as one of the duplicated file descriptors is open
            // the socket as a whole isn't going to be closed.
            let new_fd = OwnedFd::from_raw_fd(libc::dup(fd));
            let new = T::from(new_fd);
            Ok(Self(AsyncFd::new(EventedCanSocket(new))?))
        }
    }
}

impl<T: Socket> SocketOptions for AsyncCanSocket<T> {}

impl<T: Socket> AsRawFd for AsyncCanSocket<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.0.get_ref().0.as_raw_fd()
    }
}

/// Asynchronous Can Socket
pub type CanSocket = AsyncCanSocket<crate::CanSocket>;

impl CanSocket {
    /// Write a CAN frame to the socket asynchronously
    ///
    /// This uses the semantics of socketcan's `write_frame_insist`,
    /// IE: it will automatically retry when it fails on an EINTR
    pub fn write_frame(&self, frame: CanFrame) -> Result<CanWriteFuture> {
        Ok(CanWriteFuture {
            socket: self.try_clone()?,
            frame,
        })
    }
}

impl Stream for CanSocket {
    type Item = Result<CanFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            let mut ready_guard = ready!(self.0.poll_read_ready(cx))?;
            match ready_guard.try_io(|inner| inner.get_ref().get_ref().read_frame()) {
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
        self.0.get_ref().0.write_frame_insist(&item)?;
        Ok(())
    }
}

/// An Asynchronous CAN FD Socket
pub type CanFdSocket = AsyncCanSocket<crate::CanFdSocket>;

impl CanFdSocket {
    /// Write a CAN FD frame to the socket asynchronously
    ///
    /// This uses the semantics of socketcan's `write_frame_insist`,
    /// IE: it will automatically retry when it fails on an EINTR
    pub fn write_frame(&self, frame: CanFdFrame) -> Result<CanFdWriteFuture> {
        Ok(CanFdWriteFuture {
            socket: self.try_clone()?,
            frame,
        })
    }
}

/// A Future representing the eventual writing of a CanFdFrame to the socket.
///
/// Created by the CanFdSocket.write_frame() method
#[derive(Debug)]
pub struct CanFdWriteFuture {
    socket: CanFdSocket,
    frame: CanFdFrame,
}

impl Future for CanFdWriteFuture {
    type Output = IoResult<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let _ = ready!(self.socket.0.poll_write_ready(cx))?;
        match self.socket.0.get_ref().0.write_frame_insist(&self.frame) {
            Ok(_) => Poll::Ready(Ok(())),
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}

impl Stream for CanFdSocket {
    type Item = Result<CanAnyFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            let mut ready_guard = ready!(self.0.poll_read_ready(cx))?;
            match ready_guard.try_io(|inner| inner.get_ref().get_ref().read_frame()) {
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
        self.0.get_ref().0.write_frame_insist(&item)?;
        Ok(())
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(feature = "vcan_tests")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanFrame, Frame, IoErrorKind, StandardId};
    use embedded_can::Frame as EmbeddedFrame;
    use futures::{select, try_join};
    use futures_timer::Delay;
    use serial_test::serial;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_millis(100);

    /// Receive a frame from the CanSocket
    async fn recv_frame(mut socket: CanSocket) -> Result<CanSocket> {
        select!(
            frame = socket.next().fuse() => if let Some(_frame) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(TIMEOUT).fuse() => Err(IoErrorKind::TimedOut.into()),
        )
    }

    /// Receive a frame from the CanFdSocket
    async fn recv_frame_fd(mut socket: CanFdSocket) -> Result<CanFdSocket> {
        select!(
            frame = socket.next().fuse() => if let Some(_frame) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(TIMEOUT).fuse() => Err(IoErrorKind::TimedOut.into()),
        )
    }

    /// Write a test frame to the CanSocket
    async fn write_frame(socket: &CanSocket) -> Result<()> {
        let test_frame = CanFrame::new(StandardId::new(0x1).unwrap(), &[0]).unwrap();
        socket.write_frame(test_frame)?.await?;
        Ok(())
    }

    /// Write a test frame to the CanSocket
    async fn write_frame_fd(socket: &CanFdSocket) -> Result<()> {
        let test_frame =
            CanFdFrame::new(StandardId::new(0x1).unwrap(), &[0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        socket.write_frame(test_frame)?.await?;
        Ok(())
    }

    /// Attempt delivery of two messages, using a oneshot channel
    /// to prompt the second message in order to demonstrate that
    /// waiting for CAN reads is not blocking.
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

        let (x, frame_send_r) = futures::future::join(count_ids_less_than_3, send_frames).await;
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

        let (x, frame_send_r) = futures::future::join(count_ids_less_than_3, send_frames).await;
        frame_send_r?;

        assert_eq!(x, 2);

        Ok(())
    }
}
