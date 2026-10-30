//! Browser routes and a bounded back/forward history.
use crate::contracts::{Query, StatsRange};
use crate::domain::{Album, Artist, Playlist};

const MAX_HISTORY: usize = 50;

/// One place in the main view: a backend query, or Settings shown over it.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub query: Query,
    pub title: String,
    /// Header art known when the route was opened (artist picture, playlist cover).
    pub artwork: Option<String>,
    pub settings: bool,
    /// Home shows shelves (first playlists, saved albums) over the playlist query instead
    /// of its full grid.
    pub home: bool,
}

impl Route {
    pub fn browse(query: Query, title: impl Into<String>, artwork: Option<String>) -> Self {
        Self {
            query,
            title: title.into(),
            artwork,
            settings: false,
            home: false,
        }
    }
    pub fn home() -> Self {
        Self {
            home: true,
            ..Self::browse(Query::OwnPlaylists, "Home", None)
        }
    }
    /// Artist page: header picture and album grid.
    pub fn artist(artist: &Artist) -> Self {
        let query = Query::ArtistAlbums {
            artist_id: artist.id.clone(),
        };
        Self::browse(query, artist.name.clone(), artist.artwork_url.clone())
    }
    pub fn album(album: &Album) -> Self {
        let query = Query::AlbumTracks {
            album_id: album.id.clone(),
        };
        Self::browse(query, album.name.clone(), album.artwork_url.clone())
    }
    pub fn playlist(playlist: &Playlist) -> Self {
        let query = Query::PlaylistTracks {
            playlist_id: playlist.id.clone(),
        };
        Self::browse(query, playlist.name.clone(), playlist.artwork_url.clone())
    }
    /// Listening stats recorded on this device.
    pub fn stats(range: StatsRange) -> Self {
        Self::browse(Query::ListeningStats { range }, "Your stats", None)
    }
    /// Mixed search overview; empty text shows the search start page without a request.
    pub fn search(text: &str) -> Self {
        let title: &str = if text.is_empty() { "Search" } else { text };
        Self::browse(Query::Search { text: text.into() }, title, None)
    }
}

/// Whether a query belongs to search: the overview or a single-type "Show all" page.
pub fn is_search(query: &Query) -> bool {
    matches!(
        query,
        Query::Search { .. }
            | Query::SearchArtists { .. }
            | Query::SearchAlbums { .. }
            | Query::SearchTracks { .. }
            | Query::SearchPlaylists { .. }
    )
}

#[derive(Default)]
pub struct History {
    back: Vec<Route>,
    forward: Vec<Route>,
}

impl History {
    /// Remember `current` before opening a new place; a new branch clears forward.
    pub fn push(&mut self, current: Route) {
        if self.back.last() != Some(&current) {
            self.back.push(current);
            if self.back.len() > MAX_HISTORY {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }
    pub fn back(&mut self, current: Route) -> Option<Route> {
        let route: Route = self.back.pop()?;
        self.forward.push(current);
        Some(route)
    }
    pub fn forward(&mut self, current: Route) -> Option<Route> {
        let route: Route = self.forward.pop()?;
        self.back.push(current);
        Some(route)
    }
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::SpotifyId;
    #[test]
    fn back_and_forward_walk_the_same_path() {
        let album: Route = Route::browse(
            Query::AlbumTracks {
                album_id: SpotifyId("a".into()),
            },
            "Album",
            None,
        );
        let mut history: History = History::default();
        history.push(Route::home());
        assert_eq!(history.back(album.clone()), Some(Route::home()));
        assert!(!history.can_go_back());
        assert_eq!(history.forward(Route::home()), Some(album.clone()));
        history.push(album.clone());
        history.push(album);
        assert!(!history.can_go_forward());
        for _ in 0..MAX_HISTORY * 2 {
            history.push(Route::home());
            history.push(Route::browse(Query::OwnPlaylists, "x", None));
        }
        assert!(history.back.len() <= MAX_HISTORY);
    }
}
