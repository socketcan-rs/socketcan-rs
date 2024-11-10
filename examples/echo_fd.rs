// socketcan/examples/echo_fd.rs
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//
// @author Frank Pagliughi <fpagliughi@mindspring.com>
// @author Natesh Narain <nnaraindev@gmail.com>
// @date Jul 05 2022
//

use anyhow::Context;
use embedded_can::Frame as EmbeddedFrame;
use socketcan::{CanAnyFrame, CanFdFrame, CanFdSocket, Frame, Socket};
use std::{
    env,
    sync::atomic::{AtomicBool, Ordering},
};

fn frame_to_string<F: Frame>(frame: &F) -> String {
    let id = frame.raw_id();

    let data_string = frame
        .data()
        .iter()
        .fold(String::from(""), |a, b| format!("{} {:02x}", a, b));

    format!("{:08X}  [{}] {}", id, frame.dlc(), data_string)
}

// --------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "vcan0".into());

    let sock = CanFdSocket::open(&iface)
        .with_context(|| format!("Failed to open socket on interface {}", iface))?;

    sock.set_nonblocking(true)
        .with_context(|| "Failed to make socket non-blocking")?;

    static QUIT: AtomicBool = AtomicBool::new(false);

    ctrlc::set_handler(|| {
        QUIT.store(true, Ordering::Relaxed);
    })
    .expect("Failed to set signal handler");

    while !QUIT.load(Ordering::Relaxed) {
        if let Ok(CanAnyFrame::Fd(frame)) = sock.read_frame() {
            println!("{}", frame_to_string(&frame));

            let new_id = frame.can_id() + 0x01;

            if let Some(echo_frame) = CanFdFrame::new(new_id, frame.data()) {
                sock.write_frame(&echo_frame)
                    .expect("Failed to echo recieved frame");
            }
        }
    }

    Ok(())
}
