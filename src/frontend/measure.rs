//! Measured single-line text fitting.
//!
//! Iced 0.14 text widgets clip overflowing lines but cannot ellipsize them. This module
//! shapes text with the renderer's own font system, so a shortened label ends with "…"
//! exactly where the real glyphs would overflow. Results are cached because views are
//! rebuilt on every message.
use iced::advanced::graphics::text::Paragraph;
use iced::advanced::text::{self, Paragraph as _};
use iced::{Font, Pixels, Size};
use std::cell::RefCell;
use std::collections::HashMap;

/// Entries kept before the cache is cleared; roughly a few screens of labels.
const MAX_ENTRIES: usize = 4096;

/// Width of `value` on one line at `size` in `font`, in logical pixels.
pub fn width(value: &str, font: Font, size: f32) -> f32 {
    Paragraph::with_text(text::Text {
        content: value,
        bounds: Size::INFINITE,
        size: Pixels(size),
        line_height: text::LineHeight::default(),
        font,
        align_x: text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: text::Shaping::default(),
        wrapping: text::Wrapping::None,
    })
    .min_width()
}

/// Shorten `value` with an ellipsis so it fits `max` pixels; unchanged when it fits.
pub fn ellipsize(value: &str, max: f32, font: Font, size: f32) -> String {
    if width(value, font, size) <= max {
        return value.to_owned();
    }
    let ends: Vec<usize> = value.char_indices().map(|(i, _)| i).skip(1).collect();
    let shortened = |count: usize| -> String {
        let end: usize = if count == 0 { 0 } else { ends[count - 1] };
        format!("{}…", value[..end].trim_end())
    };
    // Largest prefix whose ellipsized form still fits.
    let (mut low, mut high): (usize, usize) = (0, ends.len());
    while low < high {
        let middle: usize = (low + high).div_ceil(2);
        if width(&shortened(middle), font, size) <= max {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    shortened(low)
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    value: String,
    max: u32,
    size: u32,
    font: Font,
}

/// Memoized [`ellipsize`] for view code, which only has shared access to the UI state.
#[derive(Default)]
pub struct FitCache(RefCell<HashMap<Key, String>>);

impl FitCache {
    pub fn fit(&self, value: &str, max: f32, font: Font, size: f32) -> String {
        let key: Key = Key {
            value: value.to_owned(),
            max: max.max(0.0).round() as u32,
            size: size.to_bits(),
            font,
        };
        if let Some(hit) = self.0.borrow().get(&key) {
            return hit.clone();
        }
        let fitted: String = ellipsize(value, key.max as f32, font, size);
        let mut cache = self.0.borrow_mut();
        if cache.len() >= MAX_ENTRIES {
            cache.clear();
        }
        cache.insert(key, fitted.clone());
        fitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ellipsized_text_fits_and_short_text_is_kept() {
        let font: Font = Font::DEFAULT;
        assert_eq!(ellipsize("Short", 400.0, font, 14.0), "Short");
        let long: String = "The Paper Satellites (synthetic) ".repeat(4);
        let fitted: String = ellipsize(&long, 160.0, font, 14.0);
        assert!(fitted.ends_with('…'));
        assert!(width(&fitted, font, 14.0) <= 160.0);
        // Not cut early: keeping one more character would overflow.
        let kept: usize = fitted.chars().count() - 1;
        let prefix: String = long.chars().take(kept + 1).collect();
        let longer: String = format!("{}…", prefix.trim_end());
        assert!(longer == fitted || width(&longer, font, 14.0) > 160.0);
        assert_eq!(ellipsize("Anything", 0.0, font, 14.0), "…");
    }
}
