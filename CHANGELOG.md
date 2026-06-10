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

