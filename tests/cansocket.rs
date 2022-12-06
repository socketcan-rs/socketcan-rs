use socketcan::{constants::*, CanFdSocket, CanFrame, CanNormalSocket, CanSocket, ShouldRetry};
use std::time;

#[test]
fn test_nonexistant_device() {
    assert!(CanNormalSocket::open("invalid").is_err());
}

#[test]
fn vcan0_timeout() {
    let cs = CanNormalSocket::open("vcan0").unwrap();
    cs.set_read_timeout(time::Duration::from_millis(100))
        .unwrap();
    assert!(cs.read_frame().should_retry());
}

#[test]
fn vcan0_set_error_mask() {
    let cs = CanNormalSocket::open("vcan0").unwrap();
    cs.set_error_mask(ERR_MASK_ALL).unwrap();
    cs.set_error_mask(ERR_MASK_NONE).unwrap();
}

#[test]
fn vcan0_enable_own_loopback() {
    let cs = CanNormalSocket::open("vcan0").unwrap();
    cs.set_loopback(true).unwrap();
    cs.set_recv_own_msgs(true).unwrap();

    let frame = CanFrame::init(0x123, &[], true, false).unwrap();

    cs.write_frame(&frame).unwrap();

    cs.read_frame().unwrap();
}

// #[test]
// fn vcan0_set_down() {
//     let can_if = CanInterface::open("vcan0").unwrap();
//     can_if.bring_down().unwrap();
// }

#[test]
fn vcan0_test_nonblocking() {
    let cs = CanSocket::open("vcan0").unwrap();
    cs.set_nonblocking(true).unwrap();

    // no timeout set, but should return immediately
    assert!(cs.read_frame().should_retry());
}

#[test]
#[cfg(feature = "vcan_tests")]
fn vcan0_test_fd() {
    let cs = CanFdSocket::open("vcan0").unwrap();
    for _ in 0..3 {
        let frame = cs.read_frame().unwrap();
        println!("Received frame: {:X}", frame);
        cs.write_frame(&frame).unwrap();
    }
}
