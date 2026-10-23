//! Spotify Connect: the device picker popover and the "Playing on ..." banner.
use super::*;
use crate::frontend::devices;
use iced::widget::{column, mouse_area, opaque, rule, scrollable};

/// Height of the strip below the player while another device is playing.
pub(super) const BANNER_HEIGHT: f32 = 28.0;
const POPOVER_WIDTH: f32 = 340.0;
/// Most the device list grows before it scrolls.
const LIST_HEIGHT: f32 = 300.0;
const ICON: f32 = 22.0;
/// Gap between the popover and the bar it opens from.
const ANCHOR_GAP: f32 = 8.0;

impl Ui {
    /// Name of the device the banner points at.
    fn remote_name(&self) -> &str {
        devices::active(&self.state.connect)
            .map(|device| device.name.as_str())
            .unwrap_or("another device")
    }

    /// Full-width accent strip; pressing it opens the picker. Only while a remote device plays.
    pub(super) fn connect_banner(&self) -> Option<Element<'_>> {
        let connect: &ConnectState = &self.state.connect;
        if !connect.is_remote() {
            return None;
        }
        let p: &Palette = &self.palette;
        let kind: Icon = devices::active(connect)
            .map(|device| devices::icon(&device.device_type))
            .unwrap_or(Icon::Speaker);
        let width: f32 = (self.window_size.width * 0.5).max(160.0);
        let message: String = format!("Playing on {}", self.remote_name());
        let mut content = row![space().width(Length::Fill)]
            .spacing(p.shape.spacing)
            .align_y(Alignment::Center);
        if p.icons == Icons::Lucide {
            content = content.push(icons::icon(kind, 16.0));
        }
        content = content.push(
            self.strong(self.fit_bold(&message, width, p.font_size.small))
                .size(p.font_size.small),
        );
        Some(
            button(
                container(content)
                    .height(Length::Fill)
                    .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .height(BANNER_HEIGHT)
            .padding([0.0, p.shape.spacing * 2.0])
            .style(styles::banner(p, iced::border::bottom(self.window_radius())))
            .on_press(Message::Devices)
            .into(),
        )
    }

    /// Click-away layer with the picker anchored above the now-playing bar's right end.
    pub(super) fn devices_overlay(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let banner: f32 = if self.state.connect.is_remote() {
            BANNER_HEIGHT
        } else {
            0.0
        };
        let anchored = container(opaque(self.devices_popover()))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::End)
            .padding(iced::Padding {
                top: 0.0,
                right: self.frame_edge() + p.shape.spacing,
                bottom: PLAYER_HEIGHT + banner + ANCHOR_GAP,
                left: 0.0,
            });
        mouse_area(anchored).on_press(Message::Devices).into()
    }

    fn devices_popover(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let connect: &ConnectState = &self.state.connect;
        let gap: f32 = p.shape.spacing;
        let mut content = column![].spacing(gap * 1.5);
        if let Some(current) = devices::active(connect) {
            content = content.push(self.strong("Current device"));
            content = content.push(self.current_device_row(current));
        }
        let others = devices::others(connect);
        content = content.push(self.strong("Select another device"));
        if others.is_empty() {
            content = content.push(self.no_devices());
        } else {
            let list = column(others.into_iter().map(|device| self.device_row(device))).spacing(2);
            content = content.push(
                container(scrollable(list).style(styles::scrollbar(p))).max_height(LIST_HEIGHT),
            );
        }
        if let Some((hint, error)) = devices::source_hint(&connect.source) {
            let color = if error {
                p.colors.error
            } else {
                p.colors.text_subdued
            };
            content = content.push(rule::horizontal(1).style(styles::divider(p)));
            content = content.push(self.subdued(hint).color(color));
        }
        if matches!(self.state.auth, AuthState::Ready { .. })
            && !matches!(connect.playback_auth, PlaybackAuth::Ready)
        {
            content = content.push(rule::horizontal(1).style(styles::divider(p)));
            content = content.push(self.playback_authorization());
        }
        container(content)
            .width(POPOVER_WIDTH)
            .padding(p.shape.padding)
            .style(styles::popover(p))
            .into()
    }

    /// The playing device: type icon and name in the accent color, equalizer on the right.
    fn current_device_row<'a>(&self, device: &'a PlaybackDevice) -> Element<'a> {
        let p: &Palette = &self.palette;
        let accent = p.colors.accent;
        let width: f32 = POPOVER_WIDTH - 2.0 * p.shape.padding - ICON - 16.0 - 2.0 * 12.0;
        let mut cells = row![].spacing(12).align_y(Alignment::Center);
        if p.icons == Icons::Lucide {
            cells = cells.push(icons::icon(devices::icon(&device.device_type), ICON).color(accent));
        }
        cells = cells.push(
            column![
                self.strong(self.fit_bold(&device.name, width, p.font_size.body))
                    .color(accent),
                self.subdued(devices::kind_label(device)),
            ]
            .spacing(2)
            .width(Length::Fill),
        );
        if p.icons == Icons::Lucide {
            cells = cells.push(icons::icon(Icon::Equalizer, 16.0).color(accent));
        }
        container(cells).padding([4.0, 0.0]).into()
    }

    /// One selectable device; restricted ones are disabled and say why.
    fn device_row<'a>(&self, device: &'a PlaybackDevice) -> Element<'a> {
        let p: &Palette = &self.palette;
        let target: Option<&str> = devices::transfer_id(device).filter(|_| {
            !device.is_this_app || matches!(self.state.connect.playback_auth, PlaybackAuth::Ready)
        });
        let width: f32 = POPOVER_WIDTH - 2.0 * p.shape.padding - ICON - 12.0 - 20.0;
        let subtitle: &str = if device.is_this_app
            && !matches!(self.state.connect.playback_auth, PlaybackAuth::Ready)
        {
            "Enable playback to use this computer"
        } else if target.is_none() {
            "Can't play on this device right now"
        } else {
            devices::kind_label(device)
        };
        let name = self.label(self.fit(&device.name, width, p.font_size.body));
        let mut cells = row![].spacing(12).align_y(Alignment::Center);
        if p.icons == Icons::Lucide {
            cells = cells.push(icons::icon(devices::icon(&device.device_type), ICON));
        }
        cells = cells.push(
            column![
                if target.is_none() {
                    name.color(p.colors.text_subdued)
                } else {
                    name
                },
                self.subdued(self.fit(subtitle, width, p.font_size.small)),
            ]
            .spacing(2)
            .width(Length::Fill),
        );
        let mut control = button(cells)
            .width(Length::Fill)
            .padding([8.0, 10.0])
            .style(styles::list_row(p, false));
        if let Some(id) = target {
            control = control.on_press(Message::Transfer(id.to_owned()));
        }
        control.into()
    }

    fn no_devices(&self) -> Element<'_> {
        column![
            self.label("No other devices found"),
            self.subdued("Open Spotify on another device")
        ]
        .spacing(2)
        .padding([8.0, 0.0])
        .into()
    }
}
