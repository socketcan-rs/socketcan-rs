// socketcan/tests/cansocket.rs
//
// Integration tests for CAN sockets.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

#[cfg(feature = "vcan_tests")]
use socketcan::{
    frame::{ERR_MASK_ALL, ERR_MASK_NONE},
    CanFrame, CanSocket, EmbeddedFrame, ShouldRetry, Socket, SocketOptions, StandardId,
};

#[cfg(feature = "vcan_tests")]
use std::time;

// The virtual CAN interface to use for tests.
#[cfg(feature = "vcan_tests")]
const VCAN: &str = "vcan0";

#[cfg(feature = "vcan_tests")]
#[test]
fn test_nonexistant_device() {
    assert!(CanSocket::open("invalid").is_err());
}

#[test]
#[cfg(feature = "vcan_tests")]
fn vcan_timeout() {
    let sock = CanSocket::open(VCAN).unwrap();
    // Filter out _any_ traffic
    sock.set_filter_drop_all().unwrap();
    sock.set_read_timeout(time::Duration::from_millis(100))
        .unwrap();

    assert!(sock.read_frame().should_retry());
}

#[test]
#[cfg(feature = "vcan_tests")]
fn vcan_set_error_mask() {
    let sock = CanSocket::open(VCAN).unwrap();
    sock.set_error_mask(ERR_MASK_ALL).unwrap();
    sock.set_error_mask(ERR_MASK_NONE).unwrap();
}

#[test]
#[cfg(feature = "vcan_tests")]
fn vcan_enable_own_loopback() {
    let sock = CanSocket::open(VCAN).unwrap();
    sock.set_loopback(true).unwrap();
    sock.set_recv_own_msgs(true).unwrap();

    let id = StandardId::new(0x123).unwrap();
    let frame = CanFrame::new_remote(id, 0).unwrap();

    sock.write_frame(&frame).unwrap();
    sock.read_frame().unwrap();
}

// #[test]
// fn vcan_set_down() {
//     let can_if = CanInterface::open(VCAN).unwrap();
//     can_if.bring_down().unwrap();
// }

#[test]
#[cfg(feature = "vcan_tests")]
fn vcan_test_nonblocking() {
    let sock = CanSocket::open(VCAN).unwrap();
    // Filter out _any_ traffic
    sock.set_filter_drop_all().unwrap();
    sock.set_nonblocking(true).unwrap();

    // no timeout set, but should return immediately
    assert!(sock.read_frame().should_retry());
}

/*
#[test]
#[cfg(feature = "vcan_tests")]
fn vcan_test_fd() {
    let sock = CanFdSocket::open(VCAN).unwrap();
    for _ in 0..3 {
        let frame = sock.read_frame().unwrap();
        println!("Received frame: {:X}", frame);
        sock.write_frame(&frame).unwrap();
    }
}
*/
