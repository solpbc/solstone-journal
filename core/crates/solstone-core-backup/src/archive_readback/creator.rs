// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Captured archive predecessors and the fixed installed direct creator.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use solstone_core_journal_io::media::{MediaDigest, MediaIdentity};

use super::*;

pub struct ArchiveEndpointCapture {
    pub destination: Destination,
    pub password: String,
    pub backend_env: BTreeMap<String, Option<String>>,
    pub global_options: Vec<String>,
}

pub struct ArchivePredecessorCapture {
    pub restic_path: PathBuf,
    pub write: ArchiveEndpointCapture,
    pub read: ArchiveEndpointCapture,
    pub broker_requested: bool,
    pub write_repository_requested: bool,
    pub read_repository_requested: bool,
    pub write_repository_id: Option<String>,
    pub read_repository_id: Option<String>,
    pub repository_id: Option<String>,
    pub archives: Vec<ArchiveWriterObservation>,
}

pub struct ArchiveWriterObservation {
    pub ordinal: u64,
    pub requested_argv: Vec<OsString>,
    pub sources: Vec<ArchiveWriterSourceObservation>,
    pub effects: DirectArchiveEffects,
}

pub struct ArchiveWriterSourceObservation {
    pub name: String,
    pub identity: MediaIdentity,
    pub digest: MediaDigest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectArchiveEffects {
    NotAttempted,
    Attempted {
        returncode: Option<i32>,
        observed_snapshot_id: Option<String>,
    },
}

pub struct DirectArchiveSnapshot {
    pub repository_id: String,
    pub snapshot_id: String,
}

pub struct DirectArchiveFailure {
    pub reason: ReadbackError,
    pub effects: DirectArchiveEffects,
}

enum CreatorState {
    Unknown,
    Admission,
    Direct(WriterProgress),
}
enum WriterProgress {
    NoWriter,
    Submitted,
    Accepted,
    // Read only by the Linux x86_64 and macOS settlement below.
    #[cfg_attr(
        not(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos")),
        allow(dead_code)
    )]
    Refused(ReadbackError),
}

pub(super) struct ArchivePredecessorCustody {
    capture: ArchivePredecessorCapture,
    state: CreatorState,
    pending: u64,
}
impl ArchivePredecessorCustody {
    pub(super) fn unknown(capture: ArchivePredecessorCapture) -> Self {
        Self {
            capture,
            state: CreatorState::Unknown,
            pending: 0,
        }
    }
    fn requested(capture: ArchivePredecessorCapture) -> Self {
        Self {
            capture,
            state: CreatorState::Admission,
            pending: 0,
        }
    }
    pub(super) fn creator_executor(&self) -> bool {
        !matches!(self.state, CreatorState::Unknown)
    }
    /// This creator never submitted an archive writer, and owes no earlier one.
    pub(super) fn unwritten(&self) -> bool {
        self.creator_executor() && self.pending == 0 && self.capture.archives.is_empty()
    }
    #[cfg(any(unix, windows))]
    pub(super) fn validate_input(&self, input: &ReadAdmission<'_>) -> Result<(), ReadbackError> {
        if !self.creator_executor() {
            return Ok(());
        }
        let capture = &self.capture;
        let configuration = input.operation.configuration();
        let binding = match (configuration.binding(), configuration.mode()) {
            (None, Some("byo")) => None,
            (Some(binding), Some("operated")) => Some(binding),
            _ => return Err(ReadbackError::UnsupportedBackend),
        };
        if input.operation.owned_clock().is_none()
            || capture.broker_requested != binding.is_some()
            || capture.write_repository_requested
            || capture.read_repository_requested
            || capture.write_repository_id.is_some()
            || capture.read_repository_id.is_some()
            || capture.repository_id.is_some()
            || !capture.archives.is_empty()
            || capture.restic_path != input.expected_restic_path
        {
            return Err(ReadbackError::UnsupportedBackend);
        }
        let endpoints = match binding {
            None => vec![&capture.write, &capture.read],
            Some(binding) => {
                validate_operated_writer(&capture.write, &capture.read, binding, input)?;
                vec![&capture.read]
            }
        };
        for endpoint in endpoints {
            if endpoint.destination != input.destination
                || endpoint.password != input.password
                || !endpoint.global_options.is_empty()
            {
                return Err(ReadbackError::UnsupportedBackend);
            }
            let expected = crate::assemble_backend_env(&endpoint.destination)
                .map_err(|_| ReadbackError::CredentialsUnavailable)?
                .into_iter()
                .map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key, Some(value.to_owned())))
                        .ok_or(ReadbackError::CredentialsUnavailable)
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            if endpoint.backend_env != expected {
                return Err(ReadbackError::CredentialsUnavailable);
            }
            direct_environment(&endpoint.destination, &endpoint.password)?;
        }
        Ok(())
    }
    #[cfg(unix)]
    pub(super) fn validate_member(
        &self,
        member: &CurrentInstalledMember,
    ) -> Result<(), ReadbackError> {
        if !self.creator_executor() {
            return Ok(());
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            if member.bytes() == 30_924_962
                && member.sha256()
                    == "ae7fe58ab3511f830fd31d157158620b209522ff1332b119199d2e938d72338c"
            {
                Ok(())
            } else {
                Err(ReadbackError::UnsupportedToolVersion)
            }
        }
        // No byte pin on macOS (see `pinned_member`): the manifest member,
        // revalidated around every use, reported the supported version.
        #[cfg(target_os = "macos")]
        {
            let _ = member;
            Ok(())
        }
        #[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos")))]
        {
            let _ = member;
            Err(ReadbackError::Unsupported)
        }
    }
    pub(super) fn mark_read_config(&mut self) {
        if matches!(self.state, CreatorState::Admission) {
            self.capture.read_repository_requested = true;
        }
    }
    pub(super) fn confirm_read_config(&mut self, id: &str) {
        if matches!(self.state, CreatorState::Admission) {
            self.capture.read_repository_id = Some(id.to_owned());
            self.capture.repository_id = Some(id.to_owned());
        }
    }
    // Direct readback qualifies only on Linux x86_64 and macOS.
    #[cfg_attr(
        not(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos")),
        allow(dead_code)
    )]
    pub(super) fn qualify_settlement(&self) -> Result<(), ReadbackError> {
        match self.state {
            CreatorState::Direct(WriterProgress::NoWriter | WriterProgress::Accepted)
                if self.pending == 0 =>
            {
                Ok(())
            }
            CreatorState::Direct(WriterProgress::Refused(reason)) => Err(reason),
            _ => Err(ReadbackError::ToolFailed),
        }
    }
    fn refuse(&mut self, reason: ReadbackError) {
        if !matches!(self.state, CreatorState::Direct(WriterProgress::Refused(_))) {
            self.state = CreatorState::Direct(WriterProgress::Refused(reason));
        }
    }
    fn submit(&mut self) -> Result<(), ReadbackError> {
        self.pending = self
            .pending
            .checked_add(1)
            .ok_or(ReadbackError::ToolFailed)?;
        if !matches!(self.state, CreatorState::Direct(WriterProgress::Refused(_))) {
            self.state = CreatorState::Direct(WriterProgress::Submitted);
        }
        Ok(())
    }
    #[cfg(unix)]
    fn accept(&mut self, child: CompletedChild) {
        if !child.successful() || self.pending == 0 {
            self.refuse(ReadbackError::ToolFailed);
            return;
        }
        self.pending -= 1;
        if !matches!(self.state, CreatorState::Direct(WriterProgress::Refused(_))) {
            self.state = CreatorState::Direct(WriterProgress::Accepted);
        }
        drop(child);
    }
    #[cfg(windows)]
    fn reaped_windows(&mut self) {
        if let Some(pending) = self.pending.checked_sub(1) {
            self.pending = pending;
        } else {
            self.refuse(ReadbackError::ToolFailed);
        }
    }
    #[cfg(windows)]
    fn accept_windows(&mut self) {
        if self.pending != 0 {
            self.refuse(ReadbackError::ToolFailed);
            return;
        }
        if !matches!(self.state, CreatorState::Direct(WriterProgress::Refused(_))) {
            self.state = CreatorState::Direct(WriterProgress::Accepted);
        }
    }
    fn admission_effects(&self) -> (bool, bool) {
        (
            self.capture.write_repository_requested,
            self.capture.read_repository_requested,
        )
    }
}

/// Operated storage is written only through the installed rclone serving the
/// one broker session append-only, at the location the reader reads.
#[cfg(any(unix, windows))]
fn validate_operated_writer(
    write: &ArchiveEndpointCapture,
    read: &ArchiveEndpointCapture,
    binding: &crate::HostedBinding,
    input: &ReadAdmission<'_>,
) -> Result<(), ReadbackError> {
    let rclone = operated_writer_program(write, read, binding, &input.password)?;
    #[cfg(unix)]
    {
        let member = solstone_core_installed_payload::resolve_current_member(super::RCLONE_MEMBER)
            .map_err(|_| ReadbackError::ToolUnavailable)?;
        if std::path::Path::new(rclone) != member.path() {
            return Err(ReadbackError::ToolChanged);
        }
    }
    #[cfg(windows)]
    let _ = rclone;
    Ok(())
}

/// The exact operated writer shape; returns the rclone program it names.
#[cfg(any(unix, windows))]
fn operated_writer_program<'a>(
    write: &'a ArchiveEndpointCapture,
    read: &ArchiveEndpointCapture,
    binding: &crate::HostedBinding,
    password: &str,
) -> Result<&'a str, ReadbackError> {
    let unsupported = Err(ReadbackError::UnsupportedBackend);
    let env = |key: &str| write.backend_env.get(key).cloned().flatten();
    let read_env = |key: &str| read.backend_env.get(key).cloned().flatten();
    let endpoint = env("RCLONE_CONFIG_SPB_ENDPOINT").unwrap_or_default();
    if write.password != password
        || write.destination.backend != "rclone"
        || !write.destination.credentials.is_empty()
        || write.destination.repository
            != format!("rclone:spb:{}/{}", binding.bucket, binding.prefix)
        || endpoint.trim().is_empty()
        || read.destination.repository
            != format!(
                "s3:{}/{}/{}",
                endpoint.trim_end_matches('/'),
                binding.bucket,
                binding.prefix
            )
    {
        return unsupported;
    }
    let fixed = [
        ("RCLONE_CONFIG_SPB_TYPE", "s3"),
        ("RCLONE_CONFIG_SPB_PROVIDER", "Cloudflare"),
        ("RCLONE_CONFIG_SPB_ENV_AUTH", "false"),
        ("RCLONE_CONFIG_SPB_REGION", "auto"),
        ("RCLONE_CONFIG_SPB_NO_CHECK_BUCKET", "true"),
    ];
    let session = [
        ("RCLONE_CONFIG_SPB_ACCESS_KEY_ID", "AWS_ACCESS_KEY_ID"),
        (
            "RCLONE_CONFIG_SPB_SECRET_ACCESS_KEY",
            "AWS_SECRET_ACCESS_KEY",
        ),
        ("RCLONE_CONFIG_SPB_SESSION_TOKEN", "AWS_SESSION_TOKEN"),
    ];
    if write.backend_env.len() != fixed.len() + session.len() + 1
        || fixed
            .iter()
            .any(|(key, value)| env(key).as_deref() != Some(*value))
        || session.iter().any(|(key, read_key)| {
            env(key)
                .is_none_or(|value| value.trim().is_empty() || Some(value) != read_env(read_key))
        })
    {
        return Err(ReadbackError::CredentialsUnavailable);
    }
    let [o1, program, o2, args] = write.global_options.as_slice() else {
        return unsupported;
    };
    let no_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let rclone = program
        .strip_prefix("rclone.program=\"")
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|path| !path.contains('"') && std::path::Path::new(path).is_absolute());
    match rclone {
        Some(rclone)
            if o1 == "-o"
                && o2 == "-o"
                && *args
                    == format!(
                        "rclone.args=serve restic --stdio --append-only --config {no_config}"
                    ) =>
        {
            Ok(rclone)
        }
        _ => unsupported,
    }
}

/// The writer's program arguments and environment. BYO writes through the
/// read's own direct environment; operated writes through its captured
/// append-only transport, whose program option restic parses as one CSV field.
#[cfg(any(unix, windows))]
fn writer_invocation(
    write: &ArchiveEndpointCapture,
    read_env: &BTreeMap<OsString, OsString>,
    argv: Vec<OsString>,
) -> (Vec<OsString>, BTreeMap<OsString, OsString>) {
    if write.global_options.is_empty() {
        return (argv, read_env.clone());
    }
    let mut env = BTreeMap::from([
        (
            OsString::from("RESTIC_REPOSITORY"),
            OsString::from(&write.destination.repository),
        ),
        (
            OsString::from("RESTIC_PASSWORD"),
            OsString::from(&write.password),
        ),
    ]);
    for (key, value) in &write.backend_env {
        if let Some(value) = value {
            env.insert(key.into(), value.into());
        }
    }
    let mut full: Vec<OsString> = write
        .global_options
        .iter()
        .map(|option| {
            // Windows encodes this field after admitting the program itself.
            if cfg!(unix) && option.starts_with("rclone.program=") {
                format!("\"{}\"", option.replace('"', "\"\"")).into()
            } else {
                option.into()
            }
        })
        .collect();
    full.extend(argv);
    (full, env)
}

impl OwnedReadbackAdmissionError {
    pub fn archive_admission_effects(&self) -> Option<(bool, bool)> {
        match &self.owned {
            ClosedAdmissionOwner::Input(input) => input._predecessor.as_ref(),
            ClosedAdmissionOwner::Admitted(read) => read._execution.predecessor.as_ref(),
        }
        .map(ArchivePredecessorCustody::admission_effects)
    }
    /// Whether this archive admission asked the broker for a session.
    pub fn archive_broker_requested(&self) -> bool {
        match &self.owned {
            ClosedAdmissionOwner::Input(input) => input._predecessor.as_ref(),
            ClosedAdmissionOwner::Admitted(read) => read._execution.predecessor.as_ref(),
        }
        .is_some_and(|custody| custody.capture.broker_requested)
    }
}

#[must_use]
pub struct OwnedArchiveCreatorAdmission {
    input: OwnedReadAdmission,
}
impl OwnedPreparedArchiveOperation {
    pub fn bind_archive_creator(
        self,
        capture: ArchivePredecessorCapture,
    ) -> OwnedArchiveCreatorAdmission {
        let (operation, clock) = self.into_parts();
        let mut input = ReadAdmission::capture_current(
            operation,
            capture.read.destination.clone(),
            capture.read.password.clone(),
            capture.restic_path.clone(),
        );
        input.predecessor = Some(ArchivePredecessorCustody::requested(capture));
        OwnedArchiveCreatorAdmission {
            input: OwnedReadAdmission { input, clock },
        }
    }
}
impl OwnedArchiveCreatorAdmission {
    #[cfg(unix)]
    pub fn admit(self) -> Result<OwnedArchiveCreator, OwnedReadbackAdmissionError> {
        let read = self.input.admit()?;
        let paired = (|| {
            if let Some(custody) = read.read.execution.borrow_mut().predecessor.as_mut() {
                custody.capture.write_repository_requested = true;
            }
            read.read.check_repository()?;
            read.read.revalidate_admission()?;
            if let Some(custody) = read.read.execution.borrow_mut().predecessor.as_mut() {
                custody.capture.write_repository_id = Some(read.repository_id().to_owned());
                custody.state = CreatorState::Direct(WriterProgress::NoWriter);
            }
            Ok(())
        })();
        if let Err(reason) = paired {
            if let Some(custody) = read.read.execution.borrow_mut().predecessor.as_mut() {
                custody.refuse(reason);
            }
            return Err(OwnedReadbackAdmissionError {
                reason,
                owned: ClosedAdmissionOwner::Admitted(Box::new(read.into_owned_hold())),
            });
        }
        Ok(OwnedArchiveCreator { read })
    }

    #[cfg(windows)]
    pub fn admit(
        self,
        executor: Option<Arc<dyn WindowsArchiveExecutor>>,
    ) -> Result<OwnedArchiveCreator, OwnedReadbackAdmissionError> {
        let read = self.input.admit(executor)?;
        let paired = (|| {
            if let Some(custody) = read.read.execution.borrow_mut().predecessor.as_mut() {
                custody.capture.write_repository_requested = true;
            }
            read.read.check_repository()?;
            read.read.revalidate_admission()?;
            if let Some(custody) = read.read.execution.borrow_mut().predecessor.as_mut() {
                custody.capture.write_repository_id = Some(read.repository_id().to_owned());
                custody.state = CreatorState::Direct(WriterProgress::NoWriter);
            }
            Ok(())
        })();
        if let Err(reason) = paired {
            if let Some(custody) = read.read.execution.borrow_mut().predecessor.as_mut() {
                custody.refuse(reason);
            }
            return Err(OwnedReadbackAdmissionError {
                reason,
                owned: ClosedAdmissionOwner::Admitted(Box::new(read.into_owned_hold())),
            });
        }
        Ok(OwnedArchiveCreator { read })
    }
}

#[must_use]
pub struct OwnedArchiveCreator {
    read: OwnedReadOperation,
}
impl OwnedArchiveCreator {
    pub fn prepared_operation(&self) -> &PreparedArchiveOperation<'static> {
        &self.read.read.operation
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.read.carrier()
    }
    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.read.activity()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.read.journal_root()
    }
    pub fn repository_id(&self) -> &str {
        self.read.repository_id()
    }
    pub fn revalidate_local(&self) -> Result<(), ReadbackError> {
        self.read.revalidate_admission()
    }
    pub fn current_time(&self) -> Result<i64, ReadbackError> {
        self.revalidate_local()?;
        Ok(self.prepared_operation().clock().now_unix())
    }
    pub fn into_owned_hold(self) -> OwnedDirectReadHold {
        self.read.into_owned_hold()
    }
    pub fn into_read_operation(self) -> Result<OwnedReadOperation, OwnedReadbackAdmissionError> {
        if let Err(reason) = self.read.revalidate_admission() {
            return Err(OwnedReadbackAdmissionError {
                reason,
                owned: ClosedAdmissionOwner::Admitted(Box::new(self.read.into_owned_hold())),
            });
        }
        // Failed predecessor history moves intact; only settlement qualifies it.
        Ok(self.read)
    }
    pub fn archive_owned(
        &mut self,
        sources: &[ArchiveSource<'_>],
    ) -> Result<DirectArchiveSnapshot, DirectArchiveFailure> {
        let mut argv = vec![
            "backup".into(),
            "--force".into(),
            "--no-cache".into(),
            "--tag".into(),
            super::OFFLOAD_ARCHIVE_TAG.into(),
            "--retry-lock".into(),
            "30m".into(),
        ];
        argv.extend(
            sources
                .iter()
                .map(|source| source.path().as_os_str().to_owned()),
        );
        argv.push("--json".into());
        let facts = sources
            .iter()
            .filter_map(|source| {
                source
                    .name()
                    .to_str()
                    .map(|name| ArchiveWriterSourceObservation {
                        name: name.to_owned(),
                        identity: source.identity(),
                        digest: source.digest().clone(),
                    })
            })
            .collect();
        let index = {
            let mut execution = self.read.read.execution.borrow_mut();
            let Some(custody) = execution.predecessor.as_mut() else {
                return Err(DirectArchiveFailure {
                    reason: ReadbackError::Unsupported,
                    effects: DirectArchiveEffects::NotAttempted,
                });
            };
            let index = custody.capture.archives.len();
            let ordinal = match u64::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
            {
                Some(ordinal) => ordinal,
                None => {
                    custody.refuse(ReadbackError::ToolFailed);
                    return Err(DirectArchiveFailure {
                        reason: ReadbackError::ToolFailed,
                        effects: DirectArchiveEffects::NotAttempted,
                    });
                }
            };
            custody.capture.archives.push(ArchiveWriterObservation {
                ordinal,
                requested_argv: argv.clone(),
                sources: facts,
                effects: DirectArchiveEffects::NotAttempted,
            });
            index
        };
        let mut effects = DirectArchiveEffects::NotAttempted;
        let result = self.archive_inner(sources, argv, index, &mut effects);
        if let Some(custody) = self.read.read.execution.borrow_mut().predecessor.as_mut() {
            custody.capture.archives[index].effects = effects.clone();
            if let Err(reason) = result {
                custody.refuse(reason);
            }
        }
        result.map_err(|reason| DirectArchiveFailure { reason, effects })
    }
    #[cfg(not(any(unix, windows)))]
    fn archive_inner(
        &mut self,
        _: &[ArchiveSource<'_>],
        _: Vec<OsString>,
        _: usize,
        _: &mut DirectArchiveEffects,
    ) -> Result<DirectArchiveSnapshot, ReadbackError> {
        Err(ReadbackError::Unsupported)
    }
    #[cfg(windows)]
    fn archive_inner(
        &mut self,
        sources: &[ArchiveSource<'_>],
        argv: Vec<OsString>,
        index: usize,
        effects: &mut DirectArchiveEffects,
    ) -> Result<DirectArchiveSnapshot, ReadbackError> {
        let mut names = std::collections::BTreeSet::new();
        if sources.is_empty()
            || sources.iter().any(|source| {
                !anchored_path(source)
                    || source.path().to_str().is_none()
                    || source.path().file_name() != Some(source.name())
                    || source
                        .name()
                        .to_str()
                        .is_none_or(|name| !plain_name(name) || !names.insert(name))
            })
        {
            return Err(ReadbackError::InvalidSelection);
        }
        self.revalidate_sources(sources)?;
        self.read.read.check_repository()?;
        self.revalidate_sources(sources)?;
        let timeout = self
            .prepared_operation()
            .budget()
            .remaining(self.prepared_operation().clock())?;
        let program = self.read.read.restic_path.as_os_str().to_owned();
        let mut retained: Vec<Arc<dyn Send + Sync>> =
            vec![Arc::clone(self.carrier()) as Arc<dyn Send + Sync>];
        for source in sources {
            retained.push(Arc::new(
                source
                    .retain_for_helper()
                    .map_err(|_| ReadbackError::NativeFailure)?,
            ));
        }
        {
            let mut execution = self.read.read.execution.borrow_mut();
            let custody = execution
                .predecessor
                .as_mut()
                .ok_or(ReadbackError::Unsupported)?;
            custody.submit()?;
            *effects = DirectArchiveEffects::Attempted {
                returncode: None,
                observed_snapshot_id: None,
            };
            custody.capture.archives[index].effects = effects.clone();
        }
        let (argv, env) = {
            let execution = self.read.read.execution.borrow();
            let custody = execution
                .predecessor
                .as_ref()
                .ok_or(ReadbackError::Unsupported)?;
            writer_invocation(&custody.capture.write, &self.read.read.env, argv)
        };
        let cancel = self.prepared_operation().budget().cancellation();
        let executor = Arc::clone(&self.read.read.executor);
        let output = executor.run_cancellable(
            WindowsArchiveRequest {
                program: std::path::Path::new(&program),
                argv: &argv,
                env: &env,
                timeout,
                cancellation: cancel,
                retained,
            },
            64 * 1024 * 1024,
        );
        {
            let mut execution = self.read.read.execution.borrow_mut();
            execution.retain_cleanup(&output);
            if let Some(custody) = execution.predecessor.as_mut() {
                custody.reaped_windows();
            }
        }
        if !output.quiescent() {
            return Err(self.read.read.execution_error());
        }
        let observed = Some(output.returncode);
        let snapshot = parse_summary(&output.stdout).ok();
        *effects = DirectArchiveEffects::Attempted {
            returncode: observed,
            observed_snapshot_id: snapshot.clone(),
        };
        self.revalidate_sources(sources)?;
        self.read.read.check_repository()?;
        self.revalidate_sources(sources)?;
        if !output.successful() {
            return Err(ReadbackError::ToolFailed);
        }
        let snapshot_id = snapshot.ok_or(ReadbackError::SnapshotNodeUnavailable)?;
        if let Some(custody) = self.read.read.execution.borrow_mut().predecessor.as_mut() {
            custody.accept_windows();
        }
        Ok(DirectArchiveSnapshot {
            repository_id: self.repository_id().to_owned(),
            snapshot_id,
        })
    }
    #[cfg(any(unix, windows))]
    fn revalidate_sources(&self, sources: &[ArchiveSource<'_>]) -> Result<(), ReadbackError> {
        self.revalidate_local()?;
        let result = sources.iter().try_for_each(|source| {
            source
                .revalidate()
                .map_err(|_| ReadbackError::NativeFailure)
        });
        self.revalidate_local()?;
        result
    }
    #[cfg(unix)]
    fn archive_inner(
        &mut self,
        sources: &[ArchiveSource<'_>],
        argv: Vec<OsString>,
        index: usize,
        effects: &mut DirectArchiveEffects,
    ) -> Result<DirectArchiveSnapshot, ReadbackError> {
        let mut names = std::collections::BTreeSet::new();
        if sources.is_empty()
            || sources.iter().any(|source| {
                !anchored_path(source)
                    || source.path().to_str().is_none()
                    || source.path().file_name() != Some(source.name())
                    || source
                        .name()
                        .to_str()
                        .is_none_or(|name| !plain_name(name) || !names.insert(name))
            })
        {
            return Err(ReadbackError::InvalidSelection);
        }
        #[cfg(target_os = "macos")]
        let (anchor, core) = {
            use std::os::fd::AsRawFd;
            let anchor = sources[0].anchor();
            if sources
                .iter()
                .any(|source| source.anchor().as_raw_fd() != anchor.as_raw_fd())
            {
                return Err(ReadbackError::InvalidSelection);
            }
            let core = solstone_core_installed_payload::resolve_current_member(
                crate::owned_creator::CORE_MEMBER,
            )
            .map_err(|_| ReadbackError::ToolUnavailable)?;
            (anchor, core)
        };
        self.revalidate_sources(sources)?;
        self.read.read.check_repository()?;
        self.read.read.check_repository()?;
        self.revalidate_sources(sources)?;
        let timeout = self
            .prepared_operation()
            .budget()
            .remaining(self.prepared_operation().clock())?;
        let (argv, env) = {
            let execution = self.read.read.execution.borrow();
            let custody = execution
                .predecessor
                .as_ref()
                .ok_or(ReadbackError::Unsupported)?;
            if !custody.capture.write.global_options.is_empty() {
                // Re-admit the operated transport immediately before it can run.
                solstone_core_installed_payload::resolve_current_member(super::RCLONE_MEMBER)
                    .map_err(|_| ReadbackError::ToolUnavailable)?
                    .revalidate()
                    .map_err(|_| ReadbackError::ToolChanged)?;
            }
            writer_invocation(&custody.capture.write, &self.read.read.env, argv)
        };
        #[cfg(not(target_os = "macos"))]
        let (program, argv) = (self.read.read.member.path().as_os_str().to_owned(), argv);
        #[cfg(target_os = "macos")]
        let (program, argv) = {
            revalidate_core(&core)?;
            let mut wrapped = vec![
                crate::owned_creator::ENTRY.into(),
                self.read.read.member.path().as_os_str().to_owned(),
            ];
            wrapped.extend(argv);
            (core.path().as_os_str().to_owned(), wrapped)
        };
        let request = Request {
            program,
            argv,
            env,
            timeout: Some(timeout),
            pass_fds: sources
                .iter()
                .filter_map(ArchiveSource::descriptor)
                .collect(),
        };
        {
            let mut execution = self.read.read.execution.borrow_mut();
            let custody = execution
                .predecessor
                .as_mut()
                .ok_or(ReadbackError::Unsupported)?;
            custody.submit()?;
            // This conservative call boundary records attempted effects even if spawn fails.
            *effects = DirectArchiveEffects::Attempted {
                returncode: None,
                observed_snapshot_id: None,
            };
            custody.capture.archives[index].effects = effects.clone();
        }
        #[cfg(not(target_os = "macos"))]
        let child = bounded_child::run_owned_archive_creator(
            &request,
            Some(64 * 1024 * 1024),
            self.prepared_operation().budget().cancellation(),
        );
        #[cfg(target_os = "macos")]
        let child = bounded_child::run_owned_archive_creator_at(
            &request,
            anchor,
            Some(64 * 1024 * 1024),
            self.prepared_operation().budget().cancellation(),
        );
        let observed = child.as_ref().ok().map(|child| child.output().returncode);
        let snapshot = child
            .as_ref()
            .ok()
            .and_then(|child| parse_summary(&child.output().stdout).ok());
        *effects = DirectArchiveEffects::Attempted {
            returncode: observed,
            observed_snapshot_id: snapshot.clone(),
        };
        self.revalidate_sources(sources)?;
        #[cfg(target_os = "macos")]
        revalidate_core(&core)?;
        let child = child.map_err(|_| self.read.read.execution_error())?;
        self.read.read.check_repository()?;
        self.read.read.check_repository()?;
        self.revalidate_sources(sources)?;
        if !child.successful() {
            return Err(ReadbackError::ToolFailed);
        }
        let snapshot_id = snapshot.ok_or(ReadbackError::SnapshotNodeUnavailable)?;
        if let Some(custody) = self.read.read.execution.borrow_mut().predecessor.as_mut() {
            custody.accept(child);
        }
        Ok(DirectArchiveSnapshot {
            repository_id: self.repository_id().to_owned(),
            snapshot_id,
        })
    }
}

/// Linux reads each source through its stage's descriptor; macOS reads
/// `<stage>/<name>` from inside the stage parent the child starts in.
#[cfg(unix)]
fn anchored_path(source: &ArchiveSource<'_>) -> bool {
    #[cfg(not(target_os = "macos"))]
    {
        source.path().is_absolute() && source.descriptor().is_some()
    }
    #[cfg(target_os = "macos")]
    {
        let mut components = source.path().components();
        matches!(
            (components.next(), components.next(), components.next()),
            (Some(std::path::Component::Normal(stage)), Some(std::path::Component::Normal(_)), None)
                if stage.to_str().is_some_and(|stage| stage.starts_with(".tmp-offload-") && plain_name(stage))
        )
    }
}

#[cfg(windows)]
fn anchored_path(source: &ArchiveSource<'_>) -> bool {
    source.path().is_absolute()
}

#[cfg(target_os = "macos")]
fn revalidate_core(
    core: &solstone_core_installed_payload::CurrentInstalledMember,
) -> Result<(), ReadbackError> {
    core.revalidate().map_err(|_| ReadbackError::ToolChanged)
}

#[cfg(any(unix, windows))]
fn parse_summary(output: &[u8]) -> Result<String, ReadbackError> {
    let mut summary = None;
    for line in output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
    {
        let value: Value =
            serde_json::from_slice(line).map_err(|_| ReadbackError::SnapshotNodeUnavailable)?;
        if value.get("message_type").and_then(Value::as_str) == Some("summary") {
            let id = value
                .get("snapshot_id")
                .and_then(Value::as_str)
                .filter(|id| full_hex_identity(id))
                .ok_or(ReadbackError::SnapshotNodeUnavailable)?;
            if summary.replace(id.to_owned()).is_some() {
                return Err(ReadbackError::SnapshotNodeUnavailable);
            }
        }
    }
    summary.ok_or(ReadbackError::SnapshotNodeUnavailable)
}

#[cfg(all(test, any(unix, windows)))]
mod operated_writer_tests {
    use super::*;

    const NO_CONFIG: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };
    const RCLONE: &str = if cfg!(windows) {
        "C:\\app\\rclone.exe"
    } else {
        "/app/lib/solstone-rclone/rclone"
    };

    fn binding() -> crate::HostedBinding {
        crate::HostedBinding {
            broker_endpoint: "https://broker.invalid".into(),
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: "prefix".into(),
            broker_token: "token".into(),
        }
    }

    fn read() -> ArchiveEndpointCapture {
        let destination = Destination {
            repository: "s3:https://storage.invalid/bucket/prefix".into(),
            backend: "s3".into(),
            credentials: serde_json::from_value(serde_json::json!({
                "access_key_id": "session-id",
                "secret_access_key": "session-secret",
                "session_token": "session-token",
            }))
            .unwrap(),
        };
        let backend_env = crate::assemble_backend_env(&destination)
            .unwrap()
            .into_iter()
            .map(|(key, value)| (key, value.as_str().map(str::to_owned)))
            .collect();
        ArchiveEndpointCapture {
            destination,
            password: "daily".into(),
            backend_env,
            global_options: vec![],
        }
    }

    fn write() -> ArchiveEndpointCapture {
        ArchiveEndpointCapture {
            destination: Destination {
                repository: "rclone:spb:bucket/prefix".into(),
                backend: "rclone".into(),
                credentials: Default::default(),
            },
            password: "daily".into(),
            backend_env: [
                ("RCLONE_CONFIG_SPB_TYPE", "s3"),
                ("RCLONE_CONFIG_SPB_PROVIDER", "Cloudflare"),
                ("RCLONE_CONFIG_SPB_ENV_AUTH", "false"),
                ("RCLONE_CONFIG_SPB_ACCESS_KEY_ID", "session-id"),
                ("RCLONE_CONFIG_SPB_SECRET_ACCESS_KEY", "session-secret"),
                ("RCLONE_CONFIG_SPB_SESSION_TOKEN", "session-token"),
                ("RCLONE_CONFIG_SPB_ENDPOINT", "https://storage.invalid"),
                ("RCLONE_CONFIG_SPB_REGION", "auto"),
                ("RCLONE_CONFIG_SPB_NO_CHECK_BUCKET", "true"),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), Some(value.to_owned())))
            .collect(),
            global_options: vec![
                "-o".into(),
                format!("rclone.program=\"{RCLONE}\""),
                "-o".into(),
                format!("rclone.args=serve restic --stdio --append-only --config {NO_CONFIG}"),
            ],
        }
    }

    fn admitted(write: &ArchiveEndpointCapture) -> Result<&str, ReadbackError> {
        operated_writer_program(write, &read(), &binding(), "daily")
    }

    #[test]
    fn operated_writer_is_the_append_only_transport_for_the_readers_session() {
        assert_eq!(admitted(&write()), Ok(RCLONE));

        type Tamper = fn(&mut ArchiveEndpointCapture);
        let refused: [(&str, Tamper); 7] = [
            ("append-only dropped", |w| {
                w.global_options[3] =
                    format!("rclone.args=serve restic --stdio --config {NO_CONFIG}")
            }),
            ("relative program", |w| {
                w.global_options[1] = "rclone.program=\"rclone\"".into()
            }),
            ("another prefix", |w| {
                w.destination.repository = "rclone:spb:bucket/other".into()
            }),
            ("another session", |w| {
                w.backend_env.insert(
                    "RCLONE_CONFIG_SPB_SESSION_TOKEN".into(),
                    Some("other".into()),
                );
            }),
            ("ambient credentials", |w| {
                w.backend_env
                    .insert("RCLONE_CONFIG_SPB_ENV_AUTH".into(), Some("true".into()));
            }),
            ("extra setting", |w| {
                w.backend_env
                    .insert("RCLONE_CONFIG_SPB_ACL".into(), Some("public-read".into()));
            }),
            ("another password", |w| w.password = "other".into()),
        ];
        for (case, tamper) in refused {
            let mut tampered = write();
            tamper(&mut tampered);
            assert!(admitted(&tampered).is_err(), "{case} was admitted");
        }
    }

    #[test]
    fn operated_writer_runs_with_its_own_transport_not_the_readers_credentials() {
        let read_env = direct_environment(&read().destination, "daily").unwrap();
        let (argv, env) = writer_invocation(&write(), &read_env, vec!["backup".into()]);
        let program = if cfg!(unix) {
            format!("\"rclone.program=\"\"{RCLONE}\"\"\"")
        } else {
            format!("rclone.program=\"{RCLONE}\"")
        };
        assert_eq!(
            argv,
            [
                "-o".to_owned(),
                program,
                "-o".into(),
                format!("rclone.args=serve restic --stdio --append-only --config {NO_CONFIG}"),
                "backup".into(),
            ]
            .map(OsString::from)
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("RESTIC_REPOSITORY")),
            Some(&OsString::from("rclone:spb:bucket/prefix"))
        );
        assert!(
            env.keys()
                .all(|key| !key.to_string_lossy().starts_with("AWS_"))
        );

        let (argv, env) = writer_invocation(&read(), &read_env, vec!["backup".into()]);
        assert_eq!(argv, [OsString::from("backup")]);
        assert_eq!(env, read_env);
    }
}
