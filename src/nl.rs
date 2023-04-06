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
    types::RtBuffer,
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
    use libc::c_uint;

    #[allow(unused)]
    pub const EXT_FILTER_VF: c_uint = 1 << 0;
    #[allow(unused)]
    pub const EXT_FILTER_BRVLAN: c_uint = 1 << 1;
    #[allow(unused)]
    pub const EXT_FILTER_BRVLAN_COMPRESSED: c_uint = 1 << 2;
    #[allow(unused)]
    pub const EXT_FILTER_SKIP_STATS: c_uint = 1 << 3;
    #[allow(unused)]
    pub const EXT_FILTER_MRP: c_uint = 1 << 4;
    #[allow(unused)]
    pub const EXT_FILTER_CFM_CONFIG: c_uint = 1 << 5;
    #[allow(unused)]
    pub const EXT_FILTER_CFM_STATUS: c_uint = 1 << 6;
    #[allow(unused)]
    pub const EXT_FILTER_MST: c_uint = 1 << 7;
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

    /// Sends an info message.
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
        Self::send_info_msg(Rtm::Newlink, info, &[])
    }

    /// PRIVILEGED: Attempt to create a VCAN interface. Useful for testing applications.
    /// Note that the length of the name is capped by ```libc::IFNAMSIZ```.
    pub fn create_vcan(name: &str, index: Option<u32>) -> NlResult<Self> {
        Self::create(name, index, "vcan")
    }

    /// PRIVILEGED: Create an interface of the given kind.
    /// Note that the length of the name is capped by ```libc::IFNAMSIZ```.
    pub fn create(name: &str, index: Option<u32>, kind: &str) -> NlResult<Self> {
        if name.len() > libc::IFNAMSIZ {
            return Err(NlError::Msg("Interface name too long".into()));
        }

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
            // Unfortunately netlink does not return the the if_index assigned to the interface..
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

    /// PRIVILEGED: Attempt to delete the interface.
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

    /// PRIVILEGED: Attempt to set the MTU of this interface.
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
