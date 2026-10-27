// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use solstone_core_cortex_client::{
    CortexRequest, CortexRequestClient, CortexRequestPolicy, DispatchError, UseIdAllocator,
    WaitForUsesReport,
};

use crate::helpers::ThinkStatus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchFailure {
    Unavailable,
    NotClaimed { use_id: String },
}

pub trait CortexBoundary: Send + Sync {
    fn dispatch(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
    ) -> Result<String, DispatchFailure>;
    /// The receipt must be persisted before any external send. An existing
    /// reservation is reused when no durable claim was observed.
    fn dispatch_prepared(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
        reserved: Option<&str>,
        prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
    ) -> Result<String, DispatchFailure>;
    fn wait(
        &self,
        runtime: &tokio::runtime::Runtime,
        use_ids: &[String],
        deadline: Option<Duration>,
    ) -> Result<WaitForUsesReport, String>;
}

pub trait IndexBoundary: Send + Sync {
    fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String>;

    /// Hosted runtimes supply the indexer command capability themselves.
    fn drain_pending(
        &self,
        _journal: &Path,
        _force: bool,
        _budget: Duration,
        _checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        Err("this host does not provide an in-process indexer".into())
    }
}

pub(crate) struct NativeIndexBoundary;

impl IndexBoundary for NativeIndexBoundary {
    fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String> {
        solstone_core_indexer_store::todo::enqueue_path(journal, path).map_err(|e| e.to_string())
    }
}

struct NativeCortexBoundary {
    client: CortexRequestClient,
    priority: solstone_core_journal_config::work_priority::WorkPriority,
}

impl NativeCortexBoundary {
    fn contextual_request(
        &self,
        request: &CortexRequest,
    ) -> Result<CortexRequest, DispatchFailure> {
        let mut request = request.clone();
        let env = request
            .config
            .entry("env")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or(DispatchFailure::Unavailable)?;
        env.insert(
            solstone_core_journal_config::work_priority::WORK_PRIORITY_ENV.to_owned(),
            serde_json::Value::String(self.priority.to_string()),
        );
        Ok(request)
    }
}

impl CortexBoundary for NativeCortexBoundary {
    fn dispatch(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
    ) -> Result<String, DispatchFailure> {
        let request = self.contextual_request(request)?;
        runtime
            .block_on(self.client.dispatch(&request))
            .map_err(|error| match error {
                DispatchError::Unavailable => DispatchFailure::Unavailable,
                DispatchError::NotClaimed { use_id } => DispatchFailure::NotClaimed { use_id },
            })
    }

    fn dispatch_prepared(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
        reserved: Option<&str>,
        prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
    ) -> Result<String, DispatchFailure> {
        let request = self.contextual_request(request)?;
        let result = if let Some(id) = reserved {
            let ts = id
                .parse::<i64>()
                .map_err(|_| DispatchFailure::Unavailable)?;
            prepare(id).map_err(|_| DispatchFailure::Unavailable)?;
            runtime.block_on(
                self.client
                    .dispatch_with_use_id(&request, ts, id.to_owned()),
            )
        } else {
            runtime.block_on(self.client.dispatch_prepared(&request, prepare))
        };
        result.map_err(|error| match error {
            DispatchError::Unavailable => DispatchFailure::Unavailable,
            DispatchError::NotClaimed { use_id } => DispatchFailure::NotClaimed { use_id },
        })
    }

    fn wait(
        &self,
        runtime: &tokio::runtime::Runtime,
        use_ids: &[String],
        deadline: Option<Duration>,
    ) -> Result<WaitForUsesReport, String> {
        runtime
            .block_on(self.client.wait_for_uses_with_deadline(use_ids, deadline))
            .map_err(|error| format!("cortex wait failed: {error:?}"))
    }
}

#[derive(Clone)]
pub struct ThinkContext {
    pub(crate) rebuild_recovery: bool,
    pub(crate) rebuild: Option<Arc<solstone_core_system::work_intent::RebuildLease>>,
    pub(crate) journal: PathBuf,
    pub(crate) day: String,
    pub(crate) day_dir: PathBuf,
    pub(crate) now_ms: i64,
    event_clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub(crate) talent_root: PathBuf,
    pub(crate) apps_root: PathBuf,
    pub(crate) cortex: Arc<dyn CortexBoundary>,
    pub(crate) index: Arc<dyn IndexBoundary>,
    pub(crate) status: ThinkStatus,
    pub(crate) retry: Option<(
        solstone_core_system::work_retry::Unit,
        solstone_core_system::work_retry::Attempt,
    )>,
    pub(crate) cancel_checkpoint: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) sense_executor: Option<Arc<dyn solstone_core_sense::SenseExecutor>>,
    pub(crate) phase_runner:
        Option<Arc<dyn crate::phase_process::PhaseProcessRunner + Send + Sync>>,
}

impl ThinkContext {
    #[allow(clippy::too_many_arguments)]
    pub fn hosted(
        journal: &Path,
        day: String,
        day_dir: PathBuf,
        now_ms: i64,
        talent_root: PathBuf,
        apps_root: PathBuf,
        cortex: Arc<dyn CortexBoundary>,
        index: Arc<dyn IndexBoundary>,
        event_clock: Arc<dyn Fn() -> i64 + Send + Sync>,
        retry: Option<(
            solstone_core_system::work_retry::Unit,
            solstone_core_system::work_retry::Attempt,
        )>,
        cancel_checkpoint: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        let phase_runner = Arc::new(crate::phase_process::HostedPhaseRunner {
            index: Arc::clone(&index),
            talent_root: talent_root.clone(),
            apps_root: apps_root.clone(),
            cancel_checkpoint: Arc::clone(&cancel_checkpoint),
        });
        Self {
            rebuild_recovery: false,
            rebuild: None,
            journal: journal.to_path_buf(),
            day,
            day_dir,
            now_ms,
            event_clock,
            talent_root,
            apps_root,
            cortex,
            index,
            status: ThinkStatus::default(),
            retry,
            cancel_checkpoint,
            sense_executor: None,
            phase_runner: Some(phase_runner),
        }
    }

    #[cfg(test)]
    pub(crate) fn new(
        journal: &Path,
        day: String,
        day_dir: PathBuf,
        now_ms: i64,
    ) -> Result<Self, String> {
        Self::new_with_event_clock(journal, day, day_dir, now_ms, Arc::new(move || now_ms))
    }

    pub(crate) fn new_with_event_clock(
        journal: &Path,
        day: String,
        day_dir: PathBuf,
        now_ms: i64,
        event_clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Result<Self, String> {
        let (talent_root, apps_root) = package_roots()?;
        let allocator_now = now_ms;
        Ok(Self {
            rebuild_recovery: false,
            rebuild: None,
            journal: journal.to_path_buf(),
            day,
            day_dir,
            now_ms,
            event_clock,
            talent_root,
            apps_root,
            cortex: Arc::new(NativeCortexBoundary {
                client: CortexRequestClient::with_allocator(
                    journal,
                    CortexRequestPolicy::think(),
                    UseIdAllocator::new(move || Some(allocator_now)),
                ),
                priority:
                    solstone_core_journal_config::work_priority::WorkPriority::from_environment()?,
            }),
            index: Arc::new(NativeIndexBoundary),
            status: ThinkStatus::default(),
            retry: None,
            cancel_checkpoint: Arc::new(|| false),
            sense_executor: None,
            phase_runner: None,
        })
    }

    pub fn with_sense_executor(
        mut self,
        executor: Arc<dyn solstone_core_sense::SenseExecutor>,
    ) -> Self {
        self.sense_executor = Some(executor);
        self
    }

    /// Opt in only when all execution joins the host and explicit Stop is durable.
    pub fn with_rebuild_recovery(mut self, enabled: bool) -> Self {
        self.rebuild_recovery = enabled;
        self
    }

    pub(crate) fn checkpoint(&self) -> Result<(), String> {
        if (self.cancel_checkpoint)() {
            Err("thinking cancelled".into())
        } else {
            if let Some(lease) = &self.rebuild {
                let intents = solstone_core_system::work_intent::read(&self.journal)?;
                if !intents.get(&lease.day).is_some_and(|intent| {
                    intent.id == lease.id
                        && intent.state == solstone_core_system::work_intent::IntentState::Claimed
                        && !intent.owner_stopped
                        && intent.rebuild.is_some()
                }) {
                    return Err("requested day is stopped or superseded".into());
                }
            }
            Ok(())
        }
    }

    pub(crate) fn event_now_ms(&self) -> i64 {
        (self.event_clock)()
    }

    /// The bounded request's input guard at reset/send admission. Publication
    /// retains the existing daily source and cancellation guards.
    pub(crate) fn rebuild_admission_checkpoint(&self) -> Result<(), String> {
        self.checkpoint()?;
        let Some(lease) = &self.rebuild else {
            return Ok(());
        };
        let intent = solstone_core_system::work_intent::read(&self.journal)?
            .remove(&lease.day)
            .ok_or("requested day disappeared")?;
        let progress = intent.rebuild.ok_or("requested day lost its provenance")?;
        let seal = progress.segments.ok_or("segment work needs owner review")?;
        let root = solstone_core_journal_io::JournalRoot::open(&self.journal)
            .map_err(|e| e.to_string())?;
        if root.identity().observation_id() != progress.root_binding
            || solstone_core_system::catchup::read_raw_input_fingerprint(&self.journal, &lease.day)
                .map_err(|e| e.to_string())?
                != seal.raw_fingerprint
        {
            return Err("requested day input or root changed".into());
        }
        let generation = match solstone_core_journal_io::read_health_marker(
            &self.journal,
            &lease.day,
            solstone_core_journal_io::HealthMarkerKind::Stream,
        )
        .map_err(|e| e.to_string())?
        {
            solstone_core_journal_io::HealthMarkerState::Versioned { marker, .. } => {
                marker.generation
            }
            solstone_core_journal_io::HealthMarkerState::Absent
            | solstone_core_journal_io::HealthMarkerState::LegacyEmpty { .. } => 0,
            _ => return Err("stream health marker is malformed".into()),
        };
        if generation != seal.generation {
            return Err("requested day generation changed".into());
        }
        let mut requested = self.clone();
        requested.day = lease.day.clone();
        requested.day_dir = self.journal.join("chronicle").join(&lease.day);
        if crate::daily_lifecycle::rebuild_segment_receipts(&requested, None)?
            != seal.receipts_digest
        {
            return Err("requested day segment receipts changed".into());
        }
        self.checkpoint()
    }

    #[cfg(test)]
    pub(crate) fn event_clock(&self) -> Arc<dyn Fn() -> i64 + Send + Sync> {
        Arc::clone(&self.event_clock)
    }

    #[cfg(test)]
    pub(crate) fn with_boundary(mut self, boundary: Arc<dyn CortexBoundary>) -> Self {
        self.cortex = boundary;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_event_clock(
        mut self,
        event_clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        self.event_clock = event_clock;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_index_boundary(mut self, boundary: Arc<dyn IndexBoundary>) -> Self {
        self.index = boundary;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_talent_roots(mut self, talent_root: PathBuf, apps_root: PathBuf) -> Self {
        self.talent_root = talent_root;
        self.apps_root = apps_root;
        self
    }
}

fn package_roots() -> Result<(PathBuf, PathBuf), String> {
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let directory = executable_dir.as_deref().unwrap_or(Path::new(""));
    package_roots_from_executable_dir(directory)
        .ok_or_else(|| solstone_core_journal::describe_package_roots_miss(directory))
}

fn package_roots_from_executable_dir(executable_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let root =
        solstone_core_journal::resolve_installation_root_from_executable_dir(executable_dir)?;
    let talent = root.join("solstone/talent");
    let apps = root.join("solstone/apps");
    (talent.is_dir() && apps.is_dir()).then_some((talent, apps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn ordinary_and_prepared_cortex_requests_preserve_each_callers_priority() {
        use solstone_core_journal_config::work_priority::WORK_PRIORITY_ENV;
        use std::io::Read;
        use std::os::unix::net::UnixListener;
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("health")).unwrap();
        let listener = UnixListener::bind(journal.path().join("health/callosum.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let root = journal.path().to_owned();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut requests = Vec::new();
            while requests.len() < 3 {
                let (mut stream, _) = match listener.accept() {
                    Ok(stream) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "Cortex request not sent"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                // Darwin refuses SO_RCVTIMEO once the sender has already closed
                // its half. Read through EOF with the same bounded wait even
                // when the whole request was queued before accept.
                stream.set_nonblocking(true).unwrap();
                let read_deadline = std::time::Instant::now() + Duration::from_secs(2);
                let mut received = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    assert!(
                        std::time::Instant::now() < read_deadline,
                        "request EOF not sent"
                    );
                    match stream.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => received.extend_from_slice(&buffer[..count]),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                }
                let bytes = String::from_utf8(received).unwrap();
                let request: serde_json::Value = serde_json::from_str(&bytes).unwrap();
                let name = request["name"].as_str().unwrap();
                let id = request["use_id"].as_str().unwrap();
                let directory = root.join("talents").join(name);
                fs::create_dir_all(&directory).unwrap();
                fs::write(
                    directory.join(format!("{id}_active.jsonl")),
                    serde_json::json!({"event":"request","name":name,"use_id":id}).to_string(),
                )
                .unwrap();
                requests.push(request);
            }
            requests
        });
        std::thread::scope(|scope| {
            for (index, priority) in ["1:20261011120000", "4:20261009120000", "6:20260101000000"]
                .into_iter()
                .enumerate()
            {
                let root = journal.path();
                scope.spawn(move || {
                    let boundary = NativeCortexBoundary {
                        client: CortexRequestClient::new(root, CortexRequestPolicy::think()),
                        priority: priority.parse().unwrap(),
                    };
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let request = CortexRequest::new("test", format!("priority{index}"))
                        .with_config(
                            serde_json::json!({"env":{"SOL_DAY":"20260101"}})
                                .as_object()
                                .unwrap()
                                .clone(),
                        );
                    let result = match index {
                        0 => boundary.dispatch(&runtime, &request),
                        1 => boundary.dispatch_prepared(&runtime, &request, None, &mut |_| Ok(())),
                        _ => {
                            boundary
                                .dispatch_prepared(&runtime, &request, Some("42"), &mut |_| Ok(()))
                        }
                    };
                    assert!(result.is_ok(), "{result:?}");
                    assert!(request.config["env"].get(WORK_PRIORITY_ENV).is_none());
                });
            }
        });
        let mut requests = server.join().unwrap();
        requests.sort_by_key(|request| request["name"].as_str().unwrap().to_owned());
        for (request, priority) in
            requests
                .iter()
                .zip(["1:20261011120000", "4:20261009120000", "6:20260101000000"])
        {
            assert_eq!(request["env"][WORK_PRIORITY_ENV], priority);
            assert_eq!(request["env"]["SOL_DAY"], "20260101");
        }
    }

    #[test]
    fn share_layout_requires_both_roots_and_fails_when_anchor_removed() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("tree");
        let bin = prefix.join("bin");
        let share = prefix.join("share");
        fs::create_dir_all(&bin).unwrap();
        for relative in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, relative).unwrap();
        }
        assert!(
            package_roots_from_executable_dir(&bin).is_none(),
            "talent without apps must fail closed"
        );
        let missing_apps = solstone_core_journal::describe_package_roots_miss(&bin);
        assert!(
            missing_apps.contains(&share.join("solstone/apps").display().to_string()),
            "directory-miss diagnostic must name solstone/apps: {missing_apps}"
        );
        fs::create_dir_all(share.join("solstone/apps")).unwrap();
        let (talent, apps) = package_roots_from_executable_dir(&bin).unwrap();
        assert_eq!(talent, share.join("solstone/talent"));
        assert_eq!(apps, share.join("solstone/apps"));
        fs::remove_file(share.join(solstone_core_journal::LAYOUT_BUNDLE_ANCHOR)).unwrap();
        assert!(package_roots_from_executable_dir(&bin).is_none());
    }

    #[test]
    fn native_index_boundary_queues_an_absent_path_without_opening_sqlite() {
        let journal = tempfile::tempdir().unwrap();
        let missing_output = journal.path().join("does-not-exist.md");
        assert!(!missing_output.exists());

        let result = NativeIndexBoundary.enqueue_path(journal.path(), &missing_output);
        result.unwrap();
        assert_eq!(
            solstone_core_indexer_store::todo::inspect(journal.path())
                .unwrap()
                .keys,
            ["does-not-exist.md"]
        );
        assert!(!solstone_core_indexer_store::db::db_path(journal.path()).exists());
    }
}
