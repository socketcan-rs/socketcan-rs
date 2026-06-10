// socketcan/tests/cansocket-smol.rs
//
// Integration tests for CAN sockets on the smol runtime.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Integration tests for CAN sockets on the smol runtime.
//!
//! This test target requires the `smol` feature; it is gated by
//! `required-features = ["smol"]` in `Cargo.toml`, so Cargo won't build it
//! unless `smol` is enabled (e.g. `cargo test --features smol,vcan_tests`).
//! The individual tests additionally need a live `vcan0` interface and are
//! gated on the `vcan_tests` feature.

#[cfg(feature = "vcan_tests")]
use serial_test::serial;

#[cfg(feature = "vcan_tests")]
use socketcan::{
    CanAnyFrame, CanFdFrame, EmbeddedFrame, Id, SocketOptions, StandardId, addr::CanAddr,
    frame::FdFlags, smol::CanFdSocket as AsyncCanFdSocket, smol::CanSocket as AsyncCanSocket,
};

// The virtual CAN interface to use for tests.
#[cfg(feature = "vcan_tests")]
const VCAN: &str = "vcan0";

#[cfg(feature = "vcan_tests")]
#[serial]
#[test]
fn async_can_simple() {
    smol::block_on(async {
        let writer = AsyncCanSocket::open(VCAN).unwrap();
        let reader = AsyncCanSocket::open(VCAN).unwrap();

        let frame =
            socketcan::CanFrame::new(Id::from(StandardId::new(0x14).unwrap()), &[1, 3, 3, 7])
                .unwrap();

        let (write_result, read_result) =
            futures::join!(writer.write_frame(&frame), reader.read_frame());

        assert!(write_result.is_ok());
        assert_eq!(frame.data(), read_result.unwrap().data());
    });
}

#[cfg(feature = "vcan_tests")]
#[serial]
#[test]
fn async_canfd_simple() {
    smol::block_on(async {
        let writer = AsyncCanFdSocket::open(VCAN).unwrap();
        let reader = AsyncCanFdSocket::open(VCAN).unwrap();

        let frame = CanFdFrame::with_flags(
            StandardId::new(111).unwrap(),
            // Note: OS may report this frame as a normal CAN frame if it is 8 or less bytes of payload..
            &[1, 3, 3, 7, 1, 2, 3, 4, 5],
            FdFlags::empty(),
        )
        .unwrap();

        let (write_result, read_result) =
            futures::join!(writer.write_frame(&frame), reader.read_frame());

        assert!(write_result.is_ok());
        match read_result.unwrap() {
            CanAnyFrame::Fd(read_frame) => assert_eq!(read_frame.data(), frame.data()),
            _ => panic!("Did not get FD frame back!"),
        }
    });
}

#[cfg(feature = "vcan_tests")]
#[serial]
#[test]
fn async_read_frame_with_timestamp() {
    smol::block_on(async {
        let writer = AsyncCanSocket::open(VCAN).unwrap();
        let reader = AsyncCanSocket::open(VCAN).unwrap();
        reader.set_recv_timestamp(true).unwrap();

        let frame =
            socketcan::CanFrame::new(Id::from(StandardId::new(0x77).unwrap()), &[7, 7, 7]).unwrap();
        let sent_at = std::time::SystemTime::now();

        let (write_result, read_result) = futures::join!(
            writer.write_frame(&frame),
            reader.read_frame_with_timestamp(),
        );
        write_result.unwrap();

        let (rx, ts) = read_result.unwrap();
        assert_eq!(rx.data(), frame.data());
        let delta = ts
            .duration_since(sent_at)
            .or_else(|e| Ok::<_, std::time::SystemTimeError>(e.duration()))
            .unwrap();
        assert!(
            delta < std::time::Duration::from_secs(2),
            "async timestamp out of range: {delta:?}"
        );
    });
}

#[cfg(feature = "vcan_tests")]
#[serial]
#[test]
fn async_try_read_and_try_write() {
    smol::block_on(async {
        // Mirror of tokio's `test_tryread_and_trywrite`: confirm that
        // `try_read_frame` returns `WouldBlock` when nothing is queued, that a
        // frame written via `try_write_frame` round-trips, and that the queue
        // drains to `WouldBlock` again.
        let socket1 = AsyncCanSocket::open(VCAN).unwrap();
        let socket2 = AsyncCanSocket::open(VCAN).unwrap();

        let result = socket2.try_read_frame();
        assert!(result.is_err(), "Expected no frames available");
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);

        let frame =
            socketcan::CanFrame::new(Id::from(StandardId::new(0x1).unwrap()), &[0]).unwrap();
        socket1.try_write_frame(&frame).unwrap();
        smol::Timer::after(std::time::Duration::from_millis(100)).await;

        socket2.try_read_frame().unwrap();

        let result = socket2.try_read_frame();
        assert!(result.is_err(), "Expected no frames available");
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    });
}

#[cfg(feature = "vcan_tests")]
#[serial]
#[test]
fn async_try_read_and_try_write_fd() {
    smol::block_on(async {
        // FD variant — same shape as `async_try_read_and_try_write`, but over
        // `CanFdSocket` and using a `CanFdFrame` payload.
        let socket1 = AsyncCanFdSocket::open(VCAN).unwrap();
        let socket2 = AsyncCanFdSocket::open(VCAN).unwrap();

        let result = socket2.try_read_frame();
        assert!(result.is_err(), "Expected no frames available");
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);

        let frame = CanFdFrame::new(StandardId::new(0x1).unwrap(), &[0u8; 8]).unwrap();
        socket1.try_write_frame(&frame).unwrap();
        smol::Timer::after(std::time::Duration::from_millis(100)).await;

        socket2.try_read_frame().unwrap();

        let result = socket2.try_read_frame();
        assert!(result.is_err(), "Expected no frames available");
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    });
}

#[cfg(feature = "vcan_tests")]
#[serial]
#[test]
fn async_stream_and_sink() {
    smol::block_on(async {
        use futures::{SinkExt, StreamExt};

        let mut sock = AsyncCanSocket::open(VCAN).unwrap();
        sock.set_loopback(true).unwrap();
        sock.set_recv_own_msgs(true).unwrap();

        // Sink: push two frames in.
        let f1 =
            socketcan::CanFrame::new(Id::from(StandardId::new(0x111).unwrap()), &[1, 2]).unwrap();
        let f2 =
            socketcan::CanFrame::new(Id::from(StandardId::new(0x222).unwrap()), &[3, 4]).unwrap();
        sock.send(f1).await.unwrap();
        sock.send(f2).await.unwrap();

        // Stream: pull them back out.
        let rx1 = sock.next().await.unwrap().unwrap();
        let rx2 = sock.next().await.unwrap().unwrap();
        assert_eq!(rx1.data(), f1.data());
        assert_eq!(rx2.data(), f2.data());
    });
}

#[cfg(feature = "vcan_tests")]
#[serial]
#[test]
fn async_open_if_and_open_addr() {
    smol::block_on(async {
        // Verify the two new constructors actually reach a working socket on
        // `vcan0`. Resolve the interface index via the address helper and use it
        // for both paths; if either constructor fails or rejects the resulting
        // socket, this test panics.
        let addr = CanAddr::from_iface(VCAN).unwrap();
        let by_addr = AsyncCanSocket::open_addr(&addr).unwrap();

        let ifindex = nix::net::if_::if_nametoindex(VCAN).unwrap();
        let by_if = AsyncCanSocket::open_if(ifindex).unwrap();

        // A trivial send/recv round-trip on each, just to confirm the fd is wired
        // up correctly.
        by_if.set_loopback(true).unwrap();
        by_if.set_recv_own_msgs(true).unwrap();
        let frame =
            socketcan::CanFrame::new(Id::from(StandardId::new(0x55).unwrap()), &[0x5A]).unwrap();
        by_if.write_frame(&frame).await.unwrap();
        let rx = by_if.read_frame().await.unwrap();
        assert_eq!(rx.data(), frame.data());

        by_addr.set_loopback(true).unwrap();
        by_addr.set_recv_own_msgs(true).unwrap();
        by_addr.write_frame(&frame).await.unwrap();
        let rx2 = by_addr.read_frame().await.unwrap();
        assert_eq!(rx2.data(), frame.data());
    });
}
