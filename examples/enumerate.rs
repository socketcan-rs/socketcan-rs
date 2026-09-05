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

//! An example that lists available SocketCAN interfaces and what each one
//! reports about itself.
//!
//! The per-interface details come from netlink, which needs no privileges to
//! read: an ordinary user can ask what an interface is and what its controller
//! supports. Only changing any of it requires `CAP_NET_ADMIN`.
//!
//! A field is shown as `-` when the driver does not report it.
//!
//! A `vcan` reports almost nothing, which is not an error since it has no
//! controller behind it.

use socketcan::{CanCtrlMode, CanInterface, available_interfaces, nl::InterfaceDetails};

/// The control modes, with the names the kernel and `ip` use for them.
const CTRL_MODES: [(CanCtrlMode, &str); 9] = [
    (CanCtrlMode::Loopback, "LOOPBACK"),
    (CanCtrlMode::ListenOnly, "LISTENONLY"),
    (CanCtrlMode::TripleSampling, "3_SAMPLES"),
    (CanCtrlMode::OneShot, "ONE_SHOT"),
    (CanCtrlMode::BerrReporting, "BERR_REPORTING"),
    (CanCtrlMode::Fd, "FD"),
    (CanCtrlMode::PresumeAck, "PRESUME_ACK"),
    (CanCtrlMode::NonIso, "FD_NON_ISO"),
    (CanCtrlMode::CcLen8Dlc, "CC_LEN8_DLC"),
];

// --------------------------------------------------------------------------

/// Names the modes a predicate accepts.
///
/// Takes a predicate rather than a mask because the two things printed below
/// are read differently: the enabled modes through `CanCtrlModes::has_mode()`,
/// and the supported ones from a raw `CAN_CTRLMODE_*` mask.
fn mode_names(is_set: impl Fn(CanCtrlMode) -> bool) -> String {
    let names: Vec<&str> = CTRL_MODES
        .iter()
        .filter(|(mode, _)| is_set(*mode))
        .map(|(_, name)| *name)
        .collect();

    match names.is_empty() {
        true => "none".to_string(),
        false => names.join(" | "),
    }
}

/// Prints what an interface reports about itself.
///
/// Everything here comes from the one `details()` query, rather than a call
/// per field: each getter would be its own netlink round trip.
fn print_details(details: &InterfaceDetails) {
    let can = &details.can;

    println!("    index:     {}", details.index);
    println!(
        "    state:     {}",
        match (details.is_up, can.state) {
            (true, Some(state)) => format!("up, {:?}", state),
            (true, None) => "up".to_string(),
            (false, Some(state)) => format!("down, {:?}", state),
            (false, None) => "down".to_string(),
        }
    );
    println!(
        "    mtu:       {}",
        match details.mtu {
            Some(mtu) => format!("{:?}", mtu),
            None => "-".to_string(),
        }
    );
    println!(
        "    clock:     {}",
        match can.clock {
            Some(clock) => format!("{} Hz", clock.freq),
            None => "-".to_string(),
        }
    );
    println!(
        "    bitrate:   {}",
        match (can.bit_timing, can.data_bit_timing) {
            (Some(bt), Some(dbt)) => format!("{} bps, data {} bps", bt.bitrate, dbt.bitrate),
            (Some(bt), None) => format!("{} bps", bt.bitrate),
            _ => "-".to_string(),
        }
    );
    println!(
        "    modes on:  {}",
        match can.ctrl_mode {
            Some(modes) => mode_names(|mode| modes.has_mode(mode)),
            None => "-".to_string(),
        }
    );
    println!(
        "    supported: {}",
        match can.ctrl_mode_supported {
            Some(mask) => format!("{:#06X}  {}", mask, mode_names(|m| mask & m.mask() != 0)),
            None => "-".to_string(),
        }
    );
}

// --------------------------------------------------------------------------

fn main() {
    let interfaces = match available_interfaces() {
        Ok(interfaces) => interfaces,
        Err(err) => {
            eprintln!("Error listing CAN interfaces: {}", err);
            std::process::exit(1);
        }
    };

    match interfaces.len() {
        0 => println!("No CAN interfaces found."),
        1 => println!("Found 1 CAN interface:"),
        n => println!("Found {} CAN interfaces:", n),
    };

    for name in interfaces {
        println!("\n  {}", name);

        // A query can still fail — the interface can go away between the
        // enumeration and here — so report it and carry on to the next.
        match CanInterface::open(&name).and_then(|iface| iface.details()) {
            Ok(details) => print_details(&details),
            Err(err) => println!("    query failed: {}", err),
        }
    }
}
