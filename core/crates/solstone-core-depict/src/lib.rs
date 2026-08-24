// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Standalone native still-image depiction handler.
//!
//! `journal depict` reaches this handler through the explicit native process
//! table. The Python implementation remains only as the differential reference.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use image::{DynamicImage, GenericImageView, ImageFormat, imageops::FilterType};
use serde_json::{Map, Value, json};
use solstone_core_assets::canonical_host_pair;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient, RefusalReason,
};
use solstone_core_journal::{
    detect_checkout_root, discover_home, read_config_journal, resolve_journal_path,
};
use solstone_core_journal_io::{AtomicWriteOptions, write_jsonl};
use solstone_core_local::install::rfdetr_install::{
    ENGINE_PROVENANCE_REF, RfdetrInstallError, RfdetrInstallRecord, binary_path,
    check_rfdetr_model, model_path, rfdetr_artifact_key,
};
use solstone_core_processing_record::{
    read_processing_record_header, should_reenter_analysis_output, vocab,
};

pub const ERROR_SCHEMA: &str = "solstone-depict-error-v1";
pub const DESCRIPTION_PROMPT: &str = "Describe this image in detail. Include any visible text, people, objects, setting, and notable context. Return a concise natural-language description.";
pub const USAGE: &str = "usage: journal depict [-h] [--redo] FILE\n";
const MAX_VLM_DIM: u32 = 1920;
const ENGINE_NAME: &str = "rf-detr.cpp";
const MODEL_NAME: &str = "rfdetr-nano-f16";
const THRESHOLD: f64 = 0.25;
const RFDETR_TIMEOUT: Duration = Duration::from_secs(120);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(20);

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
}

impl DepictError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Help | Self::Usage(_) => "malformed-request",
            Self::Image(_) => "image-invalid",
            Self::Wire { .. } => "generate-wire-failed",
            Self::Metadata(_) => "metadata-invalid",
            Self::Output(_) => "output-unwritable",
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
            Self::Help | Self::Usage(_) | Self::Image(_) | Self::Metadata(_) | Self::Output(_) => {
                false
            }
        }
    }

    pub fn reason_code(&self) -> Option<&str> {
        match self {
            Self::Wire { reason_code, .. } => reason_code.as_deref(),
            Self::Help | Self::Usage(_) | Self::Image(_) | Self::Metadata(_) | Self::Output(_) => {
                None
            }
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
    match args {
        [image] => Ok(Arguments {
            image_path: PathBuf::from(image),
            redo: false,
        }),
        [image, redo] if redo == "--redo" => Ok(Arguments {
            image_path: PathBuf::from(image),
            redo: true,
        }),
        _ => Err(DepictError::Usage(USAGE.to_owned())),
    }
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
        OneShotClient::sibling()?
            .with_prefix_arguments(["generate".into()])
            .execute(request)
    }
}

pub struct SystemDetector;

impl Detector for SystemDetector {
    fn detect(&self, full_png: &[u8]) -> Result<Option<Value>, String> {
        let Some(journal) = current_journal_path() else {
            return Ok(None);
        };
        let Some((binary, model)) = rfdetr_paths_at(&journal, env::consts::OS, env::consts::ARCH)?
        else {
            return Ok(None);
        };
        let temporary = DetectorTempDir::new()?;
        let input = temporary.path.join("input.png");
        let output = temporary.path.join("output.json");
        fs::write(&input, full_png).map_err(|error| error.to_string())?;
        let mut process = Command::new(binary)
            .args(["detect", "--model"])
            .arg(model)
            .args(["--input"])
            .arg(&input)
            .args(["--output"])
            .arg(&output)
            .args(["--threshold", "0.25", "--threads", "4"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?;
        if !wait_for_child(&mut process, RFDETR_TIMEOUT)?.success() {
            return Err("rfdetr-cli detect failed".to_owned());
        }
        let parsed = serde_json::from_slice(&fs::read(output).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        Ok(Some(parsed))
    }
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            child.wait().map_err(|error| error.to_string())?;
            return Err(format!(
                "rfdetr-cli detect timed out after {}s",
                timeout.as_secs()
            ));
        }
        thread::sleep(CHILD_POLL_INTERVAL);
    }
}

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

fn rfdetr_paths_at(
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let (os_name, arch) = canonical_host_pair(os_name, arch);
    let Some(key) = rfdetr_artifact_key(os_name, arch) else {
        return Ok(None);
    };
    rfdetr_paths_from_install_check(check_rfdetr_model(journal, os_name, arch), journal, key)
}

fn rfdetr_paths_from_install_check(
    result: Result<RfdetrInstallRecord, RfdetrInstallError>,
    journal: &Path,
    key: &str,
) -> Result<Option<(PathBuf, PathBuf)>, String> {
    match result {
        Ok(RfdetrInstallRecord::Installed) => {
            Ok(Some((binary_path(journal, key), model_path(journal))))
        }
        Ok(RfdetrInstallRecord::PlatformUnavailable) => Ok(None),
        Err(error)
            if matches!(
                error.reason_code.as_str(),
                "sidecar_missing"
                    | "sidecar_mismatch"
                    | "unsupported_platform"
                    | "artifact_registry_mismatch"
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("RF-DETR {}: {error}", error.reason_code)),
    }
}

struct DetectorTempDir {
    path: PathBuf,
}

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
        Err(ClientError::Protocol(error)) => Err(wire_error(
            format!("{}: {}", error.reason, error.detail),
            true,
            None,
        )),
        Err(ClientError::Decode(detail)) => Err(wire_error(detail, true, None)),
        Err(ClientError::Io(detail)) | Err(ClientError::Resolve(detail)) => {
            Err(wire_error(detail, true, None))
        }
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
        max_output_tokens: 16_384,
        thinking_budget: None,
        timeout_s: None,
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
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
    validate_image_path(image_path)?;
    let output_path = image_path.with_extension("jsonl");
    if !redo && output_path.exists() {
        let record = read_processing_record_header(&output_path);
        if !should_reenter_analysis_output(record.as_ref(), &output_path, vocab::HANDLER_DEPICT) {
            return Ok(RunOutcome::Skipped);
        }
    }
    let source = fs::read(image_path).map_err(|error| DepictError::Image(error.to_string()))?;
    let image =
        image::load_from_memory(&source).map_err(|error| DepictError::Image(error.to_string()))?;
    let full_png = encode_png(&image)?;
    let prepared = resize_for_vlm(image);
    let prepared_png = encode_png(&prepared)?;
    let description =
        match interpret_generate(wire.execute(&build_generate_request(&prepared_png)))? {
            Description::Generated(description) => description.trim().to_owned(),
            Description::NoEngine => return Ok(RunOutcome::NoEngine),
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
    let mut entry = Map::new();
    entry.insert("start".to_owned(), Value::String("00:00:00".to_owned()));
    entry.insert("text".to_owned(), Value::String(description));
    match detector.detect(&full_png) {
        Ok(Some(detection)) => match detections_block(detection) {
            Ok(block) => {
                entry.insert("detections".to_owned(), block);
            }
            Err(error) => eprintln!("native depict: detection output omitted: {error}"),
        },
        Ok(None) => {}
        Err(error) => eprintln!("native depict: detection omitted: {error}"),
    }
    write_jsonl(
        &output_path,
        [Value::Object(header), Value::Object(entry)],
        AtomicWriteOptions::default(),
    )
    .map_err(|error| DepictError::Output(error.to_string()))?;
    Ok(RunOutcome::Written)
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
            Err(_) => eprintln!("native depict: invalid SEGMENT_META JSON"),
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
        GeneratedResponse, ProtocolError, ReasonCode, ReasonCodeValue, RefusedResponse,
        decode_one_shot_response, decode_protocol_error,
    };
    use solstone_core_local::install::{
        rfdetr_install::{EngineSpec, ModelSpec},
        test_hooks::check_rfdetr_model_with_fixture_artifacts,
    };

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
            rfdetr_paths_at(root.path(), "linux", "x86_64").unwrap(),
            None
        );

        for key in ["linux-cpu-x64", "linux-cpu-arm64", "macos-metal-arm64"] {
            let paths = rfdetr_paths_from_install_check(
                Ok(RfdetrInstallRecord::Installed),
                root.path(),
                key,
            )
            .unwrap()
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
            .join("cache/providers/rfdetr/.rfdetr-install.json");
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
        assert!(error.contains("sha256_mismatch"));
        assert!(error.contains(MODEL_FILE));
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
        let error = interpret_generate(Err(ClientError::Protocol(ProtocolError {
            id: None,
            reason: "internal-failure".to_owned(),
            detail: "failed to encode provider result".to_owned(),
        })))
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
            ClientError::Io("io".to_owned()),
            ClientError::Resolve("resolve".to_owned()),
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
        let prepared = resize_for_vlm(DynamicImage::new_rgb8(3840, 2160));
        assert_eq!(prepared.dimensions(), (1920, 1080));
        let request = build_generate_request(&encode_png(&prepared).unwrap());
        assert_eq!(request.id, None);
        assert_eq!(request.context, "observe.depict");
        assert_eq!(request.system_instruction, None);
        assert_eq!(request.temperature, 0.3);
        assert_eq!(request.max_output_tokens, 16_384);
        assert_eq!(request.thinking_budget, None);
        assert_eq!(request.timeout_s, None);
        assert!(!request.json_output);
        assert_eq!(request.json_schema, None);
        assert!(request.enforce_responsiveness);
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
            (1920, 1080)
        );
    }

    #[test]
    fn recordless_sidecar_without_text_reenters_and_writes() {
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
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        assert_eq!(
            run_with_clients(&image, false, &NoEngineWire, &NoDetector).unwrap(),
            RunOutcome::NoEngine
        );
        assert!(!output.exists());
    }

    #[test]
    fn successful_write_stamps_analyzed_depict_record() {
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

    #[test]
    fn wire_failures_do_not_write_and_detection_is_fail_open() {
        let (_root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        assert!(matches!(
            run_with_clients(&image, false, &FailingWire, &NoDetector),
            Err(DepictError::Wire { .. })
        ));
        assert!(!output.exists());
        assert_eq!(
            run_with_clients(&image, false, &SuccessWire, &BrokenDetector).unwrap(),
            RunOutcome::Written
        );
        let rows: Vec<Value> = fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(rows[1].get("detections").is_none());
        assert_eq!(
            run_with_clients(&image, true, &SuccessWire, &CannedDetector).unwrap(),
            RunOutcome::Written
        );
        let rows: Vec<Value> = fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows[1]["detections"]["gate"], "still");
    }

    #[test]
    fn wrong_wire_error_schema_is_not_no_engine() {
        let (_root, image) = fixture_image();
        assert!(matches!(
            run_with_clients(&image, false, &WrongSchemaNoEngineWire, &NoDetector),
            Err(DepictError::Wire { .. })
        ));
        assert!(!image.with_extension("jsonl").exists());
    }

    #[test]
    fn stale_refusal_preserves_metadata_without_writing_output() {
        let (_root, image) = fixture_image();
        let error = run_with_clients(&image, false, &StaleWire, &NoDetector)
            .expect_err("attestation refusal must not write");
        assert_eq!(error.exit_code(), 1);
        assert!(!image.with_extension("jsonl").exists());
        let record: Value = serde_json::from_str(&error_json_line(&error)).unwrap();
        assert_eq!(record["blocking"], true);
        assert_eq!(record["reason_code"], "attestation_stale");
    }

    #[test]
    fn vlm_resize_preserves_small_images_and_caps_large_images() {
        assert_eq!(vlm_dimensions(1920, 1080), (1920, 1080));
        assert_eq!(vlm_dimensions(3840, 2160), (1920, 1080));
        assert_eq!(
            resize_for_vlm(DynamicImage::new_rgb8(3840, 2160)).dimensions(),
            (1920, 1080)
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
}
