use futures_timer::Delay;
use std::time::Duration;
use tokio;
use tokio_socketcan::{CANFrame, CANSocket};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let socket_tx = CANSocket::open("vcan0").unwrap();

    loop {
        let frame = CANFrame::new(0x1, &[0], false, false).unwrap();
        println!("Writing on vcan0");
        socket_tx.write_frame(frame).await?;
        println!("Waiting 3 seconds");
        Delay::new(Duration::from_secs(3)).await;
    }
}
