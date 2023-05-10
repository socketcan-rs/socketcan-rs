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
//! use tokio_socketcan::{CanSocket, Error};
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
use std::io;
use std::os::raw::c_uint;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::pin::Pin;
use std::task::Poll;
use std::{future::Future, os::unix::prelude::RawFd};

use futures::prelude::*;
use futures::ready;
use futures::task::Context;

use mio::{event, unix::SourceFd, Interest, Registry, Token};

pub use crate::{CanFilter, CanFrame, CanError, Error, Result, Socket};
use tokio::io::unix::AsyncFd;

/*
use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Failed to open CAN Socket")]
    CanSocketOpen(#[from] crate::CanError),
    #[error("IO error")]
    IO(#[from] io::Error),
}
 */

/// A Future representing the eventual
/// writing of a CANFrame to the socket
///
/// Created by the CanSocket.write_frame() method
#[derive(Debug)]
pub struct CanWriteFuture {
    socket: CanSocket,
    frame: CanFrame,
}

impl Future for CanWriteFuture {
    type Output = io::Result<()>;

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
pub struct EventedCanSocket(crate::CanSocket);

impl EventedCanSocket {
    fn get_ref(&self) -> &crate::CanSocket {
        &self.0
    }
}

impl AsRawFd for EventedCanSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl event::Source for EventedCanSocket {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.0.as_raw_fd()).register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.0.as_raw_fd()).reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        SourceFd(&self.0.as_raw_fd()).deregister(registry)
    }
}

/// An asynchronous I/O wrapped CanSocket
#[derive(Debug)]
pub struct CanSocket(AsyncFd<EventedCanSocket>);

impl CanSocket {
    /// Open a named CAN device such as "vcan0"
    pub fn open(ifname: &str) -> Result<Self> {
        let sock = crate::CanSocket::open(ifname)?;
        sock.set_nonblocking(true)?;
        Ok(Self(AsyncFd::new(EventedCanSocket(sock))?))
    }

    /// Open CAN device by kernel interface number
    pub fn open_if(if_index: c_uint) -> Result<CanSocket> {
        let sock = crate::CanSocket::open_iface(if_index)?;
        sock.set_nonblocking(true)?;
        Ok(Self(AsyncFd::new(EventedCanSocket(sock))?))
    }

    /// Sets the filter mask on the socket
    pub fn set_filter(&self, filters: &[CanFilter]) -> Result<()> {
        self.0.get_ref().0.set_filters(filters)?;
		Ok(())
    }

    /// Disable reception of CAN frames by setting an empty filter
    pub fn filter_drop_all(&self) -> Result<()> {
        self.0.get_ref().0.set_filter_drop_all()?;
		Ok(())
    }

    /// Accept all frames, disabling any kind of filtering.
    pub fn filter_accept_all(&self) -> Result<()> {
        self.0.get_ref().0.set_filter_accept_all()?;
		Ok(())
    }

	/// Sets the error mask on the socket
    pub fn set_error_filter(&self, mask: u32) -> Result<()> {
        self.0.get_ref().0.set_error_filter(mask)?;
		Ok(())
    }

	/// Sets the error mask on the socket to reject all errors.
    pub fn error_filter_drop_all(&self) -> Result<()> {
        self.0.get_ref().0.set_error_filter_drop_all()?;
		Ok(())
    }

	/// Sets the error mask on the socket to accept all errors.
    pub fn error_filter_accept_all(&self) -> Result<()> {
        self.0.get_ref().0.set_error_filter_accept_all()?;
		Ok(())
    }

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

    /// Clone the CanSocket by using the `dup` syscall to get another
    /// file descriptor. This method makes clones fairly cheap and
    /// avoids complexity around ownership
    fn try_clone(&self) -> Result<Self> {
        let fd = self.0.get_ref().0.as_raw_fd();
        unsafe {
            // essentially we're cheating and making it cheaper/easier
            // to manage multiple references to the socket by relying
            // on the posix behaviour of `dup()` which essentially lets
            // the kernel worry about keeping track of references;
            // as long as one of the duplicated file descriptors is open
            // the socket as a whole isn't going to be closed.
            let new_fd = libc::dup(fd);
            let new = crate::CanSocket::from_raw_fd(new_fd);
            Ok(Self(AsyncFd::new(EventedCanSocket(new))?))
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{select, try_join};
    use futures_timer::Delay;

    use std::io;
    use std::time::Duration;

    /// Receive a frame from the CanSocket
    async fn recv_frame(mut socket: CanSocket) -> io::Result<CanSocket> {
        // let mut frame_stream = socket;

        select!(
            frame = socket.next().fuse() => if let Some(_frame) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(Duration::from_millis(100)).fuse() => Err(io::Error::from(io::ErrorKind::TimedOut)),
        )
    }

    /// Write a test frame to the CanSocket
    async fn write_frame(socket: &CanSocket) -> Result<(), Error> {
        let test_frame = crate::CanFrame::new(0x1, &[0], false, false).unwrap();
        socket.write_frame(test_frame)?.await?;
        Ok(())
    }

    /// Attempt delivery of two messages, using a oneshot channel
    /// to prompt the second message in order to demonstrate that
    /// waiting for CAN reads is not blocking.
    #[tokio::test]
    async fn test_receive() -> Result<(), Error> {
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

    #[tokio::test]
    async fn test_sink_stream() -> io::Result<()> {
        let socket1 = CanSocket::open("vcan0").unwrap();
        let socket2 = CanSocket::open("vcan0").unwrap();

        let frame_id_1 = CanFrame::new(1, &[0u8], false, false).unwrap();
        let frame_id_2 = CanFrame::new(2, &[0u8], false, false).unwrap();
        let frame_id_3 = CanFrame::new(3, &[0u8], false, false).unwrap();

        let (mut sink, _stream) = socket1.split();
        let (_sink, stream) = socket2.split();

        let count_ids_less_than_3 = stream
            .map(|x| x.unwrap())
            .take_while(|frame| future::ready(frame.id() < 3))
            .fold(0u8, |acc, _frame| async move { acc + 1 });

        let send_frames = async {
            let _frame_1 = sink.send(frame_id_1).await?;
            let _frame_2 = sink.send(frame_id_2).await?;
            let _frame_3 = sink.send(frame_id_3).await?;
            println!("Sent 3 frames");
            Ok::<(), io::Error>(())
        };

        let (x, frame_send_r) = futures::future::join(count_ids_less_than_3, send_frames).await;
        frame_send_r?;

        assert_eq!(x, 2);

        Ok(())
    }
}
