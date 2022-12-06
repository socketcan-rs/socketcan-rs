//! SocketCAN support.
//!
//! The Linux kernel supports using CAN-devices through a network-like API
//! (see <https://www.kernel.org/doc/Documentation/networking/can.txt>). This
//! crate allows easy access to this functionality without having to wrestle
//! libc calls.
//!
//! # An introduction to CAN
//!
//! The CAN bus was originally designed to allow microcontrollers inside a
//! vehicle to communicate over a single shared bus. Messages called
//! *frames* are multicast to all devices on the bus.
//!
//! Every frame consists of an ID and a payload of up to 8 bytes. If two
//! devices attempt to send a frame at the same time, the device with the
//! higher ID will notice the conflict, stop sending and reattempt to sent its
//! frame in the next time slot. This means that the lower the ID, the higher
//! the priority. Since most devices have a limited buffer for outgoing frames,
//! a single device with a high priority (== low ID) can block communication
//! on that bus by sending messages too fast.
//!
//! The Linux socketcan subsystem makes the CAN bus available as a regular
//! networking device. Opening an network interface allows receiving all CAN
//! messages received on it. A device CAN be opened multiple times, every
//! client will receive all CAN frames simultaneously.
//!
//! Similarly, CAN frames can be sent to the bus by multiple client
//! simultaneously as well.
//!
//! # Hardware and more information
//!
//! More information on CAN [can be found on Wikipedia](). When not running on
//! an embedded platform with already integrated CAN components,
//! [Thomas Fischl's USBtin](http://www.fischl.de/usbtin/) (see
//! [section 2.4](http://www.fischl.de/usbtin/#socketcan)) is one of many ways
//! to get started.
//!
//! # RawFd
//!
//! Raw access to the underlying file descriptor and construction through
//! is available through the `AsRawFd`, `IntoRawFd` and `FromRawFd`
//! implementations.
//!
//! # Crate Features
//!
//! ### Default
//!
//! * **netlink** -
//!   Whether to include programmable CAN interface configuration capabilities
//!   based on netlink kernel communications. This brings in the
//!   [neli](https://docs.rs/neli/latest/neli/) library and its dependencies.
//!
//! ### Non-default
//!
//! * **utils** -
//!   Whether to build command-line utilities. This brings in additional
//!   dependencies like [anyhow](https://docs.rs/anyhow/latest/anyhow/) and
//!   [clap](https://docs.rs/clap/latest/clap/)
//!

// clippy: do not warn about things like "SocketCAN" inside the docs
#![allow(clippy::doc_markdown)]

pub mod err;
pub use err::{CanError, CanErrorDecodingFailure, CanSocketOpenError, ConstructionError};

pub mod frame;
pub use frame::{CanAnyFrame, CanFdFrame, CanFrame, Frame};

pub mod dump;

pub mod socket;
pub use socket::{CanFdSocket, CanFilter, CanSocket, ShouldRetry, Socket};

mod util;

#[cfg(feature = "netlink")]
mod nl;

#[cfg(feature = "netlink")]
pub use nl::CanInterface;

use std::io::ErrorKind;

impl embedded_can::blocking::Can for CanSocket {
    type Frame = CanFrame;
    type Error = CanError;

    fn receive(&mut self) -> Result<Self::Frame, Self::Error> {
        match self.read_frame() {
            Ok(frame) => {
                if !frame.is_error() {
                    Ok(frame)
                } else {
                    Err(frame.error().unwrap_or(CanError::Unknown(0)))
                }
            }
            Err(e) => {
                let code = e.raw_os_error().unwrap_or(0);
                Err(CanError::Unknown(code as u32))
            }
        }
    }

    fn transmit(&mut self, frame: &Self::Frame) -> Result<(), Self::Error> {
        match self.write_frame_insist(frame) {
            Ok(_) => Ok(()),
            Err(e) => {
                let code = e.raw_os_error().unwrap_or(0);
                Err(CanError::Unknown(code as u32))
            }
        }
    }
}

impl embedded_can::nb::Can for CanSocket {
    type Frame = CanFrame;
    type Error = CanError;

    fn receive(&mut self) -> nb::Result<Self::Frame, Self::Error> {
        match self.read_frame() {
            Ok(frame) => {
                if !frame.is_error() {
                    Ok(frame)
                } else {
                    let can_error = frame.error().unwrap_or(CanError::Unknown(0));
                    Err(nb::Error::Other(can_error))
                }
            }
            Err(e) => {
                let e = match e.kind() {
                    ErrorKind::WouldBlock => nb::Error::WouldBlock,
                    _ => {
                        let code = e.raw_os_error().unwrap_or(0);
                        nb::Error::Other(CanError::Unknown(code as u32))
                    }
                };
                Err(e)
            }
        }
    }

    fn transmit(&mut self, frame: &Self::Frame) -> nb::Result<Option<Self::Frame>, Self::Error> {
        match self.write_frame(&frame) {
            Ok(_) => Ok(None),
            Err(e) => {
                match e.kind() {
                    ErrorKind::WouldBlock => Err(nb::Error::WouldBlock),
                    // TODO: How to indicate buffer is full?
                    // ErrorKind::StorageFull => Ok(frame),
                    _ => {
                        let code = e.raw_os_error().unwrap_or(0);
                        Err(nb::Error::Other(CanError::Unknown(code as u32)))
                    }
                }
            }
        }
    }
}
