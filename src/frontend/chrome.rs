//! Window chrome: custom title bar plumbing, resize grips, persisted geometry and UI zoom.
//!
//! Sizes held by `Ui` (`window_size`, `restore_size`) are layout sizes: the window's logical
//! size divided by the zoom, which is what Iced reports in `Resized` and what grids and
//! virtualized lists measure. Positions are in the window system's logical pixels, since
//! that is what a new window is placed with.
use super::*;
use iced::mouse::Interaction;
use iced::widget::{column, mouse_area, row, space, stack};
use iced::window::Direction;
use iced::{Alignment, Length};

/// Smallest layout size of the window, before zoom.
pub(super) const MIN_SIZE: iced::Size = iced::Size::new(860.0, 620.0);
/// Range and step of the UI zoom.
pub(super) const ZOOM_MIN: f32 = 0.8;
pub(super) const ZOOM_MAX: f32 = 1.5;
pub(super) const ZOOM_STEP: f32 = 0.1;
/// Thickness of the invisible edge grips and the edge length of the corner grips.
const GRIP: f32 = 6.0;
const CORNER: f32 = 10.0;
/// Corner radius of the window when the app draws its own frame.
const WINDOW_RADIUS: f32 = 10.0;
/// Where a window that landed on no monitor is moved to.
const FALLBACK_POSITION: iced::Point = iced::Point::new(64.0, 64.0);

/// Zoom kept inside its range and on whole percents; invalid values become 1.0.
pub(super) fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() && zoom > 0.0 {
        (zoom.clamp(ZOOM_MIN, ZOOM_MAX) * 100.0).round() / 100.0
    } else {
        1.0
    }
}

/// Zoom after one step in or out, on tenths.
pub(super) fn step_zoom(zoom: f32, in_: bool) -> f32 {
    let step: f32 = if in_ { ZOOM_STEP } else { -ZOOM_STEP };
    clamp_zoom(((zoom + step) * 10.0).round() / 10.0)
}

impl Ui {
    /// Settings of the main window, from the remembered geometry, zoom and title bar choice.
    pub(super) fn window_settings(&self) -> window::Settings {
        let rgba = crate::app_icon::rgba(256);
        let mut settings = window::Settings {
            size: self.restore_size,
            position: self
                .restore_position
                .map_or(window::Position::Default, window::Position::Specific),
            min_size: Some(self.min_size()),
            maximized: self.maximized,
            decorations: self.system_title_bar,
            // Transparent so the corners of the custom frame can be rounded.
            transparent: true,
            exit_on_close_request: false,
            icon: window::icon::from_rgba(rgba.pixels, rgba.width, rgba.height).ok(),
            ..Default::default()
        };
        #[cfg(target_os = "linux")]
        {
            settings.platform_specific.application_id = crate::app_icon::APP_ID.into();
        }
        settings
    }

    /// Smallest window size in the window system's logical pixels.
    fn min_size(&self) -> iced::Size {
        iced::Size::new(MIN_SIZE.width * self.zoom, MIN_SIZE.height * self.zoom)
    }

    /// Whether the app draws its own title bar, window buttons and resize grips.
    pub(super) fn custom_chrome(&self) -> bool {
        !self.system_title_bar
    }

    /// Corner radius of the window frame: only the app's own frame is rounded, and not
    /// while maximized, when the corners are the screen's.
    pub(super) fn window_radius(&self) -> f32 {
        if self.custom_chrome() && !self.maximized {
            WINDOW_RADIUS
        } else {
            0.0
        }
    }

    /// Ask the window system whether the window is maximized and where it is; the answer
    /// arrives as `Message::Maximized`.
    pub(super) fn track_geometry(&self, id: window::Id) -> Task<Message> {
        window::is_maximized(id).map(Message::Maximized)
    }

    /// Record the maximized state. The geometry to restore changes only while not maximized.
    pub(super) fn set_maximized(&mut self, maximized: bool) {
        self.maximized = maximized;
        if !maximized {
            self.restore_size = self.window_size;
            self.restore_position = self.position;
        }
        if let Err(e) = persistence::Preferences::save(self) {
            self.notice = e;
        }
    }

    /// Record a window move reported by the window system (never sent on native Wayland).
    pub(super) fn set_position(&mut self, position: iced::Point) {
        self.position = Some(iced::Point::new(
            position.x * self.zoom,
            position.y * self.zoom,
        ));
    }

    /// Change the UI zoom, keeping the window's real size: layout sizes shrink as it grows.
    pub(super) fn set_zoom(&mut self, zoom: f32) -> Task<Message> {
        let zoom: f32 = clamp_zoom(zoom);
        if zoom == self.zoom {
            return Task::none();
        }
        let ratio: f32 = self.zoom / zoom;
        self.zoom = zoom;
        let rescale = |size: iced::Size| iced::Size::new(size.width * ratio, size.height * ratio);
        self.window_size = rescale(self.window_size);
        self.restore_size = rescale(self.restore_size);
        self.main_size = self.estimated_main_size();
        self.sidebar_viewport = self.window_size.height;
        self.queue_viewport = self.window_size.height;
        if let Err(e) = persistence::Preferences::save(self) {
            self.notice = e;
        }
        match self.window {
            Some(id) => window::set_min_size(id, Some(self.min_size())),
            None => Task::none(),
        }
    }

    /// Switch between the app's own title bar and the window system's.
    pub(super) fn set_system_title_bar(&mut self, system: bool) -> Task<Message> {
        if system == self.system_title_bar {
            return Task::none();
        }
        self.system_title_bar = system;
        if let Err(e) = persistence::Preferences::save(self) {
            self.notice = e;
        }
        match self.window {
            Some(id) => window::toggle_decorations(id),
            None => Task::none(),
        }
    }

    /// Move a window that opened outside every monitor (for example a saved position on a
    /// monitor that is no longer connected) to a visible spot.
    pub(super) fn ensure_on_screen(&self, id: window::Id) -> Task<Message> {
        if self.restore_position.is_none() {
            return Task::none();
        }
        window::monitor_size(id).then(move |monitor| match monitor {
            Some(_) => Task::none(),
            None => window::move_to(id, FALLBACK_POSITION),
        })
    }

    /// Transparent resize handles along the window edges, drawn above the frame. Absent when
    /// the window system draws the frame or the window is maximized.
    pub(super) fn resize_grips(&self) -> Option<Element<'_>> {
        if !self.custom_chrome() || self.maximized {
            return None;
        }
        let corner = |direction: Direction, interaction: Interaction| {
            grip(direction, interaction, CORNER.into(), CORNER.into())
        };
        let across = |direction: Direction, interaction: Interaction| {
            grip(direction, interaction, Length::Fill, GRIP.into())
        };
        let down = |direction: Direction, interaction: Interaction| {
            grip(direction, interaction, GRIP.into(), Length::Fill)
        };
        let top = row![
            corner(Direction::NorthWest, Interaction::ResizingDiagonallyDown),
            across(Direction::North, Interaction::ResizingVertically),
            corner(Direction::NorthEast, Interaction::ResizingDiagonallyUp),
        ]
        .height(CORNER)
        .align_y(Alignment::Start);
        let middle = row![
            down(Direction::West, Interaction::ResizingHorizontally),
            space().width(Length::Fill),
            down(Direction::East, Interaction::ResizingHorizontally),
        ]
        .height(Length::Fill);
        let bottom = row![
            corner(Direction::SouthWest, Interaction::ResizingDiagonallyUp),
            across(Direction::South, Interaction::ResizingVertically),
            corner(Direction::SouthEast, Interaction::ResizingDiagonallyDown),
        ]
        .height(CORNER)
        .align_y(Alignment::End);
        Some(
            column![top, middle, bottom]
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
        )
    }

    /// Wrap the frame so the resize grips sit above it when they apply.
    pub(super) fn with_resize_grips<'a>(&'a self, frame: Element<'a>) -> Element<'a> {
        match self.resize_grips() {
            Some(grips) => stack![frame, grips].into(),
            None => frame,
        }
    }
}

/// One invisible handle that starts an interactive resize from `direction`.
fn grip<'a>(
    direction: Direction,
    interaction: Interaction,
    width: Length,
    height: Length,
) -> Element<'a> {
    mouse_area(space().width(width).height(height))
        .on_press(Message::ResizeWindow(direction))
        .interaction(interaction)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_stay_in_range_on_tenths() {
        assert_eq!(step_zoom(1.0, true), 1.1);
        assert_eq!(step_zoom(1.1, true), 1.2);
        assert_eq!(step_zoom(1.5, true), 1.5);
        assert_eq!(step_zoom(0.8, false), 0.8);
        assert_eq!(step_zoom(1.0, false), 0.9);
    }

    #[test]
    fn invalid_zoom_falls_back_to_default() {
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
        assert_eq!(clamp_zoom(0.0), 1.0);
        assert_eq!(clamp_zoom(9.0), ZOOM_MAX);
        assert_eq!(clamp_zoom(0.1), ZOOM_MIN);
    }
}
