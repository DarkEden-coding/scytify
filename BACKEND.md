# Backend implementation and integration

## Dependencies

The backend uses the following root dependencies (versions/features tested here):

```toml
env_logger = "0.11"
log = "0.4"
serde_json = "1.0"
rand = "0.9"
sha2 = "0.10"
base64 = "0.22"
url = "2.5"
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls-native-certs"] }
rusqlite = { version = "0.39", features = ["bundled"] }
directories = "6.0"
keyring = { version = "4.2.0", default-features = false, features = ["v1", "zbus-secret-service-keyring-store"] }
librespot-oauth = { version = "=0.8.0", default-features = false, features = ["rustls-tls-native-roots"] }
librespot-core = { version = "0.8.0", default-features = false, features = ["rustls-tls-native-roots"] }
librespot-connect = { version = "=0.8.0", default-features = false }
librespot-protocol = "=0.8.0"
futures-util = "0.3"
librespot-playback = { version = "0.8.0", default-features = false, features = ["alsa-backend", "rustls-tls-native-roots"] }
open = "5.3"
```

Tokio uses `rt-multi-thread`, `sync`, `time`, `net`, `macros`, and `io-util`. `Cargo.lock` pins the transitive `vergen` build dependency to 9.0.6 for librespot compatibility.

The logging change adds these lockfile entries; no existing package versions were
changed, and `log` was already present transitively:

```text
anstream 1.0.0, anstyle 1.0.14, anstyle-parse 1.0.0,
anstyle-query 1.1.5, anstyle-wincon 3.0.11, colorchoice 1.0.5,
defmt 1.2.0, defmt-macros 1.2.0, defmt-parser 1.1.0,
env_filter 2.0.0, env_logger 0.11.11, is_terminal_polyfill 1.70.2,
jiff 0.2.38, jiff-core 0.1.1, jiff-static 0.2.38,
once_cell_polyfill 1.70.2, utf8parse 0.2.2
```

The separate playback OAuth change makes librespot-oauth 0.8.0 a direct,
locally patched workspace dependency and adds Tokio to its dependency list.
No existing package versions change; vergen stays at 9.0.6. Cargo's workspace
resolution also records eight packages for upstream's optional native-TLS feature:
foreign-types 0.3.2, foreign-types-shared 0.1.1, hyper-tls 0.6.0,
native-tls 0.2.18, openssl 0.10.81, openssl-macros 0.1.1,
openssl-sys 0.9.117, and tokio-native-tls 0.3.1. The app builds with rustls;
these optional native-TLS packages are not compiled.

## Implemented behavior

OAuth uses authorization-code PKCE with a pre-bound fixed loopback listener, random state, an exact callback path, bounded request parsing, and a 180-second timeout. CancelLogin aborts that task immediately. Tokens and rotated refresh tokens are stored only in Linux Secret Service; SQLite/settings never contain them. Startup refreshes expiring tokens before connection. Periodic refresh does not replace an active decoder. Synchronous credential operations use a blocking context to avoid nesting zbus's runtime inside a Tokio worker; a locked vault can delay that serialized operation.

Playback creates one librespot 0.8 `Session` with the KEYMASTER client ID and
separate playback credentials, one ALSA `Player`, and one software mixer. The
configured developer ID still seeds the existing stable device ID; changing the
session's OAuth identity does not change the advertised device identity. Spirc
registers `Custom Spotify (hostname)` as a Computer Connect device and drives that
same player. It owns local transfer handling, queue advancement, shuffle/repeat,
and published device state. The decoder requests 320 kbps, normalization, gapless
playback, and one-second position events.

The custom queue remains the local fallback when Connect initialization fails.
After Web API sign-in the picker includes this computer. Until playback consent
creates a session, that synthetic row is disabled and explains the required consent.
Whenever the decoder exists, local handoff remains available. Until Connect
registers it, its stable local ID is `custom-spotify:local-player`; this ID is never
sent to Spotify's transfer endpoint. A real Connect entry replaces the synthetic
entry without duplication.

A transfer here without Spirc fetches fresh GET `/me/player`, resolves playlist,
album, or Liked Songs membership, then pauses the remote device before loading
the local decoder at the elapsed playback position. Source selection uses raw
`Track::source_index` when available and otherwise the first matching track.
The Web API does not identify duplicate occurrences within a context. If context
resolution fails, the current track plays alone. Shuffle, repeat, and paused state
carry across. Local ownership persists through stale, paused, and empty remote
polls until an explicit transfer away succeeds. Transferring away pauses local
audio before sending the official transfer request. A failed request leaves local
audio paused and available to resume.

Session/Connect initialization has a 30-second deadline so failed internal
protocol access does not keep Web API authorization pending indefinitely. With Spirc available, source loading validates and gathers membership using
the existing official API loader, then hands the resulting tracks to Spirc. The
actor never advances the custom queue from Spirc terminal events. This avoids two
owners loading the same decoder. Restoring a checkpoint does not activate this
device or interrupt another device. A restored selection is loaded only on Play.
Local observations mirror the current track, position, options, and known next
tracks into the disk checkpoint without issuing decoder commands. A matching
preview preserves the full previously loaded source tail. After an unknown native
transfer or queue edit, restart restoration is limited to the available preview;
the live Connect queue itself remains owned by Spirc.

Web API OAuth scopes are `playlist-read-private playlist-read-collaborative user-read-private
user-library-read streaming user-read-playback-state user-modify-playback-state
user-read-currently-playing`. Tokens from earlier versions must be replaced by
signing out and signing in again. Library and playback 403 responses return a
Permission error with reauthorization instructions. Premium and developer-app
allowlisting are still required.

### Connect observation and official control

A second dealer subscription on the same Session observes
`hm://connect-state/v1/cluster`. It projects the active device, device list,
track, position anchored to the server timestamp, pause/loading status, volume,
shuffle, context and track repeat, context URI, and up to 50 next tracks.
Missing track metadata is hydrated through the existing bounded metadata cache.
Dealer observations use a latest-value watch channel; state publication and UI
commands retain the existing bounded channels.

`AppState.connect: ConnectState` exposes `source: RemoteSource` with `Live`,
`Polling`, or `Unavailable`, `this_device_id`, `active_device_id`, `devices`,
`context_uri`, and `queue_preview`. Each `PlaybackDevice` contains an optional
`id`, `name`, `device_type: DeviceType`, `is_active`, `is_this_app`, optional
`volume_percent`, `supports_volume`, and `is_restricted`. Device types include
`Computer`, `Smartphone`, `Tablet`, `Speaker`, `Tv`, and `Other(String)`.
`ConnectState::is_remote()` identifies another active device. `AppState.player`
remains the common local/remote snapshot, including the monotonic position anchor.
MPRIS reads that snapshot and sends its existing actions through the same router.

New actions are `TransferPlayback { device_id, play }`,
`SetDeviceVolume { device_id, volume_percent }`, and `RefreshDevices`. Transport,
seek, shuffle, repeat, volume, source Play, and queue actions target the other
active device through the official Web API. Local transport uses Spirc while it
owns the player, or the existing decoder/queue when Connect setup failed. Queue
append uses the official API for Connect playback on either device. Local source
jobs reject a result after authentication, queue revision, or active device changes.
Account observations and hydration reject stale authentication epochs; newer live
observations invalidate pending polling snapshots.

The official endpoints are GET `/me/player`, `/me/player/devices`,
`/me/player/queue`; PUT `/me/player` for transfer, `/play`, `/pause`, `/seek`,
`/shuffle`, `/repeat`, `/volume`; and POST `/next`, `/previous`, `/queue`.
Album and playlist playback sends `context_uri` and raw `offset.position` to the
remote device. Single tracks send `uris`. Liked Songs has no documented playable
Web API context, so remote Play loads the liked collection and starts a bounded
100-track URI window at the selected occurrence. Official queue append requests
are sequential within a batch and can partially succeed if a later request fails.
The official API cannot insert at the front of a queue or remove an occurrence;
Connect PlayNext and RemoveQueued return a capability error. The custom fallback
queue keeps those operations. The queue preview is not a complete editable queue.

If the Connect runner/session fails, or no cluster update arrives for 30 seconds,
observation falls back to GET `/me/player` roughly once a second while visible and
every eight seconds while hidden. Devices and queue are refreshed every ten
seconds in polling mode, and after successful remote controls or RefreshDevices.
Poll requests share the four-job budget with other API work and never overlap.
Official playback controls are serialized, including whole queue-append batches.
Playback requests share a Retry-After deadline so device refresh and commands
cannot bypass a polling rate limit.
Failures use exponential backoff capped at 64 seconds; 429 respects Retry-After.
Authentication/permission notices are deduplicated until a successful poll.
Any newer dealer update restores Live. A quiet paused account can therefore show
Polling even while the dealer connection remains open.

Spotify Connect and librespot playback are internal, unofficial protocols. Spotify
can change or deny access without notice; usage may be affected by Spotify's
terms. Official API fallback preserves observation/control where authorized, but
cannot advertise a replacement Connect speaker to other Spotify clients if Spirc setup fails. The local picker can still hand off to the custom decoder when a valid librespot
session exists. Without playback authorization there is no local audio fallback. No live
account, transfer, audio output, or real-device protocol compatibility was tested.

### Connect diagnostics

Logging defaults to off. To capture the failure reason and librespot internals:

```sh
RUST_LOG=librespot=debug,custom_spotify=debug cargo run --release --locked 2> /tmp/custom-spotify.log
```

`connect_error` is logged at warn level and produces one concise notice per player
installation. Backend notices wait for dismissal before later backend notices or
request errors replace them. Logs can contain account, device, and track metadata;
review them before sharing.

### Separate playback authorization

The developer app's Web API PKCE flow and token refresh are unchanged. Playback
uses `backend::playback_auth::CLIENT_ID`, the KEYMASTER value
`65b708073fc0480ea92a077233ca87bd` from librespot-core 0.8.0 `src/config.rs`.
The redirect is exactly `http://127.0.0.1:5588/login`, verified against librespot
v0.8.0 `src/main.rs`: the default `oauth_port` is `Some(5588)` around line 1278,
and the OAuth builder formats `http://127.0.0.1{port_str}/login` around line 1953.
The five requested playback scopes are `streaming app-remote-control
user-read-playback-state user-modify-playback-state user-read-currently-playing`.
All five appear in that file's `OAUTH_SCOPES` around lines 176–204. This is its
Connect/streaming subset; this app does not request librespot's library-write scopes.

`librespot-oauth` 0.8.0 performs playback PKCE with its async API. A local patch in
`vendor/librespot-oauth` makes that API's callback listener truly asynchronous:
upstream's async method calls a blocking TCP listener. The patch binds before
opening the browser, bounds callback reads, checks path/state, reports denial and
browser failures, and releases the port when the 180-second outer timeout or task
cancellation drops the future. Port 5588 being busy is reported explicitly. The
sync API remains available. See the vendor README for patch details. OAuth errors
are mapped to safe messages rather than displaying callback codes or token bodies.

Secret Service entry `spotify-oauth` holds only the developer Web API tokens.
`spotify-librespot-oauth` holds the playback access/refresh/expiry tokens and
reusable `Credentials`, bound to the Web API user ID. After a session succeeds,
`session.username()` and `session.auth_data()` become
`AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS`, matching librespot's cache design.
`librespot-core/src/session.rs` reconnects token-authenticated AP sessions using
returned reusable credentials before exposing `auth_data`; `src/login5.rs`
`login5_request` uses `SessionConfig.client_id` for its StoredCredential method.
Spirc still calls `Session::connect` itself after registering dealer listeners.

Connection order is stored AP credentials, then the independent playback OAuth
token with a bounded refresh if expiring or stored credentials failed, then
explicit browser consent. Startup never requests browser consent. Successful
rotated tokens and session credentials return to the actor and are saved only
after auth-epoch/cancellation checks, preventing late writes after sign-out.
Sign-out attempts to clear both entries even if one deletion fails. Playback
consent for a different Spotify account is rejected before installing the player.

`ConnectState.playback_auth: PlaybackAuth` has `Ready`, `NeedsAuthorization`,
`Authorizing`, and `Failed(String)`. `Action::AuthorizePlayback` starts the explicit
flow from Settings → Account or the device picker. Without a session, remote
polling/control remains available and local Play/transfer explains the missing
consent. Connect failures with an otherwise valid session keep the existing local
decoder fallback. Pausing the fallback decoder checks its local playing flag, so
startup polling cannot pause a Stopped player.

The offline fixture supports `CS_PREVIEW_REMOTE=1` and supplies fictitious Kitchen
speaker, Phone, a restricted Living room TV, and this-app devices with a synthetic playing track.
`CS_PREVIEW_NO_CONNECT=1` replaces the local registration with the synthetic row
and selects the polling footer; combine it with `CS_PREVIEW_REMOTE=1` and
`CS_PREVIEW_DEVICES=1` to inspect the failed-registration picker. It starts no
Spotify, credential, audio, desktop, or network services.
`CS_PREVIEW_PLAYBACK_AUTH=needs-authorization` shows the new consent controls;
combine it with `CS_PREVIEW_NO_CONNECT=1` for the synthetic local-device row.

Queries: `OwnPlaylists`, `PlaylistTracks`, `AlbumTracks`, `ArtistAlbums`, `LikedTracks` (`me/tracks` → `PageData::Tracks`), `SavedAlbums` (`me/albums` → `PageData::Albums`), `Search` and the paged `SearchArtists`/`SearchAlbums`/`SearchTracks`. Library pages unwrap the saved-item wrappers (`{added_at, track}`, `{added_at, album}`). Pages drop rows: playlist pages keep only playlists the user owns, and track pages drop local files, episodes and unparseable rows. Every page therefore reports `next_offset`, the request offset plus the raw item count, and the UI continues paging from it. Each remaining `Track` also carries `source_index`, its raw position in the upstream listing, which the UI uses for `start_offset`; this keeps play offsets aligned with the occurrences `load_source` counts. `Search { text }` makes one `type=artist,album,track` request and returns `PageData::Search(SearchResults)`: up to `SEARCH_LIMIT` (10, Spotify's development-mode maximum) artists, albums and tracks, skipping null entries; it is not paged and ignores the page offset. The single-type search queries return normal pages of at most 10 items for "see all" views. Other pages request at most 50 items.

Playlist, album and Liked Songs (`PlaySource::LikedSongs { start_offset }`) Play operations gather all upstream pages before atomically replacing queue membership. Liked Songs has no snapshot ID, so every gathered page must report the same `total`; a change during loading returns StaleQueue. Liked Songs offsets match `Query::LikedTracks` rows because that endpoint returns no local files or episodes. A single search result plays through `PlaySource::Track`. Start offsets are interpreted against raw Spotify source occurrences; selecting a local, episode, or unavailable occurrence returns an explicit error rather than silently moving the index. Visible queue pages hydrate metadata in bounded batches while retaining duplicate occurrence IDs. The actor keeps at most 512 hydrated tracks, prunes large batches in one pass, and fetches missing current-track metadata as playback advances.

At most four network/auth/session jobs are pending. The actor selects commands, jobs, refresh ticks, and player events, and cancellation uses Tokio abort handles. Event-channel backpressure is moved to short sender tasks so it cannot block Quit or playback. Persistence errors are surfaced; corrupt settings/session data fail startup rather than being silently discarded. Restored playback remains paused.

Artwork uses HTTPS Spotify CDN hosts only, disallows explicit ports, revalidates every redirect with redirects disabled in reqwest, and streams into a strict 2 MiB bound (including chunked responses). Artwork downloads have their own budget of twelve concurrent jobs (twice the UI's in-flight bound, to absorb cancelled jobs not yet joined), separate from the four network/auth/session jobs, so visible thumbnails never make browsing report busy. Downloaded artwork is cached by URL under the configured total byte limit with SQLite-indexed LRU eviction; missing files self-heal their index rows.

Playlists, artists and albums carry `artwork_url`: the smallest Spotify image whose shorter edge is at least 160px, else the largest sized image, else the first unsized one (playlist mosaics). Album-track pages and album Play make one extra `albums/{id}` request to attach the album (and its cover) to simplified tracks; failure of that lookup does not fail the page.

## Real runtime limitations

Web API OAuth uses only the user's developer client ID and the registered
`http://127.0.0.1:43817/callback` redirect. Playback/Connect uses the separate
KEYMASTER OAuth/session design above. Real-account logs on 2026-10-21 showed AP
login succeeding with a developer token, then login5 returning `BAD_REQUEST` with
the developer session ID and `INVALID_CREDENTIALS` after switching only the
session ID to KEYMASTER. The token's originating client identity must also match.
This design follows librespot's binary; using Spotify's client identity and
internal APIs remains a Terms of Service grey area. Spotify may reject it due to
account policy or protocol changes. No live account was used in automated verification. ALSA is the compiled output backend in this dependency set, so machines without a usable ALSA device receive a sink/player error. The bounded cache currently stores artwork only. Persistent decoded-audio caching and offline playback are explicitly deferred and are not claimed.
