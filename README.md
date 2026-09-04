# Rust SocketCAN

This Rust library implements Controller Area Network (CAN) communications on Linux using the SocketCAN subsystem, which provides a network socket interface to the CAN bus.

[Linux SocketCAN](https://docs.kernel.org/networking/can.html)

Please see the [documentation](https://docs.rs/socketcan) for details about the Rust API provided by this library.


## Latest News

A new major version, 4.0 is coming.

### What's New in Version 4.0

- More complete Error support, including multiple causes for an error.
- Improved CAN dump support
- Optional `serde` support for serializing and deserializing frames and other data types.
- Bumped the MSRV to 1.89, Edition to 2024, and updated a number of dependencies.
- Several bug fixes and additional improvements

The full list of updates and fixes is in [CHANGELOG.md](./CHANGELOG.md).

## Minimum Supported Rust Version (MSRV)

The current version of the crate targets Rust Edition 2024 with an MSRV of Rust v1.89.

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

### _smol_

Async support for the [smol](https://crates.io/crates/smol) runtime, built on [async-io](https://crates.io/crates/async-io).

This is enabled with the optional feature, `smol`.
Additionally, when building examples, the specific examples for the runtime will be built if specifying the `smol` feature.

#### Example bridge with _smol_

This is a simple example of sending data frames from one CAN interface to another. It is included in
the example applications as
[smol_bridge.rs](./examples/smol_bridge.rs).

```rust
use socketcan::{smol::CanSocket, CanFrame, Error, Result};

fn main() -> Result<()> {
    smol::block_on(async {
        let sock_rx = CanSocket::open("vcan0")?;
        let sock_tx = CanSocket::open("can0")?;

        loop {
            let frame = sock_rx.read_frame().await?;
            if matches!(frame, CanFrame::Data(_)) {
                sock_tx.write_frame(&frame).await?;
            }
        }

        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    })
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

The full example is in [examples/can_recvts.rs](https://github.com/socketcan-rs/socketcan-rs/blob/master/examples/can_recvts.rs). Async equivalents are available on the `tokio::CanSocket` and `smol::CanSocket` wrappers (and likewise for `CanFdSocket`).

## Serialization

Version 4.0 adds an optional `serde` feature for serializing frames, errors, and interface configuration. It is **not** enabled by default:

```toml
[dependencies]
socketcan = { version = "4.0", features = ["serde"] }
```

This covers the frame types (`CanFrame`, `CanAnyFrame`, and the individual data / remote / error / FD frames), `CanId`, the `IdFlags` and `FdFlags` bit flags, `CanFilter`, `CanTimestamps`, `dump::CanDumpRecord`, every error type, and the netlink configuration types.

The format is left entirely to the caller — nothing in the implementation presumes a text format. Frame payloads are serialized through serde's `serialize_bytes`, so formats with a native byte-string type (MessagePack, CBOR, bincode) use it rather than expanding the payload into a sequence of integers. JSON, which has no byte-string type, renders a byte array:

```rust
use socketcan::{CanFrame, EmbeddedFrame, StandardId};

let frame = CanFrame::new(StandardId::new(0x123).unwrap(), &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
println!("{}", serde_json::to_string(&frame)?);
// {"Data":{"id":{"Standard":291},"data":[222,173,190,239]}}
```

For that frame, JSON takes 57 bytes and MessagePack 26.

Note that a standard and an extended identifier stay distinct even when their numeric values match, and the FD flags serialize by name rather than as an opaque integer:

```text
{"id":{"Standard":1793},"flags":"BRS | FDF","data":[127]}
```

Deserialization goes back through the normal constructors, so an out-of-range identifier or an over-long payload is rejected rather than producing a malformed frame.

### Interface configuration

Probably the most useful application: keeping an interface's bit timing and control modes in a file rather than hard-coding them.

```rust
use socketcan::nl::{CanInterface, InterfaceCanParams};

let params: InterfaceCanParams = serde_json::from_str(&std::fs::read_to_string("can0.json")?)?;
let iface = CanInterface::open("can0")?;
if let Some(bt) = params.bit_timing {
    iface.set_bit_timing(bt)?;
}
```

```json
{
  "bit_timing": { "bitrate": 500000, "sample_point": 875,
                  "tq": 0, "prop_seg": 0, "phase_seg1": 0,
                  "phase_seg2": 0, "sjw": 0, "brp": 0 },
  "restart_ms": 100
}
```

Unset parameters are omitted rather than written as nulls, and a config file only needs the parameters you actually care about — `{"restart_ms": 50}` on its own is valid input.

### Errors

A decoded error frame is a single `CanError` holding one cause per error class bit, and it serializes as a flat array of those causes, since one frame commonly reports several conditions at once:

```text
[{"Controller":"RX_WARNING | TX_WARNING"},{"Counters":{"tx":112,"rx":96}}]
```

The bitfield facets carry a whole set of conditions in one cause, written as the flag names, and a cause with no detail of its own is a bare string:

```text
[{"Protocol":{"types":"FORM | STUFF | BIT0 | BIT1 | TX","location":"CrcSequence"}},"NoAck","BusError"]
```

`CanError` is non-empty by construction, and that holds across serde: deserializing an empty array fails with "a CanError must hold at least one cause" rather than being a way to build an invalid value.

The one deliberately lossy conversion is an `std::io::Error`, wherever one appears: the `Io` variant of the top-level `Error`, and the `Io` variant of a `dump::ParseError` nested in its `Parser` arm. `io::Error` implements neither serde trait and may carry an OS errno or a boxed source, so it is reduced to a kind name and a message:

```text
{"Io":{"kind":"NetworkDown","message":"Network is down (os error 100)"}}
```

After a round trip the kind and message survive, but `raw_os_error()` returns `None`, any `source()` chain is gone, and a kind with no stable name — or one introduced by a newer version of the crate — arrives as `ErrorKind::Other`. Everything else round-trips exactly, including the `Can` variant and the parser's own non-`Io` variants.

## Testing

Integrating the full suite of tests into a CI system is non-trivial as it relies on a `vcan0` virtual CAN device existing. Adding it to most Linux systems is pretty easy with root access, but attaching a vcan device to a container for CI seems difficult to implement.

Therefore, tests requiring `vcan0` were placed behind an optional feature, `vcan_tests`.

The steps to install and add a virtual interface to Linux are in the `scripts/vcan.sh` script. Run it with root privileges, then run the tests:

```sh
$ sudo ./scripts/vcan.sh
$ cargo test --features=vcan_tests
```

Note that a few tests in the `nl` module are behind a separate `netlink_tests` feature. Those create and destroy temporary CAN interfaces, so they require root privileges and will fail with a netlink `NoAck` error when run as a normal user.

### Testing Error Frames

Error frames can be injected by hand with `cansend` from [can-utils](https://github.com/linux-can/can-utils), which is useful for exercising the error decoding without needing a misbehaving bus.

An error frame is sent by using **eight hex digits** for the ID with the `CAN_ERR_FLAG` bit (`0x20000000`) set. The can-utils parser only adds `CAN_EFF_FLAG` when that bit is *absent*, so an ID of the form `2000xxxx` is transmitted verbatim as an error frame rather than as an extended-ID data frame.

The ID carries the error class bits, and the eight data bytes carry the detail for each class:

| ID bit  | Class               | Detail bytes                                        |
|---------|---------------------|-----------------------------------------------------|
| `0x001` | TX timeout          | —                                                   |
| `0x002` | Lost arbitration    | `data[0]` bit number (0 = unspecified)              |
| `0x004` | Controller problem  | `data[1]` **bitfield**                              |
| `0x008` | Protocol violation  | `data[2]` **bitfield** (type), `data[3]` (location) |
| `0x010` | Transceiver status  | `data[4]` two nibbles, CAN Low \| CAN High          |
| `0x020` | No ACK              | —                                                   |
| `0x040` | Bus off             | —                                                   |
| `0x080` | Bus error           | —                                                   |
| `0x100` | Restarted           | —                                                   |
| `0x200` | Error counters      | `data[6]` TX count, `data[7]` RX count              |

`data[5]` is reserved by the kernel and is never decoded. Several class bits are commonly set at once, and `data[1]`, `data[2]` and `data[4]` each describe more than one condition at a time, so one frame usually decodes to a single error with several causes.

Some frames worth testing, each taken from the behavior of a real in-tree driver:

```sh
# Controller state change: CRTL|CNT with both TX and RX warning bits set in
# data[1]. This is what the kernel's shared can_change_state() helper emits
# whenever the two states match. Decodes to 2 causes.
$ cansend vcan0 '20000204#000C000000007060'

# Bus error: PROT|BUSERROR|ACK with five violation bits in data[2] and a
# location in data[3], as mcp251xfd_handle_ivmif() builds it. Decodes to 3.
$ cansend vcan0 '200000A8#00009E0800000000'

# Five classes on one frame: LOSTARB|CRTL|PROT|BUSERROR|CNT, as sja1000_err()
# builds it when a single pass through the ISR sees a data overrun, a lost
# arbitration (data[0] = bit 12) and a bus error together. That driver copies
# the raw 5-bit ECC segment into data[3], so locations that
# linux/can/error.h never names show up there: 0x11 is the active error flag.
# Decodes to 5 causes.
$ cansend vcan0 '2000028E#0C01811100000030'

# Transceiver fault on both lines: TRX with data[4] = 0x44, as the
# etas_es58x driver reports a lost connection. Decodes to 1 cause.
$ cansend vcan0 '20000010#0000000044000000'

# Bus off on its own. Decodes to 1 cause.
$ cansend vcan0 '20000040#0000000000000000'
```

Error frames are **not delivered to a socket by default** — the receiving side must opt in by setting an error mask, otherwise the frames above are silently dropped. In this library that is `set_error_mask(ERR_MASK_ALL)`; with `candump` it is the `#<error_mask>` filter term:

```sh
# -e decodes error frames, 0:0 keeps ordinary data frames visible,
# and #FFFFFFFF enables every error class.
$ candump -e vcan0,0:0,#FFFFFFFF
```

To see them decoded by this library instead, use the frame-printing examples, which enable the error mask and print every condition a frame reports:

```sh
$ cargo run --features tokio --example tokio_print_frames vcan0
$ cargo run --features smol --example smol_print_frames vcan0
```

A caveat when cross-checking against `candump`: releases of can-utils from 2023 and earlier predate the kernel's `CAN_ERR_CNT` class (added in Linux 5.19), and reject any frame that sets it with `Error class 0x204 is invalid`, printing the raw bytes without decoding them. Since `CAN_ERR_CNT` accompanies most controller state changes, use the examples above for those frames or build can-utils from git. Note also that `candump` does not decode `data[4]` at all, reporting only `transceiver-status`, whereas this library resolves both nibbles into their individual CAN High and CAN Low faults.