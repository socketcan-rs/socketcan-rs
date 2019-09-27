// information from https://raw.githubusercontent.com/torvalds/linux/master/
//                  /include/uapi/linux/can/error.h

use std::convert::TryFrom;
use super::CANFrame;


#[inline(always)]
/// Helper function to retrieve a specific byte of frame data or returning an
/// `Err(..)` otherwise.
fn get_data(frame: &CANFrame, idx: u8) -> Result<u8, CANErrorDecodingFailure> {
    Ok(*r#try!(frame.data().get(idx as usize).ok_or(CANErrorDecodingFailure::NotEnoughData(idx))))
}


/// Error decoding a CANError from a CANFrame.
#[derive(Copy, Clone, Debug)]
pub enum CANErrorDecodingFailure {
    /// The supplied CANFrame did not have the error bit set.
    NotAnError,

    /// The error type is not known and cannot be decoded.
    UnknownErrorType(u32),

    /// The error type indicated a need for additional information as `data`,
    /// but the `data` field was not long enough.
    NotEnoughData(u8),

    /// The error type `ControllerProblem` was indicated and additional
    /// information found, but not recognized.
    InvalidControllerProblem,

    /// The type of the ProtocolViolation was not valid
    InvlaidViolationType,

    /// A location was specified for a ProtocolViolation, but the location
    /// was not valid.
    InvalidLocation,

    /// The supplied transciever error was invalid.
    InvalidTransceiverError,
}


#[derive(Copy, Clone, Debug)]
pub enum CANError {
    /// TX timeout (by netdevice driver)
    TransmitTimeout,

    /// Arbitration was lost. Contains the number after which arbitration was
    /// lost or 0 if unspecified
    LostArbitration(u8),
    ControllerProblem(ControllerProblem),
    ProtocolViolation {
        vtype: ViolationType,
        location: Location,
    },
    TransceiverError,
    NoAck,
    BusOff,
    BusError,
    Restarted,
    Unknown(u32),
}

#[derive(Copy, Clone, Debug)]
pub enum ControllerProblem {
    // unspecified
    Unspecified,
    // RX buffer overflow
    ReceiveBufferOverflow,
    // TX buffer overflow
    TransmitBufferOverflow,
    // reached warning level for RX errors
    ReceiveErrorWarning,
    // reached warning level for TX errors
    TransmitErrorWarning,
    // reached error passive status RX
    ReceiveErrorPassive,
    // reached error passive status TX
    TransmitErrorPassive,
    // recovered to error active state
    Active,
}

impl TryFrom<u8> for ControllerProblem {
    type Error = CANErrorDecodingFailure;

    fn try_from(val: u8) -> Result<ControllerProblem, CANErrorDecodingFailure> {
        Ok(match val {
            0x00 => ControllerProblem::Unspecified,
            0x01 => ControllerProblem::ReceiveBufferOverflow,
            0x02 => ControllerProblem::TransmitBufferOverflow,
            0x04 => ControllerProblem::ReceiveErrorWarning,
            0x08 => ControllerProblem::TransmitErrorWarning,
            0x10 => ControllerProblem::ReceiveErrorPassive,
            0x20 => ControllerProblem::TransmitErrorPassive,
            0x40 => ControllerProblem::Active,
            _ => return Err(CANErrorDecodingFailure::InvalidControllerProblem),
        })
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ViolationType {
    Unspecified, // unspecified
    SingleBitError, // single bit error
    FrameFormatError, // frame format error
    BitStuffingError, // bit stuffing error
    UnableToSendDominantBit, // unable to send dominant bit
    UnableToSendRecessiveBit, // unable to send recessive bit
    BusOverload, // bus overload
    Active, // active error announcement
    TransmissionError, // error occurred on transmission
}

impl TryFrom<u8> for ViolationType {
    type Error = CANErrorDecodingFailure;

    fn try_from(val: u8) -> Result<ViolationType, CANErrorDecodingFailure> {
        Ok(match val {
            0x00 => ViolationType::Unspecified,
            0x01 => ViolationType::SingleBitError,
            0x02 => ViolationType::FrameFormatError,
            0x04 => ViolationType::BitStuffingError,
            0x08 => ViolationType::UnableToSendDominantBit,
            0x10 => ViolationType::UnableToSendRecessiveBit,
            0x20 => ViolationType::BusOverload,
            0x40 => ViolationType::Active,
            0x80 => ViolationType::TransmissionError,
            _ => return Err(CANErrorDecodingFailure::InvlaidViolationType),
        })
    }
}

/// Location
///
/// Describes where inside a received frame an error occured.
#[derive(Copy, Clone, Debug)]
pub enum Location {
    /// Unspecified
    Unspecified,
    /// Start of frame.
    StartOfFrame,
    /// ID bits 28-21 (SFF: 10-3)
    Id2821,
    /// ID bits 20-18 (SFF: 2-0)
    Id2018,
    /// substitute RTR (SFF: RTR)
    SubstituteRtr,
    /// extension of identifier
    IdentifierExtension,
    /// ID bits 17-13
    Id1713,
    /// ID bits 12-5
    Id1205,
    /// ID bits 4-0
    Id0400,
    /// RTR bit
    Rtr,
    /// Reserved bit 1
    Reserved1,
    /// Reserved bit 0
    Reserved0,
    /// Data length
    DataLengthCode,
    /// Data section
    DataSection,
    /// CRC sequence
    CrcSequence,
    /// CRC delimiter
    CrcDelimiter,
    /// ACK slot
    AckSlot,
    /// ACK delimiter
    AckDelimiter,
    /// End-of-frame
    EndOfFrame,
    /// Intermission (between frames)
    Intermission,
}

impl TryFrom<u8> for Location {
    type Error = CANErrorDecodingFailure;

    fn try_from(val: u8) -> Result<Location, CANErrorDecodingFailure> {
        Ok(match val {
            0x00 => Location::Unspecified,
            0x03 => Location::StartOfFrame,
            0x02 => Location::Id2821,
            0x06 => Location::Id2018,
            0x04 => Location::SubstituteRtr,
            0x05 => Location::IdentifierExtension,
            0x07 => Location::Id1713,
            0x0F => Location::Id1205,
            0x0E => Location::Id0400,
            0x0C => Location::Rtr,
            0x0D => Location::Reserved1,
            0x09 => Location::Reserved0,
            0x0B => Location::DataLengthCode,
            0x0A => Location::DataSection,
            0x08 => Location::CrcSequence,
            0x18 => Location::CrcDelimiter,
            0x19 => Location::AckSlot,
            0x1B => Location::AckDelimiter,
            0x1A => Location::EndOfFrame,
            0x12 => Location::Intermission,
            _ => return Err(CANErrorDecodingFailure::InvalidLocation),
        })
    }
}

pub enum TransceiverError {
    Unspecified,
    CanHighNoWire,
    CanHighShortToBat,
    CanHighShortToVcc,
    CanHighShortToGnd,
    CanLowNoWire,
    CanLowShortToBat,
    CanLowShortToVcc,
    CanLowShortToGnd,
    CanLowShortToCanHigh,
}

impl TryFrom<u8> for TransceiverError {
    type Error = CANErrorDecodingFailure;

    fn try_from(val: u8) -> Result<TransceiverError, CANErrorDecodingFailure> {
        Ok(match val {
            0x00 => TransceiverError::Unspecified,
            0x04 => TransceiverError::CanHighNoWire,
            0x05 => TransceiverError::CanHighShortToBat,
            0x06 => TransceiverError::CanHighShortToVcc,
            0x07 => TransceiverError::CanHighShortToGnd,
            0x40 => TransceiverError::CanLowNoWire,
            0x50 => TransceiverError::CanLowShortToBat,
            0x60 => TransceiverError::CanLowShortToVcc,
            0x70 => TransceiverError::CanLowShortToGnd,
            0x80 => TransceiverError::CanLowShortToCanHigh,
            _ => return Err(CANErrorDecodingFailure::InvalidTransceiverError),
        })
    }
}

impl CANError {
    pub fn from_frame(frame: &CANFrame) -> Result<CANError, CANErrorDecodingFailure> {
        if !frame.is_error() {
            return Err(CANErrorDecodingFailure::NotAnError);
        }

        match frame.err() {
            0x00000001 => Ok(CANError::TransmitTimeout),
            0x00000002 => Ok(CANError::LostArbitration(r#try!(get_data(frame, 0)))),
            0x00000004 => {
                Ok(CANError::ControllerProblem(r#try!(ControllerProblem::try_from
                    (r#try! (get_data(frame, 1))))))
            }

            0x00000008 => {
                Ok(CANError::ProtocolViolation {
                    vtype: r#try!(ViolationType::try_from(r#try!(get_data(frame, 2)))),
                    location: r#try!(Location::try_from(r#try!(get_data(frame, 3)))),
                })
            }

            0x00000010 => Ok(CANError::TransceiverError),
            0x00000020 => Ok(CANError::NoAck),
            0x00000040 => Ok(CANError::BusOff),
            0x00000080 => Ok(CANError::BusError),
            0x00000100 => Ok(CANError::Restarted),
            e => Err(CANErrorDecodingFailure::UnknownErrorType(e)),
        }
    }
}

pub trait ControllerSpecificErrorInformation {
    fn get_ctrl_err(&self) -> Option<&[u8]>;
}

impl ControllerSpecificErrorInformation for CANFrame {
    #[inline]
    fn get_ctrl_err(&self) -> Option<&[u8]> {
        let data = self.data();

        if data.len() != 8 {
            None
        } else {
            Some(&data[5..])
        }
    }
}
