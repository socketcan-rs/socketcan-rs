#!/bin/bash
#
# This is a local build/test CI to allow testing with the virtual
# CAN interface, vcan0. It should be installed in the kernel before
# running this script. See 'vcan.sh' (run it with root permissions)
#
# Run this from the top-level crate directory (i.e. the one with the 
# Cargo.toml file).
#

#printf "Updating the crate...\n"
#cargo clean && cargo update
#[ "$?" -ne 0 ] && exit 1

printf "Format check...\n"
cargo fmt --all --check
[ "$?" -ne 0 ] && exit 1

# Get the MSRV from Cargo.toml
MSRV=$(awk '/rust-version/ { print substr($3, 2, length($3)-2) }' Cargo.toml)
N_DOT=$(echo "${MSRV}" | grep -o "\." | wc -l | xargs)
[[ ${N_DOT} == 1 ]] && MSRV="${MSRV}".0

for VER in stable ${MSRV} ; do
    printf "\n\nBuilding with default features for %s...\n" "${VER}"

    cargo clean && \
        cargo +"${VER}" check && \
        cargo +"${VER}" doc --all-features && \
        cargo +"${VER}" test && \
        cargo +"${VER}" clippy
    [ "$?" -ne 0 ] && exit 1

    printf "\n\nBuilding with no features for %s...\n" "${VER}"
    cargo clean && \
        cargo +"${VER}" check --no-default-features && \
        cargo +"${VER}" test --no-default-features && \
        cargo +"${VER}" clippy --no-default-features
    [ "$?" -ne 0 ] && exit 1

    FEATURES="vcan_tests"
    printf "\n\nBuilding with features [%s] for %s...\n" "${FEATURES}" "${VER}"
    cargo clean && \
        cargo +"${VER}" check --features="$FEATURES" && \
        cargo +"${VER}" test --features="$FEATURES" && \
        cargo +"${VER}" clippy --features="$FEATURES"
    [ "$?" -ne 0 ] && exit 1

    for FEATURE in "tokio" "async-std" "smol" "utils" "enumerate" "utils"; do
        printf "\n\nBuilding with feature [%s]...\n" "${FEATURE}" "${VER}"
        FEATURES="${FEATURE} vcan_tests"
        cargo clean && \
        cargo +"${VER}" check --no-default-features --features="${FEATURES}" && \
        cargo +"${VER}" test --no-default-features --features="${FEATURES}" && \
        cargo +"${VER}" clippy --no-default-features --features="${FEATURES}"
        [ "$?" -ne 0 ] && exit 1
    done
done

cargo clean
printf "\n\n*** All builds succeeded ***\n"

