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
//! An activity record carries no stream. Its segment keys are directories
//! under the day's streams, so the source is read from disk. Anything that
//! cannot be pinned to exactly one stream (a key that is missing, a key that
//! sits in two streams, or a record whose keys span several streams) gets no
//! label rather than a guessed one. This module performs no writes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_io::{PathOrDay, iter_segments};
use solstone_core_segment::list_stream_bindings;
use solstone_core_sol_link::client_description_store::read_descriptions;

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
    if placed.len() < 2 {
        return vec![None; placed.len()];
    }
    let key_streams = segment_streams(journal, day);
    let shapes = placed
        .iter()
        .map(|entry| Shape::of(entry.record, &key_streams))
        .collect::<Vec<_>>();
    let concurrent = (0..placed.len())
        .map(|index| runs_alongside_another_stream(index, placed, &shapes))
        .collect::<Vec<_>>();
    if concurrent.iter().all(|stream| stream.is_none()) {
        return vec![None; placed.len()];
    }
    let names = SourceNames::read(journal);
    let words = concurrent
        .iter()
        .map(|stream| stream.as_deref().and_then(|stream| names.kind(stream)))
        .collect::<Vec<_>>();
    (0..placed.len())
        .map(|index| {
            let stream = concurrent[index].as_deref()?;
            let kind = words[index]?;
            // Two computers (or two phones) side by side would both read "from
            // your computer". Name the device when its partner shares the word
            // and the two devices carry different names.
            let device = partners(index, placed, &shapes)
                .filter(|&other| words[other] == Some(kind))
                .filter_map(|other| concurrent[other].as_deref())
                .filter(|other_stream| *other_stream != stream)
                .any(|other_stream| names.device(other_stream) != names.device(stream))
                .then(|| names.device(stream))
                .flatten();
            Some(source_phrase(kind, device))
        })
        .collect()
}

/// The owner-facing phrase for a source: `from your watch`, or with the
/// device's own name when two of one kind ran together.
pub fn source_phrase(kind: &str, device: Option<&str>) -> String {
    match device {
        Some(device) => format!("from your {kind} ({device})"),
        None => format!("from your {kind}"),
    }
}

/// Every segment key of the day, with each stream directory that holds it.
fn segment_streams(journal: &Path, day: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut streams: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for segment in iter_segments(journal, PathOrDay::Day(day)).unwrap_or_default() {
        // A segment directly under the day has no stream of its own.
        let stream = segment
            .stream()
            .directory()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        streams
            .entry(segment.key().to_owned())
            .or_default()
            .insert(stream);
    }
    streams
}

/// What the source pass needs from one record.
struct Shape {
    /// Every stream its segments sit in, or `None` when any key is missing or
    /// sits in more than one stream.
    streams: Option<BTreeSet<String>>,
    /// Seconds into the day, `[start, end)`, over all its segments.
    span: Option<(u32, u32)>,
    id: String,
}

impl Shape {
    fn of(record: &Map<String, Value>, key_streams: &BTreeMap<String, BTreeSet<String>>) -> Self {
        let keys = record
            .get("segments")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        let streams = if keys.is_empty() {
            None
        } else {
            keys.iter()
                .map(|key| match key_streams.get(*key) {
                    Some(found) if found.len() == 1 => found.iter().next().cloned(),
                    _ => None,
                })
                .collect::<Option<BTreeSet<_>>>()
                .filter(|streams| streams.iter().all(|stream| !stream.is_empty()))
        };
        let span = keys.iter().filter_map(|key| segment_span(key)).reduce(
            |(start, end), (next_start, next_end)| (start.min(next_start), end.max(next_end)),
        );
        Self {
            streams,
            span,
            id: record
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }
    }

    fn single_stream(&self) -> Option<&str> {
        let streams = self.streams.as_ref()?;
        (streams.len() == 1).then(|| streams.iter().next().map(String::as_str))?
    }
}

/// `HHMMSS_<seconds>` as `[start, end)` in seconds into the day.
fn segment_span(key: &str) -> Option<(u32, u32)> {
    let (clock, length) = key.split_once('_')?;
    if clock.len() != 6 || !clock.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let length = length.parse::<u32>().ok()?;
    let hours = clock[0..2].parse::<u32>().ok()?;
    let minutes = clock[2..4].parse::<u32>().ok()?;
    let seconds = clock[4..6].parse::<u32>().ok()?;
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let start = hours * 3600 + minutes * 60 + seconds;
    Some((start, start + length.max(1)))
}

/// Other records in the same facet whose time overlaps this one. The copies of
/// one activity written under several facets share an id and are not partners.
fn partners<'a>(
    index: usize,
    placed: &'a [Placed<'_>],
    shapes: &'a [Shape],
) -> impl Iterator<Item = usize> + 'a {
    let own = &shapes[index];
    (0..placed.len()).filter(move |&other| {
        if other == index || placed[other].facet != placed[index].facet {
            return false;
        }
        if !own.id.is_empty() && shapes[other].id == own.id {
            return false;
        }
        match (own.span, shapes[other].span) {
            (Some((start, end)), Some((other_start, other_end))) => {
                start < other_end && other_start < end
            }
            _ => false,
        }
    })
}

/// The record's one stream, when a same-facet record that overlaps it in time
/// came from somewhere else. Overlap inside one stream (a replayed fragment of
/// a live record) is not a pair, and a partner whose streams are unknown
/// cannot show that it came from elsewhere.
fn runs_alongside_another_stream(
    index: usize,
    placed: &[Placed<'_>],
    shapes: &[Shape],
) -> Option<String> {
    let stream = shapes[index].single_stream()?;
    partners(index, placed, shapes)
        .any(|other| {
            shapes[other]
                .streams
                .as_ref()
                .is_some_and(|streams| streams.iter().any(|candidate| candidate != stream))
        })
        .then(|| stream.to_owned())
}

/// What the journal knows about each stream: the paired client and source it
/// is bound to, and what that client says it is.
struct SourceNames {
    bindings: BTreeMap<String, (String, String)>,
    devices: BTreeMap<String, (Option<String>, Option<String>)>,
}

impl SourceNames {
    fn read(journal: &Path) -> Self {
        let bindings = list_stream_bindings(journal)
            .unwrap_or_default()
            .into_iter()
            .map(|binding| (binding.name, (binding.cid, binding.source)))
            .collect();
        let devices = read_descriptions(journal)
            .unwrap_or_default()
            .into_iter()
            .map(|(cid, description)| {
                let reported = description.reported.as_ref();
                let name = description
                    .owner_label
                    .clone()
                    .or_else(|| reported.and_then(|reported| reported.name.clone()))
                    .filter(|name| !name.trim().is_empty());
                let device_type = reported.and_then(|reported| reported.device_type.clone());
                (cid, (name, device_type))
            })
            .collect();
        Self { bindings, devices }
    }

    /// The plain word for what captured a stream, or `None` when the journal
    /// cannot say. A stream directory name is never shown.
    fn kind(&self, stream: &str) -> Option<&'static str> {
        let Some((cid, source)) = self.bindings.get(stream) else {
            return legacy_kind(stream);
        };
        match source.as_str() {
            "watch-audio" => Some("watch"),
            "tmux" => Some("terminal"),
            "mobile-segment" => Some("phone"),
            source if source.starts_with("browser") => Some("browser"),
            // The device's own capture: say what the device says it is.
            "" | "audio" | "screen" | "camera" | "location" => {
                match self.devices.get(cid)?.1.as_deref()? {
                    "desktop" | "laptop" => Some("computer"),
                    "terminal" => Some("terminal"),
                    "phone" => Some("phone"),
                    "tablet" => Some("tablet"),
                    _ => None,
                }
            }
            // An accessory relayed through another device (a pendant through
            // the phone) is not the phone; with no word for it, say nothing.
            _ => None,
        }
    }

    /// The device's own name for a stream's client, when it has one.
    fn device(&self, stream: &str) -> Option<&str> {
        let (cid, _) = self.bindings.get(stream)?;
        self.devices.get(cid)?.0.as_deref()
    }
}

/// Streams written before paired bindings are named `<host>.<qualifier>`.
fn legacy_kind(stream: &str) -> Option<&'static str> {
    let (host, qualifier) = stream.rsplit_once('.')?;
    if host.is_empty() || host == "import" {
        return None;
    }
    match qualifier {
        "tmux" => Some("terminal"),
        "watch" => Some("watch"),
        "mobile" | "phone" => Some("phone"),
        "browser" => Some("browser"),
        _ => None,
    }
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

    #[test]
    fn legacy_stream_names_read_by_their_qualifier_only() {
        assert_eq!(legacy_kind("fedora.tmux"), Some("terminal"));
        assert_eq!(legacy_kind("iphone.watch"), Some("watch"));
        assert_eq!(legacy_kind("suze.browser"), Some("browser"));
        assert_eq!(legacy_kind("fedora"), None);
        assert_eq!(legacy_kind("import.plaud"), None);
        assert_eq!(legacy_kind("rokid-rg-glasses.glasses"), None);
    }
}
