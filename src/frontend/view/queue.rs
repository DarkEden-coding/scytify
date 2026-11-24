//! Optional queue panel: the current track, then a virtualized list of the songs the user
//! queued ("Next in queue") over the rest of the playing context ("Next up").
use super::sidebar::{FADE, GUTTER, gutter_scrollbar};
use super::*;
use crate::frontend::artwork::Shape;
use crate::frontend::layout::visible_rows;
use iced::widget::column;
use iced::widget::scrollable;

const THUMB: f32 = 40.0;

/// One fixed-height line of the upcoming list. Headings take a row each so the list stays
/// uniform for virtualization.
enum Slot<'a> {
    /// "Next in queue", with the Clear queue button.
    Queued,
    /// "Next up": the context after the queued songs.
    Context,
    Item(&'a QueueItem),
}

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
        content = content.push(if self.state.connect.is_remote() {
            column![self.strong("Next up"), self.remote_next_up()]
                .spacing(p.shape.spacing * 1.5)
                .into()
        } else {
            self.next_up()
        });
        container(self.panel(content.height(Length::Fill), p.shape.padding))
            .width(self.panel_widths().1)
            .height(Length::Fill)
            .into()
    }

    fn next_up(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let Some(data) = &self.queue_data else {
            return self.subdued("Loading queue…").into();
        };
        let items: Vec<&QueueItem> = self.queue_items();
        // Queued songs always play before the context's.
        let queued: usize = items.iter().take_while(|item| item.explicit).count();
        let mut slots: Vec<Slot> = Vec::with_capacity(items.len() + 2);
        if queued > 0 {
            slots.push(Slot::Queued);
        }
        slots.extend(items[..queued].iter().map(|item| Slot::Item(item)));
        slots.push(Slot::Context);
        slots.extend(items[queued..].iter().map(|item| Slot::Item(item)));
        let height: f32 = p.shape.row_height;
        let (start, end) =
            visible_rows(slots.len(), self.queue_scroll, self.queue_viewport, height);
        let mut rows = column![space().height(start as f32 * height)];
        for slot in &slots[start..end] {
            rows = rows.push(match slot {
                Slot::Queued => self.queue_heading(
                    "Next in queue",
                    Some(
                        button(self.subdued("Clear queue"))
                            .padding(0)
                            .style(styles::link(p))
                            .on_press(Message::Action(Action::ClearQueue))
                            .into(),
                    ),
                ),
                Slot::Context => self.queue_heading("Next up", None),
                Slot::Item(item) => self.queue_item(item),
            });
        }
        rows = rows.push(space().height((slots.len() - end) as f32 * height));
        if items.len() == queued {
            rows = rows.push(self.subdued("Nothing else is coming up"));
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

    /// A section heading row, with an optional action at its right.
    fn queue_heading<'a>(&self, title: &'a str, action: Option<Element<'a>>) -> Element<'a> {
        let mut cells =
            row![self.strong(title), space().width(Length::Fill)].align_y(Alignment::Center);
        if let Some(action) = action {
            cells = cells.push(action);
        }
        container(cells.padding(iced::Padding::ZERO.right(GUTTER)))
            .height(self.palette.shape.row_height)
            .align_y(Alignment::Center)
            .into()
    }

    /// An upcoming song; songs the user queued can be removed.
    fn queue_item<'a>(&'a self, item: &'a QueueItem) -> Element<'a> {
        let remove: Option<Message> =
            item.explicit
                .then_some(Message::Action(Action::RemoveQueued {
                    entry_id: item.entry.entry_id,
                }));
        match &item.track {
            Some(track) => self.queue_row(track, false, remove),
            None => container(self.subdued("Metadata unavailable"))
                .height(self.palette.shape.row_height)
                .align_y(Alignment::Center)
                .into(),
        }
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
        self.faded(smooth_wheel(
            scrollable(rows.padding(iced::Padding::ZERO.bottom(FADE)))
                .id(Area::Queue.id())
                .on_scroll(|v| Message::Scrolled(Area::Queue, Metrics::of(&v)))
                .direction(gutter_scrollbar())
                .style(styles::scrollbar(&self.palette))
                .height(Length::Fill),
            Area::Queue,
        ))
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
        let width: f32 = self.panel_widths().1
            - 2.0 * (p.shape.padding + styles::shadow_margin(p))
            - THUMB
            - 12.0
            - remove_width
            - GUTTER;
        let mut cells = row![
            self.cover(track.artwork_url(), THUMB, Shape::Cover),
            column![
                button(
                    self.label(self.fit(&track.name, width, p.font_size.body))
                        .wrapping(Wrapping::None)
                        .color(if current {
                            p.colors.accent
                        } else {
                            p.colors.text
                        })
                )
                .padding(0)
                .style(styles::link(p))
                .on_press(Message::TrackMenu(Box::new(track.clone()))),
                self.artist_link(track, width),
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
