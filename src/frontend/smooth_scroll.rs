//! Smooth wheel scrolling: wheel ticks move an eased target instead of jumping a line step.
//! Touchpad pixel deltas and scrollbar drags are untouched and reach the scrollable directly.
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::Id;
use iced::widget::operation::{self, AbsoluteOffset};
use iced::widget::scrollable::Viewport;
use iced::{Element, Event, Length, Rectangle, Size, Task, Vector};
use std::time::Duration;

/// Pixels per wheel line, matching the scrollable's own line step.
const LINE_PX: f32 = 60.0;
/// Easing time constant: about 95% of a wheel tick is covered after three of these.
const TAU: Duration = Duration::from_millis(45);
/// Slower easing for programmatic moves, such as following the sung lyrics line.
const FOLLOW_TAU: Duration = Duration::from_millis(140);
/// Remaining distance under which the animation lands on its target.
const SETTLE_PX: f32 = 0.5;
/// Wheel ticks closer together than this, in the same direction, build up speed.
const STREAK_GAP: Duration = Duration::from_millis(150);
/// Extra step per tick of a running streak, as a fraction of a line.
const ACCELERATION: f32 = 0.4;
/// Fastest step, in lines per tick.
const MAX_SPEED: f32 = 5.0;
/// Most of a viewport one frame may move. Rows are built for the previous frame's offset
/// with one viewport of overscan, so a larger step would briefly show blank space.
const MAX_FRAME_STEP: f32 = 0.6;

/// Scroll areas that glide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Area {
    Main,
    Sidebar,
    Queue,
    Lyrics,
}

impl Area {
    const ALL: [Area; 4] = [Area::Main, Area::Sidebar, Area::Queue, Area::Lyrics];

    pub(super) fn id(self) -> Id {
        Id::new(match self {
            Area::Main => super::MAIN_SCROLL,
            Area::Sidebar => "sidebar",
            Area::Queue => "queue",
            Area::Lyrics => "lyrics",
        })
    }
}

/// Scroll position and extent reported by a scrollable.
#[derive(Clone, Copy, Debug)]
pub(super) struct Metrics {
    pub y: f32,
    pub height: f32,
    /// Largest reachable offset.
    pub max: f32,
}

impl Metrics {
    pub(super) fn of(v: &Viewport) -> Self {
        let height: f32 = v.bounds().height;
        Self {
            y: v.absolute_offset().y,
            height,
            max: (v.content_bounds().height - height).max(0.0),
        }
    }

    /// Whether the viewport is within a couple of rows of the end of the content.
    pub(super) fn near_end(&self) -> bool {
        self.y >= self.max - 200.0
    }
}

/// Eased offset for one scroll area.
#[derive(Debug, Default)]
struct Glide {
    /// Offset last reported by the scrollable.
    observed: f32,
    /// Largest offset seen reported; unknown until the area has scrolled once.
    max: Option<f32>,
    /// Viewport height last reported.
    height: f32,
    /// Eased offset sent to the scrollable while animating.
    pos: f32,
    target: Option<f32>,
    /// Previous wheel tick and how many quick same-direction ticks preceded it.
    last_wheel: Option<(std::time::Instant, f32)>,
    streak: u32,
    /// Easing of the running animation; wheel ticks use `TAU`.
    slow: bool,
}

impl Glide {
    fn observe(&mut self, m: Metrics) {
        self.observed = m.y;
        self.max = Some(m.max);
        self.height = m.height;
        if let Some(target) = self.target.as_mut() {
            *target = target.min(m.max);
        }
    }

    /// Steps grow while ticks keep coming quickly in one direction, so a long flick covers
    /// more ground than the same number of slow ticks.
    fn accelerate(&mut self, delta: f32, now: std::time::Instant) -> f32 {
        let continues: bool = self.last_wheel.is_some_and(|(at, previous)| {
            now.saturating_duration_since(at) < STREAK_GAP && previous.signum() == delta.signum()
        });
        self.streak = if continues { self.streak + 1 } else { 0 };
        self.last_wheel = Some((now, delta));
        delta * (1.0 + ACCELERATION * self.streak as f32).min(MAX_SPEED)
    }

    fn wheel(&mut self, delta: f32, now: std::time::Instant) {
        let delta: f32 = self.accelerate(delta, now);
        let base: f32 = match self.target {
            Some(target) => target,
            None => {
                self.pos = self.observed;
                self.observed
            }
        };
        // Past-the-end targets are corrected by `observe` once the scrollable reports its extent.
        self.target = Some((base + delta).clamp(0.0, self.max.unwrap_or(f32::MAX)));
        self.slow = false;
    }

    fn ease_to(&mut self, y: f32) {
        if self.target.is_none() {
            self.pos = self.observed;
        }
        self.target = Some(y.clamp(0.0, self.max.unwrap_or(f32::MAX)));
        self.slow = true;
    }

    fn cancel(&mut self) {
        self.target = None;
    }

    /// Move toward the target; returns the offset to apply while animating.
    fn advance(&mut self, dt: Duration) -> Option<f32> {
        let target: f32 = self.target?;
        let gap: f32 = target - self.pos;
        if gap.abs() < SETTLE_PX {
            self.pos = target;
            self.target = None;
        } else {
            let tau: Duration = if self.slow { FOLLOW_TAU } else { TAU };
            let step: f32 = gap * (1.0 - (-dt.as_secs_f32() / tau.as_secs_f32()).exp());
            let limit: f32 = MAX_FRAME_STEP * self.height;
            self.pos += if limit > 0.0 {
                step.clamp(-limit, limit)
            } else {
                step
            };
        }
        Some(self.pos)
    }
}

/// Animation state for every smooth scroll area.
#[derive(Debug, Default)]
pub(super) struct SmoothScroll {
    main: Glide,
    sidebar: Glide,
    queue: Glide,
    lyrics: Glide,
    last_frame: Option<std::time::Instant>,
}

impl SmoothScroll {
    fn glide(&mut self, area: Area) -> &mut Glide {
        match area {
            Area::Main => &mut self.main,
            Area::Sidebar => &mut self.sidebar,
            Area::Queue => &mut self.queue,
            Area::Lyrics => &mut self.lyrics,
        }
    }

    pub(super) fn observe(&mut self, area: Area, metrics: Metrics) {
        self.glide(area).observe(metrics);
    }

    pub(super) fn wheel(&mut self, area: Area, delta: f32, now: std::time::Instant) {
        self.glide(area).wheel(delta, now);
    }

    pub(super) fn cancel(&mut self, area: Area) {
        self.glide(area).cancel();
    }

    /// Glide to an offset with the slower programmatic easing.
    pub(super) fn ease_to(&mut self, area: Area, y: f32) {
        self.glide(area).ease_to(y);
    }

    /// Whether `area` is still moving toward a target.
    pub(super) fn animating(&mut self, area: Area) -> bool {
        self.glide(area).target.is_some()
    }

    /// Whether frames are needed to keep animating.
    pub(super) fn active(&self) -> bool {
        [&self.main, &self.sidebar, &self.queue, &self.lyrics]
            .iter()
            .any(|glide| glide.target.is_some())
    }

    /// Advance every animation by the time since the previous frame.
    pub(super) fn frame<Message: Send + 'static>(
        &mut self,
        now: std::time::Instant,
    ) -> Task<Message> {
        let dt: Duration = self.last_frame.map_or(Duration::from_millis(16), |last| {
            now.saturating_duration_since(last)
        });
        let mut tasks: Vec<Task<Message>> = Vec::new();
        for area in Area::ALL {
            if let Some(y) = self.glide(area).advance(dt) {
                tasks.push(operation::scroll_to(
                    area.id(),
                    AbsoluteOffset { x: 0.0, y },
                ));
            }
        }
        self.last_frame = self.active().then_some(now);
        Task::batch(tasks)
    }
}

/// Wraps a scrollable: wheel line ticks are captured and reported instead of scrolling it, and
/// a left press (scrollbar drag or click) reports that the user took over.
pub(super) struct Wheel<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_wheel: Box<dyn Fn(f32) -> Message + 'a>,
    on_press: Message,
}

pub(super) fn wheel<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_wheel: impl Fn(f32) -> Message + 'a,
    on_press: Message,
) -> Wheel<'a, Message, Theme, Renderer> {
    Wheel {
        content: content.into(),
        on_wheel: Box::new(on_wheel),
        on_press,
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Wheel<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget_mut().layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if cursor.is_over(layout.bounds()) {
            match event {
                Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Lines { y, .. },
                }) => {
                    shell.publish((self.on_wheel)(-y * LINE_PX));
                    shell.capture_event();
                    return;
                }
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    shell.publish(self.on_press.clone());
                }
                _ => {}
            }
        }
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation)
    }
}

impl<'a, Message, Theme, Renderer> From<Wheel<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Theme: 'a,
    Renderer: 'a + renderer::Renderer,
{
    fn from(wheel: Wheel<'a, Message, Theme, Renderer>) -> Self {
        Element::new(wheel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tick time far from the previous one, so no streak builds.
    fn slow() -> std::time::Instant {
        std::time::Instant::now()
            + Duration::from_secs(60) * TICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
    static TICKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    #[test]
    fn quick_ticks_accelerate_and_a_pause_or_reversal_resets() {
        let mut glide = Glide::default();
        let start = std::time::Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert_eq!(glide.accelerate(60.0, at(0)), 60.0);
        assert_eq!(glide.accelerate(60.0, at(50)), 84.0);
        assert_eq!(glide.accelerate(60.0, at(100)), 108.0);
        // A pause starts over.
        assert_eq!(glide.accelerate(60.0, at(400)), 60.0);
        assert_eq!(glide.accelerate(60.0, at(450)), 84.0);
        // So does reversing direction.
        assert_eq!(glide.accelerate(-60.0, at(500)), -60.0);
        // Speed is capped.
        for i in 1..30 {
            glide.accelerate(-60.0, at(500 + 40 * i));
        }
        assert_eq!(glide.accelerate(-60.0, at(1800)), -60.0 * MAX_SPEED);
    }

    fn metrics(y: f32, max: f32) -> Metrics {
        Metrics {
            y,
            height: 600.0,
            max,
        }
    }

    #[test]
    fn wheel_accumulates_from_the_target_and_clamps() {
        let mut glide = Glide::default();
        glide.observe(metrics(100.0, 150.0));
        glide.wheel(60.0, slow());
        glide.wheel(60.0, slow());
        assert_eq!(glide.target, Some(150.0));
        glide.wheel(-300.0, slow());
        assert_eq!(glide.target, Some(0.0));
    }

    #[test]
    fn target_shrinks_with_the_content() {
        let mut glide = Glide::default();
        glide.observe(metrics(0.0, 1000.0));
        glide.wheel(500.0, slow());
        glide.observe(metrics(0.0, 200.0));
        assert_eq!(glide.target, Some(200.0));
    }

    /// A long accelerated flick never jumps past the overscanned rows in one frame.
    #[test]
    fn one_frame_moves_less_than_a_viewport() {
        let mut glide = Glide::default();
        glide.observe(metrics(0.0, 100_000.0));
        glide.wheel(20_000.0, slow());
        let mut last: f32 = 0.0;
        for _ in 0..10 {
            let y: f32 = glide.advance(Duration::from_millis(16)).unwrap();
            assert!(y - last <= MAX_FRAME_STEP * 600.0 + 0.01);
            last = y;
        }
    }

    #[test]
    fn eases_monotonically_and_settles_on_target() {
        let mut glide = Glide::default();
        glide.observe(metrics(0.0, 1000.0));
        glide.wheel(120.0, slow());
        let mut last: f32 = 0.0;
        let mut elapsed = Duration::ZERO;
        while let Some(y) = glide.advance(Duration::from_millis(16)) {
            assert!(y > last && y <= 120.0);
            last = y;
            elapsed += Duration::from_millis(16);
            assert!(elapsed < Duration::from_millis(400));
        }
        assert_eq!(last, 120.0);
        assert!(glide.target.is_none());
        assert!(elapsed >= Duration::from_millis(100));
    }

    #[test]
    fn new_tick_restarts_from_the_observed_offset_when_idle() {
        let mut glide = Glide::default();
        glide.observe(metrics(0.0, 1000.0));
        glide.wheel(60.0, slow());
        while glide.advance(Duration::from_millis(16)).is_some() {}
        glide.observe(metrics(60.0, 1000.0));
        glide.wheel(60.0, slow());
        assert_eq!(glide.target, Some(120.0));
        assert_eq!(glide.pos, 60.0);
    }
}
