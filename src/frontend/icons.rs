//! Bundled Lucide icon font (ISC, see assets/fonts/LICENSE-lucide).
//!
//! Codepoints come from `font/lucide.css` in lucide-static 1.61.0. Re-check
//! them against that file whenever `assets/fonts/lucide.ttf` is replaced.
use super::theme::{Icons, Palette};
use iced::widget::{Text, text};

/// Font bytes registered with Iced at startup.
pub const FONT_BYTES: &[u8] = include_bytes!("../../assets/fonts/lucide.ttf");
/// The family name stored inside `lucide.ttf`.
pub const FONT: iced::Font = iced::Font::with_name("lucide");

/// Glyphs placed in the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    SkipBack,
    SkipForward,
    Shuffle,
    Repeat,
    RepeatOne,
    Volume,
    VolumeLow,
    VolumeMuted,
    ListMusic,
    ListPlus,
    ListEnd,
    Library,
    Search,
    Settings,
    Home,
    ChevronLeft,
    ChevronRight,
    Music,
    User,
    Close,
    Clock,
    Alert,
    LogOut,
    Power,
    Heart,
    /// Save to Your Library (Lucide `circle-plus`).
    CirclePlus,
    /// Saved in Your Library, drawn on a filled accent circle (Lucide `check`).
    Check,
    SearchX,
    /// Window minimize (Lucide `minus`).
    Minimize,
    /// Window maximize (Lucide `square`).
    Maximize,
    /// Window restore (Lucide `copy`: two overlapping squares).
    Restore,
    /// Spotify Connect device picker (Lucide `monitor-speaker`).
    Devices,
    Speaker,
    Smartphone,
    Tablet,
    Tv,
    Laptop,
    /// Fallback for unknown device types.
    Radio,
    /// Equalizer bars next to the playing device (Lucide `audio-lines`).
    Equalizer,
    /// Lyrics view toggle (Lucide `mic-vocal`).
    Lyrics,
    /// Saved albums (Lucide `album`).
    Album,
    /// Followed artists (Lucide `users`).
    Users,
    /// Discover (Lucide `sparkles`).
    Sparkles,
    /// Copy a link (Lucide `link`).
    Link,
    /// Listening stats (Lucide `chart-column`, `bar-chart-3` in this font).
    Stats,
    /// Pull and install a newer build (Lucide `circle-arrow-down`).
    Update,
}

impl Icon {
    /// Private-use codepoint of this glyph in `lucide.ttf`.
    pub fn codepoint(self) -> char {
        match self {
            Icon::Play => '\u{e13c}',
            Icon::Pause => '\u{e12e}',
            Icon::SkipBack => '\u{e15f}',
            Icon::SkipForward => '\u{e160}',
            Icon::Shuffle => '\u{e15e}',
            Icon::Repeat => '\u{e146}',
            Icon::RepeatOne => '\u{e1fd}',
            Icon::Volume => '\u{e1ab}',
            Icon::VolumeLow => '\u{e1aa}',
            Icon::VolumeMuted => '\u{e1ac}',
            Icon::ListMusic => '\u{e2e0}',
            Icon::ListPlus => '\u{e23f}',
            Icon::ListEnd => '\u{e2df}',
            Icon::Library => '\u{e100}',
            Icon::Search => '\u{e151}',
            Icon::Settings => '\u{e154}',
            Icon::Home => '\u{e0f5}',
            Icon::ChevronLeft => '\u{e06e}',
            Icon::ChevronRight => '\u{e06f}',
            Icon::Music => '\u{e122}',
            Icon::User => '\u{e19f}',
            Icon::Close => '\u{e1b2}',
            Icon::Clock => '\u{e250}',
            Icon::Alert => '\u{e077}',
            Icon::LogOut => '\u{e10e}',
            Icon::Power => '\u{e140}',
            Icon::Heart => '\u{e0f2}',
            Icon::CirclePlus => '\u{e081}',
            Icon::Check => '\u{e06c}',
            Icon::SearchX => '\u{e4ad}',
            Icon::Minimize => '\u{e11c}',
            Icon::Maximize => '\u{e167}',
            Icon::Restore => '\u{e09e}',
            Icon::Devices => '\u{e210}',
            Icon::Speaker => '\u{e166}',
            Icon::Smartphone => '\u{e163}',
            Icon::Tablet => '\u{e17e}',
            Icon::Tv => '\u{e195}',
            Icon::Laptop => '\u{e1cd}',
            Icon::Radio => '\u{e142}',
            Icon::Equalizer => '\u{e55a}',
            Icon::Lyrics => '\u{e349}',
            Icon::Album => '\u{e03b}',
            Icon::Users => '\u{e1a4}',
            Icon::Sparkles => '\u{e412}',
            Icon::Link => '\u{e102}',
            Icon::Stats => '\u{e2a3}',
            Icon::Update => '\u{e078}',
        }
    }

    /// Readable word used when a theme selects text labels.
    pub fn label(self) -> &'static str {
        match self {
            Icon::Play => "Play",
            Icon::Pause => "Pause",
            Icon::SkipBack => "Previous",
            Icon::SkipForward => "Next",
            Icon::Shuffle => "Shuffle",
            Icon::Repeat | Icon::RepeatOne => "Repeat",
            Icon::Volume | Icon::VolumeLow => "Volume",
            Icon::VolumeMuted => "Muted",
            Icon::ListMusic => "Queue",
            Icon::ListPlus => "Add to queue",
            Icon::ListEnd => "Play next",
            Icon::Library => "Library",
            Icon::Search => "Search",
            Icon::Settings => "Settings",
            Icon::Home => "Home",
            Icon::ChevronLeft => "Back",
            Icon::ChevronRight => "Forward",
            Icon::Music => "Music",
            Icon::User => "Account",
            Icon::Close => "Close",
            Icon::Clock => "Duration",
            Icon::Alert => "Warning",
            Icon::LogOut => "Sign out",
            Icon::Power => "Quit",
            Icon::Heart => "Liked",
            Icon::CirclePlus => "Save",
            Icon::Check => "Saved",
            Icon::SearchX => "No results",
            Icon::Minimize => "Minimize",
            Icon::Maximize => "Maximize",
            Icon::Restore => "Restore",
            Icon::Devices => "Devices",
            Icon::Speaker => "Speaker",
            Icon::Smartphone => "Phone",
            Icon::Tablet => "Tablet",
            Icon::Tv => "TV",
            Icon::Laptop => "Computer",
            Icon::Radio => "Device",
            Icon::Equalizer => "Playing",
            Icon::Lyrics => "Lyrics",
            Icon::Album => "Albums",
            Icon::Users => "Artists",
            Icon::Sparkles => "Discover",
            Icon::Link => "Link",
            Icon::Stats => "Stats",
            Icon::Update => "Update",
        }
    }
}

/// Render a glyph in the icon font, sized as a square for predictable alignment.
pub fn icon<'a>(icon: Icon, size: f32) -> Text<'a> {
    text(icon.codepoint())
        .font(FONT)
        .size(size)
        .line_height(1.0)
        .width(size)
        .height(size)
        .center()
}

/// Render the glyph, or its word when the theme selects `icons = "text"`.
pub fn glyph<'a>(palette: &Palette, which: Icon, size: f32) -> Text<'a> {
    match palette.icons {
        Icons::Lucide => icon(which, size),
        Icons::Text => text(which.label())
            .font(palette.font())
            .size(palette.font_size.body),
    }
}

/// Solid play/pause shapes for the round primary buttons. Lucide only ships outlines,
/// so these are rasterized once per theme with 4x4 supersampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Solid {
    Play,
    Pause,
    /// The filled heart of a liked song.
    Heart,
}

impl Solid {
    /// Whether a point in the unit square lies inside the shape.
    fn contains(self, x: f32, y: f32) -> bool {
        match self {
            // Shifted right so the triangle looks centred inside a circle.
            Solid::Play => {
                let (left, top, bottom, tip) = (0.26, 0.16, 0.84, 0.86);
                let half: f32 = (bottom - top) / 2.0;
                x >= left && (y - 0.5).abs() <= half * (tip - x) / (tip - left)
            }
            Solid::Pause => {
                (0.18..=0.84).contains(&y)
                    && ((0.22..=0.42).contains(&x) || (0.58..=0.78).contains(&x))
            }
            // The heart curve (x² + y² − 1)³ = x²y³, which spans 2.28 × 2.25 units,
            // scaled into the square with the outline heart's margins.
            Solid::Heart => {
                let (hx, hy) = ((x - 0.5) * 2.5, 1.25 - (y - 0.06) * 2.5);
                (hx * hx + hy * hy - 1.0).powi(3) <= hx * hx * hy.powi(3)
            }
        }
    }
}

/// Rasterize a solid shape in `color` at twice `edge` for crisp downscaling.
pub fn solid(shape: Solid, color: iced::Color, edge: u32) -> iced::widget::image::Handle {
    const SAMPLES: u32 = 4;
    let size: u32 = edge * 2;
    let rgba: [u8; 4] = color.into_rgba8();
    let mut pixels: Vec<u8> = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut inside: u32 = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px: f32 = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let py: f32 = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    inside += u32::from(shape.contains(px, py));
                }
            }
            let alpha: u32 = u32::from(rgba[3]) * inside / (SAMPLES * SAMPLES);
            pixels.extend_from_slice(&[rgba[0], rgba[1], rgba[2], alpha as u8]);
        }
    }
    iced::widget::image::Handle::from_rgba(size, size, pixels)
}

/// Solid glyphs pre-rendered in the colors of the current theme.
pub struct SolidGlyphs {
    /// Player bar: background-colored icon on a text-colored circle.
    pub play: iced::widget::image::Handle,
    pub pause: iced::widget::image::Handle,
    /// Page header and card hover: on_accent icon on an accent circle.
    pub play_large: iced::widget::image::Handle,
    pub pause_large: iced::widget::image::Handle,
    pub play_card: iced::widget::image::Handle,
    /// Liked song: an accent heart.
    pub heart: iced::widget::image::Handle,
}

/// Displayed edge of each solid glyph, in logical pixels.
pub const SOLID_PLAYER: u32 = 16;
pub const SOLID_LARGE: u32 = 24;
pub const SOLID_CARD: u32 = 20;
pub const SOLID_HEART: u32 = 16;

impl SolidGlyphs {
    pub fn new(palette: &Palette) -> Self {
        let c = palette.colors;
        Self {
            play: solid(Solid::Play, c.background, SOLID_PLAYER),
            pause: solid(Solid::Pause, c.background, SOLID_PLAYER),
            play_large: solid(Solid::Play, c.on_accent, SOLID_LARGE),
            pause_large: solid(Solid::Pause, c.on_accent, SOLID_LARGE),
            play_card: solid(Solid::Play, c.on_accent, SOLID_CARD),
            heart: solid(Solid::Heart, c.accent, SOLID_HEART),
        }
    }
}
