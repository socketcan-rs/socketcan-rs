use std::time;
use ::{CANSocket, ShouldRetry};

#[test]
fn test_nonexistant_device() {
    assert!(CANSocket::open("invalid").is_err());
}

#[test]
fn vcan0_timeout() {
    let cs = CANSocket::open("vcan1").unwrap();
    cs.set_read_timeout(time::Duration::from_millis(100)).unwrap();
    assert!(cs.read_frame().should_retry());
}
