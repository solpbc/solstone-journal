// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    reason = "test code: fixtures remove files to set up states"
)]
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
            "retention-originals-{}-{}-{}",
            std::process::id(),
            // A process id comes round again on a shared box, and an earlier
            // run's folder may still be there.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos()),
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
        false,
        at(),
    )
    .unwrap()
    .linked;
    let _ = release_raw(&bed.0, proven, RawReleaseClass::Owner, at());
    keep_after(&bed.0, linked).unsettled
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
    assert!(in_trash(
        &bed.0,
        &format!("chronicle/{DAY}/{STREAM}/{DIR}"),
        "a.flac"
    ));
    assert_eq!(
        recorded_original_deletions(&bed.segment()).len(),
        2,
        "readers still read it as deleted"
    );
    let events = history(&bed);
    assert_eq!(events.len(), 1, "trashed once, never `removed`: {events:?}");
    assert_eq!(events[0].0, Event::Trashed);
    assert_eq!(events[0].1, Action::RetentionTool);

    solstone_core_trash::attach(&bed.0, &[&Originals], &item.id, Action::Recover, web()).unwrap();
    assert_eq!(fs::read(bed.segment().join("a.flac")).unwrap(), b"audio1");
    assert_eq!(fs::read(bed.segment().join("b.webm")).unwrap(), b"screen");
    assert!(
        recorded_original_deletions(&bed.segment()).is_empty(),
        "every reader reads it present"
    );
    assert!(!in_trash(
        &bed.0,
        &format!("chronicle/{DAY}/{STREAM}/{DIR}"),
        "a.flac"
    ));
    assert!(
        history(&bed)
            .iter()
            .any(|(event, _, _)| *event == Event::Recovered)
    );
}

#[test]
fn a_name_an_offload_mark_also_holds_stays_outside_the_trash() {
    use crate::marks::{Mark, MarkId, MarkState, Proposal, RemovalClass};
    let target = Target {
        day: DAY.into(),
        stream: STREAM.into(),
        dir: DIR.into(),
    };
    let mark = |class: RemovalClass, names: &[&str]| {
        let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        Mark {
            id: MarkId::derive(class, &target, &names),
            class,
            target: target.clone(),
            marked_at: "2026-10-10T00:00:00Z".into(),
            proposal: Proposal {
                bytes: 1,
                reason: "synthetic".into(),
                names,
            },
            state: MarkState::Marked,
            offload: None,
        }
    };
    let policy = mark(RemovalClass::PolicyRawRelease, &["a.flac", "b.webm"]);
    let offload = mark(RemovalClass::OffloadRawRelease, &["b.webm"]);
    let releases = of_policy_marks([&policy, &offload]);
    assert_eq!(releases.len(), 1);
    assert_eq!(
        releases[0].paths,
        vec![format!("chronicle/{DAY}/{STREAM}/{DIR}/a.flac")]
    );
}

#[test]
fn a_report_says_what_waits_in_the_trash() {
    let bed = Bed::new();
    fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
    release(&bed, &[proof("a.flac", 6)]);
    let item = solstone_core_trash::list(&bed.0).unwrap().remove(0);
    let trashed = format!("chronicle/{DAY}/{STREAM}/{DIR}/a.flac");
    let gone = format!("chronicle/{DAY}/{STREAM}/{DIR}/b.webm");
    let mut report = serde_json::json!({
        "outcome": {"targets": [{"removed": [trashed, gone], "not_removed": []}]}
    });
    annotate_report(&bed.0, &mut report);
    assert_eq!(
        report["outcome"]["targets"][0]["trashed"],
        serde_json::json!([trashed])
    );
    assert_eq!(
        report["outcome"]["targets"][0]["removed"],
        serde_json::json!([gone])
    );
    assert_eq!(report["trash"]["count"], 1);
    assert_eq!(
        report["trash"]["erase_at"],
        serde_json::json!(item.erase_at)
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
        false,
        at(),
    )
    .unwrap()
    .linked;
    // Something else holds the journal: the door refuses and keeps it.
    let root = solstone_core_journal_io::JournalRoot::open(&bed.0).unwrap();
    let busy =
        solstone_core_journal_io::activity::ActivityLease::try_acquire(std::sync::Arc::new(root))
            .unwrap();
    let (outcome, _) = release_raw(&bed.0, &proven, RawReleaseClass::Owner, at());
    assert!(outcome.removed_paths().next().is_none());
    drop(busy);
    let unsettled = keep_after(&bed.0, linked).unsettled;
    assert!(unsettled.is_empty(), "{unsettled:?}");
    assert_eq!(fs::read(bed.segment().join("a.flac")).unwrap(), b"audio1");
    assert!(solstone_core_trash::list(&bed.0).unwrap().is_empty());
    assert!(history(&bed).is_empty());
}

/// The guarantee settling rests on: while a release is part way anywhere
/// in the journal (a stage still holding the original), the door's hold
/// refuses, and the item waits; nothing is kept or written.
#[test]
fn a_release_part_way_keeps_the_item_waiting_until_it_is_resolved() {
    let bed = Bed::new();
    fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
    let linked = link_before(
        &bed.0,
        &of_proven(&[proof("a.flac", 6)]),
        RawReleaseClass::Policy,
        Action::OriginalsRelease,
        Surface::Web { page: Page::Home },
        false,
        at(),
    )
    .unwrap()
    .linked;
    let id = linked[0].id().to_owned();
    // A stage the door left, which its recovery can't read.
    let stage = bed.0.join(format!(
        "chronicle/{DAY}/{STREAM}/.tmp-offload-{}",
        "0".repeat(32)
    ));
    fs::create_dir(&stage).unwrap();
    fs::rename(bed.segment().join("a.flac"), stage.join("removal-original")).unwrap();
    fs::write(stage.join("removal-record"), b"not a record").unwrap();
    let kept = keep_after(&bed.0, linked);
    assert_eq!(kept.unsettled.len(), 1, "{kept:?}");
    assert!(
        !kept.unsettled[0].contains("couldn't be changed"),
        "refused by the door's hold, not an error: {kept:?}"
    );
    let waiting = solstone_core_trash::item(&bed.0, &id).unwrap().unwrap();
    assert_eq!(waiting.state, solstone_core_trash::State::Detaching);
    assert!(recorded_original_deletions(&bed.segment()).is_empty());
    assert!(history(&bed).is_empty());

    // The stage is resolved by putting the original back: the sweep then
    // finds the journal's name is the trash's file, and drops its own.
    fs::rename(stage.join("removal-original"), bed.segment().join("a.flac")).unwrap();
    fs::remove_dir_all(&stage).unwrap();
    solstone_core_trash::sweep(&bed.0, &[&Originals], Utc::now());
    assert!(solstone_core_trash::item(&bed.0, &id).unwrap().is_none());
    assert_eq!(fs::read(bed.segment().join("a.flac")).unwrap(), b"audio1");
    assert!(history(&bed).is_empty());
}

/// A path that isn't there is left out of the act, and an act the door
/// released nothing of leaves nothing in the trash.
#[test]
fn an_act_the_door_released_nothing_of_leaves_nothing_in_the_trash() {
    let bed = Bed::new();
    fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
    let other = bed.0.join(format!("chronicle/{DAY}/{STREAM}/080000_17"));
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("b.flac"), b"audio2").unwrap();
    let releases = releases([
        (
            Target {
                day: DAY.into(),
                stream: STREAM.into(),
                dir: DIR.into(),
            },
            format!("chronicle/{DAY}/{STREAM}/{DIR}/a.flac"),
        ),
        (
            Target {
                day: DAY.into(),
                stream: STREAM.into(),
                dir: "080000_17".into(),
            },
            format!("chronicle/{DAY}/{STREAM}/080000_17/missing-dir/b.flac"),
        ),
    ]);
    let linking = link_before(
        &bed.0,
        &releases,
        RawReleaseClass::Owner,
        Action::RetentionTool,
        Surface::Cli,
        false,
        at(),
    );
    // The second segment's path isn't present, so it is left out; the
    // act goes ahead with the first alone.
    let linking = linking.unwrap();
    assert_eq!(linking.linked.len(), 1);
    let kept = keep_after(&bed.0, linking.linked);
    assert!(kept.items.is_empty());
    assert!(solstone_core_trash::list(&bed.0).unwrap().is_empty());
    assert_eq!(fs::read(bed.segment().join("a.flac")).unwrap(), b"audio1");
}

/// A recover whose segment is gone for good refuses and stays on its
/// timer, rather than putting a lone file back where nothing is.
#[test]
fn a_recover_whose_segment_is_gone_refuses_and_keeps_the_item() {
    let bed = Bed::new();
    fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
    release(&bed, &[proof("a.flac", 6)]);
    let item = solstone_core_trash::list(&bed.0).unwrap().remove(0);
    fs::remove_dir_all(bed.segment()).unwrap();
    assert_eq!(segment_state(&bed.0, &item), SegmentState::Gone);
    let refused =
        solstone_core_trash::attach(&bed.0, &[&Originals], &item.id, Action::Recover, web())
            .unwrap_err();
    assert!(refused.to_string().contains(SEGMENT_GONE), "{refused}");
    assert_eq!(
        solstone_core_trash::item(&bed.0, &item.id)
            .unwrap()
            .unwrap()
            .state,
        solstone_core_trash::State::Trashed
    );
}

/// A stop part way left the original in a stage: the door's recovery,
/// run under the hold before the trash decides what is there, puts it
/// back, so the trash keeps it and the release never loses it.
#[test]
fn an_original_a_stop_left_in_a_stage_is_put_back_and_kept() {
    use sha2::{Digest, Sha256};
    let bed = Bed::new();
    let bytes = b"audio1";
    let stage = bed.0.join(format!(
        "chronicle/{DAY}/{STREAM}/.tmp-offload-{}",
        "1".repeat(32)
    ));
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("removal-original"), bytes).unwrap();
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    fs::write(
        stage.join("removal-record"),
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "source": DIR, "name": "a.flac",
            "digest": {"size": bytes.len(), "sha256": digest},
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(!bed.segment().join("a.flac").exists());
    let unsettled = release(&bed, &[proof("a.flac", 6)]);
    assert!(unsettled.is_empty(), "{unsettled:?}");
    let items = solstone_core_trash::list(&bed.0).unwrap();
    assert_eq!(items.len(), 1, "kept in the trash, never released unkept");
    let held = solstone_core_trash::held_file(&bed.0, &items[0].id, &items[0].paths[0]).unwrap();
    assert_eq!(fs::read(held).unwrap(), bytes);
}

/// A different file at the name now would be put aside and erased by a
/// recover: the recover refuses, and both stay.
#[test]
fn a_recover_onto_a_different_file_refuses_and_keeps_both() {
    let bed = Bed::new();
    fs::write(bed.segment().join("a.flac"), b"audio1").unwrap();
    release(&bed, &[proof("a.flac", 6)]);
    let item = solstone_core_trash::list(&bed.0).unwrap().remove(0);
    fs::write(bed.segment().join("a.flac"), b"newer!").unwrap();
    let refused =
        solstone_core_trash::attach(&bed.0, &[&Originals], &item.id, Action::Recover, web())
            .unwrap_err();
    assert!(refused.to_string().contains(NAME_TAKEN), "{refused}");
    assert_eq!(fs::read(bed.segment().join("a.flac")).unwrap(), b"newer!");
    assert!(
        solstone_core_trash::item(&bed.0, &item.id)
            .unwrap()
            .is_some()
    );
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
    assert!(
        refused.to_string().contains(RECORDING_IN_TRASH),
        "{refused}"
    );

    // Erasing the recording takes its originals with it.
    solstone_core_trash::erase(&bed.0, &recording.id, Action::Erase, web()).unwrap();
    assert!(
        solstone_core_trash::item(&bed.0, &originals.id)
            .unwrap()
            .is_none()
    );
}
