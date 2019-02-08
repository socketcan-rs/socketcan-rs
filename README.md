[![crates.io badge](https://img.shields.io/crates/v/tokio-socketcan.svg)](https://crates.io/crates/tokio-socketcan) [![documentation](https://img.shields.io/badge/documentation-docs.rs-green.svg)](https://docs.rs/tokio-socketcan)

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

# Testing

Integrating the test into a CI system is non-trivial as it relies on a `vcan0` virtual can device existing. Adding one to most linux systems is pretty easy with root access but attaching a vcan device to a container for CI seems difficult to find support for.

To run the tests locally, though, setup should be simple:

```sh
sudo modprobe vcan
sudo ip link add vcan0 type vcan
sudo ip link set vcan0 up
cargo test
```

# Changelog

## 0.1.2

* Added `futures::sink::Sink` implementation for the `CANSocket`
