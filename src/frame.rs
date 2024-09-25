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
//!
//! At the lowest level, [libc](https://crates.io/crates/libc) defines the
//! CAN frames as low-level structs that are binary compatible with the C
//! data types sent to and from the kernel:
//! - [can_frame](https://docs.rs/libc/latest/libc/struct.can_frame.html)
//!   The Classic CAN 2.0 frame with up to 8 bytes of data.
//! - [canfd_frame](https://docs.rs/libc/latest/libc/struct.canfd_frame.html)
//!   The CAN Flexible Data Rate frame with up to 64 bytes of data.
//!
//! The classic frame represents three possibilities:
//! - `CanDataFrame` - A standard CAN frame that can contain up to 8 bytes
//!   of data.
//! - `CanRemoteFrame` - A CAN Remote frame which is meant to request a
//!   transmission by another node on the bus. It contain no data.
//! - `CanErrorFrame` - This is an incoming (only) frame that contains
//!   information about a problem on the bus or in the driver. Error frames
//!   can not be sent to the bus, but can be converted to standard Rust
//!   [Error](https://doc.rust-lang.org/std/error/trait.Error.html) types.
//!

use crate::{CanError, ConstructionError};
use bitflags::bitflags;
use embedded_can::{ExtendedId, Frame as EmbeddedFrame, Id, StandardId};
use itertools::Itertools;
use libc::{can_frame, canfd_frame, canid_t};
use std::{
    ffi::c_void,
    mem::size_of,
    {convert::TryFrom, fmt, matches, mem},
};

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

/// Gets the canid_t value from an Id
/// If it's an extended ID, the CAN_EFF_FLAG bit is also set.
pub fn id_to_canid_t(id: impl Into<Id>) -> canid_t {
    let id = id.into();
    match id {
        Id::Standard(id) => id.as_raw() as canid_t,
        Id::Extended(id) => id.as_raw() | CAN_EFF_FLAG,
    }
}

/// Determines if the ID is a 29-bit extended ID.
pub fn id_is_extended(id: &Id) -> bool {
    matches!(id, Id::Extended(_))
}

/// Creates a CAN ID from a raw integer value.
///
/// If the `id` is <= 0x7FF, it's assumed to be a standard ID, otherwise
/// it is created as an Extened ID. If you require an Extended ID <= 0x7FF,
/// create it explicitly.
pub fn id_from_raw(id: u32) -> Option<Id> {
    let id = match id {
        n if n <= CAN_SFF_MASK => StandardId::new(n as u16)?.into(),
        n => ExtendedId::new(n)?.into(),
    };
    Some(id)
}

// ===== can_frame =====

/// Creates a default C `can_frame`.
/// This initializes the entire structure to zeros.
#[inline(always)]
pub fn can_frame_default() -> can_frame {
    unsafe { mem::zeroed() }
}

/// Creates a default C `can_frame`.
/// This initializes the entire structure to zeros.
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
    fn size(&self) -> usize {
        size_of::<Self::Inner>()
    }

    /// Gets a byte slice to the inner type
    fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts::<'_, u8>(
                self.as_ptr() as *const _ as *const u8,
                self.size(),
            )
        }
    }

    /// Gets a mutable byte slice to the inner type
    fn as_bytes_mut(&mut self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts::<'_, u8>(
                self.as_mut_ptr() as *mut _ as *mut u8,
                self.size(),
            )
        }
    }
}

// ===== Frame trait =====

/// Shared trait for CAN frames
#[allow(clippy::len_without_is_empty)]
pub trait Frame: EmbeddedFrame {
    /// Creates a frame using a raw, integer CAN ID.
    ///
    /// If the `id` is <= 0x7FF, it's assumed to be a standard ID, otherwise
    /// it is created as an Extened ID. If you require an Etended ID <= 0x7FF,
    /// use `new()`.
    fn from_raw_id(id: u32, data: &[u8]) -> Option<Self> {
        Self::new(id_from_raw(id)?, data)
    }

    /// Creates a remote frame using a raw, integer CAN ID.
    ///
    /// If the `id` is <= 0x7FF, it's assumed to be a standard ID, otherwise
    /// it is created as an Extened ID. If you require an Etended ID <= 0x7FF,
    /// use `new_remote()`.
    fn remote_from_raw_id(id: u32, dlc: usize) -> Option<Self> {
        Self::new_remote(id_from_raw(id)?, dlc)
    }

    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> canid_t;

    /// Return the actual raw CAN ID (without EFF/RTR/ERR flags)
    fn raw_id(&self) -> canid_t {
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
            ExtendedId::new(self.id_word() & CAN_EFF_MASK)
                .unwrap()
                .into()
        } else {
            StandardId::new((self.id_word() & CAN_SFF_MASK) as u16)
                .unwrap()
                .into()
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

    /// Sets the CAN ID for the frame
    fn set_id(&mut self, id: impl Into<Id>);

    /// Sets the data payload of the frame.
    fn set_data(&mut self, data: &[u8]) -> Result<(), ConstructionError>;
}

// ===== CanAnyFrame =====

/// An FD socket can read a raw classic 2.0 or FD frame.
#[allow(missing_debug_implementations)]
#[derive(Clone, Copy)]
pub enum CanRawFrame {
    /// A classic CAN 2.0 frame, with up to 8-bytes of data
    Classic(can_frame),
    /// A flexible data rate frame, with up to 64-bytes of data
    Fd(canfd_frame),
}

impl From<can_frame> for CanRawFrame {
    fn from(frame: can_frame) -> Self {
        Self::Classic(frame)
    }
}

impl From<canfd_frame> for CanRawFrame {
    fn from(frame: canfd_frame) -> Self {
        Self::Fd(frame)
    }
}

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

impl From<can_frame> for CanAnyFrame {
    fn from(frame: can_frame) -> Self {
        let frame = CanFrame::from(frame);
        frame.into()
    }
}

impl From<CanFdFrame> for CanAnyFrame {
    fn from(frame: CanFdFrame) -> Self {
        Self::Fd(frame)
    }
}

impl From<canfd_frame> for CanAnyFrame {
    fn from(frame: canfd_frame) -> Self {
        let frame = CanFdFrame::from(frame);
        frame.into()
    }
}

impl From<CanRawFrame> for CanAnyFrame {
    fn from(frame: CanRawFrame) -> Self {
        use CanRawFrame::*;
        match frame {
            Classic(frame) => frame.into(),
            Fd(frame) => frame.into(),
        }
    }
}

impl AsPtr for CanAnyFrame {
    type Inner = c_void;

    fn as_ptr(&self) -> *const Self::Inner {
        match self {
            CanAnyFrame::Normal(frame) => frame.as_ptr() as *const Self::Inner,
            CanAnyFrame::Remote(frame) => frame.as_ptr() as *const Self::Inner,
            CanAnyFrame::Error(frame) => frame.as_ptr() as *const Self::Inner,
            CanAnyFrame::Fd(frame) => frame.as_ptr() as *const Self::Inner,
        }
    }

    fn as_mut_ptr(&mut self) -> *mut Self::Inner {
        match self {
            CanAnyFrame::Normal(frame) => frame.as_mut_ptr() as *mut Self::Inner,
            CanAnyFrame::Remote(frame) => frame.as_mut_ptr() as *mut Self::Inner,
            CanAnyFrame::Error(frame) => frame.as_mut_ptr() as *mut Self::Inner,
            CanAnyFrame::Fd(frame) => frame.as_mut_ptr() as *mut Self::Inner,
        }
    }

    fn size(&self) -> usize {
        match self {
            CanAnyFrame::Normal(frame) => frame.size(),
            CanAnyFrame::Remote(frame) => frame.size(),
            CanAnyFrame::Error(frame) => frame.size(),
            CanAnyFrame::Fd(frame) => frame.size(),
        }
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
    fn id_word(&self) -> canid_t {
        use CanFrame::*;
        match self {
            Data(frame) => frame.id_word(),
            Remote(frame) => frame.id_word(),
            Error(frame) => frame.id_word(),
        }
    }

    /// Sets the CAN ID for the frame
    fn set_id(&mut self, id: impl Into<Id>) {
        use CanFrame::*;
        match self {
            Data(frame) => frame.set_id(id),
            Remote(frame) => frame.set_id(id),
            Error(frame) => frame.set_id(id),
        }
    }

    /// Sets the data payload of the frame.
    fn set_data(&mut self, data: &[u8]) -> Result<(), ConstructionError> {
        use CanFrame::*;
        match self {
            Data(frame) => frame.set_data(data),
            Remote(frame) => frame.set_data(data),
            Error(frame) => frame.set_data(data),
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
    /// Initializes a CAN data frame from raw parts.
    pub(crate) fn init(can_id: canid_t, data: &[u8]) -> Result<Self, ConstructionError> {
        match data.len() {
            n if n <= CAN_MAX_DLEN => {
                let mut frame = can_frame_default();
                frame.can_id = can_id;
                frame.can_dlc = n as u8;
                frame.data[..n].copy_from_slice(data);
                Ok(Self(frame))
            }
            _ => Err(ConstructionError::TooMuchData),
        }
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
        let can_id = id_to_canid_t(id);
        Self::init(can_id, data).ok()
    }

    /// Create a new remote transmission request frame.
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

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    fn data(&self) -> &[u8] {
        &self.0.data[..(self.0.can_dlc as usize)]
    }
}

impl Frame for CanDataFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> canid_t {
        self.0.can_id
    }

    /// Sets the CAN ID for the frame
    fn set_id(&mut self, id: impl Into<Id>) {
        self.0.can_id = id_to_canid_t(id);
    }

    /// Sets the data payload of the frame.
    fn set_data(&mut self, data: &[u8]) -> Result<(), ConstructionError> {
        match data.len() {
            n if n <= CAN_MAX_DLEN => {
                self.0.can_dlc = n as u8;
                self.0.data[..n].copy_from_slice(data);
                Ok(())
            }
            _ => Err(ConstructionError::TooMuchData),
        }
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
        write!(f, "{}", parts.join(" "))
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
        if frame.len() > CAN_MAX_DLEN {
            return Err(ConstructionError::TooMuchData);
        }

        CanDataFrame::init(frame.id_word(), &frame.data()[..(frame.0.len as usize)])
    }
}

impl AsRef<can_frame> for CanDataFrame {
    fn as_ref(&self) -> &can_frame {
        &self.0
    }
}

// ===== CanRemoteFrame =====

/// The classic CAN 2.0 remote request frame.
///
/// This is is meant to request a transmission by another node on the bus.
/// It contain no data.
///
/// This is highly compatible with the `can_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.can_frame.html))
#[derive(Clone, Copy)]
pub struct CanRemoteFrame(can_frame);

impl CanRemoteFrame {
    /// Sets the data length code for the frame
    pub fn set_dlc(&mut self, dlc: usize) -> Result<(), ConstructionError> {
        if dlc <= CAN_MAX_DLEN {
            self.0.can_dlc = dlc as u8;
            Ok(())
        } else {
            Err(ConstructionError::TooMuchData)
        }
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
    ///
    /// This will set the RTR flag in the CAN ID word.
    fn new_remote(id: impl Into<Id>, dlc: usize) -> Option<Self> {
        if dlc <= CAN_MAX_DLEN {
            let mut frame = can_frame_default();
            frame.can_id = id_to_canid_t(id) | CAN_RTR_FLAG;
            frame.can_dlc = dlc as u8;
            Some(Self(frame))
        } else {
            None
        }
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

    /// Data length code
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
    fn id_word(&self) -> canid_t {
        self.0.can_id
    }

    /// Sets the CAN ID for the frame.
    ///
    /// This will set the RTR flag in the CAN ID word.
    fn set_id(&mut self, id: impl Into<Id>) {
        self.0.can_id = id_to_canid_t(id) | CAN_RTR_FLAG;
    }

    /// Sets the data payload of the frame.
    /// For the Remote frame, this just updates the DLC to the length of the
    /// data slice.
    fn set_data(&mut self, data: &[u8]) -> Result<(), ConstructionError> {
        self.set_dlc(data.len())
    }
}

impl Default for CanRemoteFrame {
    /// The default remote frame has all fields and data set to zero, and all flags off.
    fn default() -> Self {
        let mut frame = can_frame_default();
        frame.can_id |= CAN_RTR_FLAG;
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
    /// Creates a CAN error frame from raw parts.
    ///
    /// Note that an application would not normally _ever_ create an error
    /// frame. This is included mainly to aid in implementing mocks and other
    /// tests for an application.
    ///
    /// The data byte slice should have the necessary codes for the supplied
    /// error. They will be zero padded to a full frame of 8 bytes.
    ///
    /// Also note:
    /// - The error flag is forced on
    /// - The other, non-error, flags are forced off
    /// - The frame data is always padded with zero's to 8 bytes,
    ///   regardless of the length of the `data` parameter provided.
    pub fn new_error(can_id: canid_t, data: &[u8]) -> Result<Self, ConstructionError> {
        match data.len() {
            n if n <= CAN_MAX_DLEN => {
                let mut frame = can_frame_default();
                frame.can_id = (can_id & CAN_ERR_MASK) | CAN_ERR_FLAG;
                frame.can_dlc = CAN_MAX_DLEN as u8;
                frame.data[..n].copy_from_slice(data);
                Ok(Self(frame))
            }
            _ => Err(ConstructionError::TooMuchData),
        }
    }

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
    /// Create an error frame.
    ///
    /// Note that an application would not normally _ever_ create an error
    /// frame. This is included mainly to aid in implementing mocks and other
    /// tests for an application.
    ///
    /// This will set the error bit in the CAN ID word.
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let can_id = id_to_canid_t(id);
        Self::new_error(can_id, data).ok()
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

    /// Check if frame is a data frame.
    fn is_data_frame(&self) -> bool {
        false
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length code
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
    fn id_word(&self) -> canid_t {
        self.0.can_id
    }

    /// Sets the CAN ID for the frame
    /// This does nothing on an error frame.
    fn set_id(&mut self, _id: impl Into<Id>) {}

    /// Sets the data payload of the frame.
    /// This is an error on an error frame.
    fn set_data(&mut self, _data: &[u8]) -> Result<(), ConstructionError> {
        Err(ConstructionError::WrongFrameType)
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
        write!(f, "{}", parts.join(" "))
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

impl From<CanError> for CanErrorFrame {
    fn from(err: CanError) -> Self {
        use CanError::*;

        let mut data = [0u8; CAN_MAX_DLEN];
        let id: canid_t = match err {
            TransmitTimeout => 0x0001,
            LostArbitration(bit) => {
                data[0] = bit;
                0x0002
            }
            ControllerProblem(prob) => {
                data[1] = prob as u8;
                0x0004
            }
            ProtocolViolation { vtype, location } => {
                data[2] = vtype as u8;
                data[3] = location as u8;
                0x0008
            }
            TransceiverError => 0x0010,
            NoAck => 0x0020,
            BusOff => 0x0040,
            BusError => 0x0080,
            Restarted => 0x0100,
            DecodingFailure(_failure) => 0,
            Unknown(e) => e,
        };
        Self::new_error(id, &data).unwrap()
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
    /// Create a new FD frame with FD flags
    pub fn with_flags(id: impl Into<Id>, data: &[u8], flags: FdFlags) -> Option<Self> {
        let can_id = id_to_canid_t(id);
        Self::init(can_id, data, flags).ok()
    }

    /// Initialize a FD frame from the raw components.
    pub(crate) fn init(
        can_id: u32,
        data: &[u8],
        fd_flags: FdFlags,
    ) -> Result<Self, ConstructionError> {
        match data.len() {
            n if n <= CANFD_MAX_DLEN => {
                let mut frame = canfd_frame_default();
                frame.can_id = can_id;
                frame.flags = fd_flags.bits();
                if n > 8 && !CanFdFrame::is_valid_data_len(n) {
                    // data must be 0 padded to the next valid DataLength
                    let new_len = CanFdFrame::next_valid_ext_dlen(n);
                    let mut padded_data: Vec<u8> = Vec::from(data);
                    padded_data.resize(new_len, 0);
                    frame.len = new_len as u8;
                    frame.data[..new_len].copy_from_slice(&padded_data);
                } else {
                    // payload length is a valid CANFD data length so no padding is required
                    frame.len = n as u8;
                    frame.data[..n].copy_from_slice(data);
                }

                Ok(Self(frame))
            }
            _ => Err(ConstructionError::TooMuchData),
        }
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

    /// Checks whether a given length is a valid CANFD data length.
    ///
    /// Valid values are `0`, `1`, `2`, `3`, `4`, `5`, `6`, `7`, `8`,
    /// `12`, `16`, `20`, `24`, `32`, `48` or `64`.
    fn is_valid_data_len(len: usize) -> bool {
        (0..=8).contains(&len) || [12, 16, 20, 24, 32, 48, 64].contains(&len)
    }

    /// Returns the next larger valid CANFD extended data length into which the given
    /// length fits, up to a maximum of CANFD_MAX_DLEN.
    fn next_valid_ext_dlen(len: usize) -> usize {
        let valid_ext_dlengths: [usize; 7] = [12, 16, 20, 24, 32, 48, 64];

        for valid_ext_len in valid_ext_dlengths {
            if valid_ext_len >= len {
                return valid_ext_len;
            }
        }
        // return CANFD_MAX_DLEN if len > CANFD_MAX_DLEN
        CANFD_MAX_DLEN
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
        let can_id = id_to_canid_t(id);
        Self::init(can_id, data, FdFlags::empty()).ok()
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

    /// Data length code
    fn dlc(&self) -> usize {
        match self.0.len {
            0..=8 => self.0.len as usize,
            12 => 0x09,
            16 => 0x0A,
            20 => 0x0B,
            24 => 0x0C,
            32 => 0x0D,
            48 => 0x0E,
            64 => 0x0F,
            // invalid data length, should never occur as the data is
            // padded to a valid CANFD data length on frame creation
            _ => 0x00,
        }
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
    fn id_word(&self) -> canid_t {
        self.0.can_id
    }

    /// Sets the CAN ID for the frame
    fn set_id(&mut self, id: impl Into<Id>) {
        self.0.can_id = id_to_canid_t(id);
    }

    /// Sets the data payload of the frame.
    fn set_data(&mut self, data: &[u8]) -> Result<(), ConstructionError> {
        match data.len() {
            n if n <= CANFD_MAX_DLEN => {
                if n > 8 && !CanFdFrame::is_valid_data_len(n) {
                    // data must be 0 padded to the next valid DataLength
                    let new_len = CanFdFrame::next_valid_ext_dlen(n);
                    let mut padded_data: Vec<u8> = Vec::from(data);
                    padded_data.resize(new_len, 0);
                    self.0.len = new_len as u8;
                    self.0.data[..new_len].copy_from_slice(&padded_data);
                } else {
                    self.0.len = n as u8;
                    self.0.data[..n].copy_from_slice(data);
                }
                Ok(())
            }
            _ => Err(ConstructionError::TooMuchData),
        }
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
        write!(f, "{}", parts.join(" "))
    }
}

impl From<CanDataFrame> for CanFdFrame {
    fn from(frame: CanDataFrame) -> Self {
        let n = frame.dlc();

        let mut fdframe = canfd_frame_default();
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
    use crate::errors;

    const STD_ID: Id = Id::Standard(StandardId::MAX);
    const EXT_ID: Id = Id::Extended(ExtendedId::MAX);

    const EXT_LOW_ID: Id = Id::Extended(unsafe { ExtendedId::new_unchecked(0x7FF) });

    const DATA: &[u8] = &[0, 1, 2, 3];
    const DATA_LEN: usize = DATA.len();

    const EXT_DATA: &[u8] = &[0xAB; 32];
    const EXT_DATA_DLC: usize = 0x0D;

    const EXT_DATA_INVALID_DLEN: &[u8] =
        &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA];
    const EXT_DATA_PADDED: &[u8] = &[
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0x00, 0x00,
    ];
    const EXT_DATA_PADDED_DLC: usize = 0x09;

    const EMPTY_DATA: &[u8] = &[];
    const ZERO_DATA: &[u8] = &[0u8; DATA_LEN];

    fn id_to_raw(id: Id) -> u32 {
        match id {
            Id::Standard(id) => id.as_raw() as u32,
            Id::Extended(id) => id.as_raw(),
        }
    }

    #[test]
    fn test_bit_flags() {
        let mut flags = IdFlags::RTR;
        assert_eq!(CAN_RTR_FLAG, flags.bits());

        flags.set(IdFlags::EFF, true);
        assert_eq!(CAN_RTR_FLAG, flags.bits() & CAN_RTR_FLAG);
        assert_eq!(CAN_EFF_FLAG, flags.bits() & CAN_EFF_FLAG);

        flags.set(IdFlags::EFF, false);
        assert_eq!(CAN_RTR_FLAG, flags.bits() & CAN_RTR_FLAG);
        assert_eq!(0, flags.bits() & CAN_EFF_FLAG);
    }

    #[test]
    fn test_defaults() {
        let frame = CanFrame::default();

        assert_eq!(0, frame.id_word());
        assert_eq!(0, frame.raw_id());
        assert!(frame.id_flags().is_empty());

        assert_eq!(0, frame.dlc());
        assert_eq!(0, frame.len());
        assert_eq!(EMPTY_DATA, frame.data());
    }

    #[test]
    fn test_data_frame() {
        let frame = CanDataFrame::new(STD_ID, DATA).unwrap();
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA, frame.data());

        let frame = CanFrame::from(frame);
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA, frame.data());

        let frame = CanDataFrame::from_raw_id(StandardId::MAX.as_raw() as u32, DATA).unwrap();
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA, frame.data());

        let frame = CanFrame::new(EXT_ID, DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        assert_eq!(id_to_raw(EXT_ID), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA, frame.data());

        let frame = CanFrame::from_raw_id(ExtendedId::MAX.as_raw(), DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        assert_eq!(id_to_raw(EXT_ID), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA, frame.data());

        // Should keep Extended flag even if ID <= 0x7FF (standard range)
        let frame = CanFrame::new(EXT_LOW_ID, DATA).unwrap();
        assert_eq!(EXT_LOW_ID, frame.id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
    }

    #[test]
    fn test_remote_frame() {
        let frame = CanRemoteFrame::default();
        assert_eq!(CAN_RTR_FLAG, frame.id_word());
        assert!(frame.is_remote_frame());
        assert_eq!(0, frame.dlc());
        assert_eq!(0, frame.len());
        assert_eq!(EMPTY_DATA, frame.data());

        assert!(frame.id_flags().contains(IdFlags::RTR));
        assert_eq!(CAN_RTR_FLAG, frame.id_word() & CAN_RTR_FLAG);

        let frame = CanRemoteFrame::new_remote(STD_ID, DATA_LEN).unwrap();
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(!frame.is_data_frame());
        assert!(frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA_LEN, frame.dlc());
        assert_eq!(DATA_LEN, frame.len());
        assert_eq!(ZERO_DATA, frame.data());

        assert!(frame.id_flags().contains(IdFlags::RTR));
        assert_eq!(CAN_RTR_FLAG, frame.id_word() & CAN_RTR_FLAG);

        let frame = CanFrame::from(frame);
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(!frame.is_data_frame());
        assert!(frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(ZERO_DATA, frame.data());

        assert!(matches!(frame, CanFrame::Remote(_)));
        assert!(frame.id_flags().contains(IdFlags::RTR));
        assert_eq!(CAN_RTR_FLAG, frame.id_word() & CAN_RTR_FLAG);

        let frame = CanFrame::new_remote(STD_ID, DATA_LEN).unwrap();
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(!frame.is_data_frame());
        assert!(frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(ZERO_DATA, frame.data());

        assert!(matches!(frame, CanFrame::Remote(_)));
        assert!(frame.id_flags().contains(IdFlags::RTR));
        assert_eq!(CAN_RTR_FLAG, frame.id_word() & CAN_RTR_FLAG);

        let frame = CanRemoteFrame::new_remote(STD_ID, CAN_MAX_DLEN + 1);
        assert!(frame.is_none());
    }

    #[test]
    fn test_error_frame() {
        // Create an error frame indicating transciever error
        // from a C frame.
        let mut frame = can_frame_default();
        frame.can_id = CAN_ERR_FLAG | 0x0010;

        let err = CanError::from(CanErrorFrame(frame));
        assert!(matches!(err, CanError::TransceiverError));

        let id = StandardId::new(0x0010).unwrap();
        let frame = CanErrorFrame::new(id, &[]).unwrap();
        assert!(!frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(frame.is_error_frame());

        let err = CanError::from(frame);
        assert!(matches!(err, CanError::TransceiverError));

        let id = ExtendedId::new(0x0020).unwrap();
        let frame = CanErrorFrame::new(id, &[]).unwrap();
        assert!(!frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(frame.is_error_frame());

        let err = CanError::from(frame);
        assert!(matches!(err, CanError::NoAck));

        // From CanErrors

        let frame = CanErrorFrame::from(CanError::TransmitTimeout);
        assert!(!frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(frame.is_error_frame());

        let err = frame.into_error();
        assert!(matches!(err, CanError::TransmitTimeout));

        let err = CanError::ProtocolViolation {
            vtype: errors::ViolationType::BitStuffingError,
            location: errors::Location::Id0400,
        };
        let frame = CanErrorFrame::from(err);
        assert!(!frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(frame.is_error_frame());

        let err = frame.into_error();
        match err {
            CanError::ProtocolViolation { vtype, location } => {
                assert_eq!(vtype, errors::ViolationType::BitStuffingError);
                assert_eq!(location, errors::Location::Id0400);
            }
            _ => {
                assert!(false);
            }
        }
    }

    #[test]
    fn test_fd_frame() {
        let frame = CanFdFrame::new(STD_ID, DATA).unwrap();
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA, frame.data());

        let frame = CanFdFrame::new(EXT_ID, DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        assert_eq!(id_to_raw(EXT_ID), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA, frame.data());

        // Should keep Extended flag even if ID <= 0x7FF (standard range)
        let frame = CanFdFrame::new(EXT_LOW_ID, DATA).unwrap();
        assert_eq!(EXT_LOW_ID, frame.id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());

        let mut frame = CanFdFrame::new(STD_ID, EXT_DATA).unwrap();
        assert_eq!(frame.dlc(), EXT_DATA_DLC);
        assert_eq!(frame.data(), EXT_DATA);
        frame.set_data(EXT_DATA_INVALID_DLEN).unwrap();
        assert_eq!(frame.data(), EXT_DATA_PADDED);
        assert_eq!(frame.dlc(), EXT_DATA_PADDED_DLC);

        let frame = CanFdFrame::new(STD_ID, EXT_DATA_INVALID_DLEN).unwrap();
        assert_eq!(frame.data(), EXT_DATA_PADDED);
        assert_eq!(frame.dlc(), EXT_DATA_PADDED_DLC);
    }

    #[test]
    fn test_frame_to_fd() {
        let frame = CanDataFrame::new(STD_ID, DATA).unwrap();

        let frame = CanFdFrame::from(frame);
        assert_eq!(STD_ID, frame.id());
        assert!(frame.is_standard());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(DATA, frame.data());
    }
}
