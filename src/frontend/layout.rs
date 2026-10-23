//! Fixed layout geometry shared by the views and by artwork visibility.
//!
//! Virtualized lists and grids only build what is on screen, so the view and the
//! request window must agree on every height above and inside them. Keep blocks that
//! precede a virtualized list at the fixed heights declared here.

/// Sidebar and optional queue panel widths.
pub const SIDEBAR_WIDTH: f32 = 280.0;
pub const QUEUE_WIDTH: f32 = 300.0;
/// Height of a sidebar playlist row: 48px thumbnail plus padding.
pub const SIDEBAR_ROW: f32 = 64.0;
/// Horizontal padding inside the main view.
pub const CONTENT_PADDING: f32 = 24.0;
/// Section heading such as "Your playlists" above a grid.
pub const SECTION_HEADING: f32 = 64.0;
/// Artist, album and playlist header with the large cover.
pub const HERO_HEIGHT: f32 = 280.0;
pub const HERO_ART: f32 = 200.0;
/// Big play button row below a playlist or album header.
pub const ACTIONS_HEIGHT: f32 = 88.0;
/// "#  Title  Album  Duration" header above a track table, including its divider.
pub const TABLE_HEADER: f32 = 41.0;

/// Minimum card width (about a 150px cover); cards stretch to fill the row.
pub const GRID_CARD_WIDTH: f32 = 170.0;
pub const GRID_GAP: f32 = 12.0;
pub const CARD_PADDING: f32 = 10.0;
/// Space between the cover and the title.
pub const CARD_TEXT_GAP: f32 = 8.0;
/// Space between the title and the subtitle.
pub const CARD_LINE_GAP: f32 = 2.0;
/// Line height of single-line text, as a factor of the font size (Iced's default).
const LINE_HEIGHT: f32 = 1.3;

/// Height of one line of text at `size`.
pub fn line(size: f32) -> f32 {
    (size * LINE_HEIGHT).ceil()
}

/// Card caption below the cover: one bold title line and one subtitle line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Caption {
    pub title: f32,
    pub subtitle: f32,
}

impl Caption {
    /// Caption for the theme's body and small font sizes.
    pub fn new(body: f32, small: f32) -> Self {
        Self {
            title: line(body),
            subtitle: line(small),
        }
    }
    fn height(&self) -> f32 {
        CARD_TEXT_GAP + self.title + CARD_LINE_GAP + self.subtitle
    }
}

/// Resolved grid for one content width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub columns: usize,
    pub card_width: f32,
    pub card_height: f32,
    pub caption: Caption,
}

impl Grid {
    /// Cover edge inside a card.
    pub fn art(&self) -> f32 {
        self.card_width - 2.0 * CARD_PADDING
    }
    /// Width the cards of a full row take; card widths are whole pixels, so this can be a
    /// few pixels less than the available width. Headings align their right end to it.
    pub fn width(&self) -> f32 {
        self.columns as f32 * self.card_width + GRID_GAP * (self.columns - 1) as f32
    }
}

/// Cards per grid row for a content width; always at least one.
pub fn grid_columns(width: f32) -> usize {
    (((width.max(0.0) + GRID_GAP) / (GRID_CARD_WIDTH + GRID_GAP)) as usize).max(1)
}

/// Columns and stretched card size for a content width; the caption sets the text rows.
pub fn grid(width: f32, caption: Caption) -> Grid {
    let columns: usize = grid_columns(width);
    let gaps: f32 = GRID_GAP * (columns - 1) as f32;
    let card_width: f32 = ((width - gaps) / columns as f32)
        .floor()
        .max(GRID_CARD_WIDTH);
    Grid {
        columns,
        card_width,
        card_height: card_width + caption.height(),
        caption,
    }
}

/// Songs listed beside the search overview's top result.
pub const SEARCH_SONGS: usize = 4;
/// Grid width below which the top result stacks above the songs.
pub const SEARCH_STACK_WIDTH: f32 = 640.0;
/// Top result card: padding, cover edge, the gap below the cover and the gap above the
/// kind pill, whose label has `PILL_PADDING` above and below.
pub const TOP_RESULT_PADDING: f32 = 20.0;
pub const TOP_RESULT_ART: f32 = 92.0;
pub const TOP_RESULT_ART_GAP: f32 = 16.0;
pub const TOP_RESULT_LINE_GAP: f32 = 8.0;
pub const PILL_PADDING: f32 = 4.0;

/// Top row of the search overview: "Top result" (40%) beside "Songs" (60%), or stacked
/// when narrow. Every height is fixed, so the shelves below start at `height()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchTop {
    pub stacked: bool,
    /// Widths of the top result and songs columns (both the full width when stacked).
    pub result_width: f32,
    pub songs_width: f32,
    pub card_height: f32,
    /// Height of the song rows; zero without songs.
    pub songs_height: f32,
}

impl SearchTop {
    /// Geometry for a content `width`, `songs` rows of `row_height` and the theme's display
    /// (name) and small (pill) font sizes.
    pub fn new(width: f32, songs: usize, row_height: f32, display: f32, small: f32) -> Self {
        let songs_height: f32 = songs.min(SEARCH_SONGS) as f32 * row_height;
        let card_content: f32 = 2.0 * TOP_RESULT_PADDING
            + TOP_RESULT_ART
            + TOP_RESULT_ART_GAP
            + line(display)
            + TOP_RESULT_LINE_GAP
            + line(small)
            + 2.0 * PILL_PADDING;
        if width < SEARCH_STACK_WIDTH {
            return Self {
                stacked: true,
                result_width: width,
                songs_width: width,
                card_height: card_content,
                songs_height,
            };
        }
        let result_width: f32 = ((width - GRID_GAP) * 0.4).floor();
        Self {
            stacked: false,
            result_width,
            songs_width: width - GRID_GAP - result_width,
            card_height: card_content.max(songs_height),
            songs_height,
        }
    }
    /// Height of the whole top row, headings included.
    pub fn height(&self) -> f32 {
        let songs: f32 = if self.stacked && self.songs_height > 0.0 {
            SECTION_HEADING + self.songs_height
        } else {
            0.0
        };
        SECTION_HEADING + self.card_height + songs + GRID_GAP
    }
}

/// Overscanned item window for a card grid, by whole rows. `offset` is measured from
/// the top of the grid.
pub fn visible_grid(len: usize, offset: f32, viewport: f32, grid: Grid) -> (usize, usize) {
    let row_height: f32 = grid.card_height + GRID_GAP;
    let first_row: usize = ((offset.max(0.0) / row_height) as usize).saturating_sub(1);
    let rows: usize = (viewport.max(0.0) / row_height).ceil() as usize + 2;
    let start: usize = first_row.saturating_mul(grid.columns).min(len);
    (start, start.saturating_add(rows * grid.columns).min(len))
}

/// Whether a block at `top` with `height` overlaps the viewport scrolled to `scroll`,
/// with one viewport of overscan on each side so art is ready when it scrolls in.
pub fn overlaps(top: f32, height: f32, scroll: f32, viewport: f32) -> bool {
    let viewport: f32 = viewport.max(0.0);
    top < scroll + 2.0 * viewport && top + height > scroll - viewport
}

/// Overscanned fixed-height row window, bounded independently of collection size.
/// `offset` is measured from the top of the first row. A scrolled frame is drawn before
/// the rows for its new offset are built, so one viewport of rows on each side keeps a
/// fast glide (capped below a viewport per frame) from exposing blank space.
pub fn visible_rows(len: usize, offset: f32, viewport: f32, row_height: f32) -> (usize, usize) {
    let row_height: f32 = row_height.max(1.0);
    let page: usize = (viewport.max(0.0) / row_height).ceil() as usize;
    let start: usize = ((offset.max(0.0) / row_height) as usize)
        .saturating_sub(page + 1)
        .min(len);
    let count: usize = 3 * page + 3;
    (start, start.saturating_add(count).min(len))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn virtualization_is_bounded() {
        assert_eq!(visible_rows(0, 0.0, 700.0, 56.0), (0, 0));
        let (a, b) = visible_rows(50000, 100000.0, 700.0, 56.0);
        assert!(b - a <= 42);
        assert!(a > 1000);
        assert_eq!(visible_rows(5, 10000.0, 700.0, 56.0), (5, 5));
    }
    #[test]
    fn grid_window_covers_whole_rows() {
        assert_eq!(grid_columns(0.0), 1);
        let caption: Caption = Caption::new(14.0, 12.0);
        let g: Grid = grid(900.0, caption);
        assert_eq!(g.columns, 5);
        assert!(g.card_width * 5.0 + GRID_GAP * 4.0 <= 900.0);
        // A 1440px window leaves about 1090px for the grid: six covers of about 150px.
        let wide: Grid = grid(1088.0, caption);
        assert_eq!(wide.columns, 6);
        assert!((150.0..=165.0).contains(&wide.art()));
        // The caption fits exactly under the cover: no dead space in the card.
        assert_eq!(g.card_height - g.card_width, 8.0 + 19.0 + 2.0 + 16.0);
        let (a, b) = visible_grid(100, 0.0, 700.0, g);
        assert_eq!(a, 0);
        assert_eq!(b % g.columns, 0);
        let (a, b) = visible_grid(100, 1024.0, 700.0, g);
        assert_eq!(a % g.columns, 0);
        assert!(a > 0 && b - a <= 7 * g.columns);
        assert_eq!(visible_grid(5, 100000.0, 700.0, g), (5, 5));
    }
    #[test]
    fn search_top_row_splits_or_stacks() {
        let wide = SearchTop::new(1088.0, 10, 56.0, 32.0, 12.0);
        assert!(!wide.stacked);
        assert_eq!(wide.result_width + GRID_GAP + wide.songs_width, 1088.0);
        // Four songs set the height; the card matches them.
        assert_eq!((wide.songs_height, wide.card_height), (224.0, 224.0));
        assert_eq!(wide.height(), SECTION_HEADING + 224.0 + GRID_GAP);
        let narrow = SearchTop::new(500.0, 2, 56.0, 32.0, 12.0);
        assert!(narrow.stacked);
        assert_eq!(narrow.songs_width, 500.0);
        assert_eq!(
            narrow.height(),
            2.0 * SECTION_HEADING + narrow.card_height + 112.0 + GRID_GAP
        );
    }
}
