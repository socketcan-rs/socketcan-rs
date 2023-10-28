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

use anyhow::Context;
use embedded_can::{blocking::Can, Frame as EmbeddedFrame, StandardId};
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
        .fold(String::from(""), |a, b| format!("{} {:02x}", a, b));

    format!("{:08X}  [{}] {}", id, frame.dlc(), data_string)
}

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

            let new_id = frame.raw_id() + 0x01;
            let new_id = StandardId::new(new_id as u16).expect("Failed to create ID");

            if let Some(echo_frame) = CanFrame::new(new_id, frame.data()) {
                sock.transmit(&echo_frame)
                    .expect("Failed to echo recieved frame");
            }
        }
    }

    Ok(())
}
