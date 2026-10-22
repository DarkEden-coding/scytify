//! Choice of the search overview's "Top result".
//!
//! Rule: the first artist, the first album and the first song compete on how well their
//! name matches the search text (exact, then starts with, then contains, then no match,
//! ignoring case). Ties go to the artist, then the album, so the artist wins unless an
//! album or song name matches strictly better.
use super::artwork::Shape;
use crate::contracts::SearchResults;
use crate::domain::{Album, Artist, Track};

#[derive(Clone, Copy, Debug)]
pub(super) enum TopResult<'a> {
    Artist(&'a Artist),
    Album(&'a Album),
    Track(&'a Track),
}

impl<'a> TopResult<'a> {
    pub(super) fn name(&self) -> &'a str {
        match self {
            Self::Artist(a) => &a.name,
            Self::Album(a) => &a.name,
            Self::Track(t) => &t.name,
        }
    }
    pub(super) fn artwork_url(&self) -> Option<&'a str> {
        match self {
            Self::Artist(a) => a.artwork_url.as_deref(),
            Self::Album(a) => a.artwork_url.as_deref(),
            Self::Track(t) => t.artwork_url(),
        }
    }
    pub(super) fn shape(&self) -> Shape {
        match self {
            Self::Artist(_) => Shape::Round,
            Self::Album(_) | Self::Track(_) => Shape::Cover,
        }
    }
}

/// The top result for `text`, or `None` without any results.
pub(super) fn top_result<'a>(results: &'a SearchResults, text: &str) -> Option<TopResult<'a>> {
    let query: String = text.trim().to_lowercase();
    let candidates = [
        results.artists.first().map(TopResult::Artist),
        results.albums.first().map(TopResult::Album),
        results.tracks.first().map(TopResult::Track),
    ];
    let mut best: Option<(u8, TopResult<'a>)> = None;
    for candidate in candidates.into_iter().flatten() {
        let score: u8 = score(candidate.name(), &query);
        if best.is_none_or(|(top, _)| score > top) {
            best = Some((score, candidate));
        }
    }
    best.map(|(_, result)| result)
}

/// How well `name` matches the lowercase `query`: 3 exact, 2 prefix, 1 contained, 0 none.
fn score(name: &str, query: &str) -> u8 {
    let name: String = name.to_lowercase();
    if name == query {
        3
    } else if name.starts_with(query) {
        2
    } else if name.contains(query) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::SpotifyId;
    fn results(artist: &str, album: &str, track: &str) -> SearchResults {
        let id = |s: &str| SpotifyId(s.into());
        SearchResults {
            artists: vec![Artist {
                id: id("artist"),
                name: artist.into(),
                artwork_url: None,
            }],
            albums: vec![Album {
                id: id("album"),
                name: album.into(),
                artists: vec![],
                artwork_url: None,
            }],
            tracks: vec![Track {
                id: id("track"),
                name: track.into(),
                artists: vec![],
                album: None,
                duration_ms: 0,
                playable: true,
                source_index: None,
            }],
        }
    }
    fn pick(r: &SearchResults, text: &str) -> &'static str {
        match top_result(r, text) {
            Some(TopResult::Artist(_)) => "artist",
            Some(TopResult::Album(_)) => "album",
            Some(TopResult::Track(_)) => "track",
            None => "none",
        }
    }
    #[test]
    fn best_name_match_wins_and_ties_prefer_the_artist() {
        assert_eq!(
            pick(
                &results("Daft Punk", "Discovery", "One More Time"),
                " daft PUNK"
            ),
            "artist"
        );
        assert_eq!(
            pick(
                &results("Daft Punk", "Discovery", "One More Time"),
                "discovery"
            ),
            "album"
        );
        assert_eq!(
            pick(
                &results("The Weeknd", "Starboy", "Blinding Lights"),
                "blinding"
            ),
            "track"
        );
        // Nothing matches better than the artist: it stays the top result.
        assert_eq!(
            pick(&results("Someone", "Else", "Entirely"), "zzz"),
            "artist"
        );
        assert_eq!(pick(&SearchResults::default(), "x"), "none");
    }
}
