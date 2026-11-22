//! Versioned, bounded local browser preferences, separate from backend credentials.
use super::*;
use serde::{Deserialize, Serialize};

/// Largest window coordinate accepted from a preferences file.
const MAX_POSITION: f32 = 16384.0;
/// Largest preferences file read; the remembered list offsets take most of it.
const MAX_FILE_BYTES: usize = 65536;

/// Where a song list was left scrolled.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListOffset {
    /// "album", "playlist" or "liked", as for the route.
    kind: String,
    key: String,
    y: f32,
}
impl ListOffset {
    fn new(list: &Query, y: f32) -> Option<Self> {
        let (kind, key) = match list {
            Query::AlbumTracks { album_id } => ("album", album_id.0.clone()),
            Query::PlaylistTracks { playlist_id } => ("playlist", playlist_id.0.clone()),
            Query::LikedTracks => ("liked", String::new()),
            _ => return None,
        };
        Some(Self {
            kind: kind.into(),
            key,
            y,
        })
    }
    fn query(&self) -> Option<Query> {
        if !self.y.is_finite() || self.y < 0.0 || self.key.len() > 1024 {
            return None;
        }
        match self.kind.as_str() {
            "album" => Some(Query::AlbumTracks {
                album_id: SpotifyId(self.key.clone()),
            }),
            "playlist" => Some(Query::PlaylistTracks {
                playlist_id: SpotifyId(self.key.clone()),
            }),
            "liked" => Some(Query::LikedTracks),
            _ => None,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Preferences {
    version: u8,
    owner: Option<String>,
    kind: String,
    key: String,
    title: String,
    offset: u64,
    width: f32,
    height: f32,
    /// Settings shown over the route, and the queue panel; absent in older files.
    settings: bool,
    queue: bool,
    /// The lyrics panel is open (instead of the queue); absent in older files.
    lyrics: bool,
    /// Dragged panel widths; 0 (absent in older files) means the default.
    sidebar_width: f32,
    queue_width: f32,
    lyrics_width: f32,
    /// Open the device picker on start. Only the offline preview writes it; never saved.
    devices: bool,
    /// Header art of the route, so a restored artist page keeps its picture.
    artwork: Option<String>,
    /// Window to restore: the size is the layout size (before zoom), the position is in the
    /// window system's logical pixels and absent where the platform hides it (native Wayland).
    /// Absent in older files.
    x: Option<f32>,
    y: Option<f32>,
    maximized: bool,
    /// UI zoom; 0 (absent in older files) means 1.0.
    zoom: f32,
    /// Use the window system's title bar instead of the app's own.
    system_title_bar: bool,
    /// Song list scroll offsets of the owner, least recently left first; absent in older files.
    lists: Vec<ListOffset>,
}
impl Preferences {
    /// Resolve the platform-specific application configuration file.
    fn path() -> Result<std::path::PathBuf, String> {
        directories::ProjectDirs::from("", "Scytify", "Scytify")
            .map(|p| p.config_dir().join("frontend-v1.json"))
            .ok_or_else(|| "Cannot locate frontend preferences directory".into())
    }
    /// Read a small versioned file; corruption is reported rather than silently ignored.
    pub(super) fn load() -> Result<Self, String> {
        if cfg!(test) {
            return Ok(Self::default());
        }
        use std::io::Read;
        let file = match std::fs::File::open(Self::path()?) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("Frontend preferences: {e}")),
        };
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Self::parse(&bytes)
    }

    /// Validate the bytes of a preferences file and bring window fields into safe bounds.
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err("Frontend preferences exceed 64 KiB".into());
        }
        let mut p: Self = serde_json::from_slice(bytes)
            .map_err(|e| format!("Invalid frontend preferences: {e}"))?;
        if !matches!(
            p.kind.as_str(),
            "library"
                | "playlists"
                | "playlist"
                | "artist"
                | "artist-popular"
                | "album"
                | "search"
                | "liked"
                | "saved-albums"
                | "followed-artists"
                | "made-for-you"
                | "stats"
        ) || p.title.len() > 1024
            || p.key.len() > 1024
            || p.artwork.as_ref().is_some_and(|a| a.len() > 1024)
            || p.version != 1
            || !(860.0..=7680.0).contains(&p.width)
            || !(620.0..=4320.0).contains(&p.height)
        {
            return Err("Unsupported frontend preferences version or window size".into());
        }
        p.sanitize_window();
        Ok(p)
    }

    /// Keep the zoom in range and drop a position that could strand the window off-screen.
    fn sanitize_window(&mut self) {
        self.zoom = chrome::clamp_zoom(self.zoom);
        let (x, y) = match (self.x, self.y) {
            (Some(x), Some(y)) if x.is_finite() && y.is_finite() => (x, y),
            _ => {
                self.x = None;
                self.y = None;
                return;
            }
        };
        // At least 120 pixels of the window's width stay reachable; the top edge keeps the
        // title bar on screen.
        self.x = Some(x.clamp(120.0 - self.width * self.zoom, MAX_POSITION));
        self.y = Some(y.clamp(0.0, MAX_POSITION));
    }

    /// Restore only known browser destinations, never executable input.
    pub(super) fn apply(mut self, ui: &mut Ui) {
        let owner = match &ui.state.auth {
            AuthState::Ready { user_id } => Some(user_id),
            _ => None,
        };
        let same_owner: bool = owner.is_some() && owner == self.owner.as_ref();
        // Never restore a route from an unverified or different account. Window size is global.
        if !same_owner {
            self.kind = "library".into();
            self.title = "Home".into();
            self.offset = 0;
            self.settings = false;
            self.artwork = None;
        }
        ui.query = match self.kind.as_str() {
            "playlist" => Query::PlaylistTracks {
                playlist_id: SpotifyId(self.key),
            },
            "artist" => Query::ArtistAlbums {
                artist_id: SpotifyId(self.key),
            },
            "artist-popular" => Query::ArtistTopTracks {
                artist_id: SpotifyId(self.key),
            },
            "album" => Query::AlbumTracks {
                album_id: SpotifyId(self.key),
            },
            // Every search page restores to the overview of its text.
            "search" => {
                self.title = navigation::Route::search(&self.key).title;
                self.offset = 0;
                ui.search = self.key.clone();
                Query::Search { text: self.key }
            }
            "liked" => Query::LikedTracks,
            "saved-albums" => Query::SavedAlbums,
            "followed-artists" => Query::FollowedArtists,
            "made-for-you" => Query::MadeForYou,
            "stats" => Query::ListeningStats {
                range: StatsRange::ALL
                    .into_iter()
                    .find(|r| r.key() == self.key)
                    .unwrap_or_default(),
            },
            _ => Query::OwnPlaylists,
        };
        // "library" (also written by older versions) is Home; "playlists" its full grid.
        ui.home = self.kind == "library";
        if ui.home {
            self.title = "Home".into();
        }
        if !self.title.is_empty() {
            ui.title = self.title;
        }
        ui.offset = self.offset;
        ui.route_artwork = self.artwork;
        ui.queue = self.queue;
        ui.devices = self.devices;
        if self.settings && !ui.settings {
            ui.open_settings();
        }
        ui.settings = self.settings;
        ui.lyrics_open = self.lyrics && !self.queue;
        ui.list_scroll = navigation::ListScroll::default();
        if same_owner {
            let start: usize = self.lists.len().saturating_sub(navigation::MAX_LIST_OFFSETS);
            for offset in &self.lists[start..] {
                if let Some(list) = offset.query() {
                    ui.list_scroll.remember(&list, offset.y);
                }
            }
        }
    }

    /// Restore window geometry, zoom and title bar choice before the window opens.
    pub(super) fn apply_window(&self, ui: &mut Ui) {
        if self.width >= 860.0 {
            ui.window_size = iced::Size::new(self.width, self.height);
            ui.restore_size = ui.window_size;
        }
        ui.restore_position = self.x.zip(self.y).map(|(x, y)| iced::Point::new(x, y));
        ui.position = ui.restore_position;
        ui.maximized = self.maximized;
        ui.zoom = chrome::clamp_zoom(self.zoom);
        let d = splits::Splits::default();
        let stored = |width: f32, default: f32| if width == 0.0 { default } else { width };
        ui.splits = splits::Splits {
            sidebar: stored(self.sidebar_width, d.sidebar),
            queue: stored(self.queue_width, d.queue),
            lyrics: stored(self.lyrics_width, d.lyrics),
        }
        .sanitized();
        ui.system_title_bar = self.system_title_bar;
    }

    /// Atomically replace frontend-only preferences after navigation or a window change.
    pub(super) fn save(ui: &Ui) -> Result<(), String> {
        if cfg!(test) {
            return Ok(());
        }
        if !matches!(ui.state.auth, AuthState::Ready { .. }) {
            // Startup has not established the account yet; keep its saved route intact and
            // update only the window fields.
            return match &ui.pending_preferences {
                Some(saved) => {
                    let mut p: Self = saved.clone();
                    p.version = 1;
                    if p.kind.is_empty() {
                        p.kind = "library".into();
                        p.title = "Home".into();
                    }
                    p.write_window(ui);
                    p.write()
                }
                None => Ok(()),
            };
        }
        Self::of(ui).write()
    }

    /// The preferences of a signed-in UI: its route, panels, list offsets and window.
    pub(super) fn of(ui: &Ui) -> Self {
        let (kind, key) = match &ui.query {
            Query::PlaylistTracks { playlist_id } => ("playlist", playlist_id.0.clone()),
            Query::ArtistAlbums { artist_id } => ("artist", artist_id.0.clone()),
            Query::ArtistTopTracks { artist_id } => ("artist-popular", artist_id.0.clone()),
            Query::AlbumTracks { album_id } => ("album", album_id.0.clone()),
            Query::Search { text }
            | Query::SearchArtists { text }
            | Query::SearchAlbums { text }
            | Query::SearchTracks { text }
            | Query::SearchPlaylists { text } => ("search", text.clone()),
            Query::LikedTracks => ("liked", String::new()),
            Query::SavedAlbums => ("saved-albums", String::new()),
            Query::FollowedArtists => ("followed-artists", String::new()),
            Query::MadeForYou => ("made-for-you", String::new()),
            Query::ListeningStats { range } => ("stats", range.key().into()),
            Query::OwnPlaylists if !ui.home => ("playlists", String::new()),
            _ => ("library", String::new()),
        };
        let mut p = Self {
            version: 1,
            owner: match &ui.state.auth {
                AuthState::Ready { user_id } => Some(user_id.clone()),
                _ => None,
            },
            kind: kind.into(),
            key,
            title: ui.title.clone(),
            offset: ui.offset,
            settings: ui.settings,
            queue: ui.queue,
            lyrics: ui.lyrics_open,
            artwork: ui.route_artwork.clone(),
            lists: ui
                .current_list_scroll()
                .entries()
                .filter_map(|(list, y)| ListOffset::new(list, *y))
                .collect(),
            ..Self::default()
        };
        p.write_window(ui);
        p
    }

    /// Copy the window geometry, zoom and title bar choice from the UI.
    fn write_window(&mut self, ui: &Ui) {
        self.width = ui.restore_size.width;
        self.height = ui.restore_size.height;
        self.x = ui.restore_position.map(|p| p.x);
        self.y = ui.restore_position.map(|p| p.y);
        self.maximized = ui.maximized;
        self.zoom = ui.zoom;
        self.system_title_bar = ui.system_title_bar;
        self.sidebar_width = ui.splits.sidebar;
        self.queue_width = ui.splits.queue;
        self.lyrics_width = ui.splits.lyrics;
    }

    /// Write the file through a temporary sibling so a crash never leaves it half-written.
    fn write(&self) -> Result<(), String> {
        let path = Self::path()?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        std::fs::rename(tmp, path).map_err(|e| format!("Saving frontend preferences: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file written by a version without the window fields.
    const OLD_FILE: &str =
        r#"{"version":1,"kind":"library","title":"Home","width":1180,"height":800}"#;

    #[test]
    fn older_files_get_default_window_fields() {
        let p = Preferences::parse(OLD_FILE.as_bytes()).unwrap();
        assert_eq!((p.x, p.y, p.maximized, p.zoom), (None, None, false, 1.0));
        assert!(!p.system_title_bar);
    }

    #[test]
    fn window_fields_round_trip() {
        let mut p = Preferences::parse(OLD_FILE.as_bytes()).unwrap();
        (p.x, p.y, p.maximized, p.zoom, p.system_title_bar) =
            (Some(120.0), Some(48.0), true, 1.2, true);
        let back = Preferences::parse(&serde_json::to_vec(&p).unwrap()).unwrap();
        assert_eq!((back.x, back.y), (Some(120.0), Some(48.0)));
        assert!(back.maximized && back.system_title_bar);
        assert_eq!(back.zoom, 1.2);
        assert_eq!((back.width, back.height), (1180.0, 800.0));
    }

    #[test]
    fn unsafe_window_fields_are_clamped() {
        let far = OLD_FILE.replace('}', r#","x":90000,"y":-500,"zoom":7}"#);
        let p = Preferences::parse(far.as_bytes()).unwrap();
        assert_eq!((p.x, p.y, p.zoom), (Some(MAX_POSITION), Some(0.0), 1.5));
        let left = OLD_FILE.replace('}', r#","x":-90000,"y":10}"#);
        let p = Preferences::parse(left.as_bytes()).unwrap();
        assert_eq!(p.x, Some(120.0 - 1180.0));
        let lone = OLD_FILE.replace('}', r#","x":10}"#);
        assert_eq!(Preferences::parse(lone.as_bytes()).unwrap().x, None);
    }

    #[test]
    fn list_offsets_round_trip_and_drop_bad_entries() {
        let file = OLD_FILE.replace(
            '}',
            r#","lists":[{"kind":"album","key":"a","y":120},{"kind":"liked","key":"","y":40},{"kind":"artist","key":"b","y":1},{"kind":"playlist","key":"p","y":-5}]}"#,
        );
        let p = Preferences::parse(file.as_bytes()).unwrap();
        let back = Preferences::parse(&serde_json::to_vec(&p).unwrap()).unwrap();
        let kept: Vec<(Query, f32)> = back
            .lists
            .iter()
            .filter_map(|o| Some((o.query()?, o.y)))
            .collect();
        let album = Query::AlbumTracks {
            album_id: SpotifyId("a".into()),
        };
        assert_eq!(kept, vec![(album, 120.0), (Query::LikedTracks, 40.0)]);
    }

    #[test]
    fn preferences_schema_is_strict() {
        assert!(serde_json::from_str::<Preferences>(r#"{"version":1,"execute":"bad"}"#).is_err());
        assert!(serde_json::from_str::<Preferences>("invalid").is_err());
    }
}
