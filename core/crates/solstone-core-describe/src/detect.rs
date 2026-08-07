// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Best-effort RF-DETR invocation for screen-gated describe rows.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const THRESHOLD: &str = "0.25";
const ENGINE_NAME: &str = "rf-detr.cpp";
const ENGINE_REF: &str = "65c0ffcc";
const MODEL_NAME: &str = "rfdetr-nano-f16";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const BINARY_ENV: &str = "SOLSTONE_DESCRIBE_DETECT_BINARY";
const TIMEOUT_ENV: &str = "SOLSTONE_DESCRIBE_DETECT_TIMEOUT_MS";

pub fn screen_gate(analysis: &Value) -> Option<String> {
    for (field, prefix) in [("primary", "primary"), ("secondary", "secondary")] {
        if let Some(value) = analysis.get(field).and_then(Value::as_str) {
            if matches!(value, "media" | "social") {
                return Some(format!("{prefix}:{value}"));
            }
        }
    }
    None
}

pub fn detections_block(result: &Value, source: &str, gate: &str) -> Result<Value, String> {
    let image = result.get("image").ok_or("detector result omitted image")?;
    let objects = result
        .get("detections")
        .ok_or("detector result omitted detections")?;
    Ok(json!({
        "engine": ENGINE_NAME,
        "engine_ref": ENGINE_REF,
        "model": MODEL_NAME,
        "threshold": 0.25,
        "source": source,
        "gate": gate,
        "image": image,
        "objects": objects,
    }))
}

pub fn detect(full_png: &[u8]) -> Result<Value, String> {
    let (binary, model) = match env::var_os(BINARY_ENV) {
        Some(binary) => (PathBuf::from(binary), PathBuf::from("stub-model")),
        None => {
            let paths = query_paths()?;
            if paths.status != "installed" {
                return Err("RF-DETR is not installed".to_owned());
            }
            match (paths.binary, paths.model) {
                (Some(binary), Some(model)) => (binary, model),
                _ => return Err("installed RF-DETR query omitted paths".to_owned()),
            }
        }
    };
    let temp = TempDir::new()?;
    let input = temp.path.join("input.png");
    let output = temp.path.join("output.json");
    fs::write(&input, full_png).map_err(|error| error.to_string())?;
    let mut child = Command::new(binary)
        .args(["detect", "--model"])
        .arg(model)
        .args(["--input"])
        .arg(&input)
        .args(["--output"])
        .arg(&output)
        .args(["--threshold", THRESHOLD, "--threads", "4"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    if !wait_for_child(&mut child, timeout())?.success() {
        return Err("rfdetr-cli detect failed".to_owned());
    }
    serde_json::from_slice(&fs::read(output).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn timeout() -> Duration {
    env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT)
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
                "rfdetr-cli detect timed out after {}ms",
                timeout.as_millis()
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

struct Paths {
    status: String,
    binary: Option<PathBuf>,
    model: Option<PathBuf>,
}

fn query_paths() -> Result<Paths, String> {
    let python = sibling_python()?;
    let output = Command::new(python)
        .args(["-P", "-m", "solstone.observe.rfdetr_paths_query"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("RF-DETR install-state query failed".to_owned());
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    Ok(Paths {
        status: value
            .get("status")
            .and_then(Value::as_str)
            .ok_or("RF-DETR query has no status")?
            .to_owned(),
        binary: value
            .get("binary_path")
            .and_then(Value::as_str)
            .map(PathBuf::from),
        model: value
            .get("model_path")
            .and_then(Value::as_str)
            .map(PathBuf::from),
    })
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

struct TempDir {
    path: PathBuf,
}
impl TempDir {
    fn new() -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-describe-rfdetr-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|error| error.to_string())?;
        Ok(Self { path })
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::{detections_block, screen_gate};
    use serde_json::json;

    #[test]
    fn primary_gate_precedes_secondary() {
        assert_eq!(
            screen_gate(&json!({"primary":"media","secondary":"social"})),
            Some("primary:media".to_owned())
        );
        assert_eq!(
            screen_gate(&json!({"primary":"code","secondary":"social"})),
            Some("secondary:social".to_owned())
        );
    }

    #[test]
    fn stored_block_renames_detections_without_filtering() {
        let result = detections_block(
            &json!({"image":{"width":1},"detections":[{"class_name":"person","score":0.1}]}),
            "screen",
            "primary:media",
        )
        .expect("block");
        assert_eq!(result["objects"][0]["score"], 0.1);
    }
}
