Rust SocketCAN
==============

This library implements Controller Area Network (CAN) communications on Linux using the SocketCAN interfaces. This provides a network socket interface to the CAN bus.

[Linux SocketCAN](https://docs.kernel.org/networking/can.html)

Please see the [documentation](https://docs.rs/socketcan) for details about the Rust API provided by this library.


## Latest News

### Version 3.0 is coming soon!

Version 3.0 should be out soon with integrated support for async/await, with the most popular runtimes, _tokio, async-std_, and _smol_.  To get started we have already merged the [tokio-socketcan](https://github.com/oefd/tokio-socketcan) crate into this one and started on `async-io`.

Unfortunaly this required a minor breaking change to the existing API, so we will bump the version to 3.0.

### Version 2.0 is finally released!

Version 2.0 is a major rewrite of the library to modernize it for the latest Rust Edition and conventions, and add CAN FD support.

## Unreleased Features in this Branch

- All of [tokio-socketcan](https://github.com/oefd/tokio-socketcan) has been merged into this crate and will be available with an `async-tokio` build feature.
- [#41](https://github.com/socketcan-rs/socketcan-rs/pull/41) Added initial support for `async-io` for use with `async-std` and `smol`
- Made 'CanAddr' pulic and added functions to help interact with low-level sockaddr types. Sockets can now be opened with an address.
- Bumped MSRV to 1.65.0

## What's New in v2.0

The v2.0 release is a fairly large rewrite of the library and adds the following features:

- CAN Flexible Data Rate (FD) support
- Proper handling of Extended CAN IDs
- Integration with the Rust Embedded HAL APIs for CAN
- Some control of the CAN network interfaces via netlink with the [neli](https://crates.io/crates/neli) crate.
- Tighter integration with [libc](https://crates.io/crates/libc) and [nix](https://crates.io/crates/nix) crates, including changes we pushed upstream to support SocketCAN
- Update to Rust Edition 2021, with updates to the dependencies.
- Update error types conforming to `std::error::Error`
- Distinct separate frame types:
    - `CanDataFrame`, `CanRemoteFrame`, `CanErrorFrame`, and `CanFdFrame`
    - Enum wrapper types `CanFrame` for the classic 2.0 frames and `CanAnyFrame` for any type of frame including the larger FD frames
- Updated documentation
- Targeting Rust Edition 2021 w/ MSRV 1.64.0

## Next Steps

A number of items did not make it into the 2.0 release. These will be added in v3.0, coming soon.

- Issue [#22](https://github.com/socketcan-rs/socketcan-rs/issues/22) Timestamps, including optional hardware timestamps
- Issue [#32](https://github.com/socketcan-rs/socketcan-rs/issues/32) Better coverage of the Netlink API to manipulate the CAN interfaces programatically.
- Better documentation. This README will be expanded with basic usage information, along with better doc comments, and perhaps creation of the wiki

We will also start looking into support of Rust async/await, prefereably in a portable way without lying on a particular library/executor. But certainly support for the main ones like Tokio would be the goal. Some folks have suggested putting this into a separate wrapper crate, but it would be better to add it here for convenience, but certainly made optional through a Cargo build feature.

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

### async-io

New support was added for the [async-io](https://crates.io/crates/async-io) runtime, supporting _async-std_ and _smol_.

This is enabled with the optional feature, `async-io`. It can also be enabled with either feature, `async-std` or `smol`. These latter two are particularly useful when building manually, as they will also build the examples for each those specific runtimes, respectively.

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