#!/bin/bash
#
# This is a local build/test CI to allow testing with the virtual
# CAN interface, vcan0. It should be installed in the kernel before
# running this script. See 'vcan.sh' (run it with root permissions)
#
# Run this from the top-level crate directory (i.e. the one with the 
# Cargo.toml file).
#

# Extract MSRV from Cargo.toml and ensure it's the full version triplet
# The value is stored in the variable `MSRV`
get_crate_msrv() {
    MSRV=$(awk '/rust-version/ { print substr($3, 2, length($3)-2) }' Cargo.toml)
    local N_DOT
    N_DOT=$(echo "${MSRV}" | grep -o "\." | wc -l | xargs)
    [[ ${N_DOT} == 1 ]] && MSRV="${MSRV}".0
}

printf "Cleaning the crate...\n"
! cargo clean && exit 1
printf "    Ok\n"

printf "\nFormat check...\n"
! cargo fmt --check --all && exit 1
printf "    Ok\n"

# Spellcheck if the 'typos' cli tool installed
# If not, you can install with cargo:
#     $ cargo install typos-cli
#
if typos -V &> /dev/null ; then
    printf "\nCheck for typos...\n"
    ! typos && exit 1
    printf "    Ok\n"
fi

get_crate_msrv
printf "\nUsing MSRV %s\n" "${MSRV}"

# Get the MSRV from Cargo.toml
MSRV=$(awk '/rust-version/ { print substr($3, 2, length($3)-2) }' Cargo.toml)
N_DOT=$(echo "${MSRV}" | grep -o "\." | wc -l | xargs)
[[ ${N_DOT} == 1 ]] && MSRV="${MSRV}".0

for VER in stable ${MSRV} ; do
    printf "\n\nBuilding with default features for %s...\n" "${VER}"

    cargo clean && \
        cargo +"${VER}" check --all-targets && \
        cargo +"${VER}" doc --all-features --no-deps && \
        cargo +"${VER}" test
    [ "$?" -ne 0 ] && exit 1
done

printf "\nChecking clippy for version: %s...\n" "${MSRV}"
cargo clean
! cargo +"${MSRV}" clippy --no-deps --all-targets -- -D warnings && exit 1

for VER in stable ${MSRV} ; do
    printf "\n\nBuilding with no features for %s...\n" "${VER}"
    cargo clean && \
        cargo +"${VER}" check --no-default-features && \
        cargo +"${VER}" test --no-default-features
    [ "$?" -ne 0 ] && exit 1

    FEATURES="vcan_tests"
    printf "\n\nBuilding with features [%s] for %s...\n" "${FEATURES}" "${VER}"
    cargo clean && \
        cargo +"${VER}" check --features="$FEATURES" --all-targets  && \
        cargo +"${VER}" test --features="$FEATURES"
    [ "$?" -ne 0 ] && exit 1

    for FEATURE in "tokio" "smol" "enumerate"; do
        printf "\n\nBuilding with feature [%s]...\n" "${FEATURE}" "${VER}"
        FEATURES="${FEATURE},utils,vcan_tests"
        cargo clean && \
            cargo +"${VER}" check --no-default-features --features="${FEATURES}" --all-targets && \
            cargo +"${VER}" test --no-default-features --features="${FEATURES}"
        [ "$?" -ne 0 ] && exit 1
    done
done

cargo clean
printf "\n\n*** All builds succeeded ***\n"

