//! Spotify's own personalization and recommendation services, reached over the playback
//! session: the developer Web API no longer serves recommendations or Spotify-made playlists.
use crate::{contracts::*, domain::*};
use futures_util::StreamExt;
use librespot_core::{Session, SpotifyUri};
use librespot_metadata::{Metadata, image::ImageSize};
use librespot_protocol::autoplay_context_request::AutoplayContextRequest;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Spotify-owned playlists (editorial, Discover Weekly, mixes) all have this ID prefix. The
/// Web API answers 404 for them in development mode.
const SPOTIFY_OWNED: &str = "37i9dQZ";
/// Track metadata requests in flight at once.
const METADATA_CONCURRENCY: usize = 16;
/// Recommendation requests one Discover batch may spend looking for fresh tracks.
const STATION_REQUESTS: usize = 6;
/// Fresh tracks in the first Discover batch, and in each one added as it runs low.
pub(super) const FIRST_BATCH: usize = 25;
pub(super) const REFILL_BATCH: usize = 15;
/// Discover adds a batch once fewer of its tracks than this are still coming up.
const REFILL_AT: usize = 5;
/// Wait after a failed batch before Discover asks again.
const REFILL_RETRY: Duration = Duration::from_secs(60);
/// Playback after which a track counts as heard: Spotify's own threshold for a stream,
/// or half of a shorter track.
const HEARD_MS: u64 = 30_000;

/// Generated cover hosts used by personalized playlists, besides the regular image CDN.
pub(super) const MIX_ART_HOSTS: [&str; 3] = [
    "pickasso.spotifycdn.com",
    "seed-mix-image.spotifycdn.com",
    "daylist.spotifycdn.com",
];

/// Recommendations gathered for Discover, and Spotify's recent plays found on the way.
pub(super) struct Batch {
    pub tracks: Vec<Track>,
    /// Permission fails for tokens granted before the recent-plays scope existed.
    pub recent: Result<Vec<SpotifyId>, AppError>,
}

/// Actor-owned Discover and listening state.
#[derive(Default)]
pub(super) struct State {
    pub run: Option<Run>,
    /// Bumped whenever playback is replaced, so a late batch cannot take over again.
    pub generation: u64,
    pub listening: Listening,
}

impl State {
    /// Stops Discover because something else is starting.
    pub fn stop(&mut self) {
        self.run = None;
        self.generation = self.generation.wrapping_add(1);
    }
}

/// A playing Discover session, kept topped up with new batches.
pub(super) struct Run {
    /// Every track Discover queued, so no batch repeats one even if it was skipped early.
    pub served: HashSet<SpotifyId>,
    /// One of its tracks has played. Until then the current track is still the previous one.
    started: bool,
    pub refilling: bool,
    retry_at: Option<Instant>,
}

impl Run {
    pub fn new(tracks: &[Track]) -> Self {
        Self {
            served: tracks.iter().map(|t| t.id.clone()).collect(),
            started: false,
            refilling: false,
            retry_at: None,
        }
    }

    /// Whether Discover is still what plays. It ends once playback moves to a track it
    /// did not queue, such as a song the user picked.
    pub fn observe(&mut self, current: Option<&SpotifyId>) -> bool {
        match current {
            Some(id) if self.served.contains(id) => {
                self.started = true;
                true
            }
            Some(_) => !self.started,
            None => true,
        }
    }

    /// Whether to fetch another batch, given the upcoming tracks.
    pub fn needs_more<'a>(
        &self,
        upcoming: impl Iterator<Item = &'a SpotifyId>,
        now: Instant,
    ) -> bool {
        self.started
            && !self.refilling
            && self.retry_at.is_none_or(|at| now >= at)
            && upcoming.filter(|id| self.served.contains(id)).count() < REFILL_AT
    }

    pub fn batch_failed(&mut self, now: Instant) {
        self.refilling = false;
        self.retry_at = Some(now + REFILL_RETRY);
    }
}

/// Notices when the playing track has been heard long enough to count as listened to.
#[derive(Default)]
pub(super) struct Listening {
    /// The current track and whether it was already recorded.
    track: Option<(SpotifyId, bool)>,
}

impl Listening {
    /// The current track, the first time its position passes the threshold.
    pub fn observe(&mut self, player: &PlayerState, position_ms: u64) -> Option<SpotifyId> {
        let current = player.current.as_ref()?;
        if self.track.as_ref().is_none_or(|(id, _)| *id != current.id) {
            self.track = Some((current.id.clone(), false));
        }
        let threshold = match current.duration_ms {
            0 => HEARD_MS,
            duration => HEARD_MS.min(duration / 2),
        };
        let (id, recorded) = self.track.as_mut()?;
        if *recorded || player.status != PlaybackStatus::Playing || position_ms < threshold {
            return None;
        }
        *recorded = true;
        Some(id.clone())
    }
}

pub(super) fn is_spotify_owned(id: &SpotifyId) -> bool {
    id.0.starts_with(SPOTIFY_OWNED)
}

/// Discover Weekly, Release Radar and the Daily Mixes from the account's home feed.
pub(super) async fn made_for_you(session: &Session) -> Result<Vec<Playlist>, AppError> {
    let body = session
        .spclient()
        .request_as_json(&reqwest::Method::GET, "/homeview/v1/home", None, None)
        .await
        .map_err(|e| unavailable("Spotify's home feed", e))?;
    let feed: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| safe(ErrorKind::Network, "Spotify returned an invalid home feed"))?;
    Ok(parse_made_for_you(&feed))
}

/// Playlist cards under "Made For …", plus Discover Weekly wherever it appears (it is often
/// only among the shortcuts), with Discover Weekly first.
fn parse_made_for_you(feed: &serde_json::Value) -> Vec<Playlist> {
    let mut section = "";
    let mut playlists: Vec<Playlist> = Vec::new();
    for item in feed["body"].as_array().into_iter().flatten() {
        let title = item["text"]["title"].as_str().unwrap_or("");
        if item["component"]["category"] == "header" {
            section = title;
            continue;
        }
        let Some(id) = item["target"]["uri"]
            .as_str()
            .and_then(|uri| uri.strip_prefix("spotify:playlist:"))
        else {
            continue;
        };
        let weekly = title == "Discover Weekly";
        // The AI DJ card is an interactive session, not a playlist with fixed tracks.
        if !(weekly || section.starts_with("Made For"))
            || title == "DJ"
            || playlists.iter().any(|p| p.id.0 == id)
        {
            continue;
        }
        let playlist = Playlist {
            id: SpotifyId(id.into()),
            name: title.into(),
            owner_id: "spotify".into(),
            snapshot_id: String::new(),
            track_count: None,
            artwork_url: item["images"]["main"]["uri"]
                .as_str()
                .filter(|url| url.starts_with("https://"))
                .map(str::to_owned),
        };
        if weekly {
            playlists.insert(0, playlist);
        } else {
            playlists.push(playlist);
        }
    }
    playlists
}

/// Every track of a Spotify-owned playlist, with metadata, in playlist order.
pub(super) async fn playlist_tracks(
    session: &Session,
    playlist_id: &SpotifyId,
) -> Result<Vec<Track>, AppError> {
    let ids = context_tracks(session, &format!("spotify:playlist:{}", playlist_id.0)).await?;
    let mut tracks = hydrate(session, &ids).await;
    // Rows keep their raw position so Play starts the context at the right track.
    for track in &mut tracks {
        track.source_index = ids.iter().position(|id| *id == track.id).map(|i| i as u64);
    }
    Ok(tracks)
}

/// The account's Liked Songs, in one request.
pub(super) async fn liked_tracks(session: &Session) -> Result<Vec<SpotifyId>, AppError> {
    context_tracks(
        session,
        &format!("spotify:user:{}:collection", session.username()),
    )
    .await
}

async fn context_tracks(session: &Session, uri: &str) -> Result<Vec<SpotifyId>, AppError> {
    let context = session
        .spclient()
        .get_context(uri)
        .await
        .map_err(|e| unavailable("This Spotify playlist", e))?;
    Ok(context
        .pages
        .iter()
        .flat_map(|page| &page.tracks)
        .filter_map(|track| track_id(track.uri.as_deref()?))
        .collect())
}

/// Up to `size` recommended tracks that are neither liked nor in `exclude`, with metadata.
/// Recommendations come from Liked Songs autoplay, then from radio stations seeded with
/// random liked songs, until enough fresh tracks turn up.
pub(super) async fn fresh_tracks(
    session: &Session,
    liked: &[SpotifyId],
    exclude: &HashSet<SpotifyId>,
    size: usize,
) -> Result<Vec<Track>, AppError> {
    let seeds: Vec<String> = {
        use rand::seq::IndexedRandom;
        liked
            .choose_multiple(&mut rand::rng(), STATION_REQUESTS - 1)
            .map(|id| format!("spotify:track:{}", id.0))
            .collect()
    };
    let collection = format!("spotify:user:{}:collection", session.username());
    let mut picked: Vec<SpotifyId> = Vec::new();
    let mut last_error = None;
    for request in 0..STATION_REQUESTS {
        if picked.len() >= size {
            break;
        }
        let candidates = match request {
            0 => autoplay(session, &collection, exclude).await,
            _ => match seeds.get(request - 1) {
                Some(seed) => station(session, seed).await,
                None => break,
            },
        };
        match candidates {
            Ok(ids) => {
                for id in ids {
                    if picked.len() < size && !exclude.contains(&id) && !picked.contains(&id) {
                        picked.push(id);
                    }
                }
            }
            Err(error) => last_error = Some(error),
        }
    }
    if picked.is_empty() {
        return Err(last_error.unwrap_or_else(|| {
            safe(
                ErrorKind::Unavailable,
                "Spotify has no new recommendations right now; like a few more songs and retry",
            )
        }));
    }
    Ok(hydrate(session, &picked).await)
}

/// What Spotify would autoplay after a context, avoiding a sample of `recent` tracks.
async fn autoplay(
    session: &Session,
    context_uri: &str,
    recent: &HashSet<SpotifyId>,
) -> Result<Vec<SpotifyId>, AppError> {
    let request = AutoplayContextRequest {
        context_uri: Some(context_uri.into()),
        recent_track_uri: recent
            .iter()
            .take(50)
            .map(|id| format!("spotify:track:{}", id.0))
            .collect(),
        ..Default::default()
    };
    let context = session
        .spclient()
        .get_autoplay_context(&request)
        .await
        .map_err(|e| unavailable("Spotify's recommendations", e))?;
    Ok(context
        .pages
        .iter()
        .flat_map(|page| &page.tracks)
        .filter_map(|track| track_id(track.uri.as_deref()?))
        .collect())
}

/// Radio station tracks for a seed URI.
async fn station(session: &Session, seed: &str) -> Result<Vec<SpotifyId>, AppError> {
    let body = session
        .spclient()
        .get_apollo_station("stations", seed, Some(50), Vec::new(), false)
        .await
        .map_err(|e| unavailable("Spotify's recommendations", e))?;
    let station: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| safe(ErrorKind::Network, "Spotify returned an invalid station"))?;
    Ok(station["tracks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|track| track_id(track["uri"].as_str()?))
        .collect())
}

/// Full track metadata in the order given; tracks Spotify cannot describe are left out.
pub(super) async fn hydrate(session: &Session, ids: &[SpotifyId]) -> Vec<Track> {
    // Owned IDs and session handles keep the spawned job's future `Send`.
    futures_util::stream::iter(ids.to_vec())
        .map(|id| {
            let session = session.clone();
            async move {
                let uri = SpotifyUri::from_uri(&format!("spotify:track:{}", id.0)).ok()?;
                let track = librespot_metadata::Track::get(&session, &uri).await.ok()?;
                convert(&track)
            }
        })
        .buffered(METADATA_CONCURRENCY)
        .filter_map(std::future::ready)
        .collect()
        .await
}

fn convert(track: &librespot_metadata::Track) -> Option<Track> {
    let artists = |list: &[librespot_metadata::artist::Artist]| {
        list.iter()
            .map(|artist| Artist {
                id: SpotifyId(artist.id.to_id().unwrap_or_default()),
                name: artist.name.clone(),
                artwork_url: None,
            })
            .collect()
    };
    let album = &track.album;
    // The default size is Spotify's 300 px cover, the same the Web API offers for cards.
    let cover = album
        .covers
        .iter()
        .find(|image| image.size == ImageSize::DEFAULT)
        .or(album.covers.first())
        .and_then(|image| image.id.to_base16().ok())
        .map(|hex| format!("https://i.scdn.co/image/{}", hex.to_lowercase()));
    Some(Track {
        id: SpotifyId(track.id.to_id().ok()?),
        name: track.name.clone(),
        artists: artists(&track.artists),
        album: Some(Album {
            id: SpotifyId(album.id.to_id().unwrap_or_default()),
            name: album.name.clone(),
            artists: artists(&album.artists),
            artwork_url: cover,
        }),
        duration_ms: track.duration.max(0) as u64,
        playable: true,
        source_index: None,
    })
}

fn track_id(uri: &str) -> Option<SpotifyId> {
    let id = SpotifyId(uri.strip_prefix("spotify:track:")?.into());
    super::api::valid_id(&id).ok()?;
    Some(id)
}

/// Answers the queries served by the playback session instead of the Web API.
pub(super) async fn fetch(
    session: &Session,
    query: &Query,
    request: PageRequest,
) -> Result<Response, AppError> {
    let data = match query {
        Query::MadeForYou => {
            let playlists = made_for_you(session).await?;
            let limit = request
                .limit
                .max(playlists.len().min(usize::from(u16::MAX)) as u16);
            PageData::Playlists(page(playlists, PageRequest { offset: 0, limit }))
        }
        Query::PlaylistTracks { playlist_id } => {
            PageData::Tracks(page(playlist_tracks(session, playlist_id).await?, request))
        }
        _ => {
            return Err(safe(
                ErrorKind::InvalidInput,
                "not a playback-session query",
            ));
        }
    };
    Ok(Response::Page(data))
}

/// Contract page over a complete list loaded in one request.
pub(super) fn page<T>(items: Vec<T>, request: PageRequest) -> Page<T> {
    let total = items.len() as u64;
    let start = request.offset.min(total);
    let end = start.saturating_add(u64::from(request.limit)).min(total);
    Page {
        offset: start,
        total: Some(total),
        has_more: end < total,
        next_offset: end,
        items: items
            .into_iter()
            .skip(start as usize)
            .take((end - start) as usize)
            .collect(),
    }
}

fn unavailable(what: &str, error: librespot_core::Error) -> AppError {
    log::warn!("{what} request failed: {error}");
    safe(
        ErrorKind::Unavailable,
        &format!("{what} could not be loaded; retry shortly"),
    )
}

fn safe(kind: ErrorKind, message: &str) -> AppError {
    super::actor::safe(kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn made_for_you_keeps_its_section_and_puts_discover_weekly_first() {
        let card = |title: &str, uri: &str, art: &str| {
            json!({"component": {"category": "card"}, "text": {"title": title},
                "target": {"uri": uri}, "images": {"main": {"uri": art}}})
        };
        let header =
            |title: &str| json!({"component": {"category": "header"}, "text": {"title": title}});
        let feed = json!({"body": [
            header("Shortcuts"),
            card("Discover Weekly", "spotify:playlist:37i9dQZEVXcGxcBVE9Vdk4", "https://pickasso.spotifycdn.com/dw"),
            card("Chill", "spotify:playlist:2QhaBhMQ7KEFDRGl8WnAEw", "https://i.scdn.co/a"),
            header("Made For Dark Eden"),
            card("Release Radar", "spotify:playlist:37i9dQZEVXbcmlsJF8dMR3", "spotify:mosaic:x"),
            card("DJ", "spotify:playlist:37i9dQZF1EYkqdzj48dyYq", "https://i.scdn.co/e"),
            card("Daily Mix 1", "spotify:playlist:37i9dQZF1E39KhCNYunHzr", "https://i.scdn.co/b"),
            card("Ralph Castelli", "spotify:artist:4o16FybIZV7WPD7tOl7PUd", "https://i.scdn.co/c"),
            header("Jump back in"),
            card("Discover Weekly", "spotify:playlist:37i9dQZEVXcGxcBVE9Vdk4", "https://pickasso.spotifycdn.com/dw"),
            card("Bangers", "spotify:playlist:3z4HvepX7XpbzKXRfrw32D", "https://i.scdn.co/d"),
        ]});
        let playlists = parse_made_for_you(&feed);
        let names: Vec<&str> = playlists.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Discover Weekly", "Release Radar", "Daily Mix 1"]);
        // Mosaic URIs are not downloadable images.
        assert_eq!(playlists[1].artwork_url, None);
        assert!(is_spotify_owned(&playlists[0].id));
        assert!(!is_spotify_owned(&SpotifyId(
            "3z4HvepX7XpbzKXRfrw32D".into()
        )));
    }

    fn track(id: &str, duration_ms: u64) -> Track {
        Track {
            id: SpotifyId(id.into()),
            name: id.into(),
            artists: vec![],
            album: None,
            duration_ms,
            playable: true,
            source_index: None,
        }
    }

    #[test]
    fn discover_ends_on_a_foreign_track_only_after_it_started_and_refills_when_low() {
        let tracks: Vec<Track> = (0..8).map(|i| track(&format!("d{i}"), 1)).collect();
        let mut run = Run::new(&tracks);
        let old = SpotifyId("previous".into());
        let now = Instant::now();
        // Spirc has not switched yet: the old track is still current.
        assert!(run.observe(Some(&old)));
        assert!(!run.needs_more(std::iter::empty(), now));
        assert!(run.observe(Some(&tracks[0].id)));
        let upcoming: Vec<SpotifyId> = tracks[1..].iter().map(|t| t.id.clone()).collect();
        assert!(!run.needs_more(upcoming.iter(), now));
        // Autoplay tracks after the batch do not count as Discover's own.
        let low = [upcoming[5].clone(), upcoming[6].clone(), old.clone()];
        assert!(run.needs_more(low.iter(), now));
        run.refilling = true;
        assert!(!run.needs_more(low.iter(), now));
        run.batch_failed(now);
        assert!(!run.needs_more(low.iter(), now));
        assert!(run.needs_more(low.iter(), now + REFILL_RETRY));
        assert!(!run.observe(Some(&old)));
    }

    #[test]
    fn a_track_counts_as_heard_once_after_thirty_seconds_or_half_its_length() {
        let mut player = crate::backend::initial_state().player;
        let mut listening = Listening::default();
        player.status = PlaybackStatus::Playing;
        player.current = Some(track("long", 200_000));
        assert_eq!(listening.observe(&player, 29_999), None);
        assert_eq!(
            listening.observe(&player, 30_000),
            Some(SpotifyId("long".into()))
        );
        assert_eq!(listening.observe(&player, 60_000), None);
        player.current = Some(track("short", 40_000));
        player.status = PlaybackStatus::Paused;
        assert_eq!(listening.observe(&player, 25_000), None);
        player.status = PlaybackStatus::Playing;
        assert_eq!(
            listening.observe(&player, 25_000),
            Some(SpotifyId("short".into()))
        );
    }

    #[test]
    fn complete_lists_page_like_the_web_api() {
        let request = |offset, limit| PageRequest { offset, limit };
        let first = page((0..30).collect(), request(0, 20));
        assert_eq!(
            (first.items.len(), first.has_more, first.next_offset),
            (20, true, 20)
        );
        let last = page((0..30).collect::<Vec<u8>>(), request(20, 20));
        assert_eq!(
            (last.items, last.has_more, last.total),
            ((20..30).collect(), false, Some(30))
        );
        assert!(
            page((0..3).collect::<Vec<u8>>(), request(9, 20))
                .items
                .is_empty()
        );
    }
}
