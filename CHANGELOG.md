# Changelog
All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.2.0](https://github.com/oefd/tokio-socketcan/tree/0.2.0) - 2020-11-12
- [BREAKING CHANGE] Migrate code to `tokio 0.2`, `futures 0.3` and therefore make it possible
  to use async/await syntax for reading and writing frames :rocket:.
- [BREAKING CHANGE] Add dependency to `thiserror 1.0` and introduce common error type `Error`.
- Update examples to async/await syntax.
