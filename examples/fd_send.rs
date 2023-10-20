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
use socketcan::{CanFdSocket, CanFrame, Result, Socket};
use std::env;

fn main() -> Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "can0".into());
    let socket_tx = CanFdSocket::open(&iface).unwrap();

    let id = StandardId::new(0x100).unwrap();
    let frame = CanFrame::new(id, &[0]).unwrap();

    println!("Writing on {}", iface);
    socket_tx.write_frame(&frame)?;

    Ok(())
}
