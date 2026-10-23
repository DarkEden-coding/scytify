use futures_util::{StreamExt, TryStreamExt};

use crate::{
    contracts::{
        AppError, ErrorKind, Page, PageData, PageRequest, PlaySource, Query, SEARCH_LIMIT,
        SearchResults,
    },
    domain::*,
};

/// Queue source identity of Liked Songs, which has no Spotify ID or snapshot.
const LIKED_SONGS_SOURCE: &str = "liked-songs";
/// Items per source membership page, the Web API maximum.
const SOURCE_PAGE: u64 = 50;
/// Source membership pages requested at once.
const SOURCE_PAGE_CONCURRENCY: usize = 6;

/// Small current Web API client; it never owns or logs credentials.
#[derive(Clone)]
pub(crate) struct Api {
    pub(super) http: reqwest::Client,
    pub(super) control_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    pub(super) playback_backoff: std::sync::Arc<std::sync::Mutex<Option<std::time::Instant>>>,
}

/// Complete, immutable source membership gathered before a queue transaction is committed.
pub(crate) struct LoadedSource {
    pub source_id: SpotifyId,
    pub snapshot_id: String,
    pub tracks: Vec<SpotifyId>,
    pub metadata: Vec<Track>,
    pub start: usize,
}
impl Api {
    /// Builds a rustls client with bounded redirect and response behavior.
    pub(crate) fn new() -> Result<Self, AppError> {
        let http = super::http_client()
            .redirect(reqwest::redirect::Policy::limited(3))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|_| err(ErrorKind::Internal, "HTTP client unavailable"))?;
        Ok(Self {
            http,
            control_lock: Default::default(),
            playback_backoff: Default::default(),
        })
    }
    /// Resolves the current user's stable Spotify ID for ownership filtering.
    pub(crate) async fn identity(&self, token: &str) -> Result<String, AppError> {
        let r = self
            .http
            .get("https://api.spotify.com/v1/me")
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| err(ErrorKind::Network, "Spotify is unreachable"))?;
        if !r.status().is_success() {
            return Err(err(
                ErrorKind::Authentication,
                "could not verify Spotify identity",
            ));
        }
        let v: serde_json::Value = r
            .json()
            .await
            .map_err(|_| err(ErrorKind::Network, "Spotify returned invalid identity data"))?;
        v.get("id")
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| err(ErrorKind::Network, "Spotify identity omitted its ID"))
    }
    /// Loads every playlist/album/Liked Songs occurrence, preserving raw source offsets while
    /// filtering episodes/local/unavailable rows only after validating the start occurrence.
    pub(crate) async fn load_source(
        &self,
        token: &str,
        source: &PlaySource,
    ) -> Result<LoadedSource, AppError> {
        self.load_source_inner(token, source, None).await
    }

    pub(super) async fn load_transfer_source(
        &self,
        token: &str,
        source: &PlaySource,
        track: &Track,
    ) -> Result<LoadedSource, AppError> {
        self.load_source_inner(token, source, Some(track)).await
    }

    async fn load_source_inner(
        &self,
        token: &str,
        source: &PlaySource,
        selected: Option<&Track>,
    ) -> Result<LoadedSource, AppError> {
        let (path, offset, source_id) = match source {
            PlaySource::Playlist {
                playlist_id,
                start_offset,
            } => (
                format!("playlists/{}/items", valid_id(playlist_id)?),
                *start_offset,
                playlist_id.clone(),
            ),
            PlaySource::Album {
                album_id,
                start_offset,
            } => (
                format!("albums/{}/tracks", valid_id(album_id)?),
                *start_offset,
                album_id.clone(),
            ),
            PlaySource::LikedSongs { start_offset } => (
                "me/tracks".to_owned(),
                *start_offset,
                SpotifyId(LIKED_SONGS_SOURCE.into()),
            ),
            PlaySource::Track(_) => {
                return Err(err(ErrorKind::InvalidInput, "track is not a collection"));
            }
        };
        let library = matches!(source, PlaySource::LikedSongs { .. });
        let path = path.as_str();
        let page = move |raw: u64| async move {
            self.get_json(token, &format!("{path}?offset={raw}&limit={SOURCE_PAGE}"))
                .await
                .map_err(|e| if library { library_scope(e) } else { e })
        };
        let snapshot = async {
            match source {
                PlaySource::Playlist { playlist_id, .. } => {
                    self.playlist_snapshot(token, playlist_id).await
                }
                PlaySource::Album { album_id, .. } => Ok(format!("album:{}", album_id.0)),
                _ => Ok(LIKED_SONGS_SOURCE.into()),
            }
        };
        // The first page reports the total, so the remaining pages are fetched concurrently.
        let (first, snapshot_id) = tokio::try_join!(page(0), snapshot)?;
        let total = first.get("total").and_then(|x| x.as_u64());
        let rest: Vec<serde_json::Value> = futures_util::stream::iter(
            (SOURCE_PAGE..total.unwrap_or(0)).step_by(SOURCE_PAGE as usize),
        )
        .map(page)
        .buffered(SOURCE_PAGE_CONCURRENCY)
        .try_collect()
        .await?;
        let mut raw_index = 0u64;
        let mut playable_before = 0usize;
        let mut selected_playable = false;
        let mut tracks = Vec::new();
        let mut metadata = Vec::new();
        for (page_offset, mut v) in (0..)
            .step_by(SOURCE_PAGE as usize)
            .zip(std::iter::once(first).chain(rest))
        {
            // Offsets are only meaningful while membership is stable (likes have no snapshot).
            if v.get("total").and_then(|x| x.as_u64()) != total {
                return Err(err(
                    ErrorKind::StaleQueue,
                    "source changed while it was loading; retry",
                ));
            }
            let items = match v.get_mut("items").map(serde_json::Value::take) {
                Some(serde_json::Value::Array(items)) => items,
                _ => return Err(err(ErrorKind::Network, "Spotify source omitted items")),
            };
            if raw_index != page_offset || items.is_empty() && raw_index < total.unwrap_or(0) {
                return Err(err(
                    ErrorKind::Network,
                    "Spotify pagination did not advance",
                ));
            }
            for item in &items {
                let track = parse_track(unwrap_item(item, Kind::Tracks));
                if raw_index < offset && track.as_ref().is_some_and(|t| t.playable) {
                    playable_before += 1;
                }
                if raw_index == offset {
                    selected_playable = track.as_ref().is_some_and(|t| t.playable);
                }
                if let Some(mut t) = track.filter(|t| t.playable) {
                    t.source_index = Some(raw_index);
                    tracks.push(t.id.clone());
                    metadata.push(t);
                }
                raw_index += 1;
            }
        }
        if let PlaySource::Playlist { playlist_id, .. } = source
            && self.playlist_snapshot(token, playlist_id).await? != snapshot_id
        {
            return Err(err(
                ErrorKind::StaleQueue,
                "playlist changed while it was loading; retry",
            ));
        }
        if offset >= raw_index {
            return Err(err(
                ErrorKind::InvalidInput,
                "start offset is outside source",
            ));
        }
        if let PlaySource::Album { album_id, .. } = source
            && let Ok(album) = self.album(token, album_id).await
        {
            attach_album(&mut metadata, &album);
        }
        if let Some(selected) = selected {
            playable_before =
                super::transfer::source_start(&metadata, selected).ok_or_else(|| {
                    err(
                        ErrorKind::Unavailable,
                        "current track is absent from its context",
                    )
                })?;
        } else if !selected_playable {
            return Err(err(
                ErrorKind::Unavailable,
                "the selected source occurrence is unavailable",
            ));
        }
        Ok(LoadedSource {
            source_id,
            snapshot_id,
            tracks,
            metadata,
            start: playable_before,
        })
    }

    /// Up to 100 playable Liked Songs starting at the selected occurrence, which must be playable.
    pub(super) async fn liked_window(
        &self,
        token: &str,
        start: u64,
    ) -> Result<Vec<SpotifyId>, AppError> {
        let page = |offset: u64| async move {
            self.get_json(
                token,
                &format!("me/tracks?offset={offset}&limit={SOURCE_PAGE}"),
            )
            .await
            .map_err(library_scope)
        };
        let (first, second) = tokio::try_join!(page(start), page(start + SOURCE_PAGE))?;
        let tracks = [first, second]
            .iter()
            .filter_map(|v| v.get("items").and_then(|x| x.as_array()))
            .flatten()
            .map(|item| parse_track(unwrap_item(item, Kind::Tracks)))
            .collect::<Vec<_>>();
        match tracks.first() {
            None => Err(err(
                ErrorKind::InvalidInput,
                "start offset is outside source",
            )),
            Some(Some(t)) if t.playable => Ok(tracks
                .into_iter()
                .flatten()
                .filter(|t| t.playable)
                .take(100)
                .map(|t| t.id)
                .collect()),
            Some(_) => Err(err(
                ErrorKind::Unavailable,
                "the selected source occurrence is unavailable",
            )),
        }
    }

    /// Current playlist revision, compared before and after gathering its membership.
    async fn playlist_snapshot(&self, token: &str, id: &SpotifyId) -> Result<String, AppError> {
        let v = self
            .get_json(
                token,
                &format!("playlists/{}?fields=snapshot_id", valid_id(id)?),
            )
            .await?;
        Ok(v.get("snapshot_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned())
    }

    /// Hydrates track IDs for visible queue rows without changing occurrence membership.
    pub(crate) async fn hydrate_tracks(
        &self,
        token: &str,
        ids: &[SpotifyId],
    ) -> Result<Vec<Track>, AppError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // Development-mode apps lost the batch `tracks?ids=` endpoint in February 2026, so
        // tracks are fetched one at a time, a few concurrently.
        let paths = ids
            .iter()
            .map(|id| valid_id(id).map(|id| format!("tracks/{id}")))
            .collect::<Result<Vec<_>, _>>()?;
        let results: Vec<Result<serde_json::Value, AppError>> = futures_util::stream::iter(paths)
            .map(|path| async move { self.get_json(token, &path).await })
            .buffered(SOURCE_PAGE_CONCURRENCY)
            .collect()
            .await;
        // Like the batch endpoint, one missing track does not fail the others.
        let mut tracks = Vec::new();
        let mut error = None;
        for result in results {
            match result {
                Ok(v) => tracks.extend(parse_track(&v)),
                Err(e) => error = error.or(Some(e)),
            }
        }
        match error {
            Some(e) if tracks.is_empty() => Err(e),
            _ => Ok(tracks),
        }
    }

    async fn get_json(&self, token: &str, path: &str) -> Result<serde_json::Value, AppError> {
        let r = self
            .http
            .get(format!("https://api.spotify.com/v1/{path}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| err(ErrorKind::Network, "Spotify is unreachable"))?;
        response_json(r).await
    }

    /// Fetches one contract page from official Spotify v1 endpoints. Spotify serves at
    /// most 50 items per request, and 10 per search type in development mode, so a larger
    /// page is fetched as concurrent upstream pages and joined: the frontend relies on a
    /// page covering its whole requested range to put it in front of the loaded run.
    pub(crate) async fn fetch(
        &self,
        token: &str,
        user_id: &str,
        q: &Query,
        p: PageRequest,
    ) -> Result<PageData, AppError> {
        if p.limit == 0 || p.limit > crate::contracts::MAX_PAGE_SIZE {
            return Err(err(ErrorKind::InvalidInput, "invalid page size"));
        }
        let step: u16 = if search_root(q).is_some() {
            SEARCH_LIMIT as u16
        } else {
            SOURCE_PAGE as u16
        };
        let chunks = (0..p.limit)
            .step_by(usize::from(step))
            .map(|start| PageRequest {
                offset: p.offset + u64::from(start),
                limit: step.min(p.limit - start),
            });
        let mut pages = futures_util::stream::iter(chunks)
            .map(|chunk| self.fetch_upstream(token, user_id, q, chunk))
            .buffered(SOURCE_PAGE_CONCURRENCY);
        let mut data: PageData = match pages.next().await {
            Some(first) => first?,
            None => return Err(err(ErrorKind::InvalidInput, "invalid page size")),
        };
        while page_continues(&data)
            && let Some(next) = pages.next().await
        {
            if !join_page(&mut data, next?) {
                break;
            }
        }
        if let (Query::AlbumTracks { album_id }, PageData::Tracks(page)) = (q, &mut data) {
            // Cosmetic: a failed album lookup still returns the playable track page.
            if let Ok(album) = self.album(token, album_id).await {
                attach_album(&mut page.items, &album);
            }
        }
        Ok(data)
    }
    /// Fetches one upstream page of at most one request's size.
    async fn fetch_upstream(
        &self,
        token: &str,
        user_id: &str,
        q: &Query,
        p: PageRequest,
    ) -> Result<PageData, AppError> {
        let page_limit = p.limit.min(SOURCE_PAGE as u16);
        let search_limit = p.limit.min(SEARCH_LIMIT as u16);
        let (path, kind, limit) = match q {
            Query::OwnPlaylists => ("me/playlists".into(), Kind::Playlists, page_limit),
            Query::PlaylistTracks { playlist_id } => (
                format!("playlists/{}/items", valid_id(playlist_id)?),
                Kind::Tracks,
                page_limit,
            ),
            Query::LikedTracks => ("me/tracks".into(), Kind::Tracks, page_limit),
            Query::SavedAlbums => ("me/albums".into(), Kind::Albums, page_limit),
            Query::Search { text } => {
                let path = search_path(text, "artist,album,track")?;
                let v = self
                    .get_json(token, &format!("{path}&limit={SEARCH_LIMIT}"))
                    .await?;
                return Ok(PageData::Search(parse_search(&v)));
            }
            Query::SearchArtists { text } => {
                (search_path(text, "artist")?, Kind::Artists, search_limit)
            }
            Query::SearchAlbums { text } => {
                (search_path(text, "album")?, Kind::Albums, search_limit)
            }
            Query::SearchTracks { text } => {
                (search_path(text, "track")?, Kind::Tracks, search_limit)
            }
            Query::ArtistAlbums { artist_id } => (
                format!("artists/{}/albums", valid_id(artist_id)?),
                Kind::Albums,
                page_limit,
            ),
            Query::AlbumTracks { album_id } => (
                format!("albums/{}/tracks", valid_id(album_id)?),
                Kind::Tracks,
                page_limit,
            ),
            Query::Queue { .. } => return Err(err(ErrorKind::InvalidInput, "queue is local")),
        };
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut v = self
            .get_json(
                token,
                &format!("{path}{sep}offset={}&limit={limit}", p.offset),
            )
            .await
            .map_err(|e| {
                if matches!(q, Query::LikedTracks | Query::SavedAlbums) {
                    library_scope(e)
                } else {
                    e
                }
            })?;
        if let Some(key) = search_root(q) {
            v = v
                .get_mut(key)
                .map(serde_json::Value::take)
                .unwrap_or_default();
        }
        map_page(kind, v, p.offset, user_id)
    }
    /// Loads album name, artists and cover for simplified album-track pages.
    async fn album(&self, token: &str, id: &SpotifyId) -> Result<Album, AppError> {
        let v = self
            .get_json(token, &format!("albums/{}", valid_id(id)?))
            .await?;
        parse_album(&v).ok_or_else(|| err(ErrorKind::Network, "Spotify returned invalid album"))
    }
}
/// Maps shared Spotify HTTP status behavior and decodes one JSON body.
pub(super) async fn response_json(r: reqwest::Response) -> Result<serde_json::Value, AppError> {
    if r.status().as_u16() == 429 {
        let retry = r
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        return Err(AppError {
            kind: ErrorKind::RateLimited,
            message: "Spotify rate limit reached".into(),
            retry_after_seconds: retry,
        });
    }
    match r.status().as_u16() {
        401 => return Err(err(ErrorKind::Authentication, "authorization expired")),
        403 => {
            return Err(err(
                ErrorKind::Permission,
                "Spotify denied this endpoint for this account or app mode",
            ));
        }
        _ if !r.status().is_success() => {
            return Err(err(ErrorKind::Network, "Spotify API request failed"));
        }
        _ => {}
    }
    if r.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(serde_json::Value::Null);
    }
    r.json()
        .await
        .map_err(|_| err(ErrorKind::Network, "Spotify returned invalid data"))
}

/// Validates an ingress Spotify base62 identifier.
pub(super) fn valid_id(id: &SpotifyId) -> Result<&str, AppError> {
    if (16..=32).contains(&id.0.len()) && id.0.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(&id.0)
    } else {
        Err(err(ErrorKind::InvalidInput, "invalid Spotify ID"))
    }
}
/// Item type of one upstream page.
#[derive(Clone, Copy)]
enum Kind {
    Playlists,
    Tracks,
    Artists,
    Albums,
}

/// Library and playlist rows wrap their object (`{added_at, track}`); catalog pages do not.
fn unwrap_item(item: &serde_json::Value, kind: Kind) -> &serde_json::Value {
    let wrapped = match kind {
        Kind::Tracks => item.get("item").or_else(|| item.get("track")),
        Kind::Albums => item.get("album"),
        Kind::Playlists | Kind::Artists => None,
    };
    wrapped.unwrap_or(item)
}

/// Search responses nest each type's page under its plural name.
fn search_root(q: &Query) -> Option<&'static str> {
    match q {
        Query::SearchArtists { .. } => Some("artists"),
        Query::SearchAlbums { .. } => Some("albums"),
        Query::SearchTracks { .. } => Some("tracks"),
        _ => None,
    }
}

/// Builds a search path for nonempty text and comma-separated Spotify types.
fn search_path(text: &str, types: &str) -> Result<String, AppError> {
    if text.trim().is_empty() {
        return Err(err(ErrorKind::InvalidInput, "search is empty"));
    }
    let q: String = url::form_urlencoded::byte_serialize(text.as_bytes()).collect();
    Ok(format!("search?q={q}&type={types}"))
}

/// Whether a joined page was full and has more rows after it.
fn page_continues(data: &PageData) -> bool {
    match data {
        PageData::Playlists(p) => p.has_more,
        PageData::Tracks(p) => p.has_more,
        PageData::Artists(p) => p.has_more,
        PageData::Albums(p) => p.has_more,
        PageData::Queue { .. } | PageData::Search(_) => false,
    }
}

/// Append an upstream page that continues `data` at its raw offset; false otherwise.
fn join_page(data: &mut PageData, next: PageData) -> bool {
    fn extend<T>(a: &mut Page<T>, b: Page<T>) -> bool {
        if b.offset != a.next_offset {
            return false;
        }
        a.items.extend(b.items);
        a.has_more = b.has_more;
        a.total = b.total.or(a.total);
        a.next_offset = b.next_offset;
        true
    }
    match (data, next) {
        (PageData::Playlists(a), PageData::Playlists(b)) => extend(a, b),
        (PageData::Tracks(a), PageData::Tracks(b)) => extend(a, b),
        (PageData::Artists(a), PageData::Artists(b)) => extend(a, b),
        (PageData::Albums(a), PageData::Albums(b)) => extend(a, b),
        _ => false,
    }
}

/// Converts endpoint-specific JSON into contract page data.
fn map_page(
    kind: Kind,
    v: serde_json::Value,
    offset: u64,
    user: &str,
) -> Result<PageData, AppError> {
    let total = v.get("total").and_then(|x| x.as_u64());
    let has_more = v.get("next").and_then(|x| x.as_str()).is_some();
    let raw = v
        .get("items")
        .and_then(|x| x.as_array())
        .ok_or_else(|| err(ErrorKind::Network, "Spotify page omitted items"))?;
    // Filtered pages still continue after every raw row they consumed.
    let next_offset: u64 = offset + raw.len() as u64;
    macro_rules! page {
        ($items:expr) => {
            Page {
                offset,
                total,
                items: $items,
                has_more,
                next_offset,
            }
        };
    }
    let items = raw.iter().map(|x| unwrap_item(x, kind));
    Ok(match kind {
        Kind::Playlists => PageData::Playlists(page!(
            items
                .filter_map(parse_playlist)
                .filter(|x| x.owner_id == user)
                .collect()
        )),
        // Skipped rows (local files, episodes) keep their raw position on the survivors.
        Kind::Tracks => PageData::Tracks(page!(
            items
                .enumerate()
                .filter_map(|(i, x)| {
                    let mut track = parse_track(x)?;
                    track.source_index = Some(offset + i as u64);
                    Some(track)
                })
                .collect()
        )),
        Kind::Artists => PageData::Artists(page!(items.filter_map(parse_artist).collect())),
        Kind::Albums => PageData::Albums(page!(items.filter_map(parse_album).collect())),
    })
}

/// Parses the mixed search overview; Spotify may return null entries, which are skipped.
fn parse_search(v: &serde_json::Value) -> SearchResults {
    fn top<T>(
        v: &serde_json::Value,
        key: &str,
        parse: fn(&serde_json::Value) -> Option<T>,
    ) -> Vec<T> {
        v.pointer(&format!("/{key}/items"))
            .and_then(|x| x.as_array())
            .map(|xs| xs.iter().filter_map(parse).take(SEARCH_LIMIT).collect())
            .unwrap_or_default()
    }
    SearchResults {
        artists: top(v, "artists", parse_artist),
        albums: top(v, "albums", parse_album),
        tracks: top(v, "tracks", parse_track),
    }
}

/// Library reads fail with 403 when a token predates the `user-library-read` scope.
fn library_scope(e: AppError) -> AppError {
    if e.kind == ErrorKind::Permission {
        err(
            ErrorKind::Permission,
            "Liked Songs and saved albums need new Spotify permission; sign out and sign in again",
        )
    } else {
        e
    }
}

/// Reads a nonempty Spotify identifier from JSON.
fn sid(v: &serde_json::Value) -> Option<SpotifyId> {
    v.as_str()
        .filter(|x| !x.is_empty())
        .map(|x| SpotifyId(x.into()))
}
/// Parses artist metadata from Spotify JSON.
fn parse_artist(v: &serde_json::Value) -> Option<Artist> {
    Some(Artist {
        id: sid(v.get("id")?)?,
        name: v.get("name")?.as_str()?.into(),
        artwork_url: card_image(v.get("images")),
    })
}
/// Parses album metadata from Spotify JSON.
fn parse_album(v: &serde_json::Value) -> Option<Album> {
    Some(Album {
        id: sid(v.get("id")?)?,
        name: v.get("name")?.as_str()?.into(),
        artists: v
            .get("artists")
            .and_then(|x| x.as_array())
            .map(|x| x.iter().filter_map(parse_artist).collect())
            .unwrap_or_default(),
        artwork_url: card_image(v.get("images")),
    })
}
/// Parses supported non-local track metadata from Spotify JSON.
pub(super) fn parse_track(v: &serde_json::Value) -> Option<Track> {
    if v.get("type").and_then(|x| x.as_str()) == Some("episode")
        || v.get("is_local").and_then(|x| x.as_bool()) == Some(true)
    {
        return None;
    }
    Some(Track {
        id: sid(v.get("id")?)?,
        name: v.get("name")?.as_str()?.into(),
        artists: v
            .get("artists")
            .and_then(|x| x.as_array())
            .map(|x| x.iter().filter_map(parse_artist).collect())
            .unwrap_or_default(),
        album: v.get("album").and_then(parse_album),
        duration_ms: v.get("duration_ms").and_then(|x| x.as_u64()).unwrap_or(0),
        playable: v
            .get("is_playable")
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
        source_index: None,
    })
}
/// Parses playlist metadata from Spotify JSON.
fn parse_playlist(v: &serde_json::Value) -> Option<Playlist> {
    Some(Playlist {
        id: sid(v.get("id")?)?,
        name: v.get("name")?.as_str()?.into(),
        owner_id: v.pointer("/owner/id")?.as_str()?.into(),
        snapshot_id: v
            .get("snapshot_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .into(),
        track_count: v
            .pointer("/items/total")
            .or_else(|| v.pointer("/tracks/total"))
            .and_then(|x| x.as_u64()),
        artwork_url: card_image(v.get("images")),
    })
}
/// Smallest edge Spotify image that still fills a grid card without upscaling.
const CARD_IMAGE_PX: u64 = 160;
/// Picks the smallest image whose shorter known edge is at least 160px, else the largest.
/// Images without dimensions (common for playlist mosaics) are used only when nothing is sized.
fn card_image(images: Option<&serde_json::Value>) -> Option<String> {
    let images = images?.as_array()?;
    let mut sized = Vec::new();
    let mut unsized_url = None;
    for image in images {
        let Some(url) = image
            .get("url")
            .and_then(|x| x.as_str())
            .filter(|u| !u.is_empty())
        else {
            continue;
        };
        let edge = ["width", "height"]
            .iter()
            .filter_map(|k| image.get(*k).and_then(|x| x.as_u64()))
            .min();
        match edge {
            Some(edge) => sized.push((edge, url)),
            None => unsized_url = unsized_url.or(Some(url)),
        }
    }
    let large_enough = sized.iter().filter(|(e, _)| *e >= CARD_IMAGE_PX).min();
    large_enough
        .or_else(|| sized.iter().max())
        .map(|(_, url)| *url)
        .or(unsized_url)
        .map(str::to_owned)
}
/// Simplified album-track objects omit their album; attach the opened album's metadata.
fn attach_album(tracks: &mut [Track], album: &Album) {
    for track in tracks.iter_mut().filter(|t| t.album.is_none()) {
        track.album = Some(album.clone());
    }
}
/// Constructs a sanitized API-facing error.
pub(crate) fn err(kind: ErrorKind, msg: &str) -> AppError {
    AppError {
        kind,
        message: msg.into(),
        retry_after_seconds: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn card_image_prefers_smallest_at_least_160px() {
        let images = json!([
            {"url": "big", "width": 640, "height": 640},
            {"url": "card", "width": 300, "height": 300},
            {"url": "tiny", "width": 64, "height": 64}
        ]);
        assert_eq!(card_image(Some(&images)).as_deref(), Some("card"));
        let small = json!([{"url": "a", "width": 64, "height": 64}, {"url": "b", "width": 120, "height": 120}]);
        assert_eq!(card_image(Some(&small)).as_deref(), Some("b"));
        let unsized_mosaic = json!([{"url": "mosaic", "width": null, "height": null}]);
        assert_eq!(card_image(Some(&unsized_mosaic)).as_deref(), Some("mosaic"));
        assert_eq!(card_image(Some(&json!(null))), None);
        assert_eq!(card_image(None), None);
    }

    #[test]
    fn pages_carry_playlist_artist_and_album_artwork() {
        let playlists = json!({"items": [{
            "id": "p1", "name": "Mine", "owner": {"id": "me"}, "snapshot_id": "s",
            "items": {"total": 3},
            "images": [{"url": "pl-640", "width": 640, "height": 640}, {"url": "pl-300", "width": 300, "height": 300}]
        }]});
        let PageData::Playlists(p) = map_page(Kind::Playlists, playlists, 0, "me").unwrap() else {
            panic!("expected playlists")
        };
        assert_eq!(p.items[0].artwork_url.as_deref(), Some("pl-300"));
        let artists = json!({"artists": {"items": [{
            "id": "a1", "name": "Someone",
            "images": [{"url": "ar-320", "width": 320, "height": 320}, {"url": "ar-160", "width": 160, "height": 160}]
        }]}});
        let PageData::Artists(p) =
            map_page(Kind::Artists, artists["artists"].clone(), 0, "me").unwrap()
        else {
            panic!("expected artists")
        };
        assert_eq!(p.items[0].artwork_url.as_deref(), Some("ar-160"));
        let tracks = json!({"items": [{"track": {
            "id": "t1", "name": "Song", "duration_ms": 1,
            "album": {"id": "al1", "name": "Record", "images": [{"url": "al-300", "width": 300, "height": 300}]}
        }}]});
        let PageData::Tracks(p) = map_page(Kind::Tracks, tracks, 0, "me").unwrap() else {
            panic!("expected tracks")
        };
        assert_eq!(p.items[0].artwork_url(), Some("al-300"));
    }

    /// Regression: a skipped local file must not shift the play offsets of later rows.
    #[test]
    fn track_rows_keep_raw_positions_past_skipped_items() {
        let page = json!({"items": [
            {"track": {"id": "t0", "name": "Before"}},
            {"track": {"id": "local", "name": "Local file", "is_local": true}},
            {"track": {"type": "episode", "id": "e1", "name": "Episode"}},
            {"track": {"id": "t3", "name": "After"}}
        ]});
        let PageData::Tracks(p) = map_page(Kind::Tracks, page, 100, "me").unwrap() else {
            panic!("expected tracks")
        };
        let positions: Vec<_> = p
            .items
            .iter()
            .map(|t| (t.id.0.as_str(), t.source_index))
            .collect();
        assert_eq!(positions, [("t0", Some(100)), ("t3", Some(103))]);
        assert_eq!(p.next_offset, 104);
    }

    /// Regression: a 100-row page joins two upstream pages, so scrolling up can put it
    /// in front of the loaded run instead of replacing the run with a 50-row page.
    #[test]
    fn upstream_pages_join_only_when_contiguous() {
        let page = |offset: u64, ids: &[&str], next: bool| {
            let items: Vec<_> = ids
                .iter()
                .map(|id| json!({"track": {"id": id, "name": id}}))
                .collect();
            let mut v = json!({"items": items, "total": 100});
            if next {
                v["next"] = json!("more");
            }
            map_page(Kind::Tracks, v, offset, "me").unwrap()
        };
        let mut data = page(0, &["a", "b"], true);
        assert!(page_continues(&data));
        assert!(!join_page(&mut data, page(5, &["x"], false)));
        assert!(join_page(&mut data, page(2, &["c"], false)));
        let PageData::Tracks(p) = &data else {
            panic!("expected tracks")
        };
        assert_eq!((p.items.len(), p.next_offset, p.has_more), (3, 3, false));
        assert!(!page_continues(&data));
    }

    /// Regression: pages continue after every raw row, also when filtering kept none.
    #[test]
    fn filtered_pages_report_raw_continuation() {
        let playlists = json!({"items": [
            {"id": "p1", "name": "Followed", "owner": {"id": "other"}, "snapshot_id": "s"},
            {"id": "p2", "name": "Mine", "owner": {"id": "me"}, "snapshot_id": "s"},
            {"id": "p3", "name": "Followed", "owner": {"id": "other"}, "snapshot_id": "s"}
        ]});
        let PageData::Playlists(p) = map_page(Kind::Playlists, playlists, 50, "me").unwrap() else {
            panic!("expected playlists")
        };
        assert_eq!((p.items.len(), p.next_offset), (1, 53));
        let locals = json!({"items": [
            {"track": {"id": "l1", "name": "Local", "is_local": true}},
            {"track": {"id": "l2", "name": "Local", "is_local": true}}
        ]});
        let PageData::Tracks(p) = map_page(Kind::Tracks, locals, 0, "me").unwrap() else {
            panic!("expected tracks")
        };
        assert_eq!((p.items.len(), p.next_offset), (0, 2));
    }

    #[test]
    fn simplified_album_tracks_gain_album_artwork() {
        let album = parse_album(&json!({
            "id": "al1", "name": "Record", "images": [{"url": "cover", "width": 300, "height": 300}]
        }))
        .unwrap();
        let mut tracks = vec![parse_track(&json!({"id": "t1", "name": "Song"})).unwrap()];
        attach_album(&mut tracks, &album);
        assert_eq!(tracks[0].artwork_url(), Some("cover"));
    }

    #[test]
    fn library_pages_unwrap_saved_tracks_and_albums() {
        let liked = json!({"total": 31, "next": "https://api.spotify.com/v1/me/tracks?offset=50",
        "items": [{"added_at": "2026-01-01T00:00:00Z", "track": {
            "id": "t1", "name": "Song", "duration_ms": 1000, "is_playable": false,
            "artists": [{"id": "a1", "name": "Someone"}],
            "album": {"id": "al1", "name": "Record", "images": [{"url": "cover", "width": 300, "height": 300}]}
        }}]});
        let PageData::Tracks(p) = map_page(Kind::Tracks, liked, 0, "me").unwrap() else {
            panic!("expected tracks")
        };
        assert_eq!((p.total, p.has_more), (Some(31), true));
        assert_eq!(p.items[0].id.0, "t1");
        assert!(!p.items[0].playable);
        assert_eq!(p.items[0].artwork_url(), Some("cover"));

        let saved = json!({"total": 1, "next": null, "items": [{"added_at": "x", "album": {
            "id": "al2", "name": "Saved", "artists": [{"id": "a2", "name": "Band"}],
            "images": [{"url": "saved-300", "width": 300, "height": 300}]
        }}]});
        let PageData::Albums(p) = map_page(Kind::Albums, saved, 50, "me").unwrap() else {
            panic!("expected albums")
        };
        assert_eq!((p.offset, p.has_more), (50, false));
        assert_eq!(p.items[0].name, "Saved");
        assert_eq!(p.items[0].artists[0].name, "Band");
        assert_eq!(p.items[0].artwork_url.as_deref(), Some("saved-300"));
    }

    #[test]
    fn search_overview_is_bounded_per_type_and_skips_nulls() {
        let tracks: Vec<_> = (0..12)
            .map(|i| json!({"id": format!("t{i}"), "name": "Song"}))
            .collect();
        let v = json!({
            "artists": {"items": [null, {"id": "a1", "name": "Someone"}]},
            "albums": {"items": [{"id": "al1", "name": "Record", "album_type": "album"}]},
            "tracks": {"items": tracks}
        });
        let r = parse_search(&v);
        assert_eq!(r.artists.len(), 1);
        assert_eq!(r.albums[0].id.0, "al1");
        assert_eq!(r.tracks.len(), SEARCH_LIMIT);
        assert!(parse_search(&json!({})).tracks.is_empty());
    }

    #[test]
    fn single_type_search_reads_its_nested_page() {
        let q = Query::SearchTracks { text: "x".into() };
        let mut v = json!({"tracks": {"total": 40, "next": "more", "items": [{"id": "t1", "name": "Song"}]}});
        let root = v.get_mut(search_root(&q).unwrap()).unwrap().take();
        let PageData::Tracks(p) = map_page(Kind::Tracks, root, 10, "me").unwrap() else {
            panic!("expected tracks")
        };
        assert_eq!((p.offset, p.total, p.items.len()), (10, Some(40), 1));
        assert_eq!(
            search_path("a b&c", "artist,album,track").unwrap(),
            "search?q=a+b%26c&type=artist,album,track"
        );
        assert!(search_path("  ", "track").is_err());
    }

    #[test]
    fn missing_library_scope_asks_to_reauthorize() {
        let e = library_scope(err(ErrorKind::Permission, "denied"));
        assert_eq!(e.kind, ErrorKind::Permission);
        assert!(e.message.contains("sign in again"));
        let other = library_scope(err(ErrorKind::Network, "down"));
        assert_eq!(other.message, "down");
    }
}
