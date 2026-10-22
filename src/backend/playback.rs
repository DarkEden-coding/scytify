use librespot_core::{
    SpotifyUri, authentication::Credentials, config::SessionConfig, session::Session,
    spotify_id::SpotifyId as LibrespotId,
};
use librespot_playback::{
    audio_backend,
    config::{AudioFormat, Bitrate, PlayerConfig},
    mixer::{self, Mixer, MixerConfig},
    player::{Player, PlayerEvent, PlayerEventChannel},
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// Connected local decoder/output plus its software volume control.
pub(crate) struct Playback {
    pub player: Arc<Player>,
    pub credentials: Credentials,
    session: Session,
    playing: std::sync::atomic::AtomicBool,
    pub connect: Option<super::connect::Connect>,
    pub connect_error: Option<String>,
    pub mixer: Arc<dyn Mixer>,
    events: Option<PlayerEventChannel>,
    event_gate: Mutex<EventGate>,
}

/// Returns the mandatory local decoder/output policy.
pub(crate) fn player_config() -> PlayerConfig {
    PlayerConfig {
        bitrate: Bitrate::Bitrate320,
        gapless: true,
        normalisation: true,
        position_update_interval: Some(Duration::from_secs(1)),
        ..PlayerConfig::default()
    }
}

impl Playback {
    /// Connects with separate librespot credentials and opens the compiled audio sink.
    pub(crate) async fn connect(
        client_id: String,
        credentials: Credentials,
        volume: f32,
    ) -> Result<Self, String> {
        tokio::time::timeout(
            Duration::from_secs(30),
            Self::connect_inner(client_id, credentials, volume),
        )
        .await
        .map_err(|_| {
            "Spotify Connect/audio connection timed out; playback tracking will use the Web API"
                .to_owned()
        })?
    }

    async fn connect_inner(
        client_id: String,
        credentials: Credentials,
        volume: f32,
    ) -> Result<Self, String> {
        let device_id = device_id(&client_id);
        // Login5 uses the AP's reusable credentials under the same KEYMASTER client
        // that minted the playback OAuth token. The developer ID only seeds device_id.
        let config = SessionConfig {
            device_id,
            client_id: super::playback_auth::CLIENT_ID.into(),
            autoplay: Some(false),
            ..SessionConfig::default()
        };
        let session = Session::new(config, None);
        let mut guard = SessionGuard(Some(session.clone()));
        let mixer =
            mixer::find(None).ok_or("no software mixer was compiled")?(MixerConfig::default())
                .map_err(|e| format!("could not initialize audio mixer: {e}"))?;
        mixer.set_volume(to_mixer_volume(volume));
        let getter = mixer.get_soft_volume();
        let backend = audio_backend::find(None).ok_or("no audio backend was compiled")?;
        let player = Player::new(player_config(), session.clone(), getter, move || {
            backend(None, AudioFormat::default())
        });
        let events = player.get_player_event_channel();
        let (connect, connect_error) = match super::connect::Connect::new(
            session.clone(),
            credentials.clone(),
            player.clone(),
            mixer.clone(),
            to_mixer_volume(volume),
        )
        .await
        {
            Ok(connect) => (Some(connect), None),
            Err(error) => {
                if error.contains("Login request was denied") || session.is_invalid() {
                    return Err(error);
                }
                if session.username().is_empty() {
                    session
                        .connect(credentials, false)
                        .await
                        .map_err(|e| format!("librespot session connection failed: {e}"))?;
                }
                (None, Some(error))
            }
        };
        let credentials = Credentials {
            username: Some(session.username()),
            auth_type: librespot_protocol::authentication::AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
            auth_data: session.auth_data(),
        };
        guard.0 = None;
        Ok(Self {
            credentials,
            session,
            playing: std::sync::atomic::AtomicBool::new(false),
            player,
            connect,
            connect_error,
            mixer,
            events: Some(events),
            event_gate: Mutex::new(EventGate::default()),
        })
    }

    /// Takes the sole player event receiver for integration into the backend select loop.
    pub(crate) fn take_events(&mut self) -> Option<PlayerEventChannel> {
        self.events.take()
    }

    /// Decodes and starts one Spotify track at a bounded millisecond position.
    pub(crate) fn load(
        &self,
        id: &crate::domain::SpotifyId,
        position_ms: u64,
        play: bool,
    ) -> Result<(), String> {
        let track = uri(id)?;
        self.event_gate.lock().unwrap().expect_track(id.clone());
        self.playing
            .store(play, std::sync::atomic::Ordering::Relaxed);
        self.player
            .load(track, play, position_ms.min(u32::MAX as u64) as u32);
        Ok(())
    }

    /// Warms the decoder for the next queue occurrence.
    pub(crate) fn preload(&self, id: &crate::domain::SpotifyId) -> Result<(), String> {
        self.player.preload(uri(id)?);
        Ok(())
    }

    /// Pauses decoded local playback.
    pub(crate) fn pause(&self) {
        if self
            .playing
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            self.player.pause();
        }
    }
    /// Stops decoding and closes the active sink stream.
    pub(crate) fn stop(&self) {
        self.playing
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.player.stop();
    }
    /// Seeks the active decoder.
    pub(crate) fn seek(&self, ms: u64) {
        self.event_gate.lock().unwrap().seek_started();
        self.player.seek(ms.min(u32::MAX as u64) as u32);
    }
    /// Validates an event against the most recent load/seek generation before exposing it.
    pub(crate) fn update(&self, event: PlayerEvent) -> Option<Update> {
        let mut gate = self.event_gate.lock().unwrap();
        if self.connect.is_some()
            && let PlayerEvent::Loading {
                play_request_id,
                track_id,
                ..
            } = &event
            && gate.request_id == Some(*play_request_id)
        {
            gate.expected_track = track_id.to_id().ok().map(crate::domain::SpotifyId);
        }
        let update = gate.update(event);
        match update {
            Some(Update::Playing(_)) => self
                .playing
                .store(true, std::sync::atomic::Ordering::Relaxed),
            Some(Update::Paused(_) | Update::End | Update::Unavailable) => self
                .playing
                .store(false, std::sync::atomic::Ordering::Relaxed),
            _ => {}
        }
        update
    }
    /// Updates software attenuation without exposing an unbounded integer conversion.
    pub(crate) fn set_volume(&self, volume: f32) {
        self.mixer.set_volume(to_mixer_volume(volume));
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop();
        self.session.shutdown();
    }
}

/// Closes partially initialized sessions when connection fails or is cancelled.
struct SessionGuard(Option<Session>);
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some(session) = &self.0 {
            session.shutdown();
        }
    }
}

/// Stable per installation/account client so Spotify does not accumulate a new speaker
/// on every connection. Machine identity is hashed and never leaves the process verbatim.
fn device_id(client_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let machine = std::fs::read_to_string("/etc/machine-id")
        .unwrap_or_else(|_| super::connect::device_name());
    let user = std::env::var("USER").unwrap_or_default();
    let digest = Sha256::digest(format!("custom-spotify:{machine}:{user}:{client_id}"));
    format!("{digest:x}")[..40].into()
}

/// Converts a validated domain ID into a librespot track URI.
fn uri(id: &crate::domain::SpotifyId) -> Result<SpotifyUri, String> {
    let id = LibrespotId::from_base62(&id.0).map_err(|_| "invalid Spotify track ID".to_owned())?;
    Ok(SpotifyUri::Track { id })
}
/// Maps normalized volume to the mixer integer range.
fn to_mixer_volume(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

/// Projects relevant, generation-checked librespot events into backend state updates.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Update {
    Loading(u64),
    Playing(u64),
    Paused(u64),
    Position(u64),
    Seeked(u64),
    End,
    Unavailable,
}

/// Per-player event generation. A load invalidates the old request immediately, while seek
/// suppresses terminal events until librespot acknowledges the new decoder position.
#[derive(Default)]
struct EventGate {
    expected_track: Option<crate::domain::SpotifyId>,
    request_id: Option<u64>,
    request_floor: Option<u64>,
    terminal_armed: bool,
    terminal_seen: bool,
}

impl EventGate {
    fn expect_track(&mut self, track: crate::domain::SpotifyId) {
        self.expected_track = Some(track);
        self.request_floor = self.request_id.or(self.request_floor);
        self.request_id = None;
        self.terminal_armed = false;
        self.terminal_seen = false;
    }
    fn seek_started(&mut self) {
        self.terminal_armed = false;
    }
    fn matches(&self, request: u64, track: &SpotifyUri) -> bool {
        self.request_id == Some(request)
            && self
                .expected_track
                .as_ref()
                .is_some_and(|expected| track.to_id().ok().as_deref() == Some(expected.0.as_str()))
    }
    fn update(&mut self, event: PlayerEvent) -> Option<Update> {
        use PlayerEvent::*;
        match event {
            PlayRequestIdChanged { play_request_id }
                if self.request_floor.is_none_or(|old| play_request_id > old) =>
            {
                self.request_id = Some(play_request_id);
                self.request_floor = Some(play_request_id);
                self.terminal_seen = false;
                None
            }
            Loading {
                play_request_id,
                track_id,
                position_ms,
            } if self.matches(play_request_id, &track_id) => {
                self.terminal_armed = true;
                Some(Update::Loading(position_ms.into()))
            }
            Playing {
                play_request_id,
                track_id,
                position_ms,
            } if self.matches(play_request_id, &track_id) => {
                self.terminal_armed = true;
                Some(Update::Playing(position_ms.into()))
            }
            Paused {
                play_request_id,
                track_id,
                position_ms,
            } if self.matches(play_request_id, &track_id) => {
                Some(Update::Paused(position_ms.into()))
            }
            Seeked {
                play_request_id,
                track_id,
                position_ms,
            } if self.matches(play_request_id, &track_id) => {
                self.terminal_armed = true;
                Some(Update::Seeked(position_ms.into()))
            }
            PositionChanged {
                play_request_id,
                track_id,
                position_ms,
            }
            | PositionCorrection {
                play_request_id,
                track_id,
                position_ms,
            } if self.matches(play_request_id, &track_id) => {
                Some(Update::Position(position_ms.into()))
            }
            EndOfTrack {
                play_request_id,
                track_id,
            } if self.matches(play_request_id, &track_id)
                && self.terminal_armed
                && !self.terminal_seen =>
            {
                self.terminal_seen = true;
                Some(Update::End)
            }
            Unavailable {
                play_request_id,
                track_id,
            } if self.matches(play_request_id, &track_id)
                && self.terminal_armed
                && !self.terminal_seen =>
            {
                self.terminal_seen = true;
                Some(Update::Unavailable)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;
    fn track(s: &str) -> (crate::domain::SpotifyId, SpotifyUri) {
        let id = crate::domain::SpotifyId(s.into());
        let uri = uri(&id).unwrap();
        (id, uri)
    }
    #[test]
    fn stale_and_duplicate_terminal_events_are_ignored() {
        let (a, au) = track("4uLU6hMCjMI75M1A2tKUQC");
        let (_, wrong) = track("0VjIjW4GlUZAMYd2vXMi3b");
        let mut gate = EventGate::default();
        gate.expect_track(a.clone());
        gate.update(PlayerEvent::PlayRequestIdChanged { play_request_id: 1 });
        assert_eq!(
            gate.update(PlayerEvent::Loading {
                play_request_id: 1,
                track_id: au.clone(),
                position_ms: 0
            }),
            Some(Update::Loading(0))
        );
        // A duplicate occurrence has the same track ID, so its request generation matters.
        gate.expect_track(a);
        gate.update(PlayerEvent::PlayRequestIdChanged { play_request_id: 1 });
        assert_eq!(
            gate.update(PlayerEvent::EndOfTrack {
                play_request_id: 1,
                track_id: au.clone()
            }),
            None
        );
        gate.update(PlayerEvent::PlayRequestIdChanged { play_request_id: 2 });
        assert_eq!(
            gate.update(PlayerEvent::Loading {
                play_request_id: 2,
                track_id: wrong,
                position_ms: 0,
            }),
            None
        );
        gate.update(PlayerEvent::Loading {
            play_request_id: 2,
            track_id: au.clone(),
            position_ms: 0,
        });
        assert_eq!(
            gate.update(PlayerEvent::EndOfTrack {
                play_request_id: 2,
                track_id: au.clone()
            }),
            Some(Update::End)
        );
        assert_eq!(
            gate.update(PlayerEvent::EndOfTrack {
                play_request_id: 2,
                track_id: au
            }),
            None
        );
    }
    #[test]
    fn terminal_is_suppressed_until_seek_acknowledgement() {
        let (a, au) = track("4uLU6hMCjMI75M1A2tKUQC");
        let mut gate = EventGate::default();
        gate.expect_track(a);
        gate.update(PlayerEvent::PlayRequestIdChanged { play_request_id: 7 });
        gate.update(PlayerEvent::Playing {
            play_request_id: 7,
            track_id: au.clone(),
            position_ms: 1,
        });
        gate.seek_started();
        assert_eq!(
            gate.update(PlayerEvent::EndOfTrack {
                play_request_id: 7,
                track_id: au.clone()
            }),
            None
        );
        assert_eq!(
            gate.update(PlayerEvent::Seeked {
                play_request_id: 7,
                track_id: au.clone(),
                position_ms: 50
            }),
            Some(Update::Seeked(50))
        );
        assert_eq!(
            gate.update(PlayerEvent::EndOfTrack {
                play_request_id: 7,
                track_id: au
            }),
            Some(Update::End)
        );
    }
}
