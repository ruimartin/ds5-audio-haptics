#!/usr/bin/env bash
# Build, copy the bundle to another machine over SSH and install it there.
#   scripts/deploy.sh deck@steamdeck
set -euo pipefail
cd "$(dirname "$0")/.."
host="${1:?usage: scripts/deploy.sh user@host}"
scripts/package.sh >/dev/null
ssh -o BatchMode=yes "$host" 'mkdir -p ~/.local/share && rm -rf ~/.local/share/ds5-audio-haptics'
scp -q -r target/bundle/ds5-audio-haptics "$host:.local/share/"
ssh -o BatchMode=yes "$host" '~/.local/share/ds5-audio-haptics/install.sh'
