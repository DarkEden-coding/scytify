//! Album, playlist and Liked Songs pages (header, play actions and a virtualized track
//! table) and search "Show all" songs.
use super::*;
use crate::frontend::artwork::Shape;
use crate::frontend::layout::{ACTIONS_HEIGHT, CONTENT_PADDING, HERO_ART, TABLE_HEADER};
use iced::widget::column;
use iced::widget::{mouse_area, rule};

/// Fixed table cells; the title and album columns share the remaining width 5:3.
const INDEX_WIDTH: f32 = 32.0;
const ACTIONS_WIDTH: f32 = 64.0;
const DURATION_WIDTH: f32 = 56.0;
const THUMB: f32 = 40.0;
const CELL_GAP: f32 = 16.0;
const ROW_PADDING: f32 = 12.0;

/// Resolved column widths for the current main view width.
struct Columns {
    album: bool,
    title: f32,
    album_width: f32,
}

impl Ui {
    pub(super) fn track_page<'a>(&'a self, page: &'a Page<Track>, data: &PageData) -> Element<'a> {
        let album: bool = matches!(self.query, Query::AlbumTracks { .. });
        let columns: Columns = self.columns(album);
        let rows: Element<'a> = self.track_rows(&page.items, page.offset, data, &columns);
        if !self.track_hero() {
            return column![self.section(&self.title), self.table_header(&columns), rows].into();
        }
        let count: u64 = page.total.unwrap_or(page.items.len() as u64);
        let mut meta = row![].spacing(6).align_y(Alignment::Center);
        if album && let Some(artists) = page.items.first().and_then(|t| t.album.as_ref()) {
            meta = meta
                .push(self.strong(artist_names(&artists.artists)))
                .push(self.label("•"));
        }
        let mut summary: String = format!("{count} songs");
        if !page.has_more && page.offset == 0 {
            let total: u64 = page.items.iter().map(|t| t.duration_ms).sum();
            summary.push_str(&format!(", {}", long_duration(total)));
        }
        meta = meta.push(self.label(summary));
        // Only the head of the list starts from its first track; a run loaded further down
        // would play from the middle.
        let first_playable: Option<usize> = (page.offset == 0)
            .then(|| page.items.iter().position(|t| t.playable))
            .flatten();
        let mut actions = row![].spacing(CONTENT_PADDING).align_y(Alignment::Center);
        if self.page_is_playing_context() {
            let playing: bool = self.state.player.status == PlaybackStatus::Playing;
            actions = actions.push(self.round_play(playing, true, Message::TogglePlayback));
        } else if let Some(index) = first_playable {
            actions = actions.push(self.round_play(
                false,
                true,
                Message::Action(Action::Play(self.source(&page.items, page.offset, index))),
            ));
        }
        let shuffle: bool = self.state.player.shuffle;
        actions = actions.push(
            self.icon_button(
                Icon::Shuffle,
                28.0,
                shuffle,
                Message::Action(Action::SetShuffle(!shuffle)),
            )
            .padding(8),
        );
        let actions = container(actions)
            .padding([0.0, CONTENT_PADDING])
            .height(ACTIONS_HEIGHT)
            .align_y(Alignment::Center);
        let art: Element<'a> = if matches!(self.query, Query::LikedTracks) {
            self.liked_cover(HERO_ART)
        } else {
            self.cover(self.header_artwork(), HERO_ART, Shape::Cover)
        };
        let hero = self.hero(
            art,
            if album { "Album" } else { "Playlist" },
            &self.title,
            meta.into(),
            Some((actions.into(), ACTIONS_HEIGHT)),
        );
        column![hero, self.table_header(&columns), rows].into()
    }

    /// Virtualized rows for every track of the list, where `items` were loaded from source
    /// offset `first`. Rows whose tracks are not loaded yet are placeholders.
    fn track_rows<'a>(
        &'a self,
        items: &'a [Track],
        first: u64,
        data: &PageData,
        columns: &Columns,
    ) -> Element<'a> {
        let (start, end) = self.visible_slots(data);
        let height: f32 = self.palette.shape.row_height;
        let mut rows = column![space().height(start as f32 * height)];
        for slot in start..end {
            let loaded = (slot as u64)
                .checked_sub(first)
                .filter(|&index| (index as usize) < items.len());
            rows = rows.push(match loaded {
                Some(index) => self.track_row(items, first, index as usize, slot, columns),
                None => self.placeholder_row(slot, columns),
            });
        }
        rows = rows.push(space().height((paging::slots(data) - end) as f32 * height));
        container(rows).padding([0.0, CONTENT_PADDING]).into()
    }

    /// Row-sized stand-in for a track that is not loaded yet: its number and a blank cover.
    fn placeholder_row<'a>(&'a self, slot: usize, columns: &Columns) -> Element<'a> {
        let mut cells = row![
            container(self.subdued((slot + 1).to_string()))
                .width(INDEX_WIDTH)
                .center_x(INDEX_WIDTH)
        ]
        .spacing(CELL_GAP)
        .align_y(Alignment::Center);
        if !columns.album {
            cells = cells.push(self.cover(None, THUMB, Shape::Cover));
        }
        container(cells)
            .padding([0.0, ROW_PADDING])
            .height(self.palette.shape.row_height)
            .align_y(Alignment::Center)
            .into()
    }

    /// Where playback starts when row `index` is played. The backend skips local files and
    /// episodes, so the context offset is the row's raw source position, not its index.
    pub(in crate::frontend) fn source(
        &self,
        items: &[Track],
        first: u64,
        index: usize,
    ) -> PlaySource {
        let track: &Track = &items[index];
        let start_offset: u64 = track.source_index.unwrap_or(first + index as u64);
        match &self.query {
            Query::PlaylistTracks { playlist_id } => PlaySource::Playlist {
                playlist_id: playlist_id.clone(),
                start_offset,
            },
            Query::AlbumTracks { album_id } => PlaySource::Album {
                album_id: album_id.clone(),
                start_offset,
            },
            Query::LikedTracks => PlaySource::LikedSongs { start_offset },
            _ => PlaySource::Track(track.id.clone()),
        }
    }

    /// Albums show no album column or thumbnails, like their printed track list.
    fn columns(&self, album: bool) -> Columns {
        let table: f32 = self.grid_width() - 2.0 * ROW_PADDING;
        let fixed: f32 = INDEX_WIDTH + ACTIONS_WIDTH + DURATION_WIDTH;
        let cells: f32 = if album { 4.0 } else { 5.0 };
        let flexible: f32 = (table - fixed - CELL_GAP * (cells - 1.0)).max(0.0);
        if album {
            Columns {
                album,
                title: flexible,
                album_width: 0.0,
            }
        } else {
            Columns {
                album,
                title: flexible * 5.0 / 8.0,
                album_width: flexible * 3.0 / 8.0,
            }
        }
    }

    /// "#  Title  Album  Duration" over a divider, matching the row cells.
    fn table_header(&self, columns: &Columns) -> Element<'_> {
        let p: &Palette = &self.palette;
        let mut cells = row![
            self.subdued("#").width(INDEX_WIDTH).center(),
            self.subdued("Title").width(Length::FillPortion(5)),
        ]
        .spacing(CELL_GAP)
        .align_y(Alignment::Center);
        if !columns.album {
            cells = cells.push(self.subdued("Album").width(Length::FillPortion(3)));
        }
        let duration: Element<'_> = if p.icons == Icons::Lucide {
            icons::icon(Icon::Clock, 16.0)
                .color(p.colors.text_subdued)
                .into()
        } else {
            self.subdued("Time").into()
        };
        cells = cells.push(space().width(ACTIONS_WIDTH)).push(
            container(duration)
                .width(DURATION_WIDTH)
                .align_x(Alignment::End),
        );
        column![
            container(cells)
                .padding([0.0, ROW_PADDING])
                .height(TABLE_HEADER - 1.0)
                .align_y(Alignment::Center),
            rule::horizontal(1).style(styles::divider(p)),
        ]
        .padding([0.0, CONTENT_PADDING])
        .into()
    }

    /// One fixed-height track row for `items[index]`, shown at list position `slot`. Click
    /// selects, double-click plays, hover reveals actions.
    fn track_row<'a>(
        &'a self,
        items: &'a [Track],
        first: u64,
        index: usize,
        slot: usize,
        columns: &Columns,
    ) -> Element<'a> {
        let p: &Palette = &self.palette;
        let item: &Track = &items[index];
        let hovered: bool = self.hovered_row == Some(slot);
        let selected: bool = self.selected_row == Some(slot);
        let current: bool = self
            .state
            .player
            .current
            .as_ref()
            .is_some_and(|t| t.id == item.id);
        let playing: bool = current && self.state.player.status == PlaybackStatus::Playing;
        let play = Message::Action(Action::Play(self.source(items, first, index)));
        let number: Element<'a> = if hovered && item.playable {
            button(glyph(p, Icon::Play, 16.0))
                .padding(0)
                .style(styles::icon_button(p, false))
                .on_press(play.clone())
                .into()
        } else if playing && p.icons == Icons::Lucide {
            icons::icon(Icon::Volume, 16.0)
                .color(p.colors.accent)
                .into()
        } else {
            self.label((first + index as u64 + 1).to_string())
                .color(if current {
                    p.colors.accent
                } else {
                    p.colors.text_subdued
                })
                .into()
        };
        let mut title_width: f32 = columns.title;
        let mut title = row![].spacing(12).align_y(Alignment::Center);
        if !columns.album {
            title = title.push(self.cover(item.artwork_url(), THUMB, Shape::Cover));
            title_width -= THUMB + 12.0;
        }
        title = title.push(self.track_title(item, current, title_width));
        let mut cells = row![
            container(number).width(INDEX_WIDTH).center_x(INDEX_WIDTH),
            container(title).width(Length::FillPortion(5)).clip(true),
        ]
        .spacing(CELL_GAP)
        .align_y(Alignment::Center);
        if !columns.album {
            cells = cells.push(
                container(self.album_link(item, columns.album_width))
                    .width(Length::FillPortion(3))
                    .clip(true),
            );
        }
        let actions: Element<'a> = if hovered {
            // The official API behind a remote device cannot insert at the front of its queue.
            let mut buttons = row![];
            if !self.state.connect.is_remote() {
                buttons = buttons.push(self.icon_button(
                    Icon::ListEnd,
                    16.0,
                    false,
                    Message::Action(Action::PlayNext(vec![item.id.clone()])),
                ));
            }
            buttons
                .push(self.icon_button(
                    Icon::ListPlus,
                    16.0,
                    false,
                    Message::Action(Action::AddToQueue(vec![item.id.clone()])),
                ))
                .into()
        } else {
            space().into()
        };
        cells = cells
            .push(
                container(actions)
                    .width(ACTIONS_WIDTH)
                    .align_x(Alignment::End),
            )
            .push(
                self.subdued(clock(item.duration_ms))
                    .width(DURATION_WIDTH)
                    .align_x(iced::alignment::Horizontal::Right),
            );
        let mut area = mouse_area(
            container(cells)
                .padding([0.0, ROW_PADDING])
                .height(p.shape.row_height)
                .align_y(Alignment::Center)
                .style(styles::track_row(p, hovered, selected)),
        )
        .on_press(Message::SelectRow(slot))
        .on_enter(Message::HoverRow(slot, true))
        .on_exit(Message::HoverRow(slot, false));
        if item.playable {
            area = area.on_double_click(play);
        }
        area.into()
    }

    /// Title over the artist links; the current track is accented, unplayable ones subdued.
    pub(super) fn track_title<'a>(
        &self,
        item: &'a Track,
        current: bool,
        width: f32,
    ) -> iced::widget::Column<'a, Message> {
        let p: &Palette = &self.palette;
        let color = if current {
            p.colors.accent
        } else if item.playable {
            p.colors.text
        } else {
            p.colors.text_subdued
        };
        column![
            self.label(self.fit(&item.name, width, p.font_size.body))
                .wrapping(Wrapping::None)
                .color(color),
            self.artist_link(item, width),
        ]
        .spacing(2)
    }

    /// Subdued artist names; the first artist opens its page.
    fn artist_link<'a>(&self, item: &'a Track, width: f32) -> Element<'a> {
        let p: &Palette = &self.palette;
        let names = text(self.fit(&artist_names(&item.artists), width, p.font_size.small))
            .font(p.font())
            .size(p.font_size.small)
            .wrapping(Wrapping::None);
        match item.artists.first() {
            Some(artist) => button(names)
                .padding(0)
                .style(styles::link(p))
                .on_press(Message::Navigate(Route::artist(artist)))
                .into(),
            None => names.color(p.colors.text_subdued).into(),
        }
    }

    /// Album name that opens the album page.
    fn album_link<'a>(&self, item: &'a Track, width: f32) -> Element<'a> {
        let p: &Palette = &self.palette;
        let Some(album) = &item.album else {
            return space().into();
        };
        button(
            self.label(self.fit(&album.name, width, p.font_size.body))
                .wrapping(Wrapping::None),
        )
        .padding(0)
        .style(styles::link(p))
        .on_press(Message::Navigate(Route::album(album)))
        .into()
    }
}
