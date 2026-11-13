// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Original recording files in the trash: what a release took from a segment
//! that stays, one item per segment.
//!
//! The door releases originals as it always has: it proves each file, stages
//! or unlinks it, and writes `original_deleted`. The trash keeps a second name
//! for each file first ([`link_before`]) and, once the door has run, keeps what
//! left ([`keep_after`]); erasing the item unlinks that last name, which is
//! when the space comes back. Its references:
//!
//! - **The segment's events**: `original_deleted` names a released file, as
//!   every reader already reads it, and `original_recovered` puts it back.
//!   Voice-only's owed read stays sticky on `original_deleted`, so an original
//!   recovered from the trash is never owed to voice-only again.
//! - **The day's stream marker**, which queues the day's follow-up.
//!
//! Its hold is the door's own for the segment. Recovering while the recording
//! itself waits in the trash is refused: the recording comes back first.

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, DurableEvent, append_durable_event};
use solstone_core_journal_io::paths::contained_path;
use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_trash::{Change, Hold, Item, Kind};

pub use solstone_core_trash::{Linked, erase_after_days};

use crate::door::SegmentHold;
use crate::original_deletion::{
    ORIGINAL_RECOVERED_EVENT, RawReleaseClass, recorded_original_deletions,
};
use crate::receipt::Target;

/// What the item keeps of the release, for its references.
const CLASS: &str = "class";

/// Refused: the recording these originals belong to is in the trash.
pub const RECORDING_IN_TRASH: &str = "recording_in_trash";

/// Refused: the recording these originals belong to is no longer in the
/// journal (erased, or removed for good).
pub const SEGMENT_GONE: &str = "segment_gone";

/// Where an originals item's segment stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentState {
    /// In the journal as itself.
    Live,
    /// Waiting in the trash as its own item.
    Trashed,
    /// Erased, or removed for good.
    Gone,
}

impl SegmentState {
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Trashed => "trashed",
            Self::Gone => "gone",
        }
    }
}

/// Where `item`'s segment stands now.
#[must_use]
pub fn segment_state(journal: &Path, item: &Item) -> SegmentState {
    let Some(segment) = item.paths.first().and_then(|path| segment_of(path)) else {
        return SegmentState::Gone;
    };
    if segment_live(journal, segment) {
        return SegmentState::Live;
    }
    match solstone_core_trash::holders(journal, segment, &item.id) {
        Ok(holders) if !holders.is_empty() => SegmentState::Trashed,
        // A trash that can't be read is never taken for gone.
        Err(_) => SegmentState::Trashed,
        Ok(_) => SegmentState::Gone,
    }
}

/// What was released, by the transcripts page's words: `audio`, `screen`.
#[must_use]
pub fn modalities(item: &Item) -> Vec<&'static str> {
    let mut found = Vec::new();
    for path in &item.paths {
        let kind = path
            .rsplit('.')
            .next()
            .and_then(solstone_core_processing_record::media_kind)
            .map(|kind| match kind {
                solstone_core_processing_record::MediaKind::Audio => "audio",
                _ => "screen",
            });
        if let Some(kind) = kind
            && !found.contains(&kind)
        {
            found.push(kind);
        }
    }
    found.sort_unstable();
    found
}

/// Whether an originals item, waiting or on its way in, holds the original
/// `name` of the segment at `segment_rel`.
#[must_use]
pub fn in_trash(journal: &Path, segment_rel: &str, name: &str) -> bool {
    let path = format!("{segment_rel}/{name}");
    originals_holder(journal, &path).is_some()
}

/// The trash's kind for original files.
pub struct Originals;

/// The originals of one segment a release is about to take.
#[derive(Clone, Debug)]
pub struct Release {
    pub target: Target,
    /// Journal-relative paths of the files, inside the segment.
    pub paths: Vec<String>,
}

/// Group journal-relative original paths by their segment.
#[must_use]
pub fn releases(paths: impl IntoIterator<Item = (Target, String)>) -> Vec<Release> {
    let mut grouped: Vec<Release> = Vec::new();
    for (target, path) in paths {
        match grouped.iter_mut().find(|release| release.target == target) {
            Some(release) => {
                if !release.paths.contains(&path) {
                    release.paths.push(path);
                }
            }
            None => grouped.push(Release {
                target,
                paths: vec![path],
            }),
        }
    }
    grouped
}

/// The originals a door release of `proven` would take, by segment.
#[must_use]
pub fn of_proven(proven: &[crate::eligibility::ProvenRaw]) -> Vec<Release> {
    releases(proven.iter().map(|item| {
        (
            Target {
                day: item.day().to_owned(),
                stream: item.stream().to_owned(),
                dir: item.dir().to_owned(),
            },
            item.rel(),
        )
    }))
}

/// The originals the policy marks among `marks` name, by segment. An offload
/// mark is media offload, which keeps a copy in the backup, and isn't trashed,
/// so a name an offload mark in the same selection also holds is left out.
#[must_use]
pub fn of_policy_marks<'m>(
    marks: impl IntoIterator<Item = &'m crate::marks::Mark>,
) -> Vec<Release> {
    let marks: Vec<&crate::marks::Mark> = marks.into_iter().collect();
    let offload = |target: &Target, name: &str| {
        marks.iter().any(|mark| {
            (mark.offload.is_some() || mark.class == crate::marks::RemovalClass::OffloadRawRelease)
                && mark.target == *target
                && mark.proposal.names.iter().any(|held| held == name)
        })
    };
    releases(
        marks
            .iter()
            .filter(|mark| {
                mark.class == crate::marks::RemovalClass::PolicyRawRelease && mark.offload.is_none()
            })
            .flat_map(|mark| {
                mark.proposal
                    .names
                    .iter()
                    .filter(|name| !offload(&mark.target, name))
                    .map(|name| {
                        (
                            mark.target.clone(),
                            crate::layout::content_rel(
                                &mark.target.day,
                                &mark.target.stream,
                                &mark.target.dir,
                                name,
                            ),
                        )
                    })
            }),
    )
}

/// The originals the policy marks among checked `marks` name, by segment.
#[must_use]
pub fn of_preflight(marks: &crate::marks::PreflightMarks) -> Vec<Release> {
    of_policy_marks(marks.as_slice().iter().map(|(_, mark)| mark))
}

/// Keep a second name for every original `releases` names, one item per
/// segment, before the door releases them. A file already gone is left out:
/// the door can't release it either. A file that can't be kept refuses the
/// whole release, and the caller then releases nothing.
pub fn link_before(
    journal: &Path,
    releases: &[Release],
    class: RawReleaseClass,
    action: Action,
    surface: Surface,
) -> Result<Vec<Linked>, String> {
    // One owner act: the history shows it as one line however many
    // recordings it took originals from.
    let operation = solstone_core_removals::OperationId::fresh(chrono::Utc::now());
    let mut linked = Vec::new();
    for release in releases {
        let paths: Vec<String> = release
            .paths
            .iter()
            .filter(|path| Originals.present(journal, path))
            .cloned()
            .collect();
        if paths.is_empty() {
            continue;
        }
        let files = paths
            .iter()
            .filter_map(|path| path.rsplit_once('/').map(|(_, name)| name.to_owned()))
            .collect();
        let unit = Unit::Originals {
            day: release.target.day.clone(),
            stream: release.target.stream.clone(),
            segment: release.target.dir.clone(),
            files,
        };
        match solstone_core_trash::detach_linked(
            journal,
            &Originals,
            unit,
            action,
            surface,
            paths,
            &[(CLASS, class.tag().as_bytes())],
            Some(operation.as_str().to_owned()),
        ) {
            Ok(item) => linked.push(item),
            Err(error) => {
                // Nothing has been released yet: let go of what was kept.
                let _ = keep_after(journal, linked);
                return Err(error.to_string());
            }
        }
    }
    Ok(linked)
}

/// Once the door has run: keep what it released, drop what it left. Returns
/// what couldn't be settled now, which the trash's sweep finishes.
pub fn keep_after(journal: &Path, linked: Vec<Linked>) -> Vec<String> {
    let mut failed = Vec::new();
    for item in linked {
        let id = item.id().to_owned();
        if let Err(error) = solstone_core_trash::finish_linked(journal, &Originals, item) {
            log::warn!("trash item {id}: originals not settled yet: {error}");
            failed.push(format!("{id}: {error}"));
        }
    }
    failed
}

/// Split a release report's `outcome.targets[].removed` into what waits in
/// the trash, `trashed`, and what is gone, `removed`; and add `trash` with
/// `erase_at`, the earliest erase of the items holding them, for the owner's
/// result. Paths are journal-relative.
pub fn annotate_report(journal: &Path, report: &mut Value) {
    let Some(targets) = report
        .pointer_mut("/outcome/targets")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut earliest: Option<String> = None;
    let mut count = 0_usize;
    for target in targets {
        let Some(fields) = target.as_object_mut() else {
            continue;
        };
        let Some(removed) = fields.get("removed").and_then(Value::as_array).cloned() else {
            continue;
        };
        let (mut trashed, mut gone) = (Vec::new(), Vec::new());
        for entry in removed {
            let holder = entry.as_str().and_then(|path| originals_holder(journal, path));
            match holder {
                Some(item) => {
                    if earliest.as_ref().is_none_or(|at| item.erase_at < *at) {
                        earliest = Some(item.erase_at.clone());
                    }
                    trashed.push(entry);
                }
                None => gone.push(entry),
            }
        }
        count += trashed.len();
        fields.insert("removed".to_owned(), Value::Array(gone));
        fields.insert("trashed".to_owned(), Value::Array(trashed));
    }
    if let Some(fields) = report.as_object_mut() {
        fields.insert(
            "trash".to_owned(),
            serde_json::json!({ "count": count, "erase_at": earliest }),
        );
    }
}

/// The originals item that holds journal-relative `path`, waiting or on its
/// way in.
fn originals_holder(journal: &Path, path: &str) -> Option<Item> {
    solstone_core_trash::holders(journal, path, "")
        .ok()?
        .iter()
        .filter_map(|id| solstone_core_trash::item(journal, id).ok().flatten())
        .find(|item| {
            matches!(item.unit, Unit::Originals { .. }) && item.paths.iter().any(|held| held == path)
        })
}

/// The segment an item's originals belong to.
fn target_of(item: &Item) -> Option<Target> {
    match &item.unit {
        Unit::Originals {
            day,
            stream,
            segment,
            ..
        } => Some(Target {
            day: day.clone(),
            stream: stream.clone(),
            dir: segment.clone(),
        }),
        _ => None,
    }
}

/// Where the segment of `path` is, from the file's own path.
fn segment_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(segment, _)| segment)
}

/// Whether the segment at `segment_rel` is in the journal as itself, not a
/// tombstone or a removal part way.
fn segment_live(journal: &Path, segment_rel: &str) -> bool {
    let place = journal.join(segment_rel);
    place.is_dir()
        && matches!(
            solstone_core_segment::owner_deletion_state(&place),
            Ok(solstone_core_segment::OwnerDeletionState::Live)
        )
}

impl Kind for Originals {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Originals { .. })
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let target = target_of(item)
            .ok_or_else(|| "this isn't a recording your journal can place".to_owned())?;
        // A recover needs its segment here as itself: waiting in the trash,
        // the segment comes back first; gone for good, this can't come back,
        // and the item stays on its timer.
        if item.state == solstone_core_trash::State::Trashed {
            match segment_state(journal, item) {
                SegmentState::Live => {}
                SegmentState::Trashed => {
                    return Err(serde_json::json!({ "code": RECORDING_IN_TRASH }).to_string());
                }
                SegmentState::Gone => {
                    return Err(serde_json::json!({ "code": SEGMENT_GONE }).to_string());
                }
            }
        }
        Ok(Box::new(SegmentHold::acquire(journal, &target)?))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        let Some(segment) = segment_of(path) else {
            return false;
        };
        segment_live(journal, segment)
            && std::fs::symlink_metadata(journal.join(path))
                .is_ok_and(|metadata| metadata.file_type().is_file())
    }

    fn returns(&self, journal: &Path, _item: &Item, path: &str) -> bool {
        // A recording erased since, or removed for good, has no place for it.
        segment_of(path).is_some_and(|segment| segment_live(journal, segment))
    }

    fn narrowed(&self, unit: &Unit, paths: &[String]) -> Unit {
        match unit {
            Unit::Originals {
                day,
                stream,
                segment,
                ..
            } => Unit::Originals {
                day: day.clone(),
                stream: stream.clone(),
                segment: segment.clone(),
                files: paths
                    .iter()
                    .filter_map(|path| path.rsplit_once('/').map(|(_, name)| name.to_owned()))
                    .collect(),
            },
            other => other.clone(),
        }
    }

    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        let target = target_of(item)
            .ok_or_else(|| "this isn't a recording your journal can place".to_owned())?;
        let mut wrote = false;
        for path in files {
            let (Some(segment), Some((_, name))) = (segment_of(path), path.rsplit_once('/')) else {
                continue;
            };
            // Gone with its recording since: that recording carries its events.
            if !segment_live(journal, segment) {
                continue;
            }
            let dir = contained_path(journal, segment).map_err(|error| error.to_string())?;
            let event = match change {
                Change::Detached => {
                    // The door wrote it unless a stop came between its release
                    // and its record.
                    if recorded_original_deletions(&dir).contains_key(name) {
                        continue;
                    }
                    "original_deleted"
                }
                Change::Attached => ORIGINAL_RECOVERED_EVENT,
            };
            let mut extra = Map::new();
            extra.insert("name".to_owned(), Value::String(name.to_owned()));
            if change == Change::Detached {
                let class = solstone_core_trash::kept(journal, &item.id, CLASS)
                    .ok()
                    .flatten()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .unwrap_or_else(|| RawReleaseClass::Owner.tag().to_owned());
                extra.insert("class".to_owned(), Value::String(class));
            }
            extra.insert("item".to_owned(), Value::String(item.id.clone()));
            let envelope = CallosumEnvelope {
                tract: "retention".to_owned(),
                event: event.to_owned(),
                ts: Some(chrono::Utc::now().timestamp_millis()),
                extra,
            };
            append_durable_event(&dir, &DurableEvent::Callosum(envelope))
                .map_err(|error| format!("the recording's events weren't written: {error}"))?;
            wrote = true;
        }
        if wrote || change == Change::Attached {
            solstone_core_journal_io::bump_stream_marker(journal, &target.day)
                .map(|_| ())
                .map_err(|error| format!("the day was not queued for follow-up: {error}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    reason = "test code"
)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use chrono::{DateTime, Utc};
    use solstone_core_removals::{Event, Page};

    use super::*;
    use crate::content::{ContentName, MediaClassifier};
    use crate::door::release_raw;
    use crate::eligibility::ProvenRaw;

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const DAY: &str = "20260805";
    const STREAM: &str = "field.audio";
    const DIR: &str = "070000_17";

    struct Bed(PathBuf);

    impl Bed {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "retention-originals-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join(format!("chronicle/{DAY}/{STREAM}/{DIR}"))).unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn segment(&self) -> PathBuf {
            self.0.join(format!("chronicle/{DAY}/{STREAM}/{DIR}"))
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn media_only(name: &ContentName) -> bool {
        matches!(name.extension().as_deref(), Some("flac" | "webm"))
    }

    fn proof(name: &str, size: u64) -> ProvenRaw {
        let classifier: &dyn MediaClassifier = &media_only;
        ProvenRaw::for_test(classifier, DAY, STREAM, DIR, name, size).unwrap()
    }

    fn at() -> DateTime<Utc> {
        Utc::now()
    }

    fn web() -> Surface {
        Surface::Web {
            page: Page::Settings,
        }
    }

    /// The command line's release, as `release-raw` runs it.
    fn release(bed: &Bed, proven: &[ProvenRaw]) -> Vec<String> {
        let linked = link_before(
            &bed.0,
            &of_proven(proven),
            RawReleaseClass::Owner,
            Action::RetentionTool,
            Surface::Cli,
        )
        .unwrap();
        let _ = release_raw(&bed.0, proven, RawReleaseClass::Owner, at());
        keep_after(&bed.0, linked)
    }

    fn history(bed: &Bed) -> Vec<(Event, Action, Unit)> {
        solstone_core_removals::history(&bed.0, 100)
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| (entry.event, entry.action, entry.unit))
            .collect()
    }

    #[test]
    fn a_released_original_waits_in_the_trash_and_plays_again_when_recovered() {
        let bed = Bed::new();
        fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
        fs::write(bed.segment().join("b.webm"), b"screen").unwrap();
        let unsettled = release(&bed, &[proof("a.flac", 6), proof("b.webm", 6)]);
        assert!(unsettled.is_empty(), "{unsettled:?}");
        assert!(!bed.segment().join("a.flac").exists());

        let items = solstone_core_trash::list(&bed.0).unwrap();
        assert_eq!(items.len(), 1, "one item per recording");
        let item = &items[0];
        assert_eq!(
            item.unit,
            Unit::Originals {
                day: DAY.into(),
                stream: STREAM.into(),
                segment: DIR.into(),
                files: vec!["a.flac".into(), "b.webm".into()],
            }
        );
        assert_eq!(modalities(item), vec!["audio", "screen"]);
        assert_eq!(segment_state(&bed.0, item), SegmentState::Live);
        assert!(in_trash(&bed.0, &format!("chronicle/{DAY}/{STREAM}/{DIR}"), "a.flac"));
        assert_eq!(
            recorded_original_deletions(&bed.segment()).len(),
            2,
            "readers still read it as deleted"
        );
        let events = history(&bed);
        assert_eq!(events.len(), 1, "trashed once, never `removed`: {events:?}");
        assert_eq!(events[0].0, Event::Trashed);
        assert_eq!(events[0].1, Action::RetentionTool);

        solstone_core_trash::attach(&bed.0, &[&Originals], &item.id, Action::Recover, web())
            .unwrap();
        assert_eq!(fs::read(bed.segment().join("a.flac")).unwrap(), b"audio1");
        assert_eq!(fs::read(bed.segment().join("b.webm")).unwrap(), b"screen");
        assert!(
            recorded_original_deletions(&bed.segment()).is_empty(),
            "every reader reads it present"
        );
        assert!(!in_trash(&bed.0, &format!("chronicle/{DAY}/{STREAM}/{DIR}"), "a.flac"));
        assert!(
            history(&bed)
                .iter()
                .any(|(event, _, _)| *event == Event::Recovered)
        );
    }

    #[test]
    fn what_the_door_keeps_never_goes_to_the_trash() {
        let bed = Bed::new();
        fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
        let proven = [proof("a.flac", 6)];
        let linked = link_before(
            &bed.0,
            &of_proven(&proven),
            RawReleaseClass::Owner,
            Action::RetentionTool,
            Surface::Cli,
        )
        .unwrap();
        // Something else holds the journal: the door refuses and keeps it.
        let root = solstone_core_journal_io::JournalRoot::open(&bed.0).unwrap();
        let busy =
            solstone_core_journal_io::activity::ActivityLease::try_acquire(std::sync::Arc::new(root))
                .unwrap();
        let (outcome, _) = release_raw(&bed.0, &proven, RawReleaseClass::Owner, at());
        assert!(outcome.removed_paths().next().is_none());
        drop(busy);
        let unsettled = keep_after(&bed.0, linked);
        assert!(unsettled.is_empty(), "{unsettled:?}");
        assert_eq!(fs::read(bed.segment().join("a.flac")).unwrap(), b"audio1");
        assert!(solstone_core_trash::list(&bed.0).unwrap().is_empty());
        assert!(history(&bed).is_empty());
    }

    #[test]
    fn a_file_already_gone_is_left_out_and_the_rest_are_released() {
        let bed = Bed::new();
        fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
        let unsettled = release(&bed, &[proof("a.flac", 6), proof("gone.flac", 6)]);
        assert!(unsettled.is_empty());
        let items = solstone_core_trash::list(&bed.0).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].paths.len(), 1);
    }

    #[test]
    fn a_recover_waits_for_its_recording_and_refuses_once_it_is_gone() {
        let bed = Bed::new();
        fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
        fs::write(bed.segment().join("a.jsonl"), b"{}\n").unwrap();
        release(&bed, &[proof("a.flac", 6)]);
        let originals = solstone_core_trash::list(&bed.0).unwrap().remove(0);

        let index = crate::notify::NoIndex;
        let recordings = crate::recording_trash::Recordings::new(&index);
        let target = Target {
            day: DAY.into(),
            stream: STREAM.into(),
            dir: DIR.into(),
        };
        let recording = recordings
            .trash(
                &bed.0,
                &target,
                Unit::Segment {
                    day: DAY.into(),
                    stream: STREAM.into(),
                    segment: DIR.into(),
                },
                Action::SegmentDelete,
                web(),
            )
            .unwrap();
        let kinds: [&dyn Kind; 2] = [&recordings, &Originals];
        let refused =
            solstone_core_trash::attach(&bed.0, &kinds, &originals.id, Action::Recover, web())
                .unwrap_err();
        assert!(refused.to_string().contains(RECORDING_IN_TRASH), "{refused}");

        // Erasing the recording takes its originals with it.
        solstone_core_trash::erase(&bed.0, &recording.id, Action::Erase, web()).unwrap();
        assert!(solstone_core_trash::item(&bed.0, &originals.id).unwrap().is_none());
    }
}
