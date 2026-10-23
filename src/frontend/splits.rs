//! Resizable panel widths: the sidebar on the left, and the queue or lyrics panel on the
//! right. Widths the user drags are kept as preferred; the frame shows them shrunk when
//! the window is too narrow to fit them beside a usable main view.

/// Narrowest main view the panels may leave.
pub const MIN_MAIN: f32 = 360.0;

/// A draggable boundary between two panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Between the sidebar and the main view.
    Sidebar,
    /// Between the main view and the right panel.
    Right,
}

/// Which panel sits on the right, sharing one slot like Spotify's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Right {
    Queue,
    Lyrics,
}

impl Right {
    fn limits(self) -> (f32, f32) {
        match self {
            Right::Queue => (260.0, 560.0),
            Right::Lyrics => (300.0, 760.0),
        }
    }
}

const SIDEBAR_LIMITS: (f32, f32) = (200.0, 480.0);

/// Preferred widths, each within its own limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Splits {
    pub sidebar: f32,
    pub queue: f32,
    pub lyrics: f32,
}

impl Default for Splits {
    fn default() -> Self {
        Self {
            sidebar: 280.0,
            queue: 300.0,
            lyrics: 380.0,
        }
    }
}

fn clamp(width: f32, (min, max): (f32, f32), fallback: f32) -> f32 {
    if width.is_finite() {
        width.clamp(min, max)
    } else {
        fallback
    }
}

impl Splits {
    /// Bring stored widths into their limits; missing or invalid ones get the defaults.
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        Self {
            sidebar: clamp(self.sidebar, SIDEBAR_LIMITS, d.sidebar),
            queue: clamp(self.queue, Right::Queue.limits(), d.queue),
            lyrics: clamp(self.lyrics, Right::Lyrics.limits(), d.lyrics),
        }
    }

    pub fn right(&self, panel: Right) -> f32 {
        match panel {
            Right::Queue => self.queue,
            Right::Lyrics => self.lyrics,
        }
    }

    /// Widths shown in a frame `inner` pixels wide (window minus edges and gaps): the
    /// right panel gives way first, then the sidebar, never below their minimums.
    pub fn fit(&self, inner: f32, right: Option<Right>) -> (f32, f32) {
        let mut sidebar: f32 = self.sidebar;
        let mut side: f32 = right.map_or(0.0, |panel| self.right(panel));
        let mut excess: f32 = sidebar + side + MIN_MAIN - inner;
        if let Some(panel) = right
            && excess > 0.0
        {
            let give: f32 = excess.min(side - panel.limits().0).max(0.0);
            side -= give;
            excess -= give;
        }
        if excess > 0.0 {
            sidebar -= excess.min(sidebar - SIDEBAR_LIMITS.0).max(0.0);
        }
        (sidebar, side)
    }

    /// Resize the panel at `handle` to `width`, within its limits and leaving the main view
    /// its minimum in a frame `inner` pixels wide; the far panel keeps its shown width.
    pub fn set(&mut self, handle: Handle, width: f32, inner: f32, right: Option<Right>) {
        let (sidebar, side) = self.fit(inner, right);
        match handle {
            Handle::Sidebar => {
                let max: f32 = (inner - side - MIN_MAIN).max(SIDEBAR_LIMITS.0);
                self.sidebar = clamp(width.min(max), SIDEBAR_LIMITS, self.sidebar);
            }
            Handle::Right => {
                let Some(panel) = right else { return };
                let max: f32 = (inner - sidebar - MIN_MAIN).max(panel.limits().0);
                let width: f32 = clamp(width.min(max), panel.limits(), side);
                match panel {
                    Right::Queue => self.queue = width,
                    Right::Lyrics => self.lyrics = width,
                }
            }
        }
    }

    /// Width shown at `handle` when a drag starts there.
    pub fn shown(&self, handle: Handle, inner: f32, right: Option<Right>) -> f32 {
        let (sidebar, side) = self.fit(inner, right);
        match handle {
            Handle::Sidebar => sidebar,
            Handle::Right => side,
        }
    }
}

/// A drag in progress: the cursor position and panel width where it took hold.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub handle: Handle,
    pub grab: Option<(f32, f32)>,
}

impl Drag {
    /// Panel width for the cursor at `x`. The sidebar grows to the right, the right panel
    /// to the left; the first move only records where the drag took hold.
    pub fn width(&mut self, x: f32, shown: f32) -> f32 {
        let (start_x, start_width) = *self.grab.get_or_insert((x, shown));
        match self.handle {
            Handle::Sidebar => start_width + (x - start_x),
            Handle::Right => start_width - (x - start_x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_widths_are_clamped_and_invalid_ones_reset() {
        let s = Splits {
            sidebar: 9_000.0,
            queue: f32::NAN,
            lyrics: 10.0,
        }
        .sanitized();
        assert_eq!(
            s,
            Splits {
                sidebar: 480.0,
                queue: 300.0,
                lyrics: 300.0
            }
        );
    }

    /// A narrow window shrinks the right panel before the sidebar, keeping the main view.
    #[test]
    fn narrow_frames_shrink_the_right_panel_first() {
        let s = Splits {
            sidebar: 400.0,
            queue: 300.0,
            lyrics: 600.0,
        };
        assert_eq!(s.fit(2_000.0, Some(Right::Lyrics)), (400.0, 600.0));
        assert_eq!(s.fit(1_200.0, Some(Right::Lyrics)), (400.0, 440.0));
        assert_eq!(s.fit(800.0, Some(Right::Lyrics)), (200.0, 300.0));
        assert_eq!(s.fit(800.0, None), (400.0, 0.0));
    }

    #[test]
    fn dragging_follows_the_cursor_within_limits_and_the_main_minimum() {
        let mut s = Splits::default();
        let mut drag = Drag {
            handle: Handle::Sidebar,
            grab: None,
        };
        let width = drag.width(500.0, s.sidebar);
        assert_eq!(width, 280.0, "grabbing does not jump");
        s.set(
            Handle::Sidebar,
            drag.width(570.0, s.sidebar),
            1_400.0,
            Some(Right::Queue),
        );
        assert_eq!(s.sidebar, 350.0);
        s.set(Handle::Sidebar, 50.0, 1_400.0, Some(Right::Queue));
        assert_eq!(s.sidebar, 200.0);
        let mut drag = Drag {
            handle: Handle::Right,
            grab: None,
        };
        drag.width(900.0, s.lyrics);
        s.set(
            Handle::Right,
            drag.width(780.0, s.lyrics),
            1_400.0,
            Some(Right::Lyrics),
        );
        assert_eq!((s.lyrics, s.queue), (500.0, 300.0));
        // Never past the point where the main view would drop below its minimum.
        s.set(Handle::Right, 2_000.0, 1_000.0, Some(Right::Lyrics));
        assert_eq!(s.lyrics, 1_000.0 - 200.0 - MIN_MAIN);
    }
}
