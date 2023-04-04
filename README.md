Rust SocketCAN
==============

This library allows Controller Area Network (CAN) communications on Linux using the SocketCAN interfaces. This provides a network socket interface to the CAN bus.

[Linux SocketCAN](https://docs.kernel.org/networking/can.html)

Please see the [documentation](https://docs.rs/socketcan) for details about the Rust API provided by this library.


## Latest News

The final set of features have been implemented for a v2.0 release!

Now some final test and cleanup, and the version will be published to crates.io. Please report any issues ASAP.

The v2.0 release is a fairly large rewrite of the library and adds the following features:

- CAN Flexible Data Rate (FD) support
- Proper handling of Extended CAN ID's
- Integration with the Rust Embedded HAL APIs for CAN
- Some control of the CAN network interfaces via netlink with the [neli](https://crates.io/crates/neli) crate.
- Tighter integration with [libc](https://crates.io/crates/libc) and [nix](https://crates.io/crates/nix) crates, including upstream
- Update to Rust Edition 2021, with updates to the dependencies.
- Standard errors conforming to `std::error::Error`
- Updated documentation

The `master` branch is currently a release candidate.
