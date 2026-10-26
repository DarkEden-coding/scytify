# Desktop integration

## macOS

The macOS adapter in `src/desktop/macos.rs` registers play, pause, toggle, stop,
next, and previous commands with `MPRemoteCommandCenter`. Commands go through the
same bounded backend channel as the UI. Track metadata, duration, elapsed time,
and playback state are published through `MPNowPlayingInfoCenter` on the main
queue. Stopped playback and playback on another Spotify Connect device clear
the Mac's Now Playing entry. The adapter removes its command targets when stopped.

The Rodio backend opens CoreAudio output when playback starts and releases it
when paused or stopped. Resuming resolves the current default output again.
This avoids keeping audio IO running silently between playback sessions and
lets macOS manage AirPods routing. Automatic switching still depends on the
AirPods settings and macOS; the app does not force a Bluetooth device selection.

After rebuilding, reinstall the app with `tools/install.sh` to update the bundle
used by launchers. Check media keys with the app in the background, AirPods
play/pause and skip controls, switching from another Apple device when starting
playback, and changing the default output while paused and while playing.

## Linux

## Dependencies for the integrator

Add these exact dependencies to the shared manifest (not changed by this worker):

```toml
mpris-server = { version = "0.10.0", default-features = false, features = ["tokio"] }
```

`src/desktop.rs` exports the requested async `run` function. It registers the app-owned MPRIS service `org.mpris.MediaPlayer2.com.scytify.Scytify` (the builder argument is `com.scytify.Scytify`). It never discovers or controls an official Spotify process.

MPRIS calls return a visible D-Bus error on command-channel backpressure. Desktop request IDs are allocated atomically in the high half of `u64`. Registration errors are returned as `AppError::Unavailable`. Dropping the application `watch::Sender` ends the adapter and unregisters the MPRIS object.

MPRIS position is extrapolated from the backend's monotonic anchor only while playing and clamped to duration. State transitions publish only changed properties. Decoder-confirmed seek revisions emit `Seeked`; ordinary position anchors do not wake desktop clients with false seek signals.

## Validation

Root checks and tests pass. The release app registered its MPRIS service on the active user bus. MPRIS Raise, setting/getting Shuffle, and graceful Quit were smoke-tested without login or audio. See `VALIDATION.md`; playback-specific desktop behavior still requires a real account.

## Application icon and window identity

`assets/icons/scytify.svg` is the hand-written source (dark grey neomorphic tile, recessed
grey disc holding five glowing green waveform bars). `assets/icons/scytify-{16,32,48,64,128,256,512}.png`
are renders of it (`magick -background none -density <4 x s * 96 / 512> scytify.svg -resize <s>x<s>`); the 16px one comes from `scytify-16.svg`, a simplified three-bar, pixel-aligned version so it stays crisp.
The in-app (window) icon is the earlier green design instead: `assets/icons/scytify-app.svg` and `scytify-app-{16,32,48,64,256}.png` (16px from `scytify-app-16.svg`), embedded in `src/app_icon.rs` for the Iced window icon. The grey icon above is the dock and launcher icon.
The icon reflects app state. Not running: grey bars (`scytify-closed-{16,32,64,128,256,512}.png`, rendered by `examples/render_dock_icon.rs` and used for the macOS bundle icon). Running: `src/dock_icon.rs` redraws the neomorphic icon in software, with the middle three bars small while not playing and all five animating while playing. `src/desktop/icon.rs` picks the frames and drives any `IconSink`; `src/desktop/macos_dock.rs` sets the Dock icon. Another platform adds an `IconSink` and an arm in `desktop::run_icon`; Linux has none yet, so its launcher icon stays static.
The app has no tray entry: it is a plain window app, and closing the window quits.

The window sets Wayland `app_id` / X11 class `scytify`, matching `packaging/scytify.desktop`
(`Icon=scytify`, `StartupWMClass=scytify`), so KDE matches the window to the launcher.
Build and install for the current user, or rebuild and reinstall after changes:

```sh
tools/install.sh
```

It builds the release binary, installs it as `~/.local/bin/scytify`, installs the icons under
`~/.local/share/icons/hicolor` and the launcher entry (its `Exec` naming that binary) under
`~/.local/share/applications`, and refreshes the icon and KDE launcher caches. A running instance is
quit through MPRIS, so it saves its playback checkpoint (terminated only if it is still running after
ten seconds), then relaunched from the new binary.

## Title bar, window position and zoom

The window draws its own title bar (see `FRONTEND.md`); Settings can switch to the system one.
Window size, maximized state and zoom are remembered. On native Wayland a client cannot read or set
its position, so Scytify does not try. To remember position under KDE, add a KWin window rule
(System Settings > Window Management > Window Rules) matching window class `scytify` with
"Position" set to "Remember" (and optionally "Size", "Maximized" and "Keep above/below"). On X11 or
XWayland the app restores its saved position itself.
