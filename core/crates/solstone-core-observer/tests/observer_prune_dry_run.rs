// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::observer_render_support::{seed_observer_owning_stream, segment_dir, write_segment};
use serde_json::json;
use solstone_core_observer::store::prune::{format_result, run_prune};

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn prune_refuses_non_utf8_segment_identity() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = tempfile::tempdir().expect("journal");
    seed_observer_owning_stream(root.path(), "abcdefgh", STREAM);
    write_segment(
        root.path(),
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    std::fs::create_dir_all(
        root.path()
            .join("chronicle")
            .join(DAY)
            .join(STREAM)
            .join(OsStr::from_bytes(b"090000_301\xff")),
    )
    .unwrap();
    let days = vec![DAY.to_owned()];
    let result = run_prune(root.path(), &days, Some(STREAM), false, 1_000);
    assert_eq!(result.refusals.len(), 1);
    assert_eq!(result.refusals[0].gate, "segment-identity");
    assert!(result.groups.is_empty());
}

const DAY: &str = "20260101";
const STREAM: &str = "workstation";

const EXPECTED_DRY_RUN: &str = "\
device prune dry-run
groups: 1
candidates: 1
deleted: 0
chain-repaired: 0
last-physical-copy: 0
refusals: 0
group 20260101/workstation/090000_*: canonical=090000_300 candidates=1
";

const EXPECTED_LAST_PHYSICAL_COPY: &str = "\
device prune dry-run
groups: 1
candidates: 1
deleted: 0
chain-repaired: 0
last-physical-copy: 1
refusals: 0
group 20260101/workstation/090000_*: canonical=090000_300 candidates=1
  would-delete: 090000_301 duplicate_of=090000_300 [last-physical-copy]
";

#[test]
fn prune_dry_run_plan_text_and_exit_code() {
    let root = tempfile::tempdir().expect("journal");
    seed_observer_owning_stream(root.path(), "abcdefgh", STREAM);
    write_segment(
        root.path(),
        DAY,
        STREAM,
        "090000_300",
        1,
        None,
        b"same bytes",
    );
    write_segment(
        root.path(),
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        b"same bytes",
    );
    write_segment(
        root.path(),
        DAY,
        STREAM,
        "100000_300",
        3,
        Some("090000_301"),
        b"unrelated one",
    );
    write_segment(
        root.path(),
        DAY,
        STREAM,
        "110000_300",
        4,
        Some("100000_300"),
        b"unrelated one",
    );
    let days = vec![DAY.to_owned()];
    let result = run_prune(root.path(), &days, Some(STREAM), false, 1_000);
    assert_eq!(
        format_result(&result),
        EXPECTED_DRY_RUN,
        "dry-run plan text"
    );
    assert_eq!(result.exit_code(), 0, "dry-run exit code");
}

#[test]
fn prune_last_physical_copy_marking_and_summary_count() {
    let root = tempfile::tempdir().expect("journal");
    seed_observer_owning_stream(root.path(), "abcdefgh", STREAM);
    let audio = b"same bytes";
    let sha = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(audio))
    };
    let canonical = segment_dir(root.path(), DAY, STREAM, "090000_300");
    std::fs::create_dir_all(&canonical).expect("canonical dir");
    std::fs::write(
        canonical.join("ingest.json"),
        json!({"schema_version": 1, "files": {"audio.flac": {"sha256": sha, "size": audio.len()}}})
            .to_string(),
    )
    .expect("manifest");
    std::fs::write(
        canonical.join("stream.json"),
        json!({"stream": STREAM, "prev_day": null, "prev_segment": null, "seq": 1}).to_string(),
    )
    .expect("marker");
    std::fs::write(
        canonical.join("audio.jsonl"),
        format!(
            "{}\n",
            json!({"_solstone_processing": {"schema": "solstone.processing.v1", "state": "analyzed", "handler": "transcribe", "input_size": audio.len()}})
        ),
    )
    .expect("proof sidecar");
    write_segment(
        root.path(),
        DAY,
        STREAM,
        "090000_301",
        2,
        Some("090000_300"),
        audio,
    );
    let days = vec![DAY.to_owned()];
    let result = run_prune(root.path(), &days, Some(STREAM), false, 1_000);
    let text = format_result(&result);
    assert_eq!(text, EXPECTED_LAST_PHYSICAL_COPY, "last-physical-copy text");
}
