use crate::domain::{
    PlayerState, PlaylistSnapshot, QueueEntry, RepeatMode, SessionSnapshot, SpotifyId, Track,
};
use rand::{SeedableRng, rngs::StdRng, seq::SliceRandom};

/// Deterministic queue reducer; occurrence IDs, rather than track IDs, define identity.
#[derive(Clone)]
pub(crate) struct Queue {
    pub snapshot: SessionSnapshot,
    source_order: Vec<QueueEntry>,
    rng: StdRng,
}

impl Queue {
    /// Creates an empty queue using `seed` only to make shuffle reproducible in tests/checkpoints.
    pub(crate) fn empty(seed: u64) -> Self {
        Self::restore(
            SessionSnapshot {
                version: 1,
                source: None,
                current: None,
                remaining: vec![],
                history: vec![],
                explicit_queue: vec![],
                play_next_count: 0,
                shuffle: false,
                repeat: RepeatMode::Off,
                position_ms: 0,
                volume: 1.0,
                next_entry_id: 1,
            },
            seed,
        )
        .unwrap()
    }

    /// Restores a validated checkpoint. Playback is always restored paused by its caller.
    pub(crate) fn restore(s: SessionSnapshot, seed: u64) -> Result<Self, &'static str> {
        if s.version != 1
            || !s.volume.is_finite()
            || !(0.0..=1.0).contains(&s.volume)
            || s.play_next_count > s.explicit_queue.len()
        {
            return Err("invalid queue checkpoint");
        }
        let source_order = s
            .source
            .as_ref()
            .map(|x| x.entries.clone())
            .unwrap_or_default();
        let mut ids = std::collections::HashSet::new();
        for e in source_order.iter().chain(s.explicit_queue.iter()) {
            if e.entry_id == 0 || !ids.insert(e.entry_id) {
                return Err("duplicate queue occurrence");
            }
        }
        Ok(Self {
            snapshot: s,
            source_order,
            rng: StdRng::seed_from_u64(seed),
        })
    }

    /// Atomically replaces membership after an upstream snapshot has been completely gathered.
    pub(crate) fn activate(
        &mut self,
        playlist_id: SpotifyId,
        snapshot_id: String,
        tracks: Vec<SpotifyId>,
        start: usize,
    ) -> Result<(), &'static str> {
        if start >= tracks.len() && !tracks.is_empty() {
            return Err("start offset is outside source");
        }
        let mut entries = Vec::with_capacity(tracks.len());
        for track_id in tracks {
            entries.push(self.new_entry(track_id)?);
        }
        let current = entries.get(start).cloned();
        self.source_order = entries.clone();
        self.snapshot.source = Some(PlaylistSnapshot {
            playlist_id,
            snapshot_id,
            entries: entries.clone(),
        });
        self.snapshot.current = current;
        self.snapshot.remaining = entries.into_iter().skip(start.saturating_add(1)).collect();
        self.snapshot.history.clear();
        self.snapshot.explicit_queue.clear();
        self.snapshot.play_next_count = 0;
        self.snapshot.position_ms = 0;
        if self.snapshot.shuffle {
            self.snapshot.remaining.shuffle(&mut self.rng);
        }
        Ok(())
    }

    /// Creates a single-track source.
    pub(crate) fn activate_track(&mut self, id: SpotifyId) -> Result<(), &'static str> {
        let e = self.new_entry(id)?;
        self.source_order = vec![e.clone()];
        self.snapshot.source = None;
        self.snapshot.current = Some(e);
        self.snapshot.position_ms = 0;
        self.snapshot.remaining.clear();
        self.snapshot.history.clear();
        self.snapshot.explicit_queue.clear();
        self.snapshot.play_next_count = 0;
        Ok(())
    }

    fn new_entry(&mut self, track_id: SpotifyId) -> Result<QueueEntry, &'static str> {
        let id = self.snapshot.next_entry_id;
        self.snapshot.next_entry_id = id.checked_add(1).ok_or("queue ID exhausted")?;
        Ok(QueueEntry {
            entry_id: id,
            track_id,
        })
    }

    /// Adds explicit items, preserving FIFO and PlayNext-prefix priority.
    pub(crate) fn enqueue(&mut self, ids: Vec<SpotifyId>, next: bool) -> Result<(), &'static str> {
        if ids.len() > 100 {
            return Err("at most 100 tracks may be queued");
        }
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            entries.push(self.new_entry(id)?);
        }
        if next {
            let count = entries.len();
            let at = self.snapshot.play_next_count;
            self.snapshot.explicit_queue.splice(at..at, entries);
            self.snapshot.play_next_count += count;
        } else {
            self.snapshot.explicit_queue.extend(entries);
        }
        Ok(())
    }

    /// Advances manually or after one generation-checked natural completion.
    pub(crate) fn next(&mut self, natural: bool) -> Option<QueueEntry> {
        if natural && self.snapshot.repeat == RepeatMode::One {
            self.snapshot.position_ms = 0;
            return self.snapshot.current.clone();
        }
        if let Some(c) = self.snapshot.current.take() {
            self.snapshot.history.push(c);
        }
        let n = if !self.snapshot.explicit_queue.is_empty() {
            if self.snapshot.play_next_count > 0 {
                self.snapshot.play_next_count -= 1;
            }
            Some(self.snapshot.explicit_queue.remove(0))
        } else {
            self.snapshot.remaining.pop_front_compat()
        };
        let n = n.or_else(|| {
            if self.snapshot.repeat != RepeatMode::Queue {
                return None;
            }
            let last = self.snapshot.history.last().map(|x| x.entry_id);
            let mut cycle = self.source_order.clone();
            if self.snapshot.shuffle {
                cycle.shuffle(&mut self.rng);
                if cycle.len() > 1 && cycle.first().map(|x| x.entry_id) == last {
                    cycle.swap(0, 1);
                }
            }
            self.snapshot.remaining = cycle;
            self.snapshot.remaining.pop_front_compat()
        });
        self.snapshot.current = n.clone();
        self.snapshot.position_ms = 0;
        n
    }

    /// Implements restart-after-three-seconds, otherwise history navigation.
    pub(crate) fn previous(&mut self) -> Option<QueueEntry> {
        if self.snapshot.position_ms > 3000 {
            self.snapshot.position_ms = 0;
            return self.snapshot.current.clone();
        }
        let prev = self.snapshot.history.pop()?;
        if let Some(c) = self.snapshot.current.replace(prev.clone()) {
            self.snapshot.remaining.insert(0, c);
        }
        self.snapshot.position_ms = 0;
        Some(prev)
    }

    /// Mirrors the local Connect player only for restart checkpoints. Spirc remains the
    /// playback owner. Keep a known full source tail when its preview still agrees.
    pub(crate) fn observe_connect(
        &mut self,
        player: &PlayerState,
        preview: &[Track],
    ) -> Result<bool, &'static str> {
        let Some(track) = player.current.as_ref() else {
            return Ok(false);
        };
        let mut changed = false;
        if self.snapshot.current.as_ref().map(|entry| &entry.track_id) != Some(&track.id) {
            let steps = self
                .snapshot
                .explicit_queue
                .iter()
                .chain(self.snapshot.remaining.iter())
                .position(|entry| entry.track_id == track.id);
            if let Some(steps) = steps {
                let count = steps + 1;
                let explicit = count.min(self.snapshot.explicit_queue.len());
                let mut consumed = self
                    .snapshot
                    .explicit_queue
                    .drain(..explicit)
                    .collect::<Vec<_>>();
                self.snapshot.play_next_count =
                    self.snapshot.play_next_count.saturating_sub(explicit);
                consumed.extend(self.snapshot.remaining.drain(..count - explicit));
                self.snapshot.history.extend(self.snapshot.current.take());
                self.snapshot.current = consumed.pop();
                self.snapshot.history.extend(consumed);
            } else {
                self.activate_track(track.id.clone())?;
            }
            changed = true;
        }
        let known = self
            .snapshot
            .explicit_queue
            .iter()
            .chain(self.snapshot.remaining.iter())
            .take(preview.len())
            .map(|entry| &entry.track_id);
        if known.ne(preview.iter().map(|track| &track.id)) {
            let entries = preview
                .iter()
                .map(|track| self.new_entry(track.id.clone()))
                .collect::<Result<Vec<_>, _>>()?;
            self.snapshot.explicit_queue.clear();
            self.snapshot.play_next_count = 0;
            self.snapshot.remaining = entries;
            changed = true;
        }
        changed |= self.snapshot.shuffle != player.shuffle
            || self.snapshot.repeat != player.repeat
            || self.snapshot.volume != player.volume;
        self.snapshot.position_ms = player.position_ms;
        self.snapshot.shuffle = player.shuffle;
        self.snapshot.repeat = player.repeat;
        self.snapshot.volume = player.volume;
        Ok(changed)
    }

    /// Toggles shuffle while retaining current and consumed occurrences.
    pub(crate) fn set_shuffle(&mut self, enabled: bool) {
        if self.snapshot.shuffle == enabled {
            return;
        }
        self.snapshot.shuffle = enabled;
        if enabled {
            self.snapshot.remaining.shuffle(&mut self.rng);
        } else {
            let left: std::collections::HashSet<_> =
                self.snapshot.remaining.iter().map(|e| e.entry_id).collect();
            self.snapshot.remaining = self
                .source_order
                .iter()
                .filter(|e| left.contains(&e.entry_id))
                .cloned()
                .collect();
        }
    }
}

trait PopFront {
    fn pop_front_compat(&mut self) -> Option<QueueEntry>;
}
impl PopFront for Vec<QueueEntry> {
    fn pop_front_compat(&mut self) -> Option<QueueEntry> {
        if self.is_empty() {
            None
        } else {
            Some(self.remove(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(s: &str) -> SpotifyId {
        SpotifyId(s.into())
    }

    #[test]
    fn explicit_priority_and_duplicate_occurrences() {
        let mut q = Queue::empty(1);
        q.activate(
            id("playlist00000000000000"),
            "snap".into(),
            vec![
                id("track0000000000000001"),
                id("track0000000000000001"),
                id("track0000000000000002"),
            ],
            0,
        )
        .unwrap();
        q.enqueue(
            vec![id("next00000000000000001"), id("next00000000000000002")],
            true,
        )
        .unwrap();
        q.enqueue(vec![id("later0000000000000001")], false).unwrap();
        assert_eq!(q.next(false).unwrap().track_id.0, "next00000000000000001");
        assert_eq!(q.next(false).unwrap().track_id.0, "next00000000000000002");
        assert_eq!(q.next(false).unwrap().track_id.0, "later0000000000000001");
        assert_ne!(q.source_order[0].entry_id, q.source_order[1].entry_id);
    }

    #[test]
    fn shuffle_toggle_preserves_current_and_restores_remaining_source_order() {
        let mut q = Queue::empty(7);
        let tracks = (0..8).map(|i| id(&format!("track{i:017}"))).collect();
        q.activate(id("playlist00000000000000"), "s".into(), tracks, 2)
            .unwrap();
        let current = q.snapshot.current.as_ref().unwrap().entry_id;
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(q.snapshot.current.as_ref().unwrap().entry_id, current);
        assert!(
            q.snapshot
                .remaining
                .windows(2)
                .all(|w| w[0].entry_id < w[1].entry_id)
        );
    }

    #[test]
    fn connect_checkpoint_follows_advancement_and_preserves_known_source_tail() {
        let mut q = Queue::empty(1);
        let tracks = (0..80)
            .map(|i| id(&format!("track{i:017}")))
            .collect::<Vec<_>>();
        q.activate(
            id("playlist00000000000000"),
            "snap".into(),
            tracks.clone(),
            0,
        )
        .unwrap();
        let mut player = crate::backend::initial_state().player;
        let track = |id: SpotifyId| Track {
            id,
            name: "Track".into(),
            artists: vec![],
            album: None,
            duration_ms: 90000,
            playable: true,
            source_index: None,
        };
        player.current = Some(track(tracks[5].clone()));
        player.position_ms = 12000;
        let preview = tracks[6..56].iter().cloned().map(track).collect::<Vec<_>>();
        assert!(q.observe_connect(&player, &preview).unwrap());
        assert_eq!(q.snapshot.current.as_ref().unwrap().track_id, tracks[5]);
        assert_eq!(q.snapshot.remaining.len(), 74);
        assert_eq!(q.snapshot.history.len(), 5);
        assert_eq!(q.snapshot.position_ms, 12000);
        let restored = Queue::restore(q.snapshot.clone(), 1).unwrap();
        assert_eq!(restored.snapshot.current.unwrap().track_id, tracks[5]);
        player.current = Some(track(id("new0000000000000000001")));
        let preview = vec![track(id("new0000000000000000002"))];
        assert!(q.observe_connect(&player, &preview).unwrap());
        assert_eq!(q.snapshot.remaining.len(), 1);
        assert!(q.snapshot.explicit_queue.is_empty());
    }

    #[test]
    fn corrupt_checkpoint_is_rejected() {
        let mut q = Queue::empty(1);
        q.snapshot.volume = f32::NAN;
        assert!(Queue::restore(q.snapshot, 1).is_err());
    }
}
