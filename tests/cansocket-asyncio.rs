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

#[cfg(all(feature = "vcan_tests", feature = "async-io"))]
use serial_test::serial;

#[cfg(all(feature = "vcan_tests", feature = "async-io"))]
use socketcan::{
    async_io::CanFdSocket as AsyncCanFdSocket, async_io::CanSocket as AsyncCanSocket,
    frame::FdFlags, CanAnyFrame, CanFdFrame, EmbeddedFrame, Id, StandardId,
};

// The virtual CAN interface to use for tests.
#[cfg(all(feature = "vcan_tests", feature = "async-io"))]
const VCAN: &str = "vcan0";

#[cfg(all(feature = "vcan_tests", feature = "async-io"))]
#[serial]
#[async_std::test]
async fn async_can_simple() {
    let writer = AsyncCanSocket::open(VCAN).unwrap();
    let reader = AsyncCanSocket::open(VCAN).unwrap();

    let frame =
        socketcan::CanFrame::new(Id::from(StandardId::new(0x14).unwrap()), &[1, 3, 3, 7]).unwrap();

    let (write_result, read_result) =
        futures::join!(writer.write_frame(&frame), reader.read_frame());

    assert!(write_result.is_ok());
    assert_eq!(frame.data(), read_result.unwrap().data());
}

#[cfg(all(feature = "vcan_tests", feature = "async-io"))]
#[serial]
#[async_std::test]
async fn async_canfd_simple() {
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
}
