// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Warning notices emitted when re-importing under `--force` across differing calendar days.

use std::io::{self, Write};

use crate::timestamp::Timestamp;

/// Describe the actual planned day without changing the allocator's re-import rules.
pub fn force_day_notice(
    fresh: &Timestamp,
    stored_source: &Timestamp,
    planned: &Timestamp,
) -> Option<String> {
    if fresh.day() == stored_source.day() {
        return None;
    }
    Some(format!(
        "{}\n",
        solstone_core_cli_copy::display(
            "journal.cli.importer.force.day.notice",
            [
                ("fresh", &fresh.day()),
                ("stored", &stored_source.day()),
                ("planned", &planned.day()),
            ],
        )
    ))
}

/// Flush the notice to stderr if `json_mode`, otherwise stdout.
pub fn flush_force_notice<W1: Write, W2: Write>(
    json_mode: bool,
    notice: &str,
    stdout: &mut W1,
    stderr: &mut W2,
) -> io::Result<()> {
    if json_mode {
        stderr.write_all(notice.as_bytes())?;
        stderr.flush()
    } else {
        stdout.write_all(notice.as_bytes())?;
        stdout.flush()
    }
}

/// Flush the notice before invoking a mutation callback. If write or flush fails, the callback is not called.
pub fn flush_force_notice_then<W1: Write, W2: Write, R, F>(
    json_mode: bool,
    notice: Option<&str>,
    stdout: &mut W1,
    stderr: &mut W2,
    mutate: F,
) -> io::Result<R>
where
    F: FnOnce() -> R,
{
    if let Some(text) = notice {
        flush_force_notice(json_mode, text, stdout, stderr)?;
    }
    Ok(mutate())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceHash;
    use crate::allocate::stored_source_timestamp_for_hash;
    use crate::timestamp::validate_timestamp;
    use std::fs;
    use std::io;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "simulated write failure",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "simulated flush failure",
            ))
        }
    }

    #[test]
    fn force_day_notice_matches_and_differs() {
        let ts1 = validate_timestamp("20260101_120000").unwrap();
        let ts2 = validate_timestamp("20260101_180000").unwrap();
        let ts3 = validate_timestamp("20260201_120000").unwrap();

        assert_eq!(force_day_notice(&ts1, &ts2, &ts1), None);

        let notice = force_day_notice(&ts3, &ts1, &ts3).unwrap();
        assert!(notice.contains("20260201"));
        assert!(notice.contains("20260101"));
        assert!(notice.contains("this --force import will use"));
    }

    #[test]
    fn flush_force_notice_destinations() {
        let notice = "test notice\n";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        flush_force_notice(false, notice, &mut stdout, &mut stderr).unwrap();
        assert_eq!(stdout, notice.as_bytes());
        assert!(stderr.is_empty());

        stdout.clear();
        stderr.clear();

        flush_force_notice(true, notice, &mut stdout, &mut stderr).unwrap();
        assert!(stdout.is_empty());
        assert_eq!(stderr, notice.as_bytes());
    }

    #[test]
    fn flush_then_executes_mutation_on_success() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut executed = false;

        let result =
            flush_force_notice_then(false, Some("notice"), &mut stdout, &mut stderr, || {
                executed = true;
                42
            });

        assert_eq!(result.unwrap(), 42);
        assert!(executed);
        assert_eq!(stdout, b"notice");
    }

    #[test]
    fn flush_then_fails_and_does_not_mutate_on_error() {
        let mut stdout = FailingWriter;
        let mut stderr = Vec::new();
        let mut executed = false;

        let result =
            flush_force_notice_then(false, Some("notice"), &mut stdout, &mut stderr, || {
                executed = true;
                42
            });

        assert!(result.is_err());
        assert!(!executed, "callback must not be invoked on flush failure");
    }

    #[test]
    fn day_selection_uses_stored_source_day_not_shifted_directory_id() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path();
        let shifted_dir_id = "20260101_120005";
        let original_source_ts = "20260101_120000";
        let fresh_ts_str = "20260201_140000";

        let import_dir = journal.join("imports").join(shifted_dir_id);
        fs::create_dir_all(&import_dir).unwrap();
        let hash = SourceHash::new(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
        );
        let meta = serde_json::json!({
            "import_id": shifted_dir_id,
            "source_timestamp": original_source_ts,
            "source_hash": hash.as_str(),
        });
        fs::write(
            import_dir.join("import.json"),
            serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();

        let stored = stored_source_timestamp_for_hash(journal, &hash)
            .unwrap()
            .expect("stored record exists");
        assert_eq!(stored.as_str(), original_source_ts);

        let fresh = validate_timestamp(fresh_ts_str).unwrap();
        let notice = force_day_notice(&fresh, &stored, &stored).unwrap();

        assert!(notice.contains("20260201"), "notice must contain fresh day");
        assert!(
            notice.contains("20260101"),
            "notice must contain stored source day"
        );
        assert!(
            !notice.contains(shifted_dir_id),
            "notice must not contain directory id"
        );
        assert!(notice.contains("this --force import will use"));
    }
}
