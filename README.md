Rust SocketCAN
==============

This library implements Controller Area Network (CAN) communications on Linux using the SocketCAN interfaces. This provides a network socket interface to the CAN bus.

[Linux SocketCAN](https://docs.kernel.org/networking/can.html)

Please see the [documentation](https://docs.rs/socketcan) for details about the Rust API provided by this library.


## Latest News

### Version 3.x adds integrated async/await!

Version 3.0 adds integrated support for async/await, with the most popular runtimes, _tokio, async-std_, and _smol_.  To get started we have already merged the [tokio-socketcan](https://github.com/oefd/tokio-socketcan) crate into this one and started on `async-io`.

Unfortunaly this required a minor breaking change to the existing API, so we will bump the version to 3.0.

The async support is optional, and can be enabled with a feature for the target runtime: `tokio`, `async-std`, or `smol`.

### What's New in Version 3.0

- Support for Rust async/await
    - All of [tokio-socketcan](https://github.com/oefd/tokio-socketcan) has been merged into this crate and will be available with an `async-tokio` build feature.
    - [#41](https://github.com/socketcan-rs/socketcan-rs/pull/41) Added initial support for `async-io` for use with `async-std` and `smol`
    - Split `SocketOptions` trait out of `Socket` trait for use with async (breaking)
    - Added cargo build features for `tokio` or `async-io`.
    - Also created specific build features for `async-std` and `smol` which just bring in the `async-io` module and alias the module name to `async-std` or `smol`, respectively, and build examples for each.

### What's New in Version 2.1

- Made `CanAddr` pulic and added functions to help interact with low-level sockaddr types. Sockets can now be opened with an address.
- Can create an `Error` directly from a `CanErrorFrame` or `std::io::ErrorKind`.
- [#46](https://github.com/socketcan-rs/socketcan-rs/issues/46)  Applications can create error frames:
    - `CanErrorFrame::new()` now works.
    - `CanErrorFrame::new_error()` is similar but more intuitive using a raw ID word.
    - `From<CanError> for CanErrorFrame` to create an error frame from a `CanError`.
- Added `Frame::from_raw_id()` and `Frame::remote_from_raw_id()`
- Bumped MSRV to 1.65.0

## Next Steps

A number of items still did not makei into a release. These will be added in v3.x, coming soon.

- Issue [#22](https://github.com/socketcan-rs/socketcan-rs/issues/22) Timestamps, including optional hardware timestamps
- Issue [#32](https://github.com/socketcan-rs/socketcan-rs/issues/32) Better coverage of the Netlink API to manipulate the CAN interfaces programatically.
- Better documentation. This README will be expanded with basic usage information, along with better doc comments, and perhaps creation of the wiki


## Minimum Supported Rust Version (MSRV)

The current version of the crate targets Rust Edition 2021 with an MSRV of Rust v1.65.0.

Note that, at this time, the MSRV is mostly diven by use of the `clap v4.0` crate for managing command-line parameters in the utilities and example applications. The core library could likely be built with an earlier version of the compiler if required.

## Async Support

### Tokio

The [tokio-socketcan](https://crates.io/crates/tokio-socketcan) crate was merged into this one to provide async support for CANbus using tokio.

This is enabled with the optional feature, `tokio`.

#### Example bridge with _tokio_

This is a simple example of sending data frames from one CAN interface to another. It is included in
the example applications as
[tokio_bridge.rs](https://github.com/socketcan-rs/socketcan-rs/blob/master/examples/tokio_bridge.rs).

```rust
use futures_util::StreamExt;
use socketcan::{tokio::CanSocket, CanFrame, Result};
use tokio;

#[tokio::main]
async fn main() -> Result<()> {
    let mut sock_rx = CanSocket::open("vcan0")?;
    let sock_tx = CanSocket::open("can0")?;

    while let Some(Ok(frame)) = sock_rx.next().await {
        if matches!(frame, CanFrame::Data(_)) {
            sock_tx.write_frame(frame)?.await?;
        }
    }

    Ok(())
}
```

### async-io  (_async-std_ & _smol_)

New support was added for the [async-io](https://crates.io/crates/async-io) runtime, supporting the [async-std](https://crates.io/crates/async-std) and [smol](https://crates.io/crates/smol) runtimes.

This is enabled with the optional feature, `async-io`. It can also be enabled with either feature, `async-std` or `smol`. Either of those specific runtime flags will simply build the `async-io` support but then also alias the `async-io` submodule with the specific feature/runtime name. This is simply for convenience.

Additionally, when building examples, the specific examples for the runtime will be built if specifying the `async-std` or `smol` feature(s).

#### Example bridge with _async-std_

This is a simple example of sending data frames from one CAN interface to another. It is included in
the example applications as
[async_std_bridge.rs](https://github.com/socketcan-rs/socketcan-rs/blob/master/examples/async_std_bridge.rs).

```rust
use socketcan::{async_std::CanSocket, CanFrame, Result};

#[async_std::main]
async fn main() -> Result<()> {
    let sock_rx = CanSocket::open("vcan0")?;
    let sock_tx = CanSocket::open("can0")?;

    loop {
        let frame = sock_rx.read_frame().await?;
        if matches!(frame, CanFrame::Data(_)) {
            sock_tx.write_frame(&frame).await?;
        }
    }
}
```

## Testing

Integrating the full suite of tests into a CI system is non-trivial as it relies on a `vcan0` virtual CAN device existing. Adding it to most Linux systems is pretty easy with root access, but attaching a vcan device to a container for CI seems difficult to implement.

Therefore, tests requiring `vcan0` were placed behind an optional feature, `vcan_tests`.

The steps to install and add a virtual interface to Linux are in the `scripts/vcan.sh` script. Run it with root proveleges, then run the tests:

```sh
$ sudo ./scripts/vcan.sh
$ cargo test --features=vcan_tests
```