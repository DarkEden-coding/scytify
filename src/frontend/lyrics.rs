//! Synced lyrics: which line and syllable is sung at a position, the measured layout of the
//! lyrics column, and where the view scrolls to keep the sung line in place.
//!
//! The view draws every line with the same fonts, sizes and wrap width used here, so the
//! measured line tops match the drawn ones and the follow target lands on the real line.
use crate::contracts::{AppError, RequestId};
use crate::domain::{Lyrics, LyricsLine, LyricsSync, SpotifyId, Syllable};
use iced::advanced::graphics::text::Paragraph;
use iced::advanced::text::{self, Paragraph as _};
use iced::{Font, Pixels, Size};
use std::time::{Duration, Instant};

/// Lead vocal text size.
pub const LEAD_SIZE: f32 = 24.0;
/// Background vocal text size.
pub const BACKGROUND_SIZE: f32 = 17.0;
/// Space between a lead line and its background vocals.
pub const BACKGROUND_GAP: f32 = 4.0;
/// Space between lines.
pub const LINE_GAP: f32 = 16.0;
/// Horizontal padding around the lyrics column.
pub const SIDE_PADDING: f32 = 20.0;
/// The sung line is kept this far down the viewport.
pub const ANCHOR: f32 = 0.33;
/// How long a manual scroll suspends following the sung line.
const MANUAL_PAUSE: Duration = Duration::from_secs(4);
/// Distance between the scrollable and the follow target that counts as a manual scroll.
const DRIFT_PX: f32 = 4.0;

pub fn line_height() -> text::LineHeight {
    text::LineHeight::Relative(1.25)
}

#[derive(Debug)]
pub enum Status {
    Loading(RequestId),
    Ready(Lyrics),
    /// The provider has no lyrics for the track.
    Missing,
    Failed(AppError),
}

/// Lyrics of the playing track and the follow state of the lyrics view.
#[derive(Debug, Default)]
pub struct Panel {
    /// Track the status belongs to.
    pub track: Option<SpotifyId>,
    pub status: Option<Status>,
    /// Size of the lyrics scroll area, known once it is laid out.
    pub viewport: Option<Size>,
    layout: Option<Layout>,
    /// Offset the view was last sent to, and the line it follows.
    target: Option<f32>,
    followed: Option<usize>,
    manual_until: Option<Instant>,
}

/// Measured top of every line for one viewport size.
#[derive(Debug)]
struct Layout {
    size: Size,
    tops: Vec<f32>,
}

impl Panel {
    /// Lyrics ready to draw.
    pub fn lyrics(&self) -> Option<&Lyrics> {
        match &self.status {
            Some(Status::Ready(lyrics)) => Some(lyrics),
            _ => None,
        }
    }

    /// Whether the view needs a frame every vsync to move its highlight.
    pub fn animated(&self) -> bool {
        self.lyrics().is_some_and(|l| l.sync != LyricsSync::Static)
    }

    pub fn loading(&self) -> Option<RequestId> {
        match self.status {
            Some(Status::Loading(id)) => Some(id),
            _ => None,
        }
    }

    /// Start over for another track.
    pub fn reset(&mut self, track: Option<SpotifyId>, status: Option<Status>) {
        self.track = track;
        self.status = status;
        self.layout = None;
        self.target = None;
        self.followed = None;
        self.manual_until = None;
    }

    /// The answer for the request in flight.
    pub fn complete(&mut self, status: Status) {
        self.status = Some(status);
        self.layout = None;
        self.followed = None;
        self.target = None;
    }

    /// The user scrolled; stop following for a while.
    pub fn manual(&mut self, now: Instant) {
        self.manual_until = Some(now + MANUAL_PAUSE);
    }

    /// The scroll area reported an offset. Away from where it was sent while no
    /// animation runs, the user moved it with a touchpad or the scrollbar.
    pub fn observe(&mut self, y: f32, max: f32, animating: bool, now: Instant) {
        if let Some(target) = self.target
            && !animating
            && (y - target.min(max)).abs() > DRIFT_PX
        {
            self.target = None;
            self.manual(now);
        }
    }

    /// Offset to glide to so the sung line sits at the anchor, when it changed.
    ///
    /// `measure` returns the line tops for a viewport; it only runs when the size changes.
    pub fn follow(
        &mut self,
        position_ms: u64,
        now: Instant,
        measure: impl FnOnce(&Lyrics, Size) -> Vec<f32>,
    ) -> Option<f32> {
        if self.manual_until.is_some_and(|until| now < until) {
            return None;
        }
        let resumed: bool = self.manual_until.take().is_some();
        let viewport: Size = self.viewport?;
        let Some(Status::Ready(lyrics)) = &self.status else {
            return None;
        };
        if lyrics.sync == LyricsSync::Static {
            return None;
        }
        let remeasured: bool = self.layout.as_ref().is_none_or(|l| l.size != viewport);
        if remeasured {
            self.layout = Some(Layout {
                size: viewport,
                tops: measure(lyrics, viewport),
            });
        }
        let line: Option<usize> = followed_line(&lyrics.lines, position_ms);
        if line == self.followed && !remeasured && !resumed && self.target.is_some() {
            return None;
        }
        self.followed = line;
        let tops: &[f32] = &self.layout.as_ref()?.tops;
        let y: f32 = line
            .and_then(|i| tops.get(i))
            .map_or(0.0, |top| (top - viewport.height * ANCHOR).max(0.0).round());
        self.target = Some(y);
        Some(y)
    }
}

/// Wrap width of the lyrics text inside a scroll area of `width`.
pub fn content_width(width: f32) -> f32 {
    (width - 2.0 * SIDE_PADDING).max(1.0)
}

/// Space above the first line, so it can rest at the anchor.
pub fn top_padding(viewport_height: f32) -> f32 {
    viewport_height * ANCHOR
}

/// Space below the last line, so it can scroll up to the anchor too.
pub fn bottom_padding(viewport_height: f32) -> f32 {
    viewport_height * (1.0 - ANCHOR)
}

/// The line the view keeps in place: the latest one that has started.
pub fn followed_line(lines: &[LyricsLine], position_ms: u64) -> Option<usize> {
    lines.iter().rposition(|line| line.start_ms <= position_ms)
}

/// Where a line is relative to the playback position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Sung,
    Singing,
    Upcoming,
}

pub fn phase(line: &LyricsLine, position_ms: u64) -> Phase {
    if position_ms < line.start_ms {
        Phase::Upcoming
    } else if position_ms < line.end_ms {
        Phase::Singing
    } else {
        Phase::Sung
    }
}

/// How much of a syllable has been sung, from 0 to 1.
pub fn progress(syllable: &Syllable, position_ms: u64) -> f32 {
    if position_ms <= syllable.start_ms {
        0.0
    } else if position_ms >= syllable.end_ms {
        1.0
    } else {
        (position_ms - syllable.start_ms) as f32 / (syllable.end_ms - syllable.start_ms) as f32
    }
}

/// Displayed text of each syllable, with the space that separates it from the next word.
pub fn pieces(syllables: &[Syllable]) -> impl Iterator<Item = String> + '_ {
    syllables.iter().enumerate().map(|(i, s)| {
        if s.joins_next || i + 1 == syllables.len() {
            s.text.clone()
        } else {
            format!("{} ", s.text)
        }
    })
}

/// Height of `syllables` wrapped to `width`, as the view draws them.
fn wrapped_height(syllables: &[Syllable], width: f32, font: Font, size: f32) -> f32 {
    let content: String = pieces(syllables).collect();
    Paragraph::with_text(text::Text {
        content: content.as_str(),
        bounds: Size::new(width, f32::INFINITY),
        size: Pixels(size),
        line_height: line_height(),
        font,
        align_x: text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::Word,
    })
    .min_bounds()
    .height
}

/// Height of one drawn line, background vocals included.
pub fn line_extent(line: &LyricsLine, width: f32, font: Font) -> f32 {
    let mut height: f32 = wrapped_height(&line.lead, width, font, LEAD_SIZE);
    if !line.background.is_empty() {
        height += BACKGROUND_GAP + wrapped_height(&line.background, width, font, BACKGROUND_SIZE);
    }
    height
}

/// Top of every line inside the scroll content of a viewport, below the top padding.
pub fn tops(lines: &[LyricsLine], viewport: Size, font: Font) -> Vec<f32> {
    let width: f32 = content_width(viewport.width);
    stack(lines, viewport.height, |line| {
        line_extent(line, width, font)
    })
}

fn stack(
    lines: &[LyricsLine],
    viewport_height: f32,
    mut extent: impl FnMut(&LyricsLine) -> f32,
) -> Vec<f32> {
    let mut y: f32 = top_padding(viewport_height);
    lines
        .iter()
        .map(|line| {
            let top: f32 = y;
            y += extent(line) + LINE_GAP;
            top
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LyricsCredit, LyricsSource};

    fn syllable(text: &str, start_ms: u64, end_ms: u64, joins_next: bool) -> Syllable {
        Syllable {
            text: text.into(),
            start_ms,
            end_ms,
            joins_next,
        }
    }

    fn line(start_ms: u64, end_ms: u64) -> LyricsLine {
        LyricsLine {
            start_ms,
            end_ms,
            opposite: false,
            lead: vec![syllable("la", start_ms, end_ms, false)],
            background: Vec::new(),
        }
    }

    fn lyrics(lines: Vec<LyricsLine>) -> Lyrics {
        Lyrics {
            track_id: SpotifyId("track".into()),
            sync: LyricsSync::Syllable,
            lines,
            credit: LyricsCredit {
                source: LyricsSource::Unknown,
                uploader: None,
                maker: None,
            },
        }
    }

    fn panel(lines: Vec<LyricsLine>) -> Panel {
        let mut panel = Panel::default();
        panel.reset(
            Some(SpotifyId("track".into())),
            Some(Status::Ready(lyrics(lines))),
        );
        panel.viewport = Some(Size::new(600.0, 300.0));
        panel
    }

    /// Every line 100px tall from the top padding.
    fn even(lyrics: &Lyrics, _: Size) -> Vec<f32> {
        stack(&lyrics.lines, 300.0, |_| 100.0 - LINE_GAP)
    }

    #[test]
    fn the_latest_started_line_is_followed_through_gaps() {
        let lines = vec![line(1_000, 2_000), line(3_000, 4_000)];
        assert_eq!(followed_line(&lines, 500), None);
        assert_eq!(followed_line(&lines, 1_500), Some(0));
        assert_eq!(followed_line(&lines, 2_500), Some(0));
        assert_eq!(followed_line(&lines, 3_000), Some(1));
        assert_eq!(phase(&lines[0], 2_500), Phase::Sung);
        assert_eq!(phase(&lines[1], 2_500), Phase::Upcoming);
    }

    #[test]
    fn syllables_fill_over_their_duration_and_join_words() {
        let s = syllable("hel", 1_000, 1_200, true);
        assert_eq!(progress(&s, 900), 0.0);
        assert_eq!(progress(&s, 1_100), 0.5);
        assert_eq!(progress(&s, 1_300), 1.0);
        let word = [
            s,
            syllable("lo", 1_200, 1_400, false),
            syllable("you", 1_400, 1_600, false),
        ];
        assert_eq!(pieces(&word).collect::<String>(), "hello you");
    }

    /// The view scrolls only when the sung line changes, so a frame per vsync is cheap.
    #[test]
    fn follows_line_changes_and_keeps_the_line_at_the_anchor() {
        let mut panel = panel(vec![line(0, 1_000), line(1_000, 2_000), line(2_000, 3_000)]);
        let now = Instant::now();
        // Line 0 sits at the top padding, which is the anchor: no scroll needed.
        assert_eq!(panel.follow(10, now, even), Some(0.0));
        assert_eq!(panel.follow(500, now, even), None);
        assert_eq!(panel.follow(1_500, now, |_, _| unreachable!()), Some(100.0));
        assert_eq!(panel.follow(2_500, now, even), Some(200.0));
    }

    #[test]
    fn a_manual_scroll_pauses_following_then_snaps_back() {
        let mut panel = panel(vec![line(0, 1_000), line(1_000, 2_000)]);
        let now = Instant::now();
        assert_eq!(panel.follow(1_500, now, even), Some(100.0));
        // The scrollable reports where it was sent: not a manual scroll.
        panel.observe(100.0, 1_000.0, false, now);
        assert_eq!(panel.follow(1_600, now, even), None);
        // A touchpad moved it elsewhere.
        panel.observe(40.0, 1_000.0, false, now);
        assert_eq!(
            panel.follow(1_700, now + Duration::from_secs(1), even),
            None
        );
        assert_eq!(
            panel.follow(1_800, now + MANUAL_PAUSE, even),
            Some(100.0),
            "following resumes on the same line"
        );
    }

    #[test]
    fn static_lyrics_never_scroll() {
        let mut panel = panel(vec![line(0, 0)]);
        if let Some(Status::Ready(lyrics)) = panel.status.as_mut() {
            lyrics.sync = LyricsSync::Static;
        }
        assert!(!panel.animated());
        assert_eq!(panel.follow(0, Instant::now(), even), None);
    }

    /// Background vocals add their own wrapped height under the lead line.
    #[test]
    fn measured_lines_wrap_and_include_background_vocals() {
        let font = Font::default();
        let mut long = line(0, 1_000);
        long.lead = (0..40).map(|i| syllable("word", i, i + 1, false)).collect();
        let one: f32 = line_extent(&line(0, 1_000), 400.0, font);
        assert!(line_extent(&long, 400.0, font) > 2.0 * one);
        let mut backed = line(0, 1_000);
        backed.background = vec![syllable("ooh", 0, 1_000, false)];
        assert!(line_extent(&backed, 400.0, font) > one + BACKGROUND_GAP);
    }
}
