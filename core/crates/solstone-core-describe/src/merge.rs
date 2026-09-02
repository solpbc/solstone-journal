// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Validation and planning for describe re-entry merges.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::extraction;
use crate::hash::format_dhash;

#[derive(Clone, Debug)]
pub(crate) struct ExistingDescribeRow {
    pub(crate) data: Value,
    pub(crate) raw_line: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ExistingDescribeArtifact {
    pub(crate) header: Value,
    pub(crate) record: Value,
    pub(crate) rows: Option<Vec<ExistingDescribeRow>>,
}

#[derive(Clone, Debug)]
pub(crate) struct IncrementalMergePlan {
    pub(crate) reusable_rows: BTreeMap<u64, ExistingDescribeRow>,
    pub(crate) phase1_gap_ids: BTreeSet<u64>,
    pub(crate) phase3_gaps: BTreeMap<u64, (Value, Vec<&'static str>)>,
}

/// Read a complete prior describe artifact after re-entry has already been selected.
pub(crate) fn read_existing_describe_artifact(path: &Path) -> Option<ExistingDescribeArtifact> {
    let contents = fs::read_to_string(path).ok()?;
    let mut lines = contents.split_inclusive('\n');
    let header_line = lines.next()?;
    let header: Value = serde_json::from_str(header_line).ok()?;
    if !header.is_object() {
        return None;
    }
    let record = header.get("_solstone_processing")?.as_object()?.clone();

    let mut rows = Vec::new();
    for raw_line in lines {
        let Ok(data) = serde_json::from_str::<Value>(raw_line) else {
            return Some(ExistingDescribeArtifact {
                header,
                record: Value::Object(record),
                rows: None,
            });
        };
        if !data.is_object() {
            return Some(ExistingDescribeArtifact {
                header,
                record: Value::Object(record),
                rows: None,
            });
        }
        rows.push(ExistingDescribeRow {
            data,
            raw_line: raw_line.to_owned(),
        });
    }
    Some(ExistingDescribeArtifact {
        header,
        record: Value::Object(record),
        rows: Some(rows),
    })
}

/// Validate a prior artifact against freshly decoded frames and classify reusable work.
pub(crate) fn build_incremental_merge_plan(
    artifact: Option<&ExistingDescribeArtifact>,
    qualified_ids: &BTreeSet<u64>,
    current_input_size: u64,
    current_first_hash: Option<u64>,
    current_last_hash: Option<u64>,
    current_qualified_count: usize,
) -> Option<IncrementalMergePlan> {
    let artifact = artifact?;
    let rows = artifact.rows.as_ref()?;
    let current_first_hash = current_first_hash?;
    let expected_first_hash = Value::String(format_dhash(current_first_hash));
    if artifact.header.get("first_hash") != Some(&expected_first_hash) {
        return None;
    }
    let expected_last_hash = match current_last_hash {
        Some(hash) => Value::String(format_dhash(hash)),
        None => Value::Null,
    };
    if artifact
        .header
        .get("last_hash")
        .cloned()
        .unwrap_or(Value::Null)
        != expected_last_hash
    {
        return None;
    }
    if artifact
        .header
        .get("qualified_count")
        .and_then(Value::as_u64)
        != u64::try_from(current_qualified_count).ok()
    {
        return None;
    }
    if artifact.record.get("input_size").and_then(Value::as_u64) != Some(current_input_size) {
        return None;
    }

    let mut reusable_rows = BTreeMap::new();
    let mut phase1_gap_ids = BTreeSet::new();
    let mut phase3_gaps = BTreeMap::new();
    let mut seen_ids = BTreeSet::new();

    for existing_row in rows {
        let row = &existing_row.data;
        let frame_id = row.get("frame_id").and_then(Value::as_u64)?;
        if !qualified_ids.contains(&frame_id) || !seen_ids.insert(frame_id) {
            return None;
        }
        let enhanced = row.get("enhanced").and_then(Value::as_bool)?;
        let Some(analysis) = row.get("analysis").filter(|analysis| analysis.is_object()) else {
            phase1_gap_ids.insert(frame_id);
            continue;
        };

        let has_error = row.get("error").is_some();
        if !has_error && !enhanced {
            reusable_rows.insert(frame_id, existing_row.clone());
            continue;
        }

        let expected = extraction::categories_for_analysis(analysis)
            .into_iter()
            .map(|category| category.name)
            .collect::<BTreeSet<_>>();
        if enhanced {
            let content = row.get("content").and_then(Value::as_object)?;
            let missing = expected
                .iter()
                .filter(|category| !content.contains_key::<str>(*category))
                .copied()
                .collect::<Vec<_>>();
            if !has_error && missing.is_empty() {
                reusable_rows.insert(frame_id, existing_row.clone());
                continue;
            }
            if !missing.is_empty() {
                if !row.get("requests").is_some_and(Value::is_array) {
                    return None;
                }
                row.get("timestamp").and_then(Value::as_f64)?;
                phase3_gaps.insert(frame_id, (row.clone(), missing));
                continue;
            }
        }
        return None;
    }

    if seen_ids != *qualified_ids {
        return None;
    }
    Some(IncrementalMergePlan {
        reusable_rows,
        phase1_gap_ids,
        phase3_gaps,
    })
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use serde_json::{Value, json};

    use super::{
        ExistingDescribeArtifact, ExistingDescribeRow, build_incremental_merge_plan,
        read_existing_describe_artifact,
    };

    const FIRST_HASH: u64 = 0x1111;
    const LAST_HASH: u64 = 0x2222;

    fn qualified_ids() -> BTreeSet<u64> {
        [1].into_iter().collect()
    }

    fn valid_row() -> ExistingDescribeRow {
        ExistingDescribeRow {
            data: json!({
                "frame_id": 1,
                "timestamp": 0.0,
                "requests": [],
                "analysis": {"primary":"code", "secondary":"none", "overlap":true},
                "enhanced": false,
            }),
            raw_line: "{\"frame_id\":1}\n".to_owned(),
        }
    }

    fn valid_artifact() -> ExistingDescribeArtifact {
        ExistingDescribeArtifact {
            header: json!({
                "first_hash": "0000000000001111",
                "last_hash": "0000000000002222",
                "qualified_count": 1,
            }),
            record: json!({"input_size": 10}),
            rows: Some(vec![valid_row()]),
        }
    }

    fn plan(artifact: Option<&ExistingDescribeArtifact>) -> Option<super::IncrementalMergePlan> {
        build_incremental_merge_plan(
            artifact,
            &qualified_ids(),
            10,
            Some(FIRST_HASH),
            Some(LAST_HASH),
            1,
        )
    }

    #[test]
    fn valid_clean_row_is_reusable() {
        let artifact = valid_artifact();
        let plan = plan(Some(&artifact)).expect("valid plan");
        assert_eq!(plan.reusable_rows.len(), 1);
        assert!(plan.phase1_gap_ids.is_empty());
        assert!(plan.phase3_gaps.is_empty());
    }

    #[test]
    fn missing_last_hash_matches_a_missing_current_last_hash() {
        let mut artifact = valid_artifact();
        artifact.header.as_object_mut().unwrap().remove("last_hash");
        assert!(
            build_incremental_merge_plan(
                Some(&artifact),
                &qualified_ids(),
                10,
                Some(FIRST_HASH),
                None,
                1,
            )
            .is_some()
        );
    }

    #[test]
    fn bail_out_cascade_rejects_each_invalid_shape() {
        #[derive(Clone, Copy)]
        enum Case {
            AbsentArtifact,
            UnreadableRows,
            MissingFirstHash,
            FirstHash,
            LastHash,
            QualifiedCount,
            InputSize,
            InvalidFrameId,
            OutsideFrameId,
            DuplicateFrameId,
            InvalidEnhanced,
            InvalidContent,
            InvalidRequests,
            InvalidTimestamp,
            ErrorUnenhanced,
            ErrorCompleteEnhanced,
            MissingFinalId,
        }
        let cases = [
            ("absent artifact", Case::AbsentArtifact),
            ("unreadable rows", Case::UnreadableRows),
            ("missing first hash", Case::MissingFirstHash),
            ("first hash", Case::FirstHash),
            ("last hash", Case::LastHash),
            ("qualified count", Case::QualifiedCount),
            ("input size", Case::InputSize),
            ("invalid frame id", Case::InvalidFrameId),
            ("outside frame id", Case::OutsideFrameId),
            ("duplicate frame id", Case::DuplicateFrameId),
            ("invalid enhanced", Case::InvalidEnhanced),
            ("invalid content", Case::InvalidContent),
            ("invalid requests", Case::InvalidRequests),
            ("invalid timestamp", Case::InvalidTimestamp),
            ("error unenhanced", Case::ErrorUnenhanced),
            ("error complete enhanced", Case::ErrorCompleteEnhanced),
            ("missing final id", Case::MissingFinalId),
        ];

        for (name, case) in cases {
            assert!(
                plan(Some(&valid_artifact())).is_some(),
                "{name} positive control"
            );
            let mut artifact = valid_artifact();
            let mut ids = qualified_ids();
            let mut first_hash = Some(FIRST_HASH);
            let current_count = 1;
            match case {
                Case::AbsentArtifact => {
                    assert!(plan(None).is_none(), "{name}");
                    continue;
                }
                Case::UnreadableRows => artifact.rows = None,
                Case::MissingFirstHash => first_hash = None,
                Case::FirstHash => artifact.header["first_hash"] = json!("different"),
                Case::LastHash => artifact.header["last_hash"] = json!("different"),
                Case::QualifiedCount => artifact.header["qualified_count"] = json!(2),
                Case::InputSize => artifact.record["input_size"] = json!(11),
                Case::InvalidFrameId => {
                    artifact.rows.as_mut().unwrap()[0].data["frame_id"] = json!(true)
                }
                Case::OutsideFrameId => {
                    artifact.rows.as_mut().unwrap()[0].data["frame_id"] = json!(2)
                }
                Case::DuplicateFrameId => artifact.rows.as_mut().unwrap().push(valid_row()),
                Case::InvalidEnhanced => {
                    artifact.rows.as_mut().unwrap()[0].data["enhanced"] = json!("false")
                }
                Case::InvalidContent => {
                    let row = &mut artifact.rows.as_mut().unwrap()[0].data;
                    row["enhanced"] = json!(true);
                    row["content"] = json!("not an object");
                }
                Case::InvalidRequests => {
                    let row = &mut artifact.rows.as_mut().unwrap()[0].data;
                    row["enhanced"] = json!(true);
                    row["content"] = json!({});
                    row["requests"] = json!({});
                }
                Case::InvalidTimestamp => {
                    let row = &mut artifact.rows.as_mut().unwrap()[0].data;
                    row["enhanced"] = json!(true);
                    row["content"] = json!({});
                    row["timestamp"] = json!(true);
                }
                Case::ErrorUnenhanced => {
                    artifact.rows.as_mut().unwrap()[0].data["error"] = json!("boom")
                }
                Case::ErrorCompleteEnhanced => {
                    let row = &mut artifact.rows.as_mut().unwrap()[0].data;
                    row["enhanced"] = json!(true);
                    row["content"] = json!({"code":"complete"});
                    row["error"] = json!("boom");
                }
                Case::MissingFinalId => {
                    ids.insert(2);
                }
            }
            assert!(
                build_incremental_merge_plan(
                    Some(&artifact),
                    &ids,
                    10,
                    first_hash,
                    Some(LAST_HASH),
                    current_count,
                )
                .is_none(),
                "{name}"
            );
        }
    }

    #[test]
    fn expected_categories_ignore_stored_request_categories() {
        let mut artifact = valid_artifact();
        let row = &mut artifact.rows.as_mut().unwrap()[0].data;
        row["enhanced"] = json!(true);
        row["content"] = json!({});
        row["requests"] = json!([{"category":"messaging"}]);
        let plan = plan(Some(&artifact)).expect("valid phase-three gap");
        assert_eq!(plan.phase3_gaps[&1].1, vec!["code"]);
    }

    #[test]
    fn artifact_reader_preserves_raw_lines_and_marks_bad_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("artifact.jsonl");
        let raw = "{\"_solstone_processing\":{}}\n{\"z\": \"café\", \"frame_id\": 1}\n";
        fs::write(&path, raw).expect("write artifact");
        let artifact = read_existing_describe_artifact(&path).expect("artifact");
        assert_eq!(
            artifact.rows.as_ref().unwrap()[0].raw_line,
            "{\"z\": \"café\", \"frame_id\": 1}\n"
        );

        fs::write(&path, "{\"_solstone_processing\":{}}\nnot json\n").expect("write bad artifact");
        assert!(
            read_existing_describe_artifact(&path)
                .unwrap()
                .rows
                .is_none()
        );
    }

    #[test]
    fn mismatched_decode_hashes_prevent_cross_video_reuse() {
        let mut artifact = valid_artifact();
        artifact.header["first_hash"] = json!("0000000000009999");
        assert!(plan(Some(&artifact)).is_none());
    }

    #[test]
    fn phase_one_gaps_are_retried_without_rejecting_the_plan() {
        let mut artifact = valid_artifact();
        artifact.rows.as_mut().unwrap()[0].data["analysis"] = Value::Null;
        let plan = plan(Some(&artifact)).expect("phase-one gap plan");
        assert_eq!(plan.phase1_gap_ids, [1].into_iter().collect());
    }
}
