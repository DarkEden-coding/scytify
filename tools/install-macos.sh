#!/usr/bin/env bash
# Build the release binary and install it as "~/Applications/Scytify.app" for the current
# user, so Spotlight, Launchpad, Raycast and Alfred find it. A running instance is asked to quit,
# then the app is opened from the new bundle.
set -euo pipefail
cd "$(dirname "$0")/.."

name=scytify
display_name="Scytify"
bundle_id=com.scytify.Scytify
app="$HOME/Applications/$display_name.app"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)

# Started by the app's own update: it quits itself once this script returns, so open the new
# build after its process (SCYTIFY_UPDATE_PID) has gone.
relaunch_after_exit() {
    local pid=$1; shift
    nohup sh -c 'while kill -0 "$0" 2>/dev/null; do sleep 0.2; done; exec "$@"' "$pid" "$@" \
        >/dev/null 2>&1 </dev/null &
}

# rustc strips through rust-objcopy, which some rustup macOS toolchains ship unable to load
# libLLVM.dylib ("stripping debug info ... failed"); Apple's strip does the same job below.
CARGO_PROFILE_RELEASE_STRIP=none cargo build --release --locked

was_running=false
# An update started from the app leaves quitting and relaunching to the app.
if [ -z "${SCYTIFY_UPDATE_PID:-}" ] && pgrep -x "$name" >/dev/null; then
    was_running=true
    echo "Quitting the running app..."
    osascript -e "tell application id \"$bundle_id\" to quit" >/dev/null 2>&1 || true
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

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The bundle icon: the grey "closed" bars. iconutil wants each size at 1x and 2x; the 1024 px
# slot (512@2x) is optional.
iconset="$work/$name.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
    cp "assets/icons/$name-closed-$size.png" "$iconset/icon_${size}x${size}.png"
done
for size in 16 32 128 256; do
    cp "assets/icons/$name-closed-$((size * 2)).png" "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o "$work/$name.icns"

# Build the bundle beside the target and swap it in, so a failed step leaves the old app intact.
staged="$work/$display_name.app"
mkdir -p "$staged/Contents/MacOS" "$staged/Contents/Resources"
cp "target/release/$name" "$staged/Contents/MacOS/$name"
strip -S "$staged/Contents/MacOS/$name"
cp "$work/$name.icns" "$staged/Contents/Resources/$name.icns"
cat >"$staged/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$display_name</string>
    <key>CFBundleDisplayName</key><string>$display_name</string>
    <key>CFBundleIdentifier</key><string>$bundle_id</string>
    <key>CFBundleExecutable</key><string>$name</string>
    <key>CFBundleIconFile</key><string>$name</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
# Sign the whole bundle so its signature covers Info.plist and resources. Keychain "Always
# Allow" trusts an ad-hoc signature only for that exact build, so every reinstall would ask
# again for each stored token; a certificate identity keeps that trust across rebuilds.
# CODESIGN_IDENTITY overrides the first Apple Development identity found.
identity=${CODESIGN_IDENTITY:-$(security find-identity -v -p codesigning |
    sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "Apple Development: .*"$/\1/p' | head -n1)}
if [ -z "$identity" ]; then
    echo "No Apple Development signing identity; ad-hoc signing. Keychain will ask again after each install."
    identity=-
fi
codesign --force --sign "$identity" "$staged"

mkdir -p "$HOME/Applications"
rm -rf "$app"
mv "$staged" "$app"

# Register with Launch Services now instead of waiting for the next scan.
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
"$lsregister" -f "$app" 2>/dev/null || true
mdimport "$app" 2>/dev/null || true

echo "Installed $app"
if [ -n "${SCYTIFY_UPDATE_PID:-}" ]; then
    relaunch_after_exit "$SCYTIFY_UPDATE_PID" open "$app"
    echo "Relaunching after the app quits."
    exit 0
fi
open "$app"
if $was_running; then
    echo "Relaunched."
else
    echo "Opened."
fi
