// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Standalone native still-image depiction handler.
//!
//! `journal depict` reaches this handler through the explicit native process
//! table.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::time::Duration;
#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use image::{DynamicImage, GenericImageView, ImageFormat, imageops::FilterType};
use serde_json::{Map, Value, json};
#[cfg(any(not(windows), test))]
use solstone_core_assets::canonical_host_pair;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient, RefusalReason,
};
#[cfg(not(windows))]
use solstone_core_journal::{
    detect_checkout_root, discover_home, read_config_journal, resolve_journal_path,
};
use solstone_core_journal_io::LockOptions;
use solstone_core_journal_io::{AtomicWriteOptions, write_jsonl};
use solstone_core_local::install::rfdetr_install::ENGINE_PROVENANCE_REF;
#[cfg(any(not(windows), test))]
use solstone_core_local::install::rfdetr_install::{
    RfdetrInstallError, RfdetrInstallRecord, binary_path, check_rfdetr_model, model_path,
    rfdetr_artifact_key,
};
use solstone_core_processing_record::{
    TerminalProofOutcome, evaluate_terminal_proof, read_processing_record_header, record_attempts,
    should_reenter_analysis_output, vocab,
};
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};

pub const ERROR_SCHEMA: &str = "solstone-depict-error-v1";
pub const DESCRIPTION_PROMPT: &str = "Describe this image in detail. Include any visible text, people, objects, setting, and notable context. Return a concise natural-language description.";
pub const USAGE: &str = "usage: solstone journal depict [-h] [--redo] [-v] [-d] FILE\n";
const MAX_VLM_DIM: u32 = 1920;
const ENGINE_NAME: &str = "rf-detr.cpp";
const MODEL_NAME: &str = "rfdetr-nano-f16";
const THRESHOLD: f64 = 0.25;
#[cfg(not(windows))]
const RFDETR_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(not(windows))]
const RFDETR_UNAVAILABLE_DETAIL: &str = "Object detection is unavailable. Run `solstone journal install-models` to check or repair the RF-DETR assets.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arguments {
    pub image_path: PathBuf,
    pub redo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepictError {
    Help,
    Usage(String),
    Image(String),
    Wire {
        detail: String,
        blocking: bool,
        reason_code: Option<String>,
    },
    Metadata(String),
    Output(String),
    Cancelled,
}

impl DepictError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Help | Self::Usage(_) => "malformed-request",
            Self::Image(_) => "image-invalid",
            Self::Wire { .. } => "generate-wire-failed",
            Self::Metadata(_) => "metadata-invalid",
            Self::Output(_) => "output-unwritable",
            Self::Cancelled => "depict-cancelled",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Help => 0,
            // This is the handler namespace: all failures are ordinary non-hold-raw.
            _ => 1,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Help => USAGE,
            Self::Cancelled => "depict cancelled",
            Self::Usage(detail)
            | Self::Image(detail)
            | Self::Wire { detail, .. }
            | Self::Metadata(detail)
            | Self::Output(detail) => detail,
        }
    }

    pub fn blocking(&self) -> bool {
        match self {
            Self::Wire { blocking, .. } => *blocking,
            Self::Help
            | Self::Usage(_)
            | Self::Image(_)
            | Self::Metadata(_)
            | Self::Output(_)
            | Self::Cancelled => false,
        }
    }

    pub fn reason_code(&self) -> Option<&str> {
        match self {
            Self::Wire { reason_code, .. } => reason_code.as_deref(),
            Self::Help
            | Self::Usage(_)
            | Self::Image(_)
            | Self::Metadata(_)
            | Self::Output(_)
            | Self::Cancelled => None,
        }
    }
}

pub fn error_json_line(error: &DepictError) -> String {
    json!({
        "schema": ERROR_SCHEMA,
        "reason": error.reason(),
        "detail": error.detail(),
        "blocking": error.blocking(),
        "reason_code": error.reason_code(),
    })
    .to_string()
}

pub fn parse_args(args: &[OsString]) -> Result<Arguments, DepictError> {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        return Err(DepictError::Help);
    }
    let mut image_path = None;
    let mut redo = false;
    for argument in args {
        if argument == "-v" || argument == "--verbose" || argument == "-d" || argument == "--debug"
        {
            continue;
        }
        if argument == "--redo" {
            if redo {
                return Err(DepictError::Usage(USAGE.to_owned()));
            }
            redo = true;
            continue;
        }
        if argument
            .to_str()
            .is_some_and(|value| value.starts_with('-'))
            || image_path.replace(PathBuf::from(argument)).is_some()
        {
            return Err(DepictError::Usage(USAGE.to_owned()));
        }
    }
    image_path
        .map(|image_path| Arguments { image_path, redo })
        .ok_or_else(|| DepictError::Usage(USAGE.to_owned()))
}

pub trait WireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError>;
}

pub trait Detector {
    fn detect(&self, full_png: &[u8]) -> Result<Option<Value>, String>;
}

pub struct SystemWireClient;

impl WireClient for SystemWireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        OneShotClient::sibling()?.execute(request)
    }
}

pub struct SystemDetector;

#[cfg(windows)]
impl Detector for SystemDetector {
    fn detect(&self, full_png: &[u8]) -> Result<Option<Value>, String> {
        use solstone_core_local::install::rfdetr_windows::{
            map_rfdetr_detect_completion, rfdetr_windows_detect_launch,
            verified_windows_rfdetr_package,
        };
        use solstone_core_system::process::{
            BoundedHelperBudget, BoundedHelperRequest, BoundedHelperResourceLimits,
            run_bounded_helper,
        };

        let package = verified_windows_rfdetr_package().map_err(|_| rfdetr_unavailable())?;
        let system_root = env::var_os("SystemRoot")
            .filter(|val| !val.is_empty())
            .ok_or_else(rfdetr_unavailable)?;
        let temporary =
            std::sync::Arc::new(DetectorTempDir::new().map_err(|_| rfdetr_unavailable())?);
        let input = temporary.path.join("input.png");
        let output = temporary.path.join("output.json");
        fs::write(&input, full_png).map_err(|_| rfdetr_unavailable())?;

        let spec = rfdetr_windows_detect_launch(&package, system_root, &input, &output);
        let mut resources = solstone_core_system::process::BoundedHelperResources::new();
        resources.retain(temporary.clone());
        let request = BoundedHelperRequest {
            resources,
            package_root: spec.package_root,
            executable: spec.executable,
            current_directory: spec.current_directory,
            arguments: spec.arguments,
            environment: spec.environment,
            stdin: spec.stdin,
            budget: BoundedHelperBudget {
                timeout: spec.timeout,
                stdin_limit_bytes: spec.stdin_limit_bytes,
                stdout_limit_bytes: spec.stdout_limit_bytes,
                stderr_limit_bytes: spec.stderr_limit_bytes,
            },
            resource_limits: Some(BoundedHelperResourceLimits {
                cpu_rate_per_10_000: spec.cpu_rate_per_10_000,
                committed_memory_bytes: spec.committed_memory_bytes,
            }),
            stdout_sink: None,
            cancellation: None,
        };
        let helper_output = run_bounded_helper(request).map_err(|_| rfdetr_unavailable())?;
        let parsed = map_rfdetr_detect_completion(helper_output.exit_code == 0, &output)
            .map_err(|_| rfdetr_unavailable())?;
        Ok(Some(parsed))
    }
}

#[cfg(not(windows))]
impl Detector for SystemDetector {
    fn detect(&self, full_png: &[u8]) -> Result<Option<Value>, String> {
        let journal = current_journal_path().ok_or_else(rfdetr_unavailable)?;
        let (binary, model) = rfdetr_paths_at(&journal, env::consts::OS, env::consts::ARCH)
            .map_err(|_| rfdetr_unavailable())?;
        let parsed =
            solstone_core_system::rfdetr::detect(full_png, &binary, &model, RFDETR_TIMEOUT)
                .map_err(|_| rfdetr_unavailable())?;
        Ok(Some(parsed))
    }
}

fn rfdetr_unavailable() -> String {
    #[cfg(windows)]
    {
        solstone_core_local::install::rfdetr_windows::RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE.to_owned()
    }
    #[cfg(not(windows))]
    {
        RFDETR_UNAVAILABLE_DETAIL.to_owned()
    }
}

#[cfg(not(windows))]
fn current_journal_path() -> Option<PathBuf> {
    let env_journal = env::var_os("SOLSTONE_JOURNAL");
    if let Some(path) = env_journal.as_deref().filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let fallback_home = env::home_dir();
    let home_env = env::var_os("HOME");
    let home = discover_home(home_env.as_deref(), fallback_home.as_deref()).ok()?;
    let config_journal = read_config_journal(&home).ok().flatten();
    let checkout_root = env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .and_then(|directory| directory.ancestors().find_map(detect_checkout_root));
    Some(
        resolve_journal_path(
            env_journal.as_deref(),
            config_journal.as_deref(),
            checkout_root.as_deref(),
            &home,
        )
        .path,
    )
}

#[cfg(any(not(windows), test))]
fn rfdetr_paths_at(
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> Result<(PathBuf, PathBuf), String> {
    let (os_name, arch) = canonical_host_pair(os_name, arch);
    if solstone_core_local::install::rfdetr_install::rfdetr_uses_package_payload(os_name, arch) {
        let package =
            solstone_core_local::install::rfdetr_windows::verified_windows_rfdetr_package()
                .map_err(|err| err.to_string())?;
        return Ok((package.binary, package.model));
    }
    let key = rfdetr_artifact_key(os_name, arch).ok_or_else(rfdetr_unavailable)?;
    rfdetr_paths_from_install_check(check_rfdetr_model(journal, os_name, arch), journal, key)
}

#[cfg(any(not(windows), test))]
fn rfdetr_paths_from_install_check(
    result: Result<RfdetrInstallRecord, RfdetrInstallError>,
    journal: &Path,
    key: &str,
) -> Result<(PathBuf, PathBuf), String> {
    match result {
        Ok(RfdetrInstallRecord::Installed) => Ok((binary_path(journal, key), model_path(journal))),
        Ok(RfdetrInstallRecord::PlatformUnavailable) | Err(_) => Err(rfdetr_unavailable()),
    }
}

#[cfg(windows)]
struct DetectorTempDir {
    path: PathBuf,
}

#[cfg(windows)]
impl DetectorTempDir {
    fn new() -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = env::temp_dir().join(format!("rfdetr_{}_{nonce}", std::process::id()));
        fs::create_dir(&path).map_err(|error| error.to_string())?;
        Ok(Self { path })
    }
}

#[cfg(windows)]
impl Drop for DetectorTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Written,
    Skipped,
    NoEngine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Description {
    Generated(String),
    NoEngine,
}

fn wire_error(detail: String, blocking: bool, reason_code: Option<String>) -> DepictError {
    DepictError::Wire {
        detail,
        blocking,
        reason_code,
    }
}

fn interpret_generate(
    response: Result<GenerateResponse, ClientError>,
) -> Result<Description, DepictError> {
    match response {
        Ok(GenerateResponse::Generated(generated)) => Ok(Description::Generated(generated.text)),
        Ok(GenerateResponse::Refused(refusal))
            if refusal.reason == RefusalReason::NoEngineConfigured =>
        {
            Ok(Description::NoEngine)
        }
        Ok(GenerateResponse::Refused(refusal)) => Err(wire_error(
            format!("{}: {}", refusal.reason.as_str(), refusal.detail),
            refusal.blocking,
            refusal.reason_code.map(|code| code.as_wire().to_owned()),
        )),
        Ok(GenerateResponse::Cancelled) => {
            Err(wire_error("generate cancelled".to_owned(), true, None))
        }
        Err(error @ ClientError::Protocol(_)) => Err(wire_error(error.to_string(), true, None)),
        Err(ClientError::Decode(detail)) => Err(wire_error(detail, true, None)),
        Err(
            error @ (ClientError::Io { .. }
            | ClientError::ProcessIo(_)
            | ClientError::InvalidResponse(_)
            | ClientError::UnexpectedChild(_)),
        ) => Err(wire_error(error.to_string(), true, None)),
        Err(ClientError::Resolve(detail)) => Err(wire_error(detail, true, None)),
    }
}

fn build_generate_request(prepared_png: &[u8]) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "observe.depict".to_owned(),
        contents: vec![
            ContentPart::Text {
                text: DESCRIPTION_PROMPT.to_owned(),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(prepared_png),
            },
        ],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 256,
        timeout_s: None,
        json_output: false,
        json_schema: None,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

pub fn run_with_clients(
    image_path: &Path,
    redo: bool,
    wire: &dyn WireClient,
    detector: &dyn Detector,
) -> Result<RunOutcome, DepictError> {
    let attempt = solstone_core_system::work_retry::raw::retry_attempt_from_environment()
        .map_err(DepictError::Output)?;
    run_with_clients_and_checkpoint(
        image_path,
        redo,
        wire,
        detector,
        &|| false,
        attempt.as_ref(),
    )
}

fn stamp_attempt(
    record: &mut Value,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<(), String> {
    match attempt {
        Some(attempt) => solstone_core_system::work_retry::raw::stamp_attempt_from(record, attempt),
        None => solstone_core_system::work_retry::raw::stamp_attempt(record),
    }
}

fn stamp_failed_input(
    record: &mut Value,
    input: &Path,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<(), String> {
    match attempt {
        Some(attempt) => {
            solstone_core_system::work_retry::raw::stamp_failed_input_from(record, input, attempt)
        }
        None => solstone_core_system::work_retry::raw::stamp_failed_input(record, input),
    }
}

pub fn run_with_clients_and_checkpoint(
    image_path: &Path,
    redo: bool,
    wire: &dyn WireClient,
    detector: &dyn Detector,
    checkpoint: &dyn Fn() -> bool,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<RunOutcome, DepictError> {
    run_depict(
        image_path,
        redo,
        wire,
        detector,
        &FailurePublicationHooks::default(),
        checkpoint,
        attempt,
    )
}

#[derive(Default)]
struct FailurePublicationHooks<'a> {
    before_lock: Option<&'a dyn Fn()>,
    before_replace: Option<&'a dyn Fn()>,
}

fn run_depict(
    image_path: &Path,
    redo: bool,
    wire: &dyn WireClient,
    detector: &dyn Detector,
    hooks: &FailurePublicationHooks<'_>,
    checkpoint: &dyn Fn() -> bool,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<RunOutcome, DepictError> {
    if checkpoint() {
        return Err(DepictError::Cancelled);
    }
    validate_image_path(image_path)?;
    let output_path = image_path.with_extension("jsonl");
    let claims = OutputClaims::acquire([output_path.clone()], LockOptions::default())
        .map_err(|error| DepictError::Output(error.to_string()))?;
    let input = InputObservation::capture(&claims, image_path)
        .map_err(|error| DepictError::Output(error.to_string()))?;
    if checkpoint() {
        return Err(DepictError::Cancelled);
    }
    let admitted = if redo {
        false
    } else if let Some(attempt) = attempt {
        let journal = image_path
            .ancestors()
            .find(|p| p.join("chronicle").is_dir())
            .ok_or_else(|| DepictError::Output("native retry journal unavailable".into()))?;
        let unit = solstone_core_system::work_retry::raw::unit_for_output(
            journal,
            &output_path,
            vocab::HANDLER_DEPICT,
        )
        .map_err(DepictError::Output)?;
        solstone_core_system::work_retry::raw::consume_context_checked(
            journal,
            &output_path,
            vocab::HANDLER_DEPICT,
            &solstone_core_system::work_retry::raw::RetryContext {
                unit_id: unit.id().map_err(DepictError::Output)?,
                attempt: attempt.clone(),
            },
            &|| {
                if checkpoint() {
                    Err("depict cancelled".into())
                } else {
                    Ok(())
                }
            },
        )
        .map_err(DepictError::Output)?
        .is_some()
    } else {
        solstone_core_system::work_retry::raw::consume_for_output(
            &output_path,
            vocab::HANDLER_DEPICT,
        )
        .map_err(DepictError::Output)?
    };
    if checkpoint() {
        return Err(DepictError::Cancelled);
    }
    if !redo && output_path.exists() {
        let record = read_processing_record_header(&output_path);
        if !admitted
            && !should_reenter_analysis_output(record.as_ref(), &output_path, vocab::HANDLER_DEPICT)
        {
            return Ok(RunOutcome::Skipped);
        }
    }
    let publication = FailurePublication {
        output_path: &output_path,
        image_path,
        claims: &claims,
        input: &input,
    };
    // A missing read has no bytes to size a processing record against.
    let source = fs::read(image_path).map_err(|error| DepictError::Image(error.to_string()))?;
    let image = match image::load_from_memory(&source) {
        Ok(image) => image,
        Err(error) => {
            return publish_failed_analysis(
                &publication,
                DepictError::Image(error.to_string()),
                vocab::REASON_CORRUPT_INPUT,
                redo,
                hooks,
                checkpoint,
                attempt,
            );
        }
    };
    let full_png = match encode_png(&image) {
        Ok(png) => png,
        Err(error) => {
            return publish_failed_analysis(
                &publication,
                error,
                vocab::REASON_CORRUPT_INPUT,
                redo,
                hooks,
                checkpoint,
                attempt,
            );
        }
    };
    let prepared = resize_for_vlm(image);
    let prepared_png = match encode_png(&prepared) {
        Ok(png) => png,
        Err(error) => {
            return publish_failed_analysis(
                &publication,
                error,
                vocab::REASON_CORRUPT_INPUT,
                redo,
                hooks,
                checkpoint,
                attempt,
            );
        }
    };
    let description = match interpret_generate(wire.execute(&build_generate_request(&prepared_png)))
    {
        Ok(Description::Generated(description)) => description.trim().to_owned(),
        Ok(Description::NoEngine) => return Ok(RunOutcome::NoEngine),
        Err(error) => {
            return publish_failed_analysis(
                &publication,
                error,
                vocab::REASON_ANALYSIS_FAILED,
                redo,
                hooks,
                checkpoint,
                attempt,
            );
        }
    };
    let mut header = build_header(&image_path.file_name().unwrap_or_default().to_string_lossy())?;
    header.insert(
        "_solstone_processing".to_owned(),
        json!({
            "schema": vocab::SCHEMA,
            "state": vocab::STATE_ANALYZED,
            "reason_code": vocab::REASON_OK,
            "handler": vocab::HANDLER_DEPICT,
            "attempted_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "input_size": source.len() as u64,
        }),
    );
    stamp_attempt(
        header
            .get_mut("_solstone_processing")
            .expect("processing record"),
        attempt,
    )
    .map_err(DepictError::Output)?;
    let mut entry = Map::new();
    entry.insert("start".to_owned(), Value::String("00:00:00".to_owned()));
    entry.insert("text".to_owned(), Value::String(description));
    match detector.detect(&full_png) {
        Ok(Some(detection)) => match detections_block(detection) {
            Ok(block) => {
                entry.insert("detections".to_owned(), block);
            }
            Err(_) => insert_detection_error(&mut entry),
        },
        Ok(None) => {}
        Err(_) => insert_detection_error(&mut entry),
    }
    if checkpoint() {
        return Err(DepictError::Cancelled);
    }
    let _commit = input
        .commit(&claims)
        .map_err(|error| DepictError::Output(error.to_string()))?;
    if checkpoint() {
        return Err(DepictError::Cancelled);
    }
    write_jsonl(
        &output_path,
        [Value::Object(header), Value::Object(entry)],
        AtomicWriteOptions::default(),
    )
    .map_err(|error| DepictError::Output(error.to_string()))?;
    Ok(RunOutcome::Written)
}

/// Write a header-only failure, unless a successful analysis is already published.
///
/// The publication lock covers the fresh read and replacement; successful
/// writers use the same lock. A held depict proof keeps those bytes and the
/// original error is returned either way.
struct FailurePublication<'a> {
    output_path: &'a Path,
    image_path: &'a Path,
    claims: &'a OutputClaims,
    input: &'a InputObservation,
}

fn publish_failed_analysis(
    publication: &FailurePublication<'_>,
    error: DepictError,
    reason_code: &str,
    redo: bool,
    hooks: &FailurePublicationHooks<'_>,
    checkpoint: &dyn Fn() -> bool,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<RunOutcome, DepictError> {
    let FailurePublication {
        output_path,
        image_path,
        claims,
        input,
    } = publication;
    let input_size = input.input_size();
    if checkpoint() {
        return Err(DepictError::Cancelled);
    }
    if let Some(before_lock) = hooks.before_lock {
        before_lock();
    }
    let _commit = match input.commit(claims) {
        Ok(lock) => lock,
        Err(persist) => return Err(join_persistence(error, &persist.to_string())),
    };
    let existing = read_processing_record_header(output_path);
    if let Some(record) = existing.as_ref() {
        let recorded_size = record
            .get("input_size")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if evaluate_terminal_proof(Some(record), vocab::HANDLER_DEPICT, recorded_size)
            == TerminalProofOutcome::Held
        {
            return Err(error);
        }
    }
    let previous = if redo {
        0
    } else if existing
        .as_ref()
        .and_then(|record| record.get("state"))
        .and_then(Value::as_str)
        == Some(vocab::STATE_FAILED)
    {
        record_attempts(existing.as_ref().expect("failed record"))
    } else {
        0
    };
    let header = match failure_document(image_path, input_size, reason_code, previous + 1, attempt)
    {
        Ok(header) => header,
        Err(persist) => return Err(join_persistence(error, persist.detail())),
    };
    if checkpoint() {
        return Err(DepictError::Cancelled);
    }
    if let Some(before_replace) = hooks.before_replace {
        before_replace();
    }
    if checkpoint() {
        return Err(DepictError::Cancelled);
    }
    if let Err(persist) = write_jsonl(
        output_path,
        [Value::Object(header)],
        AtomicWriteOptions::default(),
    ) {
        return Err(join_persistence(error, &persist.to_string()));
    }
    Err(error)
}

fn failure_document(
    image_path: &Path,
    input_size: u64,
    reason_code: &str,
    attempts: i64,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> Result<Map<String, Value>, DepictError> {
    let mut header = build_header(&image_path.file_name().unwrap_or_default().to_string_lossy())?;
    header.insert(
        "_solstone_processing".to_owned(),
        json!({
            "schema": vocab::SCHEMA,
            "state": vocab::STATE_FAILED,
            "reason_code": reason_code,
            "handler": vocab::HANDLER_DEPICT,
            "attempted_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "input_size": input_size,
            "attempts": attempts,
        }),
    );
    stamp_failed_input(
        header
            .get_mut("_solstone_processing")
            .expect("failed record"),
        image_path,
        attempt,
    )
    .map_err(DepictError::Output)?;
    Ok(header)
}

fn join_persistence(error: DepictError, persist: &str) -> DepictError {
    let detail = format!("{}; {persist}", error.detail());
    match error {
        DepictError::Image(_) => DepictError::Image(detail),
        DepictError::Wire {
            blocking,
            reason_code,
            ..
        } => DepictError::Wire {
            detail,
            blocking,
            reason_code,
        },
        other => other,
    }
}

pub fn resize_for_vlm(image: DynamicImage) -> DynamicImage {
    let (width, height) = vlm_dimensions(image.width(), image.height());
    if (width, height) == image.dimensions() {
        image
    } else {
        image.resize_exact(width, height, FilterType::Lanczos3)
    }
}

fn vlm_dimensions(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= MAX_VLM_DIM {
        return (width, height);
    }
    (
        ((u64::from(width) * u64::from(MAX_VLM_DIM)) / u64::from(longest))
            .max(1)
            .try_into()
            .expect("scaled image width fits u32"),
        ((u64::from(height) * u64::from(MAX_VLM_DIM)) / u64::from(longest))
            .max(1)
            .try_into()
            .expect("scaled image height fits u32"),
    )
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, DepictError> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|error| DepictError::Image(error.to_string()))?;
    Ok(bytes.into_inner())
}

fn validate_image_path(path: &Path) -> Result<(), DepictError> {
    if !path.is_file() {
        return Err(DepictError::Usage(format!(
            "Image not found: {}",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !is_segment_key(parent) {
        return Err(DepictError::Usage(format!(
            "Image must be in a segment directory (HHMMSS_LEN/), but parent is: {parent}"
        )));
    }
    Ok(())
}

fn is_segment_key(value: &str) -> bool {
    let Some((clock, suffix)) = value.split_once('_') else {
        return false;
    };
    if clock.len() != 6 || !clock.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let digit_count = suffix
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    digit_count > 0 && (digit_count == suffix.len() || suffix.as_bytes()[digit_count] == b'_')
}

pub fn build_header(raw_name: &str) -> Result<Map<String, Value>, DepictError> {
    let observer = env::var("OBSERVER_NAME").ok();
    let segment_meta = env::var("SEGMENT_META").ok();
    build_header_from_values(raw_name, observer.as_deref(), segment_meta.as_deref())
}

fn build_header_from_values(
    raw_name: &str,
    observer: Option<&str>,
    segment_meta: Option<&str>,
) -> Result<Map<String, Value>, DepictError> {
    let mut header = Map::new();
    header.insert("raw".to_owned(), Value::String(raw_name.to_owned()));
    header.insert("kind".to_owned(), Value::String("image".to_owned()));
    if let Some(observer) = observer.filter(|observer| !observer.is_empty()) {
        header.insert("observer".to_owned(), Value::String(observer.to_owned()));
    }
    if let Some(segment_meta) = segment_meta.filter(|segment_meta| !segment_meta.is_empty()) {
        match serde_json::from_str::<Value>(segment_meta) {
            Ok(Value::Object(meta)) => header.extend(meta),
            Ok(_) => {
                return Err(DepictError::Metadata(
                    "SEGMENT_META must be an object".to_owned(),
                ));
            }
            Err(_) => log::warn!("native depict: invalid SEGMENT_META JSON"),
        }
    }
    Ok(header)
}

fn detections_block(value: Value) -> Result<Value, String> {
    let object = value
        .as_object()
        .ok_or("detector output is not an object")?;
    let image = object
        .get("image")
        .ok_or("detector output has no image")?
        .clone();
    let objects = object
        .get("detections")
        .ok_or("detector output has no detections")?
        .clone();
    Ok(json!({
        "engine": ENGINE_NAME,
        "engine_ref": ENGINE_PROVENANCE_REF,
        "model": MODEL_NAME,
        "threshold": THRESHOLD,
        "source": "still",
        "gate": "still",
        "image": image,
        "objects": objects,
    }))
}

fn insert_detection_error(entry: &mut Map<String, Value>) {
    let detail = rfdetr_unavailable();
    log::warn!("{detail}");
    entry.insert(
        "detection_error".to_owned(),
        json!({
            "reason_code": "rfdetr-unavailable",
            "detail": detail,
        }),
    );
}

pub fn run(arguments: Arguments) -> Result<RunOutcome, DepictError> {
    run_with_clients(
        &arguments.image_path,
        arguments.redo,
        &SystemWireClient,
        &SystemDetector,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use sha2::{Digest, Sha256};
    use solstone_core_generate::{
        CapturedStream, ChildStatus, GeneratedResponse, ProtocolError, ProtocolFailure, ReasonCode,
        ReasonCodeValue, RefusedResponse, UnexpectedChildFailure, decode_one_shot_response,
        decode_protocol_error,
    };
    use solstone_core_local::install::{
        rfdetr_install::{EngineSpec, ModelSpec},
        test_hooks::check_rfdetr_model_with_fixture_artifacts,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DEPICT_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_depict_env() -> std::sync::MutexGuard<'static, ()> {
        DEPICT_ENV
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn generated(text: &str) -> GenerateResponse {
        GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test-model".to_owned(),
            usage: json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))
    }

    fn refused(
        reason: RefusalReason,
        reason_code: Option<&str>,
        blocking: bool,
    ) -> GenerateResponse {
        GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason,
            reason_code: reason_code.map(|code| {
                ReasonCodeValue::Known(ReasonCode::new(code).expect("test reason code is known"))
            }),
            retryable: false,
            blocking,
            reset_at_ms: None,
            provider: None,
            detail: "wire detail".to_owned(),
        })
    }

    struct SuccessWire;
    impl WireClient for SuccessWire {
        fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            Ok(generated("  detail  "))
        }
    }

    struct NoEngineWire;
    impl WireClient for NoEngineWire {
        fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            Ok(refused(RefusalReason::NoEngineConfigured, None, true))
        }
    }

    struct FailingWire;
    impl WireClient for FailingWire {
        fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            Ok(refused(
                RefusalReason::IncompleteText,
                Some("incomplete_text_length"),
                false,
            ))
        }
    }

    struct CountingFailWire {
        calls: AtomicUsize,
    }

    impl CountingFailWire {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl WireClient for CountingFailWire {
        fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            FailingWire.execute(request)
        }
    }

    struct CountingWire {
        calls: AtomicUsize,
    }

    impl CountingWire {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl WireClient for CountingWire {
        fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            SuccessWire.execute(request)
        }
    }

    struct WrongSchemaNoEngineWire;
    impl WireClient for WrongSchemaNoEngineWire {
        fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            decode_one_shot_response(
                &json!({"schema": "solstone-generate-response-v1", "outcome": "refused"})
                    .to_string(),
            )
            .map_err(ClientError::Decode)
        }
    }

    struct StaleWire;
    impl WireClient for StaleWire {
        fn execute(&self, _: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            Ok(refused(
                RefusalReason::AttestationStale,
                Some("attestation_stale"),
                true,
            ))
        }
    }

    struct NoDetector;
    impl Detector for NoDetector {
        fn detect(&self, _: &[u8]) -> Result<Option<Value>, String> {
            Ok(None)
        }
    }
    struct BrokenDetector;
    impl Detector for BrokenDetector {
        fn detect(&self, _: &[u8]) -> Result<Option<Value>, String> {
            Err("unavailable".to_owned())
        }
    }
    struct CannedDetector;
    impl Detector for CannedDetector {
        fn detect(&self, _: &[u8]) -> Result<Option<Value>, String> {
            Ok(Some(
                json!({"image":{"width":4,"height":4},"detections":[{"class_name":"bottle"}]}),
            ))
        }
    }
    struct MalformedDetector;
    impl Detector for MalformedDetector {
        fn detect(&self, _: &[u8]) -> Result<Option<Value>, String> {
            Ok(Some(json!({"image": {"width": 4}})))
        }
    }

    fn fixture_image() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("123456_300");
        fs::create_dir_all(&segment).unwrap();
        let image = segment.join("photo.png");
        ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
            .save(&image)
            .unwrap();
        (root, image)
    }

    #[test]
    fn native_rfdetr_query_requires_the_pinned_sidecar_and_artifacts() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            rfdetr_paths_at(root.path(), "linux", "x86_64").unwrap_err(),
            rfdetr_unavailable()
        );

        for key in ["linux-cpu-x64", "linux-cpu-arm64", "macos-metal-arm64"] {
            let paths = rfdetr_paths_from_install_check(
                Ok(RfdetrInstallRecord::Installed),
                root.path(),
                key,
            )
            .unwrap();
            assert_eq!(paths.0, binary_path(root.path(), key));
            assert_eq!(paths.1, model_path(root.path()));
        }

        const KEY: &str = "linux-cpu-x64";
        const ENGINE_VERSION: &str = "v0.1.0-solpbc.5";
        const MODEL_FILE: &str = "rfdetr-nano-f16.gguf";
        const MODEL_REPO: &str = "mudler/rfdetr-cpp-nano";
        const MODEL_REVISION: &str = "c3dc0c037df499f5503545247df6618415fca643";

        let engine_bytes = b"engine binary";
        let engine_sha256: &'static str =
            Box::leak(format!("{:x}", Sha256::digest(engine_bytes)).into_boxed_str());
        let model_bytes = b"model weights";
        let model_sha256: &'static str =
            Box::leak(format!("{:x}", Sha256::digest(model_bytes)).into_boxed_str());
        let engine = EngineSpec {
            filename: "fixture-engine.tar.gz",
            tarball_sha256: engine_sha256,
            tarball_size: engine_bytes.len() as u64,
            binary_sha256: engine_sha256,
            expected_member_path: None,
        };
        let model = ModelSpec {
            sha256: model_sha256,
            size: model_bytes.len() as u64,
        };
        let binary = binary_path(root.path(), KEY);
        let model_file = model_path(root.path());
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::create_dir_all(model_file.parent().unwrap()).unwrap();
        fs::write(&binary, engine_bytes).unwrap();
        fs::write(&model_file, model_bytes).unwrap();
        let sidecar = root
            .path()
            .join("cache")
            .join("providers")
            .join("rfdetr")
            .join(".rfdetr-install.json");
        fs::write(
            sidecar,
            json!({
                "artifact_key": KEY,
                "engine_version": ENGINE_VERSION,
                "engine_sha256": engine.tarball_sha256,
                "model_file": MODEL_FILE,
                "model_repo": MODEL_REPO,
                "model_revision": MODEL_REVISION,
                "model_sha256": model.sha256,
                "status": "installed",
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            check_rfdetr_model_with_fixture_artifacts(root.path(), KEY, &engine, &model).unwrap(),
            RfdetrInstallRecord::Installed
        );

        fs::write(&model_file, b"tampered data").unwrap();
        let error = rfdetr_paths_from_install_check(
            check_rfdetr_model_with_fixture_artifacts(root.path(), KEY, &engine, &model),
            root.path(),
            KEY,
        )
        .unwrap_err();
        assert_eq!(error, rfdetr_unavailable());
    }

    #[test]
    fn header_merges_metadata_and_rejects_non_object() {
        let header = build_header_from_values(
            "photo.png",
            Some("camera"),
            Some(r#"{"stream":"default","kind":"override"}"#),
        )
        .unwrap();
        assert_eq!(header["raw"], "photo.png");
        assert_eq!(header["kind"], "override");
        assert_eq!(header["observer"], "camera");
        assert!(build_header_from_values("photo.png", None, Some("not-json")).is_ok());
        assert!(matches!(
            build_header_from_values("photo.png", None, Some("[]")),
            Err(DepictError::Metadata(_))
        ));
    }

    #[test]
    fn handler_error_record_uses_depict_schema_and_uniform_metadata() {
        let error = DepictError::Wire {
            detail: "wire detail".to_owned(),
            blocking: true,
            reason_code: Some("attestation_stale".to_owned()),
        };
        let line: Value = serde_json::from_str(&error_json_line(&error)).unwrap();
        assert_eq!(line["schema"], "solstone-depict-error-v1");
        assert_eq!(line["reason"], "generate-wire-failed");
        assert_eq!(line["blocking"], true);
        assert_eq!(line["reason_code"], "attestation_stale");

        let line: Value =
            serde_json::from_str(&error_json_line(&DepictError::Image("bad".to_owned()))).unwrap();
        assert_eq!(line["blocking"], false);
        assert!(line["reason_code"].is_null());
    }

    #[test]
    fn interpret_generate_result_handles_all_client_error_doors() {
        for detail in ["unparseable stdout", "empty stdout", "non-protocol stderr"] {
            let error = interpret_generate(Err(ClientError::Decode(detail.to_owned())))
                .expect_err("decode failures must not produce a description");
            assert!(matches!(
                error,
                DepictError::Wire {
                    blocking: true,
                    reason_code: None,
                    ..
                }
            ));
        }
        let error = interpret_generate(Err(ClientError::Protocol(Box::new(ProtocolFailure {
            error: ProtocolError {
                id: None,
                reason: "internal-failure".to_owned(),
                detail: "failed to encode provider result".to_owned(),
            },
            status: ChildStatus {
                exit_code: Some(70),
                signal: None,
            },
            stdout: CapturedStream::empty(),
            stderr: CapturedStream::empty(),
            stdin_closed_early: false,
        }))))
        .expect_err("protocol error must not produce a description");
        assert!(matches!(
            error,
            DepictError::Wire {
                blocking: true,
                reason_code: None,
                ..
            }
        ));
        for error in [
            ClientError::Io {
                primary: "io".to_owned(),
                cleanup: None,
            },
            ClientError::Resolve("resolve".to_owned()),
            ClientError::UnexpectedChild(Box::new(UnexpectedChildFailure {
                status: ChildStatus {
                    exit_code: Some(64),
                    signal: None,
                },
                stdout: CapturedStream::empty(),
                stderr: CapturedStream::empty(),
                stdin_closed_early: false,
            })),
        ] {
            assert!(matches!(
                interpret_generate(Err(error)),
                Err(DepictError::Wire {
                    blocking: true,
                    reason_code: None,
                    ..
                })
            ));
        }
    }

    #[test]
    fn interpret_generate_result_handles_generated_and_refused_responses() {
        assert_eq!(
            interpret_generate(Ok(generated("description"))).unwrap(),
            Description::Generated("description".to_owned())
        );
        assert_eq!(
            interpret_generate(Ok(refused(RefusalReason::NoEngineConfigured, None, true))).unwrap(),
            Description::NoEngine
        );
        let error = interpret_generate(Ok(refused(
            RefusalReason::AttestationStale,
            Some("attestation_stale"),
            true,
        )))
        .expect_err("non-no-engine refusal must fail the handler");
        assert_eq!(error.reason(), "generate-wire-failed");
        assert!(error.blocking());
        assert_eq!(error.reason_code(), Some("attestation_stale"));
    }

    #[test]
    fn interpret_generate_result_refuses_v1_records_as_decode_failures() {
        let response = decode_one_shot_response(
            &json!({"schema": "solstone-generate-response-v1", "outcome": "refused"}).to_string(),
        )
        .map_err(ClientError::Decode);
        let error = interpret_generate(response).expect_err("v1 response must be rejected");
        assert!(matches!(
            error,
            DepictError::Wire {
                blocking: true,
                reason_code: None,
                ..
            }
        ));

        let detail = decode_protocol_error(
            &json!({
                "schema": "solstone-generate-error-v1",
                "reason": "no-engine-configured",
                "detail": "none",
            })
            .to_string(),
        )
        .expect_err("v1 error schema must be rejected");
        let error = interpret_generate(Err(ClientError::Decode(detail)))
            .expect_err("v1 error must be rejected");
        assert!(matches!(
            error,
            DepictError::Wire {
                blocking: true,
                reason_code: None,
                ..
            }
        ));
    }

    #[test]
    fn request_builder_preserves_resized_image_and_contract_defaults() {
        let prepared = resize_for_vlm(DynamicImage::new_rgb8(3840, 2));
        assert_eq!(prepared.dimensions(), (1920, 1));
        let request = build_generate_request(&encode_png(&prepared).unwrap());
        assert_eq!(request.id, None);
        assert_eq!(request.context, "observe.depict");
        assert_eq!(request.system_instruction, None);
        assert_eq!(request.temperature, 0.3);
        assert_eq!(request.max_output_tokens, 256);
        assert_eq!(request.timeout_s, None);
        assert!(!request.json_output);
        assert_eq!(request.json_schema, None);
        assert_eq!(request.attempt_index, 0);
        assert!(!request.exclusive_admission);
        assert_eq!(request.transport_retries, None);
        assert!(matches!(
            &request.contents[0],
            ContentPart::Text { text } if text == DESCRIPTION_PROMPT
        ));
        let ContentPart::Image { mime_type, data } = &request.contents[1] else {
            panic!("second content part must be an image")
        };
        assert_eq!(mime_type, "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        assert_eq!(
            image::load_from_memory(&decoded).unwrap().dimensions(),
            (1920, 1)
        );
    }

    #[test]
    fn recordless_sidecar_without_text_reenters_and_writes() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        fs::write(&output, "old\n").unwrap();
        assert_eq!(
            run_with_clients(&image, false, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Written
        );
    }

    #[test]
    fn sidecar_with_text_row_skips_when_not_redo() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        fs::write(
            &output,
            "{\"raw\":\"photo.png\",\"kind\":\"image\"}\n{\"start\":\"00:00:00\",\"text\":\"caption\"}\n",
        )
        .unwrap();
        assert_eq!(
            run_with_clients(&image, false, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Skipped
        );
        let preserved = fs::read_to_string(&output).unwrap();
        assert!(preserved.contains("caption"));
    }

    #[test]
    fn no_engine_does_not_write_output() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        assert_eq!(
            run_with_clients(&image, false, &NoEngineWire, &NoDetector).unwrap(),
            RunOutcome::NoEngine
        );
        assert!(!output.exists());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn writer_commit_admits_success_and_failure_but_refuses_changed_or_deleted_input() {
        struct MutatingWire<'a> {
            image: &'a Path,
            mutation: &'a str,
            fail: bool,
        }
        impl WireClient for MutatingWire<'_> {
            fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
                let options = solstone_core_journal_io::LockOptions {
                    timeout: std::time::Duration::ZERO,
                    ..Default::default()
                };
                assert!(
                    solstone_core_journal_io::hold_lock(
                        self.image.with_extension("jsonl"),
                        options
                    )
                    .is_err()
                );
                assert!(
                    solstone_core_journal_io::hold_lock(self.image.parent().unwrap(), options)
                        .is_ok()
                );
                match self.mutation {
                    "bytes" => {
                        let mut bytes = fs::read(self.image).unwrap();
                        bytes[0] ^= 1;
                        fs::write(self.image, bytes).unwrap();
                    }
                    "deleted" => {
                        fs::write(self.image.parent().unwrap().join("tombstone.json"), b"{}")
                            .unwrap()
                    }
                    "missing" => fs::remove_file(self.image).unwrap(),
                    "replaced" => {
                        let bytes = fs::read(self.image).unwrap();
                        fs::remove_file(self.image).unwrap();
                        fs::write(self.image, bytes).unwrap();
                    }
                    _ => (),
                }
                if self.fail {
                    FailingWire.execute(request)
                } else {
                    SuccessWire.execute(request)
                }
            }
        }
        let _env = lock_depict_env();
        for fail in [false, true] {
            for mutation in ["unchanged", "bytes", "deleted", "missing", "replaced"] {
                let (_root, image) = fixture_image();
                let result = run_with_clients(
                    &image,
                    false,
                    &MutatingWire {
                        image: &image,
                        mutation,
                        fail,
                    },
                    &NoDetector,
                );
                let output = image.with_extension("jsonl");
                if mutation == "unchanged" {
                    assert_eq!(result.is_ok(), !fail);
                    assert_eq!(
                        solstone_core_processing_record::read_processing_record_header(&output)
                            .unwrap()["state"],
                        if fail {
                            vocab::STATE_FAILED
                        } else {
                            vocab::STATE_ANALYZED
                        }
                    );
                } else {
                    assert!(result.is_err(), "{mutation}");
                    assert!(!output.exists(), "{mutation}");
                }
            }
        }
    }

    #[test]
    fn successful_write_stamps_analyzed_depict_record() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let input_size = fs::metadata(&image).unwrap().len();
        assert_eq!(
            run_with_clients(&image, false, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Written
        );
        let header: Value =
            serde_json::from_str(fs::read_to_string(&output).unwrap().lines().next().unwrap())
                .unwrap();
        let record = &header["_solstone_processing"];
        assert_eq!(record["schema"], vocab::SCHEMA);
        assert_eq!(record["state"], vocab::STATE_ANALYZED);
        assert_eq!(record["reason_code"], vocab::REASON_OK);
        assert_eq!(record["handler"], vocab::HANDLER_DEPICT);
        assert_eq!(record["input_size"], input_size);
        assert!(record.get("attempts").is_none());
        chrono::DateTime::parse_from_rfc3339(
            record["attempted_at"]
                .as_str()
                .expect("attempted_at must be a string"),
        )
        .expect("attempted_at must be RFC 3339");
    }

    fn assert_degraded_sidecar(output: &Path, disallowed_detail: &str) {
        let lines: Vec<String> = fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(ToOwned::to_owned)
            .collect();
        assert_eq!(lines.len(), 2);
        let header: Value = serde_json::from_str(&lines[0]).unwrap();
        let record = &header["_solstone_processing"];
        assert_eq!(record["state"], vocab::STATE_ANALYZED);
        assert_eq!(record["reason_code"], vocab::REASON_OK);
        assert_eq!(record["handler"], vocab::HANDLER_DEPICT);

        let entry: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(entry["text"], "detail");
        assert!(entry.get("detections").is_none());
        let error = &entry["detection_error"];
        assert_eq!(error["reason_code"], "rfdetr-unavailable");
        let detail = error["detail"].as_str().expect("detail must be string");
        assert!(!detail.is_empty());
        assert_ne!(detail, disallowed_detail);
    }

    #[test]
    fn broken_detector_writes_degraded_sidecar_with_analyzed_record() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let outcome = run_with_clients(&image, false, &SuccessWire, &BrokenDetector).unwrap();
        assert_eq!(outcome, RunOutcome::Written);
        assert!(output.exists());
        assert_degraded_sidecar(&output, "unavailable");
    }

    #[test]
    fn malformed_detector_writes_degraded_sidecar_with_analyzed_record() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let outcome = run_with_clients(&image, false, &SuccessWire, &MalformedDetector).unwrap();
        assert_eq!(outcome, RunOutcome::Written);
        assert!(output.exists());
        assert_degraded_sidecar(&output, "detector output has no detections");
    }

    #[test]
    fn degraded_sidecar_skips_and_avoids_second_vlm_call_when_not_redo() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let wire = CountingWire::new();
        assert_eq!(
            run_with_clients(&image, false, &wire, &BrokenDetector).unwrap(),
            RunOutcome::Written
        );
        assert_eq!(
            run_with_clients(&image, false, &wire, &BrokenDetector).unwrap(),
            RunOutcome::Skipped
        );
        assert_eq!(wire.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn detector_variations_produce_distinct_sidecar_shapes() {
        let _env = lock_depict_env();
        let (_root1, image1) = fixture_image();
        assert_eq!(
            run_with_clients(&image1, false, &SuccessWire, &CannedDetector).unwrap(),
            RunOutcome::Written
        );
        let canned_rows: Vec<Value> = fs::read_to_string(image1.with_extension("jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(canned_rows[1]["detections"]["gate"], "still");
        assert!(canned_rows[1].get("detection_error").is_none());

        let (_root2, image2) = fixture_image();
        assert_eq!(
            run_with_clients(&image2, false, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Written
        );
        let no_detector_rows: Vec<Value> = fs::read_to_string(image2.with_extension("jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(no_detector_rows[1].get("detections").is_none());
        assert!(no_detector_rows[1].get("detection_error").is_none());

        let (_root3, image3) = fixture_image();
        assert_eq!(
            run_with_clients(&image3, false, &SuccessWire, &BrokenDetector).unwrap(),
            RunOutcome::Written
        );
        let broken_rows: Vec<Value> = fs::read_to_string(image3.with_extension("jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(broken_rows[1].get("detections").is_none());
        assert_eq!(
            broken_rows[1]["detection_error"]["reason_code"],
            "rfdetr-unavailable"
        );
    }

    fn processing_record(output: &Path) -> Value {
        let lines: Vec<String> = fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(ToOwned::to_owned)
            .collect();
        assert_eq!(lines.len(), 1);
        let header: Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(header.get("text").is_none());
        header["_solstone_processing"].clone()
    }

    #[test]
    fn failing_wire_writes_a_header_only_failure() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let raw = fs::read(&image).unwrap();
        let error = run_with_clients(&image, false, &FailingWire, &NoDetector).unwrap_err();
        assert!(matches!(
            &error,
            DepictError::Wire {
                blocking: false,
                reason_code: Some(code),
                ..
            } if code == "incomplete_text_length"
        ));
        assert_eq!(error.exit_code(), 1);
        assert_eq!(fs::read(&image).unwrap(), raw);
        let record = processing_record(&output);
        assert_eq!(record["schema"], vocab::SCHEMA);
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_ANALYSIS_FAILED);
        assert_eq!(record["handler"], vocab::HANDLER_DEPICT);
        assert_eq!(record["attempts"], 1);
        assert_eq!(record["input_size"], raw.len() as u64);
    }

    #[test]
    fn corrupt_bytes_write_a_terminal_failure_and_keep_the_raw() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let raw = b"not an image";
        fs::write(&image, raw).unwrap();
        let output = image.with_extension("jsonl");
        let wire = CountingFailWire::new();
        let error = run_with_clients(&image, false, &wire, &NoDetector).unwrap_err();
        assert!(matches!(error, DepictError::Image(_)));
        assert_eq!(error.exit_code(), 1);
        assert_eq!(fs::read(&image).unwrap(), raw);
        let record = processing_record(&output);
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_CORRUPT_INPUT);
        assert_eq!(record["attempts"], 1);
        assert_eq!(record["input_size"], raw.len() as u64);
        assert_eq!(wire.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            run_with_clients(&image, false, &wire, &NoDetector).unwrap(),
            RunOutcome::Skipped
        );
        assert_eq!(wire.calls.load(Ordering::SeqCst), 0);
        assert_eq!(processing_record(&output)["attempts"], 1);
    }

    #[test]
    fn owner_readmission_reaches_native_vision_once_without_resetting_failed_output() {
        use solstone_core_system::work_retry::{self, raw};
        let _env = lock_depict_env();
        let root = tempfile::tempdir().unwrap();
        let segment = root
            .path()
            .join("chronicle")
            .join("20260101")
            .join("photos")
            .join("123456_300");
        fs::create_dir_all(&segment).unwrap();
        let image = segment.join("photo.png");
        ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
            .save(&image)
            .unwrap();
        let output = image.with_extension("jsonl");
        let wire = CountingFailWire::new();
        for _ in 0..3 {
            assert!(run_with_clients(&image, false, &wire, &NoDetector).is_err());
        }
        assert_eq!(
            run_with_clients(&image, false, &wire, &NoDetector).unwrap(),
            RunOutcome::Skipped
        );
        let evidence = raw::inspect_output(root.path(), &output).unwrap().unwrap();
        raw::observe(root.path(), &evidence).unwrap();
        let id = evidence.unit.id().unwrap();
        assert!(
            work_retry::read(root.path(), &id)
                .unwrap()
                .unwrap()
                .permission(&evidence.input, &[])
                .is_none()
        );
        work_retry::request(root.path(), &evidence.unit).unwrap();
        let attempt = work_retry::read(root.path(), &id)
            .unwrap()
            .unwrap()
            .permission(&evidence.input, &[])
            .unwrap();
        temp_env::with_vars(
            [
                (raw::JOURNAL_ENV, Some(root.path().to_str().unwrap())),
                (raw::UNIT_ENV, Some(id.as_str())),
                (raw::CAUSE_ENV, Some(attempt.cause.as_str())),
                (
                    raw::ATTEMPT_ENV,
                    Some(serde_json::to_string(&attempt).unwrap().as_str()),
                ),
            ],
            || {
                let selected_output = fs::read(&output).unwrap();
                fs::remove_file(&output).unwrap();
                assert!(run_with_clients(&image, false, &wire, &NoDetector).is_err());
                assert_eq!(wire.calls.load(Ordering::SeqCst), 3);
                assert!(
                    work_retry::read(root.path(), &id)
                        .unwrap()
                        .unwrap()
                        .attempt
                        .is_none()
                );
                fs::write(&output, selected_output).unwrap();
                let selected_bytes = fs::read(&image).unwrap();
                fs::write(&image, b"changed after selection").unwrap();
                assert!(run_with_clients(&image, false, &wire, &NoDetector).is_err());
                assert_eq!(wire.calls.load(Ordering::SeqCst), 3);
                assert!(
                    work_retry::read(root.path(), &id)
                        .unwrap()
                        .unwrap()
                        .attempt
                        .is_none()
                );
                fs::write(&image, selected_bytes).unwrap();
                assert!(run_with_clients(&image, false, &wire, &NoDetector).is_err());
                assert_eq!(wire.calls.load(Ordering::SeqCst), 4);
                assert_eq!(processing_record(&output)["attempts"], 4);
                assert!(run_with_clients(&image, false, &wire, &NoDetector).is_err());
                assert_eq!(wire.calls.load(Ordering::SeqCst), 4);
            },
        );
        assert!(
            work_retry::read(root.path(), &id)
                .unwrap()
                .unwrap()
                .attempt
                .is_some(),
            "a missing terminal receipt remains interrupted, not fresh permission"
        );
        assert!(
            work_retry::read(root.path(), &id)
                .unwrap()
                .unwrap()
                .permission(&evidence.input, &[])
                .is_none()
        );
        raw::finish_output(root.path(), &evidence.unit, &attempt).unwrap();
        let finished = work_retry::read(root.path(), &id).unwrap().unwrap();
        assert!(finished.held);
        assert!(
            finished.attempt.is_none(),
            "a failed native publication is a terminal receipt too"
        );
    }

    #[test]
    fn analysis_failures_count_to_the_bound_then_skip_without_another_vision_call() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let wire = CountingFailWire::new();
        for attempt in 1..=3 {
            let error = run_with_clients(&image, false, &wire, &NoDetector).unwrap_err();
            assert!(matches!(error, DepictError::Wire { .. }));
            assert_eq!(processing_record(&output)["attempts"], attempt);
            assert_eq!(wire.calls.load(Ordering::SeqCst), attempt as usize);
        }
        assert_eq!(
            run_with_clients(&image, false, &wire, &NoDetector).unwrap(),
            RunOutcome::Skipped
        );
        assert_eq!(wire.calls.load(Ordering::SeqCst), 3);
        assert_eq!(processing_record(&output)["attempts"], 3);
    }

    #[test]
    fn redo_success_replaces_a_failure_without_attempts() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        run_with_clients(&image, false, &FailingWire, &NoDetector).unwrap_err();
        assert_eq!(
            run_with_clients(&image, true, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Written
        );
        let lines: Vec<String> = fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(ToOwned::to_owned)
            .collect();
        assert_eq!(lines.len(), 2);
        let header: Value = serde_json::from_str(&lines[0]).unwrap();
        let record = &header["_solstone_processing"];
        assert_eq!(record["state"], vocab::STATE_ANALYZED);
        assert_eq!(record["reason_code"], vocab::REASON_OK);
        assert!(record.get("attempts").is_none());
        let entry: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(entry["text"], "detail");
    }

    #[test]
    fn failed_redo_preserves_a_successful_sidecar() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        assert_eq!(
            run_with_clients(&image, false, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Written
        );
        let before = fs::read(&output).unwrap();
        let error = run_with_clients(&image, true, &FailingWire, &NoDetector).unwrap_err();
        assert!(matches!(error, DepictError::Wire { .. }));
        assert_eq!(fs::read(&output).unwrap(), before);
    }

    #[test]
    fn a_late_failure_preserves_success_published_while_it_was_pending() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        assert_eq!(
            run_with_clients(&image, false, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Written
        );
        let published = fs::read(&output).unwrap();
        fs::remove_file(&output).unwrap();
        let publish_success = || {
            fs::write(&output, &published).unwrap();
        };
        let error = run_depict(
            &image,
            false,
            &FailingWire,
            &NoDetector,
            &FailurePublicationHooks {
                before_lock: Some(&publish_success),
                before_replace: None,
            },
            &|| false,
            None,
        )
        .unwrap_err();
        assert!(matches!(error, DepictError::Wire { .. }));
        assert_eq!(fs::read(&output).unwrap(), published);
        let header: Value = serde_json::from_str(
            std::str::from_utf8(&fs::read(&output).unwrap())
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            header["_solstone_processing"]["state"],
            vocab::STATE_ANALYZED
        );
    }

    #[test]
    fn sidecar_publication_failure_names_both_failures() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        fs::create_dir(&output).unwrap();
        let error = run_with_clients(&image, false, &FailingWire, &NoDetector).unwrap_err();
        match error {
            DepictError::Wire {
                detail,
                blocking,
                reason_code,
            } => {
                assert!(!blocking);
                assert_eq!(reason_code.as_deref(), Some("incomplete_text_length"));
                assert!(detail.contains("wire detail"));
                assert_ne!(detail, "wire detail");
            }
            other => panic!("expected the original wire error, got {other:?}"),
        }
        assert!(output.is_dir());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn publication_serializes_a_success_against_a_checked_failure() {
        use std::sync::mpsc;
        use std::time::Duration;

        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let (checked_tx, checked_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let failed_image = image.clone();
        let failure = std::thread::spawn(move || {
            let before_replace = || {
                checked_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            };
            run_depict(
                &failed_image,
                false,
                &FailingWire,
                &NoDetector,
                &FailurePublicationHooks {
                    before_lock: None,
                    before_replace: Some(&before_replace),
                },
                &|| false,
                None,
            )
        });
        checked_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let (ready_tx, ready_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let success = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let result = run_with_clients(&image, false, &SuccessWire, &NoDetector);
            finished_tx.send(()).unwrap();
            result
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let early_finish = finished_rx.recv_timeout(Duration::from_millis(100));
        resume_tx.send(()).unwrap();
        let error = failure.join().unwrap().unwrap_err();
        assert_eq!(success.join().unwrap().unwrap(), RunOutcome::Written);
        assert!(matches!(early_finish, Err(mpsc::RecvTimeoutError::Timeout)));
        assert!(matches!(error, DepictError::Wire { .. }));
        let record = read_processing_record_header(&output).unwrap();
        assert_eq!(record["state"], vocab::STATE_ANALYZED);
        assert_eq!(record["reason_code"], vocab::REASON_OK);
    }

    #[test]
    fn invalid_segment_meta_non_object_fails_before_write() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let result = temp_env::with_var("SEGMENT_META", Some("[]"), || {
            run_with_clients(&image, false, &SuccessWire, &NoDetector)
        });
        assert!(matches!(result, Err(DepictError::Metadata(_))));
        assert!(!output.exists());
    }

    #[test]
    fn redo_overwrites_existing_detections_with_degraded_record() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        assert_eq!(
            run_with_clients(&image, false, &SuccessWire, &CannedDetector).unwrap(),
            RunOutcome::Written
        );
        let canned_rows: Vec<Value> = fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(canned_rows[1]["detections"]["gate"], "still");

        assert_eq!(
            run_with_clients(&image, true, &SuccessWire, &BrokenDetector).unwrap(),
            RunOutcome::Written
        );
        let degraded_rows: Vec<Value> = fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(degraded_rows[1]["text"], "detail");
        assert_eq!(
            degraded_rows[1]["detection_error"]["reason_code"],
            "rfdetr-unavailable"
        );
        assert!(degraded_rows[1].get("detections").is_none());
        assert!(image.exists());
    }

    #[test]
    fn wrong_wire_error_schema_is_not_no_engine() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        assert!(matches!(
            run_with_clients(&image, false, &WrongSchemaNoEngineWire, &NoDetector),
            Err(DepictError::Wire { .. })
        ));
        let record = processing_record(&image.with_extension("jsonl"));
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_ANALYSIS_FAILED);
        assert_eq!(record["attempts"], 1);
    }

    #[test]
    fn stale_refusal_preserves_metadata_without_writing_output() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let error = run_with_clients(&image, false, &StaleWire, &NoDetector)
            .expect_err("attestation refusal stays an error");
        assert_eq!(error.exit_code(), 1);
        let record = processing_record(&image.with_extension("jsonl"));
        assert_eq!(record["state"], vocab::STATE_FAILED);
        assert_eq!(record["reason_code"], vocab::REASON_ANALYSIS_FAILED);
        let record: Value = serde_json::from_str(&error_json_line(&error)).unwrap();
        assert_eq!(record["blocking"], true);
        assert_eq!(record["reason_code"], "attestation_stale");
    }

    #[test]
    fn vlm_resize_preserves_small_images_and_caps_large_images() {
        assert_eq!(vlm_dimensions(1920, 1080), (1920, 1080));
        assert_eq!(vlm_dimensions(3840, 2160), (1920, 1080));
        assert_eq!(
            resize_for_vlm(DynamicImage::new_rgb8(3840, 2)).dimensions(),
            (1920, 1)
        );
    }

    #[test]
    fn exit_codes_are_never_hold_raw() {
        for error in [
            DepictError::Usage("x".to_owned()),
            DepictError::Image("x".to_owned()),
            DepictError::Wire {
                detail: "x".to_owned(),
                blocking: false,
                reason_code: None,
            },
            DepictError::Metadata("x".to_owned()),
            DepictError::Output("x".to_owned()),
        ] {
            assert_eq!(error.exit_code(), 1);
        }
        assert_eq!(DepictError::Help.exit_code(), 0);
    }

    #[test]
    fn help_flags_do_not_parse_as_an_image() {
        for args in [
            vec![OsString::from("--help")],
            vec![OsString::from("-h")],
            vec![OsString::from("photo.png"), OsString::from("--help")],
        ] {
            assert!(matches!(parse_args(&args), Err(DepictError::Help)));
        }
    }

    #[test]
    fn dispatcher_verbosity_flags_are_accepted_in_any_position() {
        for args in [
            vec![OsString::from("photo.png"), OsString::from("-v")],
            vec![OsString::from("--verbose"), OsString::from("photo.png")],
            vec![OsString::from("photo.png"), OsString::from("-d")],
            vec![OsString::from("--debug"), OsString::from("photo.png")],
            vec![
                OsString::from("-v"),
                OsString::from("--redo"),
                OsString::from("photo.png"),
                OsString::from("-d"),
            ],
        ] {
            let parsed = parse_args(&args).expect("dispatcher flags should be accepted");
            assert_eq!(parsed.image_path, PathBuf::from("photo.png"));
            assert_eq!(
                parsed.redo,
                args.iter().any(|argument| argument == "--redo")
            );
        }
    }

    #[test]
    fn unknown_or_repeated_arguments_are_rejected() {
        for args in [
            vec![OsString::from("photo.png"), OsString::from("--unknown")],
            vec![OsString::from("photo.png"), OsString::from("other.png")],
            vec![
                OsString::from("photo.png"),
                OsString::from("--redo"),
                OsString::from("--redo"),
            ],
        ] {
            assert!(matches!(parse_args(&args), Err(DepictError::Usage(_))));
        }
    }

    #[test]
    fn map_rfdetr_detect_completion_contract_in_depict() {
        use solstone_core_local::install::rfdetr_windows::map_rfdetr_detect_completion;

        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output.json");
        fs::write(
            &output,
            json!({"image": {"width": 100, "height": 100}, "detections": []}).to_string(),
        )
        .unwrap();

        let parsed = map_rfdetr_detect_completion(true, &output).expect("success parse");
        assert_eq!(parsed["detections"], json!([]));

        // planted json with failed status must return Err
        let planted = temp.path().join("planted.json");
        fs::write(
            &planted,
            json!({
                "image": {"width": 100, "height": 100},
                "detections": [{"class_name": "person", "score": 0.9}]
            })
            .to_string(),
        )
        .unwrap();
        assert!(map_rfdetr_detect_completion(false, &planted).is_err());
    }

    #[test]
    fn checkpoint_skips_success_degraded_and_failure_files() {
        let _env = lock_depict_env();

        // 1. Success case: checkpoint returns true, returns DepictError::Cancelled and writes no file.
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let result = run_with_clients_and_checkpoint(
            &image,
            false,
            &SuccessWire,
            &NoDetector,
            &|| true,
            None,
        );
        assert_eq!(result.unwrap_err(), DepictError::Cancelled);
        assert!(!output.exists());

        // Uncancelled twin writes success output.
        let result = run_with_clients_and_checkpoint(
            &image,
            false,
            &SuccessWire,
            &NoDetector,
            &|| false,
            None,
        );
        assert_eq!(result.unwrap(), RunOutcome::Written);
        assert!(output.exists());

        // 2. Degraded case: BrokenDetector produces detection error.
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let result = run_with_clients_and_checkpoint(
            &image,
            false,
            &SuccessWire,
            &BrokenDetector,
            &|| true,
            None,
        );
        assert_eq!(result.unwrap_err(), DepictError::Cancelled);
        assert!(!output.exists());

        // Uncancelled twin writes degraded output.
        let result = run_with_clients_and_checkpoint(
            &image,
            false,
            &SuccessWire,
            &BrokenDetector,
            &|| false,
            None,
        );
        assert_eq!(result.unwrap(), RunOutcome::Written);
        assert!(output.exists());
        let content = fs::read_to_string(&output).unwrap();
        assert!(content.contains("detection_error"));

        // 3. Failure case: FailingWire produces refusal/failure.
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        let result = run_with_clients_and_checkpoint(
            &image,
            false,
            &FailingWire,
            &NoDetector,
            &|| true,
            None,
        );
        assert_eq!(result.unwrap_err(), DepictError::Cancelled);
        assert!(!output.exists());

        // Uncancelled twin writes failure record.
        let result = run_with_clients_and_checkpoint(
            &image,
            false,
            &FailingWire,
            &NoDetector,
            &|| false,
            None,
        );
        assert!(result.is_err());
        assert_ne!(result.unwrap_err(), DepictError::Cancelled);
        assert!(output.exists());
        let content = fs::read_to_string(&output).unwrap();
        assert!(content.contains("state\":\"failed\"") || content.contains("\"failed\""));
    }

    #[test]
    fn explicit_attempt_is_stamped_with_no_environment_set() {
        let _env = lock_depict_env();
        let (_root, image) = fixture_image();
        let attempt = solstone_core_system::work_retry::Attempt {
            cause: "test_failure".to_owned(),
            input: "test_input_hash".to_owned(),
            reason: "test_reason".to_owned(),
        };
        let input_size = fs::metadata(&image).unwrap().len();
        let doc = failure_document(&image, input_size, "test_code", 1, Some(&attempt)).unwrap();
        let proc = doc.get("_solstone_processing").expect("processing record");
        let retry_val = proc
            .get("retry_attempt")
            .expect("retry_attempt should be stamped");
        assert_eq!(retry_val["cause"], "test_failure");
        assert_eq!(retry_val["reason"], "test_reason");
        assert_eq!(retry_val["input"], "test_input_hash");

        let doc_none = failure_document(&image, input_size, "test_code", 1, None).unwrap();
        let proc_none = doc_none
            .get("_solstone_processing")
            .expect("processing record");
        assert!(proc_none.get("retry_attempt").is_none());
    }
}
