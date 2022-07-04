// socketcan-rs/src/bin/can.rs

//! Simple CLI tool to run basic CAN bus functionality from the Linux
//! command line, similar to 'can-utils'.

use anyhow::{anyhow, Result};
use clap::{
    App,
    Arg,
    ArgMatches,
    SubCommand,
};
use socketcan::CanInterface;
use std::process;

// Make the app version the same as the package.
const VERSION: &str = env!("CARGO_PKG_VERSION");

// --------------------------------------------------------------------------

/// Process the 'iface' subcommand.
///
/// Set parameters on the interface, or bring it up or down.
#[cfg(feature = "netlink")]
fn iface_cmd(iface_name: &str, opts: &ArgMatches) -> Result<()> {
    let iface = CanInterface::open(iface_name)?;

    match opts.subcommand_name() {
        Some("up") => {
            iface.bring_up()
        },
        Some("down") => {
            iface.bring_down()
        },
        Some("bitrate") => {
            return Err(anyhow!("Unimplemented"))
        },
        _ => return Err(anyhow!("Unknown 'iface' subcommand"))
    }?;
    Ok(())
}

#[cfg(not(feature = "netlink"))]
fn iface_cmd(_iface_name: &str, _opts: &ArgMatches) -> Result<()> {
    Err(anyhow!(
        "The 'netlink' feature is required to configure an inteface."
    ))
}

// --------------------------------------------------------------------------

fn main() {
    let opts = App::new("can")
        .author("Frank Pagliughi")
        .version(VERSION)
        .about("Command line tool to interact with the CAN bus on Linux")
        .help_short("?")
        .arg(Arg::with_name("iface")
            .help("The CAN interface to use, like 'can0', 'vcan0', etc")
            .required(true)
            .index(1))
        .subcommand(
            SubCommand::with_name("iface")
                .help_short("?")
                .about("Get/set parameters on the CAN interface")
                .subcommand(
                    SubCommand::with_name("up")
                        .about("Bring the interface up")
                )
                .subcommand(
                    SubCommand::with_name("down")
                        .about("Bring the interface down")
                )
                .subcommand(
                    SubCommand::with_name("bitrate")
                        .about("Set the bit rate on the interface.")
                )

        )
        .get_matches();

    let iface_name = opts.value_of("iface").unwrap();

    let res = if let Some(sub_opts) = opts.subcommand_matches("iface") {
        iface_cmd(&iface_name, &sub_opts)
    }
    else {
        Err(anyhow!("Need to specify a subcommand (-? for help)."))
    };

    if let Err(err) = res {
        eprintln!("{}", err);
        process::exit(1);
    }
}

