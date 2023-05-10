use futures_util::StreamExt;
use tokio;
use tokio_socketcan::CANSocket;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut socket_rx = CANSocket::open("vcan0").unwrap();

    println!("Reading on vcan0");

    while let Some(next) = socket_rx.next().await {
        println!("{:#?}", next);
    }

    Ok(())
}
