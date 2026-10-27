# Validation

## Desktop parity implementation, 2026-10-27

Automated checks on macOS Apple Silicon with Rust 1.98.0:

- `cargo fmt --check` and `git diff --check`.
- `cargo clippy --locked --all-targets -- -D warnings`.
- `cargo test --locked`: 165 Scytify unit tests, 3 integration tests and 4 vendored OAuth tests, 172 total, zero failures.
- `cargo build --release --locked` completed successfully. The installed Rust toolchain emits a nonfatal strip warning: `rust-objcopy` cannot find `libLLVM.dylib`. No profile or environment override was used; the release executable is produced.

New regressions cover playlist search and non-owned playlists, followed-artist cursor paging past skipped rows, popular-song permission failures, stable duplicate sorting, keyboard playback at raw offsets, explicit shelf retry, macOS Command mapping and the current-device fallback. The offline frontend example compiles under the all-targets check. Existing dock-icon formatting and range-loop lint failures were fixed.

These are automated results. The app was not installed, launched or used against Spotify during this implementation. Parent acceptance still needs narrow-window layout, row/action-sheet clicks, clipboard behavior, Command shortcuts, collaborator navigation, filtered 797-song browsing, endpoint availability on the real developer client, device display and VoiceOver. No OAuth scope, live library or device state was changed. The earlier platform measurements below are historical evidence.

## Earlier build and automated checks

Validated with Rust 1.98.0 on Fedora 44 KDE Plasma, Wayland, Ryzen 7 3700X,
32 GB RAM, RTX 5060, and a 1920×1080 display running at 240 Hz.

Passed:

- `cargo fmt --check`
- `cargo check --locked --all-targets`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked`: 34 tests, zero failures.
- `cargo build --release --locked --examples --bins`

Regressions include shuffle occurrence order, explicit queue priority, paused
startup, invalid checkpoints, stale decoder events, seek acknowledgement,
saturated shutdown delivery, artwork quotas and decoding limits, large metadata
working-set bounds, stale UI requests, account-safe route restoration, credential
runtime nesting, and exit after backend failure.

## Native smoke tests

The release application started with isolated temporary XDG config/data folders.
No developer client ID, Spotify token, or real playback was supplied.

Verified on the current desktop:

- The native dark neumorphic preview renders playlist/sidebar/transport content.
  Preview content is explicitly synthetic and has no audio backend.
- (Historical) The app registered a StatusNotifierItem; the tray was removed on 2026-10-21 and closing the window now quits.
- Its MPRIS service exposes stopped playback and empty metadata before login.
- MPRIS Raise succeeds.
- MPRIS setting Shuffle to true is reflected by the authoritative backend state.
- MPRIS Quit shuts down the release process successfully with exit code zero.
- No official Spotify process was controlled or stopped.

Startup testing found and fixed a real nested-Tokio panic in synchronous Secret
Service access. The keyring calls now use a blocking context. A locked credential
service can still delay its serialized operation; no plaintext fallback is used.

Close-quits has an automated test. Compositor restarts, full keyboard traversal, and screen-reader behavior still
need interactive acceptance testing.

## Resource sample

`tools/measure.py` sampled the real, signed-out release app for 10.20 seconds,
idle in the background with no audio playing:

| Measurement | Result |
| --- | --- |
| Peak sampled process-tree RSS | 31.35 MB |
| Peak sampled process-tree PSS | 24.70 MB |
| Average CPU, one-core scale | 0.000% |

This is an idle sample, not a streaming benchmark. One-second sampling can miss
peaks and short-lived children. RSS counts shared pages more than once. GPU/VRAM
usage was not measured. The application shares one two-worker Tokio runtime with
Iced; the synchronous credential library also creates runtime threads.

The under-300-MB playback and under-1%-of-one-core hidden-playback targets remain
unverified until account setup and real streaming tests. The synthetic fixture
also does not establish performance for a 50,000-track Spotify playlist.

## Remaining account-dependent checks

- PKCE consent, refresh-token persistence, restart, and logout with an eligible app.
- Whether that developer client/token can establish a librespot streaming session.
- Current own-playlist, artist-search, release, and track endpoint access.
- Real track playback, output-device recovery, normalization, and gapless boundaries.
- Large playlist loading, unavailable tracks, long sessions, and network recovery.
- Ten-minute hidden playback CPU/RAM measurements and visible scrolling tests.

The bounded disk cache stores artwork and encrypted librespot audio. It does not provide user-managed downloads or a guaranteed offline library. Made for you can include Discover Weekly through the playback session; Smart Shuffle remains deferred.
