# Custom Spotify

A personal, native Rust Spotify client for Linux. Iced draws the UI without a
webview; librespot provides unofficial local playback. Spotify Premium and a
personal developer app are required. This project is not affiliated with Spotify.

## Build and run

On the development machine, Rust and the required ALSA, D-Bus, Wayland,
xkbcommon, and OpenSSL development libraries are already installed.

```sh
cargo run --release --locked
```

Use `--locked`: the lockfile pins `vergen` 9.0.6 because librespot 0.8's build
script is incompatible with the newer 9.1 dependency combination.

Linux is the primary target. On macOS (Apple Silicon), the same command builds with
Xcode's command-line tools and Rust alone: audio plays through CoreAudio and
credentials live in the login Keychain. MPRIS media keys and `tools/install.sh` are
Linux-only.

## Connect an account

1. Create a personal app at <https://developer.spotify.com/dashboard>.
2. Register this exact redirect URI: `http://127.0.0.1:43817/callback`.
3. Ensure your Premium account is authorized to use that development-mode app.
4. Enter the public Client ID in Custom Spotify's Settings. Do not enter a client secret.
5. Start authorization and complete Spotify's consent page in your browser.
6. In Settings → Account, select **Enable playback & Spotify Connect** and complete
   one extra, one-time Spotify consent. The device picker offers the same button.
   This step opens Spotify in your browser only when you request it.

The app requests private and collaborative playlists, your profile, saved library
(`user-library-read`, and `user-library-modify` plus `playlist-modify-public` to like
songs and save albums and playlists), streaming, and playback permissions
(`user-read-playback-state`, `user-modify-playback-state`,
`user-read-currently-playing`). If you authorized an earlier version, sign out in
Settings and sign in again to grant the new scopes. Spotify does not upgrade old
tokens automatically. Missing library or playback access reports a Permission error.

Custom Spotify registers as a Spotify Connect device and follows the active player
on your account. Connect uses librespot's unofficial protocol and can break when
Spotify changes it. Observation falls back to the official Web API when live
updates fail. Playback control requires Premium and Spotify may restrict developer
apps. Library editing remains outside this app.

Credentials belong in the desktop Secret Service, supplied by KDE Wallet on the
target machine. There is no plaintext credential fallback. Unlock or repair the
credential service if authorization cannot be saved.

Web API requests keep using your developer app's Client ID and PKCE tokens.
Playback and Connect use librespot's own KEYMASTER Client ID, as librespot's binary
does. Spotify's internal login5 rejects credentials minted for the developer app,
even when the access-point login succeeds. The extra consent uses
`http://127.0.0.1:5588/login`; you do not register that redirect in your own app.
Both token sets and librespot's reusable session credentials live in separate
Secret Service entries. Later starts reuse session credentials without a browser,
then try OAuth refresh if needed. Sign out clears both entries.

Until playback is enabled, browsing, Web API polling and remote playback controls
remain available. This computer stays visible in the device picker, but local
audio needs the separate librespot session. Failed or expired playback consent
can be retried in Settings. Startup never silently opens a second browser tab.

Librespot uses unofficial Spotify protocols. Using Spotify's client identity and
internal playback APIs is a Terms of Service grey area. Spotify may change or
reject this access without notice; this project does not promise live compatibility.

## Interface

The layout follows the Spotify desktop client: a navigation and playlist sidebar,
a main view with back/forward and search (a top result beside the first songs, then artists and albums), Home shelves of
your playlists and saved albums, Liked Songs, card grids for playlists, artists and albums, album and playlist pages with a track table, a now-playing bar, and an
optional queue panel. Settings and Quit live behind the gear in the top bar. Playback is read-only with respect to
your Spotify library. Manage playlists in Spotify itself.

Local TOML themes change appearance without executing code or moving navigation.
The default is the flat `themes/spotify-dark.toml`; `themes/dark-neumorphic.toml`
and `themes/material.toml` show how far the same tokens can stretch. Use an
absolute theme path in Settings when launching outside the repository.

Control icons come from the bundled [Lucide](https://lucide.dev) font
(`assets/fonts/lucide.ttf`, ISC license in `assets/fonts/LICENSE-lucide`). A
theme can set `icons = "text"` to show words instead.

Keyboard shortcuts (also listed in Settings):

| Keys | Action |
| --- | --- |
| Space | Play or pause (not while typing in a text field) |
| Ctrl+Left / Ctrl+Right | Previous or next track |
| Ctrl+Up / Ctrl+Down | Volume up or down by 5% |
| Ctrl+S | Shuffle on or off |
| Ctrl+R | Cycle repeat: off, queue, one track |
| / or Ctrl+L (Ctrl+F) | Focus search |
| Esc | Close the device picker, then Settings, then the queue panel; in the search field, leave it |
| Alt+Left / Alt+Right, mouse back/forward | Navigate back or forward |
| Ctrl+= / Ctrl+- / Ctrl+0 | Zoom in, zoom out, reset zoom |

The window draws its own title bar with minimize, maximize and close buttons; drag the
empty bar to move it and drag the edges to resize. Settings can switch to the system title bar.
Size, maximized state, zoom (Ctrl+= / Ctrl+- / Ctrl+0, 80-150%) and, where the platform allows,
position are remembered. To install the app icon and launcher entry, see `DESKTOP.md`.

Custom Spotify is a plain window app with no tray entry: closing the window quits
and stops playback. Media controls use this app's MPRIS service, not the
official Spotify application's service.

Discover Weekly, Smart Shuffle, playlist editing, lyrics, lossless playback,
and guaranteed offline listening are outside the initial release. Cached audio
is not an offline-download entitlement.

## Connect fallback and logging

This computer stays in the device picker when Spotify Connect registration fails
but the local decoder is available. Select it to pause remote playback and
continue locally at the current track and position. Playlist, album, and Liked
Songs queues are preserved when their source can be loaded; otherwise the current
track plays alone. Other Spotify clients cannot discover this fallback device.

Logging is silent by default. Capture Connect diagnostics with:

```sh
RUST_LOG=librespot=debug,custom_spotify=debug cargo run --release --locked 2> /tmp/custom-spotify.log
```

The Connect failure is logged at warn level. Review account and device metadata in
the log before sharing it. See `BACKEND.md` for fallback behavior and limitations.

## Validation

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Measure an already running release process without changing its playback:

```sh
python3 tools/measure.py <PID> --seconds 60
```

The sampler reports process-tree CPU as a percentage of one core, sampled RSS,
and PSS when permitted. RSS double-counts shared pages. It does not measure GPU
memory or guarantee capture of short-lived peaks.

Targets on the development machine are under 300 MB process-tree RAM during
playback and under 1% of one core averaged during hidden playback. These are
acceptance targets, not claims about unmeasured streaming sessions. Test visible,
hidden, paused, playing, and large-library browsing separately.

See `PLAN.md` for architecture and acceptance gates, and `BACKEND.md`,
`FRONTEND.md`, and `DESKTOP.md` for implementation-specific notes and limitations.
