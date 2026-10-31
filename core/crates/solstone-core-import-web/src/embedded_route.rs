// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! HTTP port for one retained import route.
//!
//! This module does not classify arguments, build import seams, or take a
//! recovery lease. `prepare` admits before final arguments exist. `capture`
//! checkpoints, drops an exact `solstone journal importer` prefix, and binds.
//! Preview and run move onto a `solstone-import` thread this slot retains.
//! `preview_held_after` returns a handle. Dropping it only signals that
//! thread, which then drops the prepared value before the captured import.
//! That prepared value is not a fresh snapshot tree. Dropping the HTTP future
//! does not request stop and does not drop that thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use solstone_core_import::WorkStop;
use solstone_core_import::cli_render::CliRun;
use solstone_core_journal_config::work_priority::{WorkBand, WorkPriority};

const IMPORTER_UNAVAILABLE: &str = "importer unavailable";

/// One admitted import handed to the retained route.
#[derive(Clone)]
pub struct ImportRouteRequest {
    pub args: Vec<String>,
    pub stop: WorkStop,
    pub priority: WorkPriority,
}

/// One captured import. The `Arc` holds the route's lease until the last clone drops.
pub trait CapturedImport: Send + Sync {
    fn preview(&self) -> Result<CliRun, String>;
    fn run(&self) -> Result<CliRun, String>;
}

/// Admission taken before final arguments exist.
///
/// `prepare` does not copy, relocate, write metadata, or admit a native attempt.
/// `bind` does not admit one either.
pub trait ImportAdmission: Send + Sync {
    fn checkpoint(&self) -> Result<(), String>;
    fn bind(self: Arc<Self>, args: Vec<String>) -> Result<Arc<dyn CapturedImport>, String>;
}

/// The HTTP port. `prepare` does not preview, run, or write.
pub trait RetainedImportRoute: Send + Sync {
    fn prepare(
        &self,
        stop: WorkStop,
        priority: WorkPriority,
    ) -> Result<Arc<dyn ImportAdmission>, String>;
}

/// Caller handle for a preview the retained worker still owns.
///
/// Dropping this only signals that worker. The worker drops `T`, then the
/// captured import. `T` is not a fresh snapshot tree.
struct Retained<T> {
    run: Option<CliRun>,
    release: Option<Sender<()>>,
    _prepared: std::marker::PhantomData<T>,
}

impl<T> Drop for Retained<T> {
    fn drop(&mut self) {
        self.release.take();
    }
}

impl<T> Retained<T> {
    fn into_run(mut self) -> CliRun {
        self.run.take().expect("preview run")
    }
}

struct Workers {
    handles: Vec<JoinHandle<()>>,
    pending: usize,
    closed: bool,
    failed: bool,
}

/// Retains the installed route, its stop, and the threads that run it.
pub struct ImportRouteSlot {
    route: Mutex<Option<Arc<dyn RetainedImportRoute>>>,
    stop: WorkStop,
    closed: AtomicBool,
    workers: Mutex<Workers>,
    retirement: Mutex<()>,
    workers_changed: Condvar,
    /// Import ids whose retained run has been accepted and has not finished.
    retained: Mutex<Vec<String>>,
    pause_before_spawn: AtomicBool,
    registration_waiting: AtomicBool,
    pause_before_snapshot_delete: AtomicBool,
    snapshot_delete_waiting: AtomicBool,
    pause_before_validation: AtomicBool,
    validation_waiting: AtomicBool,
}

impl ImportRouteSlot {
    pub fn new() -> Self {
        Self {
            route: Mutex::new(None),
            stop: WorkStop::new(),
            closed: AtomicBool::new(false),
            workers: Mutex::new(Workers {
                handles: Vec::new(),
                pending: 0,
                closed: false,
                failed: false,
            }),
            workers_changed: Condvar::new(),
            retirement: Mutex::new(()),
            retained: Mutex::new(Vec::new()),
            pause_before_spawn: AtomicBool::new(false),
            registration_waiting: AtomicBool::new(false),
            pause_before_snapshot_delete: AtomicBool::new(false),
            snapshot_delete_waiting: AtomicBool::new(false),
            pause_before_validation: AtomicBool::new(false),
            validation_waiting: AtomicBool::new(false),
        }
    }

    /// Block the retained worker immediately before it deletes a fresh snapshot.
    ///
    /// Production leaves the pause clear, so this returns at once.
    pub(crate) fn wait_before_snapshot_delete(&self) {
        while self.pause_before_snapshot_delete.load(Ordering::Acquire) {
            self.snapshot_delete_waiting.store(true, Ordering::Release);
            thread::yield_now();
        }
        self.snapshot_delete_waiting.store(false, Ordering::Release);
    }

    /// Block prepare's validation await. Production leaves the pause clear.
    pub(crate) fn wait_before_validation(&self) {
        while self.pause_before_validation.load(Ordering::Acquire) {
            self.validation_waiting.store(true, Ordering::Release);
            thread::yield_now();
        }
        self.validation_waiting.store(false, Ordering::Release);
    }

    /// Remember an import whose retained run has been accepted.
    ///
    /// A later start of this id replays until [`Self::forget`]. Dropping the
    /// HTTP response does not forget it.
    pub(crate) fn remember(&self, import_id: &str) {
        let mut retained = self
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !retained.iter().any(|id| id == import_id) {
            retained.push(import_id.to_owned());
        }
    }

    /// Drop a remembered import. Called from the retained thread after run.
    pub(crate) fn forget(&self, import_id: &str) {
        let mut retained = self
            .retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        retained.retain(|id| id != import_id);
    }

    /// Whether this slot still holds `import_id` as accepted work.
    pub(crate) fn retains(&self, import_id: &str) -> bool {
        self.retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|id| id == import_id)
    }

    /// Install the route that later capture calls use.
    pub fn install(&self, route: Arc<dyn RetainedImportRoute>) -> Result<(), String> {
        if self.closed.load(Ordering::Acquire) {
            return Err(IMPORTER_UNAVAILABLE.to_owned());
        }
        let mut slot = self
            .route
            .lock()
            .map_err(|_| IMPORTER_UNAVAILABLE.to_owned())?;
        if self.closed.load(Ordering::Acquire) {
            return Err(IMPORTER_UNAVAILABLE.to_owned());
        }
        *slot = Some(route);
        Ok(())
    }

    /// Take admission before final arguments exist. No copy and no native attempt.
    pub(crate) fn prepare(&self, source_time: u64) -> Result<Arc<dyn ImportAdmission>, String> {
        if self.closed.load(Ordering::Acquire) {
            return Err(IMPORTER_UNAVAILABLE.to_owned());
        }
        let route = self
            .route
            .lock()
            .map_err(|_| IMPORTER_UNAVAILABLE.to_owned())?
            .clone()
            .ok_or_else(|| IMPORTER_UNAVAILABLE.to_owned())?;
        if self.closed.load(Ordering::Acquire) {
            return Err(IMPORTER_UNAVAILABLE.to_owned());
        }
        route.prepare(
            self.stop.clone(),
            WorkPriority {
                band: WorkBand::Import,
                source_time,
            },
        )
    }

    /// Strip the launcher prefix and bind. No native attempt.
    pub(crate) fn bind(
        &self,
        admission: Arc<dyn ImportAdmission>,
        args: Vec<String>,
    ) -> Result<Arc<dyn CapturedImport>, String> {
        let args = importer_arguments(args)?;
        ImportAdmission::bind(admission, args)
    }

    /// Prepare, checkpoint, strip, and bind. Callers already know the final argv.
    pub fn capture(
        &self,
        args: Vec<String>,
        source_time: u64,
    ) -> Result<Arc<dyn CapturedImport>, String> {
        let admission = self.prepare(source_time)?;
        admission.checkpoint()?;
        self.bind(admission, args)
    }

    /// Run preview on the retained thread and return its `CliRun`.
    ///
    /// The caller keeps its `Arc`. The retained thread holds another until the
    /// caller drops the returned handle, which only signals that thread.
    pub fn preview_held(&self, captured: &Arc<dyn CapturedImport>) -> Result<CliRun, String> {
        self.preview_held_after(captured, || Ok(()))
            .map(Retained::into_run)
    }

    /// Run `prepare`, then preview, on the retained thread.
    ///
    /// On success the caller owns a signal handle. The worker keeps `T` and the
    /// captured import until that handle is dropped or the send fails. It then
    /// drops `T` before the captured import. `T` is not a fresh snapshot.
    fn preview_held_after<T, F>(
        &self,
        captured: &Arc<dyn CapturedImport>,
        prepare: F,
    ) -> Result<Retained<T>, String>
    where
        F: FnOnce() -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let held = Arc::clone(captured);
        let (sender, receiver) = mpsc::channel();
        self.retain(move |run| {
            if !run {
                drop(held);
                let _ = sender.send(Err(IMPORTER_UNAVAILABLE.to_owned()));
                return;
            }
            let prepared = match prepare() {
                Ok(value) => value,
                Err(error) => {
                    drop(held);
                    let _ = sender.send(Err(error));
                    return;
                }
            };
            let previewed = match held.preview() {
                Ok(run) => run,
                Err(error) => {
                    drop(prepared);
                    drop(held);
                    let _ = sender.send(Err(error));
                    return;
                }
            };
            let (release_sender, release_receiver) = mpsc::channel();
            let retained = Retained {
                run: Some(previewed),
                release: Some(release_sender),
                _prepared: std::marker::PhantomData,
            };
            if sender.send(Ok(retained)).is_ok() {
                let _ = release_receiver.recv();
            }
            drop(prepared);
            drop(held);
        })?;
        receiver
            .recv()
            .map_err(|_| IMPORTER_UNAVAILABLE.to_owned())?
    }

    /// Hand run to the retained thread and return once that thread owns it.
    ///
    /// `on_done` runs on that thread after `run` returns, and then the slot
    /// drops its `Arc`. Dropping the HTTP wait does not skip `on_done` and
    /// does not drop the thread.
    pub fn run_retained(
        &self,
        captured: Arc<dyn CapturedImport>,
        on_done: impl FnOnce(Result<CliRun, String>) + Send + 'static,
    ) -> Result<(), String> {
        let (owned_sender, owned_receiver) = mpsc::sync_channel(0);
        self.retain(move |run| {
            if !run {
                on_done(Err(IMPORTER_UNAVAILABLE.to_owned()));
                drop(captured);
                return;
            }
            let _ = owned_sender.send(());
            let result = captured.run();
            on_done(result);
            drop(captured);
        })?;
        owned_receiver
            .recv()
            .map_err(|_| IMPORTER_UNAVAILABLE.to_owned())?;
        Ok(())
    }

    /// Close admission and signal cancellation without waiting for a worker.
    /// Host Stop signals every owner before it starts any checked drain.
    /// Temporary recovery must use its retained target checkpoint instead.
    pub fn request_stop(&self) {
        self.closed.store(true, Ordering::Release);
        self.stop.request();
    }

    /// Close admission, request stop, and check every retained thread's retirement.
    ///
    /// Does not return while a retain is between its pending increment and
    /// handle push, or while a retained callback is still running. No route in
    /// this crate calls this. A recovery hold must not either.
    pub fn retire(&self) -> Result<(), String> {
        self.request_stop();
        // A join batch lives outside `workers` while it drains. A second
        // caller must not mistake that temporarily empty vector for retirement.
        let _retirement = self
            .retirement
            .lock()
            .map_err(|_| "import worker retirement failed".to_owned())?;
        let mut workers = self.lock_workers();
        workers.closed = true;
        self.workers_changed.notify_all();
        loop {
            if workers.pending == 0 && workers.handles.is_empty() {
                break;
            }
            if workers.handles.is_empty() {
                workers = self
                    .workers_changed
                    .wait(workers)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            let batch = std::mem::take(&mut workers.handles);
            drop(workers);
            let mut failed = false;
            for handle in batch {
                failed |= handle.join().is_err();
            }
            workers = self.lock_workers();
            workers.failed |= failed;
        }
        if workers.failed {
            Err("import worker retirement failed".to_owned())
        } else {
            Ok(())
        }
    }

    fn lock_workers(&self) -> MutexGuard<'_, Workers> {
        self.workers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn retain(&self, work: impl FnOnce(bool) + Send + 'static) -> Result<(), String> {
        if self.closed.load(Ordering::Acquire) {
            return Err(IMPORTER_UNAVAILABLE.to_owned());
        }
        {
            let mut workers = self.lock_workers();
            if workers.closed || self.closed.load(Ordering::Acquire) {
                return Err(IMPORTER_UNAVAILABLE.to_owned());
            }
            workers.pending += 1;
        }
        while self.pause_before_spawn.load(Ordering::Acquire) {
            self.registration_waiting.store(true, Ordering::Release);
            thread::yield_now();
        }
        self.registration_waiting.store(false, Ordering::Release);
        // The worker waits until its handle is stored. A closed slot tells it
        // not to run, so shutdown can join the callback without starting work.
        let (gate_sender, gate_receiver) = mpsc::channel();
        let handle = match thread::Builder::new()
            .name("solstone-import".into())
            .spawn(move || {
                let run = matches!(gate_receiver.recv(), Ok(true));
                work(run);
            }) {
            Ok(handle) => handle,
            Err(_) => {
                let mut workers = self.lock_workers();
                workers.pending -= 1;
                self.workers_changed.notify_all();
                return Err(IMPORTER_UNAVAILABLE.to_owned());
            }
        };
        let run = {
            let mut workers = self.lock_workers();
            reap_finished(&mut workers);
            let run = !workers.closed && !self.closed.load(Ordering::Acquire);
            workers.handles.push(handle);
            workers.pending -= 1;
            self.workers_changed.notify_all();
            run
        };
        if gate_sender.send(run).is_err() {
            return Err(IMPORTER_UNAVAILABLE.to_owned());
        }
        if run {
            Ok(())
        } else {
            Err(IMPORTER_UNAVAILABLE.to_owned())
        }
    }
}

impl Default for ImportRouteSlot {
    fn default() -> Self {
        Self::new()
    }
}

/// Drop the exact `solstone journal importer` prefix.
///
/// Importer arguments are the media path, the timestamp, and the flags.
/// Anything that does not start with those three words is refused. A later
/// word is kept, including a path that spells one of them.
fn importer_arguments(args: Vec<String>) -> Result<Vec<String>, String> {
    let [launcher, journal, importer, rest @ ..] = args.as_slice() else {
        return Err(IMPORTER_UNAVAILABLE.to_owned());
    };
    if launcher != "solstone" || journal != "journal" || importer != "importer" {
        return Err(IMPORTER_UNAVAILABLE.to_owned());
    }
    Ok(rest.to_vec())
}

/// Join handles whose threads have already returned. A live handle stays.
fn reap_finished(workers: &mut Workers) {
    let pending = std::mem::take(&mut workers.handles);
    for handle in pending {
        if handle.is_finished() {
            workers.failed |= handle.join().is_err();
        } else {
            workers.handles.push(handle);
        }
    }
}

/// `YYYYMMDDHHMMSS` from a validated `YYYYMMDD_HHMMSS` stamp, otherwise zero.
///
/// Callers pass the import timestamp. The underscore is not part of the
/// priority key. A shape or calendar failure contributes no source time.
pub(crate) fn source_time(timestamp: &str) -> u64 {
    let Ok(validated) = solstone_core_import::validate_timestamp(timestamp) else {
        return 0;
    };
    validated
        .as_str()
        .bytes()
        .filter(|byte| *byte != b'_')
        .fold(0_u64, |value, byte| value * 10 + u64::from(byte - b'0'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{command, contained_import_file};
    use crate::{AppState, lifecycle, preview};
    use axum::extract::{Json, State};
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use solstone_core_import::{
        AttemptState, ProjectionStatus, read_import_metadata, write_import_metadata,
    };
    use std::sync::Condvar;
    use std::sync::atomic::AtomicUsize;

    const ID: &str = "20261011_120000";

    struct Seen {
        request: ImportRouteRequest,
        running: bool,
        preview_absent: bool,
    }

    struct Fake {
        root: std::path::PathBuf,
        me: Mutex<std::sync::Weak<Fake>>,
        captures: Mutex<Vec<Seen>>,
        kept: Arc<Mutex<Vec<ImportRouteRequest>>>,
        runs: AtomicUsize,
        entered: Arc<(Mutex<bool>, Condvar)>,
        release: Arc<(Mutex<bool>, Condvar)>,
        block_run: AtomicBool,
        admit_on_run: AtomicBool,
        run_returned: AtomicBool,
        dropped_after_run: Arc<(Mutex<bool>, Condvar)>,
        preview_error: Mutex<Option<String>>,
        preview_stdout: Mutex<Option<String>>,
        preview_stderr: Mutex<Option<String>>,
        run_error: Mutex<Option<String>>,
        capture_error: Mutex<Option<String>>,
        block_capture: AtomicBool,
        execution_error: Mutex<Option<String>>,
        block_execution: AtomicBool,
        capture_entered: AtomicBool,
        capture_release: AtomicBool,
        live: AtomicUsize,
    }

    struct Held {
        fake: Arc<Fake>,
    }

    impl Fake {
        fn new(root: std::path::PathBuf) -> Arc<Self> {
            let fake = Arc::new(Self {
                root,
                me: Mutex::new(std::sync::Weak::new()),
                captures: Mutex::new(Vec::new()),
                kept: Arc::new(Mutex::new(Vec::new())),
                runs: AtomicUsize::new(0),
                entered: Arc::new((Mutex::new(false), Condvar::new())),
                release: Arc::new((Mutex::new(false), Condvar::new())),
                block_run: AtomicBool::new(false),
                admit_on_run: AtomicBool::new(false),
                run_returned: AtomicBool::new(false),
                dropped_after_run: Arc::new((Mutex::new(false), Condvar::new())),
                preview_error: Mutex::new(None),
                preview_stdout: Mutex::new(None),
                preview_stderr: Mutex::new(None),
                run_error: Mutex::new(None),
                capture_error: Mutex::new(None),
                block_capture: AtomicBool::new(false),
                execution_error: Mutex::new(None),
                block_execution: AtomicBool::new(false),
                capture_entered: AtomicBool::new(false),
                capture_release: AtomicBool::new(false),
                live: AtomicUsize::new(0),
            });
            *fake.me.lock().expect("self") = Arc::downgrade(&fake);
            fake
        }

        fn arc(&self) -> Arc<Self> {
            self.me.lock().expect("self").upgrade().expect("fake")
        }
    }

    impl Drop for Held {
        fn drop(&mut self) {
            self.fake.live.fetch_sub(1, Ordering::AcqRel);
            if self.fake.run_returned.load(Ordering::Acquire) {
                let (flag, notify) = &*self.fake.dropped_after_run;
                *flag.lock().expect("drop flag") = true;
                notify.notify_all();
            }
        }
    }

    impl CapturedImport for Held {
        fn preview(&self) -> Result<CliRun, String> {
            if let Some(error) = self
                .fake
                .preview_error
                .lock()
                .expect("preview error")
                .clone()
            {
                return Err(error);
            }
            if let Some(stderr) = self.fake.preview_stderr.lock().expect("stderr").clone() {
                return Ok(CliRun {
                    stdout: String::new(),
                    stderr,
                    exit_code: 1,
                });
            }
            if let Some(stdout) = self.fake.preview_stdout.lock().expect("stdout").clone() {
                return Ok(CliRun {
                    stdout,
                    stderr: String::new(),
                    exit_code: 0,
                });
            }
            Ok(CliRun {
                stdout: "{\"schema\":\"solstone.import.size.v1\",\"recordings\":1}".to_owned(),
                stderr: String::new(),
                exit_code: 0,
            })
        }

        fn run(&self) -> Result<CliRun, String> {
            self.fake.runs.fetch_add(1, Ordering::AcqRel);
            if self.fake.admit_on_run.load(Ordering::Acquire) {
                let started_at_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                solstone_core_import::admit_running_attempt(
                    &self.fake.root,
                    ID,
                    started_at_ms,
                    Some("ics"),
                )
                .expect("admit");
            }
            {
                let (entered, notify) = &*self.fake.entered;
                *entered.lock().expect("entered") = true;
                notify.notify_all();
            }
            if self.fake.block_run.load(Ordering::Acquire) {
                let (release, notify) = &*self.fake.release;
                let mut guard = release.lock().expect("release");
                while !*guard {
                    guard = notify.wait(guard).expect("release wait");
                }
            }
            self.fake.run_returned.store(true, Ordering::Release);
            if let Some(error) = self.fake.run_error.lock().expect("run error").clone() {
                return Err(error);
            }
            Ok(CliRun {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
    }

    struct Prepared {
        fake: Arc<Fake>,
        stop: WorkStop,
        priority: WorkPriority,
    }

    impl ImportAdmission for Prepared {
        fn checkpoint(&self) -> Result<(), String> {
            if self.stop.is_requested() {
                Err("import stopped".to_owned())
            } else {
                Ok(())
            }
        }

        fn bind(self: Arc<Self>, args: Vec<String>) -> Result<Arc<dyn CapturedImport>, String> {
            let execution = !args.iter().any(|arg| arg == "--dry-run");
            if execution {
                if self.fake.block_execution.load(Ordering::Acquire) {
                    self.fake.capture_entered.store(true, Ordering::Release);
                    spin_until(|| self.fake.capture_release.load(Ordering::Acquire));
                }
                if let Some(error) = self
                    .fake
                    .execution_error
                    .lock()
                    .expect("execution error")
                    .clone()
                {
                    return Err(error);
                }
            }
            let request = ImportRouteRequest {
                args,
                stop: self.stop.clone(),
                priority: self.priority,
            };
            let running = solstone_core_import::project_import_result(&self.fake.root, ID).status
                == ProjectionStatus::Running;
            let preview_absent = read_import_metadata(&self.fake.root, ID)
                .ok()
                .and_then(|metadata| metadata.get("preview").cloned())
                .is_none();
            self.fake.kept.lock().expect("kept").push(request.clone());
            self.fake.captures.lock().expect("captures").push(Seen {
                request,
                running,
                preview_absent,
            });
            self.fake.live.fetch_add(1, Ordering::AcqRel);
            Ok(Arc::new(Held {
                fake: Arc::clone(&self.fake),
            }))
        }
    }

    impl RetainedImportRoute for Fake {
        fn prepare(
            &self,
            stop: WorkStop,
            priority: WorkPriority,
        ) -> Result<Arc<dyn ImportAdmission>, String> {
            if self.block_capture.load(Ordering::Acquire) {
                self.capture_entered.store(true, Ordering::Release);
                spin_until(|| self.capture_release.load(Ordering::Acquire));
            }
            if let Some(error) = self.capture_error.lock().expect("capture error").clone() {
                return Err(error);
            }
            Ok(Arc::new(Prepared {
                fake: self.arc(),
                stop,
                priority,
            }))
        }
    }

    fn fixture() -> (tempfile::TempDir, Value, serde_json::Map<String, Value>) {
        let root = tempfile::tempdir().expect("journal");
        let directory = root.path().join("imports").join(ID);
        std::fs::create_dir_all(&directory).expect("import directory");
        let source = directory.join("notes.txt");
        std::fs::write(&source, "a note").expect("source");
        let mut metadata = json!({
            "import_id": ID,
            "source_timestamp": ID,
            "file_size": 6,
            "file_path": source,
            "setting": "office",
            "source_hash": solstone_core_import::hash_source(&source).expect("hash").as_str(),
        })
        .as_object()
        .expect("metadata object")
        .clone();
        solstone_core_import::saved_start::initialize_saved_start(&mut metadata, ID)
            .expect("new save");
        write_import_metadata(root.path(), ID, &metadata).expect("metadata");
        (
            root,
            json!({"path": source, "timestamp": ID, "import_id": ID}),
            metadata,
        )
    }

    fn installed(fake: &Arc<Fake>) -> Arc<ImportRouteSlot> {
        let slot = Arc::new(ImportRouteSlot::new());
        slot.install(fake.clone()).expect("install");
        slot
    }

    async fn body_of(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    const SPIN_CAP: usize = 10_000_000;

    fn spin_until(mut ready: impl FnMut() -> bool) {
        for _ in 0..SPIN_CAP {
            if ready() {
                return;
            }
            std::thread::yield_now();
        }
        panic!("embedded import preparation did not resume");
    }

    fn import_listing(root: &std::path::Path) -> Vec<String> {
        let mut names = std::fs::read_dir(root.join("imports").join(ID))
            .expect("import directory")
            .map(|entry| {
                entry
                    .expect("import entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    struct Resume<'a>(&'a AtomicBool);

    impl Drop for Resume<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    struct FlagRelease(Arc<AtomicBool>);

    impl Drop for FlagRelease {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    struct ClearPause(Arc<ImportRouteSlot>);

    impl Drop for ClearPause {
        fn drop(&mut self) {
            self.0.pause_before_spawn.store(false, Ordering::Release);
        }
    }

    struct ReleaseRun(Arc<Fake>);

    impl Drop for ReleaseRun {
        fn drop(&mut self) {
            release(&self.0);
        }
    }

    fn wait_until(pair: &(Mutex<bool>, Condvar)) {
        let (flag, notify) = pair;
        let mut guard = flag.lock().expect("flag");
        while !*guard {
            guard = notify.wait(guard).expect("wait");
        }
    }

    fn release(fake: &Fake) {
        let (flag, notify) = &*fake.release;
        *flag.lock().expect("release") = true;
        notify.notify_all();
    }

    #[test]
    fn source_time_orders_underscore_stamps_newest_first_inside_import() {
        let older = source_time("20261011_090000");
        let newer = source_time("20261011_120000");
        assert_eq!(older, 20_261_011_090_000);
        assert_eq!(newer, 20_261_011_120_000);
        let mut priorities = [
            WorkPriority {
                band: WorkBand::Import,
                source_time: older,
            },
            WorkPriority {
                band: WorkBand::Import,
                source_time: newer,
            },
        ];
        priorities.sort_by_key(|priority| priority.sort_key());
        assert_eq!(priorities[0].source_time, newer);
        assert_eq!(priorities[1].source_time, older);
    }

    #[tokio::test]
    async fn embedded_preview_uses_the_retained_route_and_the_existing_command() {
        let (root, data, metadata) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let desktop = AppState {
            root: root.path().to_path_buf(),
            route: None,
        };
        let _desktop = preview::preview(State(desktop), Json(data.clone())).await;
        assert!(fake.captures.lock().expect("captures").is_empty());

        let embedded = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let response = preview::preview(State(embedded), Json(data)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let saved = read_import_metadata(root.path(), ID).expect("saved metadata");
        let relative = saved["preview"]["relative"]
            .as_str()
            .expect("snapshot relative");
        let snapshot = contained_import_file(root.path(), ID, relative).expect("snapshot");
        let mut expected = importer_arguments(command(
            snapshot.to_str().expect("snapshot path"),
            ID,
            &metadata,
            false,
        ))
        .expect("prefix");
        expected.extend([
            "--dry-run".to_owned(),
            "--json".to_owned(),
            "--preview-zone".to_owned(),
            solstone_core_journal_config::owner_zone(root.path())
                .name()
                .to_owned(),
        ]);
        let recorded = fake.captures.lock().expect("captures");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].request.args, expected);
        assert!(!recorded[0].request.args.iter().any(|arg| {
            arg == "solstone" || arg == "journal" || arg == "importer" || arg == "--preview-token"
        }));
        assert_eq!(recorded[0].request.priority.band, WorkBand::Import);
        assert_eq!(recorded[0].request.priority.source_time, source_time(ID));
        assert!(!recorded[0].request.stop.is_requested());
        drop(recorded);
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn embedded_preview_failure_carries_the_route_result() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let refusal = "route refused this preview".to_owned();
        *fake.preview_error.lock().expect("error") = Some(refusal.clone());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(slot.clone()),
        };
        let response = preview::preview(State(state.clone()), Json(data.clone())).await;
        let body = body_of(response).await;
        assert_eq!(body["detail"], refusal);

        *fake.preview_error.lock().expect("error") = None;
        let stderr = "route stderr".to_owned();
        *fake.preview_stderr.lock().expect("stderr") = Some(stderr.clone());
        let response = preview::preview(State(state), Json(data)).await;
        let body = body_of(response).await;
        assert_eq!(body["detail"], stderr);
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn run_keeps_the_request_after_the_response_drops_and_replay_does_not_run_again() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let kept = Arc::clone(&fake.kept);
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        fake.block_run.store(true, Ordering::Release);
        let response = lifecycle::start(State(state.clone()), Json(data.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let started = body_of(response).await;
        assert!(started.get("generation").is_none());
        assert!(started.get("attempt_id").is_none());
        wait_until(&fake.entered);
        assert_eq!(kept.lock().expect("kept").len(), 2);
        assert_ne!(
            solstone_core_import::project_import_result(root.path(), ID).status,
            ProjectionStatus::Running
        );
        assert!(slot.retains(ID));

        let replay = lifecycle::start(State(state), Json(data.clone())).await;
        assert_eq!(replay.status(), StatusCode::OK);
        let body = body_of(replay).await;
        assert_eq!(body["replay"], true);
        assert_eq!(kept.lock().expect("kept").len(), 2);
        assert_eq!(fake.runs.load(Ordering::Acquire), 1);
        assert!(!kept.lock().expect("kept")[0].stop.is_requested());
        release(&fake);
        slot.retire().expect("checked import retirement");
        assert_eq!(
            solstone_core_import::saved_start::saved_start_state(
                &read_import_metadata(root.path(), ID).unwrap(),
                ID
            ),
            solstone_core_import::saved_start::SavedStartState::Consumed
        );
        // The worker returned without an attempt. A new authority over the same
        // record must still refuse a stale first-Start request.
        let restarted = Fake::new(root.path().to_path_buf());
        let restarted_slot = installed(&restarted);
        let mut stale = data;
        stale["first_start_only"] = json!(true);
        let response = lifecycle::start(
            State(AppState {
                root: root.path().to_path_buf(),
                route: Some(restarted_slot.clone()),
            }),
            Json(stale),
        )
        .await;
        assert_ne!(response.status(), StatusCode::OK);
        assert_eq!(restarted.runs.load(Ordering::Acquire), 0);
        restarted_slot.retire().unwrap();
    }

    #[tokio::test]
    async fn recreated_unconsumed_record_can_preview_and_start_but_legacy_cannot() {
        let (root, mut data, mut metadata) = fixture();
        let source_time = "20250101_090000";
        metadata.insert("source_timestamp".into(), json!(source_time));
        metadata.insert("user_timestamp".into(), json!(source_time));
        metadata.insert("setting".into(), json!("planning"));
        write_import_metadata(root.path(), ID, &metadata).unwrap();
        data["timestamp"] = json!(source_time);
        data["setting"] = json!("planning");
        let fake = Fake::new(root.path().to_path_buf());
        let old_slot = installed(&fake);
        old_slot.retire().unwrap();
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(slot.clone()),
        };
        let before = std::fs::read(root.path().join(format!("imports/{ID}/import.json"))).unwrap();
        let detail =
            crate::imports::detail(State(state.clone()), axum::extract::Path(ID.to_owned())).await;
        assert_eq!(body_of(detail).await["saved_start"]["state"], "eligible");
        assert_eq!(
            before,
            std::fs::read(root.path().join(format!("imports/{ID}/import.json"))).unwrap()
        );
        assert_eq!(fake.runs.load(Ordering::Acquire), 0);
        let previewed =
            body_of(preview::preview(State(state.clone()), Json(data.clone())).await).await;
        data["preview_token"] = previewed["preview_token"].clone();
        data["first_start_only"] = json!(true);
        assert_eq!(
            lifecycle::start(State(state), Json(data)).await.status(),
            StatusCode::OK
        );
        wait_until(&fake.entered);
        assert_eq!(fake.runs.load(Ordering::Acquire), 1);
        slot.retire().unwrap();
        assert!(!root.path().join("imports").join(source_time).exists());
        let metadata = read_import_metadata(root.path(), ID).unwrap();
        assert_eq!(metadata["source_timestamp"], source_time);
        assert_eq!(metadata["setting"], "planning");

        let (legacy_root, mut legacy_data, mut metadata) = fixture();
        metadata.remove(solstone_core_import::saved_start::SAVED_START_KEY);
        // Construct an old record, rather than asking a metadata update to erase
        // a durable witness (which the writer deliberately prevents).
        std::fs::write(
            legacy_root.path().join(format!("imports/{ID}/import.json")),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        let legacy_fake = Fake::new(legacy_root.path().to_path_buf());
        let legacy_slot = installed(&legacy_fake);
        let state = AppState {
            root: legacy_root.path().to_path_buf(),
            route: Some(legacy_slot.clone()),
        };
        let detail =
            crate::imports::detail(State(state.clone()), axum::extract::Path(ID.to_owned())).await;
        assert_eq!(body_of(detail).await["saved_start"]["state"], "unknown");
        let _ = preview::preview(State(state.clone()), Json(legacy_data.clone())).await;
        assert!(
            !read_import_metadata(legacy_root.path(), ID)
                .unwrap()
                .contains_key(solstone_core_import::saved_start::SAVED_START_KEY)
        );
        legacy_data["first_start_only"] = json!(true);
        assert_ne!(
            lifecycle::start(State(state), Json(legacy_data))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(legacy_fake.runs.load(Ordering::Acquire), 0);
        legacy_slot.retire().unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn registered_folder_previews_and_first_starts_from_its_copied_tree() {
        let journal = crate::test_support::phase_root("empty");
        let root = Arc::new(solstone_core_journal_io::JournalRoot::open(journal.path()).unwrap());
        let stamp = solstone_core_import::validate_timestamp(ID).unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("note.md"), b"folder note").unwrap();
        let folder =
            solstone_core_import::admit_folder_bound(&root, source.path(), "folder", &stamp)
                .unwrap();
        assert_eq!(folder.import_id.as_str(), ID);
        crate::register_admitted_folder(
            &root,
            &folder,
            "obsidian",
            "notes",
            "folder-row",
            &stamp,
            chrono_tz::UTC,
        )
        .unwrap();
        // Continuation reads only the copied tree, never the owner's original.
        drop(source);
        let fake = Fake::new(journal.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: journal.path().to_path_buf(),
            route: Some(slot.clone()),
        };
        let detail = body_of(
            crate::imports::detail(State(state.clone()), axum::extract::Path(ID.to_owned())).await,
        )
        .await;
        assert_eq!(detail["saved_start"]["state"], "eligible");
        let metadata = &detail["import_json"];
        assert_eq!(metadata["file_size"], Value::Null);
        // The same request body the detail page sends.
        let mut data = json!({"import_id": ID, "path": metadata["file_path"],
            "timestamp": metadata["source_timestamp"], "setting": ""});
        let previewed =
            body_of(preview::preview(State(state.clone()), Json(data.clone())).await).await;
        assert!(previewed["preview_token"].is_string(), "{previewed}");
        data["preview_token"] = previewed["preview_token"].clone();
        data["first_start_only"] = json!(true);
        assert_eq!(
            lifecycle::start(State(state), Json(data.clone()))
                .await
                .status(),
            StatusCode::OK
        );
        wait_until(&fake.entered);
        assert_eq!(fake.runs.load(Ordering::Acquire), 1);
        slot.retire().unwrap();
        assert_eq!(
            solstone_core_import::saved_start::saved_start_state(
                &read_import_metadata(journal.path(), ID).unwrap(),
                ID
            ),
            solstone_core_import::saved_start::SavedStartState::Consumed
        );
        let restarted = Fake::new(journal.path().to_path_buf());
        let restarted_slot = installed(&restarted);
        let again = lifecycle::start(
            State(AppState {
                root: journal.path().to_path_buf(),
                route: Some(restarted_slot.clone()),
            }),
            Json(data),
        )
        .await;
        assert_ne!(again.status(), StatusCode::OK);
        assert_eq!(restarted.runs.load(Ordering::Acquire), 0);
        restarted_slot.retire().unwrap();
    }

    #[tokio::test]
    async fn concurrent_first_starts_and_early_run_refusal_do_not_restore_permission() {
        let (root, mut data, _) = fixture();
        data["first_start_only"] = json!(true);
        let fake = Fake::new(root.path().to_path_buf());
        *fake.run_error.lock().unwrap() = Some("early preparation refusal".to_owned());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(slot.clone()),
        };
        let (left, right) = tokio::join!(
            lifecycle::start(State(state.clone()), Json(data.clone())),
            lifecycle::start(State(state), Json(data.clone())),
        );
        assert_eq!(
            usize::from(left.status() == StatusCode::OK)
                + usize::from(right.status() == StatusCode::OK),
            1
        );
        wait_until(&fake.entered);
        slot.retire().unwrap();
        assert_eq!(fake.runs.load(Ordering::Acquire), 1);
        assert!(
            read_import_metadata(root.path(), ID)
                .unwrap()
                .get("attempt")
                .is_none()
        );
        let restarted = Fake::new(root.path().to_path_buf());
        let restarted_slot = installed(&restarted);
        let response = lifecycle::start(
            State(AppState {
                root: root.path().to_path_buf(),
                route: Some(restarted_slot.clone()),
            }),
            Json(data),
        )
        .await;
        assert_ne!(response.status(), StatusCode::OK);
        assert_eq!(restarted.runs.load(Ordering::Acquire), 0);
        restarted_slot.retire().unwrap();
    }

    #[tokio::test]
    async fn failed_consumption_refuses_execution_and_accepted_response() {
        let (root, mut data, _) = fixture();
        data["first_start_only"] = json!(true);
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let seams = lifecycle::StartSeams {
            consume: Some(Arc::new(Mutex::new(Box::new(|_, _, _| {
                Err(solstone_core_import::ImportError::MetadataWriteFailed {
                    path: std::path::PathBuf::from("import.json"),
                    message: "controlled persistence failure".to_owned(),
                })
            })))),
            ..Default::default()
        };
        let response = lifecycle::run_start(
            root.path(),
            data,
            preview::PreviewPlan::Embedded(slot.clone()),
            seams,
        )
        .await;
        assert_ne!(response.status(), StatusCode::OK);
        assert_eq!(fake.runs.load(Ordering::Acquire), 0);
        assert!(!slot.retains(ID));
        slot.retire().unwrap();
    }

    #[tokio::test]
    async fn capture_precedes_the_preview_record_and_the_running_attempt() {
        let (root, mut data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let response = preview::preview(State(state.clone()), Json(data.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        {
            let captures = fake.captures.lock().expect("captures");
            assert_eq!(captures.len(), 1);
            assert!(!captures[0].running);
            assert!(captures[0].preview_absent);
        }
        assert!(
            read_import_metadata(root.path(), ID)
                .expect("metadata")
                .get("preview")
                .is_some()
        );

        let previewed = body_of(response).await;
        data["preview_token"] = previewed["preview_token"].clone();
        let started = lifecycle::start(State(state), Json(data)).await;
        assert_eq!(started.status(), StatusCode::OK);
        wait_until(&fake.entered);
        assert_ne!(
            solstone_core_import::project_import_result(root.path(), ID).status,
            ProjectionStatus::Running
        );
        let captures = fake.captures.lock().expect("captures");
        assert!(captures.len() > 1);
        assert!(captures.iter().all(|seen| !seen.running));
        assert!(captures[0].preview_absent);
        assert!(
            captures[0]
                .request
                .args
                .iter()
                .any(|arg| arg == "--dry-run")
        );
        let run_args = &captures.last().expect("run capture").request.args;
        assert!(
            !run_args
                .iter()
                .any(|arg| arg == "--dry-run" || arg == "--json" || arg == "--preview-zone")
        );
        assert!(run_args.windows(2).any(|pair| pair[0] == "--preview-token"));
        drop(captures);
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn a_refused_capture_does_not_create_a_preview_directory() {
        let (root, data, _) = fixture();
        let before = import_listing(root.path());
        assert!(before.iter().all(|name| !name.starts_with("preview-")));
        let fake = Fake::new(root.path().to_path_buf());
        *fake.capture_error.lock().expect("capture error") = Some("capture refused".to_owned());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let response = preview::preview(State(state), Json(data)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_of(response).await;
        assert_eq!(body["detail"], "capture refused");
        assert_eq!(import_listing(root.path()), before);
        assert!(
            read_import_metadata(root.path(), ID)
                .expect("metadata")
                .get("preview")
                .is_none()
        );
        slot.retire().expect("checked import retirement");
    }

    #[test]
    fn a_blocked_capture_does_not_copy_until_it_resumes() {
        let (root, data, _) = fixture();
        let before = import_listing(root.path());
        assert!(before.iter().all(|name| !name.starts_with("preview-")));
        let fake = Fake::new(root.path().to_path_buf());
        fake.block_capture.store(true, Ordering::Release);
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let preview = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime");
            runtime.block_on(preview::preview(State(state), Json(data)))
        });
        let release = Resume(&fake.capture_release);
        spin_until(|| fake.capture_entered.load(Ordering::Acquire));
        assert_eq!(import_listing(root.path()), before);
        assert!(fake.captures.lock().expect("captures").is_empty());
        release.0.store(true, Ordering::Release);
        let response = preview.join().expect("preview thread");
        assert_eq!(response.status(), StatusCode::OK);
        let saved = read_import_metadata(root.path(), ID).expect("metadata");
        let relative = saved["preview"]["relative"].as_str().expect("relative");
        let copied = contained_import_file(root.path(), ID, relative).expect("snapshot");
        assert_eq!(std::fs::read(&copied).expect("snapshot bytes"), b"a note");
        let mut after = import_listing(root.path());
        assert_eq!(
            after
                .iter()
                .filter(|name| name.starts_with("preview-"))
                .count(),
            1
        );
        after.retain(|name| !name.starts_with("preview-"));
        assert_eq!(after, before);
        slot.retire().expect("checked import retirement");
    }

    const CHOSEN: &str = "20261012_120000";

    fn choose_owner_date(data: &mut Value) {
        data["timestamp"] = json!(CHOSEN);
        data["timestamp_override"] = json!(true);
    }

    async fn attach_preview_token(state: &AppState, data: &mut Value) {
        let response = preview::preview(State(state.clone()), Json(data.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let previewed = body_of(response).await;
        data["preview_token"] = previewed["preview_token"].clone();
    }

    #[tokio::test]
    async fn a_refused_execution_capture_does_not_relocate_or_write_the_owner_date() {
        let (root, mut data, _) = fixture();
        let source = data["path"].clone();
        choose_owner_date(&mut data);
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        attach_preview_token(&state, &mut data).await;
        *fake.execution_error.lock().expect("execution error") =
            Some("execution refused".to_owned());
        let response = lifecycle::start(State(state), Json(data)).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = body_of(response).await;
        assert_eq!(body["detail"], "execution refused");
        assert!(root.path().join("imports").join(ID).is_dir());
        assert!(!root.path().join("imports").join(CHOSEN).exists());
        let metadata = read_import_metadata(root.path(), ID).expect("metadata");
        assert!(metadata.get("owner_timestamp").is_none());
        assert_eq!(metadata.get("file_path"), Some(&source));
        assert!(metadata.get("preview").is_some());
        assert!(metadata.get("attempt").is_none());
        assert_eq!(fake.runs.load(Ordering::Acquire), 0);
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn a_refused_execution_capture_leaves_an_occupied_destination() {
        let (root, mut data, _) = fixture();
        choose_owner_date(&mut data);
        let destination = root.path().join("imports").join(CHOSEN);
        std::fs::create_dir_all(&destination).expect("destination");
        let kept = destination.join("owner.txt");
        std::fs::write(&kept, "keep").expect("destination file");
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        attach_preview_token(&state, &mut data).await;
        *fake.execution_error.lock().expect("execution error") =
            Some("execution refused".to_owned());
        let response = lifecycle::start(State(state), Json(data)).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(root.path().join("imports").join(ID).is_dir());
        assert!(destination.is_dir());
        assert_eq!(std::fs::read(&kept).expect("kept"), b"keep");
        let metadata = read_import_metadata(root.path(), ID).expect("metadata");
        assert!(metadata.get("owner_timestamp").is_none());
        assert!(metadata.get("attempt").is_none());
        assert_eq!(fake.runs.load(Ordering::Acquire), 0);
        slot.retire().expect("checked import retirement");
    }

    #[test]
    fn a_blocked_execution_capture_relocates_only_after_resume() {
        let (root, mut data, _) = fixture();
        let source = data["path"].clone();
        choose_owner_date(&mut data);
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        runtime.block_on(attach_preview_token(&state, &mut data));
        drop(runtime);
        fake.block_execution.store(true, Ordering::Release);
        let started = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime");
            runtime.block_on(lifecycle::start(State(state), Json(data)))
        });
        let release = Resume(&fake.capture_release);
        spin_until(|| fake.capture_entered.load(Ordering::Acquire));
        assert!(!root.path().join("imports").join(CHOSEN).exists());
        let blocked = read_import_metadata(root.path(), ID).expect("original metadata");
        assert!(blocked.get("owner_timestamp").is_none());
        assert_eq!(blocked.get("file_path"), Some(&source));
        assert_eq!(fake.runs.load(Ordering::Acquire), 0);
        release.0.store(true, Ordering::Release);
        spin_until(|| fake.runs.load(Ordering::Acquire) == 1);
        assert!(!root.path().join("imports").join(ID).exists());
        let stored = read_import_metadata(root.path(), CHOSEN).expect("relocated metadata");
        assert_eq!(stored["import_id"], CHOSEN);
        assert_eq!(stored["owner_timestamp"], CHOSEN);
        let captures = fake.captures.lock().expect("captures");
        let execution = captures
            .iter()
            .find(|seen| !seen.request.args.iter().any(|arg| arg == "--dry-run"))
            .expect("execution capture");
        assert!(
            std::path::Path::new(&execution.request.args[0]).starts_with(
                root.path()
                    .canonicalize()
                    .unwrap()
                    .join("imports")
                    .join(CHOSEN)
            ),
            "{}",
            execution.request.args[0]
        );
        drop(captures);
        let response = started.join().expect("start thread");
        assert_eq!(response.status(), StatusCode::OK);
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn a_refused_run_settles_the_attempt_and_replay_does_not_capture_again() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        fake.block_run.store(true, Ordering::Release);
        let kept = Arc::clone(&fake.kept);
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let response = lifecycle::start(State(state.clone()), Json(data.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        drop(response);
        wait_until(&fake.entered);
        assert_eq!(kept.lock().expect("kept").len(), 2);
        assert_ne!(
            solstone_core_import::project_import_result(root.path(), ID).status,
            ProjectionStatus::Running
        );

        let replay = lifecycle::start(State(state), Json(data)).await;
        assert_eq!(replay.status(), StatusCode::OK);
        let body = body_of(replay).await;
        assert_eq!(body["replay"], true);
        assert_eq!(kept.lock().expect("kept").len(), 2);
        assert_eq!(fake.runs.load(Ordering::Acquire), 1);

        let started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let first =
            solstone_core_import::admit_running_attempt(root.path(), ID, started_at_ms, None)
                .expect("admit");
        solstone_core_import::record_completed_attempt(
            root.path(),
            ID,
            first.generation,
            started_at_ms.saturating_add(1),
            Some(1),
            None,
        )
        .expect("complete");
        let newer = solstone_core_import::admit_running_attempt(
            root.path(),
            ID,
            started_at_ms.saturating_add(2),
            None,
        )
        .expect("newer admit");
        release(&fake);
        wait_until(&fake.dropped_after_run);
        let facts = solstone_core_import::get_attempt_facts(
            &read_import_metadata(root.path(), ID).expect("metadata"),
        )
        .expect("attempt");
        assert_eq!(facts.generation, newer.generation);
        assert_eq!(facts.state, AttemptState::Running);
        assert_eq!(facts.attempt_id, newer.attempt_id);
        assert_eq!(kept.lock().expect("kept").len(), 2);
        slot.retire().expect("checked import retirement");
    }

    #[test]
    fn exact_prefix_keeps_a_trailing_preview_token() {
        let kept = importer_arguments(vec![
            "solstone".to_owned(),
            "journal".to_owned(),
            "importer".to_owned(),
            "journal".to_owned(),
            "--preview-token".to_owned(),
            "token".to_owned(),
        ])
        .expect("prefix");
        assert_eq!(
            kept,
            vec![
                "journal".to_owned(),
                "--preview-token".to_owned(),
                "token".to_owned(),
            ]
        );
        assert_eq!(
            importer_arguments(vec![
                "solstone".to_owned(),
                "journal".to_owned(),
                "notes.txt".to_owned(),
                "--preview-token".to_owned(),
                "token".to_owned(),
            ])
            .expect_err("malformed prefix"),
            "importer unavailable"
        );
    }

    fn worker_counts(slot: &ImportRouteSlot) -> (usize, usize) {
        let workers = slot.workers.lock().expect("workers");
        let finished = workers
            .handles
            .iter()
            .filter(|handle| handle.is_finished())
            .count();
        (workers.handles.len() - finished, finished)
    }

    fn wait_until_finished(slot: &ImportRouteSlot) {
        loop {
            let workers = slot.workers.lock().expect("workers");
            if !workers.handles.is_empty()
                && workers.handles.iter().all(|handle| handle.is_finished())
            {
                break;
            }
            drop(workers);
            std::thread::yield_now();
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn worker_panic_is_a_sticky_checked_retirement_failure() {
        let slot = ImportRouteSlot::new();
        slot.retain(|_| panic!("synthetic import worker panic"))
            .expect("worker accepted");
        assert_eq!(
            slot.retire(),
            Err("import worker retirement failed".to_owned())
        );
        assert_eq!(
            slot.retire(),
            Err("import worker retirement failed".to_owned())
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn reaping_a_panicked_worker_does_not_erase_its_retirement_failure() {
        let slot = ImportRouteSlot::new();
        slot.retain(|_| panic!("synthetic reaped import panic"))
            .expect("worker accepted");
        wait_until_finished(&slot);
        slot.retain(|_| {}).expect("next worker accepted");
        assert_eq!(
            slot.retire(),
            Err("import worker retirement failed".to_owned())
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn request_stop_signals_before_joining_a_blocked_worker() {
        let slot = Arc::new(ImportRouteSlot::new());
        let (started, entered) = mpsc::channel();
        let (release, waiting) = mpsc::channel();
        slot.retain(move |_| {
            started.send(()).expect("worker entered");
            waiting
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("worker released");
        })
        .expect("worker accepted");
        entered
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("worker entered");
        let (signalled, observed) = mpsc::channel();
        let signal_slot = Arc::clone(&slot);
        let signal = std::thread::spawn(move || {
            signal_slot.request_stop();
            signalled.send(()).expect("signal observed");
        });
        let delivered = observed.recv_timeout(std::time::Duration::from_secs(2));
        let stopped = slot.stop.is_requested();
        let live = worker_counts(&slot).0;
        release.send(()).expect("release worker");
        signal.join().expect("stop caller returned");
        assert!(delivered.is_ok(), "Stop must not join the blocked worker");
        assert!(stopped);
        assert_eq!(live, 1);
        assert!(slot.retain(|_| {}).is_err());
        slot.retire().expect("worker retired after release");
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn retirement_joins_the_remaining_worker_after_a_prior_worker_panics() {
        let slot = Arc::new(ImportRouteSlot::new());
        let (panic_go, panic_wait) = mpsc::channel();
        slot.retain(move |_| {
            panic_wait
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("panic released");
            panic!("synthetic first worker panic");
        })
        .expect("first worker accepted");
        let (entered, waiting) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let drained = Arc::new(AtomicBool::new(false));
        let worker_drained = Arc::clone(&drained);
        slot.retain(move |_| {
            entered.send(()).expect("second entered");
            blocked
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("second released");
            worker_drained.store(true, Ordering::Release);
        })
        .expect("second worker accepted");
        waiting
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("second entered");
        panic_go.send(()).expect("first may panic");
        let (finished, observed) = mpsc::channel();
        let retire_slot = Arc::clone(&slot);
        let retire = std::thread::spawn(move || {
            finished
                .send(retire_slot.retire())
                .expect("retirement result");
        });
        let premature = observed.recv_timeout(std::time::Duration::from_millis(50));
        release.send(()).expect("release second");
        let result = observed
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("all workers retired");
        retire.join().expect("retire caller returned");
        assert!(
            premature.is_err(),
            "a failed join must not skip a remaining live worker"
        );
        assert!(drained.load(Ordering::Acquire));
        assert_eq!(result, Err("import worker retirement failed".to_owned()));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn overlapping_retirements_both_wait_and_observe_the_worker_failure() {
        let slot = Arc::new(ImportRouteSlot::new());
        let (entered, waiting) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        slot.retain(move |_| {
            entered.send(()).expect("worker entered");
            blocked
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("worker released");
            panic!("synthetic overlapped retirement panic");
        })
        .expect("worker accepted");
        waiting
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("worker entered");
        let (finished, observed) = mpsc::channel();
        let first_slot = Arc::clone(&slot);
        let first_done = finished.clone();
        let first = std::thread::spawn(move || {
            first_done.send(first_slot.retire()).expect("first result");
        });
        spin_until(|| slot.workers.lock().expect("workers").handles.is_empty());
        let (second_entered, second_waiting) = mpsc::channel();
        let second_slot = Arc::clone(&slot);
        let second = std::thread::spawn(move || {
            second_entered.send(()).expect("second entered");
            finished.send(second_slot.retire()).expect("second result");
        });
        second_waiting
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("second entered");
        let premature = observed.recv_timeout(std::time::Duration::from_millis(100));
        release.send(()).expect("release worker");
        first.join().expect("first retirement returned");
        second.join().expect("second retirement returned");
        assert!(
            premature.is_err(),
            "neither caller may acknowledge a batch another caller is still joining"
        );
        for _ in 0..2 {
            assert_eq!(
                observed
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("checked result"),
                Err("import worker retirement failed".to_owned())
            );
        }
    }

    #[tokio::test]
    async fn sequential_previews_do_not_keep_finished_handles() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let first = preview::preview(State(state.clone()), Json(data.clone())).await;
        assert_eq!(first.status(), StatusCode::OK);
        wait_until_finished(&slot);
        let second = preview::preview(State(state), Json(data)).await;
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(slot.workers.lock().expect("workers").handles.len(), 1);
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn a_running_import_stays_while_a_finished_preview_is_reaped() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        fake.block_run.store(true, Ordering::Release);
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let started = lifecycle::start(State(state.clone()), Json(data.clone())).await;
        assert_eq!(started.status(), StatusCode::OK);
        wait_until(&fake.entered);
        assert_eq!(worker_counts(&slot).0, 1);

        let first = preview::preview(State(state.clone()), Json(data.clone())).await;
        assert_eq!(first.status(), StatusCode::OK);
        loop {
            let (running, finished) = worker_counts(&slot);
            if running == 1 && finished >= 1 {
                break;
            }
            std::thread::yield_now();
        }
        let before = slot.workers.lock().expect("workers").handles.len();
        let second = preview::preview(State(state), Json(data)).await;
        assert_eq!(second.status(), StatusCode::OK);
        // Returning the HTTP result releases the preview's retained value; it
        // does not join the worker that still owns its cleanup. Count the
        // blocked import after that preview has actually returned.
        spin_until(|| worker_counts(&slot).0 == 1);
        let (running, _) = worker_counts(&slot);
        assert_eq!(running, 1);
        assert_eq!(slot.workers.lock().expect("workers").handles.len(), before);
        release(&fake);
        slot.retire().expect("checked import retirement");
    }

    fn preview_names(root: &std::path::Path) -> Vec<String> {
        import_listing(root)
            .into_iter()
            .filter(|name| name.starts_with("preview-"))
            .collect()
    }

    fn launcher(path: &str) -> Vec<String> {
        vec![
            "solstone".to_owned(),
            "journal".to_owned(),
            "importer".to_owned(),
            path.to_owned(),
            "--dry-run".to_owned(),
        ]
    }

    struct BlockValue {
        entered: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
        fake: Arc<Fake>,
    }

    impl Drop for BlockValue {
        fn drop(&mut self) {
            assert!(self.fake.live.load(Ordering::Acquire) > 0);
            self.entered.store(true, Ordering::Release);
            while !self.release.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        }
    }

    #[tokio::test]
    async fn a_second_preview_reuses_the_same_snapshot_directory() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let first = preview::preview(State(state.clone()), Json(data.clone())).await;
        assert_eq!(first.status(), StatusCode::OK);
        let saved = read_import_metadata(root.path(), ID).expect("metadata");
        let relative = saved["preview"]["relative"]
            .as_str()
            .expect("relative")
            .to_owned();
        let directories = preview_names(root.path());
        assert_eq!(directories.len(), 1);
        let second = preview::preview(State(state), Json(data)).await;
        assert_eq!(second.status(), StatusCode::OK);
        let saved = read_import_metadata(root.path(), ID).expect("metadata");
        assert_eq!(
            saved["preview"]["relative"].as_str(),
            Some(relative.as_str())
        );
        assert_eq!(preview_names(root.path()), directories);
        let snapshot = contained_import_file(root.path(), ID, &relative).expect("snapshot");
        let snapshot = snapshot.to_str().expect("snapshot path");
        let captures = fake.captures.lock().expect("captures");
        assert!(captures.len() >= 2);
        assert!(
            captures
                .last()
                .expect("second capture")
                .request
                .args
                .iter()
                .any(|arg| arg == snapshot)
        );
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn a_recorded_snapshot_is_reused_without_a_process_counter() {
        let (root, data, _) = fixture();
        let source = std::path::PathBuf::from(data["path"].as_str().expect("path"));
        let prepared =
            solstone_core_import::snapshot::create(root.path(), ID, &source).expect("snapshot");
        let relative = prepared.relative.clone();
        let digest = prepared.digest.clone();
        prepared.persist();
        let mut metadata = read_import_metadata(root.path(), ID).expect("metadata");
        metadata.insert(
            "preview".to_owned(),
            json!({
                "source": source.to_str().expect("source"),
                "digest": digest,
                "relative": relative.to_str().expect("relative"),
            }),
        );
        write_import_metadata(root.path(), ID, &metadata).expect("record");
        let before = preview_names(root.path());
        assert_eq!(before.len(), 1);
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(slot.clone()),
        };
        let response = preview::preview(State(state), Json(data)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(preview_names(root.path()), before);
        let saved = read_import_metadata(root.path(), ID).expect("metadata");
        assert_eq!(saved["preview"]["relative"].as_str(), relative.to_str());
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn a_planted_preview_directory_is_not_the_saved_snapshot() {
        let (root, data, _) = fixture();
        let planted = root
            .path()
            .join("imports")
            .join(ID)
            .join("preview-0000000000000001");
        std::fs::create_dir(&planted).expect("planted directory");
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(slot.clone()),
        };
        let response = preview::preview(State(state), Json(data)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(planted.is_dir());
        let saved = read_import_metadata(root.path(), ID).expect("metadata");
        let relative = saved["preview"]["relative"].as_str().expect("relative");
        assert!(!relative.starts_with("preview-0000000000000001"));
        let directory = relative.split('/').next().expect("directory");
        assert!(directory.starts_with("preview-"));
        assert!(
            root.path()
                .join("imports")
                .join(ID)
                .join(directory)
                .is_dir()
        );
        slot.retire().expect("checked import retirement");
    }

    #[tokio::test]
    async fn a_refused_prepare_on_start_leaves_no_attempt() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        *fake.capture_error.lock().expect("capture error") = Some("capture refused".to_owned());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(slot.clone()),
        };
        let response = lifecycle::start(State(state), Json(data)).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = body_of(response).await;
        assert_eq!(body["detail"], "capture refused");
        assert!(
            read_import_metadata(root.path(), ID)
                .expect("metadata")
                .get("attempt")
                .is_none()
        );
        slot.retire().expect("checked import retirement");
    }

    #[test]
    fn a_closed_execution_bind_leaves_no_attempt() {
        let (root, mut data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        runtime.block_on(attach_preview_token(&state, &mut data));
        drop(runtime);
        fake.block_execution.store(true, Ordering::Release);
        let started = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime");
            runtime.block_on(lifecycle::start(State(state), Json(data)))
        });
        let release = Resume(&fake.capture_release);
        spin_until(|| fake.capture_entered.load(Ordering::Acquire));
        slot.closed.store(true, Ordering::Release);
        {
            let mut workers = slot.lock_workers();
            workers.closed = true;
        }
        release.0.store(true, Ordering::Release);
        let response = started.join().expect("start thread");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let body = runtime.block_on(body_of(response));
        assert_eq!(body["detail"], "importer unavailable");
        let metadata = read_import_metadata(root.path(), ID).expect("metadata");
        assert!(metadata.get("attempt").is_none());
        assert!(metadata.get("owner_timestamp").is_none());
        assert!(root.path().join("imports").join(ID).is_dir());
        slot.retire().expect("checked import retirement");
    }

    #[test]
    fn dropping_a_prepared_value_keeps_the_captured_import_alive() {
        // BlockValue is the prepared value the worker drops on release.
        // It is not a fresh snapshot tree.
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let captured = slot
            .capture(
                launcher(data["path"].as_str().expect("path")),
                source_time(ID),
            )
            .expect("capture");
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered_flag = Arc::clone(&entered);
        let release_flag = Arc::clone(&release);
        let fake_for_value = Arc::clone(&fake);
        let retained = slot
            .preview_held_after(&captured, move || {
                Ok(BlockValue {
                    entered: entered_flag,
                    release: release_flag,
                    fake: fake_for_value,
                })
            })
            .expect("preview");
        drop(captured);
        let watcher = std::thread::spawn(move || {
            spin_until(|| entered.load(Ordering::Acquire));
            release.store(true, Ordering::Release);
        });
        drop(retained);
        watcher.join().expect("watcher");
        let captured = slot
            .capture(
                launcher(data["path"].as_str().expect("path")),
                source_time(ID),
            )
            .expect("later capture");
        let run = slot.preview_held(&captured).expect("later preview");
        assert_eq!(run.exit_code, 0);
        slot.retire().expect("checked import retirement");
    }

    #[test]
    fn an_abandoned_preview_drops_before_the_hold_and_retire_waits() {
        // The worker drops BlockValue after the caller signals. This is not a fresh tree.
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let captured = slot
            .capture(
                launcher(data["path"].as_str().expect("path")),
                source_time(ID),
            )
            .expect("capture");
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered_flag = Arc::clone(&entered);
        let release_flag = Arc::clone(&release);
        let fake_for_value = Arc::clone(&fake);
        let slot_for_task = Arc::clone(&slot);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let handle = runtime.spawn_blocking(move || {
            let retained = slot_for_task
                .preview_held_after(&captured, move || {
                    Ok(BlockValue {
                        entered: entered_flag,
                        release: release_flag,
                        fake: fake_for_value,
                    })
                })
                .expect("preview");
            drop(captured);
            retained
        });
        drop(handle);
        spin_until(|| entered.load(Ordering::Acquire));
        assert!(fake.live.load(Ordering::Acquire) > 0);
        let retired = Arc::new(AtomicBool::new(false));
        let retired_flag = Arc::clone(&retired);
        let slot_for_retire = Arc::clone(&slot);
        let retire_thread = std::thread::spawn(move || {
            slot_for_retire.retire().expect("checked import retirement");
            retired_flag.store(true, Ordering::Release);
        });
        let release_guard = FlagRelease(Arc::clone(&release));
        for _ in 0..SPIN_CAP {
            assert!(!retired.load(Ordering::Acquire));
            std::thread::yield_now();
        }
        drop(release_guard);
        retire_thread.join().expect("retire");
        assert!(retired.load(Ordering::Acquire));
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(slot.clone()),
        };
        let response = runtime.block_on(preview::preview(State(state), Json(data)));
        assert_eq!(response.status(), StatusCode::OK);
        slot.retire().expect("checked import retirement");
    }

    #[test]
    fn retire_waits_while_a_preview_retain_is_unregistered_and_then_blocked() {
        let slot = Arc::new(ImportRouteSlot::new());
        let _clear_pause = ClearPause(Arc::clone(&slot));
        slot.pause_before_spawn.store(true, Ordering::Release);
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let slot_for_retain = Arc::clone(&slot);
        let entered_flag = Arc::clone(&entered);
        let release_flag = Arc::clone(&release);
        let finished_flag = Arc::clone(&finished);
        let retain_thread = std::thread::spawn(move || {
            let _ = slot_for_retain.retain(move |_run| {
                entered_flag.store(true, Ordering::Release);
                while !release_flag.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                finished_flag.store(true, Ordering::Release);
            });
        });
        spin_until(|| slot.registration_waiting.load(Ordering::Acquire));
        let retired = Arc::new(AtomicBool::new(false));
        let retired_flag = Arc::clone(&retired);
        let finished_for_retire = Arc::clone(&finished);
        let slot_for_retire = Arc::clone(&slot);
        let retire_thread = std::thread::spawn(move || {
            slot_for_retire.retire().expect("checked import retirement");
            assert!(finished_for_retire.load(Ordering::Acquire));
            retired_flag.store(true, Ordering::Release);
        });
        let release_guard = FlagRelease(Arc::clone(&release));
        for _ in 0..SPIN_CAP {
            assert!(!retired.load(Ordering::Acquire));
            std::thread::yield_now();
        }
        slot.pause_before_spawn.store(false, Ordering::Release);
        spin_until(|| entered.load(Ordering::Acquire));
        for _ in 0..SPIN_CAP {
            assert!(!retired.load(Ordering::Acquire));
            std::thread::yield_now();
        }
        drop(release_guard);
        retire_thread.join().expect("retire");
        assert!(finished.load(Ordering::Acquire));
        assert!(retired.load(Ordering::Acquire));
        retain_thread.join().expect("retain");
    }

    #[test]
    fn retire_waits_for_a_blocked_run_callback_and_on_done_finishes_first() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let mut args = launcher(data["path"].as_str().expect("path"));
        args.pop();
        let captured = slot.capture(args, source_time(ID)).expect("capture");
        fake.block_run.store(true, Ordering::Release);
        let on_done = Arc::new(AtomicBool::new(false));
        let on_done_flag = Arc::clone(&on_done);
        let slot_for_run = Arc::clone(&slot);
        let runner = std::thread::spawn(move || {
            slot_for_run
                .run_retained(captured, move |_| {
                    on_done_flag.store(true, Ordering::Release);
                })
                .expect("retained run");
        });
        spin_until(|| *fake.entered.0.lock().expect("entered"));
        let _release_run = ReleaseRun(Arc::clone(&fake));
        let retired = Arc::new(AtomicBool::new(false));
        let retired_flag = Arc::clone(&retired);
        let on_done_for_retire = Arc::clone(&on_done);
        let slot_for_retire = Arc::clone(&slot);
        let retire_thread = std::thread::spawn(move || {
            slot_for_retire.retire().expect("checked import retirement");
            assert!(on_done_for_retire.load(Ordering::Acquire));
            retired_flag.store(true, Ordering::Release);
        });
        for _ in 0..SPIN_CAP {
            assert!(!retired.load(Ordering::Acquire));
            assert!(!on_done.load(Ordering::Acquire));
            std::thread::yield_now();
        }
        release(&fake);
        retire_thread.join().expect("retire");
        assert!(on_done.load(Ordering::Acquire));
        assert!(retired.load(Ordering::Acquire));
        runner.join().expect("runner");
    }

    #[tokio::test]
    async fn a_tokenless_owner_date_start_writes_preview_on_the_relocated_row() {
        let (root, mut data, _) = fixture();
        choose_owner_date(&mut data);
        let fake = Fake::new(root.path().to_path_buf());
        fake.block_run.store(true, Ordering::Release);
        let slot = installed(&fake);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let response = lifecycle::start(State(state), Json(data)).await;
        assert_eq!(response.status(), StatusCode::OK);
        drop(response);
        wait_until(&fake.entered);
        let captures = fake.captures.lock().expect("captures");
        assert_eq!(captures.len(), 2);
        assert_eq!(fake.live.load(Ordering::Acquire), 1);
        let execution = captures
            .iter()
            .find(|seen| !seen.request.args.iter().any(|arg| arg == "--dry-run"))
            .expect("execution capture");
        assert!(
            execution
                .request
                .args
                .iter()
                .any(|arg| std::path::Path::new(arg).starts_with(
                    root.path()
                        .canonicalize()
                        .unwrap()
                        .join("imports")
                        .join(CHOSEN)
                )),
            "{:?}",
            execution.request.args
        );
        drop(captures);
        assert!(!root.path().join("imports").join(ID).exists());
        let stored = read_import_metadata(root.path(), CHOSEN).expect("relocated metadata");
        assert!(stored.get("preview").is_some());
        assert_eq!(stored["import_id"], CHOSEN);
        assert_eq!(stored["owner_timestamp"], CHOSEN);
        assert!(stored.get("attempt").is_none());
        let relative = stored["preview"]["relative"].as_str().expect("relative");
        let copied = contained_import_file(root.path(), CHOSEN, relative).expect("snapshot");
        assert_eq!(std::fs::read(&copied).expect("snapshot bytes"), b"a note");
        release(&fake);
        slot.retire().expect("checked import retirement");
    }

    struct ClearSnapshotPauses(Arc<ImportRouteSlot>);

    impl Drop for ClearSnapshotPauses {
        fn drop(&mut self) {
            self.0
                .pause_before_snapshot_delete
                .store(false, Ordering::Release);
            self.0
                .pause_before_validation
                .store(false, Ordering::Release);
        }
    }

    fn fresh_preview_directory(root: &std::path::Path) -> Option<std::path::PathBuf> {
        std::fs::read_dir(root.join("imports").join(ID))
            .expect("import directory")
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("preview-"))
                    && path.is_dir()
            })
    }

    async fn refused_size_deletes_on_the_worker(fake: Arc<Fake>, data: Value, detail: &str) {
        let root = fake.root.clone();
        let slot = installed(&fake);
        let _clear = ClearSnapshotPauses(Arc::clone(&slot));
        slot.pause_before_snapshot_delete
            .store(true, Ordering::Release);
        let state = AppState {
            root: root.clone(),
            route: Some(Arc::clone(&slot)),
        };
        let preview_task = tokio::spawn(preview::preview(State(state), Json(data)));
        let slot_wait = Arc::clone(&slot);
        tokio::task::spawn_blocking(move || {
            spin_until(|| slot_wait.snapshot_delete_waiting.load(Ordering::Acquire));
        })
        .await
        .expect("cleanup pause");
        let directory = fresh_preview_directory(&root).expect("fresh preview");
        assert!(directory.is_dir());
        assert!(fake.live.load(Ordering::Acquire) > 0);
        assert!(!preview_task.is_finished());
        slot.pause_before_snapshot_delete
            .store(false, Ordering::Release);
        let response = preview_task.await.expect("preview task");
        let body = body_of(response).await;
        assert_eq!(body["detail"], detail);
        let retired = Arc::new(AtomicBool::new(false));
        let retired_flag = Arc::clone(&retired);
        let slot_for_retire = Arc::clone(&slot);
        let retire_thread = std::thread::spawn(move || {
            slot_for_retire.retire().expect("checked import retirement");
            retired_flag.store(true, Ordering::Release);
        });
        retire_thread.join().expect("retire");
        assert!(retired.load(Ordering::Acquire));
        assert!(fresh_preview_directory(&root).is_none());
        assert!(
            read_import_metadata(&root, ID)
                .expect("metadata")
                .get("preview")
                .is_none()
        );
        assert_eq!(fake.live.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn a_malformed_size_deletes_the_fresh_preview_on_the_worker_before_the_capture_drops() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        *fake.preview_stdout.lock().expect("stdout") = Some("not json".to_owned());
        refused_size_deletes_on_the_worker(
            fake,
            data,
            "the importer did not return a size preview",
        )
        .await;
    }

    #[tokio::test]
    async fn a_nonzero_size_deletes_the_fresh_preview_on_the_worker_before_the_capture_drops() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let stderr = "size failed".to_owned();
        *fake.preview_stderr.lock().expect("stderr") = Some(stderr.clone());
        refused_size_deletes_on_the_worker(fake, data, &stderr).await;
    }

    #[tokio::test]
    async fn cancelling_prepare_embedded_start_signals_cleanup_without_deleting() {
        let (root, data, _) = fixture();
        let fake = Fake::new(root.path().to_path_buf());
        let slot = installed(&fake);
        let _clear = ClearSnapshotPauses(Arc::clone(&slot));
        slot.pause_before_validation.store(true, Ordering::Release);
        slot.pause_before_snapshot_delete
            .store(true, Ordering::Release);
        let root_path = root.path().to_path_buf();
        let prepare_slot = Arc::clone(&slot);
        let prepare = tokio::spawn(async move {
            preview::prepare_embedded_start(&root_path, data, prepare_slot).await
        });
        let slot_wait = Arc::clone(&slot);
        tokio::task::spawn_blocking(move || {
            spin_until(|| slot_wait.validation_waiting.load(Ordering::Acquire));
        })
        .await
        .expect("validation pause");
        prepare.abort();
        match prepare.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(_) => panic!("prepare should be cancelled"),
        }
        let directory = fresh_preview_directory(root.path()).expect("fresh preview");
        let retired = Arc::new(AtomicBool::new(false));
        let retired_flag = Arc::clone(&retired);
        let slot_for_retire = Arc::clone(&slot);
        let retire_thread = std::thread::spawn(move || {
            slot_for_retire.retire().expect("checked import retirement");
            retired_flag.store(true, Ordering::Release);
        });
        let second = tokio::spawn(async { true });
        assert!(second.await.expect("second task"));
        for _ in 0..SPIN_CAP {
            assert!(!retired.load(Ordering::Acquire));
            assert!(!slot.snapshot_delete_waiting.load(Ordering::Acquire));
            assert!(directory.is_dir());
            assert!(fake.live.load(Ordering::Acquire) > 0);
            std::thread::yield_now();
        }
        slot.pause_before_validation.store(false, Ordering::Release);
        let slot_wait = Arc::clone(&slot);
        tokio::task::spawn_blocking(move || {
            spin_until(|| slot_wait.snapshot_delete_waiting.load(Ordering::Acquire));
        })
        .await
        .expect("cleanup pause");
        for _ in 0..SPIN_CAP {
            assert!(!retired.load(Ordering::Acquire));
            assert!(directory.is_dir());
            assert!(fake.live.load(Ordering::Acquire) > 0);
            std::thread::yield_now();
        }
        slot.pause_before_snapshot_delete
            .store(false, Ordering::Release);
        retire_thread.join().expect("retire");
        assert!(retired.load(Ordering::Acquire));
        assert!(!directory.exists());
        assert_eq!(fake.live.load(Ordering::Acquire), 0);
        assert!(
            read_import_metadata(root.path(), ID)
                .expect("metadata")
                .get("preview")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_tokenless_settings_change_refuses_before_the_owner_date_and_deletes_after_the_response()
     {
        let (root, mut data, metadata) = fixture();
        let file_path = metadata.get("file_path").cloned();
        choose_owner_date(&mut data);
        let fake = Fake::new(root.path().to_path_buf());
        fake.block_execution.store(true, Ordering::Release);
        let slot = installed(&fake);
        let _clear = ClearSnapshotPauses(Arc::clone(&slot));
        slot.pause_before_snapshot_delete
            .store(true, Ordering::Release);
        let state = AppState {
            root: root.path().to_path_buf(),
            route: Some(Arc::clone(&slot)),
        };
        let started = tokio::spawn(lifecycle::start(State(state), Json(data)));
        let fake_wait = Arc::clone(&fake);
        tokio::task::spawn_blocking(move || {
            spin_until(|| fake_wait.capture_entered.load(Ordering::Acquire));
        })
        .await
        .expect("execution capture");
        let mut changed = read_import_metadata(root.path(), ID).expect("metadata");
        changed.insert("setting".to_owned(), json!("home"));
        write_import_metadata(root.path(), ID, &changed).expect("setting");
        fake.capture_release.store(true, Ordering::Release);
        let response = started.await.expect("start");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = body_of(response).await;
        assert_eq!(body["detail"], "the import settings changed; preview again");
        spin_until(|| slot.snapshot_delete_waiting.load(Ordering::Acquire));
        let directory = fresh_preview_directory(root.path()).expect("fresh preview");
        assert!(directory.is_dir());
        assert!(fake.live.load(Ordering::Acquire) > 0);
        assert!(root.path().join("imports").join(ID).is_dir());
        assert!(!root.path().join("imports").join(CHOSEN).exists());
        let stored = read_import_metadata(root.path(), ID).expect("metadata");
        assert!(stored.get("owner_timestamp").is_none());
        assert_eq!(stored.get("file_path"), file_path.as_ref());
        assert!(stored.get("attempt").is_none());
        assert!(stored.get("preview").is_none());
        let retired = Arc::new(AtomicBool::new(false));
        let retired_flag = Arc::clone(&retired);
        let slot_for_retire = Arc::clone(&slot);
        let retire_thread = std::thread::spawn(move || {
            slot_for_retire.retire().expect("checked import retirement");
            retired_flag.store(true, Ordering::Release);
        });
        for _ in 0..SPIN_CAP {
            assert!(!retired.load(Ordering::Acquire));
            std::thread::yield_now();
        }
        assert!(directory.is_dir());
        assert!(fake.live.load(Ordering::Acquire) > 0);
        slot.pause_before_snapshot_delete
            .store(false, Ordering::Release);
        retire_thread.join().expect("retire");
        assert!(retired.load(Ordering::Acquire));
        assert!(!directory.exists());
        assert!(
            read_import_metadata(root.path(), ID)
                .expect("metadata")
                .get("preview")
                .is_none()
        );
    }
}
