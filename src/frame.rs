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

//! CAN bus frames.

use crate::{CanError, ConstructionError};
use bitflags::bitflags;
use embedded_can::{ExtendedId, Frame as EmbeddedFrame, Id, StandardId};
use itertools::Itertools;
use libc::{can_frame, canfd_frame, canid_t};
use std::{convert::TryFrom, fmt, matches, mem};

pub use libc::{
    CANFD_BRS, CANFD_ESI, CANFD_MAX_DLEN, CAN_EFF_FLAG, CAN_EFF_MASK, CAN_ERR_FLAG, CAN_ERR_MASK,
    CAN_MAX_DLEN, CAN_RTR_FLAG, CAN_SFF_MASK,
};

/// An error mask that will cause SocketCAN to report all errors
pub const ERR_MASK_ALL: u32 = CAN_ERR_MASK;

/// An error mask that will cause SocketCAN to silently drop all errors
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

/// Gets the raw ID value from an Id
pub fn id_to_raw(id: Id) -> u32 {
    match id {
        Id::Standard(id) => id.as_raw() as u32,
        Id::Extended(id) => id.as_raw(),
    }
}

/// Determines if the ID is a 29-bit extended ID.
pub fn id_is_extended(id: &Id) -> bool {
    matches!(id, Id::Extended(_))
}

// ===== can_frame =====

/// Creates a default C `can_frame`.
#[inline(always)]
pub fn can_frame_default() -> can_frame {
    unsafe { mem::zeroed() }
}

/// Initializes a libc can_frame frame from raw parts.
pub fn can_frame_new(id: u32, data: &[u8], flags: IdFlags) -> Result<can_frame, ConstructionError> {
    let n = data.len();

    if n > CAN_MAX_DLEN {
        return Err(ConstructionError::TooMuchData);
    }

    let mut frame = can_frame_default();
    frame.can_id = init_id_word(id, flags)?;
    frame.can_dlc = n as u8;
    frame.data[..n].copy_from_slice(data);

    Ok(frame)
}

/// Creates a default C `can_frame`.
#[inline(always)]
pub fn canfd_frame_default() -> canfd_frame {
    unsafe { mem::zeroed() }
}

// ===== AsPtr trait =====

/// Trait to get a pointer to an inner type
pub trait AsPtr {
    /// The inner type to which we resolve as a pointer
    type Inner;

    /// Gets a const pointer to the inner type
    fn as_ptr(&self) -> *const Self::Inner;

    /// Gets a mutable pointer to the inner type
    fn as_mut_ptr(&mut self) -> *mut Self::Inner;

    /// The size of the inner type
    fn size() -> usize {
        std::mem::size_of::<Self::Inner>()
    }
}

// ===== Frame trait =====

/// Shared trait for CAN frames
#[allow(clippy::len_without_is_empty)]
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

    /// Check if frame is an error message
    fn is_error_frame(&self) -> bool {
        self.id_flags().contains(IdFlags::ERR)
    }
}

// ===== CanAnyFrame =====

/// Any frame type.
#[derive(Clone, Copy, Debug)]
pub enum CanAnyFrame {
    /// A classic CAN 2.0 frame, with up to 8-bytes of data
    Normal(CanDataFrame),
    /// A CAN Remote Frame
    Remote(CanRemoteFrame),
    /// An error frame
    Error(CanErrorFrame),
    /// A flexible data rate frame, with up to 64-bytes of data
    Fd(CanFdFrame),
}

impl fmt::UpperHex for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal(frame) => frame.fmt(f),
            Self::Remote(frame) => frame.fmt(f),
            Self::Error(frame) => frame.fmt(f),
            Self::Fd(frame) => frame.fmt(f),
        }
    }
}

impl From<CanFrame> for CanAnyFrame {
    fn from(frame: CanFrame) -> Self {
        use CanFrame::*;
        match frame {
            Data(frame) => Self::Normal(frame),
            Remote(frame) => Self::Remote(frame),
            Error(frame) => Self::Error(frame),
        }
    }
}

impl From<CanFdFrame> for CanAnyFrame {
    fn from(frame: CanFdFrame) -> Self {
        Self::Fd(frame)
    }
}

// ===== CanFrame =====

/// The classic CAN 2.0 frame with up to 8-bytes of data.
#[derive(Clone, Copy, Debug)]
pub enum CanFrame {
    /// A data frame
    Data(CanDataFrame),
    /// A remote frame
    Remote(CanRemoteFrame),
    /// An error frame
    Error(CanErrorFrame),
}

impl AsPtr for CanFrame {
    type Inner = can_frame;

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    fn as_ptr(&self) -> *const Self::Inner {
        use CanFrame::*;
        match self {
            Data(frame) => frame.as_ptr(),
            Remote(frame) => frame.as_ptr(),
            Error(frame) => frame.as_ptr(),
        }
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    fn as_mut_ptr(&mut self) -> *mut Self::Inner {
        use CanFrame::*;
        match self {
            Data(frame) => frame.as_mut_ptr(),
            Remote(frame) => frame.as_mut_ptr(),
            Error(frame) => frame.as_mut_ptr(),
        }
    }
}

impl EmbeddedFrame for CanFrame {
    /// Create a new CAN 2.0 data frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        CanDataFrame::new(id, data).map(CanFrame::Data)
    }

    /// Create a new remote transmission request frame.
    fn new_remote(id: impl Into<Id>, dlc: usize) -> Option<Self> {
        CanRemoteFrame::new_remote(id, dlc).map(CanFrame::Remote)
    }

    /// Check if frame uses 29-bit extended ID format.
    fn is_extended(&self) -> bool {
        use CanFrame::*;
        match self {
            Data(frame) => frame.is_extended(),
            Remote(frame) => frame.is_extended(),
            Error(frame) => frame.is_extended(),
        }
    }

    /// Check if frame is a remote transmission request.
    fn is_remote_frame(&self) -> bool {
        matches!(self, CanFrame::Remote(_))
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        use CanFrame::*;
        match self {
            Data(frame) => frame.id(),
            Remote(frame) => frame.id(),
            Error(frame) => frame.id(),
        }
    }

    /// Data length
    fn dlc(&self) -> usize {
        use CanFrame::*;
        match self {
            Data(frame) => frame.dlc(),
            Remote(frame) => frame.dlc(),
            Error(frame) => frame.dlc(),
        }
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    fn data(&self) -> &[u8] {
        use CanFrame::*;
        match self {
            Data(frame) => frame.data(),
            Remote(frame) => frame.data(),
            Error(frame) => frame.data(),
        }
    }
}

impl Frame for CanFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> u32 {
        use CanFrame::*;
        match self {
            Data(frame) => frame.id_word(),
            Remote(frame) => frame.id_word(),
            Error(frame) => frame.id_word(),
        }
    }
}

impl Default for CanFrame {
    /// The default frame is a default data frame - all fields and data set
    /// to zero, and all flags off.
    fn default() -> Self {
        CanFrame::Data(CanDataFrame::default())
    }
}

impl fmt::UpperHex for CanFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        use CanFrame::*;
        match self {
            Data(frame) => fmt::UpperHex::fmt(&frame, f),
            Remote(frame) => fmt::UpperHex::fmt(&frame, f),
            Error(frame) => fmt::UpperHex::fmt(&frame, f),
        }
    }
}

impl From<can_frame> for CanFrame {
    /// Create a `CanFrame` from a C `can_frame` struct.
    fn from(frame: can_frame) -> Self {
        if frame.can_id & CAN_ERR_FLAG != 0 {
            CanFrame::Error(CanErrorFrame(frame))
        } else if frame.can_id & CAN_RTR_FLAG != 0 {
            CanFrame::Remote(CanRemoteFrame(frame))
        } else {
            CanFrame::Data(CanDataFrame(frame))
        }
    }
}

impl From<CanDataFrame> for CanFrame {
    /// Create a `CanFrame` from a data frame
    fn from(frame: CanDataFrame) -> Self {
        Self::Data(frame)
    }
}

impl From<CanRemoteFrame> for CanFrame {
    /// Create a `CanFrame` from a remote frame
    fn from(frame: CanRemoteFrame) -> Self {
        Self::Remote(frame)
    }
}

impl From<CanErrorFrame> for CanFrame {
    /// Create a `CanFrame` from an error frame
    fn from(frame: CanErrorFrame) -> Self {
        Self::Error(frame)
    }
}

impl AsRef<can_frame> for CanFrame {
    fn as_ref(&self) -> &can_frame {
        use CanFrame::*;
        match self {
            Data(frame) => frame.as_ref(),
            Remote(frame) => frame.as_ref(),
            Error(frame) => frame.as_ref(),
        }
    }
}

impl TryFrom<CanFdFrame> for CanFrame {
    type Error = ConstructionError;

    /// Try to convert a CAN FD frame into a classic CAN 2.0 frame.
    ///
    /// This should work if it's a data frame with 8 or fewer data bytes.
    fn try_from(frame: CanFdFrame) -> Result<Self, <Self as TryFrom<CanFdFrame>>::Error> {
        CanDataFrame::try_from(frame).map(CanFrame::Data)
    }
}

// ===== CanDataFrame =====

/// The classic CAN 2.0 frame with up to 8-bytes of data.
///
/// This is highly compatible with the `can_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.can_frame.html))
#[derive(Clone, Copy)]
pub struct CanDataFrame(can_frame);

impl CanDataFrame {
    /// Initializes a CAN frame from raw parts.
    pub fn init(id: u32, data: &[u8], flags: IdFlags) -> Result<Self, ConstructionError> {
        let frame = can_frame_new(id, data, flags)?;
        Ok(Self(frame))
    }
}

impl AsPtr for CanDataFrame {
    type Inner = can_frame;

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    fn as_ptr(&self) -> *const Self::Inner {
        &self.0
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    fn as_mut_ptr(&mut self) -> *mut Self::Inner {
        &mut self.0
    }
}

impl EmbeddedFrame for CanDataFrame {
    /// Create a new CAN 2.0 data frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let id = id.into();
        let mut flags = IdFlags::empty();
        flags.set(IdFlags::EFF, id_is_extended(&id));

        let raw_id = id_to_raw(id);
        Self::init(raw_id, data, flags).ok()
    }

    /// Create a new remote transmission request frame.
    fn new_remote(id: impl Into<Id>, dlc: usize) -> Option<Self> {
        let id = id.into();
        let mut flags = IdFlags::RTR;
        flags.set(IdFlags::EFF, id_is_extended(&id));

        let raw_id = id_to_raw(id);
        let data = [0u8; 8];
        Self::init(raw_id, &data[0..dlc], flags).ok()
    }

    /// Check if frame uses 29-bit extended ID format.
    fn is_extended(&self) -> bool {
        self.id_flags().contains(IdFlags::EFF)
    }

    /// Check if frame is a remote transmission request.
    fn is_remote_frame(&self) -> bool {
        false
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    fn dlc(&self) -> usize {
        self.0.can_dlc as usize
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    fn data(&self) -> &[u8] {
        &self.0.data[..(self.0.can_dlc as usize)]
    }
}

impl Frame for CanDataFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> u32 {
        self.0.can_id
    }
}

impl Default for CanDataFrame {
    /// The default FD frame has all fields and data set to zero, and all flags off.
    fn default() -> Self {
        Self(can_frame_default())
    }
}

impl fmt::Debug for CanDataFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CanDataFrame {{ ")?;
        fmt::UpperHex::fmt(self, f)?;
        write!(f, " }}")
    }
}

impl fmt::UpperHex for CanDataFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}#", self.0.can_id)?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = " ";
        write!(f, "{}", parts.join(sep))
    }
}

impl TryFrom<can_frame> for CanDataFrame {
    type Error = ConstructionError;

    /// Try to create a `CanDataFrame` from a C `can_frame`
    ///
    /// This will succeed as long as the C frame is not marked as an error
    /// or remote frame.
    fn try_from(frame: can_frame) -> Result<Self, Self::Error> {
        if frame.can_id & (CAN_ERR_FLAG | CAN_RTR_FLAG) == 0 {
            Ok(Self(frame))
        } else {
            Err(ConstructionError::WrongFrameType)
        }
    }
}

impl TryFrom<CanFdFrame> for CanDataFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanFdFrame) -> Result<Self, Self::Error> {
        if frame.0.len > CAN_MAX_DLEN as u8 {
            return Err(ConstructionError::TooMuchData);
        }

        CanDataFrame::init(
            frame.raw_id(),
            &frame.data()[..(frame.0.len as usize)],
            frame.id_flags(),
        )
    }
}

impl AsRef<can_frame> for CanDataFrame {
    fn as_ref(&self) -> &can_frame {
        &self.0
    }
}

// ===== CanRemoteFrame =====

/// The classic CAN 2.0 frame with up to 8-bytes of data.
///
/// This is highly compatible with the `can_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.can_frame.html))
#[derive(Clone, Copy)]
pub struct CanRemoteFrame(can_frame);

impl CanRemoteFrame {
    /// Initializes a CAN frame from raw parts.
    pub fn init(id: u32, data: &[u8], flags: IdFlags) -> Result<Self, ConstructionError> {
        let n = data.len();

        if n > CAN_MAX_DLEN {
            return Err(ConstructionError::TooMuchData);
        }

        let mut frame: can_frame = unsafe { mem::zeroed() };
        frame.can_id = init_id_word(id, flags)?;
        frame.can_dlc = n as u8;
        frame.data[..n].copy_from_slice(data);

        Ok(Self(frame))
    }
}

impl AsPtr for CanRemoteFrame {
    type Inner = can_frame;

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    fn as_ptr(&self) -> *const Self::Inner {
        &self.0
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    fn as_mut_ptr(&mut self) -> *mut Self::Inner {
        &mut self.0
    }
}

impl EmbeddedFrame for CanRemoteFrame {
    /// Create a new CAN 2.0 remote frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        Self::new_remote(id, data.len())
    }

    /// Create a new remote transmission request frame.
    fn new_remote(id: impl Into<Id>, dlc: usize) -> Option<Self> {
        let id = id.into();
        let mut flags = IdFlags::RTR;
        flags.set(IdFlags::EFF, id_is_extended(&id));

        // TODO: Check for a valid DLC

        let mut frame = can_frame_default();
        frame.can_id = id_to_raw(id);
        frame.can_dlc = dlc as u8;
        Some(Self(frame))
    }

    /// Check if frame uses 29-bit extended ID format.
    fn is_extended(&self) -> bool {
        self.id_flags().contains(IdFlags::EFF)
    }

    /// Check if frame is a remote transmission request.
    fn is_remote_frame(&self) -> bool {
        true
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    /// TODO: Return the proper DLC code for remote frames
    fn dlc(&self) -> usize {
        self.0.can_dlc as usize
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    fn data(&self) -> &[u8] {
        // TODO: Is this OK, or just an empty slice?
        &self.0.data[..self.dlc()]
    }
}

impl Frame for CanRemoteFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> u32 {
        self.0.can_id
    }
}

impl Default for CanRemoteFrame {
    /// The default FD frame has all fields and data set to zero, and all flags off.
    fn default() -> Self {
        let frame: can_frame = unsafe { mem::zeroed() };
        Self(frame)
    }
}

impl fmt::Debug for CanRemoteFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CanRemoteFrame {{ ")?;
        fmt::UpperHex::fmt(self, f)?;
        write!(f, " }}")
    }
}

impl fmt::UpperHex for CanRemoteFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}#", self.0.can_id)?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        write!(f, "{}", parts.join(" "))
    }
}

impl TryFrom<can_frame> for CanRemoteFrame {
    type Error = ConstructionError;

    /// Try to create a `CanRemoteFrame` from a C `can_frame`
    ///
    /// This will only succeed the C frame is marked as a remote frame.
    fn try_from(frame: can_frame) -> Result<Self, Self::Error> {
        if frame.can_id & CAN_RTR_FLAG != 0 {
            Ok(Self(frame))
        } else {
            Err(ConstructionError::WrongFrameType)
        }
    }
}

impl AsRef<can_frame> for CanRemoteFrame {
    fn as_ref(&self) -> &can_frame {
        &self.0
    }
}

// ===== CanErrorFrame =====

/// A SocketCAN error frame.
///
/// This is returned from a read/receive by the OS or interface device
/// driver when it detects an error, such as a problem on the bus. The
/// frame encodes detailed information about the error, which can be
/// managed directly by the application or converted into a Rust error
///
/// This is highly compatible with the `can_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.can_frame.html))
#[derive(Clone, Copy)]
pub struct CanErrorFrame(can_frame);

impl CanErrorFrame {
    /// Return the error bits from the ID word of the error frame.
    pub fn error_bits(&self) -> u32 {
        self.id_word() & CAN_ERR_MASK
    }

    /// Converts this error frame into a `CanError`
    pub fn into_error(self) -> CanError {
        CanError::from(self)
    }
}

impl AsPtr for CanErrorFrame {
    type Inner = can_frame;

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    fn as_ptr(&self) -> *const Self::Inner {
        &self.0
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    fn as_mut_ptr(&mut self) -> *mut Self::Inner {
        &mut self.0
    }
}

impl EmbeddedFrame for CanErrorFrame {
    /// The application should not create an error frame.
    /// This will always return None.
    fn new(_id: impl Into<Id>, _data: &[u8]) -> Option<Self> {
        None
    }

    /// The application should not create an error frame.
    /// This will always return None.
    fn new_remote(_id: impl Into<Id>, _dlc: usize) -> Option<Self> {
        None
    }

    /// Check if frame uses 29-bit extended ID format.
    fn is_extended(&self) -> bool {
        self.id_flags().contains(IdFlags::EFF)
    }

    /// Check if frame is a remote transmission request.
    fn is_remote_frame(&self) -> bool {
        false
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    fn dlc(&self) -> usize {
        self.0.can_dlc as usize
    }

    /// A slice into the actual data.
    /// An error frame can always acess the full 8-byte data payload.
    fn data(&self) -> &[u8] {
        &self.0.data[..]
    }
}

impl Frame for CanErrorFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> u32 {
        self.0.can_id
    }
}

impl fmt::Debug for CanErrorFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CanErrorFrame {{ ")?;
        fmt::UpperHex::fmt(self, f)?;
        write!(f, " }}")
    }
}

impl fmt::UpperHex for CanErrorFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}#", self.0.can_id)?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = " ";
        write!(f, "{}", parts.join(sep))
    }
}

impl TryFrom<can_frame> for CanErrorFrame {
    type Error = ConstructionError;

    /// Try to create a `CanErrorFrame` from a C `can_frame`
    ///
    /// This will only succeed the C frame is marked as an error frame.
    fn try_from(frame: can_frame) -> Result<Self, Self::Error> {
        if frame.can_id & CAN_ERR_FLAG != 0 {
            Ok(Self(frame))
        } else {
            Err(ConstructionError::WrongFrameType)
        }
    }
}

impl AsRef<can_frame> for CanErrorFrame {
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
    /// Initialize a FD frame from the raw components.
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

        let mut frame = canfd_frame_default();
        frame.can_id = init_id_word(id, flags)?;
        frame.len = n as u8;
        frame.flags = fd_flags.bits();
        frame.data[..n].copy_from_slice(data);

        Ok(Self(frame))
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
}

impl AsPtr for CanFdFrame {
    type Inner = canfd_frame;

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    fn as_ptr(&self) -> *const Self::Inner {
        &self.0
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    fn as_mut_ptr(&mut self) -> *mut Self::Inner {
        &mut self.0
    }
}

impl EmbeddedFrame for CanFdFrame {
    /// Create a new FD frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let id = id.into();
        let mut flags = IdFlags::empty();
        flags.set(IdFlags::EFF, id_is_extended(&id));

        let raw_id = id_to_raw(id);
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
        Self(canfd_frame_default())
    }
}

impl fmt::Debug for CanFdFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CanFdFrame {{ ")?;
        fmt::UpperHex::fmt(self, f)?;
        write!(f, " }}")
    }
}

impl fmt::UpperHex for CanFdFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}##", self.0.can_id)?;
        write!(f, "{} ", self.0.flags)?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        //let sep = if f.alternate() { " " } else { " " };
        let sep = " ";
        write!(f, "{}", parts.join(sep))
    }
}

impl From<CanDataFrame> for CanFdFrame {
    fn from(frame: CanDataFrame) -> Self {
        let n = frame.dlc();

        let mut fdframe = canfd_frame_default();
        // TODO: force rtr off?
        fdframe.can_id = frame.id_word();
        fdframe.len = n as u8;
        fdframe.data[..n].copy_from_slice(&frame.data()[..n]);
        Self(fdframe)
    }
}

impl From<canfd_frame> for CanFdFrame {
    fn from(frame: canfd_frame) -> Self {
        Self(frame)
    }
}

impl AsRef<canfd_frame> for CanFdFrame {
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

    const EXT_LOW_ID: Id = Id::Extended(unsafe { ExtendedId::new_unchecked(0x7FF) });

    const DATA: &[u8] = &[0, 1, 2, 3];
    const DATA_LEN: usize = DATA.len();

    const ZERO_DATA: &[u8] = &[0u8; DATA_LEN];

    #[test]
    fn test_data_frame() {
        let frame = CanFrame::new(STD_ID, DATA).unwrap();
        assert_eq!(STD_ID, frame.id());
        //assert_eq!(STD_ID.as_raw(), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert_eq!(DATA, frame.data());

        let frame = CanFrame::new(EXT_ID, DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        //assert_eq!(EXT_ID.as_raw(), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert_eq!(DATA, frame.data());

        // Should keep Extended flag even if ID <= 0x7FF (standard range)
        let frame = CanFrame::new(EXT_LOW_ID, DATA).unwrap();
        assert_eq!(EXT_LOW_ID, frame.id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
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
        assert_eq!(ZERO_DATA, frame.data());
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
        assert_eq!(DATA, frame.data());

        let frame = CanFdFrame::new(EXT_ID, DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        //assert_eq!(EXT_ID.as_raw(), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert_eq!(DATA, frame.data());

        // Should keep Extended flag even if ID <= 0x7FF (standard range)
        let frame = CanFdFrame::new(EXT_LOW_ID, DATA).unwrap();
        assert_eq!(EXT_LOW_ID, frame.id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
    }

    #[test]
    fn test_frame_to_fd() {
        let frame = CanDataFrame::new(STD_ID, DATA).unwrap();

        let frame = CanFdFrame::from(frame);
        assert_eq!(STD_ID, frame.id());
        assert!(frame.is_standard());
        assert!(frame.is_data_frame());
        assert_eq!(DATA, frame.data());
    }
}
