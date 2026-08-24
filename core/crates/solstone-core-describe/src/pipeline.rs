// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use solstone_core_generate::{GenerateResponse, RefusalReason, SessionCompletion};
use solstone_core_journal_io::{AtomicWriteOptions, install_file};
use solstone_core_processing_record::{
    read_processing_record_header, record_attempts, should_reenter_analysis_output, vocab,
};

use crate::decode::{QualifiedFrame, process_video_with_transform, resize_for_vlm_png};
use crate::detect;
use crate::extraction;
use crate::merge;
use crate::notify;
use crate::request;
use crate::selection::{self, CategorizedFrame, CategoryOverride, SelectionError};
use crate::session::{DescribeSession, DescribeSessionFactory, SystemSessionFactory};
use crate::{ConveyFiducialMask, WinnowConfig};

pub const EXIT_PROVIDER_BLOCKED: i32 = 69;
const MAX_ATTEMPTS: u64 = 5;

#[derive(Debug)]
pub enum RunError {
    Blocked(Option<String>),
    Internal(String),
}

struct Pending {
    frame: QualifiedFrame,
    attempt: u64,
}

struct Outstanding {
    pending: Pending,
    submitted_at: Instant,
}

struct CategorizedRow {
    frame_id: u64,
    timestamp: f64,
    png: Vec<u8>,
    analysis: Option<Value>,
    error: Option<String>,
    requests: Vec<Value>,
}

struct Promotion<'a> {
    output: &'a Path,
    rows: &'a Path,
    video: &'a Path,
    input_size: u64,
    decoded: &'a crate::DescribeResult,
    model: Option<String>,
    state: &'a str,
    reason: &'a str,
    previous_attempts: i64,
    carry_forward_observer: Option<&'a str>,
}

enum RowContent {
    Raw(String),
    Value(Value),
}

struct RowTemp {
    path: PathBuf,
    file: File,
}
impl RowTemp {
    fn new(parent: &Path) -> Result<Self, RunError> {
        for number in 0..1000_u32 {
            let path = parent.join(format!(
                ".describe-{}-{number}.jsonl.tmp",
                std::process::id()
            ));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok(Self { path, file }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(RunError::Internal(error.to_string())),
            }
        }
        Err(RunError::Internal(
            "could not allocate describe temp file".to_owned(),
        ))
    }
    fn row(&mut self, row: &Value) -> Result<(), RunError> {
        writeln!(self.file, "{row}").map_err(|error| RunError::Internal(error.to_string()))?;
        self.file
            .flush()
            .map_err(|error| RunError::Internal(error.to_string()))
    }
    fn raw_row(&mut self, raw_line: &str) -> Result<(), RunError> {
        self.file
            .write_all(raw_line.as_bytes())
            .map_err(|error| RunError::Internal(error.to_string()))?;
        self.file
            .flush()
            .map_err(|error| RunError::Internal(error.to_string()))
    }
}
impl Drop for RowTemp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct DescribeOptions<'a> {
    pub video: &'a Path,
    pub journal: &'a Path,
    pub explicit_journal: Option<&'a Path>,
    pub jobs: usize,
    pub redo: bool,
    pub config: WinnowConfig,
    pub redact_rules: Vec<String>,
    pub max_extractions: u32,
    pub category_overrides: BTreeMap<String, CategoryOverride>,
}

pub fn run(options: DescribeOptions<'_>) -> Result<(), RunError> {
    run_with_factory(options, &SystemSessionFactory)
}

pub fn run_with_factory(
    options: DescribeOptions<'_>,
    factory: &dyn DescribeSessionFactory,
) -> Result<(), RunError> {
    if options.jobs == 0 {
        return Err(RunError::Internal("--jobs must be positive".to_owned()));
    }
    let mut transform = ConveyFiducialMask;
    let decoded = process_video_with_transform(options.video, &mut transform, options.config);
    run_decoded(options, factory, decoded)
}

/// Run the describe pipeline after media decoding has completed.
///
/// Keeping this seam private lets pipeline tests use small, valid synthetic frame
/// data while the CLI integration suite continues to prove the native decode,
/// masking, child-session, detector, and command-line boundaries.
fn run_decoded(
    options: DescribeOptions<'_>,
    factory: &dyn DescribeSessionFactory,
    decoded: crate::DescribeResult,
) -> Result<(), RunError> {
    let output = options.video.with_extension("jsonl");
    let mut previous_attempts = 0;
    let mut incremental_source = None;
    if !options.redo && output.exists() {
        let record = read_processing_record_header(&output);
        if !should_reenter_analysis_output(record.as_ref(), &output, vocab::HANDLER_DESCRIBE) {
            return Ok(());
        }
        previous_attempts = record.as_ref().map(record_attempts).unwrap_or(0);
        incremental_source = Some(output.clone());
    }
    let start = Instant::now();
    let parent = output
        .parent()
        .ok_or_else(|| RunError::Internal("video has no parent".to_owned()))?;
    let input_size = fs::metadata(options.video)
        .map_err(|error| RunError::Internal(error.to_string()))?
        .len();
    let mut rows = RowTemp::new(parent)?;
    let existing_artifact = incremental_source
        .as_deref()
        .and_then(merge::read_existing_describe_artifact);
    let carry_forward_observer = existing_artifact.as_ref().and_then(|artifact| {
        if std::env::var("OBSERVER_NAME").is_ok_and(|value| !value.is_empty()) {
            return None;
        }
        artifact
            .header
            .get("observer")?
            .as_str()
            .filter(|observer| !observer.is_empty())
            .map(str::to_owned)
    });
    let qualified_ids = decoded
        .qualified_frames
        .iter()
        .map(|frame| frame.frame_id)
        .collect::<BTreeSet<_>>();
    let plan = merge::build_incremental_merge_plan(
        existing_artifact.as_ref(),
        &qualified_ids,
        input_size,
        decoded.first_hash,
        decoded.last_hash,
        decoded.qualified_count,
    );
    if decoded.qualified_frames.is_empty() {
        let (state, reason) = if decoded.decode_failed {
            (vocab::STATE_FAILED, vocab::REASON_CORRUPT_INPUT)
        } else {
            (vocab::STATE_EMPTY, vocab::REASON_NO_DECODABLE_FRAMES)
        };
        promote(Promotion {
            output: &output,
            rows: &rows.path,
            video: options.video,
            input_size,
            decoded: &decoded,
            model: None,
            state,
            reason,
            previous_attempts,
            carry_forward_observer: carry_forward_observer.as_deref(),
        })?;
        send_described(&options, &output, start);
        return Ok(());
    }
    let work_key = options
        .video
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("describe");
    let session = factory
        .spawn(options.jobs, options.explicit_journal)
        .map_err(|_| blocked(options.journal, work_key, None, None, None))?;
    let instruction = request::system_instruction(&options.redact_rules);
    let frame_pngs = decoded
        .qualified_frames
        .iter()
        .map(|frame| (frame.frame_id, frame.png.clone()))
        .collect::<HashMap<_, _>>();
    let mut incremental_rows = plan.as_ref().map(|_| BTreeMap::new());
    let mut final_rows = Vec::new();
    if let (Some(plan), Some(incremental_rows)) = (&plan, &mut incremental_rows) {
        for (frame_id, row) in &plan.reusable_rows {
            incremental_rows.insert(*frame_id, RowContent::Raw(row.raw_line.clone()));
            final_rows.push(row.data.clone());
        }
    }
    let mut waiting: VecDeque<Pending> = decoded
        .qualified_frames
        .iter()
        .filter(|frame| {
            plan.as_ref()
                .is_none_or(|plan| plan.phase1_gap_ids.contains(&frame.frame_id))
        })
        .cloned()
        .map(|frame| Pending { frame, attempt: 0 })
        .collect();
    let mut outstanding: HashMap<String, Outstanding> = HashMap::new();
    let mut categorized = Vec::new();
    let mut model = None;
    let window = options.jobs.saturating_mul(2);
    while !waiting.is_empty() || !outstanding.is_empty() {
        while outstanding.len() < window {
            let Some(pending) = waiting.pop_front() else {
                break;
            };
            let png = resize_for_vlm_png(&pending.frame.png, Some(1024)).ok_or_else(|| {
                RunError::Internal("failed to resize categorization frame".to_owned())
            })?;
            let req = request::request(pending.frame.frame_id, pending.attempt, &png, &instruction);
            let id = req.id.clone().expect("describe request ids are present");
            let submitted_at = Instant::now();
            if session.submit(req).is_err() {
                return Err(blocked(options.journal, work_key, None, None, None));
            }
            outstanding.insert(
                id,
                Outstanding {
                    pending,
                    submitted_at,
                },
            );
        }
        let completion = session
            .recv_timeout(Duration::from_secs(120))
            .map_err(|_| blocked(options.journal, work_key, None, None, None))?;
        let SessionCompletion::Response(response) = completion else {
            return Err(blocked(options.journal, work_key, None, None, None));
        };
        let id = match &response {
            GenerateResponse::Generated(g) => g.id.clone(),
            GenerateResponse::Refused(r) => r.id.clone(),
        }
        .ok_or_else(|| RunError::Internal("session response omitted id".to_owned()))?;
        let Outstanding {
            pending,
            submitted_at,
        } = outstanding
            .remove(&id)
            .ok_or_else(|| RunError::Internal("uncorrelated response".to_owned()))?;
        let duration = submitted_at.elapsed().as_secs_f64();
        match response {
            GenerateResponse::Generated(generated) => {
                model.get_or_insert(generated.model.clone());
                let parsed = serde_json::from_str::<Value>(&generated.text).ok();
                let error = parsed
                    .as_ref()
                    .and_then(|value| value.get("error"))
                    .is_some();
                let failed = error
                    || parsed.is_none()
                    || schema_validation_failed(generated.schema_validation.as_ref());
                let error_message = if failed {
                    Some("Invalid JSON response".to_owned())
                } else {
                    None
                };
                categorized.push(CategorizedRow {
                    frame_id: pending.frame.frame_id,
                    timestamp: pending.frame.timestamp,
                    png: pending.frame.png,
                    analysis: (!failed).then_some(parsed).flatten(),
                    error: error_message,
                    requests: vec![request_record(
                        "describe",
                        generated.model,
                        duration,
                        pending.attempt,
                        None,
                    )],
                });
            }
            GenerateResponse::Refused(refusal) => {
                let code = refusal.reason_code.as_ref().map(|value| value.as_wire());
                if refusal.reason == RefusalReason::NoEngineConfigured || refusal.blocking {
                    let _ = session.close();
                    return Err(blocked(
                        options.journal,
                        work_key,
                        code,
                        refusal.provider.as_deref(),
                        Some("observe.describe.frame"),
                    ));
                }
                if refusal.retryable && pending.attempt + 1 < MAX_ATTEMPTS {
                    waiting.push_back(Pending {
                        frame: pending.frame,
                        attempt: pending.attempt + 1,
                    });
                } else {
                    categorized.push(CategorizedRow {
                        frame_id: pending.frame.frame_id,
                        timestamp: pending.frame.timestamp,
                        png: pending.frame.png,
                        analysis: None,
                        error: Some(refusal.detail),
                        requests: vec![request_record(
                            "describe",
                            String::new(),
                            duration,
                            pending.attempt,
                            None,
                        )],
                    });
                }
            }
        }
    }
    let total_frames = categorized.len();
    let failed_frames = categorized.iter().filter(|row| row.error.is_some()).count();
    if total_frames > 0 && failed_frames == total_frames && plan.is_none() {
        let _ = session.close();
        promote(Promotion {
            output: &output,
            rows: &rows.path,
            video: options.video,
            input_size,
            decoded: &decoded,
            model,
            state: vocab::STATE_FAILED,
            reason: vocab::REASON_ANALYSIS_FAILED,
            previous_attempts,
            carry_forward_observer: carry_forward_observer.as_deref(),
        })?;
        return Err(RunError::Internal(
            "all describe frame requests failed".to_owned(),
        ));
    }
    let mut selection_frames = categorized
        .iter()
        .filter_map(|row| {
            row.analysis.as_ref().map(|analysis| CategorizedFrame {
                frame_id: row.frame_id,
                timestamp: row.timestamp,
                analysis: analysis.clone(),
            })
        })
        .collect::<Vec<_>>();
    selection_frames.sort_unstable_by_key(|frame| frame.frame_id);
    let selected = if selection_frames.is_empty() {
        Vec::new()
    } else {
        match selection::select(
            session.as_ref(),
            &selection_frames,
            options.max_extractions,
            &options.category_overrides,
        ) {
            Ok(selected) => selected,
            Err(SelectionError::Blocked {
                reason_code,
                provider,
            }) => {
                let _ = session.close();
                return Err(blocked(
                    options.journal,
                    work_key,
                    reason_code.as_deref(),
                    provider.as_deref(),
                    Some("observe.extract.selection"),
                ));
            }
            Err(SelectionError::Session) => {
                let _ = session.close();
                return Err(blocked(options.journal, work_key, None, None, None));
            }
        }
    };
    let selected = selected.into_iter().collect::<HashSet<_>>();
    let mut detection_disabled = false;
    for row in categorized {
        let mut result =
            json!({"frame_id":row.frame_id,"timestamp":row.timestamp,"requests":row.requests});
        if let Some(analysis) = &row.analysis {
            result["analysis"] = analysis.clone();
        }
        if let Some(error) = &row.error {
            result["error"] = json!(error);
        }
        let Some(analysis) = row.analysis.as_ref() else {
            result["enhanced"] = json!(false);
            emit_value(&mut rows, &mut incremental_rows, result.clone())?;
            final_rows.push(result);
            continue;
        };
        if let Some(detections) =
            maybe_detect(&mut detection_disabled, analysis, &row.png, options.journal)
        {
            result["detections"] = detections;
        }
        if !selected.contains(&row.frame_id) {
            result["enhanced"] = json!(false);
            emit_value(&mut rows, &mut incremental_rows, result.clone())?;
            final_rows.push(result);
            continue;
        }
        let categories = extraction::categories_for_analysis(analysis);
        if categories.is_empty() {
            result["enhanced"] = json!(false);
            emit_value(&mut rows, &mut incremental_rows, result.clone())?;
            final_rows.push(result);
            continue;
        }
        result["enhanced"] = json!(true);
        result["content"] = json!({});
        for category in categories {
            match extract_category(
                session.as_ref(),
                row.frame_id,
                category,
                &row.png,
                &options.redact_rules,
            ) {
                Ok((value, record)) => {
                    result["requests"]
                        .as_array_mut()
                        .expect("requests array")
                        .push(record);
                    result["content"][category.name] = value;
                }
                Err(ExtractionError::Failed { error, record }) => {
                    if let Some(record) = record {
                        result["requests"]
                            .as_array_mut()
                            .expect("requests array")
                            .push(record);
                    }
                    if result.get("error").is_none() {
                        result["error"] = json!(error);
                    }
                }
                Err(ExtractionError::Blocked { code, provider }) => {
                    let _ = session.close();
                    return Err(blocked(
                        options.journal,
                        work_key,
                        code.as_deref(),
                        provider.as_deref(),
                        Some(&category.context),
                    ));
                }
                Err(ExtractionError::Session) => {
                    let _ = session.close();
                    return Err(blocked(options.journal, work_key, None, None, None));
                }
            }
        }
        emit_value(&mut rows, &mut incremental_rows, result.clone())?;
        final_rows.push(result);
    }
    if let Some(plan) = &plan {
        for (frame_id, (existing_row, missing_categories)) in &plan.phase3_gaps {
            let mut result = existing_row.clone();
            result["enhanced"] = json!(true);
            result
                .as_object_mut()
                .expect("merge rows are objects")
                .remove("error");
            let png = frame_pngs
                .get(frame_id)
                .ok_or_else(|| RunError::Internal("missing merge frame".to_owned()))?;
            let analysis = result.get("analysis").expect("merge rows have analysis");
            let categories = extraction::categories_for_analysis(analysis);
            for name in missing_categories {
                let category = categories
                    .iter()
                    .find(|category| category.name == *name)
                    .expect("merge plan category is current");
                match extract_category(
                    session.as_ref(),
                    *frame_id,
                    category,
                    png,
                    &options.redact_rules,
                ) {
                    Ok((value, record)) => {
                        result["requests"]
                            .as_array_mut()
                            .expect("requests array")
                            .push(record);
                        result["content"][*name] = value;
                    }
                    Err(ExtractionError::Failed { error, record }) => {
                        if let Some(record) = record {
                            result["requests"]
                                .as_array_mut()
                                .expect("requests array")
                                .push(record);
                        }
                        if result.get("error").is_none() {
                            result["error"] = json!(error);
                        }
                    }
                    Err(ExtractionError::Blocked { code, provider }) => {
                        let _ = session.close();
                        return Err(blocked(
                            options.journal,
                            work_key,
                            code.as_deref(),
                            provider.as_deref(),
                            Some(&category.context),
                        ));
                    }
                    Err(ExtractionError::Session) => {
                        let _ = session.close();
                        return Err(blocked(options.journal, work_key, None, None, None));
                    }
                }
            }
            emit_value(&mut rows, &mut incremental_rows, result.clone())?;
            final_rows.push(result);
        }
    }
    if let Some(incremental_rows) = incremental_rows.take() {
        for (_, row) in incremental_rows {
            match row {
                RowContent::Raw(line) => rows.raw_row(&line)?,
                RowContent::Value(row) => rows.row(&row)?,
            }
        }
    }
    let final_rows = finalize_incomplete(final_rows.into_iter().map(|row| (row, 0)).collect());
    let failures = has_row_failures(&final_rows)
        || final_rows
            .iter()
            .filter_map(|row| row.get("frame_id").and_then(Value::as_u64))
            .collect::<BTreeSet<_>>()
            != qualified_ids;
    let _ = session.close();
    let (state, reason) = verdict(decoded.decode_failed, failures);
    promote(Promotion {
        output: &output,
        rows: &rows.path,
        video: options.video,
        input_size,
        decoded: &decoded,
        model,
        state,
        reason,
        previous_attempts,
        carry_forward_observer: carry_forward_observer.as_deref(),
    })?;
    send_described(&options, &output, start);
    Ok(())
}

enum ExtractionError {
    Blocked {
        code: Option<String>,
        provider: Option<String>,
    },
    Failed {
        error: String,
        record: Option<Value>,
    },
    Session,
}

fn extract_category(
    session: &dyn DescribeSession,
    frame_id: u64,
    category: &crate::categories::CategoryMeta,
    png: &[u8],
    redact_rules: &[String],
) -> Result<(Value, Value), ExtractionError> {
    let mut attempt = 0;
    loop {
        let request = extraction::request(frame_id, category, png, attempt, redact_rules)
            .ok_or_else(|| ExtractionError::Failed {
                error: "failed to resize extraction frame".to_owned(),
                record: None,
            })?;
        let id = request.id.clone().expect("extraction ids are present");
        let submitted_at = Instant::now();
        session
            .submit(request)
            .map_err(|_| ExtractionError::Session)?;
        let completion = session
            .recv_timeout(Duration::from_secs(120))
            .map_err(|_| ExtractionError::Session)?;
        let SessionCompletion::Response(response) = completion else {
            return Err(ExtractionError::Session);
        };
        let response_id = match &response {
            GenerateResponse::Generated(generated) => generated.id.as_deref(),
            GenerateResponse::Refused(refusal) => refusal.id.as_deref(),
        };
        if response_id != Some(id.as_str()) {
            return Err(ExtractionError::Session);
        }
        let duration = submitted_at.elapsed().as_secs_f64();
        let (failure, model) = match response {
            GenerateResponse::Generated(generated) => {
                let model = generated.model;
                match extraction::parse_response(
                    category,
                    &generated.text,
                    &generated.finish_reason,
                ) {
                    Ok(value) => {
                        return Ok((
                            value,
                            request_record(
                                "category",
                                model,
                                duration,
                                attempt,
                                Some(category.name),
                            ),
                        ));
                    }
                    Err(error) => (error, model),
                }
            }
            GenerateResponse::Refused(refusal) => {
                if refusal.reason == RefusalReason::NoEngineConfigured || refusal.blocking {
                    return Err(ExtractionError::Blocked {
                        code: refusal.reason_code.map(|value| value.as_wire().to_owned()),
                        provider: refusal.provider,
                    });
                }
                if !refusal.retryable {
                    return Err(ExtractionError::Failed {
                        error: refusal.detail,
                        record: Some(request_record(
                            "category",
                            String::new(),
                            duration,
                            attempt,
                            Some(category.name),
                        )),
                    });
                }
                (refusal.detail, String::new())
            }
        };
        if attempt + 1 >= MAX_ATTEMPTS {
            return Err(ExtractionError::Failed {
                error: failure,
                record: Some(request_record(
                    "category",
                    model,
                    duration,
                    attempt,
                    Some(category.name),
                )),
            });
        }
        attempt += 1;
    }
}

fn request_record(
    request_type: &str,
    model: String,
    duration: f64,
    retries: u64,
    category: Option<&str>,
) -> Value {
    let mut record = Map::new();
    record.insert("type".to_owned(), json!(request_type));
    record.insert("model".to_owned(), json!(model));
    record.insert("duration".to_owned(), json!(duration));
    if let Some(category) = category {
        record.insert("category".to_owned(), json!(category));
    }
    if retries > 0 {
        record.insert("retries".to_owned(), json!(retries));
    }
    Value::Object(record)
}

fn finalize_incomplete(mut results: Vec<(Value, usize)>) -> Vec<Value> {
    results
        .drain(..)
        .map(|(mut result, pending)| {
            if pending > 0 {
                result["error"] = json!("Extraction never completed");
            }
            result
        })
        .collect()
}

fn has_row_failures(rows: &[Value]) -> bool {
    rows.iter().any(|row| row.get("error").is_some())
}

fn schema_validation_failed(validation: Option<&Value>) -> bool {
    validation.is_some_and(|validation| {
        validation.get("valid") == Some(&Value::Bool(false))
            || validation
                .get("errors")
                .and_then(Value::as_array)
                .is_some_and(|errors| !errors.is_empty())
    })
}

fn emit_value(
    rows: &mut RowTemp,
    incremental_rows: &mut Option<BTreeMap<u64, RowContent>>,
    row: Value,
) -> Result<(), RunError> {
    if let Some(incremental_rows) = incremental_rows {
        let frame_id = row
            .get("frame_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| RunError::Internal("describe row omitted frame_id".to_owned()))?;
        incremental_rows.insert(frame_id, RowContent::Value(row));
        Ok(())
    } else {
        rows.row(&row)
    }
}

fn verdict(decode_failed: bool, failures: bool) -> (&'static str, &'static str) {
    if decode_failed {
        (vocab::STATE_FAILED, vocab::REASON_CORRUPT_INPUT)
    } else if failures {
        (vocab::STATE_FAILED, vocab::REASON_ANALYSIS_FAILED)
    } else {
        (vocab::STATE_ANALYZED, vocab::REASON_OK)
    }
}

fn maybe_detect(
    disabled: &mut bool,
    analysis: &Value,
    png: &[u8],
    journal: &Path,
) -> Option<Value> {
    if *disabled {
        return None;
    }
    let gate = detect::screen_gate(analysis)?;
    let result = detect::detect(png, journal)
        .and_then(|result| detect::detections_block(&result, "screen", &gate));
    match result {
        Ok(result) => Some(result),
        Err(_) => {
            *disabled = true;
            None
        }
    }
}

fn blocked(
    journal: &Path,
    work_key: &str,
    code: Option<&str>,
    provider: Option<&str>,
    context: Option<&str>,
) -> RunError {
    notify::blocked(journal, work_key, code, provider, context);
    RunError::Blocked(code.map(str::to_owned))
}

fn send_described(options: &DescribeOptions<'_>, output: &Path, start: Instant) {
    let day = notify::day_for_path(options.video);
    let segment = notify::segment_for_video_path(options.video);
    let observer = std::env::var("OBSERVER_NAME")
        .ok()
        .filter(|observer| !observer.is_empty());
    notify::described(
        options.journal,
        options.video,
        output,
        start.elapsed().as_millis() as u64,
        day.as_deref(),
        segment.as_deref(),
        observer.as_deref(),
    );
}

fn promote(promotion: Promotion<'_>) -> Result<(), RunError> {
    let parent = promotion
        .output
        .parent()
        .ok_or_else(|| RunError::Internal("output has no parent".to_owned()))?;
    let final_path = parent.join(format!(".describe-final-{}.jsonl.tmp", std::process::id()));
    let result = (|| {
        let mut header = Map::new();
        header.insert(
            "raw".to_owned(),
            json!(
                promotion
                    .video
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or_default()
            ),
        );
        if let Some(observer) = std::env::var("OBSERVER_NAME")
            .ok()
            .filter(|observer| !observer.is_empty())
            .or_else(|| promotion.carry_forward_observer.map(str::to_owned))
        {
            header.insert("observer".to_owned(), json!(observer));
        }
        if let Ok(meta) = std::env::var("SEGMENT_META") {
            match serde_json::from_str::<Value>(&meta) {
                Ok(Value::Object(values)) => header.extend(values),
                Ok(_) => {
                    return Err(RunError::Internal(
                        "SEGMENT_META must be an object".to_owned(),
                    ));
                }
                Err(_) => {}
            }
        }
        header.insert(
            "first_hash".to_owned(),
            json!(promotion.decoded.first_hash.map(|v| format!("{v:016x}"))),
        );
        header.insert(
            "last_hash".to_owned(),
            json!(promotion.decoded.last_hash.map(|v| format!("{v:016x}"))),
        );
        header.insert(
            "qualified_count".to_owned(),
            json!(promotion.decoded.qualified_count),
        );
        if let Some(model) = promotion.model {
            header.insert("_solstone_thinking".to_owned(), json!({"model":model}));
        }
        let mut record = json!({"schema":vocab::SCHEMA,"state":promotion.state,"reason_code":promotion.reason,"handler":vocab::HANDLER_DESCRIBE,"attempted_at":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),"input_size":promotion.input_size});
        if promotion.state == vocab::STATE_FAILED {
            record["attempts"] = json!(promotion.previous_attempts + 1);
        }
        header.insert("_solstone_processing".to_owned(), record);
        let mut final_file =
            File::create(&final_path).map_err(|error| RunError::Internal(error.to_string()))?;
        writeln!(final_file, "{}", Value::Object(header))
            .map_err(|error| RunError::Internal(error.to_string()))?;
        let mut row_file =
            File::open(promotion.rows).map_err(|error| RunError::Internal(error.to_string()))?;
        std::io::copy(&mut row_file, &mut final_file)
            .map_err(|error| RunError::Internal(error.to_string()))?;
        drop(final_file);
        install_file(&final_path, promotion.output, AtomicWriteOptions::default())
            .map_err(|error| RunError::Internal(error.to_string()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&final_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use solstone_core_processing_record::vocab;

    use super::{finalize_incomplete, has_row_failures, verdict};

    #[test]
    fn incomplete_extractions_keep_partial_content_and_fail_verdict() {
        let rows = finalize_incomplete(vec![(
            json!({"frame_id":1,"enhanced":true,"content":{"code":"partial"}}),
            1,
        )]);
        assert_eq!(rows[0]["content"]["code"], "partial");
        assert_eq!(rows[0]["error"], "Extraction never completed");
        assert!(has_row_failures(&rows));
        assert_eq!(
            verdict(false, has_row_failures(&rows)),
            (vocab::STATE_FAILED, vocab::REASON_ANALYSIS_FAILED)
        );
    }
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod pipeline_tests;
