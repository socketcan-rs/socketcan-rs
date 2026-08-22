# CHANGELOG

The change log for the Rust [socketcan](https://crates.io/crates/socketcan) library.

## Version 4.0.0  (Unreleased)

- Bumped MSRV to v1.89.0
- Bumped Rust Edition to 2024
- Removed direct support for `async-std` which is no longer maintained.
- [#88](https://github.com/socketcan-rs/socketcan-rs/pull/88) Updated _smol_ to v2.0
- [#99](https://github.com/socketcan-rs/socketcan-rs/pull/99) Update `neli` and `clap`
- Replaced the unmaintained `libudev` 0.3 dependency with `udev` 0.9 for interface enumeration (`enumerate` feature).
    - `udev` reports errors as `io::Error`, so the bespoke `From<libudev::Error> for Error` conversion was removed
- Updated `socket2` to v0.6
    - The `From<CanAddr> for SockAddr` now fills socket2's new `SockAddrStorage` wrapper type
- Updated `nix` to v0.31 (no API changes required) and dropped the unused `process` feature (only `poll` and `net` are used)
- Dropped the `itertools` dependency. Its only use was joining a frame's payload bytes into a hex string when formatting a candump record, which now writes them straight to the formatter, as the `Frame` hex formatting already did — one allocation per byte fewer, and no dependency
- Updated the `serial_test` dev-dependency to v3.5 (no API changes required)
- Updated the `futures-timer` dev-dependency to v3.0. Its `Delay` future now yields `()` instead of `io::Result<()>`, so the `.await?` in the `tokio_send`/`smol_send` examples became `.await`
- **Breaking:** the `AsPtr::as_bytes` and `AsPtr::as_bytes_mut` trait methods are now `unsafe fn` with a documented `# Safety` contract (reading uninitialised padding through the returned byte slice is UB). This completes the soundness fix begun in 3.6.0 (which marked the free `as_bytes`/`as_bytes_mut` helpers `unsafe`). Callers of these trait methods must wrap calls in `unsafe`
- **New optional `serde` feature** (non-default) for serializing frames, errors, and interface configuration
    - Frames (`CanFrame`, `CanAnyFrame`, `CanDataFrame`, `CanRemoteFrame`, `CanErrorFrame`, `CanFdFrame`), `CanId`, `IdFlags`/`FdFlags`, `CanFilter`, `CanTimestamps`, `dump::CanDumpRecord`, every error type, and the netlink configuration types (`InterfaceDetails`, `InterfaceCanParams`, `CanBitTiming`, `CanCtrlModes`, `Mtu`, `CanState`, …)
    - The frame types wrap the C `can_frame`/`canfd_frame`, which have no serde impls, so they convert through logical repr types (`CanDataFrameRepr` and friends). The raw C structs are deliberately never serialized: they contain padding and possibly-uninitialised bytes, and their layout is not portable. Deserializing routes back through the normal constructors, so an out-of-range identifier or an over-long payload is rejected rather than producing a malformed frame
    - Frame payloads serialize through `serialize_bytes`, so formats with a native byte-string type (MessagePack, CBOR, bincode) use it instead of a sequence of integers. JSON, which has no byte-string type, renders a byte array
    - `CanError` serializes as a flat array of its causes rather than exposing its internal storage, and deserializing an empty array is an **error** — the non-empty invariant holds across serde
    - `IdFlags`/`FdFlags` serialize as flag-name strings (`"BRS | FDF"`) via `bitflags`' serde support
    - `Error::Can` round-trips exactly. An `io::Error` is **lossy** by necessity, since it implements neither serde trait: it is reduced to an `ErrorKind` name plus the message. After a round trip `raw_os_error()` is `None`, any `source()` chain is gone, and a kind with no stable name (or one a newer version introduced) arrives as `ErrorKind::Other`. This applies both to `Error::Io` and to the `Io` variant of a `dump::ParseError` nested in `Error::Parser`; the parser's other variants round-trip exactly
    - Unset `Option` fields on the netlink configuration types are omitted rather than written as nulls, and are optional on input, so a config file need only carry the parameters you care about
    - Useful in particular for keeping interface configuration — bitrates, control modes — in a JSON or TOML file
- **candump log format:** error frames are now handled in both directions by the `dump` module. Previously neither worked — the parser rejected an error line outright (the error-class bits sit above `CAN_EFF_MASK`, so decoding the ID failed), while `Display` wrote the ID with `CAN_ERR_FLAG` stripped, so its own output re-parsed as a *standard data frame*
    - The parser branches on `CAN_ERR_FLAG` before decoding the identifier. An eight-digit ID field is ambiguous — extended data frame or error frame — and that bit is the only thing distinguishing them, which is how can-utils resolves it too
    - `Display` now emits the ID with `CAN_ERR_FLAG` included, so a line round-trips exactly. Confirmed against real `candump -L` output
    - Parsed error frames flow through the new `CanError` decoding, so a multi-condition error line decodes fully
    - The `dump` module docs now state the full grammar, cite the can-utils `parse_canframe()` comment as the source of truth, and cross-reference `doc/CanDumpLogFormat.md`
    - Not yet supported: the `_len8_dlc` suffix (`123#1122334455667788_E`), which is still rejected with `ParseError::InvalidCanFrame`
- **Breaking:** an error frame now decodes into *all* the conditions it describes, not just one. A single SocketCAN error frame routinely reports several at once, and the old decoder discarded almost all of them
    - New `ErrorCause` type carrying one condition: `TransmitTimeout`, `LostArbitration(u8)`, `Controller(ControllerProblems)`, `Protocol { types, location }`, `Transceiver { canh, canl }`, `NoAck`, `BusOff`, `BusError`, `Restarted`, `Counters { tx, rx }`, `DecodingFailure` and `Unknown(u32)`. It is `#[non_exhaustive]`, since future kernels may define more error class bits
    - `CanError` is now the whole decoded frame: **one** error with a non-empty list of causes, one per class bit set in the frame's CAN ID. It has `new()`, `from_multiple()`, `from_iter_checked()`, `first()`, `last()`, `len()`, `is_single()`, `causes()`, `contains_kind()`, both `IntoIterator` impls, `Display` and `embedded_can::Error`
    - Typed accessors (`lost_arbitration()`, `controller()`, `protocol()`, `transceiver()`, `counters()`) and predicates (`is_transmit_timeout()`, `is_no_ack()`, `is_bus_off()`, `is_bus_error()`, `is_restarted()`, `has_counters()`) reach a specific cause without matching over the list by hand
    - `Error::Can` still carries a `CanError`, `CanErrorFrame::into_error()` keeps its name, and `From<CanErrorFrame> for CanError` still exists — all three now yield every condition the frame reported rather than a single one
    - Previously the decoder matched the *entire* error-class word against one value, so any frame setting more than one class bit fell through to `Unknown(bits)` and the whole error was lost. For example an `mcp251xfd` bus-error frame (`CAN_ERR_PROT | CAN_ERR_BUSERROR | CAN_ERR_ACK`) decoded to `Unknown(0xA8)`; it now decodes to three causes, the protocol one naming all five violation types it reported
    - Cause order is part of the contract: ascending error class bit. Class bits the crate does not know are collected into a single trailing `Unknown` cause rather than being dropped
- **Breaking:** the bitfield data bytes are modeled as sets rather than as single values. The `ControllerProblem` and `ViolationType` enums (one variant per bit) are replaced by the `bitflags` types `ControllerProblems` and `ViolationTypes`, so `data[1]` and `data[2]` each decode to one cause carrying every condition the byte reports
    - The kernel's shared `can_change_state()` helper ORs both the TX and RX state codes into `data[1]` whenever the two states match, so `data[1] = 0x0C` is the normal encoding of a symmetric warning transition — it used to decode to `DecodingFailure`, and now yields `Controller(RX_WARNING | TX_WARNING)`
    - Callers that need to walk a bitfield themselves get the whole `bitflags` API: `all()`, `bits()`, `contains()`, `iter()` and the set operators
    - `ControllerProblem::Active` is now the `ControllerProblems::ACTIVE` flag, which displays as "back to error active". The kernel means "recovered *to* error-active state", which the old name read as the opposite
    - Either flags type renders the names of the conditions it holds, comma-separated, and never renders as nothing: a set naming no known condition reads as "unspecified", the same as the empty set. Only `from_bits_retain()` can build such a set — the decoder truncates unknown bits — but the rendering no longer depends on that
- **Breaking:** `ErrorCause::Counters { tx, rx }` covers `CAN_ERR_CNT` frames, which were previously dropped entirely. Added `CAN_ERROR_WARNING_THRESHOLD`, `CAN_ERROR_PASSIVE_THRESHOLD` and `CAN_BUS_OFF_THRESHOLD` re-exports for interpreting the counter values
- **Breaking:** the `TransceiverError` enum is replaced by `ErrorCause::Transceiver { canh: Option<CanHighFault>, canl: Option<CanLowFault> }`, decoded from `data[4]` — which was previously never read at all, leaving the whole `TransceiverError` enum unreachable. `data[4]` is two independent nibbles (CAN High in the low half, CAN Low in the high half), so a fault on both lines is one cause naming both
- **Breaking:** `Location` gains the five codes that real controllers emit but `linux/can/error.h` does not name (`ActiveErrorFlag`, `TolerateDominantBits`, `PassiveErrorFlag`, `ErrorDelimiter`, `OverloadFlag`) — the `sja1000` driver copies the raw 5-bit error-code-capture segment straight into `data[3]`. Unknown codes are now preserved as `Location::Reserved(u8)`, so decoding a location cannot fail; `TryFrom<u8> for Location` is replaced by the infallible `Location::from_raw()`/`as_raw()`
- **Breaking:** removed four `CanErrorDecodingFailure` variants that no code path could produce: `NotAnError`, `UnknownErrorType`, `NotEnoughData` (leftovers from an older decoder) and `InvalidLocation` (now unreachable, per the above)
- **Breaking:** the top-level `Error` gains a `Parser` variant (feature `dump`) carrying a `dump::ParseError`, replacing the `From<ParseError> for Error` conversion that flattened every parse error into an `io::Error` of kind `InvalidData`, keeping only its message. Parse errors now keep their identity, and `?` still lifts them into `socketcan::Error`
- **Breaking:** the top-level `Error` gains an `Nl` variant (feature `netlink`) carrying the new `nl::NlError`, replacing the `From<RouterError<T, P>> for Error` conversion that flattened every netlink failure into an `io::Error` of kind `Other` holding nothing but the message text
    - `NlError` keeps what a caller can act on: `Netlink { errno }` for an error packet from the kernel — with `errno()` and `io_kind()` accessors, so a privileged operation refused to a normal user tests as `PermissionDenied` — plus `NoAck`, `UnexpectedAck`, `BadSeqOrPid { seq, pid }`, `ClosedChannel`, and `Msg(String)` for the message-level failures that have no structure worth keeping
    - Genuine I/O failures now stay `Error::Io` with their original `ErrorKind`, and with their errno where neli's socket layer had one, instead of arriving as kind `Other`
    - `NlError` is an owned, non-generic *summary* rather than the neli error itself. `RouterError<T, P>` is generic over the message type and payload and is 128 bytes wide, so carrying it would both put neli into this crate's public API — making a neli major bump breaking for downstream — and grow every `Result<_, Error>` from the current 48 bytes to match
    - Round-trips exactly through serde, holding neither an `io::Error` nor any neli type
    - The summarizing itself is `From<neli::err::RouterError<T, P>> for NlError`, so code holding a neli error can reduce it directly. The crate-level `From<…> for Error` conversions only decide the split, keeping a genuine I/O failure as `Error::Io` and handing everything netlink-shaped to `NlError`
- **Breaking:** every `CanInterface` method now returns the crate-level `Result` instead of `RouterInfoResult` (that is, `Result<_, neli::err::RouterError<Rtm, Ifinfomsg>>`), and `delete()` returns `Result<(), (Self, Error)>`. Callers that matched a `RouterError` now match `Error::Nl(NlError)` or `Error::Io`
    - neli no longer appears in any public signature of the `nl` module: the `RouterInfoError` and `RouterInfoResult` aliases are gone, and the `TryFrom` impls that converted a neli `Rtattr`/`RtBuffer` to and from `InterfaceCanParams` are now the internal `InterfaceCanParams::from_link_info()` and `InterfaceCanParams::to_rtbuffer()`. What remains are the `From<…> for Error` impls for neli's error types, which is what lets `?` convert at the boundary
    - With the 128-byte error out of the module's signatures, the module-wide `#![allow(clippy::result_large_err)]` and its TODO are gone
- **Breaking:** `CanInterface::open()` returns the crate-level `Result` as well, rather than `Result<Self, nix::Error>`. Together with the change above, that leaves no foreign error type anywhere in the `nl` module's API — every function there reports `socketcan::Error`
    - New `From<nix::Error> for Error`, mapping an errno from a `nix` call onto `Error::Io`. Nothing is lost, since `nix::Error` *is* an errno: opening an unknown interface still reports `ENODEV`, now reachable through `io::Error::raw_os_error()` and `kind()` like any other system error
    - The rendered message changes with the type, so a program printing the error shows `No such device (os error 19)` where nix would have written `ENODEV: No such device`
- `ErrorCause::kind()` now maps protocol violations onto the specific `embedded_can::ErrorKind` values `Bit`, `Form` and `Stuff` rather than reporting everything as `Other`. `CanError::kind()` scans its causes and returns the first specific kind present, so a frame carrying both a controller warning and a missing ACK reports `Acknowledge`
- `CanError`'s `Display` joins its causes with "; ". A protocol violation names its location once and lists every type reported there — `protocol violation at CRC sequence: frame format error, bit stuffing error, …`
- `ErrorCause::Unknown(_)` is now printed in hex
- The error-class bit constants (`CAN_ERR_CRTL`, `CAN_ERR_CNT`, …) are re-exported from `socketcan::errors`, so building or inspecting error frames no longer needs a direct `libc` dependency. Internally the decoder now uses these constants instead of hardcoded hex literals
- Added `CanInterface::ctrlmodes()` getter (feature `netlink`) to pair with `set_ctrlmodes()`, returning the kernel-reported control-mode bits as `Option<CanCtrlModes>`
- `CanInterface::set_data_bitrate()` now has the same debug-build sanity checks as `set_bitrate()` (bitrate and sample-point range), with an FD-appropriate upper bound of 8 Mbit/s for the data phase


## [Version 3.6.1](https://github.com/socketcan-rs/socketcan-rs/compare/v3.6.0..v3.6.1)  (2026-06-10)

- [#101](https://github.com/socketcan-rs/socketcan-rs/pull/101) Add libc::ioctl fix for musl targets
    Fixed broken build in v3.6.0 for musl targets


## [Version 3.6.0](https://github.com/socketcan-rs/socketcan-rs/compare/v3.5.0..v3.6.0)  (2026-06-09)

- Added ability to get timestamp for received frames
    - New `CanTimestamps` type carrying socket-layer, network-stack software, and hardware receive timestamps
    - `SocketOptions::set_recv_timestamp` (`SO_TIMESTAMPNS`) and `SocketOptions::set_timestamping` (`SO_TIMESTAMPING`) to enable delivery on the socket
    - `Socket::read_frame_with_timestamp`, `Socket::read_frame_with_timestamps`, and `Socket::read_frame_with_hw_timestamp` on the `Socket` trait (default implementations return `ENOSYS` to preserve semver for out-of-tree `Socket` implementors)
    - `CanSocket::has_hw_timestamps` / `CanFdSocket::has_hw_timestamps` query interface capability via `ETHTOOL_GET_TS_INFO`
    - Re-exports for the `SOF_TIMESTAMPING_*` flag constants from the crate root
    - All read methods deliver the frame and ancillary timestamp data in a single `recvmsg()` call, eliminating the race window of the old `SIOCGSTAMPNS` approach
    - Async equivalents on the `tokio::CanSocket`/`CanFdSocket` and `async_io::CanSocket`/`CanFdSocket` wrappers
- `async_io::CanSocket` and `async_io::CanFdSocket` gained `open_if(ifindex: u32)` and `open_addr(&CanAddr)` constructors (parity with the tokio wrappers, which previously had all three)
- `async_io::CanSocket` and `async_io::CanFdSocket` now implement `futures::Stream` (yielding `Result<CanFrame>` / `Result<CanAnyFrame>`) and `futures::Sink` (over `CanFrame` / `CanAnyFrame`), parity with the tokio wrappers. The `async-io`, `async-std`, and `smol` features now pull in `futures` (previously it was wired in only via the `tokio` feature)
- `async_io::CanSocket` and `async_io::CanFdSocket` gained `try_read_frame()` and `try_write_frame()` methods, parity with the tokio wrappers (added in #84). Both return `WouldBlock` when no frame is available / send buffer is full and go straight to the underlying non-blocking fd (bypassing the async-io reactor); mixing with the async-path methods is safe
- New example `tokio_recvts` — tokio mirror of `can_recvts`, prints software and hardware timestamps alongside each frame
- Bumped MSRV to v1.75.0
- All frame types now derive `PartialEq`, `Eq`, and `Hash` — both the concrete frame structs (`CanDataFrame`, `CanRemoteFrame`, `CanErrorFrame`, `CanFdFrame`) and the wrapper enums (`CanFrame`, `CanAnyFrame`, `CanRawFrame`). Equality is field-wise on the underlying `libc::can_frame` / `libc::canfd_frame`, which means it includes every byte of the structure (id, dlc, flags, the libc `__pad`/`__res0` fields, and the full data array). Note that `set_data` does not zero the unused trailing bytes of `can_frame::data`, so two semantically-equivalent frames built by different code paths may still compare unequal — callers should treat equality as "byte-identical wire image" rather than "same logical frame".
- Enabled the `extra_traits` feature on the `libc` dependency so the trait derives can flow through (`libc::can_frame` / `canfd_frame` only `derive(PartialEq, Eq, Hash)` when that feature is on).
- Bug fixes:
    - `recvmsg()` ancillary control buffer is now properly aligned and validated; `MSG_TRUNC`/`MSG_CTRUNC` handled correctly
    - `timespec_to_duration` no longer wraps on a negative `tv_sec` in release builds
    - `From<canfd_frame> for CanFdFrame` normalises non-spec lengths so `dlc()` and `data()` stay consistent and no uninitialised bytes can leak
    - `TryFrom<can_frame> for CanErrorFrame` forces `can_dlc = CAN_MAX_DLEN` so the len/dlc/data invariant holds
    - `CanDataFrame::set_id` and `CanFdFrame::set_id` preserve `CAN_ERR_FLAG`/`CAN_RTR_FLAG` bits in the ID word
    - `CanId + u32` no longer panics on overflow in debug builds
    - `AsPtr::as_bytes_mut` now returns `&mut [u8]` instead of `&[u8]`
    - `rcan` CLI no longer contains duplicate `loopback` subcommand arms
    - `examples/can_recvts.rs` now requests the full set of timestamp flags so software and hardware timestamps actually arrive
    - `examples/fd_send.rs` now sends an actual CAN FD frame
    - `fmt::UpperHex` on classic frames uses `raw_id()` (no flag-bit leakage), zero-pads the ID to 3 chars (SFF) / 8 chars (EFF), joins data bytes without spaces, and emits `#R<dlc>` for remote frames so the output matches candump's log format
    - `fmt::UpperHex` on `CanFdFrame` prints the FD flags as a single hex nibble between `##` and the data bytes (no stray space)
    - `CanRemoteFrame::data()` now returns `&[]` (spec-correct: remote frames carry only a DLC); use `dlc()` to read the requested length
    - `CanInterface::create` rejects names of length `IFNAMSIZ` and above (off-by-one — `IFNAMSIZ` includes the trailing NUL)
    - `CAN_TERMINATION_DISABLED` is now `u16` (matches the rest of the termination API)
    - `From<libudev::Error>` preserves the underlying description on the wrapped `io::Error`
    - `CanAddr` gained hand-rolled `PartialEq`/`Eq`/`Hash` impls comparing `(can_family, can_ifindex)` only; deriving them would compare the `can_addr` union plus padding, which is unsound
    - `CanAddr::Debug` now renders the `can_addr` union bytes (J1939 / ISO-TP fields are no longer dropped)
    - `From<sockaddr_can> for CanAddr` now `debug_assert!`s `can_family == AF_CAN`
    - `available_interfaces()` was silently ignoring udev errors and returning an empty list of interfaces. It now returns an error on udev failure.
    - Tokio `Sink::poll_close` no longer attempts a spurious `clear_ready()`; `Sink::start_send` issues a single non-blocking `write_frame()` instead of busy-retrying via `write_frame_insist`
    - Typo: "socke options" → "socket options" in `set_socket_option_mult` doc
    - `dump::Reader` caps each line at 64 KiB so a malformed or hostile log can't OOM the reader; over-long lines produce `InvalidCanFrame`
    - `dump::Reader` requires exactly six mantissa digits on the timestamp (real candump format), and uses checked arithmetic so an overflow errors instead of producing a wrong timestamp
    - `dump::Reader` propagates remote-frame DLC parse errors via `InvalidCanFrame` (previously silently coerced to 0); the DLC is now parsed as a hex nibble matching candump's `R<X>` format
    - `dump::CanDumpRecord` `Display` now emits parseable lines for error frames (`<error_bits>#<8 hex bytes>`) and FD frames (`##<flag-nibble><bytes>`), and zero-pads the ID width (3 hex for SFF, 8 hex for EFF) on all variants
- New `Error` conversions:
    - `From<neli::err::NlError<T, P>>` (feature `netlink`) — netlink errors flow into the crate-level `Error` via `io::Error::other`
    - `From<dump::ParseError>` (feature `dump`) — dump-parse errors flow into `Error` via `io::Error::new(InvalidData, …)` (passing through I/O variants)
- Docs:
    - `Socket::read_frame` documents concurrent-reader semantics (each `&self` reader sees a disjoint subset of frames)
    - `CanCtrlModes::has_mode` documents that it inspects `flags` (kernel-reported state) and ignores pending `mask` bits
    - `CanFdFrame::new_remote` documents that CAN FD has no RTR by spec, so the method always returns `None`
- Internals:
    - `crate::as_bytes` / `crate::as_bytes_mut` helpers are now `unsafe fn` with a proper `# Safety` contract; call sites annotated
- Issues & PR's
    - [#89](https://github.com/socketcan-rs/socketcan-rs/issues/89) CanInterface binds to hardcoded nl_pid
    - [#81](https://github.com/socketcan-rs/socketcan-rs/pull/81) Remove explicit 'mio' dependency.


## [Version 3.5.0](https://github.com/socketcan-rs/socketcan-rs/compare/v3.4.0..v3.5.0)  (2024-12-29)

- `CanAnyFrame` implements `From` trait for `CanDataFrame`, `CanRemoteFrame`, and `CanErrorFrame`.
- `CanFdSocket` implementa `TryFrom` trait for `CanSocket`
- Added FdFlags::FDF bit mask for CANFD_FDF
    - The FDF flag is forced on when creating a CanFdFrame.
- Updates to `dump` module:
    - Re-implemented with text parsing
    - `ParseError` now implements std `Error` trait via `thiserror::Error`
    - Parses FdFlags field properly
    - CANFD_FDF bit flag recognized on input
    - Fixed reading remote frames
    - Now reads remote length
    - `CanDumpRecord` changes:
        - Removed lifetime and made `device` field an owned `String`
	- Implemented `Clone` and `Display` traits.
        - `Display` trait is compatible with the candump log record format
    - `dump::Reader` is now an Iterator itself, returning full `CanDumpRecord` items
    - New unit tests
- [#59](https://github.com/socketcan-rs/socketcan-rs/issues/59) Embedded Hal for CanFdSocket


## [Version 3.4.0](https://github.com/socketcan-rs/socketcan-rs/compare/v3.3.1..v3.4.0)  (2024-12-26)

- Re-implemented CAN raw sockets using [socket2](https://crates.io/crates/socket2)
- Added a 'CanId' type with more flexibility than embedded_can::Id
- Moved from UD utility functions and types from frame module to id
- Added a CAN FD example, [echo_fd](https://github.com/socketcan-rs/socketcan-rs/blob/master/examples/echo_fd.rs)
- Split out `CanAddr` and related code into a new `addr` module.
- New `CanRawFrame` encapsulates either type of libc, raw, CAN frame (Classic or FD)
- Raw frame reads for CanSocket and CanFdSocket.
- Implemented `Read` and `Write` traits for `CanSocket`
- InterfaceCanParams now has all items as Option<>. Can be used to get or set multiple options.
- [#58](https://github.com/socketcan-rs/socketcan-rs/pull/58) Add new API to enumerate available SocketCAN interfaces
- [#60](https://github.com/socketcan-rs/socketcan-rs/pull/60) Make `CanState` public
- [#61](https://github.com/socketcan-rs/socketcan-rs/pull/61) `CanFdSocket` read_frame crash fix
- [#64](https://github.com/socketcan-rs/socketcan-rs/pull/64) Make termination u16 and add `set_termination`
- [#65](https://github.com/socketcan-rs/socketcan-rs/pull/65) Dump parsing also optionally trims off CR at the line end
- [#66](https://github.com/socketcan-rs/socketcan-rs/pull/66) 1CanInterface1: add 1set_can_params1 method to set multiple parameters
- [#67](https://github.com/socketcan-rs/socketcan-rs/pull/67) Improved tokio async implementation
- [#68](https://github.com/socketcan-rs/socketcan-rs/pull/68) remove unnecessary qualifications
- [#73](https://github.com/socketcan-rs/socketcan-rs/pull/73) Update some dependencies
    - `itertools` to v0.13, `nix` to v0.29, `bitflags` to v2.6, `mio` to v1
- [#74](https://github.com/socketcan-rs/socketcan-rs/issues/74) CanFDFrames with ExtendedID are not correctly parsed by socketcan::dump::Reader
- [#75](https://github.com/socketcan-rs/socketcan-rs/pull/75) Fix DLC and add padding for CANFD frames
- [#76](https://github.com/socketcan-rs/socketcan-rs/pull/76) Add CanCtrlModes::has_mode(mode: CanCtrlMode)
- [#80](https://github.com/socketcan-rs/socketcan-rs/pull/80) Friendly non-Linux compilation error
    - Remove unused byte_conv dependency


## [Version 3.3.1](https://github.com/socketcan-rs/socketcan-rs/compare/v3.3.0..v3.3.1)  (2023-10-27)

- [#78](https://github.com/socketcan-rs/socketcan-rs/issues/78) Memory error receiving CAN FD frames.


## [Version 3.3.0](https://github.com/socketcan-rs/socketcan-rs/compare/v3.2.0..v3.3.0)  (2023-10-27)

- [#53](https://github.com/socketcan-rs/socketcan-rs/pull/53) Added CanFD support for tokio
- Serialized tokio unit tests and put them behind the "vcan_tests" feature


## [Version 3.2.0](https://github.com/socketcan-rs/socketcan-rs/compare/v3.1.0..v3.2.0)  (2023-10-16)

- [#32](https://github.com/socketcan-rs/socketcan-rs/issues/32) Further expanded netlink functionality:
    - Added setters for most additional interface CAN parameters
    - Ability to query back interface CAN parameters
    - Expanded `InterfaceDetails` to include CAN-specific parameters
    - Better integration of low-level types with `neli`
    - Significant cleanup of the `nl` module
    - Split the `nl` module into separate sources for higher and lower-level code


## [Version 3.1.0](https://github.com/socketcan-rs/socketcan-rs/compare/v3.0.0..v3.1.0)  (2023-10-12)

- [#32](https://github.com/socketcan-rs/socketcan-rs/issues/32) Added a number of netlink commands to modify the CAN interface parameters. including: setting the bitrate and (for FD) setting the data bitrate, setting control modes, manually restarting the interface, and setting the automatic restart delay time.
    - [PR #50](https://github.com/socketcan-rs/socketcan-rs/pull/50) Add set_bitrate method
- [PR #45](https://github.com/socketcan-rs/socketcan-rs/pull/45) Dump handles extended IDs
- [PR #44](https://github.com/socketcan-rs/socketcan-rs/pull/44) Fix clippy warnings
- [PR #43](https://github.com/socketcan-rs/socketcan-rs/pull/43) Implement AsPtr for CanAnyFrame


## [Version 3.0.0](https://github.com/socketcan-rs/socketcan-rs/compare/v2.0.0..v3.0.0)  (2023-09-19)

- Support for Rust async/await
    - All of [tokio-socketcan](https://github.com/oefd/tokio-socketcan) has been merged into this crate and will be available with an `async-tokio` build feature.
    - [#41](https://github.com/socketcan-rs/socketcan-rs/pull/41) Added initial support for `async-io` for use with `async-std` and `smol`
    - Split `SocketOptions` trait out of `Socket` trait for use with async (breaking)
    - Added cargo build features for `tokio` or `async-io`.
    - Also created specific build features for `async-std` and `smol` which just bring in the `async-io` module and alias the module name to `async-std` or `smol`, respectively, and build examples for each.


## [Version 2.1.0](https://github.com/socketcan-rs/socketcan-rs/compare/v2.0.0..v2.1.0)  (2023-09-19)

- Made `CanAddr` public and added functions to help interact with low-level sockaddr types. Sockets can now be opened with an address.
- Can create an `Error` directly from a `CanErrorFrame` or `std::io::ErrorKind`.
- [#46](https://github.com/socketcan-rs/socketcan-rs/issues/46)  Applications can create error frames:
    - `CanErrorFrame::new()` now works.
    - `CanErrorFrame::new_error()` is similar but more intuitive using a raw ID word.
    - `From<CanError> for CanErrorFrame` to create an error frame from a `CanError`.
- Added `Frame::from_raw_id()` and `Frame::remote_from_raw_id()`
- Bumped MSRV to 1.65.0


## Version 2.0.0  (2023-04-06)

Extensive rework of the crate to cleanup, refactor, and modernize the library and add some new features like CAN FD support.

- Moved to Rust Edition 2021 w/ MSRV 1.64
- Refactored frames into different types: Data, Remote, Error (and now FD), that can be managed through enumeraed wrapper types `CanFrame` and/or `CanFdFrame`
- Pushed some implementation upstream to the _libc_ and _nix_ crates, and/or adapted upstream types.
     - CAN 2.0 frames based on `libc::can_frame`
     - CAN FD frames based on `libc::canfd_frame`
- [#33](https://github.com/socketcan-rs/socketcan-rs/pull/33) Netlink extensions
    - Creating and deleting interfaces
    - Setting MTU (to/from FD)
- [#21](https://github.com/socketcan-rs/socketcan-rs/pull/21) New CI using GitHub Actions
- [#20](https://github.com/socketcan-rs/socketcan-rs/pull/20) Composite PR with some modernization
    - Pulls in [#13](https://github.com/socketcan-rs/socketcan-rs/pull/13), and updates to the latest `neli` v0.6
    - Updates `nix` dependency to latest v0.23
    - Moves to Rust 2018 w/ MSRV 1.54
    - Errors conform to std::error::Error
- [#16](https://github.com/socketcan-rs/socketcan-rs/pull/16) Add CAN FD support
- [#24](https://github.com/socketcan-rs/socketcan-rs/pull/24) Embedded HAL Traits
    - Plus some source refactoring into more coherent modules

