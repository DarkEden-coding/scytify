//! Internal Spotify Connect transport. Spirc and observation share the decoder's Session.
use super::remote::{self, Observation};
use crate::{contracts::Action, domain::*};
use futures_util::StreamExt;
use librespot_connect::{ConnectConfig, LoadRequest, LoadRequestOptions, PlayingTrack, Spirc};
use librespot_core::{Session, authentication::Credentials, dealer::protocol::Message};
use librespot_playback::{mixer::Mixer, player::Player};
use librespot_protocol::connect::ClusterUpdate;
use std::{
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, task::JoinHandle};

pub(super) struct Connect {
    pub spirc: Spirc,
    pub device_id: String,
    pub observations: watch::Receiver<Option<Observation>>,
    runner: JoinHandle<()>,
    observer: JoinHandle<()>,
    session: Session,
}

impl Connect {
    pub async fn new(
        session: Session,
        credentials: Credentials,
        player: Arc<Player>,
        mixer: Arc<dyn Mixer>,
        volume: u16,
    ) -> Result<Self, String> {
        let mut updates = session
            .dealer()
            .listen_for::<ClusterUpdate>("hm://connect-state/v1/cluster", Message::from_raw)
            .map_err(|e| format!("Connect subscription failed: {e}"))?;
        let config = ConnectConfig {
            name: device_name(),
            device_type: librespot_core::config::DeviceType::Computer,
            initial_volume: volume,
            ..ConnectConfig::default()
        };
        let (spirc, task) = Spirc::new(config, session.clone(), credentials, player, mixer)
            .await
            .map_err(|e| format!("Connect initialization failed: {e}"))?;
        let (sender, observations) = watch::channel(None);
        let device_id = session.device_id().to_owned();
        let observer = tokio::spawn(async move {
            while let Some(update) = updates.next().await {
                match update {
                    Ok(update) => {
                        if let Some(cluster) = update.cluster.as_ref() {
                            sender.send_replace(Some(parse_cluster(cluster)));
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            spirc,
            device_id,
            observations,
            runner: tokio::spawn(task),
            observer,
            session,
        })
    }

    pub fn healthy(&self) -> bool {
        !self.runner.is_finished() && !self.observer.is_finished() && !self.session.is_invalid()
    }

    /// Local transport is serialized through Spirc so its queue and published state agree.
    pub fn control(&self, action: &Action) -> Result<(), String> {
        let result = match action {
            Action::Resume => self.spirc.play(),
            Action::Pause => self.spirc.pause(),
            Action::Next => self.spirc.next(),
            Action::Previous => self.spirc.prev(),
            Action::Seek { position_ms } => self
                .spirc
                .set_position_ms((*position_ms).min(u32::MAX as u64) as u32),
            Action::SetVolume(volume) => {
                if !volume.is_finite() || !(0.0..=1.0).contains(volume) {
                    return Err("volume must be between zero and one".into());
                }
                self.spirc
                    .set_volume((volume * u16::MAX as f32).round() as u16)
            }
            Action::SetShuffle(value) => self.spirc.shuffle(*value),
            Action::SetRepeat(value) => {
                self.spirc
                    .repeat(*value == RepeatMode::Queue)
                    .map_err(|e| e.to_string())?;
                self.spirc.repeat_track(*value == RepeatMode::One)
            }
            _ => return Err("unsupported local Connect command".into()),
        };
        result.map_err(|e| format!("Connect command failed: {e}"))
    }

    pub fn load(
        &self,
        tracks: Vec<SpotifyId>,
        start: usize,
        position_ms: u64,
        play: bool,
    ) -> Result<(), String> {
        let tracks = tracks
            .iter()
            .map(remote::track_uri)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.message)?;
        let options = LoadRequestOptions {
            start_playing: play,
            seek_to: position_ms.min(u32::MAX as u64) as u32,
            playing_track: Some(PlayingTrack::Index(start.min(u32::MAX as usize) as u32)),
            ..LoadRequestOptions::default()
        };
        self.spirc.activate().map_err(|e| e.to_string())?;
        self.spirc
            .load(LoadRequest::from_tracks(tracks, options))
            .map_err(|e| e.to_string())
    }
}

impl Drop for Connect {
    fn drop(&mut self) {
        let _ = self.spirc.shutdown();
        self.runner.abort();
        self.observer.abort();
        self.session.shutdown();
    }
}

pub(super) fn device_name() -> String {
    let hostname = std::fs::read_to_string("/etc/hostname").unwrap_or_else(|_| "Computer".into());
    format!(
        "Custom Spotify ({})",
        hostname.trim().chars().take(64).collect::<String>()
    )
}

fn parse_cluster(cluster: &librespot_protocol::connect::Cluster) -> Observation {
    let player = &cluster.player_state;
    let devices: Vec<_> = cluster
        .device
        .iter()
        .take(100)
        .map(|(id, info)| PlaybackDevice {
            id: Some(id.clone()),
            name: info.name.clone(),
            device_type: remote::device_type(&format!(
                "{:?}",
                info.device_type.enum_value_or_default()
            )),
            is_active: id == &cluster.active_device_id,
            is_this_app: false,
            volume_percent: Some(
                ((info.volume.min(u16::MAX as u32) * 100) / u16::MAX as u32) as u8,
            ),
            supports_volume: !info.capabilities.disable_volume,
            is_restricted: !info.disallow_transfer_reasons.is_empty()
                || info.capabilities.restrict_to_local,
        })
        .collect();
    let active = devices
        .iter()
        .find(|device| device.is_active)
        .cloned()
        .or_else(|| {
            (!cluster.active_device_id.is_empty()).then(|| PlaybackDevice {
                id: Some(cluster.active_device_id.clone()),
                name: "Spotify device".into(),
                device_type: DeviceType::Other("unknown".into()),
                is_active: true,
                is_this_app: false,
                volume_percent: None,
                supports_volume: false,
                is_restricted: false,
            })
        });
    let status = if active.is_none() || (!player.is_playing && !player.is_paused) {
        PlaybackStatus::Stopped
    } else if player.is_paused {
        PlaybackStatus::Paused
    } else if player.is_buffering {
        PlaybackStatus::Loading
    } else {
        PlaybackStatus::Playing
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    // The cluster carries server time, avoiding dependence on the workstation clock.
    let server_ms = if cluster.server_timestamp_ms > 0 {
        cluster.server_timestamp_ms
    } else {
        now_ms
    };
    let elapsed = if status == PlaybackStatus::Playing {
        server_ms.saturating_sub(player.timestamp).max(0) as u64
    } else {
        0
    };
    let track = player
        .track
        .as_ref()
        .and_then(|t| parse_track(t, player.duration.max(0) as u64));
    Observation {
        devices: Some(devices),
        active,
        track,
        position_ms: (player.position_as_of_timestamp.max(0) as u64).saturating_add(elapsed),
        observed_at: Instant::now(),
        status,
        shuffle: player.options.shuffling_context,
        repeat: if player.options.repeating_track {
            RepeatMode::One
        } else if player.options.repeating_context {
            RepeatMode::Queue
        } else {
            RepeatMode::Off
        },
        context_uri: (!player.context_uri.is_empty()).then(|| player.context_uri.clone()),
        queue: Some(
            player
                .next_tracks
                .iter()
                .take(remote::QUEUE_PREVIEW_LIMIT)
                .filter_map(|track| parse_track(track, 0))
                .collect(),
        ),
    }
}

fn parse_track(
    track: &librespot_protocol::player::ProvidedTrack,
    duration_ms: u64,
) -> Option<Track> {
    let id = SpotifyId(track.uri.strip_prefix("spotify:track:")?.into());
    remote::track_uri(&id).ok()?;
    let name = track
        .metadata
        .get("title")
        .cloned()
        .unwrap_or_else(|| "Spotify track".into());
    let artists = track
        .metadata
        .get("artist_name")
        .map(|name| {
            vec![Artist {
                id: SpotifyId(
                    track
                        .artist_uri
                        .strip_prefix("spotify:artist:")
                        .unwrap_or_default()
                        .into(),
                ),
                name: name.clone(),
                artwork_url: None,
            }]
        })
        .unwrap_or_default();
    Some(Track {
        id,
        name,
        artists,
        album: None,
        duration_ms,
        playable: track.disallow_reasons.is_empty(),
        source_index: None,
    })
}

/// The dealer pushes every cluster change, so silence while idle is not staleness:
/// once one update has arrived, a healthy Connect stays the live source.
pub(super) fn live_recent(last: Option<Instant>, healthy: bool) -> bool {
    healthy && last.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn cluster_projects_device_options_position_and_duplicate_queue() {
        let mut cluster = librespot_protocol::connect::Cluster {
            active_device_id: "phone".into(),
            server_timestamp_ms: 12000,
            ..Default::default()
        };
        let mut player = librespot_protocol::player::PlayerState {
            is_playing: true,
            timestamp: 10000,
            position_as_of_timestamp: 3000,
            duration: 90000,
            ..Default::default()
        };
        let track = librespot_protocol::player::ProvidedTrack {
            uri: "spotify:track:0123456789012345678901".into(),
            ..Default::default()
        };
        player.track = Some(track.clone()).into();
        player.next_tracks = vec![track.clone(), track];
        player.options.mut_or_insert_default().repeating_context = true;
        cluster.player_state = Some(player).into();
        let observation = parse_cluster(&cluster);
        assert_eq!(observation.position_ms, 5000);
        assert_eq!(observation.repeat, RepeatMode::Queue);
        assert_eq!(observation.queue.unwrap().len(), 2);
        assert_eq!(observation.active.unwrap().id.as_deref(), Some("phone"));
        cluster.player_state.mut_or_insert_default().is_paused = true;
        cluster.player_state.mut_or_insert_default().is_buffering = true;
        assert_eq!(parse_cluster(&cluster).status, PlaybackStatus::Paused);
        assert!(live_recent(Some(Instant::now()), true));
        // A long idle silence stays live; only a missing first update or an unhealthy
        // Connect falls back to polling.
        assert!(live_recent(
            Some(Instant::now() - Duration::from_secs(600)),
            true
        ));
        assert!(!live_recent(None, true));
        assert!(!live_recent(Some(Instant::now()), false));
    }
}
