// socketcan/src/frame.rs
//
// Implements frames for CANbus 2.0 and FD for SocketCAN on Linux.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

use crate::err::{CanError, CanErrorDecodingFailure, ConstructionError};
use crate::util::hal_id_to_raw;
use bitflags::bitflags;
use embedded_can::{ExtendedId, Frame as EmbeddedFrame, Id, StandardId};
use libc::{can_frame, canfd_frame, canid_t};

use std::{convert::TryFrom, fmt, mem};

use itertools::Itertools;

pub use libc::{
    CANFD_BRS, CANFD_ESI, CANFD_MAX_DLEN, CAN_EFF_FLAG, CAN_EFF_MASK, CAN_ERR_FLAG, CAN_ERR_MASK,
    CAN_MAX_DLEN, CAN_RTR_FLAG, CAN_SFF_MASK,
};

/// an error mask that will cause SocketCAN to report all errors
pub const ERR_MASK_ALL: u32 = CAN_ERR_MASK;

/// an error mask that will cause SocketCAN to silently drop all errors
pub const ERR_MASK_NONE: u32 = 0;

bitflags! {
    /// Bit flags in the composite SocketCAN ID word.
    pub struct IdFlags: canid_t {
        /// Indicates frame uses a 29-bit extended ID
        const EFF = CAN_EFF_FLAG;
        /// Indicates a remote request frame.
        const RTR = CAN_RTR_FLAG;
        /// Indicates an error frame.
        const ERR = CAN_ERR_FLAG;
    }

    /// Bit flags for the Flexible Data (FD) frames.
    pub struct FdFlags: u8 {
        /// Bit rate switch (second bit rate for payload data)
        const BRS = CANFD_BRS as u8;
        /// Error state indicator of the transmitting node
        const ESI = CANFD_ESI as u8;
    }
}

/// Creates a composite 32-bit CAN ID word for SocketCAN.
///
/// The ID 'word' is composed of the CAN ID along with the EFF/RTR/ERR bit flags.
fn init_id_word(id: canid_t, mut flags: IdFlags) -> Result<canid_t, ConstructionError> {
    if id > CAN_EFF_MASK {
        return Err(ConstructionError::IDTooLarge);
    }

    if id > CAN_SFF_MASK {
        flags |= IdFlags::EFF;
    }

    Ok(id | flags.bits())
}

fn is_extended(id: &Id) -> bool {
    match id {
        Id::Standard(_) => false,
        Id::Extended(_) => true,
    }
}

fn slice_to_array<const S: usize>(data: &[u8]) -> [u8; S] {
    let mut arr = [0; S];
    for (i, b) in data.iter().enumerate() {
        arr[i] = *b;
    }
    arr
}

// ===== Frame trait =====

pub trait Frame: EmbeddedFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> canid_t;

    /// Return the actual raw CAN ID (without EFF/RTR/ERR flags)
    fn raw_id(&self) -> canid_t {
        // TODO: This probably isn't necessary. Just use EFF_MASK?
        let mask = if self.is_extended() {
            CAN_EFF_MASK
        } else {
            CAN_SFF_MASK
        };
        self.id_word() & mask
    }

    /// Returns the EFF/RTR/ERR flags from the ID word
    fn id_flags(&self) -> IdFlags {
        IdFlags::from_bits_truncate(self.id_word())
    }

    /// Return the CAN ID as the embedded HAL Id type.
    fn hal_id(&self) -> Id {
        if self.is_extended() {
            Id::Extended(ExtendedId::new(self.id_word() & CAN_EFF_MASK).unwrap())
        } else {
            Id::Standard(StandardId::new((self.id_word() & CAN_SFF_MASK) as u16).unwrap())
        }
    }

    /// Get the data length
    fn len(&self) -> usize {
        self.dlc()
    }

    /// Return the error message
    fn err(&self) -> u32 {
        self.id_word() & CAN_ERR_MASK
    }

    /// Check if frame is an error message
    fn is_error(&self) -> bool {
        self.id_flags().contains(IdFlags::ERR)
    }

    fn error(&self) -> Result<CanError, CanErrorDecodingFailure>
    where
        Self: Sized,
    {
        CanError::from_frame(self)
    }
}

// ===== CanAnyFrame =====

/// Any frame type.
pub enum CanAnyFrame {
    /// A classic CAN 2.0 frame, with up to 8-bytes of data
    Normal(CanFrame),
    /// A flexible data rate frame, with up to 64-bytes of data
    Fd(CanFdFrame),
}

impl fmt::Debug for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Normal(frame) => {
                write!(f, "CAN Frame {:?}", frame)
            }

            Self::Fd(frame) => {
                write!(f, "CAN FD Frame {:?}", frame)
            }
        }
    }
}

impl fmt::UpperHex for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal(frame) => frame.fmt(f),
            Self::Fd(frame) => frame.fmt(f),
        }
    }
}

impl From<CanFrame> for CanAnyFrame {
    fn from(frame: CanFrame) -> Self {
        Self::Normal(frame)
    }
}

impl From<CanFdFrame> for CanAnyFrame {
    fn from(frame: CanFdFrame) -> Self {
        Self::Fd(frame)
    }
}

// ===== CanFrame =====

/// The classic CAN 2.0 frame with up to 8-bytes of data.
///
/// This is highly compatible with the `can_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.can_frame.html))
#[derive(Clone, Copy)]
pub struct CanFrame(can_frame);

impl CanFrame {
    /// Initializes a CAN frame from raw parts.
    pub fn init(id: u32, data: &[u8], flags: IdFlags) -> Result<Self, ConstructionError> {
        let n = data.len();

        if n > CAN_MAX_DLEN {
            return Err(ConstructionError::TooMuchData);
        }

        let mut frame: can_frame = unsafe { mem::zeroed() };
        frame.can_id = init_id_word(id, flags)?;
        frame.can_dlc = n as u8;
        (&mut frame.data[..n]).copy_from_slice(data);

        Ok(Self(frame))
    }

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    pub fn as_ptr(&self) -> *const can_frame {
        &self.0 as *const can_frame
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    pub fn as_mut_ptr(&mut self) -> *mut can_frame {
        &mut self.0 as *mut can_frame
    }
}

impl EmbeddedFrame for CanFrame {
    /// Create a new CAN 2.0 data frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let id = id.into();
        let mut flags = IdFlags::empty();
        flags.set(IdFlags::EFF, is_extended(&id));

        let raw_id = hal_id_to_raw(id);
        Self::init(raw_id, data, flags).ok()
    }

    /// Create a new remote transmission request frame.
    fn new_remote(id: impl Into<Id>, dlc: usize) -> Option<Self> {
        let id = id.into();
        let mut flags = IdFlags::RTR;
        flags.set(IdFlags::EFF, is_extended(&id));

        let raw_id = hal_id_to_raw(id);
        let data = [0u8; 8];
        Self::init(raw_id, &data[0..dlc], flags).ok()
    }

    /// Check if frame uses 29-bit extended ID format.
    fn is_extended(&self) -> bool {
        self.id_flags().contains(IdFlags::EFF)
    }

    /// Check if frame is a remote transmission request.
    fn is_remote_frame(&self) -> bool {
        self.id_flags().contains(IdFlags::RTR)
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    /// TODO: Return the proper DLC code for remote frames?
    fn dlc(&self) -> usize {
        self.0.can_dlc as usize
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    fn data(&self) -> &[u8] {
        &self.0.data[..(self.0.can_dlc as usize)]
    }
}

impl Frame for CanFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> u32 {
        self.0.can_id
    }
}

impl Default for CanFrame {
    /// The default FD frame has all fields and data set to zero, and all flags off.
    fn default() -> Self {
        let frame: can_frame = unsafe { mem::zeroed() };
        Self(frame)
    }
}

impl fmt::Debug for CanFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let _ = write!(f, "CanFrame {{ ")?;
        let _ = fmt::UpperHex::fmt(self, f)?;
        write!(f, " }}")
    }
}

impl fmt::UpperHex for CanFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self.0.can_id, "#")?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}

impl TryFrom<CanFdFrame> for CanFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanFdFrame) -> Result<Self, Self::Error> {
        if frame.0.len > CAN_MAX_DLEN as u8 {
            return Err(ConstructionError::TooMuchData);
        }

        CanFrame::init(
            frame.raw_id(),
            &frame.data()[..(frame.0.len as usize)],
            frame.id_flags(),
        )
    }
}

impl AsRef<libc::can_frame> for CanFrame {
    fn as_ref(&self) -> &can_frame {
        &self.0
    }
}

// ===== CanFdFrame =====

/// The CAN flexible data rate frame with up to 64-bytes of data.
///
/// This is highly compatible with the `canfd_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.canfd_frame.html))
#[derive(Clone, Copy)]
pub struct CanFdFrame(canfd_frame);

impl CanFdFrame {
    pub fn init(
        id: u32,
        data: &[u8],
        mut flags: IdFlags,
        fd_flags: FdFlags,
    ) -> Result<Self, ConstructionError> {
        let n = data.len();

        if n > CAN_MAX_DLEN {
            return Err(ConstructionError::TooMuchData);
        }

        flags.remove(IdFlags::RTR);

        let mut frame = Self::default();
        frame.0.can_id = init_id_word(id, flags)?;
        frame.0.len = n as u8;
        frame.0.flags = fd_flags.bits();
        (&mut frame.0.data[..n]).copy_from_slice(data);

        Ok(frame)
    }

    /// Gets the flags for the FD frame.
    ///
    /// These are the bits from the separate FD frame flags, not the flags
    /// in the composite ID word.
    pub fn flags(&self) -> FdFlags {
        FdFlags::from_bits_truncate(self.0.flags)
    }

    /// Whether the frame uses a bit rate switch (second bit rate for
    /// payload data).
    pub fn is_brs(&self) -> bool {
        self.flags().contains(FdFlags::BRS)
    }

    /// Sets whether the frame uses a bit rate switch.
    pub fn set_brs(&mut self, on: bool) {
        if on {
            self.0.flags |= CANFD_BRS as u8;
        } else {
            self.0.flags &= !(CANFD_BRS as u8);
        }
    }

    /// Gets the error state indicator of the transmitting node
    pub fn is_esi(&self) -> bool {
        self.flags().contains(FdFlags::ESI)
    }

    /// Sets the error state indicator of the transmitting node
    pub fn set_esi(&mut self, on: bool) {
        if on {
            self.0.flags |= CANFD_ESI as u8;
        } else {
            self.0.flags &= !CANFD_ESI as u8;
        }
    }

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    pub fn as_ptr(&self) -> *const canfd_frame {
        &self.0 as *const canfd_frame
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    pub fn as_mut_ptr(&mut self) -> *mut canfd_frame {
        &mut self.0 as *mut canfd_frame
    }
}

impl EmbeddedFrame for CanFdFrame {
    /// Create a new frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let id = id.into();
        let mut flags = IdFlags::empty();
        flags.set(IdFlags::EFF, is_extended(&id));

        let raw_id = hal_id_to_raw(id);
        Self::init(raw_id, data, flags, FdFlags::empty()).ok()
    }

    /// CAN FD frames don't support remote
    fn new_remote(_id: impl Into<Id>, _dlc: usize) -> Option<Self> {
        None
    }

    /// Check if frame uses 29-bit extended ID format.
    fn is_extended(&self) -> bool {
        self.id_flags().contains(IdFlags::EFF)
    }

    /// The FD frames don't support remote request
    fn is_remote_frame(&self) -> bool {
        false
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    fn dlc(&self) -> usize {
        self.0.len as usize
    }

    /// A slice into the actual data.
    ///
    /// For normal CAN frames the slice will always be <= 8 bytes in length.
    fn data(&self) -> &[u8] {
        &self.0.data[..(self.0.len as usize)]
    }
}

impl Frame for CanFdFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> u32 {
        self.0.can_id
    }
}

impl Default for CanFdFrame {
    /// The default FD frame has all fields and data set to zero, and all flags off.
    fn default() -> Self {
        let frame: canfd_frame = unsafe { mem::zeroed() };
        Self(frame)
    }
}

impl fmt::Debug for CanFdFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let _ = write!(f, "CanFdFrame {{ ")?;
        let _ = fmt::UpperHex::fmt(self, f)?;
        write!(f, " }}")
    }
}

impl fmt::UpperHex for CanFdFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self.0.can_id, "##")?;
        write!(f, "{} ", self.0.flags)?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}

impl From<CanFrame> for CanFdFrame {
    fn from(frame: CanFrame) -> Self {
        let mut fdframe = Self::default();
        // TODO: force rtr off?
        fdframe.0.can_id = frame.0.can_id;
        fdframe.0.len = frame.0.can_dlc as u8;
        fdframe.0.data = slice_to_array::<CANFD_MAX_DLEN>(frame.data());
        fdframe
    }
}

impl AsRef<libc::canfd_frame> for CanFdFrame {
    fn as_ref(&self) -> &canfd_frame {
        &self.0
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    const STD_ID: Id = Id::Standard(StandardId::MAX);
    const EXT_ID: Id = Id::Extended(ExtendedId::MAX);

    const DATA: &[u8] = &[0, 1, 2, 3];
    const DATA_LEN: usize = DATA.len();

    #[test]
    fn test_data_frame() {
        let frame = CanFrame::new(STD_ID, DATA).unwrap();
        assert_eq!(STD_ID, frame.id());
        //assert_eq!(STD_ID.as_raw(), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());

        let frame = CanFrame::new(EXT_ID, DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        //assert_eq!(EXT_ID.as_raw(), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
    }

    #[test]
    fn test_remote_frame() {
        let frame = CanFrame::new_remote(STD_ID, DATA_LEN).unwrap();
        assert_eq!(STD_ID, frame.id());
        //assert_eq!(STD_ID.as_raw(), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(!frame.is_data_frame());
        assert!(frame.is_remote_frame());
    }

    #[test]
    fn test_fd_frame() {
        let frame = CanFdFrame::new(STD_ID, DATA).unwrap();
        assert_eq!(STD_ID, frame.id());
        //assert_eq!(STD_ID.as_raw(), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());

        let frame = CanFdFrame::new(EXT_ID, DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        //assert_eq!(EXT_ID.as_raw(), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
    }
}
