// socketcan/examples/async_std_send_frames.rs

use async_std::task;
use embedded_can::{Frame, StandardId};
use futures_timer::Delay;
use socketcan::{async_std::CanSocket, CanFrame, Error, Result};
use std::time::Duration;

fn main() -> Result<()> {
    task::block_on(async {
        let sock = CanSocket::open("vcan0")?;

        loop {
            let id = StandardId::new(0x100).unwrap();
            let frame = CanFrame::new(id, &[0]).unwrap();

            println!("Writing on vcan0");
            sock.write_frame(&frame).await?;

            println!("Waiting 3 seconds");
            Delay::new(Duration::from_secs(3)).await?;
        }

        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    })
}
