// socketcan/examples/smol_send_frames.rs
//
// Example application for using smol with socketcan-rs.
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
//! This sends data frames to the CANbus.
//!

use embedded_can::{Frame, StandardId};
use futures_timer::Delay;
use socketcan::{smol::CanSocket, CanFrame, Error, Result};
use std::{env, time::Duration};

fn main() -> Result<()> {
    smol::block_on(async {
        let iface = env::args().nth(1).unwrap_or_else(|| "vcan0".into());
        let sock = CanSocket::open(&iface)?;

        loop {
            let id = StandardId::new(0x100).unwrap();
            let frame = CanFrame::new(id, &[0]).unwrap();

            println!("Writing on {}", iface);
            sock.write_frame(&frame).await?;

            println!("Waiting 3 seconds");
            Delay::new(Duration::from_secs(3)).await?;
        }

        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    })
}
