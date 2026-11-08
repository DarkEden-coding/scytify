//! "Only songs with lyrics". Whether a song has lyrics is known only by asking the lyrics
//! provider, so while the filter is on the loaded songs are checked a few at a time and a
//! song appears once it is known to have lyrics. The backend caches every answer on disk.
use super::*;
use std::time::Instant;

/// Lyrics lookups in flight at once. The backend refuses new work beyond four jobs, and the
/// lyrics panel needs one of them.
const MAX_PROBES: usize = 2;
/// Longest pause the filter takes after a failure that resolves itself.
const MAX_PAUSE: Duration = Duration::from_secs(600);

#[derive(Default)]
pub(super) struct LyricsFilter {
    pub(super) only: bool,
    /// Songs checked so far: whether the provider has lyrics for them.
    known: HashMap<SpotifyId, bool>,
    probes: HashMap<RequestId, SpotifyId>,
    /// Songs of the current list before this index are known or being checked.
    cursor: usize,
    /// Why checking stopped; toggling the filter again retries.
    pub(super) error: Option<String>,
    /// Checking is paused until then after a rate limit, busy backend or dropped connection.
    pub(super) retry_at: Option<Instant>,
}

impl LyricsFilter {
    /// Whether `track` passes the filter: it has lyrics, or the filter is off.
    pub(super) fn keeps(&self, track: &Track) -> bool {
        !self.only || self.known.get(&track.id) == Some(&true)
    }

    /// The list changed under the cursor; check it again from the start.
    pub(super) fn restart(&mut self) {
        self.cursor = 0;
    }

    pub(super) fn set_only(&mut self, only: bool) {
        self.only = only;
        self.error = None;
        self.retry_at = None;
        self.cursor = 0;
    }

    pub(super) fn clear(&mut self) {
        *self = Self {
            only: self.only,
            ..Self::default()
        };
    }

    /// Record the outcome of a lookup. False when `id` was not one of this filter's.
    pub(super) fn finish(&mut self, id: RequestId, result: &Result<Response, AppError>) -> bool {
        self.finish_at(id, result, Instant::now())
    }

    fn finish_at(
        &mut self,
        id: RequestId,
        result: &Result<Response, AppError>,
        now: Instant,
    ) -> bool {
        let Some(track) = self.probes.remove(&id) else {
            return false;
        };
        match result {
            Ok(Response::Lyrics(found)) => {
                self.known.insert(track, found.is_some());
            }
            // The provider cannot look this song up; it has no lyrics for our purposes.
            Err(e) if e.kind == ErrorKind::InvalidInput => {
                self.known.insert(track, false);
            }
            // Passing trouble: wait it out, then check this song again.
            Err(e) if passing(e.kind) => {
                let default = if e.kind == ErrorKind::RateLimited { 60 } else { 5 };
                let wait = Duration::from_secs(e.retry_after_seconds.unwrap_or(default));
                let until = now + wait.min(MAX_PAUSE);
                self.retry_at = Some(self.retry_at.map_or(until, |at| at.max(until)));
                self.cursor = 0;
            }
            // Needs the user, such as a rejected key.
            Err(e) => self.error = Some(e.message.clone()),
            Ok(_) => {}
        }
        true
    }

    /// Whether checking is paused at `now`; a pause that has run out is cleared.
    fn paused(&mut self, now: Instant) -> bool {
        match self.retry_at {
            Some(at) if at > now => true,
            Some(_) => {
                self.retry_at = None;
                false
            }
            None => false,
        }
    }

    /// Songs of `items` not yet checked.
    fn unchecked(&self, items: &[Track]) -> usize {
        items
            .iter()
            .filter(|t| !self.known.contains_key(&t.id))
            .count()
    }
}

impl Ui {
    /// The songs the current page lists, as far as the filter looks at them.
    fn filterable_tracks(&self) -> &[Track] {
        match &self.data {
            Some(PageData::Tracks(page)) => &page.items,
            Some(PageData::Search(results)) => &results.tracks,
            _ => &[],
        }
    }

    /// Start lookups for the next unchecked songs while there are free slots.
    pub(super) fn probe_lyrics(&mut self) {
        let filter = &self.lyrics_filter;
        if !filter.only || filter.error.is_some() || !self.state.lyrics_key_set {
            return;
        }
        if self.lyrics_filter.paused(Instant::now()) {
            return;
        }
        while self.lyrics_filter.probes.len() < MAX_PROBES {
            let filter = &self.lyrics_filter;
            let items: &[Track] = self.filterable_tracks();
            let mut index: usize = filter.cursor;
            while items.get(index).is_some_and(|t| {
                filter.known.contains_key(&t.id) || filter.probes.values().any(|p| *p == t.id)
            }) {
                index += 1;
            }
            let Some(track) = items.get(index).map(|t| t.id.clone()) else {
                self.lyrics_filter.cursor = items.len();
                return;
            };
            let Some(id) = self.send(Action::Lyrics {
                track_id: track.clone(),
            }) else {
                return;
            };
            self.lyrics_filter.cursor = index + 1;
            self.lyrics_filter.probes.insert(id, track);
        }
    }

    /// Positions in `items` to show, after the text filter, the lyrics filter and the sort.
    pub(super) fn track_indices(&self, items: &[Track]) -> Vec<usize> {
        track_filter::indices(items, &self.track_filter, self.track_sort, |t| {
            self.lyrics_filter.keeps(t)
        })
    }

    /// The search overview's songs that pass the lyrics filter.
    pub(super) fn search_tracks<'a>(&self, results: &'a SearchResults) -> Vec<&'a Track> {
        results
            .tracks
            .iter()
            .filter(|t| self.lyrics_filter.keeps(t))
            .collect()
    }

    /// What the lyrics filter is waiting for or missing, for the line under the song search.
    pub(super) fn lyrics_status(&self) -> Option<String> {
        let filter = &self.lyrics_filter;
        if !filter.only {
            return None;
        }
        if !self.state.lyrics_key_set {
            return Some(
                "Add a Spicy Lyrics API key in Settings → Lyrics to filter by lyrics".into(),
            );
        }
        if let Some(error) = &filter.error {
            return Some(format!(
                "Lyrics check stopped: {error} Turn the filter off and on to retry"
            ));
        }
        if let Some(at) = filter.retry_at {
            let secs = at.saturating_duration_since(Instant::now()).as_secs() + 1;
            return Some(format!(
                "Lyrics lookups paused by the provider; resuming in {secs}s"
            ));
        }
        let left: usize = filter.unchecked(self.filterable_tracks());
        (left > 0).then(|| format!("Checking lyrics, {left} songs left"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyrics(found: bool) -> Result<Response, AppError> {
        Ok(Response::Lyrics(found.then(|| Lyrics {
            track_id: SpotifyId("t".into()),
            sync: LyricsSync::Static,
            lines: vec![],
            credit: LyricsCredit {
                source: LyricsSource::Unknown,
                uploader: None,
                maker: None,
            },
        })))
    }

    #[test]
    fn passing_failures_pause_and_recheck_but_a_rejected_key_stops() {
        let mut filter = LyricsFilter::default();
        filter.set_only(true);
        filter.cursor = 5;
        let now = Instant::now();
        let failure = |kind, retry_after_seconds| {
            Err(AppError {
                kind,
                message: "m".into(),
                retry_after_seconds,
            })
        };
        filter.probes.insert(RequestId(1), SpotifyId("a".into()));
        assert!(filter.finish_at(RequestId(1), &failure(ErrorKind::RateLimited, Some(30)), now));
        assert_eq!(filter.cursor, 0, "the song is checked again");
        assert!(filter.error.is_none());
        assert!(filter.paused(now + Duration::from_secs(29)));
        assert!(!filter.paused(now + Duration::from_secs(30)));
        assert!(filter.retry_at.is_none());

        filter.probes.insert(RequestId(2), SpotifyId("a".into()));
        filter.finish_at(RequestId(2), &failure(ErrorKind::Unavailable, None), now);
        assert!(filter.paused(now + Duration::from_secs(4)), "short default pause");

        filter.probes.insert(RequestId(3), SpotifyId("a".into()));
        filter.finish_at(RequestId(3), &failure(ErrorKind::Authentication, None), now);
        assert!(filter.error.is_some());
    }

    #[test]
    fn only_songs_known_to_have_lyrics_pass() {
        let track = |id: &str| Track {
            id: SpotifyId(id.into()),
            name: id.into(),
            artists: vec![],
            album: None,
            duration_ms: 0,
            playable: true,
            source_index: None,
        };
        let mut filter = LyricsFilter::default();
        assert!(filter.keeps(&track("a")));
        filter.set_only(true);
        for (n, (id, found)) in [("a", true), ("b", false)].into_iter().enumerate() {
            filter
                .probes
                .insert(RequestId(n as u64), SpotifyId(id.into()));
            assert!(filter.finish(RequestId(n as u64), &lyrics(found)));
        }
        assert!(filter.keeps(&track("a")));
        assert!(!filter.keeps(&track("b")), "no lyrics");
        assert!(!filter.keeps(&track("c")), "not checked yet");
        assert!(!filter.finish(RequestId(9), &lyrics(true)), "not a lookup");
    }
}
