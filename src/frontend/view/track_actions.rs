//! One track action sheet shared by tables, search, the player and queue.
use super::*;
use iced::widget::column;
impl Ui {
    pub(super) fn track_actions(&self, track: &Track) -> Element<'_> {
        let p = &self.palette;
        let mut actions = column![
            self.strong(track.name.clone()),
            self.control(self.label("Close"))
                .on_press(Message::CloseTrackMenu),
        ]
        .spacing(8);
        if track.playable {
            actions = actions.push(self.control(self.label("Play song")).on_press(
                Message::PlayTrack(PlaySource::Track(track.id.clone()), Box::new(track.clone())),
            ));
            if !self.state.connect.is_remote() {
                actions = actions.push(
                    self.control(self.label("Play next"))
                        .on_press(Message::Action(Action::PlayNext(vec![track.id.clone()]))),
                );
            }
            actions = actions.push(
                self.control(self.label("Add to queue"))
                    .on_press(Message::Action(Action::AddToQueue(vec![track.id.clone()]))),
            );
        } else {
            actions = actions
                .push(self.subdued("Spotify reports this song as unavailable for playback."));
        }
        actions = actions.push(self.like_button(track));
        if let Some(album) = &track.album {
            actions = actions.push(
                self.control(self.label(format!("Go to album: {}", album.name)))
                    .on_press(Message::Navigate(Route::album(album))),
            );
        }
        for artist in &track.artists {
            actions = actions.push(
                self.control(self.label(format!("Go to artist: {}", artist.name)))
                    .on_press(Message::Navigate(Route::artist(artist))),
            );
        }
        actions = actions.push(self.control(self.label("Copy Spotify song link")).on_press(
            Message::CopyLink(format!("https://open.spotify.com/track/{}", track.id.0)),
        ));
        let sheet = container(iced::widget::scrollable(actions).height(Length::Shrink))
            .padding(20)
            .max_width(460)
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
}
