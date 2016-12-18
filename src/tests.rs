use ::CanSocket;

#[test]
fn test_nonexistant_device() {
    assert!(CanSocket::open("invalid").is_err());
}


#[cfg(feature = "vcan_tests")]
mod vcan_tests {
    use ::{CanSocket, ERR_MASK_ALL, ERR_MASK_NONE};
    use std::time;
    use ::ShouldRetry;

    #[test]
    fn vcan0_timeout() {
        let cs = CanSocket::open("vcan0").unwrap();
        cs.set_read_timeout(time::Duration::from_millis(100)).unwrap();
        assert!(cs.read_frame().should_retry());
    }

    #[test]
    fn vcan0_set_error_mask() {
        let cs = CanSocket::open("vcan0").unwrap();
        cs.set_error_mask(ERR_MASK_ALL).unwrap();
        cs.set_error_mask(ERR_MASK_NONE).unwrap();
    }
}
