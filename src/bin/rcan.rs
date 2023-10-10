// socketcan-rs/src/bin/can.rs

//! Simple CLI tool to run basic CAN bus functionality from the Linux
//! command line, similar to 'can-utils'.

use anyhow::{anyhow, Result};
use clap::{arg, value_parser, ArgAction, ArgMatches, Command};
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
    if let Some(_sub_opts) = opts.subcommand_matches("up") {
        let iface = CanInterface::open(iface_name)?;
        iface.bring_up()?;
    } else if let Some(_sub_opts) = opts.subcommand_matches("down") {
        let iface = CanInterface::open(iface_name)?;
        iface.bring_down()?;
    } else if let Some(sub_opts) = opts.subcommand_matches("bitrate") {
        let bitrate = *sub_opts.get_one::<u32>("bitrate").unwrap();
        let iface = CanInterface::open(iface_name)?;
        iface.set_bitrate(bitrate, None)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("add") {
        let idx = sub_opts.get_one::<u32>("num").copied();
        let typ = sub_opts.get_one::<String>("type").unwrap();
        println!("Add {} idx: {:?}, type: {}", iface_name, idx, typ);
        CanInterface::create(iface_name, idx, typ)?;
    } else if let Some(_sub_opts) = opts.subcommand_matches("delete") {
        let iface = CanInterface::open(iface_name)?;
        if let Err((_iface, err)) = iface.delete() {
            return Err(err.into());
        }
    } else if let Some(_sub_opts) = opts.subcommand_matches("details") {
        let iface = CanInterface::open(iface_name)?;
        let details = iface.details()?;
        println!("{:?}", details);
    } else {
        return Err(anyhow!("Unknown 'iface' subcommand"));
    };
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
    let opts = Command::new("can")
        .author("Frank Pagliughi")
        .version(VERSION)
        .about("Command line tool to interact with the CAN bus on Linux")
        .disable_help_flag(true)
        .arg(
            arg!(--help "Print help information")
                .short('?')
                .action(ArgAction::Help)
                .global(true),
        )
        .arg(
            arg!(<iface> "The CAN interface to use, like 'can0', 'vcan0', etc")
                .required(true)
                .index(1),
        )
        .subcommand(
            Command::new("iface")
                .about("Get/set parameters on the CAN interface")
                .subcommand(Command::new("up").about("Bring the interface up"))
                .subcommand(Command::new("down").about("Bring the interface down"))
                .subcommand(
                    Command::new("bitrate")
                        .about("Set the bit rate on the interface.")
                        .arg(
                            arg!(<bitrate> "The bit rate (in Hz)")
                                .required(true)
                                .value_parser(value_parser!(u32)),
                        ),
                )
                .subcommand(
                    Command::new("add")
                        .about("Create and add a new CAN interface")
                        .arg(
                            arg!(<num> "The interface number (i.e. 0 for 'vcan0')")
                                .required(false)
                                .value_parser(value_parser!(u32)),
                        )
                        .arg(
                            arg!(<type> "The interface type (i.e. vcan', etc)")
                                .required(false)
                                .default_value("vcan"),
                        ),
                )
                .subcommand(Command::new("delete").about("Delete the interface"))
                .subcommand(Command::new("details").about("Get details about the interface")),
        )
        .get_matches();

    let iface_name = opts.get_one::<String>("iface").unwrap();

    let res = if let Some(sub_opts) = opts.subcommand_matches("iface") {
        iface_cmd(iface_name, sub_opts)
    } else {
        Err(anyhow!("Need to specify a subcommand (-? for help)."))
    };

    if let Err(err) = res {
        eprintln!("{}", err);
        process::exit(1);
    }
}
