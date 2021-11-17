# Changelog
All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.3.1](https://github.com/oefd/tokio-socketcan/tree/0.3.1) - 2021-11-17
- Reexport `socketcan::CANFilter`. Thank you @andresv.
- Update to `mio 0.8`.

## [0.3.0](https://github.com/oefd/tokio-socketcan/tree/0.3.0) - 2021-02-10
- [BREAKING CHANGE] Migrate code to `tokio 1` and mio 0.7.

## [0.2.0](https://github.com/oefd/tokio-socketcan/tree/0.2.0) - 2020-11-12
- [BREAKING CHANGE] Migrate code to `tokio 0.2`, `futures 0.3` and therefore make it possible
  to use async/await syntax for reading and writing frames :rocket:.
- [BREAKING CHANGE] Add dependency to `thiserror 1.0` and introduce common error type `Error`.
- Update examples to async/await syntax.

## 0.1.3
- Fixed error events being effectively delayed in delivery until the next non-error arrived.

## [0.1.1](https://github.com/oefd/tokio-socketcan/tree/0.1.1) - 2019-01-10
- Added `futures::sink::Sink` implementation for the `CANSocket`
