// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Standalone native still-image depiction handler.
//!
//! It is intentionally not wired into `journal depict` yet.  The Python
//! implementation remains the differential reference until the cutover lode.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use image::{DynamicImage, GenericImageView, ImageFormat, imageops::FilterType};
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{AtomicWriteOptions, write_jsonl};

pub const REQUEST_SCHEMA: &str = "solstone-generate-request-v1";
pub const RESPONSE_SCHEMA: &str = "solstone-generate-response-v1";
pub const ERROR_SCHEMA: &str = "solstone-generate-error-v1";
pub const DESCRIPTION_PROMPT: &str = "Describe this image in detail. Include any visible text, people, objects, setting, and notable context. Return a concise natural-language description.";
pub const USAGE: &str = "Usage: solstone-core-depict <image_path> [--redo]";
const MAX_VLM_DIM: u32 = 1920;
const ENGINE_NAME: &str = "rf-detr.cpp";
const ENGINE_REF: &str = "65c0ffcc";
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
    Usage(String),
    Image(String),
    Wire(String),
    Metadata(String),
    Output(String),
}

impl DepictError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Usage(_) => "malformed-request",
            Self::Image(_) => "image-invalid",
            Self::Wire(_) => "generate-wire-failed",
            Self::Metadata(_) => "metadata-invalid",
            Self::Output(_) => "output-unwritable",
        }
    }

    pub fn exit_code(&self) -> i32 {
        // This is the handler namespace: all failures are ordinary non-hold-raw.
        1
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Usage(detail)
            | Self::Image(detail)
            | Self::Wire(detail)
            | Self::Metadata(detail)
            | Self::Output(detail) => detail,
        }
    }
}

pub fn error_json_line(error: &DepictError) -> String {
    json!({"schema": ERROR_SCHEMA, "reason": error.reason(), "detail": error.detail()}).to_string()
}

pub fn parse_args(args: &[OsString]) -> Result<Arguments, DepictError> {
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

#[derive(Debug, Clone, PartialEq)]
pub enum WireFailure {
    NoEngine,
    Failure { reason: String, detail: String },
}

pub trait WireClient {
    fn describe(&self, image_png: &[u8]) -> Result<String, WireFailure>;
}

pub trait Detector {
    fn detect(&self, full_png: &[u8]) -> Result<Option<Value>, String>;
}

pub struct SystemWireClient;

impl WireClient for SystemWireClient {
    fn describe(&self, image_png: &[u8]) -> Result<String, WireFailure> {
        let helper = sibling_executable("solstone-generate-wire").map_err(|detail| {
            WireFailure::Failure {
                reason: "wire-launch-failed".to_owned(),
                detail,
            }
        })?;
        let request = json!({
            "schema": REQUEST_SCHEMA,
            "contents": [
                {"type": "text", "text": DESCRIPTION_PROMPT},
                {"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(image_png), "mime_type": "image/png"}
            ],
            "context": "observe.depict"
        });
        let mut child = Command::new(helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| WireFailure::Failure {
                reason: "wire-launch-failed".to_owned(),
                detail: error.to_string(),
            })?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(request.to_string().as_bytes())
                .map_err(|error| WireFailure::Failure {
                    reason: "wire-write-failed".to_owned(),
                    detail: error.to_string(),
                })?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| WireFailure::Failure {
                reason: "wire-wait-failed".to_owned(),
                detail: error.to_string(),
            })?;
        if output.status.success() {
            let parsed: Value =
                serde_json::from_slice(&output.stdout).map_err(|error| WireFailure::Failure {
                    reason: "malformed-response".to_owned(),
                    detail: error.to_string(),
                })?;
            let text = parsed
                .get("result")
                .and_then(Value::as_object)
                .and_then(|result| result.get("text"))
                .and_then(Value::as_str)
                .ok_or_else(|| WireFailure::Failure {
                    reason: "malformed-response".to_owned(),
                    detail: "response has no result text".to_owned(),
                })?;
            if parsed.get("schema").and_then(Value::as_str) != Some(RESPONSE_SCHEMA) {
                return Err(WireFailure::Failure {
                    reason: "malformed-response".to_owned(),
                    detail: "response schema is not supported".to_owned(),
                });
            }
            return Ok(text.to_owned());
        }
        Err(wire_failure_from_stderr(
            output.status.code(),
            &output.stderr,
        ))
    }
}

pub struct SystemDetector;

impl Detector for SystemDetector {
    fn detect(&self, full_png: &[u8]) -> Result<Option<Value>, String> {
        let query = query_rfdetr_paths()?;
        if query.status != "installed" {
            return Ok(None);
        }
        let (binary, model) = match (query.binary_path, query.model_path) {
            (Some(binary), Some(model)) => (binary, model),
            _ => return Err("installed RF-DETR query omitted paths".to_owned()),
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

fn wire_failure_from_stderr(exit_code: Option<i32>, stderr: &[u8]) -> WireFailure {
    let error: Value = serde_json::from_slice(stderr).unwrap_or(Value::Null);
    let schema_matches = error.get("schema").and_then(Value::as_str) == Some(ERROR_SCHEMA);
    let reason = error
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("wire-failed");
    let detail = error
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or("generate wire failed");
    if schema_matches && reason == "no-engine-configured" && exit_code == Some(69) {
        WireFailure::NoEngine
    } else {
        WireFailure::Failure {
            reason: reason.to_owned(),
            detail: detail.to_owned(),
        }
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

struct RfdetrPaths {
    status: String,
    binary_path: Option<PathBuf>,
    model_path: Option<PathBuf>,
}

fn query_rfdetr_paths() -> Result<RfdetrPaths, String> {
    let python = sibling_python()?;
    let output = Command::new(python)
        .args(["-P", "-m", "solstone.observe.rfdetr_paths_query"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("RF-DETR install-state query failed".to_owned());
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    Ok(RfdetrPaths {
        status: value
            .get("status")
            .and_then(Value::as_str)
            .ok_or("RF-DETR query has no status")?
            .to_owned(),
        binary_path: value
            .get("binary_path")
            .and_then(Value::as_str)
            .map(PathBuf::from),
        model_path: value
            .get("model_path")
            .and_then(Value::as_str)
            .map(PathBuf::from),
    })
}

fn sibling_executable(name: &str) -> Result<PathBuf, String> {
    let current = env::current_exe().map_err(|error| error.to_string())?;
    let candidate = current
        .parent()
        .ok_or("native executable has no parent")?
        .join(name);
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(format!(
            "missing sibling executable {}",
            candidate.display()
        ))
    }
}

fn sibling_python() -> Result<PathBuf, String> {
    let current = env::current_exe().map_err(|error| error.to_string())?;
    let directory = current.parent().ok_or("native executable has no parent")?;
    for name in ["python3", "python"] {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("missing sibling Python interpreter".to_owned())
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

pub fn run_with_clients(
    image_path: &Path,
    redo: bool,
    wire: &dyn WireClient,
    detector: &dyn Detector,
) -> Result<RunOutcome, DepictError> {
    validate_image_path(image_path)?;
    let output_path = image_path.with_extension("jsonl");
    if output_path.exists() && !redo {
        return Ok(RunOutcome::Skipped);
    }
    let source = fs::read(image_path).map_err(|error| DepictError::Image(error.to_string()))?;
    let image =
        image::load_from_memory(&source).map_err(|error| DepictError::Image(error.to_string()))?;
    let full_png = encode_png(&image)?;
    let prepared = resize_for_vlm(image);
    let prepared_png = encode_png(&prepared)?;
    let description = match wire.describe(&prepared_png) {
        Ok(description) => description.trim().to_owned(),
        Err(WireFailure::NoEngine) => return Ok(RunOutcome::NoEngine),
        Err(WireFailure::Failure { reason, detail }) => {
            return Err(DepictError::Wire(format!("{reason}: {detail}")));
        }
    };
    let header = build_header(&image_path.file_name().unwrap_or_default().to_string_lossy())?;
    let mut entry = Map::new();
    entry.insert("start".to_owned(), Value::String("00:00:00".to_owned()));
    entry.insert("text".to_owned(), Value::String(description));
    match detector.detect(&full_png) {
        Ok(Some(result)) => match detections_block(result) {
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

fn detections_block(result: Value) -> Result<Value, String> {
    let object = result
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
        "engine_ref": ENGINE_REF,
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

    struct SuccessWire;
    impl WireClient for SuccessWire {
        fn describe(&self, _: &[u8]) -> Result<String, WireFailure> {
            Ok("  detail  ".to_owned())
        }
    }
    struct NoEngineWire;
    impl WireClient for NoEngineWire {
        fn describe(&self, _: &[u8]) -> Result<String, WireFailure> {
            Err(WireFailure::NoEngine)
        }
    }
    struct FailingWire;
    impl WireClient for FailingWire {
        fn describe(&self, _: &[u8]) -> Result<String, WireFailure> {
            Err(WireFailure::Failure {
                reason: "incomplete-text".to_owned(),
                detail: "bad".to_owned(),
            })
        }
    }
    struct WrongSchemaNoEngineWire;
    impl WireClient for WrongSchemaNoEngineWire {
        fn describe(&self, _: &[u8]) -> Result<String, WireFailure> {
            Err(wire_failure_from_stderr(
                Some(69),
                br#"{"schema":"wrong-schema","reason":"no-engine-configured","detail":"none"}"#,
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

    fn fixture_image() -> (PathBuf, PathBuf) {
        let root = env::temp_dir().join(format!(
            "depict-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let segment = root.join("123456_300");
        fs::create_dir_all(&segment).unwrap();
        let image = segment.join("photo.png");
        ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 4, Rgb([255, 0, 0]))
            .save(&image)
            .unwrap();
        (root, image)
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
    fn skip_redo_and_no_engine_preserve_output_rules() {
        let (root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        fs::write(&output, "old\n").unwrap();
        assert_eq!(
            run_with_clients(&image, false, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Skipped
        );
        assert_eq!(
            run_with_clients(&image, true, &SuccessWire, &NoDetector).unwrap(),
            RunOutcome::Written
        );
        fs::remove_file(&output).unwrap();
        assert_eq!(
            run_with_clients(&image, false, &NoEngineWire, &NoDetector).unwrap(),
            RunOutcome::NoEngine
        );
        assert!(!output.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wire_failures_do_not_write_and_detection_is_fail_open() {
        let (root, image) = fixture_image();
        let output = image.with_extension("jsonl");
        assert!(matches!(
            run_with_clients(&image, false, &FailingWire, &NoDetector),
            Err(DepictError::Wire(_))
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
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wrong_wire_error_schema_is_not_no_engine() {
        let (root, image) = fixture_image();
        assert!(matches!(
            run_with_clients(&image, false, &WrongSchemaNoEngineWire, &NoDetector),
            Err(DepictError::Wire(_))
        ));
        assert!(!image.with_extension("jsonl").exists());
        let _ = fs::remove_dir_all(root);
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
            DepictError::Wire("x".to_owned()),
            DepictError::Metadata("x".to_owned()),
            DepictError::Output("x".to_owned()),
        ] {
            assert_eq!(error.exit_code(), 1);
        }
    }
}
