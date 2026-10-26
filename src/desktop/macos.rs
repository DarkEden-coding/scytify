//! MediaPlayer registration lives on AppKit's main queue. Callbacks only enqueue backend commands.
use crate::contracts::{Action, AppError, Command, Event, RequestId};
use crate::domain::{AppState, PlaybackStatus};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSMutableDictionary, NSNumber, NSString};
use objc2_media_player::*;
use std::{
    cell::RefCell,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
use tokio::sync::{mpsc, watch};

static NEXT_ID: AtomicU64 = AtomicU64::new(1u64 << 63);

thread_local! {
    static ADAPTER: RefCell<Option<Adapter>> = const { RefCell::new(None) };
}

#[derive(Clone, Debug, PartialEq)]
struct NowPlaying {
    title: String,
    artist: String,
    album: String,
    id: String,
    duration_ms: u64,
    position_ms: u64,
    observed_at: Instant,
    status: PlaybackStatus,
}

impl NowPlaying {
    fn from_state(state: &AppState) -> Option<Self> {
        // A remote Connect device must not claim this Mac's Now Playing session.
        if state.connect.is_remote() {
            return None;
        }
        let player = &state.player;
        let track = player.current.as_ref()?;
        if player.status == PlaybackStatus::Stopped {
            return None;
        }
        Some(Self {
            title: track.name.clone(),
            artist: track
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            album: track
                .album
                .as_ref()
                .map_or(String::new(), |a| a.name.clone()),
            id: track.id.0.clone(),
            duration_ms: track.duration_ms,
            position_ms: player.position_ms,
            observed_at: player.observed_at,
            status: player.status,
        })
    }

    fn position_seconds(&self) -> f64 {
        let elapsed = if self.status == PlaybackStatus::Playing {
            self.observed_at.elapsed().as_secs_f64()
        } else {
            0.0
        };
        (self.position_ms as f64 / 1000.0 + elapsed).min(self.duration_ms as f64 / 1000.0)
    }
}

struct Adapter {
    center: Retained<MPNowPlayingInfoCenter>,
    targets: Vec<(Retained<MPRemoteCommand>, Retained<AnyObject>)>,
    published: Option<NowPlaying>,
}

impl Adapter {
    fn new(commands: mpsc::Sender<Command>, state: watch::Receiver<AppState>) -> Self {
        // SAFETY: MediaPlayer objects are created, used, and released only on the main queue.
        unsafe {
            let remote = MPRemoteCommandCenter::sharedCommandCenter();
            let mut adapter = Self {
                center: MPNowPlayingInfoCenter::defaultCenter(),
                targets: Vec::new(),
                published: None,
            };
            for (command, action) in [
                (remote.playCommand(), Some(Action::Resume)),
                (remote.pauseCommand(), Some(Action::Pause)),
                (remote.togglePlayPauseCommand(), None),
                (remote.stopCommand(), Some(Action::Pause)),
                (remote.nextTrackCommand(), Some(Action::Next)),
                (remote.previousTrackCommand(), Some(Action::Previous)),
            ] {
                let commands = commands.clone();
                let state = state.clone();
                let handler =
                    RcBlock::new(move |_event: std::ptr::NonNull<MPRemoteCommandEvent>| {
                        let state = state.borrow();
                        if NowPlaying::from_state(&state).is_none() {
                            return MPRemoteCommandHandlerStatus::NoActionableNowPlayingItem;
                        }
                        let action = action.clone().unwrap_or_else(|| {
                            if state.player.status == PlaybackStatus::Playing {
                                Action::Pause
                            } else {
                                Action::Resume
                            }
                        });
                        send_command(&commands, action)
                    });
                let target = command.addTargetWithHandler(&handler);
                command.setEnabled(false);
                adapter.targets.push((command, target));
            }
            adapter
        }
    }

    fn publish(&mut self, now: Option<NowPlaying>) {
        if self.published == now {
            return;
        }
        // SAFETY: Dictionary keys and values use MediaPlayer's documented types. The framework
        // copies the dictionary; all temporary Objective-C values remain alive during each call.
        unsafe {
            for (command, _) in &self.targets {
                command.setEnabled(now.is_some());
            }
            if let Some(now) = &now {
                let info = NSMutableDictionary::<NSString, AnyObject>::new();
                info.insert(MPMediaItemPropertyTitle, &*NSString::from_str(&now.title));
                info.insert(MPMediaItemPropertyArtist, &*NSString::from_str(&now.artist));
                info.insert(
                    MPMediaItemPropertyAlbumTitle,
                    &*NSString::from_str(&now.album),
                );
                info.insert(
                    MPNowPlayingInfoPropertyExternalContentIdentifier,
                    &*NSString::from_str(&now.id),
                );
                info.insert(
                    MPMediaItemPropertyPlaybackDuration,
                    &*NSNumber::new_f64(now.duration_ms as f64 / 1000.0),
                );
                info.insert(
                    MPNowPlayingInfoPropertyElapsedPlaybackTime,
                    &*NSNumber::new_f64(now.position_seconds()),
                );
                info.insert(
                    MPNowPlayingInfoPropertyMediaType,
                    &*NSNumber::new_u64(MPNowPlayingInfoMediaType::Audio.0 as u64),
                );
                info.insert(
                    MPNowPlayingInfoPropertyDefaultPlaybackRate,
                    &*NSNumber::new_f64(1.0),
                );
                let playing = now.status == PlaybackStatus::Playing;
                info.insert(
                    MPNowPlayingInfoPropertyPlaybackRate,
                    &*NSNumber::new_f64(if playing { 1.0 } else { 0.0 }),
                );
                self.center.setNowPlayingInfo(Some(&info));
                self.center.setPlaybackState(if playing {
                    MPNowPlayingPlaybackState::Playing
                } else {
                    MPNowPlayingPlaybackState::Paused
                });
            } else {
                self.center
                    .setPlaybackState(MPNowPlayingPlaybackState::Stopped);
                self.center.setNowPlayingInfo(None);
            }
        }
        self.published = now;
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        // SAFETY: These are the exact opaque target tokens returned by addTargetWithHandler.
        unsafe {
            for (command, target) in &self.targets {
                command.removeTarget(Some(target));
                command.setEnabled(false);
            }
            self.center
                .setPlaybackState(MPNowPlayingPlaybackState::Stopped);
            self.center.setNowPlayingInfo(None);
        }
    }
}

fn send_command(commands: &mpsc::Sender<Command>, action: Action) -> MPRemoteCommandHandlerStatus {
    let id = RequestId(NEXT_ID.fetch_add(1, Ordering::Relaxed) | (1u64 << 63));
    match commands.try_send(Command { id, action }) {
        Ok(()) => MPRemoteCommandHandlerStatus::Success,
        Err(_) => MPRemoteCommandHandlerStatus::CommandFailed,
    }
}

struct Registration;
impl Drop for Registration {
    fn drop(&mut self) {
        DispatchQueue::main().exec_async(|| {
            ADAPTER.with(|adapter| {
                adapter.borrow_mut().take();
            })
        });
    }
}

pub async fn run(
    commands: mpsc::Sender<Command>,
    _: mpsc::Sender<Event>,
    mut state: watch::Receiver<AppState>,
) -> Result<(), AppError> {
    let callback_state = state.clone();
    DispatchQueue::main().exec_async(move || {
        ADAPTER
            .with(|adapter| *adapter.borrow_mut() = Some(Adapter::new(commands, callback_state)));
    });
    let _registration = Registration;
    loop {
        let now = NowPlaying::from_state(&state.borrow_and_update());
        DispatchQueue::main().exec_async(move || {
            ADAPTER.with(|adapter| {
                if let Some(adapter) = adapter.borrow_mut().as_mut() {
                    adapter.publish(now);
                }
            });
        });
        if state.changed().await.is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_commands_reach_backend_and_report_backpressure() {
        let (commands, mut received) = mpsc::channel(1);
        assert_eq!(
            send_command(&commands, Action::Next),
            MPRemoteCommandHandlerStatus::Success
        );
        assert_eq!(
            send_command(&commands, Action::Pause),
            MPRemoteCommandHandlerStatus::CommandFailed
        );
        let command = received.try_recv().unwrap();
        assert!(matches!(command.action, Action::Next));
        assert_ne!(command.id.0 & (1u64 << 63), 0);
        drop(received);
        assert_eq!(
            send_command(&commands, Action::Resume),
            MPRemoteCommandHandlerStatus::CommandFailed
        );
    }

    #[test]
    fn remote_connect_playback_does_not_claim_mac_media_controls() {
        let mut state = crate::backend::initial_state();
        state.player.current = Some(crate::domain::Track {
            id: crate::domain::SpotifyId("track".into()),
            name: "Song".into(),
            artists: vec![],
            album: None,
            duration_ms: 60_000,
            playable: true,
            source_index: None,
        });
        state.player.status = PlaybackStatus::Playing;
        state.player.position_ms = 59_000;
        state.player.observed_at = Instant::now() - std::time::Duration::from_secs(5);
        assert_eq!(
            NowPlaying::from_state(&state).unwrap().position_seconds(),
            60.0
        );
        state.player.status = PlaybackStatus::Paused;
        assert_eq!(
            NowPlaying::from_state(&state).unwrap().position_seconds(),
            59.0
        );
        state.connect.active_device_id = Some("another-device".into());
        assert!(NowPlaying::from_state(&state).is_none());
        state.connect.active_device_id = None;
        state.player.status = PlaybackStatus::Stopped;
        assert!(NowPlaying::from_state(&state).is_none());
    }
}
