// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use solstone_core_observer::store::prune::run_prune;
use solstone_core_observer::store::record::ObserverRecord;
use solstone_core_observer::store::write::save_observer;

const DAY: &str = "20260101";
const SUCCESSOR_DAY: &str = "20260103";
const UNTOUCHED_DAY: &str = "20260104";
const STREAM: &str = "workstation";

struct Fixture {
    root: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture(name: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!(
        "observer-prune-after-deletion-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    Fixture { root }
}

fn seed_observer(root: &Path, prefix: &str, stream: &str) {
    let record = ObserverRecord::from_value(json!({
        "key": format!("{prefix}12345678"),
        "name": stream,
        "stream": stream,
    }))
    .expect("record");
    save_observer(root, &record).expect("save observer");
}

fn segment_dir(root: &Path, day: &str, stream: &str, segment: &str) -> PathBuf {
    root.join("chronicle").join(day).join(stream).join(segment)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn write_marker(
    dir: &Path,
    stream: &str,
    prev_day: Option<&str>,
    prev_segment: Option<&str>,
    seq: u64,
) {
    let marker = json!({
        "stream": stream,
        "prev_day": prev_day,
        "prev_segment": prev_segment,
        "seq": seq,
    });
    fs::write(dir.join("stream.json"), marker.to_string()).expect("marker");
}

fn write_segment(
    root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    seq: u64,
    prev_segment: Option<&str>,
    audio: &[u8],
) -> PathBuf {
    let dir = segment_dir(root, day, stream, segment);
    fs::create_dir_all(&dir).expect("segment dir");
    fs::write(dir.join("audio.flac"), audio).expect("audio");
    let manifest = json!({
        "schema_version": 1,
        "files": {"audio.flac": {"sha256": sha256_hex(audio), "size": audio.len()}},
    });
    fs::write(dir.join("ingest.json"), manifest.to_string()).expect("manifest");
    write_marker(&dir, stream, prev_segment.map(|_| day), prev_segment, seq);
    dir
}

/// A stream registry state file, as written by the same code prune's tail
/// repair reads: `journal/streams/<name>.json`.
fn write_stream_state(root: &Path, stream: &str, last_day: &str, last_segment: &str, seq: u64) {
    let path = root.join("streams").join(format!("{stream}.json"));
    fs::create_dir_all(path.parent().expect("parent")).expect("streams dir");
    let state = json!({
        "name": stream,
        "type": "observer",
        "host": null,
        "platform": null,
        "created_at": 1,
        "last_day": last_day,
        "last_segment": last_segment,
        "seq": seq,
    });
    fs::write(path, state.to_string()).expect("write state");
}

#[test]
fn a_pruned_segment_in_the_middle_of_a_chain_repairs_the_survivor_predecessor() {
    let fixture = fixture("mid-chain");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    // 090000_300 and 090000_301 are same-start duplicates; 090000_301 chains
    // to 100000_300, which must repair its prev pointer to 090000_300 once
    // 090000_301 is pruned -- proving repair reaches through the deleted
    // segment rather than leaving a dangling pointer or losing the chain.
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    let downstream = write_segment(
        &fixture.root,
        SUCCESSOR_DAY,
        STREAM,
        "100000_300",
        3,
        None,
        b"downstream",
    );
    write_marker(&downstream, STREAM, Some(DAY), Some("090000_301"), 3);
    write_segment(
        &fixture.root,
        UNTOUCHED_DAY,
        "unrelated",
        "120000_300",
        1,
        None,
        b"unrelated",
    );
    write_stream_state(&fixture.root, STREAM, SUCCESSOR_DAY, "100000_300", 3);

    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert!(
        result.refusals.is_empty(),
        "refusals: {:?}",
        result.refusals
    );
    assert_eq!(result.deleted.len(), 1);
    assert_eq!(result.chain_repaired, 1);

    let downstream_marker: serde_json::Value =
        serde_json::from_slice(&fs::read(downstream.join("stream.json")).expect("marker"))
            .expect("json");
    assert_eq!(downstream_marker["prev_segment"], "090000_300");
    assert_eq!(downstream_marker["prev_day"], DAY);
    assert_eq!(
        downstream_marker["seq"], 3,
        "seq must never be renumbered by a repair"
    );

    // The stream tail still points at the untouched downstream segment, so
    // registry state needs no repair -- but seq must still be preserved.
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture.root.join("streams").join(format!("{STREAM}.json"))).expect("state"),
    )
    .expect("json");
    assert_eq!(state["last_segment"], "100000_300");
    assert_eq!(state["seq"], 3);
    assert!(
        fixture
            .root
            .join(format!("chronicle/{DAY}/health/stream.updated"))
            .is_file(),
        "the deletion day must be dirty"
    );
    assert!(
        fixture
            .root
            .join(format!("chronicle/{SUCCESSOR_DAY}/health/stream.updated"))
            .is_file(),
        "the durably repaired successor day must be dirty"
    );
    assert!(
        !fixture
            .root
            .join(format!("chronicle/{UNTOUCHED_DAY}/health/stream.updated"))
            .exists(),
        "an untouched day must not be dirtied"
    );
}

#[test]
fn pruning_the_tail_segment_repairs_the_registry_state_to_the_new_survivor() {
    let fixture = fixture("tail-repair");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    // The registry tail points at the segment that is about to be pruned.
    write_stream_state(&fixture.root, STREAM, DAY, "090000_301", 2);

    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert!(
        result.refusals.is_empty(),
        "refusals: {:?}",
        result.refusals
    );
    assert_eq!(result.deleted.len(), 1);

    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture.root.join("streams").join(format!("{STREAM}.json"))).expect("state"),
    )
    .expect("json");
    assert_eq!(
        state["last_segment"], "090000_300",
        "tail must move to the surviving canonical"
    );
    assert_eq!(state["seq"], 2, "seq is preserved, never renumbered down");
}

#[test]
fn pruning_a_non_tail_segment_leaves_the_registry_state_byte_identical() {
    let fixture = fixture("tail-still-present");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "100000_300",
        3,
        Some("090000_301"),
        b"downstream",
    );
    write_stream_state(&fixture.root, STREAM, DAY, "100000_300", 3);
    let state_path = fixture.root.join("streams").join(format!("{STREAM}.json"));
    let before = fs::read(&state_path).expect("state before prune");

    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert!(
        result.refusals.is_empty(),
        "refusals: {:?}",
        result.refusals
    );
    assert_eq!(result.deleted.len(), 1);
    assert_eq!(fs::read(state_path).expect("state after prune"), before);
}

#[test]
fn recognized_derived_outputs_do_not_refuse_and_one_stray_file_does() {
    let recognized = fixture("derived-recognized");
    seed_observer(&recognized.root, "abcdefgh", STREAM);
    write_segment(
        &recognized.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    let duplicate = write_segment(
        &recognized.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    fs::write(duplicate.join("audio.jsonl"), "{}").expect("same-stem sidecar");
    fs::write(duplicate.join("events.jsonl"), "{}").expect("events");
    fs::write(duplicate.join("timeline.json"), "{}").expect("timeline");
    fs::create_dir_all(duplicate.join("talents")).expect("talents dir");
    fs::write(duplicate.join("talents/sense.json"), "{}").expect("talents output");

    let result = run_prune(
        &recognized.root,
        &[DAY.to_owned()],
        Some(STREAM),
        true,
        1_000,
    );
    assert!(
        result.refusals.is_empty(),
        "recognized derived outputs must not refuse: {:?}",
        result.refusals
    );
    assert_eq!(result.deleted.len(), 1);

    let stray = fixture("derived-stray");
    seed_observer(&stray.root, "abcdefgh", STREAM);
    write_segment(
        &stray.root,
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    let duplicate = write_segment(
        &stray.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    fs::write(duplicate.join("unexpected.bin"), b"???").expect("stray file");

    let result = run_prune(&stray.root, &[DAY.to_owned()], Some(STREAM), true, 1_000);
    assert_eq!(result.exit_code(), 2);
    assert!(
        result
            .refusals
            .iter()
            .any(|refusal| refusal.gate == "derived-output"
                && refusal.file.as_deref() == Some("unexpected.bin")),
        "refusals: {:?}",
        result.refusals
    );
    assert!(duplicate.exists());
}

#[test]
fn last_physical_copy_is_surfaced_in_dry_run_and_execute_and_prunes_the_last_bytes() {
    let fixture = fixture("last-physical-copy");
    seed_observer(&fixture.root, "abcdefgh", STREAM);
    let audio = b"same bytes";
    let sha = sha256_hex(audio);

    // The canonical holds this content ONLY via terminal processing proof:
    // its ingest.json declares the file, but the bytes are absent and a
    // sidecar records that transcription already consumed and confirmed them.
    let canonical = segment_dir(&fixture.root, DAY, STREAM, "090000_300");
    fs::create_dir_all(&canonical).expect("canonical dir");
    let manifest = json!({
        "schema_version": 1,
        "files": {"audio.flac": {"sha256": sha, "size": audio.len()}},
    });
    fs::write(canonical.join("ingest.json"), manifest.to_string()).expect("manifest");
    write_marker(&canonical, STREAM, None, None, 1);
    let proof = json!({
        "_solstone_processing": {
            "schema": "solstone.processing.v1",
            "state": "analyzed",
            "handler": "transcribe",
            "input_size": audio.len(),
        }
    });
    fs::write(canonical.join("audio.jsonl"), format!("{}\n", proof)).expect("proof sidecar");

    // The candidate is a true duplicate that still holds the real bytes.
    let candidate = write_segment(
        &fixture.root,
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        audio,
    );

    let dry = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), false, 1_000);
    assert!(dry.refusals.is_empty(), "refusals: {:?}", dry.refusals);
    assert_eq!(dry.last_physical_copy_count(), 1);
    assert!(dry.groups[0].candidates[0].last_physical_copy);

    let result = run_prune(&fixture.root, &[DAY.to_owned()], Some(STREAM), true, 2_000);
    assert!(
        result.refusals.is_empty(),
        "refusals: {:?}",
        result.refusals
    );
    assert_eq!(result.last_physical_copy_count(), 1);
    assert!(result.deleted[0].last_physical_copy);
    assert!(
        !candidate.exists(),
        "the last physical bytes are deleted; only the proof-backed canonical survives"
    );
    assert!(canonical.exists());
}
