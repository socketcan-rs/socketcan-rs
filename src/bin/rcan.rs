// socketcan-rs/src/bin/can.rs

//! Simple CLI tool to run basic CAN bus functionality from the Linux
//! command line, similar to 'can-utils'.

use anyhow::{anyhow, Result};
use clap::{arg, value_parser, ArgAction, ArgMatches, Command};
use socketcan::{CanInterface, CanCtrlMode};
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
    } else if let Some(sub_opts) = opts.subcommand_matches("loopback") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::Loopback, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("loopback") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::Loopback, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("loopback") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::Loopback, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("listen-only") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::ListenOnly, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("triple-sampling") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::TripleSampling, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("one-shot") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::OneShot, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("berr-reporting") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::BerrReporting, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("fd") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::Fd, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("fd-non-iso") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::NonIso, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("presume-ack") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::PresumeAck, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("cc-len8-dlc") {
        let on = sub_opts.get_one::<String>("on").unwrap() == "on";
        let iface = CanInterface::open(iface_name)?;
        iface.set_ctrlmode(CanCtrlMode::CcLen8Dlc, on)?;
    } else if let Some(sub_opts) = opts.subcommand_matches("restart-ms") {
        let ms = *sub_opts.get_one::<u32>("ms").unwrap();
        let iface = CanInterface::open(iface_name)?;
        iface.set_restart_ms(ms)?;
    } else if let Some(_sub_opts) = opts.subcommand_matches("restart") {
        let iface = CanInterface::open(iface_name)?;
        iface.restart()?;
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
                        .about("Set the bit rate on the interface")
                        .arg(
                            arg!(<bitrate> "The bit rate (in Hz)")
                                .required(true)
                                .value_parser(value_parser!(u32)),
                        ),
                )
                .subcommand(
                    Command::new("loopback")
                        .about("Put the interface into loopback mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("listen-only")
                        .about("Put the interface into listen-only mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("triple-sampling")
                        .about("Put the interface into triple sampling mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("one-shot")
                        .about("Put the interface into one-shot mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("berr-reporting")
                        .about("Put the interface into BERR reporting mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("fd")
                        .about("Put the interface into FD mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("fd-non-iso")
                        .about("Put the interface into non-ISO FD mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("presume-ack")
                        .about("Put the interface into presume ACK mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("cc-len8-dlc")
                        .about("Put the interface into classic CAN DLC mode")
                        .arg(
                            arg!(<on> "Enable/disable mode")
                                .required(true)
                                .value_parser(["on", "off"])
                        ),
                )
                .subcommand(
                    Command::new("restart-ms")
                        .about("Set the automatic restart delay time (in ms)")
                        .arg(
                            arg!(<ms> "The automatic restart delay time (in ms)")
                                .required(true)
                                .value_parser(value_parser!(u32)),
                        ),
                )
                .subcommand(Command::new("restart").about("Restart the interface"))
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
