use CanSocket;

#[test]
fn test_nonexistant_device() {
    assert!(CanSocket::open("invalid").is_err());
}


#[cfg(feature = "vcan_tests")]
mod vcan_tests {
    use {CanFrame, CanInterface, CanSocket, CanBCMSocket, ERR_MASK_ALL, ERR_MASK_NONE};
    use std::time;
    use ShouldRetry;

    #[test]
    fn vcan0_timeout() {
        let cs = CanSocket::open("vcan0").unwrap();
        cs.set_read_timeout(time::Duration::from_millis(100))
            .unwrap();
        assert!(cs.read_frame().should_retry());
    }

    #[test]
    fn vcan0_set_error_mask() {
        let cs = CanSocket::open("vcan0").unwrap();
        cs.set_error_mask(ERR_MASK_ALL).unwrap();
        cs.set_error_mask(ERR_MASK_NONE).unwrap();
    }

    #[test]
    fn vcan0_enable_own_loopback() {
        let cs = CanSocket::open("vcan0").unwrap();
        cs.set_loopback(true).unwrap();
        cs.set_recv_own_msgs(true).unwrap();

        let frame = CanFrame::new(0x123, &[], true, false).unwrap();

        cs.write_frame(&frame).unwrap();

        cs.read_frame().unwrap();
    }

    #[test]
    fn vcan0_set_down() {
        let can_if = CanInterface::open("vcan0").unwrap();
        can_if.bring_down().unwrap();
    }

    #[test]
    fn vcan0_test_nonblocking() {
        let cs = CanSocket::open("vcan0").unwrap();
        cs.set_nonblocking(true);

        // no timeout set, but should return immediately
        assert!(cs.read_frame().should_retry());
    }

    #[test]
    fn vcan0_bcm_filter() {
        let cbs = CanBCMSocket::open("vcan0").unwrap();
        let ival = time::Duration::from_millis(1);
        cbs.filter_id(0x123, ival, ival).unwrap();

        let cs = CanSocket::open("vcan0").unwrap();
        let frame = CanFrame::new(0x123, &[], true, false).unwrap();
        cs.write_frame(&frame).unwrap();

        // TODO this currently blocks the tests and requires a manual
        // cansend vcan0 123#1122334455667788
        let msghead = cbs.read_frames().unwrap();
        assert!(msghead.frames()[0].id() == 0x123);
    }

    #[test]
    fn vcan0_bcm_filter_delete() {
        let cbs = CanBCMSocket::open("vcan0").unwrap();
        let ival = time::Duration::from_millis(1);
        cbs.filter_id(0x123, ival, ival).unwrap();

        cbs.filter_delete(0x123).unwrap();
    }

    #[test]
    fn vcan0_bcm_filter_delete_err() {
        let cbs = CanBCMSocket::open("vcan0").unwrap();
        assert!(cbs.filter_delete(0x124).is_err())
    }

}
