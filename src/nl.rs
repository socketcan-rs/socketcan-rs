// socketcan/src/nl.rs
//
// Netlink access to the SocketCAN interfaces.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Netlink module
//!
//! The netlink module contains the netlink-based management capabilities of
//! the socketcan crate. Quoth wikipedia:
//!
//!
//! > Netlink socket family is a Linux kernel interface used for inter-process
//! > communication (IPC) between both the kernel and userspace processes, and
//! > between different userspace processes, in a way similar to the Unix
//! > domain sockets.
//!

use neli::{
    consts::{
        nl::{NlType, NlmF, NlmFFlags},
        rtnl::{Arphrd, RtAddrFamily, Rtm},
        rtnl::{Iff, IffFlags, Ifla, IflaInfo},
        socket::NlFamily,
    },
    err::NlError,
    nl::{NlPayload, Nlmsghdr},
    rtnl::{Ifinfomsg, Rtattr},
    socket::NlSocketHandle,
    types::{Buffer, RtBuffer},
    ToBytes,
};
use nix::{self, net::if_::if_nametoindex, unistd};
use std::{
    ffi::CString,
    fmt::Debug,
    os::raw::{c_int, c_uint},
};

/// A result for Netlink errors.
type NlResult<T> = Result<T, NlError>;

/// Gets a byte slice for any sized variable.
///
/// Note that this should normally be unsafe, but since we're only
/// using it internally for types sent to the kernel, it's OK.
fn as_bytes<T: Sized>(val: &T) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts::<'_, u8>(
            val as *const _ as *const u8,
            std::mem::size_of::<T>(),
        )
    }
}

/// SocketCAN CanInterface
///
/// Controlled through the kernel's Netlink interface, CAN devices can be
/// brought up or down or configured through this.
///
/// Note while that this API is designed in an RAII-fashion, it cannot really
/// make the same guarantees: It is entirely possible for another user/process
/// to modify, remove and re-add an interface while you are holding this object
/// with a reference to it.
///
/// Some actions possible on this interface require the process/user to have
/// the `CAP_NET_ADMIN` capability, like the root user does. This is
/// indicated by their documentation starting with "PRIVILEGED:".
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct CanInterface {
    if_index: c_uint,
}

/// The details of the interface which can be obtained with the
/// `CanInterface::detail()` function.
#[allow(missing_copy_implementations)]
#[derive(Debug, Default, Clone)]
pub struct InterfaceDetails {
    pub name: Option<String>,
    pub index: c_uint,
    pub is_up: bool,
    pub mtu: Option<Mtu>,
}

impl InterfaceDetails {
    /// Creates a new set of interface details with the specified `index`.
    pub fn new(index: c_uint) -> Self {
        Self {
            index,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Mtu {
    Standard = 16,
    Fd = 72,
}

// These are missing from libc and neli, adding them here as a stand-in for now.
mod rt {
    #![allow(non_camel_case_types, unused)]

    use libc::{c_char, c_uint};
    use std::io;

    pub const EXT_FILTER_VF: c_uint = 1 << 0;
    pub const EXT_FILTER_BRVLAN: c_uint = 1 << 1;
    pub const EXT_FILTER_BRVLAN_COMPRESSED: c_uint = 1 << 2;
    pub const EXT_FILTER_SKIP_STATS: c_uint = 1 << 3;
    pub const EXT_FILTER_MRP: c_uint = 1 << 4;
    pub const EXT_FILTER_CFM_CONFIG: c_uint = 1 << 5;
    pub const EXT_FILTER_CFM_STATUS: c_uint = 1 << 6;
    pub const EXT_FILTER_MST: c_uint = 1 << 7;

    ///
    /// Missing from libc, from linux/can/netlink.h:
    ///
    /// CAN bit-timing parameters
    ///
    /// For further information, please read chapter "8 BIT TIMING
    /// REQUIREMENTS" of the "Bosch CAN Specification version 2.0"
    /// at http://www.semiconductors.bosch.de/pdf/can2spec.pdf.
    ///
    #[repr(C)]
    #[derive(Debug, Default, Clone, Copy)]
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

    ///
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

    ///
    /// CAN clock parameters
    ///
    #[repr(C)]
    #[derive(Debug, Default, Clone, Copy)]
    pub struct can_clock {
        pub freq: u32, // CAN system clock frequency in Hz
    }

    ///
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

    ///
    /// CAN bus error counters
    ///
    #[repr(C)]
    #[derive(Debug, Default, Copy, Clone)]
    pub struct can_berr_counter {
        pub txerr: u16,
        pub rxerr: u16,
    }

    ///
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

    /// Currently missing from libc, from linux/can/netlink.h:
    ///
    /// CAN netlink interface
    ///
    #[repr(u16)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum IflaCan {
        Unspec,
        BitTiming,
        BitTimingConst,
        Clock,
        State,
        CtrlMode,
        RestartMs,
        Restart,
        BerrCounter,
        DataBitTiming,
        DataBitTimingConst,
        Termination,
        TerminationConst,
        BitRateConst,
        DataBitRateConst,
        BitRateMax,
        Tdc,
        CtrlModeExt,
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
}

///
/// CAN control modes
///
/// Note that these correspond to the bit _numbers_ for the control mode bits.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanCtrlMode {
    /// Loopback mode
    Loopback,
    /// Listen-only mode
    ListenOnly,
    /// Triple sampling mode
    TripleSampling,
    /// One-Shot mode
    OneShot,
    /// Bus-error reporting
    BerrReporting,
    /// CAN FD mode
    Fd,
    /// Ignore missing CAN ACKs
    PresumeAck,
    /// CAN FD in non-ISO mode
    NonIso,
    /// Classic CAN DLC option
    CcLen8Dlc,
}

impl CanCtrlMode {
    /// Get the mask for the specific control mode
    pub fn mask(&self) -> u32 {
        1u32 << (*self as u32)
    }
}

impl CanInterface {
    /// Open a CAN interface by name.
    ///
    /// Similar to `open_if`, but looks up the device by name instead
    pub fn open(ifname: &str) -> Result<Self, nix::Error> {
        let if_index = if_nametoindex(ifname)?;
        Ok(Self::open_iface(if_index))
    }

    /// Open a CAN interface.
    ///
    /// Creates a new `CanInterface` instance. No actual "opening" is necessary
    /// or performed when calling this function.
    pub fn open_iface(if_index: u32) -> Self {
        Self {
            if_index: if_index as c_uint,
        }
    }

    /// Sends an info message to the kernel.
    fn send_info_msg(msg_type: Rtm, info: Ifinfomsg, additional_flags: &[NlmF]) -> NlResult<()> {
        let mut nl = Self::open_route_socket()?;

        // prepare message
        let hdr = Nlmsghdr::new(
            None,
            msg_type,
            {
                let mut flags = NlmFFlags::new(&[NlmF::Request, NlmF::Ack]);
                for flag in additional_flags {
                    flags.set(flag);
                }
                flags
            },
            None,
            None,
            NlPayload::Payload(info),
        );
        // send the message
        Self::send_and_read_ack(&mut nl, hdr)
    }

    /// Sends a netlink message down a netlink socket, and checks if an ACK was
    /// properly received.
    fn send_and_read_ack<T, P>(sock: &mut NlSocketHandle, msg: Nlmsghdr<T, P>) -> NlResult<()>
    where
        T: NlType + Debug,
        P: ToBytes + Debug,
    {
        sock.send(msg)?;

        // This will actually produce an Err if the response is a netlink error, no need to match.
        if let Some(Nlmsghdr {
            nl_payload: NlPayload::Ack(_),
            ..
        }) = sock.recv()?
        {
            Ok(())
        } else {
            Err(NlError::NoAck)
        }
    }

    /// Opens a new netlink socket, bound to this process' PID.
    /// The function is generic to allow for usage in contexts where NlError has specific,
    /// non-default generic parameters.
    fn open_route_socket<T, P>() -> Result<NlSocketHandle, NlError<T, P>> {
        // retrieve PID
        let pid = unistd::getpid().as_raw() as u32;

        // open and bind socket
        // groups is set to None(0), because we want no notifications
        let sock = NlSocketHandle::connect(NlFamily::Route, Some(pid), &[])?;
        Ok(sock)
    }

    // Send a netlink CAN command down to the kernel.
    fn send_cmd(&self, cmd: rt::IflaCan, cmd_data: &[u8]) -> NlResult<()> {
        let info = Ifinfomsg::new(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            IffFlags::empty(),
            IffFlags::empty(),
            {
                let mut rtattrs = RtBuffer::new();
                let mut link_info = Rtattr::new(None, Ifla::Linkinfo, Buffer::new())?;
                link_info.add_nested_attribute(&Rtattr::new(None, IflaInfo::Kind, "can")?)?;
                let mut data = Rtattr::new(None, IflaInfo::Data, Buffer::new())?;

                data.add_nested_attribute(&Rtattr::new(None, cmd as u16, cmd_data)?)?;
                link_info.add_nested_attribute(&data)?;
                rtattrs.push(link_info);
                rtattrs
            },
        );
        Self::send_info_msg(Rtm::Newlink, info, &[])
    }

    /// Bring down this interface.
    ///
    /// Use a netlink control socket to set the interface status to "down".
    pub fn bring_down(&self) -> NlResult<()> {
        let info = Ifinfomsg::down(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            RtBuffer::new(),
        );
        Self::send_info_msg(Rtm::Newlink, info, &[])
    }

    /// Bring up this interface
    ///
    /// Brings the interface up by settings its "up" flag enabled via netlink.
    pub fn bring_up(&self) -> NlResult<()> {
        let info = Ifinfomsg::up(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            RtBuffer::new(),
        );
        Self::send_info_msg(Rtm::Newlink, info, &[])
    }

    /// Create a virtual CAN (VCAN) interface.
    ///
    /// Useful for testing applications when a physical CAN interface and
    /// bus is not available.
    ///
    /// Note that the length of the name is capped by ```libc::IFNAMSIZ```.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn create_vcan(name: &str, index: Option<u32>) -> NlResult<Self> {
        Self::create(name, index, "vcan")
    }

    /// Create an interface of the given kind.
    ///
    /// Note that the length of the name is capped by ```libc::IFNAMSIZ```.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn create<I>(name: &str, index: I, kind: &str) -> NlResult<Self>
    where
        I: Into<Option<u32>>,
    {
        if name.len() > libc::IFNAMSIZ {
            return Err(NlError::Msg("Interface name too long".into()));
        }
        let index = index.into();

        let info = Ifinfomsg::new(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            index.unwrap_or(0) as c_int,
            IffFlags::empty(),
            IffFlags::empty(),
            {
                let mut buffer = RtBuffer::new();
                buffer.push(Rtattr::new(None, Ifla::Ifname, name)?);
                let mut linkinfo = Rtattr::new(None, Ifla::Linkinfo, Vec::<u8>::new())?;
                linkinfo.add_nested_attribute(&Rtattr::new(None, IflaInfo::Kind, kind)?)?;
                buffer.push(linkinfo);
                buffer
            },
        );
        Self::send_info_msg(Rtm::Newlink, info, &[NlmF::Create, NlmF::Excl])?;

        if let Some(if_index) = index {
            Ok(Self { if_index })
        } else {
            // Unfortunately netlink does not return the the if_index assigned to the interface.
            if let Ok(if_index) = if_nametoindex(name) {
                Ok(Self { if_index })
            } else {
                Err(NlError::Msg(
                    "Interface must have been deleted between request and this if_nametoindex"
                        .into(),
                ))
            }
        }
    }

    /// Delete the interface.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn delete(self) -> Result<(), (Self, NlError)> {
        let info = Ifinfomsg::new(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            IffFlags::empty(),
            IffFlags::empty(),
            RtBuffer::new(),
        );
        match Self::send_info_msg(Rtm::Dellink, info, &[]) {
            Ok(()) => Ok(()),
            Err(err) => Err((self, err)),
        }
    }

    /// Attempt to query detailed information on the interface.
    pub fn details(&self) -> Result<InterfaceDetails, NlError<Rtm, Ifinfomsg>> {
        let info = Ifinfomsg::new(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            IffFlags::empty(),
            IffFlags::empty(),
            {
                let mut buffer = RtBuffer::new();
                buffer.push(Rtattr::new(None, Ifla::ExtMask, rt::EXT_FILTER_VF).unwrap());
                buffer
            },
        );

        let mut nl = Self::open_route_socket()?;

        let hdr = Nlmsghdr::new(
            None,
            Rtm::Getlink,
            NlmFFlags::new(&[NlmF::Request]),
            None,
            None,
            NlPayload::Payload(info),
        );
        nl.send(hdr)?;

        match nl.recv::<'_, Rtm, Ifinfomsg>()? {
            Some(msg_hdr) => {
                let mut info = InterfaceDetails::new(self.if_index);

                if let Ok(payload) = msg_hdr.get_payload() {
                    info.is_up = payload.ifi_flags.contains(&Iff::Up);

                    for attr in payload.rtattrs.iter() {
                        match attr.rta_type {
                            Ifla::Ifname => {
                                if let Ok(string) =
                                    CString::from_vec_with_nul(Vec::from(attr.rta_payload.as_ref()))
                                {
                                    if let Ok(string) = string.into_string() {
                                        info.name = Some(string);
                                    }
                                }
                            }
                            Ifla::Mtu => {
                                if attr.rta_payload.len() == 4 {
                                    let mut bytes = [0u8; 4];
                                    for (index, byte) in
                                        attr.rta_payload.as_ref().iter().enumerate()
                                    {
                                        bytes[index] = *byte;
                                    }

                                    const STANDARD: u32 = Mtu::Standard as u32;
                                    const FD: u32 = Mtu::Fd as u32;

                                    info.mtu = match u32::from_ne_bytes(bytes) {
                                        STANDARD => Some(Mtu::Standard),
                                        FD => Some(Mtu::Fd),
                                        _ => None,
                                    }
                                }
                            }
                            _ => (),
                        }
                    }
                }

                Ok(info)
            }
            None => Err(NlError::NoAck),
        }
    }

    /// Set the MTU of this interface.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_mtu(&self, mtu: Mtu) -> NlResult<()> {
        let info = Ifinfomsg::new(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            IffFlags::empty(),
            IffFlags::empty(),
            {
                let mut buffer = RtBuffer::new();
                buffer.push(Rtattr::new(
                    None,
                    Ifla::Mtu,
                    &u32::to_ne_bytes(mtu as u32)[..],
                )?);
                buffer
            },
        );
        Self::send_info_msg(Rtm::Newlink, info, &[])
    }

    /// Set the bitrate and, optionally, sample point of this interface.
    ///
    /// The bitrate can *not* be changed if the interface is UP. It is
    /// specified in Hz (bps) while the sample point is given in tenths
    /// of a percent/
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_bitrate<P>(&self, bitrate: u32, sample_point: P) -> NlResult<()>
    where
        P: Into<Option<u32>>,
    {
        let sample_point: u32 = sample_point.into().unwrap_or(0);

        debug_assert!(
            0 < bitrate && bitrate <= 1000000,
            "Bitrate must be within 1..=1000000, received {}.",
            bitrate
        );
        debug_assert!(
            sample_point < 1000,
            "Sample point must be within 0..1000, received {}.",
            sample_point
        );

        let timing = rt::can_bittiming {
            bitrate,
            sample_point,
            ..rt::can_bittiming::default()
        };

        self.send_cmd(rt::IflaCan::BitTiming, as_bytes(&timing))
    }

    /// Set the data bitrate and, optionally, data sample point of this
    /// interface.
    ///
    /// This only applies to interfaces in FD mode.
    ///
    /// The data bitrate can *not* be changed if the interface is UP. It is
    /// specified in Hz (bps) while the sample point is given in tenths
    /// of a percent/
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_data_bitrate<P>(&self, bitrate: u32, sample_point: P) -> NlResult<()>
    where
        P: Into<Option<u32>>,
    {
        let sample_point: u32 = sample_point.into().unwrap_or(0);

        let timing = rt::can_bittiming {
            bitrate,
            sample_point,
            ..rt::can_bittiming::default()
        };

        self.send_cmd(rt::IflaCan::DataBitTiming, as_bytes(&timing))
    }

    /// Set the full control mode (bit) collection.
    pub fn set_full_ctrlmode(&self, ctrlmode: rt::can_ctrlmode) -> NlResult<()> {
        self.send_cmd(rt::IflaCan::CtrlMode, as_bytes(&ctrlmode))
    }

    /// Set or clear an individual control mode parameter.
    pub fn set_ctrlmode(&self, mode: CanCtrlMode, on: bool) -> NlResult<()> {
        let mask = mode.mask();
        let flags = if on { mask } else { 0 };
        self.set_full_ctrlmode(rt::can_ctrlmode { mask, flags })
    }

    /// Set the automatic restart milliseconds of the interface
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_restart_ms(&self, restart_ms: u32) -> NlResult<()> {
        self.send_cmd(rt::IflaCan::RestartMs, &restart_ms.to_ne_bytes())
    }

    /// Manually restart the interface.
    ///
    /// Note that a manual restart if only permitted if automatic restart is
    /// disabled and the device is in the bus-off state.
    /// See: linux/drivers/net/can/dev/dev.c
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    /// Common Errors:
    ///     EINVAL - The interface is down or automatic restarts are enabled
    ///     EBUSY - The interface is not in a bus-off state
    ///
    pub fn restart(&self) -> NlResult<()> {
        // Note: The linux code shows the data type to be u32, but never
        // appears to access the value sent. iproute2 sends a 1, so we do
        // too!
        // See: linux/drivers/net/can/dev/netlink.c
        let restart_data: u32 = 1;
        self.send_cmd(rt::IflaCan::Restart, &restart_data.to_ne_bytes())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_as_bytes() {
        let bitrate = 500000;
        let sample_point = 750;
        let timing = rt::can_bittiming {
            bitrate,
            sample_point,
            ..rt::can_bittiming::default()
        };

        assert_eq!(
            unsafe {
                std::slice::from_raw_parts::<'_, u8>(
                    &timing as *const _ as *const u8,
                    std::mem::size_of::<rt::can_bittiming>(),
                )
            },
            as_bytes(&timing)
        );
    }
}

#[cfg(test)]
#[cfg(feature = "netlink_tests")]
pub mod tests {
    use std::ops::Deref;

    use serial_test::serial;

    use super::*;

    /// RAII-style helper to create and clean-up a specific vcan interface for a single test.
    /// Using drop here ensures that the interface always gets cleaned up
    /// (although a restart would also remove it).
    ///
    /// Intended for use (ONLY) in tests as follows:
    /// ```
    /// #[test]
    /// fn my_test() {
    ///     let interface = TemporaryInterface::new("my_test").unwrap();
    ///     // use the interface..
    /// }
    /// ```
    /// Please note that there is a limit to the length of interface names,
    /// namely 16 characters on Linux.
    pub struct TemporaryInterface {
        interface: CanInterface,
    }

    impl TemporaryInterface {
        #[allow(unused)]
        pub fn new(name: &str) -> NlResult<Self> {
            Ok(Self {
                interface: CanInterface::create_vcan(name, None)?,
            })
        }
    }

    impl Drop for TemporaryInterface {
        fn drop(&mut self) {
            assert!(CanInterface::open_iface(self.interface.if_index)
                .delete()
                .is_ok());
        }
    }

    impl Deref for TemporaryInterface {
        type Target = CanInterface;

        fn deref(&self) -> &Self::Target {
            &self.interface
        }
    }

    #[cfg(feature = "netlink_tests")]
    #[test]
    #[serial]
    fn up_down() {
        let interface = TemporaryInterface::new("up_down").unwrap();

        assert!(interface.bring_up().is_ok());
        assert!(interface.details().unwrap().is_up);

        assert!(interface.bring_down().is_ok());
        assert!(!interface.details().unwrap().is_up);
    }

    #[cfg(feature = "netlink_tests")]
    #[test]
    #[serial]
    fn details() {
        let interface = TemporaryInterface::new("info").unwrap();
        let details = interface.details().unwrap();
        assert_eq!("info", details.name.unwrap());
        assert!(details.mtu.is_some());
        assert!(!details.is_up);
    }

    #[cfg(feature = "netlink_tests")]
    #[test]
    #[serial]
    fn mtu() {
        let interface = TemporaryInterface::new("mtu").unwrap();

        assert!(interface.set_mtu(Mtu::Fd).is_ok());
        assert_eq!(Mtu::Fd, interface.details().unwrap().mtu.unwrap());

        assert!(interface.set_mtu(Mtu::Standard).is_ok());
        assert_eq!(Mtu::Standard, interface.details().unwrap().mtu.unwrap());
    }
}
