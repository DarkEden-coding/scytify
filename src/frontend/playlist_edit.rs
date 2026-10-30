//! Adding songs to and removing them from the user's playlists from the save sheet. Like
//! library toggles, an edit shows at once and rolls back if Spotify refuses it.
use super::*;

/// A playlist and one of its songs.
type Membership = (SpotifyId, SpotifyId);

/// An edit Spotify has not confirmed yet.
struct Edit {
    membership: Membership,
    /// What was shown before, restored if the request fails.
    before: Option<bool>,
}

#[derive(Default)]
pub(super) struct PlaylistEdits {
    /// Membership learned from edits this session. Spotify has no cheap "contains" check
    /// for playlists, so anything else is unknown.
    known: HashMap<Membership, bool>,
    pending: HashMap<RequestId, Edit>,
}

impl PlaylistEdits {
    /// Forget everything; another account has its own playlists.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

impl Ui {
    /// Playlists the user owns and so can add songs to, in sidebar order.
    pub(super) fn editable_playlists(&self) -> Vec<&Playlist> {
        let AuthState::Ready { user_id } = &self.state.auth else {
            return Vec::new();
        };
        match &self.sidebar_data {
            Some(PageData::Playlists(page)) => page
                .items
                .iter()
                .filter(|p| &p.owner_id == user_id)
                .collect(),
            _ => Vec::new(),
        }
    }
    /// Whether a song is in a playlist: as edited this session, or as the open playlist
    /// page lists it. Unknown counts as not in it.
    pub(super) fn in_playlist(&self, playlist_id: &SpotifyId, track_id: &SpotifyId) -> bool {
        let membership: Membership = (playlist_id.clone(), track_id.clone());
        if let Some(&known) = self.playlist_edits.known.get(&membership) {
            return known;
        }
        matches!(&self.query, Query::PlaylistTracks { playlist_id: open } if open == playlist_id)
            && matches!(&self.data, Some(PageData::Tracks(page))
                if page.items.iter().any(|t| &t.id == track_id))
    }
    /// Add a song to a playlist, or remove it if it is already there.
    pub(super) fn toggle_in_playlist(&mut self, playlist_id: SpotifyId, track_id: SpotifyId) {
        let added: bool = !self.in_playlist(&playlist_id, &track_id);
        let Some(id) = self.send(Action::SetInPlaylist {
            playlist_id: playlist_id.clone(),
            track_id: track_id.clone(),
            added,
        }) else {
            return;
        };
        let membership: Membership = (playlist_id, track_id);
        let before: Option<bool> = self.playlist_edits.known.insert(membership.clone(), added);
        self.playlist_edits
            .pending
            .insert(id, Edit { membership, before });
    }
    /// Handles the completion of a playlist edit; false if `id` was another request.
    pub(super) fn complete_playlist_edit(
        &mut self,
        id: RequestId,
        result: &Result<Response, AppError>,
    ) -> bool {
        let Some(edit) = self.playlist_edits.pending.remove(&id) else {
            return false;
        };
        let (playlist_id, _) = &edit.membership;
        match result {
            Ok(_) => {
                // The open playlist page shows the confirmed change.
                if matches!(&self.query, Query::PlaylistTracks { playlist_id: open } if open == playlist_id)
                {
                    self.fetch();
                }
            }
            Err(e) => {
                // A newer edit of the same membership already replaced what this one showed.
                let superseded: bool = self
                    .playlist_edits
                    .pending
                    .values()
                    .any(|other| other.membership == edit.membership);
                if !superseded {
                    match edit.before {
                        Some(known) => self.playlist_edits.known.insert(edit.membership, known),
                        None => self.playlist_edits.known.remove(&edit.membership),
                    };
                }
                self.show_notice(e.message.clone());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist(id: &str, owner: &str) -> Playlist {
        Playlist {
            id: SpotifyId(id.into()),
            name: id.into(),
            owner_id: owner.into(),
            snapshot_id: String::new(),
            track_count: None,
            artwork_url: None,
        }
    }

    /// Only the user's own playlists are offered; an edit shows at once, a confirmed add
    /// sticks, and a refused removal rolls back with a notice.
    #[test]
    fn playlist_edits_are_optimistic_and_roll_back() {
        let (mut ui, mut back) = crate::frontend::channel_tests::fixture();
        ui.state.auth = AuthState::Ready {
            user_id: "me".into(),
        };
        ui.sidebar_data = Some(PageData::Playlists(Page {
            offset: 0,
            total: Some(2),
            items: vec![playlist("mine", "me"), playlist("theirs", "other")],
            has_more: false,
            next_offset: 2,
        }));
        let names: Vec<&str> = ui
            .editable_playlists()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["mine"]);

        let (mine, song) = (SpotifyId("mine".into()), SpotifyId("song".into()));
        let mut toggle = |ui: &mut Ui| {
            let _ = ui.update(Message::ToggleInPlaylist(mine.clone(), song.clone()));
            std::iter::from_fn(|| back.commands.try_recv().ok())
                .find(|c| matches!(c.action, Action::SetInPlaylist { .. }))
                .unwrap()
        };
        let add = toggle(&mut ui);
        assert!(matches!(
            add.action,
            Action::SetInPlaylist { added: true, .. }
        ));
        assert!(ui.in_playlist(&mine, &song));
        let finish = |ui: &mut Ui, id, result| {
            let _ = ui.update(Message::Backend(Event::Completed { id, result }));
        };
        finish(&mut ui, add.id, Ok(Response::Accepted));
        assert!(ui.in_playlist(&mine, &song));

        let remove = toggle(&mut ui);
        assert!(matches!(
            remove.action,
            Action::SetInPlaylist { added: false, .. }
        ));
        assert!(!ui.in_playlist(&mine, &song));
        finish(
            &mut ui,
            remove.id,
            Err(AppError {
                kind: ErrorKind::Network,
                message: "Spotify API request failed".into(),
                retry_after_seconds: None,
            }),
        );
        assert!(ui.in_playlist(&mine, &song));
        assert_eq!(ui.notice, "Spotify API request failed");
    }
}
