// socketcan/examples/tokio_recvts.rs
//
// Example: receive CAN frames asynchronously (tokio) and print their
// timestamps. Tokio mirror of `examples/can_recvts.rs`.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//
// Usage:
//   tokio_recvts [interface]     (default interface: can0)

use socketcan::{
    Frame, SOF_TIMESTAMPING_OPT_CMSG, SOF_TIMESTAMPING_RAW_HARDWARE, SOF_TIMESTAMPING_RX_HARDWARE,
    SOF_TIMESTAMPING_RX_SOFTWARE, SOF_TIMESTAMPING_SOFTWARE, SocketOptions, tokio::CanSocket,
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

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "can0".to_string());
    let sock = CanSocket::open(&iface)?;

    if sock.has_hw_timestamps() {
        println!("HW timestamps supported on {iface}");
        sock.set_recv_timestamp(true)?;
        // Each timestamp source needs two flags: one to select when it
        // is taken (RX_*) and one to request that it be reported in the
        // ancillary data (SOFTWARE / RAW_HARDWARE). OPT_CMSG is required
        // for RX cmsg delivery on non-IP sockets like CAN raw.
        sock.set_timestamping(
            SOF_TIMESTAMPING_RX_HARDWARE
                | SOF_TIMESTAMPING_RAW_HARDWARE
                | SOF_TIMESTAMPING_RX_SOFTWARE
                | SOF_TIMESTAMPING_SOFTWARE
                | SOF_TIMESTAMPING_OPT_CMSG,
        )?;
        loop {
            let (frame, ts) = sock.read_frame_with_timestamps().await?;
            let sw = ts
                .sw
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| format!("{:.6}", d.as_secs_f64()))
                .unwrap_or_else(|| "-".into());
            let hw_ns = ts
                .hw
                .map(|d| d.as_nanos().to_string())
                .unwrap_or_else(|| "-".into());
            println!("{sw}  ({hw_ns})  {}", frame_info(&frame));
        }
    } else {
        println!("HW timestamps not supported on {iface}; using software timestamps");
        sock.set_recv_timestamp(true)?;
        loop {
            let (frame, ts) = sock.read_frame_with_timestamp().await?;
            let sw = ts
                .duration_since(UNIX_EPOCH)
                .map(|d| format!("{:.6}", d.as_secs_f64()))
                .unwrap_or_else(|_| "-".into());
            println!("{sw}  {}", frame_info(&frame));
        }
    }
}
