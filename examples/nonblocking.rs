//
// echo.rs
//
// @author Natesh Narain <nnaraindev@gmail.com>
// @date Jul 05 2022
//

use anyhow::Context;
use clap::Parser;

use socketcan::{Frame, CanSocket as CanSocketT, CanNormalSocket as CanSocket, CanNormalFrame as CanFrame};
use embedded_hal::can::{nb::Can, Frame as EmbeddedFrame, Id, StandardId};
use nb::block;


#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// CAN interface
    #[clap(value_parser)]
    interface: String,
}


fn main() -> anyhow::Result<()> {

    let args = Args::parse();
    let can_interface = args.interface;

    let mut socket = CanSocket::open(&can_interface)
        .with_context(|| format!("Failed to open socket on interface {}", can_interface))?;
    socket.set_nonblocking(true).with_context(|| format!("Failed to make socket non-blocking"))?;

    let frame = block!(socket.receive());

    if let Ok(frame) = frame {
        println!("{}", frame_to_string(&frame));
    }

    let write_frame = CanFrame::new(
        StandardId::new(0x1f1).unwrap(),
        &[1, 2, 3, 4]
    ).expect("Failed to create CAN frame");

    if let Err(e) = block!(socket.transmit(&write_frame)) {
        println!("{}", e);
    }

    Ok(())
}

fn frame_to_string<F: Frame>(f: &F) -> String {
    let id = get_raw_id(&f.id());
    let data_string = f.data().iter().fold(String::from(""), |a, b| format!("{} {:02x}", a, b));

    format!("{:08X}  [{}] {}", id, f.dlc(), data_string)
}

fn get_raw_id(id: &Id) -> u32 {
    match id {
        Id::Standard(id) => id.as_raw() as u32,
        Id::Extended(id) => id.as_raw(),
    }
}
