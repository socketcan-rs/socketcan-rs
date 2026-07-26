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
    CanError, CanErrorFrame, CanFrame, CanSocket, EmbeddedFrame, SOF_TIMESTAMPING_OPT_CMSG,
    SOF_TIMESTAMPING_RX_SOFTWARE, SOF_TIMESTAMPING_SOFTWARE, ShouldRetry, Socket, SocketOptions,
    StandardId,
    errors::{
        CAN_ERR_ACK, CAN_ERR_BUSERROR, CAN_ERR_CNT, CAN_ERR_CRTL, CAN_ERR_PROT, ControllerProblem,
        Location, ViolationType,
    },
    id::{ERR_MASK_ALL, ERR_MASK_NONE},
};

#[cfg(feature = "vcan_tests")]
use serial_test::serial;

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
#[serial]
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
#[serial]
fn vcan_set_error_mask() {
    let sock = CanSocket::open(VCAN).unwrap();
    sock.set_error_mask(ERR_MASK_ALL).unwrap();
    sock.set_error_mask(ERR_MASK_NONE).unwrap();
}

/// Round-trips a multi-bit error frame through the kernel and checks that
/// every condition it describes survives.
///
/// The frame mirrors what `mcp251xfd_handle_ivmif()` emits: three error
/// classes at once, with five protocol violation bits packed into `data[2]`
/// and one location in `data[3]`. Before the v4 errors rework this decoded
/// to a single `CanError::Unknown(0xA8)`, discarding everything.
#[test]
#[cfg(feature = "vcan_tests")]
#[serial]
fn vcan_multi_bit_error_frame_round_trip() {
    let sock = CanSocket::open(VCAN).unwrap();
    sock.set_loopback(true).unwrap();
    sock.set_recv_own_msgs(true).unwrap();
    sock.set_error_mask(ERR_MASK_ALL).unwrap();

    // data[2] = STUFF|FORM|TX|BIT1|BIT0, data[3] = CRC sequence.
    let bits = CAN_ERR_PROT | CAN_ERR_BUSERROR | CAN_ERR_ACK;
    let data = [0u8, 0, 0x9E, 0x08, 0, 0, 0, 0];
    let frame = CanErrorFrame::new_error(bits, &data).unwrap();

    sock.write_frame(&frame).unwrap();

    // The receive path converts an error frame into an Err(Error::Can(..)),
    // so read the raw frame back instead and decode it explicitly.
    let echoed = sock.read_frame().unwrap();
    let echoed = match echoed {
        CanFrame::Error(f) => f,
        other => panic!("expected an error frame, got {:?}", other),
    };
    assert_eq!(echoed.error_bits(), bits);

    let errs = echoed.into_errors();
    assert_eq!(errs.len(), 7, "decoded: {}", errs);

    let all: Vec<CanError> = errs.iter().copied().collect();
    let violations: Vec<ViolationType> = all
        .iter()
        .filter_map(|e| match e {
            CanError::ProtocolViolation { vtype, location } => {
                assert_eq!(*location, Location::CrcSequence);
                Some(*vtype)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        violations,
        vec![
            ViolationType::FrameFormatError,
            ViolationType::BitStuffingError,
            ViolationType::UnableToSendDominantBit,
            ViolationType::UnableToSendRecessiveBit,
            ViolationType::TransmissionError,
        ]
    );
    assert!(all.contains(&CanError::NoAck));
    assert!(all.contains(&CanError::BusError));

    // And it re-encodes to the exact bytes the kernel handed us.
    assert_eq!(CanErrorFrame::from(errs), echoed);
}

/// Checks the `CAN_ERR_CRTL | CAN_ERR_CNT` frame that accompanies every
/// controller state change, with both TX and RX warning bits set in
/// `data[1]` the way the kernel's shared `can_change_state()` does.
#[test]
#[cfg(feature = "vcan_tests")]
#[serial]
fn vcan_controller_state_change_error_frame() {
    let sock = CanSocket::open(VCAN).unwrap();
    sock.set_loopback(true).unwrap();
    sock.set_recv_own_msgs(true).unwrap();
    sock.set_error_mask(ERR_MASK_ALL).unwrap();

    let bits = CAN_ERR_CRTL | CAN_ERR_CNT;
    let data = [0u8, 0x0C, 0, 0, 0, 0, 112, 96];
    sock.write_frame(&CanErrorFrame::new_error(bits, &data).unwrap())
        .unwrap();

    let echoed = match sock.read_frame().unwrap() {
        CanFrame::Error(f) => f,
        other => panic!("expected an error frame, got {:?}", other),
    };

    let errs = echoed.into_errors();
    let all: Vec<CanError> = errs.iter().copied().collect();
    assert_eq!(
        all,
        vec![
            CanError::ControllerProblem(ControllerProblem::ReceiveErrorWarning),
            CanError::ControllerProblem(ControllerProblem::TransmitErrorWarning),
            CanError::Counters { tx: 112, rx: 96 },
        ],
        "decoded: {}",
        errs
    );
}

#[test]
#[cfg(feature = "vcan_tests")]
#[serial]
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
#[serial]
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
#[serial]
fn vcan_has_hw_timestamps_returns_false() {
    // vcan is a software-only driver, so it must never claim HW timestamp
    // support — and the query must not panic on an unbound/SW interface.
    let sock = CanSocket::open(VCAN).unwrap();
    assert!(!sock.has_hw_timestamps());
}

#[test]
#[cfg(feature = "vcan_tests")]
#[serial]
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
#[serial]
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
