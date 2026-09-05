#!/bin/bash

# Brings up a CAN 2.0 interface with the specified bitrate.
# Must have root privileges to run this script

if (( $EUID != 0 )); then
  echo "This script must be run as root"
  exit 1
fi

IFACE=can0
[ -n "$1" ] && IFACE=$1

BITRATE=250000
[ -n "$2" ] && BITRATE=$2

# The bitrate can only be set while the interface is down, and setting a
# down interface down again is a harmless no-op.
#
# Configure the interface with requested bitrate
#   and then bring the interface up
ip link set ${IFACE} down && \
    ip link set ${IFACE} type can bitrate ${BITRATE} && \
    ip link set ${IFACE} up
