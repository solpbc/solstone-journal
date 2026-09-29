// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-modality transcript reprocessing.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use solstone_core_system_health::{DataState, derive_modality_state};

use crate::delete::{valid_day, valid_key, valid_stream};
use crate::{AppState, legacy_error_response};

const SENSE_BINARY: &str = "solstone-core";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SenseRequest {
    pub(crate) day: String,
    pub(crate) stream: String,
    pub(crate) key: String,
    pub(crate) modality: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ChildExit {
    pub(crate) code: i32,
    pub(crate) stderr: String,
}

pub(crate) trait SenseChild: Send {
    fn wait(self: Box<Self>) -> Result<ChildExit, String>;
}

pub(crate) trait SenseSpawner: Send + Sync {
    fn spawn(
        &self,
        request: &SenseRequest,
        #[cfg(windows)] context: Option<&crate::SenseLaunchContext>,
    ) -> Result<Box<dyn SenseChild>, String>;
}

pub(crate) fn sense_reprocess_args(request: &SenseRequest) -> Vec<String> {
    vec![
        "sense".to_string(),
        "--day".to_string(),
        request.day.clone(),
        "--segment".to_string(),
        request.key.clone(),
        "--stream".to_string(),
        request.stream.clone(),
        "--reprocess".to_string(),
        request.modality.clone(),
    ]
}

#[derive(Default)]
pub(crate) struct ProcessSenseSpawner;

impl SenseSpawner for ProcessSenseSpawner {
    fn spawn(
        &self,
        request: &SenseRequest,
        #[cfg(windows)] context: Option<&crate::SenseLaunchContext>,
    ) -> Result<Box<dyn SenseChild>, String> {
        use solstone_core_system::process::{
            CommandLaunchRequest, Disposition, LaunchError, launch_command, launch_command_hosted,
        };
        let helper = sibling_sense_binary()?;
        let launch_id = format!(
            "convey-reprocess-{}",
            random_hex().map_err(|e| e.to_string())?
        );
        #[cfg(unix)]
        let provenance = solstone_core_system::lifecycle::hosted_child_launch_provenance(
            launch_id,
            Duration::from_secs(5),
        )
        .map_err(|error| error.to_string())?;
        #[cfg(windows)]
        let context = context
            .ok_or_else(|| "reprocessing generation was not admitted at Convey entry".to_owned())?;
        #[cfg(windows)]
        let provenance = context
            .hosted_parent
            .as_ref()
            .map(|parent| parent.child_launch_provenance(launch_id));
        let command = CommandLaunchRequest {
            #[cfg(windows)]
            read_file_grants: context.generation.read_file_grants.clone(),
            program: helper.into_os_string(),
            arguments: sense_reprocess_args(request)
                .into_iter()
                .map(Into::into)
                .collect(),
            #[cfg(unix)]
            environment: BTreeMap::new(),
            #[cfg(windows)]
            environment: context.generation.environment.clone(),
            current_dir: None,
            process_group: false,
            stdin_piped: false,
            stdout_piped: false,
            stderr_piped: true,
        };
        let terminate = Box::new(|child: &mut std::process::Child, _| {
            child.kill().map_err(LaunchError::Terminate)
        });
        let child = match provenance {
            Some(provenance) => launch_command_hosted(
                Disposition::InheritedParentScope,
                command,
                provenance,
                terminate,
            ),
            None => launch_command(Disposition::InheritedParentScope, command, terminate),
        }
        .map_err(|error| error.to_string())?;
        Ok(Box::new(ProcessChild(child)))
    }
}

struct ProcessChild(solstone_core_system::process::LaunchAuthority);

impl SenseChild for ProcessChild {
    fn wait(self: Box<Self>) -> Result<ChildExit, String> {
        let output = self
            .0
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        Ok(ChildExit {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

pub(crate) async fn reprocess_segment(
    State(state): State<Arc<AppState>>,
    #[cfg(windows)] context: Option<axum::extract::Extension<crate::SenseLaunchContext>>,
    RoutePath((day, stream, key)): RoutePath<(String, String, String)>,
    body: Bytes,
) -> Response {
    if !valid_day(&day) {
        return invalid_day("Invalid day format", StatusCode::BAD_REQUEST);
    }
    if !valid_key(&key) {
        return invalid_segment("Invalid segment key format", StatusCode::BAD_REQUEST);
    }
    if !valid_stream(&stream) {
        return invalid_segment("Invalid stream format", StatusCode::BAD_REQUEST);
    }
    let day_dir = state.journal_root.join("chronicle").join(&day);
    if !day_dir.is_dir() {
        return invalid_day("Day not found", StatusCode::NOT_FOUND);
    }
    let segment_dir =
        crate::segment_media::physical_segment_dir(&state.journal_root, &day, &stream, &key);
    if !segment_dir.is_dir() {
        return invalid_segment("Segment not found", StatusCode::NOT_FOUND);
    }
    // valid_day/valid_stream/valid_key exclude a `..` path component, so this
    // is defense in depth for a future validator regression, not reachable now;
    // retention also refuses per-entry removals outside the journal.
    if segment_dir.strip_prefix(&day_dir).is_err() {
        return invalid_segment("Invalid segment path", StatusCode::FORBIDDEN);
    }
    let modality = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("modality")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    let Some(modality) =
        modality.filter(|value| matches!(value.as_str(), "audio" | "screen" | "image"))
    else {
        return legacy_error_response(
            "invalid_request_value",
            "one of those values couldn't be used.",
            "modality must be audio, screen, or image",
            StatusCode::BAD_REQUEST,
        );
    };

    let signals = modality_signals(&segment_dir, &modality, state.clock.now());
    if signals.state == DataState::Analyzed {
        return legacy_error_response(
            "invalid_operation_for_state",
            "that action isn't available in the current state.",
            "Segment modality is already analyzed",
            StatusCode::BAD_REQUEST,
        );
    }
    if signals.state == DataState::Purged || !signals.has_raw {
        return legacy_error_response(
            "raw_media_not_available",
            "analysis couldn't run because the raw media is no longer available.",
            "Raw media is no longer available",
            StatusCode::BAD_REQUEST,
        );
    }
    let failed_path = failed_marker_path(&segment_dir, &modality);
    if signals.state == DataState::Analyzing {
        return running_response(&segment_dir, &modality, state.clock.now());
    }
    if matches!(signals.state, DataState::Failed | DataState::FailedFinal) {
        repair_modality_markers(&segment_dir, &modality, signals.has_chunks);
        let _ = fs::remove_file(&failed_path);
    }

    let marker = match tokio::task::spawn_blocking({
        let segment_dir = segment_dir.clone();
        let modality = modality.clone();
        move || create_analyzing_marker(&segment_dir, &modality)
    })
    .await
    {
        Ok(Ok(marker)) => marker,
        Ok(Err(CreateMarkerError::Exists)) => {
            return running_response(&segment_dir, &modality, state.clock.now());
        }
        Ok(Err(CreateMarkerError::Io(error))) => {
            return legacy_error_response(
                "file_read_failed",
                "that file couldn't be read.",
                format!("Failed to create analysis marker: {error}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
        Err(error) => {
            return legacy_error_response(
                "file_read_failed",
                "that file couldn't be read.",
                format!("Failed to create analysis marker: {error}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let request = SenseRequest {
        day,
        stream,
        key,
        modality: modality.clone(),
    };
    let child = match state.sense_spawner.spawn(
        &request,
        #[cfg(windows)]
        context.as_ref().map(|extension| &extension.0),
    ) {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_file(&marker.path);
            return legacy_error_response(
                "file_read_failed",
                "that file couldn't be read.",
                format!("Failed to start analysis: {error}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let watch_state = Arc::clone(&state);
    let watch_segment = segment_dir.clone();
    let watch_modality = modality.clone();
    let request_id = marker.request_id.clone();
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || child.wait()).await;
        if let Ok(Ok(exit)) = result {
            watch_reprocess_completion(
                &watch_segment,
                &watch_modality,
                &request_id,
                exit,
                watch_state.clock.now(),
            );
        }
    });
    let data_state = data_state(
        &segment_dir,
        state.clock.now(),
        Some((&modality, DataState::Analyzing)),
    );
    Json(json!({"data_state":data_state,"marker":{"started_at":marker.started_at},"repair_status":"accepted"})).into_response()
}

fn sibling_sense_binary() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    sibling_sense_binary_beside(&current)
}

/// Resolve only beside the executable that owns the server process. Keeping the
/// path input explicit lets the adjacency rule be tested without consulting or
/// mutating this process's PATH.
fn sibling_sense_binary_beside(current: &Path) -> Result<PathBuf, String> {
    let path = current
        .parent()
        .ok_or_else(|| "current executable has no parent".to_owned())?
        .join(format!("{SENSE_BINARY}{}", std::env::consts::EXE_SUFFIX));
    let metadata = fs::metadata(&path).map_err(|_| format!("helper-missing:{}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!("helper-not-executable:{}", path.display()));
        }
    }
    Ok(path)
}

#[derive(Debug)]
struct Marker {
    path: PathBuf,
    request_id: String,
    started_at: String,
}

#[derive(Debug)]
enum CreateMarkerError {
    Exists,
    Io(std::io::Error),
}

fn create_analyzing_marker(
    segment_dir: &Path,
    modality: &str,
) -> Result<Marker, CreateMarkerError> {
    let path = analyzing_marker_path(segment_dir, modality);
    let request_id = random_hex().map_err(CreateMarkerError::Io)?;
    let started_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let body = json!({"started_at":started_at,"modality":modality,"request_id":request_id});
    let temporary = segment_dir.join(format!(".analyzing_{modality}.{request_id}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(CreateMarkerError::Io)?;
    if let Err(error) = file
        .write_all(body.to_string().as_bytes())
        .and_then(|_| file.write_all(b"\n"))
    {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(CreateMarkerError::Io(error));
    }
    drop(file);
    // Linking a closed inode publishes complete JSON without replacing an
    // existing claim, including when another server process wins the race.
    let published = fs::hard_link(&temporary, &path);
    let _ = fs::remove_file(&temporary);
    if let Err(error) = published {
        return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
            CreateMarkerError::Exists
        } else {
            CreateMarkerError::Io(error)
        });
    }
    Ok(Marker {
        path,
        request_id,
        started_at,
    })
}

fn random_hex() -> Result<String, std::io::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn analyzing_marker_path(segment_dir: &Path, modality: &str) -> PathBuf {
    segment_dir.join(format!(".analyzing_{modality}"))
}

fn failed_marker_path(segment_dir: &Path, modality: &str) -> PathBuf {
    segment_dir.join(format!(".analyze_failed_{modality}"))
}

fn marker_payload(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn running_response(segment_dir: &Path, modality: &str, now: chrono::DateTime<Utc>) -> Response {
    let marker = marker_payload(&analyzing_marker_path(segment_dir, modality));
    let data_state = data_state(segment_dir, now, Some((modality, DataState::Analyzing)));
    Json(json!({"data_state":data_state,"marker":{"started_at":marker.get("started_at").and_then(Value::as_str).unwrap_or_default()},"repair_status":"running"})).into_response()
}

fn watch_reprocess_completion(
    segment_dir: &Path,
    modality: &str,
    request_id: &str,
    exit: ChildExit,
    now: chrono::DateTime<Utc>,
) {
    let marker = analyzing_marker_path(segment_dir, modality);
    if marker_payload(&marker)
        .get("request_id")
        .and_then(Value::as_str)
        != Some(request_id)
    {
        return;
    }
    if exit.code == 0 {
        let state = modality_signals(segment_dir, modality, now).state;
        let success = if modality == "image" {
            state == DataState::Analyzed
        } else {
            matches!(state, DataState::Analyzed | DataState::Empty)
        };
        if success {
            let _ = fs::remove_file(marker);
            return;
        }
        write_failed_marker(
            segment_dir,
            modality,
            "no_output",
            "worker exited 0 without analyzed chunks",
            Some("no_output"),
        );
        return;
    }
    let stderr = tail(&exit.stderr, 512);
    write_failed_marker(
        segment_dir,
        modality,
        &format!("exit_{}", exit.code),
        &stderr,
        None,
    );
}

fn tail(value: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let start = value
        .char_indices()
        .rev()
        .nth(limit.saturating_sub(1))
        .map(|(index, _)| index)
        .unwrap_or(0);
    value[start..].to_owned()
}

fn write_failed_marker(
    segment_dir: &Path,
    modality: &str,
    reason: &str,
    detail: &str,
    reason_code: Option<&str>,
) {
    let marker = analyzing_marker_path(segment_dir, modality);
    let payload = marker_payload(&marker);
    let mut failed = serde_json::Map::new();
    failed.insert(
        "started_at".into(),
        payload
            .get("started_at")
            .cloned()
            .unwrap_or_else(|| json!("")),
    );
    failed.insert("modality".into(), json!(modality));
    failed.insert("reason".into(), json!(reason));
    failed.insert(
        "failed_at".into(),
        json!(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    failed.insert("detail".into(), json!(detail));
    if let Some(reason_code) = reason_code {
        failed.insert("reason_code".into(), json!(reason_code));
    }
    let failed_path = failed_marker_path(segment_dir, modality);
    let temporary = failed_path.with_extension("tmp");
    if fs::write(&temporary, format!("{}\n", Value::Object(failed))).is_ok() {
        let _ = fs::rename(temporary, &failed_path);
    }
    let _ = fs::remove_file(marker);
}

/// Repair stale/corrupt analyzing markers before a retry. A populated marker
/// either becomes a failure record or is removed when real chunks now win.
fn repair_modality_markers(segment_dir: &Path, modality: &str, has_chunks: bool) {
    let marker = analyzing_marker_path(segment_dir, modality);
    if has_chunks {
        let _ = fs::remove_file(marker);
        return;
    }
    if marker.is_file() {
        write_failed_marker(
            segment_dir,
            modality,
            "stale",
            "repaired before reprocess",
            None,
        );
    }
}

#[derive(Clone, Copy)]
struct Signals {
    state: DataState,
    has_raw: bool,
    has_chunks: bool,
}

fn modality_signals(segment_dir: &Path, modality: &str, now: chrono::DateTime<Utc>) -> Signals {
    if modality == "image" {
        let parent_name = segment_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        return match solstone_core_system_health::unclaimed_image_state(
            segment_dir,
            parent_name,
            now,
        ) {
            Ok(unclaimed) => Signals {
                has_raw: unclaimed.state != DataState::Absent,
                has_chunks: unclaimed.state == DataState::Analyzed,
                state: unclaimed.state,
            },
            Err(_) => Signals {
                state: DataState::Absent,
                has_raw: false,
                has_chunks: false,
            },
        };
    }
    let mut has_raw = false;
    let mut has_jsonl = false;
    let mut has_chunks = false;
    let mut record = None;
    if let Ok(entries) = fs::read_dir(segment_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let kind = solstone_core_processing_record::media_kind(extension);
            if (modality == "audio"
                && kind == Some(solstone_core_processing_record::MediaKind::Audio))
                || (modality == "screen"
                    && kind == Some(solstone_core_processing_record::MediaKind::Video))
            {
                has_raw = true;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if name.ends_with(&format!("{modality}.jsonl")) {
                has_jsonl = true;
                if let Ok(text) = fs::read_to_string(path) {
                    for (index, line) in text.lines().enumerate() {
                        let Ok(value) = serde_json::from_str::<Value>(line) else {
                            continue;
                        };
                        if index == 0 {
                            record = value.get("_solstone_processing").cloned();
                        }
                        has_chunks |= match modality {
                            "audio" => value.get("start").is_some(),
                            _ => value.get("frame_id").is_some(),
                        };
                    }
                }
            }
        }
    }
    let state = derive_modality_state(
        segment_dir,
        modality,
        has_chunks,
        has_jsonl,
        has_raw,
        record.as_ref(),
        now,
    );
    Signals {
        state,
        has_raw,
        has_chunks,
    }
}

fn data_state(
    segment_dir: &Path,
    now: chrono::DateTime<Utc>,
    override_state: Option<(&str, DataState)>,
) -> BTreeMap<String, String> {
    let mut states = BTreeMap::new();
    for modality in ["audio", "screen", "image"] {
        let state = override_state
            .filter(|(name, _)| *name == modality)
            .map(|(_, state)| state)
            .unwrap_or_else(|| modality_signals(segment_dir, modality, now).state);
        if state != DataState::Absent {
            states.insert(modality.to_owned(), state.as_str().to_owned());
        }
    }
    states
}

fn invalid_day(detail: &str, status: StatusCode) -> Response {
    legacy_error_response("invalid_day", "that day couldn't be used.", detail, status)
}

fn invalid_segment(detail: &str, status: StatusCode) -> Response {
    legacy_error_response(
        "invalid_segment_or_stream",
        "that segment or stream couldn't be used.",
        detail,
        status,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use chrono::{TimeZone, Utc};
    use serde_json::Value;
    use tempfile::TempDir;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    use super::{
        ChildExit, CreateMarkerError, SenseChild, SenseRequest, SenseSpawner,
        analyzing_marker_path, create_analyzing_marker, data_state, failed_marker_path,
        marker_payload, repair_modality_markers, sense_reprocess_args, sibling_sense_binary_beside,
        watch_reprocess_completion,
    };

    struct FailingSpawner;

    impl SenseSpawner for FailingSpawner {
        fn spawn(
            &self,
            _request: &SenseRequest,
            #[cfg(windows)] _context: Option<&crate::SenseLaunchContext>,
        ) -> Result<Box<dyn SenseChild>, String> {
            Err("spawn failed".into())
        }
    }

    struct DelayedSpawner {
        launches: Arc<AtomicUsize>,
        release: Arc<Notify>,
    }

    impl SenseSpawner for DelayedSpawner {
        fn spawn(
            &self,
            _request: &SenseRequest,
            #[cfg(windows)] _context: Option<&crate::SenseLaunchContext>,
        ) -> Result<Box<dyn SenseChild>, String> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(DelayedChild {
                release: Arc::clone(&self.release),
            }))
        }
    }

    struct DelayedChild {
        release: Arc<Notify>,
    }

    impl SenseChild for DelayedChild {
        fn wait(self: Box<Self>) -> Result<ChildExit, String> {
            tokio::runtime::Handle::current().block_on(self.release.notified());
            Ok(ChildExit {
                code: 0,
                stderr: String::new(),
            })
        }
    }

    struct ExpectingSpawner {
        expected: SenseRequest,
        calls: AtomicUsize,
    }

    impl SenseSpawner for ExpectingSpawner {
        fn spawn(
            &self,
            request: &SenseRequest,
            #[cfg(windows)] _context: Option<&crate::SenseLaunchContext>,
        ) -> Result<Box<dyn SenseChild>, String> {
            if request != &self.expected {
                return Err(format!("unexpected request: {request:?}"));
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(ImmediateChild {
                code: self.expected.key.len() as i32,
            }))
        }
    }

    struct ImmediateChild {
        code: i32,
    }

    impl SenseChild for ImmediateChild {
        fn wait(self: Box<Self>) -> Result<ChildExit, String> {
            Ok(ChildExit {
                code: self.code,
                stderr: String::new(),
            })
        }
    }

    fn shell() -> axum::response::Response {
        axum::response::Response::new(Body::empty())
    }

    fn write(root: &std::path::Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, Vec<u8>)> {
        fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<PathBuf, (u64, Vec<u8>)>) {
            for entry in fs::read_dir(path).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(root, &path, snapshot);
                } else {
                    let metadata = fs::metadata(&path).unwrap();
                    snapshot.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        (metadata.len(), fs::read(path).unwrap()),
                    );
                }
            }
        }
        let mut result = BTreeMap::new();
        visit(root, root, &mut result);
        result
    }

    async fn wait_until(predicate: impl Fn() -> bool, what: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !predicate() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{what}"));
    }

    fn assert_only_segment_changed(
        before: &BTreeMap<PathBuf, (u64, Vec<u8>)>,
        after: &BTreeMap<PathBuf, (u64, Vec<u8>)>,
    ) {
        for path in before.keys().chain(after.keys()) {
            if before.get(path) != after.get(path) {
                assert!(
                    path.starts_with("chronicle/20260731/field/090000_300"),
                    "unexpected journal mutation: {}",
                    path.display()
                );
            }
        }
    }

    fn test_router(root: &std::path::Path, spawner: Arc<dyn SenseSpawner>) -> axum::Router {
        crate::router_with_test_spawner(
            root.to_path_buf(),
            crate::Clock::fixed(Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap()),
            shell,
            Duration::from_secs(10),
            spawner,
        )
    }

    #[test]
    fn exclusive_marker_creation_reports_existing_marker() {
        let root = TempDir::new().expect("segment");
        let first = create_analyzing_marker(root.path(), "audio").expect("first marker");
        assert!(matches!(
            create_analyzing_marker(root.path(), "audio"),
            Err(CreateMarkerError::Exists)
        ));
        assert!(first.path.is_file());
        let payload = marker_payload(&first.path);
        assert_eq!(payload["request_id"], first.request_id);
        assert_eq!(payload["started_at"], first.started_at);
        assert_eq!(payload["modality"], "audio");
        assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn repair_modality_markers_is_noop_fresh_and_repairs_a_populated_stale_marker() {
        let root = TempDir::new().expect("segment");
        repair_modality_markers(root.path(), "audio", false);
        assert!(!analyzing_marker_path(root.path(), "audio").exists());
        assert!(!failed_marker_path(root.path(), "audio").exists());

        let marker = create_analyzing_marker(root.path(), "audio").expect("marker");
        fs::File::open(&marker.path)
            .unwrap()
            .set_modified(UNIX_EPOCH)
            .unwrap();
        assert_eq!(
            super::modality_signals(
                root.path(),
                "audio",
                Utc.with_ymd_and_hms(2026, 7, 31, 9, 0, 0).unwrap()
            )
            .state,
            super::DataState::Failed
        );
        repair_modality_markers(root.path(), "audio", false);
        assert!(!marker.path.exists());
        assert!(failed_marker_path(root.path(), "audio").exists());
    }

    #[test]
    fn sibling_sense_binary_beside_resolves_only_the_adjacent_solstone_core() {
        let root = TempDir::new().expect("shims");
        let adjacent = root.path().join("adjacent");
        fs::create_dir_all(&adjacent).unwrap();
        let sibling = adjacent.join("solstone-core");
        fs::write(&sibling, b"helper").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&sibling).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&sibling, permissions).unwrap();
        }
        assert_eq!(
            sibling_sense_binary_beside(&adjacent.join("server")).unwrap(),
            sibling
        );
        assert!(
            sibling_sense_binary_beside(Path::new("/"))
                .unwrap_err()
                .contains("current executable has no parent")
        );
        assert!(
            sibling_sense_binary_beside(&root.path().join("missing").join("server"))
                .unwrap_err()
                .starts_with("helper-missing:")
        );
    }

    #[tokio::test]
    async fn reprocess_handler_forwards_the_sense_request_to_the_injected_spawner() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "chronicle/20260731/field/090000_300/audio.flac",
            b"raw",
        );
        let expected = SenseRequest {
            day: "20260731".into(),
            stream: "field".into(),
            key: "090000_300".into(),
            modality: "audio".into(),
        };
        let spawner = Arc::new(ExpectingSpawner {
            expected: expected.clone(),
            calls: AtomicUsize::new(0),
        });
        let response = test_router(root.path(), Arc::clone(&spawner) as Arc<dyn SenseSpawner>)
            .oneshot(
                Request::post("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
                    .body(Body::from(r#"{"modality":"audio"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 1);

        let mismatch = TempDir::new().unwrap();
        write(
            mismatch.path(),
            "chronicle/20260731/field/090000_300/audio.flac",
            b"raw",
        );
        let rejected = test_router(
            mismatch.path(),
            Arc::new(ExpectingSpawner {
                expected: SenseRequest {
                    key: "other".into(),
                    ..expected
                },
                calls: AtomicUsize::new(0),
            }),
        )
        .oneshot(
            Request::post("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
                .body(Body::from(r#"{"modality":"audio"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(rejected.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn watcher_writes_only_for_its_own_request_and_preserves_exit_shape() {
        let root = TempDir::new().expect("segment");
        let now = Utc.with_ymd_and_hms(2026, 7, 31, 9, 0, 0).unwrap();
        let marker = create_analyzing_marker(root.path(), "audio").expect("marker");
        watch_reprocess_completion(
            root.path(),
            "audio",
            &marker.request_id,
            ChildExit {
                code: 7,
                stderr: "broken".into(),
            },
            now,
        );
        let failed: Value = serde_json::from_str(
            &fs::read_to_string(failed_marker_path(root.path(), "audio")).expect("failed marker"),
        )
        .expect("json");
        assert_eq!(failed["reason"], "exit_7");
        assert!(failed.get("reason_code").is_none());

        let replacement =
            create_analyzing_marker(root.path(), "audio").expect("replacement marker");
        watch_reprocess_completion(
            root.path(),
            "audio",
            "wrong",
            ChildExit {
                code: 0,
                stderr: String::new(),
            },
            now,
        );
        assert!(analyzing_marker_path(root.path(), "audio").is_file());
        assert_eq!(
            replacement.request_id,
            super::marker_payload(&analyzing_marker_path(root.path(), "audio"))["request_id"]
        );

        fs::remove_file(analyzing_marker_path(root.path(), "audio")).unwrap();
        let no_output = create_analyzing_marker(root.path(), "audio").expect("no-output marker");
        watch_reprocess_completion(
            root.path(),
            "audio",
            &no_output.request_id,
            ChildExit {
                code: 0,
                stderr: String::new(),
            },
            now,
        );
        let no_output_failed: Value = serde_json::from_str(
            &fs::read_to_string(failed_marker_path(root.path(), "audio")).unwrap(),
        )
        .unwrap();
        assert_eq!(no_output_failed["reason_code"], "no_output");

        let success = create_analyzing_marker(root.path(), "audio").expect("success marker");
        fs::write(root.path().join("audio.jsonl"), b"{\"start\": \"1\"}\n").unwrap();
        watch_reprocess_completion(
            root.path(),
            "audio",
            &success.request_id,
            ChildExit {
                code: 0,
                stderr: String::new(),
            },
            now,
        );
        assert!(!analyzing_marker_path(root.path(), "audio").exists());
    }

    #[tokio::test]
    async fn missing_day_and_segment_are_distinct_404_reprocess_refusals() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "chronicle/20260731/field/090000_300/audio.flac",
            b"raw",
        );
        let app = test_router(root.path(), Arc::new(FailingSpawner));
        for (path, expected) in [
            (
                "/app/transcripts/api/segment/20260730/field/090000_300/reprocess",
                "invalid_day",
            ),
            (
                "/app/transcripts/api/segment/20260731/field/090001_300/reprocess",
                "invalid_segment_or_stream",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::post(path)
                        .body(Body::from(r#"{"modality":"audio"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(body["reason_code"], expected);
        }
    }

    #[tokio::test]
    async fn failed_spawn_removes_the_marker_it_created() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "chronicle/20260731/field/090000_300/audio.flac",
            b"raw",
        );
        let response = test_router(root.path(), Arc::new(FailingSpawner))
            .oneshot(
                Request::post("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
                    .body(Body::from(r#"{"modality":"audio"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            !analyzing_marker_path(
                &root.path().join("chronicle/20260731/field/090000_300"),
                "audio"
            )
            .exists()
        );
    }

    #[tokio::test]
    async fn concurrent_reprocess_creates_one_marker_and_spawns_once() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "chronicle/20260731/field/090000_300/audio.flac",
            b"raw",
        );
        let before = snapshot(root.path());
        let launches = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let app = test_router(
            root.path(),
            Arc::new(DelayedSpawner {
                launches: Arc::clone(&launches),
                release: Arc::clone(&release),
            }),
        );
        let path = "/app/transcripts/api/segment/20260731/field/090000_300/reprocess";
        let first = app.clone().oneshot(
            Request::post(path)
                .body(Body::from(r#"{"modality":"audio"}"#))
                .unwrap(),
        );
        let second = app.clone().oneshot(
            Request::post(path)
                .body(Body::from(r#"{"modality":"audio"}"#))
                .unwrap(),
        );
        let (first, second) = tokio::join!(first, second);
        let first: Value = serde_json::from_slice(
            &to_bytes(first.unwrap().into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let second: Value = serde_json::from_slice(
            &to_bytes(second.unwrap().into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(launches.load(Ordering::SeqCst), 1);
        let statuses = [
            first["repair_status"].as_str().unwrap(),
            second["repair_status"].as_str().unwrap(),
        ];
        assert!(statuses.contains(&"accepted"));
        assert!(statuses.contains(&"running"));
        assert_eq!(
            first["marker"]["started_at"],
            second["marker"]["started_at"]
        );
        for response in [&first, &second] {
            assert_eq!(
                response
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                std::collections::BTreeSet::from([
                    "data_state".into(),
                    "marker".into(),
                    "repair_status".into(),
                ])
            );
            assert_eq!(
                response["marker"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                std::collections::BTreeSet::from(["started_at".into()])
            );
        }
        release.notify_one();
        let marker = analyzing_marker_path(
            &root.path().join("chronicle/20260731/field/090000_300"),
            "audio",
        );
        wait_until(
            || !marker.exists(),
            "reprocess watcher did not settle the analyzing marker",
        )
        .await;
        let after = snapshot(root.path());
        assert_only_segment_changed(&before, &after);
    }

    #[test]
    fn sense_reprocess_args_parses_with_solstone_core_cli() {
        let request = SenseRequest {
            day: "20260731".into(),
            stream: "field".into(),
            key: "090000_300".into(),
            modality: "image".into(),
        };
        let args = super::sense_reprocess_args(&request);
        let os_args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
        let parsed = solstone_core_cli::evaluate_args(&os_args).unwrap();
        match parsed {
            solstone_core_cli::Command::Sense(options) => {
                assert_eq!(options.day, Some("20260731".into()));
                assert_eq!(options.segment, Some("090000_300".into()));
                assert_eq!(options.stream, Some("field".into()));
                assert_eq!(
                    options.reprocess,
                    Some(solstone_core_cli::SenseReprocessKind::Image)
                );
            }
            other => panic!("expected Command::Sense, got {other:?}"),
        }
    }

    #[test]
    fn watcher_handles_image_modality_completion() {
        let root = TempDir::new().expect("segment");
        let now = Utc.with_ymd_and_hms(2026, 7, 31, 9, 0, 0).unwrap();

        // 1. Exit 0 with bare still -> analyzing marker removed, data_state is failed
        write(root.path(), "photo.jpg", b"jpeg bytes");
        let marker = create_analyzing_marker(root.path(), "image").expect("marker");
        watch_reprocess_completion(
            root.path(),
            "image",
            &marker.request_id,
            ChildExit {
                code: 0,
                stderr: String::new(),
            },
            now,
        );
        assert!(!analyzing_marker_path(root.path(), "image").exists());
        assert_eq!(data_state(root.path(), now, None)["image"], "failed");
        let failed_path = failed_marker_path(root.path(), "image");
        assert!(failed_path.is_file());
        fs::remove_file(&failed_path).unwrap();

        // 2. Exit 0 with analyzed sidecar -> analyzing marker removed, data_state is analyzed
        write(
            root.path(),
            "photo.jsonl",
            b"{\"_solstone_processing\":{\"handler\":\"depict\",\"schema_version\":1,\"model\":\"mock\",\"device\":\"cpu\",\"state\":\"analyzed\"}}\n{\"start\":\"00:00:00\",\"text\":\"desk\"}\n",
        );
        let marker2 = create_analyzing_marker(root.path(), "image").expect("marker2");
        watch_reprocess_completion(
            root.path(),
            "image",
            &marker2.request_id,
            ChildExit {
                code: 0,
                stderr: String::new(),
            },
            now,
        );
        assert!(!analyzing_marker_path(root.path(), "image").exists());
        assert!(!failed_marker_path(root.path(), "image").exists());
        assert_eq!(data_state(root.path(), now, None)["image"], "analyzed");

        // 3. Exit 1 with bare still -> analyzing marker removed, data_state is failed
        fs::remove_file(root.path().join("photo.jsonl")).unwrap();
        let marker3 = create_analyzing_marker(root.path(), "image").expect("marker3");
        watch_reprocess_completion(
            root.path(),
            "image",
            &marker3.request_id,
            ChildExit {
                code: 1,
                stderr: "failure output".into(),
            },
            now,
        );
        assert!(!analyzing_marker_path(root.path(), "image").exists());
        assert_eq!(data_state(root.path(), now, None)["image"], "failed");
        assert!(failed_marker_path(root.path(), "image").is_file());
    }

    #[tokio::test]
    async fn reprocess_image_endpoint_flow() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "chronicle/20260731/field/090000_300/photo.jpg",
            b"raw photo",
        );
        let spawner = Arc::new(ExpectingSpawner {
            expected: SenseRequest {
                day: "20260731".into(),
                stream: "field".into(),
                key: "090000_300".into(),
                modality: "image".into(),
            },
            calls: AtomicUsize::new(0),
        });
        let response = test_router(root.path(), Arc::clone(&spawner) as Arc<dyn SenseSpawner>)
            .oneshot(
                Request::post("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
                    .body(Body::from(r#"{"modality":"image"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 1);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["data_state"]["image"], "analyzing");
        let args = sense_reprocess_args(&spawner.expected);
        let os_args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
        let parsed = solstone_core_cli::evaluate_args(&os_args).unwrap();
        match parsed {
            solstone_core_cli::Command::Sense(options) => {
                assert_eq!(options.day, Some("20260731".into()));
                assert_eq!(options.segment, Some("090000_300".into()));
                assert_eq!(options.stream, Some("field".into()));
                assert_eq!(
                    options.reprocess,
                    Some(solstone_core_cli::SenseReprocessKind::Image)
                );
            }
            other => panic!("expected Command::Sense, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reprocess_image_no_raw_media_returns_error() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "chronicle/20260731/field/090000_300/audio.flac",
            b"raw audio",
        );
        let response = test_router(root.path(), Arc::new(FailingSpawner))
            .oneshot(
                Request::post("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
                    .body(Body::from(r#"{"modality":"image"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["reason_code"], "raw_media_not_available");
        assert!(body.get("modality").is_none());
    }

    #[tokio::test]
    async fn concurrent_reprocess_image_creates_one_marker_and_spawns_once() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "chronicle/20260731/field/090000_300/photo.jpg",
            b"raw photo",
        );
        let launches = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());
        let app = test_router(
            root.path(),
            Arc::new(DelayedSpawner {
                launches: Arc::clone(&launches),
                release: Arc::clone(&release),
            }),
        );
        let path = "/app/transcripts/api/segment/20260731/field/090000_300/reprocess";
        let first = app.clone().oneshot(
            Request::post(path)
                .body(Body::from(r#"{"modality":"image"}"#))
                .unwrap(),
        );
        let second = app.clone().oneshot(
            Request::post(path)
                .body(Body::from(r#"{"modality":"image"}"#))
                .unwrap(),
        );
        let (first, second) = tokio::join!(first, second);
        let first: Value = serde_json::from_slice(
            &to_bytes(first.unwrap().into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let second: Value = serde_json::from_slice(
            &to_bytes(second.unwrap().into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(launches.load(Ordering::SeqCst), 1);
        let statuses = [
            first["repair_status"].as_str().unwrap(),
            second["repair_status"].as_str().unwrap(),
        ];
        assert!(statuses.contains(&"accepted"));
        assert!(statuses.contains(&"running"));
        assert_eq!(
            first["marker"]["started_at"],
            second["marker"]["started_at"]
        );
        for response in [&first, &second] {
            assert_eq!(
                response
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                std::collections::BTreeSet::from([
                    "data_state".into(),
                    "marker".into(),
                    "repair_status".into(),
                ])
            );
            assert_eq!(
                response["marker"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>(),
                std::collections::BTreeSet::from(["started_at".into()])
            );
        }
        release.notify_one();
    }

    #[tokio::test]
    async fn reprocess_audio_endpoint_flow_accepts_opus() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "chronicle/20260731/field/090000_300/audio.opus",
            b"raw opus",
        );
        let spawner = Arc::new(ExpectingSpawner {
            expected: SenseRequest {
                day: "20260731".into(),
                stream: "field".into(),
                key: "090000_300".into(),
                modality: "audio".into(),
            },
            calls: AtomicUsize::new(0),
        });
        let response = test_router(root.path(), Arc::clone(&spawner) as Arc<dyn SenseSpawner>)
            .oneshot(
                Request::post("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
                    .body(Body::from(r#"{"modality":"audio"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 1);
    }

    struct BlockingChannelChild {
        receiver: std::sync::Mutex<std::sync::mpsc::Receiver<ChildExit>>,
    }

    impl SenseChild for BlockingChannelChild {
        fn wait(self: Box<Self>) -> Result<ChildExit, String> {
            self.receiver
                .lock()
                .unwrap()
                .recv()
                .map_err(|error| error.to_string())
        }
    }

    struct RecordingChannelSpawner {
        recorded: std::sync::Mutex<Option<SenseRequest>>,
        sender: std::sync::Mutex<std::sync::mpsc::Sender<ChildExit>>,
    }

    impl SenseSpawner for RecordingChannelSpawner {
        fn spawn(
            &self,
            request: &SenseRequest,
            #[cfg(windows)] _context: Option<&crate::SenseLaunchContext>,
        ) -> Result<Box<dyn SenseChild>, String> {
            *self.recorded.lock().unwrap() = Some(request.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            *self.sender.lock().unwrap() = tx;
            Ok(Box::new(BlockingChannelChild {
                receiver: std::sync::Mutex::new(rx),
            }))
        }
    }

    #[tokio::test]
    async fn reprocess_direct_segment_endpoint_and_settlement() {
        let root = TempDir::new().unwrap();
        let day = "20260731";
        let key = "090000_300";
        let named_stream = "field";

        // Direct segment
        let direct_dir = root.path().join(format!("chronicle/{day}/{key}"));
        fs::create_dir_all(&direct_dir).unwrap();
        fs::write(direct_dir.join("audio.flac"), b"raw direct flac").unwrap();

        // Named sibling segment
        let named_dir = root
            .path()
            .join(format!("chronicle/{day}/{named_stream}/{key}"));
        fs::create_dir_all(&named_dir).unwrap();
        fs::write(named_dir.join("audio.flac"), b"raw named flac").unwrap();

        let named_flac_before = fs::read(named_dir.join("audio.flac")).unwrap();

        let (dummy_tx, _) = std::sync::mpsc::channel();
        let spawner = Arc::new(RecordingChannelSpawner {
            recorded: std::sync::Mutex::new(None),
            sender: std::sync::Mutex::new(dummy_tx),
        });

        let app = test_router(root.path(), Arc::clone(&spawner) as Arc<dyn SenseSpawner>);
        let response = app
            .oneshot(
                Request::post(format!(
                    "/app/transcripts/api/segment/{day}/_default/{key}/reprocess"
                ))
                .body(Body::from(r#"{"modality":"audio"}"#))
                .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["repair_status"], "accepted");
        assert!(body["data_state"].is_object());
        assert!(body["marker"]["started_at"].is_string());

        // Assert spawner recorded request with stream _default
        let recorded = spawner.recorded.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.day, day);
        assert_eq!(recorded.stream, "_default");
        assert_eq!(recorded.key, key);
        assert_eq!(recorded.modality, "audio");

        // Assert sense_reprocess_args contains --stream _default and unchanged key
        let args = sense_reprocess_args(&recorded);
        let stream_idx = args.iter().position(|a| a == "--stream").unwrap();
        assert_eq!(args[stream_idx + 1], "_default");
        let seg_idx = args.iter().position(|a| a == "--segment").unwrap();
        assert_eq!(args[seg_idx + 1], key);

        // Marker in physical direct directory, no chronicle/<day>/_default/
        let marker_path = direct_dir.join(".analyzing_audio");
        assert!(marker_path.is_file());
        let marker_data = marker_payload(&marker_path);
        let request_id = marker_data["request_id"].as_str().unwrap().to_owned();
        assert!(
            !root
                .path()
                .join(format!("chronicle/{day}/_default"))
                .exists()
        );

        // Named sibling untouched
        assert_eq!(
            fs::read(named_dir.join("audio.flac")).unwrap(),
            named_flac_before
        );

        // Settle success first while the child is still parked
        let now = Utc.with_ymd_and_hms(2026, 8, 2, 0, 0, 0).unwrap();
        fs::write(direct_dir.join("audio.jsonl"), b"{\"start\": \"1\"}\n").unwrap();

        watch_reprocess_completion(
            &direct_dir,
            "audio",
            &request_id,
            ChildExit {
                code: 0,
                stderr: String::new(),
            },
            now,
        );

        assert!(!direct_dir.join(".analyzing_audio").exists());
        let current_state = data_state(&direct_dir, now, None);
        assert_eq!(
            current_state.get("audio").map(|s| s.as_str()),
            Some("analyzed")
        );
        assert_eq!(
            fs::read(named_dir.join("audio.flac")).unwrap(),
            named_flac_before
        );

        // Only then send the exit so the parked wait returns
        let _ = spawner.sender.lock().unwrap().send(ChildExit {
            code: 0,
            stderr: String::new(),
        });

        // Test settlement: failure
        let failed_marker = create_analyzing_marker(&direct_dir, "audio").unwrap();
        watch_reprocess_completion(
            &direct_dir,
            "audio",
            &failed_marker.request_id,
            ChildExit {
                code: 1,
                stderr: "crash".into(),
            },
            now,
        );
        assert!(!direct_dir.join(".analyzing_audio").exists());
        assert!(direct_dir.join(".analyze_failed_audio").is_file());
        assert_eq!(
            fs::read(named_dir.join("audio.flac")).unwrap(),
            named_flac_before
        );
    }
}
