# Librespot Playback 0.8.0 output buffer patch

Source: the crates.io librespot-playback 0.8.0 release, MIT license included.
The root Cargo.toml patches that exact version. Package versions and Cargo.lock
entries are otherwise unchanged.

The ALSA sink asked for up to half a second of output buffer. That buffer is the
delay of every transport change: the player drains it before reporting a pause, and
seeks and track skips keep playing what it already holds. This copy lowers the
largest requested buffer to 150 ms in `src/audio_backend/alsa.rs`. The smallest
accepted buffer (100 ms) and the period sizing are unchanged.
