#!/usr/bin/env bash
# Install or remove ds5-audio-haptics for the current user. Touches only
# ~/.local/bin, ~/.config/systemd/user and ~/.config/ds5-audio-haptics.
#   ./install.sh            install and start
#   ./install.sh uninstall  stop and remove (keeps the config file)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bin="$HOME/.local/bin"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
conf="${XDG_CONFIG_HOME:-$HOME/.config}/ds5-audio-haptics"

if [ "${1:-}" = uninstall ]; then
    systemctl --user disable --now ds5-audio-haptics.service 2>/dev/null || true
    rm -f "$units/ds5-audio-haptics.service" "$bin/ds5-audio-haptics" "$bin/ds5-tone"
    systemctl --user daemon-reload
    echo "removed (config kept in $conf)"
    exit 0
fi

mkdir -p "$bin" "$units" "$conf"
systemctl --user stop ds5-audio-haptics.service 2>/dev/null || true
install -m 755 "$here/ds5-audio-haptics" "$here/ds5-tone" "$bin/"
install -m 644 "$here/ds5-audio-haptics.service" "$units/"
[ -e "$conf/config.toml" ] || install -m 644 "$here/config.example.toml" "$conf/config.toml"
systemctl --user daemon-reload
systemctl --user enable --now ds5-audio-haptics.service
sleep 1
systemctl --user --no-pager --lines=5 status ds5-audio-haptics.service || true
