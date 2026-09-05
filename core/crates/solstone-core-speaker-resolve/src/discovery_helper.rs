// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded client for the native discovery-cluster helper.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use solstone_core_system::lifecycle::HostedServiceParentRuntime;
use solstone_core_system::process::{
    CommandLaunchRequest, Disposition, LaunchAuthority, launch_command, launch_command_hosted,
};

const HELPER: &str = "solstone-core-speakers-analyze";
const REQUEST_SCHEMA: &str = "solstone-speaker-discovery-cluster-request-v1";
const RESPONSE_SCHEMA: &str = "solstone-speaker-discovery-cluster-response-v1";
const ALGORITHM: &str = "hdbscan-eom-euclidean-f64-prim-mst";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static HOSTED_CHILD_LAUNCH_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const STDOUT_LIMIT: usize = 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(180);
const TERMINATE_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct InvocationBudget {
    timeout: Duration,
    terminate_grace: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
}

const DEFAULT_BUDGET: InvocationBudget = InvocationBudget {
    timeout: TIMEOUT,
    terminate_grace: TERMINATE_GRACE,
    stdout_limit: STDOUT_LIMIT,
    stderr_limit: STDERR_LIMIT,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("speaker discovery helper failed at {stage}: {reason}")]
pub struct DiscoveryHelperError {
    pub stage: &'static str,
    pub reason: String,
}

/// Run the bounded clustering helper using the caller's hosted process authority.
pub fn discovery_cluster(
    embeddings: Vec<Vec<f32>>,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<Vec<i64>, DiscoveryHelperError> {
    let helper = sibling_helper()?;
    run(&helper, &embeddings, hosted_parent)
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

fn run(
    helper: &Path,
    embeddings: &[Vec<f32>],
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<Vec<i64>, DiscoveryHelperError> {
    run_with_budget(helper, embeddings, DEFAULT_BUDGET, hosted_parent)
}

fn run_with_budget(
    helper: &Path,
    embeddings: &[Vec<f32>],
    budget: InvocationBudget,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<Vec<i64>, DiscoveryHelperError> {
    let width = embeddings.first().map(Vec::len).unwrap_or(0);
    if width == 0 || embeddings.iter().any(|row| row.len() != width) {
        return Err(invoke("invalid-shape"));
    }
    let dir = temp_dir()?;
    let result = (|| {
        let payload = dir.join("embeddings.f32le");
        write_payload(&payload, embeddings)?;
        let payload_request = json!({"schema":REQUEST_SCHEMA,"embeddings_f32le_path":payload,"payload_format":"raw-f32le-row-major-v1","dtype":"float32-le","shape":[embeddings.len(),width],"min_cluster_size":5,"min_samples":3});
        let command = CommandLaunchRequest {
            program: helper.as_os_str().to_os_string(),
            arguments: vec![OsString::from("discovery-cluster")],
            environment: BTreeMap::new(),
            current_dir: None,
            process_group: false,
            stdin_piped: true,
            stdout_piped: true,
            stderr_piped: true,
        };
        let terminate = Box::new(|child: &mut Child, grace| {
            terminate_raw_child(child, grace);
            Ok(())
        });
        let mut child = match hosted_parent {
            Some(parent) => launch_command_hosted(
                Disposition::IndependentBoundedHelper {
                    timeout: budget.timeout,
                },
                command,
                parent.child_launch_provenance(format!(
                    "convey-speakers-{}",
                    HOSTED_CHILD_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                )),
                terminate,
            ),
            None => launch_command(
                Disposition::IndependentBoundedHelper {
                    timeout: budget.timeout,
                },
                command,
                terminate,
            ),
        }
        .map_err(|error| invoke(error.to_string()))?;
        child
            .take_stdin()
            .ok_or_else(|| invoke("stdin-unavailable"))?
            .write_all(payload_request.to_string().as_bytes())
            .map_err(|error| invoke(error.to_string()))?;
        let stdout = child
            .take_stdout()
            .ok_or_else(|| invoke("stdout-unavailable"))?;
        let stderr = child
            .take_stderr()
            .ok_or_else(|| invoke("stderr-unavailable"))?;
        let (capture_tx, capture_rx) = mpsc::channel();
        let stdout_reader = drain(stdout, budget.stdout_limit, "stdout", capture_tx.clone());
        let stderr_reader = drain(stderr, budget.stderr_limit, "stderr", capture_tx);
        let deadline = Instant::now() + budget.timeout;
        loop {
            if let Ok(error) = capture_rx.try_recv() {
                terminate_and_reap(&mut child, budget.terminate_grace);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(invoke(error));
            }
            if let Some(exit_code) = child.poll().map_err(|error| invoke(error.to_string()))? {
                let stdout = join_capture(stdout_reader)?;
                let _stderr = join_capture(stderr_reader)?;
                if exit_code != 0 {
                    return Err(invoke(format!("exit-{exit_code}")));
                }
                return parse(&String::from_utf8_lossy(&stdout), embeddings.len());
            }
            if Instant::now() >= deadline {
                terminate_and_reap(&mut child, budget.terminate_grace);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(invoke("timeout"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    let _ = fs::remove_dir_all(dir);
    result
}

fn drain<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    name: &'static str,
    sender: mpsc::Sender<String>,
) -> thread::JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            if output.len() + read > limit {
                let error = format!("{name}-too-large");
                let _ = sender.send(error.clone());
                return Err(error);
            }
            output.extend_from_slice(&buffer[..read]);
        }
        Ok(output)
    })
}

fn join_capture(
    reader: thread::JoinHandle<Result<Vec<u8>, String>>,
) -> Result<Vec<u8>, DiscoveryHelperError> {
    reader
        .join()
        .map_err(|_| invoke("capture-thread-panicked"))?
        .map_err(invoke)
}

fn terminate_and_reap(child: &mut LaunchAuthority, grace: Duration) {
    if child.terminate_exact(grace).is_err() {
        let _ = child.terminate(grace);
    }
}

fn terminate_raw_child(child: &mut Child, grace: Duration) {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;

        let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }

    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
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
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn drive_discovery_cluster_helper(
    helper: &Path,
    timeout: Duration,
    terminate_grace: Duration,
    stdout_limit: usize,
) -> Result<Vec<i64>, (String, String)> {
    run_with_budget(
        helper,
        &[vec![1.0]],
        InvocationBudget {
            timeout,
            terminate_grace,
            stdout_limit,
            stderr_limit: 1024,
        },
        None,
    )
    .map_err(|error| (error.stage.to_owned(), error.reason))
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
