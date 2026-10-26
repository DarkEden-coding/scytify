# Scytify — implementation handoff

## Status and ownership

An initial implementation now builds and passes automated checks. Astra handled
architecture/frontend work and SOL implemented the service/backend modules.
`VALIDATION.md` records native smoke tests and idle resource measurements.

The sections below retain the original design and handoff requirements, not a
claim that every acceptance gate passed. Live Spotify streaming, streaming resource
budgets, full metadata/audio disk caching, and later macOS/discovery work remain
open. The implemented cache stores artwork; themes use safe system fonts and
bundled icon choices rather than arbitrary local font/SVG assets.
Architecture/frontend requested: **Astra, low reasoning**; backend: **SOL**.
The dispatcher must select those models; this repository cannot select a model.
Ponytail skill read: use a single process, concrete modules, no service framework.

**Concurrent work, no shared-file edits:**

| Owner | Exclusive files | Deliverable |
|---|---|---|
| SOL backend | `src/backend/**`, `tests/backend_*.rs`, `BACKEND.md` | Auth, API, player/queue, persistence/cache, Linux desktop adapters |
| Astra UI (low) | `src/frontend/**`, `themes/**`, `tests/frontend_*.rs`, `FRONTEND.md` | Iced daemon, theme parser, fixed layout, virtualized pages, channel bridge |
| Main/integrator | `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `src/lib.rs`, `src/domain.rs`, `src/contracts.rs`, `PLAN.md`, packaging | Dependency merge, module declarations, startup/shutdown, contract amendments |

Implementers must **not** change shared files or generate a shared lockfile while
working concurrently. Report dependency additions and contract problems to the
integrator. Integrator first adds agreed dependencies and module declarations
once each `mod.rs` exists, then runs checks; alternatively each worker checks an
isolated temporary harness outside this repo pointing at its files. Never claim
an uncompiled worker implementation is validated. No commits requested.

## Decisions and inspected environment

Actual toolchain: rustc/cargo 1.98.0; manifest MSRV 1.88 (only current toolchain
checked). pkg-config finds ALSA 1.2.16.1, Wayland 1.25.0, xkbcommon 1.13.1,
D-Bus 1.16.2. Integrator inspected Fedora 44 KDE Wayland, Ryzen 7 3700X,
32 GB installed RAM, RTX 5060, and 1920x1080 at 240 Hz. PipeWire and
WirePlumber are active; KDE Secret Service and StatusNotifierWatcher are available.
Hardware is identified but playback performance has not been benchmarked.

Choose **Iced 0.14.0** native, `default-features = false`, features
`["tiny-skia", "wayland", "x11", "tokio", "image", "svg"]` initially.
Use `iced::daemon`, not `application`: documented to survive zero windows and
open windows explicitly. Close destroys the window/surface while state and
subscriptions survive; tray Show recreates it. This avoids hidden rendering.
Software tiny-skia avoids a heavyweight GPU baseline and NVIDIA-specific Vulkan
issues; benchmark before choosing it permanently. Wgpu remains a measured
alternative, not a second enabled renderer by default. This is an event-driven
Elm UI with retained widget state, not a claim that Iced is a fully retained
scene graph. Eg(u)i/eframe 0.36.1 also supports event-driven repaint and efficient
virtual rows, but its immediate-mode model and hidden-window lifecycle need more
care; neither toolkit automatically meets memory/CPU targets. Iced daemon makes
this app's lifecycle more explicit. No webview, CEF, browser subprocess, or
continuous animation loop.

Crates inspected via `cargo search/info`: librespot 0.8.0, tokio 1.53.1,
keyring 4.2.0, ksni 0.3.6, mpris-server 0.10.0. Current scaffold actually locks
only serde 1.0.229 and tokio 1.53.1 plus transitive dependencies. Integrator should
add Iced and librespot versions above, **librespot with defaults off**,
`alsa-backend` + `rustls-tls-native-roots` on Linux. PipeWire via default ALSA
routing, not exclusive hardware access. macOS later uses rodio/CoreAudio instead.
Use keyring 4.2.0 with explicit Linux Secret Service store, and macOS Keychain
later; inspect v4 API rather than copying v3 examples. No kernel-keyring or
file-store fallback. ksni for KDE StatusNotifierItem; mpris-server for MPRIS.
Backend may add reqwest (rustls, JSON), serde_json, rusqlite, directories,
rand, OAuth/PKCE and SHA-256 helpers as actually needed; pin verified versions in
its dependency brief. Prefer small official HTTP endpoint wrappers to an API SDK
with stale development-mode endpoints. Theme parser may add `toml`.

## Exact boundary and startup

`src/domain.rs` and `src/contracts.rs` are authoritative Rust types, not pseudocode.
No tokens cross the boundary. Backend owns authoritative session; UI owns views,
window, theme, transient selection and decoded artwork only.

Backend public functions to implement (with rustdoc):

```rust
pub fn initial_state() -> scytify::domain::AppState;
pub async fn run(port: scytify::contracts::BackendPort)
    -> Result<(), scytify::contracts::AppError>;
```

Paths inside the library use `crate::...`.
Frontend public function: `pub fn run(port: crate::contracts::FrontendPort)
-> iced::Result` (blocking native event loop on main thread).

Integrator creates a Tokio runtime (2 workers initially), enters its context,
calls `channels(backend::initial_state())`, spawns backend `run`, then calls
frontend `run` on main thread. Keep runtime alive until UI exits, wait bounded
shutdown, report backend panic/error without secrets. Handle early UI startup
failure by sending Quit. `main` owns initialization only, not queue logic.

Commands have unique `RequestId`s (UI monotonic low half of u64; desktop high
half). One completion per non-cancelled command; `Accepted` means accepted,
not finished playing. Fetch/Artwork complete with typed payload. Cancellation
may race completion; UI ignores stale IDs. Errors are safe display values.
`watch<AppState>` coalesces small state; bounded MPSC carries responses/events.
Never await a full event queue on the audio/session owner: bounded delivery task
with cancellation, bounded API concurrency (4), stop accepting new fetches under
backpressure; no unbounded spawned send tasks. Command overload shows busy.
Playback events and MPRIS updates run even when UI hidden. No hidden periodic UI
subscription: visible progress can tick 250–1000ms, backend state on transitions
and coarse anchors only. Desktop Show uses `Event::ShowWindow`; Quit checkpoints
and shuts down audio, emits `QuitReady`, then UI uses `iced::exit`.

## Backend brief (SOL)

Suggested concrete modules only: `backend/mod.rs`, `auth.rs`, `api.rs`,
`queue.rs`, `playback.rs`, `storage.rs`, `desktop.rs`. Merge small ones if simpler.
Document all named production functions; no trait-per-module scaffolding.

1. **Feasibility gate before polishing:** create personal developer app, register
   exact loopback `http://127.0.0.1:PORT/callback`, allowlist own account if required.
   Browser authorization code + PKCE S256, random state, exact callback path,
   loopback-only listener, timeout and cancellation, no client secret. Bind before
   opening browser; never log callback query. Request only necessary scopes
   (`playlist-read-private`, `playlist-read-collaborative`, identity as necessary;
   verify `streaming` feasibility for playback). Client ID is public configuration.
   Store refresh/reusable librespot credentials **only** in Secret Service;
   locked/missing service produces actionable blocked state. Never serialize
   librespot's default credential cache. Disable it or replace storage explicitly.
   Refresh single-flight, rotate credentials atomically; logout revokes locally,
   clears account metadata/session/art cache, stops playback.
2. **Critical uncertainty:** librespot OAuth source explicitly warns that some
   scopes may not work with custom client IDs. Prove a developer-client token can
   create a librespot session and decode audio; Web API login success is not proof.
   If separate supported librespot browser authorization is required, explain it
   explicitly and keep credential records separate. No copied official client ID,
   token scraping, speculative private endpoints, or silent alternative login.
   Stop and report a blocker if no supported flow works. No Spotify installation
   needed; do not use Web API playback controls as a substitute for local audio.
3. Web API: own playlists (filter owner against current user), playlist items,
   artist search, artist albums, album tracks. Paginate, debounce/cancel search,
   coalesce duplicate requests, honor Retry-After, refresh once on 401, distinguish
   403 restrictions. Upstream page limits may be below contract max 100. Map
   null/local/episode/relinked/unavailable items explicitly; no panic on omissions.
   Check live **2026 development-mode docs**, including renamed playlist-item
   endpoints and endpoint availability. Artist top tracks not required: album
   tracks cover requested browsing. Do not invent unsupported API workarounds.
4. Queue: immutable session membership on explicit Play; gather pages into SQLite
   staging and check playlist snapshot consistency before activation. Report
   progress/errors without replacing current session partially. Snapshot cap must
   be explicit if introduced; never silently truncate large libraries. Current
   occurrence stable across shuffle toggles. Shuffling permutes only remaining
   base occurrences, not current or explicit queue. No repeat within a cycle;
   duplicate playlist occurrences are distinct, so duplicate recordings can play.
   Disabling shuffle restores source order among still-unplayed occurrences.
   PlayNext FIFO prefix beats AddToQueue FIFO then base sequence; neither mutates
   the source. Explicit requests may intentionally repeat a track. Repeat Off
   stops after exhaustion; Queue rebuilds source cycle (not consumed explicit
   requests), avoiding immediate same occurrence across cycles when possible;
   One repeats on natural end, manual Next still advances. Previous >3s restarts
   current, otherwise history; manual history navigation can repeat by intent.
   Natural end must be idempotent and generation-tagged so stale player events
   cannot advance a replacement session. Repeat-one and explicit repeats are
   documented exceptions to shuffle no-repeat. Persist actual permutation,
   remaining/history, explicit prefix length and unique next entry ID.
5. Highest available lossy: request librespot 320kbps Vorbis, gapless/prefetch and
   normalization enabled. Verify actual 0.8 player options and audio behavior;
   do not claim codec/bitrate universally available. ReplayGain/normalization
   limiter avoids clipping; no fake normalization toggle. Handle preload failure,
   seek, output loss and unavailable tracks without advancing indefinitely.
6. SQLite metadata/session and artwork/audio managed by one bounded cache budget,
   default **1,000,000,000 bytes**, configurable. WAL/temp/staging counted; metadata
   LRU and artwork/audio LRU share total. Reserve/pin current+prefetch only; if
   storage would exceed limit, stream without disk caching or reject allocation.
   Do not assume librespot's cache limit includes SQLite/artwork. Bound downloads,
   artwork encoded bytes (e.g. 2MiB) and pixels/edge (max 512); validate HTTPS
   Spotify image host allowlist, redirects, decoding limits. Never fetch arbitrary
   localhost URLs. Persist large queue IDs on disk; hydrate only requested pages.
   Tiny config and credential-service records are outside disposable cache;
   session checkpoint must be separately documented/countable, never an unlimited
   cache exemption. If quota cannot fit a snapshot, report error before Play.
   Atomic versioned checkpoints on transitions and coarse position updates;
   validate corruption, schema, IDs, volume and queue invariants. Restore paused,
   no autoplay or automatic full-library refresh at startup. No offline promise.
7. Linux: tray Show/Quit and MPRIS org.mpris.MediaPlayer2 name, metadata,
   PlaybackStatus, Position, seek/Seeked, LoopStatus, Shuffle, volume and transport.
   MPRIS reads backend state; commands use the same queue reducer. No GTK app
   indicator dependency. Close hides only after tray registration succeeds;
   failure keeps a recoverable window and clear warning (do not strand a process).
   Desktop D-Bus registration should retry after service restarts.

## Frontend brief (Astra low)

`frontend::run(FrontendPort) -> iced::Result`. Use daemon boot to open one window,
handle CloseRequested explicitly, release image handles on close, send visibility
command; reopen on ShowWindow. A persistent channel subscription owns receivers
exactly once; no timer polling, no recreated receiver per view. Store at most
several pages per view and discard stale request generations. Test fake ports
without network/auth to work independently from backend.

Fixed layout: library/sidebar left, large content center, bottom player always
visible, queue panel only when requested. Dark, strong paired light/dark extruded
shadows and inset selected wells, with readable contrast and visible keyboard
focus. Iced single-shadow container is not enough for paired neumorphism: compose
cached canvas/custom drawing for paired rounded shadows, verify renderer blur
support; never render expensive full-window dynamic blur each frame. Flat
Material reference theme must exercise the same fixed layout. Virtualize fixed
height playlist/track/queue rows (viewport + overscan/spacers); a scrollable with
10,000 eagerly built children is not virtualization. Preserve selection/scroll
by stable IDs and occurrence IDs. Lazy artwork, bounded decoded LRU (~24MiB
starting cap), 64–256px thumbnails, no hidden image loading/rendering.

Local versioned TOML theme, `serde(deny_unknown_fields)`; tokens: colors
(background/surface/text/muted/accent/error/focus), font family and approved local
font paths, spacing scale, radii, paired shadow offsets/blur/strength, icon mapping,
style enum `neumorphic|material`. Provide dark-neumorphic and material examples.
Themes contain **no code**, templates, arbitrary expressions, layout definitions,
remote fonts, shell commands or plugins. Bound TOML/font/icon size and shadow
ranges; relative paths confined to theme directory (canonicalize symlinks), icons
from bundled names or sanitized local SVG. Malformed theme keeps previous good
theme with visible error. Font availability fallback and SVG safety must be
explicit. Theme hot reload not required. Keyboard transport, focus order, labels,
scalable text/HiDPI and screen-reader/accessibility verification are acceptance
work, not sacrificed for shadows.

Onboarding clearly explains developer Client ID/redirect setup, credential-service
errors, Premium/development-mode restrictions, authorization in external browser.
No credentials in forms. UI settings send full `Settings`; never overwrite a
cache/client setting just to change a theme. Queue pages use revision checks;
`QueueItem` includes optional hydrated metadata. Requests capped at MAX_PAGE_SIZE,
queue input vectors bounded by backend (e.g. 100 IDs/command).

## Milestones and validation

0. **Gate**: compile minimal Iced daemon + tray on Fedora/KDE Wayland and separately
   prove browser PKCE -> secure credential persistence -> librespot local audio.
   Confirm official endpoints with this exact developer account. Blockers surfaced
   before full feature implementation.
1. Parallel SOL backend core and Astra UI mock implementation under ownership above.
   Unit tests: empty/one/duplicate queues, shuffle toggle current preservation,
   cycle exhaustion, explicit FIFO priority, repeat/manual skip, stale end events,
   paused restore and corrupt checkpoints; paginated cancellation/rate limits;
   channel saturation; theme bounds/path rejection; virtual row calculation.
2. Integrate auth/library/playback/queue; real large playlists, playlist changed
   mid-snapshot, unavailable media, disconnect/reconnect, 401/403/429, credential
   service locked/unavailable, storage full/cache shrink. Never put test tokens in
   fixtures/logs. End-to-end requires owner's account; no interactive tests here.
3. Tray/MPRIS/hidden lifecycle and themes. Check repeated close/show, tray failure,
   lock screen, D-Bus restart, seek/repeat/shuffle via `playerctl`, graceful Quit,
   kill/restart restore paused. Gapless listening + recorded boundary test on a
   known continuous album; normalization/clipping test with real tracks.
4. Release-mode measurement on stated Ryzen 3700X/RTX 5060 Fedora44 KDE Wayland:
   process-tree RSS/PSS (`smem`/`/proc`), CPU (`pidstat -u`), cold start, 50k+
   synthetic metadata, visible scrolling/artwork, playback and hidden 10-minute
   runs. Targets **<300MB process-tree RAM** and **<1% of one core hidden CPU**
   are unverified goals, especially while decoding audio. Record renderer, display
   scale, compositor, cache sizes and separate paused/playing numbers. Lower
   caches/renderer overhead based on evidence; no guaranteed hardware promise.
5. Packaging Linux first (desktop file/icon, dependencies, config/cache paths),
   then macOS audio/Keychain/tray adaptation; no fake portable Linux D-Bus layer.

Scaffold validation: `cargo fmt --check`, `cargo check --locked`, `cargo test
--locked`. Full UI/audio dependency build and live integration remain pending.

## References and limitations

- https://docs.rs/iced/0.14.0/iced/daemon/index.html (also inspected local daemon.rs)
- https://docs.rs/librespot/0.8.0/ and librespot-oauth 0.8.0 source (custom scope warning)
- https://docs.rs/keyring/4.2.0/
- https://developer.spotify.com/documentation/web-api/concepts/quota-modes
- https://developer.spotify.com/documentation/web-api/tutorials/february-2026-migration-guide
- https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow
- https://developer.spotify.com/documentation/web-api/concepts/redirect_uri

Premium is required; developer access and Spotify policy may change. Librespot is
unofficial and can break or carry account/terms risks even for personal use.
No speculative private API, public distribution approval, offline downloads,
lossless, lyrics, recommendations, playlist editing, equalizer, plugin system,
multi-account, Spotify Connect discovery or full library prefetch in this scope.
