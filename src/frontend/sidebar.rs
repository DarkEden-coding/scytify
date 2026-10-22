//! Independent playlist navigation; loading and filtering never change the main browser.
use super::*;
impl Ui {
    /// Fetch the first sidebar metadata page, cancelling its predecessors.
    pub(super) fn fetch_sidebar(&mut self) {
        let previous = self.sidebar_pending.take();
        self.cancel(previous);
        let more = self.sidebar_more.take();
        self.cancel(more);
        self.sidebar_data = None;
        self.sidebar_scroll = 0.0;
        self.sidebar_pending = self.send(Action::Fetch {
            query: Query::OwnPlaylists,
            page: PageRequest {
                offset: self.sidebar_offset,
                limit: MAX_PAGE_SIZE,
            },
        });
    }
    /// Append the next bounded playlist page when the list is scrolled to its end.
    pub(super) fn sidebar_load_more(&mut self) {
        if self.sidebar_more.is_some() || self.sidebar_pending.is_some() {
            return;
        }
        let Some(data) = &self.sidebar_data else {
            return;
        };
        if !paging::can_extend(data) {
            return;
        }
        let offset: u64 = paging::next_offset(data);
        self.sidebar_more = self.send(Action::Fetch {
            query: Query::OwnPlaylists,
            page: PageRequest {
                offset,
                limit: MAX_PAGE_SIZE,
            },
        });
    }
    /// Whether the pinned Liked Songs row is listed: always, unless the filter excludes it.
    pub(super) fn liked_pinned(&self) -> bool {
        "liked songs".contains(&self.sidebar_filter.to_lowercase())
    }
    /// Height above the playlist rows inside the sidebar list: the pinned Liked Songs row.
    pub(super) fn sidebar_list_top(&self) -> f32 {
        if self.liked_pinned() {
            layout::SIDEBAR_ROW
        } else {
            0.0
        }
    }
    /// Loaded playlists matching the case-insensitive filter.
    pub(super) fn sidebar_items(&self) -> Vec<&Playlist> {
        let filter: String = self.sidebar_filter.to_lowercase();
        match &self.sidebar_data {
            Some(PageData::Playlists(page)) => page
                .items
                .iter()
                .filter(|p| p.name.to_lowercase().contains(&filter))
                .collect(),
            _ => Vec::new(),
        }
    }
}
