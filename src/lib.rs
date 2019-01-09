use std::io;
use std::os::raw::c_uint;
use std::os::unix::io::{AsRawFd, FromRawFd};

use libc;

use futures;

use mio::event::Evented;
use mio::unix::EventedFd;
use mio::{Poll, PollOpt, Ready, Token};

use tokio::prelude::*;
use tokio::reactor::PollEvented2;

use socketcan;
pub use socketcan::CANFrame;
pub use socketcan::CANSocketOpenError;

pub struct CANWriteFuture {
    socket: CANSocket,
    frame: CANFrame,
}

impl Future for CANWriteFuture {
    type Item = ();
    type Error = io::Error;

    fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
        futures::try_ready!(self.socket.0.poll_write_ready());
        match self.socket.0.get_ref().0.write_frame_insist(&self.frame) {
            Ok(_) => Ok(Async::Ready(())),
            Err(err) => Err(err),
        }
    }
}

/// A CAN socket wrapped for mio
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

/// Wrapped socketcan CANSocket with asynchronous I/O
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

    pub fn write_frame(&self, frame: CANFrame) -> CANWriteFuture {
        CANWriteFuture {
            socket: self.clone(),
            frame: frame,
        }
    }
}

impl Clone for CANSocket {
    fn clone(&self) -> Self {
        let fd = self.0.get_ref().0.as_raw_fd();
        unsafe {
            // essentially we're cheating and making it cheaper/easier
            // to manage multiple references to the socket by relying
            // on the linux behaviour of `dup()` which essentially lets
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

    /// Determine if the socket is ready to read from, and read if we can
    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        futures::try_ready!(self.0.poll_read_ready(Ready::readable()));
        // WouldBlock shouldn't come back to us here, but this is paranoia.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_recv() {
        let socket1 = CANSocket::open("vcan0").unwrap();
        let socket2 = CANSocket::open("vcan0").unwrap();

        let test_frame = socketcan::CANFrame::new(0x1, &[0], false, false).unwrap();
        let send_frame = socket1.write_frame(test_frame).map_err(|err| {
            println!("io error: {:?}", err);
        });

        let recv_frames = future::lazy(move || {
            socket2
                .into_future()
                .map(|(_frame, _stream_fut)| ())
                .map_err(|err| format!("io error: {:?}", err))
                .timeout(Duration::from_millis(10000))
                .map_err(|timeout| format!("timeout: {:?}", timeout))
        });

        let mut rt = tokio::runtime::Runtime::new().unwrap();
        rt.spawn(send_frame);
        rt.block_on(recv_frames).unwrap();
    }
}
