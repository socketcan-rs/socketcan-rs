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
use libc::{canid_t, sa_family_t, sockaddr, sockaddr_can, sockaddr_storage, socklen_t};
use nix::net::if_::if_nametoindex;
use socket2::{SockAddr, SockAddrStorage};
use std::{fmt, io, mem, mem::size_of, os::raw::c_int};

pub use libc::{AF_CAN, CAN_RAW, PF_CAN};

// The J1939 "unset" markers and address limits, for use with
// `CanAddr::new_j1939()` and `CanAddr::from_iface_j1939()`.
//
// A J1939 address field left at zero is not "unspecified" — zero is a valid
// name, PGN and node address — so a socket that means "any" has to say so
// with these.
//
// Each is typed to match the `CanAddr::new_j1939()` parameter it belongs to,
// rather than being re-exported from libc: `libc::J1939_NO_NAME` is a
// `c_ulong`, which is 32 bits wide on a 32-bit target although the kernel
// field is a `__u64`, so a re-export would force a cast on some targets and
// warn about a needless one on others.

/// A J1939 node name meaning "unset" (`J1939_NO_NAME`).
// The cast is a no-op on a 64-bit target and a widening one on a 32-bit
// target, where `c_ulong` is 32 bits wide; clippy only ever sees the former.
#[allow(clippy::unnecessary_cast)]
pub const J1939_NO_NAME: u64 = libc::J1939_NO_NAME as u64;

/// A J1939 Parameter Group Number meaning "unset" (`J1939_NO_PGN`).
pub const J1939_NO_PGN: u32 = libc::J1939_NO_PGN;

/// A J1939 node address meaning "unset" (`J1939_NO_ADDR`).
pub const J1939_NO_ADDR: u8 = libc::J1939_NO_ADDR;

/// The J1939 idle node address (`J1939_IDLE_ADDR`).
pub const J1939_IDLE_ADDR: u8 = libc::J1939_IDLE_ADDR;

/// The highest node address usable for unicast (`J1939_MAX_UNICAST_ADDR`).
pub const J1939_MAX_UNICAST_ADDR: u8 = libc::J1939_MAX_UNICAST_ADDR;

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
/// The J1939 and ISO-TP constructors below build an address for a socket of
/// that protocol, which this crate does not open: its socket types speak
/// `CAN_RAW`. See the *Other CAN protocols* section of the [crate
/// documentation](crate) for what to do with such an address.
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
    ///
    /// For a socket you open yourself with the `CAN_J1939` protocol — see the
    /// *Other CAN protocols* section of the [crate documentation](crate).
    /// Use [`J1939_NO_NAME`], [`J1939_NO_PGN`] and [`J1939_NO_ADDR`] for the
    /// fields you mean to leave unspecified; zero is a valid value for all
    /// three, so it does not mean "any".
    pub fn new_j1939(ifindex: u32, name: u64, pgn: u32, jaddr: u8) -> Self {
        let mut addr = Self::new(ifindex);
        addr.0.can_addr.j1939.name = name;
        addr.0.can_addr.j1939.pgn = pgn;
        addr.0.can_addr.j1939.addr = jaddr;
        addr
    }

    /// Creates a new CAN ISO-TP socket address for the specified interface
    /// by index.
    ///
    /// For a socket you open yourself with the `CAN_ISOTP` protocol — see the
    /// *Other CAN protocols* section of the [crate documentation](crate).
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

    /// Gets the interface index.
    ///
    /// Zero means "all interfaces" for a raw socket.
    pub fn ifindex(&self) -> u32 {
        self.0.can_ifindex as u32
    }

    /// Gets the J1939 node name, from the `can_addr.j1939` fields.
    ///
    /// `sockaddr_can` holds the J1939 and ISO-TP fields in a union, and
    /// carries no discriminator for which is in use.
    ///
    /// [`J1939_NO_NAME`](libc::J1939_NO_NAME) means "unset".
    pub fn j1939_name(&self) -> u64 {
        // SAFETY: every field of both union variants is an integer, so any
        // bit pattern is a valid value of the type being read. The struct is
        // fully initialised by every constructor. See the note above on
        // reading the variant that was not written.
        unsafe { self.0.can_addr.j1939.name }
    }

    /// Gets the J1939 Parameter Group Number, from the `can_addr.j1939`
    /// fields.
    ///
    /// [`J1939_NO_PGN`](libc::J1939_NO_PGN) means "unset". See
    /// [`j1939_name()`](Self::j1939_name) on reading the union.
    pub fn j1939_pgn(&self) -> u32 {
        // SAFETY: as in `j1939_name()`.
        unsafe { self.0.can_addr.j1939.pgn }
    }

    /// Gets the J1939 source/destination address, from the `can_addr.j1939`
    /// fields.
    ///
    /// [`J1939_NO_ADDR`](libc::J1939_NO_ADDR) means "unset". See
    /// [`j1939_name()`](Self::j1939_name) on reading the union.
    pub fn j1939_addr(&self) -> u8 {
        // SAFETY: as in `j1939_name()`.
        unsafe { self.0.can_addr.j1939.addr }
    }

    /// Gets the ISO-TP receive identifier, from the `can_addr.tp` fields.
    ///
    /// This is the raw composite ID word, so an extended identifier carries
    /// [`CAN_EFF_FLAG`](crate::id::CAN_EFF_FLAG); pass it through
    /// [`id_from_raw()`](crate::id::id_from_raw) for a typed identifier. See
    /// [`j1939_name()`](Self::j1939_name) on reading the union.
    pub fn tp_rx_id(&self) -> canid_t {
        // SAFETY: as in `j1939_name()`.
        unsafe { self.0.can_addr.tp.rx_id }
    }

    /// Gets the ISO-TP transmit identifier, from the `can_addr.tp` fields.
    ///
    /// This is the raw composite ID word, as with
    /// [`tp_rx_id()`](Self::tp_rx_id).
    pub fn tp_tx_id(&self) -> canid_t {
        // SAFETY: as in `j1939_name()`.
        unsafe { self.0.can_addr.tp.tx_id }
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
        // Render the can_addr union as raw bytes. There is no discriminator
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
        // socket2 0.6 takes its own `SockAddrStorage` (repr(transparent) over
        // `sockaddr_storage`) rather than the libc type. Fill it via `view_as`,
        // the conversion pattern documented on `SockAddrStorage`.
        let mut s2_storage = SockAddrStorage::zeroed();
        // SAFETY: `sockaddr_storage` is a valid `sockaddr_*` storage type for
        // this platform, and `s2_storage` is at least as large.
        unsafe {
            *s2_storage.view_as::<sockaddr_storage>() = storage;
            SockAddr::new(s2_storage, len)
        }
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
    use embedded_can::{ExtendedId, StandardId};

    const IDX: u32 = 42;

    #[test]
    fn test_addr() {
        let _addr = CanAddr::new(IDX);

        assert_eq!(size_of::<sockaddr_can>(), CanAddr::len());
    }

    #[test]
    fn test_addr_to_sock_addr() {
        let addr = CanAddr::new(IDX);

        let (sock_addr, len) = addr.into_storage();

        assert_eq!(CanAddr::len() as socklen_t, len);
        // SAFETY: both values are fully initialised — `addr` via `CanAddr::new`
        // and `sock_addr` returned from `into_storage` which zero-initialises
        // `sockaddr_storage` before copying.
        let (lhs, rhs) = unsafe { (as_bytes(&addr), as_bytes(&sock_addr)) };
        assert_eq!(lhs, &rhs[0..len as usize]);
    }

    /// Every field a constructor writes can be read back, without the caller
    /// reaching into the union itself.
    #[test]
    fn fields_read_back() {
        let raw = CanAddr::new(IDX);
        assert_eq!(raw.ifindex(), IDX);

        let j = CanAddr::new_j1939(IDX, 0x1234_5678_9ABC_DEF0, 0x0EA00, 0x42);
        assert_eq!(j.ifindex(), IDX);
        assert_eq!(j.j1939_name(), 0x1234_5678_9ABC_DEF0);
        assert_eq!(j.j1939_pgn(), 0x0EA00);
        assert_eq!(j.j1939_addr(), 0x42);

        let tp = CanAddr::new_isotp(
            IDX,
            StandardId::new(0x123).unwrap(),
            ExtendedId::new(0x1F00_4711).unwrap(),
        );
        assert_eq!(tp.ifindex(), IDX);
        assert_eq!(tp.tp_rx_id(), 0x123);
        // An extended identifier keeps its EFF flag in the raw ID word.
        assert_eq!(tp.tp_tx_id(), 0x1F00_4711 | crate::id::CAN_EFF_FLAG);
        assert_eq!(
            crate::id::id_from_raw(tp.tp_rx_id()),
            Some(Id::Standard(StandardId::new(0x123).unwrap()))
        );
    }

    /// An address that arrives from the kernel — a `recvfrom()` peer, say —
    /// reads back the same way.
    #[test]
    fn fields_read_back_through_from_sockaddr() {
        let sent = CanAddr::new_j1939(IDX, 0xAAAA_BBBB_CCCC_DDDD, 0x0EE00, 0x21);
        let received = CanAddr::from(*sent.as_ref());

        assert_eq!(received.ifindex(), IDX);
        assert_eq!(received.j1939_name(), 0xAAAA_BBBB_CCCC_DDDD);
        assert_eq!(received.j1939_pgn(), 0x0EE00);
        assert_eq!(received.j1939_addr(), 0x21);
    }

    /// The union has no discriminator, so reading the variant that was not
    /// written reinterprets bytes rather than failing. Documented on
    /// `j1939_name()`; pinned here so the behaviour is not mistaken for a bug.
    #[test]
    fn reading_the_other_variant_reinterprets() {
        let tp = CanAddr::new_isotp(
            IDX,
            StandardId::new(0x123).unwrap(),
            StandardId::new(0x321).unwrap(),
        );

        // rx_id and tx_id occupy the same eight bytes as the J1939 name.
        let expected = u64::from(0x123u32) | (u64::from(0x321u32) << 32);
        assert_eq!(tp.j1939_name(), expected);
    }
}
