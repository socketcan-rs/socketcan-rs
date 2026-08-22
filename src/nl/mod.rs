// socketcan/src/nl/mod.rs
//
// Netlink access to the SocketCAN interfaces.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! CAN Netlink access
//!
//! The netlink module contains the netlink-based management capabilities of
//! the socketcan crate.
//!
//! For SocketCAN, netlink is the primary way for a user-space application to
//! query or set the parameters of a CAN interface, such as the bitrate, the
//! control mode bits, and so forth. It also allows the application to get
//! statistics from the interface and send commands to it, including
//! performing a bus restart.
//!
//1 Netlink is a socket-based mechanism, similar to Unix-domain sockets, which
//! allows a user-space program communicate with the kernel.
//!
//! Unfortunately, the SocketCAN netlink API does not appear to be documented
//! _anywhere_. The netlink functional summary on the SocketCAN page is here:
//!
//! <https://www.kernel.org/doc/html/latest/networking/can.html#netlink-interface-to-set-get-devices-properties>
//!
//! The CAN netlink header file for the Linux kernel has the definition of
//! the constants and data structures that are sent back and forth to the
//! kernel over netlink. It can be found in the Linux sources here:
//!
//! <https://github.com/torvalds/linux/blob/master/include/uapi/linux/can/netlink.h?ts=4>
//!
//! The corresponding kernel code that receives and processes messages from
//! userspace is useful to help figure out what the kernel expects. It's here:
//!
//! <https://github.com/torvalds/linux/blob/master/drivers/net/can/dev/netlink.c?ts=4>
//! <https://github.com/torvalds/linux/blob/master/drivers/net/can/dev/dev.c?ts=4>
//!
//! The main Linux user-space client to communicate with network interfaces,
//! including CAN is _iproute2_. The CAN-specific code for it is here:
//!
//! <https://github.com/iproute2/iproute2/blob/main/ip/iplink_can.c?ts=4>
//!
//! There is also a C user-space library for SocketCAN, which primarily
//! deals with the Netlink interface. There are several forks, but one of
//! the later ones with updated documents is here:
//!
//! <https://github.com/lalten/libsocketcan>
//!

use crate::Result;
use neli::{
    FromBytes, FromBytesWithInput, Size, ToBytes,
    attr::Attribute,
    consts::{
        nl::{NlType, NlmF},
        rtnl::{Arphrd, Iff, Ifla, IflaInfo, RtAddrFamily, Rtm},
        socket::NlFamily,
    },
    err::RouterError,
    nl::{NlPayload, Nlmsghdr, NlmsghdrBuilder},
    rtnl::{Ifinfomsg, IfinfomsgBuilder, Rtattr, RtattrBuilder},
    socket::synchronous::NlSocketHandle,
    types::{Buffer, RtBuffer},
    utils::Groups,
};
use nix::{self, net::if_::if_nametoindex};
use rt::IflaCan;
use std::{ffi::CStr, fmt::Debug, io, os::raw::c_uint};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Low-level Netlink CAN struct bindings.
mod rt;

pub use rt::CanState;
use rt::can_ctrlmode;

/// A failure reported by the netlink protocol layer.
///
/// This crate's netlink calls go through `neli`, whose [`RouterError`] is
/// generic over the message type and payload and holds whole netlink messages
/// — it is 128 bytes wide. This is the owned, non-generic summary of it that
/// the crate-level [`Error`](crate::Error) carries in its `Nl` variant: it
/// keeps what a caller can act on, the kernel's errno above all, without
/// putting `neli` types in this crate's public API or growing every
/// `Result<_, Error>` to the size of a `RouterError`.
///
/// Genuine I/O failures are *not* here: those arrive as
/// [`Error::Io`](crate::Error::Io) with their original kind intact.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum NlError {
    /// The kernel rejected the request, returning this errno.
    ///
    /// Held positive, the way [`io::Error::from_raw_os_error()`] expects,
    /// although netlink sends it negated on the wire.
    #[error("netlink error: {}", io::Error::from_raw_os_error(*errno))]
    Netlink {
        /// The errno the kernel reported
        errno: i32,
    },
    /// No ACK arrived for a request that asked for one.
    #[error("no netlink ack received")]
    NoAck,
    /// An ACK arrived for a request that did not ask for one.
    #[error("unexpected netlink ack received")]
    UnexpectedAck,
    /// A reply carried a sequence number or port ID that was not the one
    /// requested.
    #[error("netlink reply with bad sequence number or port id (seq {seq}, pid {pid})")]
    BadSeqOrPid {
        /// The sequence number of the offending reply
        seq: u32,
        /// The port ID of the offending reply
        pid: u32,
    },
    /// The channel carrying netlink messages closed.
    #[error("netlink channel closed")]
    ClosedChannel,
    /// A message-level failure — serialization, deserialization, or an
    /// arbitrary `neli` message — reduced to its text.
    #[error("netlink: {0}")]
    Msg(String),
}

impl NlError {
    /// The errno the kernel reported, if this was an error packet.
    pub fn errno(&self) -> Option<i32> {
        match *self {
            Self::Netlink { errno } => Some(errno),
            _ => None,
        }
    }

    /// The kernel's errno as an [`io::ErrorKind`], if there was one.
    ///
    /// Lets a caller test a netlink rejection the same way as any other
    /// system error: a privileged operation attempted as a normal user
    /// gives `Some(io::ErrorKind::PermissionDenied)`.
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        self.errno().map(|e| io::Error::from_raw_os_error(e).kind())
    }
}

impl<T, P> From<RouterError<T, P>> for NlError
where
    T: NlType,
    P: Debug,
{
    /// Converts a `neli` router error into this owned, summarized form.
    /// The crate-level [`Error`](crate::Error) then carries this in its `Nl`
    /// variant.
    ///
    /// What a caller can act on is kept rather than flattened into a message:
    /// an error packet from the kernel keeps its errno, and each protocol-level
    /// condition gets its own variant. Only the message-level failures —
    /// serialization, deserialization, an arbitrary `neli` message — are
    /// reduced to text, having no structure worth keeping.
    ///
    /// An I/O failure has no home here. The crate-level `From<RouterError>`
    /// for [`Error`](crate::Error) takes those first and keeps them as
    /// [`Error::Io`](crate::Error::Io) with their kind and errno intact,
    /// so this conversion only ever sees the rest.
    fn from(e: RouterError<T, P>) -> Self {
        use RouterError::*;
        match e {
            // An error packet from the kernel. Netlink negates the errno.
            Nlmsgerr(err) => Self::Netlink {
                errno: -*err.error(),
            },
            NoAck => Self::NoAck,
            UnexpectedAck => Self::UnexpectedAck,
            ClosedChannel => Self::ClosedChannel,
            BadSeqOrPid(msg) => Self::BadSeqOrPid {
                seq: *msg.nl_seq(),
                pid: *msg.nl_pid(),
            },
            // Serialization, deserialization and arbitrary messages, plus any
            // variant a later `neli` adds.
            other => Self::Msg(other.to_string()),
        }
    }
}

// --------------------------------------------------------------------------

/// CAN bit-timing parameters
pub type CanBitTiming = rt::can_bittiming;
/// CAN bit-timing const parameters
pub type CanBitTimingConst = rt::can_bittiming_const;
/// CAN clock parameter
pub type CanClock = rt::can_clock;
/// CAN bus error counters
pub type CanBerrCounter = rt::can_berr_counter;

/// The details of the interface which can be obtained with the
/// `CanInterface::details()` function.
#[allow(missing_copy_implementations)]
#[derive(Debug, Default, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct InterfaceDetails {
    /// The name of the interface
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub name: Option<String>,
    /// The index of the interface
    pub index: c_uint,
    /// Whether the interface is currently up
    pub is_up: bool,
    /// The MTU size of the interface (Standard or FD frames support)
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub mtu: Option<Mtu>,
    /// The CAN-specific parameters for the interface
    pub can: InterfaceCanParams,
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

/// The MTU size for the interface
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Mtu {
    /// Standard CAN frame, 8-byte data (16-byte total)
    Standard = 16,
    /// FD CAN frame, 64-byte data (64-byte total)
    Fd = 72,
}

impl TryFrom<u32> for Mtu {
    type Error = io::Error;

    fn try_from(val: u32) -> std::result::Result<Self, Self::Error> {
        match val {
            16 => Ok(Mtu::Standard),
            72 => Ok(Mtu::Fd),
            _ => Err(io::Error::from(io::ErrorKind::InvalidData)),
        }
    }
}

/// The CAN-specific parameters for the interface.
#[allow(missing_copy_implementations)]
#[derive(Debug, Default, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct InterfaceCanParams {
    /// The CAN bit timing parameters
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub bit_timing: Option<CanBitTiming>,
    /// The bit timing const parameters
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub bit_timing_const: Option<CanBitTimingConst>,
    /// The CAN clock parameters (read only)
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub clock: Option<CanClock>,
    /// The CAN bus state (read-only)
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub state: Option<CanState>,
    /// The automatic restart time (in millisec)
    /// Zero means auto-restart is disabled.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub restart_ms: Option<u32>,
    /// The bit error counter (read-only)
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub berr_counter: Option<CanBerrCounter>,
    /// The control mode bits
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub ctrl_mode: Option<CanCtrlModes>,
    /// The FD data bit timing
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub data_bit_timing: Option<CanBitTiming>,
    /// The FD data bit timing const parameters
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub data_bit_timing_const: Option<CanBitTimingConst>,
    /// The CANbus termination resistance
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub termination: Option<u16>,
}

impl InterfaceCanParams {
    /// Parses the CAN parameters out of a Linkinfo attribute.
    ///
    /// Internal: this takes a neli type, which is deliberately kept out of
    /// the crate's public API.
    pub(crate) fn from_link_info(link_info: &Rtattr<Ifla, Buffer>) -> Result<Self> {
        let mut params = Self::default();

        for info in link_info.get_attr_handle::<IflaInfo>()?.get_attrs() {
            if *info.rta_type() == IflaInfo::Data {
                for attr in info.get_attr_handle::<IflaCan>()?.get_attrs() {
                    match attr.rta_type() {
                        IflaCan::BitTiming => {
                            params.bit_timing = Some(attr.get_payload_as::<CanBitTiming>()?);
                        }
                        IflaCan::BitTimingConst => {
                            params.bit_timing_const =
                                Some(attr.get_payload_as::<CanBitTimingConst>()?);
                        }
                        IflaCan::Clock => {
                            params.clock = Some(attr.get_payload_as::<CanClock>()?);
                        }
                        IflaCan::State => {
                            params.state = CanState::try_from(attr.get_payload_as::<u32>()?).ok();
                        }
                        IflaCan::CtrlMode => {
                            let ctrl_mode = attr.get_payload_as::<can_ctrlmode>()?;
                            params.ctrl_mode = Some(CanCtrlModes(ctrl_mode));
                        }
                        IflaCan::RestartMs => {
                            params.restart_ms = Some(attr.get_payload_as::<u32>()?);
                        }
                        IflaCan::BerrCounter => {
                            params.berr_counter = Some(attr.get_payload_as::<CanBerrCounter>()?);
                        }
                        IflaCan::DataBitTiming => {
                            params.data_bit_timing = Some(attr.get_payload_as::<CanBitTiming>()?);
                        }
                        IflaCan::DataBitTimingConst => {
                            params.data_bit_timing_const =
                                Some(attr.get_payload_as::<CanBitTimingConst>()?);
                        }
                        IflaCan::Termination => {
                            params.termination = Some(attr.get_payload_as::<u16>()?);
                        }
                        _ => (),
                    }
                }
            }
        }
        Ok(params)
    }

    /// Renders the CAN parameters into a netlink attribute buffer.
    ///
    /// Internal: this yields a neli type, which is deliberately kept out of
    /// the crate's public API.
    pub(crate) fn to_rtbuffer(&self) -> Result<RtBuffer<Ifla, Buffer>> {
        let mut rtattrs: RtBuffer<Ifla, Buffer> = RtBuffer::new();
        let mut data = RtattrBuilder::default()
            .rta_type(IflaInfo::Data)
            .rta_payload(Buffer::new())
            .build()?;

        if let Some(bt) = self.bit_timing {
            data = data.nest(
                &RtattrBuilder::default()
                    .rta_type(IflaCan::BitTiming)
                    .rta_payload(bt)
                    .build()?,
            )?;
        }
        if let Some(r) = self.restart_ms {
            data = data.nest(
                &RtattrBuilder::default()
                    .rta_type(IflaCan::RestartMs)
                    .rta_payload(&r.to_ne_bytes()[..])
                    .build()?,
            )?;
        }
        if let Some(cm) = self.ctrl_mode {
            data = data.nest(
                &RtattrBuilder::<_, can_ctrlmode>::default()
                    .rta_type(IflaCan::CtrlMode)
                    .rta_payload(cm.into())
                    .build()?,
            )?;
        }
        if let Some(dbt) = self.data_bit_timing {
            data = data.nest(
                &RtattrBuilder::default()
                    .rta_type(IflaCan::DataBitTiming)
                    .rta_payload(dbt)
                    .build()?,
            )?;
        }
        if let Some(t) = self.termination {
            data = data.nest(
                &RtattrBuilder::default()
                    .rta_type(IflaCan::Termination)
                    .rta_payload(t)
                    .build()?,
            )?;
        }

        let mut link_info = RtattrBuilder::default()
            .rta_type(Ifla::Linkinfo)
            .rta_payload(Buffer::new())
            .build()?;
        link_info = link_info.nest(
            &RtattrBuilder::default()
                .rta_type(IflaInfo::Kind)
                .rta_payload("can")
                .build()?,
        )?;
        link_info = link_info.nest(&data)?;

        rtattrs.push(link_info);
        Ok(rtattrs)
    }
}

// --------------------------------------------------------------------------

///
/// CAN control modes
///
/// Note that these correspond to the bit _numbers_ for the control mode bits.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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

/// The collection of control modes
#[derive(Debug, Default, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CanCtrlModes(can_ctrlmode);

impl CanCtrlModes {
    /// Create a set of CAN control modes from a mask and set of flags.
    pub fn new(mask: u32, flags: u32) -> Self {
        Self(can_ctrlmode { mask, flags })
    }

    /// Create the set of mode flags for a single mode
    pub fn from_mode(mode: CanCtrlMode, on: bool) -> Self {
        let mask = mode.mask();
        let flags = if on { mask } else { 0 };
        Self::new(mask, flags)
    }

    /// Adds a mode flag to the existing set of modes.
    pub fn add(&mut self, mode: CanCtrlMode, on: bool) {
        let mask = mode.mask();
        self.0.mask |= mask;
        if on {
            self.0.flags |= mask;
        }
    }

    /// Clears all of the mode flags in the collection
    #[inline]
    pub fn clear(&mut self) {
        self.0 = can_ctrlmode::default();
    }

    /// Test if this CanCtrlModes has a specific `mode` turned on.
    ///
    /// This inspects the `flags` field — i.e. the kernel-reported current mode
    /// state — and is intended for use on a [CanCtrlModes] obtained from
    /// [CanInterface::details]. When used on a value being built up to *set*
    /// modes, the result will only reflect bits already pushed into `flags`,
    /// not pending changes recorded in `mask`.
    ///
    /// # Examples
    ///
    /// ```
    /// use socketcan::nl::CanCtrlModes;
    /// use socketcan::CanCtrlMode;
    ///
    /// let modes = CanCtrlModes::new(0x20, 0x20); // This is bit 5 (CanCtrlMode::Fd)
    /// assert_eq!(modes.has_mode(CanCtrlMode::Fd), true);
    /// assert_eq!(modes.has_mode(CanCtrlMode::ListenOnly), false);
    /// ```
    #[inline]
    pub fn has_mode(&self, mode: CanCtrlMode) -> bool {
        (mode.mask() & self.0.flags) != 0
    }
}

impl From<can_ctrlmode> for CanCtrlModes {
    fn from(mode: can_ctrlmode) -> Self {
        Self(mode)
    }
}

impl From<CanCtrlModes> for can_ctrlmode {
    fn from(mode: CanCtrlModes) -> Self {
        mode.0
    }
}

// --------------------------------------------------------------------------

/// SocketCAN Netlink CanInterface
///
/// Controlled through the kernel's Netlink interface, CAN devices can be
/// brought up or down or configured or queried through this.
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

/// Resolves a caller's requested interface index into a real request.
///
/// Index 0 is netlink's own way of saying "unspecified" — `ifi_index = 0`
/// with `NLM_F_CREATE` asks the kernel to assign one — so `Some(0)` means
/// the same thing as `None` and must not be taken as the index of the
/// resulting interface. Reading it literally handed back a `CanInterface`
/// addressing interface 0 for the rest of its life.
fn requested_index(index: impl Into<Option<u32>>) -> Option<u32> {
    index.into().filter(|index| *index != 0)
}

impl CanInterface {
    /// Open a CAN interface by name.
    ///
    /// Similar to `open_iface`, but looks up the device by name instead of
    /// the interface index. An unknown name reports the `if_nametoindex()`
    /// errno — typically `ENODEV` — as [`Error::Io`](crate::Error::Io).
    pub fn open(ifname: &str) -> Result<Self> {
        let if_index = if_nametoindex(ifname)?;
        Ok(Self::open_iface(if_index))
    }

    /// Open a CAN interface.
    ///
    /// Creates a new `CanInterface` instance.
    ///
    /// Note that no actual "opening" or checks are performed when calling
    /// this function, nor does it test to determine if the interface with
    /// the specified index actually exists.
    pub fn open_iface(if_index: u32) -> Self {
        let if_index = if_index as c_uint;
        Self { if_index }
    }

    /// Creates an `Ifinfomsg` for this CAN interface from a buffer
    fn info_msg(&self, buf: RtBuffer<Ifla, Buffer>) -> Ifinfomsg {
        IfinfomsgBuilder::default()
            .ifi_family(RtAddrFamily::Unspecified)
            .ifi_type(Arphrd::Netrom)
            .ifi_index(self.if_index as i32)
            .rtattrs(buf)
            .build()
            .unwrap()
    }

    /// Sends an info message to the kernel.
    fn send_info_msg(msg_type: Rtm, info: Ifinfomsg, additional_flags: NlmF) -> Result<()> {
        let mut nl = Self::open_route_socket()?;

        // prepare message
        let hdr = NlmsghdrBuilder::default()
            .nl_type(msg_type)
            .nl_flags(NlmF::REQUEST | NlmF::ACK | additional_flags)
            .nl_payload(NlPayload::Payload(info))
            .build()
            .unwrap();
        // send the message
        Self::send_and_read_ack(&mut nl, &hdr)
    }

    /// Sends a message down a netlink socket, and checks if an ACK was
    /// properly received.
    fn send_and_read_ack<T, P>(sock: &mut NlSocketHandle, msg: &Nlmsghdr<T, P>) -> Result<()>
    where
        T: NlType + Debug,
        P: ToBytes + Debug + Size + FromBytesWithInput<Input = usize>,
    {
        sock.send(msg)?;

        // This will actually produce an Err if the response is a netlink error,
        // no need to match.
        if sock
            .recv::<T, P>()?
            .0
            .next()
            .transpose()?
            .is_some_and(|msg| matches!(msg.nl_payload(), NlPayload::Ack(_)))
        {
            Ok(())
        } else {
            Err(NlError::NoAck.into())
        }
    }

    /// Opens a new netlink socket with a kernel-assigned port ID.
    ///
    /// Passing `None` for the port ID lets the kernel pick a unique value,
    /// which avoids `EADDRINUSE` when multiple netlink sockets are open
    /// in the same process — for example, from concurrent calls on
    /// different threads, or when a getter is invoked while a setter is
    /// still in flight. Binding all sockets to `Pid::this()` would collide.
    fn open_route_socket() -> Result<NlSocketHandle> {
        // groups is empty because we want no multicast notifications
        let sock = NlSocketHandle::connect(NlFamily::Route, None, Groups::empty())?;
        Ok(sock)
    }

    /// Sends a query to the kernel and returns the response info message
    /// to the caller.
    fn query_details(&self) -> Result<Option<Nlmsghdr<Rtm, Ifinfomsg>>> {
        let sock = Self::open_route_socket()?;

        let info = self.info_msg({
            let mut buffer = RtBuffer::new();
            buffer.push(
                RtattrBuilder::default()
                    .rta_type(Ifla::ExtMask)
                    .rta_payload(rt::EXT_FILTER_VF)
                    .build()
                    .unwrap(),
            );
            buffer
        });

        let hdr = NlmsghdrBuilder::default()
            .nl_type(Rtm::Getlink)
            .nl_flags(NlmF::REQUEST)
            .nl_payload(NlPayload::Payload(info))
            .build()
            .unwrap();

        sock.send(&hdr)?;

        let mut iter = sock.recv::<Rtm, Ifinfomsg>()?.0;
        let Some(msg) = iter.next().transpose()? else {
            return Ok(None);
        };

        // A rejected query comes back as an NLMSG_ERROR message rather than as
        // a `RouterError`, since this request did not ask for an ACK. Its
        // payload is not an `Ifinfomsg`, so `get_payload()` would report
        // `None` and every caller would read the reply as an interface with
        // nothing to say — an unknown index looked like a real interface that
        // was merely down. Surface the errno instead.
        if let NlPayload::Err(err) = msg.nl_payload() {
            let errno = -*err.error();
            if errno != 0 {
                return Err(NlError::Netlink { errno }.into());
            }
        }

        Ok(Some(msg))
    }

    /// Bring down this interface.
    ///
    /// Use a netlink control socket to set the interface status to "down".
    pub fn bring_down(&self) -> Result<()> {
        // Specific iface down info
        let info = IfinfomsgBuilder::default()
            .down()
            .ifi_family(RtAddrFamily::Unspecified)
            .ifi_type(Arphrd::Netrom)
            .ifi_index(self.if_index as i32)
            .rtattrs(RtBuffer::new())
            .build()
            .unwrap();
        Self::send_info_msg(Rtm::Newlink, info, NlmF::empty())
    }

    /// Bring up this interface
    ///
    /// Brings the interface up by settings its "up" flag enabled via netlink.
    pub fn bring_up(&self) -> Result<()> {
        // Specific iface up info
        let info = IfinfomsgBuilder::default()
            .up()
            .ifi_family(RtAddrFamily::Unspecified)
            .ifi_type(Arphrd::Netrom)
            .ifi_index(self.if_index as i32)
            .build()
            .unwrap();
        Self::send_info_msg(Rtm::Newlink, info, NlmF::empty())
    }

    /// Create a virtual CAN (VCAN) interface.
    ///
    /// Useful for testing applications when a physical CAN interface and
    /// bus is not available.
    ///
    /// Note that the length of the name is capped by ```libc::IFNAMSIZ```.
    /// See [`create()`](Self::create) for how `index` is treated.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn create_vcan(name: &str, index: Option<u32>) -> Result<Self> {
        Self::create(name, index, "vcan")
    }

    /// Create an interface of the given kind.
    ///
    /// Note that the length of the name is capped by ```libc::IFNAMSIZ```.
    ///
    /// `index` requests a specific interface index. `None` — or `Some(0)`,
    /// which is how netlink itself spells "unspecified" — lets the kernel
    /// assign one, which is then looked up by name, since netlink does not
    /// report the index it picked.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn create<I>(name: &str, index: I, kind: &str) -> Result<Self>
    where
        I: Into<Option<u32>>,
    {
        // Remember: IFNAMSIZ includes the trailing NUL, so a name may be at
        // most IFNAMSIZ - 1 characters long.
        if name.len() >= libc::IFNAMSIZ {
            return Err(NlError::Msg("Interface name too long".into()).into());
        }
        let index = requested_index(index);

        let info = IfinfomsgBuilder::default()
            .ifi_family(RtAddrFamily::Unspecified)
            .ifi_type(Arphrd::Netrom)
            .ifi_index(index.unwrap_or(0) as i32)
            .rtattrs({
                let mut buffer = RtBuffer::new();
                buffer.push(
                    RtattrBuilder::default()
                        .rta_type(Ifla::Ifname)
                        .rta_payload(name)
                        .build()?,
                );
                let linkinfo = RtattrBuilder::default()
                    .rta_type(Ifla::Linkinfo)
                    .rta_payload(Vec::<u8>::new())
                    .build()?
                    .nest(
                        &RtattrBuilder::default()
                            .rta_type(IflaInfo::Kind)
                            .rta_payload(kind)
                            .build()?,
                    )?;
                buffer.push(linkinfo);
                buffer
            })
            .build()
            .unwrap();
        Self::send_info_msg(Rtm::Newlink, info, NlmF::CREATE | NlmF::EXCL)?;

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
                )
                .into())
            }
        }
    }

    /// Delete the interface.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn delete(self) -> std::result::Result<(), (Self, crate::Error)> {
        let info = self.info_msg(RtBuffer::new());
        match Self::send_info_msg(Rtm::Dellink, info, NlmF::empty()) {
            Ok(()) => Ok(()),
            Err(err) => Err((self, err)),
        }
    }

    /// Attempt to query detailed information on the interface.
    ///
    /// A single netlink round trip, returning the interface's name, index,
    /// up/down state and MTU together with every CAN parameter — so this is
    /// cheaper than calling two of the individual getters. See
    /// [`can_params()`](Self::can_params) for the CAN parameters alone.
    pub fn details(&self) -> Result<InterfaceDetails> {
        match self.query_details()? {
            Some(msg_hdr) => {
                let mut info = InterfaceDetails::new(self.if_index);

                if let Some(payload) = msg_hdr.get_payload() {
                    info.is_up = payload.ifi_flags().contains(Iff::UP);

                    for attr in payload.rtattrs().iter() {
                        match attr.rta_type() {
                            Ifla::Ifname => {
                                // Stops at the first NUL, so any padding the
                                // kernel left after the name is ignored.
                                info.name = CStr::from_bytes_until_nul(attr.rta_payload().as_ref())
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .ok();
                            }
                            Ifla::Mtu => {
                                info.mtu = attr
                                    .get_payload_as::<u32>()
                                    .ok()
                                    .and_then(|mtu| Mtu::try_from(mtu).ok());
                            }
                            Ifla::Linkinfo => {
                                info.can = InterfaceCanParams::from_link_info(attr)?;
                            }
                            _ => (),
                        }
                    }
                }

                Ok(info)
            }
            None => Err(NlError::NoAck.into()),
        }
    }

    /// Set the MTU of this interface.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_mtu(&self, mtu: Mtu) -> Result<()> {
        let mtu = mtu as u32;
        let info = self.info_msg({
            let mut buffer = RtBuffer::new();
            buffer.push(
                RtattrBuilder::default()
                    .rta_type(Ifla::Mtu)
                    .rta_payload(&mtu.to_ne_bytes()[..])
                    .build()?,
            );
            buffer
        });
        Self::send_info_msg(Rtm::Newlink, info, NlmF::empty())
    }

    /// Set a CAN-specific parameter.
    ///
    /// This send a netlink message down to the kernel to set an attribute
    /// in the link info, such as bitrate, control modes, etc
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_can_param<P>(&self, param_type: IflaCan, param: P) -> Result<()>
    where
        P: ToBytes + Size,
    {
        let info = self.info_msg({
            let data = RtattrBuilder::default()
                .rta_type(IflaInfo::Data)
                .rta_payload(Buffer::new())
                .build()?
                .nest(
                    &RtattrBuilder::default()
                        .rta_type(param_type)
                        .rta_payload(param)
                        .build()?,
                )?;

            let link_info = RtattrBuilder::default()
                .rta_type(Ifla::Linkinfo)
                .rta_payload(Buffer::new())
                .build()?
                .nest(
                    &RtattrBuilder::default()
                        .rta_type(IflaInfo::Kind)
                        .rta_payload("can")
                        .build()?,
                )?
                .nest(&data)?;

            let mut rtattrs = RtBuffer::new();
            rtattrs.push(link_info);
            rtattrs
        });
        Self::send_info_msg(Rtm::Newlink, info, NlmF::empty())
    }

    /// Set a CAN-specific set of parameters.
    ///
    /// This sends a netlink message down to the kernel to set multiple
    /// attributes in the link info, such as bitrate, control modes, etc.
    ///
    /// If you have many attributes to set this is preferred to calling
    /// [set_can_params][CanInterface::set_can_param] multiple times, since this only sends a
    /// single netlink message. Also some CAN drivers might only accept
    /// a set of attributes, not over multiple messages.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_can_params(&self, params: &InterfaceCanParams) -> Result<()> {
        let info = self.info_msg(params.to_rtbuffer()?);
        Self::send_info_msg(Rtm::Newlink, info, NlmF::empty())
    }

    /// Reads every CAN parameter of the interface in a single query.
    ///
    /// The individual getters — [`bit_timing()`](Self::bit_timing),
    /// [`state()`](Self::state), [`ctrlmodes()`](Self::ctrlmodes) and the
    /// rest — each open a netlink socket and exchange a message, so reading
    /// several of them costs one round trip apiece. This asks for all of
    /// them at once, which is what the kernel sends anyway: the reply to a
    /// single `RTM_GETLINK` carries the whole parameter set.
    ///
    /// A parameter the interface does not report is `None`, and an interface
    /// with no CAN link information at all — a `vcan`, for instance — yields
    /// the default, with every field `None`.
    ///
    /// [`details()`](Self::details) is the same query with the interface's
    /// name, index, flags and MTU alongside these parameters.
    pub fn can_params(&self) -> Result<InterfaceCanParams> {
        let Some(hdr) = self.query_details()? else {
            return Err(NlError::NoAck.into());
        };
        let Some(payload) = hdr.get_payload() else {
            return Ok(InterfaceCanParams::default());
        };
        for attr in payload.rtattrs().iter() {
            if *attr.rta_type() == Ifla::Linkinfo {
                return InterfaceCanParams::from_link_info(attr);
            }
        }
        Ok(InterfaceCanParams::default())
    }

    /// Attempt to query an individual CAN parameter on the interface.
    ///
    /// One netlink round trip per call; see [`can_params()`](Self::can_params)
    /// to read the whole set at once.
    pub fn can_param<P>(&self, param: IflaCan) -> Result<Option<P>>
    where
        P: FromBytes + Clone,
    {
        if let Some(hdr) = self.query_details()? {
            if let Some(payload) = hdr.get_payload() {
                for top_attr in payload.rtattrs().iter() {
                    if *top_attr.rta_type() == Ifla::Linkinfo {
                        for info in top_attr.get_attr_handle::<IflaInfo>()?.get_attrs() {
                            if *info.rta_type() == IflaInfo::Data {
                                for attr in info.get_attr_handle::<IflaCan>()?.get_attrs() {
                                    if *attr.rta_type() == param {
                                        return Ok(Some(attr.get_payload_as::<P>()?));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(None)
        } else {
            Err(NlError::NoAck.into())
        }
    }

    /// Gets the current bit rate for the interface.
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn bit_rate(&self) -> Result<Option<u32>> {
        Ok(self.bit_timing()?.map(|timing| timing.bitrate))
    }

    /// Set the bitrate and, optionally, sample point of this interface.
    ///
    /// The bitrate can *not* be changed if the interface is UP. It is
    /// specified in Hz (bps) while the sample point is given in tenths
    /// of a percent/
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_bitrate<P>(&self, bitrate: u32, sample_point: P) -> Result<()>
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

        self.set_bit_timing(CanBitTiming {
            bitrate,
            sample_point,
            ..CanBitTiming::default()
        })
    }

    /// Gets the bit timing params for the interface
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn bit_timing(&self) -> Result<Option<CanBitTiming>> {
        self.can_param::<CanBitTiming>(IflaCan::BitTiming)
    }

    /// Sets the bit timing params for the interface
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_bit_timing(&self, timing: CanBitTiming) -> Result<()> {
        self.set_can_param(IflaCan::BitTiming, timing)
    }

    /// Gets the bit timing const data for the interface
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn bit_timing_const(&self) -> Result<Option<CanBitTimingConst>> {
        self.can_param::<CanBitTimingConst>(IflaCan::BitTimingConst)
    }

    /// Gets the clock frequency for the interface
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn clock(&self) -> Result<Option<u32>> {
        Ok(self
            .can_param::<CanClock>(IflaCan::Clock)?
            .map(|clk| clk.freq))
    }

    /// Gets the state of the interface
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn state(&self) -> Result<Option<CanState>> {
        Ok(self
            .can_param::<u32>(IflaCan::State)?
            .and_then(|st| CanState::try_from(st).ok()))
    }

    /// Set the full control mode (bit) collection.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_ctrlmodes<M>(&self, ctrlmode: M) -> Result<()>
    where
        M: Into<CanCtrlModes>,
    {
        let modes = ctrlmode.into();
        let modes: can_ctrlmode = modes.into();
        self.set_can_param(IflaCan::CtrlMode, modes)
    }

    /// Set or clear an individual control mode parameter.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_ctrlmode(&self, mode: CanCtrlMode, on: bool) -> Result<()> {
        self.set_ctrlmodes(CanCtrlModes::from_mode(mode, on))
    }

    /// Gets the control mode (bit) collection for the interface.
    ///
    /// The returned [`CanCtrlModes`] carries the kernel-reported `flags`
    /// (current state) alongside the `mask`; use [`CanCtrlModes::has_mode`]
    /// to test individual modes. Returns `None` if the interface reports no
    /// control-mode attribute.
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn ctrlmodes(&self) -> Result<Option<CanCtrlModes>> {
        Ok(self
            .can_param::<can_ctrlmode>(IflaCan::CtrlMode)?
            .map(CanCtrlModes))
    }

    /// Gets the automatic CANbus restart time for the interface, in milliseconds.
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn restart_ms(&self) -> Result<Option<u32>> {
        self.can_param::<u32>(IflaCan::RestartMs)
    }

    /// Set the automatic restart milliseconds of the interface
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_restart_ms(&self, restart_ms: u32) -> Result<()> {
        self.set_can_param(IflaCan::RestartMs, &restart_ms.to_ne_bytes()[..])
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
    pub fn restart(&self) -> Result<()> {
        // Note: The linux code shows the data type to be u32, but never
        // appears to access the value sent. iproute2 sends a 1, so we do
        // too!
        // See: linux/drivers/net/can/dev/netlink.c
        let restart_data: u32 = 1;
        self.set_can_param(IflaCan::Restart, &restart_data.to_ne_bytes()[..])
    }

    /// Gets the bus error counter from the interface
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn berr_counter(&self) -> Result<Option<CanBerrCounter>> {
        self.can_param::<CanBerrCounter>(IflaCan::BerrCounter)
    }

    /// Gets the data bit timing params for the interface
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn data_bit_timing(&self) -> Result<Option<CanBitTiming>> {
        self.can_param::<CanBitTiming>(IflaCan::DataBitTiming)
    }

    /// Sets the data bit timing params for the interface
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_data_bit_timing(&self, timing: CanBitTiming) -> Result<()> {
        self.set_can_param(IflaCan::DataBitTiming, timing)
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
    pub fn set_data_bitrate<P>(&self, bitrate: u32, sample_point: P) -> Result<()>
    where
        P: Into<Option<u32>>,
    {
        let sample_point: u32 = sample_point.into().unwrap_or(0);

        // The FD data phase runs faster than the classical 1 Mbit/s nominal
        // limit (commonly 2..8 Mbit/s), so the upper sanity bound is higher
        // than `set_bitrate`'s. This is a debug-only sanity check to catch
        // gross programmer errors; the kernel still validates the real value.
        debug_assert!(
            0 < bitrate && bitrate <= 8000000,
            "Data bitrate must be within 1..=8000000, received {}.",
            bitrate
        );
        debug_assert!(
            sample_point < 1000,
            "Sample point must be within 0..1000, received {}.",
            sample_point
        );

        self.set_data_bit_timing(CanBitTiming {
            bitrate,
            sample_point,
            ..CanBitTiming::default()
        })
    }

    /// Gets the data bit timing const params for the interface
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn data_bit_timing_const(&self) -> Result<Option<CanBitTimingConst>> {
        self.can_param::<CanBitTimingConst>(IflaCan::DataBitTimingConst)
    }

    /// Sets the CANbus termination for the interface
    ///
    /// Not all interfaces support setting a termination.
    /// Termination is in ohms. Your interface most likely only supports
    /// certain values. Common values are 0 and 120.
    ///
    /// PRIVILEGED: This requires root privilege.
    ///
    pub fn set_termination(&self, termination: u16) -> Result<()> {
        self.set_can_param(IflaCan::Termination, termination)
    }

    /// Gets the CANbus termination for the interface
    ///
    /// One netlink round trip; see [`can_params()`](Self::can_params) to read
    /// every parameter at once.
    pub fn termination(&self) -> Result<Option<u16>> {
        self.can_param::<u16>(IflaCan::Termination)
    }
}

/////////////////////////////////////////////////////////////////////////////

/// Tests that need neither a netlink socket nor privileges.
#[cfg(test)]
mod unit_tests {
    use super::*;

    /// Each protocol-level condition neli reports keeps its own variant, and
    /// only the message-level failures fall back to text. The crate-level
    /// conversion into [`crate::Error`] is tested separately, in `errors`.
    #[test]
    fn router_error_summary() {
        type RtErr = RouterError<Rtm, Ifinfomsg>;

        assert_eq!(NlError::from(RtErr::NoAck), NlError::NoAck);
        assert_eq!(NlError::from(RtErr::UnexpectedAck), NlError::UnexpectedAck);
        assert_eq!(NlError::from(RtErr::ClosedChannel), NlError::ClosedChannel);
        assert!(matches!(
            NlError::from(RtErr::new("malformed attribute")),
            NlError::Msg(_)
        ));

        // An I/O failure has no variant here, so it degrades to text. Callers
        // never see that: `Error::from()` keeps those as `Error::Io`.
        assert!(matches!(
            NlError::from(RtErr::Io(io::ErrorKind::PermissionDenied)),
            NlError::Msg(_)
        ));
    }

    /// The batch reader agrees with the same query made through
    /// [`CanInterface::details()`], which is where the per-parameter getters
    /// would each go separately.
    ///
    /// Read-only, so no privileges are needed — but it does need an
    /// interface, hence `vcan_tests`. A `vcan` reports no CAN link
    /// information, so both sides are the default here; what this pins is
    /// that the query succeeds and that the two paths agree.
    #[cfg(feature = "vcan_tests")]
    #[test]
    fn can_params_agrees_with_details() {
        let iface = CanInterface::open("vcan0").expect("vcan0 must exist");

        let params = iface.can_params().expect("can_params");
        let details = iface.details().expect("details");

        // The netlink parameter types have no `PartialEq`, so compare their
        // rendering, which covers every field.
        assert_eq!(format!("{params:?}"), format!("{:?}", details.can));
    }

    /// A query against an interface that does not exist reports the kernel's
    /// errno, rather than an interface with nothing to say.
    ///
    /// The kernel answers `RTM_GETLINK` for an unknown index with an
    /// `NLMSG_ERROR` message carrying `ENODEV`. Since the request asks for no
    /// ACK, neli hands that back as a message whose payload is not an
    /// `Ifinfomsg` instead of as a `RouterError`, and every read path used to
    /// treat the missing payload as "no parameters set": `details()` returned
    /// a plausible-looking record for an interface that was never there.
    ///
    /// Needs no interface and no privileges — the index simply has to be one
    /// the kernel does not know.
    #[test]
    fn query_on_a_missing_interface_reports_enodev() {
        let iface = CanInterface::open_iface(999_999);

        let results: [(&str, Result<()>); 4] = [
            ("details", iface.details().map(|_| ())),
            ("can_params", iface.can_params().map(|_| ())),
            ("bit_timing", iface.bit_timing().map(|_| ())),
            ("state", iface.state().map(|_| ())),
        ];

        for (name, res) in results {
            match res {
                Err(crate::Error::Nl(NlError::Netlink { errno })) => {
                    assert_eq!(errno, libc::ENODEV, "{name}");
                }
                other => panic!("{name}: expected ENODEV, got {other:?}"),
            }
        }
    }

    /// Index 0 means "unspecified", the same as no index at all, so
    /// `create()` looks the assigned index up by name instead of taking the
    /// caller's 0 as the answer.
    #[test]
    fn index_zero_is_unspecified() {
        assert_eq!(requested_index(None), None);
        assert_eq!(requested_index(0), None);
        assert_eq!(requested_index(Some(0)), None);
        assert_eq!(requested_index(1), Some(1));
        assert_eq!(requested_index(Some(42)), Some(42));
    }
}

/////////////////////////////////////////////////////////////////////////////

/// Netlink tests for SocketCAN control
#[cfg(feature = "netlink_tests")]
#[cfg(test)]
pub mod tests {
    use super::*;
    use serial_test::serial;
    use std::ops::Deref;

    /// RAII-style helper to create and clean-up a specific vcan interface for a single test.
    /// Using drop here ensures that the interface always gets cleaned up
    /// (although a restart would also remove it).
    ///
    /// Intended for use (ONLY) in tests as follows:
    /// ```ignore
    /// let interface = TemporaryInterface::new("my_test").unwrap();
    /// // use the interface..
    /// ```
    /// Please note that there is a limit to the length of interface names,
    /// namely 16 characters on Linux.
    #[allow(missing_copy_implementations)]
    #[derive(Debug)]
    pub struct TemporaryInterface {
        interface: CanInterface,
    }

    impl TemporaryInterface {
        /// Creates a temporaty interface
        #[allow(unused)]
        pub fn new(name: &str) -> Result<Self> {
            Ok(Self {
                interface: CanInterface::create_vcan(name, None)?,
            })
        }
    }

    impl Drop for TemporaryInterface {
        fn drop(&mut self) {
            assert!(
                CanInterface::open_iface(self.interface.if_index)
                    .delete()
                    .is_ok()
            );
        }
    }

    impl Deref for TemporaryInterface {
        type Target = CanInterface;

        fn deref(&self) -> &Self::Target {
            &self.interface
        }
    }

    #[test]
    #[serial]
    fn up_down() {
        let interface = TemporaryInterface::new("up_down").unwrap();

        assert!(interface.bring_up().is_ok());
        assert!(interface.details().unwrap().is_up);

        assert!(interface.bring_down().is_ok());
        assert!(!interface.details().unwrap().is_up);
    }

    #[test]
    #[serial]
    fn details() {
        let interface = TemporaryInterface::new("info").unwrap();
        let details = interface.details().unwrap();
        assert_eq!("info", details.name.unwrap());
        assert!(details.mtu.is_some());
        assert!(!details.is_up);
    }

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
