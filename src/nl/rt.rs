// socketcan/src/nl/rt.rs
//
// Low-level Netlink SocketCAN data structs, constants, and bindings.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Low-level Netlink SocketCAN data structs, constants, and bindings.
//!
//! These are missing from the `libc` and `neli` Rust crates, adding them
//! here as a stand-in for now. Most of these will be pushed upstream,
//! if/when possible.
//!

#![allow(non_camel_case_types, unused)]

use super::{as_bytes, as_bytes_mut};
use libc::{c_char, c_uint};
use neli::{
    consts::rtnl::{RtaType, RtaTypeWrapper},
    err::{DeError, SerError},
    impl_trait, neli_enum, FromBytes, Size, ToBytes,
};
use std::{
    io::{self, Cursor, Read, Write},
    mem,
};

pub const EXT_FILTER_VF: c_uint = 1 << 0;
pub const EXT_FILTER_BRVLAN: c_uint = 1 << 1;
pub const EXT_FILTER_BRVLAN_COMPRESSED: c_uint = 1 << 2;
pub const EXT_FILTER_SKIP_STATS: c_uint = 1 << 3;
pub const EXT_FILTER_MRP: c_uint = 1 << 4;
pub const EXT_FILTER_CFM_CONFIG: c_uint = 1 << 5;
pub const EXT_FILTER_CFM_STATUS: c_uint = 1 << 6;
pub const EXT_FILTER_MST: c_uint = 1 << 7;

/// CAN bit-timing parameters
///
/// For further information, please read chapter "8 BIT TIMING
/// REQUIREMENTS" of the "Bosch CAN Specification version 2.0"
/// at http://www.semiconductors.bosch.de/pdf/can2spec.pdf.
///
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, FromBytes, ToBytes, Size)]
pub struct can_bittiming {
    pub bitrate: u32,      // Bit-rate in bits/second
    pub sample_point: u32, // Sample point in one-tenth of a percent
    pub tq: u32,           // Time quanta (TQ) in nanoseconds
    pub prop_seg: u32,     // Propagation segment in TQs
    pub phase_seg1: u32,   // Phase buffer segment 1 in TQs
    pub phase_seg2: u32,   // Phase buffer segment 2 in TQs
    pub sjw: u32,          // Synchronisation jump width in TQs
    pub brp: u32,          // Bit-rate prescaler
}

/// CAN hardware-dependent bit-timing constant
/// Missing from libc, from linux/can/netlink.h:
///
/// Used for calculating and checking bit-timing parameters
///
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct can_bittiming_const {
    pub name: [c_char; 16], // Name of the CAN controller hardware
    pub tseg1_min: u32,     // Time segment 1 = prop_seg + phase_seg1
    pub tseg1_max: u32,
    pub tseg2_min: u32, // Time segment 2 = phase_seg2
    pub tseg2_max: u32,
    pub sjw_max: u32, // Synchronisation jump width
    pub brp_min: u32, // Bit-rate prescaler
    pub brp_max: u32,
    pub brp_inc: u32,
}

impl ToBytes for can_bittiming_const {
    fn to_bytes(&self, buf: &mut Cursor<Vec<u8>>) -> Result<(), SerError> {
        buf.write_all(as_bytes(self))?;
        Ok(())
    }
}

impl<'a> FromBytes<'a> for can_bittiming_const {
    fn from_bytes(buf: &mut Cursor<&'a [u8]>) -> Result<Self, DeError> {
        let mut timing_const: can_bittiming_const = unsafe { mem::zeroed() };
        buf.read_exact(as_bytes_mut(&mut timing_const))?;
        Ok(timing_const)
    }
}

impl Size for can_bittiming_const {
    fn unpadded_size(&self) -> usize {
        std::mem::size_of::<can_bittiming_const>()
    }
}

/// CAN clock parameters
///
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, FromBytes, ToBytes, Size)]
pub struct can_clock {
    pub freq: u32, // CAN system clock frequency in Hz
}

/// CAN operational and error states
///
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanState {
    ErrorActive,  // RX/TX error count < 96
    ErrorWarning, // RX/TX error count < 128
    ErrorPassive, // RX/TX error count < 256
    BusOff,       // RX/TX error count >= 256
    Stopped,      // Device is stopped
    Sleeping,     // Device is sleeping
}

impl TryFrom<u32> for CanState {
    type Error = io::Error;

    fn try_from(val: u32) -> Result<Self, Self::Error> {
        use CanState::*;

        match val {
            0 => Ok(ErrorActive),
            1 => Ok(ErrorWarning),
            2 => Ok(ErrorPassive),
            3 => Ok(BusOff),
            4 => Ok(Stopped),
            5 => Ok(Sleeping),
            _ => Err(io::Error::from(io::ErrorKind::InvalidData)),
        }
    }
}

/// CAN bus error counters
///
#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes, ToBytes, Size)]
pub struct can_berr_counter {
    pub txerr: u16,
    pub rxerr: u16,
}

/// CAN controller mode
///
/// To set or clear a bit, set the `mask` for that bit, then set or clear
/// the bit in the `flags` and send via `set_ctrlmode()`.
///
#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes, ToBytes, Size)]
pub struct can_ctrlmode {
    pub mask: u32,
    pub flags: u32,
}

/// Loopback mode
pub const CAN_CTRLMODE_LOOPBACK: u32 = 0x01;
/// Listen-only mode
pub const CAN_CTRLMODE_LISTENONLY: u32 = 0x02;
/// Triple sampling mode
pub const CAN_CTRLMODE_3_SAMPLES: u32 = 0x04;
/// One-Shot mode
pub const CAN_CTRLMODE_ONE_SHOT: u32 = 0x08;
/// Bus-error reporting
pub const CAN_CTRLMODE_BERR_REPORTING: u32 = 0x10;
/// CAN FD mode
pub const CAN_CTRLMODE_FD: u32 = 0x20;
/// Ignore missing CAN ACKs
pub const CAN_CTRLMODE_PRESUME_ACK: u32 = 0x40;
/// CAN FD in non-ISO mode
pub const CAN_CTRLMODE_FD_NON_ISO: u32 = 0x80;
/// Classic CAN DLC option
pub const CAN_CTRLMODE_CC_LEN8_DLC: u32 = 0x100;

/// u16 termination range: 1..65535 Ohms
pub const CAN_TERMINATION_DISABLED: u32 = 0;

///
/// CAN device statistics
///
#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes)]
pub struct can_device_stats {
    pub bus_error: u32,        // Bus errors
    pub error_warning: u32,    // Changes to error warning state
    pub error_passive: u32,    // Changes to error passive state
    pub bus_off: u32,          // Changes to bus off state
    pub arbitration_lost: u32, // Arbitration lost errors
    pub restarts: u32,         // CAN controller re-starts
}

pub const IFLA_CAN_UNSPEC: u16 = 0;
pub const IFLA_CAN_BITTIMING: u16 = 1;
pub const IFLA_CAN_BITTIMING_CONST: u16 = 2;
pub const IFLA_CAN_CLOCK: u16 = 3;
pub const IFLA_CAN_STATE: u16 = 4;
pub const IFLA_CAN_CTRLMODE: u16 = 5;
pub const IFLA_CAN_RESTART_MS: u16 = 6;
pub const IFLA_CAN_RESTART: u16 = 7;
pub const IFLA_CAN_BERR_COUNTER: u16 = 8;
pub const IFLA_CAN_DATA_BITTIMING: u16 = 9;
pub const IFLA_CAN_DATA_BITTIMING_CONST: u16 = 10;
pub const IFLA_CAN_TERMINATION: u16 = 11;
pub const IFLA_CAN_TERMINATION_CONST: u16 = 12;
pub const IFLA_CAN_BITRATE_CONST: u16 = 13;
pub const IFLA_CAN_DATA_BITRATE_CONST: u16 = 14;
pub const IFLA_CAN_BITRATE_MAX: u16 = 15;
pub const IFLA_CAN_TDC: u16 = 16;
pub const IFLA_CAN_CTRLMODE_EXT: u16 = 17;

/// CAN netlink interface
///
#[neli_enum(serialized_type = "libc::c_ushort")]
pub enum IflaCan {
    Unspec = IFLA_CAN_UNSPEC,
    BitTiming = IFLA_CAN_BITTIMING,
    BitTimingConst = IFLA_CAN_BITTIMING_CONST,
    Clock = IFLA_CAN_CLOCK,
    State = IFLA_CAN_STATE,
    CtrlMode = IFLA_CAN_CTRLMODE,
    RestartMs = IFLA_CAN_RESTART_MS,
    Restart = IFLA_CAN_RESTART,
    BerrCounter = IFLA_CAN_BERR_COUNTER,
    DataBitTiming = IFLA_CAN_DATA_BITTIMING,
    DataBitTimingConst = IFLA_CAN_DATA_BITTIMING_CONST,
    Termination = IFLA_CAN_TERMINATION,
    TerminationConst = IFLA_CAN_TERMINATION_CONST,
    BitRateConst = IFLA_CAN_BITRATE_CONST,
    DataBitRateConst = IFLA_CAN_DATA_BITRATE_CONST,
    BitRateMax = IFLA_CAN_BITRATE_MAX,
    Tdc = IFLA_CAN_TDC,
    CtrlModeExt = IFLA_CAN_CTRLMODE_EXT,
}

impl RtaType for IflaCan {}
