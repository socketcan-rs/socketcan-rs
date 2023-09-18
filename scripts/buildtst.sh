#!/bin/bash
#
# This is a local build/test CI to allow testing with the virtual
# CAN interface, vcan0. It should be installed in the kernel before
# running this script. See 'vcan.sh' (run it with root permissions)
#

printf "Updating the crate...\n"
cargo clean && cargo update
[ "$?" -ne 0 ] && exit 1

printf "Format check...\n"
cargo fmt --all --check
[ "$?" -ne 0 ] && exit 1

printf "\n\nBuilding with default features...\n"
cargo clean && cargo build && cargo doc && cargo test && cargo clippy
[ "$?" -ne 0 ] && exit 1

printf "\n\nBuilding with no features...\n"
cargo clean && \
    cargo build --no-default-features && \
    cargo doc --no-default-features && \
    cargo test --no-default-features && \
    cargo clippy --no-default-features
[ "$?" -ne 0 ] && exit 1

FEATURES="vcan_tests"
printf "\n\nBuilding with features [${FEATURES}]...\n"
cargo clean && \
    cargo build --features="$FEATURES" && \
    cargo doc --features="$FEATURES" && \
    cargo test --features="$FEATURES" && \
    cargo clippy --features="$FEATURES"
[ "$?" -ne 0 ] && exit 1

for FEATURE in "tokio" "async-std" "smol"; do
    printf "\n\nBuilding with feature [${FEATURE}]...\n"
	FEATURES="${FEATURE} vcan_tests"
    cargo clean && \
	cargo build --no-default-features --features="${FEATURES}" && \
	cargo doc --no-default-features --features="${FEATURES}" && \
	cargo test --no-default-features --features="${FEATURES}" && \
	cargo clippy --no-default-features --features="${FEATURES}"
    [ "$?" -ne 0 ] && exit 1
done

cargo clean
printf "\n\n*** All builds succeeded ***\n"
