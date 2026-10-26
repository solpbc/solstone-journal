// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One retained foreground consumer of the shared importer grammar and worker.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use solstone_core_generate::{ClientError, GenerateExecutor, GenerateRequest, GenerateResponse};
use solstone_core_import::cli_render::CliRun;
use solstone_core_import::{WorkStop, WorkStopGuard};
use solstone_core_import_host::call::{self, ArgumentClass, BodyAdmission, ImportSeams};
use solstone_core_import_sources::archive::FullReindexRequester;
use solstone_core_import_sources::document::PdfWorker;
use solstone_core_journal_config::work_priority::WorkPriority;
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::whole_journal_restore::{
    TargetUse, begin_target_use_bound, whole_journal_restore_blocks,
};
use solstone_core_system::queue::CallableTaskContext;

use super::{Checkpoint, HostGenerator, HostedCommand, ImportFactorySlot, UNCOMPLETED_HOLD_EXIT};

type StopCheckpoint = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// An admitted owner request. Capture it before queue acceptance, then prepare
/// and execute it only on the retained worker. Its authority cannot be replaced.
///
/// ```compile_fail
/// use journal_mobile_serving::foreground::ImportRequest;
/// fn replace_stop(request: &mut ImportRequest) {
///     request.stop = Default::default();
/// }
/// ```
pub struct ImportRequest {
    context: ImportContext,
    factory: Arc<ImportSeamFactory>,
    lease: Arc<TargetUse>,
}

/// Retain an admitted request through outer metadata settlement. This handle
/// carries no execution inputs or native resources and cannot admit new work.
#[derive(Clone)]
pub struct ImportKeepalive {
    _lease: Arc<TargetUse>,
}

impl ImportRequest {
    /// Capture before moving the request to its retained worker. Drop after
    /// that worker and the request's outer metadata work have both retired.
    pub fn keepalive(&self) -> ImportKeepalive {
        ImportKeepalive {
            _lease: Arc::clone(&self.lease),
        }
    }

    /// Run native preparation on the retained worker. The request already owns
    /// its target use, including time spent waiting in that worker's queue.
    pub fn prepare(self) -> Result<PreparedImport, String> {
        let Self {
            context,
            factory,
            lease,
        } = self;
        if context.stop.is_requested() {
            return Err("import stopped".into());
        }
        let root = Arc::clone(&context.root);
        let stop = context.stop.clone();
        let class = context.class.clone();
        let _work = WorkStopGuard::install(&stop);
        let mut prepared = factory(context)?;
        if stop.is_requested() {
            return Err("import stopped".into());
        }
        validate_resolved(&prepared.args, &class)?;
        prepared.bound = Some(BoundImport {
            root,
            stop,
            class,
            _lease: lease,
        });
        Ok(prepared)
    }
}

/// Read-only native preparation context. Only the installed native factory
/// receives this view; an ordinary request cannot extract its execution seams.
pub struct ImportContext {
    root: Arc<JournalRoot>,
    args: Vec<String>,
    class: ArgumentClass,
    generator: Arc<dyn GenerateExecutor + Send + Sync>,
    stop: WorkStop,
}

impl ImportContext {
    pub fn root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn arguments(&self) -> &[String] {
        &self.args
    }
    pub fn classification(&self) -> &ArgumentClass {
        &self.class
    }
    pub fn stop(&self) -> &WorkStop {
        &self.stop
    }

    /// Reuse the host's Generate authority for text, image and document calls.
    /// The native adapter supplies its admitted in-process PDF implementation.
    pub fn seams_with_pdf(&self, pdf: Box<dyn PdfWorker + Send>) -> ImportSeams {
        let generate = ImportGenerate {
            root: Arc::clone(&self.root),
            executor: Arc::clone(&self.generator),
            stop: self.stop.clone(),
        };
        ImportSeams {
            text: Box::new(generate.clone()),
            image: Box::new(generate.clone()),
            model: Box::new(generate),
            pdf,
            reindex: Box::new(ImportReindex {
                root: Arc::clone(&self.root),
                stop: self.stop.clone(),
            }),
        }
    }
}

/// Resolved native inputs. Resource guards stay on the original callable
/// worker until the import handle has joined, including cancellation/unwind.
/// They need not be Send: only ImportSeams crosses onto the import thread.
///
/// ```compile_fail
/// use journal_mobile_serving::foreground::PreparedImport;
/// fn extract_seams(prepared: &mut PreparedImport) {
///     let _ = &mut prepared.seams;
/// }
/// ```
///
/// ```compile_fail
/// use journal_mobile_serving::foreground::PreparedImport;
/// fn require_send<T: Send>() {}
/// require_send::<PreparedImport>();
/// ```
pub struct PreparedImport {
    args: Vec<String>,
    seams: ImportSeams,
    resources: Vec<Box<dyn Any>>,
    bound: Option<BoundImport>,
}

struct BoundImport {
    root: Arc<JournalRoot>,
    stop: WorkStop,
    class: ArgumentClass,
    _lease: Arc<TargetUse>,
}

// Fields retire in declaration order on every exit, including start failure.
// Native grant cleanup remains inside the admitted target use.
struct ImportCustody {
    _resources: Vec<Box<dyn Any>>,
    bound: BoundImport,
}

impl PreparedImport {
    pub fn new(args: Vec<String>, seams: ImportSeams) -> Self {
        Self {
            args,
            seams,
            resources: Vec::new(),
            bound: None,
        }
    }

    pub fn with_resources<T: 'static>(mut self, resources: T) -> Self {
        self.resources.push(Box::new(resources));
        self
    }

    /// Read-only shared preview under the same owner/checkpoint and resources.
    pub fn preview(&self) -> CliRun {
        let Some(bound) = &self.bound else {
            return refused("importer unavailable");
        };
        if bound.stop.is_requested() {
            return refused("import stopped");
        }
        if let Err(error) = validate_resolved(&self.args, &bound.class) {
            return refused(&error);
        }
        let _work = WorkStopGuard::install(&bound.stop);
        call::preview(&self.args, bound.root.canonical_path(), &self.seams)
    }

    /// Start and join on this retained worker. Neither the request lease nor
    /// native resource guards are released while an import frame is live.
    pub fn run(self) -> std::io::Result<(CliRun, Option<String>)> {
        let bound = self
            .bound
            .as_ref()
            .ok_or_else(|| std::io::Error::other("importer unavailable"))?;
        if bound.stop.is_requested() {
            return Ok((refused("import stopped"), None));
        }
        if let Err(error) = validate_resolved(&self.args, &bound.class) {
            return Ok((refused(&error), None));
        }
        let Self {
            args,
            seams,
            resources,
            bound,
        } = self;
        let custody = ImportCustody {
            _resources: resources,
            bound: bound.expect("bound import checked before moving its resources"),
        };
        let handle = call::start(
            args,
            custody.bound.root.canonical_path().to_path_buf(),
            seams,
            custody.bound.stop.clone(),
        )?;
        let result = handle.join();
        // Named owner, explicitly retired only after the native join.
        drop(custody);
        Ok(result)
    }
}

/// Installed per foreground owner. Native folder grants, configuration and PDF
/// admission belong to this preparation boundary; there is no global factory.
pub type ImportSeamFactory = dyn Fn(ImportContext) -> Result<PreparedImport, String> + Send + Sync;

pub(super) fn admit(args: &[String]) -> bool {
    call::classify_arguments(args).is_ok_and(|class| class.body != BodyAdmission::Held)
}

pub(super) fn request(
    root: Arc<JournalRoot>,
    generator: &HostGenerator,
    imports: &ImportFactorySlot,
    host_cancelled: Checkpoint,
    args: Vec<String>,
    original: WorkStop,
    priority: WorkPriority,
) -> Result<ImportRequest, String> {
    let class = call::classify_arguments(&args)?;
    if class.body == BodyAdmission::Held {
        return Err("this import is unavailable".into());
    }
    if original.is_requested() || host_cancelled() {
        return Err("import stopped".into());
    }
    let lease = Arc::new(
        begin_target_use_bound(Arc::clone(&root))
            .map_err(|_| "your journal is unavailable".to_owned())?,
    );
    let factory = imports
        .lock()
        .map_err(|_| "importer unavailable")?
        .as_ref()
        .cloned()
        .ok_or("importer unavailable")?;
    let stop = {
        let root = Arc::clone(&root);
        WorkStop::linked_with_checkpoint(Arc::new(AtomicBool::new(false)), None, move || {
            original.is_requested()
                || host_cancelled()
                || root.revalidate_canonical_binding().is_err()
                || whole_journal_restore_blocks(root.canonical_path())
        })
    };
    if stop.is_requested() {
        return Err("import stopped".into());
    }
    let cancelled: Checkpoint = {
        let stop = stop.clone();
        Arc::new(move || stop.is_requested())
    };
    Ok(ImportRequest {
        context: ImportContext {
            root,
            args,
            class,
            generator: generator.executor(cancelled, priority),
            stop,
        },
        factory,
        lease,
    })
}

fn validate_resolved(args: &[String], expected: &ArgumentClass) -> Result<(), String> {
    let resolved = call::classify_arguments(args)?;
    if resolved.body != expected.body || resolved.sync_source != expected.sync_source {
        return Err("this import could not start".into());
    }
    if matches!(resolved.sync_source.as_deref(), Some("obsidian" | "audio"))
        && resolved.explicit_path.is_none()
    {
        return Err("source connection is unavailable".into());
    }
    Ok(())
}

fn refused(message: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{message}\n"),
        exit_code: 1,
    }
}

pub(super) fn execute(
    hosted: &HostedCommand,
    args: &[String],
    control: &CallableTaskContext,
    checkpoint: StopCheckpoint,
) -> i32 {
    let stop = WorkStop::linked_with_checkpoint(
        Arc::clone(&control.cancelled),
        Some(control.deadline),
        move || checkpoint().is_err(),
    );
    if stop.is_requested() {
        return 1;
    }
    let prepared = match request(
        Arc::clone(&hosted.root),
        &hosted.generator,
        &hosted.imports,
        Arc::clone(&hosted.cancelled),
        args.to_vec(),
        stop.clone(),
        control.priority,
    )
    .and_then(ImportRequest::prepare)
    {
        Ok(prepared) => prepared,
        Err(error) => {
            log::warn!("foreground import preparation: {error}");
            return if stop.is_requested() {
                1
            } else {
                UNCOMPLETED_HOLD_EXIT
            };
        }
    };
    let (run, _import_id) = match prepared.run() {
        Ok(result) => result,
        Err(error) => {
            log::warn!("foreground import start: {error}");
            return 1;
        }
    };
    if !run.stdout.is_empty() {
        log::info!("{}", run.stdout.trim_end());
    }
    if !run.stderr.is_empty() {
        log::warn!("{}", run.stderr.trim_end());
    }
    // The shared worker owns the terminal facts. A Stop arriving after a
    // successful commit must not rewrite that actual completed outcome.
    run.exit_code
}

#[derive(Clone)]
struct ImportGenerate {
    root: Arc<JournalRoot>,
    executor: Arc<dyn GenerateExecutor + Send + Sync>,
    stop: WorkStop,
}

impl ImportGenerate {
    fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        if self.stop.is_requested() || self.root.revalidate_canonical_binding().is_err() {
            return Ok(GenerateResponse::Cancelled);
        }
        let result = self.executor.execute(request);
        if self.stop.is_requested() || self.root.revalidate_canonical_binding().is_err() {
            Ok(GenerateResponse::Cancelled)
        } else {
            result
        }
    }
}

impl solstone_core_import::text::WireClient for ImportGenerate {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.generate(request)
    }
}
impl solstone_core_import_sources::image::WireClient for ImportGenerate {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.generate(request)
    }
}
impl solstone_core_import_sources::document::DocumentModelClient for ImportGenerate {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.generate(request)
    }
}

struct ImportReindex {
    root: Arc<JournalRoot>,
    stop: WorkStop,
}
impl FullReindexRequester for ImportReindex {
    fn request_full_reindex(&self) -> Result<bool, String> {
        if self.stop.is_requested() {
            return Err("import stopped".into());
        }
        self.root
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        solstone_core_import_host::registry::IndexScan {
            journal: self.root.canonical_path().to_path_buf(),
        }
        .request_full_reindex()
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "import_commands_tests.rs"]
mod tests;
