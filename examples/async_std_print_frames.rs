// socketcan/examples/async_std_print_frames.rs

use async_std::task;
use socketcan::{async_std::CanSocket, CanFrame, Error, Result};
use std::env;

fn main() -> Result<()> {
    task::block_on(async {
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
        Ok::<(), Error>(())
    })
}
