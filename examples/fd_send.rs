// socketcan/examples/fd_send.rs
//
// Example: send a single CAN FD frame on the given interface.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.
//

//! A SocketCAN FD send example.
//!
//! Opens a `CanFdSocket` and writes one CAN FD data frame to the bus.

use embedded_can::{Frame, StandardId};
use socketcan::{CanFdFrame, CanFdSocket, Result, Socket};
use std::env;

fn main() -> Result<()> {
    let iface = env::args().nth(1).unwrap_or_else(|| "can0".into());
    let socket_tx = CanFdSocket::open(&iface).unwrap();

    let id = StandardId::new(0x100).unwrap();
    // 12-byte payload demonstrates an extended FD length (DLC 9).
    let frame = CanFdFrame::new(id, &[0xDE, 0xAD, 0xBE, 0xEF, 0, 1, 2, 3, 4, 5, 6, 7]).unwrap();

    println!("Writing FD frame on {}", iface);
    socket_tx.write_frame(&frame)?;

    Ok(())
}
