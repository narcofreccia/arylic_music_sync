//! Now-playing metadata, mapped from librespot's `AudioItem` to a small
//! serializable shape for the frontend (Phase S3).
//!
//! The conversion itself touches librespot types, but the fiddly bits — joining
//! multiple artists and choosing the best cover image — are factored into pure
//! functions ([`join_artists`], [`pick_largest_cover`]) that are unit-tested
//! without any librespot machinery.

use librespot_metadata::audio::item::{AudioItem, UniqueFields};
use serde::Serialize;

/// Now-playing track metadata surfaced to the UI via the `spotify-state` event.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// URL of the largest available cover image, if any.
    pub art_url: Option<String>,
    pub duration_ms: u32,
}

impl TrackMeta {
    /// Map a librespot `AudioItem` (carried by `PlayerEvent::TrackChanged`) to the
    /// UI shape. Handles tracks, podcast episodes and local files; unknown artist
    /// or album degrade to empty strings rather than failing.
    pub fn from_audio_item(item: &AudioItem) -> Self {
        let (artist, album) = match &item.unique_fields {
            UniqueFields::Track { artists, album, .. } => {
                let names: Vec<String> = artists.0.iter().map(|a| a.name.clone()).collect();
                (join_artists(&names), album.clone())
            }
            UniqueFields::Episode { show_name, .. } => (show_name.clone(), String::new()),
            UniqueFields::Local {
                artists, album, ..
            } => (
                artists.clone().unwrap_or_default(),
                album.clone().unwrap_or_default(),
            ),
        };

        let covers: Vec<(String, i32, i32)> = item
            .covers
            .iter()
            .map(|c| (c.url.clone(), c.width, c.height))
            .collect();

        Self {
            title: item.name.clone(),
            artist,
            album,
            art_url: pick_largest_cover(&covers),
            duration_ms: item.duration_ms,
        }
    }
}

/// Join artist names for display: `"A"`, `"A & B"` collapses to comma-separated
/// `"A, B"`. Empty names are dropped so a stray blank never yields `", "`.
pub fn join_artists(names: &[String]) -> String {
    names
        .iter()
        .filter(|n| !n.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

/// Pick the URL of the largest cover (by pixel area) from `(url, width, height)`
/// triples. Zero-area entries (dimensions unknown) still count so a lone
/// dimensionless cover is not discarded. Returns `None` if there are no covers.
pub fn pick_largest_cover(covers: &[(String, i32, i32)]) -> Option<String> {
    covers
        .iter()
        .max_by_key(|(_, w, h)| (*w as i64) * (*h as i64))
        .map(|(url, _, _)| url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_artists_comma_separates() {
        assert_eq!(join_artists(&["A".into(), "B".into()]), "A, B");
        assert_eq!(join_artists(&["Solo".into()]), "Solo");
        assert_eq!(join_artists(&[]), "");
    }

    #[test]
    fn join_artists_drops_blanks() {
        assert_eq!(
            join_artists(&["A".into(), "  ".into(), "C".into()]),
            "A, C"
        );
    }

    #[test]
    fn pick_largest_cover_by_area() {
        let covers = vec![
            ("small".into(), 64, 64),
            ("big".into(), 640, 640),
            ("mid".into(), 300, 300),
        ];
        assert_eq!(pick_largest_cover(&covers), Some("big".into()));
    }

    #[test]
    fn pick_largest_cover_handles_empty_and_dimensionless() {
        assert_eq!(pick_largest_cover(&[]), None);
        assert_eq!(
            pick_largest_cover(&[("only".into(), 0, 0)]),
            Some("only".into())
        );
    }
}
