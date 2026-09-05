#!/bin/bash

# Brings up a CAN FD interface with the specified bitrate and data bitrate.
# Must have root privileges to run this script

if (( $EUID != 0 )); then
  echo "This script must be run as root"
  exit 1
fi

IFACE=can0
[ -n "$1" ] && IFACE=$1

BITRATE=250000
[ -n "$2" ] && BITRATE=$2

DBITRATE=1000000
[ -n "$3" ] && DBITRATE=$3

# The bitrates and control modes can only be set while the interface is down,
# and setting a down interface down again is a harmless no-op.
#
# Configure the interface with requested bitrate, data bitrate, and enable FD
#   and then bring the interface up
ip link set ${IFACE} down && \
    ip link set ${IFACE} type can bitrate ${BITRATE} dbitrate ${DBITRATE} fd on && \
    ip link set ${IFACE} up
