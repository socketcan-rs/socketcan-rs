# CHANGELOG

The change log for the Rust [socketcan](https://crates.io/crates/socketcan) library.


## Unreleased features

- `CanAnyFrame` implements `From` trait for `CanDataFrame`, `CanRemoteFrame`, and `CanErrorFrame`.
- `CanFdSocket` implementa `TryFrom` trait for `CanSocket`
- Added FdFlags::FDF bit mask for CANFD_FDF
    - The FDF flag is forced on when creating a CanFdFrame.
- Updates to dump module:
    - Re-implemented with text parsing
    - `ParseError` now implements std `Error` trait via `thiserror::Error` 
    - Parses FdFlags field properly 
    - CANFD_FDF bit flag recognized on input
    - Fixed reading remote frames
    - Now reads remote length
    - `CanDumpRecord` changes:
        - Removed lifetime and made `device` field an owned `String`
	- Implmentd `Clone` and `Display` traits.
    - `dump::Reader` is now an Iterator itself
        - Returns full `CanDumpRecord` items
    - New unit tests
- [#59](https://github.com/socketcan-rs/socketcan-rs/issues/59) Embedded Hal for CanFdSocket


## [Version 3.4.0](https://github.com/socketcan-rs/socketcan-rs/compare/v3.3.1..v3.4.0)  (2024-12-26)

- Re-implemented CAN raw sockets using [socket2](https://crates.io/crates/socket2)
- Added a 'CanId' type with more flexibility than embedded_can::Id
- Moved from UD utility functions and types from frame module to id
- Added a CAN FD example, [echo_fd](https://github.com/socketcan-rs/socketcan-rs/blob/master/examples/echo_fd.rs)
- Split out `CanAddr` and related code into a new `addr` module.
- New `CanRawFrame` encapsulatea either type of libc, raw, CAN frame (Classic or FD)
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

