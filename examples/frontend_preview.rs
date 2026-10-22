//! Offline, synthetic UI fixture. Never starts production services or reads account data.
use custom_spotify::{backend, contracts::*, domain::*, frontend};

/// Bound fixture collections using the same page contract as production.
fn page<T: Clone>(items: &[T], request: PageRequest) -> Page<T> {
    let start = (request.offset as usize).min(items.len());
    let end = (start + usize::from(request.limit.min(MAX_PAGE_SIZE))).min(items.len());
    Page {
        offset: request.offset,
        total: Some(items.len() as u64),
        items: items[start..end].to_vec(),
        has_more: end < items.len(),
        next_offset: end as u64,
    }
}

/// A track listing page whose rows carry their source position, like production pages.
fn listing(tracks: &[Track], request: PageRequest) -> Page<Track> {
    let mut page: Page<Track> = page(tracks, request);
    for (i, track) in page.items.iter_mut().enumerate() {
        track.source_index = Some(request.offset + i as u64);
    }
    page
}

/// Fictitious catalog: artists with albums, album tracks, playlists mixing those tracks,
/// and a library of liked songs and saved albums.
struct Catalog {
    artists: Vec<Artist>,
    albums: Vec<Album>,
    tracks: Vec<Track>,
    playlists: Vec<Playlist>,
    liked: Vec<Track>,
    saved_albums: Vec<Album>,
}

const ARTISTS: [&str; 4] = [
    "The Paper Satellites",
    "Juniper Static",
    "Moth & Lantern",
    "Coastal Arithmetic",
];
const ALBUMS: [&str; 8] = [
    "After the Last Tram",
    "Lowlight Atlas",
    "Velvet Machinery",
    "Small Hours Club",
    "Glasshouse Weather",
    "Northbound Postcards",
    "Tidal Equations",
    "Harbor Light Index",
];
/// Four tracks per album.
const TRACKS: [&str; 32] = [
    "Windows at Dusk",
    "Quiet Intersections",
    "A Map of Sunday",
    "Blue Hour",
    "Signals Across the River",
    "Home Before Morning",
    "Paper Moons",
    "Static Bloom",
    "Afterimage",
    "Copper Skyline",
    "Slow Satellites",
    "Lanterns in the Rain",
    "Folded Letters",
    "Night Ferry",
    "Half-Remembered Song",
    "Neon Orchard",
    "Echo Cartography",
    "Soft Machinery",
    "Midnight Arithmetic",
    "Salt and Static",
    "Low Tide Waltz",
    "Radio Fields",
    "Glass Lighthouse",
    "Postcard from Nowhere",
    "Pocket Constellations",
    "Weathervane",
    "Second Platform",
    "Dim the Harbor",
    "Lighthouse Static",
    "Index of Tides",
    "Elevator Hymn",
    "Last Call Lanterns",
];
const PLAYLISTS: [&str; 12] = [
    "Evening Commute",
    "Focus · Instrumentals",
    "Weekend Discoveries",
    "Late Night Radio",
    "Slow Sundays",
    "Rainy Window",
    "Kitchen Dance Floor",
    "Deep Work",
    "Road Trip North",
    "Morning Coffee",
    "Synth Afternoons",
    "Quiet Corners",
];

impl Catalog {
    fn new() -> Self {
        let artists: Vec<Artist> = ARTISTS
            .iter()
            .enumerate()
            .map(|(i, name)| Artist {
                id: SpotifyId(format!("previewartist{i}")),
                name: format!("{name} (synthetic)"),
                artwork_url: Some(format!("synthetic://artist/{i}")),
            })
            .collect();
        let albums: Vec<Album> = ALBUMS
            .iter()
            .enumerate()
            .map(|(i, name)| Album {
                id: SpotifyId(format!("previewalbum{i}")),
                name: (*name).into(),
                artists: vec![artists[i % artists.len()].clone()],
                artwork_url: Some(format!("synthetic://album/{i}")),
            })
            .collect();
        let tracks: Vec<Track> = TRACKS
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let album = &albums[i / 4];
                Track {
                    id: SpotifyId(format!("previewtrack{i}")),
                    name: (*name).into(),
                    artists: album.artists.clone(),
                    album: Some(album.clone()),
                    duration_ms: 154_000 + (i as u64 * 37_000) % 140_000,
                    playable: true,
                    source_index: None,
                }
            })
            .collect();
        let playlists: Vec<Playlist> = PLAYLISTS
            .iter()
            .enumerate()
            .map(|(i, name)| Playlist {
                id: SpotifyId(format!("previewplaylist{i}")),
                name: (*name).into(),
                owner_id: "synthetic-preview".into(),
                snapshot_id: "fixture-v1".into(),
                track_count: Some(16 + i as u64),
                artwork_url: Some(format!("synthetic://playlist/{i}")),
            })
            .collect();
        // 30 distinct liked tracks in a scattered order (7 is coprime with 32).
        let liked: Vec<Track> = (0..30)
            .map(|i| tracks[(i * 7) % tracks.len()].clone())
            .collect();
        let saved_albums: Vec<Album> = [5, 1, 6, 3, 0].map(|i| albums[i].clone()).to_vec();
        Self {
            artists,
            albums,
            tracks,
            playlists,
            liked,
            saved_albums,
        }
    }
    /// A playlist is a rotated selection across every album.
    fn playlist_tracks(&self, id: &SpotifyId) -> Vec<Track> {
        let index: usize =
            id.0.trim_start_matches("previewplaylist")
                .parse()
                .unwrap_or(0);
        let count: usize = 16 + index;
        (0..count)
            .map(|i| self.tracks[(index * 5 + i * 7) % self.tracks.len()].clone())
            .collect()
    }
    fn album_tracks(&self, id: &SpotifyId) -> Vec<Track> {
        self.tracks
            .iter()
            .filter(|t| t.album.as_ref().is_some_and(|a| &a.id == id))
            .cloned()
            .collect()
    }
    fn artist_albums(&self, id: &SpotifyId) -> Vec<Album> {
        self.albums
            .iter()
            .filter(|a| a.artists.iter().any(|artist| &artist.id == id))
            .cloned()
            .collect()
    }
    /// Artists whose name matches, case-insensitively.
    fn search_artists(&self, text: &str) -> Vec<Artist> {
        let needle: String = text.to_lowercase();
        self.artists
            .iter()
            .filter(|a| a.name.to_lowercase().contains(&needle))
            .cloned()
            .collect()
    }
    /// Albums whose name or artist matches, case-insensitively.
    fn search_albums(&self, text: &str) -> Vec<Album> {
        let needle: String = text.to_lowercase();
        self.albums
            .iter()
            .filter(|a| {
                a.name.to_lowercase().contains(&needle)
                    || a.artists
                        .iter()
                        .any(|x| x.name.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect()
    }
    /// Tracks whose name or artist matches, case-insensitively.
    fn search_tracks(&self, text: &str) -> Vec<Track> {
        let needle: String = text.to_lowercase();
        self.tracks
            .iter()
            .filter(|t| {
                t.name.to_lowercase().contains(&needle)
                    || t.artists
                        .iter()
                        .any(|x| x.name.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect()
    }
    /// Mixed overview bounded like Spotify's development-mode search.
    fn search(&self, text: &str) -> SearchResults {
        fn top<T>(mut items: Vec<T>) -> Vec<T> {
            items.truncate(SEARCH_LIMIT);
            items
        }
        SearchResults {
            artists: top(self.search_artists(text)),
            albums: top(self.search_albums(text)),
            tracks: top(self.search_tracks(text)),
        }
    }
}

/// Single-type search pages, at most `SEARCH_LIMIT` items like production.
fn search_page<T: Clone>(items: &[T], request: PageRequest) -> Page<T> {
    page(items, search_request(request))
}

fn search_request(request: PageRequest) -> PageRequest {
    let limit: u16 = request.limit.min(SEARCH_LIMIT as u16);
    PageRequest { limit, ..request }
}

/// Convert hue (0..360) with fixed saturation/value into RGB.
fn hue(degrees: f32, saturation: f32, value: f32) -> [f32; 3] {
    let c: f32 = value * saturation;
    let h: f32 = (degrees.rem_euclid(360.0)) / 60.0;
    let x: f32 = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m: f32 = value - c;
    [r + m, g + m, b + m]
}

/// Draw a deterministic gradient with a simple shape per URL and encode it as PNG.
/// Artists get rings, albums a disc, playlists stripes, so card kinds are distinguishable.
fn synthetic_artwork(url: &str) -> Vec<u8> {
    use image::ImageEncoder;
    const EDGE: u32 = 300;
    let seed: u32 = url.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    let base: f32 = (seed % 360) as f32;
    let from: [f32; 3] = hue(base, 0.65, 0.85);
    let to: [f32; 3] = hue(base + 50.0 + (seed >> 9) as f32 % 80.0, 0.75, 0.35);
    let accent: [f32; 3] = hue(base + 180.0, 0.35, 0.95);
    let kind: &str = url.split('/').nth(2).unwrap_or("");
    let mut pixels: Vec<u8> = Vec::with_capacity((EDGE * EDGE * 3) as usize);
    for y in 0..EDGE {
        for x in 0..EDGE {
            let (fx, fy) = (x as f32 / EDGE as f32, y as f32 / EDGE as f32);
            let t: f32 = (fx * 0.6 + fy * 0.4).clamp(0.0, 1.0);
            let mut rgb: [f32; 3] = [0, 1, 2].map(|i| from[i] * (1.0 - t) + to[i] * t);
            let (dx, dy) = (fx - 0.62, fy - 0.42);
            let distance: f32 = (dx * dx + dy * dy).sqrt();
            let shape: f32 = match kind {
                "artist" => {
                    if ((distance * 22.0) as u32).is_multiple_of(2) && distance < 0.42 {
                        0.35
                    } else {
                        0.0
                    }
                }
                "album" => {
                    if distance < 0.26 {
                        0.55
                    } else if distance < 0.29 {
                        0.2
                    } else {
                        0.0
                    }
                }
                _ => {
                    if (((fx + fy * 0.5) * 9.0) as u32).is_multiple_of(3) {
                        0.25
                    } else {
                        0.0
                    }
                }
            };
            for i in 0..3 {
                rgb[i] = rgb[i] * (1.0 - shape) + accent[i] * shape;
                pixels.push((rgb[i] * 255.0) as u8);
            }
        }
    }
    let mut bytes: Vec<u8> = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&pixels, EDGE, EDGE, image::ExtendedColorType::Rgb8)
        .expect("in-memory PNG encoding");
    bytes
}

/// Serve clearly synthetic metadata and artwork entirely in memory, without desktop adapters.
async fn serve(mut port: BackendPort, mut state: AppState) {
    let catalog: Catalog = Catalog::new();
    let tracks: &[Track] = &catalog.tracks;
    state.player.current = Some(tracks[0].clone());
    state.player.current_entry_id = Some(1);
    state.player.position_ms = 67_000;
    let remote = std::env::var("CS_PREVIEW_REMOTE").as_deref() == Ok("1");
    state.player.status = if remote {
        PlaybackStatus::Playing
    } else {
        PlaybackStatus::Paused
    };
    state.connect = ConnectState {
        playback_auth: if std::env::var("CS_PREVIEW_PLAYBACK_AUTH").as_deref()
            == Ok("needs-authorization")
        {
            PlaybackAuth::NeedsAuthorization
        } else {
            PlaybackAuth::Ready
        },
        source: match std::env::var("CS_PREVIEW_SOURCE").as_deref() {
            Ok("polling") => RemoteSource::Polling,
            Ok("unavailable") => RemoteSource::Unavailable,
            _ => RemoteSource::Live,
        },
        this_device_id: Some("preview-local".into()),
        active_device_id: Some(
            if remote {
                "preview-kitchen"
            } else {
                "preview-local"
            }
            .into(),
        ),
        context_uri: Some("spotify:playlist:previewplaylist3".into()),
        queue_preview: tracks[1..13].to_vec(),
        devices: vec![
            PlaybackDevice {
                id: Some("preview-local".into()),
                name: "Custom Spotify (Preview)".into(),
                device_type: DeviceType::Computer,
                is_active: !remote,
                is_this_app: true,
                volume_percent: Some(65),
                supports_volume: true,
                is_restricted: false,
            },
            PlaybackDevice {
                id: Some("preview-kitchen".into()),
                name: "Kitchen speaker".into(),
                device_type: DeviceType::Speaker,
                is_active: remote,
                is_this_app: false,
                volume_percent: Some(42),
                supports_volume: true,
                is_restricted: false,
            },
            PlaybackDevice {
                id: None,
                name: "Living room TV".into(),
                device_type: DeviceType::Tv,
                is_active: false,
                is_this_app: false,
                volume_percent: None,
                supports_volume: false,
                is_restricted: true,
            },
            PlaybackDevice {
                id: Some("preview-phone".into()),
                name: "Phone".into(),
                device_type: DeviceType::Smartphone,
                is_active: false,
                is_this_app: false,
                volume_percent: Some(80),
                supports_volume: true,
                is_restricted: false,
            },
        ],
    };
    if std::env::var("CS_PREVIEW_NO_CONNECT").as_deref() == Ok("1") {
        state.connect.devices.retain(|device| !device.is_this_app);
        state.connect.ensure_local_device(
            "custom-spotify:local-player",
            "Custom Spotify (Preview)".into(),
            0.65,
        );
        if !remote {
            state.connect.active_device_id = state.connect.this_device_id.clone();
            for device in &mut state.connect.devices {
                device.is_active = device.is_this_app;
            }
        }
        state.connect.source = RemoteSource::Polling;
    }
    state.player.volume = if remote { 0.42 } else { 0.65 };
    port.state.send_replace(state.clone());
    if let Ok(message) = std::env::var("CS_PREVIEW_NOTICE") {
        let notice = AppError {
            kind: ErrorKind::Network,
            message,
            retry_after_seconds: None,
        };
        let _ = port.events.send(Event::Notice(notice)).await;
    }
    while let Some(command) = port.commands.recv().await {
        let response = match command.action {
            Action::Quit => {
                let _ = port.events.send(Event::QuitReady).await;
                break;
            }
            Action::Fetch {
                query,
                page: request,
            } => Response::Page(match query {
                Query::OwnPlaylists => PageData::Playlists(page(&catalog.playlists, request)),
                Query::PlaylistTracks { playlist_id } => {
                    PageData::Tracks(listing(&catalog.playlist_tracks(&playlist_id), request))
                }
                Query::AlbumTracks { album_id } => {
                    PageData::Tracks(listing(&catalog.album_tracks(&album_id), request))
                }
                Query::LikedTracks => PageData::Tracks(listing(&catalog.liked, request)),
                Query::SavedAlbums => PageData::Albums(page(&catalog.saved_albums, request)),
                Query::Search { text } => PageData::Search(catalog.search(&text)),
                Query::SearchArtists { text } => {
                    PageData::Artists(search_page(&catalog.search_artists(&text), request))
                }
                Query::SearchAlbums { text } => {
                    PageData::Albums(search_page(&catalog.search_albums(&text), request))
                }
                Query::SearchTracks { text } => PageData::Tracks(listing(
                    &catalog.search_tracks(&text),
                    search_request(request),
                )),
                Query::ArtistAlbums { artist_id } => {
                    PageData::Albums(page(&catalog.artist_albums(&artist_id), request))
                }
                Query::Queue { revision } => PageData::Queue {
                    revision,
                    page: page(
                        &tracks[..12]
                            .iter()
                            .enumerate()
                            .map(|(i, t)| QueueItem {
                                entry: QueueEntry {
                                    entry_id: i as u64 + 1,
                                    track_id: t.id.clone(),
                                },
                                track: Some(t.clone()),
                                explicit: i < 2,
                            })
                            .collect::<Vec<_>>(),
                        request,
                    ),
                },
            }),
            Action::Configure(settings) => {
                state.settings = settings;
                Response::Accepted
            }
            Action::Pause => {
                state.player.status = PlaybackStatus::Paused;
                Response::Accepted
            }
            Action::Resume | Action::Play(_) => {
                state.player.status = PlaybackStatus::Playing;
                Response::Accepted
            }
            Action::Seek { position_ms } => {
                state.player.position_ms = position_ms;
                Response::Accepted
            }
            Action::SetVolume(v) => {
                state.player.volume = v;
                if let Some(device) = state.connect.devices.iter_mut().find(|d| d.is_active) {
                    device.volume_percent = Some((v * 100.0).round() as u8);
                }
                Response::Accepted
            }
            Action::TransferPlayback { device_id, play } => {
                state.connect.active_device_id = Some(device_id.clone());
                for device in &mut state.connect.devices {
                    device.is_active = device.id.as_ref() == Some(&device_id);
                }
                state.player.status = if play {
                    PlaybackStatus::Playing
                } else {
                    PlaybackStatus::Paused
                };
                Response::Accepted
            }
            Action::SetDeviceVolume {
                device_id,
                volume_percent,
            } => {
                if let Some(device) = state
                    .connect
                    .devices
                    .iter_mut()
                    .find(|d| d.id.as_ref() == Some(&device_id))
                {
                    device.volume_percent = Some(volume_percent);
                    if device.is_active {
                        state.player.volume = f32::from(volume_percent) / 100.0;
                    }
                }
                Response::Accepted
            }
            Action::SetShuffle(v) => {
                state.player.shuffle = v;
                Response::Accepted
            }
            Action::SetRepeat(v) => {
                state.player.repeat = v;
                Response::Accepted
            }
            // Artwork is generated, never fetched; the fixture stays offline.
            Action::Artwork { url, .. } => Response::Artwork {
                bytes: synthetic_artwork(&url),
            },
            Action::CancelRequest(_) => continue,
            _ => Response::Accepted,
        };
        state.player.observed_at = std::time::Instant::now();
        port.state.send_replace(state.clone());
        if port
            .events
            .send(Event::Completed {
                id: command.id,
                result: Ok(response),
            })
            .await
            .is_err()
        {
            break;
        }
    }
}

/// Seed the isolated frontend preferences so the window opens on a chosen page, for
/// headless screenshots: home, playlist, album, artist, liked, saved-albums, search,
/// settings or queue. Search opens with the text in `CS_PREVIEW_QUERY`, or empty.
/// `CS_PREVIEW_DEVICES=1` opens the device picker and `CS_PREVIEW_SOURCE=polling|unavailable`
/// changes the Connect source. `CS_PREVIEW_WIDTH` sets the window width (default 1280) to check narrow layouts.
fn preview_route(route: &str) -> Result<(), Box<dyn std::error::Error>> {
    let query: String = std::env::var("CS_PREVIEW_QUERY").unwrap_or_default();
    let width: f32 = match std::env::var("CS_PREVIEW_WIDTH") {
        Ok(width) => width.parse()?,
        Err(_) => 1280.0,
    };
    let (kind, key, title, artwork) = match route {
        "home" | "queue" | "settings" => ("library", "", "Home", ""),
        "playlist" => (
            "playlist",
            "previewplaylist3",
            "Late Night Radio",
            "synthetic://playlist/3",
        ),
        "album" => (
            "album",
            "previewalbum0",
            "After the Last Tram",
            "synthetic://album/0",
        ),
        "artist" => (
            "artist",
            "previewartist0",
            "The Paper Satellites (synthetic)",
            "synthetic://artist/0",
        ),
        "liked" => ("liked", "", "Liked Songs", ""),
        "saved-albums" => ("saved-albums", "", "Saved albums", ""),
        "search" => ("search", query.as_str(), query.as_str(), ""),
        other => return Err(format!("Unknown CS_PREVIEW_ROUTE {other:?}").into()),
    };
    let preferences = serde_json::json!({
        "version": 1,
        "owner": "synthetic-preview",
        "kind": kind,
        "key": key,
        "title": title,
        "offset": 0,
        "width": width,
        "height": 800.0,
        "settings": route == "settings",
        "queue": route == "queue",
        "devices": std::env::var("CS_PREVIEW_DEVICES").as_deref() == Ok("1"),
        "artwork": (!artwork.is_empty()).then_some(artwork),
    });
    let dirs = directories::ProjectDirs::from("", "CustomSpotify", "CustomSpotify")
        .ok_or("No preview config directory")?;
    std::fs::create_dir_all(dirs.config_dir())?;
    std::fs::write(
        dirs.config_dir().join("frontend-v1.json"),
        serde_json::to_vec(&preferences)?,
    )?;
    Ok(())
}

/// Require caller-isolated XDG directories before opening the native preview window.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for variable in ["XDG_DATA_HOME", "XDG_CONFIG_HOME"] {
        let path = std::env::var_os(variable)
            .ok_or("Set temporary XDG_DATA_HOME and XDG_CONFIG_HOME for preview")?;
        if !std::path::Path::new(&path).is_absolute() {
            return Err("Preview XDG paths must be absolute".into());
        }
    }
    if let Ok(route) = std::env::var("CS_PREVIEW_ROUTE") {
        preview_route(&route)?;
    }
    let mut state = backend::initial_state();
    state.auth = AuthState::Ready {
        user_id: "synthetic-preview".into(),
    };
    state.settings.client_id = Some("SYNTHETIC-OFFLINE-PREVIEW".into());
    state.settings.theme_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let (front, back) = channels(state.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.spawn(serve(back, state));
    let _context = runtime.enter();
    frontend::run(front)?;
    Ok(())
}
