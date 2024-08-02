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
use socketcan::{tokio::CanSocket, CanFrame, Result};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    let socket_tx = CanSocket::open("vcan0").unwrap();

    loop {
        let id = StandardId::new(0x100).unwrap();
        let frame = CanFrame::new(id, &[0]).unwrap();

        println!("Writing on vcan0");
        socket_tx.write_frame(frame).await?;

        println!("Waiting 3 seconds");
        Delay::new(Duration::from_secs(3)).await?;
    }
}
