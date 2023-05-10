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
//! use tokio_socketcan::{CANSocket, Error};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Error> {
//!     let mut socket_rx = CANSocket::open("vcan0")?;
//!     let socket_tx = CANSocket::open("vcan0")?;
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

use thiserror::Error as ThisError;

pub use socketcan::{CanFilter, CanFrame, CanError, Socket};
use tokio::io::unix::AsyncFd;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Failed to open CAN Socket")]
    CANSocketOpen(#[from] socketcan::CanError),
    #[error("IO error")]
    IO(#[from] io::Error),
}

/// A Future representing the eventual
/// writing of a CANFrame to the socket
///
/// Created by the CANSocket.write_frame() method
pub struct CANWriteFuture {
    socket: CANSocket,
    frame: CanFrame,
}

impl Future for CANWriteFuture {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let _ = ready!(self.socket.0.poll_write_ready(cx))?;
        match self.socket.0.get_ref().0.write_frame_insist(&self.frame) {
            Ok(_) => Poll::Ready(Ok(())),
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}

/// A socketcan::CANSocket wrapped for mio eventing
/// to allow it be integrated in turn into tokio
#[derive(Debug)]
pub struct EventedCANSocket(socketcan::CanSocket);

impl EventedCANSocket {
    fn get_ref(&self) -> &socketcan::CanSocket {
        &self.0
    }
}

impl AsRawFd for EventedCANSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl event::Source for EventedCANSocket {
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

/// An asynchronous I/O wrapped socketcan::CANSocket
#[derive(Debug)]
pub struct CANSocket(AsyncFd<EventedCANSocket>);

impl CANSocket {
    /// Open a named CAN device such as "vcan0"
    pub fn open(ifname: &str) -> Result<CANSocket, Error> {
        let sock = socketcan::CanSocket::open(ifname)?;
        sock.set_nonblocking(true)?;
        Ok(CANSocket(AsyncFd::new(EventedCANSocket(sock))?))
    }

    /// Open CAN device by kernel interface number
    pub fn open_if(if_index: c_uint) -> Result<CANSocket, Error> {
        let sock = socketcan::CanSocket::open_iface(if_index)?;
        sock.set_nonblocking(true)?;
        Ok(CANSocket(AsyncFd::new(EventedCANSocket(sock))?))
    }

    /// Sets the filter mask on the socket
    pub fn set_filter(&self, filters: &[CanFilter]) -> io::Result<()> {
        self.0.get_ref().0.set_filters(filters)
    }

    /// Disable reception of CAN frames by setting an empty filter
    pub fn filter_drop_all(&self) -> io::Result<()> {
        self.0.get_ref().0.set_filter_drop_all()
    }

    /// Accept all frames, disabling any kind of filtering.
    pub fn filter_accept_all(&self) -> io::Result<()> {
        self.0.get_ref().0.set_filter_accept_all()
    }

    pub fn set_error_filter(&self, mask: u32) -> io::Result<()> {
        self.0.get_ref().0.set_error_filter(mask)
    }

    pub fn error_filter_drop_all(&self) -> io::Result<()> {
        self.0.get_ref().0.set_error_filter_drop_all()
    }

    pub fn error_filter_accept_all(&self) -> io::Result<()> {
        self.0.get_ref().0.set_error_filter_accept_all()
    }

    /// Write a CANFrame to the socket asynchronously
    ///
    /// This uses the semantics of socketcan's `write_frame_insist`,
    /// IE: it will automatically retry when it fails on an EINTR
    pub fn write_frame(&self, frame: CanFrame) -> Result<CANWriteFuture, Error> {
        Ok(CANWriteFuture {
            socket: self.try_clone()?,
            frame,
        })
    }

    /// Clone the CANSocket by using the `dup` syscall to get another
    /// file descriptor. This method makes clones fairly cheap and
    /// avoids complexity around ownership
    fn try_clone(&self) -> Result<Self, Error> {
        let fd = self.0.get_ref().0.as_raw_fd();
        unsafe {
            // essentially we're cheating and making it cheaper/easier
            // to manage multiple references to the socket by relying
            // on the posix behaviour of `dup()` which essentially lets
            // the kernel worry about keeping track of references;
            // as long as one of the duplicated file descriptors is open
            // the socket as a whole isn't going to be closed.
            let new_fd = libc::dup(fd);
            let new = socketcan::CanSocket::from_raw_fd(new_fd);
            Ok(CANSocket(AsyncFd::new(EventedCANSocket(new))?))
        }
    }
}

impl Stream for CANSocket {
    type Item = io::Result<CanFrame>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            let mut ready_guard = ready!(self.0.poll_read_ready(cx))?;
            match ready_guard.try_io(|inner| inner.get_ref().get_ref().read_frame()) {
                Ok(result) => return Poll::Ready(Some(result)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl Sink<CanFrame> for CANSocket {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let _ = ready!(self.0.poll_write_ready(cx))?;
        Poll::Ready(Ok(()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let mut ready_guard = ready!(self.0.poll_write_ready(cx))?;
        ready_guard.clear_ready();
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: CanFrame) -> Result<(), Self::Error> {
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

    /// Receive a frame from the CANSocket
    async fn recv_frame(mut socket: CANSocket) -> io::Result<CANSocket> {
        // let mut frame_stream = socket;

        select!(
            frame = socket.next().fuse() => if let Some(_frame) = frame { Ok(socket) } else { panic!("unexpected") },
            _timeout = Delay::new(Duration::from_millis(100)).fuse() => Err(io::Error::from(io::ErrorKind::TimedOut)),
        )
    }

    /// Write a test frame to the CANSocket
    async fn write_frame(socket: &CANSocket) -> Result<(), Error> {
        let test_frame = socketcan::CANFrame::new(0x1, &[0], false, false).unwrap();
        socket.write_frame(test_frame)?.await?;
        Ok(())
    }

    /// Attempt delivery of two messages, using a oneshot channel
    /// to prompt the second message in order to demonstrate that
    /// waiting for CAN reads is not blocking.
    #[tokio::test]
    async fn test_receive() -> Result<(), Error> {
        let socket1 = CANSocket::open("vcan0").unwrap();
        let socket2 = CANSocket::open("vcan0").unwrap();

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
        let socket1 = CANSocket::open("vcan0").unwrap();
        let socket2 = CANSocket::open("vcan0").unwrap();

        let frame_id_1 = CANFrame::new(1, &[0u8], false, false).unwrap();
        let frame_id_2 = CANFrame::new(2, &[0u8], false, false).unwrap();
        let frame_id_3 = CANFrame::new(3, &[0u8], false, false).unwrap();

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
