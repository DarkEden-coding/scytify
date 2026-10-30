//! Local listening stats: how long each song plays, recorded on this device, and the
//! totals and rankings the stats page shows.
use super::storage::{self, Storage};
use crate::{contracts::*, domain::*};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Listening shorter than this is not stored.
const MIN_STORED_MS: u64 = 1000;
/// How often a running listen is written, so an unclean exit loses at most this much.
const SAVE_EVERY: Duration = Duration::from_secs(30);
/// Longest gap between observations still counted as listening, so a suspended machine
/// does not count the time it slept.
const MAX_STEP: Duration = Duration::from_secs(2);
/// Listening that counts as a play: 30 seconds, or half of a shorter song.
const PLAY_MS: u64 = 30_000;
/// A jump back to within this much of the start, from further in, starts a new listen.
const RESTART_MS: u64 = 5_000;
const TOP_TRACKS: usize = 10;
const TOP_ARTISTS: usize = 8;
const DAY_SECONDS: i64 = 86_400;

/// The song being listened to now.
struct Listen {
    track: Track,
    started_at: i64,
    listened_ms: u64,
    position_ms: u64,
    saved_at: Instant,
    saved_ms: u64,
    /// Where this listen is stored, once it has been.
    row: Option<i64>,
}

impl Listen {
    fn counted(&self) -> bool {
        let threshold = match self.track.duration_ms {
            0 => PLAY_MS,
            duration => PLAY_MS.min(duration / 2),
        };
        self.listened_ms >= threshold
    }

    fn save(&mut self, store: &Storage, now: Instant) {
        self.saved_at = now;
        if self.listened_ms < MIN_STORED_MS || self.listened_ms == self.saved_ms {
            return;
        }
        self.saved_ms = self.listened_ms;
        let counted = self.counted();
        match store.save_listen(
            self.row,
            &self.track,
            self.started_at,
            self.listened_ms,
            counted,
        ) {
            Ok(row) => self.row = Some(row),
            Err(error) => log::warn!("could not record listening time: {error}"),
        }
    }
}

/// Follows the player and accumulates the time each song actually plays.
#[derive(Default)]
pub(super) struct Recorder {
    listen: Option<Listen>,
    last_observed: Option<Instant>,
}

impl Recorder {
    /// Account for the time since the previous observation. Call it often (every tick).
    pub fn observe(
        &mut self,
        player: &PlayerState,
        position_ms: u64,
        now: Instant,
        store: &Storage,
    ) {
        let step = self
            .last_observed
            .replace(now)
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        let current = player.current.as_ref();
        let restarted = self
            .listen
            .as_ref()
            .is_some_and(|l| position_ms < RESTART_MS && l.position_ms >= position_ms + RESTART_MS);
        if restarted || current.map(|t| &t.id) != self.listen.as_ref().map(|l| &l.track.id) {
            self.finish(store, now);
            self.listen = current.map(|track| Listen {
                track: track.clone(),
                started_at: storage::now(),
                listened_ms: 0,
                position_ms,
                saved_at: now,
                saved_ms: 0,
                row: None,
            });
        }
        let (Some(listen), Some(track)) = (self.listen.as_mut(), current) else {
            return;
        };
        // Metadata of a song started elsewhere can arrive after it starts.
        listen.track.clone_from(track);
        listen.position_ms = position_ms;
        if player.status == PlaybackStatus::Playing {
            listen.listened_ms += step.min(MAX_STEP).as_millis() as u64;
        }
        if now.saturating_duration_since(listen.saved_at) >= SAVE_EVERY {
            listen.save(store, now);
        }
    }

    /// Write the current listen; call before the backend stops.
    pub fn finish(&mut self, store: &Storage, now: Instant) {
        if let Some(mut listen) = self.listen.take() {
            listen.save(store, now);
        }
    }
}

/// Totals and rankings over `range`, counted back from `now` (seconds since the epoch).
pub(super) fn summary(
    store: &Storage,
    range: StatsRange,
    now: i64,
) -> Result<ListeningStats, String> {
    let since = range
        .days()
        .map_or(i64::MIN, |days| now - days * DAY_SECONDS);
    let mut stats = ListeningStats {
        range,
        by_hour: store.listen_hours(since)?,
        ..ListeningStats::default()
    };
    let mut tracks: Vec<RankedTrack> = Vec::new();
    let mut artists: HashMap<SpotifyId, RankedArtist> = HashMap::new();
    for (track, listened_ms, plays) in store.listen_totals(since)? {
        stats.listened_ms += listened_ms;
        stats.plays += plays;
        if plays == 0 {
            continue;
        }
        for artist in &track.artists {
            let entry = artists
                .entry(artist.id.clone())
                .or_insert_with(|| RankedArtist {
                    artist: artist.clone(),
                    plays: 0,
                    listened_ms: 0,
                });
            entry.plays += plays;
            entry.listened_ms += listened_ms;
        }
        tracks.push(RankedTrack {
            track,
            plays,
            listened_ms,
        });
    }
    stats.songs = tracks.len() as u64;
    stats.artists = artists.len() as u64;
    tracks.sort_by(|a, b| {
        (b.plays, b.listened_ms, &a.track.name).cmp(&(a.plays, a.listened_ms, &b.track.name))
    });
    tracks.truncate(TOP_TRACKS);
    stats.top_tracks = tracks;
    let mut artists: Vec<RankedArtist> = artists.into_values().collect();
    artists.sort_by(|a, b| {
        (b.listened_ms, b.plays, &a.artist.name).cmp(&(a.listened_ms, a.plays, &b.artist.name))
    });
    artists.truncate(TOP_ARTISTS);
    stats.top_artists = artists;
    let (today, days) = store.listen_days(now)?;
    (stats.streak_days, stats.longest_streak_days) = streaks(today, &days);
    Ok(stats)
}

/// The current and longest runs of consecutive days in `days` (sorted, distinct). The
/// current run still counts until a full day passes without listening.
fn streaks(today: i64, days: &[i64]) -> (u64, u64) {
    let mut run: u64 = 0;
    let mut longest: u64 = 0;
    let mut previous: Option<i64> = None;
    for &day in days {
        run = if previous == Some(day - 1) {
            run + 1
        } else {
            1
        };
        longest = longest.max(run);
        previous = Some(day);
    }
    let current = match previous {
        Some(last) if last >= today - 1 => run,
        _ => 0,
    };
    (current, longest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: char, artist: &str, duration_ms: u64) -> Track {
        Track {
            id: SpotifyId(id.to_string().repeat(22)),
            name: format!("Song {id}"),
            artists: vec![Artist {
                id: SpotifyId(artist.repeat(22)),
                name: format!("Artist {artist}"),
                artwork_url: None,
            }],
            album: None,
            duration_ms,
            playable: true,
            source_index: None,
        }
    }

    fn playing(track: &Track) -> PlayerState {
        let mut player = crate::backend::initial_state().player;
        player.current = Some(track.clone());
        player.status = PlaybackStatus::Playing;
        player
    }

    #[test]
    fn streaks_count_consecutive_days_until_a_day_is_missed() {
        assert_eq!(streaks(10, &[]), (0, 0));
        assert_eq!(streaks(10, &[3, 4, 5, 8, 9, 10]), (3, 3));
        assert_eq!(streaks(10, &[1, 2, 3, 4, 9]), (1, 4));
        assert_eq!(streaks(10, &[7, 8]), (0, 2));
    }

    /// Only time spent playing counts, skips are kept as time without a play, and the
    /// summary ranks songs by plays and artists by time.
    #[test]
    fn recorded_listening_is_summarized() {
        let root = std::env::temp_dir().join(format!("scytify-stats-{}", rand::random::<u64>()));
        let store = Storage::open_at(&root).unwrap();
        let a = track('a', "x", 200_000);
        let b = track('b', "y", 200_000);
        let mut recorder = Recorder::default();
        let mut at = Instant::now();
        // One observation per second at `position_ms` into the song.
        let mut play = |player: &PlayerState, position_ms: u64, seconds: u64| {
            for _ in 0..seconds {
                at += Duration::from_secs(1);
                recorder.observe(player, position_ms, at, &store);
            }
        };
        // Song a plays 40 s, pauses for 100 s, then b is skipped after 10 s.
        play(&playing(&a), 60_000, 40);
        let mut paused = playing(&a);
        paused.status = PlaybackStatus::Paused;
        play(&paused, 60_000, 100);
        play(&playing(&b), 60_000, 10);
        // Song a again, then started over from the beginning: two more plays.
        play(&playing(&a), 60_000, 35);
        play(&playing(&a), 0, 1);
        play(&playing(&a), 60_000, 35);
        recorder.finish(&store, at);

        let stats = summary(&store, StatsRange::Week, storage::now()).unwrap();
        assert_eq!(stats.plays, 3);
        assert_eq!(stats.songs, 1);
        assert_eq!(stats.artists, 1);
        // 39 s of the first 40 count: the very first observation has nothing to measure from.
        assert_eq!(stats.listened_ms, 120_000);
        assert_eq!(stats.by_hour.iter().sum::<u64>(), 120_000);
        assert_eq!(stats.top_tracks[0].track.id, a.id);
        assert_eq!(stats.top_tracks[0].plays, 3);
        assert_eq!(stats.top_artists[0].listened_ms, 110_000);
        assert_eq!((stats.streak_days, stats.longest_streak_days), (1, 1));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
