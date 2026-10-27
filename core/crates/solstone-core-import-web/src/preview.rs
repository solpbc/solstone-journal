// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Preview and start share a source snapshot and normalized options. Planning
//! runs in the native importer process, away from the web executor.

use crate::{
    AppState,
    http::json as json_response,
    lifecycle::{command, contained_import_file, detect_inprocess_source, source_hint},
    mobile_import_contract::ImportErrorKind,
};
use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_import::cli_render::CliRun;
use solstone_core_import::{ImportMetadata, read_import_metadata, write_import_metadata};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, mpsc},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewError {
    AlreadyRunning,
    NothingToImport,
    SourceChanged(String),
    InvalidRequest(String),
    NotFound(String),
    PlanFailed(String),
}

impl PreviewError {
    pub const fn kind(&self) -> ImportErrorKind {
        match self {
            Self::AlreadyRunning => ImportErrorKind::AlreadyRunning,
            Self::NothingToImport => ImportErrorKind::NothingToImport,
            Self::SourceChanged(_) => ImportErrorKind::SourceChanged,
            Self::InvalidRequest(_) => ImportErrorKind::InvalidRequest,
            Self::NotFound(_) => ImportErrorKind::NotFound,
            Self::PlanFailed(_) => ImportErrorKind::PlanFailed,
        }
    }

    pub const fn owner_sentence(&self) -> &'static str {
        match self {
            Self::AlreadyRunning => "this import has already started.",
            Self::NothingToImport => "there is nothing new in this source to import.",
            Self::SourceChanged(_) => "something changed, so choose the source again.",
            Self::InvalidRequest(_) => "this import's source or date isn't valid.",
            Self::NotFound(_) => {
                "this import or its source wasn't found, so choose the source again."
            }
            Self::PlanFailed(_) => "the import could not be prepared.",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::AlreadyRunning => "this import is already running",
            Self::NothingToImport => "there are no recordings to import from this source",
            Self::SourceChanged(s)
            | Self::InvalidRequest(s)
            | Self::NotFound(s)
            | Self::PlanFailed(s) => s.as_str(),
        }
    }
}

pub(crate) fn preview_error_body(reason_code: &str, error: &PreviewError) -> Value {
    json!({
        "error": error.owner_sentence(),
        "reason_code": reason_code,
        "detail": error.detail(),
        "kind": error.kind().as_wire(),
    })
}

pub(crate) fn preview_error_response(
    status: StatusCode,
    reason_code: &str,
    error: &PreviewError,
) -> Response {
    json_response(status, preview_error_body(reason_code, error))
}

pub(crate) type PreviewPlanStub = dyn Fn(
        &Path,
        &Value,
        &ImportMetadata,
    )
        -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, PreviewError>> + Send>>
    + Send
    + Sync;

#[derive(Clone)]
pub(crate) enum PreviewPlan<'a> {
    Program(&'a Path),
    #[cfg_attr(not(test), allow(dead_code))]
    Stub(std::sync::Arc<PreviewPlanStub>),
    Embedded(Arc<crate::ImportRouteSlot>),
}

struct Input {
    id: String,
    source: PathBuf,
    metadata: ImportMetadata,
    options: Value,
}

fn input(root: &Path, data: &Value) -> Result<Input, PreviewError> {
    let path = data
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| PreviewError::InvalidRequest("choose a source first".to_owned()))?;
    let timestamp = data
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or_else(|| PreviewError::InvalidRequest("choose an import date first".to_owned()))?;
    solstone_core_import::validate_timestamp(timestamp)
        .map_err(|error| PreviewError::InvalidRequest(error.to_string()))?;
    let id = crate::lifecycle::resolve_import_id(root, data)
        .ok_or_else(|| PreviewError::NotFound("missing staged import".to_owned()))?;
    solstone_core_import::validate_timestamp(&id)
        .map_err(|error| PreviewError::InvalidRequest(error.to_string()))?;
    contained_import_file(root, &id, "import.json")
        .map_err(|error| PreviewError::NotFound(error.to_string()))?;
    let mut metadata = read_import_metadata(root, &id)
        .map_err(|error| PreviewError::NotFound(error.to_string()))?;
    if metadata.get("file_path").and_then(Value::as_str) != Some(path) {
        return Err(PreviewError::SourceChanged(
            "the staged source changed; choose it again".to_owned(),
        ));
    }
    if matches!(
        solstone_core_import::project_import_result(root, &id).status,
        solstone_core_import::ProjectionStatus::Running
    ) {
        return Err(PreviewError::AlreadyRunning);
    }
    let mut options = json!({
        "timestamp": timestamp,
        "timestamp_override": crate::lifecycle::owner_timestamp_choice(data, &metadata).is_some(),
        "resolved_timestamp": data.get("resolved_timestamp").and_then(Value::as_str),
        "force": data.get("force").and_then(Value::as_bool).unwrap_or(false),
        "source": source_hint(&metadata),
        "setting": metadata.get("setting"),
        "zone": solstone_core_journal_config::owner_zone(root).name(),
    });
    crate::audio_archive::configure_options(root, data, &mut metadata, &mut options)?;
    Ok(Input {
        id,
        source: PathBuf::from(path),
        metadata,
        options,
    })
}

/// The native importer beside this executable, which plans every preview.
pub(crate) fn planner() -> Result<PathBuf, PreviewError> {
    let executable =
        std::env::current_exe().map_err(|error| PreviewError::PlanFailed(error.to_string()))?;
    Ok(executable
        .parent()
        .ok_or_else(|| PreviewError::PlanFailed("cannot locate the importer".to_owned()))?
        .join(if cfg!(windows) {
            "solstone-core.exe"
        } else {
            "solstone-core"
        }))
}

fn planned_command(
    path: &Path,
    options: &Value,
    metadata: &ImportMetadata,
) -> Result<Vec<String>, PreviewError> {
    let mut metadata = metadata.clone();
    metadata.remove("preview");
    if crate::audio_archive::selected(&metadata) {
        metadata.insert("archive_options".into(), options.clone());
    }
    if options["timestamp_override"].as_bool().unwrap_or(false) {
        metadata.insert("owner_timestamp".to_owned(), options["timestamp"].clone());
    }
    let path = path
        .to_str()
        .ok_or_else(|| PreviewError::InvalidRequest("the source path is not UTF-8".to_owned()))?;
    // Start imports an image or document by the kind it was saved as, with or without a source
    // hint. The planner only recognizes those kinds when told, so it is told the same kind.
    if source_hint(&metadata).is_none()
        && let Some(source) = detect_inprocess_source(&metadata, path)
    {
        metadata.insert("source_hint".to_owned(), json!(source.name()));
    }
    Ok(command(
        path,
        options["timestamp"]
            .as_str()
            .ok_or_else(|| PreviewError::InvalidRequest("missing timestamp".to_owned()))?,
        &metadata,
        options["force"].as_bool().unwrap_or(false),
    ))
}

fn preview_command(
    path: &Path,
    options: &Value,
    metadata: &ImportMetadata,
) -> Result<Vec<String>, PreviewError> {
    let mut args = planned_command(path, options, metadata)?;
    if crate::audio_archive::selected(metadata) {
        // Library preview maps an explicit resume to read-only archive status.
        if metadata
            .get("_archive_run_id")
            .and_then(Value::as_str)
            .is_none()
        {
            args.push("--dry-run".into());
        }
        return Ok(args);
    }
    let zone = options["zone"]
        .as_str()
        .ok_or_else(|| PreviewError::PlanFailed("missing time zone".to_owned()))?;
    args.extend([
        "--dry-run".to_owned(),
        "--json".to_owned(),
        "--preview-zone".to_owned(),
        zone.to_owned(),
    ]);
    Ok(args)
}

fn size_from_preview(run: CliRun) -> Result<Value, PreviewError> {
    if run.exit_code != 0 {
        return Err(PreviewError::PlanFailed(run.stderr));
    }
    let size: Value = serde_json::from_str(&run.stdout).map_err(|_| {
        PreviewError::PlanFailed("the importer did not return a size preview".to_owned())
    })?;
    require_size_preview(size)
}

fn snapshot_failure(error: String) -> PreviewError {
    match error.as_str() {
        "choose a source outside its import staging folder" | "source has no filename" => {
            PreviewError::InvalidRequest(error)
        }
        "the source changed while preparing its preview; try again" => {
            PreviewError::SourceChanged(error)
        }
        _ => PreviewError::PlanFailed(error),
    }
}

fn checked_input(root: &Path, data: &Value) -> Result<Input, PreviewError> {
    let input = input(root, data)?;
    let hash = solstone_core_import::hash_source(&input.source)
        .map_err(|error| PreviewError::NotFound(error.to_string()))?;
    if input.metadata.get("source_hash").and_then(Value::as_str) != Some(hash.as_str()) {
        return Err(PreviewError::SourceChanged(
            "the staged source changed; choose it again before previewing".to_owned(),
        ));
    }
    Ok(input)
}

fn require_size_preview(size: Value) -> Result<Value, PreviewError> {
    if let Some(size) = crate::audio_archive::size(&size) {
        return Ok(size);
    }
    if size.get("schema").and_then(Value::as_str) != Some("solstone.import.size.v1") {
        return Err(PreviewError::PlanFailed(
            "this source does not provide a recording preview".to_owned(),
        ));
    }
    Ok(size)
}

type Captured = Arc<dyn crate::CapturedImport>;

async fn plan(
    program: PreviewPlan<'_>,
    path: &Path,
    options: &Value,
    metadata: &ImportMetadata,
) -> Result<(Value, Option<Captured>), PreviewError> {
    match program {
        PreviewPlan::Program(prog) => Ok((
            plan_using(prog, path, options, metadata, Duration::from_secs(600)).await?,
            None,
        )),
        PreviewPlan::Stub(stub) => Ok((stub(path, options, metadata).await?, None)),
        PreviewPlan::Embedded(slot) => {
            let (size, captured) = plan_embedded(slot, path, options, metadata).await?;
            Ok((size, Some(captured)))
        }
    }
}

async fn plan_embedded(
    slot: Arc<crate::ImportRouteSlot>,
    path: &Path,
    options: &Value,
    metadata: &ImportMetadata,
) -> Result<(Value, Captured), PreviewError> {
    let args = preview_command(path, options, metadata)?;
    let source_time =
        crate::embedded_route::source_time(options["timestamp"].as_str().unwrap_or(""));
    let captured = slot
        .capture(args, source_time)
        .map_err(PreviewError::PlanFailed)?;
    let preview_slot = Arc::clone(&slot);
    tokio::task::spawn_blocking(move || plan_embedded_on_worker(preview_slot, captured))
        .await
        .map_err(|error| PreviewError::PlanFailed(error.to_string()))?
}

fn plan_embedded_on_worker(
    slot: Arc<crate::ImportRouteSlot>,
    captured: Captured,
) -> Result<(Value, Captured), PreviewError> {
    let (sender, receiver) = mpsc::channel();
    slot.retain(move |run| {
        if !run {
            drop(captured);
            let _ = sender.send(Err(PreviewError::PlanFailed(
                "importer unavailable".to_owned(),
            )));
            return;
        }
        let previewed = match captured.preview() {
            Ok(run) => run,
            Err(error) => {
                drop(captured);
                let _ = sender.send(Err(PreviewError::PlanFailed(error)));
                return;
            }
        };
        match size_from_preview(previewed) {
            Ok(size) => {
                let _ = sender.send(Ok((size, captured)));
            }
            Err(error) => {
                drop(captured);
                let _ = sender.send(Err(error));
            }
        }
    })
    .map_err(PreviewError::PlanFailed)?;
    receiver
        .recv()
        .map_err(|_| PreviewError::PlanFailed("importer unavailable".to_owned()))?
}

/// Prepare, snapshot, preview, and parse the size on the retained worker.
///
/// The async task receives the size and a release handle. It does not receive
/// the snapshot. Dropping the handle signals that worker to delete a fresh tree
/// before it drops the captured import. Keeping the handle waits until that
/// worker has persisted the snapshot and dropped the captured import.
async fn embedded_snapshot(
    slot: Arc<crate::ImportRouteSlot>,
    root: &Path,
    input: &Input,
) -> Result<EmbeddedReady, PreviewError> {
    let job = SnapshotJob {
        slot,
        journal: root.to_path_buf(),
        import_id: input.id.clone(),
        source: input.source.clone(),
        options: input.options.clone(),
        metadata: input.metadata.clone(),
        source_time: crate::embedded_route::source_time(
            input.options["timestamp"].as_str().unwrap_or(""),
        ),
    };
    tokio::task::spawn_blocking(move || embedded_snapshot_on_worker(job))
        .await
        .map_err(|error| PreviewError::PlanFailed(error.to_string()))?
}

struct SnapshotJob {
    slot: Arc<crate::ImportRouteSlot>,
    journal: PathBuf,
    import_id: String,
    source: PathBuf,
    options: Value,
    metadata: ImportMetadata,
    source_time: u64,
}

enum SnapshotFate {
    Keep,
    Discard,
}

struct SnapshotRelease {
    command: Option<mpsc::Sender<SnapshotFate>>,
    done: Option<mpsc::Receiver<()>>,
}

impl SnapshotRelease {
    fn keep(mut self) {
        if let Some(command) = self.command.take() {
            let _ = command.send(SnapshotFate::Keep);
        }
        if let Some(done) = self.done.take() {
            let _ = done.recv();
        }
    }
}

impl Drop for SnapshotRelease {
    fn drop(&mut self) {
        if let Some(command) = self.command.take() {
            let _ = command.send(SnapshotFate::Discard);
        }
    }
}

struct EmbeddedReady {
    size: Value,
    relative: PathBuf,
    digest: String,
    release: SnapshotRelease,
}

fn embedded_snapshot_on_worker(job: SnapshotJob) -> Result<EmbeddedReady, PreviewError> {
    let (sender, receiver) = mpsc::channel();
    let retain_slot = Arc::clone(&job.slot);
    retain_slot
        .retain(move |run| prepare_retained_snapshot(job, run, sender))
        .map_err(PreviewError::PlanFailed)?;
    receiver
        .recv()
        .map_err(|_| PreviewError::PlanFailed("importer unavailable".to_owned()))?
}

fn prepare_retained_snapshot(
    job: SnapshotJob,
    run: bool,
    sender: mpsc::Sender<Result<EmbeddedReady, PreviewError>>,
) {
    let SnapshotJob {
        slot,
        journal,
        import_id,
        source,
        options,
        metadata,
        source_time,
    } = job;
    if !run {
        let _ = sender.send(Err(PreviewError::PlanFailed(
            "importer unavailable".to_owned(),
        )));
        return;
    }
    let admission = match slot.prepare(source_time) {
        Ok(admission) => admission,
        Err(error) => {
            let _ = sender.send(Err(PreviewError::PlanFailed(error)));
            return;
        }
    };
    if let Err(error) = admission.checkpoint() {
        drop(admission);
        let _ = sender.send(Err(PreviewError::PlanFailed(error)));
        return;
    }
    let snapshot = match solstone_core_import::snapshot::create(&journal, &import_id, &source) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            drop(admission);
            let _ = sender.send(Err(snapshot_failure(error)));
            return;
        }
    };
    let relative = match snapshot.relative.to_str() {
        Some(relative) => relative.to_owned(),
        None => {
            discard_retained(
                &slot,
                snapshot,
                None,
                admission,
                &sender,
                PreviewError::PlanFailed("invalid snapshot path".to_owned()),
            );
            return;
        }
    };
    let contained = match contained_import_file(&journal, &import_id, &relative) {
        Ok(path) => path,
        Err(error) => {
            discard_retained(
                &slot,
                snapshot,
                None,
                admission,
                &sender,
                PreviewError::NotFound(error.to_string()),
            );
            return;
        }
    };
    let args = match preview_command(&contained, &options, &metadata) {
        Ok(args) => args,
        Err(error) => {
            discard_retained(&slot, snapshot, None, admission, &sender, error);
            return;
        }
    };
    let captured = match slot.bind(Arc::clone(&admission), args) {
        Ok(captured) => captured,
        Err(error) => {
            discard_retained(
                &slot,
                snapshot,
                None,
                admission,
                &sender,
                PreviewError::PlanFailed(error),
            );
            return;
        }
    };
    let previewed = match captured.preview() {
        Ok(run) => run,
        Err(error) => {
            discard_retained(
                &slot,
                snapshot,
                Some(captured),
                admission,
                &sender,
                PreviewError::PlanFailed(error),
            );
            return;
        }
    };
    let size = match size_from_preview(previewed) {
        Ok(size) => size,
        Err(error) => {
            discard_retained(&slot, snapshot, Some(captured), admission, &sender, error);
            return;
        }
    };
    let (command_sender, command_receiver) = mpsc::channel();
    let (done_sender, done_receiver) = mpsc::channel();
    let ready = EmbeddedReady {
        size,
        relative: snapshot.relative.clone(),
        digest: snapshot.digest.clone(),
        release: SnapshotRelease {
            command: Some(command_sender),
            done: Some(done_receiver),
        },
    };
    if sender.send(Ok(ready)).is_err() {
        discard_retained_parts(&slot, snapshot, Some(captured), admission);
        return;
    }
    let fate = command_receiver.recv().unwrap_or(SnapshotFate::Discard);
    match fate {
        SnapshotFate::Keep => snapshot.persist(),
        SnapshotFate::Discard => {
            slot.wait_before_snapshot_delete();
            snapshot.delete_unkept();
        }
    }
    drop(captured);
    drop(admission);
    let _ = done_sender.send(());
}

fn discard_retained(
    slot: &crate::ImportRouteSlot,
    snapshot: solstone_core_import::snapshot::Snapshot,
    captured: Option<Captured>,
    admission: Arc<dyn crate::ImportAdmission>,
    sender: &mpsc::Sender<Result<EmbeddedReady, PreviewError>>,
    error: PreviewError,
) {
    discard_retained_parts(slot, snapshot, captured, admission);
    let _ = sender.send(Err(error));
}

fn discard_retained_parts(
    slot: &crate::ImportRouteSlot,
    snapshot: solstone_core_import::snapshot::Snapshot,
    captured: Option<Captured>,
    admission: Arc<dyn crate::ImportAdmission>,
) {
    slot.wait_before_snapshot_delete();
    snapshot.delete_unkept();
    drop(captured);
    drop(admission);
}

async fn plan_using(
    program: &Path,
    path: &Path,
    options: &Value,
    metadata: &ImportMetadata,
    bound: Duration,
) -> Result<Value, PreviewError> {
    let args = preview_command(path, options, metadata)?;
    let mut child = tokio::process::Command::new(program);
    child
        .args(&args[2..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(bound, child.output())
        .await
        .map_err(|_| {
            PreviewError::PlanFailed(
                "the import preview timed out; try a smaller source".to_owned(),
            )
        })?
        .map_err(|error| {
            PreviewError::PlanFailed(format!("could not prepare the import preview: {error}"))
        })?;
    if !output.status.success() {
        return Err(PreviewError::PlanFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let size: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        PreviewError::PlanFailed("the importer did not return a size preview".to_owned())
    })?;
    require_size_preview(size)
}

pub(crate) async fn preview(State(state): State<AppState>, Json(data): Json<Value>) -> Response {
    let resolved_id = crate::lifecycle::resolve_import_id(&state.root, &data);
    let _guard = if let Some(id) = resolved_id.as_deref() {
        Some(crate::start_lock::acquire(&state.root, id).await)
    } else {
        None
    };
    let previewed = if let Some(slot) = state.route.clone() {
        create_preview(state.root, data, PreviewPlan::Embedded(slot)).await
    } else {
        match planner() {
            Ok(program) => create_preview(state.root, data, PreviewPlan::Program(&program)).await,
            Err(error) => Err(error),
        }
    };
    match previewed {
        Ok(preview) => json_response(StatusCode::OK, preview),
        Err(error) => {
            preview_error_response(StatusCode::BAD_REQUEST, "import_preview_failed", &error)
        }
    }
}

/// A preview record known in memory and not yet written.
pub(crate) struct PendingPreview {
    pub(crate) record: Value,
    id: String,
    snapshot: Option<solstone_core_import::snapshot::Snapshot>,
    release: Option<SnapshotRelease>,
    hold: Option<Captured>,
    setting: Option<Value>,
    source_hint: Option<String>,
}

impl Drop for PendingPreview {
    fn drop(&mut self) {
        drop(self.snapshot.take());
        drop(self.release.take());
        drop(self.hold.take());
    }
}

struct AssembledPreview {
    response: Value,
    pending: PendingPreview,
}

/// Write a preview record. The planning lease is held until this returns.
pub(crate) fn commit_pending_preview(
    root: &Path,
    mut pending: PendingPreview,
) -> Result<(), PreviewError> {
    let committed = (|| {
        let mut metadata = read_import_metadata(root, &pending.id)
            .map_err(|error| PreviewError::NotFound(error.to_string()))?;
        // A settings edit during planning must not bless the earlier options.
        if metadata.get("setting") != pending.setting.as_ref()
            || source_hint(&metadata) != pending.source_hint
        {
            return Err(PreviewError::SourceChanged(
                "the import settings changed; preview again".to_owned(),
            ));
        }
        metadata.insert("preview".to_owned(), pending.record.clone());
        write_import_metadata(root, &pending.id, &metadata)
            .map_err(|error| PreviewError::PlanFailed(error.to_string()))?;
        if let Some(snapshot) = pending.snapshot.take() {
            snapshot.persist();
        }
        if let Some(release) = pending.release.take() {
            release.keep();
        }
        Ok(())
    })();
    drop(pending.snapshot.take());
    drop(pending.release.take());
    drop(pending.hold.take());
    committed
}

async fn assemble_preview(
    root: &Path,
    data: Value,
    program: PreviewPlan<'_>,
) -> Result<AssembledPreview, PreviewError> {
    let snapshot_root = root.to_path_buf();
    let input = tokio::task::spawn_blocking(move || checked_input(&snapshot_root, &data))
        .await
        .map_err(|error| PreviewError::PlanFailed(error.to_string()))??;
    // Desktop still snapshots, then plans. Embedded prepares on the retained worker.
    let prepared = match program {
        PreviewPlan::Embedded(slot) => {
            let ready = embedded_snapshot(slot, root, &input).await?;
            AssembledParts {
                size: ready.size,
                relative: ready.relative,
                digest: ready.digest,
                hold: None,
                snapshot: None,
                release: Some(ready.release),
            }
        }
        other => {
            let snapshot_root = root.to_path_buf();
            let source = input.source.clone();
            let id = input.id.clone();
            let snapshot = tokio::task::spawn_blocking(move || {
                solstone_core_import::snapshot::create(&snapshot_root, &id, &source)
                    .map_err(snapshot_failure)
            })
            .await
            .map_err(|error| PreviewError::PlanFailed(error.to_string()))??;
            let relative_str = snapshot
                .relative
                .to_str()
                .ok_or_else(|| PreviewError::PlanFailed("invalid snapshot path".to_owned()))?;
            let source = contained_import_file(root, &input.id, relative_str)
                .map_err(|error| PreviewError::NotFound(error.to_string()))?;
            let (size, hold) = plan(other, &source, &input.options, &input.metadata).await?;
            AssembledParts {
                size,
                relative: snapshot.relative.clone(),
                digest: snapshot.digest.clone(),
                hold,
                snapshot: Some(snapshot),
                release: None,
            }
        }
    };
    let record = json!({"source": input.source, "relative": prepared.relative, "digest": prepared.digest, "options": input.options, "size": prepared.size});
    let token = format!("{:x}", Sha256::digest(record.to_string().as_bytes()));
    let mut record = record;
    record["token"] = json!(token);
    let notice = if input.options["force"].as_bool() == Some(true) {
        let previous = input
            .metadata
            .get("source_timestamp")
            .and_then(Value::as_str)
            .unwrap_or(&input.id);
        let planned = input.options["timestamp"]
            .as_str()
            .ok_or_else(|| PreviewError::InvalidRequest("missing timestamp".to_owned()))?;
        let fresh = input.options["resolved_timestamp"]
            .as_str()
            .unwrap_or(planned);
        match (
            solstone_core_import::validate_timestamp(fresh),
            solstone_core_import::validate_timestamp(previous),
            solstone_core_import::validate_timestamp(planned),
        ) {
            (Ok(fresh), Ok(previous), Ok(planned)) => {
                solstone_core_import::force_day_notice(&fresh, &previous, &planned)
            }
            _ => None,
        }
    } else {
        None
    };
    let response = json!({"preview_token": token, "import_id": input.id, "size": prepared.size, "force_notice": notice});
    Ok(AssembledPreview {
        response,
        pending: PendingPreview {
            id: input.id,
            record,
            snapshot: prepared.snapshot,
            release: prepared.release,
            hold: prepared.hold,
            setting: input.metadata.get("setting").cloned(),
            source_hint: source_hint(&input.metadata),
        },
    })
}

struct AssembledParts {
    size: Value,
    relative: PathBuf,
    digest: String,
    hold: Option<Captured>,
    snapshot: Option<solstone_core_import::snapshot::Snapshot>,
    release: Option<SnapshotRelease>,
}

async fn create_preview(
    root: PathBuf,
    data: Value,
    program: PreviewPlan<'_>,
) -> Result<Value, PreviewError> {
    let assembled = assemble_preview(&root, data, program).await?;
    let response = assembled.response.clone();
    tokio::task::spawn_blocking(move || {
        commit_pending_preview(&root, assembled.pending)?;
        Ok::<_, PreviewError>(response)
    })
    .await
    .map_err(|error| PreviewError::PlanFailed(error.to_string()))?
}

fn accept_record(
    root: &Path,
    data: &Value,
    input: &Input,
    record: &Value,
) -> Result<PathBuf, PreviewError> {
    let token = data
        .get("preview_token")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            PreviewError::InvalidRequest("preview the import before starting".to_owned())
        })?;
    if record.get("token").and_then(Value::as_str) != Some(token)
        || record["options"] != input.options
        || record["source"] != json!(input.source)
    {
        return Err(PreviewError::SourceChanged(
            "the source or settings changed; preview again before starting".to_owned(),
        ));
    }
    let digest = record["digest"]
        .as_str()
        .ok_or_else(|| PreviewError::PlanFailed("the saved preview is unreadable".to_owned()))?;
    solstone_core_import::snapshot::verify(&input.source, digest)
        .map_err(PreviewError::SourceChanged)?;
    let relative = record["relative"]
        .as_str()
        .ok_or_else(|| PreviewError::PlanFailed("the saved preview is unreadable".to_owned()))?;
    let snapshot = contained_import_file(root, &input.id, relative)
        .map_err(|error| PreviewError::NotFound(error.to_string()))?;
    solstone_core_import::snapshot::verify(&snapshot, digest)
        .map_err(PreviewError::SourceChanged)?;
    Ok(snapshot)
}

fn reject_empty(size: &Value) -> Result<(), PreviewError> {
    if size["schema"] == crate::audio_archive::SIZE_SCHEMA && size["tiles"].as_u64() == Some(0) {
        return Err(PreviewError::NothingToImport);
    }
    if size["recordings"].as_u64() == Some(0) && size["other_units"].is_null() {
        return Err(PreviewError::NothingToImport);
    }
    Ok(())
}

fn load_saved_preview(root: &Path, data: &Value) -> Result<(Input, Value, PathBuf), PreviewError> {
    let input = input(root, data)?;
    let record = input.metadata.get("preview").cloned().ok_or_else(|| {
        PreviewError::InvalidRequest("preview the import before starting".to_owned())
    })?;
    let snapshot = accept_record(root, data, &input, &record)?;
    Ok((input, record, snapshot))
}

async fn confirm_existing_preview(
    root: &Path,
    mut data: Value,
    program: PreviewPlan<'_>,
    previewed_here: bool,
) -> Result<Value, PreviewError> {
    let checked_root = root.to_path_buf();
    let checked_data = data.clone();
    let (input, record, snapshot) =
        tokio::task::spawn_blocking(move || load_saved_preview(&checked_root, &checked_data))
            .await
            .map_err(|error| PreviewError::PlanFailed(error.to_string()))??;
    let (current, hold) = if previewed_here {
        (record["size"].clone(), None)
    } else {
        plan(program, &snapshot, &input.options, &input.metadata).await?
    };
    if current != record["size"] {
        drop(hold);
        return Err(PreviewError::SourceChanged(
            "the import size changed as your journal changed; preview it again".to_owned(),
        ));
    }
    drop(hold);
    reject_empty(&current)?;
    data["import_id"] = json!(input.id);
    data["_preview_source"] = record["relative"].clone();
    Ok(data)
}

pub(crate) async fn validate_start(
    root: &Path,
    mut data: Value,
    program: PreviewPlan<'_>,
) -> Result<Value, PreviewError> {
    // No client can supply the private, already-validated start fields.
    if let Some(object) = data.as_object_mut() {
        object.remove("_preview_source");
    }
    // A start without a token is the owner's start instruction from a client with no size
    // step, such as a share from the phone. It is previewed here, under the same snapshot,
    // settings and size binding as a preview the owner saw.
    let previewed_here = data.get("preview_token").and_then(Value::as_str).is_none();
    if previewed_here {
        let preview = create_preview(root.to_path_buf(), data.clone(), program.clone()).await?;
        data["preview_token"] = preview["preview_token"].clone();
    }
    confirm_existing_preview(root, data, program, previewed_here).await
}

/// Prepare an embedded start. A start that still needs a preview keeps that
/// record in memory so the caller can capture the final command before writing it.
pub(crate) async fn prepare_embedded_start(
    root: &Path,
    mut data: Value,
    slot: Arc<crate::ImportRouteSlot>,
) -> Result<(Value, Option<PendingPreview>), PreviewError> {
    if let Some(object) = data.as_object_mut() {
        object.remove("_preview_source");
    }
    let previewed_here = data.get("preview_token").and_then(Value::as_str).is_none();
    if !previewed_here {
        let data = confirm_existing_preview(root, data, PreviewPlan::Embedded(slot), false).await?;
        return Ok((data, None));
    }
    let validation = Arc::clone(&slot);
    let assembled = assemble_preview(root, data.clone(), PreviewPlan::Embedded(slot)).await?;
    data["preview_token"] = assembled.response["preview_token"].clone();
    let checked_root = root.to_path_buf();
    let checked_data = data.clone();
    // The preview moves into the blocking reader. Cancelling this await detaches
    // that reader and must not signal cleanup before the reader returns.
    let pending = assembled.pending;
    let (input, pending) = tokio::task::spawn_blocking(move || {
        validation.wait_before_validation();
        let input = match input(&checked_root, &checked_data) {
            Ok(input) => input,
            Err(error) => {
                drop(pending);
                return Err(error);
            }
        };
        if let Err(error) = accept_record(&checked_root, &checked_data, &input, &pending.record) {
            drop(pending);
            return Err(error);
        }
        Ok((input, pending))
    })
    .await
    .map_err(|error| PreviewError::PlanFailed(error.to_string()))??;
    reject_empty(&pending.record["size"])?;
    data["import_id"] = json!(input.id);
    data["_preview_source"] = pending.record["relative"].clone();
    Ok((data, Some(pending)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    const ID: &str = "20261011_120000";
    fn fixture() -> (tempfile::TempDir, Value) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("imports").join(ID);
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("notes.txt");
        fs::write(&source, "a note").unwrap();
        write_import_metadata(
            root.path(),
            ID,
            json!({"file_path":source, "setting":"office", "source_hash":solstone_core_import::hash_source(&source).unwrap().as_str()})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        (root, json!({"path":source, "timestamp":ID, "import_id":ID}))
    }
    #[tokio::test]
    async fn settings_for_a_local_pointer_use_its_import_id_and_check_the_source() {
        let (root, _) = fixture();
        let source = root.path().join("owner-notes.txt");
        fs::write(&source, "owner material").unwrap();
        let mut metadata = read_import_metadata(root.path(), ID).unwrap();
        metadata.insert("file_path".into(), json!(source));
        write_import_metadata(root.path(), ID, &metadata).unwrap();
        let state = AppState {
            root: root.path().to_path_buf(),
            route: None,
        };
        let response = crate::lifecycle::meta(
            State(state.clone()),
            Json(json!({"path":source,"import_id":ID,"setting":"home"})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            read_import_metadata(root.path(), ID).unwrap()["setting"],
            "home"
        );
        let response = crate::lifecycle::meta(
            State(state),
            Json(
                json!({"path":root.path().join("different.txt"),"import_id":ID,"setting":"wrong"}),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            read_import_metadata(root.path(), ID).unwrap()["setting"],
            "home"
        );
        assert_eq!(fs::read_to_string(source).unwrap(), "owner material");
    }
    #[tokio::test]
    async fn a_start_that_cannot_be_previewed_is_refused_without_admission_or_outputs() {
        let (root, data) = fixture();
        let error = validate_start(
            root.path(),
            data,
            PreviewPlan::Program(&root.path().join("no-importer")),
        )
        .await
        .unwrap_err();
        assert!(
            error
                .detail()
                .contains("could not prepare the import preview"),
            "{:?}",
            error
        );
        assert!(!root.path().join("chronicle").exists());
        assert!(
            read_import_metadata(root.path(), ID)
                .unwrap()
                .get("preview")
                .is_none()
        );
        assert_eq!(
            solstone_core_import::project_import_result(root.path(), ID).status,
            solstone_core_import::ProjectionStatus::Pending
        );
    }
    #[tokio::test]
    async fn changed_source_settings_and_date_refuse_before_planning() {
        let (root, mut data) = fixture();
        let input = input(root.path(), &data).unwrap();
        let snapshot =
            solstone_core_import::snapshot::create(root.path(), ID, &input.source).unwrap();
        let mut metadata = input.metadata.clone();
        metadata.insert(
            "preview".into(),
            json!({"token":"token", "source":input.source,"relative":snapshot.relative,"digest":snapshot.digest,"options":input.options,"size":{"recordings":1,"days":1}}),
        );
        write_import_metadata(root.path(), ID, &metadata).unwrap();
        snapshot.persist();
        data["preview_token"] = json!("token");
        let mut changed = data.clone();
        changed["timestamp"] = json!("20261012_120000");
        assert!(
            validate_start(
                root.path(),
                changed,
                PreviewPlan::Program(Path::new("unused")),
            )
            .await
            .unwrap_err()
            .detail()
            .contains("settings changed")
        );
        metadata.insert("setting".into(), json!("home"));
        write_import_metadata(root.path(), ID, &metadata).unwrap();
        assert!(
            validate_start(
                root.path(),
                data.clone(),
                PreviewPlan::Program(Path::new("unused")),
            )
            .await
            .unwrap_err()
            .detail()
            .contains("settings changed")
        );
        metadata.insert("setting".into(), json!("office"));
        write_import_metadata(root.path(), ID, &metadata).unwrap();
        fs::write(input.source, "a changed note").unwrap();
        assert!(
            validate_start(root.path(), data, PreviewPlan::Program(Path::new("unused")),)
                .await
                .unwrap_err()
                .detail()
                .contains("source changed")
        );
        assert!(!root.path().join("chronicle").exists());
    }
    #[cfg(all(unix, feature = "full-tests"))]
    #[tokio::test]
    async fn a_blocked_preview_keeps_the_executor_responsive_and_times_out() {
        use std::os::unix::fs::PermissionsExt;
        let (root, data) = fixture();
        let input = input(root.path(), &data).unwrap();
        let program = root.path().join("blocked-preview");
        fs::write(&program, "#!/bin/sh\nexec sleep 60\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let preview = plan_using(
            &program,
            &input.source,
            &input.options,
            &input.metadata,
            Duration::from_millis(100),
        );
        tokio::pin!(preview);
        tokio::select! {
            result = &mut preview => panic!("the preview should still be blocked: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(10)) => {},
        }
        assert!(preview.await.unwrap_err().detail().contains("timed out"));
    }

    struct HoldWatch {
        path: PathBuf,
        expect_present: bool,
        dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl Drop for HoldWatch {
        fn drop(&mut self) {
            assert_eq!(self.path.exists(), self.expect_present);
            self.dropped
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    impl crate::CapturedImport for HoldWatch {
        fn preview(&self) -> Result<CliRun, String> {
            Err("unused".to_owned())
        }

        fn run(&self) -> Result<CliRun, String> {
            Err("unused".to_owned())
        }
    }

    fn settings_refusal(
        root: &Path,
        snapshot: solstone_core_import::snapshot::Snapshot,
        present: bool,
    ) {
        let directory = root
            .join("imports")
            .join(ID)
            .join(snapshot.relative.parent().expect("snapshot directory"));
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pending = PendingPreview {
            record: json!({}),
            id: ID.to_owned(),
            snapshot: Some(snapshot),
            release: None,
            hold: Some(Arc::new(HoldWatch {
                path: directory.clone(),
                expect_present: present,
                dropped: Arc::clone(&dropped),
            })),
            setting: Some(json!("office")),
            source_hint: None,
        };
        let mut metadata = read_import_metadata(root, ID).unwrap();
        metadata.insert("setting".into(), json!("home"));
        write_import_metadata(root, ID, &metadata).unwrap();
        match commit_pending_preview(root, pending) {
            Err(PreviewError::SourceChanged(message)) => {
                assert_eq!(message, "the import settings changed; preview again");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(directory.exists(), present);
    }

    #[test]
    fn a_settings_change_drops_a_fresh_snapshot_before_the_hold_and_keeps_a_reused_one() {
        let (root, data) = fixture();
        let input = input(root.path(), &data).unwrap();
        let fresh = solstone_core_import::snapshot::create(root.path(), ID, &input.source).unwrap();
        settings_refusal(root.path(), fresh, false);

        let mut metadata = read_import_metadata(root.path(), ID).unwrap();
        metadata.insert("setting".into(), json!("office"));
        write_import_metadata(root.path(), ID, &metadata).unwrap();
        let prepared =
            solstone_core_import::snapshot::create(root.path(), ID, &input.source).unwrap();
        let relative = prepared.relative.clone();
        let digest = prepared.digest.clone();
        prepared.persist();
        metadata.insert(
            "preview".into(),
            json!({
                "source": input.source.to_str().unwrap(),
                "digest": digest,
                "relative": relative.to_str().unwrap(),
            }),
        );
        write_import_metadata(root.path(), ID, &metadata).unwrap();
        let reused =
            solstone_core_import::snapshot::create(root.path(), ID, &input.source).unwrap();
        assert_eq!(reused.relative, relative);
        settings_refusal(root.path(), reused, true);
    }
}
