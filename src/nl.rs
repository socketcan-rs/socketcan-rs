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
//!
//! The netlink module currently relies on the
//! [netlink-rs](https://crates.io/crates/netlink-rs), which has some
//! deficiencies. If those are not fixed, a reimplmentation of netlink-rs'
//! functionality might be required.

use byte_conv::As as AsBytes;
use libc::{self, c_char, c_ushort, c_int, c_uint};
use netlink_rs::socket::{Msg as NetlinkMessage, Socket as NetlinkSocket, NetlinkAddr,
                         Payload as NetlinkPayload, NlMsgHeader};
use netlink_rs::Protocol as NetlinkProtocol;
use nix;
use nix::net::if_::if_nametoindex;
use std::{mem, io};

// linux/rtnetlink.h
const RTM_NEWLINK: u16 = 16;

// linux/socket.h
const AF_UNSPEC: c_char = 0;

// linux/if.h; netdevice(7)
const IFF_UP: c_uint = 1;

/// Mirrors the `struct ifinfomsg` (see rtnetlink(7))
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct IfInfoMsg {
    /// Address family, should always be `AF_UNSPEC`
    family: c_char,

    /// Padding bytes, should be set to 0.
    _pad: c_char, // must be 0

    /// Device type (FIXME: ?)
    dev_type: c_ushort,

    /// The interface index, can be retrieved using `if_nametoindex` from the
    /// `nix` crate.
    index: c_int,

    /// Device flags (FIXME: ?)
    flags: c_uint,

    /// Change mask
    change: c_uint,
}

impl IfInfoMsg {
    fn new(if_index: i32, flags: c_uint, change: c_uint) -> IfInfoMsg {
        IfInfoMsg {
            family: AF_UNSPEC,
            _pad: 0,

            // dev_type: ARPHRD_CAN,
            dev_type: 0,
            index: if_index,
            flags: flags,

            change: change,
        }
    }
}


/// Sends a netlink message down a netlink socket, and checks if an ACK was
/// properly received.
fn send_and_read_ack(sock: &mut NetlinkSocket,
                     msg: NetlinkMessage,
                     dest: &NetlinkAddr)
                     -> io::Result<()> {

    let msg_len = msg.header().msg_length() as usize;
    let bytes_sent = sock.send(msg, dest)?;
    if bytes_sent != msg_len {
        return Err(io::Error::new(io::ErrorKind::Other, "Incomplete write"));
    }

    // receive all pending messages
    let (addr, msgs) = sock.recv()?;

    match msgs.into_iter().nth(0) {
        Some(msg) => {
            println!("Received Address: {:?}", addr);
            println!("Received Message: {:?}", msg);
            match *msg.payload() {
                NetlinkPayload::Ack(_) => (),
                NetlinkPayload::Err(errno, _) => {
                    return Err(io::Error::from_raw_os_error(-errno));
                }
                NetlinkPayload::None => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData,
                                              "Received no payload when
                                              expecting an ACK"));

                }
                NetlinkPayload::Data(_) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData,
                                              "Received data when expecting an ACK"));
                }
            }
        }
        None => {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof,
                                      "ACK expected, but got nothing instead"))
        }
    }

    Ok(())

}

/// Opens a new netlink socket, bound to this process' PID
fn open_nl_route_socket() -> io::Result<NetlinkSocket> {
    let sock = NetlinkSocket::new(NetlinkProtocol::Route)?;

    // retrieve PID
    let pid = unsafe { libc::getpid() } as u32;

    // after opening the socket, bind it to be able to receive messages back
    // groups is set to 0, because we want no notifications
    let bind_addr = NetlinkAddr::new(pid, 0);
    sock.bind(bind_addr)?;

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
    pub fn bring_down(&self) -> io::Result<()> {
        let mut nl = open_nl_route_socket()?;

        // prepare message
        let mut header = NlMsgHeader::user_defined(RTM_NEWLINK, mem::size_of::<IfInfoMsg>() as u32);
        header.ack();

        // settings flags to 0 and change to IFF_UP will disable the IFF_UP flag
        let info = IfInfoMsg::new(self.if_index as i32, 0, IFF_UP);
        let msg = NetlinkMessage::new(header, NetlinkPayload::Data(info.as_bytes()));

        // send the message
        send_and_read_ack(&mut nl, msg, &NetlinkAddr::new(0, 0))
    }

    /// Bring up CAN interface
    ///
    /// Brings the interface up by settings its "up" flag enabled via netlink.
    pub fn bring_up(&self) -> io::Result<()> {
        let mut nl = open_nl_route_socket()?;

        let mut header = NlMsgHeader::user_defined(RTM_NEWLINK, mem::size_of::<IfInfoMsg>() as u32);
        header.ack();

        let info = IfInfoMsg::new(self.if_index as i32, IFF_UP, IFF_UP);
        let msg = NetlinkMessage::new(header, NetlinkPayload::Data(info.as_bytes()));

        // send the message
        send_and_read_ack(&mut nl, msg, &NetlinkAddr::new(0, 0))
    }
}
