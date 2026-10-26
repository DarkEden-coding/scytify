//! Native desktop media controls.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::run;

#[cfg(target_os = "linux")]
mod linux {
    use crate::contracts::{Action, AppError, Command, ErrorKind, Event, RequestId};
    use crate::domain::{AppState, PlaybackStatus as DomainStatus, RepeatMode};
    use mpris_server::{
        LoopStatus, Metadata, PlaybackStatus, PlayerInterface, Property, RootInterface, Server,
        Signal, Time, TrackId, Volume, zbus,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;
    use tokio::sync::{mpsc, watch};

    static NEXT_ID: AtomicU64 = AtomicU64::new(1u64 << 63);

    /// Reserve the high request-ID range for desktop clients, separate from GUI requests.
    fn request_id() -> RequestId {
        RequestId(NEXT_ID.fetch_add(1, Ordering::Relaxed) | (1u64 << 63))
    }

    /// Fail a desktop request explicitly when the bounded playback command channel is full.
    fn send_command(tx: &mpsc::Sender<Command>, action: Action) -> Result<(), zbus::fdo::Error> {
        tx.try_send(Command {
            id: request_id(),
            action,
        })
        .map_err(|_| zbus::fdo::Error::Failed("Custom Spotify command queue is busy".into()))
    }

    #[derive(Clone)]
    struct Adapter {
        commands: mpsc::Sender<Command>,
        events: mpsc::Sender<Event>,
        state: watch::Receiver<AppState>,
    }

    impl RootInterface for Adapter {
        async fn raise(&self) -> zbus::fdo::Result<()> {
            self.events
                .try_send(Event::ShowWindow)
                .map_err(|_| zbus::fdo::Error::Failed("Custom Spotify event queue is busy".into()))
        }
        async fn quit(&self) -> zbus::fdo::Result<()> {
            send_command(&self.commands, Action::Quit)
        }
        async fn can_quit(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }
        async fn fullscreen(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }
        async fn set_fullscreen(&self, _: bool) -> zbus::Result<()> {
            Ok(())
        }
        async fn can_set_fullscreen(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }
        async fn can_raise(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }
        async fn has_track_list(&self) -> zbus::fdo::Result<bool> {
            Ok(false)
        }
        async fn identity(&self) -> zbus::fdo::Result<String> {
            Ok("Custom Spotify".into())
        }
        async fn desktop_entry(&self) -> zbus::fdo::Result<String> {
            Ok("custom-spotify".into())
        }
        async fn supported_uri_schemes(&self) -> zbus::fdo::Result<Vec<String>> {
            Ok(vec![])
        }
        async fn supported_mime_types(&self) -> zbus::fdo::Result<Vec<String>> {
            Ok(vec![])
        }
    }

    impl PlayerInterface for Adapter {
        async fn next(&self) -> zbus::fdo::Result<()> {
            send_command(&self.commands, Action::Next)
        }
        async fn previous(&self) -> zbus::fdo::Result<()> {
            send_command(&self.commands, Action::Previous)
        }
        async fn pause(&self) -> zbus::fdo::Result<()> {
            send_command(&self.commands, Action::Pause)
        }
        async fn play_pause(&self) -> zbus::fdo::Result<()> {
            let a = if self.state.borrow().player.status == DomainStatus::Playing {
                Action::Pause
            } else {
                Action::Resume
            };
            send_command(&self.commands, a)
        }
        async fn stop(&self) -> zbus::fdo::Result<()> {
            send_command(&self.commands, Action::Pause)
        }
        async fn play(&self) -> zbus::fdo::Result<()> {
            send_command(&self.commands, Action::Resume)
        }
        async fn seek(&self, offset: Time) -> zbus::fdo::Result<()> {
            let p = anchored_ms(&self.state.borrow());
            let target =
                (p as i128 + offset.as_micros() as i128 / 1000).clamp(0, u64::MAX as i128) as u64;
            send_command(
                &self.commands,
                Action::Seek {
                    position_ms: target,
                },
            )
        }
        async fn set_position(&self, track_id: TrackId, position: Time) -> zbus::fdo::Result<()> {
            if track_id == current_track_id(&self.state.borrow()) {
                send_command(
                    &self.commands,
                    Action::Seek {
                        position_ms: position.as_micros().max(0) as u64 / 1000,
                    },
                )?;
            }
            Ok(())
        }
        async fn open_uri(&self, _: String) -> zbus::fdo::Result<()> {
            Err(zbus::fdo::Error::NotSupported(
                "URI playback is not supported".into(),
            ))
        }
        async fn playback_status(&self) -> zbus::fdo::Result<PlaybackStatus> {
            Ok(status(self.state.borrow().player.status))
        }
        async fn loop_status(&self) -> zbus::fdo::Result<LoopStatus> {
            Ok(loop_status(self.state.borrow().player.repeat))
        }
        async fn set_loop_status(&self, value: LoopStatus) -> zbus::Result<()> {
            send_command(&self.commands, Action::SetRepeat(repeat_mode(value))).map_err(Into::into)
        }
        async fn rate(&self) -> zbus::fdo::Result<f64> {
            Ok(1.0)
        }
        async fn set_rate(&self, _: f64) -> zbus::Result<()> {
            Ok(())
        }
        async fn shuffle(&self) -> zbus::fdo::Result<bool> {
            Ok(self.state.borrow().player.shuffle)
        }
        async fn set_shuffle(&self, value: bool) -> zbus::Result<()> {
            send_command(&self.commands, Action::SetShuffle(value)).map_err(Into::into)
        }
        async fn metadata(&self) -> zbus::fdo::Result<Metadata> {
            Ok(metadata(&self.state.borrow()))
        }
        async fn volume(&self) -> zbus::fdo::Result<Volume> {
            Ok(self.state.borrow().player.volume as f64)
        }
        async fn set_volume(&self, value: Volume) -> zbus::Result<()> {
            send_command(
                &self.commands,
                Action::SetVolume(value.clamp(0.0, 1.0) as f32),
            )
            .map_err(Into::into)
        }
        async fn position(&self) -> zbus::fdo::Result<Time> {
            Ok(Time::from_millis(anchored_ms(&self.state.borrow()) as i64))
        }
        async fn minimum_rate(&self) -> zbus::fdo::Result<f64> {
            Ok(1.0)
        }
        async fn maximum_rate(&self) -> zbus::fdo::Result<f64> {
            Ok(1.0)
        }
        async fn can_go_next(&self) -> zbus::fdo::Result<bool> {
            Ok(self.state.borrow().player.current.is_some())
        }
        async fn can_go_previous(&self) -> zbus::fdo::Result<bool> {
            Ok(self.state.borrow().player.current.is_some())
        }
        async fn can_play(&self) -> zbus::fdo::Result<bool> {
            Ok(self.state.borrow().player.current.is_some())
        }
        async fn can_pause(&self) -> zbus::fdo::Result<bool> {
            Ok(self.state.borrow().player.current.is_some())
        }
        async fn can_seek(&self) -> zbus::fdo::Result<bool> {
            Ok(self.state.borrow().player.current.is_some())
        }
        async fn can_control(&self) -> zbus::fdo::Result<bool> {
            Ok(true)
        }
    }

    /// Represent loading and empty playback using MPRIS's stopped state.
    fn status(value: DomainStatus) -> PlaybackStatus {
        match value {
            DomainStatus::Playing => PlaybackStatus::Playing,
            DomainStatus::Paused => PlaybackStatus::Paused,
            _ => PlaybackStatus::Stopped,
        }
    }
    /// Translate the queue's repeat policy into MPRIS terminology.
    fn loop_status(value: RepeatMode) -> LoopStatus {
        match value {
            RepeatMode::Off => LoopStatus::None,
            RepeatMode::Queue => LoopStatus::Playlist,
            RepeatMode::One => LoopStatus::Track,
        }
    }
    /// Map a desktop repeat request to the authoritative queue policy.
    fn repeat_mode(value: LoopStatus) -> RepeatMode {
        match value {
            LoopStatus::None => RepeatMode::Off,
            LoopStatus::Playlist => RepeatMode::Queue,
            LoopStatus::Track => RepeatMode::One,
        }
    }
    /// Answer position queries without periodic desktop timers or audio polling.
    fn anchored_ms(state: &AppState) -> u64 {
        let p = &state.player;
        if p.status == DomainStatus::Playing {
            p.position_ms
                .saturating_add(
                    Instant::now()
                        .saturating_duration_since(p.observed_at)
                        .as_millis()
                        .min(u64::MAX as u128) as u64,
                )
                .min(p.current.as_ref().map_or(u64::MAX, |t| t.duration_ms))
        } else {
            p.position_ms
        }
    }
    /// Use occurrence identity so duplicate recordings have distinct MPRIS track paths.
    fn current_track_id(state: &AppState) -> TrackId {
        state
            .player
            .current_entry_id
            .and_then(|id| TrackId::try_from(format!("/com/customspotify/track/{id}")).ok())
            .or_else(|| {
                let track = state.player.current.as_ref()?;
                TrackId::try_from(format!(
                    "/com/customspotify/connect/{}_{}",
                    track.id.0, state.player.queue_revision
                ))
                .ok()
            })
            .unwrap_or(TrackId::NO_TRACK)
    }
    /// Expose the current hydrated track without fetching any additional metadata.
    fn metadata(state: &AppState) -> Metadata {
        let Some(track) = &state.player.current else {
            return Metadata::new();
        };
        let mut m = Metadata::new();
        m.set_trackid(Some(current_track_id(state)));
        m.set_length(Some(Time::from_millis(
            track.duration_ms.min(i64::MAX as u64) as i64,
        )));
        m.set_title(Some(track.name.clone()));
        m.set_artist(Some(
            track
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>(),
        ));
        if let Some(album) = &track.album {
            m.set_album(Some(album.name.clone()));
            m.set_art_url(album.artwork_url.clone());
        }
        m
    }

    /// Runs the MPRIS service until the application state channel closes.
    pub async fn run(
        commands: mpsc::Sender<Command>,
        events: mpsc::Sender<Event>,
        mut state: watch::Receiver<AppState>,
    ) -> Result<(), AppError> {
        let adapter = Adapter {
            commands: commands.clone(),
            events,
            state: state.clone(),
        };
        let server = match Server::new("com.customspotify.CustomSpotify", adapter).await {
            Ok(s) => s,
            Err(e) => {
                return Err(error(format!("MPRIS registration failed: {e}")));
            }
        };
        let mut old = state.borrow().clone();
        while state.changed().await.is_ok() {
            let new = state.borrow().clone();
            let props = changed_properties(&old, &new);
            if new.player.current_entry_id == old.player.current_entry_id
                && new.player.seek_revision != old.player.seek_revision
            {
                let _ = server
                    .emit(Signal::Seeked {
                        position: Time::from_millis(
                            new.player.position_ms.min(i64::MAX as u64) as i64
                        ),
                    })
                    .await;
            }
            if !props.is_empty() {
                server
                    .properties_changed(props)
                    .await
                    .map_err(|e| error(format!("MPRIS state publication failed: {e}")))?;
            }
            old = new;
        }
        Ok(())
    }

    /// Publish only changed values, avoiding metadata broadcasts for unrelated UI/cache updates.
    fn changed_properties(old: &AppState, new: &AppState) -> Vec<Property> {
        let mut properties = Vec::new();
        if old.player.status != new.player.status {
            properties.push(Property::PlaybackStatus(status(new.player.status)));
        }
        let new_metadata = metadata(new);
        if metadata(old) != new_metadata {
            properties.push(Property::Metadata(new_metadata));
        }
        if old.player.repeat != new.player.repeat {
            properties.push(Property::LoopStatus(loop_status(new.player.repeat)));
        }
        if old.player.shuffle != new.player.shuffle {
            properties.push(Property::Shuffle(new.player.shuffle));
        }
        if old.player.volume != new.player.volume {
            properties.push(Property::Volume(new.player.volume as f64));
        }
        if old.player.current.is_some() != new.player.current.is_some() {
            let available = new.player.current.is_some();
            properties.extend([
                Property::CanPlay(available),
                Property::CanPause(available),
                Property::CanSeek(available),
                Property::CanGoNext(available),
                Property::CanGoPrevious(available),
            ]);
        }
        properties
    }

    /// Translate a desktop-service error into non-secret display text.
    fn error(message: String) -> AppError {
        AppError {
            kind: ErrorKind::Unavailable,
            message,
            retry_after_seconds: None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn mappings_round_trip() {
            for r in [RepeatMode::Off, RepeatMode::Queue, RepeatMode::One] {
                assert_eq!(repeat_mode(loop_status(r)), r);
            }
        }

        #[test]
        fn unrelated_state_does_not_broadcast_player_properties() {
            let state = crate::backend::initial_state();
            let mut changed = state.clone();
            changed.cache_used_bytes = 4096;
            assert!(changed_properties(&state, &changed).is_empty());
            changed.player.shuffle = true;
            assert_eq!(changed_properties(&state, &changed).len(), 1);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::run;

/// Other platforms currently have no desktop media adapter.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub async fn run(
    _: tokio::sync::mpsc::Sender<crate::contracts::Command>,
    _: tokio::sync::mpsc::Sender<crate::contracts::Event>,
    _: tokio::sync::watch::Receiver<crate::domain::AppState>,
) -> Result<(), crate::contracts::AppError> {
    Ok(())
}
