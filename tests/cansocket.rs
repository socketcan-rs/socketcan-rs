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
    id::{ERR_MASK_ALL, ERR_MASK_NONE},
    CanFrame, CanSocket, EmbeddedFrame, ShouldRetry, Socket, SocketOptions, StandardId,
    SOF_TIMESTAMPING_OPT_CMSG, SOF_TIMESTAMPING_RX_SOFTWARE, SOF_TIMESTAMPING_SOFTWARE,
};

#[cfg(feature = "vcan_tests")]
use std::time::{self, SystemTime};

// The virtual CAN interface to use for tests.
#[cfg(feature = "vcan_tests")]
const VCAN: &str = "vcan0";

#[cfg(feature = "vcan_tests")]
#[test]
fn test_nonexistent_device() {
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

#[test]
#[cfg(feature = "vcan_tests")]
fn vcan_has_hw_timestamps_returns_false() {
    // vcan is a software-only driver, so it must never claim HW timestamp
    // support — and the query must not panic on an unbound/SW interface.
    let sock = CanSocket::open(VCAN).unwrap();
    assert!(!sock.has_hw_timestamps());
}

#[test]
#[cfg(feature = "vcan_tests")]
fn vcan_read_frame_with_timestamp() {
    let sock = CanSocket::open(VCAN).unwrap();
    sock.set_loopback(true).unwrap();
    sock.set_recv_own_msgs(true).unwrap();
    sock.set_recv_timestamp(true).unwrap();

    let id = StandardId::new(0x321).unwrap();
    let frame = CanFrame::new(id, &[0xAA, 0xBB]).unwrap();
    let sent_at = SystemTime::now();
    sock.write_frame(&frame).unwrap();

    let (rx, ts) = sock.read_frame_with_timestamp().unwrap();
    assert_eq!(rx.data(), frame.data());

    // Socket-layer timestamp should land within a couple of seconds of "now".
    let delta = ts
        .duration_since(sent_at)
        .or_else(|e| Ok::<_, std::time::SystemTimeError>(e.duration()))
        .unwrap();
    assert!(
        delta < time::Duration::from_secs(2),
        "timestamp out of expected range: {delta:?}"
    );
}

#[test]
#[cfg(feature = "vcan_tests")]
fn vcan_read_frame_with_timestamps_populates_sw() {
    let sock = CanSocket::open(VCAN).unwrap();
    sock.set_loopback(true).unwrap();
    sock.set_recv_own_msgs(true).unwrap();
    sock.set_recv_timestamp(true).unwrap();
    sock.set_timestamping(
        SOF_TIMESTAMPING_RX_SOFTWARE | SOF_TIMESTAMPING_SOFTWARE | SOF_TIMESTAMPING_OPT_CMSG,
    )
    .unwrap();

    let id = StandardId::new(0x456).unwrap();
    let frame = CanFrame::new(id, &[0x11, 0x22, 0x33]).unwrap();
    sock.write_frame(&frame).unwrap();

    let (_rx, ts) = sock.read_frame_with_timestamps().unwrap();
    assert!(ts.socket.is_some(), "SO_TIMESTAMPNS not delivered");
    assert!(ts.sw.is_some(), "RX_SOFTWARE not delivered");
    // vcan has no hardware clock; ts.hw should be None.
    assert!(ts.hw.is_none(), "vcan should not report a hw timestamp");
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
