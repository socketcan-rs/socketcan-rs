// socketcan/build.rs
//
// The build file for the Rust SocketCAN library.
//
// The library and utilities can only be build for Linux.
// This build file is ensures the user gets a clear, concise error message
// when attempting to build on or for another target rather than a long list
// of confusing bugs.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");

    match env::var("CARGO_CFG_TARGET_OS") {
        Ok(val) if val == "linux" => Ok(()),
        _ => Err("Building for anything but Linux is not supported by socketcan".into()),
    }
}
