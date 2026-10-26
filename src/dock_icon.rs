//! The dock and launcher icon, `assets/icons/scytify.svg` redrawn in software so its bars can
//! follow playback. Platform adapters only decide how to hand a rendered frame to the OS.
use crate::{
    app_icon::Rgba,
    domain::{AppState, PlaybackStatus},
};
use std::f32::consts::TAU;

/// What the icon shows. `Closed` is the icon of an app that is not running, which the OS draws
/// from the installed bundle assets, so only asset generation asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconState {
    Closed,
    Paused,
    Playing,
}

impl IconState {
    /// A running app shows `Playing` only while this device is producing audio.
    pub fn from_app(state: &AppState) -> Self {
        if state.player.status == PlaybackStatus::Playing && !state.connect.is_remote() {
            Self::Playing
        } else {
            Self::Paused
        }
    }
}

/// Bar centres in the 512-unit design space of the SVG.
const BAR_X: [f32; 5] = [148.0, 202.0, 256.0, 310.0, 364.0];
/// Bar lengths in `scytify.svg`, also the closed icon.
const CLOSED_LEN: [f32; 5] = [64.0, 144.0, 212.0, 132.0, 52.0];
/// Longest each bar grows while playing.
const PLAYING_PEAK: [f32; 5] = [110.0, 170.0, 212.0, 150.0, 100.0];
/// Removed from each of the middle three bars when paused. A shared trim keeps the gaps between
/// their tops equal to the closed icon's.
const PAUSED_TRIM: f32 = 96.0;
const PLAYING_HZ: f32 = 1.1;
const CENTER: f32 = 256.0;
const BAR_RADIUS: f32 = 16.0;
const GLOW_SIGMA: f32 = 10.0;
const GLOW_OPACITY: f32 = 0.4;

/// Length of each bar, or `None` when the bar is not drawn. `seconds` only matters when playing.
fn bar_lengths(state: IconState, seconds: f32) -> [Option<f32>; 5] {
    std::array::from_fn(|i| match state {
        IconState::Closed => Some(CLOSED_LEN[i]),
        IconState::Paused => (1..=3).contains(&i).then(|| CLOSED_LEN[i] - PAUSED_TRIM),
        IconState::Playing => {
            let wobble = (seconds * 2.3 + i as f32).sin() * 0.6;
            let level = 0.5 + 0.5 * (seconds * TAU * PLAYING_HZ + i as f32 * 1.15 + wobble).sin();
            Some(PLAYING_PEAK[i] * level)
        }
    })
}

type Premultiplied = [f32; 4];
type Stops<'a> = &'a [(f32, [f32; 4])];

const TILE: Stops = &[(0.0, [0.204, 0.208, 0.227, 1.0]), (1.0, [0.090, 0.094, 0.106, 1.0])];
const RIM: Stops = &[
    (0.0, [1.0, 1.0, 1.0, 0.18]),
    (0.5, [1.0, 1.0, 1.0, 0.0]),
    (1.0, [0.0, 0.0, 0.0, 0.45]),
];
const WELL: Stops = &[(0.0, [0.071, 0.075, 0.086, 1.0]), (1.0, [0.169, 0.173, 0.192, 1.0])];
const WELL_RIM: Stops = &[(0.0, [0.0, 0.0, 0.0, 0.6]), (1.0, [1.0, 1.0, 1.0, 0.12])];
const BAR_LIVE: Stops = &[(0.0, [0.235, 0.941, 0.486, 1.0]), (1.0, [0.090, 0.698, 0.298, 1.0])];
const BAR_CLOSED: Stops = &[(0.0, [0.420, 0.431, 0.455, 1.0]), (1.0, [0.275, 0.282, 0.302, 1.0])];
const GLOW_RGB: [f32; 3] = [0.118, 0.843, 0.376];

fn gradient(stops: Stops, t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    for pair in stops.windows(2) {
        let ((a_at, a), (b_at, b)) = (pair[0], pair[1]);
        if t <= b_at {
            let k = (t - a_at) / (b_at - a_at);
            return std::array::from_fn(|i| a[i] + (b[i] - a[i]) * k);
        }
    }
    stops[stops.len() - 1].1
}

/// Paint `rgb` at `alpha` over a premultiplied pixel.
fn over(dst: &mut Premultiplied, rgb: [f32; 3], alpha: f32) {
    let keep = 1.0 - alpha;
    for i in 0..3 {
        dst[i] = rgb[i] * alpha + dst[i] * keep;
    }
    dst[3] = alpha + dst[3] * keep;
}

fn round_rect_distance(x: f32, y: f32, half: f32, radius: f32) -> f32 {
    let (qx, qy) = ((x - CENTER).abs() - half + radius, (y - CENTER).abs() - half + radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

fn circle_distance(x: f32, y: f32, radius: f32) -> f32 {
    (x - CENTER).hypot(y - CENTER) - radius
}

fn segment_distance(x: f32, y: f32, bar_x: f32, length: f32) -> f32 {
    let half = length / 2.0;
    let dy = (y - CENTER).abs() - half;
    (x - bar_x).hypot(dy.max(0.0))
}

/// Anti-aliased coverage of a signed distance, with `pixel` the pixel edge in design units.
fn coverage(distance: f32, pixel: f32) -> f32 {
    (0.5 - distance / pixel).clamp(0.0, 1.0)
}

/// Abramowitz and Stegun 7.1.26, accurate to about 1.5e-7.
fn erf(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = ((((1.061_405_4 * t - 1.453_152_1) * t + 1.421_413_8) * t - 0.284_496_74) * t
        + 0.254_829_6)
        * t;
    (1.0 - poly * (-x * x).exp()).copysign(x)
}

/// Gaussian-blurred coverage of the span `[from, to]` at `at`.
fn blurred_span(from: f32, to: f32, at: f32) -> f32 {
    let scale = GLOW_SIGMA * std::f32::consts::SQRT_2;
    0.5 * (erf((to - at) / scale) - erf((from - at) / scale))
}

/// Renders icon frames at one size. The tile, rim and well never change, so they are drawn once.
pub struct DockIconRenderer {
    size: u32,
    /// Design units per output pixel.
    pixel: f32,
    /// Fraction of the canvas the 512-unit design spans; the rest is transparent margin.
    art_scale: f32,
    background: Vec<Premultiplied>,
}

impl DockIconRenderer {
    /// `art_scale` of 1.0 fills the canvas edge to edge. Some systems expect a margin around the
    /// art when the app supplies its own icon image (macOS uses about 0.8).
    pub fn new(size: u32, art_scale: f32) -> Self {
        let pixel = 512.0 / (size as f32 * art_scale);
        let mut renderer = Self {
            size,
            pixel,
            art_scale,
            background: Vec::new(),
        };
        let mut background = Vec::with_capacity((size * size) as usize);
        for row in 0..size {
            for col in 0..size {
                let (x, y) = (renderer.design(col), renderer.design(row));
                background.push(Self::background_pixel(x, y, pixel));
            }
        }
        renderer.background = background;
        renderer
    }

    /// Design-space coordinate of the centre of output pixel `index` along either axis.
    fn design(&self, index: u32) -> f32 {
        ((index as f32 + 0.5) / self.size as f32 - 0.5) * 512.0 / self.art_scale + CENTER
    }

    fn background_pixel(x: f32, y: f32, pixel: f32) -> Premultiplied {
        let mut out = [0.0; 4];
        let diagonal = (x + y) / 1024.0;
        let paint = |out: &mut Premultiplied, stops: Stops, t: f32, cover: f32| {
            let c = gradient(stops, t);
            over(out, [c[0], c[1], c[2]], c[3] * cover);
        };
        paint(&mut out, TILE, diagonal, coverage(round_rect_distance(x, y, 256.0, 116.0), pixel));
        let rim = round_rect_distance(x, y, 254.0, 114.0).abs() - 2.0;
        paint(&mut out, RIM, diagonal, coverage(rim, pixel));
        // The well gradients span the disc's bounding box, 68..444 on both axes.
        let well_t = ((x - 68.0) + (y - 68.0)) / (2.0 * 376.0);
        paint(&mut out, WELL, well_t, coverage(circle_distance(x, y, 188.0), pixel));
        let well_rim = circle_distance(x, y, 187.0).abs() - 1.5;
        paint(&mut out, WELL_RIM, well_t, coverage(well_rim, pixel));
        out
    }

    /// One frame. `seconds` drives the playing animation and is ignored by the other states.
    pub fn render(&self, state: IconState, seconds: f32) -> Rgba {
        let size = self.size as usize;
        let pixel = self.pixel;
        let lengths = bar_lengths(state, seconds);
        let glow = state != IconState::Closed;
        let bar_stops = if glow { BAR_LIVE } else { BAR_CLOSED };

        // The blur is separable per bar, so each axis is evaluated once per column or row.
        let centres = || (0..self.size).map(|k| self.design(k));
        let columns: Vec<[f32; 5]> = centres()
            .map(|x| {
                std::array::from_fn(|i| blurred_span(BAR_X[i] - BAR_RADIUS, BAR_X[i] + BAR_RADIUS, x))
            })
            .collect();
        let rows: Vec<[f32; 5]> = centres()
            .map(|y| {
                std::array::from_fn(|i| match lengths[i] {
                    Some(len) if glow => {
                        let reach = len / 2.0 + BAR_RADIUS;
                        blurred_span(CENTER - reach, CENTER + reach, y)
                    }
                    _ => 0.0,
                })
            })
            .collect();

        let mut pixels = Vec::with_capacity(size * size * 4);
        for row in 0..size {
            let y = self.design(row as u32);
            for col in 0..size {
                let x = self.design(col as u32);
                let mut px = self.background[row * size + col];
                if glow && px[3] > 0.0 {
                    let spread: f32 = (0..5).map(|i| columns[col][i] * rows[row][i]).sum();
                    over(&mut px, GLOW_RGB, GLOW_OPACITY * spread.min(1.0));
                }
                let mut bar = 0.0_f32;
                for i in 0..5 {
                    if let Some(len) = lengths[i]
                        && (x - BAR_X[i]).abs() < BAR_RADIUS + pixel
                    {
                        let d = segment_distance(x, y, BAR_X[i], len) - BAR_RADIUS;
                        bar = bar.max(coverage(d, pixel));
                    }
                }
                if bar > 0.0 {
                    let c = gradient(bar_stops, (y - 150.0) / 212.0);
                    over(&mut px, [c[0], c[1], c[2]], bar);
                }
                let a = px[3];
                for channel in &px[..3] {
                    let straight = if a > 0.0 { channel / a } else { 0.0 };
                    pixels.push((straight.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                }
                pixels.push((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
        Rgba {
            width: self.size,
            height: self.size,
            pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(icon: &Rgba, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * icon.width + x) * 4) as usize;
        icon.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn paused_keeps_the_closed_gaps_between_bar_tops() {
        let top = |len: [Option<f32>; 5], i: usize| CENTER - len[i].unwrap() / 2.0;
        let closed = bar_lengths(IconState::Closed, 0.0);
        let paused = bar_lengths(IconState::Paused, 0.0);
        assert_eq!(paused[0], None);
        assert_eq!(paused[4], None);
        for (a, b) in [(1, 2), (2, 3)] {
            assert_eq!(top(closed, a) - top(closed, b), top(paused, a) - top(paused, b));
        }
        assert!(paused[2] > paused[1] && paused[2] > paused[3]);
    }

    #[test]
    fn playing_bars_stay_within_their_peaks_and_move() {
        for step in 0..200 {
            let lengths = bar_lengths(IconState::Playing, step as f32 * 0.05);
            for (len, peak) in lengths.iter().zip(PLAYING_PEAK) {
                assert!((0.0..=peak).contains(&len.unwrap()));
            }
        }
        assert_ne!(
            bar_lengths(IconState::Playing, 0.0),
            bar_lengths(IconState::Playing, 0.2)
        );
    }

    #[test]
    fn frames_are_rounded_tiles_with_state_specific_bars() {
        let renderer = DockIconRenderer::new(128, 1.0);
        let closed = renderer.render(IconState::Closed, 0.0);
        let paused = renderer.render(IconState::Paused, 0.0);
        assert_eq!(closed.pixels.len(), 128 * 128 * 4);
        // Corners are transparent, the tile and the centre bar are opaque.
        assert_eq!(pixel(&closed, 0, 0)[3], 0);
        assert_eq!(pixel(&closed, 64, 8)[3], 255);
        // Outer bars exist only when closed; the centre bar is grey when closed, green otherwise.
        let outer = (148 / 4, 128 / 2);
        assert_ne!(pixel(&closed, outer.0, outer.1), pixel(&paused, outer.0, outer.1));
        let (c, p) = (pixel(&closed, 64, 64), pixel(&paused, 64, 64));
        assert!(c[1].abs_diff(c[0]) < 12);
        assert!(p[1] > p[0] + 60);
    }

    #[test]
    fn art_scale_leaves_a_transparent_margin_around_the_tile() {
        let icon = DockIconRenderer::new(128, 0.8).render(IconState::Paused, 0.0);
        // The tile spans 80% of the canvas: edge midpoints are clear, the inset edge is filled.
        assert_eq!(pixel(&icon, 64, 4)[3], 0);
        assert_eq!(pixel(&icon, 4, 64)[3], 0);
        assert_eq!(pixel(&icon, 64, 14)[3], 255);
        assert_eq!(pixel(&icon, 14, 64)[3], 255);
    }

    #[test]
    fn only_local_playback_animates() {
        let mut state = crate::backend::initial_state();
        assert_eq!(IconState::from_app(&state), IconState::Paused);
        state.player.status = PlaybackStatus::Playing;
        assert_eq!(IconState::from_app(&state), IconState::Playing);
        state.connect.active_device_id = Some("another-device".into());
        assert_eq!(IconState::from_app(&state), IconState::Paused);
    }
}
