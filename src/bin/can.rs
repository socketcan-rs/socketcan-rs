// socketcan-rs/src/bin/can.rs

//! Simple CLI tool to run basic CAN bus functionality from the Linux
//! command line, similar to 'can-utils'.

use anyhow::Result;
use clap::{
    App,
    Arg,
    SubCommand,
    //value_t_or_exit,
    crate_version,
};
use socketcan::*;
use std::process;

#[cfg(not(feature = "netlink"))]
use anyhow::anyhow;

// --------------------------------------------------------------------------

/// Bring the interface up or down
#[cfg(feature = "netlink")]
fn iface_up(iface_name: &str, up: bool) -> Result<()> {
    let iface = CanInterface::open(iface_name)?;
    if up {
        iface.bring_up()
    }
    else {
        iface.bring_down()
    }?;
    Ok(())
}

#[cfg(not(feature = "netlink"))]
fn iface_up(_iface_name: &str, _up: bool) -> Result<()> {
    Err(anyhow!(
        "The 'netlink' feature is required to bring an inteface up or down."
    ))
}

// --------------------------------------------------------------------------

fn main() {
    let opts = App::new("can")
        .version(crate_version!())
        .about("Command line tool to interact with the CAN bus on Linux")
        .help_short("?")
        .arg(Arg::with_name("iface")
            .help("The CAN interface to use, like 'can0', 'vcan0', etc")
            .required(true)
            .index(1))
        .subcommand(
            // Actually, we probably want 'up' and 'down' to be under an iface command?
            //   like "./can can0 iface [up | down]
            SubCommand::with_name("up")
                .about("Bring the interface up")
        )
        .subcommand(
            SubCommand::with_name("down")
                .about("Bring the interface up")
        )
        .get_matches();

    let iface_name = opts.value_of("iface").unwrap();

    let res = match opts.subcommand_name() {
        Some("up") => {
            iface_up(&iface_name, true)
        },
        Some("down") => {
            iface_up(&iface_name, false)
        },
        Some(_) | None => {
            eprintln!("Need to specify a subcommand (-? for help).");
            eprintln!("{}", opts.usage());
            process::exit(1);
        },
    };

    if let Err(err) = res {
        eprintln!("{}", err);
    }
}

