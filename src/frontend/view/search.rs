//! Search overview: "Top result" beside the first songs (stacked when narrow), then one
//! row of artists and one of albums, each with "Show all". Also the start page.
use super::*;
use crate::frontend::artwork::Shape;
use crate::frontend::layout::{
    CONTENT_PADDING, GRID_GAP, PILL_PADDING, SEARCH_SONGS, SearchTop, TOP_RESULT_ART,
    TOP_RESULT_ART_GAP, TOP_RESULT_LINE_GAP, TOP_RESULT_PADDING,
};
use crate::frontend::search::TopResult;
use iced::widget::{column, hover, mouse_area, stack};

/// Thumbnail of a compact song row and the gap after it.
const THUMB: f32 = 40.0;
const THUMB_GAP: f32 = 12.0;
const DURATION_WIDTH: f32 = 56.0;
const ROW_PADDING: f32 = 8.0;

impl Ui {
    /// Overview of one search text. Sections without results are left out.
    pub(super) fn search_view<'a>(
        &'a self,
        text: &'a str,
        results: &'a SearchResults,
    ) -> Element<'a> {
        let show_all = |query: Query, kind: &str| -> Message {
            Message::Navigate(Route::browse(query, format!("{kind} for “{text}”"), None))
        };
        let mut page = column![];
        if let Some(top) = self.top_result(results) {
            let songs = show_all(
                Query::SearchTracks {
                    text: text.to_owned(),
                },
                "Songs",
            );
            page = page.push(self.search_top(top, &results.tracks, songs));
        }
        if !results.artists.is_empty() {
            page = page.push(self.shelf(
                "Artists",
                show_all(
                    Query::SearchArtists {
                        text: text.to_owned(),
                    },
                    "Artists",
                ),
                &results.artists,
                None,
                |item, grid| self.artist_card(item, grid),
            ));
        }
        if !results.albums.is_empty() {
            page = page.push(self.shelf(
                "Albums",
                show_all(
                    Query::SearchAlbums {
                        text: text.to_owned(),
                    },
                    "Albums",
                ),
                &results.albums,
                None,
                |item, grid| self.album_card(item, grid),
            ));
        }
        page.into()
    }

    /// Search page before anything is typed.
    pub(super) fn search_start(&self) -> Element<'_> {
        self.centered_notice(
            Icon::Search,
            "Search for artists, albums, or songs".into(),
            "Start typing in the search box above.",
        )
    }

    /// "Top result" beside "Songs", or above it when narrow; heights from `SearchTop`.
    fn search_top<'a>(
        &'a self,
        top: TopResult<'a>,
        tracks: &'a [Track],
        show_songs: Message,
    ) -> Element<'a> {
        let geometry: SearchTop = self.search_geometry(tracks.len());
        let card = self.top_result_card(top, &geometry);
        let songs: Option<Element<'a>> = (!tracks.is_empty()).then(|| {
            container(self.search_songs(tracks, geometry.songs_width))
                .height(geometry.songs_height)
                .into()
        });
        let heading = |title: &'a str, show_all: Option<Message>, width: f32| {
            container(self.heading_link(title, show_all))
                .width(width)
                .height(layout::SECTION_HEADING)
                .align_y(Alignment::Center)
        };
        let songs_heading = || heading("Songs", Some(show_songs.clone()), geometry.songs_width);
        let mut content = column![];
        if geometry.stacked {
            content = content
                .push(heading("Top result", None, geometry.result_width))
                .push(card);
            if let Some(songs) = songs {
                content = content.push(songs_heading()).push(songs);
            }
        } else {
            let mut headings =
                row![heading("Top result", None, geometry.result_width)].spacing(GRID_GAP);
            let mut body = row![card].spacing(GRID_GAP);
            if let Some(songs) = songs {
                headings = headings.push(songs_heading());
                body = body.push(songs);
            }
            content = content.push(headings).push(body);
        }
        container(content)
            .padding(iced::Padding::from([0.0, CONTENT_PADDING]).bottom(GRID_GAP))
            .height(geometry.height())
            .into()
    }

    /// Large card: cover (round for artists), the name in display size and a kind pill.
    /// Albums and songs reveal a play button while hovered.
    fn top_result_card<'a>(&'a self, top: TopResult<'a>, geometry: &SearchTop) -> Element<'a> {
        let p: &Palette = &self.palette;
        let inner: f32 = geometry.result_width - 2.0 * TOP_RESULT_PADDING;
        let (kind, artists, open, play): (&str, &[Artist], Message, Option<Message>) = match top {
            TopResult::Artist(a) => ("Artist", &[], Message::Navigate(Route::artist(a)), None),
            TopResult::Album(a) => (
                "Album",
                &a.artists,
                Message::Navigate(Route::album(a)),
                Some(Message::Action(Action::Play(PlaySource::Album {
                    album_id: a.id.clone(),
                    start_offset: 0,
                }))),
            ),
            TopResult::Track(t) => {
                let play = Message::PlayTrack(PlaySource::Track(t.id.clone()), Box::new(t.clone()));
                // A song opens its album; without one, the card plays it.
                let open: Message = t
                    .album
                    .as_ref()
                    .map_or(play.clone(), |a| Message::Navigate(Route::album(a)));
                ("Song", &t.artists, open, t.playable.then_some(play))
            }
        };
        let pill = container(
            text(kind)
                .font(p.bold_font())
                .size(p.font_size.small)
                .wrapping(Wrapping::None),
        )
        .padding([PILL_PADDING, 10.0])
        .style(styles::pill(p));
        let mut meta = row![pill].spacing(8).align_y(Alignment::Center);
        if !artists.is_empty() {
            meta = meta.push(self.subdued(self.fit(
                &artist_names(artists),
                (inner - 80.0).max(0.0),
                p.font_size.small,
            )));
        }
        let name = text(self.fit_bold(top.name(), inner, p.font_size.display))
            .font(p.bold_font())
            .size(p.font_size.display)
            .wrapping(Wrapping::None);
        let body = column![
            self.cover(top.artwork_url(), TOP_RESULT_ART, top.shape()),
            space().height(TOP_RESULT_ART_GAP),
            name,
            space().height(TOP_RESULT_LINE_GAP),
            meta,
        ];
        let card = button(body)
            .padding(TOP_RESULT_PADDING)
            .width(geometry.result_width)
            .height(geometry.card_height)
            .style(styles::top_result(p))
            .on_press(open);
        match play {
            Some(play) => hover(
                card,
                container(self.card_play(play))
                    .width(geometry.result_width)
                    .height(geometry.card_height)
                    .align_x(Alignment::End)
                    .align_y(Alignment::End)
                    .padding(TOP_RESULT_PADDING),
            ),
            None => card.into(),
        }
    }

    /// The first songs as compact rows: thumbnail (a play button while hovered), title and
    /// artists, duration. Click selects, double-click plays.
    fn search_songs<'a>(&'a self, tracks: &'a [Track], width: f32) -> Element<'a> {
        let p: &Palette = &self.palette;
        let text_width: f32 = width - 2.0 * ROW_PADDING - THUMB - 2.0 * THUMB_GAP - DURATION_WIDTH;
        let mut rows = column![];
        for (index, item) in tracks.iter().enumerate().take(SEARCH_SONGS) {
            let hovered: bool = self.hovered_row == Some(index);
            let selected: bool = self.selected_row == Some(index);
            let current: bool = self
                .state
                .player
                .current
                .as_ref()
                .is_some_and(|t| t.id == item.id);
            let play =
                Message::PlayTrack(PlaySource::Track(item.id.clone()), Box::new(item.clone()));
            let mut thumb = stack![self.cover(item.artwork_url(), THUMB, Shape::Cover)];
            if hovered && item.playable {
                thumb = thumb.push(
                    button(
                        container(glyph(p, Icon::Play, 16.0).color(iced::Color::WHITE))
                            .center(THUMB)
                            .style(styles::scrim(p)),
                    )
                    .padding(0)
                    .style(styles::icon_button(p, false))
                    .on_press(play.clone()),
                );
            }
            let cells = row![
                thumb,
                self.track_title(item, current, text_width)
                    .width(Length::Fill),
                self.subdued(clock(item.duration_ms))
                    .width(DURATION_WIDTH)
                    .align_x(iced::alignment::Horizontal::Right),
            ]
            .spacing(THUMB_GAP)
            .align_y(Alignment::Center);
            let mut area = mouse_area(
                container(cells)
                    .padding([0.0, ROW_PADDING])
                    .width(width)
                    .height(p.shape.row_height)
                    .align_y(Alignment::Center)
                    .clip(true)
                    .style(styles::track_row(p, hovered, selected)),
            )
            .on_press(Message::SelectRow(index))
            .on_enter(Message::HoverRow(index, true))
            .on_exit(Message::HoverRow(index, false));
            if item.playable {
                area = area.on_double_click(play);
            }
            rows = rows.push(area);
        }
        rows.into()
    }
}
