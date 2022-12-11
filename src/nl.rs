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
use std::{
    fmt::Debug,
    os::raw::{c_int, c_uint},
};

use neli::consts::rtnl::{IffFlags, Ifla, IflaInfo};
use neli::rtnl::Rtattr;
use neli::{
    consts::{
        nl::{NlType, NlmF, NlmFFlags},
        rtnl::{Arphrd, RtAddrFamily, Rtm},
        socket::NlFamily,
    },
    err::NlError,
    nl::{NlPayload, Nlmsghdr},
    rtnl::Ifinfomsg,
    socket::NlSocketHandle,
    types::RtBuffer,
    ToBytes,
};
use nix::{self, net::if_::if_nametoindex, unistd};

/// A result for Netlink errors.
type NlResult<T> = Result<T, NlError>;

/// SocketCAN interface
///
/// Controlled through the kernel's Netlink interface, CAN devices can be
/// brought up or down or configured through this.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct CanInterface {
    if_index: c_uint,
}

impl CanInterface {
    /// Open CAN interface by name
    ///
    /// Similar to `open_if`, but looks up the device by name instead
    pub fn open(ifname: &str) -> Result<Self, nix::Error> {
        let if_index = if_nametoindex(ifname)?;
        Ok(Self::open_iface(if_index))
    }

    /// Open CAN interface
    ///
    /// Creates a new `CanInterface` instance. No actual "opening" is necessary
    /// or performed when calling this function.
    pub fn open_iface(if_index: u32) -> Self {
        Self {
            if_index: if_index as c_uint,
        }
    }

    /// Sends an info message
    fn send_info_msg(info: Ifinfomsg, additional_flags: &[NlmF]) -> NlResult<()> {
        let mut nl = Self::open_route_socket()?;

        // prepare message
        let hdr = Nlmsghdr::new(
            None,
            Rtm::Newlink,
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
            RtBuffer::new(),
        );
        Self::send_info_msg(info, &[])
    }

    /// Bring up CAN interface
    ///
    /// Brings the interface up by settings its "up" flag enabled via netlink.
    pub fn bring_up(&self) -> NlResult<()> {
        let info = Ifinfomsg::up(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            RtBuffer::new(),
        );
        Self::send_info_msg(info, &[])
    }

    pub fn create_vcan(name: &str) -> NlResult<Self> {
        debug_assert!(name.len() <= libc::IFNAMSIZ);

        let info = Ifinfomsg::new(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            0,
            IffFlags::empty(),
            IffFlags::empty(),
            {
                let mut buffer = RtBuffer::new();
                buffer.push(Rtattr::new(None, Ifla::Ifname, name)?);
                let mut linkinfo = Rtattr::new(None, Ifla::Linkinfo, Vec::<u8>::new())?;
                linkinfo.add_nested_attribute(&Rtattr::new(None, IflaInfo::Kind, "vcan")?)?;
                buffer.push(linkinfo);
                buffer
            },
        );
        let _ = Self::send_info_msg(info, &[NlmF::Create, NlmF::Excl])?;
        if let Ok(if_index) = if_nametoindex(name) {
            Ok(Self { if_index })
        } else {
            Err(NlError::Msg(
                "Interface must have been deleted between request and this check"
                    .parse()
                    .unwrap(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AutoInterface {
        interface: CanInterface,
    }

    impl AutoInterface {
        fn new(name: &str) -> NlResult<Self> {
            Ok(Self {
                interface: CanInterface::create_vcan(name)?,
            })
        }
    }

    impl Drop for AutoInterface {
        fn drop(&mut self) {
            let _ = interface.remove();
        }
    }

    #[cfg(feature = "vcan_tests")]
    #[cfg(feature = "root_tests")]
    #[test]
    fn bring_up() {
        let interface = dbg!(CanInterface::create_vcan("bring_up")).unwrap();
        assert!(dbg!(interface.bring_up()).is_ok())
    }
}
