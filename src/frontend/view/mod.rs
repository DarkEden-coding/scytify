//! Native layout: sidebar, top bar, main view, optional queue and the now-playing bar.
//! Sizes come from `layout` and the theme tokens; colors only from the theme.
mod browse;
mod devices;
mod lyrics;
mod player;
mod queue;
mod save_sheet;
mod search;
mod settings;
mod sidebar;
mod topbar;
mod track_actions;
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
/// Space between a notice and the player bar below it.
const TOAST_GAP: f32 = 12.0;
/// Width of the grab area over a panel gap.
const SPLIT_GRIP: f32 = 8.0;

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
        } else if self.lyrics_open {
            body = body.push(self.lyrics_view());
        }
        let edge: f32 = self.frame_edge();
        let banner = self.connect_banner();
        // The banner takes over the bottom corners when it sits under the frame.
        let corners: iced::border::Radius = if banner.is_some() {
            iced::border::top(self.window_radius())
        } else {
            self.window_radius().into()
        };
        let frame = container(
            column![body, container(self.player_view()).height(PLAYER_HEIGHT)].spacing(edge),
        )
        .style(styles::root(&self.palette, corners))
        .padding(iced::Padding::new(edge).bottom(0.0))
        .width(Length::Fill)
        .height(Length::Fill);
        let mut window = column![frame].width(Length::Fill).height(Length::Fill);
        if let Some(banner) = banner {
            window = window.push(banner);
        }
        let mut layers = stack![window, self.split_handles()];
        if !self.notice.is_empty() {
            // Centered over the whole window, floating just above the player bar.
            layers = layers.push(
                container(self.toast())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::End)
                    .padding(iced::Padding::ZERO.bottom(PLAYER_HEIGHT + edge + TOAST_GAP)),
            );
        }
        if self.devices {
            layers = layers.push(self.devices_overlay());
        }
        if let Some(track) = &self.track_menu {
            layers = layers.push(self.track_actions(track));
        }
        if let Some(track) = &self.save_sheet {
            layers = layers.push(self.save_sheet(track));
        }
        self.with_resize_grips(layers.into())
    }

    /// Top bar and the scrolling page inside one surface panel.
    fn main_view(&self) -> Element<'_> {
        let page: Element<'_> = if self.settings {
            self.settings_view()
        } else {
            self.browser_view()
        };
        self.panel(column![self.top_bar(), page].height(Length::Fill), 0.0)
    }

    /// Invisible grips over the gaps between panels that resize them when dragged.
    fn split_handles(&self) -> Element<'_> {
        let edge: f32 = self.frame_edge();
        let gap: f32 = self.panel_gap();
        let margin: f32 = styles::shadow_margin(&self.palette);
        let (sidebar, right) = self.panel_widths();
        let grip = |handle| -> Element<'_> {
            iced::widget::mouse_area(space().width(SPLIT_GRIP).height(Length::Fill))
                .on_press(Message::SplitStart(handle))
                .interaction(iced::mouse::Interaction::ResizingHorizontally)
                .into()
        };
        let mut handles = row![
            space().width(edge + sidebar + gap / 2.0 - SPLIT_GRIP / 2.0),
            grip(splits::Handle::Sidebar),
            space().width(Length::Fill),
        ];
        if self.right_panel().is_some() {
            handles = handles
                .push(grip(splits::Handle::Right))
                .push(space().width(edge + right + gap / 2.0 + 2.0 * margin - SPLIT_GRIP / 2.0));
        }
        container(handles)
            .height(Length::Fill)
            .padding(iced::Padding::new(edge).bottom(PLAYER_HEIGHT + edge))
            .into()
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
            .push(self.label(&self.notice).wrapping(Wrapping::WordOrGlyph))
            .push(self.icon_button(Icon::Close, 14.0, false, Message::DismissNotice));
        container(content)
            .max_width(520)
            .padding(iced::Padding::from([8.0, p.shape.padding]).right(8.0))
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
        let hint = match which {
            Icon::Play => "Play",
            Icon::Pause => "Pause",
            Icon::SkipBack => "Previous song",
            Icon::SkipForward => "Next song",
            Icon::Shuffle => "Toggle shuffle",
            Icon::Repeat | Icon::RepeatOne => "Cycle repeat mode",
            Icon::Volume | Icon::VolumeLow | Icon::VolumeMuted => "Mute or restore volume",
            Icon::ListMusic => "Open or close queue",
            Icon::ListPlus => "Add to queue",
            Icon::ListEnd => "Play next",
            Icon::Library => "Your library",
            Icon::Search => "Search",
            Icon::Settings => "Settings",
            Icon::Home => "Home",
            Icon::ChevronLeft => "Back",
            Icon::ChevronRight => "Forward",
            Icon::Close => "Close or remove",
            Icon::LogOut => "Sign out",
            Icon::Power => "Quit Scytify",
            Icon::Heart => "Like song",
            Icon::CirclePlus => "Save to library",
            Icon::Check => "Saved to library",
            Icon::SearchX => "Clear search",
            Icon::Minimize => "Minimize",
            Icon::Maximize => "Maximize",
            Icon::Restore => "Restore window",
            Icon::Devices => "Choose playback device",
            Icon::Lyrics => "Open or close lyrics",
            _ => "Open details",
        };
        button(self.tip(glyph(&self.palette, which, size), hint))
            .padding(6)
            .style(styles::icon_button(&self.palette, active))
            .on_press(message)
    }

    /// Like toggle of a song: an outlined heart, or a filled accent heart while liked.
    pub(super) fn like_button<'a>(&self, track: &Track) -> Element<'a> {
        let p: &Palette = &self.palette;
        let liked: bool = self.is_saved(&LibraryItem::Track(track.id.clone()));
        let content: Element<'a> = match (p.icons, liked) {
            (Icons::Lucide, true) => iced::widget::image(self.glyphs.heart.clone())
                .width(icons::SOLID_HEART)
                .height(icons::SOLID_HEART)
                .into(),
            (Icons::Lucide, false) => icons::icon(Icon::Heart, 16.0).into(),
            (Icons::Text, _) => self.label(if liked { "Liked" } else { "Like" }).into(),
        };
        // Like Spotify, the first press likes the song; once liked, it opens the save sheet.
        let (tip, message): (&str, Message) = if liked {
            ("Save in…", Message::SaveSheet(Box::new(track.clone())))
        } else {
            (
                "Add to Liked Songs",
                Message::ToggleSaved(library::Entry::Track(track.clone())),
            )
        };
        self.tip(
            button(content)
                .padding(6)
                .style(styles::icon_button(p, liked))
                .on_press(message),
            tip,
        )
    }

    /// Your Library toggle of an album or playlist page: a plus in a circle, or a check on
    /// a filled accent circle while saved.
    pub(super) fn save_button<'a>(&self, entry: library::Entry) -> Element<'a> {
        const EDGE: f32 = 32.0;
        let p: &Palette = &self.palette;
        let saved: bool = self.is_saved(&entry.item());
        let content: Element<'a> = match (p.icons, saved) {
            (Icons::Lucide, true) => {
                container(icons::icon(Icon::Check, EDGE * 0.55).color(p.colors.on_accent))
                    .center(EDGE * 0.92)
                    .style(styles::dot(p))
                    .into()
            }
            (Icons::Lucide, false) => icons::icon(Icon::CirclePlus, EDGE).into(),
            (Icons::Text, _) => {
                glyph(p, if saved { Icon::Check } else { Icon::CirclePlus }, EDGE).into()
            }
        };
        let tip: &str = if saved {
            "Remove from Your Library"
        } else {
            "Save to Your Library"
        };
        self.tip(
            button(content)
                .padding(8)
                .style(styles::icon_button(p, saved))
                .on_press(Message::ToggleSaved(entry)),
            tip,
        )
    }

    /// Short hint shown above a control while hovered.
    pub(super) fn tip<'a>(&self, control: impl Into<Element<'a>>, hint: &'a str) -> Element<'a> {
        iced::widget::tooltip(
            control,
            container(self.subdued(hint))
                .padding([6.0, 10.0])
                .style(styles::popover(&self.palette)),
            iced::widget::tooltip::Position::Top,
        )
        .into()
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
    pub(super) fn icon_label<'a>(
        &self,
        which: Icon,
        label: impl text::IntoFragment<'a>,
    ) -> Element<'a> {
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
