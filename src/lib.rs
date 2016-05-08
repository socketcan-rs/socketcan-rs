//! socketCAN support.
//!
//! The Linux kernel supports using CAN-devices using a network-like API
//! (see https://www.kernel.org/doc/Documentation/networking/can.txt). This
//! crate allows easy access to this functionality without having to wrestle
//! libc calls.

extern crate itertools;
extern crate libc;
extern crate nix;

use libc::{c_int, c_short, c_void, c_uint, socket, SOCK_RAW, close, bind,
           sockaddr, read, write};
use itertools::Itertools;
use std::{io, error, fmt};
use std::mem::size_of;
use nix::net::if_::if_nametoindex;

// constants stolen from C headers
const AF_CAN: c_int = 29;
const PF_CAN: c_int = 29;
const CAN_RAW: c_int = 1;

/// if set, uses 29 bit extended format
pub const EFF_FLAG: u32 = 0x80000000;

/// remote transmission request flag
pub const RTR_FLAG: u32 = 0x40000000;

/// error flag
pub const ERR_FLAG: u32 = 0x20000000;

/// valid bits in standard frame id
pub const SFF_MASK: u32 = 0x000007ff;

/// valid bits in extended frame id
pub const EFF_MASK: u32 = 0x1fffffff;

/// valid bits in error frame
pub const ERR_MASK: u32 = 0x1fffffff;

#[derive(Debug)]
#[repr(C)]
struct CANAddr {
    _af_can: c_short,
    if_index: c_int,  // address familiy,
    rx_id: u32,
    tx_id: u32,
}

#[derive(Debug)]
pub enum CANSocketOpenError {
    /// Device could not be found
    LookupError(nix::Error),

    /// System error while trying to look up device name
    IOError(io::Error),
}

impl fmt::Display for CANSocketOpenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CANSocketOpenError::LookupError(ref e) =>
                write!(f, "CAN Device not found: {}", e),
            CANSocketOpenError::IOError(ref e) =>
                write!(f, "IO: {}", e),
        }
    }
}

impl error::Error for CANSocketOpenError {
    fn description(&self) -> &str {
        match *self {
            CANSocketOpenError::LookupError(_)
                => "can device not found",
            CANSocketOpenError::IOError(ref e)
                => e.description(),
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            CANSocketOpenError::LookupError(ref e) => Some(e),
            CANSocketOpenError::IOError(ref e) => Some(e),
        }
    }
}


#[derive(Debug)]
pub enum ConstructionError {
    IDTooLarge,
    TooMuchData,
}

impl fmt::Display for ConstructionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ConstructionError::IDTooLarge
                => write!(f, "CAN ID too large"),
            ConstructionError::TooMuchData
                => write!(f, "Payload is larger than CAN maximum of 8 bytes")
        }
    }
}

impl error::Error for ConstructionError {
    fn description(&self) -> &str {
        match *self {
            ConstructionError::IDTooLarge => "can id too large",
            ConstructionError::TooMuchData => "too much data",
        }
    }
}

impl From<nix::Error> for CANSocketOpenError {
    fn from(e: nix::Error) -> CANSocketOpenError {
        CANSocketOpenError::LookupError(e)
    }
}

impl From<io::Error> for CANSocketOpenError {
    fn from(e: io::Error) -> CANSocketOpenError {
        CANSocketOpenError::IOError(e)
    }
}

/// A socket for a CAN device. Just a wrapped file descriptor.
///
/// Will be closed upon deallocation. To close manually, use std::drop::Drop.
pub struct CANSocket {
    fd: c_int,
}

impl CANSocket {
    /// Open a named CAN device.
    ///
    /// Usually the more common case, opens a socket can device by name, such
    /// as "vcan0" or "socan0".
    pub fn open(ifname: &str) -> Result<CANSocket, CANSocketOpenError> {
        let if_index = try!(if_nametoindex(ifname));
        CANSocket::open_if(if_index)
    }

    /// Open CAN device by interface number.
    ///
    /// Opens a CAN device by kernel interface number.
    pub fn open_if(if_index: c_uint) -> Result<CANSocket, CANSocketOpenError> {
        let addr = CANAddr{
            _af_can: AF_CAN as c_short,
            if_index: if_index as c_int,
            rx_id: 0, // ?
            tx_id: 0, // ?
        };

        // open socket
        let sock_fd;
        unsafe {
            sock_fd = socket(PF_CAN, SOCK_RAW, CAN_RAW);
        }

        if sock_fd == -1 {
            return Err(CANSocketOpenError::from(io::Error::last_os_error()));
        }

        // bind it
        let bind_rv;
        unsafe {
            let sockaddr_ptr = &addr as *const CANAddr;
            bind_rv = bind(sock_fd, sockaddr_ptr as *const sockaddr,
                size_of::<CANAddr> () as u32);
        }

        if bind_rv == -1 {
            let e = io::Error::last_os_error();
            unsafe {
                close(sock_fd);
            }
            return Err(CANSocketOpenError::from(e))
        }

        Ok(CANSocket{fd: sock_fd})
    }

    fn close(&mut self) -> io::Result<()> {
        unsafe {
            let rv = close(self.fd);
            if rv != -1 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Blocking read a single can frame.
    pub fn read_frame(&self) -> io::Result<CANFrame> {
        let mut frame = CANFrame{
            _id: 0,
            _data_len: 0,
            _pad: 0,
            _res0: 0,
            _res1: 0,
            _data: [0; 8],
        };

        let read_rv = unsafe {
            let frame_ptr = &mut frame as *mut CANFrame;
            read(self.fd, frame_ptr as *mut c_void, size_of::<CANFrame>())
        };

        if read_rv as usize != size_of::<CANFrame>() {
            return Err(io::Error::last_os_error());
        }

        Ok(frame)
    }

    pub fn write_frame(&self, frame: &CANFrame) -> io::Result<()> {
        // not a mutable reference needed (see std::net::UdpSocket) for
        // a comparison
        // debug!("Sending: {:?}", frame);

        let write_rv = unsafe {
            let frame_ptr = frame as *const CANFrame;
            write(self.fd, frame_ptr as *const c_void, size_of::<CANFrame>())
        };

        if write_rv as usize != size_of::<CANFrame>() {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }
}

impl Drop for CANSocket {
    fn drop(&mut self) {
        self.close().ok();  // ignore result
    }
}

/// CANFrame
///
/// Same memory layout as the underlying kernel struct.
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct CANFrame {
    /// 32 bit CAN_ID + EFF/RTR/ERR flags
    pub _id: u32,

    /// data length. Bytes beyond are not valid
    _data_len: u8,

    /// padding
    _pad: u8,

    /// reserved
    _res0: u8,

    /// reserved
    _res1: u8,

    /// buffer for data
    _data: [u8; 8],
}

impl CANFrame {
    pub fn new(id: u32, data: &[u8], rtr: bool, err: bool) -> Result<CANFrame,
    ConstructionError> {
        let mut _id = id;

        if data.len() > 8 {
            return Err(ConstructionError::TooMuchData)
        }

        if id > EFF_MASK {
            return Err(ConstructionError::IDTooLarge)
        }

        // set EFF_FLAG on large message
        if id > SFF_MASK {
            _id |= EFF_FLAG;
        }

        if rtr {
            _id |= RTR_FLAG;
        }

        if err {
            _id |= ERR_FLAG;
        }

        let mut full_data = [0; 8];

        // not cool =/
        for (n, c) in data.iter().enumerate() {
            full_data[n] = *c;
        }

        Ok(CANFrame{
            _id: _id,
            _data_len: data.len() as u8,
            _pad: 0,
            _res0: 0,
            _res1: 0,
            _data: full_data,
        })
    }

    /// Return the actual CAN ID (without EFF/RTR/ERR flags)
    pub fn id(&self) -> u32 {
        if self.is_extended() {
            self._id & EFF_MASK
        } else {
            self._id & SFF_MASK
        }
    }

    /// Return the error message
    pub fn err(&self) -> u32 {
        return self._id & ERR_MASK
    }

    /// Check if frame uses 29 bit extended frame format
    pub fn is_extended(&self) -> bool {
        self._id & EFF_FLAG != 0
    }

    /// Check if frame is an error message
    pub fn is_error(&self) -> bool {
        self._id & ERR_FLAG != 0
    }

    /// Check if frame is a remote transmission request
    pub fn is_rtr(&self) -> bool {
        self._id & RTR_FLAG != 0
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    pub fn data(&self) -> &[u8] {
        &self._data[..(self._data_len as usize)]
    }
}

impl fmt::UpperHex for CANFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        try!(write!(f, "{:X}#", self.id()));

        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));

        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}
