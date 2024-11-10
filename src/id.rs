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
use embedded_can::{ExtendedId, Id, StandardId};
use std::{io, ops};

/*pub*/
use libc::{CAN_EFF_MASK, CAN_SFF_MASK};

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
        CanId::Standard(id)
    }
}

impl From<ExtendedId> for CanId {
    #[inline]
    fn from(id: ExtendedId) -> Self {
        CanId::Extended(id)
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
        use CanId::*;
        let id = match id {
            n if n <= CAN_SFF_MASK => Standard(
                StandardId::new(n as u16).ok_or(io::Error::from(io::ErrorKind::InvalidInput))?,
            ),
            n => Extended(ExtendedId::new(n).ok_or(io::Error::from(io::ErrorKind::InvalidInput))?),
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
                Standard(StandardId::new((id.as_raw() + val as u16) & CAN_SFF_MASK as u16).unwrap())
            }
            Extended(id) => Extended(ExtendedId::new((id.as_raw() + val) & CAN_EFF_MASK).unwrap()),
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
