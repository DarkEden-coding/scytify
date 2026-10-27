//! A view of loaded occurrences. Indices always refer to the original source page.
use crate::domain::Track;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Source,
    Title,
    Artist,
    Album,
    Duration,
}
impl Sort {
    pub const ALL: [Self; 5] = [
        Self::Source,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::Duration,
    ];
}
impl std::fmt::Display for Sort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Source => "Custom order",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Duration => "Duration",
        })
    }
}
pub fn indices(items: &[Track], filter: &str, sort: Sort) -> Vec<usize> {
    let filter = filter.trim().to_lowercase();
    let mut indices: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            filter.is_empty()
                || t.name.to_lowercase().contains(&filter)
                || t.artists
                    .iter()
                    .any(|a| a.name.to_lowercase().contains(&filter))
                || t.album
                    .as_ref()
                    .is_some_and(|a| a.name.to_lowercase().contains(&filter))
        })
        .map(|(i, _)| i)
        .collect();
    indices.sort_by_cached_key(|&i| {
        let t = &items[i];
        match sort {
            Sort::Source => (String::new(), i as u64),
            Sort::Title => (t.name.to_lowercase(), 0),
            Sort::Artist => (
                t.artists
                    .first()
                    .map_or_else(String::new, |a| a.name.to_lowercase()),
                0,
            ),
            Sort::Album => (
                t.album
                    .as_ref()
                    .map_or_else(String::new, |a| a.name.to_lowercase()),
                0,
            ),
            Sort::Duration => (String::new(), t.duration_ms),
        }
    });
    indices
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::SpotifyId;
    #[test]
    fn filtered_sorted_duplicates_keep_their_original_occurrences() {
        let track = |name: &str, source| Track {
            id: SpotifyId("same-id".into()),
            name: name.into(),
            artists: vec![],
            album: None,
            duration_ms: 0,
            playable: true,
            source_index: Some(source),
        };
        let tracks = vec![track("Zebra", 4), track("Alpha", 8), track("Alpha", 12)];
        let shown = indices(&tracks, "ALP", Sort::Title);
        assert_eq!(shown, vec![1, 2]);
        assert_eq!(
            shown
                .iter()
                .map(|&i| tracks[i].source_index.unwrap())
                .collect::<Vec<_>>(),
            vec![8, 12]
        );
    }
}
