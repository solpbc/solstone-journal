// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a Strava import says it did, in `imports/<id>/summary.md`.
//!
//! Every sentence the summary can hold is written here, so the words can be
//! read and changed in one place. It holds counts and the facet's name, never
//! a workout's words or a name read from them.

use std::fmt::Write as _;

use crate::body_activity::RecordCounts;
use crate::strava_filing::{FacetBasis, Filing, RunFacet};
use crate::strava_names::{NamesWait, StravaNamesOutcome};

/// Everything the summary counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StravaRunSummary {
    pub new_workouts: u64,
    pub updated_workouts: u64,
    /// Workouts an earlier version placed at the wrong time, placed again.
    pub replaced_workouts: u64,
    pub facet: RunFacet,
    /// The facet's title as the owner sees it, when the facet has one.
    pub facet_title: Option<String>,
    pub filing: Filing,
    pub records: RecordCounts,
    pub names: StravaNamesOutcome,
    pub conversion: crate::strava_convert::ConversionCounts,
}

fn plural(count: u64, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

const NO_FACET_MEANS: &str = "a workout with no facet isn't listed with your activities, no people or things come from it, and agents limited to some facets won't see it. search and the body app still show it.";

/// `one` when `count` is 1, otherwise `many`.
fn either<'a>(count: u64, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

impl StravaRunSummary {
    /// The summary's text, in the owner's words.
    #[must_use]
    pub(crate) fn text(&self) -> String {
        let mut text = String::from("# Strava import\n\n");
        let added = match self.new_workouts {
            0 => "no new workouts".to_owned(),
            count => format!("{} added", plural(count, "workout", "workouts")),
        };
        if self.updated_workouts > 0 {
            let _ = writeln!(
                text,
                "{added}, {} changed since the last import and brought up to date.\n",
                plural(self.updated_workouts, "workout", "workouts"),
            );
        } else {
            let _ = writeln!(text, "{added}.\n");
        }
        if self.replaced_workouts > 0 {
            let _ = writeln!(
                text,
                "{} placed again at the right time, where an earlier version of your journal had put {} at the wrong time.\n",
                plural(self.replaced_workouts, "workout was", "workouts were"),
                either(self.replaced_workouts, "it", "them"),
            );
        }
        self.facet_section(&mut text);
        self.records_section(&mut text);
        self.names_section(&mut text);
        text
    }

    /// The facet as the owner names it: its title, or its folder's name.
    fn shown_facet(&self) -> Option<&str> {
        self.facet_title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .or(self.facet.facet.as_deref())
    }

    fn facet_section(&self, text: &mut String) {
        let _ = writeln!(text, "## facet\n");
        match (self.shown_facet(), self.facet.basis) {
            (Some(facet), FacetBasis::Chosen) => {
                let _ = writeln!(text, "you chose the {facet} facet for this import.\n");
            }
            (Some(facet), _) => {
                let _ = writeln!(
                    text,
                    "filed under the {facet} facet, the one you chose for Strava imports.\n"
                );
            }
            (None, FacetBasis::ChosenNone) => {
                let _ = writeln!(
                    text,
                    "you chose no facet for this import: {NO_FACET_MEANS}\n"
                );
            }
            (None, _) => {
                let _ = writeln!(
                    text,
                    "filed under no facet, because none was chosen for this import and none is set for Strava imports, or the one set is muted or gone: {NO_FACET_MEANS}\n"
                );
            }
        }
        let Some(facet) = self.shown_facet() else {
            return;
        };
        let filing = &self.filing;
        if filing.filed > 0 {
            let _ = writeln!(
                text,
                "{} now filed under {facet}.{}\n",
                plural(filing.filed, "workout is", "workouts are"),
                match filing.moved {
                    0 => String::new(),
                    1 if filing.filed == 1 => " it moved from another facet.".to_owned(),
                    moved => format!(" {moved} of them moved from another facet."),
                }
            );
        }
        if filing.kept_sensed > 0 {
            let _ = writeln!(
                text,
                "{} left where {}, because {} already listed with your activities.\n",
                plural(filing.kept_sensed, "workout was", "workouts were"),
                either(filing.kept_sensed, "it was", "they were"),
                either(filing.kept_sensed, "it's", "they're"),
            );
        }
        if filing.kept_unreadable > 0 {
            let _ = writeln!(
                text,
                "{} left where {}, because your journal couldn't read what's listed with your activities on {} day. importing again tries once more.\n",
                plural(filing.kept_unreadable, "workout was", "workouts were"),
                either(filing.kept_unreadable, "it was", "they were"),
                either(filing.kept_unreadable, "its", "their"),
            );
        }
    }

    fn records_section(&self, text: &mut String) {
        let _ = writeln!(text, "## activities\n");
        let records = &self.records;
        if records.written > 0 {
            let _ = writeln!(
                text,
                "{} newly listed with your activities on {}.\n",
                plural(records.written, "workout is", "workouts are"),
                either(records.written, "its day", "their days"),
            );
        }
        if records.no_facet > 0 {
            let _ = writeln!(
                text,
                "{} no facet, so {} listed with your activities.\n",
                plural(records.no_facet, "workout has", "workouts have"),
                either(records.no_facet, "it isn't", "they aren't"),
            );
        }
        if !records.refused.is_empty() {
            let _ = writeln!(
                text,
                "not every workout could be listed with your activities this time. importing again tries once more.\n"
            );
        }
        let waiting = self.conversion.waiting + self.conversion.skipped_locked;
        if waiting > 0 {
            let _ = writeln!(
                text,
                "{} from before couldn't be updated yet. your journal tries again the next time it starts.\n",
                plural(waiting, "earlier workout", "earlier workouts"),
            );
        }
        if self.conversion.moves_open > 0 {
            let _ = writeln!(
                text,
                "{} still being moved, and can be edited once that finishes.\n",
                plural(self.conversion.moves_open, "record is", "records are"),
            );
        }
    }

    fn names_section(&self, text: &mut String) {
        let _ = writeln!(text, "## people and gear\n");
        let names = &self.names;
        if names.store_unavailable {
            let _ = writeln!(
                text,
                "your journal couldn't open or save what it noted from your workouts, so not every person or piece of gear in them was read or added this time. importing again tries once more.\n"
            );
        }
        let read = names.calls - names.failed;
        let mut parts = Vec::new();
        if read > 0 {
            parts.push(format!(
                "{} read for the people and gear they name.",
                plural(read, "workout's own words were", "workouts' own words were")
            ));
        }
        if names.cached > 0 {
            parts.push(format!(
                "{} read on an earlier import.",
                plural(names.cached, "workout's were", "workouts' were")
            ));
        }
        if names.failed > 0 {
            parts.push(format!(
                "{} couldn't be read, and importing again tries once more.",
                plural(names.failed, "workout", "workouts")
            ));
        }
        if !parts.is_empty() {
            let _ = writeln!(text, "{}\n", parts.join(" "));
        }
        if names.waiting > 0 {
            let waiting = plural(
                names.waiting,
                "workout's words weren't",
                "workouts' words weren't",
            );
            let when = match names.wait {
                Some(NamesWait::WaitingForWindow) => {
                    "they're read when you import during your processing window"
                }
                Some(NamesWait::NoCondition) => {
                    "your journal waits for a processing window, and none is set up, so they're read once one is and you import again"
                }
                Some(NamesWait::LaneUnavailable) => {
                    "no thinking is set up that may read them, so they're read once it is and you import again"
                }
                Some(NamesWait::SettingsUnreadable) => {
                    "your journal's settings couldn't be read, so they're read when you import again"
                }
                Some(NamesWait::Stopped) | None => "they're read when you import again",
            };
            let _ = writeln!(text, "{waiting} read this time: {when}.\n");
        }
        if names.elsewhere > 0 {
            let _ = writeln!(
                text,
                "{} read, because another import is reading workouts' words right now. they're read when you import again.\n",
                plural(
                    names.elsewhere,
                    "workout's words weren't",
                    "workouts' words weren't"
                ),
            );
        }
        if names.set_aside > 0 {
            let _ = writeln!(
                text,
                "{} read, because {} filed under a facet that's muted or gone. they're read once that facet is on again, or an import files them under another.\n",
                plural(
                    names.set_aside,
                    "workout's words weren't",
                    "workouts' words weren't"
                ),
                either(names.set_aside, "it's", "they're"),
            );
        }
        if names.promotion_unavailable {
            let _ = writeln!(
                text,
                "no people or things could be added this time, because your journal couldn't read your facets. importing again tries once more.\n"
            );
        } else if names.created > 0 {
            let _ = writeln!(
                text,
                "{} added from your workouts.\n",
                plural(
                    names.created,
                    "person or thing was",
                    "people and things were"
                ),
            );
        } else {
            let _ = writeln!(text, "no people or things were added this time.\n");
        }
    }

    /// The counts as `key=value` pairs, for the command's own line.
    #[must_use]
    pub(crate) fn count_keys(&self) -> String {
        format!(
            "facet={} records_written={} records_kept={} records_no_facet={} records_refused={} filed={} moved={} kept_sensed={} kept_unreadable={} names_calls={} names_failed={} names_cached={} names_waiting={} names_elsewhere={} names_unfiled={} names_set_aside={} names_created={}",
            self.facet.facet.as_deref().unwrap_or("none"),
            self.records.written,
            self.records.sensed,
            self.records.no_facet,
            self.records.refused.len(),
            self.filing.filed,
            self.filing.moved,
            self.filing.kept_sensed,
            self.filing.kept_unreadable,
            self.names.calls,
            self.names.failed,
            self.names.cached,
            self.names.waiting,
            self.names.elsewhere,
            self.names.unfiled,
            self.names.set_aside,
            self.names.created,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(facet: Option<&str>, basis: FacetBasis) -> StravaRunSummary {
        StravaRunSummary {
            conversion: Default::default(),
            new_workouts: 2,
            updated_workouts: 1,
            replaced_workouts: 0,
            facet_title: None,
            facet: RunFacet {
                facet: facet.map(str::to_owned),
                basis,
            },
            filing: Filing {
                filed: 2,
                moved: 1,
                kept_sensed: 1,
                kept_unreadable: 1,
                refiled_tiles: Vec::new(),
            },
            records: RecordCounts {
                written: 2,
                sensed: 1,
                no_facet: 0,
                ..RecordCounts::default()
            },
            names: StravaNamesOutcome {
                calls: 3,
                failed: 1,
                cached: 4,
                waiting: 2,
                wait: Some(NamesWait::WaitingForWindow),
                created: 1,
                ..StravaNamesOutcome::default()
            },
        }
    }

    #[test]
    fn the_summary_has_a_section_per_step_and_every_count() {
        let facet_paragraphs = |text: &str| {
            let start = text.find("## facet").unwrap();
            let end = text.find("## activities").unwrap();
            text[start..end]
                .lines()
                .skip(1)
                .filter(|line| !line.trim().is_empty())
                .count()
        };
        let chosen = summary(Some("body"), FacetBasis::Chosen);
        let text = chosen.text();
        assert_eq!(text.matches("\n## ").count(), 3, "{text}");
        // Which facet, how many were filed, and both kinds left where they were.
        assert_eq!(facet_paragraphs(&text), 4, "{text}");
        assert_eq!(
            chosen.count_keys(),
            "facet=body records_written=2 records_kept=1 records_no_facet=0 records_refused=0 filed=2 moved=1 kept_sensed=1 kept_unreadable=1 names_calls=3 names_failed=1 names_cached=4 names_waiting=2 names_elsewhere=0 names_unfiled=0 names_set_aside=0 names_created=1"
        );
        // With no facet, nothing was filed, whatever the counts say.
        let none = summary(None, FacetBasis::NoDefault).text();
        assert_eq!(none.matches("\n## ").count(), 3, "{none}");
        assert_eq!(facet_paragraphs(&none), 1, "{none}");
    }

    /// The people and gear section's paragraphs for `names`.
    fn paragraphs(names: StravaNamesOutcome) -> Vec<String> {
        let mut run = summary(Some("body"), FacetBasis::Chosen);
        run.names = names;
        let text = run.text();
        text[text.find("## people and gear").unwrap()..]
            .split("\n\n")
            .skip(1)
            .filter(|paragraph| !paragraph.trim().is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    }

    /// Words another import kept from being read, and words under a muted
    /// facet, each add one paragraph of their own, saying how many.
    #[test]
    fn words_left_to_another_import_and_words_set_aside_each_say_how_many() {
        let read = StravaNamesOutcome {
            calls: 2,
            ..StravaNamesOutcome::default()
        };
        let plain = paragraphs(read.clone());
        let added = |names: StravaNamesOutcome| {
            let added = paragraphs(names)
                .into_iter()
                .filter(|paragraph| !plain.contains(paragraph))
                .collect::<Vec<_>>();
            assert_eq!(added.len(), 1, "{added:?}");
            added.into_iter().next().unwrap()
        };
        let mut lines = Vec::new();
        for count in [1, 3] {
            let elsewhere = StravaNamesOutcome {
                elsewhere: count,
                ..read.clone()
            };
            let set_aside = StravaNamesOutcome {
                set_aside: count,
                ..read.clone()
            };
            for (names, key) in [
                (elsewhere, "names_elsewhere"),
                (set_aside, "names_set_aside"),
            ] {
                let mut run = summary(Some("body"), FacetBasis::Chosen);
                run.names = names.clone();
                assert!(
                    run.count_keys().contains(&format!("{key}={count}")),
                    "{}",
                    run.count_keys()
                );
                let line = added(names);
                assert!(line.starts_with(&format!("{count} ")), "{line}");
                lines.push(line);
            }
        }
        // Each says something of its own, in one and in many.
        let distinct = lines.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(distinct.len(), lines.len(), "{lines:?}");
    }

    /// Words that couldn't be read and words read but not made into anyone
    /// are told apart, each in its own paragraph.
    #[test]
    fn a_store_that_couldnt_open_and_names_not_added_say_different_things() {
        let read = StravaNamesOutcome {
            calls: 2,
            ..StravaNamesOutcome::default()
        };
        let plain = paragraphs(read.clone());
        let unopened = paragraphs(StravaNamesOutcome {
            store_unavailable: true,
            ..read.clone()
        });
        let not_added = paragraphs(StravaNamesOutcome {
            promotion_unavailable: true,
            ..read
        });
        let store_line = unopened
            .iter()
            .find(|paragraph| !plain.contains(paragraph))
            .unwrap();
        let promotion_line = not_added
            .iter()
            .find(|paragraph| !plain.contains(paragraph))
            .unwrap();
        assert_ne!(store_line, promotion_line);
        // Read, but nothing added: it never says the words weren't read, and
        // it still says they were.
        assert!(!not_added.contains(store_line));
        assert_eq!(not_added[0], plain[0]);
        assert_eq!(not_added.len(), plain.len());
    }

    #[test]
    fn workouts_still_waiting_are_counted_with_no_names_in_the_words() {
        let mut waiting = summary(Some("body"), FacetBasis::Chosen);
        waiting.conversion.waiting = 1;
        waiting.conversion.skipped_locked = 1;
        waiting.conversion.moves_open = 1;
        let text = waiting.text();
        assert!(
            text.contains("2 earlier workouts from before couldn't be updated yet"),
            "{text}"
        );
        assert!(text.contains("1 record is still being moved"), "{text}");
        let none = summary(Some("body"), FacetBasis::Chosen).text();
        assert!(!none.contains("from before"), "{none}");
        assert!(!none.contains("being moved"), "{none}");
    }
}
