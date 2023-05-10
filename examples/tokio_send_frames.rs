use embedded_can::{Frame, StandardId};
use futures_timer::Delay;
use socketcan::{r#async::tokio::CanSocket, CanFrame, Error};
use std::time::Duration;
use tokio;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let socket_tx = CanSocket::open("vcan0").unwrap();

    loop {
        let id = StandardId::new(0x100).unwrap();
        let frame = CanFrame::new(id, &[0]).unwrap();

        println!("Writing on vcan0");
        socket_tx.write_frame(frame)?.await?;

        println!("Waiting 3 seconds");
        Delay::new(Duration::from_secs(3)).await?;
    }
}
