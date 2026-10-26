// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Explicit iOS linked backup authority. Only normal app startup installs build
//! and private-container authority; every invocation separately captures a root.

mod archive;
mod build_identity;
mod bundle;
mod command;
pub(crate) mod native;
mod outcome;
pub mod repository;
mod root;
mod storage;
pub mod teardown;

pub use root::IosRootToolRunner;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use solstone_core_backup::Destination;
use solstone_core_backup::archive_admission::ArchiveCancellation;
use solstone_core_journal_io::JournalRoot;
use zeroize::Zeroizing;

use crate::engine::ClosedToolError;
use crate::repo::RepoError;
use crate::runner::{PassedHandle, ResticResult, RunnerError, guard_argv};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Bundle,
    Build,
    Storage,
    Conflict,
    Root,
    Drain,
}

impl Refusal {
    fn code(self) -> &'static str {
        match self {
            Self::Bundle => "ios_backup_bundle_refused",
            Self::Build => "ios_backup_linked_build_refused",
            Self::Storage => "ios_backup_private_storage_refused",
            Self::Conflict => "ios_backup_installation_conflict",
            Self::Root => "ios_backup_root_changed",
            Self::Drain => "ios_backup_native_drain_unconfirmed",
        }
    }
}

struct Installed {
    bundle: bundle::Bundle,
    storage: storage::Storage,
}

impl Installed {
    fn revalidate(&self) -> Result<(), Refusal> {
        if native::drain_failed() {
            return Err(Refusal::Drain);
        }
        self.bundle.revalidate()?;
        self.storage.revalidate()
    }

    fn same_installation(&self, other: &Self) -> bool {
        self.bundle.same_installation(&other.bundle)
            && self.storage.same_installation(&other.storage)
    }
}

static INSTALLED: OnceLock<Arc<Installed>> = OnceLock::new();

/// Called by the app's fixed startup FFI, never by journal owner IPC. A failed
/// attempt installs nothing. There is no reset, caller path or replacement API.
pub fn admit_ios_backup_at_startup() -> Result<(), &'static str> {
    let install = || -> Result<(), Refusal> {
        let candidate = Arc::new(Installed {
            bundle: bundle::Bundle::discover()?,
            storage: storage::Storage::create_private_storage()?,
        });
        candidate.revalidate()?;
        match INSTALLED.set(Arc::clone(&candidate)) {
            Ok(()) => Ok(()),
            Err(_) => {
                let installed = INSTALLED.get().ok_or(Refusal::Conflict)?;
                installed.revalidate()?;
                if installed.same_installation(&candidate) {
                    Ok(())
                } else {
                    Err(Refusal::Conflict)
                }
            }
        }
    };
    install().map_err(Refusal::code)
}

pub struct AdmittedIosBackupAuthority {
    installed: Arc<Installed>,
    root: Arc<JournalRoot>,
}

impl std::fmt::Debug for AdmittedIosBackupAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmittedIosBackupAuthority")
            .finish_non_exhaustive()
    }
}

impl AdmittedIosBackupAuthority {
    fn revalidate(&self) -> Result<(), Refusal> {
        self.installed.revalidate()?;
        self.root
            .revalidate_canonical_binding()
            .map_err(|_| Refusal::Root)
    }

    /// The installed private-storage authority for this process. A replacement
    /// install is a different generation and cannot finish an earlier call.
    fn generation(&self) -> u64 {
        Arc::as_ptr(&self.installed) as u64
    }

    fn open_destination_state(&self, namespace: &str) -> Result<storage::Directory, Refusal> {
        self.revalidate()?;
        self.installed.storage.create_state(namespace)
    }
}

/// Retained custody of the app's journal root as the target of one whole-journal
/// restore. The native activity lease lasts until this value drops, covering
/// every later step of the restore, not only the native copy.
pub struct RestoreTarget {
    target: native::Target,
    root: Arc<JournalRoot>,
    path: String,
}

impl std::fmt::Debug for RestoreTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RestoreTarget")
            .finish_non_exhaustive()
    }
}

impl RestoreTarget {
    pub fn admit(root: Arc<JournalRoot>) -> io::Result<Self> {
        root.revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let path = root
            .canonical_path()
            .to_str()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "restore target path is not UTF-8",
                )
            })?
            .to_owned();
        let target = native::Target::admit(root.as_fd(), &path)
            .map_err(|_| io::Error::other("restore target admission refused"))?;
        let admitted = Self { target, root, path };
        admitted.revalidate()?;
        Ok(admitted)
    }

    pub fn revalidate(&self) -> io::Result<()> {
        self.target
            .revalidate()
            .map_err(|_| io::Error::other("restore target binding changed"))?;
        self.root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)
    }

    pub fn path(&self) -> &std::path::Path {
        self.root.canonical_path()
    }
}

pub fn admitted_ios_backup(
    root: Arc<JournalRoot>,
) -> Result<Arc<AdmittedIosBackupAuthority>, ClosedToolError> {
    let unavailable = |detail: &str| ClosedToolError::ResticUnavailable {
        detail: detail.to_owned(),
        guidance: solstone_core_installed_payload::guidance::PACKAGE_MISMATCH.to_owned(),
    };
    let installed = INSTALLED
        .get()
        .ok_or_else(|| unavailable("ios_backup_startup_admission_missing"))?;
    let authority = Arc::new(AdmittedIosBackupAuthority {
        installed: Arc::clone(installed),
        root,
    });
    authority
        .revalidate()
        .map_err(|reason| unavailable(reason.code()))?;
    Ok(authority)
}

fn refused(reason: &'static str) -> RunnerError {
    RunnerError::Process(io::Error::new(io::ErrorKind::Unsupported, reason))
}

#[derive(Debug)]
pub enum ExecuteError {
    Admission(RunnerError),
    Entry(i32),
}

impl From<RunnerError> for ExecuteError {
    fn from(error: RunnerError) -> Self {
        Self::Admission(error)
    }
}

impl ExecuteError {
    fn into_runner(self) -> RunnerError {
        match self {
            Self::Admission(error) => error,
            Self::Entry(status) => RunnerError::Process(io::Error::other(format!(
                "ios_backup_native_entry_refused:status={status}"
            ))),
        }
    }

    fn into_repo(self) -> RepoError {
        match self {
            Self::Entry(status) if (1..=11).contains(&status) => {
                RepoError::Callable(crate::repo::IosCallableFailure {
                    native_status: status,
                    diagnostic: "ENTRY_REFUSED".to_owned(),
                    key_id: None,
                    snapshot_id: None,
                    cancellation_requested: false,
                    deadline_requested: false,
                    monitor_failed: false,
                })
            }
            _ => RepoError::Failed,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_call<'a>(
    authority: &AdmittedIosBackupAuthority,
    args: &[String],
    repository: &'a str,
    password: &str,
    next_password: Option<&str>,
    env: Option<&'a BTreeMap<String, Option<String>>>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<repository::Repository<'a>, RunnerError> {
    let validated = validate_call_shape(
        authority,
        args,
        repository,
        password,
        next_password,
        env,
        cancellation,
    );
    if let Err(error) = &validated {
        // Every refusal reason here is a static code, never built from inputs.
        let reason = std::error::Error::source(error).map(ToString::to_string);
        log::warn!(
            "ios backup call refused before the bridge: {error}: {}",
            reason.as_deref().unwrap_or("")
        );
    }
    validated
}

fn validate_call_shape<'a>(
    authority: &AdmittedIosBackupAuthority,
    args: &[String],
    repository: &'a str,
    password: &str,
    next_password: Option<&str>,
    env: Option<&'a BTreeMap<String, Option<String>>>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<repository::Repository<'a>, RunnerError> {
    authority
        .revalidate()
        .map_err(|reason| refused(reason.code()))?;
    if password.is_empty()
        || password.len() > 256
        || password.contains('\0')
        || next_password
            .is_some_and(|next| next.is_empty() || next.len() > 256 || next.contains('\0'))
        || args.len() > 131080
        || args
            .iter()
            .try_fold(0usize, |sum, arg| sum.checked_add(arg.len()))
            .is_none_or(|sum| sum > 8 * 1024 * 1024 + 16384)
    {
        return Err(refused("ios_backup_request_refused"));
    }
    // Parse borrowed inputs and enforce all credential caps before copying any
    // configured secret into the argv guard's zeroizing storage.
    let repository =
        repository::parse(repository, env).map_err(|_| refused("ios_backup_repository_refused"))?;
    let mut secrets = Zeroizing::new(vec![password.to_owned()]);
    if let Some(next) = next_password {
        secrets.push(next.to_owned());
    }
    if let Some(env) = env {
        for key in [
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
        ] {
            if let Some(Some(value)) = env.get(key) {
                secrets.push(value.clone());
            }
        }
    }
    guard_argv(args, &secrets)?;
    if cancellation.is_some_and(ArchiveCancellation::is_cancelled) {
        return Err(RunnerError::Process(io::Error::new(
            io::ErrorKind::Interrupted,
            "ios_backup_cancelled_before_entry",
        )));
    }
    Ok(repository)
}

fn policy(
    authority: &AdmittedIosBackupAuthority,
    operation: &command::Operation,
) -> Result<Vec<u8>, RunnerError> {
    let command::Operation::Snapshot {
        source,
        exclusions,
        exclude_file,
    } = operation
    else {
        return Ok(Vec::new());
    };
    if source.as_os_str() != authority.root.canonical_path().as_os_str() {
        return Err(refused("ios_backup_snapshot_root_mismatch"));
    }
    let mut patterns = exclusions.clone();
    if let Some(path) = exclude_file {
        let parent = JournalRoot::open(
            path.parent()
                .ok_or_else(|| refused("ios_backup_exclude_file_refused"))?,
        )
        .map_err(|_| refused("ios_backup_exclude_file_refused"))?;
        parent
            .revalidate_canonical_binding()
            .map_err(|_| refused("ios_backup_exclude_file_refused"))?;
        let (file, bytes) = bundle::read_bound(
            &parent,
            path.file_name().unwrap_or(OsStr::new("")),
            8 * 1024 * 1024,
        )
        .map_err(|_| refused("ios_backup_exclude_file_refused"))?;
        let metadata = file.metadata().map_err(RunnerError::Process)?;
        if metadata.mode() & 0o7777 != 0o600 || metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err(refused("ios_backup_exclude_file_refused"));
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| refused("ios_backup_exclude_file_refused"))?;
        if text.contains(['\0', '\r']) {
            return Err(refused("ios_backup_exclude_file_refused"));
        }
        for line in text.lines() {
            patterns.push(
                command::relative_exclusion(source, line, true)
                    .map_err(|_| refused("ios_backup_exclude_file_refused"))?,
            );
            if patterns.len() > 65536 {
                return Err(refused("ios_backup_exclusion_limit"));
            }
        }
    }
    if patterns.len() > 65536 {
        return Err(refused("ios_backup_exclusion_limit"));
    }
    let bytes = serde_json::to_vec(&patterns).map_err(|_| refused("ios_backup_exclusion_limit"))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(refused("ios_backup_exclusion_limit"));
    }
    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
fn execute(
    authority: &AdmittedIosBackupAuthority,
    operation: i32,
    source: bool,
    repository: repository::Repository<'_>,
    password: &str,
    next_password: &str,
    selection: &[u8],
    options: &native::Options,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
    metadata: bool,
) -> Result<native::Completed, ExecuteError> {
    let password = Zeroizing::new(password.as_bytes().to_vec());
    let next_password = Zeroizing::new(next_password.as_bytes().to_vec());
    let workspace = if source {
        Some(
            authority
                .installed
                .storage
                .create_workspace()
                .map_err(|reason| refused(reason.code()))?,
        )
    } else {
        None
    };
    let private_workspace = workspace
        .as_ref()
        .map(|workspace| native::PrivateDirectory {
            path: workspace.path(),
            descriptor: workspace.as_fd(),
        });
    let snapshot_source = if source {
        Some(native::Source {
            path: authority
                .root
                .canonical_path()
                .to_str()
                .ok_or_else(|| refused("ios_backup_root_encoding_refused"))?,
            descriptor: authority.root.as_fd(),
        })
    } else {
        None
    };
    authority
        .revalidate()
        .map_err(|reason| refused(reason.code()))?;
    let state;
    let call = match repository {
        repository::Repository::Local(repository) => native::Call::local(
            operation,
            &password,
            &next_password,
            repository,
            snapshot_source,
            private_workspace,
            selection,
            options,
        ),
        repository::Repository::S3 {
            destination,
            access,
            secret,
            token,
            append_only,
        } => {
            state = authority
                .installed
                .storage
                .create_state(&destination.namespace())
                .map_err(|reason| refused(reason.code()))?;
            state
                .revalidate()
                .map_err(|reason| refused(reason.code()))?;
            let remote = native::Remote {
                endpoint: &destination.endpoint,
                region: &destination.region,
                bucket: &destination.bucket,
                prefix: &destination.prefix,
                access: access.as_bytes(),
                secret: secret.as_bytes(),
                token: token.as_bytes(),
                state: native::PrivateDirectory {
                    path: state.path(),
                    descriptor: state.as_fd(),
                },
            };
            // Options version 2 carries the create-only bit for an append-only
            // (operated) destination; owner storage keeps ordinary writes.
            let options = native::Options {
                version: if append_only { 2 } else { options.version },
                reserved1: u32::from(append_only),
                ..*options
            };
            native::Call::remote(
                operation,
                &password,
                &next_password,
                &remote,
                snapshot_source,
                private_workspace,
                selection,
                &options,
            )
        }
    }
    .map_err(|status| {
        log::warn!("ios backup operation {operation} refused at entry: status {status}");
        ExecuteError::Entry(status)
    })?;
    let cancelled = || cancellation.is_some_and(ArchiveCancellation::is_cancelled);
    // All surrounding captured authorities, buffers, directories and the actual
    // caller's lifetime remain live until actual wait/result/free returns.
    let completed = call.wait(timeout, Some(&cancelled), metadata);
    log_failure(operation, &completed);
    Ok(completed)
}

// The bridge's diagnostic is a bounded code, never a credential or path.
/// Open the admitted repository and return its config id. This is qualification
/// only: it does not archive, list, or download.
pub(crate) fn open_repository_id(
    authority: &AdmittedIosBackupAuthority,
    repository: &str,
    password: &str,
    env: Option<&BTreeMap<String, Option<String>>>,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<String, crate::engine::ArchiveError> {
    if cancellation.is_some_and(ArchiveCancellation::is_cancelled) {
        return Err(crate::engine::ArchiveError::Cancelled);
    }
    let parsed = repository::parse(repository, env)
        .map_err(|_| crate::engine::ArchiveError::ConfigurationUnavailable)?;
    authority
        .revalidate()
        .map_err(|_| crate::engine::ArchiveError::ConfigurationChanged)?;
    let options = native::Options {
        version: 1,
        check_read_mode: 0,
        retry_lock_ms: 0,
        group: 0,
        groups: 0,
        reserved1: 0,
        reserved2: 0,
    };
    let completed = execute(
        authority,
        2,
        false,
        parsed,
        password,
        "",
        &[],
        &options,
        timeout,
        cancellation,
        true,
    )
    .map_err(|_| crate::engine::ArchiveError::ToolFailed { reason: "failed" })?;
    if completed.cancellation_requested || completed.fields.status == 3 {
        return Err(crate::engine::ArchiveError::Cancelled);
    }
    if completed.deadline_requested {
        return Err(crate::engine::ArchiveError::DeadlineExceeded);
    }
    if completed.monitor_failed || completed.fields.status != 0 {
        return Err(crate::engine::ArchiveError::ToolFailed { reason: "failed" });
    }
    authority
        .revalidate()
        .map_err(|_| crate::engine::ArchiveError::ConfigurationChanged)?;
    let bytes = completed
        .metadata
        .as_ref()
        .ok()
        .and_then(Option::as_deref)
        .ok_or(crate::engine::ArchiveError::RepositoryIdentityInvalid)?;
    outcome::repository_config_id(bytes)
        .map_err(|_| crate::engine::ArchiveError::RepositoryIdentityInvalid)
}

fn log_failure(operation: i32, completed: &native::Completed) {
    if completed.fields.status == 0 {
        return;
    }
    let end = completed
        .fields
        .diagnostic
        .iter()
        .position(|byte| *byte == 0);
    let diagnostic = std::str::from_utf8(&completed.fields.diagnostic[..end.unwrap_or(0)])
        .unwrap_or("unprintable");
    log::warn!(
        "ios backup operation {operation} failed: status {} diagnostic {diagnostic}",
        completed.fields.status
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    authority: &AdmittedIosBackupAuthority,
    args: &[String],
    repository: &str,
    password: &str,
    env: Option<&BTreeMap<String, Option<String>>>,
    json: bool,
    max_repack_size: Option<&str>,
    timeout: Option<Duration>,
    pass_fds: &[PassedHandle<'_>],
    stdin: Option<Vec<u8>>,
    cancellation: Option<&ArchiveCancellation>,
    restore_target: Option<&RestoreTarget>,
) -> Result<ResticResult, RunnerError> {
    if max_repack_size.is_some() || !pass_fds.is_empty() || stdin.is_some() {
        return Err(refused("ios_backup_unsupported_process_input"));
    }
    let repository = validate_call(
        authority,
        args,
        repository,
        password,
        None,
        env,
        cancellation,
    )?;
    let command = command::decode(args).map_err(|_| refused("ios_backup_unsupported_command"))?;
    if let command::Operation::Restore {
        snapshot,
        selector,
        target,
    } = &command.operation
    {
        let completed = restore(
            authority,
            repository,
            password,
            restore_target.ok_or_else(|| refused("ios_backup_restore_target_missing"))?,
            target,
            snapshot,
            selector,
            timeout,
            cancellation,
        )?;
        return outcome::project(Some(&command.operation), completed, args, json);
    }
    let selection = policy(authority, &command.operation)?;
    let mut options = native::Options {
        version: 1,
        check_read_mode: 0,
        retry_lock_ms: command.retry_lock_millis,
        group: 0,
        groups: 0,
        reserved1: 0,
        reserved2: 0,
    };
    let operation = match &command.operation {
        command::Operation::Init => 1,
        command::Operation::Open => 2,
        command::Operation::Snapshot { .. } => 4,
        command::Operation::ListSnapshots => 5,
        command::Operation::Restore { .. } => unreachable!("restore dispatched above"),
        command::Operation::Check(mode) => {
            // The native check modes: structure 1, all data 2, one data group 3.
            match mode {
                command::CheckRead::Structure => options.check_read_mode = 1,
                command::CheckRead::All => options.check_read_mode = 2,
                command::CheckRead::Group { group, groups } => {
                    options.check_read_mode = 3;
                    options.group = *group;
                    options.groups = *groups;
                }
            }
            7
        }
    };
    let metadata = matches!(
        command.operation,
        command::Operation::Open | command::Operation::ListSnapshots
    );
    let completed = execute(
        authority,
        operation,
        operation == 4,
        repository,
        password,
        "",
        &selection,
        &options,
        timeout,
        cancellation,
        metadata,
    )
    .map_err(ExecuteError::into_runner)?;
    outcome::project(Some(&command.operation), completed, args, json)
}

#[allow(clippy::too_many_arguments)]
fn restore(
    authority: &AdmittedIosBackupAuthority,
    repository: repository::Repository<'_>,
    password: &str,
    admitted: &RestoreTarget,
    target: &std::path::Path,
    snapshot: &str,
    selector: &str,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<native::Completed, RunnerError> {
    // The requested target must be exactly the admitted root, which must be the
    // backup authority's own retained journal.
    if target.as_os_str() != admitted.path.as_str()
        || admitted.root.canonical_path() != authority.root.canonical_path()
    {
        return Err(refused("ios_backup_restore_target_mismatch"));
    }
    // Restore writes only its repository lock remotely, so it needs no
    // create-only condition.
    let repository::Repository::S3 {
        destination,
        access,
        secret,
        token,
        ..
    } = repository
    else {
        return Err(refused("ios_backup_restore_local_unsupported"));
    };
    admitted
        .revalidate()
        .map_err(|_| refused("ios_backup_restore_target_changed"))?;
    authority
        .revalidate()
        .map_err(|reason| refused(reason.code()))?;
    let password = Zeroizing::new(password.as_bytes().to_vec());
    let state = authority
        .installed
        .storage
        .create_state(&destination.namespace())
        .map_err(|reason| refused(reason.code()))?;
    state
        .revalidate()
        .map_err(|reason| refused(reason.code()))?;
    let remote = native::Remote {
        endpoint: &destination.endpoint,
        region: &destination.region,
        bucket: &destination.bucket,
        prefix: &destination.prefix,
        access: access.as_bytes(),
        secret: secret.as_bytes(),
        token: token.as_bytes(),
        state: native::PrivateDirectory {
            path: state.path(),
            descriptor: state.as_fd(),
        },
    };
    let call = native::Call::remote_restore(
        &password,
        &remote,
        &admitted.target,
        &admitted.path,
        snapshot,
        selector,
    )
    .map_err(|status| {
        log::warn!("ios backup restore refused at entry: status {status}");
        ExecuteError::Entry(status).into_runner()
    })?;
    let cancelled = || cancellation.is_some_and(ArchiveCancellation::is_cancelled);
    // The target, state directory and credentials stay borrowed until wait/free.
    let completed = call.wait(timeout, Some(&cancelled), true);
    log_failure(6, &completed);
    Ok(completed)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn add_key(
    authority: &AdmittedIosBackupAuthority,
    destination: &Destination,
    password: &str,
    next_password: &str,
    labels: &[String],
    env: &BTreeMap<String, Option<String>>,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<(), RepoError> {
    if !labels.is_empty() {
        return Err(RepoError::Failed);
    }
    let repository = validate_call(
        authority,
        &[],
        &destination.repository,
        password,
        Some(next_password),
        Some(env),
        cancellation,
    )
    .map_err(|_| RepoError::Failed)?;
    let options = native::Options {
        version: 1,
        check_read_mode: 0,
        retry_lock_ms: 0,
        group: 0,
        groups: 0,
        reserved1: 0,
        reserved2: 0,
    };
    let completed = execute(
        authority,
        3,
        false,
        repository,
        password,
        next_password,
        &[],
        &options,
        timeout,
        cancellation,
        false,
    )
    .map_err(ExecuteError::into_repo)?;
    if let Some(failure) = outcome::callable_failure(&completed).map_err(|_| RepoError::Failed)? {
        return Err(RepoError::Callable(failure));
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
pub(crate) enum AddOutcome {
    Added,
    Resumed(String),
}

impl std::fmt::Debug for AddOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Added => write!(f, "Added"),
            Self::Resumed(_) => write!(f, "Resumed([redacted])"),
        }
    }
}

pub(crate) fn add_recovery_key_rotation(
    authority: &AdmittedIosBackupAuthority,
    destination: &Destination,
    daily_key: &str,
    candidate: &str,
    env: &BTreeMap<String, Option<String>>,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<AddOutcome, RepoError> {
    let repository = validate_call(
        authority,
        &[],
        &destination.repository,
        daily_key,
        Some(candidate),
        Some(env),
        cancellation,
    )
    .map_err(|_| RepoError::Failed)?;
    let repository::Repository::S3 {
        destination: s3_dest,
        access,
        secret,
        token,
        append_only,
    } = repository
    else {
        return Err(RepoError::Failed);
    };

    authority
        .revalidate()
        .map_err(|reason| refused(reason.code()))
        .map_err(|_| RepoError::Failed)?;
    let daily_password = Zeroizing::new(daily_key.as_bytes().to_vec());
    let candidate_password = Zeroizing::new(candidate.as_bytes().to_vec());
    let state = authority
        .installed
        .storage
        .create_state(&s3_dest.namespace())
        .map_err(|reason| refused(reason.code()))
        .map_err(|_| RepoError::Failed)?;
    state
        .revalidate()
        .map_err(|reason| refused(reason.code()))
        .map_err(|_| RepoError::Failed)?;
    let remote = native::Remote {
        endpoint: &s3_dest.endpoint,
        region: &s3_dest.region,
        bucket: &s3_dest.bucket,
        prefix: &s3_dest.prefix,
        access: access.as_bytes(),
        secret: secret.as_bytes(),
        token: token.as_bytes(),
        state: native::PrivateDirectory {
            path: state.path(),
            descriptor: state.as_fd(),
        },
    };
    let options = native::Options {
        version: if append_only { 2 } else { 1 },
        check_read_mode: 0,
        retry_lock_ms: 0,
        group: 0,
        groups: 0,
        reserved1: u32::from(append_only),
        reserved2: 0,
    };
    let call = native::Call::remote_add_recovery_key_rotation(
        &daily_password,
        &candidate_password,
        &remote,
        &options,
        1,
    )
    .map_err(|status| {
        log::warn!("ios backup add recovery key rotation refused at entry: status {status}");
        RepoError::Failed
    })?;
    let cancelled = || cancellation.is_some_and(ArchiveCancellation::is_cancelled);
    let completed = call.wait(timeout, Some(&cancelled), true);
    log_failure(3, &completed);

    let is_remote_pending = completed.fields.status == 11
        && outcome::diagnostic(&completed.fields).is_ok_and(|d| d == "REMOTE_PENDING");

    if is_remote_pending {
        if let Ok(Some(bytes)) = completed.metadata
            && let Ok(resumed_pw) = crate::rotation::resume::decode_recovery_add_resume(&bytes)
        {
            return Ok(AddOutcome::Resumed(resumed_pw));
        }
        return Err(RepoError::Failed);
    }

    if let Some(failure) = outcome::callable_failure(&completed).map_err(|_| RepoError::Failed)? {
        return Err(RepoError::Callable(failure));
    }
    Ok(AddOutcome::Added)
}

pub(crate) fn list_keys(
    authority: &AdmittedIosBackupAuthority,
    destination: &Destination,
    password: &str,
    env: &BTreeMap<String, Option<String>>,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<Vec<(String, bool)>, RepoError> {
    let repository = validate_call(
        authority,
        &[],
        &destination.repository,
        password,
        None,
        Some(env),
        cancellation,
    )
    .map_err(|_| RepoError::Failed)?;
    let options = native::Options {
        version: 1,
        check_read_mode: 0,
        retry_lock_ms: 0,
        group: 0,
        groups: 0,
        reserved1: 0,
        reserved2: 0,
    };
    let completed = execute(
        authority,
        9,
        false,
        repository,
        password,
        "",
        &[],
        &options,
        timeout,
        cancellation,
        true,
    )
    .map_err(ExecuteError::into_repo)?;
    if let Some(failure) =
        outcome::callable_remove_failure(&completed).map_err(|_| RepoError::Failed)?
    {
        return Err(RepoError::Callable(failure));
    }
    outcome::decode_recovery_keys(&completed).map_err(|_| RepoError::Failed)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn remove_key(
    authority: &AdmittedIosBackupAuthority,
    destination: &Destination,
    password: &str,
    key_id: &str,
    env: &BTreeMap<String, Option<String>>,
    timeout: Option<Duration>,
    cancellation: Option<&ArchiveCancellation>,
) -> Result<(), RepoError> {
    if !outcome::identifier(key_id) {
        return Err(RepoError::Failed);
    }
    let repository = validate_call(
        authority,
        &[],
        &destination.repository,
        password,
        None,
        Some(env),
        cancellation,
    )
    .map_err(|_| RepoError::Failed)?;
    authority
        .revalidate()
        .map_err(|reason| refused(reason.code()))
        .map_err(|_| RepoError::Failed)?;
    let password = Zeroizing::new(password.as_bytes().to_vec());
    let options = native::Options {
        version: 1,
        check_read_mode: 0,
        retry_lock_ms: 0,
        group: 0,
        groups: 0,
        reserved1: 0,
        reserved2: 0,
    };
    let state;
    let call = match repository {
        repository::Repository::Local(repo_path) => {
            native::Call::local_remove_key(&password, repo_path, key_id, &options, 1)
        }
        repository::Repository::S3 {
            destination,
            access,
            secret,
            token,
            append_only,
        } => {
            state = authority
                .installed
                .storage
                .create_state(&destination.namespace())
                .map_err(|reason| refused(reason.code()))
                .map_err(|_| RepoError::Failed)?;
            state
                .revalidate()
                .map_err(|reason| refused(reason.code()))
                .map_err(|_| RepoError::Failed)?;
            let remote = native::Remote {
                endpoint: &destination.endpoint,
                region: &destination.region,
                bucket: &destination.bucket,
                prefix: &destination.prefix,
                access: access.as_bytes(),
                secret: secret.as_bytes(),
                token: token.as_bytes(),
                state: native::PrivateDirectory {
                    path: state.path(),
                    descriptor: state.as_fd(),
                },
            };
            let options = native::Options {
                version: if append_only { 2 } else { options.version },
                reserved1: u32::from(append_only),
                ..options
            };
            native::Call::remote_remove_key(&password, &remote, key_id, &options, 1)
        }
    }
    .map_err(|status| {
        log::warn!("ios backup remove key refused at entry: status {status}");
        RepoError::Failed
    })?;
    let cancelled = || cancellation.is_some_and(ArchiveCancellation::is_cancelled);
    let completed = call.wait(timeout, Some(&cancelled), false);
    log_failure(10, &completed);
    if let Some(failure) =
        outcome::callable_remove_failure(&completed).map_err(|_| RepoError::Failed)?
    {
        return Err(RepoError::Callable(failure));
    }
    Ok(())
}
