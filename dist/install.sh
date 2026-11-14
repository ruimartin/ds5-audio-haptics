#!/usr/bin/env bash
# Install or remove ds5-audio-haptics for the current user. Touches only
# ~/.local/bin, ~/.config/systemd/user, ~/.config/ds5-audio-haptics and one launcher
# in ~/.local/share/applications.
#   ./install.sh            install and start
#   ./install.sh uninstall  stop and remove (keeps the config file)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bin="$HOME/.local/bin"
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
conf="${XDG_CONFIG_HOME:-$HOME/.config}/ds5-audio-haptics"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

if [ "${1:-}" = uninstall ]; then
    systemctl --user disable --now ds5-audio-haptics.service 2>/dev/null || true
    rm -f "$units/ds5-audio-haptics.service" "$bin/ds5-audio-haptics" "$bin/ds5-tone" "$bin/ds5-trigger" \
        "$apps/ds5-audio-haptics.desktop"
    systemctl --user daemon-reload
    echo "removed (config kept in $conf)"
    exit 0
fi

mkdir -p "$bin" "$units" "$conf" "$apps"
systemctl --user stop ds5-audio-haptics.service 2>/dev/null || true
install -m 755 "$here/ds5-audio-haptics" "$here/ds5-tone" "$here/ds5-trigger" "$bin/"
install -m 644 "$here/ds5-audio-haptics.service" "$units/"
[ -e "$conf/config.toml" ] || install -m 644 "$here/config.example.toml" "$conf/config.toml"
# Launcher for the settings page (application menu, "DualSense Audio Haptics").
cat > "$apps/ds5-audio-haptics.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=DualSense Audio Haptics
Comment=Tune the haptics driven by system audio
Exec="$bin/ds5-audio-haptics" ui
Icon=input-gaming
Terminal=false
Categories=Settings;HardwareSettings;
DESKTOP
systemctl --user daemon-reload
systemctl --user enable --now ds5-audio-haptics.service
sleep 1
systemctl --user --no-pager --lines=5 status ds5-audio-haptics.service || true
