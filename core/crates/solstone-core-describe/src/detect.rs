// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Best-effort RF-DETR invocation for screen-gated describe rows.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_assets::canonical_host_pair;
use solstone_core_local::install::rfdetr_install::{
    ENGINE_PROVENANCE_REF, RfdetrInstallError, RfdetrInstallRecord, binary_path,
    check_rfdetr_model, model_path, rfdetr_artifact_key,
};

const THRESHOLD: &str = "0.25";
const ENGINE_NAME: &str = "rf-detr.cpp";
const MODEL_NAME: &str = "rfdetr-nano-f16";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const BINARY_ENV: &str = "SOLSTONE_DESCRIBE_DETECT_BINARY";
const TIMEOUT_ENV: &str = "SOLSTONE_DESCRIBE_DETECT_TIMEOUT_MS";

pub fn screen_gate(analysis: &Value) -> Option<String> {
    for (field, prefix) in [("primary", "primary"), ("secondary", "secondary")] {
        if let Some(value) = analysis.get(field).and_then(Value::as_str)
            && matches!(value, "media" | "social")
        {
            return Some(format!("{prefix}:{value}"));
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
        "engine_ref": ENGINE_PROVENANCE_REF,
        "model": MODEL_NAME,
        "threshold": 0.25,
        "source": source,
        "gate": gate,
        "image": image,
        "objects": objects,
    }))
}

pub fn detect(full_png: &[u8], journal: &Path) -> Result<Value, String> {
    let (binary, model) = match env::var_os(BINARY_ENV) {
        Some(binary) => (PathBuf::from(binary), PathBuf::from("stub-model")),
        None => native_paths(journal)?,
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

fn native_paths(journal: &Path) -> Result<(PathBuf, PathBuf), String> {
    let (os, arch) = canonical_host_pair(env::consts::OS, env::consts::ARCH);
    let key = rfdetr_artifact_key(os, arch);
    let result = check_rfdetr_model(journal, os, arch);
    paths_from_install_check(result, journal, key)
}

fn paths_from_install_check(
    result: Result<RfdetrInstallRecord, RfdetrInstallError>,
    journal: &Path,
    key: Option<&str>,
) -> Result<(PathBuf, PathBuf), String> {
    match result {
        Ok(RfdetrInstallRecord::PlatformUnavailable) => {
            Err("RF-DETR is unavailable on this platform".to_owned())
        }
        Err(_) => Err("RF-DETR is not installed".to_owned()),
        Ok(RfdetrInstallRecord::Installed) => {
            let key = key.ok_or_else(|| "RF-DETR is not installed".to_owned())?;
            let binary = binary_path(journal, key);
            let model = model_path(journal);
            if !binary.is_file() || !model.is_file() {
                return Err("RF-DETR is not installed".to_owned());
            }
            Ok((binary, model))
        }
    }
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
    use std::fs;
    #[cfg(unix)]
    use std::io::Read;
    #[cfg(unix)]
    use std::process::{Command, Stdio};
    #[cfg(unix)]
    use std::time::Duration;

    use super::{
        RfdetrInstallError, RfdetrInstallRecord, binary_path, detections_block, model_path,
        paths_from_install_check, screen_gate, wait_for_child,
    };
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

    #[cfg(unix)]
    #[test]
    fn detector_timeout_cancels_a_child_that_reported_ready() {
        let mut child = Command::new("sh")
            .args(["-c", "printf ready; exec sleep 120"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start ready child");
        let mut ready = [0; 5];
        child
            .stdout
            .as_mut()
            .expect("child stdout")
            .read_exact(&mut ready)
            .expect("ready event");
        assert_eq!(&ready, b"ready");

        let error = wait_for_child(&mut child, Duration::from_millis(100))
            .expect_err("ready child must be cancelled at the failure ceiling");
        assert_eq!(error, "rfdetr-cli detect timed out after 100ms");
        assert!(child.try_wait().expect("reaped child").is_some());
    }

    #[test]
    fn native_install_query_covers_all_provider_worlds_at_the_given_journal() {
        let journal = std::env::temp_dir().join(format!(
            "solstone-describe-detect-journal-{}",
            std::process::id()
        ));
        fs::create_dir_all(&journal).expect("create unique journal");

        assert_eq!(
            paths_from_install_check(Ok(RfdetrInstallRecord::PlatformUnavailable), &journal, None),
            Err("RF-DETR is unavailable on this platform".to_owned())
        );
        assert_eq!(
            paths_from_install_check(
                Err(RfdetrInstallError::new("sidecar_missing", "missing", 65)),
                &journal,
                Some("linux-cpu-x64"),
            ),
            Err("RF-DETR is not installed".to_owned())
        );

        let binary = binary_path(&journal, "linux-cpu-x64");
        let model = model_path(&journal);
        fs::create_dir_all(binary.parent().expect("binary parent")).expect("binary parent");
        fs::create_dir_all(model.parent().expect("model parent")).expect("model parent");
        fs::write(&binary, b"binary").expect("write binary");
        fs::write(&model, b"model").expect("write model");
        assert_eq!(
            paths_from_install_check(
                Ok(RfdetrInstallRecord::Installed),
                &journal,
                Some("linux-cpu-x64"),
            ),
            Ok((binary.clone(), model.clone()))
        );
        assert!(binary.starts_with(&journal));
        assert!(model.starts_with(&journal));

        fs::remove_file(&model).expect("remove installed model");
        assert_eq!(
            paths_from_install_check(
                Ok(RfdetrInstallRecord::Installed),
                &journal,
                Some("linux-cpu-x64"),
            ),
            Err("RF-DETR is not installed".to_owned())
        );
        fs::remove_dir_all(&journal).expect("remove unique journal");
    }
}
