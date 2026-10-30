//! "Save in" sheet of a song, opened from a liked song's heart: Liked Songs, its album and
//! the user's own playlists, each toggled in place.
use super::*;
use iced::widget::{column, rule};

impl Ui {
    pub(super) fn save_sheet(&self, track: &Track) -> Element<'_> {
        let p = &self.palette;
        let title = column![
            self.strong("Save in"),
            self.subdued(self.fit(&track.name, 340.0, p.font_size.small))
                .wrapping(Wrapping::None),
        ]
        .spacing(2)
        .width(Length::Fill);
        let header = row![
            title,
            self.icon_button(Icon::Close, 16.0, false, Message::CloseSaveSheet)
        ]
        .spacing(p.shape.spacing)
        .align_y(Alignment::Center);
        let mut rows = column![header, rule::horizontal(1).style(styles::divider(p))].spacing(8);
        rows = rows.push(self.save_row(
            Icon::Heart,
            "Liked Songs".to_owned(),
            self.is_saved(&LibraryItem::Track(track.id.clone())),
            Message::ToggleSaved(library::Entry::Track(track.clone())),
        ));
        if let Some(album) = &track.album {
            rows = rows.push(self.save_row(
                Icon::Album,
                format!("Album: {}", album.name),
                self.is_saved(&LibraryItem::Album(album.id.clone())),
                Message::ToggleSaved(library::Entry::Album(album.clone())),
            ));
        }
        let playlists: Vec<&Playlist> = self.editable_playlists();
        if !playlists.is_empty() {
            rows = rows
                .push(rule::horizontal(1).style(styles::divider(p)))
                .push(self.subdued("Your playlists"));
        }
        for playlist in playlists {
            rows = rows.push(self.save_row(
                Icon::ListMusic,
                playlist.name.clone(),
                self.in_playlist(&playlist.id, &track.id),
                Message::ToggleInPlaylist(playlist.id.clone(), track.id.clone()),
            ));
        }
        let sheet = container(iced::widget::scrollable(rows).height(Length::Shrink))
            .padding(16)
            .width(400)
            .max_height((self.window_size.height - 80.0).max(200.0))
            .style(styles::popover(p));
        iced::widget::mouse_area(
            container(iced::widget::opaque(sheet))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .on_press(Message::CloseSaveSheet)
        .into()
    }

    /// One destination: its name, then a check while the song is saved there or a plus.
    fn save_row<'a>(
        &self,
        which: Icon,
        label: String,
        saved: bool,
        message: Message,
    ) -> Element<'a> {
        let p: &Palette = &self.palette;
        let mark = glyph(p, if saved { Icon::Check } else { Icon::CirclePlus }, 16.0);
        let mark = if saved {
            mark.color(p.colors.accent)
        } else {
            mark
        };
        let label = self.fit(&label, 300.0, p.font_size.body);
        button(
            row![
                container(self.icon_label(which, label)).width(Length::Fill),
                mark
            ]
            .align_y(Alignment::Center),
        )
        .padding([8.0, 10.0])
        .width(Length::Fill)
        .style(styles::list_row(p, false))
        .on_press(message)
        .into()
    }
}
