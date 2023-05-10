use futures_util::StreamExt;
use socketcan::{r#async::tokio::CanSocket, CanFrame};
use tokio;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut socket_rx = CanSocket::open("vcan0").unwrap();

    println!("Reading on vcan0");

    while let Some(res) = socket_rx.next().await {
        match res {
            Ok(CanFrame::Data(frame)) => println!("{:?}", frame),
            Ok(CanFrame::Remote(frame)) => println!("{:?}", frame),
            Ok(CanFrame::Error(frame)) => println!("{:?}", frame),
            Err(err) => eprintln!("{}", err),
        }
    }

    Ok(())
}
