// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded client for the native discovery-cluster helper.

#![allow(dead_code)] // Wired into the scan route in the following conversion slice.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const HELPER: &str = "solstone-core-speakers-analyze";
const REQUEST_SCHEMA: &str = "solstone-speaker-discovery-cluster-request-v1";
const RESPONSE_SCHEMA: &str = "solstone-speaker-discovery-cluster-response-v1";
const ALGORITHM: &str = "hdbscan-eom-euclidean-f64-prim-mst";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveryHelperError {
    pub(crate) stage: &'static str,
    pub(crate) reason: String,
}

pub(crate) async fn discovery_cluster(
    embeddings: Vec<Vec<f32>>,
) -> Result<Vec<i64>, DiscoveryHelperError> {
    let helper = sibling_helper()?;
    tokio::task::spawn_blocking(move || run(&helper, &embeddings))
        .await
        .map_err(|_| DiscoveryHelperError {
            stage: "invoke",
            reason: "task-join".to_owned(),
        })?
}

fn sibling_helper() -> Result<PathBuf, DiscoveryHelperError> {
    let executable = std::env::current_exe().map_err(|error| invoke(error.to_string()))?;
    let path = executable
        .parent()
        .ok_or_else(|| invoke("current-exe-parent"))?
        .join(HELPER);
    let metadata =
        fs::metadata(&path).map_err(|_| invoke(format!("helper-missing:{}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(invoke(format!("helper-not-executable:{}", path.display())));
        }
    }
    Ok(path)
}

fn run(helper: &Path, embeddings: &[Vec<f32>]) -> Result<Vec<i64>, DiscoveryHelperError> {
    let width = embeddings.first().map(Vec::len).unwrap_or(0);
    if width == 0 || embeddings.iter().any(|row| row.len() != width) {
        return Err(invoke("invalid-shape"));
    }
    let dir = temp_dir()?;
    let result = (|| {
        let payload = dir.join("embeddings.f32le");
        write_payload(&payload, embeddings)?;
        let request = json!({"schema":REQUEST_SCHEMA,"embeddings_f32le_path":payload,"payload_format":"raw-f32le-row-major-v1","dtype":"float32-le","shape":[embeddings.len(),width],"min_cluster_size":5,"min_samples":3});
        let mut child = Command::new(helper)
            .arg("discovery-cluster")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| invoke(error.to_string()))?;
        child
            .stdin
            .take()
            .ok_or_else(|| invoke("stdin-unavailable"))?
            .write_all(request.to_string().as_bytes())
            .map_err(|error| invoke(error.to_string()))?;
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| invoke(error.to_string()))?
            {
                if !status.success() {
                    return Err(invoke(format!("exit-{}", status.code().unwrap_or(-1))));
                }
                let output = child
                    .wait_with_output()
                    .map_err(|error| invoke(error.to_string()))?;
                if output.stdout.len() > 1024 * 1024 || output.stderr.len() > 64 * 1024 {
                    return Err(invoke("output-too-large"));
                }
                return parse(&String::from_utf8_lossy(&output.stdout), embeddings.len());
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(invoke("timeout"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    let _ = fs::remove_dir_all(dir);
    result
}
fn temp_dir() -> Result<PathBuf, DiscoveryHelperError> {
    let path = Path::new("/var/tmp").join(format!(
        "solstone-speakers-analyze-discovery-cluster-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).map_err(|error| invoke(error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|error| invoke(error.to_string()))?;
    }
    Ok(path)
}
fn write_payload(path: &Path, rows: &[Vec<f32>]) -> Result<(), DiscoveryHelperError> {
    let mut file = File::create_new(path).map_err(|error| invoke(error.to_string()))?;
    for row in rows {
        for value in row {
            file.write_all(&value.to_le_bytes())
                .map_err(|error| invoke(error.to_string()))?;
        }
    }
    file.sync_all().map_err(|error| invoke(error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| invoke(error.to_string()))?;
    }
    Ok(())
}
fn parse(stdout: &str, rows: usize) -> Result<Vec<i64>, DiscoveryHelperError> {
    let value: Value =
        serde_json::from_str(stdout).map_err(|_| response("response-json-invalid"))?;
    if value.get("schema").and_then(Value::as_str) != Some(RESPONSE_SCHEMA) {
        return Err(response("schema-mismatch"));
    }
    let labels = value
        .get("labels")
        .and_then(Value::as_array)
        .ok_or_else(|| response("labels-invalid"))?
        .iter()
        .map(Value::as_i64)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| response("labels-invalid"))?;
    if labels.len() != rows {
        return Err(response("label-count-mismatch"));
    }
    let params = value.get("parameters").and_then(Value::as_object);
    if params
        .and_then(|p| p.get("min_cluster_size"))
        .and_then(Value::as_i64)
        != Some(5)
        || params
            .and_then(|p| p.get("min_samples"))
            .and_then(Value::as_i64)
            != Some(3)
        || value.get("algorithm").and_then(Value::as_str) != Some(ALGORITHM)
    {
        return Err(response("parameters-mismatch"));
    }
    let noise = labels.iter().filter(|&&label| label == -1).count() as i64;
    let clusters = labels
        .iter()
        .filter(|&&label| label != -1)
        .collect::<std::collections::BTreeSet<_>>()
        .len() as i64;
    if value.get("noise_count").and_then(Value::as_i64) != Some(noise)
        || value.get("cluster_count").and_then(Value::as_i64) != Some(clusters)
    {
        return Err(response("count-mismatch"));
    }
    Ok(labels)
}
fn invoke(reason: impl ToString) -> DiscoveryHelperError {
    DiscoveryHelperError {
        stage: "invoke",
        reason: reason.to_string(),
    }
}
fn response(reason: &str) -> DiscoveryHelperError {
    DiscoveryHelperError {
        stage: "response",
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_validation_separates_stages() {
        assert_eq!(parse("no", 1).unwrap_err().stage, "response");
        let valid = json!({"schema":RESPONSE_SCHEMA,"labels":[-1],"parameters":{"min_cluster_size":5,"min_samples":3},"algorithm":ALGORITHM,"noise_count":1,"cluster_count":0});
        assert_eq!(parse(&valid.to_string(), 1), Ok(vec![-1]));
    }
}
