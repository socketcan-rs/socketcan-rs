Rust SocketCAN support
======================

**Maintenance status**: This crate is in the process of entering renewed joint-maintership with [@fpagliughi](https://github.com/fpagliughi). Please stay patient for a while for this to get cleaned up. -- @mbr.

The Linux kernel supports using CAN-devices through a network-like API
(see https://www.kernel.org/doc/Documentation/networking/can.txt). This
crate allows easy access to this functionality without having to wrestle
libc calls.

Please see the [documentation](https://docs.rs/socketcan) for details.
