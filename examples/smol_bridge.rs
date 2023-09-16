// socketcan/examples/smol_bridge.rs
//
// Example application for using smol with socketcan-rs.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! A SocketCAN example using smol.
//!
//! This sends CAN data frames received on one interface to another.
//!

use socketcan::{smol::CanSocket, CanFrame, Error, Result};

fn main() -> Result<()> {
    smol::block_on(async {
        let sock_rx = CanSocket::open("vcan0")?;
        let sock_tx = CanSocket::open("can0")?;

        loop {
            let frame = sock_rx.read_frame().await?;
            if matches!(frame, CanFrame::Data(_)) {
                sock_tx.write_frame(&frame).await?;
            }
        }

        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    })
}
