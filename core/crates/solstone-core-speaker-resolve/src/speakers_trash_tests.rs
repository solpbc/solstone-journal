// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::Cell;
use std::fs;
use std::path::Path;

use solstone_core_removals::{Event, Page};
use solstone_core_retention::notify::{NotifyError, PruneCounts};
use solstone_core_retention::recording_trash::Recordings;
use solstone_core_retention::{NoIndex, Target};
use tempfile::TempDir;

use super::*;

const DAY: &str = "20260801";
/// A recording in the direct layout: the day's own child.
const DIRECT: &str = "chronicle/20260801/090000_300";
/// A recording on a named stream.
const NAMED: &str = "chronicle/20260801/mic/100000_300";
const PERSON: &str = "entities/bob";

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn read(root: &Path, rel: &str) -> Vec<u8> {
    fs::read(root.join(rel)).unwrap()
}

/// Voice files in both layouts, a person and the owner, the pool, and the
/// owner's own decisions beside them.
fn journal() -> TempDir {
    let root = TempDir::new().unwrap();
    let root_path = root.path();
    write(
        root_path,
        "config/journal.json",
        br#"{"setup":{"completed_at":1700000000000}}"#,
    );
    for segment in [DIRECT, NAMED] {
        write(root_path, &format!("{segment}/audio.npz"), b"embeddings");
        write(root_path, &format!("{segment}/audio.jsonl"), b"transcript");
        write(
            root_path,
            &format!("{segment}/talents/speaker_labels.json"),
            b"labels",
        );
        write(
            root_path,
            &format!("{segment}/agents/speaker_corrections.json"),
            b"corrections",
        );
        write(
            root_path,
            &format!("{segment}/talents/owner_line_labels.json"),
            b"owner answers",
        );
        write(
            root_path,
            &format!("{segment}/talents/speakers.json"),
            b"meeting names",
        );
    }
    write(
        root_path,
        "chronicle/20260801/health/stream.npz",
        b"not voice",
    );
    for (dir, name) in [(PERSON, "Bob"), ("entities/owner", "Owner")] {
        write(
            root_path,
            &format!("{dir}/entity.json"),
            format!("{{\"name\":\"{name}\"}}").as_bytes(),
        );
    }
    write(
        root_path,
        &format!("{PERSON}/voiceprints.npz"),
        b"bob voice",
    );
    write(
        root_path,
        "entities/owner/owner_centroid.npz",
        b"owner voice",
    );
    write(root_path, OWNER_CANDIDATE_PATH, b"candidate");
    write(root_path, POOL, b"pool");
    write(
        root_path,
        "awareness/current.json",
        br#"{"rejected_candidates":[3]}"#,
    );
    write(root_path, "awareness/discovery_clusters.json", b"{}");
    write(
        root_path,
        "awareness/discovery_clusters.resolved.json",
        b"{}",
    );
    write(
        root_path,
        "speakers/cluster-dismissals.jsonl",
        b"dismissed\n",
    );
    write(root_path, "speakers/keep-separate.jsonl", b"separate\n");
    root
}

const OWNER_CANDIDATE_PATH: &str = crate::artifact_wipe::OWNER_CANDIDATE;
const POOL: &str = crate::artifact_wipe::SPEAKER_CANDIDATES;

fn wiped_paths() -> Vec<String> {
    let mut paths = vec![
        OWNER_CANDIDATE_PATH.to_owned(),
        POOL.to_owned(),
        format!("{PERSON}/voiceprints.npz"),
        "entities/owner/owner_centroid.npz".to_owned(),
    ];
    for segment in [DIRECT, NAMED] {
        paths.push(format!("{segment}/audio.npz"));
        paths.push(format!("{segment}/talents/speaker_labels.json"));
        paths.push(format!("{segment}/agents/speaker_corrections.json"));
    }
    paths.sort();
    paths
}

/// What the wipe leaves, as the owner's decisions or not voice.
fn kept() -> Vec<String> {
    let mut kept = vec![
        "awareness/current.json".to_owned(),
        "speakers/cluster-dismissals.jsonl".to_owned(),
        "speakers/keep-separate.jsonl".to_owned(),
        "chronicle/20260801/health/stream.npz".to_owned(),
        format!("{PERSON}/entity.json"),
    ];
    for segment in [DIRECT, NAMED] {
        kept.push(format!("{segment}/audio.jsonl"));
        kept.push(format!("{segment}/talents/owner_line_labels.json"));
        kept.push(format!("{segment}/talents/speakers.json"));
    }
    kept
}

fn surface() -> Surface {
    Surface::Web {
        page: Page::Speakers,
    }
}

fn wipe_now(root: &Path) -> Wiped {
    Speakers::new(&NoIndex)
        .wipe(root, surface(), false)
        .unwrap()
}

/// A hold's refusal, as the JSON the trash page reads.
fn held(error: &TrashError) -> Value {
    let TrashError::Held(reason) = error else {
        panic!("not a hold refusal: {error}");
    };
    serde_json::from_str(reason).unwrap()
}

fn item_id(wiped: &Wiped) -> String {
    wiped.trash.as_ref().unwrap().0.clone()
}

fn recover(root: &Path, id: &str) -> Result<Item, TrashError> {
    let speakers = Speakers::new(&NoIndex);
    let recordings = Recordings::new(&NoIndex, &solstone_core_retention::made_from::NothingMade);
    let people = StandIn(Unit::Entity);
    let imports = StandIn(Unit::Import {
        source: "ics".to_owned(),
        id: "20260801_090000".to_owned(),
    });
    solstone_core_trash::attach(
        root,
        &[&speakers, &recordings, &people, &imports],
        id,
        Action::Recover,
        surface(),
    )
}

/// Events the deletion record holds for the wipe.
fn speaker_events(root: &Path) -> Vec<Event> {
    let mut events: Vec<Event> = solstone_core_removals::history(root, 100)
        .unwrap()
        .entries
        .into_iter()
        .filter(|entry| entry.unit == Unit::Speakers)
        .map(|entry| entry.event)
        .collect();
    events.sort_by_key(|event| format!("{event:?}"));
    events
}

/// Another kind of thing in the trash that holds a folder whole, standing in
/// for a person or an import: its hold and references do nothing.
struct StandIn(Unit);

struct Free;

impl Hold for Free {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Kind for StandIn {
    fn handles(&self, unit: &Unit) -> bool {
        *unit == self.0
    }

    fn hold<'h>(&'h self, _journal: &Path, _item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        Ok(Box::new(Free))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        journal.join(path).exists()
    }

    fn references(
        &self,
        _journal: &Path,
        _item: &Item,
        _change: Change,
        _files: &[String],
    ) -> Result<(), String> {
        Ok(())
    }
}

fn trash_person(root: &Path) -> Item {
    solstone_core_trash::detach_labelled(
        root,
        &StandIn(Unit::Entity),
        Unit::Entity,
        Action::EntityDelete,
        surface(),
        vec![PERSON.to_owned()],
        Some("Bob".to_owned()),
    )
    .unwrap()
}

fn trash_recording(root: &Path) -> Item {
    Recordings::new(&NoIndex, &solstone_core_retention::made_from::NothingMade)
        .trash(
            root,
            &Target {
                day: DAY.to_owned(),
                stream: solstone_core_journal_io::DEFAULT_STREAM.to_owned(),
                dir: "090000_300".to_owned(),
            },
            Unit::Segment {
                day: DAY.to_owned(),
                stream: solstone_core_journal_io::DEFAULT_STREAM.to_owned(),
                segment: "090000_300".to_owned(),
            },
            Action::SegmentDelete,
            surface(),
        )
        .unwrap()
}

#[test]
fn the_wipe_takes_every_layouts_voice_files_and_the_pool_and_leaves_the_owners_decisions() {
    let root = journal();
    let dry = crate::artifact_wipe::dry_run(root.path()).unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.total_files, wiped_paths().len());
    assert_eq!(dry.speaker_candidates.paths, vec![POOL.to_owned()]);
    assert!(
        wiped_paths()
            .iter()
            .all(|path| root.path().join(path).is_file())
    );

    let wiped = wipe_now(root.path());
    let item = solstone_core_trash::item(root.path(), &item_id(&wiped))
        .unwrap()
        .unwrap();
    assert_eq!(item.paths, wiped_paths(), "the dry run named the same set");
    assert_eq!(item.state, State::Trashed);
    assert!(!wiped.report.dry_run);
    assert_eq!(wiped.report.total_files, dry.total_files);
    assert_eq!(wiped.report.total_bytes, dry.total_bytes);
    assert_eq!(wiped.report.segment_embeddings.count, 2);
    assert_eq!(wiped.report.speaker_labels.count, 2);
    assert_eq!(wiped.report.speaker_corrections.count, 2);
    assert_eq!(wiped.report.entity_voiceprints.count, 1);
    assert_eq!(wiped.report.owner_centroids.count, 1);
    assert_eq!(wiped.report.owner_candidate.count, 1);
    for path in wiped_paths() {
        assert!(!root.path().join(&path).exists(), "{path} left the journal");
    }
    for path in kept() {
        assert!(root.path().join(&path).is_file(), "{path} stays");
    }
    for cache in [
        "awareness/discovery_clusters.json",
        "awareness/discovery_clusters.resolved.json",
    ] {
        assert!(!root.path().join(cache).exists(), "{cache} was cleared");
    }
    assert_eq!(speaker_events(root.path()), vec![Event::Trashed]);
    let value = wiped.to_json();
    assert_eq!(value["trash"]["id"], item.id.as_str());
    assert_eq!(value["trash"]["erase_at"], item.erase_at.as_str());
    assert_eq!(
        value["trash"]["erase_day"],
        solstone_core_journal_config::owner_day(root.path(), item.erase_at().unwrap())
    );
    assert!(value.get("erased").is_none());
}

#[test]
fn a_wipe_with_nothing_to_take_makes_no_item() {
    let root = TempDir::new().unwrap();
    let wiped = wipe_now(root.path());
    assert!(wiped.trash.is_none());
    assert_eq!(wiped.report.total_files, 0);
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    assert!(speaker_events(root.path()).is_empty());
}

#[test]
fn a_wipe_recovers_exactly_and_erases_at_once_when_asked() {
    let root = journal();
    let wiped = wipe_now(root.path());
    write(root.path(), "awareness/discovery_clusters.json", b"{}");
    recover(root.path(), &item_id(&wiped)).unwrap();
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    let fresh = journal();
    for path in wiped_paths() {
        assert_eq!(
            read(root.path(), &path),
            read(fresh.path(), &path),
            "{path}"
        );
    }
    assert!(
        !root
            .path()
            .join("awareness/discovery_clusters.json")
            .exists()
    );
    assert_eq!(
        speaker_events(root.path()),
        vec![Event::Recovered, Event::Trashed]
    );

    let erased = Speakers::new(&NoIndex)
        .wipe(root.path(), surface(), true)
        .unwrap();
    assert!(erased.erased);
    assert_eq!(erased.to_json()["erased"], true);
    assert!(erased.to_json().get("trash").is_none());
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    for path in wiped_paths() {
        assert!(!root.path().join(&path).exists(), "{path} was erased");
    }
    for path in kept() {
        assert!(root.path().join(&path).is_file(), "{path} stays");
    }
}

/// Search that fails once, as a stop after the move would leave it.
struct FailsOnce(Cell<bool>);

impl IndexNotify for FailsOnce {
    fn paths_removed(&self, _removed: &[RemovedPath]) -> Result<PruneCounts, NotifyError> {
        if self.0.replace(false) {
            return Err(NotifyError {
                reason: "stopped".to_owned(),
            });
        }
        Ok(PruneCounts::default())
    }

    fn paths_returned(&self, _returned: &[String]) -> Result<(), NotifyError> {
        Ok(())
    }
}

#[test]
fn a_wipe_that_stopped_part_way_is_finished_by_the_sweep() {
    let root = journal();
    let index = FailsOnce(Cell::new(true));
    let wiped = Speakers::new(&index)
        .wipe(root.path(), surface(), false)
        .unwrap();
    let id = item_id(&wiped);
    let item = solstone_core_trash::item(root.path(), &id)
        .unwrap()
        .unwrap();
    assert_eq!(item.state, State::Detaching);
    // As if the stop came before the last file moved.
    let last = item.paths.last().unwrap().clone();
    let held = solstone_core_trash::held_file(root.path(), &id, &last).unwrap();
    fs::rename(&held, root.path().join(&last)).unwrap();

    let speakers = Speakers::new(&index);
    let swept = solstone_core_trash::sweep(root.path(), &[&speakers], chrono::Utc::now());
    assert_eq!(swept.resumed, 1, "{swept:?}");
    assert!(swept.failed.is_empty(), "{swept:?}");
    let item = solstone_core_trash::item(root.path(), &id)
        .unwrap()
        .unwrap();
    assert_eq!(item.state, State::Trashed);
    for path in wiped_paths() {
        assert!(!root.path().join(&path).exists(), "{path} left the journal");
        let held = solstone_core_trash::held_file(root.path(), &id, &path).unwrap();
        assert!(held.is_file(), "{path} is held once");
    }
    assert_eq!(speaker_events(root.path()), vec![Event::Trashed]);
}

#[test]
fn recovering_overwrites_labels_made_since_and_says_so() {
    let root = journal();
    let wiped = wipe_now(root.path());
    let labels = format!("{NAMED}/talents/speaker_labels.json");
    write(root.path(), &labels, b"made since");
    recover(root.path(), &item_id(&wiped)).unwrap();
    assert_eq!(read(root.path(), &labels), b"labels");
    assert_eq!(
        speaker_events(root.path()),
        vec![Event::Erased, Event::Recovered, Event::Trashed]
    );
}

#[test]
fn a_recording_trashed_after_the_wipe_comes_back_first() {
    let root = journal();
    let wiped = wipe_now(root.path());
    let recording = trash_recording(root.path());
    let error = recover(root.path(), &item_id(&wiped)).unwrap_err();
    assert_eq!(
        held(&error),
        json!({"code": "waits_wipe", "segments": 1, "imports": 0, "people": 0})
    );
    assert!(!root.path().join(format!("{DIRECT}/audio.npz")).exists());

    recover(root.path(), &recording.id).unwrap();
    recover(root.path(), &item_id(&wiped)).unwrap();
    assert_eq!(
        read(root.path(), &format!("{DIRECT}/audio.npz")),
        b"embeddings"
    );
    assert_eq!(
        read(
            root.path(),
            &format!("{DIRECT}/talents/speaker_labels.json")
        ),
        b"labels"
    );
    assert_eq!(
        read(root.path(), &format!("{DIRECT}/audio.jsonl")),
        b"transcript"
    );
    assert!(!speaker_events(root.path()).contains(&Event::Erased));
}

#[test]
fn a_recording_trashed_then_erased_takes_its_voice_files_with_the_wipe() {
    let root = journal();
    let wiped = wipe_now(root.path());
    let recording = trash_recording(root.path());
    solstone_core_trash::erase(root.path(), &recording.id, Action::Erase, surface()).unwrap();
    let before: Vec<_> = fs::read_dir(root.path().join(DIRECT))
        .map(|listing| listing.flatten().map(|entry| entry.file_name()).collect())
        .unwrap_or_default();
    assert_eq!(
        before,
        vec![std::ffi::OsString::from(
            solstone_core_retention::tombstone::TOMBSTONE_NAME
        )],
        "the erased recording left only its marker"
    );

    recover(root.path(), &item_id(&wiped)).unwrap();
    let after: Vec<_> = fs::read_dir(root.path().join(DIRECT))
        .map(|listing| listing.flatten().map(|entry| entry.file_name()).collect())
        .unwrap_or_default();
    assert_eq!(
        after, before,
        "nothing landed beside what the recording left"
    );
    assert!(!root.path().join(format!("{DIRECT}/audio.npz")).exists());
    assert!(!root.path().join(format!("{DIRECT}/talents")).exists());
    assert_eq!(
        read(root.path(), &format!("{NAMED}/audio.npz")),
        b"embeddings"
    );
    assert!(speaker_events(root.path()).contains(&Event::Erased));
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
}

#[test]
fn a_person_trashed_after_the_wipe_comes_back_first() {
    let root = journal();
    let wiped = wipe_now(root.path());
    let person = trash_person(root.path());
    let error = recover(root.path(), &item_id(&wiped)).unwrap_err();
    assert_eq!(
        held(&error),
        json!({"code": "waits_wipe", "segments": 0, "imports": 0, "people": 1, "name": "Bob"})
    );
    assert!(!root.path().join(PERSON).exists());

    recover(root.path(), &person.id).unwrap();
    recover(root.path(), &item_id(&wiped)).unwrap();
    assert_eq!(
        read(root.path(), &format!("{PERSON}/voiceprints.npz")),
        b"bob voice"
    );
    assert!(!speaker_events(root.path()).contains(&Event::Erased));
}

#[test]
fn a_person_erased_merged_or_gone_takes_their_voiceprint_with_the_wipe() {
    for how in ["erased", "merged away"] {
        let root = journal();
        let wiped = wipe_now(root.path());
        match how {
            "erased" => {
                let person = trash_person(root.path());
                solstone_core_trash::erase(root.path(), &person.id, Action::Erase, surface())
                    .unwrap();
            }
            // A merge leaves no folder for the one merged in.
            _ => fs::remove_dir_all(root.path().join(PERSON)).unwrap(),
        }
        recover(root.path(), &item_id(&wiped)).unwrap();
        assert!(!root.path().join(PERSON).exists(), "{how}: no folder made");
        assert_eq!(
            read(root.path(), "entities/owner/owner_centroid.npz"),
            b"owner voice",
            "{how}"
        );
        assert!(
            speaker_events(root.path()).contains(&Event::Erased),
            "{how}"
        );
        assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    }
}

#[test]
fn a_recording_held_by_an_import_comes_back_first() {
    let root = journal();
    let wiped = wipe_now(root.path());
    let unit = Unit::Import {
        source: "ics".to_owned(),
        id: "20260801_090000".to_owned(),
    };
    solstone_core_trash::detach_labelled(
        root.path(),
        &StandIn(unit.clone()),
        unit,
        Action::ImportDelete,
        surface(),
        vec![NAMED.to_owned()],
        Some("notes.ics".to_owned()),
    )
    .unwrap();
    let error = recover(root.path(), &item_id(&wiped)).unwrap_err();
    assert_eq!(
        held(&error),
        json!({"code": "waits_wipe", "segments": 0, "imports": 1, "people": 0})
    );
}

#[test]
fn several_homes_in_the_trash_are_counted_by_kind() {
    let root = journal();
    let wiped = wipe_now(root.path());
    trash_recording(root.path());
    trash_person(root.path());
    let error = recover(root.path(), &item_id(&wiped)).unwrap_err();
    assert_eq!(
        held(&error),
        json!({"code": "waits_wipe", "segments": 1, "imports": 0, "people": 1, "name": "Bob"})
    );
}

#[test]
fn a_wipe_while_the_journal_is_held_refuses_as_busy_and_moves_nothing() {
    let root = journal();
    // A recording's trash, or any whole-recording work, holds the lease.
    let _lease =
        solstone_core_retention::door::SegmentHold::acquire_lease(root.path(), &[]).unwrap();
    let error = Speakers::new(&NoIndex)
        .wipe(root.path(), surface(), false)
        .unwrap_err();
    assert_eq!(held(&error), json!({"code": "busy"}));
    assert_eq!(refusal_code(&error.to_string()).as_deref(), Some("busy"));
    for path in wiped_paths() {
        assert!(root.path().join(&path).is_file(), "{path} stays");
    }
    assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
}

#[test]
fn a_file_returns_only_while_its_home_is_there() {
    let root = journal();
    let wiped = wipe_now(root.path());
    let item = solstone_core_trash::item(root.path(), &item_id(&wiped))
        .unwrap()
        .unwrap();
    let speakers = Speakers::new(&NoIndex);
    let segment_file = format!("{NAMED}/talents/speaker_labels.json");
    let person_file = format!("{PERSON}/voiceprints.npz");
    assert!(speakers.returns(root.path(), &item, &segment_file));
    assert!(speakers.returns(root.path(), &item, &person_file));
    assert!(speakers.returns(root.path(), &item, POOL));
    // The homes go just before.
    fs::remove_dir_all(root.path().join(NAMED)).unwrap();
    fs::remove_file(root.path().join(PERSON).join("entity.json")).unwrap();
    assert!(!speakers.returns(root.path(), &item, &segment_file));
    assert!(!speakers.returns(root.path(), &item, &person_file));
    // A folder holding only a removal's marker isn't a live recording.
    write(
        root.path(),
        &format!(
            "{NAMED}/{}",
            solstone_core_retention::tombstone::TOMBSTONE_NAME
        ),
        b"{}",
    );
    assert!(!speakers.returns(root.path(), &item, &segment_file));
    assert!(speakers.returns(root.path(), &item, POOL));
}

#[cfg(feature = "full-tests")]
#[test]
fn the_pool_moves_only_once_its_writer_lets_go() {
    use std::sync::mpsc;
    use std::time::Duration;

    let root = journal();
    let journal_path = root.path().to_path_buf();
    let (locked, wait) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let _lock = hold_lock(journal_path.join(POOL), LockOptions::default()).unwrap();
        locked.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        fs::write(journal_path.join(POOL), b"the writer's last").unwrap();
    });
    wait.recv().unwrap();
    let wiped = wipe_now(root.path());
    writer.join().unwrap();
    let id = item_id(&wiped);
    let held = solstone_core_trash::held_file(root.path(), &id, POOL).unwrap();
    assert_eq!(fs::read(held).unwrap(), b"the writer's last");
    assert!(!root.path().join(POOL).exists());
}
