// socketcan/src/timestamp.rs
//
// Timestamp types and helpers for SocketCAN sockets.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Timestamp support for SocketCAN sockets.
//!
//! Timestamps are delivered atomically with the frame data via `recvmsg()`
//! and ancillary control messages, avoiding the two-syscall race of the old
//! `SIOCGSTAMPNS` approach.
//!
//! # Usage
//!
//! 1. Enable the desired timestamp mode on the socket with
//!    [`SocketOptions::set_recv_timestamp`] or [`SocketOptions::set_timestamping`].
//! 2. Call the corresponding read method on the socket.
//!
//! [`SocketOptions::set_recv_timestamp`]: crate::SocketOptions::set_recv_timestamp
//! [`SocketOptions::set_timestamping`]: crate::SocketOptions::set_timestamping

use std::time::{Duration, SystemTime};

// ===== SOF_TIMESTAMPING_* flags (not yet in libc) =====

/// Software transmit timestamp, generated just before driver queues the packet.
pub const SOF_TIMESTAMPING_TX_HARDWARE: u32 = 1 << 0;
/// Software transmit timestamp, generated when the packet leaves the network stack.
pub const SOF_TIMESTAMPING_TX_SOFTWARE: u32 = 1 << 1;
/// Hardware receive timestamp.
pub const SOF_TIMESTAMPING_RX_HARDWARE: u32 = 1 << 2;
/// Software receive timestamp, generated when the packet enters the network stack.
pub const SOF_TIMESTAMPING_RX_SOFTWARE: u32 = 1 << 3;
/// Alias for [`SOF_TIMESTAMPING_RX_SOFTWARE`]; kept for source compatibility.
pub const SOF_TIMESTAMPING_SOFTWARE: u32 = 1 << 4;
/// Report the raw hardware clock value (not wall-clock time).
pub const SOF_TIMESTAMPING_RAW_HARDWARE: u32 = 1 << 6;
/// Deliver `SO_TIMESTAMPING` timestamps via a control message on receive.
///
/// Required for RX timestamps to actually appear in the ancillary data
/// returned by `recvmsg()`.
pub const SOF_TIMESTAMPING_OPT_CMSG: u32 = 1 << 10;

// ===== ethtool constants / structs (not in libc) =====

pub(crate) const ETHTOOL_GET_TS_INFO: u32 = 0x0000_0041;

/// Mirror of `ethtool_ts_info` from `<linux/ethtool.h>`.
#[repr(C)]
pub(crate) struct EthtoolTsInfo {
    pub cmd: u32,
    pub so_timestamping: u32,
    pub phc_index: i32,
    pub tx_types: u32,
    pub tx_reserved: [u32; 3],
    pub rx_filters: u32,
    pub rx_reserved: [u32; 3],
}

// ===== Public timestamp types =====

/// Timestamps associated with a received CAN frame.
///
/// Each field is `None` when the corresponding timestamp mode was not enabled
/// on the socket before the frame was read.
///
/// Enable socket-layer timestamps with [`SocketOptions::set_recv_timestamp`]
/// and network-stack / hardware timestamps with [`SocketOptions::set_timestamping`].
///
/// [`SocketOptions::set_recv_timestamp`]: crate::SocketOptions::set_recv_timestamp
/// [`SocketOptions::set_timestamping`]: crate::SocketOptions::set_timestamping
#[derive(Debug, Clone, Copy, Default)]
pub struct CanTimestamps {
    /// `SO_TIMESTAMPNS` — socket-layer arrival time (wall clock).
    pub socket: Option<SystemTime>,
    /// `SOF_TIMESTAMPING_RX_SOFTWARE` — network-stack entry time (wall clock).
    pub sw: Option<SystemTime>,
    /// `SOF_TIMESTAMPING_RX_HARDWARE` — raw hardware clock value.
    ///
    /// Not a wall-clock time; reported as nanoseconds in the adapter's own
    /// clock domain.
    pub hw: Option<Duration>,
}

// ===== Private conversion helpers =====

/// Convert a `libc::timespec` to a `SystemTime`.
///
/// Assumes the timespec is a non-negative UNIX timestamp, which is always
/// the case for kernel-generated socket timestamps.
#[inline]
pub(crate) fn timespec_to_system_time(ts: libc::timespec) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Convert a `libc::timespec` to a `Duration`.
///
/// Used for hardware timestamps, which are reported in the adapter's own
/// clock domain rather than as wall-clock time.
#[inline]
pub(crate) fn timespec_to_duration(ts: libc::timespec) -> Duration {
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}
