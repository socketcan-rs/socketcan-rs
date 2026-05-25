// socketcan/examples/can_recvts.rs
//
// Example: receive CAN frames and print their timestamps.
// Mirrors canbusrecvts.cpp from the sockpp C++ library.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//
// Usage:
//   can_recvts [interface]     (default interface: can0)

use socketcan::{
    CanSocket, Frame, Socket, SocketOptions, SOF_TIMESTAMPING_OPT_CMSG,
    SOF_TIMESTAMPING_RX_HARDWARE, SOF_TIMESTAMPING_RX_SOFTWARE,
};
use std::{env, time::UNIX_EPOCH};

fn frame_info<F: Frame>(frame: &F) -> String {
    let id = frame.raw_id();
    let data = frame
        .data()
        .iter()
        .fold(String::new(), |s, b| format!("{s} {b:02X}"));
    format!("{id:08X}  [{}]{data}", frame.dlc())
}

fn main() -> std::io::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "can0".to_string());
    let sock = CanSocket::open(&iface)?;

    if sock.has_hw_timestamps() {
        println!("HW timestamps supported on {iface}");
        sock.set_recv_timestamp(true)?;
        sock.set_timestamping(
            SOF_TIMESTAMPING_RX_HARDWARE | SOF_TIMESTAMPING_RX_SOFTWARE | SOF_TIMESTAMPING_OPT_CMSG,
        )?;
        loop {
            let (frame, ts) = sock.read_frame_with_timestamps()?;
            let sw = ts
                .sw
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let hw_ns = ts.hw.map(|d| d.as_nanos()).unwrap_or(0);
            println!("{sw:.6}  ({hw_ns})  {}", frame_info(&frame));
        }
    } else {
        println!("HW timestamps not supported on {iface}; using software timestamps");
        sock.set_recv_timestamp(true)?;
        loop {
            let (frame, ts) = sock.read_frame_with_timestamp()?;
            let sw = ts
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            println!("{sw:.6}  {}", frame_info(&frame));
        }
    }
}
