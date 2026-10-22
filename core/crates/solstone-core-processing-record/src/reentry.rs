// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded JSONL evidence reads and analysis-output re-entry decisions.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde_json::Value;

use crate::rows::{LeadingRow, read_leading_rows};
use crate::{analysis_row_key, is_failure_exhausted, vocab};

/// Branch-2 allowlist for recordless re-entry (describe | depict).
fn reenters_on_analysis_output(handler: &str) -> bool {
    matches!(handler, vocab::HANDLER_DESCRIBE | vocab::HANDLER_DEPICT)
}

fn reenters_failed_analysis_record(handler: &str) -> bool {
    reenters_on_analysis_output(handler) || handler == vocab::HANDLER_TRANSCRIBE
}

/// Read one bounded JSONL metadata header (the first row) as an object.
pub fn read_jsonl_header(path: &Path) -> Option<Value> {
    let mut file = File::open(path).ok()?;
    let mut window = Vec::with_capacity(vocab::MAX_FIRST_ROW_BYTES);
    file.by_ref()
        .take(vocab::MAX_FIRST_ROW_BYTES as u64)
        .read_to_end(&mut window)
        .ok()?;
    let newline = window.iter().position(|byte| *byte == b'\n')?;
    let header = std::str::from_utf8(&window[..newline]).ok()?;
    let header @ Value::Object(_) = serde_json::from_str(header).ok()? else {
        return None;
    };
    Some(header)
}

/// Read one bounded JSONL metadata header's processing record.
pub fn read_processing_record_header(path: &Path) -> Option<Value> {
    read_jsonl_header(path)?
        .get("_solstone_processing")
        .filter(|record| record.is_object())
        .cloned()
}

/// Return whether either of the first two nonblank JSONL rows carries `row_key`
/// as a top-level key.
///
/// Each row is read whole, in constant memory, so its length does not matter;
/// at most two rows are read ([`read_leading_rows`]). A row that is not one UTF-8
/// JSON object on its own line is no evidence, but still counts as one of the two.
pub fn jsonl_has_row_with_key(path: &Path, row_key: &str) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    read_leading_rows(BufReader::new(file), Some(row_key)).is_ok_and(|rows| {
        rows.iter()
            .any(|row| matches!(row, LeadingRow::Object { has_key: true, .. }))
    })
}

/// Return whether an existing analysis output should be retried by `handler`.
///
/// Transcribe re-enters failed, non-exhausted records via branch 1, and its
/// handler skip guards consult this predicate (`transcribe/src/args.rs:297`
/// `already_processed`). Branch 2 recordless re-entry remains describe and depict only.
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
            .is_some_and(reenters_failed_analysis_record)
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

    use super::{
        jsonl_has_row_with_key, read_processing_record_header, should_reenter_analysis_output,
    };
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
        assert!(should_reenter_analysis_output(
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

    /// A header row followed by one message row that alone runs past the old
    /// header-sized window.
    #[test]
    fn row_key_is_found_in_a_row_longer_than_the_header_bound() {
        let path = temporary_path("long-row");
        let text = "a".repeat(3 * vocab::MAX_FIRST_ROW_BYTES);
        write(
            &path,
            &format!(
                "{{\"imported\":{{\"id\":\"x\"}}}}\n{}\n",
                json!({"start": "00:00:00", "speaker": "Human", "text": text})
            ),
        );
        assert!(jsonl_has_row_with_key(
            &path,
            vocab::AUDIO_TRANSCRIPT_ROW_KEY
        ));
        assert!(!jsonl_has_row_with_key(&path, "missing"));
        fs::remove_file(path).expect("remove sidecar");
    }

    /// Short rows, but a multi-byte character straddles the old window's last byte.
    #[test]
    fn row_key_is_found_when_a_later_row_splits_a_character_at_the_bound() {
        let path = temporary_path("split-char");
        let bound = vocab::MAX_FIRST_ROW_BYTES;
        let mut contents = format!(
            "{{\"imported\":{{\"id\":\"x\"}}}}\n{}\n",
            json!({"start": "00:00:00", "text": "hi"})
        );
        let filler = format!(
            "{}\n",
            json!({"start": "00:00:01", "text": "b".repeat(1000)})
        );
        while contents.len() + filler.len() < bound - 2000 {
            contents.push_str(&filler);
        }
        // Pad the last row so its four-byte character begins two bytes before the bound.
        let lead = "{\"start\":\"00:00:02\",\"text\":\"";
        let pad = "c".repeat(bound - 2 - contents.len() - lead.len());
        contents.push_str(&format!("{lead}{pad}\u{1F600}\"}}\n"));
        assert!(contents.len() > bound);
        assert!(!contents.is_char_boundary(bound));
        write(&path, &contents);
        assert!(jsonl_has_row_with_key(
            &path,
            vocab::AUDIO_TRANSCRIPT_ROW_KEY
        ));
        fs::remove_file(path).expect("remove sidecar");
    }

    #[test]
    fn row_key_evidence_reads_only_the_first_two_nonblank_rows() {
        let path = temporary_path("rows");
        let cases: [(&str, bool); 9] = [
            ("\n  \n{}\n\n{\"start\":0}\n", true),
            ("{}\n{}\n{\"start\":0}\n", false),
            ("not-json\n{\"start\":0}\n", true),
            ("[\"start\"]\n{\"other\":{\"start\":0}}\n", false),
            ("{\"start\":0} trailing\n", false),
            ("{\"start\":0}{\"start\":1}\n", false),
            ("{\"start\":0,\"text\":\"cut", false),
            ("{\"text\":\"cut\n{\"start\":0}\n", true),
            ("{\"start\":0}", true),
        ];
        for (contents, expected) in cases {
            write(&path, contents);
            assert_eq!(
                jsonl_has_row_with_key(&path, vocab::AUDIO_TRANSCRIPT_ROW_KEY),
                expected,
                "{contents:?}"
            );
        }
        fs::remove_file(&path).expect("remove sidecar");
        assert!(!jsonl_has_row_with_key(
            &path,
            vocab::AUDIO_TRANSCRIPT_ROW_KEY
        ));
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
