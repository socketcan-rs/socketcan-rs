//
// echo.rs
//
// @author Natesh Narain <nnaraindev@gmail.com>
// @date Jul 05 2022
//

use anyhow::Context;
use clap::Parser;

use socketcan::{CanSocket, CanFrame};
use embedded_hal::can::{blocking::Can, Frame, Id, StandardId};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;


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

    let shutdown = AtomicBool::new(false);
    let shutdown = Arc::new(shutdown);
    let signal_shutdown = shutdown.clone();

    ctrlc::set_handler(move ||{
        signal_shutdown.store(true, Ordering::Relaxed);
    })
    .expect("Failed to set signal handler");

    while !shutdown.load(Ordering::Relaxed) {
        match socket.receive() {
            Ok(frame) => {
                println!("{}", frame_to_string(&frame));

                let new_id = get_raw_id(&frame.id()) + 0x01;
                let new_id = StandardId::new(new_id as u16).expect("Failed to create ID");

                if let Some(echo_frame) = CanFrame::new(new_id, frame.data()) {
                    socket.transmit(&echo_frame).expect("Failed to echo recieved frame");
                }
            },
            Err(_) => {},
        }
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
