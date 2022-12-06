Rust SocketCAN
==============

This library allows Controller Area Network (CAN) communications on Linux using the SocketCAN interfaces. This provides a network socket interface to the CAN bus.

[Linux SocketCAN](https://docs.kernel.org/networking/can.html)

**Maintenance status**: This crate is in the process of entering renewed joint maintainership with [@fpagliughi

Please see the [documentation](https://docs.rs/socketcan) for details about the Rust API provided by this library.


## Latest News

After a period of some stagnation, this library is currently being upated for a v2.0 release to add the following features:

- CAN Flexible Data Rate (FD) support
- Proper handling of Extended CAN ID's
- Integration with the Rust Embedded HAL APIs for CAN
- Control of the CAN network interfaces via netlink with the [neli](https://crates.io/crates/neli) crate.
- Tighter integration with [libc](https://crates.io/crates/libc) and [nix](https://crates.io/crates/nix) crates, including upstream
- Updated documentation and dependencies

Note that the `master` branch will be in heavy flux over the next few weeks and should be assumed to be highly unstable.

