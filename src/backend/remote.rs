//! Official Web API playback observation and commands. No internal control endpoints.
use super::api::{Api, err, parse_track, response_json, valid_id};
use crate::{contracts::*, domain::*};
use reqwest::Method;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(super) const QUEUE_PREVIEW_LIMIT: usize = 50;

#[derive(Clone, Debug)]
pub(super) struct Observation {
    pub devices: Option<Vec<PlaybackDevice>>,
    pub active: Option<PlaybackDevice>,
    pub track: Option<Track>,
    pub position_ms: u64,
    pub observed_at: Instant,
    pub status: PlaybackStatus,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub context_uri: Option<String>,
    pub queue: Option<Vec<Track>>,
}

impl Api {
    pub(super) async fn player_request(
        &self,
        token: &str,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<Value, AppError> {
        self.timed_player_request(token, method, path, query, body)
            .await
            .map(|(value, _)| value)
    }

    async fn timed_player_request(
        &self,
        token: &str,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<(Value, Instant), AppError> {
        if let Some(until) = *self.playback_backoff.lock().unwrap()
            && until > Instant::now()
        {
            return Err(AppError {
                kind: ErrorKind::RateLimited,
                message: "Spotify playback rate limit reached".into(),
                retry_after_seconds: Some(
                    until
                        .saturating_duration_since(Instant::now())
                        .as_secs()
                        .saturating_add(1),
                ),
            });
        }
        let mut url = url::Url::parse(&format!("https://api.spotify.com/v1/me/player{path}"))
            .map_err(|_| err(ErrorKind::InvalidInput, "invalid playback endpoint"))?;
        url.query_pairs_mut().extend_pairs(
            query
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
        let mut request = self.http.request(method, url).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let sent_at = Instant::now();
        let response = request
            .send()
            .await
            .map_err(|_| err(ErrorKind::Network, "Spotify playback is unreachable"))?;
        let received_at = Instant::now();
        // Spotify's timestamp can be stale. Estimate the measurement using half
        // the request RTT, excluding JSON parsing and subsequent detail requests.
        let observed_at = request_midpoint(sent_at, received_at);
        let result = response_json(response).await.map_err(playback_scope);
        if let Err(error) = &result
            && error.kind == ErrorKind::RateLimited
        {
            *self.playback_backoff.lock().unwrap() = Instant::now().checked_add(
                Duration::from_secs(error.retry_after_seconds.unwrap_or(5).max(1)),
            );
        }
        result.map(|value| (value, observed_at))
    }

    pub(super) async fn observe(
        &self,
        token: &str,
        details: bool,
    ) -> Result<Observation, AppError> {
        let (value, observed_at) = self
            .timed_player_request(token, Method::GET, "", &[], None)
            .await?;
        let mut observation = parse_player(&value, observed_at);
        if details {
            let devices = self
                .player_request(token, Method::GET, "/devices", &[], None)
                .await?;
            observation.devices = Some(parse_devices(&devices));
            let queue = self
                .player_request(token, Method::GET, "/queue", &[], None)
                .await?;
            observation.queue = Some(parse_queue(&queue));
        }
        Ok(observation)
    }

    pub(super) async fn control(
        &self,
        token: &str,
        device: Option<&str>,
        action: &Action,
    ) -> Result<(), AppError> {
        let _control = self.control_lock.lock().await;
        let mut query = Vec::new();
        if let Some(id) = device {
            query.push(("device_id".into(), valid_device(id)?.into()));
        }
        let (method, path, body) = match action {
            Action::TransferPlayback { device_id, play } => {
                query.clear();
                (
                    Method::PUT,
                    "",
                    Some(json!({"device_ids": [valid_device(device_id)?], "play": play})),
                )
            }
            Action::Resume => (Method::PUT, "/play", None),
            Action::Pause => (Method::PUT, "/pause", None),
            Action::Next => (Method::POST, "/next", None),
            Action::Previous => (Method::POST, "/previous", None),
            Action::Seek { position_ms } => {
                query.push(("position_ms".into(), position_ms.to_string()));
                (Method::PUT, "/seek", None)
            }
            Action::SetShuffle(value) => {
                query.push(("state".into(), value.to_string()));
                (Method::PUT, "/shuffle", None)
            }
            Action::SetRepeat(value) => {
                query.push((
                    "state".into(),
                    match value {
                        RepeatMode::Off => "off",
                        RepeatMode::Queue => "context",
                        RepeatMode::One => "track",
                    }
                    .into(),
                ));
                (Method::PUT, "/repeat", None)
            }
            Action::SetVolume(value) => {
                if !value.is_finite() || !(0.0..=1.0).contains(value) {
                    return Err(err(
                        ErrorKind::InvalidInput,
                        "volume must be between zero and one",
                    ));
                }
                query.push(("volume_percent".into(), (value * 100.0).round().to_string()));
                (Method::PUT, "/volume", None)
            }
            Action::SetDeviceVolume {
                device_id,
                volume_percent,
            } => {
                if *volume_percent > 100 {
                    return Err(err(
                        ErrorKind::InvalidInput,
                        "volume must be between zero and 100",
                    ));
                }
                query = vec![
                    ("device_id".into(), valid_device(device_id)?.into()),
                    ("volume_percent".into(), volume_percent.to_string()),
                ];
                (Method::PUT, "/volume", None)
            }
            Action::Play(source) => {
                let body = self.play_body(token, source).await?;
                (Method::PUT, "/play", Some(body))
            }
            Action::AddToQueue(ids) => {
                if ids.len() > 100 {
                    return Err(err(
                        ErrorKind::InvalidInput,
                        "at most 100 tracks may be queued",
                    ));
                }
                // Validate the entire batch before sending any request.
                let uris = ids.iter().map(track_uri).collect::<Result<Vec<_>, _>>()?;
                for uri in uris {
                    let mut query = query.clone();
                    query.push(("uri".into(), uri));
                    self.player_request(token, Method::POST, "/queue", &query, None)
                        .await?;
                }
                return Ok(());
            }
            Action::PlayNext(_) | Action::RemoveQueued { .. } => {
                return Err(err(
                    ErrorKind::Unavailable,
                    "Spotify's official API cannot insert at the front of or remove from a Connect queue",
                ));
            }
            _ => return Err(err(ErrorKind::InvalidInput, "unsupported playback command")),
        };
        self.player_request(token, method, path, &query, body)
            .await?;
        Ok(())
    }

    async fn play_body(&self, token: &str, source: &PlaySource) -> Result<Value, AppError> {
        Ok(match source {
            PlaySource::Track(id) => json!({"uris": [track_uri(id)?], "position_ms": 0}),
            PlaySource::Playlist {
                playlist_id,
                start_offset,
            } => json!({
                "context_uri": format!("spotify:playlist:{}", valid_id(playlist_id)?),
                "offset": {"position": start_offset}, "position_ms": 0
            }),
            PlaySource::Album {
                album_id,
                start_offset,
            } => json!({
                "context_uri": format!("spotify:album:{}", valid_id(album_id)?),
                "offset": {"position": start_offset}, "position_ms": 0
            }),
            PlaySource::LikedSongs { start_offset } => {
                // Web API play accepts at most 100 URIs: the selected occurrence and what follows.
                let uris = self
                    .liked_window(token, *start_offset)
                    .await?
                    .iter()
                    .map(track_uri)
                    .collect::<Result<Vec<_>, _>>()?;
                json!({"uris": uris, "offset": {"position": 0}, "position_ms": 0})
            }
        })
    }
}

pub(super) fn track_uri(id: &SpotifyId) -> Result<String, AppError> {
    Ok(format!("spotify:track:{}", valid_id(id)?))
}

fn valid_device(id: &str) -> Result<&str, AppError> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        Err(err(ErrorKind::InvalidInput, "invalid Spotify device ID"))
    } else {
        Ok(id)
    }
}

fn playback_scope(error: AppError) -> AppError {
    if error.kind == ErrorKind::Permission {
        err(
            ErrorKind::Permission,
            "Spotify denied playback access. Sign out and sign in again to grant user-read-playback-state, user-modify-playback-state and user-read-currently-playing; Premium and app authorization are also required.",
        )
    } else {
        error
    }
}

pub(super) fn device_type(value: &str) -> DeviceType {
    match value.to_ascii_lowercase().as_str() {
        "computer" => DeviceType::Computer,
        "smartphone" => DeviceType::Smartphone,
        "tablet" => DeviceType::Tablet,
        "speaker" => DeviceType::Speaker,
        "tv" => DeviceType::Tv,
        _ => DeviceType::Other(value.into()),
    }
}

fn parse_device(value: &Value) -> Option<PlaybackDevice> {
    Some(PlaybackDevice {
        id: value["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        name: value["name"].as_str()?.into(),
        device_type: device_type(value["type"].as_str().unwrap_or("unknown")),
        is_active: value["is_active"].as_bool().unwrap_or(false),
        is_this_app: false,
        volume_percent: value["volume_percent"]
            .as_u64()
            .filter(|v| *v <= 100)
            .map(|v| v as u8),
        supports_volume: value["supports_volume"].as_bool().unwrap_or(false),
        is_restricted: value["is_restricted"].as_bool().unwrap_or(false),
    })
}

fn parse_devices(value: &Value) -> Vec<PlaybackDevice> {
    value["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_device)
        .take(100)
        .collect()
}

fn parse_queue(value: &Value) -> Vec<Track> {
    value["queue"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_track)
        .take(QUEUE_PREVIEW_LIMIT)
        .collect()
}

fn request_midpoint(sent_at: Instant, received_at: Instant) -> Instant {
    sent_at + received_at.saturating_duration_since(sent_at) / 2
}

/// Keep a stable playing anchor when a fresh measurement only differs by jitter.
fn keep_anchor(player: &PlayerState, observation: &Observation, unchanged: bool) -> bool {
    unchanged
        && player.status == PlaybackStatus::Playing
        && observation.status == PlaybackStatus::Playing
        && super::playback::position_at(player, observation.observed_at)
            .abs_diff(observation.position_ms)
            <= 250
}

fn parse_player(value: &Value, observed_at: Instant) -> Observation {
    Observation {
        devices: None,
        active: parse_device(&value["device"]).map(|mut device| {
            device.is_active = true;
            device
        }),
        track: parse_track(&value["item"]),
        position_ms: value["progress_ms"].as_u64().unwrap_or(0),
        observed_at,
        status: if value.is_null() {
            PlaybackStatus::Stopped
        } else if value["is_playing"].as_bool().unwrap_or(false) {
            PlaybackStatus::Playing
        } else {
            PlaybackStatus::Paused
        },
        shuffle: value["shuffle_state"].as_bool().unwrap_or(false),
        repeat: match value["repeat_state"].as_str() {
            Some("context") => RepeatMode::Queue,
            Some("track") => RepeatMode::One,
            _ => RepeatMode::Off,
        },
        context_uri: value["context"]["uri"].as_str().map(str::to_owned),
        queue: None,
    }
}

/// Poll state is independent of UI progress ticks and respects hidden-window cadence.
pub(super) struct Polling {
    pub visible: bool,
    pub pending: bool,
    pub next: Instant,
    pub details_at: Instant,
    failures: u32,
    pub revision: u64,
    pub last_live: Option<Instant>,
    pub notice_kind: Option<ErrorKind>,
}

impl Polling {
    pub fn new() -> Self {
        Self {
            visible: true,
            pending: false,
            next: Instant::now(),
            details_at: Instant::now(),
            failures: 0,
            revision: 0,
            last_live: None,
            notice_kind: None,
        }
    }
    pub fn due(&self) -> bool {
        !self.pending && Instant::now() >= self.next
    }
    /// Schedules the next poll. While Spirc plays on this device its decoder already
    /// reports playback and the dealer pushes takeovers, so polls only refresh devices.
    pub fn finish(
        &mut self,
        result: &Result<Observation, AppError>,
        source: &mut RemoteSource,
        decoder_owned: bool,
    ) {
        self.pending = false;
        let seconds = match result {
            Ok(_) => {
                self.failures = 0;
                *source = RemoteSource::Polling;
                match (self.visible, decoder_owned) {
                    (false, _) => 8,
                    (true, true) => 5,
                    (true, false) => 1,
                }
            }
            Err(error) => {
                *source = RemoteSource::Unavailable;
                self.failures = self.failures.saturating_add(1);
                error
                    .retry_after_seconds
                    .unwrap_or_else(|| 2_u64.pow(self.failures.min(6)))
                    .max(1)
            }
        };
        self.next = Instant::now() + Duration::from_secs(seconds);
    }
}

/// Applies an account snapshot to the shared UI/MPRIS player. Local fallback is left alone.
pub(super) fn apply(
    state: &mut AppState,
    mut observation: Observation,
    source: RemoteSource,
    spirc: bool,
) {
    let local_owned = !spirc
        && matches!(state.connect.playback_auth, PlaybackAuth::Ready)
        && state.connect.this_device_id.is_some()
        && (state.connect.active_device_id == state.connect.this_device_id
            || (state.player.current_entry_id.is_some()
                && state.player.status == PlaybackStatus::Playing));
    let previous_device = state.connect.active_device_id.clone();
    let had_active =
        previous_device.is_some() || state.connect.devices.iter().any(|device| device.is_active);
    let has_active = observation.active.is_some();
    let this_id = state.connect.this_device_id.as_deref();
    if let Some(devices) = observation.devices.take() {
        state.connect.devices = devices;
    }
    if local_owned {
        state.connect.active_device_id = state.connect.this_device_id.clone();
        for device in &mut state.connect.devices {
            device.is_active = device.id == state.connect.this_device_id;
        }
    } else if let Some(mut active) = observation.active.take() {
        active.is_this_app = active.id.as_deref() == this_id;
        state.connect.active_device_id = active.id.clone();
        for device in &mut state.connect.devices {
            device.is_active = device.id == active.id;
        }
        if let Some(existing) = state
            .connect
            .devices
            .iter_mut()
            .find(|device| device.id == active.id)
        {
            *existing = active;
        } else {
            state.connect.devices.push(active);
        }
    } else {
        state.connect.active_device_id = None;
        for device in &mut state.connect.devices {
            device.is_active = false;
        }
    }
    for device in &mut state.connect.devices {
        device.is_this_app = device.id.is_some() && device.id.as_deref() == this_id;
    }
    state.connect.source = source;
    if let Some(id) = state.connect.this_device_id.clone() {
        state
            .connect
            .ensure_local_device(&id, super::connect::device_name(), state.player.volume);
    }
    if local_owned {
        return;
    }
    if (!spirc && !state.connect.is_remote()) || (!has_active && !had_active) {
        return;
    }
    let queue_changed = observation.queue.as_ref().is_some_and(|queue| {
        queue
            .iter()
            .map(|t| &t.id)
            .ne(state.connect.queue_preview.iter().map(|t| &t.id))
    });
    // Spirc plays through this device's decoder, whose events are newer and more precise
    // than any account snapshot: a poll can predate the last pause, seek or skip.
    if spirc && !state.connect.is_remote() {
        if queue_changed || state.connect.context_uri != observation.context_uri {
            state.player.queue_revision = state.player.queue_revision.wrapping_add(1);
        }
        if let Some(queue) = observation.queue {
            state.connect.queue_preview = queue;
        }
        state.connect.context_uri = observation.context_uri;
        return;
    }
    let old_track = state.player.current.as_ref().map(|track| &track.id);
    let same_playback = previous_device == state.connect.active_device_id
        && old_track == observation.track.as_ref().map(|track| &track.id);
    let changed = !same_playback || state.connect.context_uri != observation.context_uri;
    if !changed && state.connect.is_remote() {
        let expected = super::playback::position_at(&state.player, observation.observed_at);
        if expected.abs_diff(observation.position_ms) > 1500 {
            // Only confirmed discontinuities in remote observations emit MPRIS Seeked.
            state.player.seek_revision = state.player.seek_revision.wrapping_add(1);
        }
    }
    let preserve_anchor = keep_anchor(&state.player, &observation, same_playback);
    if changed || queue_changed {
        state.player.queue_revision = state.player.queue_revision.wrapping_add(1);
    }
    if changed {
        state.connect.queue_preview.clear();
    }
    if let Some(queue) = observation.queue {
        state.connect.queue_preview = queue;
    }
    state.connect.context_uri = observation.context_uri;
    state.player.current = observation.track;
    state.player.current_entry_id = None;
    if !preserve_anchor {
        state.player.position_ms = observation.position_ms.min(
            state
                .player
                .current
                .as_ref()
                .map_or(u64::MAX, |t| t.duration_ms),
        );
        state.player.observed_at = observation.observed_at;
    }
    state.player.status = observation.status;
    state.player.shuffle = observation.shuffle;
    state.player.repeat = observation.repeat;
    if let Some(volume) = state
        .connect
        .devices
        .iter()
        .find(|d| d.is_active)
        .and_then(|d| d.volume_percent)
    {
        state.player.volume = f32::from(volume) / 100.0;
    }
}

/// Routes account controls to the active device, including unsupported queue edits so
/// they return an explicit capability error instead of mutating an unrelated local queue.
pub(super) fn routes_remote(state: &AppState, action: &Action) -> bool {
    matches!(
        action,
        Action::TransferPlayback { .. } | Action::SetDeviceVolume { .. } | Action::RefreshDevices
    ) || state.connect.is_remote() && is_control(action)
}

pub(super) fn is_control(action: &Action) -> bool {
    matches!(
        action,
        Action::Play(_)
            | Action::Resume
            | Action::Pause
            | Action::Next
            | Action::Previous
            | Action::Seek { .. }
            | Action::SetVolume(_)
            | Action::SetShuffle(_)
            | Action::SetRepeat(_)
            | Action::PlayNext(_)
            | Action::AddToQueue(_)
            | Action::RemoveQueued { .. }
    )
}

pub(super) fn queue_page(
    state: &AppState,
    revision: u64,
    request: PageRequest,
) -> Result<Response, AppError> {
    if request.limit == 0 || request.limit > MAX_PAGE_SIZE {
        return Err(err(ErrorKind::InvalidInput, "invalid page size"));
    }
    if revision != state.player.queue_revision {
        return Err(err(ErrorKind::StaleQueue, "queue changed"));
    }
    let queue = &state.connect.queue_preview;
    let start = usize::try_from(request.offset)
        .unwrap_or(usize::MAX)
        .min(queue.len());
    let end = start
        .saturating_add(request.limit as usize)
        .min(queue.len());
    let items = queue[start..end]
        .iter()
        .enumerate()
        .map(|(index, track)| QueueItem {
            entry: QueueEntry {
                entry_id: (start + index) as u64 + 1,
                track_id: track.id.clone(),
            },
            track: Some(track.clone()),
            explicit: false,
        })
        .collect();
    Ok(Response::Page(PageData::Queue {
        revision,
        page: Page {
            offset: request.offset,
            total: Some(queue.len() as u64),
            items,
            has_more: end < queue.len(),
            next_offset: end as u64,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_player(value: &Value) -> Observation {
        super::parse_player(value, Instant::now())
    }

    #[test]
    fn poll_measurement_uses_half_rtt_and_ignores_stale_server_timestamp() {
        let sent = Instant::now();
        let received = sent + Duration::from_millis(160);
        let measured = request_midpoint(sent, received);
        assert_eq!(measured, sent + Duration::from_millis(80));
        let observation = super::parse_player(
            &json!({"timestamp": 1, "progress_ms": 1000, "is_playing": true}),
            measured,
        );
        assert_eq!(observation.position_ms, 1000);
        assert_eq!(observation.observed_at, measured);
    }

    #[test]
    fn polling_and_dealer_jitter_preserve_anchors_but_jumps_and_transitions_replace_them() {
        for source in [RemoteSource::Polling, RemoteSource::Live] {
            let mut state = crate::backend::initial_state();
            let start = Instant::now();
            let mut observation = parse_player(&json!({
                "device": {"id": "phone", "name": "Phone", "type": "Smartphone"},
                "item": {"id": "0123456789012345678901", "name": "Song", "duration_ms": 120000, "artists": []},
                "progress_ms": 12000, "is_playing": true
            }));
            observation.observed_at = start;
            apply(&mut state, observation.clone(), source, false);
            for jitter in [-250_i64, -40, 80, 250] {
                observation.observed_at = start + Duration::from_secs(1);
                observation.position_ms = (13_000 + jitter) as u64;
                observation.context_uri = Some(format!("spotify:playlist:{jitter}"));
                apply(&mut state, observation.clone(), source, false);
                assert_eq!(state.player.position_ms, 12_000);
                assert_eq!(state.player.observed_at, start);
            }
            observation.position_ms = 20_000;
            apply(&mut state, observation.clone(), source, false);
            assert_eq!(state.player.position_ms, 20_000);
            assert_eq!(state.player.observed_at, observation.observed_at);
            assert_eq!(state.player.seek_revision, 1);
            observation.status = PlaybackStatus::Paused;
            observation.position_ms = 20_010;
            apply(&mut state, observation.clone(), source, false);
            assert_eq!(state.player.position_ms, 20_010);
            assert_eq!(state.player.status, PlaybackStatus::Paused);
            observation.status = PlaybackStatus::Playing;
            observation.track.as_mut().unwrap().id = SpotifyId("new-track".into());
            observation.position_ms = 0;
            apply(&mut state, observation.clone(), source, false);
            assert_eq!(state.player.position_ms, 0);
            observation.active.as_mut().unwrap().id = Some("speaker".into());
            observation.position_ms = 100;
            apply(&mut state, observation, source, false);
            assert_eq!(state.player.position_ms, 100);
        }
    }

    #[test]
    fn local_spirc_snapshots_never_override_the_decoder() {
        let mut state = crate::backend::initial_state();
        state.connect.this_device_id = Some("local".into());
        let mut observation = parse_player(&json!({
            "device": {"id": "local", "name": "App", "type": "Computer"},
            "item": {"id": "0123456789012345678901", "name": "Song", "duration_ms": 120000, "artists": []},
            "progress_ms": 12000, "is_playing": true
        }));
        apply(&mut state, observation.clone(), RemoteSource::Live, true);
        let decoder_time = observation.observed_at + Duration::from_millis(100);
        super::super::playback::apply_anchor(
            &mut state.player,
            &super::super::playback::Update::Playing(12_100),
            decoder_time,
        );
        // A poll sent before the user resumed: paused, an older position and old options.
        observation.observed_at += Duration::from_secs(1);
        observation.position_ms = 11_400;
        observation.status = PlaybackStatus::Paused;
        observation.shuffle = true;
        observation.context_uri = Some("spotify:album:0123456789012345678901".into());
        apply(&mut state, observation, RemoteSource::Polling, true);
        assert_eq!(state.player.status, PlaybackStatus::Playing);
        assert!(!state.player.shuffle);
        assert_eq!(state.player.position_ms, 12_100);
        assert_eq!(state.player.observed_at, decoder_time);
        assert_eq!(
            state.connect.context_uri.as_deref(),
            Some("spotify:album:0123456789012345678901")
        );
    }

    #[test]
    fn local_device_is_offered_once_and_real_registration_replaces_synthetic() {
        let mut state = crate::backend::initial_state();
        let local = super::super::transfer::LOCAL_DEVICE_ID;
        for _ in 0..2 {
            state.connect.ensure_local_device(local, "App".into(), 0.5);
        }
        assert_eq!(state.connect.devices.len(), 1);
        assert!(state.connect.devices[0].is_this_app);
        assert_eq!(state.connect.devices[0].device_type, DeviceType::Computer);
        assert!(state.connect.devices[0].supports_volume);
        let mut real = state.connect.devices[0].clone();
        real.id = Some("spotify-id".into());
        real.name = "Registered app".into();
        real.is_this_app = false;
        state.connect.devices.push(real);
        for _ in 0..2 {
            state
                .connect
                .ensure_local_device("spotify-id", "App".into(), 0.5);
        }
        assert_eq!(state.connect.devices.len(), 1);
        assert_eq!(state.connect.devices[0].name, "Registered app");
        assert_eq!(state.connect.this_device_id.as_deref(), Some("spotify-id"));
    }

    #[test]
    fn missing_playback_consent_does_not_claim_remote_audio_as_local() {
        let mut state = crate::backend::initial_state();
        state.connect.ensure_local_device(
            super::super::transfer::LOCAL_DEVICE_ID,
            "App".into(),
            0.5,
        );
        state.player.current_entry_id = Some(1); // A restored queue can coexist with remote playback.
        let observation = json!({"device":{"id":"phone", "name":"Phone", "type":"Smartphone"}, "is_playing":true});
        for _ in 0..2 {
            apply(
                &mut state,
                parse_player(&observation),
                RemoteSource::Polling,
                false,
            );
            assert_eq!(state.connect.active_device_id.as_deref(), Some("phone"));
            assert!(routes_remote(&state, &Action::Pause));
        }
        assert_eq!(
            state
                .connect
                .devices
                .iter()
                .filter(|d| d.is_this_app)
                .count(),
            1
        );
    }

    #[test]
    fn local_handoff_survives_stale_active_paused_and_empty_polls() {
        let mut state = crate::backend::initial_state();
        state.connect.playback_auth = PlaybackAuth::Ready;
        let local = super::super::transfer::LOCAL_DEVICE_ID;
        state.connect.ensure_local_device(local, "App".into(), 0.5);
        state.connect.active_device_id = Some(local.into());
        state.player.current_entry_id = Some(1);
        state.player.position_ms = 14_000;
        state.player.status = PlaybackStatus::Playing;
        state.connect.active_device_id = Some("stale-remote".into());
        apply(
            &mut state,
            parse_player(&Value::Null),
            RemoteSource::Polling,
            false,
        );
        assert_eq!(state.connect.active_device_id.as_deref(), Some(local));
        for status in [
            PlaybackStatus::Loading,
            PlaybackStatus::Playing,
            PlaybackStatus::Paused,
        ] {
            state.player.status = status;
            for value in [
                json!({"device":{"id":"macbook", "name":"MacBook", "type":"Computer"},"is_playing":true}),
                json!({"device":{"id":"macbook", "name":"MacBook", "type":"Computer"},"is_playing":false}),
                Value::Null,
            ] {
                let mut observation = parse_player(&value);
                observation.devices = Some(vec![]);
                apply(&mut state, observation, RemoteSource::Polling, false);
                assert!(!routes_remote(&state, &Action::Pause));
                assert_eq!(state.player.status, status);
                assert_eq!(state.player.position_ms, 14_000);
                assert_eq!(
                    state
                        .connect
                        .devices
                        .iter()
                        .filter(|d| d.is_this_app)
                        .count(),
                    1
                );
                assert!(
                    state
                        .connect
                        .devices
                        .iter()
                        .find(|d| d.is_this_app)
                        .unwrap()
                        .is_active
                );
            }
        }
        state.connect.active_device_id = Some("macbook".into());
        state.player.status = PlaybackStatus::Paused;
        apply(
            &mut state,
            parse_player(
                &json!({"device":{"id":"macbook","name":"MacBook","type":"Computer"},"is_playing":true}),
            ),
            RemoteSource::Polling,
            false,
        );
        assert!(routes_remote(&state, &Action::Pause));
    }

    #[test]
    fn official_player_devices_queue_and_empty_response() {
        let track = json!({"id":"0123456789012345678901", "name":"A song", "duration_ms": 120000, "artists":[]});
        let value = json!({"device":{"id":"phone", "name":"Phone", "type":"Smartphone", "volume_percent":42,"supports_volume":true}, "item":track,"is_playing":true,"progress_ms":1234,"shuffle_state":true,"repeat_state":"track","context":{"uri":"spotify:album:example"}});
        let player = parse_player(&value);
        assert_eq!(player.status, PlaybackStatus::Playing);
        assert_eq!(player.position_ms, 1234);
        assert_eq!(player.repeat, RepeatMode::One);
        assert!(player.shuffle);
        assert_eq!(player.active.unwrap().device_type, DeviceType::Smartphone);
        let devices = parse_devices(
            &json!({"devices":[value["device"], {"id":null,"name":"TV","type":"TV","is_restricted":true,"volume_percent":null}]}),
        );
        assert_eq!(devices.len(), 2);
        assert!(devices[1].is_restricted);
        assert_eq!(devices[1].volume_percent, None);
        let queue = parse_queue(
            &json!({"currently_playing":track,"queue":[track,null,{"type":"episode"}, track]}),
        );
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].id, queue[1].id);
        assert_eq!(parse_player(&Value::Null).status, PlaybackStatus::Stopped);
    }
    #[test]
    fn active_device_routes_controls_and_keeps_local_queue_untouched() {
        let mut state = crate::backend::initial_state();
        state.connect.this_device_id = Some("local".into());
        state.connect.active_device_id = Some("phone".into());
        for action in [
            Action::Pause,
            Action::Next,
            Action::AddToQueue(vec![]),
            Action::PlayNext(vec![]),
            Action::Play(PlaySource::LikedSongs { start_offset: 0 }),
        ] {
            assert!(routes_remote(&state, &action));
        }
        state.connect.active_device_id = Some("local".into());
        assert!(!routes_remote(&state, &Action::Pause));
        assert!(routes_remote(
            &state,
            &Action::TransferPlayback {
                device_id: "phone".into(),
                play: true
            }
        ));
    }
    #[test]
    fn fallback_recovers_and_honors_rate_limit_and_hidden_cadence() {
        let mut polling = Polling::new();
        let mut source = RemoteSource::Live;
        let mut error = err(ErrorKind::RateLimited, "limit");
        error.retry_after_seconds = Some(60);
        polling.finish(&Err(error), &mut source, false);
        assert_eq!(source, RemoteSource::Unavailable);
        assert!(polling.next.duration_since(Instant::now()) > Duration::from_secs(59));
        polling.visible = false;
        polling.finish(&Ok(parse_player(&Value::Null)), &mut source, false);
        assert_eq!(source, RemoteSource::Polling);
        assert!(polling.next.duration_since(Instant::now()) > Duration::from_secs(7));
        polling.visible = true;
        polling.finish(&Ok(parse_player(&Value::Null)), &mut source, true);
        assert!(polling.next.duration_since(Instant::now()) > Duration::from_secs(4));
        let mut state = crate::backend::initial_state();
        apply(
            &mut state,
            parse_player(&Value::Null),
            RemoteSource::Live,
            true,
        );
        assert_eq!(state.connect.source, RemoteSource::Live);
        state.player.status = PlaybackStatus::Paused;
        state.player.position_ms = 12000;
        apply(
            &mut state,
            parse_player(&Value::Null),
            RemoteSource::Live,
            true,
        );
        assert_eq!(state.player.status, PlaybackStatus::Paused);
        assert_eq!(state.player.position_ms, 12000);
    }
}
