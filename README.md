[![crates.io badge](https://img.shields.io/crates/v/tokio-socketcan.svg)](https://crates.io/crates/tokio-socketcan)

# tokio-socketcan

[SocketCAN](https://www.kernel.org/doc/Documentation/networking/can.txt) support for [tokio](https://tokio.rs/) based on the [socketcan crate](https://crates.io/crates/socketcan).

# Example  echo server

```rust
use futures::stream::Stream;
use futures::future::{self, Future};

let socket_rx = tokio_socketcan::CANSocket::open("vcan0").unwrap();
let socket_tx = tokio_socketcan::CANSocket::open("vcan0").unwrap();

tokio::run(socket_rx.for_each(move |frame| {
    socket_tx.write_frame(frame)
}).map_err(|_err| {}));
```
