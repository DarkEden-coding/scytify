//! Your Library membership of what the UI shows: liked songs, saved albums and followed
//! playlists. A toggle shows everywhere at once and rolls back if Spotify refuses it; the
//! lists that hold the item (Liked Songs, the sidebar, the saved-albums shelf) follow once
//! Spotify confirms it.
use super::*;

/// What a toggle saves or removes, with the metadata the library lists show.
#[derive(Clone, Debug)]
pub(super) enum Entry {
    Track(Track),
    Album(Album),
    Playlist(Playlist),
}

impl Entry {
    pub(super) fn item(&self) -> LibraryItem {
        match self {
            Entry::Track(track) => LibraryItem::Track(track.id.clone()),
            Entry::Album(album) => LibraryItem::Album(album.id.clone()),
            Entry::Playlist(playlist) => LibraryItem::Playlist(playlist.id.clone()),
        }
    }
}

/// A toggle Spotify has not confirmed yet.
struct Toggle {
    entry: Entry,
    saved: bool,
    /// What was shown before, restored if the request fails.
    before: Option<bool>,
}

#[derive(Default)]
pub(super) struct Library {
    known: HashMap<LibraryItem, bool>,
    /// The one check in flight and the items it asks about.
    check: Option<(RequestId, Vec<LibraryItem>)>,
    /// Items a check failed for; asked about again after the next page load.
    failed: HashSet<LibraryItem>,
    /// The token lacks library permission; checking stops until the account changes.
    denied: bool,
    toggles: HashMap<RequestId, Toggle>,
}

impl Library {
    /// Whether the item is saved, as last reported or toggled; `None` until known.
    pub(super) fn saved(&self, item: &LibraryItem) -> Option<bool> {
        self.known.get(item).copied()
    }
    /// Records membership a listing implies, such as every row of Liked Songs. Pending
    /// toggles win: the listing may predate them.
    pub(super) fn mark(&mut self, items: impl IntoIterator<Item = LibraryItem>, saved: bool) {
        for item in items {
            if !self.toggling(&item) {
                self.known.insert(item, saved);
            }
        }
    }
    /// Forget everything; another account has its own library.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    /// A new page gets another chance at items a check failed for.
    pub(super) fn retry_failed(&mut self) {
        self.failed.clear();
    }
    fn toggling(&self, item: &LibraryItem) -> bool {
        self.toggles
            .values()
            .any(|toggle| &toggle.entry.item() == item)
    }
    /// The wanted items worth asking about, in order and without duplicates, bounded by
    /// one request. Nothing while a check is in flight.
    fn unknown(&self, wanted: impl IntoIterator<Item = LibraryItem>) -> Vec<LibraryItem> {
        if self.check.is_some() || self.denied {
            return Vec::new();
        }
        let mut unknown: Vec<LibraryItem> = Vec::new();
        for item in wanted {
            if unknown.len() == MAX_SAVED_CHECK {
                break;
            }
            if !self.known.contains_key(&item)
                && !self.failed.contains(&item)
                && !self.toggling(&item)
                && !unknown.contains(&item)
            {
                unknown.push(item);
            }
        }
        unknown
    }
    /// Shows a toggle at once and remembers how to undo it.
    fn toggled(&mut self, id: RequestId, entry: Entry, saved: bool) {
        let item: LibraryItem = entry.item();
        let before: Option<bool> = self.known.insert(item, saved);
        self.toggles.insert(
            id,
            Toggle {
                entry,
                saved,
                before,
            },
        );
    }
    /// Applies a finished check or toggle; `None` if `id` was neither. A confirmed toggle
    /// returns its entry and state so the lists can follow it.
    fn complete(
        &mut self,
        id: RequestId,
        result: &Result<Response, AppError>,
    ) -> Option<Option<(Entry, bool)>> {
        if let Some((_, items)) = self.check.take_if(|(check, _)| *check == id) {
            match result {
                Ok(Response::Saved(flags)) if flags.len() == items.len() => {
                    self.mark_checked(items, flags);
                }
                Err(e) if e.kind == ErrorKind::Permission => self.denied = true,
                _ => self.failed.extend(items),
            }
            return Some(None);
        }
        let toggle: Toggle = self.toggles.remove(&id)?;
        if result.is_ok() {
            return Some(Some((toggle.entry, toggle.saved)));
        }
        // A newer toggle of the same item already replaced what this one showed.
        let item: LibraryItem = toggle.entry.item();
        if !self.toggling(&item) {
            match toggle.before {
                Some(saved) => self.known.insert(item, saved),
                None => self.known.remove(&item),
            };
        }
        Some(None)
    }
    fn mark_checked(&mut self, items: Vec<LibraryItem>, flags: &[bool]) {
        for (item, &saved) in items.into_iter().zip(flags) {
            if !self.toggling(&item) {
                self.known.insert(item, saved);
            }
        }
    }
}

impl Ui {
    /// Whether an item is in Your Library; unknown counts as not saved.
    pub(super) fn is_saved(&self, item: &LibraryItem) -> bool {
        self.library.saved(item).unwrap_or(false)
    }
    /// Save or remove an item, showing the result before Spotify confirms it.
    pub(super) fn toggle_saved(&mut self, entry: Entry) {
        let saved: bool = !self.is_saved(&entry.item());
        if let Some(id) = self.send(Action::SetSaved {
            item: entry.item(),
            saved,
        }) {
            self.library.toggled(id, entry, saved);
        }
    }
    /// Ask Spotify about displayed items whose library state is unknown, one bounded
    /// request at a time.
    pub(super) fn check_library(&mut self) {
        if self.window.is_none() || !matches!(self.state.auth, AuthState::Ready { .. }) {
            return;
        }
        let unknown: Vec<LibraryItem> = self.library.unknown(self.shown_library_items());
        if unknown.is_empty() {
            return;
        }
        if let Some(id) = self.send(Action::CheckSaved(unknown.clone())) {
            self.library.check = Some((id, unknown));
        }
    }
    /// Handles the completion of a library request; false if `id` was another request.
    pub(super) fn complete_library(
        &mut self,
        id: RequestId,
        result: &Result<Response, AppError>,
    ) -> bool {
        let Some(confirmed) = self.library.complete(id, result) else {
            return false;
        };
        // Checks are background requests: only failures that need the user are news.
        let quiet: bool = self.quiet.remove(&id);
        if let Some((entry, saved)) = confirmed {
            self.follow_saved(entry, saved);
        } else if let Err(e) = result
            && !(quiet && passing(e.kind))
        {
            self.show_notice(e.message.clone());
        }
        true
    }
    /// Library items on screen: the playing song, song rows, and the album or playlist
    /// whose page is open.
    fn shown_library_items(&self) -> Vec<LibraryItem> {
        let mut items: Vec<LibraryItem> = Vec::new();
        if let Some(track) = &self.state.player.current {
            items.push(LibraryItem::Track(track.id.clone()));
        }
        if self.settings {
            return items;
        }
        if let Some(item) = self.header_library_item() {
            items.push(item);
        }
        if let Some(data) = &self.data {
            let (start, end) = self.browser_window(data);
            let tracks: &[Track] = match data {
                PageData::Tracks(page) => &page.items[start..end],
                PageData::Search(results) => &results.tracks[start..end],
                _ => &[],
            };
            items.extend(tracks.iter().map(|t| LibraryItem::Track(t.id.clone())));
        }
        items
    }
    /// The album or followable playlist whose page is open. Liked Songs is the library
    /// itself, and the user's own playlists are theirs already.
    pub(super) fn header_library_item(&self) -> Option<LibraryItem> {
        match &self.query {
            Query::AlbumTracks { album_id } => Some(LibraryItem::Album(album_id.clone())),
            Query::PlaylistTracks { playlist_id } if !self.owns_playlist(playlist_id) => {
                Some(LibraryItem::Playlist(playlist_id.clone()))
            }
            _ => None,
        }
    }
    /// Whether the user owns a playlist, by the sidebar's list; unknown until it loads.
    fn owns_playlist(&self, id: &SpotifyId) -> bool {
        let AuthState::Ready { user_id } = &self.state.auth else {
            return true;
        };
        match &self.sidebar_data {
            Some(PageData::Playlists(page)) => page
                .items
                .iter()
                .any(|p| &p.id == id && &p.owner_id == user_id),
            _ => true,
        }
    }
    /// What the header toggle of the open album or playlist page saves.
    pub(super) fn header_entry(&self) -> Option<Entry> {
        let artwork_url: Option<String> = self.header_artwork().map(str::to_owned);
        match self.header_library_item()? {
            LibraryItem::Album(id) => {
                let artists: Vec<Artist> = match &self.data {
                    Some(PageData::Tracks(page)) => page
                        .items
                        .first()
                        .and_then(|t| t.album.as_ref())
                        .map(|a| a.artists.clone())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                Some(Entry::Album(Album {
                    id,
                    name: self.title.clone(),
                    artists,
                    artwork_url,
                }))
            }
            LibraryItem::Playlist(id) => {
                let known: Option<&Playlist> = self.mixes.iter().flatten().find(|p| p.id == id);
                Some(Entry::Playlist(known.cloned().unwrap_or(Playlist {
                    id,
                    name: self.title.clone(),
                    owner_id: String::new(),
                    snapshot_id: String::new(),
                    track_count: None,
                    artwork_url,
                })))
            }
            LibraryItem::Track(_) => None,
        }
    }
    /// Put a confirmed save into, or take a removal out of, the lists that show it.
    fn follow_saved(&mut self, entry: Entry, saved: bool) {
        match entry {
            Entry::Track(track) => {
                if let Some(count) = self.liked_count.as_mut() {
                    *count = if saved {
                        *count + 1
                    } else {
                        count.saturating_sub(1)
                    };
                }
                if let (Query::LikedTracks, Some(PageData::Tracks(page))) =
                    (&self.query, &mut self.data)
                {
                    if saved {
                        prepend_track(page, track);
                    } else {
                        remove_track(page, &track.id);
                    }
                    self.selected_row = None;
                }
            }
            Entry::Album(album) => {
                if let Some(shelf) = self.shelf.as_mut() {
                    update_listing(shelf, album.clone(), saved, |a| a.id == album.id);
                }
                if let (Query::SavedAlbums, Some(PageData::Albums(page))) =
                    (&self.query, &mut self.data)
                {
                    update_listing(page, album.clone(), saved, |a| a.id == album.id);
                }
            }
            Entry::Playlist(playlist) => {
                if let Some(PageData::Playlists(page)) = self.sidebar_data.as_mut() {
                    let id: SpotifyId = playlist.id.clone();
                    update_listing(page, playlist, saved, |p| p.id == id);
                }
            }
        }
    }
}

/// Newest saves come first in Spotify's library listings.
fn update_listing<T>(page: &mut Page<T>, item: T, saved: bool, same: impl Fn(&T) -> bool) {
    if saved {
        if !page.items.iter().any(&same) {
            prepend(page, item);
        }
    } else {
        remove(page, same);
    }
}

/// Insert a new first row. A run that starts further down only moves down by one.
fn prepend<T>(page: &mut Page<T>, item: T) {
    if page.offset == 0 {
        page.items.insert(0, item);
    } else {
        page.offset += 1;
    }
    page.next_offset += 1;
    page.total = page.total.map(|total| total + 1);
}

/// Remove a loaded row; the rows after it move up by one. Returns its index.
fn remove<T>(page: &mut Page<T>, same: impl Fn(&T) -> bool) -> Option<usize> {
    let index: usize = page.items.iter().position(same)?;
    page.items.remove(index);
    page.next_offset = page.next_offset.saturating_sub(1);
    page.total = page.total.map(|total| total.saturating_sub(1));
    Some(index)
}

/// Like `prepend`, also moving the raw source positions that playback offsets use.
fn prepend_track(page: &mut Page<Track>, mut track: Track) {
    if page.items.iter().any(|t| t.id == track.id) {
        return;
    }
    for item in &mut page.items {
        item.source_index = item.source_index.map(|i| i + 1);
    }
    track.source_index = Some(0);
    prepend(page, track);
}

/// Like `remove`, also moving the raw source positions of the rows after it.
fn remove_track(page: &mut Page<Track>, id: &SpotifyId) {
    if let Some(index) = remove(page, |t| &t.id == id) {
        for item in &mut page.items[index..] {
            item.source_index = item.source_index.map(|i| i.saturating_sub(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::BackendPort;

    fn track(id: &str, source_index: Option<u64>) -> Track {
        Track {
            id: SpotifyId(id.into()),
            name: id.into(),
            artists: Vec::new(),
            album: None,
            duration_ms: 1000,
            playable: true,
            source_index,
        }
    }
    fn sent(back: &mut BackendPort) -> Vec<Command> {
        std::iter::from_fn(|| back.commands.try_recv().ok()).collect()
    }
    fn finish(ui: &mut Ui, id: RequestId, result: Result<Response, AppError>) {
        let _ = ui.update(Message::Backend(Event::Completed { id, result }));
    }
    /// Signed in with the window open, showing Liked Songs with two loaded songs.
    fn liked_songs() -> (Ui, BackendPort) {
        let (mut ui, mut back) = crate::frontend::channel_tests::fixture();
        ui.window = Some(window::Id::unique());
        ui.state.auth = AuthState::Ready {
            user_id: "me".into(),
        };
        ui.home = false;
        ui.query = Query::LikedTracks;
        ui.fetch();
        let fetch = sent(&mut back).pop().unwrap();
        finish(
            &mut ui,
            fetch.id,
            Ok(Response::Page(PageData::Tracks(Page {
                offset: 0,
                total: Some(2),
                items: vec![track("a", Some(0)), track("b", Some(1))],
                has_more: false,
                next_offset: 2,
            }))),
        );
        (ui, back)
    }
    fn liked_ids(ui: &Ui) -> Vec<(String, Option<u64>)> {
        let Some(PageData::Tracks(page)) = &ui.data else {
            panic!("expected Liked Songs");
        };
        page.items
            .iter()
            .map(|t| (t.id.0.clone(), t.source_index))
            .collect()
    }

    /// A like shows at once, rolls back with a notice when Spotify refuses it, and once
    /// confirmed appears at the top of Liked Songs; unliking takes the row out again.
    #[test]
    fn likes_are_optimistic_roll_back_and_update_liked_songs() {
        let (mut ui, mut back) = liked_songs();
        ui.liked_count = Some(2);
        let c = track("c", None);
        let item = LibraryItem::Track(c.id.clone());
        // Listed songs are known liked; only the playing song is asked about.
        ui.state.player.current = Some(c.clone());
        let _ = ui.update(Message::Tick);
        let check = sent(&mut back).pop().unwrap();
        assert!(matches!(&check.action, Action::CheckSaved(items) if *items == [item.clone()]));
        let _ = ui.update(Message::Tick);
        assert!(sent(&mut back).is_empty(), "one check at a time");
        finish(&mut ui, check.id, Ok(Response::Saved(vec![false])));
        assert_eq!(ui.library.saved(&item), Some(false));

        let _ = ui.update(Message::ToggleSaved(Entry::Track(c.clone())));
        let like = sent(&mut back).pop().unwrap();
        assert!(matches!(&like.action, Action::SetSaved { item: i, saved: true } if *i == item));
        assert!(ui.is_saved(&item));
        ui.notice.clear();
        finish(
            &mut ui,
            like.id,
            Err(AppError {
                kind: ErrorKind::Permission,
                message: "Your Library needs new Spotify permission".into(),
                retry_after_seconds: None,
            }),
        );
        assert!(!ui.is_saved(&item));
        assert!(ui.notice.contains("permission"));
        assert_eq!(liked_ids(&ui).len(), 2);

        let _ = ui.update(Message::ToggleSaved(Entry::Track(c.clone())));
        let like = sent(&mut back).pop().unwrap();
        finish(&mut ui, like.id, Ok(Response::Accepted));
        assert!(ui.is_saved(&item));
        assert_eq!(
            liked_ids(&ui),
            [
                ("c".into(), Some(0)),
                ("a".into(), Some(1)),
                ("b".into(), Some(2))
            ]
        );
        assert_eq!(ui.liked_count, Some(3));

        let _ = ui.update(Message::ToggleSaved(Entry::Track(track("a", Some(1)))));
        let unlike = sent(&mut back).pop().unwrap();
        assert!(matches!(
            unlike.action,
            Action::SetSaved { saved: false, .. }
        ));
        finish(&mut ui, unlike.id, Ok(Response::Accepted));
        assert_eq!(
            liked_ids(&ui),
            [("c".into(), Some(0)), ("b".into(), Some(1))]
        );
        assert_eq!(ui.liked_count, Some(2));
        let Some(PageData::Tracks(page)) = &ui.data else {
            unreachable!()
        };
        assert_eq!((page.total, page.next_offset), (Some(2), 2));
    }

    /// Albums and other people's playlists get a header toggle; the user's own playlists
    /// do not, and following one adds it to the sidebar once Spotify confirms it.
    #[test]
    fn headers_offer_saving_except_for_own_playlists() {
        let (mut ui, mut back) = liked_songs();
        let playlist = |id: &str, owner: &str| Playlist {
            id: SpotifyId(id.into()),
            name: id.into(),
            owner_id: owner.into(),
            snapshot_id: String::new(),
            track_count: None,
            artwork_url: None,
        };
        ui.sidebar_data = Some(PageData::Playlists(Page {
            offset: 0,
            total: Some(1),
            items: vec![playlist("mine", "me")],
            has_more: false,
            next_offset: 1,
        }));
        ui.query = Query::AlbumTracks {
            album_id: SpotifyId("album".into()),
        };
        assert!(matches!(ui.header_entry(), Some(Entry::Album(a)) if a.id.0 == "album"));
        ui.query = Query::PlaylistTracks {
            playlist_id: SpotifyId("mine".into()),
        };
        assert!(ui.header_entry().is_none());
        ui.query = Query::PlaylistTracks {
            playlist_id: SpotifyId("mix".into()),
        };
        ui.mixes = Some(vec![playlist("mix", "spotify")]);
        let Some(entry) = ui.header_entry() else {
            panic!("a Spotify playlist can be followed");
        };
        sent(&mut back);
        let _ = ui.update(Message::ToggleSaved(entry));
        let follow = sent(&mut back)
            .into_iter()
            .find(|c| matches!(c.action, Action::SetSaved { .. }))
            .unwrap();
        finish(&mut ui, follow.id, Ok(Response::Accepted));
        let Some(PageData::Playlists(page)) = &ui.sidebar_data else {
            unreachable!()
        };
        let ids: Vec<&str> = page.items.iter().map(|p| p.id.0.as_str()).collect();
        assert_eq!(ids, ["mix", "mine"]);
        assert_eq!(page.total, Some(2));
    }
}
