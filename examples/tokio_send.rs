// socketcan/examples/tokio_send.rs
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

//! A SocketCAN example using tokio.
//!
//! This sends data frames to the CANbus.
//!

use embedded_can::{Frame, StandardId};
use futures_timer::Delay;
use socketcan::{CanFrame, Result, tokio::CanSocket};
use std::{env, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "vcan0".into());
    let socket_tx = CanSocket::open(&iface).unwrap();

    loop {
        let id = StandardId::new(0x100).unwrap();
        let frame = CanFrame::new(id, &[0]).unwrap();

        println!("Writing on {iface}");
        socket_tx.write_frame(frame).await?;

        println!("Waiting 3 seconds");
        Delay::new(Duration::from_secs(3)).await?;
    }
}
