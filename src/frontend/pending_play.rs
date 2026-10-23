//! Optimistic now-playing: a clicked track shows in the player at once, before Spotify
//! confirms the load, so the click feels instant even while audio is still buffering.
use crate::{contracts::RequestId, domain::*};
use std::time::{Duration, Instant};

/// Longest the clicked track is shown without the backend confirming it.
const PATIENCE: Duration = Duration::from_secs(5);

pub(super) struct PendingPlay {
    track: Track,
    pub(super) request: RequestId,
    at: Instant,
    /// The backend's own view, restored if the request fails.
    backend: Backend,
}

struct Backend {
    current: Option<Track>,
    status: PlaybackStatus,
    position_ms: u64,
    observed_at: Instant,
}

impl Backend {
    fn of(player: &PlayerState) -> Self {
        Self {
            current: player.current.clone(),
            status: player.status,
            position_ms: player.position_ms,
            observed_at: player.observed_at,
        }
    }
}

impl PendingPlay {
    pub(super) fn new(track: Track, request: RequestId, player: &PlayerState) -> Self {
        Self {
            track,
            request,
            at: Instant::now(),
            backend: Backend::of(player),
        }
    }

    /// Records a backend update; returns whether the clicked track should still be shown.
    pub(super) fn observe(&mut self, player: &PlayerState) -> bool {
        self.backend = Backend::of(player);
        let confirmed: bool = player
            .current
            .as_ref()
            .is_some_and(|t| t.id == self.track.id)
            && player.status != PlaybackStatus::Loading;
        !confirmed && self.at.elapsed() < PATIENCE
    }

    /// Shows the clicked track as playing from its start.
    pub(super) fn show(&self, player: &mut PlayerState) {
        player.current = Some(self.track.clone());
        player.status = PlaybackStatus::Playing;
        player.position_ms = 0;
        player.observed_at = self.at;
    }

    /// Puts back what the backend last reported.
    pub(super) fn restore(self, player: &mut PlayerState) {
        player.current = self.backend.current;
        player.status = self.backend.status;
        player.position_ms = self.backend.position_ms;
        player.observed_at = self.backend.observed_at;
    }
}
