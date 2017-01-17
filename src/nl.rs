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
    // FIXME: check return length
    let _ = sock.send(msg, dest)?;

    // receive all pending messages
    let (addr, msgs) = sock.recv()?;

    match msgs.into_iter().nth(0) {
        Some(msg) => {
            println!("Received Address: {:?}", addr);
            println!("Received Message: {:?}", msg);
            match *msg.payload() {
                NetlinkPayload::Ack(_) => (),
                _ => panic!("Message received is not an ACK"),
            }
        }
        None => panic!("Expect ACK, but no ACK received"),
    }

    Ok(())

}

/// Opens a new netlink socket, bound to this process' PID
fn open_nl_route_socket() -> io::Result<NetlinkSocket> {
    let sock = NetlinkSocket::new(NetlinkProtocol::Route)?;

    // retrieve PID
    let pid = unsafe { libc::getpid() } as u32;
    let pid = 0;  // FIXME: is this necessary?

    // after opening the socket, bind it to be able to receive messages back
    // groups is set to 0, because we want no notifications
    let bind_addr = NetlinkAddr::new(pid, 0);
    sock.bind(bind_addr)?;

    Ok(sock)
}

/// Brings down a CAN interface
fn bring_down(if_index: c_uint) -> io::Result<()> {
    let mut nl = open_nl_route_socket()?;

    // prepare message
    let mut header = NlMsgHeader::user_defined(RTM_NEWLINK, mem::size_of::<IfInfoMsg>() as u32);
    header.ack();

    let info = IfInfoMsg::new(if_index as i32, 0, IFF_UP);
    let msg = NetlinkMessage::new(header, NetlinkPayload::Data(info.as_bytes()));

    let kernel_addr = NetlinkAddr::new(0, 0);

    // send the message
    send_and_read_ack(&mut nl, msg, &kernel_addr)
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
}
