// socketcan/src/id.rs
//
// Implements CANbus Identifiers.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Implementation of CANbus standard and extended identifiers.

use crate::{Error, Result};
use bitflags::bitflags;
use embedded_can::{ExtendedId, Id, StandardId};
use libc::canid_t;
use std::{io, ops};

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
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct IdFlags: canid_t {
        /// Indicates frame uses a 29-bit extended ID
        const EFF = CAN_EFF_FLAG;
        /// Indicates a remote request frame.
        const RTR = CAN_RTR_FLAG;
        /// Indicates an error frame.
        const ERR = CAN_ERR_FLAG;
    }

    /// Bit flags for the Flexible Data (FD) frames.
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Ord, Eq, Hash)]
    pub struct FdFlags: u8 {
        /// Bit rate switch (second bit rate for payload data)
        const BRS = CANFD_BRS as u8;
        /// Error state indicator of the transmitting node
        const ESI = CANFD_ESI as u8;
        /// Mark CAN FD for dual use of struct canfd_frame
        /// Added in Linux kernel v5.14
        const FDF = 0x04u8;     // TODO: Sent upstream to libc 2024-12-27
    }
}

/// Gets the canid_t value from an Id
/// If it's an extended ID, the CAN_EFF_FLAG bit is also set.
pub fn id_to_canid_t(id: impl Into<Id>) -> canid_t {
    use Id::*;
    match id.into() {
        Standard(id) => id.as_raw() as canid_t,
        Extended(id) => id.as_raw() | CAN_EFF_FLAG,
    }
}

/// Determines if the ID is a standard, 11-bit, ID.
#[inline]
pub fn id_is_standard(id: &Id) -> bool {
    matches!(id, Id::Standard(_))
}

/// Determines if the ID is an extended, 29-bit, ID.
#[inline]
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

/////////////////////////////////////////////////////////////////////////////
/// A CAN identifier that can be standard or extended.
///
/// This is similar to and generally interchangeable with
/// [embedded_can::Id](https://docs.rs/embedded-can/latest/embedded_can/enum.Id.html)
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum CanId {
    /// Standard 11-bit Identifier (`0..=0x7FF`).
    Standard(StandardId),
    /// Extended 29-bit Identifier (`0..=0x1FFF_FFFF`).
    Extended(ExtendedId),
}

impl CanId {
    /// Creates a standard, 11-bit, ID
    pub fn standard(id: u16) -> Option<Self> {
        let id = StandardId::new(id)?;
        Some(Self::Standard(id))
    }

    /// Creates an extended, 29-bit, ID
    pub fn extended(id: u32) -> Option<Self> {
        let id = ExtendedId::new(id)?;
        Some(Self::Extended(id))
    }

    /// Gets the embedded_can::Id representation of the value.
    pub fn as_id(&self) -> Id {
        use CanId::*;
        match self {
            Standard(id) => Id::Standard(*id),
            Extended(id) => Id::Extended(*id),
        }
    }

    /// Gets the raw numeric value of the ID
    pub fn as_raw(&self) -> u32 {
        use CanId::*;
        match self {
            Standard(id) => id.as_raw() as u32,
            Extended(id) => id.as_raw(),
        }
    }

    /// Determines if the ID is a standard, 11-bit, ID.
    #[inline]
    pub fn is_standard(&self) -> bool {
        matches!(self, CanId::Standard(_))
    }

    /// Determines if the ID is an extended, 29-bit, ID.
    #[inline]
    pub fn is_extended(&self) -> bool {
        matches!(self, CanId::Extended(_))
    }
}

/// Implement `Ord` according to the CAN arbitration rules
///
/// This defers to the `Ord` implementation in the embedded_can crate.
impl Ord for CanId {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_id().cmp(&other.as_id())
    }
}

impl PartialOrd for CanId {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl From<StandardId> for CanId {
    #[inline]
    fn from(id: StandardId) -> Self {
        Self::Standard(id)
    }
}

impl From<ExtendedId> for CanId {
    #[inline]
    fn from(id: ExtendedId) -> Self {
        Self::Extended(id)
    }
}

impl From<Id> for CanId {
    /// Gets the embedded_can::Id representation of the value.
    fn from(id: Id) -> Self {
        use Id::*;
        match id {
            Standard(id) => Self::Standard(id),
            Extended(id) => Self::Extended(id),
        }
    }
}

impl From<CanId> for Id {
    #[inline]
    fn from(id: CanId) -> Self {
        id.as_id()
    }
}

/// Creates a CAN ID from a raw integer value.
///
/// If the `id` is <= 0x7FF, it's assumed to be a standard ID, otherwise
/// it is created as an Extened ID. If you require an Extended ID <= 0x7FF,
/// create it explicitly.
impl TryFrom<u32> for CanId {
    type Error = Error;

    fn try_from(id: u32) -> Result<Self> {
        let id = match id {
            n if n <= CAN_SFF_MASK => {
                Self::standard(n as u16).ok_or(io::Error::from(io::ErrorKind::InvalidInput))?
            }
            n => Self::extended(n).ok_or(io::Error::from(io::ErrorKind::InvalidInput))?,
        };
        Ok(id)
    }
}

impl ops::Add<u32> for CanId {
    type Output = Self;

    fn add(self, val: u32) -> Self::Output {
        use CanId::*;
        match self {
            Standard(id) => {
                Self::standard((id.as_raw() + val as u16) & CAN_SFF_MASK as u16).unwrap()
            }
            Extended(id) => Self::extended((id.as_raw() + val) & CAN_EFF_MASK).unwrap(),
        }
    }
}

impl ops::AddAssign<u32> for CanId {
    fn add_assign(&mut self, other: u32) {
        *self = *self + other;
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    // A standard ID
    const ID: u32 = 0x100;

    #[test]
    fn test_id_conv() {
        let sid = StandardId::MAX;
        let id = CanId::from(sid);

        assert!(id.is_standard());
        assert!(matches!(id, CanId::Standard(_)));
        assert_eq!(id.as_raw(), sid.as_raw() as u32);

        let eid = ExtendedId::MAX;
        let id = CanId::from(eid);

        assert!(id.is_extended());
        assert!(matches!(id, CanId::Extended(_)));
        assert_eq!(id.as_raw(), eid.as_raw());

        let sid = Id::from(StandardId::MAX);
        let id = CanId::from(sid);

        assert!(id.is_standard());
        assert!(matches!(id, CanId::Standard(_)));
        match sid {
            Id::Standard(sid) => assert_eq!(id.as_raw(), sid.as_raw() as u32),
            _ => assert!(false),
        };

        let eid = Id::from(ExtendedId::MAX);
        let id = CanId::from(eid);

        assert!(id.is_extended());
        assert!(matches!(id, CanId::Extended(_)));
        match eid {
            Id::Extended(eid) => assert_eq!(id.as_raw(), eid.as_raw()),
            _ => assert!(false),
        }
    }

    #[test]
    fn test_id_raw() {
        let id = CanId::try_from(ID).unwrap();
        assert!(matches!(id, CanId::Standard(_)));
        assert_eq!(id.as_raw(), ID);
    }

    #[test]
    fn test_id_add() {
        let id = CanId::try_from(ID).unwrap();
        let id = id + 1;

        assert!(matches!(id, CanId::Standard(_)));
        assert_eq!(id.as_raw(), ID + 1);

        let mut id = CanId::try_from(ID).unwrap();
        id += 1;

        assert!(matches!(id, CanId::Standard(_)));
        assert_eq!(id.as_raw(), ID + 1);
    }
}
