//! Web API handoff to the custom local queue when Spirc registration is unavailable.
use super::{
    api::{Api, LoadedSource, err},
    remote::Observation,
};
use crate::{contracts::*, domain::*};
use std::time::Instant;

pub(super) const LOCAL_DEVICE_ID: &str = "scytify:local-player";

pub(super) struct LocalTransfer {
    pub pause_device_id: Option<String>,
    pub source: LoadedSource,
    pub position_ms: u64,
    pub play: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub context_uri: Option<String>,
}

pub(super) fn source_start(tracks: &[Track], selected: &Track) -> Option<usize> {
    tracks
        .iter()
        .position(|track| {
            track.id == selected.id
                && selected.source_index.is_some()
                && track.source_index == selected.source_index
        })
        .or_else(|| tracks.iter().position(|track| track.id == selected.id))
}

pub(super) fn context_source(uri: Option<&str>) -> Option<PlaySource> {
    let uri = uri?;
    if let Some(id) = uri.strip_prefix("spotify:playlist:") {
        Some(PlaySource::Playlist {
            playlist_id: SpotifyId(id.into()),
            start_offset: 0,
        })
    } else if let Some(id) = uri.strip_prefix("spotify:album:") {
        Some(PlaySource::Album {
            album_id: SpotifyId(id.into()),
            start_offset: 0,
        })
    } else if uri == "spotify:collection:tracks" || uri.ends_with(":collection") {
        Some(PlaySource::LikedSongs { start_offset: 0 })
    } else {
        None
    }
}

/// Pure handoff plan. Source membership has already resolved raw Spotify occurrences.
fn plan(
    observation: Observation,
    source: Option<LoadedSource>,
    now: Instant,
) -> Result<LocalTransfer, AppError> {
    let track = observation
        .track
        .ok_or_else(|| err(ErrorKind::Unavailable, "No remote track to transfer"))?;
    let elapsed = if observation.status == PlaybackStatus::Playing {
        now.saturating_duration_since(observation.observed_at)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    } else {
        0
    };
    let position_ms = observation
        .position_ms
        .saturating_add(elapsed)
        .min(track.duration_ms);
    let resolved = source.is_some();
    let source = source.unwrap_or_else(|| LoadedSource {
        source_id: track.id.clone(),
        snapshot_id: String::new(),
        tracks: vec![track.id.clone()],
        metadata: vec![track],
        start: 0,
    });
    Ok(LocalTransfer {
        pause_device_id: observation.active.and_then(|d| d.id),
        source,
        position_ms,
        play: observation.status == PlaybackStatus::Playing,
        shuffle: observation.shuffle,
        repeat: observation.repeat,
        context_uri: if resolved {
            observation.context_uri
        } else {
            None
        },
    })
}

impl Api {
    pub(super) async fn transfer_local(&self, token: &str) -> Result<LocalTransfer, AppError> {
        // Serialize the complete handoff with all official playback controls.
        let _control = self.control_lock.lock().await;
        let observation = self.observe(token, false).await?;
        let track = observation
            .track
            .as_ref()
            .ok_or_else(|| err(ErrorKind::Unavailable, "No remote track to transfer"))?;
        let source = if let Some(source) = context_source(observation.context_uri.as_deref()) {
            self.load_transfer_source(token, &source, track).await.ok()
        } else {
            None
        };
        let transfer = plan(observation, source, Instant::now())?;
        let query = transfer
            .pause_device_id
            .as_ref()
            .map(|id| vec![("device_id".into(), id.clone())])
            .unwrap_or_default();
        self.player_request(token, reqwest::Method::PUT, "/pause", &query, None)
            .await?;
        Ok(transfer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn track(id: &str, raw: u64) -> Track {
        Track {
            id: SpotifyId(id.into()),
            name: id.into(),
            artists: vec![],
            album: None,
            duration_ms: 120_000,
            playable: true,
            source_index: Some(raw),
        }
    }

    #[test]
    fn handoff_pauses_remote_and_loads_matching_occurrence_at_elapsed_position() {
        let tracks = vec![track("before", 0), track("song", 3), track("song", 7)];
        let selected = tracks[2].clone();
        let start = source_start(&tracks, &selected).unwrap();
        assert_eq!(
            start, 2,
            "raw offsets include skipped rows and duplicate occurrences"
        );
        let source = LoadedSource {
            source_id: SpotifyId("playlist".into()),
            snapshot_id: "v1".into(),
            tracks: tracks.iter().map(|t| t.id.clone()).collect(),
            metadata: tracks,
            start,
        };
        let now = Instant::now();
        let observation = Observation {
            devices: None,
            active: Some(PlaybackDevice {
                id: Some("macbook".into()),
                name: "MacBook".into(),
                device_type: DeviceType::Computer,
                is_active: true,
                is_this_app: false,
                volume_percent: None,
                supports_volume: true,
                is_restricted: false,
            }),
            track: Some(selected),
            position_ms: 12_000,
            observed_at: now - Duration::from_secs(2),
            status: PlaybackStatus::Playing,
            shuffle: true,
            repeat: RepeatMode::Queue,
            context_uri: Some("spotify:playlist:playlist".into()),
            queue: None,
        };
        let transfer = plan(observation.clone(), Some(source), now).unwrap();
        assert_eq!(transfer.pause_device_id.as_deref(), Some("macbook"));
        assert_eq!(transfer.source.start, 2);
        assert_eq!(transfer.position_ms, 14_000);
        assert!(transfer.play && transfer.shuffle);
        assert_eq!(transfer.repeat, RepeatMode::Queue);
        let mut paused = observation;
        paused.status = PlaybackStatus::Paused;
        let transfer = plan(paused, None, now).unwrap();
        assert_eq!(transfer.source.tracks, [SpotifyId("song".into())]);
        assert_eq!(transfer.source.start, 0);
        assert_eq!(transfer.position_ms, 12_000);
        assert!(!transfer.play);
        assert!(transfer.context_uri.is_none());
    }

    #[test]
    fn contexts_and_unknown_occurrences_resolve_without_guessing_raw_offsets() {
        for uri in [
            "spotify:playlist:abc",
            "spotify:album:abc",
            "spotify:user:me:collection",
            "spotify:collection:tracks",
        ] {
            assert!(context_source(Some(uri)).is_some());
        }
        assert!(context_source(Some("spotify:artist:abc")).is_none());
        let tracks = vec![track("before", 0), track("song", 3)];
        let mut remote = track("song", 0);
        remote.source_index = None;
        assert_eq!(source_start(&tracks, &remote), Some(1));
        assert_eq!(source_start(&tracks, &track("missing", 0)), None);
    }
}
