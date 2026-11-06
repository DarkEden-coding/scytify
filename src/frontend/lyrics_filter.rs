//! "Only songs with lyrics". Whether a song has lyrics is known only by asking the lyrics
//! provider, so while the filter is on the loaded songs are checked a few at a time and a
//! song appears once it is known to have lyrics. The backend caches every answer on disk.
use super::*;

/// Lyrics lookups in flight at once. The backend refuses new work beyond four jobs, and the
/// lyrics panel needs one of them.
const MAX_PROBES: usize = 2;

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
            Err(e) => self.error = Some(e.message.clone()),
            Ok(_) => {}
        }
        true
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
