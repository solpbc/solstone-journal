// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use solstone_core_generate::{GenerateResponse, RefusalReason, SessionCompletion};
use solstone_core_processing_record::{
    read_processing_record_header, record_attempts, should_reenter_analysis_output, vocab,
};

use crate::WinnowConfig;
use crate::categories::OutputKind;
use crate::decode::{PullNext, QualifiedFrame, RedecodeFail, resize_for_vlm_png};
use solstone_core_journal_io::LockOptions;
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};

use crate::detect;
use crate::extraction;
use crate::merge;
use crate::notify;
use crate::request;
use crate::screen_ocr::{self, OcrWorkerFactory};
use crate::selection::{self, CategorizedFrame, CategoryOverride, SelectionError};
use crate::session::{DescribeSession, DescribeSessionFactory, SystemSessionFactory};

pub const EXIT_PROVIDER_BLOCKED: i32 = 69;
const MAX_ATTEMPTS: u64 = 5;
#[cfg(not(windows))]
const RFDETR_UNAVAILABLE_DETAIL: &str = "Object detection is unavailable. Run `solstone journal install-models` to check or repair the RF-DETR assets.";

#[derive(Debug)]
pub enum RunError {
    Blocked(Option<String>),
    Cancelled,
    Internal(String),
}

/// Screen detection for one PNG. Describe does not depend on depict.
///
/// The value is the detector's raw JSON (`image` and `detections`).
pub trait ScreenDetector {
    fn detect(&self, png: &[u8], journal: &Path) -> Result<Value, String>;
}

/// Desktop detector: the existing child or packaged helper.
pub struct SpawnScreenDetector;

impl ScreenDetector for SpawnScreenDetector {
    fn detect(&self, png: &[u8], journal: &Path) -> Result<Value, String> {
        detect::detect(png, journal)
    }
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
    /// Stored in completion order while the PNG is held. `None` means detect
    /// still runs later, in that same order.
    detection: Option<Result<Option<Value>, String>>,
}

struct Promotion<'a> {
    claims: &'a OutputClaims,
    input: &'a InputObservation,
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
    existing: Option<&'a merge::ExistingDescribeArtifact>,
    attempt: Option<&'a solstone_core_system::work_retry::Attempt>,
    checkpoint: &'a dyn Fn() -> bool,
    ocr: Option<&'a screen_ocr::PreparedPublication>,
}

struct OcrPublication<'a> {
    segment: &'a Path,
    binding: Option<&'a screen_ocr::SourceBinding>,
    decoded: &'a crate::DescribeResult,
    plan: Option<&'a merge::IncrementalMergePlan>,
    categorized: &'a [CategorizedRow],
    overrides: &'a BTreeMap<String, CategoryOverride>,
    recipe_id: &'a str,
    redo: bool,
    worker_factory: &'a mut dyn OcrWorkerFactory,
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
    let resolver = screen_ocr::InstalledHelperResolver;
    let mut ocr = screen_ocr::ResolvedWorkerFactory::new(&resolver);
    run_with_clients(
        options,
        factory,
        &SpawnScreenDetector,
        &mut ocr,
        &|| false,
        None,
    )
}

/// In-process describe. One file, caller-owned session, detector, OCR, and cancel.
pub fn run_with_clients(
    options: DescribeOptions<'_>,
    sessions: &dyn DescribeSessionFactory,
    detector: &dyn ScreenDetector,
    ocr: &mut dyn OcrWorkerFactory,
    checkpoint: &dyn Fn() -> bool,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<(), RunError> {
    run_prepared(
        options,
        sessions,
        detector,
        ocr,
        checkpoint,
        attempt,
        DecodeStart::Video,
    )
}

/// Run the describe pipeline after media decoding has completed.
///
/// Keeping this seam private lets pipeline tests use small, valid synthetic frame
/// data while the CLI integration suite continues to prove the native decode,
/// child-session, detector, and command-line boundaries.
#[cfg(all(test, feature = "full-tests"))]
fn run_decoded(
    options: DescribeOptions<'_>,
    factory: &dyn DescribeSessionFactory,
    decoded: crate::DescribeResult,
) -> Result<(), RunError> {
    run_with_decoder(options, factory, || decoded)
}

#[allow(dead_code)]
fn run_with_decoder(
    options: DescribeOptions<'_>,
    factory: &dyn DescribeSessionFactory,
    decode: impl FnOnce() -> crate::DescribeResult,
) -> Result<(), RunError> {
    let resolver = screen_ocr::InstalledHelperResolver;
    let mut ocr_factory = screen_ocr::ResolvedWorkerFactory::new(&resolver);
    run_prepared(
        options,
        factory,
        &SpawnScreenDetector,
        &mut ocr_factory,
        &|| false,
        None,
        DecodeStart::Deferred(Box::new(decode)),
    )
}

#[cfg(all(test, feature = "full-tests"))]
fn run_decoded_with_ocr_factory(
    options: DescribeOptions<'_>,
    factory: &dyn DescribeSessionFactory,
    decoded: crate::DescribeResult,
    ocr_factory: &mut dyn OcrWorkerFactory,
) -> Result<(), RunError> {
    run_prepared(
        options,
        factory,
        &SpawnScreenDetector,
        ocr_factory,
        &|| false,
        None,
        DecodeStart::Ready(decoded),
    )
}

#[allow(dead_code)]
enum DecodeStart<'a> {
    Ready(crate::DescribeResult),
    /// Invoked only after the output claim is held, so a test can observe that lock.
    Deferred(Box<dyn FnOnce() -> crate::DescribeResult + 'a>),
    Video,
}

struct LivePull {
    pull: crate::decode::FramePull,
    eof: bool,
    window: usize,
    held_pngs: usize,
}

impl LivePull {
    fn pull_available(
        &mut self,
        checkpoint: &dyn Fn() -> bool,
    ) -> Result<Vec<QualifiedFrame>, RunError> {
        let mut frames = Vec::new();
        while self.held_pngs < self.window && !self.eof {
            match self.pull.next(checkpoint) {
                PullNext::Frame(frame) => {
                    self.held_pngs += 1;
                    frames.push(frame);
                }
                PullNext::Done => self.eof = true,
                PullNext::Cancelled => return Err(RunError::Cancelled),
            }
        }
        Ok(frames)
    }

    fn release(&mut self, count: usize) {
        self.held_pngs = self.held_pngs.saturating_sub(count);
    }
}

fn run_prepared(
    options: DescribeOptions<'_>,
    factory: &dyn DescribeSessionFactory,
    detector: &dyn ScreenDetector,
    ocr_factory: &mut dyn OcrWorkerFactory,
    checkpoint: &dyn Fn() -> bool,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
    decode_start: DecodeStart<'_>,
) -> Result<(), RunError> {
    if options.jobs == 0 {
        return Err(RunError::Internal("--jobs must be positive".to_owned()));
    }
    let output = options.video.with_extension("jsonl");
    let claims = OutputClaims::acquire([output.clone()], LockOptions::default())
        .map_err(|error| RunError::Internal(error.to_string()))?;
    let segment = output.parent().unwrap_or_else(|| Path::new(""));
    let recipe_id = ocr_factory.recipe_id();
    let input = match InputObservation::capture_checked(&claims, options.video, checkpoint) {
        Ok(input) => input,
        Err(_) if checkpoint() => return Err(RunError::Cancelled),
        Err(error) => {
            if let Ok(identity) = claims.segment_identity(segment) {
                let identity = identity.observation_id();
                let source_id = screen_ocr::source_identity(&identity, options.video);
                let _ = screen_ocr::invalidate_source_state(
                    options.journal,
                    segment,
                    &screen_ocr::sidecar_path(segment, &source_id),
                    &source_id,
                    &identity,
                    &recipe_id,
                    &claims,
                    &|| false,
                );
            }
            return Err(RunError::Internal(error.to_string()));
        }
    };
    let source_binding = screen_ocr::source_binding(&claims, &input, options.video, segment).ok();
    if let Some(binding) = &source_binding {
        let path = screen_ocr::sidecar_path(segment, &binding.source_id);
        let sidecar = screen_ocr::load_sidecar(&path);
        let invalid = match &sidecar {
            Some(sidecar) => {
                sidecar.segment_identity != binding.segment_identity
                    || sidecar.source_sha256 != binding.source_sha256
                    || sidecar.recipe_id != recipe_id
            }
            None => fs::symlink_metadata(&path).is_ok(),
        };
        if invalid {
            let _ = screen_ocr::invalidate_source_state(
                options.journal,
                segment,
                &path,
                &binding.source_id,
                &binding.segment_identity,
                &recipe_id,
                &claims,
                &|| false,
            );
        }
    }
    let mut previous_attempts = 0;
    let mut incremental_source = None;
    let admitted = if options.redo {
        false
    } else if let Some(attempt) = attempt {
        let unit = solstone_core_system::work_retry::raw::unit_for_output(
            options.journal,
            &output,
            vocab::HANDLER_DESCRIBE,
        )
        .map_err(RunError::Internal)?;
        solstone_core_system::work_retry::raw::consume_context_checked(
            options.journal,
            &output,
            vocab::HANDLER_DESCRIBE,
            &solstone_core_system::work_retry::raw::RetryContext {
                unit_id: unit.id().map_err(RunError::Internal)?,
                attempt: attempt.clone(),
            },
            &|| {
                if checkpoint() {
                    Err("describe cancelled".to_owned())
                } else {
                    Ok(())
                }
            },
        )
        .map_err(RunError::Internal)?
        .is_some()
    } else {
        solstone_core_system::work_retry::raw::consume_from_environment(
            options.journal,
            &output,
            vocab::HANDLER_DESCRIBE,
        )
        .map_err(RunError::Internal)?
    };
    if !options.redo && output.exists() {
        let record = read_processing_record_header(&output);
        if !admitted
            && !should_reenter_analysis_output(record.as_ref(), &output, vocab::HANDLER_DESCRIBE)
        {
            return Ok(());
        }
        previous_attempts = record.as_ref().map(record_attempts).unwrap_or(0);
        incremental_source = Some(output.clone());
    }
    let window = crate::DescribeWorkBound::for_jobs(options.jobs).max_png_slots;
    let (mut decoded, video_source) = match decode_start {
        DecodeStart::Ready(decoded) => (decoded, false),
        DecodeStart::Deferred(decode) => (decode(), false),
        DecodeStart::Video => {
            let decoded = walk_video_metadata(options.video, options.config, checkpoint)?;
            (decoded, true)
        }
    };
    let redecode = video_source;
    let start = Instant::now();
    let parent = output
        .parent()
        .ok_or_else(|| RunError::Internal("video has no parent".to_owned()))?;
    let input_size = input.input_size();
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
    let mut qualified_ids = decoded
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
        &options.category_overrides,
    );
    if decoded.qualified_frames.is_empty() {
        if checkpoint() {
            return Err(RunError::Cancelled);
        }
        let mut staged_ocr = None;
        prepare_ocr_best_effort::<(), RunError>(
            Ok(()),
            OcrPublication {
                segment,
                binding: source_binding.as_ref(),
                decoded: &decoded,
                plan: plan.as_ref(),
                categorized: &[],
                overrides: &options.category_overrides,
                recipe_id: &recipe_id,
                redo: options.redo,
                worker_factory: ocr_factory,
            },
            checkpoint,
            &mut staged_ocr,
        )??;
        let (state, reason) = if decoded.decode_failed {
            (vocab::STATE_FAILED, vocab::REASON_CORRUPT_INPUT)
        } else {
            (vocab::STATE_EMPTY, vocab::REASON_NO_DECODABLE_FRAMES)
        };
        if checkpoint() {
            return Err(RunError::Cancelled);
        }
        let prepared_ocr =
            prepare_ocr_publication(options.journal, segment, staged_ocr.as_ref(), checkpoint)?;
        promote(Promotion {
            claims: &claims,
            input: &input,
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
            existing: existing_artifact.as_ref(),
            attempt,
            checkpoint,
            ocr: prepared_ocr.as_ref(),
        })?;
        send_described(&options, &output, start);
        return Ok(());
    }
    let png_ids = if video_source {
        plan.as_ref()
            .map(|plan| plan.phase1_gap_ids.clone())
            .unwrap_or_else(|| qualified_ids.clone())
    } else {
        BTreeSet::new()
    };
    let mut queued_pngs = BTreeSet::new();
    let mut live = if video_source && !png_ids.is_empty() {
        let mut pull = crate::decode::FramePull::open(options.video, options.config, true)
            .map_err(|_| {
                RunError::Internal(
                    "describe could not reopen the video to materialize frame payloads".to_owned(),
                )
            })?;
        pull.retain_pngs(png_ids.clone());
        let mut live = LivePull {
            pull,
            eof: false,
            window,
            held_pngs: 0,
        };
        for frame in live.pull_available(checkpoint)? {
            accept_materialized_frame(&mut decoded, &png_ids, &mut queued_pngs, &frame)?;
        }
        if live.eof {
            ensure_png_ids_queued(&png_ids, &queued_pngs)?;
        }
        Some(live)
    } else {
        None
    };
    let mut incremental_ocr = live.as_ref().is_some_and(|live| !live.eof);
    let streamed = incremental_ocr;
    let work_key = options
        .video
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("describe");
    let frame_pngs = if redecode {
        HashMap::new()
    } else {
        decoded
            .qualified_frames
            .iter()
            .map(|frame| (frame.frame_id, frame.png.clone()))
            .collect::<HashMap<_, _>>()
    };
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
            let gap = plan
                .as_ref()
                .is_none_or(|plan| plan.phase1_gap_ids.contains(&frame.frame_id));
            gap && (!video_source || !frame.png.is_empty())
        })
        .cloned()
        .map(|frame| Pending { frame, attempt: 0 })
        .collect();
    let mut outstanding: HashMap<String, Outstanding> = HashMap::new();
    let mut categorized = Vec::new();
    let mut model = None;
    let mut session = None;
    let mut detection_disabled_early = None;
    let mut streaming_ocr = screen_ocr::StreamingOcr::new();
    let mut streaming_sidecar = None;
    let categorization = (|| -> Result<(), RunError> {
        session = Some(
            factory
                .spawn(options.jobs, options.explicit_journal)
                .map_err(|_| blocked(options.journal, work_key, None, None, None))?,
        );
        let session = session.as_deref().expect("spawned describe session");
        let instruction = request::system_instruction(&options.redact_rules);
        loop {
            if checkpoint() {
                return Err(RunError::Cancelled);
            }
            if incremental_ocr && let Some(live) = live.as_mut() {
                let released = release_held_frames(
                    &mut categorized,
                    &mut decoded,
                    &mut detection_disabled_early,
                    detector,
                    options.journal,
                    &options.category_overrides,
                    &source_binding,
                    &recipe_id,
                    ocr_factory,
                    &mut streaming_ocr,
                    &mut streaming_sidecar,
                    &png_ids,
                    checkpoint,
                )?;
                live.release(released);
                let more = live.pull_available(checkpoint)?;
                for frame in more {
                    accept_materialized_frame(&mut decoded, &png_ids, &mut queued_pngs, &frame)?;
                    waiting.push_back(Pending { frame, attempt: 0 });
                }
                if live.eof {
                    ensure_png_ids_queued(&png_ids, &queued_pngs)?;
                    incremental_ocr = false;
                }
            }
            if waiting.is_empty() && outstanding.is_empty() {
                if live.as_ref().is_some_and(|pull| !pull.eof) {
                    return Err(RunError::Internal(
                        "describe frame window made no progress".to_owned(),
                    ));
                }
                break;
            }
            while outstanding.len() < window {
                let Some(pending) = waiting.pop_front() else {
                    break;
                };
                let png = resize_for_vlm_png(&pending.frame.png, Some(1024)).ok_or_else(|| {
                    RunError::Internal("failed to resize categorization frame".to_owned())
                })?;
                let req =
                    request::request(pending.frame.frame_id, pending.attempt, &png, &instruction);
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
            if matches!(response, GenerateResponse::Cancelled) {
                let _ = session.close();
                return Err(RunError::Cancelled);
            }
            let id = match &response {
                GenerateResponse::Generated(g) => g.id.clone(),
                GenerateResponse::Refused(r) => r.id.clone(),
                GenerateResponse::Cancelled => None,
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
                GenerateResponse::Cancelled => {
                    let _ = session.close();
                    return Err(RunError::Cancelled);
                }
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
                        detection: None,
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
                            detection: None,
                        });
                    }
                }
            }
        }
        Ok(())
    })();
    if matches!(categorization, Err(RunError::Cancelled)) {
        return Err(RunError::Cancelled);
    }
    decoded.qualified_count = decoded.qualified_frames.len();
    qualified_ids = decoded
        .qualified_frames
        .iter()
        .map(|frame| frame.frame_id)
        .collect();
    if streamed {
        let released = release_held_frames(
            &mut categorized,
            &mut decoded,
            &mut detection_disabled_early,
            detector,
            options.journal,
            &options.category_overrides,
            &source_binding,
            &recipe_id,
            ocr_factory,
            &mut streaming_ocr,
            &mut streaming_sidecar,
            &png_ids,
            checkpoint,
        )?;
        if let Some(live) = live.as_mut() {
            live.release(released);
        }
    }
    if checkpoint() {
        return Err(RunError::Cancelled);
    }
    let mut staged_ocr = None;
    let categorization = if video_source && plan.is_some() {
        // Retry analyses can be reused without retaining their PNGs. Reopen the
        // exact input and run the OCR-only pass one frame at a time instead of
        // interpreting metadata-only PNGs as changed or corrupt cached text.
        staged_ocr = prepare_video_ocr(
            options.video,
            options.config,
            &decoded,
            plan.as_ref(),
            &categorized,
            &options.category_overrides,
            source_binding.as_ref(),
            &recipe_id,
            ocr_factory,
            checkpoint,
        )?;
        categorization
    } else if streamed {
        if let Some(sidecar) = streaming_sidecar.as_mut() {
            let analyses = ocr_analyses(plan.as_ref(), &categorized);
            sidecar.admission_policy_id =
                screen_ocr::admission_policy_id(&analyses, &options.category_overrides);
            sidecar.decode =
                screen_ocr::decode_coverage(decoded.decode_failed, decoded.qualified_frames.len());
            screen_ocr::set_frame_categories(sidecar, &analyses, &options.category_overrides);
            streaming_ocr.finish(sidecar, ocr_factory);
        }
        staged_ocr = streaming_sidecar;
        categorization
    } else {
        prepare_ocr_best_effort(
            categorization,
            OcrPublication {
                segment,
                binding: source_binding.as_ref(),
                decoded: &decoded,
                plan: plan.as_ref(),
                categorized: &categorized,
                overrides: &options.category_overrides,
                recipe_id: &recipe_id,
                redo: options.redo,
                worker_factory: ocr_factory,
            },
            checkpoint,
            &mut staged_ocr,
        )?
    };
    let staged_ocr = std::cell::RefCell::new(staged_ocr);
    let publish_ocr_diagnostics = |diagnostic_rows: &[Value]| -> Result<(), RunError> {
        if checkpoint() {
            return Err(RunError::Cancelled);
        }
        if let Some(sidecar) = staged_ocr.borrow_mut().as_mut() {
            screen_ocr::update_frame_diagnostics(sidecar, diagnostic_rows);
        }
        Ok(())
    };
    categorization?;
    let session_ref = session
        .as_deref()
        .expect("successful categorization has session");
    let total_frames = categorized.len();
    let failed_frames = categorized.iter().filter(|row| row.error.is_some()).count();
    if total_frames > 0 && failed_frames == total_frames && plan.is_none() {
        let _ = session_ref.close();
        if checkpoint() {
            return Err(RunError::Cancelled);
        }
        promote(Promotion {
            claims: &claims,
            input: &input,
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
            existing: existing_artifact.as_ref(),
            attempt,
            checkpoint,
            ocr: None,
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
            session_ref,
            &selection_frames,
            options.max_extractions,
            &options.category_overrides,
        ) {
            Ok(selected) => selected,
            Err(SelectionError::Blocked {
                reason_code,
                provider,
            }) => {
                let _ = session_ref.close();
                return Err(blocked(
                    options.journal,
                    work_key,
                    reason_code.as_deref(),
                    provider.as_deref(),
                    Some("observe.extract.selection"),
                ));
            }
            Err(SelectionError::Session) => {
                let _ = session_ref.close();
                return Err(blocked(options.journal, work_key, None, None, None));
            }
            Err(SelectionError::Cancelled) => {
                let _ = session_ref.close();
                return Err(RunError::Cancelled);
            }
        }
    };
    let selected = selected.into_iter().collect::<HashSet<_>>();
    let mut detection_disabled = None;
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
        if checkpoint() {
            return Err(RunError::Cancelled);
        }
        let detection = if let Some(stored) = row.detection.clone() {
            stored
        } else {
            maybe_detect(
                &mut detection_disabled,
                analysis,
                &row.png,
                options.journal,
                detector,
            )
        };
        match detection {
            Ok(Some(detections)) => result["detections"] = detections,
            Ok(None) => {}
            Err(detail) => {
                result["detection_error"] = json!({
                    "reason_code": "rfdetr-unavailable",
                    "detail": detail,
                });
            }
        }
        if !selected.contains(&row.frame_id) {
            result["enhanced"] = json!(false);
            emit_value(&mut rows, &mut incremental_rows, result.clone())?;
            final_rows.push(result);
            continue;
        }
        let categories = extraction::categories_for_analysis(analysis, &options.category_overrides);
        if categories.is_empty() {
            result["enhanced"] = json!(false);
            emit_value(&mut rows, &mut incremental_rows, result.clone())?;
            final_rows.push(result);
            continue;
        }
        result["enhanced"] = json!(true);
        result["content"] = json!({});
        let png = owned_frame_png(
            redecode,
            options.video,
            options.config,
            row.frame_id,
            row.timestamp,
            &row.png,
            checkpoint,
        )?;
        for category in categories {
            match extract_category(
                session_ref,
                row.frame_id,
                category,
                &png,
                &options.redact_rules,
                checkpoint,
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
                    let mut diagnostic_rows = final_rows.clone();
                    let mut failed = result.clone();
                    failed["error"] = json!("extraction_failed");
                    diagnostic_rows.push(failed);
                    publish_ocr_diagnostics(&diagnostic_rows)?;
                    let _ = session_ref.close();
                    return Err(blocked(
                        options.journal,
                        work_key,
                        code.as_deref(),
                        provider.as_deref(),
                        Some(&category.context),
                    ));
                }
                Err(ExtractionError::Cancelled) => {
                    let _ = session_ref.close();
                    return Err(RunError::Cancelled);
                }
                Err(ExtractionError::Session) => {
                    let mut diagnostic_rows = final_rows.clone();
                    let mut failed = result.clone();
                    failed["error"] = json!("extraction_failed");
                    diagnostic_rows.push(failed);
                    publish_ocr_diagnostics(&diagnostic_rows)?;
                    let _ = session_ref.close();
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
            let timestamp = result
                .get("timestamp")
                .and_then(Value::as_f64)
                .ok_or_else(|| {
                    RunError::Internal(format!("describe frame {frame_id} omitted timestamp"))
                })?;
            let png = if redecode {
                owned_frame_png(
                    true,
                    options.video,
                    options.config,
                    *frame_id,
                    timestamp,
                    &[],
                    checkpoint,
                )?
            } else {
                frame_pngs
                    .get(frame_id)
                    .cloned()
                    .ok_or_else(|| RunError::Internal("missing merge frame".to_owned()))?
            };
            let analysis = result.get("analysis").expect("merge rows have analysis");
            let categories =
                extraction::categories_for_analysis(analysis, &options.category_overrides);
            for name in missing_categories {
                let category = categories
                    .iter()
                    .find(|category| category.name == *name)
                    .expect("merge plan category is current");
                match extract_category(
                    session_ref,
                    *frame_id,
                    category,
                    &png,
                    &options.redact_rules,
                    checkpoint,
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
                        let mut diagnostic_rows = final_rows.clone();
                        let mut failed = result.clone();
                        failed["error"] = json!("extraction_failed");
                        diagnostic_rows.push(failed);
                        publish_ocr_diagnostics(&diagnostic_rows)?;
                        let _ = session_ref.close();
                        return Err(blocked(
                            options.journal,
                            work_key,
                            code.as_deref(),
                            provider.as_deref(),
                            Some(&category.context),
                        ));
                    }
                    Err(ExtractionError::Cancelled) => {
                        let _ = session_ref.close();
                        return Err(RunError::Cancelled);
                    }
                    Err(ExtractionError::Session) => {
                        let mut diagnostic_rows = final_rows.clone();
                        let mut failed = result.clone();
                        failed["error"] = json!("extraction_failed");
                        diagnostic_rows.push(failed);
                        publish_ocr_diagnostics(&diagnostic_rows)?;
                        let _ = session_ref.close();
                        return Err(blocked(options.journal, work_key, None, None, None));
                    }
                }
            }
            emit_value(&mut rows, &mut incremental_rows, result.clone())?;
            final_rows.push(result);
        }
    }
    publish_ocr_diagnostics(&final_rows)?;
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
    let _ = session_ref.close();
    let (state, reason) = verdict(decoded.decode_failed, failures);
    if checkpoint() {
        return Err(RunError::Cancelled);
    }
    let staged_ocr = staged_ocr.into_inner();
    let prepared_ocr =
        prepare_ocr_publication(options.journal, segment, staged_ocr.as_ref(), checkpoint)?;
    promote(Promotion {
        claims: &claims,
        input: &input,
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
        existing: existing_artifact.as_ref(),
        attempt,
        checkpoint,
        ocr: prepared_ocr.as_ref(),
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
    Cancelled,
}

/// One forward metadata pass. PNGs stay empty so the merge plan sees every qualified id.
#[allow(clippy::result_large_err)]
fn walk_video_metadata(
    video: &Path,
    config: WinnowConfig,
    checkpoint: &dyn Fn() -> bool,
) -> Result<crate::DescribeResult, RunError> {
    let mut pull = match crate::decode::FramePull::open(video, config, false) {
        Ok(pull) => pull,
        Err(result) => return Ok(result),
    };
    loop {
        match pull.next(checkpoint) {
            PullNext::Frame(_) => {}
            PullNext::Done => break,
            PullNext::Cancelled => return Err(RunError::Cancelled),
        }
    }
    Ok(pull.metadata())
}

fn accept_materialized_frame(
    decoded: &mut crate::DescribeResult,
    png_ids: &BTreeSet<u64>,
    queued: &mut BTreeSet<u64>,
    frame: &QualifiedFrame,
) -> Result<(), RunError> {
    if !png_ids.contains(&frame.frame_id) || !queued.insert(frame.frame_id) {
        return Err(frame_did_not_reappear(frame.frame_id));
    }
    let Some(existing) = decoded
        .qualified_frames
        .iter_mut()
        .find(|row| row.frame_id == frame.frame_id)
    else {
        return Err(frame_did_not_reappear(frame.frame_id));
    };
    if existing.timestamp.to_bits() != frame.timestamp.to_bits() {
        return Err(frame_did_not_reappear(frame.frame_id));
    }
    existing.png.clone_from(&frame.png);
    Ok(())
}

fn ensure_png_ids_queued(png_ids: &BTreeSet<u64>, queued: &BTreeSet<u64>) -> Result<(), RunError> {
    if let Some(frame_id) = png_ids.iter().find(|frame_id| !queued.contains(frame_id)) {
        return Err(frame_did_not_reappear(*frame_id));
    }
    Ok(())
}

fn frame_did_not_reappear(frame_id: u64) -> RunError {
    RunError::Internal(format!(
        "describe frame {frame_id} did not reappear with the same timestamp"
    ))
}

fn owned_frame_png(
    redecode: bool,
    video: &Path,
    config: WinnowConfig,
    frame_id: u64,
    timestamp: f64,
    held: &[u8],
    checkpoint: &dyn Fn() -> bool,
) -> Result<Vec<u8>, RunError> {
    if !redecode {
        return Ok(held.to_vec());
    }
    if checkpoint() {
        return Err(RunError::Cancelled);
    }
    match crate::decode::redecode_frame(video, config, frame_id, timestamp, checkpoint) {
        Ok(png) => Ok(png),
        Err(RedecodeFail::Cancelled) => Err(RunError::Cancelled),
        Err(RedecodeFail::Missing) => Err(RunError::Internal(format!(
            "describe frame {frame_id} did not reappear with the same timestamp"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn release_held_frames(
    categorized: &mut [CategorizedRow],
    decoded: &mut crate::DescribeResult,
    detection_disabled: &mut Option<String>,
    detector: &dyn ScreenDetector,
    journal: &Path,
    overrides: &BTreeMap<String, CategoryOverride>,
    binding: &Option<screen_ocr::SourceBinding>,
    recipe_id: &str,
    ocr_factory: &mut dyn OcrWorkerFactory,
    streaming_ocr: &mut screen_ocr::StreamingOcr,
    streaming_sidecar: &mut Option<screen_ocr::PrivateSidecar>,
    png_ids: &BTreeSet<u64>,
    checkpoint: &dyn Fn() -> bool,
) -> Result<usize, RunError> {
    if checkpoint() {
        return Err(RunError::Cancelled);
    }
    if let Some(binding) = binding {
        let sidecar = streaming_sidecar.get_or_insert_with(|| {
            let mut sidecar = screen_ocr::expected_sidecar(
                binding.source_id.clone(),
                binding.segment_identity.clone(),
                binding.source_sha256.clone(),
                recipe_id.to_owned(),
                String::new(),
                screen_ocr::DecodeCoverage::Complete,
                &[],
            );
            sidecar.source_name = binding.source_name.clone();
            sidecar
        });
        for frame in &decoded.qualified_frames {
            if sidecar
                .frames
                .iter()
                .any(|row| row.frame_id == frame.frame_id)
            {
                continue;
            }
            let staged = screen_ocr::expected_sidecar(
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                screen_ocr::DecodeCoverage::Complete,
                std::slice::from_ref(frame),
            );
            if let Some(row) = staged.frames.into_iter().next() {
                sidecar.frames.push(row);
            }
        }
    }
    for row in categorized.iter_mut() {
        if row.detection.is_some() || row.analysis.is_none() {
            continue;
        }
        if checkpoint() {
            return Err(RunError::Cancelled);
        }
        let analysis = row.analysis.as_ref().expect("analysis row");
        let detection = maybe_detect(detection_disabled, analysis, &row.png, journal, detector);
        row.detection = Some(detection);
    }
    let mut released = 0_usize;
    let mut index = streaming_ocr.fed();
    while index < decoded.qualified_frames.len() {
        let frame_id = decoded.qualified_frames[index].frame_id;
        if !png_ids.is_empty() && !png_ids.contains(&frame_id) {
            if streaming_ocr.fed() != index {
                break;
            }
            streaming_ocr.advance();
            index += 1;
            continue;
        }
        let Some(row_index) = categorized.iter().position(|row| row.frame_id == frame_id) else {
            break;
        };
        let detect_ready =
            categorized[row_index].analysis.is_none() || categorized[row_index].detection.is_some();
        if !detect_ready {
            break;
        }
        if checkpoint() {
            return Err(RunError::Cancelled);
        }
        let ignored = frame_ignored(&categorized[row_index], overrides);
        if binding.is_some() {
            let png = decoded.qualified_frames[index].png.clone();
            let sidecar = streaming_sidecar
                .as_mut()
                .expect("a bound source has a sidecar");
            streaming_ocr.feed(sidecar, index, &png, ignored, ocr_factory);
        } else if streaming_ocr.fed() == index {
            streaming_ocr.advance();
        }
        let mut cleared = false;
        if !categorized[row_index].png.is_empty() {
            categorized[row_index].png = Vec::new();
            cleared = true;
        }
        if !decoded.qualified_frames[index].png.is_empty() {
            decoded.qualified_frames[index].png = Vec::new();
            cleared = true;
        }
        if cleared {
            released += 1;
        }
        index += 1;
    }
    Ok(released)
}

fn frame_ignored(row: &CategorizedRow, overrides: &BTreeMap<String, CategoryOverride>) -> bool {
    let Some(analysis) = row.analysis.as_ref() else {
        return false;
    };
    let analyses = BTreeMap::from([(row.frame_id, analysis.clone())]);
    screen_ocr::ignored_frame_ids(&analyses, overrides).contains(&row.frame_id)
}

fn extract_category(
    session: &dyn DescribeSession,
    frame_id: u64,
    category: &crate::categories::CategoryMeta,
    png: &[u8],
    redact_rules: &[String],
    checkpoint: &dyn Fn() -> bool,
) -> Result<(Value, Value), ExtractionError> {
    if checkpoint() {
        return Err(ExtractionError::Cancelled);
    }
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
        if matches!(response, GenerateResponse::Cancelled) {
            return Err(ExtractionError::Cancelled);
        }
        let response_id = match &response {
            GenerateResponse::Generated(generated) => generated.id.as_deref(),
            GenerateResponse::Refused(refusal) => refusal.id.as_deref(),
            GenerateResponse::Cancelled => None,
        };
        if response_id != Some(id.as_str()) {
            return Err(ExtractionError::Session);
        }
        let duration = submitted_at.elapsed().as_secs_f64();
        let (failure, model) = match response {
            GenerateResponse::Cancelled => {
                return Err(ExtractionError::Cancelled);
            }
            GenerateResponse::Generated(generated) => {
                let model = generated.model;
                match extraction::parse_response(
                    category,
                    &generated.text,
                    &generated.finish_reason,
                ) {
                    Ok(value) => {
                        if category.output == OutputKind::Json
                            && schema_validation_failed(generated.schema_validation.as_ref())
                        {
                            return Err(ExtractionError::Failed {
                                error: schema_validation_error_message(
                                    category.name,
                                    generated.schema_validation.as_ref(),
                                ),
                                record: Some(request_record(
                                    "category",
                                    model,
                                    duration,
                                    attempt,
                                    Some(category.name),
                                )),
                            });
                        }
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

fn schema_validation_error_message(category_name: &str, validation: Option<&Value>) -> String {
    if let Some(errors) = validation
        .and_then(|v| v.get("errors"))
        .and_then(Value::as_array)
        .filter(|errors| !errors.is_empty())
    {
        format!(
            "Schema validation failed for {category_name}: {}",
            Value::Array(errors.clone())
        )
    } else {
        format!("Schema validation failed for {category_name}")
    }
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
    disabled: &mut Option<String>,
    analysis: &Value,
    png: &[u8],
    journal: &Path,
    detector: &dyn ScreenDetector,
) -> Result<Option<Value>, String> {
    let Some(gate) = detect::screen_gate(analysis) else {
        return Ok(None);
    };
    if let Some(detail) = disabled.as_ref() {
        return Err(detail.clone());
    }
    let result = detector
        .detect(png, journal)
        .and_then(|result| detect::detections_block(&result, "screen", &gate))
        .map_err(|_| rfdetr_unavailable_detail().to_owned());
    match result {
        Ok(result) => Ok(Some(result)),
        Err(detail) => {
            *disabled = Some(detail.clone());
            Err(detail)
        }
    }
}

fn rfdetr_unavailable_detail() -> &'static str {
    #[cfg(windows)]
    {
        solstone_core_local::install::rfdetr_windows::RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE
    }
    #[cfg(not(windows))]
    {
        RFDETR_UNAVAILABLE_DETAIL
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

fn prepare_ocr_publication(
    journal: &Path,
    segment: &Path,
    sidecar: Option<&screen_ocr::PrivateSidecar>,
    checkpoint: &dyn Fn() -> bool,
) -> Result<Option<screen_ocr::PreparedPublication>, RunError> {
    if checkpoint() {
        return Err(RunError::Cancelled);
    }
    let Some(sidecar) = sidecar else {
        return Ok(None);
    };
    match screen_ocr::PreparedPublication::prepare(journal, segment, sidecar, checkpoint) {
        Ok(publication) => Ok(Some(publication)),
        Err(_) if checkpoint() => Err(RunError::Cancelled),
        Err(error) if error.to_string() == "describe cancelled" => Err(RunError::Cancelled),
        Err(error) => Err(RunError::Internal(format!(
            "screen OCR withdrawal could not be prepared: {error}"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_video_ocr(
    video: &Path,
    config: WinnowConfig,
    decoded: &crate::DescribeResult,
    plan: Option<&merge::IncrementalMergePlan>,
    categorized: &[CategorizedRow],
    overrides: &BTreeMap<String, CategoryOverride>,
    binding: Option<&screen_ocr::SourceBinding>,
    recipe_id: &str,
    factory: &mut dyn OcrWorkerFactory,
    checkpoint: &dyn Fn() -> bool,
) -> Result<Option<screen_ocr::PrivateSidecar>, RunError> {
    let Some(binding) = binding else {
        return Ok(None);
    };
    let analyses = ocr_analyses(plan, categorized);
    let ignored = screen_ocr::ignored_frame_ids(&analyses, overrides);
    let mut sidecar = screen_ocr::expected_sidecar(
        binding.source_id.clone(),
        binding.segment_identity.clone(),
        binding.source_sha256.clone(),
        recipe_id.to_owned(),
        screen_ocr::admission_policy_id(&analyses, overrides),
        screen_ocr::decode_coverage(decoded.decode_failed, decoded.qualified_frames.len()),
        &decoded.qualified_frames,
    );
    sidecar.source_name = binding.source_name.clone();
    screen_ocr::set_frame_categories(&mut sidecar, &analyses, overrides);
    let cached = video.parent().and_then(|segment| {
        screen_ocr::load_sidecar(&screen_ocr::sidecar_path(segment, &binding.source_id))
    });
    if let Some(cached) = cached.as_ref()
        && screen_ocr::cache_matches_metadata(cached, &sidecar, &ignored)
    {
        return Ok(Some(cached.clone()));
    }
    let mut pull = crate::decode::FramePull::open(video, config, true)
        .map_err(|_| RunError::Internal("could not reopen video for OCR retry".into()))?;
    let mut ocr = screen_ocr::StreamingOcr::new();
    loop {
        match pull.next(checkpoint) {
            PullNext::Frame(frame) => {
                let index = ocr.fed();
                if decoded.qualified_frames.get(index).is_none_or(|expected| {
                    expected.frame_id != frame.frame_id || expected.timestamp != frame.timestamp
                }) {
                    return Err(RunError::Internal("OCR retry frame binding changed".into()));
                }
                ocr.feed(
                    &mut sidecar,
                    index,
                    &frame.png,
                    ignored.contains(&frame.frame_id),
                    factory,
                );
            }
            PullNext::Cancelled => return Err(RunError::Cancelled),
            PullNext::Done => break,
        }
    }
    if ocr.fed() != decoded.qualified_frames.len() {
        return Err(RunError::Internal(
            "OCR retry did not reach the last qualified frame".into(),
        ));
    }
    ocr.finish(&mut sidecar, factory);
    if let Some(cached) = cached.as_ref() {
        screen_ocr::retain_valid_cached_frames(&mut sidecar, cached);
    }
    Ok(Some(sidecar))
}

fn prepare_ocr_best_effort<T, E>(
    outcome: Result<T, E>,
    publication: OcrPublication<'_>,
    checkpoint: &dyn Fn() -> bool,
    staged: &mut Option<screen_ocr::PrivateSidecar>,
) -> Result<Result<T, E>, RunError> {
    let OcrPublication {
        segment,
        binding,
        decoded,
        plan,
        categorized,
        overrides,
        recipe_id,
        redo,
        worker_factory,
    } = publication;
    let Some(binding) = binding else {
        return Ok(outcome);
    };
    let analyses = ocr_analyses(plan, categorized);
    let ignored = screen_ocr::ignored_frame_ids(&analyses, overrides);
    let mut expected = screen_ocr::expected_sidecar(
        binding.source_id.clone(),
        binding.segment_identity.clone(),
        binding.source_sha256.clone(),
        recipe_id.to_owned(),
        screen_ocr::admission_policy_id(&analyses, overrides),
        screen_ocr::decode_coverage(decoded.decode_failed, decoded.qualified_frames.len()),
        &decoded.qualified_frames,
    );
    expected.source_name = binding.source_name.clone();
    screen_ocr::set_frame_categories(&mut expected, &analyses, overrides);
    let path = screen_ocr::sidecar_path(segment, &binding.source_id);
    let cached = screen_ocr::load_sidecar(&path);
    let cache_hit = !redo
        && cached
            .as_ref()
            .is_some_and(|cached| screen_ocr::cache_matches(cached, &expected, &ignored, false));
    let mut sidecar = if cache_hit {
        cached.expect("cache match has a sidecar")
    } else {
        expected
    };
    let cancel = std::cell::Cell::new(false);
    let stop = &|| {
        if checkpoint() {
            cancel.set(true);
            true
        } else {
            false
        }
    };
    let outcome = complete_private_ocr(
        outcome,
        &mut sidecar,
        &decoded.qualified_frames,
        &ignored,
        worker_factory,
        !cache_hit,
        stop,
        |sidecar| {
            if cancel.get() || checkpoint() {
                cancel.set(true);
                return Err(());
            }
            *staged = Some(sidecar.clone());
            Ok(())
        },
    );
    if cancel.get() {
        return Err(RunError::Cancelled);
    }
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
fn complete_private_ocr<T, E>(
    outcome: Result<T, E>,
    sidecar: &mut screen_ocr::PrivateSidecar,
    frames: &[QualifiedFrame],
    ignored_ids: &BTreeSet<u64>,
    worker_factory: &mut dyn OcrWorkerFactory,
    run_worker: bool,
    stop: &dyn Fn() -> bool,
    publish: impl FnOnce(&screen_ocr::PrivateSidecar) -> Result<(), ()>,
) -> Result<T, E> {
    if run_worker
        && screen_ocr::run_worker_with_factory(sidecar, frames, ignored_ids, worker_factory, stop)
    {
        return outcome;
    }
    if sidecar.frames.iter().any(|frame| {
        matches!(frame.state, screen_ocr::CoverageState::Error)
            || frame.error_code.as_deref() == Some("helper_unavailable")
    }) {
        log::warn!("screen OCR coverage is incomplete; private frame diagnostics record the gap");
    }
    let _ = publish(sidecar);
    outcome
}

#[cfg(all(test, not(feature = "full-tests")))]
mod ocr_boundary_tests {
    use std::cell::Cell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::Cursor;
    use std::rc::Rc;

    use image::{DynamicImage, ImageFormat};
    use serde_json::{Value, json};

    use super::complete_private_ocr;
    use crate::decode::QualifiedFrame;
    use crate::merge::{ExistingDescribeRow, IncrementalMergePlan};
    use crate::screen_ocr::{
        self, CoverageState, DecodeCoverage, OcrFailure, OcrWorker, OcrWorkerFactory,
        PrivateSidecar, RawOccurrence, ResolvedWorkerFactory, UnavailableHelperResolver,
    };
    use crate::selection::{CategoryOverride, Importance};

    #[derive(Clone, Copy)]
    enum FakeBehavior {
        Absent,
        Success,
        Protocol,
        Timeout,
    }

    struct FakeFactory {
        behavior: FakeBehavior,
        creates: usize,
        recognizes: Rc<Cell<usize>>,
    }

    impl FakeFactory {
        fn new(behavior: FakeBehavior) -> Self {
            Self {
                behavior,
                creates: 0,
                recognizes: Rc::new(Cell::new(0)),
            }
        }
    }

    impl OcrWorkerFactory for FakeFactory {
        fn recipe_id(&mut self) -> String {
            "in-memory-recipe".to_owned()
        }

        fn create(&mut self) -> Result<Option<Box<dyn OcrWorker>>, OcrFailure> {
            self.creates += 1;
            if matches!(self.behavior, FakeBehavior::Absent) {
                return Ok(None);
            }
            Ok(Some(Box::new(FakeWorker {
                behavior: self.behavior,
                recognizes: Rc::clone(&self.recognizes),
            })))
        }
    }

    struct FakeWorker {
        behavior: FakeBehavior,
        recognizes: Rc<Cell<usize>>,
    }

    impl OcrWorker for FakeWorker {
        fn recognize(&mut self, _png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure> {
            self.recognizes.set(self.recognizes.get() + 1);
            match self.behavior {
                FakeBehavior::Success => Ok(vec![RawOccurrence {
                    line_order: 0,
                    text: "private recognition".to_owned(),
                    confidence: Some(0.9),
                    geometry: vec![[0.0, 0.0]],
                    recurrence: 0,
                }]),
                FakeBehavior::Protocol => Err(OcrFailure::Protocol),
                FakeBehavior::Timeout => Err(OcrFailure::Timeout),
                FakeBehavior::Absent => unreachable!("absent factory creates no worker"),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    struct CseObservations {
        c_calls: usize,
        selected_ids: Vec<u64>,
        selection_calls: usize,
        extraction_calls: usize,
        frame_rows: Vec<Value>,
        processing_state: &'static str,
        processing_reason: &'static str,
        header_present: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum PipelineFailure {
        AllCategorizationFailed {
            c_calls: usize,
            frame_rows: usize,
            verdict: &'static str,
            header_present: bool,
        },
        ProviderBlocked {
            c_calls: usize,
            selection_calls: usize,
            extraction_calls: usize,
            header_present: bool,
        },
    }

    fn frames(ids: &[u64]) -> Vec<QualifiedFrame> {
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .expect("encode in-memory image");
        ids.iter()
            .enumerate()
            .map(|(index, frame_id)| QualifiedFrame {
                frame_id: *frame_id,
                timestamp: 3.0 + index as f64,
                png: png.get_ref().clone(),
            })
            .collect()
    }

    fn sidecar(frames: &[QualifiedFrame]) -> PrivateSidecar {
        screen_ocr::expected_sidecar(
            "source".to_owned(),
            "segment".to_owned(),
            "input-digest".to_owned(),
            "recipe".to_owned(),
            "policy".to_owned(),
            DecodeCoverage::Complete,
            frames,
        )
    }

    fn ignored_overlap() -> (BTreeMap<u64, Value>, BTreeMap<String, CategoryOverride>) {
        let analyses = BTreeMap::from([
            (
                1,
                json!({"primary":"browsing", "secondary":"messaging", "overlap":true}),
            ),
            (
                2,
                json!({"primary":"code", "secondary":"none", "overlap":false}),
            ),
            (
                3,
                json!({"primary":"browsing", "secondary":"none", "overlap":false}),
            ),
            (
                4,
                json!({"primary":"browsing", "secondary":"none", "overlap":false}),
            ),
        ]);
        let overrides = BTreeMap::from([(
            "messaging".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Ignore),
                extraction: None,
            },
        )]);
        (analyses, overrides)
    }

    #[test]
    fn fake_ocr_outcomes_preserve_successful_cse_observations() {
        let frames = frames(&[1, 2, 3, 4]);
        let (analyses, overrides) = ignored_overlap();
        let ignored = screen_ocr::ignored_frame_ids(&analyses, &overrides);
        assert_eq!(ignored, BTreeSet::from([1]));
        let baseline = CseObservations {
            c_calls: 4,
            selected_ids: vec![2],
            selection_calls: 1,
            extraction_calls: 1,
            frame_rows: vec![json!({"frame_id":2,"enhanced":true,"content":{"code":"detail"}})],
            processing_state: "analyzed",
            processing_reason: "ok",
            header_present: true,
        };

        for behavior in [
            FakeBehavior::Absent,
            FakeBehavior::Success,
            FakeBehavior::Protocol,
            FakeBehavior::Timeout,
        ] {
            let mut coverage = sidecar(&frames);
            let mut factory = FakeFactory::new(behavior);
            let actual: Result<CseObservations, PipelineFailure> = complete_private_ocr(
                Ok(baseline.clone()),
                &mut coverage,
                &frames,
                &ignored,
                &mut factory,
                true,
                &|| false,
                |_| Ok(()),
            );
            assert_eq!(actual, Ok(baseline.clone()));
            assert_eq!(coverage.frames[0].state, CoverageState::NotRun);
            match behavior {
                FakeBehavior::Absent => {
                    assert_eq!(factory.recognizes.get(), 0);
                    assert_eq!(coverage.state, CoverageState::NotRun);
                }
                FakeBehavior::Success => {
                    assert_eq!(factory.recognizes.get(), 3);
                    assert_eq!(coverage.state, CoverageState::Success);
                }
                FakeBehavior::Protocol | FakeBehavior::Timeout => {
                    assert_eq!(factory.recognizes.get(), 1);
                    assert_eq!(coverage.state, CoverageState::Error);
                    assert_eq!(coverage.frames[1].state, CoverageState::Error);
                    assert_eq!(coverage.frames[2].state, CoverageState::NotRun);
                }
            }
        }
    }

    #[test]
    fn unqualified_installed_resolution_records_a_gap_without_changing_cse() {
        let frames = frames(&[1, 2, 3, 4]);
        let ignored = BTreeSet::new();
        let baseline = CseObservations {
            c_calls: 4,
            selected_ids: vec![2],
            selection_calls: 1,
            extraction_calls: 1,
            frame_rows: vec![json!({"frame_id":2,"enhanced":true,"content":{"code":"detail"}})],
            processing_state: "analyzed",
            processing_reason: "ok",
            header_present: true,
        };
        let resolver = UnavailableHelperResolver;
        let mut worker_factory = ResolvedWorkerFactory::new(&resolver);
        let mut coverage = sidecar(&frames);

        let actual: Result<CseObservations, PipelineFailure> = complete_private_ocr(
            Ok(baseline.clone()),
            &mut coverage,
            &frames,
            &ignored,
            &mut worker_factory,
            true,
            &|| false,
            |_| Ok(()),
        );

        assert_eq!(actual, Ok(baseline));
        assert_eq!(coverage.state, CoverageState::NotRun);
        assert!(coverage.frames.iter().all(|frame| {
            frame.state == CoverageState::NotRun
                && frame.error_code.as_deref() == Some("helper_unavailable")
        }));
    }

    #[test]
    fn fake_ocr_runs_before_all_c_failed_and_provider_blocked_returns() {
        let frames = frames(&[1, 2, 3, 4]);
        let no_analyses = BTreeMap::new();
        let ignored = screen_ocr::ignored_frame_ids(&no_analyses, &BTreeMap::new());
        for (failure, expected_calls) in [
            (
                PipelineFailure::AllCategorizationFailed {
                    c_calls: 4,
                    frame_rows: 0,
                    verdict: "failed:analysis_failed",
                    header_present: true,
                },
                4,
            ),
            (
                PipelineFailure::ProviderBlocked {
                    c_calls: 0,
                    selection_calls: 0,
                    extraction_calls: 0,
                    header_present: false,
                },
                4,
            ),
        ] {
            let mut coverage = sidecar(&frames);
            let mut factory = FakeFactory::new(FakeBehavior::Success);
            let actual: Result<(), PipelineFailure> = complete_private_ocr(
                Err(failure.clone()),
                &mut coverage,
                &frames,
                &ignored,
                &mut factory,
                true,
                &|| false,
                |_| Ok(()),
            );
            assert_eq!(actual, Err(failure));
            assert_eq!(factory.recognizes.get(), expected_calls);
            assert_eq!(coverage.state, CoverageState::Success);
        }
    }

    #[test]
    fn primary_secondary_unknown_and_failed_analysis_admission_is_conservative() {
        let frames = frames(&[1, 2, 3, 4, 5]);
        let analyses = BTreeMap::from([
            (
                1,
                json!({"primary":"browsing", "secondary":"messaging", "overlap":true}),
            ),
            (
                2,
                json!({"primary":"messaging", "secondary":"browsing", "overlap":false}),
            ),
            (3, json!({"primary":"unknown-category", "secondary":"none"})),
            // Frame 4 has no analysis and frame 5 represents a C-failed row.
        ]);
        let overrides = BTreeMap::from([(
            "messaging".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Ignore),
                extraction: None,
            },
        )]);
        let ignored = screen_ocr::ignored_frame_ids(&analyses, &overrides);
        assert_eq!(ignored, BTreeSet::from([1, 2]));
        let mut coverage = sidecar(&frames);
        let mut factory = FakeFactory::new(FakeBehavior::Success);
        complete_private_ocr(
            Ok::<(), PipelineFailure>(()),
            &mut coverage,
            &frames,
            &ignored,
            &mut factory,
            true,
            &|| false,
            |_| Ok(()),
        )
        .expect("OCR is best effort");
        assert_eq!(factory.recognizes.get(), 3);
        assert_eq!(coverage.frames[0].state, CoverageState::NotRun);
        assert_eq!(coverage.frames[1].state, CoverageState::NotRun);
        assert!(
            coverage.frames[2..]
                .iter()
                .all(|frame| frame.state == CoverageState::Success)
        );
    }

    #[test]
    fn cached_and_current_category_rows_both_control_ignore_admission() {
        let plan = IncrementalMergePlan {
            reusable_rows: BTreeMap::from([(
                1,
                ExistingDescribeRow {
                    data: json!({"frame_id":1,"analysis":{"primary":"messaging","secondary":"none"}}),
                    raw_line: String::new(),
                },
            )]),
            phase1_gap_ids: BTreeSet::new(),
            phase3_gaps: BTreeMap::from([(
                2,
                (
                    json!({"analysis":{"primary":"browsing","secondary":"code","overlap":true}}),
                    vec!["code"],
                ),
            )]),
        };
        let current = [super::CategorizedRow {
            frame_id: 3,
            timestamp: 0.0,
            png: Vec::new(),
            analysis: Some(json!({"primary":"unknown-category","secondary":"none"})),
            error: None,
            requests: Vec::new(),
            detection: None,
        }];
        let analyses = super::ocr_analyses(Some(&plan), &current);
        let overrides = BTreeMap::from([
            (
                "messaging".to_owned(),
                CategoryOverride {
                    importance: Some(Importance::Ignore),
                    extraction: None,
                },
            ),
            (
                "code".to_owned(),
                CategoryOverride {
                    importance: Some(Importance::Ignore),
                    extraction: None,
                },
            ),
        ]);
        assert_eq!(
            screen_ocr::ignored_frame_ids(&analyses, &overrides),
            BTreeSet::from([1, 2])
        );
        assert!(analyses.contains_key(&3));
    }

    #[test]
    fn cache_reuse_skips_worker_and_publication_failure_is_best_effort() {
        let frames = frames(&[1]);
        let mut coverage = sidecar(&frames);
        coverage.state = CoverageState::Success;
        coverage.frames[0].state = CoverageState::Success;
        let mut factory = FakeFactory::new(FakeBehavior::Success);
        let baseline = CseObservations {
            c_calls: 1,
            selected_ids: vec![1],
            selection_calls: 0,
            extraction_calls: 1,
            frame_rows: vec![json!({"frame_id":1,"enhanced":true})],
            processing_state: "analyzed",
            processing_reason: "ok",
            header_present: true,
        };
        let ignored = BTreeSet::new();
        assert_eq!(
            complete_private_ocr::<CseObservations, PipelineFailure>(
                Ok(baseline.clone()),
                &mut coverage,
                &frames,
                &ignored,
                &mut factory,
                false,
                &|| false,
                |_| Ok(()),
            ),
            Ok(baseline.clone())
        );
        assert_eq!(factory.creates, 0);

        let mut failed_write_coverage = sidecar(&frames);
        let mut failed_write_factory = FakeFactory::new(FakeBehavior::Success);
        assert_eq!(
            complete_private_ocr::<CseObservations, PipelineFailure>(
                Ok(baseline.clone()),
                &mut failed_write_coverage,
                &frames,
                &ignored,
                &mut failed_write_factory,
                true,
                &|| false,
                |_| Err(()),
            ),
            Ok(baseline)
        );
        assert_eq!(failed_write_factory.recognizes.get(), 1);
        assert_eq!(failed_write_coverage.state, CoverageState::Success);
    }
}

fn ocr_analyses(
    plan: Option<&merge::IncrementalMergePlan>,
    categorized: &[CategorizedRow],
) -> BTreeMap<u64, Value> {
    let mut analyses = BTreeMap::new();
    if let Some(plan) = plan {
        for row in plan.reusable_rows.values() {
            if let Some(analysis) = row.data.get("analysis").filter(|value| value.is_object())
                && let Some(frame_id) = row.data.get("frame_id").and_then(Value::as_u64)
            {
                analyses.insert(frame_id, analysis.clone());
            }
        }
        for (frame_id, (row, _)) in &plan.phase3_gaps {
            if let Some(analysis) = row.get("analysis").filter(|value| value.is_object()) {
                analyses.insert(*frame_id, analysis.clone());
            }
        }
    }
    for row in categorized {
        if let Some(analysis) = row.analysis.as_ref() {
            analyses.insert(row.frame_id, analysis.clone());
        }
    }
    analyses
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

fn stamp_processing(
    record: &mut Value,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<(), RunError> {
    match attempt {
        Some(attempt) => solstone_core_system::work_retry::raw::stamp_attempt_from(record, attempt),
        None => solstone_core_system::work_retry::raw::stamp_attempt(record),
    }
    .map_err(RunError::Internal)
}

fn stamp_failed(
    record: &mut Value,
    input: &InputObservation,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<(), RunError> {
    stamp_processing(record, attempt)?;
    if record.get("state").and_then(Value::as_str) == Some(vocab::STATE_FAILED) {
        // The final checked commit revalidates these exact admitted bytes.
        // Do not perform another uncancellable whole-input hash for bookkeeping.
        record["input_sha256"] = json!(
            input
                .input_sha256()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
    }
    Ok(())
}

fn promote(promotion: Promotion<'_>) -> Result<(), RunError> {
    check_promotion(&promotion)?;
    if let Some(existing) = retained_reading(&promotion)? {
        return keep_existing_reading(&promotion, existing);
    }
    let prepared = crate::publication::StagedFile::prepare(promotion.output, promotion.checkpoint, |final_file| {
        (|| -> Result<(), RunError> {
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
        if let Some(model) = &promotion.model {
            header.insert("_solstone_thinking".to_owned(), json!({"model":model}));
        }
        let mut record = json!({"schema":vocab::SCHEMA,"state":promotion.state,"reason_code":promotion.reason,"handler":vocab::HANDLER_DESCRIBE,"attempted_at":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),"input_size":promotion.input_size});
        stamp_processing(&mut record, promotion.attempt)?;
        if promotion.state == vocab::STATE_FAILED {
            record["attempts"] = json!(promotion.previous_attempts + 1);
            stamp_failed(&mut record, promotion.input, promotion.attempt)?;
        }
        header.insert("_solstone_processing".to_owned(), record);
        writeln!(final_file, "{}", Value::Object(header))
            .map_err(|error| RunError::Internal(error.to_string()))?;
        let mut row_file =
            File::open(promotion.rows).map_err(|error| RunError::Internal(error.to_string()))?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_promotion(&promotion)?;
            let count = row_file
                .read(&mut buffer)
                .map_err(|error| RunError::Internal(error.to_string()))?;
            if count == 0 {
                break;
            }
            final_file
                .write_all(&buffer[..count])
                .map_err(|error| RunError::Internal(error.to_string()))?;
        }
        Ok(())
        })().map_err(|error| match error {
            RunError::Cancelled => std::io::Error::other("describe cancelled"),
            RunError::Internal(detail) => std::io::Error::other(detail),
            RunError::Blocked(detail) => std::io::Error::other(format!("describe blocked: {detail:?}")),
        })
    }).map_err(|error| promotion_error(&promotion, error))?;
    install_promoted(&promotion, &prepared, true)
}

fn promotion_error(promotion: &Promotion<'_>, error: std::io::Error) -> RunError {
    if error.to_string() == "describe cancelled" || (promotion.checkpoint)() {
        RunError::Cancelled
    } else {
        RunError::Internal(error.to_string())
    }
}

fn install_promoted(
    promotion: &Promotion<'_>,
    prepared: &crate::publication::StagedFile,
    publish_ocr: bool,
) -> Result<(), RunError> {
    prepared
        .validate_stage()
        .map_err(|error| promotion_error(promotion, error))?;
    check_promotion(promotion)?;
    let _commit = promotion
        .input
        .commit_checked(promotion.claims, promotion.checkpoint)
        .map_err(|error| {
            if (promotion.checkpoint)() {
                RunError::Cancelled
            } else {
                RunError::Internal(error.to_string())
            }
        })?;
    check_promotion(promotion)?;
    if let Some(ocr) = promotion.ocr {
        ocr.validate_authority()
            .map_err(|error| promotion_error(promotion, error))?;
    }
    check_promotion(promotion)?;
    prepared
        .install()
        .map_err(|error| RunError::Internal(error.to_string()))?;
    if publish_ocr
        && let Some(ocr) = promotion.ocr
        && let Err(error) = ocr.install(&_commit)
    {
        log::warn!("screen OCR publication failed: {:?}", error.kind());
    }
    Ok(())
}

fn check_promotion(promotion: &Promotion<'_>) -> Result<(), RunError> {
    if (promotion.checkpoint)() {
        Err(RunError::Cancelled)
    } else {
        Ok(())
    }
}

/// Return the prior reading a retry must keep: one with more analyzed frames
/// than an incomplete new outcome (a failed or empty retry never shrinks it).
fn retained_reading<'a>(
    promotion: &Promotion<'a>,
) -> Result<Option<&'a merge::ExistingDescribeArtifact>, RunError> {
    let Some(existing) = promotion.existing else {
        return Ok(None);
    };
    if promotion.state == vocab::STATE_ANALYZED {
        return Ok(None);
    }
    let Some(rows) = existing.rows.as_ref() else {
        return Ok(None);
    };
    let kept = rows
        .iter()
        .filter(|row| merge::is_analyzed(&row.data))
        .count();
    if kept == 0 {
        return Ok(None);
    }
    let file = File::open(promotion.rows).map_err(|error| RunError::Internal(error.to_string()))?;
    let mut fresh = 0;
    for line in BufReader::new(file).lines() {
        check_promotion(promotion)?;
        let line = line.map_err(|error| RunError::Internal(error.to_string()))?;
        if serde_json::from_str::<Value>(&line)
            .ok()
            .is_some_and(|row| merge::is_analyzed(&row))
        {
            fresh += 1;
        }
    }
    Ok((kept > fresh).then_some(existing))
}

/// Rewrite the prior reading unchanged except for this attempt's bookkeeping,
/// so the bounded retry still exhausts.
fn keep_existing_reading(
    promotion: &Promotion<'_>,
    existing: &merge::ExistingDescribeArtifact,
) -> Result<(), RunError> {
    let mut header = existing.header.clone();
    let mut record = existing.record.clone();
    record["attempted_at"] =
        json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    record["attempts"] = json!(promotion.previous_attempts + 1);
    stamp_failed(&mut record, promotion.input, promotion.attempt)?;
    header["_solstone_processing"] = record;
    let prepared = crate::publication::StagedFile::prepare(
        promotion.output,
        promotion.checkpoint,
        |final_file| {
            writeln!(final_file, "{header}")?;
            for row in existing.rows.iter().flatten() {
                final_file.write_all(row.raw_line.as_bytes())?;
            }
            Ok(())
        },
    )
    .map_err(|error| promotion_error(promotion, error))?;
    install_promoted(promotion, &prepared, false)
}

#[cfg(all(test, not(feature = "full-tests")))]
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

#[cfg(all(test, feature = "full-tests"))]
#[path = "pipeline_tests.rs"]
mod pipeline_tests;
