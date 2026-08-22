// socketcan/examples/replay_log.rs
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//
// @author Frank Pagliughi <fpagliughi@mindspring.com>
// @date Dec 28, 2024
//
//! Reads a `candump` log file and sends the frames out to the CANbus.
//!
//! The frames are paced to the timestamps recorded in the log, so a log with
//! a two-second pause in it takes two seconds to get past. Pass `--fast` to
//! send everything as quickly as the socket accepts it instead.
//!
//! This implementation requires a CAN FD interface, so that every frame type
//! a log can hold — classical, remote, FD and error — can be replayed.
//!

use anyhow::{Context, Result};
use clap::{ArgAction, Command, arg};
use socketcan::{CanAnyFrame, CanFdSocket, Socket, dump::Reader};
use std::{
    process, thread,
    time::{Duration, Instant},
};

// Make the app version the same as the package.
const VERSION: &str = env!("CARGO_PKG_VERSION");

// Open the interface, then iterate through the records in the file
// sending them out to the bus.
//
// Unless `fast` is set, each frame is held back until its recorded offset
// from the first record has elapsed. The wait is computed against the log's
// own timeline rather than the gap to the previous frame, so the time spent
// writing a frame does not accumulate as drift.
fn play(filename: &str, iface: &str, fast: bool) -> Result<()> {
    let sock = CanFdSocket::open(iface)
        .with_context(|| format!("Failed to open FD socket on interface '{}'", iface))?;

    let reader = Reader::from_file(filename)
        .with_context(|| format!("Error opening log file '{}'", filename))?;

    let start = Instant::now();
    let mut first_t_us: Option<u64> = None;

    for rec in reader {
        let rec = rec?;
        let first = *first_t_us.get_or_insert(rec.t_us);

        if !fast {
            // `saturating_sub` covers a log whose timestamps step backwards:
            // such a record is sent immediately rather than never.
            let offset = Duration::from_micros(rec.t_us.saturating_sub(first));
            if let Some(delay) = offset.checked_sub(start.elapsed()) {
                thread::sleep(delay);
            }
        }

        println!("{}", rec);

        // Error frames are replayed too, the way `cansend` can inject them.
        // A real driver may refuse them; a vcan loops them straight back,
        // which is how the error decoding is exercised.
        use CanAnyFrame::*;
        match rec.frame {
            Normal(frame) => sock.write_frame(&frame)?,
            Remote(frame) => sock.write_frame(&frame)?,
            Fd(frame) => sock.write_frame(&frame)?,
            Error(frame) => sock.write_frame(&frame)?,
        }
    }

    Ok(())
}

// --------------------------------------------------------------------------

fn main() {
    let opts = Command::new("can")
        .author("Frank Pagliughi")
        .version(VERSION)
        .about("SocketCAN example to play a candump file")
        .disable_help_flag(true)
        .arg(
            arg!(--help "Print help information")
                .short('?')
                .action(ArgAction::Help)
                .global(true),
        )
        .arg(arg!(<iface> "The CAN interface to use, like 'can0', 'vcan0', etc").required(true))
        .arg(arg!(<file> "The candump log file to read").required(true))
        .arg(
            arg!(--fast "Send as fast as possible, ignoring the recorded timestamps")
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    let iface = opts.get_one::<String>("iface").unwrap();
    let filename = opts.get_one::<String>("file").unwrap();
    let fast = opts.get_flag("fast");

    if let Err(err) = play(filename, iface, fast) {
        eprintln!("{}", err);
        process::exit(1);
    }
}
