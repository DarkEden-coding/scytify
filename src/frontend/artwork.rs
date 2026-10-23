//! Visible-only artwork requests and a 48 MiB decoded cache.
//!
//! The update loop calls `fetch_artwork` after every message. It derives the wanted URLs from
//! what is on screen (player, page header, the virtualized browser, sidebar and queue windows),
//! cancels requests that scrolled away, and keeps at most `ARTWORK_IN_FLIGHT` requests pending.
//! Corners are rounded while decoding because the software renderer ignores image radii.
//! Decoding also records the cover's dominant color for the page header tint. It runs on the
//! blocking pool, so covers arriving while scrolling never stall a frame.
use super::layout::{SIDEBAR_ROW, visible_rows};
use super::tint;
use super::*;
use iced::widget::image::Handle;
use std::collections::VecDeque;

const EDGE_PX: u16 = 160;
const MAX_DECODED_BYTES: usize = 48 * 1024 * 1024;
const MAX_ENTRIES: usize = 512;
const MAX_FAILED: usize = 64;

/// How a cover is cut: album and playlist art gets the theme's rounded corners, artists a circle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    Cover,
    Round,
}

/// Indices of an overscanned row window, those on screen first. The window extends one
/// viewport above and two below, so the screen starts about a third of the way in.
fn nearest_first(start: usize, end: usize) -> impl Iterator<Item = usize> {
    let focus: usize = start + (end - start) / 3;
    let mut indices: Vec<usize> = (start..end).collect();
    indices.sort_by_key(|&i| i.abs_diff(focus));
    indices.into_iter()
}

/// Fade pixels outside a rounded rectangle to transparent, with one pixel of antialiasing.
fn round_corners(pixels: &mut image::RgbaImage, rounding: f32) {
    let (w, h) = (pixels.width() as f32, pixels.height() as f32);
    let radius: f32 = rounding.clamp(0.0, 0.5) * w.min(h);
    if radius < 0.5 {
        return;
    }
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let cx: f32 = px.clamp(radius, w - radius);
        let cy: f32 = py.clamp(radius, h - radius);
        let distance: f32 = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        let coverage: f32 = (radius - distance + 0.5).clamp(0.0, 1.0);
        pixel[3] = (f32::from(pixel[3]) * coverage) as u8;
    }
}

/// One decoded cover: the image, its decoded size and the header tint taken from it.
#[derive(Clone, Debug)]
pub(super) struct Decoded {
    handle: Handle,
    size: usize,
    tint: Option<iced::Color>,
}

#[derive(Default)]
pub(super) struct Artwork {
    entries: HashMap<String, Decoded>,
    /// Eviction order, oldest first. Wanted entries move to the back, so visible art survives.
    order: VecDeque<String>,
    pending: HashMap<RequestId, (String, Shape)>,
    /// Covers that arrived and are being decoded off the UI thread.
    decoding: HashSet<String>,
    failed: VecDeque<String>,
    bytes: usize,
}
impl Artwork {
    /// Find a small decoded image without retaining backend encoded buffers.
    fn get(&self, url: &str) -> Option<Handle> {
        self.entries.get(url).map(|e| e.handle.clone())
    }
    /// Dominant color of a decoded cover, computed once while decoding.
    pub(super) fn tint(&self, url: &str) -> Option<iced::Color> {
        self.entries.get(url).and_then(|e| e.tint)
    }
    fn is_pending(&self, url: &str) -> bool {
        self.decoding.contains(url) || self.pending.values().any(|(u, _)| u == url)
    }
    /// Mark a cached URL as recently wanted.
    fn touch(&mut self, url: &str) {
        if let Some(i) = self.order.iter().position(|u| u == url)
            && i + 1 != self.order.len()
            && let Some(u) = self.order.remove(i)
        {
            self.order.push_back(u);
        }
    }
    /// Evict oldest entries until the decoded image fits both budgets.
    fn insert(&mut self, url: String, decoded: Decoded) {
        let size: usize = decoded.size;
        while self.bytes + size > MAX_DECODED_BYTES || self.entries.len() >= MAX_ENTRIES {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&old) {
                self.bytes -= entry.size;
            }
        }
        self.bytes += size;
        self.order.push_back(url.clone());
        self.entries.insert(url, decoded);
    }
    fn fail(&mut self, url: String) {
        self.failed.push_back(url);
        if self.failed.len() > MAX_FAILED {
            self.failed.pop_front();
        }
    }
    /// Decode PNG/JPEG only after checking dimensions and enforcing decoder allocation limits.
    /// `rounding` is the corner radius as a fraction of the shorter edge (0.5 = circle).
    fn decode(bytes: Vec<u8>, rounding: f32) -> Result<Decoded, String> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Artwork exceeds encoded budget".into());
        }
        let format = image::guess_format(&bytes).map_err(|e| e.to_string())?;
        if !matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg) {
            return Err("Artwork must be PNG or JPEG".into());
        }
        let reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format);
        let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
        if w == 0 || h == 0 || w > 2048 || h > 2048 {
            return Err("Artwork dimensions exceed 2048 pixels".into());
        }
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(2048);
        limits.max_image_height = Some(2048);
        limits.max_alloc = Some(24 * 1024 * 1024);
        reader.limits(limits);
        let edge = u32::from(EDGE_PX);
        let mut pixels = reader
            .decode()
            .map_err(|e| e.to_string())?
            .thumbnail(edge, edge)
            .to_rgba8();
        let tint = tint::dominant(&pixels);
        round_corners(&mut pixels, rounding);
        Ok(Decoded {
            size: pixels.len(),
            handle: Handle::from_rgba(pixels.width(), pixels.height(), pixels.into_raw()),
            tint,
        })
    }
}
impl Ui {
    /// Decoded artwork for a URL, or `None` while loading, failed, or never requested.
    /// Requests come from `fetch_artwork`; rendering an item never starts network work.
    pub(super) fn artwork_handle(&self, url: &str) -> Option<Handle> {
        self.artwork.get(url)
    }
    /// Cancel in-flight artwork when the native window is hidden.
    pub(super) fn cancel_artwork(&mut self) {
        let ids: Vec<_> = self.artwork.pending.drain().map(|(id, _)| id).collect();
        for id in ids {
            self.send(Action::CancelRequest(id));
        }
    }
    /// Corner radius of covers as a fraction of their edge, derived from the theme.
    fn cover_rounding(&self) -> f32 {
        (self.palette.shape.radius_small / 150.0).min(0.1)
    }
    /// Artwork URLs on screen, most important first: player, page header, browser,
    /// sidebar, queue.
    fn wanted_artwork(&self) -> Vec<(String, Shape)> {
        let mut urls: Vec<(&str, Shape)> = Vec::new();
        fn cover(url: &str) -> (&str, Shape) {
            (url, Shape::Cover)
        }
        if let Some(url) = self
            .state
            .player
            .current
            .as_ref()
            .and_then(Track::artwork_url)
        {
            urls.push(cover(url));
        }
        if !self.settings {
            if let Some(url) = self.header_artwork() {
                let shape: Shape = if matches!(self.query, Query::ArtistAlbums { .. }) {
                    Shape::Round
                } else {
                    Shape::Cover
                };
                urls.push((url, shape));
            }
            if let Some(data) = &self.data {
                let (start, end) = self.browser_window(data);
                match data {
                    PageData::Playlists(p) => urls.extend(
                        p.items[start..end]
                            .iter()
                            .filter_map(|x| x.artwork_url.as_deref())
                            .map(cover),
                    ),
                    PageData::Artists(p) => urls.extend(
                        p.items[start..end]
                            .iter()
                            .filter_map(|x| Some((x.artwork_url.as_deref()?, Shape::Round))),
                    ),
                    PageData::Albums(p) => urls.extend(
                        p.items[start..end]
                            .iter()
                            .filter_map(|x| x.artwork_url.as_deref())
                            .map(cover),
                    ),
                    PageData::Tracks(p) => urls.extend(
                        nearest_first(start, end)
                            .filter_map(|i| p.items[i].artwork_url())
                            .map(cover),
                    ),
                    PageData::Queue { page, .. } => urls.extend(
                        page.items[start..end]
                            .iter()
                            .filter_map(|x| x.track.as_ref()?.artwork_url())
                            .map(cover),
                    ),
                    PageData::Search(r) => {
                        let layout = self.search_layout(r);
                        let (columns, shelf) = (self.grid_layout().columns, self.shelf_height());
                        if let Some(top) = self.top_result(r)
                            && let Some(url) = top.artwork_url()
                            && layout
                                .top
                                .is_some_and(|row| self.on_screen(0.0, row.height()))
                        {
                            urls.push((url, top.shape()));
                        }
                        // Song rows: the window is empty unless the top row is on screen.
                        urls.extend(
                            r.tracks[start..end]
                                .iter()
                                .filter_map(Track::artwork_url)
                                .map(cover),
                        );
                        if layout.artists.is_some_and(|top| self.on_screen(top, shelf)) {
                            urls.extend(
                                r.artists.iter().take(columns).filter_map(|x| {
                                    Some((x.artwork_url.as_deref()?, Shape::Round))
                                }),
                            );
                        }
                        if layout.albums.is_some_and(|top| self.on_screen(top, shelf)) {
                            urls.extend(
                                r.albums
                                    .iter()
                                    .take(columns)
                                    .filter_map(|x| x.artwork_url.as_deref())
                                    .map(cover),
                            );
                        }
                    }
                }
            }
            // The saved-albums shelf sits below the playlist shelf on Home.
            let shelf: f32 = self.shelf_height();
            if self.home
                && let Some(albums) = &self.shelf
                && self.on_screen(shelf, shelf)
            {
                urls.extend(
                    albums
                        .items
                        .iter()
                        .take(self.grid_layout().columns)
                        .filter_map(|x| x.artwork_url.as_deref())
                        .map(cover),
                );
            }
        }
        let playlists: Vec<&Playlist> = self.sidebar_items();
        let (start, end) = visible_rows(
            playlists.len(),
            self.sidebar_scroll - self.sidebar_list_top(),
            self.sidebar_viewport,
            SIDEBAR_ROW,
        );
        urls.extend(
            playlists[start..end]
                .iter()
                .filter_map(|x| x.artwork_url.as_deref())
                .map(cover),
        );
        if self.queue {
            let row_height: f32 = self.palette.shape.row_height;
            if self.state.connect.is_remote() {
                let tracks: &[Track] = &self.state.connect.queue_preview;
                let (start, end) = visible_rows(
                    tracks.len(),
                    self.queue_scroll,
                    self.queue_viewport,
                    row_height,
                );
                urls.extend(
                    tracks[start..end]
                        .iter()
                        .filter_map(Track::artwork_url)
                        .map(cover),
                );
            } else {
                let items: Vec<&QueueItem> = self.queue_items();
                let (start, end) = visible_rows(
                    items.len(),
                    self.queue_scroll,
                    self.queue_viewport,
                    row_height,
                );
                urls.extend(
                    items[start..end]
                        .iter()
                        .filter_map(|x| x.track.as_ref()?.artwork_url())
                        .map(cover),
                );
            }
        }
        let mut unique: Vec<(String, Shape)> = Vec::with_capacity(urls.len());
        for (url, shape) in urls {
            if !unique.iter().any(|(u, _)| u == url) {
                unique.push((url.to_owned(), shape));
            }
        }
        unique
    }
    /// Reconcile pending requests with what is visible: cancel requests that scrolled away,
    /// keep visible cached entries fresh, and request missing ones up to the in-flight bound.
    pub(super) fn fetch_artwork(&mut self) {
        if self.window.is_none() {
            return;
        }
        let wanted = self.wanted_artwork();
        let stale: Vec<RequestId> = self
            .artwork
            .pending
            .iter()
            .filter(|(_, (url, _))| !wanted.iter().any(|(u, _)| u == url))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.artwork.pending.remove(&id);
            self.send(Action::CancelRequest(id));
        }
        for (url, shape) in wanted {
            if self.artwork.entries.contains_key(&url) {
                self.artwork.touch(&url);
                continue;
            }
            if self.artwork.is_pending(&url)
                || self.artwork.failed.contains(&url)
                || self.artwork.pending.len() >= ARTWORK_IN_FLIGHT
            {
                continue;
            }
            if let Some(id) = self.send(Action::Artwork {
                url: url.clone(),
                edge_px: EDGE_PX,
            }) {
                self.artwork.pending.insert(id, (url, shape));
            }
        }
    }
    /// Consume an artwork completion; `None` if the request was not (or no longer) ours.
    /// Bytes are decoded on the blocking pool and land through `artwork_decoded`. Failures
    /// are suppressed silently: the placeholder is the visible result, and a bounded failed
    /// list prevents request loops.
    pub(super) fn complete_artwork(
        &mut self,
        id: RequestId,
        bytes: Option<Vec<u8>>,
    ) -> Option<Task<Message>> {
        let (url, shape) = self.artwork.pending.remove(&id)?;
        let Some(bytes) = bytes else {
            self.artwork.fail(url);
            return Some(Task::none());
        };
        let rounding: f32 = match shape {
            Shape::Cover => self.cover_rounding(),
            Shape::Round => 0.5,
        };
        self.artwork.decoding.insert(url.clone());
        Some(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || Artwork::decode(bytes, rounding))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
            },
            move |decoded| Message::ArtworkDecoded(url.clone(), decoded),
        ))
    }
    /// Store a cover decoded off the UI thread, unless the cache was reset meanwhile.
    pub(super) fn artwork_decoded(&mut self, url: String, decoded: Result<Decoded, String>) {
        if !self.artwork.decoding.remove(&url) {
            return;
        }
        match decoded {
            Ok(decoded) => self.artwork.insert(url, decoded),
            Err(_) => self.artwork.fail(url),
        }
    }
    /// Drop decoded covers whose baked corners no longer match the theme.
    pub(super) fn reset_artwork(&mut self) {
        self.cancel_artwork();
        self.artwork = Default::default();
    }
    /// Liked Songs stand-in cover: the accent gradient with a heart, at any edge.
    pub(super) fn liked_cover(&self, edge: f32) -> Element<'_> {
        let p: &Palette = &self.palette;
        // Like the art placeholders, the heart is drawn even for text-icon themes.
        let heart =
            icons::icon(icons::Icon::Heart, (edge * 0.42).round()).color(p.colors.on_accent);
        iced::widget::container(heart)
            .center_x(edge)
            .center_y(edge)
            .style(styles::liked_cover(p, self.cover_rounding() * edge))
            .into()
    }
    /// Render cached artwork or a themed placeholder; never starts network work.
    pub(super) fn cover(&self, url: Option<&str>, edge: f32, shape: Shape) -> Element<'_> {
        if let Some(handle) = url.and_then(|u| self.artwork_handle(u)) {
            return iced::widget::image(handle)
                .width(edge)
                .height(edge)
                .content_fit(iced::ContentFit::Cover)
                .into();
        }
        let radius: f32 = match shape {
            Shape::Cover => self.cover_rounding() * edge,
            Shape::Round => edge / 2.0,
        };
        let icon = match shape {
            Shape::Cover => icons::Icon::Music,
            Shape::Round => icons::Icon::User,
        };
        iced::widget::container(
            icons::icon(icon, (edge * 0.36).round()).color(self.palette.colors.text_subdued),
        )
        .center_x(edge)
        .center_y(edge)
        .style(styles::placeholder(&self.palette, radius))
        .into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn png(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &vec![255; (width * height * 4) as usize],
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        bytes
    }
    #[test]
    fn dimensions_are_checked_and_decoded_images_are_small() {
        assert!(Artwork::decode(png(2049, 1), 0.0).is_err());
        let decoded = Artwork::decode(png(320, 320), 0.5).unwrap();
        assert_eq!(decoded.size, 160 * 160 * 4);
        assert_eq!(decoded.tint, Some(iced::Color::WHITE));
    }
    #[test]
    fn round_corners_are_transparent() {
        let mut pixels = image::RgbaImage::from_pixel(40, 40, image::Rgba([255; 4]));
        round_corners(&mut pixels, 0.5);
        assert_eq!(pixels.get_pixel(0, 0)[3], 0);
        assert_eq!(pixels.get_pixel(20, 20)[3], 255);
        assert!(pixels.get_pixel(20, 0)[3] > 240);
    }
    #[test]
    fn rejects_non_image_and_large_encoded_input() {
        assert!(Artwork::decode(vec![0; 8 * 1024 * 1024 + 1], 0.0).is_err());
        assert!(Artwork::decode(b"not artwork".to_vec(), 0.0).is_err());
    }
    #[test]
    fn cache_evicts_oldest_unwanted_entry_first() {
        let mut art = Artwork::default();
        let decoded = Artwork::decode(png(160, 160), 0.1).unwrap();
        let (handle, size, tint) = (decoded.handle, decoded.size, decoded.tint);
        let entry = |handle: &Handle| Decoded {
            handle: handle.clone(),
            size,
            tint,
        };
        // 160px thumbnails hit the 48 MiB budget before the entry limit.
        let fits: usize = MAX_DECODED_BYTES / size;
        for i in 0..fits {
            art.insert(format!("u{i}"), entry(&handle));
        }
        let kept = art.entries.len();
        assert_eq!(kept, fits);
        art.touch("u0");
        art.insert("new".into(), entry(&handle));
        assert!(art.bytes <= MAX_DECODED_BYTES);
        assert_eq!(art.entries.len(), kept);
        assert!(art.get("u0").is_some());
        assert!(art.get("u1").is_none());
        assert!(art.get("new").is_some());
    }
}
