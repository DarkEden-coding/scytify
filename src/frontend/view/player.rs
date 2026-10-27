//! Full-width now-playing bar: track, transport with progress, and output controls.
use super::*;
use crate::frontend::artwork::Shape;
use crate::frontend::devices;
use iced::widget::column;
use iced::widget::{progress_bar, slider, tooltip};

const ART: f32 = 56.0;
const VOLUME_WIDTH: f32 = 120.0;
const TIME_WIDTH: f32 = 40.0;

impl Ui {
    /// Render transport from authoritative anchors; extrapolation is clamped to track duration.
    pub(super) fn player_view(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let player: &PlayerState = &self.state.player;
        let duration: u64 = player.current.as_ref().map(|t| t.duration_ms).unwrap_or(0);
        let position: u64 = self.position_ms();
        let position: u64 = self
            .seek_hold
            .and_then(|held| held.shown())
            .map_or(position, |held| held.min(duration));
        let playing: bool = player.status == PlaybackStatus::Playing;
        let controls = row![
            self.toggle(
                Icon::Shuffle,
                player.shuffle,
                Message::Action(Action::SetShuffle(!player.shuffle))
            ),
            self.icon_button(
                Icon::SkipBack,
                20.0,
                false,
                Message::Action(Action::Previous)
            ),
            self.round_play(playing, false, Message::TogglePlayback),
            self.icon_button(
                Icon::SkipForward,
                20.0,
                false,
                Message::Action(Action::Next)
            ),
            self.toggle(
                if player.repeat == RepeatMode::One {
                    Icon::RepeatOne
                } else {
                    Icon::Repeat
                },
                player.repeat != RepeatMode::Off,
                Message::Action(Action::SetRepeat(next_repeat(player.repeat)))
            ),
        ]
        .spacing(p.shape.spacing * 2.0)
        .align_y(Alignment::Center);
        let progress = row![
            self.subdued(clock(position))
                .width(TIME_WIDTH)
                .align_x(iced::alignment::Horizontal::Right),
            slider(0.0..=duration.max(1) as f64, position as f64, |v| {
                Message::Seek(v as u64)
            })
            .on_release(Message::SeekReleased)
            .step(1000.0)
            .style(styles::slider(p)),
            self.subdued(clock(duration)).width(TIME_WIDTH),
        ]
        .spacing(p.shape.spacing)
        .align_y(Alignment::Center);
        let center = column![controls, progress]
            .spacing(6)
            .align_x(Alignment::Center)
            .max_width(640);
        let output = row![
            self.icon_button(Icon::Lyrics, 18.0, self.lyrics_open, Message::Lyrics),
            self.icon_button(Icon::ListMusic, 18.0, self.queue, Message::Queue),
            self.devices_button(),
            self.volume_controls(),
        ]
        .spacing(p.shape.spacing / 2.0)
        .align_y(Alignment::Center);
        row![
            container(self.now_playing())
                .width(Length::FillPortion(3))
                .align_y(Alignment::Center),
            container(center)
                .width(Length::FillPortion(4))
                .center_x(Length::FillPortion(4)),
            container(output)
                .width(Length::FillPortion(3))
                .align_x(Alignment::End),
        ]
        .padding([0.0, p.shape.spacing])
        .height(Length::Fill)
        .align_y(Alignment::Center)
        .into()
    }

    /// Connect button; accent while another device plays.
    fn devices_button(&self) -> Element<'_> {
        self.icon_button(
            Icon::Devices,
            18.0,
            self.state.connect.is_remote(),
            Message::Devices,
        )
        .into()
    }

    /// Mute button and volume slider. A remote device that cannot set its volume gets an
    /// inert rail and a hint instead.
    fn volume_controls(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let volume: f32 = self
            .volume_hold
            .and_then(|held| held.shown())
            .unwrap_or(self.state.player.volume);
        let icon: Icon = if volume <= 0.0 {
            Icon::VolumeMuted
        } else if volume < 0.5 {
            Icon::VolumeLow
        } else {
            Icon::Volume
        };
        if devices::volume_supported(&self.state.connect) {
            return row![
                self.icon_button(icon, 18.0, false, Message::ToggleMute),
                slider(0.0..=1.0, volume, Message::Volume)
                    .on_release(Message::VolumeReleased)
                    .step(0.01_f32)
                    .width(VOLUME_WIDTH)
                    .style(styles::slider(p)),
            ]
            .spacing(p.shape.spacing / 2.0)
            .align_y(Alignment::Center)
            .into();
        }
        let inert = row![
            button(glyph(p, icon, 18.0))
                .padding(6)
                .style(styles::icon_button(p, false)),
            container(
                progress_bar(0.0..=1.0, volume)
                    .girth(4)
                    .length(VOLUME_WIDTH)
                    .style(styles::inert_rail(p)),
            )
            .padding([6.0, 0.0]),
        ]
        .spacing(p.shape.spacing / 2.0)
        .align_y(Alignment::Center);
        tooltip(
            inert,
            container(self.subdued("This device doesn't support volume control"))
                .padding([6.0, 10.0])
                .style(styles::popover(p)),
            tooltip::Position::Top,
        )
        .into()
    }

    /// Cover, bold title and subdued artist of the current track.
    fn now_playing(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let Some(track) = &self.state.player.current else {
            return row![
                self.cover(None, ART, Shape::Cover),
                self.subdued("Choose something from your library"),
            ]
            .spacing(14)
            .align_y(Alignment::Center)
            .into();
        };
        // The left column is three tenths of the bar, minus the cover, the heart and gaps.
        let width: f32 =
            (self.window_size.width * 0.3 - ART - super::tracklist::LIKE_WIDTH - 52.0).max(80.0);
        let album = track
            .album
            .as_ref()
            .map(|album| Message::Navigate(Route::album(album)))
            .unwrap_or_else(|| Message::TrackMenu(Box::new(track.clone())));
        iced::widget::mouse_area(
            row![
                button(self.cover(track.artwork_url(), ART, Shape::Cover))
                    .padding(0)
                    .style(styles::link(p))
                    .on_press(album.clone()),
                column![
                    button(self.strong(self.fit_bold(&track.name, width, p.font_size.body)))
                        .padding(0)
                        .style(styles::link(p))
                        .on_press(album.clone()),
                    self.artist_link(track, width),
                ]
                .spacing(4),
                self.like_button(track),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .on_right_press(Message::TrackMenu(Box::new(track.clone())))
        .into()
    }

    /// Mode toggle with a small accent dot below while it is on.
    fn toggle(&self, which: Icon, active: bool, message: Message) -> Element<'_> {
        let control = self.icon_button(which, 18.0, active, message);
        if !active {
            return control.into();
        }
        stack![
            control,
            container(
                container(space())
                    .width(4)
                    .height(4)
                    .style(styles::dot(&self.palette))
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::End),
        ]
        .into()
    }
}
