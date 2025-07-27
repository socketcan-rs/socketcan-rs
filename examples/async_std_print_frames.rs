// socketcan/examples/async_std_print_frames.rs
//
// Example application for using async-std with socketcan-rs.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//

//! A SocketCAN example using async-std.
//!
//! This receives CAN frames and prints them to the console.
//!

use socketcan::{async_std::CanSocket, CanFrame};
use socketcan_raw::Result;
use std::env;

#[async_std::main]
async fn main() -> Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "vcan0".into());
    let sock = CanSocket::open(&iface)?;

    println!("Reading on {}", iface);

    loop {
        match sock.read_frame().await {
            Ok(CanFrame::Data(frame)) => println!("{:?}", frame),
            Ok(CanFrame::Remote(frame)) => println!("{:?}", frame),
            Ok(CanFrame::Error(frame)) => println!("{:?}", frame),
            Err(err) => eprintln!("{}", err),
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}
