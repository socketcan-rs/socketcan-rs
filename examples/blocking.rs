// socketcan/examples/blocking.rs
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
use std::env;

fn frame_to_string<F: Frame>(frame: &F) -> String {
    let id = frame.raw_id();
    let data_string = frame
        .data()
        .iter()
        .fold(String::from(""), |a, b| format!("{} {:02x}", a, b));

    format!("{:X}  [{}] {}", id, frame.dlc(), data_string)
}

// --------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "vcan0".into());

    let mut sock = CanSocket::open(&iface)
        .with_context(|| format!("Failed to open socket on interface {}", iface))?;

    let frame = sock.receive().context("Receiving frame")?;

    println!("{}  {}", iface, frame_to_string(&frame));

    let frame = CanFrame::new(StandardId::new(0x1f1).unwrap(), &[1, 2, 3, 4])
        .context("Creating CAN frame")?;

    sock.transmit(&frame).context("Transmitting frame")?;

    Ok(())
}
