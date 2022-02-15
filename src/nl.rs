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

use libc::{self, c_int, c_uint};
use neli::{
    consts::{
        nl::{NlmF, NlmFFlags, NlType},
        rtnl::{Arphrd, RtAddrFamily, Rtm},
        socket::NlFamily,
    },
    err::NlError,
    nl::{Nlmsghdr, NlPayload},
    rtnl::Ifinfomsg,
    ToBytes,
    types::RtBuffer,
    socket::NlSocketHandle,
};
use nix::{self, unistd, net::if_::if_nametoindex};
use std::{
    result,
    fmt::Debug,
};

/// A result for Netlink errors.
type NlResult<T> = result::Result<T, NlError>;

/// SocketCAN interface
///
/// Controlled through the kernel's Netlink interface, CAN devices can be
/// brought up or down or configured through this.
pub struct CanInterface {
    if_index: c_uint,
}

impl CanInterface {
    /// Open CAN interface by name
    ///
    /// Similar to `open_if`, but looks up the device by name instead
    pub fn open(ifname: &str) -> result::Result<Self, nix::Error> {
        let if_index = if_nametoindex(ifname)?;
        Ok(Self::open_iface(if_index))
    }

    /// Open CAN interface
    ///
    /// Creates a new `CanInterface` instance. No actual "opening" is necessary
    /// or performed when calling this function.
    pub fn open_iface(if_index: u32) -> Self {
        Self { if_index: if_index as c_uint }
    }

    /// Sends an info message
    fn send_info_msg(info: Ifinfomsg) -> NlResult<()> {
        let mut nl = Self::open_route_socket()?;

        // prepare message
        let hdr = Nlmsghdr::new(
            None,
            Rtm::Newlink,
            NlmFFlags::new(&[NlmF::Request, NlmF::Ack]),
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
        // TODO: Implement this
        //sock.recv_ack()?;
        Ok(())
    }

    /// Opens a new netlink socket, bound to this process' PID
    fn open_route_socket() -> NlResult<NlSocketHandle> {
        // retrieve PID
        let pid = unistd::getpid().as_raw() as u32;

        // open and bind socket
        // groups is set to None(0), because we want no notifications
        let sock = NlSocketHandle::connect(NlFamily::Route, Some(pid), &[])?;
        Ok(sock)
    }

    /// Bring down CAN interface
    ///
    /// Use a netlink control socket to set the interface status to "down".
    pub fn bring_down(&self) -> NlResult<()> {
        let info = Ifinfomsg::down(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            RtBuffer::new()
        );
        Self::send_info_msg(info)
    }

    /// Bring up CAN interface
    ///
    /// Brings the interface up by settings its "up" flag enabled via netlink.
    pub fn bring_up(&self) -> NlResult<()> {
        let info = Ifinfomsg::up(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            RtBuffer::new()
        );
        Self::send_info_msg(info)
    }
}
