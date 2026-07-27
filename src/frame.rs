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

use crate::{CanError, CanErrors, ConstructionError, id::CanId};
use embedded_can::{ExtendedId, Frame as EmbeddedFrame, Id, StandardId};
use libc::{can_frame, canfd_frame, canid_t};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void,
    mem::size_of,
    {convert::TryFrom, fmt, matches, mem},
};

// TODO: Remove these on the next major ver update.
pub use crate::id::{
    CAN_EFF_FLAG, CAN_EFF_MASK, CAN_ERR_FLAG, CAN_ERR_MASK, CAN_MAX_DLEN, CAN_RTR_FLAG,
    CAN_SFF_MASK, CANFD_BRS, CANFD_ESI, CANFD_FDF, CANFD_MAX_DLEN, ERR_MASK_ALL, ERR_MASK_NONE,
    FdFlags, IdFlags, id_from_raw, id_is_extended, id_to_canid_t,
};

// ===== can_frame =====

/// Creates a default C `can_frame`.
/// This initializes the entire structure to zeros.
#[inline(always)]
pub fn can_frame_default() -> can_frame {
    unsafe { mem::zeroed() }
}

/// Creates a default C `canfd_frame`.
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
    ///
    /// # Safety
    ///
    /// All `self.size()` bytes of the inner value — including any padding —
    /// must be initialised at the time of this call. Reading the returned
    /// slice is undefined behaviour otherwise. Note that `set_data` does not
    /// zero the unused trailing bytes of `can_frame::data`, so a frame built
    /// through typed accessors may still contain uninitialised padding.
    unsafe fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts::<'_, u8>(
                self.as_ptr() as *const _ as *const u8,
                self.size(),
            )
        }
    }

    /// Gets a mutable byte slice to the inner type
    ///
    /// # Safety
    ///
    /// Either all `self.size()` bytes of the inner value must be initialised
    /// at the time of the call, OR the caller must overwrite the entire slice
    /// before reading from it. Constructing the slice is sound, but reading
    /// uninitialised bytes through it is undefined behaviour.
    unsafe fn as_bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.as_mut_ptr() as *mut u8, self.size()) }
    }
}

// ===== Frame trait =====

/// Shared trait for CAN frames
#[allow(clippy::len_without_is_empty)]
pub trait Frame: EmbeddedFrame {
    /// Creates a frame using a raw, integer CAN ID.
    ///
    /// If the `id` is <= 0x7FF, it's assumed to be a standard ID, otherwise
    /// it is created as an Extended ID. If you require an Extended ID <= 0x7FF,
    /// use `new()`.
    fn from_raw_id(id: u32, data: &[u8]) -> Option<Self> {
        Self::new(id_from_raw(id)?, data)
    }

    /// Creates a remote frame using a raw, integer CAN ID.
    ///
    /// If the `id` is <= 0x7FF, it's assumed to be a standard ID, otherwise
    /// it is created as an Extended ID. If you require an Extended ID <= 0x7FF,
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

    /// Return the CAN ID.
    fn can_id(&self) -> CanId {
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

    /// Return the CAN ID as the embedded HAL Id type.
    fn hal_id(&self) -> Id {
        self.can_id().as_id()
    }

    /// Get the data length
    fn len(&self) -> usize {
        // For standard frames, dlc == len
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
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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

impl Frame for CanAnyFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> canid_t {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.id_word(),
            Remote(frame) => frame.id_word(),
            Error(frame) => frame.id_word(),
            Fd(frame) => frame.id_word(),
        }
    }

    /// Sets the CAN ID for the frame
    fn set_id(&mut self, id: impl Into<Id>) {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.set_id(id),
            Remote(frame) => frame.set_id(id),
            Error(frame) => frame.set_id(id),
            Fd(frame) => frame.set_id(id),
        }
    }

    /// Sets the data payload of the frame.
    fn set_data(&mut self, data: &[u8]) -> Result<(), ConstructionError> {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.set_data(data),
            Remote(frame) => frame.set_data(data),
            Error(frame) => frame.set_data(data),
            Fd(frame) => frame.set_data(data),
        }
    }
}

impl EmbeddedFrame for CanAnyFrame {
    /// Create a new CAN frame.
    ///
    /// Picks `CanDataFrame` when the data fits in a classic 8-byte payload,
    /// otherwise creates a `CanFdFrame`.
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        if data.len() <= CAN_MAX_DLEN {
            CanDataFrame::new(id, data).map(CanAnyFrame::Normal)
        } else {
            CanFdFrame::new(id, data).map(CanAnyFrame::Fd)
        }
    }

    /// Create a new remote transmission request frame.
    fn new_remote(id: impl Into<Id>, dlc: usize) -> Option<Self> {
        CanRemoteFrame::new_remote(id, dlc).map(CanAnyFrame::Remote)
    }

    /// Check if frame uses 29-bit extended ID format.
    fn is_extended(&self) -> bool {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.is_extended(),
            Remote(frame) => frame.is_extended(),
            Error(frame) => frame.is_extended(),
            Fd(frame) => frame.is_extended(),
        }
    }

    /// Check if frame is a remote transmission request.
    fn is_remote_frame(&self) -> bool {
        matches!(self, CanAnyFrame::Remote(_))
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.id(),
            Remote(frame) => frame.id(),
            Error(frame) => frame.id(),
            Fd(frame) => frame.id(),
        }
    }

    /// Data length
    fn dlc(&self) -> usize {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.dlc(),
            Remote(frame) => frame.dlc(),
            Error(frame) => frame.dlc(),
            Fd(frame) => frame.dlc(),
        }
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    fn data(&self) -> &[u8] {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.data(),
            Remote(frame) => frame.data(),
            Error(frame) => frame.data(),
            Fd(frame) => frame.data(),
        }
    }
}

impl fmt::UpperHex for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.fmt(f),
            Remote(frame) => frame.fmt(f),
            Error(frame) => frame.fmt(f),
            Fd(frame) => frame.fmt(f),
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

impl From<CanDataFrame> for CanAnyFrame {
    fn from(frame: CanDataFrame) -> Self {
        Self::Normal(frame)
    }
}

impl From<CanRemoteFrame> for CanAnyFrame {
    fn from(frame: CanRemoteFrame) -> Self {
        Self::Remote(frame)
    }
}

impl From<CanErrorFrame> for CanAnyFrame {
    fn from(frame: CanErrorFrame) -> Self {
        Self::Error(frame)
    }
}

impl From<CanFdFrame> for CanAnyFrame {
    fn from(frame: CanFdFrame) -> Self {
        Self::Fd(frame)
    }
}

impl From<can_frame> for CanAnyFrame {
    fn from(frame: can_frame) -> Self {
        let frame = CanFrame::from(frame);
        frame.into()
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
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.as_ptr() as *const Self::Inner,
            Remote(frame) => frame.as_ptr() as *const Self::Inner,
            Error(frame) => frame.as_ptr() as *const Self::Inner,
            Fd(frame) => frame.as_ptr() as *const Self::Inner,
        }
    }

    fn as_mut_ptr(&mut self) -> *mut Self::Inner {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.as_mut_ptr() as *mut Self::Inner,
            Remote(frame) => frame.as_mut_ptr() as *mut Self::Inner,
            Error(frame) => frame.as_mut_ptr() as *mut Self::Inner,
            Fd(frame) => frame.as_mut_ptr() as *mut Self::Inner,
        }
    }

    fn size(&self) -> usize {
        use CanAnyFrame::*;
        match self {
            Normal(frame) => frame.size(),
            Remote(frame) => frame.size(),
            Error(frame) => frame.size(),
            Fd(frame) => frame.size(),
        }
    }
}

impl TryFrom<CanAnyFrame> for CanDataFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanAnyFrame) -> Result<Self, Self::Error> {
        match frame {
            CanAnyFrame::Normal(f) => Ok(f),
            _ => Err(ConstructionError::WrongFrameType),
        }
    }
}

impl TryFrom<CanAnyFrame> for CanRemoteFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanAnyFrame) -> Result<Self, Self::Error> {
        match frame {
            CanAnyFrame::Remote(f) => Ok(f),
            _ => Err(ConstructionError::WrongFrameType),
        }
    }
}

impl TryFrom<CanAnyFrame> for CanErrorFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanAnyFrame) -> Result<Self, Self::Error> {
        match frame {
            CanAnyFrame::Error(f) => Ok(f),
            _ => Err(ConstructionError::WrongFrameType),
        }
    }
}

impl TryFrom<CanAnyFrame> for CanFdFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanAnyFrame) -> Result<Self, Self::Error> {
        match frame {
            CanAnyFrame::Fd(f) => Ok(f),
            _ => Err(ConstructionError::WrongFrameType),
        }
    }
}

// ===== CanFrame =====

/// The classic CAN 2.0 frame with up to 8-bytes of data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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

impl TryFrom<CanFrame> for CanDataFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanFrame) -> Result<Self, Self::Error> {
        match frame {
            CanFrame::Data(f) => Ok(f),
            _ => Err(ConstructionError::WrongFrameType),
        }
    }
}

impl TryFrom<CanFrame> for CanRemoteFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanFrame) -> Result<Self, Self::Error> {
        match frame {
            CanFrame::Remote(f) => Ok(f),
            _ => Err(ConstructionError::WrongFrameType),
        }
    }
}

impl TryFrom<CanFrame> for CanErrorFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanFrame) -> Result<Self, Self::Error> {
        match frame {
            CanFrame::Error(f) => Ok(f),
            _ => Err(ConstructionError::WrongFrameType),
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
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(into = "CanDataFrameRepr", try_from = "CanDataFrameRepr")
)]
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

    /// Sets the CAN ID for the frame.
    ///
    /// Preserves any RTR/ERR flag bits already in the ID word — the type's
    /// invariant says they shouldn't be set on a data frame, but masking
    /// defensively avoids silent state loss if a caller has manipulated
    /// the inner `can_frame` directly.
    fn set_id(&mut self, id: impl Into<Id>) {
        self.0.can_id = id_to_canid_t(id) | (self.0.can_id & (CAN_ERR_FLAG | CAN_RTR_FLAG));
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
    /// The default data frame has all fields and data set to zero, and all flags off.
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
        if self.is_extended() {
            write!(f, "{:08X}#", self.raw_id())?;
        } else {
            write!(f, "{:03X}#", self.raw_id())?;
        }
        for byte in self.data() {
            write!(f, "{:02X}", byte)?;
        }
        Ok(())
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
        match frame.len() {
            n if n > CAN_MAX_DLEN => Err(ConstructionError::TooMuchData),
            n => CanDataFrame::init(frame.id_word(), &frame.data()[..n]),
        }
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
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(into = "CanRemoteFrameRepr", try_from = "CanRemoteFrameRepr")
)]
pub struct CanRemoteFrame(can_frame);

impl CanRemoteFrame {
    /// Initializes a CAN data frame from raw parts.
    pub(crate) fn init(can_id: canid_t, len: usize) -> Result<Self, ConstructionError> {
        match len {
            n if n <= CAN_MAX_DLEN => {
                let mut frame = can_frame_default();
                frame.can_id = can_id | CAN_RTR_FLAG;
                frame.can_dlc = n as u8;
                Ok(Self(frame))
            }
            _ => Err(ConstructionError::TooMuchData),
        }
    }

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
        let can_id = id_to_canid_t(id);
        Self::init(can_id, dlc).ok()
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

    /// A slice into the actual data.
    ///
    /// Remote frames carry no payload by spec — only the DLC is meaningful.
    /// This always returns an empty slice; use [`dlc()`](Self::dlc) to read
    /// the requested length.
    fn data(&self) -> &[u8] {
        &[]
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
        if self.is_extended() {
            write!(f, "{:08X}", self.raw_id())?;
        } else {
            write!(f, "{:03X}", self.raw_id())?;
        }
        let dlc = self.dlc();
        if dlc == 0 {
            f.write_str("#R")
        } else {
            write!(f, "#R{:X}", dlc)
        }
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
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(into = "CanErrorFrameRepr", try_from = "CanErrorFrameRepr")
)]
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

    /// Decodes this error frame into the set of errors it describes.
    ///
    /// A single error frame commonly reports several distinct conditions, so
    /// this yields a non-empty [`CanErrors`] collection rather than one
    /// error. See the [errors module](crate::errors) for the layout.
    pub fn into_errors(self) -> CanErrors {
        CanErrors::from(self)
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
    /// An error frame can always access the full 8-byte data payload.
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
        // Render the full 29-bit error-class field so future kernel additions
        // outside CAN_SFF_MASK stay readable. `error_bits()` already strips
        // CAN_ERR_FLAG and any other non-class bits.
        write!(f, "{:08X}#", self.error_bits())?;
        for byte in self.data() {
            write!(f, "{:02X}", byte)?;
        }
        Ok(())
    }
}

impl TryFrom<can_frame> for CanErrorFrame {
    type Error = ConstructionError;

    /// Try to create a `CanErrorFrame` from a C `can_frame`
    ///
    /// This will only succeed the C frame is marked as an error frame.
    fn try_from(mut frame: can_frame) -> Result<Self, Self::Error> {
        if frame.can_id & CAN_ERR_FLAG != 0 {
            // Error frames carry the full 8-byte payload by convention; force
            // can_dlc so that `dlc()`, `len()` and `data()` agree.
            frame.can_dlc = CAN_MAX_DLEN as u8;
            Ok(Self(frame))
        } else {
            Err(ConstructionError::WrongFrameType)
        }
    }
}

impl From<CanErrors> for CanErrorFrame {
    /// Encodes a set of errors back into a single error frame.
    ///
    /// This is the inverse of [`CanErrorFrame::into_errors()`] and
    /// round-trips byte-for-byte for anything that has a wire
    /// representation. Class bits and the bitfield data bytes accumulate, so
    /// several controller problems or violation types collapse back into the
    /// one byte they came from.
    ///
    /// The one lossy case is [`CanError::DecodingFailure`], which describes a
    /// byte pattern we could not interpret rather than a condition the bus
    /// reported; it has no encoding and is skipped. Note also that several
    /// protocol violations sharing a location all write the same `data[3]`,
    /// and that a set holding violations with *differing* locations cannot be
    /// represented — the last one written wins, since the frame has only one
    /// location byte.
    fn from(errs: CanErrors) -> Self {
        use CanError::*;

        let mut data = [0u8; CAN_MAX_DLEN];
        let mut id: canid_t = 0;

        for err in &errs {
            match *err {
                TransmitTimeout => id |= libc::CAN_ERR_TX_TIMEOUT,
                LostArbitration(bit) => {
                    id |= libc::CAN_ERR_LOSTARB;
                    data[0] = bit;
                }
                ControllerProblem(prob) => {
                    id |= libc::CAN_ERR_CRTL;
                    // OR, not assign: data[1] is a bitfield and a frame can
                    // report several problems at once.
                    data[1] |= prob as u8;
                }
                ProtocolViolation { vtype, location } => {
                    id |= libc::CAN_ERR_PROT;
                    data[2] |= vtype as u8;
                    data[3] = location.as_raw();
                }
                TransceiverError(trx) => {
                    id |= libc::CAN_ERR_TRX;
                    // Two independent nibbles: CAN High in the low half,
                    // CAN Low in the high half.
                    data[4] |= trx as u8;
                }
                NoAck => id |= libc::CAN_ERR_ACK,
                BusOff => id |= libc::CAN_ERR_BUSOFF,
                BusError => id |= libc::CAN_ERR_BUSERROR,
                Restarted => id |= libc::CAN_ERR_RESTARTED,
                Counters { tx, rx } => {
                    id |= libc::CAN_ERR_CNT;
                    data[6] = tx;
                    data[7] = rx;
                }
                // No wire representation; see the note above.
                DecodingFailure(_) => (),
                Unknown(bits) => id |= bits,
            }
        }
        Self::new_error(id, &data).unwrap()
    }
}

impl From<CanError> for CanErrorFrame {
    /// Encodes a single error into an error frame.
    fn from(err: CanError) -> Self {
        Self::from(CanErrors::from_single(err))
    }
}

impl AsRef<can_frame> for CanErrorFrame {
    fn as_ref(&self) -> &can_frame {
        &self.0
    }
}

// ===== CanFdFrame =====

// Valid extended data lengths
const VALID_EXT_DLENGTHS: [usize; 7] = [12, 16, 20, 24, 32, 48, 64];

/// The CAN flexible data rate frame with up to 64-bytes of data.
///
/// This is highly compatible with the `canfd_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.canfd_frame.html))
///
/// Payload data that is greater than 8 bytes and whose data length does
/// not match a valid CANFD data length is padded with 0 bytes to the
/// next higher valid CANFD data length.
///
/// Note:
///   - The FDF flag is forced on when created.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(into = "CanFdFrameRepr", try_from = "CanFdFrameRepr")
)]
pub struct CanFdFrame(canfd_frame);

impl CanFdFrame {
    /// Create a new FD frame with FD flags
    pub fn with_flags(id: impl Into<Id>, data: &[u8], flags: FdFlags) -> Option<Self> {
        let can_id = id_to_canid_t(id);
        Self::init(can_id, data, flags).ok()
    }

    /// Initialize an FD frame from the raw components.
    pub(crate) fn init(
        can_id: u32,
        data: &[u8],
        fd_flags: FdFlags,
    ) -> Result<Self, ConstructionError> {
        match data.len() {
            n if n <= CANFD_MAX_DLEN => {
                let mut frame = canfd_frame_default();
                frame.can_id = can_id;
                frame.flags = (fd_flags | FdFlags::FDF).bits();
                frame.data[..n].copy_from_slice(data);
                frame.len = Self::next_valid_ext_dlen(n) as u8;
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
            self.0.flags &= !(CANFD_ESI as u8);
        }
    }

    /// Checks whether a given length is a valid CANFD data length.
    ///
    /// Valid values are `0` - `8`, `12`, `16`, `20`, `24`, `32`, `48` or `64`.
    pub fn is_valid_data_len(len: usize) -> bool {
        len <= CAN_MAX_DLEN || VALID_EXT_DLENGTHS.contains(&len)
    }

    /// Returns the next larger valid CANFD extended data length into which
    /// the given length fits, up to a maximum of CANFD_MAX_DLEN.
    pub fn next_valid_ext_dlen(len: usize) -> usize {
        if len <= CAN_MAX_DLEN {
            return len;
        }
        for valid_ext_len in VALID_EXT_DLENGTHS {
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

    /// CAN FD does not support remote transmission requests by spec
    /// (CAN FD frames have no RTR bit), so this always returns `None`.
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
    /// This should only be one of the valid CAN FD data lengths.
    fn data(&self) -> &[u8] {
        &self.0.data[..(self.0.len as usize)]
    }
}

impl Frame for CanFdFrame {
    /// Get the composite SocketCAN ID word, with EFF/RTR/ERR flags
    fn id_word(&self) -> canid_t {
        self.0.can_id
    }

    /// Get the data length
    fn len(&self) -> usize {
        // For FD frames, len not always equal to dlc
        self.0.len as usize
    }

    /// Sets the CAN ID for the frame.
    ///
    /// Preserves any RTR/ERR flag bits already in the ID word. FD frames
    /// don't carry these by spec, but masking defensively avoids silent
    /// state loss if a caller has manipulated the inner `canfd_frame`
    /// directly.
    fn set_id(&mut self, id: impl Into<Id>) {
        self.0.can_id = id_to_canid_t(id) | (self.0.can_id & (CAN_ERR_FLAG | CAN_RTR_FLAG));
    }

    /// Sets the data payload of the frame.
    fn set_data(&mut self, data: &[u8]) -> Result<(), ConstructionError> {
        match data.len() {
            n if n <= CANFD_MAX_DLEN => {
                self.0.data[..n].copy_from_slice(data);
                self.0.data[n..].fill(0);
                self.0.len = Self::next_valid_ext_dlen(n) as u8;
                Ok(())
            }
            _ => Err(ConstructionError::TooMuchData),
        }
    }
}

impl Default for CanFdFrame {
    /// The default FD frame has all fields and data set to zero, and all flags off.
    fn default() -> Self {
        let mut frame = canfd_frame_default();
        frame.flags |= CANFD_FDF as u8;
        Self(frame)
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
        if self.is_extended() {
            write!(f, "{:08X}##{:X}", self.raw_id(), self.0.flags & 0x0F)?;
        } else {
            write!(f, "{:03X}##{:X}", self.raw_id(), self.0.flags & 0x0F)?;
        }
        for byte in self.data() {
            write!(f, "{:02X}", byte)?;
        }
        Ok(())
    }
}

impl From<CanDataFrame> for CanFdFrame {
    fn from(frame: CanDataFrame) -> Self {
        let n = frame.len();

        let mut fdframe = canfd_frame_default();
        fdframe.can_id = frame.id_word();
        fdframe.flags = CANFD_FDF as u8;
        fdframe.len = n as u8;
        fdframe.data[..n].copy_from_slice(&frame.data()[..n]);
        Self(fdframe)
    }
}

impl From<canfd_frame> for CanFdFrame {
    fn from(mut frame: canfd_frame) -> Self {
        frame.flags |= CANFD_FDF as u8;
        // Defensively normalise `len` so that `dlc()` and `data()` agree
        // even if a non-spec length somehow lands here. First clamp to the
        // buffer size to keep `data()` from indexing out of bounds, then
        // round up to the next valid CANFD length and zero the padding so
        // we don't expose uninitialised bytes.
        let actual = (frame.len as usize).min(CANFD_MAX_DLEN);
        let normalised = Self::next_valid_ext_dlen(actual);
        frame.data[actual..normalised].fill(0);
        frame.len = normalised as u8;
        Self(frame)
    }
}

impl AsRef<canfd_frame> for CanFdFrame {
    fn as_ref(&self) -> &canfd_frame {
        &self.0
    }
}

/////////////////////////////////////////////////////////////////////////////
// serde support
//
// The frame types are newtypes over the C `can_frame` / `canfd_frame`, neither
// of which implements serde, so a plain derive is not possible. Each type
// converts through a logical repr instead, which also gets validation on
// deserialize for free by routing back through the normal constructors.
//
// Note that the raw C structs are deliberately *not* serialized: they contain
// padding and potentially-uninitialised bytes (the same reason `as_bytes` is
// `unsafe`), and their layout is not portable.

/// `serialize_with` / `deserialize_with` for frame payload fields.
///
/// Goes through `serialize_bytes` so formats with a native byte-string type
/// (MessagePack `bin`, CBOR byte strings, bincode) use it rather than encoding
/// a sequence of integers. Text formats such as JSON have no byte-string type
/// and fall back to a sequence, which is why the visitor accepts both.
#[cfg(feature = "serde")]
mod payload {
    use serde::{
        Deserializer, Serializer,
        de::{Error, SeqAccess, Visitor},
    };
    use std::fmt;

    pub fn serialize<S: Serializer>(data: &[u8], ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_bytes(data)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<u8>, D::Error> {
        struct V;

        impl<'de> Visitor<'de> for V {
            type Value = Vec<u8>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a byte string or a sequence of bytes")
            }

            fn visit_bytes<E: Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                Ok(v.to_vec())
            }

            fn visit_byte_buf<E: Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                Ok(v)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element()? {
                    out.push(b);
                }
                Ok(out)
            }
        }

        de.deserialize_bytes(V)
    }
}

/// Serialized form of a [`CanDataFrame`].
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanDataFrameRepr {
    /// The frame identifier
    pub id: CanId,
    /// The payload, 0..=8 bytes
    #[serde(default, with = "payload", skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<u8>,
}

#[cfg(feature = "serde")]
impl From<CanDataFrame> for CanDataFrameRepr {
    fn from(frame: CanDataFrame) -> Self {
        Self {
            id: frame.can_id(),
            data: frame.data().to_vec(),
        }
    }
}

#[cfg(feature = "serde")]
impl TryFrom<CanDataFrameRepr> for CanDataFrame {
    type Error = ConstructionError;

    fn try_from(repr: CanDataFrameRepr) -> Result<Self, Self::Error> {
        Self::new(repr.id, &repr.data).ok_or(ConstructionError::TooMuchData)
    }
}

/// Serialized form of a [`CanRemoteFrame`].
///
/// A remote frame carries no payload; only the requested length is meaningful.
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CanRemoteFrameRepr {
    /// The frame identifier
    pub id: CanId,
    /// The requested data length code
    #[serde(default)]
    pub dlc: usize,
}

#[cfg(feature = "serde")]
impl From<CanRemoteFrame> for CanRemoteFrameRepr {
    fn from(frame: CanRemoteFrame) -> Self {
        Self {
            id: frame.can_id(),
            dlc: frame.dlc(),
        }
    }
}

#[cfg(feature = "serde")]
impl TryFrom<CanRemoteFrameRepr> for CanRemoteFrame {
    type Error = ConstructionError;

    fn try_from(repr: CanRemoteFrameRepr) -> Result<Self, Self::Error> {
        Self::new_remote(repr.id, repr.dlc).ok_or(ConstructionError::TooMuchData)
    }
}

/// Serialized form of a [`CanErrorFrame`].
///
/// The identifier of an error frame is a set of error-class bits rather than a
/// bus address, and the payload is always the full eight class-detail bytes.
/// See the [errors module](crate::errors) for their meaning.
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanErrorFrameRepr {
    /// The error class bits, without `CAN_ERR_FLAG`
    pub error_bits: u32,
    /// The eight error-detail bytes
    #[serde(default, with = "payload", skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<u8>,
}

#[cfg(feature = "serde")]
impl From<CanErrorFrame> for CanErrorFrameRepr {
    fn from(frame: CanErrorFrame) -> Self {
        Self {
            error_bits: frame.error_bits(),
            data: frame.data().to_vec(),
        }
    }
}

#[cfg(feature = "serde")]
impl TryFrom<CanErrorFrameRepr> for CanErrorFrame {
    type Error = ConstructionError;

    fn try_from(repr: CanErrorFrameRepr) -> Result<Self, Self::Error> {
        Self::new_error(repr.error_bits, &repr.data)
    }
}

/// Serialized form of a [`CanFdFrame`].
#[cfg(feature = "serde")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanFdFrameRepr {
    /// The frame identifier
    pub id: CanId,
    /// The FD flags (BRS / ESI / FDF)
    pub flags: FdFlags,
    /// The payload, 0..=64 bytes
    #[serde(default, with = "payload", skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<u8>,
}

#[cfg(feature = "serde")]
impl From<CanFdFrame> for CanFdFrameRepr {
    fn from(frame: CanFdFrame) -> Self {
        Self {
            id: frame.can_id(),
            flags: frame.flags(),
            data: frame.data().to_vec(),
        }
    }
}

#[cfg(feature = "serde")]
impl TryFrom<CanFdFrameRepr> for CanFdFrame {
    type Error = ConstructionError;

    fn try_from(repr: CanFdFrameRepr) -> Result<Self, Self::Error> {
        Self::with_flags(repr.id, &repr.data, repr.flags).ok_or(ConstructionError::TooMuchData)
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
        assert_eq!(frame.data(), DATA);
        assert_eq!(frame.len(), DATA.len());
        assert_eq!(frame.data().len(), DATA.len());
        assert_eq!(frame.dlc(), DATA.len());

        let frame = CanFrame::from(frame);
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(frame.data(), DATA);
        assert_eq!(frame.len(), DATA.len());
        assert_eq!(frame.data().len(), DATA.len());
        assert_eq!(frame.dlc(), DATA.len());

        let frame = CanDataFrame::from_raw_id(StandardId::MAX.as_raw() as u32, DATA).unwrap();
        assert_eq!(STD_ID, frame.id());
        assert_eq!(id_to_raw(STD_ID), frame.raw_id());
        assert!(frame.is_standard());
        assert!(!frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(frame.data(), DATA);
        assert_eq!(frame.len(), DATA.len());
        assert_eq!(frame.data().len(), DATA.len());
        assert_eq!(frame.dlc(), DATA.len());

        let frame = CanFrame::new(EXT_ID, DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        assert_eq!(id_to_raw(EXT_ID), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(frame.data(), DATA);
        assert_eq!(frame.len(), DATA.len());
        assert_eq!(frame.data().len(), DATA.len());
        assert_eq!(frame.dlc(), DATA.len());

        let frame = CanFrame::from_raw_id(ExtendedId::MAX.as_raw(), DATA).unwrap();
        assert_eq!(EXT_ID, frame.id());
        assert_eq!(id_to_raw(EXT_ID), frame.raw_id());
        assert!(!frame.is_standard());
        assert!(frame.is_extended());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert_eq!(frame.data(), DATA);
        assert_eq!(frame.len(), DATA.len());
        assert_eq!(frame.data().len(), DATA.len());
        assert_eq!(frame.dlc(), DATA.len());

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
        assert_eq!(EMPTY_DATA, frame.data());

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
        assert_eq!(EMPTY_DATA, frame.data());

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
        assert_eq!(EMPTY_DATA, frame.data());

        assert!(matches!(frame, CanFrame::Remote(_)));
        assert!(frame.id_flags().contains(IdFlags::RTR));
        assert_eq!(CAN_RTR_FLAG, frame.id_word() & CAN_RTR_FLAG);

        let frame = CanRemoteFrame::new_remote(STD_ID, CAN_MAX_DLEN + 1);
        assert!(frame.is_none());
    }

    #[test]
    fn test_error_frame() {
        // Create an error frame indicating transceiver error
        // from a C frame.
        let mut frame = can_frame_default();
        frame.can_id = CAN_ERR_FLAG | 0x0010;

        // A bare CAN_ERR_TRX with data[4] == 0 is an unspecified
        // transceiver fault.
        let errs = CanErrors::from(CanErrorFrame(frame));
        assert_eq!(
            *errs.first(),
            CanError::TransceiverError(errors::TransceiverError::Unspecified)
        );

        let id = StandardId::new(0x0010).unwrap();
        let frame = CanErrorFrame::new(id, &[]).unwrap();
        assert!(!frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(frame.is_error_frame());

        let errs = CanErrors::from(frame);
        assert_eq!(
            *errs.first(),
            CanError::TransceiverError(errors::TransceiverError::Unspecified)
        );

        let id = ExtendedId::new(0x0020).unwrap();
        let frame = CanErrorFrame::new(id, &[]).unwrap();
        assert!(!frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(frame.is_error_frame());

        let errs = CanErrors::from(frame);
        assert_eq!(*errs.first(), CanError::NoAck);
        assert!(errs.is_single());

        // From CanErrors

        let frame = CanErrorFrame::from(CanError::TransmitTimeout);
        assert!(!frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(frame.is_error_frame());

        let errs = frame.into_errors();
        assert!(errs.is_single());
        assert_eq!(*errs.first(), CanError::TransmitTimeout);

        let err = CanError::ProtocolViolation {
            vtype: errors::ViolationType::BitStuffingError,
            location: errors::Location::Id0400,
        };
        let frame = CanErrorFrame::from(err);
        assert!(!frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(frame.is_error_frame());

        let errs = frame.into_errors();
        assert_eq!(errs.len(), 1);
        assert_eq!(*errs.first(), err);
    }

    /// Encoding a decoded frame must reproduce it byte for byte.
    ///
    /// This is the guard against the family of encode bugs where a bitfield
    /// byte is assigned rather than OR'd, a class bit is written without its
    /// data byte (or the reverse), or a whole field is dropped.
    #[test]
    fn test_error_frame_round_trip() {
        use crate::errors::{ControllerProblem, Location, TransceiverError, ViolationType};
        use libc::{
            CAN_ERR_ACK, CAN_ERR_BUSERROR, CAN_ERR_BUSOFF, CAN_ERR_CNT, CAN_ERR_CRTL,
            CAN_ERR_LOSTARB, CAN_ERR_PROT, CAN_ERR_RESTARTED, CAN_ERR_TRX, CAN_ERR_TX_TIMEOUT,
        };

        // (class bits, data) pairs drawn from real driver behaviour.
        let cases: &[(u32, [u8; 8])] = &[
            // Single classes.
            (CAN_ERR_TX_TIMEOUT, [0; 8]),
            (CAN_ERR_BUSOFF, [0; 8]),
            (CAN_ERR_RESTARTED, [0; 8]),
            (CAN_ERR_ACK, [0; 8]),
            (CAN_ERR_LOSTARB, [7, 0, 0, 0, 0, 0, 0, 0]),
            // can_change_state(): symmetric warning and passive transitions.
            (CAN_ERR_CRTL, [0, 0x0C, 0, 0, 0, 0, 0, 0]),
            (CAN_ERR_CRTL, [0, 0x30, 0, 0, 0, 0, 0, 0]),
            // sja1000: overrun plus a state change.
            (CAN_ERR_CRTL, [0, 0x0D, 0, 0, 0, 0, 0, 0]),
            // m_can / peak_canfd: CRTL|CNT written literally.
            (CAN_ERR_CRTL | CAN_ERR_CNT, [0, 0x10, 0, 0, 0, 0, 130, 42]),
            // mcp251xfd_handle_ivmif(): three classes, five data[2] bits.
            (
                CAN_ERR_PROT | CAN_ERR_BUSERROR | CAN_ERR_ACK,
                [0, 0, 0x9E, 0x08, 0, 0, 0, 0],
            ),
            // es58x: both transceiver halves set.
            (CAN_ERR_TRX, [0, 0, 0, 0, 0x44, 0, 0, 0]),
            (CAN_ERR_TRX, [0, 0, 0, 0, 0x05, 0, 0, 0]),
            // sja1000 five-class accumulation.
            (
                CAN_ERR_CRTL | CAN_ERR_CNT | CAN_ERR_PROT | CAN_ERR_BUSERROR | CAN_ERR_LOSTARB,
                [3, 0x0D, 0x86, 0x1C, 0, 0, 200, 190],
            ),
            // A location with no name, preserved as Location::Reserved.
            (CAN_ERR_PROT, [0, 0, 0x02, 0x1F, 0, 0, 0, 0]),
            // Unspecified sub-codes.
            (CAN_ERR_CRTL, [0; 8]),
            (CAN_ERR_PROT, [0, 0, 0, 0x03, 0, 0, 0, 0]),
            (CAN_ERR_TRX, [0; 8]),
        ];

        for (bits, data) in cases {
            let orig = CanErrorFrame::new_error(*bits, data).unwrap();
            let errs = orig.into_errors();
            let again = CanErrorFrame::from(errs.clone());
            assert_eq!(
                again, orig,
                "round-trip failed for bits={:#x} data={:02X?}\n  decoded as: {}",
                bits, data, errs
            );
        }

        // Every value of data[3] survives a round trip, including the seven
        // in-range codes with no name and everything above 0x1F.
        for v in 0u8..=0xFF {
            let orig =
                CanErrorFrame::new_error(CAN_ERR_PROT, &[0, 0, 0x02, v, 0, 0, 0, 0]).unwrap();
            assert_eq!(
                CanErrorFrame::from(orig.into_errors()),
                orig,
                "data[3]={v:#04x}"
            );
        }

        // The documented lossy case: a decoding failure has no encoding.
        let errs = CanErrors::new(
            CanError::BusOff,
            [CanError::DecodingFailure(
                errors::CanErrorDecodingFailure::InvalidControllerProblem,
            )],
        );
        let frame = CanErrorFrame::from(errs);
        assert_eq!(frame.error_bits(), CAN_ERR_BUSOFF);
        assert_eq!(frame.into_errors().len(), 1);

        // Sanity: the pieces used above really do carry their raw values.
        assert_eq!(ControllerProblem::ReceiveErrorWarning as u8, 0x04);
        assert_eq!(ViolationType::TransmissionError as u8, 0x80);
        assert_eq!(TransceiverError::CanLowNoWire as u8, 0x40);
        assert_eq!(Location::OverloadFlag.as_raw(), 0x1C);
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
    }

    #[test]
    fn test_fd_ext_data_len() {
        assert!(CanFdFrame::is_valid_data_len(8));
        assert!(CanFdFrame::is_valid_data_len(12));
        assert!(CanFdFrame::is_valid_data_len(24));
        assert!(CanFdFrame::is_valid_data_len(64));

        assert!(!CanFdFrame::is_valid_data_len(28));
        assert!(!CanFdFrame::is_valid_data_len(42));
        assert!(!CanFdFrame::is_valid_data_len(65));

        assert_eq!(CanFdFrame::next_valid_ext_dlen(9), 12);
        assert_eq!(CanFdFrame::next_valid_ext_dlen(13), 16);
        assert_eq!(CanFdFrame::next_valid_ext_dlen(17), 20);
        assert_eq!(CanFdFrame::next_valid_ext_dlen(21), 24);
        assert_eq!(CanFdFrame::next_valid_ext_dlen(25), 32);
        assert_eq!(CanFdFrame::next_valid_ext_dlen(33), 48);
        assert_eq!(CanFdFrame::next_valid_ext_dlen(49), 64);

        assert_eq!(CanFdFrame::next_valid_ext_dlen(99), 64);
    }

    #[test]
    fn test_fd_frame_padding() {
        // Creating a frame w/ invalid length should "pad up"
        let mut frame = CanFdFrame::new(STD_ID, EXT_DATA_INVALID_DLEN).unwrap();

        assert_eq!(frame.data(), EXT_DATA_PADDED);
        assert_eq!(frame.len(), EXT_DATA_PADDED.len());
        assert_eq!(frame.data().len(), frame.len());
        assert_eq!(frame.dlc(), EXT_DATA_PADDED_DLC);

        // Creating a frame w/ valid length
        frame = CanFdFrame::new(STD_ID, EXT_DATA).unwrap();

        assert_eq!(frame.data(), EXT_DATA);
        assert_eq!(frame.len(), EXT_DATA.len());
        assert_eq!(frame.data().len(), frame.len());
        assert_eq!(frame.dlc(), EXT_DATA_DLC);

        // Setting frame data to smaller length should pad w/ zeros
        frame.set_data(EXT_DATA_INVALID_DLEN).unwrap();

        assert_eq!(frame.data(), EXT_DATA_PADDED);
        assert_eq!(frame.len(), EXT_DATA_PADDED.len());
        assert_eq!(frame.data().len(), frame.len());
        assert_eq!(frame.dlc(), EXT_DATA_PADDED_DLC);
    }

    #[test]
    fn test_to_fd_frame() {
        let data_frame = CanDataFrame::new(STD_ID, DATA).unwrap();

        let frame = CanFdFrame::from(data_frame);

        assert_eq!(STD_ID, frame.id());
        assert!(frame.is_standard());
        assert!(frame.is_data_frame());
        assert!(!frame.is_remote_frame());
        assert!(!frame.is_error_frame());
        assert!(frame.flags().contains(FdFlags::FDF));
        assert_eq!(frame.len(), DATA_LEN);
        assert_eq!(frame.data().len(), DATA_LEN);
        assert_eq!(frame.data(), DATA);

        let fdframe = canfd_frame_default();
        let frame = CanFdFrame::from(fdframe);
        assert!(frame.flags().contains(FdFlags::FDF));
    }

    #[test]
    fn test_fd_to_data_frame() {
        let fdframe = CanFdFrame::new(STD_ID, DATA).unwrap();
        assert!(fdframe.flags().contains(FdFlags::FDF));

        let frame = CanDataFrame::try_from(fdframe).unwrap();

        assert_eq!(STD_ID, frame.id());
        assert_eq!(frame.len(), DATA_LEN);
        assert_eq!(frame.data().len(), DATA_LEN);
        assert_eq!(frame.data(), DATA);

        // Make sure FD flags turned off
        let mut fdframe = canfd_frame_default();
        // SAFETY: `fdframe` is zero-initialised by `canfd_frame_default`;
        // `frame.0` was constructed by `CanDataFrame::new` which zeroes the
        // backing `can_frame` then sets fields, so all bytes are initialised.
        unsafe {
            crate::as_bytes_mut(&mut fdframe)[..size_of::<can_frame>()]
                .clone_from_slice(crate::as_bytes(&frame.0));
        }
        assert_eq!(fdframe.flags, 0);
    }

    #[test]
    fn test_frame_eq() {
        // Two CanDataFrames built from the same id/data are equal.
        let a = CanDataFrame::new(STD_ID, DATA).unwrap();
        let b = CanDataFrame::new(STD_ID, DATA).unwrap();
        assert_eq!(a, b);

        // Different data → not equal.
        let c = CanDataFrame::new(STD_ID, &[0xDE, 0xAD]).unwrap();
        assert_ne!(a, c);

        // Different id → not equal.
        let d = CanDataFrame::new(EXT_ID, DATA).unwrap();
        assert_ne!(a, d);

        // Remote frames with the same DLC are equal.
        let r1 = CanRemoteFrame::new_remote(STD_ID, 4).unwrap();
        let r2 = CanRemoteFrame::new_remote(STD_ID, 4).unwrap();
        assert_eq!(r1, r2);
        let r3 = CanRemoteFrame::new_remote(STD_ID, 5).unwrap();
        assert_ne!(r1, r3);

        // Error frames with the same class+data are equal.
        let e1 = CanErrorFrame::new_error(0x10, &[]).unwrap();
        let e2 = CanErrorFrame::new_error(0x10, &[]).unwrap();
        assert_eq!(e1, e2);
        let e3 = CanErrorFrame::new_error(0x20, &[]).unwrap();
        assert_ne!(e1, e3);

        // FD frames: equal across constructor, distinct on id/data.
        let fd1 = CanFdFrame::new(STD_ID, DATA).unwrap();
        let fd2 = CanFdFrame::new(STD_ID, DATA).unwrap();
        assert_eq!(fd1, fd2);
        let fd3 = CanFdFrame::new(STD_ID, &[0u8; 16]).unwrap();
        assert_ne!(fd1, fd3);

        // CanFrame variants do not cross-compare.
        let data = CanFrame::Data(a);
        let remote = CanFrame::Remote(r1);
        assert_ne!(data, remote);

        // CanRawFrame: same variant + same bytes => equal; cross-variant unequal.
        // `CanRawFrame` intentionally doesn't impl `Debug`, so use `assert!`
        // rather than `assert_eq!`.
        let raw_a: CanRawFrame = (*a.as_ref()).into();
        let raw_b: CanRawFrame = (*b.as_ref()).into();
        assert!(raw_a == raw_b);
        let raw_fd: CanRawFrame = (*fd1.as_ref()).into();
        assert!(raw_a != raw_fd);

        // Hash agrees with Eq on equal frames; insertion into a HashSet
        // dedupes byte-identical frames.
        use std::collections::HashSet;
        let mut set: HashSet<CanDataFrame> = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 1);
        set.insert(c);
        assert_eq!(set.len(), 2);
    }
}
