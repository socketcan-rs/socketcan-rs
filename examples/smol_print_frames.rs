// socketcan/examples/smol_print_frames.rs
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//

//! A SocketCAN example using smol.
//!
//! This receives CAN frames and prints them to the console, including
//! decoded error frames.
//!

use socketcan::{
    CanErrorFrame, CanFrame, Error, Result, SocketOptions, id::ERR_MASK_ALL, smol::CanSocket,
};
use std::env;

// --------------------------------------------------------------------------

/// Prints an error frame and everything it reports.
///
/// A single error frame can describe several distinct conditions at once —
/// a controller state change arrives together with the current error
/// counters, for instance — so it decodes into a collection rather than a
/// single error.
fn print_error_frame(frame: CanErrorFrame) {
    println!("{:?}", frame);
    let errs = frame.into_error();
    if errs.is_single() {
        println!("    error: {}", errs.first());
    } else {
        println!("    {} errors:", errs.len());
        for err in &errs {
            println!("      - {}", err);
        }
    }
}

// --------------------------------------------------------------------------

fn main() -> Result<()> {
    smol::block_on(async {
        let iface = env::args().nth(1).unwrap_or_else(|| "can0".into());
        let sock = CanSocket::open(&iface)?;

        // Error frames are not delivered by default; ask for all of them.
        sock.set_error_mask(ERR_MASK_ALL)?;

        println!("Reading on {}", iface);

        loop {
            match sock.read_frame().await {
                Ok(CanFrame::Data(frame)) => println!("{:?}", frame),
                Ok(CanFrame::Remote(frame)) => println!("{:?}", frame),
                Ok(CanFrame::Error(frame)) => print_error_frame(frame),
                Err(err) => eprintln!("{}", err),
            }
        }

        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    })
}
