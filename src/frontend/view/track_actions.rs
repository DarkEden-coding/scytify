//! One track action sheet shared by tables, search, the player and queue.
use super::*;
use iced::widget::{column, rule};

impl Ui {
    pub(super) fn track_actions(&self, track: &Track) -> Element<'_> {
        let p = &self.palette;
        let title = column![
            self.strong(self.fit_bold(&track.name, 340.0, p.font_size.body)),
            self.subdued(self.fit(&artist_names(&track.artists), 340.0, p.font_size.small))
                .wrapping(Wrapping::None),
        ]
        .spacing(2)
        .width(Length::Fill);
        let header = row![
            title,
            self.icon_button(Icon::Close, 16.0, false, Message::CloseTrackMenu)
        ]
        .spacing(p.shape.spacing)
        .align_y(Alignment::Center);
        let mut actions = column![header, rule::horizontal(1).style(styles::divider(p))].spacing(8);
        if track.playable {
            actions = actions.push(self.menu_item(
                Icon::Play,
                "Play song",
                Message::PlayTrack(PlaySource::Track(track.id.clone()), Box::new(track.clone())),
            ));
            if !self.state.connect.is_remote() {
                actions = actions.push(self.menu_item(
                    Icon::ListEnd,
                    "Play next",
                    Message::Action(Action::PlayNext(vec![track.id.clone()])),
                ));
            }
            actions = actions.push(self.menu_item(
                Icon::ListPlus,
                "Add to queue",
                Message::Action(Action::AddToQueue(vec![track.id.clone()])),
            ));
        } else {
            actions = actions
                .push(self.subdued("Spotify reports this song as unavailable for playback."));
        }
        let liked: bool = self.is_saved(&LibraryItem::Track(track.id.clone()));
        actions = actions.push(self.menu_item(
            Icon::Heart,
            if liked {
                "Remove from Liked Songs"
            } else {
                "Add to Liked Songs"
            },
            Message::ToggleSaved(library::Entry::Track(track.clone())),
        ));
        if let Some(album) = &track.album {
            actions = actions.push(self.menu_item(
                Icon::Album,
                format!("Go to album: {}", album.name),
                Message::Navigate(Route::album(album)),
            ));
        }
        for artist in &track.artists {
            actions = actions.push(self.menu_item(
                Icon::User,
                format!("Go to artist: {}", artist.name),
                Message::Navigate(Route::artist(artist)),
            ));
        }
        actions = actions.push(self.menu_item(
            Icon::Link,
            "Copy song link",
            Message::CopyLink(format!("https://open.spotify.com/track/{}", track.id.0)),
        ));
        let sheet = container(iced::widget::scrollable(actions).height(Length::Shrink))
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
        .on_press(Message::CloseTrackMenu)
        .into()
    }

    /// A full-width row of the action sheet: icon, label, and the action on press.
    fn menu_item<'a>(
        &self,
        which: Icon,
        label: impl text::IntoFragment<'a>,
        message: Message,
    ) -> Element<'a> {
        button(self.icon_label(which, label))
            .padding([8.0, 10.0])
            .width(Length::Fill)
            .style(styles::list_row(&self.palette, false))
            .on_press(message)
            .into()
    }
}
