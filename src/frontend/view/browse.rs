//! Scrolling main page: Home shelves, card grids for playlists, albums and artists,
//! shared page headers, empty states and onboarding.
use super::sidebar::thin_scrollbar;
use super::*;
use crate::frontend::artwork::Shape;
use crate::frontend::layout::{
    CARD_LINE_GAP, CARD_PADDING, CARD_TEXT_GAP, CONTENT_PADDING, GRID_GAP, Grid, HERO_ART,
    HERO_HEIGHT, SECTION_HEADING,
};
use iced::widget::column;
use iced::widget::{hover, scrollable, sensor};

impl Ui {
    /// The current page, inside the measured main scroll area.
    pub(super) fn browser_view(&self) -> Element<'_> {
        if !matches!(self.state.auth, AuthState::Ready { .. }) {
            return self.scroll_page(self.onboarding());
        }
        let Some(data) = &self.data else {
            if matches!(&self.query, Query::Search { text } if text.trim().is_empty()) {
                return self.scroll_page(self.search_start());
            }
            if let Some(error) = &self.page_error {
                return self.scroll_page(
                    column![
                        self.strong("This page could not be loaded."),
                        self.subdued(error),
                        self.control(self.label("Retry"))
                            .on_press(Message::RetryPage),
                    ]
                    .spacing(self.palette.shape.spacing)
                    .padding(CONTENT_PADDING)
                    .into(),
                );
            }
            let status = if self.pending.is_some() {
                "Loading…"
            } else {
                "Nothing loaded yet."
            };
            return self.scroll_page(
                container(self.subdued(status))
                    .padding(CONTENT_PADDING)
                    .into(),
            );
        };
        let content: Element<'_> = match data {
            PageData::Playlists(page) if self.home => self.home_view(page),
            PageData::Playlists(page) => column![
                self.section(&self.title),
                self.grid(&page.items, data, Shape::Cover, |item, grid| self
                    .playlist_card(item, grid)),
            ]
            .into(),
            PageData::Artists(page) => column![
                self.section(&self.title),
                self.grid(&page.items, data, Shape::Round, |item, grid| self
                    .artist_card(item, grid)),
            ]
            .into(),
            PageData::Albums(page) if self.artist_page() => {
                let count: u64 = page.total.unwrap_or(page.items.len() as u64);
                let meta = self.label(format!("{count} albums"));
                column![
                    self.hero(
                        self.cover(self.header_artwork(), HERO_ART, Shape::Round),
                        "Artist",
                        &self.title,
                        meta.into(),
                        None,
                    ),
                    self.section("Albums"),
                    self.grid(&page.items, data, Shape::Cover, |item, grid| self
                        .album_card(item, grid)),
                ]
                .into()
            }
            PageData::Albums(page) => column![
                self.section(&self.title),
                self.grid(&page.items, data, Shape::Cover, |item, grid| self
                    .album_card(item, grid)),
            ]
            .into(),
            PageData::Tracks(page) => self.track_page(page, data),
            PageData::Search(results) => match &self.query {
                Query::Search { text } => self.search_view(text, results),
                _ => column![].into(),
            },
            PageData::Queue { .. } => column![].into(),
        };
        let mut page = column![content];
        // Home shelves carry their own empty states.
        if !self.home && paging::slots(data) == 0 {
            page = page.push(self.empty_page());
        }
        self.scroll_page(page.into())
    }

    /// Home: Made For You, then a shelf of your playlists over a shelf of saved albums,
    /// each one row of cards with "Show all" opening the full, paged grid.
    fn home_view<'a>(&'a self, playlists: &'a Page<Playlist>) -> Element<'a> {
        let albums: (&'a [Album], Option<&'a str>) = match &self.shelf {
            _ if self.shelf_failed => (&[], Some("Saved albums could not be loaded.")),
            Some(page) if page.items.is_empty() => (&[], Some("Albums you save will appear here.")),
            Some(page) => (&page.items, None),
            None => (&[], Some("Loading…")),
        };
        let all_albums = Message::Navigate(Route::browse(Query::SavedAlbums, "Saved albums", None));
        let all_playlists =
            Message::Navigate(Route::browse(Query::OwnPlaylists, "Your playlists", None));
        column![
            self.mixes_shelf(),
            self.shelf(
                "Your playlists",
                all_playlists,
                &playlists.items,
                (playlists.items.is_empty()).then_some("Playlists you create will appear here."),
                |item, grid| self.playlist_card(item, grid),
            ),
            self.shelf(
                "Saved albums",
                all_albums,
                albums.0,
                albums.1,
                |item, grid| { self.album_card(item, grid) }
            ),
        ]
        .into()
    }

    /// Discover first, then Discover Weekly and Spotify's other mixes. All of them need the
    /// playback session, so without it the shelf explains how to enable it.
    fn mixes_shelf(&self) -> Element<'_> {
        let grid: Grid = self.grid_layout();
        let status: Option<String> = if self.state.connect.playback_auth != PlaybackAuth::Ready {
            Some("Enable playback & Spotify Connect in Settings → Account to see Discover and your mixes.".into())
        } else if let Some(error) = &self.mixes_failed {
            Some(format!("Your mixes could not be loaded. {error}"))
        } else if self.mixes.is_none() {
            Some("Loading…".into())
        } else {
            None
        };
        let body: Element<'_> = match (&status, &self.mixes) {
            (Some(status), _) if self.state.connect.playback_auth != PlaybackAuth::Ready => {
                self.subdued(status.clone()).into()
            }
            (Some(status), _) => row![self.discover_card(grid), self.subdued(status.clone())]
                .spacing(GRID_GAP)
                .into(),
            (None, mixes) => {
                let mut cards = row![self.discover_card(grid)].spacing(GRID_GAP);
                for item in mixes.iter().flatten().take(grid.columns.saturating_sub(1)) {
                    cards = cards.push(self.playlist_card(item, grid));
                }
                cards.into()
            }
        };
        let all = Message::Navigate(Route::browse(Query::MadeForYou, "Made for you", None));
        column![
            self.section_link("Made for you", self.mixes.is_some().then_some(all)),
            container(body)
                .padding([0.0, CONTENT_PADDING])
                .height(grid.card_height + GRID_GAP),
        ]
        .into()
    }

    /// Endless recommendations of songs you have not liked or heard before.
    fn discover_card(&self, grid: Grid) -> Element<'_> {
        let subtitle: &str = if self.state.discover_active {
            "Playing now"
        } else {
            "Songs you haven't heard"
        };
        self.card(
            grid,
            self.gradient_cover(Icon::Radio, grid.art()),
            "Discover",
            subtitle.into(),
            Message::Action(Action::PlayDiscover),
            Some(Message::Action(Action::PlayDiscover)),
        )
    }

    /// Large centered icon, heading and hint for search start and no-result pages.
    pub(super) fn centered_notice(
        &self,
        which: Icon,
        title: String,
        hint: &'static str,
    ) -> Element<'_> {
        let p: &Palette = &self.palette;
        let mut content = column![]
            .spacing(p.shape.spacing * 1.5)
            .align_x(Alignment::Center);
        if p.icons == Icons::Lucide {
            content = content.push(icons::icon(which, 56.0).color(p.colors.text_subdued));
        }
        let title: String = self.fit_bold(
            &title,
            self.grid_width() - 2.0 * CONTENT_PADDING,
            p.font_size.heading,
        );
        content = content
            .push(self.heading(title))
            .push(self.label(hint).color(p.colors.text_subdued));
        container(content)
            .padding(iced::Padding::new(CONTENT_PADDING).top(CONTENT_PADDING * 5.0))
            .center_x(Length::Fill)
            .into()
    }

    /// Message for a page without items, worded for what was asked.
    fn empty_page(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let (title, hint): (String, &str) = match &self.query {
            Query::Search { text } => {
                return self.centered_notice(
                    Icon::SearchX,
                    format!("No results found for “{text}”"),
                    "Check the spelling, or try different keywords.",
                );
            }
            Query::SearchArtists { .. }
            | Query::SearchAlbums { .. }
            | Query::SearchTracks { .. } => ("No results".into(), "Try another search."),
            Query::LikedTracks => (
                "Songs you like will appear here".into(),
                "Save songs in Spotify by tapping the heart.",
            ),
            Query::SavedAlbums => (
                "Albums you save will appear here".into(),
                "Save albums in Spotify to find them here.",
            ),
            _ => ("Nothing here yet".into(), ""),
        };
        let mut content = column![self.strong(title)].spacing(6);
        if !hint.is_empty() {
            content = content.push(self.label(hint).color(p.colors.text_subdued));
        }
        container(content)
            .padding([p.shape.spacing * 2.0, CONTENT_PADDING])
            .into()
    }

    /// Main scroll area; reports its size and scroll position.
    fn scroll_page<'a>(&self, content: Element<'a>) -> Element<'a> {
        let area = scrollable(
            container(content)
                .width(Length::Fill)
                .padding(iced::Padding::ZERO.bottom(CONTENT_PADDING)),
        )
        .id(Area::Main.id())
        .on_scroll(|v| Message::Scrolled(Area::Main, Metrics::of(&v)))
        .direction(thin_scrollbar())
        .style(styles::scrollbar(&self.palette))
        .width(Length::Fill)
        .height(Length::Fill);
        sensor(smooth_wheel(area, Area::Main))
            .on_show(Message::MainResized)
            .on_resize(Message::MainResized)
            .into()
    }

    /// Fixed-height section heading above a grid.
    pub(super) fn section<'a>(&self, title: &'a str) -> Element<'a> {
        self.section_link(title, None)
    }

    /// Section heading with an optional "Show all" link that ends at the grid's right edge.
    fn section_link<'a>(&self, title: &'a str, show_all: Option<Message>) -> Element<'a> {
        container(self.heading_link(title, show_all))
            .padding([0.0, CONTENT_PADDING])
            .width(self.grid_layout().width() + 2.0 * CONTENT_PADDING)
            .height(SECTION_HEADING)
            .align_y(Alignment::Center)
            .into()
    }

    /// Heading filling its width, with an optional "Show all" link at the right end.
    pub(super) fn heading_link<'a>(
        &self,
        title: &'a str,
        show_all: Option<Message>,
    ) -> Element<'a> {
        let p: &Palette = &self.palette;
        let mut content = row![self.heading(title)]
            .width(Length::Fill)
            .align_y(Alignment::Center);
        if let Some(message) = show_all {
            content = content.push(space().width(Length::Fill)).push(
                button(
                    text("Show all")
                        .font(p.bold_font())
                        .size(p.font_size.small)
                        .wrapping(Wrapping::None),
                )
                .padding([4.0, 0.0])
                .style(styles::link(p))
                .on_press(message),
            );
        }
        content.into()
    }

    /// A heading with "Show all" over one row of cards (the first that fit), or `status`
    /// in its place. The height is fixed, so blocks below stay where `layout` expects.
    pub(super) fn shelf<'a, T>(
        &'a self,
        title: &'a str,
        show_all: Message,
        items: &'a [T],
        status: Option<&'a str>,
        card: impl Fn(&'a T, Grid) -> Element<'a>,
    ) -> Element<'a> {
        let grid: Grid = self.grid_layout();
        let body: Element<'a> = match status {
            Some(status) => self.subdued(status).into(),
            None => {
                let mut cards = row![].spacing(GRID_GAP);
                for item in items.iter().take(grid.columns) {
                    cards = cards.push(card(item, grid));
                }
                cards.into()
            }
        };
        column![
            self.section_link(title, status.is_none().then_some(show_all)),
            container(body)
                .padding([0.0, CONTENT_PADDING])
                .height(grid.card_height + GRID_GAP),
        ]
        .into()
    }

    /// Virtualized card grid as long as the whole collection; only visible rows plus
    /// overscan are built, and slots whose items are not loaded yet are placeholders.
    fn grid<'a, T>(
        &'a self,
        items: &'a [T],
        data: &PageData,
        shape: Shape,
        card: impl Fn(&'a T, Grid) -> Element<'a>,
    ) -> Element<'a> {
        let grid: Grid = self.grid_layout();
        let (start, end) = self.visible_slots(data);
        let first: usize = paging::first_offset(data) as usize;
        let row_height: f32 = grid.card_height + GRID_GAP;
        let total_rows: usize = paging::slots(data).div_ceil(grid.columns);
        let first_row: usize = start / grid.columns;
        let end_row: usize = end.div_ceil(grid.columns);
        let mut rows = column![space().height(first_row as f32 * row_height)];
        for row_index in first_row..end_row {
            let mut cards = row![].spacing(GRID_GAP);
            let row_start: usize = row_index * grid.columns;
            for slot in row_start..(row_start + grid.columns).min(end) {
                cards = cards.push(
                    match slot.checked_sub(first).and_then(|index| items.get(index)) {
                        Some(item) => card(item, grid),
                        None => self.placeholder_card(grid, shape),
                    },
                );
            }
            rows = rows.push(container(cards).height(row_height));
        }
        rows = rows.push(space().height((total_rows - end_row) as f32 * row_height));
        container(rows).padding([0.0, CONTENT_PADDING]).into()
    }

    /// Card-sized stand-in for an item that is not loaded yet.
    fn placeholder_card(&self, grid: Grid, shape: Shape) -> Element<'_> {
        container(self.cover(None, grid.art(), shape))
            .padding(CARD_PADDING)
            .width(grid.card_width)
            .height(grid.card_height)
            .into()
    }

    fn playlist_card<'a>(&'a self, item: &'a Playlist, grid: Grid) -> Element<'a> {
        let subtitle: String = match item.track_count {
            Some(n) => format!("Playlist · {n} tracks"),
            None => "Playlist".into(),
        };
        self.card(
            grid,
            self.cover(item.artwork_url.as_deref(), grid.art(), Shape::Cover),
            &item.name,
            subtitle,
            Message::Navigate(Route::playlist(item)),
            Some(Message::Action(Action::Play(PlaySource::Playlist {
                playlist_id: item.id.clone(),
                start_offset: 0,
            }))),
        )
    }

    pub(super) fn artist_card<'a>(&'a self, item: &'a Artist, grid: Grid) -> Element<'a> {
        self.card(
            grid,
            self.cover(item.artwork_url.as_deref(), grid.art(), Shape::Round),
            &item.name,
            "Artist".into(),
            Message::Navigate(Route::artist(item)),
            None,
        )
    }

    pub(super) fn album_card<'a>(&'a self, item: &'a Album, grid: Grid) -> Element<'a> {
        self.card(
            grid,
            self.cover(item.artwork_url.as_deref(), grid.art(), Shape::Cover),
            &item.name,
            artist_names(&item.artists),
            Message::Navigate(Route::album(item)),
            Some(Message::Action(Action::Play(PlaySource::Album {
                album_id: item.id.clone(),
                start_offset: 0,
            }))),
        )
    }

    /// Cover with a one-line bold title and subdued subtitle tight below it; hovering the
    /// cover reveals a play button.
    fn card<'a>(
        &'a self,
        grid: Grid,
        art: Element<'a>,
        title: &str,
        subtitle: String,
        open: Message,
        play: Option<Message>,
    ) -> Element<'a> {
        let p: &Palette = &self.palette;
        let edge: f32 = grid.art();
        let art: Element<'a> = match play {
            Some(play) => hover(
                art,
                container(self.card_play(play))
                    .width(edge)
                    .height(edge)
                    .align_x(Alignment::End)
                    .align_y(Alignment::End)
                    .padding(8),
            ),
            None => art,
        };
        let caption = grid.caption;
        let body = column![
            art,
            space().height(CARD_TEXT_GAP),
            container(self.strong(self.fit_bold(title, edge, p.font_size.body)))
                .height(caption.title)
                .clip(true),
            space().height(CARD_LINE_GAP),
            container(
                self.subdued(self.fit(&subtitle, edge, p.font_size.small))
                    .wrapping(Wrapping::None)
            )
            .height(caption.subtitle)
            .clip(true),
        ];
        button(body)
            .padding(CARD_PADDING)
            .width(grid.card_width)
            .height(grid.card_height)
            .style(styles::card(p))
            .on_press(open)
            .into()
    }

    /// Round accent play button shown over a hovered card cover.
    pub(super) fn card_play(&self, message: Message) -> Element<'_> {
        let p: &Palette = &self.palette;
        let content: Element<'_> = if p.icons == Icons::Lucide {
            iced::widget::image(self.glyphs.play_card.clone())
                .width(icons::SOLID_CARD)
                .height(icons::SOLID_CARD)
                .into()
        } else {
            glyph(p, Icon::Play, 16.0).into()
        };
        button(content)
            .padding(14)
            .style(styles::primary_button(p))
            .on_press(message)
            .into()
    }

    /// Large page header: cover, kind label, display-size title and a metadata line, on a
    /// backdrop tinted from the cover. `below` (the play actions) continues the fade.
    pub(super) fn hero<'a>(
        &'a self,
        art: Element<'a>,
        kind: &'a str,
        title: &str,
        meta: Element<'a>,
        below: Option<(Element<'a>, f32)>,
    ) -> Element<'a> {
        let p: &Palette = &self.palette;
        let available: f32 = self.main_size.width - 3.0 * CONTENT_PADDING - HERO_ART;
        let (size, title) = self.hero_title(title, available);
        let title = text(title)
            .font(p.bold_font())
            .size(size)
            .wrapping(Wrapping::None);
        let header = container(
            row![
                art,
                column![
                    text(kind)
                        .font(p.bold_font())
                        .size(p.font_size.small)
                        .color(p.colors.text),
                    title,
                    meta,
                ]
                .spacing(p.shape.spacing)
                .width(Length::Fill),
            ]
            .spacing(CONTENT_PADDING)
            .align_y(Alignment::End),
        )
        .padding(CONTENT_PADDING)
        .width(Length::Fill)
        .height(HERO_HEIGHT)
        .align_y(Alignment::End)
        .clip(true);
        let (content, height): (Element<'a>, f32) = match below {
            Some((below, extra)) => (
                column![header, container(below).height(extra)].into(),
                HERO_HEIGHT + extra,
            ),
            None => (header.into(), HERO_HEIGHT),
        };
        // Liked Songs has no cover; its header takes the accent like its gradient art.
        let tint: Option<iced::Color> = if matches!(self.query, Query::LikedTracks) {
            Some(p.colors.accent)
        } else {
            self.header_artwork().and_then(|url| self.artwork.tint(url))
        };
        container(content)
            .width(Length::Fill)
            .height(height)
            .style(styles::hero(p, tint, HERO_HEIGHT / height))
            .into()
    }

    /// Largest display size at which `title` fits on one line, stepping down for longer
    /// names; titles too long even at the smallest step are ellipsized.
    fn hero_title(&self, title: &str, available: f32) -> (f32, String) {
        let display: f32 = self.palette.font_size.display;
        let sizes: [f32; 5] = [1.75, 1.45, 1.2, 1.0, 0.8].map(|step| (display * step).round());
        match sizes
            .iter()
            .find(|&&size| self.fit_bold(title, available, size) == title)
        {
            Some(&size) => (size, title.to_owned()),
            None => {
                let smallest: f32 = sizes[sizes.len() - 1];
                (smallest, self.fit_bold(title, available, smallest))
            }
        }
    }

    /// Account prerequisites shown instead of library content until sign-in completes.
    fn onboarding(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let status: String = match &self.state.auth {
            AuthState::Authorizing => "Complete authorization in your browser".into(),
            AuthState::Blocked { reason } => reason.clone(),
            _ => "Connect your Spotify account".into(),
        };
        container(
            column![
                text(status)
                    .font(p.bold_font())
                    .size(p.font_size.display),
                self.label("Requires Premium and an eligible developer app. Set your public Client ID in Settings and register the exact loopback redirect used by the backend. Credentials stay in the OS Secret Service, never in this form.")
                    .color(p.colors.text_subdued),
                row![
                    self.primary("Authorize in browser", Message::Action(Action::BeginLogin)),
                    self.control(self.label("Account setup")).on_press(Message::Settings),
                    self.control(self.label("Cancel"))
                        .on_press(Message::Action(Action::CancelLogin)),
                ]
                .spacing(p.shape.spacing)
                .align_y(Alignment::Center),
            ]
            .spacing(p.shape.spacing * 2.0)
            .max_width(640),
        )
        .padding(CONTENT_PADDING * 2.0)
        .into()
    }
}
