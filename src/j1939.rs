// socketcan/src/j1939.rs
//
// SAE J1939 Protocol Support: 29-bit Extended Identifier Decomposition,
// Parameter Group Number (PGN) parsing, and Priority/Source Address extraction.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Implementation of SAE J1939 29-bit Extended CAN Identifier decomposition.
//!
//! Compliant with SAE J1939-21 (Data Link Layer).
//! Decomposes 29-bit extended CAN arbitration identifiers into:
//! - **Priority** (3 bits)
//! - **Extended Data Page / EDP** (1 bit)
//! - **Data Page / DP** (1 bit)
//! - **PDU Format / PF** (8 bits)
//! - **PDU Specific / PS** (8 bits: Destination Address for PDU1, Group Extension for PDU2)
//! - **Source Address / SA** (8 bits)
//! - **Parameter Group Number / PGN** (18-bit composite)

use crate::ExtendedId;
use core::fmt;

/// An SAE J1939 29-bit identifier decomposed into its functional protocol fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct J1939Id {
    /// Priority (0 - 7, where 0 is highest priority).
    pub priority: u8,
    /// Extended Data Page (bit 25).
    pub edp: bool,
    /// Data Page (bit 24).
    pub dp: bool,
    /// PDU Format (bits 23-16).
    pub pdu_format: u8,
    /// PDU Specific (bits 15-8).
    pub pdu_specific: u8,
    /// Source Address (bits 7-0).
    pub source_address: u8,
    /// Parameter Group Number (PGN, 18 bits).
    pub pgn: u32,
}

impl J1939Id {
    /// Global Broadcast Address per SAE J1939 (0xFF / 255).
    pub const BROADCAST_ADDRESS: u8 = 0xFF;
    /// Null / Unassigned Address per SAE J1939 (0xFE / 254).
    pub const NULL_ADDRESS: u8 = 0xFE;

    /// Decomposes a 29-bit raw CAN extended identifier into an SAE J1939 identifier.
    ///
    /// # PGN Calculation Rule (SAE J1939-21)
    /// - If `pdu_format < 240` (PDU1 format, point-to-point):
    ///   The `pdu_specific` byte contains the Destination Address (DA).
    ///   `PGN = (DP << 16) | (PF << 8)` (bits 15-8 are zeroed in the PGN).
    /// - If `pdu_format >= 240` (PDU2 format, broadcast):
    ///   The `pdu_specific` byte contains the Group Extension (GE).
    ///   `PGN = (DP << 16) | (PF << 8) | PS`.
    pub fn from_raw(raw_id: u32) -> Self {
        let id = raw_id & 0x1FFF_FFFF; // Mask to 29 bits

        let priority = ((id >> 26) & 0x07) as u8;
        let edp = ((id >> 25) & 0x01) != 0;
        let dp = ((id >> 24) & 0x01) != 0;
        let pdu_format = ((id >> 16) & 0xFF) as u8;
        let pdu_specific = ((id >> 8) & 0xFF) as u8;
        let source_address = (id & 0xFF) as u8;

        let dp_bit = (dp as u32) << 16;
        let edp_bit = (edp as u32) << 17;

        let pgn = if pdu_format < 240 {
            // PDU1: PDU Specific represents Destination Address (DA)
            edp_bit | dp_bit | ((pdu_format as u32) << 8)
        } else {
            // PDU2: PDU Specific represents Group Extension (GE)
            edp_bit | dp_bit | ((pdu_format as u32) << 8) | (pdu_specific as u32)
        };

        Self {
            priority,
            edp,
            dp,
            pdu_format,
            pdu_specific,
            source_address,
            pgn,
        }
    }

    /// Converts from an `embedded_can::ExtendedId`.
    pub fn from_extended_id(ext_id: ExtendedId) -> Self {
        Self::from_raw(ext_id.as_raw())
    }

    /// Converts the J1939 fields back into a raw 29-bit extended CAN ID.
    pub fn to_raw(&self) -> u32 {
        let prio = (self.priority as u32 & 0x07) << 26;
        let edp = (self.edp as u32) << 25;
        let dp = (self.dp as u32) << 24;
        let pf = (self.pdu_format as u32) << 16;
        let ps = (self.pdu_specific as u32) << 8;
        let sa = self.source_address as u32;

        prio | edp | dp | pf | ps | sa
    }

    /// Constructs an `embedded_can::ExtendedId` from this J1939 identifier.
    pub fn to_extended_id(&self) -> Option<ExtendedId> {
        ExtendedId::new(self.to_raw())
    }

    /// Returns `true` if this message uses PDU1 format (point-to-point addressing).
    pub fn is_pdu1(&self) -> bool {
        self.pdu_format < 240
    }

    /// Returns `true` if this message uses PDU2 format (broadcast / group extension).
    pub fn is_pdu2(&self) -> bool {
        self.pdu_format >= 240
    }

    /// If PDU1, returns `Some(destination_address)`. If PDU2 (broadcast), returns `None`.
    pub fn destination_address(&self) -> Option<u8> {
        if self.is_pdu1() {
            Some(self.pdu_specific)
        } else {
            None
        }
    }
}

impl From<ExtendedId> for J1939Id {
    fn from(id: ExtendedId) -> Self {
        Self::from_extended_id(id)
    }
}

impl fmt::Display for J1939Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_pdu1() {
            write!(
                f,
                "J1939 [PGN: 0x{:04X} ({}), Pri: {}, SA: 0x{:02X}, DA: 0x{:02X}]",
                self.pgn,
                self.pgn,
                self.priority,
                self.source_address,
                self.pdu_specific
            )
        } else {
            write!(
                f,
                "J1939 [PGN: 0x{:04X} ({}), Pri: {}, SA: 0x{:02X}, Broadcast]",
                self.pgn,
                self.pgn,
                self.priority,
                self.source_address
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pdu2_broadcast_eec1() {
        // Electronic Engine Controller 1 (EEC1)
        // Standard raw ID: 0x0CF00400 (Priority 3, PGN 61444 / 0xF004, SA 0x00)
        let raw = 0x0CF00400;
        let id = J1939Id::from_raw(raw);

        assert_eq!(id.priority, 3);
        assert_eq!(id.edp, false);
        assert_eq!(id.dp, false);
        assert_eq!(id.pdu_format, 0xF0); // 240 -> PDU2
        assert_eq!(id.pdu_specific, 0x04);
        assert_eq!(id.source_address, 0x00);
        assert_eq!(id.pgn, 61444); // 0xF004
        assert!(id.is_pdu2());
        assert!(!id.is_pdu1());
        assert_eq!(id.destination_address(), None);
        assert_eq!(id.to_raw(), raw);
    }

    #[test]
    fn test_pdu1_point_to_point() {
        // Torque / Speed Control 1 (TSC1)
        // PGN 0 (0x0000). Priority 3, PF 0x00, DA 0x00 (Engine), SA 0x03 (Transmission)
        // Raw ID: (3 << 26) | (0x00 << 16) | (0x00 << 8) | 0x03 = 0x0C000003
        let raw = 0x0C000003;
        let id = J1939Id::from_raw(raw);

        assert_eq!(id.priority, 3);
        assert_eq!(id.pdu_format, 0x00); // < 240 -> PDU1
        assert_eq!(id.pdu_specific, 0x00); // Destination Address = 0x00
        assert_eq!(id.source_address, 0x03);
        assert_eq!(id.pgn, 0); // PDU1 zeroes the DA in the PGN
        assert!(id.is_pdu1());
        assert_eq!(id.destination_address(), Some(0x00));
        assert_eq!(id.to_raw(), raw);
    }

    #[test]
    fn test_data_page_bit() {
        // Test with DP (Data Page) bit 24 asserted
        // Priority 6, DP = 1, PF = 0xFE, GE = 0xD6, SA = 0x21
        // Raw: (6 << 26) | (1 << 24) | (0xFE << 16) | (0xD6 << 8) | 0x21
        let raw = (6 << 26) | (1 << 24) | (0xFE << 16) | (0xD6 << 8) | 0x21;
        let id = J1939Id::from_raw(raw);

        assert_eq!(id.priority, 6);
        assert!(id.dp);
        assert_eq!(id.pdu_format, 0xFE);
        assert_eq!(id.pdu_specific, 0xD6);
        assert_eq!(id.source_address, 0x21);
        // PGN = (1 << 16) | (0xFE << 8) | 0xD6 = 65536 + 65024 + 214 = 130774
        assert_eq!(id.pgn, (1 << 16) | (0xFE << 8) | 0xD6);
        assert_eq!(id.to_raw(), raw);
    }
}
