# Rust SocketCAN

This Rust library implements Controller Area Network (CAN) communications on Linux using the SocketCAN subsystem, which provides a network socket interface to the CAN bus.

[Linux SocketCAN](https://docs.kernel.org/networking/can.html)

Please see the [documentation](https://docs.rs/socketcan) for details about the Rust API provided by this library.


## Latest News

Version 3.6 **finally** gets us support for timestamps on incoming frames. This includes software and (where the driver supports it) hardware timestamps that can be delivered alongside each frame via a single `recvmsg()` call. See the "Timestamps" section below.

### What's New in Version 3.6

- **Timestamps on Incoming Frames**
    - Application can chose Software or Hardware timestamps
        - Software timestamps provide system (wall clock) time at several places in the network stack.
        - Hardware provides monotonic, nanosecond integer time. Good for precise differencing between frames.
        - Application can request any combination of possible timestamps.
- Did an in-depth review of bugs and memory safety issues, with fixes (See the CHANGELOG)
- Bumped MSRV to v1.75
    - The older v1.70 was becoming increasingly difficult to maintain.
- The full list of updates and fixes is in [CHANGELOG.md](./CHANGELOG.md).

## Minimum Supported Rust Version (MSRV)

The current version of the crate targets Rust Edition 2021 with an MSRV of Rust v1.75.

Note that, the core library can likely compile with an earlier version if dependencies are carefully selected, but tests are being done with the latest stable compiler and the stated MSRV.

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

#[tokio::main]
async fn main() -> Result<()> {
    let mut sock_rx = CanSocket::open("vcan0")?;
    let sock_tx = CanSocket::open("can0")?;

    while let Some(Ok(frame)) = sock_rx.next().await {
        if matches!(frame, CanFrame::Data(_)) {
            sock_tx.write_frame(frame).await?;
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

## Timestamps

Version 3.6 adds receive timestamps for CAN frames. Three sources are supported, each enabled independently via socket options:

| Source     | Option                                                | What it reports                                |
|------------|-------------------------------------------------------|------------------------------------------------|
| `socket`   | `SO_TIMESTAMPNS` via `set_recv_timestamp(true)`       | Wall-clock arrival at the socket layer         |
| `sw`       | `SO_TIMESTAMPING` with `RX_SOFTWARE \| SOFTWARE`      | Wall-clock arrival at the network stack        |
| `hw`       | `SO_TIMESTAMPING` with `RX_HARDWARE \| RAW_HARDWARE`  | Raw hardware-clock value from the CAN adapter  |

Note:
- The `sw` option gets the timestamp a little earlier in the receive process and is slightly more accurate
All read methods deliver the frame and any enabled timestamps atomically in one `recvmsg()` call. Hardware support can be queried with `has_hw_timestamps()` before enabling.

```rust
use socketcan::{
    CanSocket, Socket, SocketOptions,
    SOF_TIMESTAMPING_OPT_CMSG, SOF_TIMESTAMPING_RX_SOFTWARE,
    SOF_TIMESTAMPING_SOFTWARE,
};

let sock = CanSocket::open("can0")?;
sock.set_recv_timestamp(true)?;
sock.set_timestamping(
    SOF_TIMESTAMPING_RX_SOFTWARE
        | SOF_TIMESTAMPING_SOFTWARE
        | SOF_TIMESTAMPING_OPT_CMSG,
)?;

let (frame, ts) = sock.read_frame_with_timestamps()?;
println!("socket: {:?}, sw: {:?}", ts.socket, ts.sw);
```

The full example is in [examples/can_recvts.rs](https://github.com/socketcan-rs/socketcan-rs/blob/master/examples/can_recvts.rs). Async equivalents are available on the `tokio::CanSocket` and `async_io::CanSocket` wrappers (and likewise for `CanFdSocket`).

## Testing

Integrating the full suite of tests into a CI system is non-trivial as it relies on a `vcan0` virtual CAN device existing. Adding it to most Linux systems is pretty easy with root access, but attaching a vcan device to a container for CI seems difficult to implement.

Therefore, tests requiring `vcan0` were placed behind an optional feature, `vcan_tests`.

The steps to install and add a virtual interface to Linux are in the `scripts/vcan.sh` script. Run it with root privileges, then run the tests:

```sh
$ sudo ./scripts/vcan.sh
$ cargo test --features=vcan_tests
```