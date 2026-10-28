//! Navigation block and the virtualized "Your Library" playlist list.
use super::*;
use crate::frontend::artwork::Shape;
use crate::frontend::layout::{SIDEBAR_ROW, visible_rows};
use iced::widget::column;
use iced::widget::{scrollable, text_input};

/// Title of the Liked Songs page and its sidebar row.
pub(super) const LIKED_SONGS: &str = "Liked Songs";
/// Thumbnail edge of a sidebar playlist row.
const THUMB: f32 = 48.0;
/// Height of the fade over the bottom of a side list.
pub(super) const FADE: f32 = 20.0;

impl Ui {
    pub(super) fn sidebar_view(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let home: bool = !self.settings && self.home;
        let searching: bool = !self.settings && navigation::is_search(&self.query);
        let nav = self.panel(
            column![
                self.nav(Icon::Home, "Home", home, Message::Navigate(Route::home())),
                self.nav(Icon::Search, "Search", searching, Message::OpenSearch),
            ]
            .spacing(4),
            p.shape.spacing,
        );
        let mut header = row![].spacing(12).align_y(Alignment::Center);
        if p.icons == Icons::Lucide {
            header = header.push(icons::icon(Icon::Library, 22.0).color(p.colors.text_subdued));
        }
        header = header.push(self.strong("Your Library").color(p.colors.text_subdued));
        let library = self.panel(
            column![
                container(header).padding([4.0, 8.0]),
                self.nav(
                    Icon::Album,
                    "Saved albums",
                    !self.settings && matches!(self.query, Query::SavedAlbums),
                    Message::Navigate(Route::browse(Query::SavedAlbums, "Saved albums", None))
                ),
                self.nav(
                    Icon::Users,
                    "Followed artists",
                    !self.settings && matches!(self.query, Query::FollowedArtists),
                    Message::Navigate(Route::browse(
                        Query::FollowedArtists,
                        "Followed artists",
                        None
                    ))
                ),
                text_input("Filter playlists", &self.sidebar_filter)
                    .font(p.font())
                    .size(p.font_size.small)
                    .style(styles::text_input(p))
                    .on_input(Message::SidebarFilter)
                    .padding([8.0, 12.0]),
                self.playlist_list(),
            ]
            .spacing(p.shape.spacing * 1.5)
            .height(Length::Fill),
            p.shape.spacing,
        );
        container(column![container(nav).height(Length::Shrink), library].spacing(self.panel_gap()))
            .width(self.panel_widths().0)
            .height(Length::Fill)
            .into()
    }

    /// Navigation link with a leading icon unless the theme uses text labels.
    fn nav<'a>(&self, which: Icon, label: &'a str, active: bool, message: Message) -> Element<'a> {
        let mut content = row![].spacing(16).align_y(Alignment::Center);
        if self.palette.icons == Icons::Lucide {
            content = content.push(icons::icon(which, 22.0));
        }
        let text = if active {
            self.strong(label)
        } else {
            self.label(label).wrapping(Wrapping::None)
        };
        button(content.push(text.size(self.palette.font_size.body + 1.0)))
            .padding([10, 12])
            .width(Length::Fill)
            .style(styles::ghost_button(&self.palette, active))
            .on_press(message)
            .into()
    }

    /// Pinned Liked Songs, then fixed-height playlist rows; only the visible window and
    /// its overscan are built.
    fn playlist_list(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let mut rows = column![];
        if self.liked_pinned() {
            rows = rows.push(self.liked_row());
        }
        if self.sidebar_data.is_none() {
            let status = if self.sidebar_pending.is_some() {
                "Loading playlists…"
            } else {
                "No playlists loaded"
            };
            return rows
                .push(container(self.subdued(status)).padding([8, 8]))
                .into();
        }
        let items: Vec<&Playlist> = self.sidebar_items();
        let (start, end) = visible_rows(
            items.len(),
            self.sidebar_scroll - self.sidebar_list_top(),
            self.sidebar_viewport,
            SIDEBAR_ROW,
        );
        rows = rows.push(space().height(start as f32 * SIDEBAR_ROW));
        for playlist in &items[start..end] {
            rows = rows.push(self.playlist_row(playlist));
        }
        rows = rows.push(space().height((items.len() - end) as f32 * SIDEBAR_ROW));
        if items.is_empty() && !self.sidebar_filter.is_empty() {
            rows = rows.push(container(self.subdued("No playlists match")).padding([8, 8]));
        }
        if let Some(data) = &self.sidebar_data {
            if self.sidebar_more.is_some() {
                rows = rows.push(container(self.subdued("Loading…")).padding([8, 8]));
            } else if paging::can_extend(data) {
                rows = rows.push(
                    button(self.subdued("Show more"))
                        .padding([8, 8])
                        .style(styles::ghost_button(p, false))
                        .on_press(Message::SidebarMore),
                );
            } else if paging::page_info(data).2 {
                rows = rows.push(
                    container(self.subdued(format!(
                        "Showing the first {} playlists",
                        paging::page_info(data).0
                    )))
                    .padding([8, 8]),
                );
            }
        }
        let list = scrollable(rows.padding(iced::Padding::ZERO.bottom(FADE)))
            .id(Area::Sidebar.id())
            .on_scroll(|v| Message::Scrolled(Area::Sidebar, Metrics::of(&v)))
            .direction(gutter_scrollbar())
            .style(styles::scrollbar(p))
            .height(Length::Fill);
        self.faded(smooth_wheel(list, Area::Sidebar))
    }

    /// Overlay a fade on the bottom edge of a side list, leaving the scrollbar gutter clear.
    pub(super) fn faded<'a>(&self, list: impl Into<Element<'a>>) -> Element<'a> {
        let fade = container(space())
            .width(Length::Fill)
            .height(FADE)
            .style(styles::fade(&self.palette));
        stack![
            list.into(),
            container(fade)
                .padding(iced::Padding::ZERO.right(GUTTER))
                .height(Length::Fill)
                .align_y(Alignment::End),
        ]
        .into()
    }

    /// One playlist: cover thumbnail, name and a subdued description line.
    fn playlist_row<'a>(&'a self, playlist: &'a Playlist) -> Element<'a> {
        let selected: bool = !self.settings
            && matches!(&self.query, Query::PlaylistTracks { playlist_id } if *playlist_id == playlist.id);
        let detail: String = match playlist.track_count {
            Some(n) => format!("Playlist · {n} tracks"),
            None => "Playlist".into(),
        };
        self.library_row(
            self.cover(playlist.artwork_url.as_deref(), THUMB, Shape::Cover),
            &playlist.name,
            detail,
            selected,
            Route::playlist(playlist),
        )
    }

    /// Liked Songs, pinned above the playlists with its gradient heart cover.
    fn liked_row(&self) -> Element<'_> {
        let selected: bool = !self.settings && matches!(self.query, Query::LikedTracks);
        let detail: String = match self.liked_count {
            Some(n) => format!("Playlist · {n} songs"),
            None => "Playlist".into(),
        };
        self.library_row(
            self.liked_cover(THUMB),
            LIKED_SONGS,
            detail,
            selected,
            Route::browse(Query::LikedTracks, LIKED_SONGS, None),
        )
    }

    /// A "Your Library" row: thumbnail, name and a subdued detail line.
    fn library_row<'a>(
        &'a self,
        art: Element<'a>,
        name: &str,
        detail: String,
        selected: bool,
        route: Route,
    ) -> Element<'a> {
        let p: &Palette = &self.palette;
        // Panel padding, row padding, thumbnail, gap and the scrollbar gutter.
        let text_width: f32 = self.panel_widths().0
            - 2.0 * (p.shape.spacing + styles::shadow_margin(p))
            - 2.0 * 8.0
            - THUMB
            - 12.0
            - GUTTER;
        let name = self
            .label(self.fit(name, text_width, p.font_size.body))
            .wrapping(Wrapping::None)
            .color(if selected {
                p.colors.accent
            } else {
                p.colors.text
            });
        let detail = self.fit(&detail, text_width, p.font_size.small);
        button(
            row![
                art,
                column![name, self.subdued(detail).wrapping(Wrapping::None)].spacing(2),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding(8)
        .width(Length::Fill)
        .height(SIDEBAR_ROW)
        .style(styles::list_row(p, selected))
        .on_press(Message::Navigate(route))
        .into()
    }
}

/// Narrow overlay scrollbar used by the main page.
pub(super) fn thin_scrollbar() -> iced::widget::scrollable::Direction {
    iced::widget::scrollable::Direction::Vertical(thin())
}

/// Narrow scrollbar in its own gutter, for side lists whose rows reach the right edge.
pub(super) fn gutter_scrollbar() -> iced::widget::scrollable::Direction {
    iced::widget::scrollable::Direction::Vertical(thin().spacing(GUTTER - 10.0))
}

/// Width taken by [`gutter_scrollbar`]: bar, margins and spacing.
pub(super) const GUTTER: f32 = 14.0;

fn thin() -> iced::widget::scrollable::Scrollbar {
    iced::widget::scrollable::Scrollbar::new()
        .width(6)
        .scroller_width(6)
        .margin(2)
}
