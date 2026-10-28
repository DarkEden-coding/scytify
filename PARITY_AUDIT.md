# Spotify desktop parity audit

Audit date: 2026-10-27. Scytify commit: `f25d2dfbfd224bb4d7bec68a098212dd9d0b967b`.

Scytify supports the basic music browsing and playback loop, but it does not yet replace Spotify desktop for library organization, playlist management, artist discovery, queue interaction, or accessible use. The highest-value work is completing those everyday workflows before matching Spotify's larger recommendation, social, and video features.

## Scope and installation

"Main app" means the official Spotify desktop app. Both apps were inspected on the same signed-in macOS account. Reference version: Spotify `1.3.6.1921`, macOS `26.6.2`, Apple Silicon. Spotify features vary by account, region, and rollout; this report describes the reference client available here.

- `git pull --ff-only` reported already up to date on `main`.
- `tools/install.sh` built the locked release and updated `~/Applications/Scytify.app`, then relaunched it.
- `codesign --verify --deep --strict` passed.
- The built and installed executables share UUID `A033F0E7-EDE6-383E-B229-F0EF281B75DB`. Their hashes differ because installation strips and signs the executable.
- Application code was not changed for this audit. This report is the only repository addition.

Evidence labels used below:

- **Live + source**: observed in the running app and confirmed in implementation.
- **Source**: implementation or contract lacks the workflow; it was not exercised end to end.
- **Observed**: visible discrepancy requiring further diagnosis before calling it a general defect.

P1 means a core workflow or accessibility gap. P2 means a useful workflow or presentation gap. P3 means an advanced feature, service dependency, or broader product expansion. These are suggested implementation priorities, not crash severities. Deliberately excluded features still count as parity gaps.

## Live verification

| Area | What was checked | Result |
| --- | --- | --- |
| Installation | Release build, bundle update, signature, executable UUID | Passed |
| Session | App relaunched with existing account, library, and playback available | Passed; a fresh OAuth flow was not tested |
| Home | Your playlists, Saved albums shelf, Made for you | Loaded; Discover Weekly and Daily Mixes appeared after their requests completed |
| Search | Search for `Tomppabeats`, top result, songs, artists, albums | Loaded; opening the artist and Harbor album worked |
| Artist | Tomppabeats in both clients | Scytify had an album grid; Spotify also exposed artist playback, following, popular songs, You Liked, featured playlists, radio, and About |
| Playlist | `idk`, 797 songs, in both clients | Initial tracks and artwork loaded; Spotify had editing, collaboration, download, search, sort, and column controls absent in Scytify |
| Track actions | Right-click on the first playlist track | No context menu; hover displayed play-next, enqueue, and heart actions |
| Playback | Space to pause, then Space to resume | Play/pause icon changed correctly; playback was restored to playing |
| Playback progression | Observed `talk2nice` advance to `REMEDY` | Current title, artwork, duration, and queue updated; this does not establish audio gaplessness |
| Queue | Opened from the player | Current song and upcoming tracks rendered; source confirms rows are display-only except explicit-entry removal |
| Devices | Opened during Scytify playback with Spotify desktop running | Both clients were listed, but Scytify omitted the Current device section and listed itself under Select another device |
| Lyrics | Opened for REMEDY | Loading resolved to a clear "No lyrics for this song" message; positive synced-lyrics rendering was not verified |
| Keyboard | Command+L, Control+L, Escape | Command+L did not focus search; Control+L did; Escape dismissed panels/settings |
| Settings | Opened Appearance and Account; inspected remaining settings source and Spotify settings | Existing controls rendered; no preference, credential, or library writes were performed |
| Accessibility | macOS accessibility tree on multiple Scytify screens | Only window/menu entries exposed; Spotify exposed labeled buttons, links, headings, tables, and sliders |

Loading screens were revisited after network requests completed. Initial "Loading..." states are not counted as failures. Browsing, panel inspection, and the pause/resume test left the existing playback queue intact. Scytify was returned to the original playlist page with playback running and panels closed. The search field retains the audit query.

## Core workflow gaps

| ID | Priority | Gap and user impact | Evidence and implementation entry point |
| --- | --- | --- | --- |
| G01 | P1 | Your Library is incomplete. Saved albums and followed artists have no sidebar entries. Most playlists owned by other users are filtered out, including library items visible in Spotify. Users cannot reach their full saved collection from the library. There is an exception for playlist IDs treated as Spotify-owned, so it is not strictly an owned-playlists-only implementation. | Live + source: [sidebar](src/frontend/view/sidebar.rs), [playlist filtering](src/backend/api.rs), [Spotify playlist exception](src/backend/discover.rs) |
| G02 | P1 | Global search cannot find playlists. Search only requests artists, albums, and tracks, leaving no general way to locate an arbitrary public or followed playlist. | Live + source: [search contract](src/contracts.rs), [search request](src/backend/api.rs), [search view](src/frontend/view/search.rs) |
| G03 | P1 | Playlists cannot be created, renamed, deleted, or have their image/description edited. Spotify exposes Create and Edit details. | Live + source: [actions](src/contracts.rs), [playlist page](src/frontend/view/tracklist.rs) |
| G04 | P1 | No add-to-playlist, remove-from-playlist, move/copy, or track reordering workflow. Enqueue is not a substitute for saving a track into a playlist. | Live + source: [actions](src/contracts.rs), [track row](src/frontend/view/tracklist.rs) |
| G05 | P1 | No search within a playlist or Liked Songs. Finding a song in the 797-track reference playlist requires scrolling or leaving the page for global search. | Live + source: [track page](src/frontend/view/tracklist.rs), [UI messages](src/frontend/mod.rs) |
| G06 | P1 | No track sorting or configurable table columns. Headers are static text. Spotify exposes Custom order, sortable headers, column visibility, and column resizing. | Live + source: [table header and columns](src/frontend/view/tracklist.rs) |
| G07 | P1 | Artist pages cannot start the artist's popular tracks or artist context. They only list albums. There is no Popular section, artist Play/Shuffle, or You Liked collection. | Live + source: [artist route](src/frontend/navigation.rs), [browse view](src/frontend/view/browse.rs), [queries and play sources](src/contracts.rs) |
| G08 | P1 | No artist follow/unfollow or followed-artist library. Artist membership is not represented by LibraryItem. | Live + source: [library contract](src/contracts.rs), [artist view](src/frontend/view/browse.rs) |
| G09 | P1 | Track context menus are absent. There is no shared place for Go to song/album/artist, Share, Copy link, Credits, Add to playlist, or track radio. The existing hover buttons cover only a small subset. | Live + source: [track row](src/frontend/view/tracklist.rs), [search rows](src/frontend/view/search.rs) |
| G10 | P1 | Now-playing cover, title, and artist text are not navigation controls. Users cannot jump from the song currently playing to its album or artist. | Live + source: [now_playing](src/frontend/view/player.rs) |
| G11 | P1 | Queue rows cannot play/jump to an occurrence, open album/artist, or show a context menu. They render cover and text, with removal only for explicit local queue entries. | Live + source: [queue_row](src/frontend/view/queue.rs) |
| G12 | P1 | No usable control tree for macOS assistive technology. Track names, buttons, inputs, and sliders were absent from accessibility output. This blocks screen-reader access and reliable semantic automation. | Live: repeated native AX inspection returned only window and menu entries; [Iced app setup](src/frontend/mod.rs), [dependency features](Cargo.toml) |
| G13 | P2 | Track selection is a single row index with mouse selection. There is no multi-select, Shift range, select-all, arrow-key row navigation, or Enter-to-play-selected command. | Source: [selected_row and SelectRow](src/frontend/mod.rs), [row mouse handlers](src/frontend/view/tracklist.rs), [shortcuts](src/frontend/shortcuts.rs) |
| G14 | P2 | Multiple artist names share a link to the first artist only. On collaborative tracks, the second or third artist cannot be opened directly. Album-header artist names are plain text as well. | Source: [artist_link and track_page](src/frontend/view/tracklist.rs) |
| G15 | P2 | Command-key conventions and native menus are missing. Command+L did not focus search, while Control+L worked. Scytify's menu tree lacks Spotify's File/Edit/View/Playback/Window/Help structure. | Live + source: [shortcut mapping](src/frontend/shortcuts.rs), [desktop integration](src/desktop.rs) |

## Library, discovery, and metadata gaps

| ID | Priority | Gap and user impact | Evidence and implementation entry point |
| --- | --- | --- | --- |
| G16 | P2 | Library organization has no type chips, Recents/Alphabetical/Creator sorting, user pins, folders, grid/list modes, compact mode, or collapse/expand control. The sidebar has a playlist filter and resizable width. | Live + source: [sidebar](src/frontend/view/sidebar.rs), [sidebar state](src/frontend/sidebar.rs) |
| G17 | P2 | Sidebar filtering searches only loaded playlist metadata. Loading stops growing at the configured bound, so it is not a full-library search for a very large collection. | Source: [sidebar filtering](src/frontend/sidebar.rs), [sidebar Show more/bound](src/frontend/view/sidebar.rs), [paging bound](src/frontend/paging.rs) |
| G18 | P2 | Home has Made for you, Your playlists, and Saved albums. No recently played shortcuts/history, release feed, editorial/genre shelves, or Music/Podcasts filters. The custom Discover stream is useful but does not provide those separate destinations. | Live + source: [home_view](src/frontend/view/browse.rs), [queries](src/contracts.rs) |
| G19 | P2 | Empty search is an instruction page rather than Spotify's Browse categories, genres, and recent-search history. There are no persistent result-type chips or search suggestions. Show all does provide artist/album/song routes. | Live + source: [search_start and search_view](src/frontend/view/search.rs), [search route handling](src/frontend/navigation.rs) |
| G20 | P2 | Artist pages omit verified status, monthly listeners, biography, artist imagery beyond the header, featured playlists, and radio links. Discography is one album grid without release-type filters. | Live + source: [Artist model](src/domain.rs), [artist route](src/frontend/navigation.rs), [browse view](src/frontend/view/browse.rs) |
| G21 | P2 | Playlist headers omit owner navigation, description, privacy/collaboration status, and full duration for a partially loaded collection. The reference 797-song playlist showed only the count in Scytify; Spotify showed owner and 36 hr 14 min. | Live + source: [Playlist model](src/domain.rs), [track_page summary](src/frontend/view/tracklist.rs) |
| G22 | P2 | Album metadata lacks release date/year, album type, copyright/label text, disc grouping, and credits. Spotify's Harbor page showed year, release date, rights text, and credits. | Live + source: [Album and Track models](src/domain.rs), [metadata parsers](src/backend/api.rs), [track page](src/frontend/view/tracklist.rs) |
| G23 | P2 | Explicit-content markers, date added, play counts, and a visible reason for unavailable tracks are absent. Track stores a playable flag, but no explicit flag or restriction explanation. | Source: [Track model](src/domain.rs), [parse_track](src/backend/api.rs), [track row](src/frontend/view/tracklist.rs) |
| G24 | P2 | No share/copy-link or incoming Spotify URL/URI workflow. A user cannot paste a Spotify link to open its destination, or produce a share link from an item. The macOS bundle does not register Spotify URL handling. | Source: [topbar](src/frontend/view/topbar.rs), [actions](src/contracts.rs), [bundle plist generation](tools/install-macos.sh), [main](src/main.rs) |
| G25 | P2 | No refresh/retry control on failed Home shelves. Mix and saved-album failures are latched; the UI displays failure text without an inline retry. Main pages and lyrics have different recovery paths. | Source: [ensure_mixes and shelf request state](src/frontend/mod.rs), [home shelves](src/frontend/view/browse.rs) |

## Playback, queue, and service gaps

| ID | Priority | Gap and user impact | Evidence and implementation entry point |
| --- | --- | --- | --- |
| G26 | P2 | Queue management lacks drag reordering, clear queue, multi-select, and a distinct explicit-queue/context presentation. Remote playback is preview-only for removal and play-next. | Live + source: [queue view](src/frontend/view/queue.rs), [remote action gating](src/frontend/view/tracklist.rs), [backend commands](src/backend/commands.rs) |
| G27 | P2 | No full Now Playing view with expanded art, playback context link, artist information, credits, and next-up card. Only queue and lyrics share the right panel. | Live + source: [player](src/frontend/view/player.rs), [main view](src/frontend/view/mod.rs) |
| G28 | P2 | No miniplayer or dedicated full-screen playback view. Maximizing the normal window is a different operation. | Live + source: [player controls](src/frontend/view/player.rs), [UI messages](src/frontend/mod.rs) |
| G29 | P2 | Audio quality and normalization are fixed rather than user-selectable. PlayerConfig requests 320 kbps, gapless, and normalization. No crossfade, Automix, normalization level, or quality controls are exposed. | Live settings comparison + source: [player_config](src/backend/playback.rs), [settings](src/frontend/view/settings.rs) |
| G30 | P2 | Autoplay changes require a later Scytify start instead of applying to the current session. The settings text explicitly says so. | Live + source: [Playback settings](src/frontend/view/settings.rs), [playback setup](src/backend/playback.rs) |
| G31 | P2 | No per-item hide/dislike, artist exclusion, explicit-content preference, or context-specific radio command. Discover supplies a separate endless recommendation stream, not those controls. | Source: [actions and play sources](src/contracts.rs), [settings](src/frontend/view/settings.rs), [discover](src/backend/discover.rs) |
| G32 | P3 | No user-managed downloads, offline mode, download quality/status, or guaranteed offline library. Encrypted audio caching exists but is not an offline-download workflow. | Source: [storage](src/backend/storage.rs), [playback](src/backend/playback.rs), [settings](src/frontend/view/settings.rs), [contracts](src/contracts.rs) |
| G33 | P3 | No local-file library or playback. Local tracks and episodes are dropped from track parsing rather than exposed as supported items. | Source: [parse_track](src/backend/api.rs), [domain](src/domain.rs) |
| G34 | P3 | No podcast, episode, audiobook, or chapter model, search, library, progress, speed, or playback workflow. | Live library comparison + source: [queries/play sources](src/contracts.rs), [domain](src/domain.rs), [parse_track](src/backend/api.rs) |
| G35 | P3 | No music videos, video playback, Canvas, or video preferences. | Live Spotify settings comparison + source: [domain](src/domain.rs), [player](src/frontend/view/player.rs), [settings](src/frontend/view/settings.rs) |
| G36 | P3 | Lyrics depend on a separate Spicy Lyrics key and catalog. A panel and timed-line/syllable implementation exist, so lyrics are not missing wholesale. Full-page presentation and Spotify catalog parity are not provided; positive timed rendering remains unverified in this session. | Live no-lyrics result + source: [lyrics view](src/frontend/view/lyrics.rs), [provider](src/backend/lyrics.rs), [settings](src/frontend/view/settings.rs) |
| G37 | P3 | Smart Shuffle and Spotify's playlist Mix/Explore workflows are absent. Standard shuffle and the custom Discover stream exist. | Live Spotify playlist/settings comparison + source: [actions](src/contracts.rs), [discover](src/backend/discover.rs) |
| G38 | P3 | No collaboration invitations, Blend creation, Jam, listening activity, friends, profile navigation, or release notifications. Shared playlists visible in Spotify's library are not equivalent to supported collaboration tools. | Live Spotify navigation/playlist comparison + source: [contracts](src/contracts.rs), [domain](src/domain.rs), [sidebar](src/frontend/view/sidebar.rs) |
| G39 | P3 | No in-app language selection, private-session/listening-activity controls, profile visibility preferences, notification preferences, or settings search. | Live Spotify settings comparison + source: [settings view](src/frontend/view/settings.rs) |
| G40 | P2 | Sign-in requires a personal developer Client ID plus separate playback consent; lyrics require another provider key. Spotify has a simpler consumer sign-in. This is an onboarding/service constraint, not a missing theme control. | Source: [settings/onboarding](src/frontend/view/settings.rs), [OAuth](src/backend/auth.rs), [playback consent](src/backend/playback_auth.rs) |

## Visual and interaction gaps

| ID | Priority | Gap and user impact | Evidence and implementation entry point |
| --- | --- | --- | --- |
| G41 | P2 | The default macOS window uses right-side minimize/maximize/close icons rather than native traffic lights, and Home/Search remain in a separate left block. Spotify uses a global Home/search bar with library-specific controls. | Live + source: [topbar](src/frontend/view/topbar.rs), [sidebar](src/frontend/view/sidebar.rs), [chrome](src/frontend/chrome.rs). System title bar is already available as an option. |
| G42 | P2 | Typography is visibly different. Scytify's bold titles/section labels rendered with a monospaced appearance while regular text was sans-serif. Liked Songs uses a green cover rather than Spotify's purple/blue treatment; heart actions also differ from the reference client's add/check controls. | Live visual comparison; [font selection](src/frontend/theme.rs), [liked cover](src/frontend/view/sidebar.rs), [library buttons](src/frontend/view/mod.rs). Font fallback needs diagnosis before choosing a fix. |
| G43 | P2 | Shared icon_button controls have no descriptive tooltip. Queue/lyrics/device controls and hover queue actions depend on glyph recognition, unlike the reference client's labeled accessible controls. Some save/disabled-volume controls do have dedicated tooltips. | Source: [icon_button](src/frontend/view/mod.rs), [player](src/frontend/view/player.rs), [track actions](src/frontend/view/tracklist.rs) |
| G44 | P2 | Large hero artwork reuses card-sized images and decoded thumbnails, so it can look soft on Retina displays. The reference Harbor and artist headers exposed this visually. Art resolution should follow drawn size and display scale while retaining cache limits. | Live visual observation + source: [card_image selection](src/backend/api.rs), [artwork decoding/request sizes](src/frontend/artwork.rs), [hero drawing](src/frontend/view/browse.rs) |
| G45 | P2 | Playlist hero/action/table headers scroll as ordinary content rather than providing Spotify-style persistent context/actions. Table columns also cannot be resized when panels reduce the available width. | Source: [track page composition](src/frontend/view/tracklist.rs), [scroll_page](src/frontend/view/browse.rs) |

## Observed issues and source-level risks

These need focused reproduction before implementation. They are not included as proven general failures of playback or library writes.

1. **P1, current device missing during local playback.** Spotify showed "Playing on Scytify (Computer)" and the Scytify progress bar was advancing. Scytify's picker showed only "Select another device", including Scytify itself. Reproduce after startup and after device refresh; compare `active_device_id`, `is_active`, and `is_this_app`. The UI omits the current section whenever [devices::active](src/frontend/devices.rs) returns None. Do not transfer playback merely to paper over the display state.
2. **P2, displayed row numbering after filtered items.** [parse_track/map_page](src/backend/api.rs) preserve each surviving track's raw `source_index`, and Play uses it correctly. [track_row](src/frontend/view/tracklist.rs) labels rows with `first + index + 1`. After a local file or episode is dropped, displayed numbers can diverge from Spotify's original positions. Test a mixed playlist across page boundaries. This is a source-level risk, not a live reproduction in the reference playlist.
3. **P2, generic queue metadata near the hydration boundary.** One visible row showed "Spotify track" while earlier rows had full metadata. The backend deliberately bounds hydration. Verify whether approaching the row resolves it, and whether a placeholder should clearly indicate loading versus unavailable metadata. This audit did not establish a persistent failure.

An apparent Harbor save-state mismatch was investigated and excluded: the artist page opened a different album release than the pinned library item. Spotify also offered "Save to Your Library" on that same release. Matching names and artwork are not sufficient evidence of inconsistent saved state.

## Checks and documentation drift

| Check | Result |
| --- | --- |
| Locked release installation | Passed |
| Bundle signature verification | Passed |
| `cargo test --locked` | Passed: 159 Scytify unit tests, 3 integration tests, 4 vendored OAuth tests; 166 total |
| `cargo fmt --check` | Failed on existing formatting in `examples/render_dock_icon.rs`, `src/desktop/macos_dock.rs`, and `src/dock_icon.rs` |
| `cargo clippy --locked --all-targets -- -D warnings` | Failed on `needless_range_loop` at `src/dock_icon.rs:211` and `:213` |

The failed checks predate this report and were not automatically fixed as part of an audit.

Existing documentation should not be used as the current parity checklist:

- README still says library editing is read-only and lists lyrics and Discover Weekly as outside the initial release. The current source supports saving tracks/albums/playlists, a lyrics provider, and Made for you.
- VALIDATION lists 34 tests and says audio caching is not implemented. The current suite and playback/storage code have moved beyond that description.
- FRONTEND describes an older main-list paging behavior. The current [paging module](src/frontend/paging.rs) loads a bounded run around the viewport and replaces it on distant jumps. The 1000-item working-set bound is not a 1000-song maximum for the main track list. Sidebar loading still has its own bound.

## Recommended implementation order

1. Fix accessible control exposure and macOS Command shortcuts. Diagnose the missing current-device state. These affect basic operation and confidence in playback state.
2. Add complete library destinations and playlist search results. Establish how followed/collaborative playlist access can work with current API restrictions before promising full coverage.
3. Add playlist-local filtering and sorting, a shared context menu, independent artist links, and navigation from now-playing/queue metadata. Preserve raw source positions when filtering or sorting a playback context.
4. Add artist popular tracks/playback/following, then playlist editing and bulk selection. Both require new backend contracts rather than only new buttons.
5. Complete queue interaction, metadata, persistent headers, tooltips, and artwork sizing. Add focused behavior tests for queue occurrences and filtered/sorted playback positions.
6. Treat downloads, local files, spoken-word content, social features, Smart Shuffle, and video as separately scoped projects with explicit service capability checks.

## Remaining acceptance work

The audit covers every current Scytify view module and the relevant query/action/domain contracts, with live spot checks above. It is not an exhaustive certification of every account or platform state. The following remain unverified:

- Library writes and rollback against the live account, playlist edits, and destructive flows. No likes, saves, removals, or consent changes were made.
- Transfer to another device and back, remote queue writes, remote seek/volume, hardware media keys, AirPods controls, and audio-output disconnect recovery.
- Positive line/syllable-synced lyrics, manual-scroll recovery, and lyric seek alignment.
- Large collections beyond the reference 797-track playlist, filtered raw rows, rapid scrollbar jumps, unavailable-track recovery, and pagination under rate limits.
- VoiceOver interaction, complete keyboard traversal, narrow-window/zoom extremes, and custom-theme visual coverage.
- Offline/network interruption, credential expiry, fresh sign-in, Linux desktop integration, long playback sessions, measured CPU/RAM, audio fidelity, and gapless boundaries.

These are acceptance-test gaps, not evidence that the existing implementations fail.

## Implementation status, 2026-10-27

This section records the bounded implementation after the audit above. The audit observations and acceptance history are preserved. These changes do not close all 45 gaps. Automated checks and the live acceptance notes below distinguish implemented behavior from endpoint restrictions.

| IDs | Status | Implemented behavior and remaining limits |
| --- | --- | --- |
| G01 | Partial | Saved albums and Followed artists have sidebar destinations. Library playlists no longer filter out other owners. Spotify still controls which playlists and contents this developer client can access. Followed artists use cursor paging. New sign-ins request `user-follow-read`; existing tokens require user-initiated renewed consent. No live consent was granted during testing. |
| G02 | Covered in code | Global search includes playlists, a playlist shelf with artwork, and paged Show all. Search remains subject to Spotify's per-type development-mode limits. |
| G05, G06 | Partial | Playlist, album and Liked Songs pages filter loaded songs by title/artist/album and stably sort by title, artist, album or duration. Custom order restores source order. The loaded count and Load more control expose the existing working-set bound. Filters are local to that loaded section, not a whole-library index. View sorting does not reorder playback. Raw offsets and duplicate occurrences remain intact. Column configuration/resizing is deferred. |
| G07 | Partial | Artist discography offers Popular songs via the official top-tracks endpoint. Denied/unavailable capability is a visible page error; Back returns to discography. Individual popular songs play through the existing standalone source. Full artist-context playback, shuffle and You Liked remain deferred. |
| G08 | Partial | Followed-artist destination and cursor contract are present. Artist follow/unfollow remains deferred. Only the read permission is requested on new sign-ins, with no artist-follow write scope. |
| G09, G24 | Partial | Track/search rows offer a right-click action sheet shared with now-playing and queue titles. Actions include standalone Play song, supported queue operations, library save, album and individual-artist navigation, and Copy Spotify song link. Playlist/album headers also copy links. Playlist content editing, credits, radio, incoming URL/URI routing and native URL registration are deferred. |
| G10, G14 | Covered in code | Player cover/title opens the album; each collaborator name opens that artist. Track rows, queue artist labels and album-header artists use individual links. The now-playing area also opens song actions on right-click. Width clipping still needs live visual acceptance. |
| G11, G26 | Partial | Queue titles open the shared actions and artist labels navigate individually. Play song starts a new standalone context, not an occurrence jump. Existing local explicit-entry removal remains unchanged. Queue occurrence jumping, clearing, multi-select and drag ordering are deferred rather than guessing identity from duplicate track IDs or redesigning Spirc's queue ownership. |
| G13 | Partial | Up/Down selects loaded track-page rows; Enter plays the selected occurrence and scrolling reveals it. Text input keeps captured keys. Window replacement/prepend clears stale selection. Multi-select, range selection, select-all and full keyboard focus traversal remain deferred. |
| G15 | Partial | macOS Command equivalents work alongside Control; Settings hints reflect the platform. Native menu overhaul remains deferred. |
| G25 | Covered in code | Home offers Retry failed shelves while retaining single-flight requests and failure latches. |
| G43 | Partial | Shared icon controls now have descriptive action hints. These tooltips do not create native accessibility semantics, and exhaustive tooltip placement needs live acceptance. |
| Observed device/numbering issues | Fixed in code | During Playing, an absent active-device report may display this computer as current without transferring playback. Reported activity wins, and the inferred current row is excluded from alternatives. Track numbers use raw `source_index + 1`, including gaps from skipped rows. Device inference is display-only and cannot replace authoritative backend activity. |
| G03, G04 | Deferred | Playlist creation/rename/content edits require ownership, snapshot-aware mutations and occurrence-safe UI flows. No partial mutation flow or new permissions were added in this effort. Existing track/album/playlist library save/remove remains supported. |
| G12 | Deferred, toolkit blocker | The pinned Iced 0.14 dependency has no AccessKit feature or button accessibility-label API. The observed missing macOS control tree needs a toolkit integration/change beyond this bounded task. Keyboard additions and tooltips are not a screen-reader fix. |
| G16–G23, G27–G42, G44–G45 | Deferred except changes listed above | No broad library organization, metadata expansion, native chrome/font/artwork redesign, recommendations, offline/local-file support, spoken-word/video/social features, playback-engine expansion or queue overhaul was undertaken. G20 gains only the Popular songs destination noted under G07. |

Checks on the integrated tree: formatting, strict locked all-target Clippy, locked tests and a locked release build. The test suite now includes 166 Scytify unit tests, 3 integration tests and 4 vendored OAuth tests, 173 total. New behavioral coverage exercises filtered/sorted duplicate playback offsets and keyboard selection, explicit shelf retry, playlist search/non-owned playlist parsing, cursor paging past skipped artist rows, popular-song capability failure, Command mapping and the missing-current-device fallback. Existing dock renderer formatting and `needless_range_loop` errors are fixed. Release compilation succeeds with a nonfatal toolchain warning: debug-info stripping via `rust-objcopy` cannot load `libLLVM.dylib`. No build-profile or environment override was introduced.

The implementation delegate did not install or operate the app. Parent review installed the combined build and performed the acceptance checks below. No fresh OAuth, live library mutation, device transfer, external messaging, push or PR action was performed.


### Parent review and live acceptance

The final build was installed through `tools/install.sh` and its bundle signature verified. Live checks confirmed saved-album navigation, non-owned playlists in the sidebar, Command+L search focus, playlist search and its paged grid, local filtering and title sorting of the 797-song playlist, raw source row numbers, arrow-key selection, right-click song actions, album-header artist navigation, and the current remote device plus its tooltip. Search artwork exposed two missing Spotify cover hosts; the HTTPS redirect allowlist now accepts the exact `image-cdn-ak.spotifycdn.com` and `image-cdn-fa.spotifycdn.com` hosts while retaining scheme, port, redirect, and byte limits.

Followed artists was denied with the old token. New authorization requests include the read scope and the error now explains how to renew consent. Existing credentials were retained and no browser consent was launched. The artist Popular songs endpoint was also denied by Spotify for this developer client/account; its capability explanation rendered correctly. This destination is implemented but remains unavailable on the tested account.

Review also found that playback-session playlist hydration assigned every duplicate track its first occurrence's offset. Positions are now assigned before missing metadata is dropped, and a regression covers duplicate IDs separated by failed metadata. Recommendation metadata continues to have no collection position.

User playback moved to an iPhone during acceptance. Testing left it there and avoided starting selected songs, enqueueing, changing volume, or editing the live library. Clipboard content, narrow-window layout, positive artist-follow access after renewed consent, and VoiceOver remain unverified. Filters and sorts intentionally cover loaded songs; playlist editing, queue occurrence jumping, full artist-context playback, screen-reader integration, and the larger product features remain deferred.
