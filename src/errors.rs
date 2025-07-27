// socketcan/src/errors.rs
//
// Implements errors for Rust SocketCAN library on Linux.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! CAN bus errors.
//!
//! Most information about the errors on the CANbus are determined from an
//! error frame. To receive them, the error mask must be set on the socket
//! for the types of errors that the application would like to receive.
//!
//! See [RAW Socket Option CAN_RAW_ERR_FILTER](https://docs.kernel.org/networking/can.html#raw-socket-option-can-raw-err-filter)
//!
//! The general types of errors are encoded in the error bits of the CAN ID
//! of an error frame. This is reported with [`CanError`]. Specific errors
//! might indicate that more information can be obtained from the data bytes
//! in the error frame.
//!
//! ```text
//! Lost Arbitration   (0x02) => data[0]
//! Controller Problem (0x04) => data[1]
//! Protocol Violation (0x08) => data[2..3]
//! Transceiver Status (0x10) => data[4]
//!
//! Error Counters (0x200) =>
//!   TX Error Counter => data[6]
//!   RX Error Counter => data[7]
//! ```
//!
//! All of this error information is not well documented, but can be extracted
//! from the Linux kernel header file:
//! [linux/can/error.h](https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/can/error.h)
//!

use crate::{CanErrorFrame, EmbeddedFrame, Frame};
use socketcan_raw::{CanError, ControllerProblem, Error, Location, ViolationType};

impl From<CanErrorFrame> for Error {
    fn from(frame: CanErrorFrame) -> Self {
        Error::Can(CanError::from(frame))
    }
}

impl From<CanErrorFrame> for CanError {
    /// Constructs a CAN error from an error frame.
    fn from(frame: CanErrorFrame) -> Self {
        // Note that the CanErrorFrame is guaranteed to have the full 8-byte
        // data payload.
        match frame.error_bits() {
            0x0001 => CanError::TransmitTimeout,
            0x0002 => CanError::LostArbitration(frame.data()[0]),
            0x0004 => match ControllerProblem::try_from(frame.data()[1]) {
                Ok(err) => CanError::ControllerProblem(err),
                Err(err) => CanError::DecodingFailure(err),
            },
            0x0008 => {
                match (
                    ViolationType::try_from(frame.data()[2]),
                    Location::try_from(frame.data()[3]),
                ) {
                    (Ok(vtype), Ok(location)) => CanError::ProtocolViolation { vtype, location },
                    (Err(err), _) | (_, Err(err)) => CanError::DecodingFailure(err),
                }
            }
            0x0010 => CanError::TransceiverError,
            0x0020 => CanError::NoAck,
            0x0040 => CanError::BusOff,
            0x0080 => CanError::BusError,
            0x0100 => CanError::Restarted,
            err => CanError::Unknown(err),
        }
    }
}

/// Get the controller specific error information.
pub trait ControllerSpecificErrorInformation {
    /// Get the controller specific error information.
    fn get_ctrl_err(&self) -> Option<&[u8]>;
}

impl<T: Frame> ControllerSpecificErrorInformation for T {
    /// Get the controller specific error information.
    fn get_ctrl_err(&self) -> Option<&[u8]> {
        let data = self.data();

        if data.len() == 8 {
            Some(&data[5..])
        } else {
            None
        }
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use socketcan_raw::Error;
    use std::io;

    #[test]
    fn test_errors() {
        const KIND: io::ErrorKind = io::ErrorKind::TimedOut;

        // From an IO error.
        let err = Error::from(io::Error::from(KIND));
        if let Error::Io(ioerr) = err {
            assert_eq!(ioerr.kind(), KIND);
        } else {
            panic!("Wrong error conversion");
        }

        // Straight from an ErrorKind
        let err = Error::from(KIND);
        if let Error::Io(ioerr) = err {
            assert_eq!(ioerr.kind(), KIND);
        } else {
            panic!("Wrong error conversion");
        }
    }
}
