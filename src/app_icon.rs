//! The application icon, embedded as PNG renders of `assets/icons/scytify.svg`.

/// Edge lengths of the embedded renders, smallest first.
pub const SIZES: [u32; 5] = [16, 32, 48, 64, 256];

/// Embedded PNG for each entry of [`SIZES`].
const PNGS: [&[u8]; 5] = [
    include_bytes!("../assets/icons/scytify-16.png"),
    include_bytes!("../assets/icons/scytify-32.png"),
    include_bytes!("../assets/icons/scytify-48.png"),
    include_bytes!("../assets/icons/scytify-64.png"),
    include_bytes!("../assets/icons/scytify-256.png"),
];

/// Application id: the Wayland `app_id` and X11 class, matching `scytify.desktop`.
pub const APP_ID: &str = "scytify";

/// A decoded RGBA8 icon.
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decode the embedded render of the given edge length. Panics on a size not in [`SIZES`] or a
/// corrupt asset; both are build-time mistakes covered by the tests.
pub fn rgba(size: u32) -> Rgba {
    let index: usize = SIZES
        .iter()
        .position(|s| *s == size)
        .expect("size is embedded");
    let image = image::load_from_memory_with_format(PNGS[index], image::ImageFormat::Png)
        .expect("embedded icon is a valid PNG")
        .into_rgba8();
    Rgba {
        width: image.width(),
        height: image.height(),
        pixels: image.into_raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_renders_match_their_sizes() {
        for size in SIZES {
            let icon: Rgba = rgba(size);
            assert_eq!((icon.width, icon.height), (size, size));
            assert_eq!(icon.pixels.len(), (size * size * 4) as usize);
        }
    }
}
