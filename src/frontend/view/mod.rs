//! Native layout: sidebar, top bar, main view, optional queue and the now-playing bar.
//! Sizes come from `layout` and the theme tokens; colors only from the theme.
mod browse;
mod devices;
mod player;
mod queue;
mod search;
mod settings;
mod sidebar;
mod topbar;
mod tracklist;

use super::icons::{self, Icon, glyph};
use super::smooth_scroll::{Area, Metrics};
use super::theme::Icons;
use super::*;
use iced::widget::text::Wrapping;
use iced::widget::{Text, button, column, container, row, space, stack, text};
use iced::{Alignment, Length};

/// Height of the now-playing bar.
const PLAYER_HEIGHT: f32 = 80.0;

/// Make wheel ticks over a scrollable glide instead of stepping.
fn smooth_wheel<'a>(scroll: impl Into<Element<'a>>, area: Area) -> Element<'a> {
    super::smooth_scroll::wheel(
        scroll,
        move |delta| Message::Wheel(area, delta),
        Message::GlideStop(area),
    )
    .into()
}

impl Ui {
    /// Render the fixed application frame.
    pub(super) fn view(&self, _: window::Id) -> Element<'_> {
        let mut body = row![self.sidebar_view(), self.main_view()]
            .spacing(self.panel_gap())
            .height(Length::Fill);
        if self.queue {
            body = body.push(self.queue_view());
        }
        let edge: f32 = self.frame_edge();
        let frame = container(
            column![body, container(self.player_view()).height(PLAYER_HEIGHT)].spacing(edge),
        )
        .style(styles::root(&self.palette))
        .padding(iced::Padding::new(edge).bottom(0.0))
        .width(Length::Fill)
        .height(Length::Fill);
        let mut window = column![frame].width(Length::Fill).height(Length::Fill);
        if let Some(banner) = self.connect_banner() {
            window = window.push(banner);
        }
        let mut layers = stack![window];
        if self.devices {
            layers = layers.push(self.devices_overlay());
        }
        self.with_resize_grips(layers.into())
    }

    /// Top bar and the scrolling page inside one surface panel, with notices above it.
    fn main_view(&self) -> Element<'_> {
        let page: Element<'_> = if self.settings {
            self.settings_view()
        } else {
            self.browser_view()
        };
        let panel = self.panel(column![self.top_bar(), page].height(Length::Fill), 0.0);
        let mut layers = stack![panel].width(Length::Fill).height(Length::Fill);
        if !self.notice.is_empty() {
            layers = layers.push(
                container(self.toast())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::End)
                    .padding(layout::CONTENT_PADDING),
            );
        }
        layers.into()
    }

    /// Dismissible notice for errors and warnings.
    fn toast(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let mut content = row![]
            .spacing(p.shape.spacing * 1.5)
            .align_y(Alignment::Center);
        if p.icons == Icons::Lucide {
            content = content.push(icons::icon(Icon::Alert, 18.0).color(p.colors.error));
        }
        content = content
            .push(self.label(&self.notice).width(Length::Fill))
            .push(self.icon_button(Icon::Close, 16.0, false, Message::DismissNotice));
        container(content)
            .max_width(560)
            .padding([10.0, p.shape.padding])
            .style(styles::toast(p))
            .into()
    }

    /// Padding around the panels. Raised panels draw their shadow inside a margin, so the
    /// padding shrinks by that margin to keep the visible gaps at `spacing`.
    pub(super) fn frame_edge(&self) -> f32 {
        (self.palette.shape.spacing - styles::shadow_margin(&self.palette)).max(0.0)
    }

    /// Layout spacing between two raised panels, so their visible gap is `spacing`.
    pub(super) fn panel_gap(&self) -> f32 {
        (self.palette.shape.spacing - 2.0 * styles::shadow_margin(&self.palette)).max(0.0)
    }

    /// Wrap content in a theme surface panel with the given inner padding.
    pub(super) fn panel<'a>(&self, child: impl Into<Element<'a>>, padding: f32) -> Element<'a> {
        let p: &Palette = &self.palette;
        styles::raised(
            p,
            p.shape.radius_large,
            container(child)
                .padding(padding)
                .width(Length::Fill)
                .height(Length::Fill)
                .clip(true)
                .style(styles::panel(p)),
        )
    }

    /// Body text in the theme family.
    pub(super) fn label<'a>(&self, value: impl text::IntoFragment<'a>) -> Text<'a> {
        text(value)
            .font(self.palette.font())
            .size(self.palette.font_size.body)
    }
    /// Bold single-line text, for titles and names.
    pub(super) fn strong<'a>(&self, value: impl text::IntoFragment<'a>) -> Text<'a> {
        text(value)
            .font(self.palette.bold_font())
            .size(self.palette.font_size.body)
            .wrapping(Wrapping::None)
    }
    /// Small secondary text such as metadata and durations.
    pub(super) fn subdued<'a>(&self, value: impl text::IntoFragment<'a>) -> Text<'a> {
        text(value)
            .font(self.palette.font())
            .size(self.palette.font_size.small)
            .color(self.palette.colors.text_subdued)
    }
    /// Section heading such as "Your playlists".
    pub(super) fn heading<'a>(&self, value: impl text::IntoFragment<'a>) -> Text<'a> {
        text(value)
            .font(self.palette.bold_font())
            .size(self.palette.font_size.heading)
            .wrapping(Wrapping::None)
    }

    /// `value` in the theme font, ellipsized to fit `width` at `size`.
    pub(super) fn fit(&self, value: &str, width: f32, size: f32) -> String {
        self.fits.fit(value, width, self.palette.font(), size)
    }
    /// Like [`Self::fit`], for bold titles.
    pub(super) fn fit_bold(&self, value: &str, width: f32, size: f32) -> String {
        self.fits.fit(value, width, self.palette.bold_font(), size)
    }

    /// Transparent icon control; `active` draws it in the accent color.
    pub(super) fn icon_button<'a>(
        &self,
        which: Icon,
        size: f32,
        active: bool,
        message: Message,
    ) -> iced::widget::Button<'a, Message> {
        button(glyph(&self.palette, which, size))
            .padding(6)
            .style(styles::icon_button(&self.palette, active))
            .on_press(message)
    }

    /// Raised secondary button for ordinary actions.
    pub(super) fn control<'a>(
        &self,
        content: impl Into<Element<'a>>,
    ) -> iced::widget::Button<'a, Message> {
        button(content)
            .padding([9.0, self.palette.shape.padding])
            .style(styles::secondary_button(&self.palette))
    }

    /// Leading icon and label for a control; just the label for text-icon themes.
    pub(super) fn icon_label<'a>(&self, which: Icon, label: &'a str) -> Element<'a> {
        let mut content = row![].spacing(8).align_y(Alignment::Center);
        if self.palette.icons == Icons::Lucide {
            content = content.push(icons::icon(which, 16.0));
        }
        content.push(self.label(label)).into()
    }

    /// Accent pill button for the main action of a form.
    pub(super) fn primary<'a>(&self, label: &'a str, message: Message) -> Element<'a> {
        button(self.strong(label).color(self.palette.colors.on_accent))
            .padding([10.0, self.palette.shape.padding * 1.5])
            .style(styles::primary_button(&self.palette))
            .on_press(message)
            .into()
    }

    /// Solid play or pause on a round button: accent for page headers, inverse in the player.
    pub(super) fn round_play(&self, playing: bool, large: bool, message: Message) -> Element<'_> {
        let p: &Palette = &self.palette;
        let (edge, handle, style): (u32, _, _) = match (large, playing) {
            (true, false) => (icons::SOLID_LARGE, &self.glyphs.play_large, true),
            (true, true) => (icons::SOLID_LARGE, &self.glyphs.pause_large, true),
            (false, false) => (icons::SOLID_PLAYER, &self.glyphs.play, false),
            (false, true) => (icons::SOLID_PLAYER, &self.glyphs.pause, false),
        };
        let content: Element<'_> = if p.icons == Icons::Lucide {
            iced::widget::image(handle.clone())
                .width(edge)
                .height(edge)
                .into()
        } else {
            glyph(p, if playing { Icon::Pause } else { Icon::Play }, 16.0).into()
        };
        let padding: f32 = if large { 16.0 } else { 8.0 };
        let control = button(content).padding(padding).on_press(message);
        if style {
            control.style(styles::primary_button(p)).into()
        } else {
            control.style(styles::inverse_button(p)).into()
        }
    }
}
