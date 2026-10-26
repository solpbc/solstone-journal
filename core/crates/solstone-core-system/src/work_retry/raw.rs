// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native analysis retry scope. A queued permission is rechecked and consumed
//! only by the matching native writer under its existing OutputClaims.

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Attempt, DECLARED_FIXES, Unit};

pub const JOURNAL_ENV: &str = "SOLSTONE_RETRY_JOURNAL";
pub const UNIT_ENV: &str = "SOLSTONE_RETRY_UNIT";
pub const CAUSE_ENV: &str = "SOLSTONE_RETRY_CAUSE";
pub const ATTEMPT_ENV: &str = "SOLSTONE_RETRY_ATTEMPT";

#[derive(Clone, Debug, PartialEq)]
pub struct RetryContext {
    pub unit_id: String,
    pub attempt: Attempt,
}

pub struct Evidence {
    pub unit: Unit,
    pub reason: String,
    pub input: String,
    pub baseline: String,
    pub input_path: PathBuf,
}

fn component(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', '\0'])
}

pub fn unit_for_output(journal: &Path, output: &Path, handler: &str) -> Result<Unit, String> {
    let relative = output
        .strip_prefix(journal.join("chronicle"))
        .map_err(|e| e.to_string())?;
    let parts = relative
        .iter()
        .map(|p| p.to_str().ok_or("non-UTF8 retry coordinate"))
        .collect::<Result<Vec<_>, _>>()?;
    let (day, stream, segment, output, direct) = match parts.as_slice() {
        [day, segment, output] => (
            *day,
            solstone_core_journal_io::DEFAULT_STREAM,
            *segment,
            *output,
            true,
        ),
        [day, stream, segment, output] => (*day, *stream, *segment, *output, false),
        _ => return Err("invalid native retry coordinate".into()),
    };
    let unit = Unit::Raw {
        day: day.into(),
        stream: stream.into(),
        segment: segment.into(),
        handler: handler.into(),
        output: output.into(),
        direct,
    };
    unit.validate()?;
    Ok(unit)
}

/// The header stays bounded and existing-file read errors remain errors,
/// never an absent processing record that could manufacture a fresh attempt.
fn header(output: &Path) -> Result<Value, String> {
    bounded_header(output)?.ok_or_else(|| "native processing header is incomplete".into())
}

/// `None` when the first row fills the bound: like any sidecar, that file has no
/// readable record. A shorter row without its newline is still an error.
fn bounded_header(output: &Path) -> Result<Option<Value>, String> {
    let bound = solstone_core_processing_record::vocab::MAX_FIRST_ROW_BYTES;
    let file = match std::fs::File::open(output) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut reader = std::io::BufReader::new(file.take(bound as u64));
    let mut row = Vec::new();
    reader
        .read_until(b'\n', &mut row)
        .map_err(|e| e.to_string())?;
    if row.last() != Some(&b'\n') {
        if row.len() == bound {
            return Ok(None);
        }
        return Err("native processing header is incomplete".into());
    }
    let header: Value = serde_json::from_slice(&row).map_err(|e| e.to_string())?;
    if !header.is_object() {
        return Err("native processing header is not an object".into());
    }
    Ok(Some(header))
}

pub fn inspect_output(journal: &Path, output: &Path) -> Result<Option<Evidence>, String> {
    inspect_output_checked(journal, output, &|| Ok(()))
}

pub fn inspect_output_checked(
    journal: &Path,
    output: &Path,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<Evidence>, String> {
    let Some(header) = bounded_header(output)? else {
        return Ok(None);
    };
    let Some(record) = header.get("_solstone_processing") else {
        return Ok(None);
    };
    if !record.is_object() {
        return Err("native processing record is malformed".into());
    }
    if record.get("schema").and_then(Value::as_str)
        != Some(solstone_core_processing_record::vocab::SCHEMA)
    {
        return Err("native processing schema is unrecognized".into());
    }
    if record.get("state").and_then(Value::as_str) != Some("failed")
        || !solstone_core_processing_record::is_failure_exhausted(record)
    {
        return Ok(None);
    }
    let handler = record
        .get("handler")
        .and_then(Value::as_str)
        .ok_or("native failure has no handler")?;
    let unit = unit_for_output(journal, output, handler)?;
    let raw = header
        .get("raw")
        .and_then(Value::as_str)
        .filter(|s| component(s))
        .ok_or("native failure has no safe source filename")?;
    let input_path = output
        .parent()
        .ok_or("native output has no parent")?
        .join(raw);
    if input_path.with_extension("jsonl") != output {
        return Err("native failure source does not match its output".into());
    }
    let input = super::file_fingerprint_checked(&input_path, checkpoint)?;
    let baseline = match record.get("input_sha256") {
        None => input.clone(),
        Some(Value::String(value))
            if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            value.clone()
        }
        Some(_) => return Err("native failure input fingerprint is malformed".into()),
    };
    let reason = record
        .get("reason_code")
        .and_then(Value::as_str)
        .unwrap_or("analysis_failed")
        .to_owned();
    Ok(Some(Evidence {
        unit,
        reason,
        input,
        baseline,
        input_path,
    }))
}

pub fn observe(journal: &Path, evidence: &Evidence) -> Result<(), String> {
    observe_checked(journal, evidence, &|| Ok(()))
}

pub fn observe_checked(
    journal: &Path,
    evidence: &Evidence,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    super::observe_held_checked(
        journal,
        &evidence.unit,
        &evidence.reason,
        &evidence.baseline,
        checkpoint,
    )
}

fn selected(
    journal: &Path,
    output: &Path,
    handler: &str,
) -> Result<Option<(Unit, Attempt)>, String> {
    let Some(id) = std::env::var_os(UNIT_ENV) else {
        return Ok(None);
    };
    if std::env::var_os(JOURNAL_ENV).as_deref() != Some(journal.as_os_str()) {
        return Err("native retry journal does not match".into());
    }
    let id = id.to_str().ok_or("native retry identity is not UTF-8")?;
    let cause = std::env::var(CAUSE_ENV).map_err(|e| e.to_string())?;
    let selected: Attempt =
        serde_json::from_str(&std::env::var(ATTEMPT_ENV).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let unit = unit_for_output(journal, output, handler)?;
    if unit.id()? != id {
        return Ok(None);
    }
    let evidence = inspect_output(journal, output)?.ok_or("native output is no longer held")?;
    if evidence.unit != unit {
        return Err("native failure handler changed".into());
    }
    let record = super::read(journal, id)?.ok_or("native retry permission is missing")?;
    let attempt = record
        .permission(&evidence.input, DECLARED_FIXES)
        .filter(|attempt| {
            attempt == &selected && attempt.cause == cause && attempt.reason == evidence.reason
        })
        .ok_or("native retry permission changed")?;
    Ok(Some((unit, attempt)))
}

/// Read-only eligibility for an output. None when not an exhausted held failure or permission() is None.
pub fn context_for_output(
    journal: &Path,
    output: &Path,
    handler: &str,
) -> Result<Option<RetryContext>, String> {
    let unit = unit_for_output(journal, output, handler)?;
    let evidence = match inspect_output(journal, output)? {
        Some(e) => e,
        None => return Ok(None),
    };
    if evidence.unit != unit {
        return Ok(None);
    }
    let record = match super::read(journal, &unit.id()?)? {
        Some(r) => r,
        None => return Ok(None),
    };
    let attempt = match record
        .permission(&evidence.input, DECLARED_FIXES)
        .filter(|attempt| attempt.reason == evidence.reason)
    {
        Some(a) => a,
        None => return Ok(None),
    };
    Ok(Some(RetryContext {
        unit_id: unit.id()?,
        attempt,
    }))
}

/// Preflight with explicit context. True only when unit id matches and permission is still current.
pub fn waiting_context(
    journal: &Path,
    output: &Path,
    handler: &str,
    context: &RetryContext,
) -> bool {
    let Ok(unit) = unit_for_output(journal, output, handler) else {
        return false;
    };
    let Ok(id) = unit.id() else {
        return false;
    };
    if id != context.unit_id {
        return false;
    }
    context_for_output(journal, output, handler)
        .is_ok_and(|c| c.as_ref().map(|c| &c.attempt) == Some(&context.attempt))
}

/// Preflight only. The native writer repeats this under OutputClaims.
pub fn waiting_from_environment(output: &Path, handler: &str) -> bool {
    let Some(journal) = std::env::var_os(JOURNAL_ENV) else {
        return false;
    };
    selected(Path::new(&journal), output, handler).is_ok_and(|grant| grant.is_some())
}

/// Never call before acquiring the native output execution claim.
pub fn consume_context(
    journal: &Path,
    output: &Path,
    handler: &str,
    context: &RetryContext,
) -> Result<Option<Attempt>, String> {
    consume_context_checked(journal, output, handler, context, &|| Ok(()))
}

pub fn consume_context_checked(
    journal: &Path,
    output: &Path,
    handler: &str,
    context: &RetryContext,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Option<Attempt>, String> {
    checkpoint()?;
    let unit = unit_for_output(journal, output, handler)?;
    if unit.id()? != context.unit_id {
        return Ok(None);
    }
    let evidence = inspect_output(journal, output)?.ok_or("native output is no longer held")?;
    if evidence.unit != unit {
        return Err("native failure handler changed".into());
    }
    super::consume_checked(
        journal,
        &unit,
        &context.attempt,
        &evidence.reason,
        &evidence.input,
        DECLARED_FIXES,
        checkpoint,
    )?;
    Ok(Some(context.attempt.clone()))
}

/// Never call before acquiring the native output execution claim.
pub fn consume_from_environment(
    journal: &Path,
    output: &Path,
    handler: &str,
) -> Result<bool, String> {
    let Some((unit, attempt)) = selected(journal, output, handler)? else {
        return Ok(false);
    };
    super::consume(
        journal,
        &unit,
        &attempt,
        &attempt.reason,
        &attempt.input,
        DECLARED_FIXES,
    )?;
    Ok(true)
}

pub fn consume_for_output(output: &Path, handler: &str) -> Result<bool, String> {
    let Some(journal) = std::env::var_os(JOURNAL_ENV) else {
        return Ok(false);
    };
    consume_from_environment(Path::new(&journal), output, handler)
}

pub fn retry_attempt_from_environment() -> Result<Option<Attempt>, String> {
    match std::env::var(ATTEMPT_ENV) {
        Ok(value) => serde_json::from_str(&value)
            .map(Some)
            .map_err(|e| e.to_string()),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Called by the owning native publication writer before saving its failed
/// processing record, while its existing input observation still protects it.
pub fn stamp_failed_input(record: &mut Value, input: &Path) -> Result<(), String> {
    stamp_attempt(record)?;
    if record.get("state").and_then(Value::as_str) == Some("failed") {
        let hash = super::file_fingerprint(input)?;
        record
            .as_object_mut()
            .ok_or("processing record is not an object")?
            .insert("input_sha256".into(), Value::String(hash));
    }
    Ok(())
}

pub fn stamp_failed_input_from(
    record: &mut Value,
    input: &Path,
    attempt: &Attempt,
) -> Result<(), String> {
    stamp_attempt_from(record, attempt)?;
    if record.get("state").and_then(Value::as_str) == Some("failed") {
        let hash = super::file_fingerprint(input)?;
        record
            .as_object_mut()
            .ok_or("processing record is not an object")?
            .insert("input_sha256".into(), Value::String(hash));
    }
    Ok(())
}

/// Bind terminal publication to the selected attempt, including successful output.
pub fn stamp_attempt(record: &mut Value) -> Result<(), String> {
    if let Ok(value) = std::env::var(ATTEMPT_ENV) {
        let attempt: Attempt = serde_json::from_str(&value).map_err(|e| e.to_string())?;
        record["retry_attempt"] = serde_json::to_value(attempt).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn stamp_attempt_from(record: &mut Value, attempt: &Attempt) -> Result<(), String> {
    record["retry_attempt"] = serde_json::to_value(attempt).map_err(|e| e.to_string())?;
    Ok(())
}

/// What a held attempt is allowed to do with the output as it is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldAttempt {
    /// The same consumed attempt still matches this input, reason, and failure.
    Continue,
    /// The writer already published this attempt's terminal receipt.
    Accepted,
    /// The held attempt is not this input, reason, or receipt.
    Decline,
}

/// Adjudicate a held attempt from the retry record and the current output.
///
/// Attempt identity is decided before the input is read, so a finished or
/// not-yet-consumed attempt does not hash the recording. Continue requires the
/// current fingerprint, the failure reason, and a consumed cause with no new
/// permission. Accepted requires the published receipt. Anything else declines.
pub fn classify_held_attempt(
    journal: &Path,
    output: &Path,
    input: &Path,
    handler: &str,
    context: &RetryContext,
) -> Result<HeldAttempt, String> {
    classify_held_attempt_checked(journal, output, input, handler, context, &|| Ok(()))
}

/// Same adjudication, polling `checkpoint` before each fingerprint chunk.
pub fn classify_held_attempt_checked(
    journal: &Path,
    output: &Path,
    input: &Path,
    handler: &str,
    context: &RetryContext,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<HeldAttempt, String> {
    let unit = unit_for_output(journal, output, handler)?;
    if unit.id()? != context.unit_id {
        return Ok(HeldAttempt::Decline);
    }
    let Some(record) = super::read(journal, &context.unit_id)? else {
        return Ok(HeldAttempt::Decline);
    };
    if !record.held || record.attempt.as_ref() != Some(&context.attempt) {
        return Ok(HeldAttempt::Decline);
    }
    let Some(header) = bounded_header(output)? else {
        return Ok(HeldAttempt::Decline);
    };
    let Some(processing) = header.get("_solstone_processing") else {
        return Ok(HeldAttempt::Decline);
    };
    if processing.get("schema").and_then(Value::as_str)
        != Some(solstone_core_processing_record::vocab::SCHEMA)
        || processing.get("handler").and_then(Value::as_str) != Some(handler)
    {
        return Ok(HeldAttempt::Decline);
    }
    let fingerprint = super::file_fingerprint_checked(input, checkpoint)?;
    if fingerprint != context.attempt.input || fingerprint != record.input {
        return Ok(HeldAttempt::Decline);
    }
    let attempt_value =
        serde_json::to_value(&context.attempt).map_err(|error| error.to_string())?;
    let state = processing.get("state").and_then(Value::as_str);
    if matches!(state, Some("analyzed" | "empty"))
        && processing.get("retry_attempt") == Some(&attempt_value)
    {
        return Ok(HeldAttempt::Accepted);
    }
    if state != Some("failed") {
        return Ok(HeldAttempt::Decline);
    }
    let header_reason = processing
        .get("reason_code")
        .and_then(Value::as_str)
        .unwrap_or("analysis_failed");
    if context.attempt.reason != record.reason || context.attempt.reason != header_reason {
        return Ok(HeldAttempt::Decline);
    }
    if !record.consumed.contains(&context.attempt.cause) {
        return Ok(HeldAttempt::Decline);
    }
    if record.permission(&fingerprint, DECLARED_FIXES).is_some() {
        return Ok(HeldAttempt::Decline);
    }
    let Some(evidence) = inspect_output_checked(journal, output, checkpoint)? else {
        return Ok(HeldAttempt::Decline);
    };
    if evidence.unit != unit
        || evidence.input != fingerprint
        || evidence.reason != context.attempt.reason
    {
        return Ok(HeldAttempt::Decline);
    }
    Ok(HeldAttempt::Continue)
}

/// A child exiting successfully is not a terminal receipt. Require a consumed
/// matching attempt and a recognized processing disposition from the writer.
pub fn finish_output(journal: &Path, unit: &Unit, attempt: &Attempt) -> Result<(), String> {
    finish_output_checked(journal, unit, attempt, &|| Ok(()))
}

/// Reconcile an actual terminal receipt only while the caller still admits publication.
pub fn finish_output_checked(
    journal: &Path,
    unit: &Unit,
    attempt: &Attempt,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    let Unit::Raw {
        output, handler, ..
    } = unit
    else {
        return Err("not native analysis".into());
    };
    let record = super::read(journal, &unit.id()?)?.ok_or("retry evidence is missing")?;
    if record.attempt.as_ref() != Some(attempt) {
        return Err("native writer did not consume the retry permission".into());
    }
    let output = unit.segment_directory(journal)?.join(output);
    let header = header(&output)?;
    let processing = header
        .get("_solstone_processing")
        .ok_or("native terminal receipt is missing")?;
    if processing.get("schema").and_then(Value::as_str)
        != Some(solstone_core_processing_record::vocab::SCHEMA)
        || processing.get("handler").and_then(Value::as_str) != Some(handler.as_str())
    {
        return Err("native terminal receipt changed".into());
    }
    if processing.get("retry_attempt")
        != Some(&serde_json::to_value(attempt).map_err(|e| e.to_string())?)
    {
        return Err("native writer has not published this retry's terminal receipt".into());
    }
    let remaining = match processing.get("state").and_then(Value::as_str) {
        Some("failed") => Some(
            processing
                .get("reason_code")
                .and_then(Value::as_str)
                .unwrap_or("analysis_failed"),
        ),
        Some("analyzed" | "empty") => None,
        _ => return Err("native terminal receipt is unreadable".into()),
    };
    super::finish_checked(journal, unit, attempt, remaining, checkpoint)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use serde_json::json;

    fn output(root: &Path) -> PathBuf {
        let segment = root.join("chronicle/20260101/audio/120000_300");
        std::fs::create_dir_all(&segment).unwrap();
        std::fs::write(segment.join("speech.wav"), b"accepted audio bytes").unwrap();
        segment.join("speech.jsonl")
    }

    fn failed(output: &Path, reason: &str) {
        let header = json!({"raw":"speech.wav", "_solstone_processing": {
            "schema": solstone_core_processing_record::vocab::SCHEMA,
            "state":"failed", "handler":"transcribe", "reason_code":reason,
            "attempts": 5, "first_failure_at":"2026-01-01T00:00:00Z",
            "last_failure_at":"2026-01-01T03:00:00Z"
        }});
        std::fs::write(output, format!("{header}\n")).unwrap();
    }

    #[test]
    fn cancelled_raw_completion_after_retry_record_lock_wait_and_positive_twin() {
        use solstone_core_journal_io::{LockOptions, hold_lock};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, mpsc};
        use std::time::Duration;
        for cancel in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let output = output(root.path());
            failed(&output, "service_unavailable");
            let evidence = inspect_output(root.path(), &output).unwrap().unwrap();
            observe(root.path(), &evidence).unwrap();
            super::super::request(root.path(), &evidence.unit).unwrap();
            let context = context_for_output(root.path(), &output, "transcribe")
                .unwrap()
                .unwrap();
            consume_context(root.path(), &output, "transcribe", &context)
                .unwrap()
                .unwrap();
            let receipt = serde_json::json!({"raw":"audio.wav","_solstone_processing":{
                "schema":"solstone.processing.v1","handler":"transcribe","state":"analyzed","retry_attempt":context.attempt
            }});
            std::fs::write(&output, format!("{receipt}\n")).unwrap();
            let path = super::super::path(root.path(), &context.unit_id).unwrap();
            let before = std::fs::read(&path).unwrap();
            let held = hold_lock(&path, LockOptions::default()).unwrap();
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = cancelled.clone();
            let journal = root.path().to_path_buf();
            let (started, ready) = mpsc::channel();
            let (checkpoint, reached) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let calls = AtomicUsize::new(0);
                finish_output_checked(&journal, &evidence.unit, &context.attempt, &|| {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        started.send(()).unwrap();
                    } else {
                        checkpoint.send(()).unwrap();
                    }
                    if flag.load(Ordering::SeqCst) {
                        Err("cancelled".into())
                    } else {
                        Ok(())
                    }
                })
            });
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(reached.recv_timeout(Duration::from_millis(100)).is_err());
            cancelled.store(cancel, Ordering::SeqCst);
            drop(held);
            let result = worker.join().unwrap();
            if cancel {
                assert_eq!(result.unwrap_err(), "cancelled");
                assert_eq!(std::fs::read(&path).unwrap(), before);
            } else {
                result.unwrap();
                let record =
                    super::super::read(root.path(), &path.file_stem().unwrap().to_string_lossy())
                        .unwrap()
                        .unwrap();
                assert!(record.attempt.is_none());
                assert!(!record.held);
            }
        }
    }
    #[test]
    fn fresh_admitted_audio_has_no_retry_context_without_a_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let output = output(root.path());
        assert_eq!(
            context_for_output(root.path(), &output, "transcribe").unwrap(),
            None
        );
        std::fs::write(&output, b"{broken\n").unwrap();
        assert!(context_for_output(root.path(), &output, "transcribe").is_err());
        std::fs::remove_file(&output).unwrap();
        std::fs::create_dir(&output).unwrap();
        assert!(context_for_output(root.path(), &output, "transcribe").is_err());
    }

    #[test]
    fn changed_failure_reason_refuses_stale_context_without_consuming_permission() {
        let root = tempfile::tempdir().unwrap();
        let output = output(root.path());
        failed(&output, "service_unavailable");
        let evidence = inspect_output(root.path(), &output)
            .unwrap()
            .expect("exhausted failure");
        observe(root.path(), &evidence).unwrap();
        super::super::request(root.path(), &evidence.unit).unwrap();
        let context = context_for_output(root.path(), &output, "transcribe")
            .unwrap()
            .unwrap();
        let before = super::super::read(root.path(), &context.unit_id)
            .unwrap()
            .unwrap();
        failed(&output, "corrupt_input");
        assert_eq!(
            context_for_output(root.path(), &output, "transcribe").unwrap(),
            None
        );
        assert!(consume_context(root.path(), &output, "transcribe", &context).is_err());
        let after = super::super::read(root.path(), &context.unit_id)
            .unwrap()
            .unwrap();
        assert_eq!(after.consumed, before.consumed);
        assert_eq!(after.attempt, before.attempt);
        failed(&output, "service_unavailable");
        assert_eq!(
            context_for_output(root.path(), &output, "transcribe").unwrap(),
            Some(context.clone())
        );
        assert_eq!(
            consume_context(root.path(), &output, "transcribe", &context).unwrap(),
            Some(context.attempt)
        );
        assert_eq!(
            context_for_output(root.path(), &output, "transcribe").unwrap(),
            None
        );
    }
}
