//
// echo.rs
//
// @author Natesh Narain <nnaraindev@gmail.com>
// @date Jul 05 2022
//

use anyhow::Context;
use clap::Parser;

use embedded_can::{nb::Can, Frame as EmbeddedFrame, StandardId};
use nb::block;
use socketcan::{CanFrame, CanSocket, Frame, Socket};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// CAN interface
    #[clap(value_parser)]
    interface: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let iface = args.interface;

    let mut sock = CanSocket::open(&iface)
        .with_context(|| format!("Failed to open socket on interface {}", iface))?;

    sock.set_nonblocking(true)
        .context("Failed to make socket non-blocking")?;

    let frame = block!(sock.receive()).context("Receiving frame")?;

    println!("{}  {}", iface, frame_to_string(&frame));

    let frame = CanFrame::new(StandardId::new(0x1f1).unwrap(), &[1, 2, 3, 4])
        .context("Creating CAN frame")?;

    block!(sock.transmit(&frame)).context("Transmitting frame")?;

    Ok(())
}

fn frame_to_string<F: Frame>(frame: &F) -> String {
    let id = frame.raw_id();
    let data_string = frame
        .data()
        .iter()
        .fold(String::from(""), |a, b| format!("{} {:02x}", a, b));

    format!("{:X}  [{}] {}", id, frame.dlc(), data_string)
}
