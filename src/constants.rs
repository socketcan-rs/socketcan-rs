//
// constants.rs
//
// @author Natesh Narain <nnaraindev@gmail.com>
// @date Jul 06 2022
//
use std::{
    os::raw::c_int,
};

// constants stolen from C headers
pub const AF_CAN: c_int = 29;
pub const PF_CAN: c_int = 29;
pub const CAN_RAW: c_int = 1;
pub const SOL_CAN_BASE: c_int = 100;
pub const SOL_CAN_RAW: c_int = SOL_CAN_BASE + CAN_RAW;
pub const CAN_RAW_FILTER: c_int = 1;
pub const CAN_RAW_ERR_FILTER: c_int = 2;
pub const CAN_RAW_LOOPBACK: c_int = 3;
pub const CAN_RAW_RECV_OWN_MSGS: c_int = 4;
// unused:
// const CAN_RAW_FD_FRAMES: c_int = 5;
pub const CAN_RAW_JOIN_FILTERS: c_int = 6;


// get timestamp in a struct timeval (us accuracy)
// const SIOCGSTAMP: c_int = 0x8906;

// get timestamp in a struct timespec (ns accuracy)
pub const SIOCGSTAMPNS: c_int = 0x8907;

/// if set, indicate 29 bit extended format
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


/// an error mask that will cause SocketCAN to report all errors
pub const ERR_MASK_ALL: u32 = ERR_MASK;

/// an error mask that will cause SocketCAN to silently drop all errors
pub const ERR_MASK_NONE: u32 = 0;
