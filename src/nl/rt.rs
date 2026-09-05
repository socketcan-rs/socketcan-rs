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
//! What is left here is what upstream cannot supply:
//!
//! * The CAN netlink structs. `libc` has these — `libc::can_bittiming` and
//!   friends — but they are foreign types, and the `neli` traits that
//!   serialize them (`FromBytes`, `ToBytes`, `Size`) are foreign traits, so
//!   the orphan rule forbids implementing the latter for the former. They
//!   stay defined here so the derives can be applied.
//! * [`IflaCan`], the `IFLA_CAN_*` attribute type, which `neli` does not
//!   define. Its variant values come from `libc`.
//!
//! Constants that `libc` does provide are used from there rather than
//! duplicated: `CAN_STATE_*` and `CAN_CTRLMODE_*` (`linux/can/netlink.h`),
//! and `RTEXT_FILTER_*` (`linux/rtnetlink.h`).
//!

#![allow(non_camel_case_types, unused)]

use crate::{as_bytes, as_bytes_mut};
use libc::{c_char, c_uint};
use neli::{
    FromBytes, Size, ToBytes,
    consts::rtnl::{RtaType, RtaTypeWrapper},
    err::{DeError, SerError},
    impl_trait, neli_enum,
};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Cursor, Read, Write},
    mem,
    mem::size_of,
};

/// CAN bit-timing parameters
///
/// For further information, please read chapter "8 BIT TIMING
/// REQUIREMENTS" of the "Bosch CAN Specification version 2.0"
/// at <http://www.semiconductors.bosch.de/pdf/can2spec.pdf>.
///
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, FromBytes, ToBytes, Size)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
        // SAFETY: `can_bittiming_const` is a `#[repr(C)]` struct of eight
        // `u32` fields with no padding, so every byte is initialised.
        buf.write_all(unsafe { as_bytes(self) })?;
        Ok(())
    }
}

impl FromBytes for can_bittiming_const {
    fn from_bytes(buf: &mut Cursor<impl AsRef<[u8]>>) -> Result<Self, DeError> {
        let mut timing_const: can_bittiming_const = unsafe { mem::zeroed() };
        // SAFETY: `timing_const` is fully zero-initialised on the line above.
        buf.read_exact(unsafe { as_bytes_mut(&mut timing_const) })?;
        Ok(timing_const)
    }
}

impl Size for can_bittiming_const {
    fn unpadded_size(&self) -> usize {
        size_of::<can_bittiming_const>()
    }
}

/// CAN clock parameters
///
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, FromBytes, ToBytes, Size)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct can_clock {
    pub freq: u32, // CAN system clock frequency in Hz
}

/// CAN bus error counters
///
#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes, ToBytes, Size)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct can_ctrlmode {
    pub mask: u32,
    pub flags: u32,
}

/// u16 termination range: 1..65535 Ohms
/// Netlink marks a nested attribute by setting this bit in its type field
/// (`NLA_F_NESTED` from `linux/netlink.h`, which neither `libc` nor `neli`
/// exposes). It must be masked off before the type is matched.
pub const NLA_F_NESTED: u16 = 0x8000;

pub const CAN_TERMINATION_DISABLED: u16 = 0;

///
/// CAN device statistics
///
#[repr(C)]
#[derive(Debug, Default, Copy, Clone, FromBytes)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct can_device_stats {
    pub bus_error: u32,        // Bus errors
    pub error_warning: u32,    // Changes to error warning state
    pub error_passive: u32,    // Changes to error passive state
    pub bus_off: u32,          // Changes to bus off state
    pub arbitration_lost: u32, // Arbitration lost errors
    pub restarts: u32,         // CAN controller re-starts
}

/// CAN netlink interface
///
/// The attributes nested inside `IFLA_CAN_CTRLMODE_EXT`.
#[neli_enum(serialized_type = "libc::c_ushort")]
pub enum IflaCanCtrlModeExt {
    /// The unspecified attribute, zero. Never sent or matched.
    Unspec = libc::IFLA_CAN_CTRLMODE_UNSPEC as u16,
    /// The control modes the driver supports, a `u32` mask of `CAN_CTRLMODE_*`
    /// bits (read-only). Parsed into
    /// `InterfaceCanParams::ctrl_mode_supported`.
    Supported = libc::IFLA_CAN_CTRLMODE_SUPPORTED as u16,
}

impl RtaType for IflaCanCtrlModeExt {}

#[neli_enum(serialized_type = "libc::c_ushort")]
pub enum IflaCan {
    /// The unspecified attribute, zero. Never sent or matched.
    Unspec = libc::IFLA_CAN_UNSPEC as u16,
    /// Nominal bit timing, a [`can_bittiming`].
    BitTiming = libc::IFLA_CAN_BITTIMING as u16,
    /// The controller's nominal bit-timing limits, a
    /// [`can_bittiming_const`] (read-only).
    BitTimingConst = libc::IFLA_CAN_BITTIMING_CONST as u16,
    /// The controller's base clock frequency, a [`can_clock`] (read-only).
    Clock = libc::IFLA_CAN_CLOCK as u16,
    /// The controller's state, a `u32` from the kernel's `enum can_state`.
    State = libc::IFLA_CAN_STATE as u16,
    /// The control modes currently enabled, a [`can_ctrlmode`] of a mask and
    /// the flags to apply within it.
    CtrlMode = libc::IFLA_CAN_CTRLMODE as u16,
    /// The automatic bus-off restart delay in milliseconds, a `u32`; zero
    /// disables it.
    RestartMs = libc::IFLA_CAN_RESTART_MS as u16,
    /// Triggers a manual bus-off restart. Write-only, and the kernel ignores
    /// the payload value; `CanInterface::restart()` sends a `u32`, as
    /// iproute2 does. Only permitted while automatic restart is disabled and
    /// the controller is bus-off.
    Restart = libc::IFLA_CAN_RESTART as u16,
    /// The current TX and RX error counters, a [`can_berr_counter`]
    /// (read-only).
    BerrCounter = libc::IFLA_CAN_BERR_COUNTER as u16,
    /// CAN FD data-phase bit timing, a [`can_bittiming`].
    DataBitTiming = libc::IFLA_CAN_DATA_BITTIMING as u16,
    /// The controller's data-phase bit-timing limits, a
    /// [`can_bittiming_const`] (read-only).
    DataBitTimingConst = libc::IFLA_CAN_DATA_BITTIMING_CONST as u16,
    /// The bus termination resistance in ohms, a `u16`; zero is
    /// [`CAN_TERMINATION_DISABLED`].
    Termination = libc::IFLA_CAN_TERMINATION as u16,
    /// The termination values the hardware can switch to, a list of `u16`
    /// (read-only).
    ///
    /// **Not parsed**: no accessor reads it, so a reply carrying it falls
    /// through. Reachable through `CanInterface::can_param_bytes()`.
    TerminationConst = libc::IFLA_CAN_TERMINATION_CONST as u16,
    /// The fixed nominal bitrates the hardware supports, a list of `u32`
    /// (read-only). Reported by controllers that cannot derive arbitrary
    /// rates from a bit-timing calculation.
    ///
    /// **Not parsed**, as with [`TerminationConst`](Self::TerminationConst).
    BitRateConst = libc::IFLA_CAN_BITRATE_CONST as u16,
    /// The fixed data-phase bitrates the hardware supports, a list of `u32`
    /// (read-only).
    ///
    /// **Not parsed**, as with [`TerminationConst`](Self::TerminationConst).
    DataBitRateConst = libc::IFLA_CAN_DATA_BITRATE_CONST as u16,
    /// The highest bitrate the hardware accepts, a `u32` (read-only).
    ///
    /// **Not parsed**, as with [`TerminationConst`](Self::TerminationConst).
    BitRateMax = libc::IFLA_CAN_BITRATE_MAX as u16,
    /// Transmitter Delay Compensation, a nested attribute carrying the
    /// TDCV/TDCO/TDCF values and their driver-reported bounds.
    ///
    /// Declared so the type mirrors the kernel's attribute list, but **not
    /// parsed**: nothing reads or writes it, and a reply carrying it falls
    /// through the match in `InterfaceCanParams::from_link_info()`. Only
    /// drivers that set `tdc_const` report it — a PEAK PCAN-USB FD, for one,
    /// does not — so there was no hardware here to verify an implementation
    /// against. The nested-attribute plumbing added for
    /// [`IflaCanCtrlModeExt`] is what it would build on.
    Tdc = libc::IFLA_CAN_TDC as u16,
    /// The supported control modes, nested. Parsed into
    /// `InterfaceCanParams::ctrl_mode_supported`.
    CtrlModeExt = libc::IFLA_CAN_CTRLMODE_EXT as u16,
}

impl RtaType for IflaCan {}

/////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    /// Our mirrors of the `linux/can/netlink.h` structs must match the ones
    /// `libc` now defines, byte for byte.
    ///
    /// They cannot simply *be* libc's: the `neli` traits that serialize them
    /// are foreign traits, and libc's types are foreign types, so the orphan
    /// rule forbids the impls. Keeping a copy is only safe while it stays
    /// identical, which is what this checks — size, alignment and the offset
    /// of every field.
    #[test]
    fn structs_match_libc() {
        macro_rules! same_layout {
            ($ours:ty, $theirs:ty, [$($field:ident),+ $(,)?]) => {{
                assert_eq!(
                    size_of::<$ours>(),
                    size_of::<$theirs>(),
                    concat!(stringify!($ours), ": size")
                );
                assert_eq!(
                    align_of::<$ours>(),
                    align_of::<$theirs>(),
                    concat!(stringify!($ours), ": alignment")
                );
                $(
                    assert_eq!(
                        offset_of!($ours, $field),
                        offset_of!($theirs, $field),
                        concat!(stringify!($ours), ".", stringify!($field), ": offset")
                    );
                )+
            }};
        }

        same_layout!(
            can_bittiming,
            libc::can_bittiming,
            [
                bitrate,
                sample_point,
                tq,
                prop_seg,
                phase_seg1,
                phase_seg2,
                sjw,
                brp
            ]
        );
        same_layout!(
            can_bittiming_const,
            libc::can_bittiming_const,
            [
                name, tseg1_min, tseg1_max, tseg2_min, tseg2_max, sjw_max, brp_min, brp_max,
                brp_inc
            ]
        );
        same_layout!(can_clock, libc::can_clock, [freq]);
        same_layout!(can_berr_counter, libc::can_berr_counter, [txerr, rxerr]);
        same_layout!(can_ctrlmode, libc::can_ctrlmode, [mask, flags]);
    }

    #[test]
    fn test_as_bytes() {
        let bitrate = 500000;
        let sample_point = 750;
        let timing = can_bittiming {
            bitrate,
            sample_point,
            ..can_bittiming::default()
        };

        assert_eq!(
            unsafe {
                std::slice::from_raw_parts::<'_, u8>(
                    &timing as *const _ as *const u8,
                    size_of::<can_bittiming>(),
                )
            },
            // SAFETY: `can_bittiming` is a `#[repr(C)]` struct of `u32`
            // fields fully initialised by the literal above.
            unsafe { as_bytes(&timing) }
        );
    }
}
