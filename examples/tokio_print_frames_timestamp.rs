// socketcan/examples/tokio_print_frames_timestamp.rs
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

//! A SocketCAN example using Tokio and timestamps.
//!
//! This receives CAN frames and prints them to the console with it's timestamp.
//!

use futures_util::StreamExt;
use socketcan::{tokio::CanSocketTimestamp, CanFrame};
use std::env;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "vcan0".into());
    let mut sock = CanSocketTimestamp::open(&iface).unwrap();

    println!("Reading on {}", iface);

    while let Some(res) = sock.next().await {
        match res {
            Ok((CanFrame::Data(frame), t)) => {
                if let Some(t) = t {
                    println!("[{:?}]:{:?}", t, frame)
                } else {
                    println!("{:?}", frame)
                }
            }
            Ok((CanFrame::Remote(frame), t)) => {
                if let Some(t) = t {
                    println!("[{:?}]:{:?}", t, frame)
                } else {
                    println!("{:?}", frame)
                }
            }
            Ok((CanFrame::Error(frame), t)) => {
                if let Some(t) = t {
                    println!("[{:?}]:{:?}", t, frame)
                } else {
                    println!("{:?}", frame)
                }
            }
            Err(err) => eprintln!("{}", err),
        }
    }

    Ok(())
}
