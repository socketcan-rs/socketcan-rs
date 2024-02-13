// socketcan/examples/enumerate.rs
//
// Example application for listing available SocketCAN interfaces
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! An example that lists available SocketCAN interfaces.

use socketcan::available_interfaces;

fn main() {
    match available_interfaces() {
        Ok(interfaces) => {
            match interfaces.len() {
                0 => println!("No CAN interfaces found."),
                1 => println!("Found 1 CAN interface:"),
                n => println!("Found {} CAN interfaces:", n),
            };

            for iface in interfaces {
                println!("{}", iface);
            }
        }
        Err(e) => {
            eprintln!("{:?}", e);
            eprintln!("Error listing CAN interfaces")
        }
    }
}
