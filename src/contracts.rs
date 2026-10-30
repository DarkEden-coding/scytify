//! Bounded, asynchronous UI/backend boundary. No toolkit, OAuth tokens, or audio buffers.
use crate::domain::*;
use tokio::sync::{mpsc, watch};

pub const COMMAND_CAPACITY: usize = 64;
pub const EVENT_CAPACITY: usize = 32;
pub const MAX_PAGE_SIZE: u16 = 100;
/// Artwork requests the UI keeps in flight; the backend reserves the same separate budget.
pub const ARTWORK_IN_FLIGHT: usize = 6;
/// Spotify caps development-mode search pages at 10 results per type.
pub const SEARCH_LIMIT: usize = 10;
/// Items one `CheckSaved` request may ask about; the backend splits it into upstream calls.
pub const MAX_SAVED_CHECK: usize = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

/// Offset is app-local; backend maps current upstream pagination into this contract.
#[derive(Clone, Copy, Debug)]
pub struct PageRequest {
    pub offset: u64,
    pub limit: u16,
}

#[derive(Clone, Debug)]
pub struct Page<T> {
    pub offset: u64,
    pub total: Option<u64>,
    pub items: Vec<T>,
    pub has_more: bool,
    /// Upstream offset after the last raw row this page consumed. The backend drops rows
    /// (local files, episodes, malformed items), so this can exceed
    /// `offset + items.len()`; the following page starts here.
    pub next_offset: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    OwnPlaylists,
    PlaylistTracks {
        playlist_id: SpotifyId,
    },
    /// Mixed overview: one bounded, non-paged `PageData::Search`; offset is ignored.
    Search {
        text: String,
    },
    /// Paged single-type search ("see all"), at most `SEARCH_LIMIT` items per page.
    SearchArtists {
        text: String,
    },
    SearchAlbums {
        text: String,
    },
    SearchTracks {
        text: String,
    },
    SearchPlaylists {
        text: String,
    },
    /// The user's Liked Songs, newest first. Requires the `user-library-read` scope.
    LikedTracks,
    /// The user's saved albums, newest first. Requires the `user-library-read` scope.
    SavedAlbums,
    /// Artists followed by the user. Requires the `user-follow-read` scope.
    FollowedArtists,
    /// A bounded popular-song list, when Spotify grants this endpoint to the client.
    ArtistTopTracks {
        artist_id: SpotifyId,
    },
    ArtistAlbums {
        artist_id: SpotifyId,
    },
    AlbumTracks {
        album_id: SpotifyId,
    },
    Queue {
        revision: u64,
    },
    /// Spotify's personalized playlists (Discover Weekly, Release Radar, Daily Mixes), read
    /// from the home feed over the playback session. One non-paged `PageData::Playlists`.
    MadeForYou,
    /// Listening recorded on this device. One non-paged `PageData::Stats`; needs no sign-in.
    ListeningStats {
        range: StatsRange,
    },
}

/// How far back listening stats look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatsRange {
    Week,
    #[default]
    Month,
    AllTime,
}

impl StatsRange {
    pub const ALL: [StatsRange; 3] = [StatsRange::Week, StatsRange::Month, StatsRange::AllTime];

    /// Stable name for saved preferences.
    pub fn key(self) -> &'static str {
        match self {
            StatsRange::Week => "week",
            StatsRange::Month => "month",
            StatsRange::AllTime => "all",
        }
    }

    /// Days covered, or `None` for everything recorded.
    pub fn days(self) -> Option<i64> {
        match self {
            StatsRange::Week => Some(7),
            StatsRange::Month => Some(30),
            StatsRange::AllTime => None,
        }
    }
}

/// Listening totals and rankings over one `StatsRange`.
#[derive(Clone, Debug, Default)]
pub struct ListeningStats {
    pub range: StatsRange,
    pub listened_ms: u64,
    /// Listens that lasted long enough to count as a play.
    pub plays: u64,
    pub songs: u64,
    pub artists: u64,
    /// Consecutive days with listening, ending today or yesterday; independent of the range.
    pub streak_days: u64,
    pub longest_streak_days: u64,
    /// Listening time by local hour of day, midnight first.
    pub by_hour: [u64; 24],
    /// Most played songs, most first.
    pub top_tracks: Vec<RankedTrack>,
    /// Most listened artists by time, most first.
    pub top_artists: Vec<RankedArtist>,
}

#[derive(Clone, Debug)]
pub struct RankedTrack {
    pub track: Track,
    pub plays: u64,
    pub listened_ms: u64,
}

#[derive(Clone, Debug)]
pub struct RankedArtist {
    pub artist: Artist,
    pub plays: u64,
    pub listened_ms: u64,
}

#[derive(Clone, Debug)]
pub enum PageData {
    Playlists(Page<Playlist>),
    Tracks(Page<Track>),
    Artists(Page<Artist>),
    Albums(Page<Album>),
    Search(SearchResults),
    Queue {
        revision: u64,
        page: Page<QueueItem>,
    },
    Stats(Box<ListeningStats>),
}

/// Top matches per type for one search text, each at most `SEARCH_LIMIT` long.
#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
}

/// Something the user can keep in Your Library: a liked song, a saved album or a followed
/// playlist.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LibraryItem {
    Track(SpotifyId),
    Album(SpotifyId),
    Playlist(SpotifyId),
}

#[derive(Clone, Debug)]
pub enum PlaySource {
    /// Offset is the raw occurrence. Spirc loads the context directly; the local fallback
    /// fetches complete membership before atomically replacing the session.
    Playlist {
        playlist_id: SpotifyId,
        start_offset: u64,
    },
    Album {
        album_id: SpotifyId,
        start_offset: u64,
    },
    /// Liked Songs as one context, ordered as `Query::LikedTracks` returns them.
    LikedSongs {
        start_offset: u64,
    },
    Track(SpotifyId),
}

#[derive(Clone, Debug)]
pub enum Action {
    Configure(Settings),
    BeginLogin,
    /// One-time librespot consent, explicitly requested by the user.
    AuthorizePlayback,
    CancelLogin,
    Logout,
    /// Remove disposable metadata/art/audio without clearing credentials or session preferences.
    ClearCache,
    Fetch {
        query: Query,
        page: PageRequest,
    },
    CancelRequest(RequestId),
    Play(PlaySource),
    /// Endless recommendations, skipping liked songs and tracks heard before. Needs the
    /// playback session; the backend keeps the queue topped up while it plays.
    PlayDiscover,
    Resume,
    Pause,
    Next,
    Previous,
    Seek {
        position_ms: u64,
    },
    SetVolume(f32),
    TransferPlayback {
        device_id: String,
        play: bool,
    },
    SetDeviceVolume {
        device_id: String,
        volume_percent: u8,
    },
    RefreshDevices,
    SetShuffle(bool),
    SetRepeat(RepeatMode),
    /// FIFO within consecutive play-next insertions; ahead of AddToQueue and base sequence.
    PlayNext(Vec<SpotifyId>),
    AddToQueue(Vec<SpotifyId>),
    RemoveQueued {
        entry_id: u64,
    },
    /// Fetch/decode only bounded artwork sizes; never permit arbitrary filesystem paths.
    Artwork {
        url: String,
        edge_px: u16,
    },
    SetVisible(bool),
    /// Add an item to (`saved`) or remove it from Your Library. Needs `user-library-modify`.
    SetSaved {
        item: LibraryItem,
        saved: bool,
    },
    /// Add a song to (`added`) or remove every occurrence of it from a playlist the user
    /// can edit. Needs `playlist-modify-public` or `playlist-modify-private`.
    SetInPlaylist {
        playlist_id: SpotifyId,
        track_id: SpotifyId,
        added: bool,
    },
    /// Whether each item is in Your Library. Answers `Response::Saved` in the same order;
    /// at most `MAX_SAVED_CHECK` items.
    CheckSaved(Vec<LibraryItem>),
    /// Synced lyrics for one track. Answers `Response::Lyrics`, cached on disk.
    Lyrics {
        track_id: SpotifyId,
    },
    /// Store (Some) or forget (None) the Spicy Lyrics API key in the Secret Service.
    SetLyricsKey(Option<String>),
    Quit,
}

#[derive(Clone, Debug)]
pub struct Command {
    pub id: RequestId,
    pub action: Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Authentication,
    CredentialStore,
    Permission,
    RateLimited,
    Network,
    Unavailable,
    InvalidInput,
    StaleQueue,
    Internal,
}

/// Safe display text only; redact tokens, authorization headers and OAuth callback queries.
#[derive(Clone, Debug)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    pub retry_after_seconds: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum Response {
    Accepted,
    Page(PageData),
    /// Bounded encoded PNG/JPEG bytes, not a GPU texture; UI owns small decoded LRU.
    Artwork {
        bytes: Vec<u8>,
    },
    /// `None` when the provider has no lyrics for the track.
    Lyrics(Option<Lyrics>),
    /// One flag per item of a `CheckSaved` request.
    Saved(Vec<bool>),
}

#[derive(Clone, Debug)]
pub enum Event {
    Completed {
        id: RequestId,
        result: Result<Response, AppError>,
    },
    /// Tray and MPRIS Raise request the UI to create/focus its window.
    ShowWindow,
    /// Backend has checkpointed and stopped playback; UI may terminate the process.
    QuitReady,
    Notice(AppError),
}

/// UI sends commands without blocking its event loop; full channel is a visible busy result.
pub struct FrontendPort {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    pub state: watch::Receiver<AppState>,
}

/// One backend task owns mutable session state. Desktop adapters use a cloned command sender.
pub struct BackendPort {
    pub commands: mpsc::Receiver<Command>,
    pub events: mpsc::Sender<Event>,
    pub state: watch::Sender<AppState>,
    pub desktop_commands: mpsc::Sender<Command>,
}

/// Construct bounded channels with caller-provided initial state; starts no tasks or services.
pub fn channels(initial: AppState) -> (FrontendPort, BackendPort) {
    let (commands_tx, commands_rx) = mpsc::channel(COMMAND_CAPACITY);
    let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
    let (state_tx, state_rx) = watch::channel(initial);
    (
        FrontendPort {
            commands: commands_tx.clone(),
            events: events_rx,
            state: state_rx,
        },
        BackendPort {
            commands: commands_rx,
            events: events_tx,
            state: state_tx,
            desktop_commands: commands_tx,
        },
    )
}
