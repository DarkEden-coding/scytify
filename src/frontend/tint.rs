//! Page header tint taken from cover art.
//!
//! `dominant` runs once per decoded cover and favors saturated pixels, so a colorful cover
//! does not average out to gray. `readable` then boosts saturation and moves lightness away
//! from the theme text until text on the tint keeps at least 4.5:1 contrast.
use super::theme::contrast;
use iced::Color;

/// Minimum contrast between the header text and its tinted backdrop.
const MIN_CONTRAST: f32 = 4.5;

/// Saturation-weighted average color of the opaque pixels, sampled on a sparse grid.
pub fn dominant(pixels: &image::RgbaImage) -> Option<Color> {
    let step: usize = (pixels.width().max(pixels.height()) as usize / 32).max(1);
    let (mut sum, mut weight): ([f32; 3], f32) = ([0.0; 3], 0.0);
    for y in (0..pixels.height()).step_by(step) {
        for x in (0..pixels.width()).step_by(step) {
            let [r, g, b, a] = pixels.get_pixel(x, y).0;
            if a < 128 {
                continue;
            }
            let rgb: [f32; 3] = [r, g, b].map(|v| f32::from(v) / 255.0);
            let (_, saturation, _) = hsl(Color::from_rgb(rgb[0], rgb[1], rgb[2]));
            let w: f32 = 0.1 + saturation * saturation;
            for (total, channel) in sum.iter_mut().zip(rgb) {
                *total += channel * w;
            }
            weight += w;
        }
    }
    (weight > 0.0).then(|| Color::from_rgb(sum[0] / weight, sum[1] / weight, sum[2] / weight))
}

/// A vivid version of `color` that keeps `text` readable on top of it.
pub fn readable(color: Color, text: Color) -> Color {
    let (hue, saturation, lightness) = hsl(color);
    let saturation: f32 = (saturation * 1.3 + 0.1).min(0.85);
    let light_text: bool = hsl(text).2 >= 0.5;
    // Keep the backdrop in a calm band before checking contrast.
    let mut lightness: f32 = if light_text {
        lightness.clamp(0.18, 0.42)
    } else {
        lightness.clamp(0.6, 0.85)
    };
    let mut tint: Color = from_hsl(hue, saturation, lightness);
    while contrast(tint, text) < MIN_CONTRAST && (0.0..=1.0).contains(&lightness) {
        lightness += if light_text { -0.02 } else { 0.02 };
        tint = from_hsl(hue, saturation, lightness.clamp(0.0, 1.0));
    }
    tint
}

/// Hue in turns (0..1), saturation and lightness of an sRGB color.
fn hsl(c: Color) -> (f32, f32, f32) {
    let max: f32 = c.r.max(c.g).max(c.b);
    let min: f32 = c.r.min(c.g).min(c.b);
    let lightness: f32 = (max + min) / 2.0;
    let delta: f32 = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation: f32 = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue: f32 = if max == c.r {
        ((c.g - c.b) / delta).rem_euclid(6.0)
    } else if max == c.g {
        (c.b - c.r) / delta + 2.0
    } else {
        (c.r - c.g) / delta + 4.0
    };
    (hue / 6.0, saturation.min(1.0), lightness)
}

fn from_hsl(hue: f32, saturation: f32, lightness: f32) -> Color {
    let chroma: f32 = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h: f32 = hue * 6.0;
    let x: f32 = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m: f32 = lightness - chroma / 2.0;
    Color::from_rgb(r + m, g + m, b + m)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dominant_color_prefers_saturated_opaque_pixels() {
        let mut pixels = image::RgbaImage::from_pixel(64, 64, image::Rgba([128, 128, 128, 255]));
        for y in 0..64 {
            for x in 0..16 {
                pixels.put_pixel(x, y, image::Rgba([220, 30, 30, 255]));
            }
        }
        let color: Color = dominant(&pixels).unwrap();
        assert!(color.r > color.g + 0.15, "{color:?}");
        let transparent = image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 0]));
        assert!(dominant(&transparent).is_none());
    }
    #[test]
    fn tint_keeps_text_readable_and_hue() {
        for cover in [
            Color::WHITE,
            Color::from_rgb(1.0, 0.9, 0.2),
            Color::from_rgb(0.4, 0.8, 0.9),
            Color::BLACK,
        ] {
            assert!(contrast(readable(cover, Color::WHITE), Color::WHITE) >= MIN_CONTRAST);
            let dark_text: Color = Color::from_rgb(0.1, 0.1, 0.1);
            assert!(contrast(readable(cover, dark_text), dark_text) >= MIN_CONTRAST);
        }
        let red: Color = readable(Color::from_rgb(0.9, 0.2, 0.2), Color::WHITE);
        assert!(red.r > red.g && red.r > red.b);
    }
}
