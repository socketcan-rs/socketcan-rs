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
    CanErrorFrame, CanFrame, CanSocket, EmbeddedFrame, ErrorCause, ShouldRetry, Socket,
    SocketOptions, StandardId,
    errors::{
        CAN_ERR_ACK, CAN_ERR_BUSERROR, CAN_ERR_CNT, CAN_ERR_CRTL, CAN_ERR_PROT, ControllerProblems,
        Location, ViolationTypes,
    },
    id::{ERR_MASK_ALL, ERR_MASK_NONE},
    timestamp::{
        SOF_TIMESTAMPING_OPT_CMSG, SOF_TIMESTAMPING_RX_SOFTWARE, SOF_TIMESTAMPING_SOFTWARE,
    },
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

/// A downstream crate can open a socket for another CAN protocol and bind it
/// with our address type, using only this crate's re-exports.
///
/// This is the path the `nl`- and frame-shaped sockets here deliberately do
/// not cover: `CAN_J1939` and `CAN_ISOTP` sockets carry reassembled payloads,
/// so a crate implementing them brings its own socket type and needs from us
/// only the protocol number, the address, and the option plumbing.
#[test]
#[cfg(feature = "vcan_tests")]
#[serial]
fn vcan_other_protocols_bind_with_our_addr() {
    use socket2::{Domain, Protocol, Socket as Sock2, Type};
    use socketcan::{
        CanAddr,
        addr::{AF_CAN, J1939_NO_ADDR, J1939_NO_NAME, J1939_NO_PGN},
        socket::{CAN_ISOTP, CAN_J1939},
    };

    // By name, and with no casts: each constant is typed for the parameter it
    // belongs to.
    let j1939 =
        CanAddr::from_iface_j1939(VCAN, J1939_NO_NAME, J1939_NO_PGN, J1939_NO_ADDR).unwrap();
    let isotp = CanAddr::from_iface_isotp(
        VCAN,
        StandardId::new(0x123).unwrap(),
        StandardId::new(0x321).unwrap(),
    )
    .unwrap();

    for (proto, addr) in [(CAN_J1939, j1939), (CAN_ISOTP, isotp)] {
        let sock = Sock2::new_raw(
            Domain::from(AF_CAN),
            Type::DGRAM,
            Some(Protocol::from(proto)),
        )
        .unwrap();
        sock.bind(&addr.into_sock_addr()).unwrap();
    }

    // The J1939 sentinels are the "unset" markers, not zero.
    assert_eq!(j1939.j1939_pgn(), J1939_NO_PGN);
    assert_eq!(j1939.j1939_addr(), J1939_NO_ADDR);
    assert_eq!(isotp.tp_rx_id(), 0x123);
}

/// A real CAN controller reports which control modes it supports.
///
/// `IFLA_CAN_CTRLMODE_EXT` needs hardware: a `vcan` sends no CAN link
/// information at all, so nothing in the rest of the suite exercises the
/// nested-attribute path. Point `SOCKETCAN_FD_IFACE` at an FD-capable
/// interface to run it — the interface may be down, and needs no bus:
///
/// ```text
/// SOCKETCAN_FD_IFACE=can0 cargo test --features netlink -- --nocapture
/// ```
#[test]
#[cfg(feature = "netlink")]
fn hw_supported_ctrlmodes() {
    use socketcan::{CanCtrlMode, CanInterface};

    let Ok(name) = std::env::var("SOCKETCAN_FD_IFACE") else {
        eprintln!("skipped: set SOCKETCAN_FD_IFACE to an FD-capable interface");
        return;
    };

    let iface = CanInterface::open(&name).expect("interface should exist");
    let supported = iface
        .supported_ctrlmodes()
        .expect("query should succeed")
        .expect("an FD controller reports IFLA_CAN_CTRLMODE_EXT");

    let named = [
        (CanCtrlMode::Loopback, "LOOPBACK"),
        (CanCtrlMode::ListenOnly, "LISTENONLY"),
        (CanCtrlMode::TripleSampling, "3_SAMPLES"),
        (CanCtrlMode::OneShot, "ONE_SHOT"),
        (CanCtrlMode::BerrReporting, "BERR_REPORTING"),
        (CanCtrlMode::Fd, "FD"),
        (CanCtrlMode::PresumeAck, "PRESUME_ACK"),
        (CanCtrlMode::NonIso, "FD_NON_ISO"),
        (CanCtrlMode::CcLen8Dlc, "CC_LEN8_DLC"),
    ];
    let list: Vec<&str> = named
        .iter()
        .filter(|(m, _)| supported & m.mask() != 0)
        .map(|(_, n)| *n)
        .collect();
    eprintln!("{name}: supported = {supported:#06X} {list:?}");

    // The interface was named as FD-capable, so that bit must be set. This
    // also proves the nested attribute was decoded rather than skipped: the
    // kernel sends its type with NLA_F_NESTED set.
    assert!(
        supported & CanCtrlMode::Fd.mask() != 0,
        "{name} reports {supported:#06X}, without CAN_CTRLMODE_FD"
    );

    // The same mask reaches the batch reader and the full details.
    assert_eq!(
        iface.can_params().unwrap().ctrl_mode_supported,
        Some(supported)
    );
    assert_eq!(
        iface.details().unwrap().can.ctrl_mode_supported,
        Some(supported)
    );
}

/// Every CAN option getter reports what its setter just wrote.
///
/// The kernel is the only authority on what a socket actually holds, so these
/// go through it rather than caching anything on the Rust side.
#[test]
#[cfg(feature = "vcan_tests")]
#[serial]
fn vcan_option_getters_mirror_setters() {
    let sock = CanSocket::open(VCAN).unwrap();

    for on in [true, false] {
        sock.set_loopback(on).unwrap();
        assert_eq!(sock.loopback().unwrap(), on, "loopback {on}");

        sock.set_recv_own_msgs(on).unwrap();
        assert_eq!(sock.recv_own_msgs().unwrap(), on, "recv_own_msgs {on}");

        sock.set_join_filters(on).unwrap();
        assert_eq!(sock.join_filters().unwrap(), on, "join_filters {on}");
    }

    for mask in [ERR_MASK_ALL, ERR_MASK_NONE, 0x0000_0004] {
        sock.set_error_filter(mask).unwrap();
        assert_eq!(sock.error_filter().unwrap(), mask, "mask {mask:#x}");
        // The alias pair reads the same option.
        assert_eq!(sock.error_mask().unwrap(), mask);
    }

    // Defaults, as the kernel sets them for a fresh raw socket: loopback on,
    // own messages off, no error frames.
    let fresh = CanSocket::open(VCAN).unwrap();
    assert!(fresh.loopback().unwrap());
    assert!(!fresh.recv_own_msgs().unwrap());
    assert_eq!(fresh.error_filter().unwrap(), ERR_MASK_NONE);
}

/// The three setters are three spellings of one `setsockopt()` call, and the
/// empty-buffer case still clears an option rather than sending a dangling
/// pointer.
///
/// `set_socket_option()` and `set_socket_option_mult()` delegate to
/// `set_socket_option_bytes()`, so this checks the delegation end to end
/// against what the kernel actually holds afterwards.
#[test]
#[cfg(feature = "vcan_tests")]
#[serial]
fn vcan_socket_option_setters_agree() {
    use socketcan::socket::{CAN_RAW_ERR_FILTER, CAN_RAW_FILTER, CAN_RAW_LOOPBACK, SOL_CAN_RAW};

    let sock = CanSocket::open(VCAN).unwrap();

    // Typed, and as raw bytes: the same option, the same result.
    //
    // SAFETY: `c_int` has no padding, so every byte of the value is
    // initialised — the contract `set_socket_option()` asks of its caller.
    unsafe { sock.set_socket_option(SOL_CAN_RAW, CAN_RAW_LOOPBACK, &0i32) }.unwrap();
    assert_eq!(
        sock.get_socket_option_int(SOL_CAN_RAW, CAN_RAW_LOOPBACK)
            .unwrap(),
        0
    );

    sock.set_socket_option_bytes(SOL_CAN_RAW, CAN_RAW_LOOPBACK, &1i32.to_ne_bytes())
        .unwrap();
    assert_eq!(
        sock.get_socket_option_int(SOL_CAN_RAW, CAN_RAW_LOOPBACK)
            .unwrap(),
        1
    );

    // And the safe scalar setter, which is what the crate's own wrappers use.
    sock.set_socket_option_int(SOL_CAN_RAW, CAN_RAW_LOOPBACK, 0)
        .unwrap();
    assert_eq!(
        sock.get_socket_option_int(SOL_CAN_RAW, CAN_RAW_LOOPBACK)
            .unwrap(),
        0
    );

    // A slice of values, read back through the byte getter.
    sock.set_error_mask(ERR_MASK_ALL).unwrap();
    let mut buf = [0u8; 4];
    sock.get_socket_option_bytes(SOL_CAN_RAW, CAN_RAW_ERR_FILTER, &mut buf)
        .unwrap();
    assert_eq!(u32::from_ne_bytes(buf), ERR_MASK_ALL);

    // Both empty cases must succeed: the primitive sends a null pointer with
    // length 0 for them, which is how a filter list is cleared.
    sock.set_filter_drop_all().unwrap();
    sock.set_socket_option_bytes(SOL_CAN_RAW, CAN_RAW_FILTER, &[])
        .unwrap();

    // ... and the socket still works afterwards: filters accept frames again.
    sock.set_filter_accept_all().unwrap();
}

/// A CAN socket option round-trips through the setter and the new getter.
///
/// This is the shape a crate implementing another CAN protocol needs: set an
/// option, then read back what the kernel actually holds.
#[test]
#[cfg(feature = "vcan_tests")]
#[serial]
fn vcan_socket_option_round_trip() {
    use socketcan::socket::{CAN_RAW_LOOPBACK, CAN_RAW_RECV_OWN_MSGS, SOL_CAN_RAW};

    let sock = CanSocket::open(VCAN).unwrap();

    for (name, opt) in [
        ("CAN_RAW_LOOPBACK", CAN_RAW_LOOPBACK),
        ("CAN_RAW_RECV_OWN_MSGS", CAN_RAW_RECV_OWN_MSGS),
    ] {
        for on in [true, false] {
            match opt {
                CAN_RAW_LOOPBACK => sock.set_loopback(on).unwrap(),
                _ => sock.set_recv_own_msgs(on).unwrap(),
            }
            let got = sock.get_socket_option_int(SOL_CAN_RAW, opt).unwrap();
            assert_eq!(got, i32::from(on), "{name} set to {on}");
        }
    }

    // The error mask is a 32-bit value, read here through the byte form.
    sock.set_error_mask(ERR_MASK_ALL).unwrap();
    let mut buf = [0u8; 4];
    let n = sock
        .get_socket_option_bytes(SOL_CAN_RAW, socketcan::socket::CAN_RAW_ERR_FILTER, &mut buf)
        .unwrap();
    assert_eq!(n, 4);
    assert_eq!(u32::from_ne_bytes(buf), ERR_MASK_ALL);
}

/// Round-trips a multi-bit error frame through the kernel and checks that
/// every condition it describes survives.
///
/// The frame mirrors what `mcp251xfd_handle_ivmif()` emits: three error
/// classes at once, with five protocol violation bits packed into `data[2]`
/// and one location in `data[3]`. Before the v4 errors rework this decoded
/// to a single `Unknown(0xA8)`, discarding everything.
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

    let err = echoed.into_error();
    // The five protocol violations fold into one Protocol cause, so the frame
    // decodes to Protocol + NoAck + BusError.
    assert_eq!(err.len(), 3, "decoded: {}", err);

    let (types, location) = err.protocol().expect("a protocol violation");
    assert_eq!(location, Location::CrcSequence);
    assert_eq!(
        types,
        ViolationTypes::FORM
            | ViolationTypes::STUFF
            | ViolationTypes::BIT0
            | ViolationTypes::BIT1
            | ViolationTypes::TX,
    );
    assert!(err.is_no_ack());
    assert!(err.is_bus_error());

    // And it re-encodes to the exact bytes the kernel handed us.
    assert_eq!(CanErrorFrame::from(err), echoed);
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

    let err = echoed.into_error();
    let all: Vec<ErrorCause> = err.causes().copied().collect();
    assert_eq!(
        all,
        vec![
            ErrorCause::Controller(ControllerProblems::RX_WARNING | ControllerProblems::TX_WARNING),
            ErrorCause::Counters { tx: 112, rx: 96 },
        ],
        "decoded: {}",
        err
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
