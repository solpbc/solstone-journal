// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Where an activity came from, for the rows that need to say so.
//!
//! Each capture stream writes its own activities, so an owner who captures one
//! stretch on two streams (the desktop and the terminal, or the watch and the
//! desktop) gets two activities in one facet. Both are kept and both are shown.
//! A row that ran alongside another row in its facet from a different stream
//! names its source, and every other row stays as it was.
//!
//! A record may name its stream. Which stream that is, the span of its
//! segments, and whether two rows ran alongside each other are decided by
//! `solstone_core_facets::place_activity_records`. This module turns that
//! stream into the phrase an owner reads. A row that cannot be pinned to one
//! named stream gets no label. This module performs no writes.

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_window_catalog::{ENGLISH, strings, text};

use crate::provenance::{DeviceWord, LabelTableStatus, load_stream_labels, resolve_stream_label};

/// One activity record as the source pass sees it: the facet it was read from
/// and the record itself.
pub struct Placed<'a> {
    pub facet: &'a str,
    pub record: &'a Map<String, Value>,
}

/// The source phrase for each placed record, in input order. `None` for every
/// record that does not run alongside a same-facet record from another stream,
/// and for every record whose own source is unknown, ambiguous or mixed.
pub fn concurrent_source_labels(
    journal: &Path,
    day: &str,
    placed: &[Placed<'_>],
) -> Vec<Option<String>> {
    concurrent_source_labels_in(journal, day, placed, ENGLISH)
}

pub(crate) fn concurrent_source_labels_in(
    journal: &Path,
    day: &str,
    placed: &[Placed<'_>],
    locale: &str,
) -> Vec<Option<String>> {
    if placed.len() < 2 {
        return vec![None; placed.len()];
    }
    let placed_records = placed
        .iter()
        .map(|entry| solstone_core_facets::PlacedRecord {
            facet: entry.facet,
            record: entry.record,
        })
        .collect::<Vec<_>>();
    let courses = solstone_core_facets::place_activity_records(journal, day, &placed_records);
    let concurrent = courses
        .iter()
        .map(|course| {
            if course.concurrent {
                course.stream.clone()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if concurrent.iter().all(|stream| stream.is_none()) {
        return vec![None; placed.len()];
    }
    let table = load_stream_labels(journal);
    if table.status == LabelTableStatus::Failed {
        return vec![None; placed.len()];
    }
    let parts = concurrent
        .iter()
        .map(|stream| stream.as_deref().map(|s| resolve_stream_label(&table, s)))
        .collect::<Vec<_>>();
    let words = parts
        .iter()
        .map(|part| part.as_ref().and_then(|p| p.device_word()))
        .collect::<Vec<_>>();

    (0..placed.len())
        .map(|index| {
            let stream = concurrent[index].as_deref()?;
            let device_word = words[index]?;
            let part = parts[index].as_ref()?;
            let identity = part.identity();
            let this_id = identity;
            let show_parenthetical = if let Some(this_id_str) = this_id {
                courses[index]
                    .partners
                    .iter()
                    .copied()
                    .filter(|&other| words[other] == Some(device_word))
                    .filter_map(|other| {
                        let other_stream = concurrent[other].as_deref()?;
                        (other_stream != stream).then_some(other)
                    })
                    .any(|other| {
                        if let Some(other_id_str) = parts[other].as_ref().and_then(|p| p.identity())
                        {
                            this_id_str != other_id_str
                        } else {
                            false
                        }
                    })
            } else {
                false
            };
            let device = if show_parenthetical { this_id } else { None };
            Some(source_phrase_in(
                locale,
                localized_device_word(locale, device_word).as_str(),
                device,
            ))
        })
        .collect()
}

/// The owner-facing phrase for a source: `from your watch`, or with the
/// device's own name when two of one kind ran together.
pub fn source_phrase(kind: &str, device: Option<&str>) -> String {
    source_phrase_in(ENGLISH, kind, device)
}

fn source_phrase_in(locale: &str, kind: &str, device: Option<&str>) -> String {
    match device {
        Some(device) => strings(
            locale,
            "journal.window.home.activity.source.phrase.named",
            [("kind", kind), ("device", device)],
        ),
        None => strings(
            locale,
            "journal.window.home.activity.source.phrase",
            [("kind", kind)],
        ),
    }
}

fn localized_device_word(locale: &str, word: DeviceWord) -> String {
    let id = match word {
        DeviceWord::Watch => "journal.window.sources.source.or.date.unit.label.watch",
        DeviceWord::Terminal => "journal.window.sources.source.or.date.unit.label.terminal",
        DeviceWord::Phone => "journal.window.home.activity.source.device.phone",
        DeviceWord::Tablet => "journal.window.home.activity.source.device.tablet",
        DeviceWord::Computer => "journal.window.home.activity.source.device.computer",
        DeviceWord::Mac => "journal.window.home.activity.source.device.mac",
        DeviceWord::Iphone => "journal.window.home.activity.source.device.iphone",
        DeviceWord::Ipad => "journal.window.home.activity.source.device.ipad",
        DeviceWord::Browser => "journal.window.home.activity.source.device.browser",
    };
    text(locale, id)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    const DAY: &str = "20261003";
    const DESKTOP_CID: &str =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";
    const TERMINAL_CID: &str =
        "sha256:2222222222222222222222222222222222222222222222222222222222222222";
    const PHONE_CID: &str =
        "sha256:3333333333333333333333333333333333333333333333333333333333333333";
    const LAPTOP_CID: &str =
        "sha256:4444444444444444444444444444444444444444444444444444444444444444";

    fn segment(root: &Path, stream: &str, key: &str) {
        fs::create_dir_all(root.join("chronicle").join(DAY).join(stream).join(key)).unwrap();
    }

    fn bind(root: &Path, stream: &str, cid: &str, source: &str) {
        let path = root.join("streams").join(format!("{stream}.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            json!({"name": stream, "kind": "observer", "host": null, "platform": null,
                   "created_at": 1, "last_day": DAY, "last_segment": null, "seq": 1,
                   "cid": cid, "source": source})
            .to_string(),
        )
        .unwrap();
    }

    fn describe(root: &Path, devices: &[(&str, &str, &str)]) {
        let map = devices
            .iter()
            .map(|(cid, name, device_type)| {
                (
                    (*cid).to_owned(),
                    json!({"protocol_version": 1, "revision": 1, "owner_label": null,
                           "updated_at": null,
                           "reported": {"name": name, "platform": "linux",
                                        "device_type": device_type, "app_id": null,
                                        "app_version": null}}),
                )
            })
            .collect::<Map<_, _>>();
        fs::create_dir_all(root.join("link")).unwrap();
        fs::write(
            root.join("link/client-descriptions.json"),
            Value::Object(map).to_string(),
        )
        .unwrap();
    }

    fn record(id: &str, segments: &[&str]) -> Map<String, Value> {
        json!({"id": id, "segments": segments})
            .as_object()
            .unwrap()
            .clone()
    }

    fn labels(root: &Path, rows: &[(&str, Map<String, Value>)]) -> Vec<Option<String>> {
        let placed = rows
            .iter()
            .map(|(facet, record)| Placed { facet, record })
            .collect::<Vec<_>>();
        concurrent_source_labels(root, DAY, &placed)
    }

    /// The desktop and the terminal, one facet, one stretch, two streams.
    fn desktop_and_terminal(root: &Path) {
        for key in ["100000_300", "100500_300"] {
            segment(root, "device_2", key);
        }
        for key in ["100100_300", "100600_300"] {
            segment(root, "extro_tmux", key);
        }
        bind(root, "device_2", DESKTOP_CID, "");
        bind(root, "extro_tmux", TERMINAL_CID, "tmux");
        describe(
            root,
            &[
                (DESKTOP_CID, "fedora", "desktop"),
                (TERMINAL_CID, "fedora", "terminal"),
            ],
        );
    }

    #[test]
    fn a_concurrent_pair_from_two_streams_names_each_source() {
        let root = TempDir::new().unwrap();
        desktop_and_terminal(root.path());
        let got = labels(
            root.path(),
            &[
                (
                    "work",
                    record("email_100000_300", &["100000_300", "100500_300"]),
                ),
                (
                    "work",
                    record("terminal_100100_300", &["100100_300", "100600_300"]),
                ),
            ],
        );
        assert_eq!(
            got,
            vec![
                Some(source_phrase("computer", None)),
                Some(source_phrase("terminal", None))
            ]
        );
    }

    #[test]
    fn a_pair_in_two_facets_or_two_times_is_not_concurrent() {
        let root = TempDir::new().unwrap();
        desktop_and_terminal(root.path());
        segment(root.path(), "extro_tmux", "120000_300");
        let got = labels(
            root.path(),
            &[
                (
                    "work",
                    record("email_100000_300", &["100000_300", "100500_300"]),
                ),
                (
                    "home",
                    record("terminal_100100_300", &["100100_300", "100600_300"]),
                ),
                ("work", record("terminal_120000_300", &["120000_300"])),
            ],
        );
        assert_eq!(got, vec![None, None, None]);
    }

    #[test]
    fn a_key_held_by_two_streams_gives_its_record_no_source() {
        let root = TempDir::new().unwrap();
        desktop_and_terminal(root.path());
        // The same key under both streams: the record cannot say which it was.
        segment(root.path(), "extro_tmux", "100500_300");
        let got = labels(
            root.path(),
            &[
                (
                    "work",
                    record("email_100000_300", &["100000_300", "100500_300"]),
                ),
                (
                    "work",
                    record("terminal_100100_300", &["100100_300", "100600_300"]),
                ),
            ],
        );
        // The ambiguous record stays unlabelled, and its partner cannot be
        // shown to have come from elsewhere, so it stays unlabelled too.
        assert_eq!(got, vec![None, None]);
    }

    #[test]
    fn a_record_spanning_two_streams_claims_no_single_source() {
        let root = TempDir::new().unwrap();
        desktop_and_terminal(root.path());
        segment(root.path(), "device_watch-audio", "100200_300");
        bind(root.path(), "device_watch-audio", PHONE_CID, "watch-audio");
        let got = labels(
            root.path(),
            &[
                // Written by the old shared machine: desktop and terminal keys.
                (
                    "work",
                    record("coding_100000_300", &["100000_300", "100100_300"]),
                ),
                ("work", record("meeting_100200_300", &["100200_300"])),
            ],
        );
        assert_eq!(got, vec![None, Some(source_phrase("watch", None))]);
    }

    #[test]
    fn a_fragment_overlapping_its_live_record_in_one_stream_is_not_a_pair() {
        let root = TempDir::new().unwrap();
        desktop_and_terminal(root.path());
        let got = labels(
            root.path(),
            &[
                (
                    "work",
                    record("email_100000_300", &["100000_300", "100500_300"]),
                ),
                // A replayed subset of the same stream minted its own record.
                ("work", record("email_100500_300", &["100500_300"])),
            ],
        );
        assert_eq!(got, vec![None, None]);
    }

    #[test]
    fn copies_of_one_activity_in_two_facets_are_not_partners() {
        let root = TempDir::new().unwrap();
        desktop_and_terminal(root.path());
        let got = labels(
            root.path(),
            &[
                ("work", record("email_100000_300", &["100000_300"])),
                ("work", record("email_100000_300", &["100000_300"])),
            ],
        );
        assert_eq!(got, vec![None, None]);
    }

    #[test]
    fn two_of_one_kind_are_told_apart_by_the_devices_own_name() {
        let root = TempDir::new().unwrap();
        segment(root.path(), "device_2", "100000_300");
        segment(root.path(), "device_3", "100100_300");
        bind(root.path(), "device_2", DESKTOP_CID, "");
        bind(root.path(), "device_3", LAPTOP_CID, "");
        describe(
            root.path(),
            &[
                (DESKTOP_CID, "studio", "desktop"),
                (LAPTOP_CID, "travel", "desktop"),
            ],
        );
        let got = labels(
            root.path(),
            &[
                ("work", record("email_100000_300", &["100000_300"])),
                ("work", record("email_100100_300", &["100100_300"])),
            ],
        );
        assert_eq!(
            got,
            vec![
                Some(source_phrase("computer", Some("studio"))),
                Some(source_phrase("computer", Some("travel")))
            ]
        );
    }

    #[test]
    fn a_stream_the_journal_cannot_name_shows_no_source() {
        let root = TempDir::new().unwrap();
        desktop_and_terminal(root.path());
        // A pendant relayed by the phone: the phone did not hear it.
        segment(root.path(), "device_omi-audio", "100200_300");
        bind(root.path(), "device_omi-audio", PHONE_CID, "omi-audio");
        let got = labels(
            root.path(),
            &[
                ("work", record("email_100000_300", &["100000_300"])),
                ("work", record("meeting_100200_300", &["100200_300"])),
            ],
        );
        assert_eq!(got, vec![Some(source_phrase("computer", None)), None]);
    }
}
