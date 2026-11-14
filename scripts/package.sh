#!/usr/bin/env bash
# Build and assemble the install bundle: target/bundle/ds5-audio-haptics/ and a .tar.gz.
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/build.sh
out=target/x86_64-unknown-linux-musl/release
dir=target/bundle/ds5-audio-haptics
rm -rf target/bundle && mkdir -p "$dir"
cp "$out/ds5-audio-haptics" "$out/ds5-tone" dist/install.sh dist/ds5-audio-haptics.service \
    config.example.toml LICENSE README.md "$dir/"
tar -C target/bundle -czf target/bundle/ds5-audio-haptics-x86_64-linux.tar.gz ds5-audio-haptics
echo "target/bundle/ds5-audio-haptics-x86_64-linux.tar.gz"
