// socketcan/src/enumerate.rs
//
// Implements support for enumerating available SocketCAN network interfaces.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! SocketCAN interface enumeration.
//!
//! This module provides functionality to enumerate available SocketCAN
//! network interfaces. It uses the Linux `udev` facility to search for
//! matching devices.
//!
//! # Availability
//!
//! This module requires the `enumerate` cargo feature flag.

//! Requires the

use crate::Result;

use libc::ARPHRD_CAN;
use udev::Enumerator;

/// Scans the system for available SocketCAN network interfaces and returns a
/// list of them.
pub fn available_interfaces() -> Result<Vec<String>> {
    let mut interfaces = Vec::new();

    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("net")?;
    enumerator.match_attribute("type", ARPHRD_CAN.to_string())?;

    let devices = enumerator.scan_devices()?;
    for d in devices {
        if let Some(interface) = d.property_value("INTERFACE")
            && let Some(interface) = interface.to_str()
        {
            interfaces.push(String::from(interface));
        }
    }
    Ok(interfaces)
}
