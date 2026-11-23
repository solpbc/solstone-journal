// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a Strava import says it did, in `imports/<id>/summary.md`.
//!
//! Every sentence the summary can hold is written here, so the words can be
//! read and changed in one place. It holds counts and the facet's name, never
//! a workout's words or a name read from them.

#[cfg(test)]
use std::collections::BTreeMap;

use solstone_core_format::stored_markdown::{
    Document,
    body_summary::{Copy, FacetsSpec, StravaWaitReason},
};
use solstone_core_import_sources::apple_health::UNNAMED_FILING;

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
    /// Workouts already listed that the owner's facet moved, and from where.
    pub listed_moved: Option<(usize, Vec<String>)>,
    /// `(shown, would move)` when more would have moved than the owner saw,
    /// and so none did.
    pub listed_over: Option<(usize, usize)>,
}

impl StravaRunSummary {
    /// The summary's text, in the owner's words.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn text(&self) -> String {
        self.document().into_text()
    }

    /// The summary as a stored Markdown document carrying verified display spans.
    #[must_use]
    pub(crate) fn document(&self) -> Document {
        let mut doc = Document::default();
        doc.push_body_summary(Copy::Heading {
            source: "Strava".to_owned(),
        });
        doc.push_str("\n\n");
        doc.push_body_summary(Copy::StravaOpening {
            new_workouts: self.new_workouts,
            updated_workouts: self.updated_workouts,
        });
        doc.push_str("\n\n");
        if self.replaced_workouts > 0 {
            doc.push_body_summary(Copy::StravaReplaced {
                count: self.replaced_workouts,
            });
            doc.push_str("\n\n");
        }
        self.facet_section(&mut doc);
        self.records_section(&mut doc);
        self.names_section(&mut doc);
        doc
    }

    /// The facet as the owner names it: its title, or its folder's name.
    fn shown_facet(&self) -> Option<&str> {
        self.facet_title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .or(self.facet.facet.as_deref())
    }

    fn facet_section(&self, doc: &mut Document) {
        doc.push_body_summary(Copy::StravaSectionFacet);
        doc.push_str("\n\n");
        match (self.shown_facet(), self.facet.basis) {
            (Some(facet), FacetBasis::Chosen) => {
                doc.push_body_summary(Copy::StravaFacetChosen {
                    facet: facet.to_owned(),
                });
                doc.push_str("\n\n");
                if let Some((count, from)) = &self.listed_moved {
                    doc.push_body_summary(Copy::StravaListedMoved {
                        count: *count as u64,
                        facets: FacetsSpec::from_titles(from, UNNAMED_FILING),
                    });
                    doc.push_str("\n\n");
                }
                if let Some((shown, count)) = self.listed_over {
                    doc.push_body_summary(Copy::StravaListedOver {
                        shown,
                        count: count as u64,
                    });
                    doc.push_str("\n\n");
                }
            }
            (Some(facet), _) => {
                doc.push_body_summary(Copy::StravaFacetDefault {
                    facet: facet.to_owned(),
                });
                doc.push_str("\n\n");
            }
            (None, FacetBasis::ChosenNone) => {
                doc.push_body_summary(Copy::StravaFacetChosenNone);
                doc.push_str("\n\n");
            }
            (None, _) => {
                doc.push_body_summary(Copy::StravaFacetUnset);
                doc.push_str("\n\n");
            }
        }
        let filing = &self.filing;
        if let Some(facet) = self.shown_facet()
            && filing.filed > 0
        {
            doc.push_body_summary(Copy::StravaFiled {
                count: filing.filed,
                facet: facet.to_owned(),
                moved: filing.moved,
            });
            doc.push_str("\n\n");
        }
        for (earlier, count) in &filing.kept_filed {
            doc.push_body_summary(Copy::StravaKeptFiled {
                count: *count,
                facet: earlier.clone(),
            });
            doc.push_str("\n\n");
        }
        if filing.kept_sensed > 0 {
            doc.push_body_summary(Copy::StravaKeptSensed {
                count: filing.kept_sensed,
            });
            doc.push_str("\n\n");
        }
        if filing.kept_unreadable > 0 {
            doc.push_body_summary(Copy::StravaKeptUnreadable {
                count: filing.kept_unreadable,
            });
            doc.push_str("\n\n");
        }
    }

    fn records_section(&self, doc: &mut Document) {
        doc.push_body_summary(Copy::StravaSectionActivities);
        doc.push_str("\n\n");
        let records = &self.records;
        if records.written > 0 {
            doc.push_body_summary(Copy::StravaRecordsWritten {
                count: records.written,
            });
            doc.push_str("\n\n");
        }
        if records.no_facet > 0 {
            doc.push_body_summary(Copy::StravaRecordsNoFacet {
                count: records.no_facet,
            });
            doc.push_str("\n\n");
        }
        if !records.refused.is_empty() {
            doc.push_body_summary(Copy::StravaRecordsRefused);
            doc.push_str("\n\n");
        }
        if self.conversion.waiting + self.conversion.skipped_locked > 0 {
            doc.push_body_summary(Copy::StravaConversionWaiting);
            doc.push_str("\n\n");
        }
        if self.conversion.no_workout > 0 {
            doc.push_body_summary(Copy::StravaConversionNoWorkout {
                count: self.conversion.no_workout,
            });
            doc.push_str("\n\n");
        }
        if self.conversion.moves_open > 0 {
            doc.push_body_summary(Copy::StravaConversionMovesOpen {
                count: self.conversion.moves_open,
            });
            doc.push_str("\n\n");
        }
    }

    fn names_section(&self, doc: &mut Document) {
        doc.push_body_summary(Copy::StravaSectionPeopleAndGear);
        doc.push_str("\n\n");
        let names = &self.names;
        if names.store_unavailable {
            doc.push_body_summary(Copy::StravaNamesStoreUnavailable);
            doc.push_str("\n\n");
        }
        let read = names.calls - names.failed;
        let mut any_part = false;
        if read > 0 {
            doc.push_body_summary(Copy::StravaNamesRead { count: read });
            any_part = true;
        }
        if names.cached > 0 {
            if any_part {
                doc.push_str(" ");
            }
            doc.push_body_summary(Copy::StravaNamesCached {
                count: names.cached,
            });
            any_part = true;
        }
        if names.failed > 0 {
            if any_part {
                doc.push_str(" ");
            }
            doc.push_body_summary(Copy::StravaNamesFailed {
                count: names.failed,
            });
            any_part = true;
        }
        if any_part {
            doc.push_str("\n\n");
        }
        if names.waiting > 0 {
            let reason = match names.wait {
                Some(NamesWait::WaitingForWindow) => StravaWaitReason::WaitingForWindow,
                Some(NamesWait::NoCondition) => StravaWaitReason::NoCondition,
                Some(NamesWait::LaneUnavailable) => StravaWaitReason::LaneUnavailable,
                Some(NamesWait::SettingsUnreadable) => StravaWaitReason::SettingsUnreadable,
                Some(NamesWait::Stopped) | None => StravaWaitReason::Stopped,
            };
            doc.push_body_summary(Copy::StravaNamesWaiting {
                count: names.waiting,
                reason,
            });
            doc.push_str("\n\n");
        }
        if names.elsewhere > 0 {
            doc.push_body_summary(Copy::StravaNamesElsewhere {
                count: names.elsewhere,
            });
            doc.push_str("\n\n");
        }
        if names.set_aside > 0 {
            doc.push_body_summary(Copy::StravaNamesSetAside {
                count: names.set_aside,
            });
            doc.push_str("\n\n");
        }
        if names.promotion_unavailable {
            doc.push_body_summary(Copy::StravaNamesPromotionUnavailable);
            doc.push_str("\n\n");
        } else if names.created > 0 {
            doc.push_body_summary(Copy::StravaNamesCreated {
                count: names.created,
            });
            doc.push_str("\n\n");
        } else {
            doc.push_body_summary(Copy::StravaNamesNoneCreated);
            doc.push_str("\n\n");
        }
    }

    /// The counts as `key=value` pairs, for the command's own line.
    #[must_use]
    pub(crate) fn count_keys(&self) -> String {
        format!(
            "facet={} records_written={} records_kept={} records_no_facet={} records_refused={} filed={} moved={} kept_sensed={} kept_filed={} kept_unreadable={} names_calls={} names_failed={} names_cached={} names_waiting={} names_elsewhere={} names_unfiled={} names_set_aside={} names_created={}",
            self.facet.facet.as_deref().unwrap_or("none"),
            self.records.written,
            self.records.sensed,
            self.records.no_facet,
            self.records.refused.len(),
            self.filing.filed,
            self.filing.moved,
            self.filing.kept_sensed,
            self.filing.kept_filed.values().sum::<u64>(),
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
            listed_moved: None,
            listed_over: None,
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
                kept_filed: BTreeMap::new(),
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
            "facet=body records_written=2 records_kept=1 records_no_facet=0 records_refused=0 filed=2 moved=1 kept_sensed=1 kept_filed=0 kept_unreadable=1 names_calls=3 names_failed=1 names_cached=4 names_waiting=2 names_elsewhere=0 names_unfiled=0 names_set_aside=0 names_created=1"
        );
        // With no facet, new workouts are unfiled and earlier filings are still disclosed.
        let none = summary(None, FacetBasis::NoDefault).text();
        assert_eq!(none.matches("\n## ").count(), 3, "{none}");
        assert_eq!(facet_paragraphs(&none), 3, "{none}");
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
            text.contains("some earlier workouts couldn't be updated yet"),
            "{text}"
        );
        assert!(
            text.contains("1 record couldn't finish moving yet"),
            "{text}"
        );
        let none = summary(Some("body"), FacetBasis::Chosen).text();
        assert!(!none.contains("earlier workouts"), "{none}");
        assert!(!none.contains("finish moving"), "{none}");
    }
}
