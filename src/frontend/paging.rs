//! Incremental loading: following pages are appended to a bounded window instead of
//! replacing it, so long lists scroll continuously. Each request stays one bounded page.
use crate::contracts::{Page, PageData};

/// Items a single list loads before it stops appending; beyond this the browser falls
/// back to paging.
pub const MAX_LOADED_ITEMS: usize = 1000;

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

/// Append `next` when it directly continues `existing` (same kind, same queue revision,
/// contiguous offset). Returns false and leaves `existing` untouched otherwise.
pub fn append(existing: &mut PageData, next: PageData) -> bool {
    if first_offset(&next) != next_offset(existing) {
        return false;
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
        _ => false,
    }
}

/// Append a page already checked to continue `existing`. Pages are kept whole so the
/// raw continuation stays exact; the window may pass the bound by less than one page.
fn extend<T>(existing: &mut Page<T>, next: Page<T>) -> bool {
    existing.items.extend(next.items);
    existing.has_more = next.has_more;
    existing.total = next.total.or(existing.total);
    existing.next_offset = next.next_offset;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Artist, SpotifyId, Track};
    fn artists(offset: u64, len: usize, has_more: bool) -> PageData {
        PageData::Artists(Page {
            offset,
            total: None,
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
        assert!(!append(&mut data, artists(200, 100, true)));
        assert!(!append(
            &mut data,
            PageData::Tracks(Page {
                offset: 100,
                total: None,
                items: vec![],
                has_more: false,
                next_offset: 100,
            })
        ));
        assert_eq!(page_info(&data).0, 100);
        for page in 1..10 {
            assert!(can_extend(&data));
            assert!(append(&mut data, artists(page * 100, 100, true)));
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
        assert!(!append(
            &mut data,
            tracks(49, 51, (49..100).map(track).collect())
        ));
        // Raw rows 50..100 were all local files: nothing kept, still advances.
        assert!(append(&mut data, tracks(50, 50, vec![])));
        assert_eq!(next_offset(&data), 100);
        assert!(append(
            &mut data,
            tracks(100, 100, (100..200).map(track).collect())
        ));
        assert_eq!((page_info(&data).0, next_offset(&data)), (149, 200));
    }
}
