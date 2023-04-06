Rust SocketCAN
==============

This library allows Controller Area Network (CAN) communications on Linux using the SocketCAN interfaces. This provides a network socket interface to the CAN bus.

[Linux SocketCAN](https://docs.kernel.org/networking/can.html)

Please see the [documentation](https://docs.rs/socketcan) for details about the Rust API provided by this library.


## Latest News

Version 2.0 is finally released!

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

A number of items did not make it into the 2.0 release. These will be added in a follow-up v2.1, coming soon.

- Issue [#22](https://github.com/socketcan-rs/socketcan-rs/issues/22) Timestamps, including optional hardware timestamps
- Issue [#32](https://github.com/socketcan-rs/socketcan-rs/issues/32) Better coverage of the Netlink API to manipulate the CAN interfaces programatically.
- Better documentation. This README will be expanded with basic usage information, along with better doc comments, and perhaps creation of the wiki

We will also start looking into support of Rust async/await, prefereably in a portable way without lying on a particular library/executor. But certainly support for the main ones like Tokio would be the goal. Some folks have suggested putting this into a separate wrapper crate, but it would be better to add it here for convenience, but certainly made optional through a Cargo build feature.

## Minimum Supported Rust Version (MSRV)

The current version of the crate targets Rust Edition 2021 with an MSRV of Rust v1.64.0.

Note that, at this time, the MSRV is mostly diven by use of the `clap v4.0` crate for managing command-line parameters in the utilities and example applications. The core library could likely be built with an earlier version of the compiler if required.

