// socketcan/examples/echo.rs
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//
// @author Natesh Narain <nnaraindev@gmail.com>
// @date Jul 05 2022
//
//! Listen on a CAN interface and echo any CAN 2.0 data frames back to
//! the bus.
//!
//! The frames are sent back on CAN ID +1.
//!
//! You can test send frames to the application like this:
//!
//!```text
//! $ cansend can0 110#00112233
//! $ cansend can0 110#0011223344556677
//!```
//!

use anyhow::Context;
use embedded_can::{blocking::Can, Frame as EmbeddedFrame};
use socketcan::{CanFrame, CanSocket, Frame, Socket};
use std::{
    env,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

fn frame_to_string<F: Frame>(frame: &F) -> String {
    let id = frame.raw_id();

    let data_string = frame
        .data()
        .iter()
        .fold(String::new(), |a, b| format!("{} {:02X}", a, b));

    format!("{:08X}  [{}] {}", id, frame.dlc(), data_string)
}

// --------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "vcan0".into());

    let mut sock = CanSocket::open(&iface)
        .with_context(|| format!("Failed to open socket on interface {}", iface))?;

    static QUIT: AtomicBool = AtomicBool::new(false);

    ctrlc::set_handler(|| {
        QUIT.store(true, Ordering::Relaxed);
    })
    .expect("Failed to set ^C handler");

    while !QUIT.load(Ordering::Relaxed) {
        if let Ok(frame) = sock.read_frame_timeout(Duration::from_millis(100)) {
            println!("{}", frame_to_string(&frame));

            let new_id = frame.can_id() + 0x01;

            if let Some(echo_frame) = CanFrame::new(new_id, frame.data()) {
                sock.transmit(&echo_frame)
                    .expect("Failed to echo received frame");
            }
        }
    }

    Ok(())
}
