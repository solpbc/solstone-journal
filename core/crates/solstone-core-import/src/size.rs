// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-derived import size. Coordinates are plans, never reservations or consent.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Recording {
    pub day: String,
    pub stream: String,
    pub segment: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportSize {
    pub recordings: BTreeSet<Recording>,
    pub skipped: usize,
    /// Sources without chronicle recordings (for example body data) name their own unit.
    pub other_units: Option<(String, usize)>,
    /// Workouts already listed that this import's facet would move, and the
    /// facets they'd move from.
    pub listed_moves: Option<(usize, Vec<String>)>,
}

impl ImportSize {
    pub fn days(&self) -> BTreeSet<&str> {
        self.recordings
            .iter()
            .map(|recording| recording.day.as_str())
            .collect()
    }

    pub fn render(&self, json: bool) -> String {
        if json {
            let mut size = serde_json::json!({
                "schema": "solstone.import.size.v1",
                "recordings": self.recordings.len(),
                "days": self.days().len(),
                "skipped": self.skipped,
                "other_units": self.other_units,
            });
            if let Some((count, from)) = &self.listed_moves {
                size["listed_moves"] = serde_json::json!({"count": count, "from": from});
            }
            format!("{size}\n")
        } else {
            let mut result = format!(
                "{} {} across {} {}\n",
                self.recordings.len(),
                if self.recordings.len() == 1 {
                    "recording"
                } else {
                    "recordings"
                },
                self.days().len(),
                if self.days().len() == 1 {
                    "day"
                } else {
                    "days"
                }
            );
            if let Some((unit, count)) = &self.other_units {
                result.push_str(&format!("{count} {unit}\n"));
            }
            if self.skipped > 0 {
                result.push_str(&format!("{} skipped\n", self.skipped));
            }
            if let Some((count, from)) = &self.listed_moves {
                result.push_str(&format!(
                    "{} {} already listed on the activities page would move to this import's facet from {}\n",
                    count,
                    if *count == 1 { "workout" } else { "workouts" },
                    from.join(", ")
                ));
            }
            result
        }
    }

    /// Whether a size notice rendered by [`ImportSize::render`] names anything to
    /// add or update. A preview that finds nothing (an already imported file,
    /// archive content already present) has nothing to announce before the import,
    /// so the import's own outcome line stays the first line owners and scripts read.
    /// Output this renderer did not produce is treated as an announcement.
    pub fn rendered_announces_work(rendered: &str) -> bool {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(rendered.trim_end()) {
            return !(value["schema"] == "solstone.import.size.v1"
                && value["recordings"] == 0
                && value["other_units"].is_null()
                && value["listed_moves"].is_null());
        }
        let mut lines = rendered.lines();
        !(lines.next() == Some("0 recordings across 0 days")
            && lines.all(|line| {
                line.strip_suffix(" skipped")
                    .is_some_and(|count| count.parse::<usize>().is_ok())
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sized(recordings: usize, skipped: usize, other_units: Option<(&str, usize)>) -> ImportSize {
        ImportSize {
            recordings: (0..recordings)
                .map(|index| Recording {
                    day: format!("2026031{index}"),
                    stream: "import.audio".to_owned(),
                    segment: "120000_60".to_owned(),
                })
                .collect(),
            skipped,
            other_units: other_units.map(|(unit, count)| (unit.to_owned(), count)),
            listed_moves: None,
        }
    }

    #[test]
    fn only_a_size_with_nothing_to_add_is_quiet_in_both_renderings() {
        for json in [false, true] {
            for (size, announces) in [
                (sized(0, 0, None), false),
                (sized(0, 1, None), false),
                (sized(0, 12, None), false),
                (sized(1, 0, None), true),
                (sized(2, 1, None), true),
                (sized(0, 0, Some(("body rows", 3))), true),
                (sized(0, 1, Some(("body rows", 3))), true),
            ] {
                assert_eq!(
                    ImportSize::rendered_announces_work(&size.render(json)),
                    announces,
                    "{size:?} json={json}"
                );
            }
        }
        assert!(ImportSize::rendered_announces_work(
            "{\"source\":\"apple_health\"}\n"
        ));
        assert!(ImportSize::rendered_announces_work(
            "2 events, 1 unique attendees\n"
        ));
    }

    #[test]
    fn listed_moves_are_said_in_both_forms_and_absent_when_none() {
        let mut size = sized(1, 0, None);
        assert!(!size.render(true).contains("listed_moves"));
        assert!(!size.render(false).contains("already listed"));
        size.listed_moves = Some((2, vec!["Work".to_owned()]));
        assert!(
            size.render(true)
                .contains(r#""listed_moves":{"count":2,"from":["Work"]}"#)
        );
        assert!(size.render(false).contains("2 workouts already listed"));
    }
}
