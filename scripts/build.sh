#!/usr/bin/env bash
# Build static (musl) binaries. Uses cargo from PATH when it has the musl target,
# otherwise the toolchain inside a distrobox (see README, "Building").
#   scripts/build.sh          release build
#   scripts/build.sh test     run the unit tests
set -euo pipefail
cd "$(dirname "$0")/.."
target=x86_64-unknown-linux-musl
if command -v cargo >/dev/null && rustc --print target-libdir --target "$target" 2>/dev/null | xargs test -d; then
    exec cargo "${@:-build}" --release --target "$target"
fi
box="${DS5_DISTROBOX:-dsdeck-dev}"
tool="${DS5_TOOLCHAIN:-$HOME/.local/share/dsdeck-dev}"
exec distrobox enter "$box" -- env CARGO_HOME="$tool/cargo" RUSTUP_HOME="$tool/rustup" \
    "$tool/cargo/bin/cargo" "${@:-build}" --release --target "$target"
