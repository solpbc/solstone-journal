// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono_tz::Tz;
use solstone_core_format::content::{IndexChunk, PlaceFormatter, ProducedChunks};
use solstone_core_journal_io::paths::Segment;

use crate::{AreaTier, DeviceDirectory, accuracy::Packs, places::PlaceStore};

/// Read-only naming context for one location file. A caller must have read a
/// valid place store; an unavailable store cannot be replaced by an empty one.
pub struct SegmentPlaceFormatter<'a> {
    pub day: &'a str,
    pub segment: &'a Segment,
    pub tier: &'a AreaTier,
    pub packs: Option<&'a Packs>,
    pub directory: &'a dyn DeviceDirectory,
    pub owner: Tz,
    pub places: &'a PlaceStore,
}

impl PlaceFormatter for SegmentPlaceFormatter<'_> {
    fn render(&self, text: &str) -> ProducedChunks {
        let mut produced = ProducedChunks {
            chunks: Vec::new(),
            agent_override: None,
            header: None,
            error: None,
            warnings: Vec::new(),
        };
        match crate::day::read_segment_text(
            self.day,
            self.segment,
            text,
            self.tier,
            self.packs,
            self.directory,
            self.owner,
        ) {
            Ok(view) => produced.chunks.push(IndexChunk {
                content: crate::projection::render_day_document(&view, self.places),
                occurrence_time_ms: None,
                source: None,
                group_offset_ms: None,
            }),
            Err(error) => produced.error = Some(error),
        }
        produced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day::tests::{directory, fix, iphone_header, tier, write};
    use crate::places::{Place, PlaceStore};
    use solstone_core_format::content::{FileFormat, FormatterView, format_file};

    #[test]
    fn one_segment_uses_day_names_and_hides_without_coordinates() {
        let root = tempfile::tempdir().unwrap();
        let day = "20261001";
        let text = format!(
            "{}\n{}\n",
            iphone_header("full", "full"),
            r#"{"arrival":"2026-10-01T21:02:00Z","departure":"2026-10-01T22:40:00Z","h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#
        );
        write(
            root.path(),
            day,
            "phone_mobile-segment",
            "150000_300",
            &text,
        );
        // An unrelated segment must contribute no text to this formatter.
        write(
            root.path(),
            day,
            "phone_mobile-segment",
            "140000_300",
            &format!(
                "{}\n{}\n",
                iphone_header("full", "full"),
                fix("2026-10-01T20:00:10Z", 40.01, -105.3, 10.0)
            ),
        );
        let segment =
            Segment::resolve_exact(root.path(), day, "phone_mobile-segment", "150000_300")
                .unwrap()
                .unwrap();
        let directory = directory();
        let mut places = PlaceStore {
            version: 1,
            places: vec![Place {
                id: "named".to_owned(),
                latitude: 39.7527,
                longitude: -105.0001,
                name: Some("climbing gym".to_owned()),
                alias: None,
                hidden: false,
                days: Vec::new(),
                dormant: false,
            }],
        };
        let render = |places: &PlaceStore, text: &str| {
            let formatter = SegmentPlaceFormatter {
                day,
                segment: &segment,
                tier: tier(),
                packs: None,
                directory: &directory,
                owner: chrono_tz::America::Denver,
                places,
            };
            format_file(
                FileFormat::Place,
                "location.jsonl",
                text,
                FormatterView::Places(&formatter),
            )
        };
        let visible = render(&places, &text);
        let prose = &visible.chunks[0].content;
        assert!(prose.contains("climbing gym"));
        assert!(prose.contains("Denver"));
        assert!(!prose.contains("14:00"));
        for coordinate in ["39.7527", "105.0001", "40.01", "105.3"] {
            assert!(!prose.contains(coordinate));
        }
        places.places[0].hidden = true;
        let hidden = render(&places, &text);
        assert!(!hidden.chunks[0].content.contains("climbing gym"));
        assert!(!hidden.chunks[0].content.contains("Denver"));
        assert!(!hidden.chunks[0].content.contains("15:02"));
        let damaged = render(&places, "not a location row");
        assert!(damaged.error.is_some());
        assert!(damaged.chunks.is_empty());
    }

    #[test]
    fn android_readings_use_the_same_coordinate_free_projection() {
        let root = tempfile::tempdir().unwrap();
        let text = r#"{"provider":"gps","timestamp":1790884810000,"lat":39.7555,"lon":-105.2211,"accuracy":10,"fixAge":0}"#;
        write(
            root.path(),
            "20261001",
            "pixel_location",
            "140000_300",
            text,
        );
        let segment =
            Segment::resolve_exact(root.path(), "20261001", "pixel_location", "140000_300")
                .unwrap()
                .unwrap();
        let directory = directory();
        let places = PlaceStore::default();
        let formatter = SegmentPlaceFormatter {
            day: "20261001",
            segment: &segment,
            tier: tier(),
            packs: None,
            directory: &directory,
            owner: chrono_tz::America::Denver,
            places: &places,
        };
        let result = format_file(
            FileFormat::Place,
            "location.jsonl",
            text,
            FormatterView::Places(&formatter),
        );
        assert!(result.error.is_none());
        let prose = &result.chunks[0].content;
        assert!(prose.contains("Golden"), "{prose}");
        assert!(!prose.contains("39.7555"));
        assert!(!prose.contains("105.2211"));
    }
}
