//! "Save in" sheet of a song, opened from a liked song's heart: Liked Songs, its album and
//! the user's own playlists, each toggled in place.
use super::*;
use iced::widget::{column, rule};

impl Ui {
    pub(super) fn save_sheet(&self, track: &Track) -> Element<'_> {
        let p = &self.palette;
        let title = column![
            self.strong("Save in").size(p.font_size.body + 4.0),
            self.subdued(self.fit(&track.name, 340.0, p.font_size.small))
                .wrapping(Wrapping::None),
        ]
        .spacing(4)
        .width(Length::Fill);
        let header = row![
            title,
            self.icon_button(Icon::Close, 16.0, false, Message::CloseSaveSheet)
        ]
        .spacing(p.shape.spacing)
        .align_y(Alignment::Center);
        let mut rows = column![header, rule::horizontal(1).style(styles::divider(p))].spacing(2);
        let liked: bool = self.is_saved(&LibraryItem::Track(track.id.clone()));
        let heart: Element<'_> = if liked && p.icons == Icons::Lucide {
            iced::widget::image(self.glyphs.heart.clone())
                .width(icons::SOLID_HEART)
                .height(icons::SOLID_HEART)
                .into()
        } else {
            glyph(p, Icon::Heart, 16.0).into()
        };
        rows = rows.push(self.save_row(
            heart,
            "Liked Songs".to_owned(),
            liked,
            Message::ToggleSaved(library::Entry::Track(track.clone())),
        ));
        if let Some(album) = &track.album {
            rows = rows.push(self.save_row(
                glyph(p, Icon::Album, 16.0).into(),
                format!("Album: {}", album.name),
                self.is_saved(&LibraryItem::Album(album.id.clone())),
                Message::ToggleSaved(library::Entry::Album(album.clone())),
            ));
        }
        let playlists: Vec<&Playlist> = self.editable_playlists();
        if !playlists.is_empty() {
            rows = rows
                .push(container(rule::horizontal(1).style(styles::divider(p))).padding([6, 0]))
                .push(container(self.subdued("Your playlists")).padding([4.0, 10.0]));
        }
        for playlist in playlists {
            rows = rows.push(self.save_row(
                glyph(p, Icon::ListMusic, 16.0).into(),
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

    /// One destination: its icon and name, then a filled accent check while the song is
    /// saved there, or an outlined plus of the same size.
    fn save_row<'a>(
        &self,
        icon: Element<'a>,
        label: String,
        saved: bool,
        message: Message,
    ) -> Element<'a> {
        const MARK: f32 = 20.0;
        let p: &Palette = &self.palette;
        let mark: Element<'a> = match (p.icons, saved) {
            (Icons::Lucide, true) => {
                container(icons::icon(Icon::Check, MARK * 0.6).color(p.colors.on_accent))
                    .center(MARK)
                    .style(styles::dot(p))
                    .into()
            }
            (Icons::Lucide, false) => {
                container(icons::icon(Icon::CirclePlus, MARK).color(p.colors.text_subdued))
                    .center(MARK)
                    .into()
            }
            (Icons::Text, _) => self.subdued(if saved { "Saved" } else { "Add" }).into(),
        };
        let label = self.fit(&label, 290.0, p.font_size.body);
        let mut content = row![].spacing(12).align_y(Alignment::Center);
        if p.icons == Icons::Lucide {
            content = content.push(container(icon).center(20));
        }
        button(
            content
                .push(container(self.label(label)).width(Length::Fill))
                .push(mark),
        )
        .padding([10.0, 10.0])
        .width(Length::Fill)
        .style(styles::list_row(p, false))
        .on_press(message)
        .into()
    }
}
