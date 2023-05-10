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

use std::io;

use async_io::Async;

use crate::{frame::AsPtr, CanAnyFrame, CanFrame, Socket};

macro_rules! create_async_socket {
    ($target_type:ident, $wrapped_type:ty, $frame_type:ty) => {
        #[doc = concat!("Async version of ", stringify!($wrapped_type),". See the original type's documentation for details.")]
        #[allow(missing_copy_implementations)]
        #[derive(Debug)]
        pub struct $target_type {
            inner: Async<$wrapped_type>,
        }

        impl TryFrom<$wrapped_type> for $target_type {
            type Error = io::Error;

            fn try_from(value: $wrapped_type) -> Result<Self, Self::Error> {
                Ok(Self {
                    inner: Async::new(value)?,
                })
            }
        }

        impl $target_type {
            /// Open a named CAN device.
            ///
            /// Usually the more common case, opens a socket can device by name, such
            /// as "can0", "vcan0", or "socan0".
            pub fn open(ifname: &str) -> io::Result<Self> {
                <$wrapped_type>::open(ifname)?.try_into()
            }

            /// Permits access to the inner synchronous socket, for example to change options.
            pub fn blocking(&self) -> &$wrapped_type {
                self.inner.as_ref()
            }

            /// Permits mutable access to the inner synchronous socket, for example to change options.
            pub fn blocking_mut(&mut self) -> &mut $wrapped_type {
                self.inner.as_mut()
            }

            /// Writes a frame to the socket asynchronously.
            pub async fn write_frame<F>(&self, frame: &F) -> io::Result<()>
            where
                F: Into<$frame_type> + AsPtr,
            {
                self.inner.write_with(|fd| fd.write_frame(frame)).await
            }

            /// Reads a frame from the socket asynchronously.
            pub async fn read_frame(&self) -> io::Result<$frame_type> {
                self.inner.read_with(|fd| fd.read_frame()).await
            }
        }
    };
}

create_async_socket!(CanSocket, super::CanSocket, CanFrame);
create_async_socket!(CanFdSocket, super::CanFdSocket, CanAnyFrame);
