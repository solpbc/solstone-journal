// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-language differential: the Rust dry-run plan's formatted text and
//! exit code must match `solstone/apps/observer/prune.py`'s over the same
//! fixture, including the `last-physical-copy` marking and summary count.

use super::common;

use serde_json::json;
use solstone_core_observer::store::prune::{format_result, run_prune};

const DAY: &str = "20260101";
const STREAM: &str = "workstation";

#[test]
fn dry_run_plan_matches_the_python_reference() {
    common::with_utc_tz(|| {
        let root = common::root("prune-dry-run-parity");
        common::seed_observer_owning_stream(&root, "abcdefgh", STREAM);

        // A true same-start duplicate pair.
        common::write_segment(&root, DAY, STREAM, "090000_300", 1, None, b"same bytes");
        common::write_segment(
            &root,
            DAY,
            STREAM,
            "090000_301",
            2,
            Some("090000_300"),
            b"same bytes",
        );
        // A distinct pair at a different start: must never group.
        common::write_segment(
            &root,
            DAY,
            STREAM,
            "100000_300",
            3,
            Some("090000_301"),
            b"unrelated one",
        );
        common::write_segment(
            &root,
            DAY,
            STREAM,
            "110000_300",
            4,
            Some("100000_300"),
            b"unrelated one",
        );

        let days = vec![DAY.to_owned()];
        let rust_result = run_prune(&root, &days, Some(STREAM), false, 1_000);
        let rust_text = format_result(&rust_result);
        let rust_code = rust_result.exit_code();

        let oracle = common::oracle_prune(&root, &days, Some(STREAM), false);
        let oracle_text = oracle["stdout"].as_str().expect("oracle stdout");
        let oracle_code = oracle["code"].as_i64().expect("oracle code");

        assert_eq!(
            rust_text, oracle_text,
            "Rust and Python dry-run report text must match exactly"
        );
        assert_eq!(i64::from(rust_code), oracle_code);

        common::cleanup(root);
    });
}

#[test]
fn last_physical_copy_marking_and_summary_count_match_the_python_reference() {
    common::with_utc_tz(|| {
        let root = common::root("prune-dry-run-last-physical-copy-parity");
        common::seed_observer_owning_stream(&root, "abcdefgh", STREAM);

        let audio = b"same bytes";
        let sha = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(audio))
        };
        let canonical = common::segment_dir(&root, DAY, STREAM, "090000_300");
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
        common::write_segment(
            &root,
            DAY,
            STREAM,
            "090000_301",
            2,
            Some("090000_300"),
            audio,
        );

        let days = vec![DAY.to_owned()];
        let rust_result = run_prune(&root, &days, Some(STREAM), false, 1_000);
        let rust_text = format_result(&rust_result);

        let oracle = common::oracle_prune(&root, &days, Some(STREAM), false);
        let oracle_text = oracle["stdout"].as_str().expect("oracle stdout");

        assert_eq!(rust_text, oracle_text);
        assert!(rust_text.contains("last-physical-copy: 1"));

        common::cleanup(root);
    });
}
