//! Optional queue panel: the current track and a virtualized "Next up" list.
use super::sidebar::{FADE, GUTTER, gutter_scrollbar};
use super::*;
use crate::frontend::artwork::Shape;
use crate::frontend::layout::{QUEUE_WIDTH, visible_rows};
use iced::widget::column;
use iced::widget::scrollable;

const THUMB: f32 = 40.0;

impl Ui {
    pub(super) fn queue_view(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let header = row![
            self.heading("Queue").size(p.font_size.body + 4.0),
            space().width(Length::Fill),
            self.icon_button(Icon::Close, 18.0, false, Message::Queue),
        ]
        .align_y(Alignment::Center);
        let mut content = column![header].spacing(p.shape.spacing * 1.5);
        content = content.push(self.strong("Now playing"));
        content = content.push(match &self.state.player.current {
            Some(track) => self.queue_row(track, true, None),
            None => self.subdued("Nothing playing").into(),
        });
        content = content.push(self.strong("Next up"));
        content = content.push(if self.state.connect.is_remote() {
            self.remote_next_up()
        } else {
            self.next_up()
        });
        container(self.panel(content.height(Length::Fill), p.shape.padding))
            .width(QUEUE_WIDTH)
            .height(Length::Fill)
            .into()
    }

    fn next_up(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let Some(data) = &self.queue_data else {
            return self.subdued("Loading queue…").into();
        };
        let items: Vec<&QueueItem> = self.queue_items();
        let height: f32 = p.shape.row_height;
        let (start, end) =
            visible_rows(items.len(), self.queue_scroll, self.queue_viewport, height);
        let mut rows = column![space().height(start as f32 * height)];
        for item in &items[start..end] {
            let remove: Option<Message> =
                item.explicit
                    .then_some(Message::Action(Action::RemoveQueued {
                        entry_id: item.entry.entry_id,
                    }));
            rows = rows.push(match &item.track {
                Some(track) => self.queue_row(track, false, remove),
                None => container(self.subdued("Metadata unavailable"))
                    .height(height)
                    .align_y(Alignment::Center)
                    .into(),
            });
        }
        rows = rows.push(space().height((items.len() - end) as f32 * height));
        if items.is_empty() {
            rows = rows.push(self.subdued("Your queue is empty"));
        }
        if self.queue_more.is_some() {
            rows = rows.push(container(self.subdued("Loading…")).padding([8, 0]));
        } else if paging::can_extend(data) {
            rows = rows.push(
                button(self.subdued("Show more"))
                    .padding([8, 0])
                    .style(styles::ghost_button(p, false))
                    .on_press(Message::QueueMore),
            );
        }
        self.queue_scroll_area(rows)
    }

    /// Another device's upcoming tracks, as far as Spotify reports them. The official API
    /// can neither remove nor reorder them, so the rows have no actions.
    fn remote_next_up(&self) -> Element<'_> {
        let tracks: &[Track] = &self.state.connect.queue_preview;
        let height: f32 = self.palette.shape.row_height;
        let (start, end) =
            visible_rows(tracks.len(), self.queue_scroll, self.queue_viewport, height);
        let mut rows = column![space().height(start as f32 * height)];
        for track in &tracks[start..end] {
            rows = rows.push(self.queue_row(track, false, None));
        }
        rows = rows.push(space().height((tracks.len() - end) as f32 * height));
        if tracks.is_empty() {
            rows = rows.push(self.subdued("Nothing is queued next"));
        }
        self.queue_scroll_area(rows)
    }

    /// Scrolling list that reports its offset, with the fade over its bottom edge.
    fn queue_scroll_area<'a>(&self, rows: iced::widget::Column<'a, Message>) -> Element<'a> {
        self.faded(
            scrollable(rows.padding(iced::Padding::ZERO.bottom(FADE)))
                .on_scroll(|v| {
                    let y: f32 = v.absolute_offset().y;
                    let height: f32 = v.bounds().height;
                    Message::QueueScroll(y, height, y + height >= v.content_bounds().height - 200.0)
                })
                .direction(gutter_scrollbar())
                .style(styles::scrollbar(&self.palette))
                .height(Length::Fill),
        )
    }

    /// Thumbnail, title and artist; explicit entries can be removed.
    fn queue_row<'a>(
        &'a self,
        track: &'a Track,
        current: bool,
        remove: Option<Message>,
    ) -> Element<'a> {
        let p: &Palette = &self.palette;
        let remove_width: f32 = if remove.is_some() { 38.0 } else { 0.0 };
        let width: f32 = QUEUE_WIDTH
            - 2.0 * (p.shape.padding + styles::shadow_margin(p))
            - THUMB
            - 12.0
            - remove_width
            - GUTTER;
        let mut cells = row![
            self.cover(track.artwork_url(), THUMB, Shape::Cover),
            column![
                self.label(self.fit(&track.name, width, p.font_size.body))
                    .wrapping(Wrapping::None)
                    .color(if current {
                        p.colors.accent
                    } else {
                        p.colors.text
                    }),
                self.subdued(self.fit(&artist_names(&track.artists), width, p.font_size.small))
                    .wrapping(Wrapping::None),
            ]
            .spacing(2)
            .width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        if let Some(remove) = remove {
            cells = cells.push(self.icon_button(Icon::Close, 14.0, false, remove));
        }
        container(cells)
            .height(p.shape.row_height)
            .align_y(Alignment::Center)
            .into()
    }
}
