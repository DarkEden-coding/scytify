//! Bounded, declarative design tokens; layout is deliberately not configurable.
//!
//! Version 2 themes group tokens into `[colors]`, `[shape]`, `[shadow]` and
//! `[font_size]` tables. Version 1 files are still accepted and migrated with
//! derived colors and default sizes.
use iced::Color;
use serde::{Deserialize, Deserializer};
use std::path::Path;

const MAX_THEME_BYTES: usize = 16 * 1024;
const MIN_TEXT_CONTRAST: f32 = 4.5;

/// A validated theme. All fields are plain values, so style closures can copy it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    version: u8,
    pub font_family: FontFamily,
    #[serde(default)]
    pub icons: Icons,
    pub colors: Colors,
    pub shape: Shape,
    pub shadow: Shadow,
    pub font_size: FontSizes,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    /// Window background behind every panel.
    #[serde(deserialize_with = "hex")]
    pub background: Color,
    /// Panels such as the sidebar, main view and cards.
    #[serde(deserialize_with = "hex")]
    pub surface: Color,
    /// Controls and selected rows placed on a surface.
    #[serde(deserialize_with = "hex")]
    pub surface_raised: Color,
    /// Hover state for rows, cards and buttons.
    #[serde(deserialize_with = "hex")]
    pub surface_hover: Color,
    /// Dividers, input outlines and slider rails.
    #[serde(deserialize_with = "hex")]
    pub border: Color,
    #[serde(deserialize_with = "hex")]
    pub text: Color,
    /// Secondary text and idle icons.
    #[serde(deserialize_with = "hex")]
    pub text_subdued: Color,
    /// Primary actions, active icons and the selected row's text.
    #[serde(deserialize_with = "hex")]
    pub accent: Color,
    /// Text and icons drawn on an accent fill.
    #[serde(deserialize_with = "hex")]
    pub on_accent: Color,
    #[serde(deserialize_with = "hex")]
    pub error: Color,
    /// Keyboard focus rings and hover outlines.
    #[serde(deserialize_with = "hex")]
    pub focus: Color,
    /// Fill of the window close button while hovered. Optional in theme files.
    #[serde(default = "default_danger_hover", deserialize_with = "hex")]
    pub danger_hover: Color,
    /// Icon drawn on `danger_hover`. Optional in theme files.
    #[serde(default = "default_on_danger", deserialize_with = "hex")]
    pub on_danger: Color,
}

/// Windows-style close red, for themes that do not set `danger_hover`.
fn default_danger_hover() -> Color {
    Color::from_rgb8(0xE8, 0x11, 0x23)
}

/// White, for themes that do not set `on_danger`.
fn default_on_danger() -> Color {
    Color::WHITE
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    /// Buttons, inputs and list rows.
    pub radius_small: f32,
    /// Panels and cards.
    pub radius_large: f32,
    /// Outline width of cards, panels and inputs; 0 for flat themes.
    pub border_width: f32,
    /// Gap between neighbouring elements.
    pub spacing: f32,
    /// Inner padding of panels and cards.
    pub padding: f32,
    /// Fixed height of virtualized list rows.
    pub row_height: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Shadow {
    pub style: ShadowStyle,
    pub offset: f32,
    pub blur: f32,
    /// Opacity of the dark shadow, 0.0 to 0.6.
    pub strength: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ShadowStyle {
    /// No shadows at all.
    Flat,
    /// One drop shadow below raised elements.
    Soft,
    /// A dark shadow paired with an opposite light highlight.
    Neumorphic,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FontSizes {
    /// Captions, durations and metadata.
    pub small: f32,
    /// Rows, buttons and inputs.
    pub body: f32,
    /// Section headings.
    pub heading: f32,
    /// Page titles.
    pub display: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub enum FontFamily {
    #[serde(rename = "sans-serif")]
    SansSerif,
    #[serde(rename = "serif")]
    Serif,
    #[serde(rename = "monospace")]
    Monospace,
}

/// How controls are labelled: bundled Lucide glyphs, or words only.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Icons {
    #[default]
    #[serde(alias = "symbols")]
    Lucide,
    Text,
}

/// The version 1 schema, kept so existing local themes still load.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteV1 {
    #[serde(rename = "version")]
    _version: u8,
    style: StyleV1,
    #[serde(deserialize_with = "hex")]
    background: Color,
    #[serde(deserialize_with = "hex")]
    surface: Color,
    #[serde(deserialize_with = "hex")]
    text: Color,
    #[serde(deserialize_with = "hex")]
    muted: Color,
    #[serde(deserialize_with = "hex")]
    accent: Color,
    #[serde(deserialize_with = "hex")]
    error: Color,
    #[serde(deserialize_with = "hex")]
    focus: Color,
    font_family: FontFamily,
    #[serde(default)]
    icons: Icons,
    spacing: f32,
    radius: f32,
    shadow_offset: f32,
    shadow_blur: f32,
    shadow_strength: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StyleV1 {
    Neumorphic,
    Material,
}

impl From<PaletteV1> for Palette {
    /// Derive the newer surface tokens from the old two-surface palette.
    fn from(v1: PaletteV1) -> Self {
        Self {
            version: 2,
            font_family: v1.font_family,
            icons: v1.icons,
            colors: Colors {
                background: v1.background,
                surface: v1.surface,
                surface_raised: mix(v1.surface, v1.text, 0.06),
                surface_hover: mix(v1.surface, v1.text, 0.10),
                border: mix(v1.surface, v1.text, 0.18),
                text: v1.text,
                text_subdued: v1.muted,
                accent: v1.accent,
                on_accent: v1.background,
                error: v1.error,
                focus: v1.focus,
                danger_hover: default_danger_hover(),
                on_danger: default_on_danger(),
            },
            shape: Shape {
                radius_small: v1.radius,
                radius_large: v1.radius,
                border_width: 0.0,
                spacing: v1.spacing,
                padding: v1.spacing,
                row_height: 56.0,
            },
            shadow: Shadow {
                style: match v1.style {
                    StyleV1::Neumorphic => ShadowStyle::Neumorphic,
                    StyleV1::Material => ShadowStyle::Soft,
                },
                offset: v1.shadow_offset,
                blur: v1.shadow_blur,
                strength: v1.shadow_strength,
            },
            font_size: FontSizes {
                small: 12.0,
                body: 14.0,
                heading: 24.0,
                display: 32.0,
            },
        }
    }
}

impl Palette {
    /// Load a small, strict local theme; arbitrary fonts, SVG, and executable extensions are rejected.
    pub fn load(path: &Path) -> Result<Self, String> {
        use std::io::Read;
        let file: std::fs::File = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut source: String = String::new();
        file.take(MAX_THEME_BYTES as u64 + 1)
            .read_to_string(&mut source)
            .map_err(|e| e.to_string())?;
        Self::parse(&source)
    }

    /// Parse, migrate and validate a theme before it replaces the current palette.
    pub fn parse(source: &str) -> Result<Self, String> {
        if source.len() > MAX_THEME_BYTES {
            return Err("Theme exceeds 16 KiB".into());
        }
        #[derive(Deserialize)]
        struct Header {
            version: u8,
        }
        let header: Header = toml::from_str(source).map_err(|e| e.to_string())?;
        let palette: Self = match header.version {
            1 => toml::from_str::<PaletteV1>(source)
                .map_err(|e| e.to_string())?
                .into(),
            2 => toml::from_str(source).map_err(|e| e.to_string())?,
            _ => return Err("Unsupported theme version; expected 1 or 2".into()),
        };
        palette.validate()?;
        Ok(palette)
    }

    /// Reject token values outside the ranges the layout is designed for.
    fn validate(&self) -> Result<(), String> {
        let s: &Shape = &self.shape;
        let f: &FontSizes = &self.font_size;
        let ranges: [(&str, f32, f32, f32); 14] = [
            ("shape.radius_small", s.radius_small, 0.0, 32.0),
            ("shape.radius_large", s.radius_large, 0.0, 32.0),
            ("shape.border_width", s.border_width, 0.0, 3.0),
            ("shape.spacing", s.spacing, 4.0, 24.0),
            ("shape.padding", s.padding, 4.0, 32.0),
            ("shape.row_height", s.row_height, 40.0, 80.0),
            ("shadow.offset", self.shadow.offset, 0.0, 12.0),
            ("shadow.blur", self.shadow.blur, 0.0, 24.0),
            ("shadow.strength", self.shadow.strength, 0.0, 0.6),
            ("font_size.small", f.small, 10.0, 16.0),
            ("font_size.body", f.body, 12.0, 20.0),
            ("font_size.heading", f.heading, 16.0, 40.0),
            ("font_size.display", f.display, 20.0, 72.0),
            // The body size must fit a row with room for a second line.
            ("shape.row_height / body", s.row_height / f.body, 2.5, 8.0),
        ];
        for (name, value, min, max) in ranges {
            if !(min..=max).contains(&value) {
                return Err(format!(
                    "Theme token {name} must be between {min} and {max}"
                ));
            }
        }
        let c: &Colors = &self.colors;
        let backgrounds = [
            ("background", c.background),
            ("surface", c.surface),
            ("surface_raised", c.surface_raised),
            ("surface_hover", c.surface_hover),
        ];
        let foregrounds = [
            ("text", c.text),
            ("text_subdued", c.text_subdued),
            ("accent", c.accent),
            ("error", c.error),
            ("focus", c.focus),
        ];
        for (fg_name, fg) in foregrounds {
            for (bg_name, bg) in backgrounds {
                if contrast(fg, bg) < MIN_TEXT_CONTRAST {
                    return Err(format!(
                        "Theme color {fg_name} needs 4.5:1 contrast against {bg_name}"
                    ));
                }
            }
        }
        if contrast(c.on_accent, c.accent) < MIN_TEXT_CONTRAST {
            return Err("Theme color on_accent needs 4.5:1 contrast against accent".into());
        }
        if contrast(c.on_danger, c.danger_hover) < MIN_TEXT_CONTRAST {
            return Err("Theme color on_danger needs 4.5:1 contrast against danger_hover".into());
        }
        Ok(())
    }

    /// Return the bundled default theme.
    pub fn default_theme() -> Self {
        Self::parse(include_str!("../../themes/spotify-dark.toml"))
            .expect("bundled theme is validated by tests")
    }

    /// Map the tokens onto Iced's palette for widgets without an explicit style.
    pub fn iced_theme(&self) -> iced::Theme {
        let c: &Colors = &self.colors;
        iced::Theme::custom(
            "Scytify",
            iced::theme::Palette {
                background: c.background,
                text: c.text,
                primary: c.accent,
                success: c.accent,
                warning: c.focus,
                danger: c.error,
            },
        )
    }

    /// Select a safe system font family without loading files or remote resources.
    pub fn font(&self) -> iced::Font {
        iced::Font {
            family: match self.font_family {
                FontFamily::SansSerif => iced::font::Family::SansSerif,
                FontFamily::Serif => iced::font::Family::Serif,
                FontFamily::Monospace => iced::font::Family::Monospace,
            },
            ..iced::Font::DEFAULT
        }
    }

    /// The theme font in bold, for titles and headings.
    pub fn bold_font(&self) -> iced::Font {
        iced::Font {
            weight: iced::font::Weight::Bold,
            ..self.font()
        }
    }
}

/// Deserialize a `#RRGGBB` token into an opaque color.
fn hex<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Color, D::Error> {
    let token: String = String::deserialize(deserializer)?;
    match token.strip_prefix('#') {
        Some(digits) if digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()) => {
            let rgb: u32 = u32::from_str_radix(digits, 16).map_err(serde::de::Error::custom)?;
            Ok(Color::from_rgb8(
                (rgb >> 16) as u8,
                (rgb >> 8) as u8,
                rgb as u8,
            ))
        }
        _ => Err(serde::de::Error::custom(format!(
            "color {token:?} must be #RRGGBB"
        ))),
    }
}

/// Blend two opaque colors; `amount` 0.0 returns `a`, 1.0 returns `b`.
pub fn mix(a: Color, b: Color, amount: f32) -> Color {
    Color::from_rgb(
        a.r + (b.r - a.r) * amount,
        a.g + (b.g - a.g) * amount,
        a.b + (b.b - a.b) * amount,
    )
}

/// Compute WCAG relative contrast between two opaque sRGB colors.
pub(super) fn contrast(a: Color, b: Color) -> f32 {
    /// Convert one channel from sRGB to linear luminance.
    fn linear(v: f32) -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    /// Compute luminance using the standard RGB weights.
    fn luminance(c: Color) -> f32 {
        0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
    }
    let x: f32 = luminance(a);
    let y: f32 = luminance(b);
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SPOTIFY: &str = include_str!("../../themes/spotify-dark.toml");

    #[test]
    fn bundled_themes_parse_with_their_shadow_styles() {
        let styles = [
            (SPOTIFY, ShadowStyle::Flat),
            (
                include_str!("../../themes/dark-neumorphic.toml"),
                ShadowStyle::Neumorphic,
            ),
            (
                include_str!("../../themes/material.toml"),
                ShadowStyle::Soft,
            ),
        ];
        for (source, style) in styles {
            assert_eq!(Palette::parse(source).unwrap().shadow.style, style);
        }
        assert_eq!(Palette::default_theme().shadow.style, ShadowStyle::Flat);
    }

    #[test]
    fn invalid_themes_are_rejected() {
        let unknown_key: String = format!("layout = 'remote'\n{SPOTIFY}");
        let bad_range: String = SPOTIFY.replace("blur = 0", "blur = 999");
        let bad_color: String = SPOTIFY.replace("\"#B3B3B3\"", "\"B3B3B3\"");
        let low_contrast: String = SPOTIFY.replace("\"#B3B3B3\"", "\"#333333\"");
        let future: String = SPOTIFY.replace("version = 2", "version = 3");
        for source in [unknown_key, bad_range, bad_color, low_contrast, future] {
            assert!(Palette::parse(&source).is_err(), "{source}");
        }
        assert!(Palette::parse(&" ".repeat(MAX_THEME_BYTES + 1)).is_err());
    }

    #[test]
    fn version_one_themes_migrate() {
        let v1: &str = r##"
            version = 1
            style = "neumorphic"
            background = "#171B22"
            surface = "#222832"
            text = "#F1F4F8"
            muted = "#A7B3C5"
            accent = "#9BDFBC"
            error = "#FF9999"
            focus = "#B9D7FF"
            font_family = "sans-serif"
            icons = "symbols"
            spacing = 12
            radius = 18
            shadow_offset = 5
            shadow_blur = 12
            shadow_strength = 0.35
        "##;
        let palette: Palette = Palette::parse(v1).unwrap();
        assert_eq!(palette.shadow.style, ShadowStyle::Neumorphic);
        assert_eq!(palette.icons, Icons::Lucide);
        assert_eq!(palette.shape.radius_large, 18.0);
        assert!(Palette::parse(&v1.replace("radius = 18", "radius = 99")).is_err());
    }
}
