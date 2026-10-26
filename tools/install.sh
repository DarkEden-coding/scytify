#!/usr/bin/env bash
# Build the release binary and install it, its icons and its launcher entry for the current
# user. A running instance is quit through MPRIS, so it saves its playback checkpoint, and is
# relaunched from the new binary.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" == Darwin ]]; then
    exec tools/install-macos.sh
fi

name=custom-spotify
bin="$HOME/.local/bin/$name"
data="${XDG_DATA_HOME:-$HOME/.local/share}"

cargo build --release

was_running=false
if pgrep -x "$name" >/dev/null; then
    was_running=true
    echo "Quitting the running app..."
    dbus-send --session --type=method_call \
        --dest=org.mpris.MediaPlayer2.com.customspotify.CustomSpotify \
        /org/mpris/MediaPlayer2 org.mpris.MediaPlayer2.Quit 2>/dev/null || true
    for _ in $(seq 50); do
        pgrep -x "$name" >/dev/null || break
        sleep 0.2
    done
    if pgrep -x "$name" >/dev/null; then
        echo "Still running after 10 s; terminating it."
        pkill -x "$name" || true
        sleep 1
    fi
fi

install -Dm755 "target/release/$name" "$bin"
for size in 16 32 48 64 128 256 512; do
    install -Dm644 "assets/icons/$name-$size.png" \
        "$data/icons/hicolor/${size}x${size}/apps/$name.png"
done
install -Dm644 "assets/icons/$name.svg" "$data/icons/hicolor/scalable/apps/$name.svg"
# Launchers do not always inherit ~/.local/bin on PATH, so the entry names the binary exactly.
mkdir -p "$data/applications"
sed "s|^Exec=.*|Exec=$bin|" "packaging/$name.desktop" >"$data/applications/$name.desktop"
chmod 644 "$data/applications/$name.desktop"
gtk-update-icon-cache -q "$data/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$data/applications" 2>/dev/null || true
kbuildsycoca6 >/dev/null 2>&1 || true

echo "Installed $bin"
if $was_running; then
    setsid -f "$bin" >/dev/null 2>&1 </dev/null
    echo "Relaunched."
fi
