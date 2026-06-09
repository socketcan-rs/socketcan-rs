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
//! This implementation requires a CAN FD interface to allow for all possible
//!

use anyhow::{Context, Result};
use clap::{arg, ArgAction, Command};
use socketcan::{dump::Reader, CanAnyFrame, CanFdSocket, Socket};
use std::process;

// Make the app version the same as the package.
const VERSION: &str = env!("CARGO_PKG_VERSION");

// Open the interface, then iterate through the records in the file
// sending them out to the bus.
fn play(filename: &str, iface: &str) -> Result<()> {
    let sock = CanFdSocket::open(iface)
        .with_context(|| format!("Failed to open FD socket on interface '{}'", iface))?;

    let reader = Reader::from_file(filename)
        .with_context(|| format!("Error opening log file '{}'", filename))?;

    for rec in reader {
        let rec = rec?;
        println!("{}", rec);

        use CanAnyFrame::*;
        match rec.frame {
            Normal(frame) => sock.write_frame(&frame)?,
            Remote(frame) => sock.write_frame(&frame)?,
            Fd(frame) => sock.write_frame(&frame)?,
            _ => (),
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
        .get_matches();

    let iface = opts.get_one::<String>("iface").unwrap();
    let filename = opts.get_one::<String>("file").unwrap();

    if let Err(err) = play(filename, iface) {
        eprintln!("{}", err);
        process::exit(1);
    }
}
