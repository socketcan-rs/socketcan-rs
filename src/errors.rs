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

use num_derive::FromPrimitive;    
use num_traits::FromPrimitive;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use crate::{CanErrorFrame, EmbeddedFrame, Frame};
use std::{convert::TryFrom, error, fmt, io};
use thiserror::Error;

// ===== Composite Error for the crate =====

/// Composite SocketCAN error.
///
/// This can be any of the underlying errors from this library. The two main
/// error sources are either CAN errors coming in through received error
/// frames or from typical system I/O errors.
#[derive(Error, Debug)]
pub enum Error {
    /// A CANbus error, usually from an error frame
    #[error(transparent)]
    Can(#[from] CanError),
    /// An I/O Error
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl embedded_can::Error for Error {
    fn kind(&self) -> embedded_can::ErrorKind {
        match self {
            Error::Can(err) => err.kind(),
            _ => embedded_can::ErrorKind::Other,
        }
    }
}

impl From<CanErrorFrame> for Error {
    fn from(frame: CanErrorFrame) -> Self {
        Error::Can(CanError::from(frame))
    }
}

impl From<io::ErrorKind> for Error {
    /// Creates an Io error straight from an io::ErrorKind
    fn from(kind: io::ErrorKind) -> Self {
        Self::from(io::Error::from(kind))
    }
}

#[cfg(feature = "enumerate")]
impl From<libudev::Error> for Error {
    /// Creates an Io error from a libudev::Error, preserving the underlying
    /// description as the `io::Error` message.
    fn from(e: libudev::Error) -> Error {
        let kind = match e.kind() {
            libudev::ErrorKind::NoMem => io::ErrorKind::OutOfMemory,
            libudev::ErrorKind::InvalidInput => io::ErrorKind::InvalidInput,
            libudev::ErrorKind::Io(kind) => kind,
        };
        Self::Io(io::Error::new(kind, e.to_string()))
    }
}

#[cfg(feature = "netlink")]
impl<T, P> From<neli::err::NlError<T, P>> for Error
where
    T: neli::consts::nl::NlType,
    P: fmt::Debug,
{
    /// Wraps a netlink error as an [`io::Error`] of kind `Other`, preserving
    /// the underlying description. Lets callers `?` netlink results across
    /// module boundaries into the crate-level [`enum@Error`].
    fn from(e: neli::err::NlError<T, P>) -> Error {
        Self::Io(io::Error::other(e.to_string()))
    }
}

#[cfg(feature = "dump")]
impl From<crate::dump::ParseError> for Error {
    /// Maps a [`ParseError`](crate::dump::ParseError) into an [`io::Error`]
    /// of kind `InvalidData`, preserving the description. Lets callers `?`
    /// dump-parsing results into the crate-level [`enum@Error`].
    fn from(e: crate::dump::ParseError) -> Error {
        use crate::dump::ParseError;
        match e {
            ParseError::Io(io_err) => Self::Io(io_err),
            other => Self::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                other.to_string(),
            )),
        }
    }
}

/// A result that can derive from any of the CAN errors.
pub type Result<T> = std::result::Result<T, Error>;

/// An I/O specific error
pub type IoError = io::Error;

/// A kind of I/O error
pub type IoErrorKind = io::ErrorKind;

/// An I/O specific result
pub type IoResult<T> = io::Result<T>;


/// Error status of the CAN controller.
///
/// This is derived from `data[1]` of an error frame
/// see original in https://github.com/torvalds/linux/blob/master/include/uapi/linux/can/error.h
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, FromPrimitive)]
#[repr(u32)]
pub enum CanErrorFlags {
    /// TX timeout (by netdevice driver)
    TxTimeout = 0x00000001,
    /// Lost arbitration / data[0]
    LostArbitration = 0x00000002,
    /// Controller problems / data[1]
    ControllerProblems = 0x00000004,
    /// Protocol violations / data[2..3]
    ProtocolViolations = 0x00000008,
    /// Transceiver status / data[4]
    TransceiverStatus = 0x00000010,
    /// Received no ACK on transmission
    NoAck = 0x00000020,
    /// Bus off
    BusOff = 0x00000040,
    /// Bus error (may flood!)
    BusError = 0x00000080,
    /// Controller restarted
    Restarted = 0x00000100,
    /// TX error counter / data[6] RX error counter / data[7]
    ErrorCounter = 0x00000200,
}

// ===== CanError ====

/// A CAN bus error derived from an error frame.
///
/// An CAN interface device driver can send detailed error information up
/// to the application in an "error frame". These are selectable by the
/// application by applying an error bitmask to the socket to choose which
/// types of errors to receive.
///
/// The error frame can then be converted into this `CanError` which is a
/// proper Rust error type which implements std::error::Error.
///
/// Most error types here corresponds to a bit in the error mask of a CAN ID
/// word of an error frame - a frame in which the CAN error flag
/// (`CAN_ERR_FLAG`) is set. But there are additional types to handle any
/// problems decoding the error frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanError {
    /// TX timeout (by netdevice driver)
    pub transmit_timeout: bool,
    /// Arbitration was lost. data[0]
    /// Contains the bit number after which arbitration was lost or 0 if unspecified.
    pub lost_arbitration: Option<LostArbitration>,
    /// Controller problem(s) data[1]
    pub controller_problems: Vec<ControllerProblem>,
    /// Protocol violation(s) at the specified [`Location`] data[2]
    pub protocol_violations: Vec<ViolationType>,
    /// protocol violation location data[3]
    pub location: Option<Location>,
    /// Transceiver Error
    pub transceiver_error: Option<TransceiverError>,
    /// No ACK received for current CAN frame.
    pub no_ack: bool,
    /// Bus off (due to too many detected errors)
    pub bus_off: bool,
    /// Bus error (due to too many detected errors)
    pub bus_error: bool,
    /// The bus has been restarted
    pub restarted: bool,
    /// error counters, tx data[6] and rx data[7]. filled if either the CAN_ERR_CNT bit is set or bytes 6-7 are nonzero
    pub error_counts: Option<ErrorCounts>,    
    /// list of errors decoding the error frame
    pub decoding_failures: Vec<CanErrorDecodingFailure>,
    /// Unknown, possibly invalid, error
    pub unknown: Option<u32>,
}

impl error::Error for CanError {}

impl fmt::Display for CanError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut parts = Vec::new();

        if self.transmit_timeout {
            parts.push("transmission timeout".to_string());
        }
        if let Some(ref lost_arbitration) = self.lost_arbitration {
            parts.push(format!("arbitration lost at bit {}", lost_arbitration.bit));
        }
        if !self.controller_problems.is_empty() {
            let mut controller_problems_line: String = String::new();
            if self.controller_problems.len() > 1 {
                controller_problems_line.push_str("multiple controller problems: ");
            } else {
                controller_problems_line.push_str("controller problem: ");
            }
            controller_problems_line.push_str(&format!(
                "{}",
                self.controller_problems
                    .iter()
                    .map(|err| format!("{}", err))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            parts.push(controller_problems_line);
        }
        if !self.protocol_violations.is_empty() {
            let mut protocol_violations_line: String = String::new();
            if self.protocol_violations.len() > 1 {
                protocol_violations_line.push_str("multiple protocol violations: ");
            } else {
                protocol_violations_line.push_str("protocol violation: ");
            }
            protocol_violations_line.push_str(&format!(
                "{}",
                self.protocol_violations
                    .iter()
                    .map(|err| format!("{}", err))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if let Some(ref location) = self.location {
                protocol_violations_line.push_str(&format!(" at {}", location));
            } else {
                protocol_violations_line.push_str(" (location decoding failed)");
            }
            parts.push(protocol_violations_line);
        }
        if let Some(ref transceiver_error) = self.transceiver_error {
            parts.push(format!("transceiver error: {}", transceiver_error));
        }
        if self.no_ack {
            parts.push("no ack on tx".to_string());
        }
        if self.bus_off {
            parts.push("bus off".to_string());
        }
        if self.bus_error {
            parts.push("bus error".to_string());
        }
        if self.restarted {
            parts.push("restarted after bus off".to_string());
        }
        if let Some(ref error_counts) = self.error_counts {
            parts.push(format!("error counts: tx {}, rx {}", error_counts.tx, error_counts.rx));
        }
        for err in &self.decoding_failures {
            parts.push(format!("decoding failure: {}", err));
        }
        if let Some(ref unknown) = self.unknown {
            parts.push(format!("unknown error ({})", unknown));
        }

        write!(f, "{}", parts.join("\n"))
    }
}

impl embedded_can::Error for CanError {
    fn kind(&self) -> embedded_can::ErrorKind {
        // a single error frame could translate multiple ways. just match the first way found
        // there are problably a few more translations possible
        if !self.controller_problems.is_empty() {
            use ControllerProblem::*;
            for controller_problem in &self.controller_problems {
                if *controller_problem == ReceiveBufferOverflow || *controller_problem == TransmitBufferOverflow {
                    return embedded_can::ErrorKind::Overrun
                }
            }
            return embedded_can::ErrorKind::Other;
        }

        if self.no_ack {
            return embedded_can::ErrorKind::Acknowledge;
        }
        return embedded_can::ErrorKind::Other
    }
}

impl From<CanErrorFrame> for CanError {
    /// Constructs a CAN error from an error frame.
    fn from(frame: CanErrorFrame) -> Self {
        // Note that the CanErrorFrame is guaranteed to have the full 8-byte
        // data payload.
        let mut can_error: CanError = CanError::default();
        if (frame.error_bits() & (CanErrorFlags::TxTimeout as u32)) != 0 {
            can_error.transmit_timeout = true;
        }
        if (frame.error_bits() & (CanErrorFlags::LostArbitration as u32)) != 0 {
            can_error.lost_arbitration = Some(LostArbitration {
                bit: frame.data()[0],
            });
        }
        if (frame.error_bits() & (CanErrorFlags::ControllerProblems as u32)) != 0 {
            for problem in ControllerProblem::iter() {
                if (frame.data()[1] & problem as u8) != 0 {
                    can_error.controller_problems.push(problem);
                }
            }
            // CAN_ERR_CRTL is set, but no controller problems were found
            if can_error.controller_problems.is_empty() {
                can_error.decoding_failures.push(CanErrorDecodingFailure::CtrlBitSetButNoneFound);
            }
        }
        if (frame.error_bits() & (CanErrorFlags::ProtocolViolations as u32)) != 0 {
            for vtype in ViolationType::iter() {
                if (frame.data()[2] & vtype as u8) != 0 {
                    can_error.protocol_violations.push(vtype);
                }
            }
            // CAN_ERR_PROT is set, but no protocol violations were found
            if can_error.protocol_violations.is_empty() {
                can_error.decoding_failures.push(CanErrorDecodingFailure::ProtBitSetButNoneFound);
            }
            match Location::try_from(frame.data()[3]) {
                Ok(location) => can_error.location = Some(location),
                Err(err) => {
                    can_error.decoding_failures.push(err);
                    // leave location as None
                }
            }
        }
        if (frame.error_bits() & (CanErrorFlags::TransceiverStatus as u32)) != 0 {
            match TransceiverError::try_from(frame.data()[4]) {
                Ok(err) => can_error.transceiver_error = Some(err),
                Err(err) => can_error.decoding_failures.push(err),
            }
        }
        if (frame.error_bits() & (CanErrorFlags::NoAck as u32)) != 0 {
            can_error.no_ack = true;
        }
        if (frame.error_bits() & (CanErrorFlags::BusOff as u32)) != 0 {
            can_error.bus_off = true;
        }
        if (frame.error_bits() & (CanErrorFlags::BusError as u32)) != 0 {
            can_error.bus_error = true;
        }
        if (frame.error_bits() & (CanErrorFlags::Restarted as u32)) != 0 {
            can_error.restarted = true;
        }
        if (frame.error_bits() & (CanErrorFlags::ErrorCounter as u32)) != 0 {
            can_error.error_counts = Some(ErrorCounts {
                tx: frame.data()[6],
                rx: frame.data()[7],
            });
        } else if frame.data()[6] != 0 || frame.data()[7] != 0 {
            can_error.error_counts = Some(ErrorCounts {
                tx: frame.data()[6],
                rx: frame.data()[7],
            });
        }
        can_error
    }
}

// ===== LostArbitration =====

/// populated if CAN_ERR_LOSTARB is set
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LostArbitration {
    /// The bit number after which arbitration was lost
    pub bit: u8,
}

// ===== ControllerProblem =====

/// Error status of the CAN controller.
///
/// This is derived from `data[1]` of an error frame
/// the flags don't conflict so there can be multiple problems
/// see original in https://github.com/torvalds/linux/blob/master/include/uapi/linux/can/error.h
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, FromPrimitive, EnumIter)]
#[repr(u8)]
pub enum ControllerProblem {
    /// unspecified
    Unspecified = 0x00,
    /// RX buffer overflow
    ReceiveBufferOverflow = 0x01,
    /// TX buffer overflow
    TransmitBufferOverflow = 0x02,
    /// reached warning level for RX errors
    ReceiveErrorWarning = 0x04,
    /// reached warning level for TX errors
    TransmitErrorWarning = 0x08,
    /// reached error passive status RX
    ReceiveErrorPassive = 0x10,
    /// reached error passive status TX
    TransmitErrorPassive = 0x20,
    /// recovered to error active state
    BackToErrorActive = 0x40,
}

impl error::Error for ControllerProblem {}
impl fmt::Display for ControllerProblem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ControllerProblem::*;
        let msg = match *self {
            Unspecified => "unspecified controller problem",
            ReceiveBufferOverflow => "receive buffer overflow",
            TransmitBufferOverflow => "transmit buffer overflow",
            ReceiveErrorWarning => "ERROR WARNING (receive)",
            TransmitErrorWarning => "ERROR WARNING (transmit)",
            ReceiveErrorPassive => "ERROR PASSIVE (receive)",
            TransmitErrorPassive => "ERROR PASSIVE (transmit)",
            BackToErrorActive => "back to ERROR ACTIVE",
        };
        write!(f, "{}", msg)
    }
}

// ===== ViolationType =====

/// populated if CAN_ERR_PROT is set. This is derived from `data[2]` of an error frame.
/// the flags don't conflict so there can be multiple types, but only one location (in data[3])
/// see original in https://github.com/torvalds/linux/blob/master/include/uapi/linux/can/error.h
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive, EnumIter)]
#[repr(u8)]
pub enum ViolationType {
    /// Unspecified Violation
    Unspecified = 0x00,
    /// Single Bit Error
    SingleBitError = 0x01,
    /// Frame formatting error
    FrameFormatError = 0x02,
    /// Bit stuffing error
    BitStuffingError = 0x04,
    /// A dominant bit was sent, but not received
    UnableToSendDominantBit = 0x08,
    /// A recessive bit was sent, but not received
    UnableToSendRecessiveBit = 0x10,
    /// Bus overloaded
    BusOverload = 0x20,
    /// Bus is active (again)
    Active = 0x40,
    /// Transmission Error
    TransmissionError = 0x80,
}

impl error::Error for ViolationType {}

impl fmt::Display for ViolationType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ViolationType::*;
        let msg = match *self {
            Unspecified => "unspecified",
            SingleBitError => "single bit error",
            FrameFormatError => "frame format error",
            BitStuffingError => "bit stuffing error",
            UnableToSendDominantBit => "unable to send dominant bit",
            UnableToSendRecessiveBit => "unable to send recessive bit",
            BusOverload => "bus overload",
            Active => "active",
            TransmissionError => "transmission error",
        };
        write!(f, "{}", msg)
    }
}

/// The location of a CANbus protocol violation.
///
/// This describes the position inside a received frame (as in the field
/// or bit) at which an error occurred.
///
/// This is derived from `data[3]` of an error frame.
/// see original in https://github.com/torvalds/linux/blob/master/include/uapi/linux/can/error.h
#[derive(Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq, FromPrimitive)]
#[repr(u8)]
pub enum Location {
    /// Unspecified
    Unspecified = 0x00,
    /// Start of frame
    StartOfFrame = 0x03,
    /// ID bits 28-21 (SFF: 10-3)
    Id2821 = 0x02,
    /// ID bits 20-18 (SFF: 2-0)
    Id2018 = 0x06,
    /// substitute RTR (SFF: RTR)
    SubstituteRtr = 0x04,
    /// extension of identifier
    IdentifierExtension = 0x05,
    /// ID bits 17-13
    Id1713 = 0x07,
    /// ID bits 12-5
    Id1205 = 0x0F,
    /// ID bits 4-0
    Id0400 = 0x0E,
    /// RTR bit
    Rtr = 0x0C,
    /// Reserved bit 1
    Reserved1 = 0x0D,
    /// Reserved bit 0
    Reserved0 = 0x09,
    /// Data length
    DataLengthCode = 0x0B,
    /// Data section
    DataSection = 0x0A,
    /// CRC sequence
    CrcSequence = 0x08,
    /// CRC delimiter
    CrcDelimiter = 0x18,
    /// ACK slot
    AckSlot = 0x19,
    /// ACK delimiter
    AckDelimiter = 0x1B,
    /// End-of-frame
    EndOfFrame = 0x1A,
    /// Intermission (between frames)
    Intermission = 0x12,

    /// the following aren't in the current linux error.h but are valid
    /// Active Error Flag
    ActiveErrorFlag = 0x11,
    /// Passive Error Flag
    PassiveErrorFlag = 0x16,
    /// Tolerate Dominant Bits
    TolerateDominantBits = 0x13,
    /// Error Delimiter
    ErrorDelimiter = 0x17,
    /// Overload Flag
    OverloadFlag = 0x1C,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use Location::*;
        let msg = match *self {
            Unspecified => "unspecified location",
            StartOfFrame => "start of frame",
            Id2821 => "ID, bits 28-21",
            Id2018 => "ID, bits 20-18",
            SubstituteRtr => "substitute RTR bit",
            IdentifierExtension => "ID, extension",
            Id1713 => "ID, bits 17-13",
            Id1205 => "ID, bits 12-05",
            Id0400 => "ID, bits 04-00",
            Rtr => "RTR bit",
            Reserved1 => "reserved bit 1",
            Reserved0 => "reserved bit 0",
            DataLengthCode => "data length code",
            DataSection => "data section",
            CrcSequence => "CRC sequence",
            CrcDelimiter => "CRC delimiter",
            AckSlot => "ACK slot",
            AckDelimiter => "ACK delimiter",
            EndOfFrame => "end of frame",
            Intermission => "intermission",
            ActiveErrorFlag => "active error flag",
            PassiveErrorFlag => "passive error flag",
            TolerateDominantBits => "tolerate dominant bits",
            ErrorDelimiter => "error delimiter",
            OverloadFlag => "overload flag",
        };
        write!(f, "{}", msg)
    }
}

impl TryFrom<u8> for Location {
    type Error = CanErrorDecodingFailure;

    fn try_from(val: u8) -> std::result::Result<Self, Self::Error> {
        Location::from_u8(val).ok_or(CanErrorDecodingFailure::InvalidProtocolViolationLocation)
    }
}

// ===== TransceiverError =====

/// The error status of the CAN transceiver.
///
/// This is derived from `data[4]` of an error frame if CAN_ERR_TRX is set
/// see original in https://github.com/torvalds/linux/blob/master/include/uapi/linux/can/error.h
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, FromPrimitive)]
#[repr(u8)]
pub enum TransceiverError {
    /// Unsecified
    Unspecified = 0x00,
    /// CAN High, no wire
    CanHighNoWire = 0x04,
    /// CAN High, short to BAT
    CanHighShortToBat = 0x05,
    /// CAN High, short to VCC
    CanHighShortToVcc = 0x06,
    /// CAN High, short to GND
    CanHighShortToGnd = 0x07,
    /// CAN Low, no wire
    CanLowNoWire = 0x40,
    /// CAN Low, short to BAT
    CanLowShortToBat = 0x50,
    /// CAN Low, short to VCC
    CanLowShortToVcc = 0x60,
    /// CAN Low, short to GND
    CanLowShortToGnd = 0x70,
    /// CAN Low short to  CAN High
    CanLowShortToCanHigh = 0x80,
}

impl TryFrom<u8> for TransceiverError {
    type Error = CanErrorDecodingFailure;

    fn try_from(val: u8) -> std::result::Result<Self, Self::Error> {
        TransceiverError::from_u8(val).ok_or(CanErrorDecodingFailure::InvalidTransceiverError)
    }
}

impl fmt::Display for TransceiverError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use TransceiverError::*;
        let msg = match *self {
            Unspecified => "unspecified",
            CanHighNoWire => "CAN High, no wire",
            CanHighShortToBat => "CAN High, short to BAT",
            CanHighShortToVcc => "CAN High, short to VCC",
            CanHighShortToGnd => "CAN High, short to GND",
            CanLowNoWire => "CAN Low, no wire",
            CanLowShortToBat => "CAN Low, short to BAT",
            CanLowShortToVcc => "CAN Low, short to VCC",
            CanLowShortToGnd => "CAN Low, short to GND",
            CanLowShortToCanHigh => "CAN Low, short to CAN High",
        };
        write!(f, "{}", msg)
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

// ===== CanErrorDecodingFailure =====

/// Error decoding a CanError from a CanErrorFrame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CanErrorDecodingFailure {
    /// The supplied CANFrame did not have the error bit set.
    NotAnError,
    /// The error type is not known and cannot be decoded.
    UnknownErrorType(u32),
    /// The error type indicated a need for additional information as `data`,
    /// but the `data` field was not long enough.
    NotEnoughData(u8),
    /// The error type `ControllerProblem` was indicated but none of the data[1] bits decoded to any problem
    CtrlBitSetButNoneFound,
    /// the error type `ProtocolViolation` was indicated but none of the data[2] bits decoded to a valid type
    ProtBitSetButNoneFound,
    /// A location was specified for a ProtocolViolation, but the location
    /// was not valid.
    InvalidProtocolViolationLocation,
    /// The supplied transceiver error was invalid.
    InvalidTransceiverError,
}

impl error::Error for CanErrorDecodingFailure {}

impl fmt::Display for CanErrorDecodingFailure {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use CanErrorDecodingFailure::*;
        let msg = match *self {
            NotAnError => "CAN frame is not an error",
            UnknownErrorType(_) => "unknown error type",
            NotEnoughData(_) => "not enough data",
            CtrlBitSetButNoneFound => "controller problem bit set, but no problems found",
            ProtBitSetButNoneFound => "protocol problem bit set, but no violations found",
            InvalidProtocolViolationLocation => "not a valid protocol violation location",
            InvalidTransceiverError => "not a valid transceiver error",
        };
        write!(f, "{}", msg)
    }
}

/// populated if CAN_ERR_CNT is set *or* if either error count is nonzero
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ErrorCounts {
    /// TX error counter data[6]
    pub tx: u8,
    /// RX error counter data[7]
    pub rx: u8,
}

// ===== ConstructionError =====

#[derive(Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
/// Error that occurs when creating CAN packets
pub enum ConstructionError {
    /// Trying to create a specific frame type from an incompatible type
    WrongFrameType,
    /// CAN ID was outside the range of valid IDs
    IDTooLarge,
    /// Larger payload reported than can be held in the frame.
    TooMuchData,
}

impl error::Error for ConstructionError {}

impl fmt::Display for ConstructionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ConstructionError::*;
        let msg = match *self {
            WrongFrameType => "Incompatible frame type",
            IDTooLarge => "CAN ID too large",
            TooMuchData => "Payload is too large",
        };
        write!(f, "{}", msg)
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use embedded_can::{ExtendedId, Frame};

    use crate::{CanErrorFrame, Error};
    use std::io;

    use super::CanError;

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

    #[test]
    fn test_error_printing() {
        // compare our error printing to the printing in linux can-util's C implementation
        // see snprintf_can_error_frame() in https://github.com/linux-can/can-utils/blob/master/lib.c

        // snprintf_can_error_frame():
        // 1. prints the error class
        // 2. prints information for each of CAN_ERR_LOSTARB, CAN_ERR_CRTL, CAN_ERR_PROT, CAN_ERR_CNT, if flags are set
        //  a. CAN_ERR_TRX details in data[4] aren't printed (I think this is an omission)
        // 3. prints error counts if they're nonzero even if CAN_ERR_CNT wasn't set

        // for example:
        // candump -c -ta -H -d -e -x vcan0,0:0,#FFFFFFFF &
        // cansend vcan0 200001FF#00.01.02.03.04.05.06.07 (would be "3FF" for all flags, but a bug in 2023 and earlier candump versions prevents printing if CAN_ERR_CNT is set)
        // prints:
        // (0000000000.000000)  vcan0  TX - -  200001FF   [8]  00 01 02 03 04 05 06 07   ERRORFRAME
        //    tx-timeout
        //    lost-arbitration{at bit 0}
        //    controller-problem{rx-overflow}
        //    protocol-violation{{frame-format-error}{start-of-frame}}
        //    transceiver-status
        //    no-acknowledgement-on-tx
        //    bus-off
        //    bus-error
        //    restarted-after-bus-off
        //    error-counter-tx-rx{{6}{7}}

        // create a can frame with id 200001FF and data 00 01 02 03 04 05 06 07 to match the candump test value above
        let id = ExtendedId::new(0x1FF).unwrap(); // the leading 0x2 CAN_ERR_FLAG is built in
        let frame = CanErrorFrame::new(id, &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]).unwrap();

        let can_error: CanError = frame.into();
        assert_eq!(format!("{}", can_error), r#"transmission timeout
arbitration lost at bit 0
controller problem: receive buffer overflow
protocol violation: frame format error at start of frame
transceiver error: CAN High, no wire
no ack on tx
bus off
bus error
restarted after bus off
error counts: tx 6, rx 7"#);

        // same, but error values where possible
        // shows multiple controller problems and multiple protocol violations

        // candump comparison:
        // candump -c -ta -H -d -e -x vcan0,0:0,#FFFFFFFF &
        // cansend vcan0 200001FF#FE.FE.FE.FE.FE.FE.FE.FE (would be "3FF" for all flags, but a bug in 2023 and earlier candump versions prevents printing if CAN_ERR_CNT is set)
        // prints:
        // (0000000000.000000)  vcan0  TX - -  200001FF   [8]  FE FE FE FE FE FE FE FE   ERRORFRAME
        //    tx-timeout
        //    lost-arbitration{at bit 254}
        //    controller-problem{tx-overflow,rx-error-warning,tx-error-warning,rx-error-passive,tx-error-passive,back-to-error-active}
        //    protocol-violation{{frame-format-error,bit-stuffing-error,tx-dominant-bit-error,tx-recessive-bit-error,bus-overload,active-error,error-on-tx}{}}
        //    transceiver-status
        //    no-acknowledgement-on-tx
        //    bus-off
        //    bus-error
        //    restarted-after-bus-off
        //    error-counter-tx-rx{{254}{254}}

        let frame = CanErrorFrame::new(id, &[0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE, 0xFE]).unwrap();
        let can_error: CanError = frame.into();
        assert_eq!(format!("{}", can_error), r#"transmission timeout
arbitration lost at bit 254
multiple controller problems: transmit buffer overflow, ERROR WARNING (receive), ERROR WARNING (transmit), ERROR PASSIVE (receive), ERROR PASSIVE (transmit), back to ERROR ACTIVE
multiple protocol violations: frame format error, bit stuffing error, unable to send dominant bit, unable to send recessive bit, bus overload, active, transmission error (location decoding failed)
no ack on tx
bus off
bus error
restarted after bus off
error counts: tx 254, rx 254
decoding failure: not a valid protocol violation location
decoding failure: not a valid transceiver error"#);
    }
}
