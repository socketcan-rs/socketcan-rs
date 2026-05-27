// socketcan/src/addr.rs
//
// SocketCAN address types.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! SocketCAN address type.

use crate::id::id_to_canid_t;
use embedded_can::Id;
use libc::{sa_family_t, sockaddr, sockaddr_can, sockaddr_storage, socklen_t};
use nix::net::if_::if_nametoindex;
use socket2::SockAddr;
use std::{fmt, io, mem, mem::size_of, os::raw::c_int};

pub use libc::{AF_CAN, CAN_RAW, PF_CAN};

/// CAN socket address.
///
/// This is the address for use with CAN sockets. It is simply an address to
/// the SocketCAN host interface. It can be created by looking up the name
/// of the interface, like "can0", "vcan0", etc, or an interface index can
/// be specified directly, if known. An index of zero can be used to read
/// frames from all interfaces.
///
/// This is based on, and compatible with, the `sockaddr_can` struct from
/// libc.
/// [ref](https://docs.rs/libc/latest/libc/struct.sockaddr_can.html)
///
/// Equality and hashing consider only `can_family` and `can_ifindex`. The
/// `can_addr` union (J1939 / ISO-TP fields) is not compared: there is no
/// runtime discriminator for which union variant is active, so a byte-wise
/// compare across the union plus its padding would be both
/// undefined-behaviour-adjacent and incorrect for any non-raw socket
/// flavour. Callers that need to compare J1939 or ISO-TP addresses should
/// compare the relevant fields explicitly.
#[derive(Clone, Copy)]
pub struct CanAddr(sockaddr_can);

impl CanAddr {
    /// Creates a new CAN socket address for the specified interface by index.
    /// An index of zero can be used to read from all interfaces.
    pub fn new(ifindex: u32) -> Self {
        let mut addr = Self::default();
        addr.0.can_ifindex = ifindex as c_int;
        addr
    }

    /// Creates a new CAN J1939 socket address for the specified interface
    /// by index.
    pub fn new_j1939(ifindex: u32, name: u64, pgn: u32, jaddr: u8) -> Self {
        let mut addr = Self::new(ifindex);
        addr.0.can_addr.j1939.name = name;
        addr.0.can_addr.j1939.pgn = pgn;
        addr.0.can_addr.j1939.addr = jaddr;
        addr
    }

    /// Creates a new CAN ISO-TP socket address for the specified interface
    /// by index.
    pub fn new_isotp<R, T>(ifindex: u32, rx_id: R, tx_id: T) -> Self
    where
        R: Into<Id>,
        T: Into<Id>,
    {
        let mut addr = Self::new(ifindex);
        addr.0.can_addr.tp.rx_id = id_to_canid_t(rx_id);
        addr.0.can_addr.tp.tx_id = id_to_canid_t(tx_id);
        addr
    }

    /// Try to create an address from an interface name.
    pub fn from_iface(ifname: &str) -> io::Result<Self> {
        let ifindex = if_nametoindex(ifname)?;
        Ok(Self::new(ifindex))
    }

    /// Try to create a J1939 address from an interface name.
    pub fn from_iface_j1939(ifname: &str, name: u64, pgn: u32, jaddr: u8) -> io::Result<Self> {
        let mut addr = Self::from_iface(ifname)?;
        addr.0.can_addr.j1939.name = name;
        addr.0.can_addr.j1939.pgn = pgn;
        addr.0.can_addr.j1939.addr = jaddr;
        Ok(addr)
    }

    /// Try to create a ISO-TP address from an interface name.
    pub fn from_iface_isotp<R, T>(ifname: &str, rx_id: R, tx_id: T) -> io::Result<Self>
    where
        R: Into<Id>,
        T: Into<Id>,
    {
        let mut addr = Self::from_iface(ifname)?;
        addr.0.can_addr.tp.rx_id = id_to_canid_t(rx_id);
        addr.0.can_addr.tp.tx_id = id_to_canid_t(tx_id);
        Ok(addr)
    }

    /// Gets the address of the structure as a `sockaddr_can` pointer.
    pub fn as_ptr(&self) -> *const sockaddr_can {
        &self.0
    }

    /// Gets the address of the structure as a `sockaddr` pointer.
    pub fn as_sockaddr_ptr(&self) -> *const sockaddr {
        self.as_ptr().cast()
    }

    /// Gets the size of the address structure.
    pub fn len() -> usize {
        size_of::<sockaddr_can>()
    }

    /// Gets the underlying address as a byte slice
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: `CanAddr` is constructed only through `new`/`new_j1939`/
        // `new_isotp`/`From<sockaddr_can>`, all of which initialise the
        // entire `sockaddr_can` (via `mem::zeroed` plus typed field writes).
        unsafe { crate::as_bytes(&self.0) }
    }

    /// Converts the address into a `sockaddr_storage` type.
    /// The storage type is a generic socket address container with enough
    /// space to hold any address in the system (not just CAN addresses).
    pub fn into_storage(self) -> (sockaddr_storage, socklen_t) {
        let can_addr = self.as_bytes();
        let len = can_addr.len();

        let mut storage: sockaddr_storage = unsafe { mem::zeroed() };
        // SAFETY: `storage` is fully zero-initialised on the line above.
        let sock_addr = unsafe { crate::as_bytes_mut(&mut storage) };

        sock_addr[..len].copy_from_slice(can_addr);
        (storage, len as socklen_t)
    }

    /// Converts the address into a `socket2::SockAddr`
    pub fn into_sock_addr(self) -> SockAddr {
        SockAddr::from(self)
    }
}

impl Default for CanAddr {
    fn default() -> Self {
        let mut addr: sockaddr_can = unsafe { mem::zeroed() };
        addr.can_family = AF_CAN as sa_family_t;
        Self(addr)
    }
}

impl fmt::Debug for CanAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Render the can_addr union as raw bytes — there is no discriminator
        // for which union variant is active, so the bytes are the best we can
        // safely show. Callers know the variant from their socket type.
        // SAFETY: `CanAddr` is constructed only through `new`/`new_j1939`/
        // `new_isotp`/`From<sockaddr_can>`, all of which fully initialise the
        // structure (`mem::zeroed` plus typed field writes), so every byte of
        // the union storage has been written before being read here.
        let addr_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                (&self.0.can_addr as *const _) as *const u8,
                size_of::<libc::__c_anonymous_sockaddr_can_can_addr>(),
            )
        };
        f.debug_struct("CanAddr")
            .field("can_family", &self.0.can_family)
            .field("can_ifindex", &self.0.can_ifindex)
            .field("can_addr", &format_args!("{:02X?}", addr_bytes))
            .finish()
    }
}

impl PartialEq for CanAddr {
    /// Compares two `CanAddr` by `can_family` and `can_ifindex`.
    /// See the type-level docs for why the `can_addr` union is excluded.
    fn eq(&self, other: &Self) -> bool {
        self.0.can_family == other.0.can_family && self.0.can_ifindex == other.0.can_ifindex
    }
}

impl Eq for CanAddr {}

impl std::hash::Hash for CanAddr {
    /// Hashes `can_family` and `can_ifindex`; mirrors [`PartialEq`].
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.can_family.hash(state);
        self.0.can_ifindex.hash(state);
    }
}

impl From<sockaddr_can> for CanAddr {
    fn from(addr: sockaddr_can) -> Self {
        debug_assert_eq!(
            addr.can_family, AF_CAN as sa_family_t,
            "CanAddr: sockaddr_can must have can_family == AF_CAN",
        );
        Self(addr)
    }
}

impl From<CanAddr> for SockAddr {
    fn from(addr: CanAddr) -> Self {
        let (storage, len) = addr.into_storage();
        unsafe { SockAddr::new(storage, len) }
    }
}

impl AsRef<sockaddr_can> for CanAddr {
    fn as_ref(&self) -> &sockaddr_can {
        &self.0
    }
}

/////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;
    use crate::as_bytes;

    const IDX: u32 = 42;

    #[test]
    fn test_addr() {
        let _addr = CanAddr::new(IDX);

        assert_eq!(size_of::<sockaddr_can>(), CanAddr::len());
    }

    #[test]
    fn test_addr_to_sock_addr() {
        let addr = CanAddr::new(IDX);

        let (sock_addr, len) = addr.clone().into_storage();

        assert_eq!(CanAddr::len() as socklen_t, len);
        // SAFETY: both values are fully initialised — `addr` via `CanAddr::new`
        // and `sock_addr` returned from `into_storage` which zero-initialises
        // `sockaddr_storage` before copying.
        let (lhs, rhs) = unsafe { (as_bytes(&addr), as_bytes(&sock_addr)) };
        assert_eq!(lhs, &rhs[0..len as usize]);
    }
}
