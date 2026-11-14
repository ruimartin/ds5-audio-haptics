#!/usr/bin/env bash
# Build static (musl) binaries. Uses cargo from PATH when it has the musl target,
# otherwise the toolchain inside a distrobox (see README, "Building").
#   scripts/build.sh          release build
#   scripts/build.sh test     run the unit tests
set -euo pipefail
cd "$(dirname "$0")/.."
target=x86_64-unknown-linux-musl
box="${DS5_DISTROBOX:-dsdeck-dev}"
tool="${DS5_TOOLCHAIN:-$HOME/.local/share/dsdeck-dev}"
# Keep the builder's directory names (and so their user name) out of the binaries:
# file paths compiled into panic messages are rewritten to neutral ones.
remap="--remap-path-prefix=$PWD=. --remap-path-prefix=$HOME=/home/builder"
remap="$remap --remap-path-prefix=$(readlink -f "$HOME")=/home/builder"
export RUSTFLAGS="${RUSTFLAGS:-} $remap"
if command -v cargo >/dev/null && rustc --print target-libdir --target "$target" 2>/dev/null | xargs test -d; then
    exec cargo "${@:-build}" --release --target "$target"
fi
exec distrobox enter "$box" -- env CARGO_HOME="$tool/cargo" RUSTUP_HOME="$tool/rustup" \
    RUSTFLAGS="$RUSTFLAGS" \
    "$tool/cargo/bin/cargo" "${@:-build}" --release --target "$target"
