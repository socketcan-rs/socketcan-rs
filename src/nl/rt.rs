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

use libc::{c_char, c_uint};
use neli::{FromBytes, Size, ToBytes};
use std::io;

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
#[derive(Debug, Default, Clone, Copy, FromBytes, ToBytes)]
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

impl Size for can_bittiming {
    fn unpadded_size(&self) -> usize {
        std::mem::size_of::<can_bittiming>()
    }
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

/// CAN clock parameters
///
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, FromBytes, ToBytes)]
pub struct can_clock {
    pub freq: u32, // CAN system clock frequency in Hz
}

impl Size for can_clock {
    fn unpadded_size(&self) -> usize {
        std::mem::size_of::<can_clock>()
    }
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
#[derive(Debug, Default, Copy, Clone)]
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
#[derive(Debug, Default, Copy, Clone)]
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
#[derive(Debug, Default, Copy, Clone)]
pub struct can_device_stats {
    pub bus_error: u32,        // Bus errors
    pub error_warning: u32,    // Changes to error warning state
    pub error_passive: u32,    // Changes to error passive state
    pub bus_off: u32,          // Changes to bus off state
    pub arbitration_lost: u32, // Arbitration lost errors
    pub restarts: u32,         // CAN controller re-starts
}

pub use neli::consts::rtnl::IflaCan;

/*
/// Currently missing from libc, from linux/can/netlink.h:
///
/// CAN netlink interface
///
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IflaCan {
    Unspec = 0,
    BitTiming = 1,
    BitTimingConst = 2,
    Clock = 3,
    State = 4,
    CtrlMode = 5,
    RestartMs = 6,
    Restart = 7,
    BerrCounter = 8,
    DataBitTiming = 9,
    DataBitTimingConst = 10,
    Termination = 11,
    TerminationConst = 12,
    BitRateConst = 13,
    DataBitRateConst = 14,
    BitRateMax = 15,
    Tdc = 16,
    CtrlModeExt = 17,
}

impl From<u16> for IflaCan {
    fn from(val: u16) -> Self {
        use IflaCan::*;

        match val {
            1 => BitTiming,
            2 => BitTimingConst,
            3 => Clock,
            4 => State,
            5 => CtrlMode,
            6 => RestartMs,
            7 => Restart,
            8 => BerrCounter,
            9 => DataBitTiming,
            10 => DataBitTimingConst,
            11 => Termination,
            12 => TerminationConst,
            13 => BitRateConst,
            14 => DataBitRateConst,
            15 => BitRateMax,
            16 => Tdc,
            17 => CtrlModeExt,
            _ => Unspec,
        }
    }
}
*/
