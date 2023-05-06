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

use std::mem::{size_of, MaybeUninit};
use std::{
    io, mem,
    os::{raw::c_void, unix::io::AsRawFd},
};

use async_io::Async;
use libc::{can_frame, canfd_frame, read, ssize_t, write};

use crate::{frame::AsPtr, CanAnyFrame, CanFdFrame, CanFrame, Socket};

macro_rules! write_frame {
    ($target:expr, $frame_type:ty, $frame:expr) => {
        $target
            .write_with(|fd| {
                let ret = unsafe {
                    write(
                        fd.as_raw_fd(),
                        $frame.as_ptr() as *const c_void,
                        <$frame_type>::size(),
                    )
                };
                if ret == <$frame_type>::size() as isize {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            })
            .await
    };
}

// ===== CanSocket =====

/// A socket for a classic CAN 2.0 device.
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
pub struct CanSocket {
    inner: Async<super::CanSocket>,
}

impl TryFrom<super::CanSocket> for CanSocket {
    type Error = io::Error;

    fn try_from(value: super::CanSocket) -> Result<Self, Self::Error> {
        Ok(Self {
            inner: Async::new(value)?,
        })
    }
}

impl CanSocket {
    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "can0", "vcan0", or "socan0".
    pub fn open(ifname: &str) -> io::Result<Self> {
        super::CanSocket::open(ifname)?.try_into()
    }

    /// Permits access to the inner synchronous socket, for example to perform settings.
    pub fn as_sync_socket(&self) -> &super::CanSocket {
        self.inner.as_ref()
    }

    /// Writes a standard 2.0 frame to the socket asynchronously.
    pub async fn write_frame<F>(&self, frame: &F) -> io::Result<()>
    where
        F: Into<CanFrame> + AsPtr,
    {
        write_frame!(self.inner, F, frame)
    }

    /// Reads a standard 2.0 frame from the socket asynchronously.
    pub async fn read_frame(&self) -> io::Result<CanFrame> {
        let mut frame = MaybeUninit::<can_frame>::uninit();

        self.inner
            .read_with(|fd| {
                let ret = unsafe {
                    read(
                        fd.as_raw_fd(),
                        frame.as_mut_ptr() as *mut c_void,
                        size_of::<can_frame>() as libc::size_t,
                    )
                };
                if ret == size_of::<can_frame>() as isize {
                    Ok(ret)
                } else {
                    Err(io::Error::last_os_error())
                }
            })
            .await?;

        //  SAFETY: Return value was okay and we trust the c library to have properly
        //          filled the value.
        let frame = unsafe { frame.assume_init() };
        Ok(frame.into())
    }
}

// ===== CanFdSocket =====

/// A socket for CAN FD devices.
///
/// This can transmit and receive CAN 2.0 frames with up to 8-bytes of data,
/// or CAN Flexible Data (FD) frames with up to 64-bytes of data.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct CanFdSocket {
    /// The raw file descriptor
    inner: Async<super::CanFdSocket>,
}

impl TryFrom<super::CanFdSocket> for CanFdSocket {
    type Error = io::Error;

    fn try_from(value: super::CanFdSocket) -> Result<Self, Self::Error> {
        Ok(Self {
            inner: Async::new(value)?,
        })
    }
}

impl CanFdSocket {
    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "can0", "vcan0", or "socan0".
    pub fn open(ifname: &str) -> io::Result<Self> {
        super::CanFdSocket::open(ifname)?.try_into()
    }

    /// Permits access to the inner synchronous socket, for example to perform settings.
    pub fn as_sync_socket(&self) -> &super::CanFdSocket {
        self.inner.as_ref()
    }

    /// Writes either a standard 2.0 frame or an FD frame to the socket asynchronously.
    pub async fn write_frame<F>(&self, frame: &F) -> io::Result<()>
    where
        F: Into<CanAnyFrame> + AsPtr,
    {
        write_frame!(self.inner, F, frame)
    }

    /// Reads either a standard 2.0 frame or an FD frame from the socket asynchronously.
    pub async fn read_frame(&self) -> io::Result<CanAnyFrame> {
        let mut frame = MaybeUninit::<canfd_frame>::uninit();

        self.inner
            .read_with(|fd| {
                let ret = unsafe {
                    read(
                        fd.as_raw_fd(),
                        frame.as_mut_ptr() as *mut c_void,
                        size_of::<canfd_frame>() as libc::size_t,
                    )
                };

                if ret == size_of::<can_frame>() as ssize_t {
                    // SAFETY: We are assuming that the C standard library upholds its contract
                    //         and writes a valid can_frame into the buffer.
                    let frame = unsafe { mem::transmute_copy::<_, can_frame>(&frame) };
                    Ok(<can_frame as Into<CanFrame>>::into(frame).into())
                } else if ret == size_of::<canfd_frame>() as ssize_t {
                    // SAFETY: We are assuming that the C standard library upholds its contract
                    //         and writes a valid canfd_frame into the buffer.
                    let frame = unsafe { frame.assume_init() };
                    Ok(<canfd_frame as Into<CanFdFrame>>::into(frame).into())
                } else {
                    Err(io::Error::last_os_error())
                }
            })
            .await
    }
}
