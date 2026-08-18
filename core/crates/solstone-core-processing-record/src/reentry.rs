// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded JSONL evidence reads and analysis-output re-entry decisions.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::{analysis_row_key, is_failure_exhausted, vocab};

/// Transcribe does not re-enter because it already treats an existing sidecar as
/// done (`transcribe/src/args.rs:285-286` `already_processed`: `!redo &&
/// jsonl.exists()`) and never consults this predicate.
fn reenters_on_analysis_output(handler: &str) -> bool {
    matches!(handler, vocab::HANDLER_DESCRIBE | vocab::HANDLER_DEPICT)
}

/// Read one bounded JSONL metadata header's processing record.
pub fn read_processing_record_header(path: &Path) -> Option<Value> {
    let mut file = File::open(path).ok()?;
    let mut window = Vec::with_capacity(vocab::MAX_FIRST_ROW_BYTES);
    file.by_ref()
        .take(vocab::MAX_FIRST_ROW_BYTES as u64)
        .read_to_end(&mut window)
        .ok()?;
    let newline = window.iter().position(|byte| *byte == b'\n')?;
    let header = std::str::from_utf8(&window[..newline]).ok()?;
    let Value::Object(header) = serde_json::from_str(header).ok()? else {
        return None;
    };
    header
        .get("_solstone_processing")
        .filter(|record| record.is_object())
        .cloned()
}

/// Return whether either of the first two nonblank JSONL rows carries `row_key`.
///
/// This deliberately bounds Python's unbounded processing_record.py:92 probe to
/// one header-sized window. An unreadable or truncated window never manufactures
/// row evidence and therefore remains conservative for re-entry.
pub fn jsonl_has_row_with_key(path: &Path, row_key: &str) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut window = Vec::with_capacity(vocab::MAX_FIRST_ROW_BYTES);
    if file
        .by_ref()
        .take(vocab::MAX_FIRST_ROW_BYTES as u64)
        .read_to_end(&mut window)
        .is_err()
    {
        return false;
    }
    let Ok(text) = std::str::from_utf8(&window) else {
        return false;
    };
    text.split('\n')
        .filter(|line| !line.trim().is_empty())
        .take(2)
        .any(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|row| row.as_object().map(|row| row.contains_key(row_key)))
                .unwrap_or(false)
        })
}

/// Return whether an existing analysis output should be retried by `handler`.
pub fn should_reenter_analysis_output(
    record: Option<&Value>,
    output_path: &Path,
    handler: &str,
) -> bool {
    if let Some(record) = record
        && record.get("state").and_then(Value::as_str) == Some(vocab::STATE_FAILED)
        && record
            .get("handler")
            .and_then(Value::as_str)
            .is_some_and(reenters_on_analysis_output)
        && !is_failure_exhausted(record)
    {
        return true;
    }
    record.is_none()
        && reenters_on_analysis_output(handler)
        && analysis_row_key(handler).is_some_and(|key| !jsonl_has_row_with_key(output_path, key))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::{read_processing_record_header, should_reenter_analysis_output};
    use crate::test_support::reserve_temp_path;
    use crate::vocab;

    fn temporary_path(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-processing-record-reentry-{name}.jsonl"))
    }

    fn write(path: &Path, contents: &str) {
        fs::write(path, contents).expect("write sidecar");
    }

    #[test]
    fn retryable_describe_failures_reenter_at_the_strict_bound() {
        let path = temporary_path("attempts");
        write(&path, "{}\n");
        let retryable = json!({
            "state": vocab::STATE_FAILED,
            "handler": vocab::HANDLER_DESCRIBE,
            "attempts": 2,
        });
        assert!(should_reenter_analysis_output(
            Some(&retryable),
            &path,
            vocab::HANDLER_DESCRIBE
        ));

        let exhausted = json!({
            "state": vocab::STATE_FAILED,
            "handler": vocab::HANDLER_DESCRIBE,
            "attempts": 3,
        });
        assert!(!should_reenter_analysis_output(
            Some(&exhausted),
            &path,
            vocab::HANDLER_DESCRIBE
        ));

        let corrupt = json!({
            "state": vocab::STATE_FAILED,
            "handler": vocab::HANDLER_DESCRIBE,
            "reason_code": vocab::REASON_CORRUPT_INPUT,
            "attempts": 0,
        });
        assert!(!should_reenter_analysis_output(
            Some(&corrupt),
            &path,
            vocab::HANDLER_DESCRIBE
        ));

        let transcribe = json!({
            "state": vocab::STATE_FAILED,
            "handler": vocab::HANDLER_TRANSCRIBE,
            "attempts": 2,
        });
        assert!(!should_reenter_analysis_output(
            Some(&transcribe),
            &path,
            vocab::HANDLER_TRANSCRIBE
        ));
        fs::remove_file(path).expect("remove sidecar");
    }

    #[test]
    fn recordless_outputs_reenter_only_without_describe_row_evidence() {
        let path = temporary_path("recordless");
        write(&path, "{\"raw\":\"screen.webm\"}\n");
        assert!(should_reenter_analysis_output(
            None,
            &path,
            vocab::HANDLER_DESCRIBE
        ));

        write(
            &path,
            "{\"raw\":\"screen.webm\"}\n{\"frame_id\":1,\"timestamp\":0.0}\n",
        );
        assert!(!should_reenter_analysis_output(
            None,
            &path,
            vocab::HANDLER_DESCRIBE
        ));
        assert!(!should_reenter_analysis_output(
            None,
            &path,
            vocab::HANDLER_TRANSCRIBE
        ));
        fs::remove_file(path).expect("remove sidecar");
    }

    #[test]
    fn retryable_depict_failures_follow_the_same_bound() {
        let path = temporary_path("depict-attempts");
        write(&path, "{}\n");
        let retryable = json!({
            "state": vocab::STATE_FAILED,
            "handler": vocab::HANDLER_DEPICT,
            "attempts": 2,
        });
        assert!(should_reenter_analysis_output(
            Some(&retryable),
            &path,
            vocab::HANDLER_DEPICT
        ));

        let exhausted = json!({
            "state": vocab::STATE_FAILED,
            "handler": vocab::HANDLER_DEPICT,
            "attempts": 3,
        });
        assert!(!should_reenter_analysis_output(
            Some(&exhausted),
            &path,
            vocab::HANDLER_DEPICT
        ));

        let corrupt = json!({
            "state": vocab::STATE_FAILED,
            "handler": vocab::HANDLER_DEPICT,
            "reason_code": vocab::REASON_CORRUPT_INPUT,
            "attempts": 0,
        });
        assert!(!should_reenter_analysis_output(
            Some(&corrupt),
            &path,
            vocab::HANDLER_DEPICT
        ));
        fs::remove_file(path).expect("remove sidecar");
    }

    #[test]
    fn recordless_depict_outputs_reenter_only_without_text_row() {
        let path = temporary_path("depict-recordless");
        write(&path, "{\"raw\":\"photo.png\",\"kind\":\"image\"}\n");
        assert!(should_reenter_analysis_output(
            None,
            &path,
            vocab::HANDLER_DEPICT
        ));

        write(
            &path,
            "{\"raw\":\"photo.png\",\"kind\":\"image\"}\n{\"start\":\"00:00:00\",\"text\":\"caption\"}\n",
        );
        assert!(!should_reenter_analysis_output(
            None,
            &path,
            vocab::HANDLER_DEPICT
        ));
        fs::remove_file(path).expect("remove sidecar");
    }

    #[test]
    fn non_object_header_record_is_recordless() {
        let path = temporary_path("invalid-record");
        write(&path, "{\"_solstone_processing\":[]}\n");
        assert_eq!(read_processing_record_header(&path), None);
        assert!(should_reenter_analysis_output(
            read_processing_record_header(&path).as_ref(),
            &path,
            vocab::HANDLER_DESCRIBE
        ));
        fs::remove_file(path).expect("remove sidecar");
    }
}
