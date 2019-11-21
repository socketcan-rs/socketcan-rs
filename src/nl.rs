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
use neli;
use neli::consts::nl::{Rtm, NlmF};
use neli::consts::rtnl::{Arphrd, Iff, RtAddrFamily};
use neli::consts::socket::NlFamily;
use neli::consts::NlType;
use neli::err::NlError;
use neli::nl::Nlmsghdr;
use neli::rtnl::{Ifinfomsg, Rtattrs};
use neli::socket::*;
use nix;
use nix::net::if_::if_nametoindex;

/// Sends a netlink message down a netlink socket, and checks if an ACK was
/// properly received.
fn send_and_read_ack<T, P>(sock: &mut NlSocket, msg: Nlmsghdr<T, P>) -> Result<(), NlError>
where
    T: neli::Nl + NlType,
    P: neli::Nl,
{
    sock.send_nl(msg)?;

    println!("Message sent, waiting for ACK");

    // receive pending message
    sock.recv_ack()?;

    println!("ACK received");

    Ok(())
}

/// Opens a new netlink socket, bound to this process' PID
fn open_nl_route_socket() -> Result<NlSocket, NlError> {
    // retrieve PID
    let pid = unsafe { libc::getpid() } as u32;

    // open and bind socket
    // groups is set to None(0), because we want no notifications
    let sock = NlSocket::connect(NlFamily::Route, Some(pid), None, false)?;

    Ok(sock)
}

/// SocketCAN interface
///
/// Controlled through the kernel's netlink interface, CAN devices can be
/// brought up or down or configured through this.
pub struct CanInterface {
    if_index: c_uint,
}

impl CanInterface {
    /// Open CAN interface by name
    ///
    /// Similar to `open_if`, but looks up the device by name instead
    pub fn open(ifname: &str) -> Result<CanInterface, nix::Error> {
        let if_index = if_nametoindex(ifname)?;
        Ok(CanInterface::open_if(if_index))
    }

    /// Open CAN interface
    ///
    /// Creates a new `CanInterface` instance. No actual "opening" is necessary
    /// or performed when calling this function.
    pub fn open_if(if_index: c_uint) -> CanInterface {
        CanInterface { if_index: if_index }
    }

    /// Bring down CAN interface
    ///
    /// Use a netlink control socket to set the interface status to "down".
    pub fn bring_down(&self) -> Result<(), NlError> {
        let mut nl = open_nl_route_socket()?;

        // settings flags to 0 and change to IFF_UP will disable the IFF_UP flag
        // let info = IfInfoMsg::new(self.if_index as i32, 0, Iff::Up);
        let mut info = Ifinfomsg::new(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            vec![],
            Rtattrs::empty(),
        );
        info.set_ifi_change(Iff::Up.into());

        // prepare message
        let msg = Nlmsghdr::new(
            None,
            Rtm::Newlink,
            vec![NlmF::Request, NlmF::Ack],
            None,
            Some(0),
            info,
        );
        // send the message
        send_and_read_ack(&mut nl, msg)
    }

    /// Bring up CAN interface
    ///
    /// Brings the interface up by settings its "up" flag enabled via netlink.
    pub fn bring_up(&self) -> Result<(), NlError> {
        let mut nl = open_nl_route_socket()?;

        // let info = IfinfoMsg::new(self.if_index as i32, Iff::Up, Iff::Up);
        let mut info = Ifinfomsg::new(
            RtAddrFamily::Unspecified,
            Arphrd::Netrom,
            self.if_index as c_int,
            vec![Iff::Up],
            Rtattrs::empty(),
        );
        info.set_ifi_change(Iff::Up.into());
        let msg = Nlmsghdr::new(
            None,
            Rtm::Newlink,
            vec![NlmF::Request, NlmF::Ack],
            None,
            Some(0),
            info,
        );
        send_and_read_ack(&mut nl, msg)
    }
}
