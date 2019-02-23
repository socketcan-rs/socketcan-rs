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
//! use futures::stream::Stream;
//! use futures::future::{self, Future};
//!
//! let socket_rx = tokio_socketcan::CANSocket::open("vcan0").unwrap();
//! let socket_tx = tokio_socketcan::CANSocket::open("vcan0").unwrap();
//!
//! tokio::run(socket_rx.for_each(move |frame| {
//!     socket_tx.write_frame(frame)
//! }).map_err(|_err| {}));
//!
//! ```
use std::io;
use std::os::raw::c_uint;
use std::os::unix::io::{AsRawFd, FromRawFd};

use libc;

use futures::{self, try_ready};

use mio::event::Evented;
use mio::unix::EventedFd;
use mio::{unix::UnixReady, Poll, PollOpt, Ready, Token};

use tokio::prelude::*;
use tokio::reactor::PollEvented2;

use socketcan;
pub use socketcan::CANFrame;
pub use socketcan::CANSocketOpenError;

/// A Future representing the eventual
/// writing of a CANFrame to the socket
///
/// Created by the CANSocket.write_frame() method
pub struct CANWriteFuture {
    socket: CANSocket,
    frame: CANFrame,
}

impl Future for CANWriteFuture {
    type Item = ();
    type Error = io::Error;

    fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
        try_ready!(self.socket.0.poll_write_ready());
        match self.socket.0.get_ref().0.write_frame_insist(&self.frame) {
            Ok(_) => Ok(Async::Ready(())),
            Err(err) => Err(err),
        }
    }
}

/// A socketcan::CANSocket wrapped for mio eventing
/// to allow it be integrated in turn into tokio
#[derive(Debug)]
pub struct EventedCANSocket(socketcan::CANSocket);

impl EventedCANSocket {
    fn get_ref(&self) -> &socketcan::CANSocket {
        &self.0
    }
}

impl Evented for EventedCANSocket {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0.as_raw_fd()).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0.as_raw_fd()).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        EventedFd(&self.0.as_raw_fd()).deregister(poll)
    }
}

/// An asynchronous I/O wrapped socketcan::CANSocket
#[derive(Debug)]
pub struct CANSocket(PollEvented2<EventedCANSocket>);

impl CANSocket {
    /// Open a named CAN device such as "vcan0"
    pub fn open(ifname: &str) -> Result<CANSocket, CANSocketOpenError> {
        let sock = socketcan::CANSocket::open(ifname)?;
        sock.set_nonblocking(true)?;
        Ok(CANSocket(PollEvented2::new(EventedCANSocket(sock))))
    }

    /// Open CAN device by kernel interface number
    pub fn open_if(if_index: c_uint) -> Result<CANSocket, CANSocketOpenError> {
        let sock = socketcan::CANSocket::open_if(if_index)?;
        sock.set_nonblocking(true)?;
        Ok(CANSocket(PollEvented2::new(EventedCANSocket(sock))))
    }

    /// Sets the filter mask on the socket
    pub fn set_filter(&self, filters: &[socketcan::CANFilter]) -> io::Result<()> {
        self.0.get_ref().0.set_filter(filters)
    }

    /// Disable reception of CAN frames by setting an empty filter
    pub fn filter_drop_all(&self) -> io::Result<()> {
        self.0.get_ref().0.filter_drop_all()
    }

    /// Accept all frames, disabling any kind of filtering.
    pub fn filter_accept_all(&self) -> io::Result<()> {
        self.0.get_ref().0.filter_accept_all()
    }

    pub fn set_error_filter(&self, mask: u32) -> io::Result<()> {
        self.0.get_ref().0.set_error_filter(mask)
    }

    pub fn error_filter_drop_all(&self) -> io::Result<()> {
        self.0.get_ref().0.error_filter_drop_all()
    }

    pub fn error_filter_accept_all(&self) -> io::Result<()> {
        self.0.get_ref().0.error_filter_accept_all()
    }

    /// Write a CANFrame to the socket asynchronously
    ///
    /// This uses the semantics of socketcan's `write_frame_insist`,
    /// IE: it will automatically retry when it fails on an EINTR
    pub fn write_frame(&self, frame: CANFrame) -> CANWriteFuture {
        CANWriteFuture {
            socket: self.clone(),
            frame: frame,
        }
    }
}

impl Clone for CANSocket {
    /// Clone the CANSocket by using the `dup` syscall to get another
    /// file descriptor. This method makes clones fairly cheap and
    /// avoids complexity around ownership
    fn clone(&self) -> Self {
        let fd = self.0.get_ref().0.as_raw_fd();
        unsafe {
            // essentially we're cheating and making it cheaper/easier
            // to manage multiple references to the socket by relying
            // on the posix behaviour of `dup()` which essentially lets
            // the kernel worry about keeping track of references;
            // as long as one of the duplicated file descriptors is open
            // the socket as a whole isn't going to be closed.
            let new_fd = libc::dup(fd);
            let new = socketcan::CANSocket::from_raw_fd(new_fd);
            CANSocket(PollEvented2::new(EventedCANSocket(new)))
        }
    }
}

impl Stream for CANSocket {
    type Item = CANFrame;
    type Error = io::Error;

    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        try_ready!(self
            .0
            .poll_read_ready(Ready::readable() | UnixReady::error()));
        match self.0.get_ref().get_ref().read_frame() {
            Ok(frame) => Ok(Async::Ready(Some(frame))),
            Err(err) => {
                if err.kind() == io::ErrorKind::WouldBlock {
                    self.0.clear_read_ready(Ready::readable())?;
                    Ok(Async::NotReady)
                } else {
                    Err(err)
                }
            }
        }
    }
}

impl Sink for CANSocket {
    type SinkItem = CANFrame;
    type SinkError = io::Error;

    fn start_send(
        &mut self,
        item: Self::SinkItem,
    ) -> Result<AsyncSink<Self::SinkItem>, Self::SinkError> {
        match self.0.get_ref().0.write_frame_insist(&item) {
            Ok(_) => Ok(AsyncSink::Ready),
            Err(err) => {
                if err.kind() == io::ErrorKind::WouldBlock {
                    self.0.clear_write_ready()?;
                    Ok(AsyncSink::NotReady(item))
                } else {
                    Err(err)
                }
            }
        }
    }

    /// All progress is completed immediately in the start_send
    fn poll_complete(&mut self) -> Result<Async<()>, Self::SinkError> {
        Ok(Async::Ready(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::ok;
    use std::time::Duration;

    /// Receive a frame from the CANSocket
    fn recv_frame(socket: CANSocket) -> Box<Future<Item = CANSocket, Error = String> + Send> {
        Box::new(
            socket
                .into_future()
                .map(|(_frame, stream)| stream)
                .map_err(|err| format!("io error: {:?}", err))
                .timeout(Duration::from_millis(100))
                .map_err(|timeout| format!("timeout: {:?}", timeout)),
        )
    }

    /// Write a test frame to the CANSocket
    fn write_frame(socket: &CANSocket) -> Box<Future<Item = (), Error = ()> + Send> {
        let test_frame = socketcan::CANFrame::new(0x1, &[0], false, false).unwrap();
        Box::new(socket.write_frame(test_frame).map_err(|err| {
            println!("io error: {:?}", err);
        }))
    }

    /// Attempt delivery of two messages, using a oneshot channel
    /// to prompt the second message in order to demonstrate that
    /// waiting for CAN reads is not blocking.
    #[test]
    fn test_receive() {
        let socket1 = CANSocket::open("vcan0").unwrap();
        let socket2 = CANSocket::open("vcan0").unwrap();
        let (tx, rx) = futures::sync::oneshot::channel::<()>();

        let send_frames = write_frame(&socket1)
            .and_then(|_| rx.map(|_| {}).map_err(|_| panic!()))
            .and_then(move |_| write_frame(&socket1));

        let recv_frames = recv_frame(socket2).and_then(|stream_continuation| {
            tx.send(()).unwrap();
            recv_frame(stream_continuation)
        });

        let mut rt = tokio::runtime::Runtime::new().unwrap();
        rt.spawn(send_frames);
        rt.block_on(recv_frames).unwrap();
    }

    #[test]
    fn test_sink_stream() {
        static mut COUNTER: usize = 0;

        let socket1 = CANSocket::open("vcan0").unwrap();
        let socket2 = CANSocket::open("vcan0").unwrap();

        let frame_id_1 = CANFrame::new(1, &[0u8], false, false).unwrap();
        let frame_id_2 = CANFrame::new(2, &[0u8], false, false).unwrap();
        let frame_id_3 = CANFrame::new(3, &[0u8], false, false).unwrap();

        let (sink, _stream) = socket1.split();
        let (_sink, stream) = socket2.split();

        let take_ids_less_than_3 = stream.take_while(|frame| ok(frame.id() < 3)).for_each(|_| {
            unsafe { COUNTER += 1 };
            ok(())
        });

        let send_frames = sink
            .send(frame_id_1)
            .and_then(move |sink| sink.send(frame_id_2))
            .and_then(move |sink| sink.send(frame_id_3))
            .and_then(|_| ok(()))
            .map_err(|_| panic!());

        let mut rt = tokio::runtime::Runtime::new().unwrap();
        rt.spawn(send_frames);
        rt.block_on(take_ids_less_than_3).unwrap();
        unsafe { assert_eq!(COUNTER, 2) };
    }
}
