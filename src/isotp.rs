// socketcan/src/isotp.rs
//
// Implementation of ISO-TP (ISO 15765-2) CAN transport protocol sockets for Linux.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Implementation of ISO 15765-2 (ISO-TP) CAN transport protocol sockets on Linux.
//!
//! ISO-TP is an international standard for segmenting and reassembling CAN data packets
//! that exceed the standard CAN frame payload size (up to 4095 bytes in CAN 2.0, and
//! up to 4 GiB with CAN FD). It defines flow control, consecutive frame sequence numbers,
//! and block-size pacing between sender and receiver.
//!
//! It is the transport layer used by automotive diagnostics systems, including
//! Unified Diagnostic Services (UDS, ISO 14229) and On-Board Diagnostics (OBD-II, ISO 15031-5).
//!
//! Linux has native in-kernel ISO-TP support via the `can-isotp` driver module, exposing
//! an `AF_CAN` socket with `SOCK_DGRAM` and protocol `CAN_ISOTP`.
//!
//! # Examples
//!
//! Opening a blocking ISO-TP socket on interface `can0` to send and receive diagnostic payloads:
//!
//! ```no_run
//! use socketcan::{CanAddr, CanIsoTpSocket, StandardId};
//! use std::io::{Read, Write};
//!
//! # fn main() -> std::io::Result<()> {
//! // Open an ISO-TP socket listening on Rx ID 0x7E8 and transmitting on Tx ID 0x7E0
//! let mut sock = CanIsoTpSocket::open("vcan0", StandardId::new(0x7E8).unwrap(), StandardId::new(0x7E0).unwrap())?;
//!
//! // Transmit a multi-frame UDS diagnostic request (segmented automatically by the kernel)
//! let request = [0x22, 0xF1, 0x90]; // ReadDataByIdentifier: VIN
//! sock.write_all(&request)?;
//!
//! // Receive the multi-frame response (reassembled automatically by the kernel)
//! let mut response = [0u8; 512];
//! let n = sock.read(&mut response)?;
//! println!("Received {} bytes of diagnostic response", n);
//! # Ok(())
//! # }
//! ```

use crate::{
    addr::CanAddr,
    socket::{CAN_ISOTP, CAN_MTU, CANFD_MTU, SOL_CAN_BASE, SocketOptions},
};
use bitflags::bitflags;
use embedded_can::Id;
use libc::{AF_CAN, c_int, c_void, socklen_t};
use socket2::SockAddr;
use std::{
    io::{self, Read, Write},
    mem::size_of,
    os::unix::io::{AsFd, AsRawFd, BorrowedFd, IntoRawFd, OwnedFd, RawFd},
    time::Duration,
};

/// The socket option level for ISO-TP sockets.
///
/// `SOL_CAN_ISOTP = SOL_CAN_BASE + CAN_ISOTP = 106`
pub const SOL_CAN_ISOTP: c_int = SOL_CAN_BASE + CAN_ISOTP;

/// Socket option to configure ISO-TP protocol parameters (`can_isotp_options`).
pub const CAN_ISOTP_OPTS: c_int = 1;

/// Socket option to configure ISO-TP reception flow control options (`can_isotp_fc_options`).
pub const CAN_ISOTP_RECV_FC: c_int = 2;

/// Socket option to force transmission STmin timer in nanoseconds (`__u32`).
pub const CAN_ISOTP_TX_STMIN: c_int = 3;

/// Socket option to force minimum receive separation time in nanoseconds (`__u32`).
pub const CAN_ISOTP_RX_STMIN: c_int = 4;

/// Socket option to configure link-layer parameters (`can_isotp_ll_options`).
pub const CAN_ISOTP_LL_OPTS: c_int = 5;

bitflags! {
    /// Flags for ISO-TP socket behavior (`can_isotp_options.flags`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct IsoTpFlags: u32 {
        /// Listen only (do not transmit Flow Control frames).
        const LISTEN_MODE   = 0x0001;
        /// Enable 1-byte extended addressing.
        const EXTEND_ADDR   = 0x0002;
        /// Enable CAN frame padding on the transmit path.
        const TX_PADDING    = 0x0004;
        /// Enable CAN frame padding on the receive path.
        const RX_PADDING    = 0x0008;
        /// Check received CAN frame padding length.
        const CHK_PAD_LEN   = 0x0010;
        /// Check received CAN frame padding content.
        const CHK_PAD_DATA  = 0x0020;
        /// Half duplex error state handling.
        const HALF_DUPLEX   = 0x0040;
        /// Ignore STmin value provided in received Flow Control frame.
        const FORCE_TXSTMIN = 0x0080;
        /// Ignore consecutive frames arriving faster than forced RX STmin.
        const FORCE_RXSTMIN = 0x0100;
        /// Different receive extended addressing.
        const RX_EXT_ADDR   = 0x0200;
        /// Wait for transmit completion before returning from write.
        const WAIT_TX_DONE  = 0x0400;
        /// 1-to-N functional addressing for Single Frames.
        const SF_BROADCAST  = 0x0800;
        /// 1-to-N transmission without Flow Control.
        const CF_BROADCAST  = 0x1000;
        /// Dynamic Flow Control parameters (Block Size and STmin).
        const DYN_FC_PARMS  = 0x2000;
    }
}

/// Default padding byte used to prevent CAN bit-stuffing overhead (`0xCC`).
pub const CAN_ISOTP_DEFAULT_PAD_CONTENT: u8 = 0xCC;

/// Default frame transmission time in nanoseconds (50 µs).
pub const CAN_ISOTP_DEFAULT_FRAME_TXTIME: u32 = 50_000;

/// Special value for `frame_txtime` meaning "zero nanoseconds" for testing.
pub const CAN_ISOTP_FRAME_TXTIME_ZERO: u32 = 0xFFFF_FFFF;

/// Socket options configuring ISO-TP protocol behavior.
///
/// Matches the Linux kernel's `struct can_isotp_options`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IsoTpOptions {
    /// Flags for ISO-TP behavior (see [`IsoTpFlags`]).
    pub flags: u32,
    /// Frame transmission time in nanoseconds (N_As / N_Ar).
    pub frame_txtime: u32,
    /// Extended address byte when [`IsoTpFlags::EXTEND_ADDR`] is set.
    pub ext_address: u8,
    /// Padding byte content for transmitted frames (default `0xCC`).
    pub txpad_content: u8,
    /// Expected padding byte content for received frames (default `0xCC`).
    pub rxpad_content: u8,
    /// Separate receive extended address when [`IsoTpFlags::RX_EXT_ADDR`] is set.
    pub rx_ext_address: u8,
}

impl Default for IsoTpOptions {
    fn default() -> Self {
        Self {
            flags: 0,
            frame_txtime: CAN_ISOTP_DEFAULT_FRAME_TXTIME,
            ext_address: 0x00,
            txpad_content: CAN_ISOTP_DEFAULT_PAD_CONTENT,
            rxpad_content: CAN_ISOTP_DEFAULT_PAD_CONTENT,
            rx_ext_address: 0x00,
        }
    }
}

impl IsoTpOptions {
    /// Creates a new `IsoTpOptions` instance with kernel defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures the flags for this options set.
    pub fn with_flags(mut self, flags: IsoTpFlags) -> Self {
        self.flags = flags.bits();
        self
    }

    /// Configures extended addressing with the specified address byte.
    pub fn with_extended_address(mut self, ext_address: u8) -> Self {
        self.flags |= IsoTpFlags::EXTEND_ADDR.bits();
        self.ext_address = ext_address;
        self
    }

    /// Configures transmit padding with the given padding byte content.
    pub fn with_tx_padding(mut self, pad_byte: u8) -> Self {
        self.flags |= IsoTpFlags::TX_PADDING.bits();
        self.txpad_content = pad_byte;
        self
    }

    /// Configures receive padding checking with the given padding byte content.
    pub fn with_rx_padding(mut self, pad_byte: u8) -> Self {
        self.flags |= IsoTpFlags::RX_PADDING.bits() | IsoTpFlags::CHK_PAD_DATA.bits();
        self.rxpad_content = pad_byte;
        self
    }
}

/// Flow control parameters for received ISO-TP messages.
///
/// Matches the Linux kernel's `struct can_isotp_fc_options`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct IsoTpFlowControlOptions {
    /// Block size (BS) provided in Flow Control frames.
    ///
    /// `0` means no block size limits (transmit entire message without further FC frames).
    pub bs: u8,
    /// Separation time (STmin) requested from the transmitter in Flow Control frames:
    /// - `0x00 - 0x7F`: 0 to 127 milliseconds.
    /// - `0xF1 - 0xF9`: 100 to 900 microseconds.
    pub stmin: u8,
    /// Maximum number of Wait Frame (FC N_PDU WT) transmissions allowed.
    ///
    /// `0` disables wait frame generation.
    pub wftmax: u8,
}

impl IsoTpFlowControlOptions {
    /// Creates a new `IsoTpFlowControlOptions` with specified block size, separation time, and wait frames.
    pub fn new(bs: u8, stmin: u8, wftmax: u8) -> Self {
        Self { bs, stmin, wftmax }
    }
}

/// Link-layer options for ISO-TP sockets (e.g. CAN FD configuration).
///
/// Matches the Linux kernel's `struct can_isotp_ll_options`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IsoTpLinkLayerOptions {
    /// Link layer MTU: `CAN_MTU` (16 bytes) for Classical CAN 2.0, or `CANFD_MTU` (72 bytes) for CAN FD.
    pub mtu: u8,
    /// Link layer data length in bytes (`8`, `12`, `16`, `20`, `24`, `32`, `48`, or `64`).
    pub tx_dl: u8,
    /// Transmission flags for CAN FD frames (e.g. Bit Rate Switch).
    pub tx_flags: u8,
}

impl Default for IsoTpLinkLayerOptions {
    fn default() -> Self {
        Self {
            mtu: CAN_MTU as u8,
            tx_dl: 8,
            tx_flags: 0,
        }
    }
}

impl IsoTpLinkLayerOptions {
    /// Creates Classical CAN 2.0 link layer options with 8-byte frames.
    pub fn classical_can() -> Self {
        Self::default()
    }

    /// Creates CAN FD link layer options with specified maximum payload length (up to 64 bytes).
    pub fn can_fd(tx_dl: u8, tx_flags: u8) -> Self {
        Self {
            mtu: CANFD_MTU as u8,
            tx_dl,
            tx_flags,
        }
    }
}

/// An ISO 15765-2 (ISO-TP) CAN transport protocol socket.
///
/// Handles transport-layer segmentation and reassembly of CAN messages up to 4095 bytes
/// (or up to 4 GiB with CAN FD) over Linux SocketCAN.
#[derive(Debug)]
pub struct CanIsoTpSocket(socket2::Socket);

impl CanIsoTpSocket {
    /// Opens an ISO-TP socket bound to the specified network interface name and CAN identifiers.
    ///
    /// # Arguments
    ///
    /// * `ifname` - Name of the CAN network interface (e.g., `"can0"`, `"vcan0"`).
    /// * `rx_id` - CAN identifier this socket listens on for incoming frames.
    /// * `tx_id` - CAN identifier this socket transmits outgoing frames with.
    pub fn open<R, T>(ifname: &str, rx_id: R, tx_id: T) -> io::Result<Self>
    where
        R: Into<Id>,
        T: Into<Id>,
    {
        let addr = CanAddr::from_iface_isotp(ifname, rx_id, tx_id)?;
        Self::open_addr(&addr)
    }

    /// Opens an ISO-TP socket bound to the given [`CanAddr`].
    pub fn open_addr(addr: &CanAddr) -> io::Result<Self> {
        let af_can = socket2::Domain::from(AF_CAN);
        let can_isotp = socket2::Protocol::from(CAN_ISOTP);

        let sock = socket2::Socket::new_raw(af_can, socket2::Type::DGRAM, Some(can_isotp))?;
        sock.bind(&SockAddr::from(*addr))?;
        Ok(Self(sock))
    }

    /// Gets a shared reference to the underlying socket object.
    pub fn as_raw_socket(&self) -> &socket2::Socket {
        &self.0
    }

    /// Gets a mutable reference to the underlying socket object.
    pub fn as_raw_socket_mut(&mut self) -> &mut socket2::Socket {
        &mut self.0
    }

    /// Sets the socket into non-blocking or blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.0.set_nonblocking(nonblocking)
    }

    /// Sets the timeout for read operations.
    pub fn set_read_timeout(&self, duration: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(duration)
    }

    /// Gets the read timeout if one was configured.
    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.0.read_timeout()
    }

    /// Sets the timeout for write operations.
    pub fn set_write_timeout(&self, duration: Option<Duration>) -> io::Result<()> {
        self.0.set_write_timeout(duration)
    }

    /// Gets the write timeout if one was configured.
    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        self.0.write_timeout()
    }

    /// Receives a complete reassembled ISO-TP PDU from the socket.
    ///
    /// Returns the number of bytes read into `buf`.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = unsafe {
            libc::recv(
                self.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }

    /// Transmits an arbitrary length payload over ISO-TP.
    ///
    /// The kernel driver segments the buffer into CAN frames, manages flow control
    /// handshakes, and returns once transmission is initiated (or completed if
    /// [`IsoTpFlags::WAIT_TX_DONE`] was set).
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        let ret = unsafe {
            libc::send(
                self.as_raw_fd(),
                buf.as_ptr().cast(),
                buf.len(),
                0,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as usize)
        }
    }

    /// Sets the ISO-TP protocol behavior options on this socket.
    pub fn set_isotp_options(&self, opts: &IsoTpOptions) -> io::Result<()> {
        let ret = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                SOL_CAN_ISOTP,
                CAN_ISOTP_OPTS,
                opts as *const _ as *const c_void,
                size_of::<IsoTpOptions>() as socklen_t,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Gets the current ISO-TP protocol behavior options from this socket.
    pub fn get_isotp_options(&self) -> io::Result<IsoTpOptions> {
        let mut opts = IsoTpOptions::default();
        let mut len = size_of::<IsoTpOptions>() as socklen_t;
        let ret = unsafe {
            libc::getsockopt(
                self.as_raw_fd(),
                SOL_CAN_ISOTP,
                CAN_ISOTP_OPTS,
                &mut opts as *mut _ as *mut c_void,
                &mut len,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(opts)
        }
    }

    /// Sets the ISO-TP receive flow control parameters on this socket.
    pub fn set_flow_control_options(&self, opts: &IsoTpFlowControlOptions) -> io::Result<()> {
        let ret = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                SOL_CAN_ISOTP,
                CAN_ISOTP_RECV_FC,
                opts as *const _ as *const c_void,
                size_of::<IsoTpFlowControlOptions>() as socklen_t,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Gets the current ISO-TP receive flow control parameters from this socket.
    pub fn get_flow_control_options(&self) -> io::Result<IsoTpFlowControlOptions> {
        let mut opts = IsoTpFlowControlOptions::default();
        let mut len = size_of::<IsoTpFlowControlOptions>() as socklen_t;
        let ret = unsafe {
            libc::getsockopt(
                self.as_raw_fd(),
                SOL_CAN_ISOTP,
                CAN_ISOTP_RECV_FC,
                &mut opts as *mut _ as *mut c_void,
                &mut len,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(opts)
        }
    }

    /// Sets the link layer parameters on this socket (e.g. for CAN FD).
    pub fn set_link_layer_options(&self, opts: &IsoTpLinkLayerOptions) -> io::Result<()> {
        let ret = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                SOL_CAN_ISOTP,
                CAN_ISOTP_LL_OPTS,
                opts as *const _ as *const c_void,
                size_of::<IsoTpLinkLayerOptions>() as socklen_t,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Gets the current link layer parameters from this socket.
    pub fn get_link_layer_options(&self) -> io::Result<IsoTpLinkLayerOptions> {
        let mut opts = IsoTpLinkLayerOptions::default();
        let mut len = size_of::<IsoTpLinkLayerOptions>() as socklen_t;
        let ret = unsafe {
            libc::getsockopt(
                self.as_raw_fd(),
                SOL_CAN_ISOTP,
                CAN_ISOTP_LL_OPTS,
                &mut opts as *mut _ as *mut c_void,
                &mut len,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(opts)
        }
    }

    /// Overrides the transmit separation time (STmin) in nanoseconds.
    pub fn set_tx_stmin(&self, stmin_ns: u32) -> io::Result<()> {
        let ret = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                SOL_CAN_ISOTP,
                CAN_ISOTP_TX_STMIN,
                &stmin_ns as *const _ as *const c_void,
                size_of::<u32>() as socklen_t,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Overrides the minimum receive separation time in nanoseconds.
    pub fn set_rx_stmin(&self, stmin_ns: u32) -> io::Result<()> {
        let ret = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                SOL_CAN_ISOTP,
                CAN_ISOTP_RX_STMIN,
                &stmin_ns as *const _ as *const c_void,
                size_of::<u32>() as socklen_t,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl SocketOptions for CanIsoTpSocket {}

impl AsRawFd for CanIsoTpSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl AsFd for CanIsoTpSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl From<OwnedFd> for CanIsoTpSocket {
    fn from(fd: OwnedFd) -> Self {
        Self(socket2::Socket::from(fd))
    }
}

impl IntoRawFd for CanIsoTpSocket {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl Read for CanIsoTpSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.recv(buf)
    }
}

impl Read for &CanIsoTpSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.recv(buf)
    }
}

impl Write for CanIsoTpSocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.send(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for &CanIsoTpSocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.send(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_isotp_struct_sizes() {
        assert_eq!(size_of::<IsoTpOptions>(), 12);
        assert_eq!(size_of::<IsoTpFlowControlOptions>(), 3);
        assert_eq!(size_of::<IsoTpLinkLayerOptions>(), 3);
    }

    #[test]
    fn test_isotp_options_builder() {
        let opts = IsoTpOptions::new()
            .with_extended_address(0x12)
            .with_tx_padding(0xAA)
            .with_rx_padding(0x55);

        assert_eq!(opts.ext_address, 0x12);
        assert_eq!(opts.txpad_content, 0xAA);
        assert_eq!(opts.rxpad_content, 0x55);
        assert_eq!(
            opts.flags,
            (IsoTpFlags::EXTEND_ADDR | IsoTpFlags::TX_PADDING | IsoTpFlags::RX_PADDING | IsoTpFlags::CHK_PAD_DATA).bits()
        );
    }

    #[test]
    fn test_isotp_socket_options_live_kernel() {
        // Test opening a raw CAN_ISOTP socket and querying default kernel options
        let af_can = socket2::Domain::from(AF_CAN);
        let can_isotp = socket2::Protocol::from(CAN_ISOTP);

        if let Ok(sock) = socket2::Socket::new_raw(af_can, socket2::Type::DGRAM, Some(can_isotp)) {
            let isotp_sock = CanIsoTpSocket(sock);

            // Verify default options
            let opts = isotp_sock.get_isotp_options().expect("getsockopt CAN_ISOTP_OPTS failed");
            assert_eq!(opts.frame_txtime, CAN_ISOTP_DEFAULT_FRAME_TXTIME);
            assert_eq!(opts.txpad_content, CAN_ISOTP_DEFAULT_PAD_CONTENT);
            assert_eq!(opts.rxpad_content, CAN_ISOTP_DEFAULT_PAD_CONTENT);

            let fc = isotp_sock.get_flow_control_options().expect("getsockopt CAN_ISOTP_RECV_FC failed");
            assert_eq!(fc.bs, 0);
            assert_eq!(fc.stmin, 0);

            let ll = isotp_sock.get_link_layer_options().expect("getsockopt CAN_ISOTP_LL_OPTS failed");
            assert_eq!(ll.mtu, CAN_MTU as u8);
            assert_eq!(ll.tx_dl, 8);

            // Test setting custom options and reading them back
            let custom_opts = IsoTpOptions::new()
                .with_extended_address(0x42)
                .with_tx_padding(0xAA);
            isotp_sock.set_isotp_options(&custom_opts).expect("setsockopt CAN_ISOTP_OPTS failed");
            let read_opts = isotp_sock.get_isotp_options().expect("getsockopt CAN_ISOTP_OPTS failed");
            assert_eq!(read_opts.ext_address, 0x42);
            assert_eq!(read_opts.txpad_content, 0xAA);

            // Test setting custom flow control options
            let custom_fc = IsoTpFlowControlOptions::new(16, 5, 3);
            isotp_sock.set_flow_control_options(&custom_fc).expect("setsockopt CAN_ISOTP_RECV_FC failed");
            let read_fc = isotp_sock.get_flow_control_options().expect("getsockopt CAN_ISOTP_RECV_FC failed");
            assert_eq!(read_fc.bs, 16);
            assert_eq!(read_fc.stmin, 5);
            assert_eq!(read_fc.wftmax, 3);

            // Test setting timeouts
            isotp_sock.set_read_timeout(Some(Duration::from_millis(250))).unwrap();
            assert_eq!(isotp_sock.read_timeout().unwrap(), Some(Duration::from_millis(250)));
            isotp_sock.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
            assert_eq!(isotp_sock.write_timeout().unwrap(), Some(Duration::from_millis(500)));

            // Test setting nonblocking mode
            isotp_sock.set_nonblocking(true).unwrap();
        }
    }

    #[test]
    fn test_isotp_open_nonexistent_interface() {
        use embedded_can::StandardId;
        let res = CanIsoTpSocket::open(
            "nonexistent_iface_999",
            StandardId::new(0x7E8).unwrap(),
            StandardId::new(0x7E0).unwrap(),
        );
        assert!(res.is_err());
    }
}
