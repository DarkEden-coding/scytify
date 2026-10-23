//! Windowed loading: the scroll list is as long as the collection's total, but only one
//! contiguous run of items is loaded. Slots before and after the run are placeholders;
//! the run grows forwards and backwards as the viewport nears its edges and is replaced
//! when the viewport jumps away. Each request stays one bounded page.
use crate::contracts::{MAX_PAGE_SIZE, Page, PageData};

/// Items a single run loads before it stops growing; beyond this the viewport's run is
/// replaced instead.
pub const MAX_LOADED_ITEMS: usize = 1000;

/// Slots loaded beyond the visible window in the scroll direction.
const PREFETCH: usize = 50;

/// Uniform pagination information without cloning metadata: (len, total, has_more).
pub fn page_info(data: &PageData) -> (usize, Option<u64>, bool) {
    match data {
        PageData::Playlists(p) => (p.items.len(), p.total, p.has_more),
        PageData::Tracks(p) => (p.items.len(), p.total, p.has_more),
        PageData::Artists(p) => (p.items.len(), p.total, p.has_more),
        PageData::Albums(p) => (p.items.len(), p.total, p.has_more),
        PageData::Queue { page: p, .. } => (p.items.len(), p.total, p.has_more),
        // Bounded overview: never paged or appended.
        PageData::Search(r) => (
            r.artists.len() + r.albums.len() + r.tracks.len(),
            None,
            false,
        ),
    }
}

/// Backend offset of the first loaded item.
pub fn first_offset(data: &PageData) -> u64 {
    match data {
        PageData::Playlists(p) => p.offset,
        PageData::Tracks(p) => p.offset,
        PageData::Artists(p) => p.offset,
        PageData::Albums(p) => p.offset,
        PageData::Queue { page: p, .. } => p.offset,
        PageData::Search(_) => 0,
    }
}

/// Offset of the page that would continue `data`: after the last raw upstream row, so
/// rows the backend dropped (local files, episodes, others' playlists) are not requested
/// again and a page that kept no rows still advances.
pub fn next_offset(data: &PageData) -> u64 {
    match data {
        PageData::Playlists(p) => p.next_offset,
        PageData::Tracks(p) => p.next_offset,
        PageData::Artists(p) => p.next_offset,
        PageData::Albums(p) => p.next_offset,
        PageData::Queue { page: p, .. } => p.next_offset,
        PageData::Search(_) => 0,
    }
}

/// Whether another page may be appended within the loaded-item bound.
pub fn can_extend(data: &PageData) -> bool {
    let (len, _, more) = page_info(data);
    more && len < MAX_LOADED_ITEMS
}

/// Length of the scroll list: placeholders before the run, the run, and placeholders for
/// the rest of the known total. Without a total the list ends with the run.
pub fn slots(data: &PageData) -> usize {
    let (len, total, more) = page_info(data);
    let after: u64 = match total {
        Some(total) if more => total.saturating_sub(next_offset(data)),
        _ => 0,
    };
    first_offset(data) as usize + len + after as usize
}

/// How a fetched page joins the loaded run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillKind {
    /// Continues the run after its last row.
    Append,
    /// Precedes the run.
    Prepend,
    /// Replaces the run; the viewport moved away from it.
    Replace,
}

/// A page the visible window needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    pub kind: FillKind,
    pub offset: u64,
    pub limit: u16,
}

impl Fill {
    fn new(kind: FillKind, offset: u64, limit: u16) -> Self {
        Self {
            kind,
            offset,
            limit,
        }
    }
    /// Whether an in-flight request still serves `wanted`. A replacement stays valid while
    /// the window has moved only a little, so dragging the scrollbar does not restart it
    /// on every event.
    pub fn serves(&self, wanted: &Fill) -> bool {
        self == wanted
            || (self.kind == FillKind::Replace
                && wanted.kind == FillKind::Replace
                && self.offset.abs_diff(wanted.offset) < u64::from(MAX_PAGE_SIZE / 4))
    }
}

/// The page to fetch for the visible `window` of slots, if the loaded run does not cover
/// it with some prefetch margin.
pub fn wanted_fill(data: &PageData, (start, end): (usize, usize)) -> Option<Fill> {
    let (len, _, more) = page_info(data);
    let first: usize = first_offset(data) as usize;
    let loaded_end: usize = first + len;
    let from: usize = start.saturating_sub(PREFETCH);
    let to: usize = end.saturating_add(PREFETCH);
    let replace = || {
        let offset: usize = start.saturating_sub(usize::from(MAX_PAGE_SIZE) / 2);
        Fill::new(FillKind::Replace, offset as u64, MAX_PAGE_SIZE)
    };
    if to > loaded_end && more {
        return Some(if from <= loaded_end && can_extend(data) {
            Fill::new(FillKind::Append, next_offset(data), MAX_PAGE_SIZE)
        } else {
            replace()
        });
    }
    if from < first {
        return Some(if to >= first && len < MAX_LOADED_ITEMS {
            let offset: u64 = (first as u64).saturating_sub(u64::from(MAX_PAGE_SIZE));
            Fill::new(FillKind::Prepend, offset, (first as u64 - offset) as u16)
        } else {
            replace()
        });
    }
    None
}

/// Append `next` when it directly continues `existing` (same kind, same queue revision,
/// contiguous offset). Hands `next` back and leaves `existing` untouched otherwise.
pub fn append(existing: &mut PageData, next: PageData) -> Result<(), PageData> {
    if first_offset(&next) != next_offset(existing) {
        return Err(next);
    }
    match (existing, next) {
        (PageData::Playlists(a), PageData::Playlists(b)) => extend(a, b),
        (PageData::Tracks(a), PageData::Tracks(b)) => extend(a, b),
        (PageData::Artists(a), PageData::Artists(b)) => extend(a, b),
        (PageData::Albums(a), PageData::Albums(b)) => extend(a, b),
        (
            PageData::Queue {
                revision: r,
                page: a,
            },
            PageData::Queue { revision, page: b },
        ) if *r == revision => extend(a, b),
        (_, next) => Err(next),
    }
}

/// Put `previous` in front of `existing` when it ends exactly where the run begins.
/// Hands `previous` back and leaves `existing` untouched otherwise.
pub fn prepend(existing: &mut PageData, previous: PageData) -> Result<(), PageData> {
    if next_offset(&previous) != first_offset(existing) {
        return Err(previous);
    }
    match (existing, previous) {
        (PageData::Playlists(a), PageData::Playlists(b)) => extend_front(a, b),
        (PageData::Tracks(a), PageData::Tracks(b)) => extend_front(a, b),
        (PageData::Artists(a), PageData::Artists(b)) => extend_front(a, b),
        (PageData::Albums(a), PageData::Albums(b)) => extend_front(a, b),
        (_, previous) => Err(previous),
    }
}

/// Append a page already checked to continue `existing`. Pages are kept whole so the
/// raw continuation stays exact; the window may pass the bound by less than one page.
fn extend<T>(existing: &mut Page<T>, next: Page<T>) -> Result<(), PageData> {
    existing.items.extend(next.items);
    existing.has_more = next.has_more;
    existing.total = next.total.or(existing.total);
    existing.next_offset = next.next_offset;
    Ok(())
}

/// Put a page already checked to precede `existing` in front of it.
fn extend_front<T>(existing: &mut Page<T>, mut previous: Page<T>) -> Result<(), PageData> {
    previous.items.append(&mut existing.items);
    existing.items = previous.items;
    existing.offset = previous.offset;
    existing.total = previous.total.or(existing.total);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Artist, SpotifyId, Track};
    fn artists(offset: u64, len: usize, has_more: bool) -> PageData {
        artists_of(offset, len, None, has_more)
    }
    fn artists_of(offset: u64, len: usize, total: Option<u64>, has_more: bool) -> PageData {
        PageData::Artists(Page {
            offset,
            total,
            items: (0..len)
                .map(|i| Artist {
                    id: SpotifyId(format!("a{i}")),
                    name: String::new(),
                    artwork_url: None,
                })
                .collect(),
            has_more,
            next_offset: offset + len as u64,
        })
    }
    #[test]
    fn appends_only_contiguous_pages_within_bound() {
        let mut data: PageData = artists(0, 100, true);
        assert!(append(&mut data, artists(200, 100, true)).is_err());
        assert!(
            append(
                &mut data,
                PageData::Tracks(Page {
                    offset: 100,
                    total: None,
                    items: vec![],
                    has_more: false,
                    next_offset: 100,
                })
            )
            .is_err()
        );
        assert_eq!(page_info(&data).0, 100);
        for page in 1..10 {
            assert!(can_extend(&data));
            assert!(append(&mut data, artists(page * 100, 100, true)).is_ok());
        }
        assert_eq!(page_info(&data).0, MAX_LOADED_ITEMS);
        assert!(!can_extend(&data));
        assert_eq!(next_offset(&data), 1000);
    }
    /// Rows the backend dropped are neither requested again nor stall paging, even when
    /// a whole page was dropped.
    #[test]
    fn pages_continue_after_raw_rows() {
        let track = |i: u64| Track {
            id: SpotifyId(format!("t{i}")),
            name: String::new(),
            artists: vec![],
            album: None,
            duration_ms: 0,
            playable: true,
            source_index: Some(i),
        };
        let tracks = |offset: u64, raw: u64, items: Vec<Track>| {
            PageData::Tracks(Page {
                offset,
                total: Some(200),
                items,
                has_more: offset + raw < 200,
                next_offset: offset + raw,
            })
        };
        // Raw rows 0..50 with a local file at 3 dropped: 49 tracks, continuation 50.
        let mut data = tracks(0, 50, (0..50).filter(|&i| i != 3).map(track).collect());
        assert_eq!(next_offset(&data), 50);
        assert!(append(&mut data, tracks(49, 51, (49..100).map(track).collect())).is_err());
        // Raw rows 50..100 were all local files: nothing kept, still advances.
        assert!(append(&mut data, tracks(50, 50, vec![])).is_ok());
        assert_eq!(next_offset(&data), 100);
        assert!(append(&mut data, tracks(100, 100, (100..200).map(track).collect())).is_ok());
        assert_eq!((page_info(&data).0, next_offset(&data)), (149, 200));
    }
    /// The list is as long as the total from the first page on, so its end can be reached
    /// before anything past the first page is loaded.
    #[test]
    fn slots_cover_the_known_total() {
        assert_eq!(slots(&artists_of(0, 100, Some(5000), true)), 5000);
        assert_eq!(slots(&artists_of(300, 100, Some(5000), true)), 5000);
        // The last page leaves no trailing placeholders; an unknown total none either.
        assert_eq!(slots(&artists_of(4900, 100, Some(5000), false)), 5000);
        assert_eq!(slots(&artists(0, 100, true)), 100);
    }
    #[test]
    fn window_inside_the_run_loads_nothing() {
        let data = artists_of(0, 100, Some(5000), true);
        assert_eq!(wanted_fill(&data, (0, 20)), None);
        assert_eq!(wanted_fill(&data, (10, 40)), None);
    }
    #[test]
    fn window_at_the_run_edge_extends_it() {
        let data = artists_of(200, 100, Some(5000), true);
        assert_eq!(
            wanted_fill(&data, (270, 290)),
            Some(Fill::new(FillKind::Append, 300, 100))
        );
        assert_eq!(
            wanted_fill(&data, (210, 230)),
            Some(Fill::new(FillKind::Prepend, 100, 100))
        );
        // Unknown totals still grow at the end.
        let open = artists(0, 100, true);
        assert_eq!(
            wanted_fill(&open, (90, 100)),
            Some(Fill::new(FillKind::Append, 100, 100))
        );
        // Nothing follows the last page.
        let last = artists_of(4900, 100, Some(5000), false);
        assert_eq!(wanted_fill(&last, (4990, 5000)), None);
    }
    #[test]
    fn window_far_from_the_run_replaces_it() {
        let data = artists_of(0, 100, Some(5000), true);
        assert_eq!(
            wanted_fill(&data, (4000, 4020)),
            Some(Fill::new(FillKind::Replace, 3950, 100))
        );
        let data = artists_of(4000, 100, Some(5000), true);
        assert_eq!(
            wanted_fill(&data, (0, 20)),
            Some(Fill::new(FillKind::Replace, 0, 100))
        );
        // A run at its bound is replaced instead of growing.
        let full = artists_of(0, MAX_LOADED_ITEMS, Some(5000), true);
        assert_eq!(
            wanted_fill(&full, (990, 1010)),
            Some(Fill::new(FillKind::Replace, 940, 100))
        );
    }
    #[test]
    fn replacement_survives_small_moves_only() {
        let sent = Fill::new(FillKind::Replace, 3950, 100);
        assert!(sent.serves(&Fill::new(FillKind::Replace, 3960, 100)));
        assert!(!sent.serves(&Fill::new(FillKind::Replace, 3000, 100)));
        assert!(!sent.serves(&Fill::new(FillKind::Append, 3950, 100)));
    }
    #[test]
    fn prepends_only_the_page_ending_at_the_run() {
        let mut data = artists_of(200, 100, Some(5000), true);
        assert!(prepend(&mut data, artists_of(50, 100, Some(5000), true)).is_err());
        assert!(prepend(&mut data, artists_of(100, 100, Some(5000), true)).is_ok());
        assert_eq!((first_offset(&data), page_info(&data).0), (100, 200));
        assert_eq!(next_offset(&data), 300);
        assert_eq!(slots(&data), 5000);
    }
}
