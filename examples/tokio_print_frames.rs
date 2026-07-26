// socketcan/examples/tokio_print_frames.rs
//
// Example application for using Tokio with socketcan-rs.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//

//! A SocketCAN example using Tokio.
//!
//! This receives CAN frames and prints them to the console, including
//! decoded error frames.
//!

use futures_util::StreamExt;
use socketcan::{CanErrorFrame, CanFrame, SocketOptions, id::ERR_MASK_ALL, tokio::CanSocket};
use std::env;

/// Prints an error frame and everything it reports.
///
/// A single error frame can describe several distinct conditions at once —
/// a controller state change arrives together with the current error
/// counters, for instance — so it decodes into a collection rather than a
/// single error.
fn print_error_frame(frame: CanErrorFrame) {
    println!("{:?}", frame);
    let errs = frame.into_errors();
    if errs.is_single() {
        println!("    error: {}", errs.first());
    } else {
        println!("    {} errors:", errs.len());
        for err in &errs {
            println!("      - {}", err);
        }
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "vcan0".into());
    let mut sock = CanSocket::open(&iface).unwrap();

    // Error frames are not delivered by default; ask for all of them.
    sock.set_error_mask(ERR_MASK_ALL).unwrap();

    println!("Reading on {}", iface);

    while let Some(res) = sock.next().await {
        match res {
            Ok(CanFrame::Data(frame)) => println!("{:?}", frame),
            Ok(CanFrame::Remote(frame)) => println!("{:?}", frame),
            Ok(CanFrame::Error(frame)) => print_error_frame(frame),
            Err(err) => eprintln!("{}", err),
        }
    }

    Ok(())
}
