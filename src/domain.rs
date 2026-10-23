//! Owned metadata and serializable session data; never put authentication secrets here.
use serde::{Deserialize, Serialize};

/// Spotify base62 ID, validated by the backend at ingress; not an arbitrary URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpotifyId(pub String);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artist {
    pub id: SpotifyId,
    pub name: String,
    /// Card-sized artist image; simplified artists nested in tracks/albums have none.
    pub artwork_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Album {
    pub id: SpotifyId,
    pub name: String,
    pub artists: Vec<Artist>,
    /// Card-sized cover (smallest Spotify image at least 160px), shared by its tracks.
    pub artwork_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: SpotifyId,
    pub name: String,
    pub artists: Vec<Artist>,
    pub album: Option<Album>,
    pub duration_ms: u64,
    pub playable: bool,
    /// Raw position of this row in the listing it was loaded from (playlist, album, Liked
    /// Songs, search page), counting local files and episodes the backend skipped. Play
    /// offsets and page continuations must use it rather than the row's index. `None`
    /// for tracks that do not come from a listing page.
    #[serde(default)]
    pub source_index: Option<u64>,
}

impl Track {
    /// Album cover used for track-row thumbnails and the player.
    pub fn artwork_url(&self) -> Option<&str> {
        self.album.as_ref()?.artwork_url.as_deref()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub id: SpotifyId,
    pub name: String,
    pub owner_id: String,
    pub snapshot_id: String,
    pub track_count: Option<u64>,
    /// Card-sized playlist cover, often Spotify's generated mosaic.
    pub artwork_url: Option<String>,
}

/// A stable occurrence, not a track ID: duplicate playlist tracks remain distinguishable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueEntry {
    pub entry_id: u64,
    pub track_id: SpotifyId,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    #[default]
    Off,
    Queue,
    One,
}

/// Immutable playlist membership captured on Play, including duplicate occurrences.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaylistSnapshot {
    pub playlist_id: SpotifyId,
    pub snapshot_id: String,
    pub entries: Vec<QueueEntry>,
}

/// Versioned, atomic disk checkpoint. Restore always pauses, regardless of prior state.
/// Large membership is persisted here, never pushed wholesale into the UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub version: u32,
    pub source: Option<PlaylistSnapshot>,
    pub current: Option<QueueEntry>,
    pub remaining: Vec<QueueEntry>,
    pub history: Vec<QueueEntry>,
    pub explicit_queue: Vec<QueueEntry>,
    /// Prefix of explicit_queue occupied by PlayNext; preserves FIFO insertion order.
    pub play_next_count: usize,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub position_ms: u64,
    pub volume: f32,
    pub next_entry_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackStatus {
    Stopped,
    Loading,
    Paused,
    Playing,
}

/// Small latest-value state. Position is an anchor; UI extrapolates only while visible.
#[derive(Clone, Debug)]
pub struct PlayerState {
    pub status: PlaybackStatus,
    pub current: Option<Track>,
    pub current_entry_id: Option<u64>,
    pub position_ms: u64,
    pub observed_at: std::time::Instant,
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub queue_revision: u64,
    /// Decoder-confirmed seeks, distinct from ordinary position anchors for MPRIS.
    pub seek_revision: u64,
}

#[derive(Clone, Debug)]
pub enum AuthState {
    NeedsClientId,
    SignedOut,
    Authorizing,
    Ready { user_id: String },
    Blocked { reason: String },
}

/// Playback consent is separate from the user's developer Web API authorization.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PlaybackAuth {
    Ready,
    #[default]
    NeedsAuthorization,
    Authorizing,
    Failed(String),
}

/// Transport used to observe the account's active Spotify player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoteSource {
    Live,
    Polling,
    #[default]
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceType {
    Computer,
    Smartphone,
    Tablet,
    Speaker,
    Tv,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackDevice {
    /// Spotify may omit IDs for devices that cannot be targeted.
    pub id: Option<String>,
    pub name: String,
    pub device_type: DeviceType,
    pub is_active: bool,
    pub is_this_app: bool,
    pub volume_percent: Option<u8>,
    pub supports_volume: bool,
    pub is_restricted: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectState {
    pub source: RemoteSource,
    pub playback_auth: PlaybackAuth,
    pub this_device_id: Option<String>,
    pub active_device_id: Option<String>,
    pub devices: Vec<PlaybackDevice>,
    pub context_uri: Option<String>,
    /// Bounded next-track preview, not a complete editable queue.
    pub queue_preview: Vec<Track>,
}

impl ConnectState {
    /// Keeps the local decoder selectable even before Spotify registers its Connect device.
    pub fn ensure_local_device(&mut self, id: &str, name: String, volume: f32) {
        self.this_device_id = Some(id.into());
        let has_real = self
            .devices
            .iter()
            .any(|device| device.id.as_deref() == Some(id));
        let mut found = false;
        self.devices.retain_mut(|device| {
            if has_real && device.is_this_app && device.id.as_deref() != Some(id) {
                return false;
            }
            if device.is_this_app || device.id.as_deref() == Some(id) {
                if found {
                    return false;
                }
                found = true;
                device.id = Some(id.into());
                device.is_this_app = true;
                device.is_active = self.active_device_id.as_deref() == Some(id);
            }
            true
        });
        if !found {
            self.devices.push(PlaybackDevice {
                id: Some(id.into()),
                name,
                device_type: DeviceType::Computer,
                is_active: self.active_device_id.as_deref() == Some(id),
                is_this_app: true,
                volume_percent: Some((volume.clamp(0.0, 1.0) * 100.0).round() as u8),
                supports_volume: true,
                is_restricted: false,
            });
        }
    }

    pub fn is_remote(&self) -> bool {
        match self.active_device_id.as_ref() {
            Some(id) => Some(id) != self.this_device_id.as_ref(),
            None => self
                .devices
                .iter()
                .any(|device| device.is_active && !device.is_this_app),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppState {
    /// Persisted configuration lets the UI edit one field without resetting the others.
    pub settings: Settings,
    pub auth: AuthState,
    pub player: PlayerState,
    pub connect: ConnectState,
    pub cache_used_bytes: u64,
    pub cache_limit_bytes: u64,
    /// A Spicy Lyrics API key is stored; the key itself never leaves the backend.
    pub lyrics_key_set: bool,
}

/// Backend validates ranges; default total cache limit is 1_000_000_000 bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub client_id: Option<String>,
    pub cache_limit_bytes: u64,
    pub theme_path: Option<std::path::PathBuf>,
}

/// Hydrated visible queue row; unavailable metadata does not erase the occurrence.
#[derive(Clone, Debug)]
pub struct QueueItem {
    pub entry: QueueEntry,
    pub track: Option<Track>,
    pub explicit: bool,
}

/// Time-synced lyrics for one track, already normalized from the provider's format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lyrics {
    pub track_id: SpotifyId,
    pub sync: LyricsSync,
    pub lines: Vec<LyricsLine>,
    pub credit: LyricsCredit,
}

/// How precisely `LyricsLine` timings can be trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LyricsSync {
    /// Every syllable carries its own timing.
    Syllable,
    /// Lines are timed; each line's `lead` is one syllable spanning the whole line.
    Line,
    /// No timings: every time is zero and the UI shows plain text.
    Static,
}

/// One displayed line. Times are milliseconds from the start of the track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LyricsLine {
    pub start_ms: u64,
    pub end_ms: u64,
    /// A second singer's line, drawn on the opposite side.
    pub opposite: bool,
    pub lead: Vec<Syllable>,
    /// Background vocals; empty for line-synced and static lyrics.
    pub background: Vec<Syllable>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Syllable {
    /// Without surrounding whitespace.
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    /// The next syllable continues this word, so no space follows.
    pub joins_next: bool,
}

/// Attribution the provider's terms require on screen next to the lyrics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LyricsCredit {
    pub source: LyricsSource,
    /// Community syncs only.
    pub uploader: Option<Contributor>,
    pub maker: Option<Contributor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LyricsSource {
    /// A community sync uploaded to Spicy Lyrics.
    Community,
    AppleMusic,
    Spotify,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contributor {
    pub name: String,
    pub url: String,
}
