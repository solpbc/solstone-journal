// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact archive bytes in an owned target. These values never authorize release.

#[cfg(any(
    target_os = "ios",
    all(
        any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        ),
        feature = "native-archive-host-tests"
    )
))]
#[doc(hidden)]
pub mod ios;

mod creator;
mod nodes;
mod seal;
use creator::ArchivePredecessorCustody;
pub use creator::{
    ArchiveEndpointCapture, ArchivePredecessorCapture, ArchiveWriterObservation,
    ArchiveWriterSourceObservation, DirectArchiveEffects, DirectArchiveFailure,
    DirectArchiveSnapshot, OwnedArchiveCreator, OwnedArchiveCreatorAdmission,
};
pub use nodes::{ArchiveNodeRecord, parse_bounded_nodes};
pub use seal::{OwnerObservation, same_owner};
mod historical;
pub use historical::{HistoricalArchiveFile, HistoricalSnapshotFiles};

/// Tag on the snapshots that hold offloaded media. Once the local copy is
/// removed such a snapshot can be the only copy, so backup rotation keeps it.
pub const OFFLOAD_ARCHIVE_TAG: &str = "spb-offload-archive";

/// How long a repository command waits for another run's exclusive lock,
/// such as a scheduled backup cleanup, before it gives up.
pub(crate) const REPOSITORY_RETRY_LOCK: &str = "30m";

/// restic refuses a repository command outright while another run holds the
/// repository exclusively. A scheduled backup cleanup does, and it starts on
/// the same daily tick as media offload, so every command here waits for it,
/// bounded, the way the archive writer always has. While it waits restic
/// prints a notice on stdout, ahead of the JSON or file bytes these commands
/// parse, unless it runs quietly.
#[cfg(any(unix, windows, test))]
fn waiting_for_repository(mut argv: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    argv.extend([
        "--retry-lock".into(),
        REPOSITORY_RETRY_LOCK.into(),
        "--quiet".into(),
    ]);
    argv
}

/// `version` reads no repository, so it neither waits nor needs quieting.
#[cfg(any(unix, windows))]
fn query_argv(argv: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    if argv.first().is_some_and(|command| command == "version") {
        argv
    } else {
        waiting_for_repository(argv)
    }
}

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(any(all(unix, not(target_os = "ios")), windows))]
use std::time::Duration;

#[cfg(any(unix, windows, test))]
use serde_json::Value;
use solstone_core_journal_io::JournalRoot;
#[cfg(any(unix, windows, test))]
use solstone_core_journal_io::media::MediaDigest;
use solstone_core_journal_io::media::{ArchiveSource, Cleanup, OwnedMediaTarget, PreparedMedia};

pub enum ArchiveRecoveryReadRefusal {
    Qualification(ReadbackError),
    CapturedOriginUnavailable,
    Observation(solstone_core_journal_io::archive_recovery::RecoveryBindingObservationError),
}
impl fmt::Debug for ArchiveRecoveryReadRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl fmt::Display for ArchiveRecoveryReadRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Qualification(_) => "archive read qualification refused recovery observation",
            Self::CapturedOriginUnavailable => "original archive origin unavailable",
            Self::Observation(_) => "original archive recovery observation failed",
        })
    }
}
impl std::error::Error for ArchiveRecoveryReadRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Observation(error) => Some(error),
            Self::Qualification(error) => Some(error),
            Self::CapturedOriginUnavailable => None,
        }
    }
}

use crate::Destination;
use crate::archive_activity::{
    ArchiveOperationClock, OwnedPreparedArchiveHold, OwnedPreparedArchiveOperation,
    PreparedArchiveOperation, close_owned_prepared,
};
#[cfg(windows)]
mod windows_executor;
#[cfg(windows)]
pub use windows_executor::{
    WindowsArchiveCleanup, WindowsArchiveExecutor, WindowsArchiveRequest, WindowsExecutionOutput,
};

#[cfg(any(all(unix, not(target_os = "ios")), windows))]
use crate::archive_admission::ARCHIVE_LS_TIMEOUT_SECONDS;
use crate::archive_admission::Clock;
use crate::archive_admission::{AdmissionError, ArchiveBudget, ArchiveConfiguration};
#[cfg(unix)]
use crate::bounded_child::{self, CompletedChild, Request};
#[cfg(unix)]
use solstone_core_installed_payload::CurrentInstalledMember;
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::media::RootActivityCarrier;

#[cfg(unix)]
pub(crate) const RESTIC_MEMBER: &str = "lib/solstone-restic/restic";
#[cfg(unix)]
pub(crate) const RCLONE_MEMBER: &str = "lib/solstone-rclone/rclone";
pub const SUPPORTED_RESTIC_VERSION: &str = "0.19.0";

#[cfg(unix)]
enum AdmittedTool {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    Installed(Box<CurrentInstalledMember>),
    #[cfg(target_os = "android")]
    Service {
        path: PathBuf,
        device: u64,
        inode: u64,
    },
    #[cfg(any(
        target_os = "ios",
        all(
            any(
                all(target_os = "linux", target_arch = "x86_64"),
                all(target_os = "macos", target_arch = "aarch64")
            ),
            feature = "native-archive-host-tests"
        )
    ))]
    Linked(Arc<ios::IosReadContext>),
}

#[cfg(unix)]
impl AdmittedTool {
    fn program(&self) -> Result<&Path, ReadbackError> {
        match self {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Installed(member) => Ok(member.path()),
            #[cfg(target_os = "android")]
            Self::Service { path, .. } => Ok(path),
            #[cfg(any(
                target_os = "ios",
                all(
                    any(
                        all(target_os = "linux", target_arch = "x86_64"),
                        all(target_os = "macos", target_arch = "aarch64")
                    ),
                    feature = "native-archive-host-tests"
                )
            ))]
            Self::Linked(_) => Err(ReadbackError::Unsupported),
        }
    }

    fn revalidate(&self) -> Result<(), ReadbackError> {
        match self {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Self::Installed(member) => member.revalidate().map_err(|_| ReadbackError::ToolChanged),
            #[cfg(target_os = "android")]
            Self::Service {
                path,
                device,
                inode,
            } => {
                use std::os::unix::fs::MetadataExt;
                let meta = std::fs::metadata(path).map_err(|_| ReadbackError::ToolChanged)?;
                if meta.dev() != *device || meta.ino() != *inode {
                    return Err(ReadbackError::ToolChanged);
                }
                Ok(())
            }
            #[cfg(any(
                target_os = "ios",
                all(
                    any(
                        all(target_os = "linux", target_arch = "x86_64"),
                        all(target_os = "macos", target_arch = "aarch64")
                    ),
                    feature = "native-archive-host-tests"
                )
            ))]
            Self::Linked(context) => context.revalidate(),
        }
    }
}
#[cfg(any(unix, windows))]
const MAX_QUERY_BYTES: usize = 1024 * 1024;

/// Captured inputs remain unverified until actual admission succeeds.
pub struct ReadAdmission<'a> {
    operation: PreparedArchiveOperation<'a>,
    destination: Destination,
    password: String,
    repository: RepositoryBinding,
    expected_restic_path: PathBuf,
    predecessor: Option<ArchivePredecessorCustody>,
    #[cfg(any(
        target_os = "ios",
        all(
            any(
                all(target_os = "linux", target_arch = "x86_64"),
                all(target_os = "macos", target_arch = "aarch64")
            ),
            feature = "native-archive-host-tests"
        )
    ))]
    native: Option<Arc<ios::IosReadContext>>,
}
impl<'a> ReadAdmission<'a> {
    pub fn bind(
        operation: PreparedArchiveOperation<'a>,
        destination: Destination,
        password: String,
        repository_id: String,
        expected_restic_path: PathBuf,
    ) -> Self {
        Self {
            operation,
            destination,
            password,
            repository: RepositoryBinding::Expected(repository_id),
            expected_restic_path,
            predecessor: None,
            #[cfg(any(
                target_os = "ios",
                all(
                    any(
                        all(target_os = "linux", target_arch = "x86_64"),
                        all(target_os = "macos", target_arch = "aarch64")
                    ),
                    feature = "native-archive-host-tests"
                )
            ))]
            native: None,
        }
    }
    pub fn capture_current(
        operation: PreparedArchiveOperation<'a>,
        destination: Destination,
        password: String,
        expected_restic_path: PathBuf,
    ) -> Self {
        Self {
            operation,
            destination,
            password,
            repository: RepositoryBinding::CaptureCurrent,
            expected_restic_path,
            predecessor: None,
            #[cfg(any(
                target_os = "ios",
                all(
                    any(
                        all(target_os = "linux", target_arch = "x86_64"),
                        all(target_os = "macos", target_arch = "aarch64")
                    ),
                    feature = "native-archive-host-tests"
                )
            ))]
            native: None,
        }
    }
    pub fn retain_unqualified_archive(mut self, capture: ArchivePredecessorCapture) -> Self {
        self.predecessor = Some(ArchivePredecessorCustody::unknown(capture));
        self
    }
    pub fn refuse(self, reason: ReadbackError) -> ReadbackAdmissionError<'a> {
        ReadbackAdmissionError {
            reason,
            owned: AdmissionOwner::Input(Box::new(self)),
        }
    }
}

enum RepositoryBinding {
    Expected(String),
    CaptureCurrent,
    Confirmed(String),
}

pub struct ExactArchiveFile {
    pub repository_id: String,
    pub snapshot_id: String,
    pub name: String,
    /// Exact snapshot spelling returned by restic; never normalize GUID paths.
    pub archive_path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadbackError {
    Admission(AdmissionError),
    Unsupported,
    NotConfigured,
    ToolUnavailable,
    ToolChanged,
    UnsupportedToolVersion,
    UnsupportedBackend,
    CredentialsUnavailable,
    InvalidSelection,
    RepositoryIdentityChanged,
    InvalidRepositoryIdentity,
    SnapshotNodeUnavailable,
    IncompleteDiscovery,
    ToolFailed,
    NativeFailure,
    TargetRootChanged,
}

impl From<AdmissionError> for ReadbackError {
    fn from(value: AdmissionError) -> Self {
        Self::Admission(value)
    }
}

impl fmt::Display for ReadbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(error) => error.fmt(f),
            _ => f.write_str(match self {
                Self::Unsupported => "reading archived files is unavailable on this platform",
                Self::NotConfigured => "reading archived files is not configured",
                Self::ToolUnavailable => "the installed archive tool is unavailable",
                Self::ToolChanged => "the installed archive tool changed; try again",
                Self::UnsupportedToolVersion => "the installed archive tool version is unsupported",
                Self::UnsupportedBackend => {
                    "this backup destination does not support reading archived files"
                }
                Self::CredentialsUnavailable => "archive read credentials are unavailable",
                Self::InvalidSelection => "the selected archive file is invalid",
                Self::RepositoryIdentityChanged => "the backup no longer matches",
                Self::InvalidRepositoryIdentity => "the backup could not be identified",
                Self::SnapshotNodeUnavailable => "the archived file could not be confirmed",
                Self::IncompleteDiscovery => "the archive walk did not account for every node",
                Self::ToolFailed => "the archive read command failed",
                Self::NativeFailure => "the archived file could not be prepared",
                Self::TargetRootChanged => "the file could not be prepared in your journal",
                Self::Admission(_) => unreachable!(),
            }),
        }
    }
}
impl std::error::Error for ReadbackError {}

enum AdmissionOwner<'a> {
    Input(Box<ReadAdmission<'a>>),
    Admitted(Box<ReadOperation<'a>>),
}
#[must_use = "Retain the original archive admission after refusal"]
pub struct ReadbackAdmissionError<'a> {
    reason: ReadbackError,
    owned: AdmissionOwner<'a>,
}
impl fmt::Debug for ReadbackAdmissionError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadbackAdmissionError")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for ReadbackAdmissionError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(f)
    }
}
impl std::error::Error for ReadbackAdmissionError<'_> {}
impl ReadbackAdmissionError<'_> {
    pub fn reason(&self) -> ReadbackError {
        self.reason
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        match &self.owned {
            AdmissionOwner::Input(input) => input.operation.carrier(),
            AdmissionOwner::Admitted(read) => read.carrier(),
        }
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.carrier().journal_root()
    }
}

#[cfg(unix)]
enum QueryCompletion {
    Child(CompletedChild),
    #[cfg(any(
        target_os = "ios",
        all(
            any(
                all(target_os = "linux", target_arch = "x86_64"),
                all(target_os = "macos", target_arch = "aarch64")
            ),
            feature = "native-archive-host-tests"
        )
    ))]
    Native(ios::NativeQuery),
}
#[cfg(unix)]
impl QueryCompletion {
    fn successful(&self) -> bool {
        match self {
            Self::Child(child) => child.successful(),
            #[cfg(any(
                target_os = "ios",
                all(
                    any(
                        all(target_os = "linux", target_arch = "x86_64"),
                        all(target_os = "macos", target_arch = "aarch64")
                    ),
                    feature = "native-archive-host-tests"
                )
            ))]
            Self::Native(native) => native.successful(),
        }
    }
    fn output(&self) -> &crate::bounded_child::Output {
        match self {
            Self::Child(child) => child.output(),
            #[cfg(any(
                target_os = "ios",
                all(
                    any(
                        all(target_os = "linux", target_arch = "x86_64"),
                        all(target_os = "macos", target_arch = "aarch64")
                    ),
                    feature = "native-archive-host-tests"
                )
            ))]
            Self::Native(native) => native.output(),
        }
    }
}
#[cfg(unix)]
impl From<CompletedChild> for QueryCompletion {
    fn from(child: CompletedChild) -> Self {
        Self::Child(child)
    }
}

#[derive(Default)]
struct DirectExecution {
    predecessor: Option<ArchivePredecessorCustody>,
    #[cfg(windows)]
    cleanup: Vec<Arc<dyn WindowsArchiveCleanup>>,
    #[cfg(any(unix, windows))]
    pending: u64,
    #[cfg(any(unix, windows))]
    completed: [u64; 4],
    failure: Option<ReadbackError>,
}
#[cfg(any(unix, windows))]
enum DirectInvocation {
    Version,
    Config,
    List,
    Dump,
}
impl DirectExecution {
    #[cfg(any(
        all(target_os = "linux", target_arch = "x86_64"),
        target_os = "macos",
        target_os = "android",
        target_os = "ios",
        windows
    ))]
    fn qualify_read_state(&self) -> Result<(), ReadbackError> {
        if let Some(reason) = self.failure {
            return Err(reason);
        }
        if !self.helpers_quiescent() || self.completed[0] == 0 || self.completed[1] == 0 {
            return Err(ReadbackError::ToolFailed);
        }
        Ok(())
    }
    fn fail(&mut self, reason: ReadbackError) {
        self.failure.get_or_insert(reason);
    }
    /// Every child this read launched was reaped and no archive writer was ever
    /// submitted. This is not readback success.
    #[cfg(any(
        all(target_os = "linux", target_arch = "x86_64"),
        target_os = "macos",
        target_os = "android",
        target_os = "ios",
        windows
    ))]
    fn quiescent(&self) -> bool {
        self.helpers_quiescent()
            && self
                .predecessor
                .as_ref()
                .is_none_or(ArchivePredecessorCustody::unwritten)
    }
    #[cfg(any(
        all(target_os = "linux", target_arch = "x86_64"),
        target_os = "macos",
        target_os = "android",
        target_os = "ios",
        windows
    ))]
    fn helpers_quiescent(&self) -> bool {
        #[cfg(windows)]
        if self.cleanup.iter().any(|cleanup| !cleanup.quiescent()) {
            return false;
        }
        self.pending == 0
    }
    #[cfg(windows)]
    fn retain_cleanup(&mut self, output: &WindowsExecutionOutput) {
        if let Some(cleanup) = &output.cleanup {
            self.cleanup.push(Arc::clone(cleanup));
        }
    }
    #[cfg(any(unix, windows))]
    fn attempt(&mut self) {
        if let Some(next) = self.pending.checked_add(1) {
            self.pending = next;
        } else {
            self.fail(ReadbackError::ToolFailed);
        }
    }
    /// The synchronous call returned. Any Windows Job or output worker still
    /// stopping remains separately owned by its native cleanup receipt.
    #[cfg(any(unix, windows))]
    fn returned(&mut self) {
        if let Some(next) = self.pending.checked_sub(1) {
            self.pending = next;
        } else {
            self.fail(ReadbackError::ToolFailed);
        }
    }
    #[cfg(unix)]
    fn accept(&mut self, kind: DirectInvocation, child: impl Into<QueryCompletion>) {
        let child = child.into();
        if !child.successful() {
            self.fail(ReadbackError::ToolFailed);
            return;
        }
        let index = kind as usize;
        if let Some(next) = self.completed[index].checked_add(1) {
            self.completed[index] = next;
        } else {
            self.fail(ReadbackError::ToolFailed);
        }
        // Only genuine owned completion reaches this compact record. The large
        // output is released after its caller has parsed/checked the result.
        drop(child);
    }
    /// Version and config counters move only after a confirmed linked drain.
    /// A failed or still-pending drain leaves both at zero.
    #[cfg(target_os = "ios")]
    fn note_linked_qualification(&mut self, drain_ok: bool) {
        if !drain_ok {
            return;
        }
        for index in [
            DirectInvocation::Version as usize,
            DirectInvocation::Config as usize,
        ] {
            self.completed[index] = self.completed[index].saturating_add(1);
        }
    }
    #[cfg(windows)]
    fn accept_windows(&mut self, kind: DirectInvocation, successful: bool) {
        if !successful {
            self.fail(ReadbackError::ToolFailed);
            return;
        }
        let index = kind as usize;
        if let Some(next) = self.completed[index].checked_add(1) {
            self.completed[index] = next;
        } else {
            self.fail(ReadbackError::ToolFailed);
        }
    }
}

/// Compact genuine local command completions, without an execution entrance.
#[must_use]
pub struct DirectReadCompletion {
    _execution: DirectExecution,
    root: Arc<JournalRoot>,
    configuration: ArchiveConfiguration,
    budget: ArchiveBudget,
    clock: Arc<dyn Clock + Send + Sync>,
}
impl DirectReadCompletion {
    /// Recheck only the genuine released operation's original local lifetime.
    pub fn revalidate(&self) -> Result<(), ReadbackError> {
        self.budget.remaining(self.clock.as_ref())?;
        self.configuration.revalidate(&self.root)?;
        Ok(())
    }
}

#[must_use]
pub struct SettledDirectRead<'a> {
    read: ReadOperation<'a>,
}
impl SettledDirectRead<'_> {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.read.carrier()
    }
    pub fn owned_clock(&self) -> Option<&Arc<dyn Clock + Send + Sync>> {
        self.read.operation.owned_clock()
    }
    pub fn revalidate(&self) -> Result<(), ReadbackError> {
        self.read.revalidate_admission()
    }
    pub fn release(self) -> DirectReadCompletion {
        let ReadOperation {
            operation,
            root,
            execution,
            ..
        } = self.read;
        let PreparedArchiveOperation {
            activity,
            configuration,
            budget,
            clock,
        } = operation;
        drop(activity);
        let ArchiveOperationClock::Owned(clock) = clock else {
            unreachable!("settled read requires an actual owned clock")
        };
        DirectReadCompletion {
            _execution: execution.into_inner(),
            root,
            configuration,
            budget,
            clock,
        }
    }
    pub fn into_owned_hold(self) -> Result<OwnedDirectReadHold, Box<Self>> {
        self.read
            .into_owned_hold()
            .map_err(|read| Box::new(Self { read: *read }))
    }
}
#[must_use]
pub struct HeldDirectRead<'a> {
    reason: ReadbackError,
    read: ReadOperation<'a>,
}
impl HeldDirectRead<'_> {
    pub fn reason(&self) -> ReadbackError {
        self.reason
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.read.carrier()
    }
    pub fn into_owned_hold(self) -> Result<OwnedHeldDirectRead, Box<Self>> {
        let Self { reason, read } = self;
        read.into_owned_hold()
            .map(|read| OwnedHeldDirectRead {
                reason,
                read: Box::new(read),
            })
            .map_err(|read| {
                Box::new(Self {
                    reason,
                    read: *read,
                })
            })
    }
}

/// Closed original read fields. It cannot query, bind or retire a reservation.
#[must_use]
pub struct OwnedDirectReadHold {
    operation: OwnedPreparedArchiveHold,
    _root: Arc<JournalRoot>,
    #[cfg(unix)]
    _tool: AdmittedTool,
    #[cfg(windows)]
    _restic_path: PathBuf,
    #[cfg(windows)]
    _executor: Arc<dyn WindowsArchiveExecutor>,
    #[cfg(any(unix, windows))]
    _env: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    _repository: RepositoryBinding,
    _execution: DirectExecution,
}
impl OwnedDirectReadHold {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.operation.carrier()
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.operation.journal_root()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        self.operation.owned_clock()
    }

    /// End a refused read once every child it launched was reaped and the
    /// pinned tool is unchanged. The caller must already have removed every
    /// stage it created. This releases the activity, never the carrier.
    pub fn release_refused(self) -> Result<RefusedDirectRead, Box<Self>> {
        if self.qualify_release().is_err() {
            return Err(Box::new(self));
        }
        let Self {
            operation,
            _root: root,
            _execution: execution,
            ..
        } = self;
        let OwnedPreparedArchiveHold {
            activity, clock, ..
        } = operation;
        drop(activity);
        Ok(RefusedDirectRead {
            reason: execution.failure.unwrap_or(ReadbackError::ToolFailed),
            root,
            clock,
            _execution: execution,
        })
    }

    /// End a refused read with no outside finish owner, under the same
    /// conditions as `release_refused`. Its unshared carrier retires.
    pub fn release_unused(self) -> Result<(), Box<Self>> {
        if self.qualify_release().is_err() {
            return Err(Box::new(self));
        }
        self.operation.release_unused();
        Ok(())
    }

    /// Release only with the actual native carrier retirement retained.
    pub fn release_retained(
        self,
    ) -> Result<(), crate::archive_activity::UnusedArchiveRelease<Box<Self>>> {
        use crate::archive_activity::UnusedArchiveRelease;
        if self.qualify_release().is_err() {
            return Err(UnusedArchiveRelease::Held(Box::new(self)));
        }
        self.operation
            .retire_unused()
            .map_err(UnusedArchiveRelease::Retired)
    }

    fn qualify_release(&self) -> Result<(), ReadbackError> {
        #[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
        {
            pinned_member(&self._tool)?;
            self._tool.revalidate()?;
            if !self._execution.quiescent() {
                return Err(ReadbackError::ToolFailed);
            }
            Ok(())
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            self._tool.revalidate()?;
            if !self._execution.quiescent() {
                return Err(ReadbackError::ToolFailed);
            }
            if let Some(predecessor) = &self._execution.predecessor {
                predecessor.qualify_settlement()?;
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            if !self._execution.quiescent() {
                return Err(ReadbackError::ToolFailed);
            }
            Ok(())
        }
        #[cfg(not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            target_os = "macos",
            target_os = "android",
            target_os = "ios",
            windows
        )))]
        {
            Err(ReadbackError::Unsupported)
        }
    }
}

/// A refused read whose activity has ended. It never qualifies readback; only
/// the outside finish owner may retire the empty carrier it leaves.
#[must_use]
pub struct RefusedDirectRead {
    reason: ReadbackError,
    root: Arc<JournalRoot>,
    clock: Arc<dyn Clock + Send + Sync>,
    _execution: DirectExecution,
}
impl RefusedDirectRead {
    pub fn reason(&self) -> ReadbackError {
        self.reason
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.clock
    }

    /// A released read for finish-owner tests that cannot run an installed tool.
    #[cfg(feature = "full-tests")]
    #[doc(hidden)]
    pub fn for_test(root: Arc<JournalRoot>, clock: Arc<dyn Clock + Send + Sync>) -> Self {
        Self {
            reason: ReadbackError::ToolFailed,
            root,
            clock,
            _execution: DirectExecution::default(),
        }
    }
}

#[must_use]
pub struct OwnedHeldDirectRead {
    reason: ReadbackError,
    read: Box<OwnedDirectReadHold>,
}
impl OwnedHeldDirectRead {
    pub fn reason(&self) -> ReadbackError {
        self.reason
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.read.carrier()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        self.read.owned_clock()
    }
    pub fn release_refused(self) -> Result<RefusedDirectRead, Box<Self>> {
        let Self { reason, read } = self;
        (*read)
            .release_refused()
            .map_err(|read| Box::new(Self { reason, read }))
    }
}

struct OwnedReadInput {
    operation: OwnedPreparedArchiveHold,
    _destination: Destination,
    _password: String,
    _repository: RepositoryBinding,
    _expected_restic_path: PathBuf,
    _predecessor: Option<ArchivePredecessorCustody>,
    #[cfg(any(
        target_os = "ios",
        all(
            any(
                all(target_os = "linux", target_arch = "x86_64"),
                all(target_os = "macos", target_arch = "aarch64")
            ),
            feature = "native-archive-host-tests"
        )
    ))]
    _native: Option<Arc<ios::IosReadContext>>,
}
enum ClosedAdmissionOwner {
    Input(Box<OwnedReadInput>),
    Admitted(Box<OwnedDirectReadHold>),
}
#[must_use]
pub struct OwnedReadbackAdmissionError {
    reason: ReadbackError,
    owned: ClosedAdmissionOwner,
}
impl OwnedReadbackAdmissionError {
    pub fn reason(&self) -> ReadbackError {
        self.reason
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        match &self.owned {
            ClosedAdmissionOwner::Input(input) => input.operation.carrier(),
            ClosedAdmissionOwner::Admitted(read) => read.carrier(),
        }
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.carrier().journal_root()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        match &self.owned {
            ClosedAdmissionOwner::Input(input) => input.operation.owned_clock(),
            ClosedAdmissionOwner::Admitted(read) => read.owned_clock(),
        }
    }
    /// End a refused admission that leaves nothing running: unlaunched input,
    /// or an admitted read whose every child was reaped. A retained earlier
    /// archive of unknown outcome always holds.
    pub fn release(self) -> Result<(), Box<Self>> {
        let Self { reason, owned } = self;
        match owned {
            ClosedAdmissionOwner::Input(input)
                if input
                    ._predecessor
                    .as_ref()
                    .is_none_or(ArchivePredecessorCustody::unwritten) =>
            {
                input.operation.release_unused();
                Ok(())
            }
            ClosedAdmissionOwner::Admitted(read) => (*read).release_unused().map_err(|read| {
                Box::new(Self {
                    reason,
                    owned: ClosedAdmissionOwner::Admitted(read),
                })
            }),
            owned => Err(Box::new(Self { reason, owned })),
        }
    }

    /// Preserve native retirement facts when closing an app-owned admission.
    pub fn release_retained(
        self,
    ) -> Result<(), crate::archive_activity::UnusedArchiveRelease<Box<Self>>> {
        use crate::archive_activity::UnusedArchiveRelease;
        let Self { reason, owned } = self;
        match owned {
            ClosedAdmissionOwner::Input(input)
                if input
                    ._predecessor
                    .as_ref()
                    .is_none_or(ArchivePredecessorCustody::unwritten) =>
            {
                input
                    .operation
                    .retire_unused()
                    .map_err(UnusedArchiveRelease::Retired)
            }
            ClosedAdmissionOwner::Admitted(read) => {
                (*read).release_retained().map_err(|held| match held {
                    UnusedArchiveRelease::Held(read) => {
                        UnusedArchiveRelease::Held(Box::new(Self {
                            reason,
                            owned: ClosedAdmissionOwner::Admitted(read),
                        }))
                    }
                    UnusedArchiveRelease::Retired(retirement) => {
                        UnusedArchiveRelease::Retired(retirement)
                    }
                })
            }
            owned => Err(UnusedArchiveRelease::Held(Box::new(Self { reason, owned }))),
        }
    }
}
impl ReadbackAdmissionError<'_> {
    pub fn owned_clock(&self) -> Option<&Arc<dyn Clock + Send + Sync>> {
        match &self.owned {
            AdmissionOwner::Input(input) => input.operation.owned_clock(),
            AdmissionOwner::Admitted(read) => read.owned_clock(),
        }
    }
    pub fn into_owned_hold(self) -> Result<OwnedReadbackAdmissionError, Self> {
        let owned_clock = match &self.owned {
            AdmissionOwner::Input(input) => input.operation.owned_clock(),
            AdmissionOwner::Admitted(read) => read.operation.owned_clock(),
        };
        if owned_clock.is_none() {
            return Err(self);
        }
        let Self { reason, owned } = self;
        let owned = match owned {
            AdmissionOwner::Input(input) => {
                let ReadAdmission {
                    operation,
                    destination,
                    password,
                    repository,
                    expected_restic_path,
                    predecessor,
                    #[cfg(any(
                        target_os = "ios",
                        all(
                            any(
                                all(target_os = "linux", target_arch = "x86_64"),
                                all(target_os = "macos", target_arch = "aarch64")
                            ),
                            feature = "native-archive-host-tests"
                        )
                    ))]
                    native,
                } = *input;
                let operation = operation
                    .into_owned_hold()
                    .ok()
                    .expect("checked owned clock");
                ClosedAdmissionOwner::Input(Box::new(OwnedReadInput {
                    operation,
                    _destination: destination,
                    _password: password,
                    _repository: repository,
                    _expected_restic_path: expected_restic_path,
                    _predecessor: predecessor,
                    #[cfg(any(
                        target_os = "ios",
                        all(
                            any(
                                all(target_os = "linux", target_arch = "x86_64"),
                                all(target_os = "macos", target_arch = "aarch64")
                            ),
                            feature = "native-archive-host-tests"
                        )
                    ))]
                    _native: native,
                }))
            }
            AdmissionOwner::Admitted(read) => ClosedAdmissionOwner::Admitted(Box::new(
                read.into_owned_hold().ok().expect("checked owned clock"),
            )),
        };
        Ok(OwnedReadbackAdmissionError { reason, owned })
    }
}

pub struct ReadOperation<'a> {
    operation: PreparedArchiveOperation<'a>,
    root: Arc<JournalRoot>,
    #[cfg(all(unix, feature = "full-tests"))]
    source_observations: std::cell::Cell<(usize, usize)>,
    #[cfg(unix)]
    tool: AdmittedTool,
    #[cfg(windows)]
    restic_path: PathBuf,
    #[cfg(windows)]
    executor: Arc<dyn WindowsArchiveExecutor>,
    #[cfg(any(unix, windows))]
    env: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    repository: RepositoryBinding,
    execution: std::cell::RefCell<DirectExecution>,
}

/// Captured owned inputs, consumed once by the same installed read admission.
#[must_use]
pub struct OwnedReadAdmission {
    input: ReadAdmission<'static>,
    clock: Arc<dyn Clock + Send + Sync>,
}
impl OwnedPreparedArchiveOperation {
    pub fn bind_read(
        self,
        destination: Destination,
        password: String,
        repository_id: String,
        expected_restic_path: PathBuf,
    ) -> OwnedReadAdmission {
        let (operation, clock) = self.into_parts();
        OwnedReadAdmission {
            input: ReadAdmission::bind(
                operation,
                destination,
                password,
                repository_id,
                expected_restic_path,
            ),
            clock,
        }
    }

    pub fn capture_current_read(
        self,
        destination: Destination,
        password: String,
        expected_restic_path: PathBuf,
    ) -> OwnedReadAdmission {
        let (operation, clock) = self.into_parts();
        OwnedReadAdmission {
            input: ReadAdmission::capture_current(
                operation,
                destination,
                password,
                expected_restic_path,
            ),
            clock,
        }
    }
}
impl OwnedReadAdmission {
    #[cfg(any(
        target_os = "ios",
        all(
            any(
                all(target_os = "linux", target_arch = "x86_64"),
                all(target_os = "macos", target_arch = "aarch64")
            ),
            feature = "native-archive-host-tests"
        )
    ))]
    pub fn with_ios_context(mut self, context: Arc<ios::IosReadContext>) -> Self {
        self.input.native = Some(context);
        self
    }

    #[cfg(unix)]
    pub fn admit(self) -> Result<OwnedReadOperation, OwnedReadbackAdmissionError> {
        let Self { input, clock } = self;
        match ReadOperation::admit(input) {
            Ok(read) => Ok(OwnedReadOperation { read, clock }),
            Err(failure) => Err(close_owned_admission(failure, clock)),
        }
    }
    #[cfg(windows)]
    pub fn admit(
        self,
        executor: Option<Arc<dyn WindowsArchiveExecutor>>,
    ) -> Result<OwnedReadOperation, OwnedReadbackAdmissionError> {
        let Self { input, clock } = self;
        match ReadOperation::admit(input, executor) {
            Ok(read) => Ok(OwnedReadOperation { read, clock }),
            Err(failure) => Err(close_owned_admission(failure, clock)),
        }
    }
    pub fn refuse(self, reason: ReadbackError) -> OwnedReadbackAdmissionError {
        close_owned_admission(self.input.refuse(reason), self.clock)
    }
}

/// An active read whose original owned clock cannot be replaced or extracted.
#[must_use = "Consume this original read into settlement or closed custody"]
pub struct OwnedReadOperation {
    read: ReadOperation<'static>,
    clock: Arc<dyn Clock + Send + Sync>,
}
impl OwnedReadOperation {
    pub fn as_read(&self) -> &ReadOperation<'static> {
        &self.read
    }
    pub fn observe_recovery_binding(
        &self,
    ) -> Result<
        solstone_core_journal_io::archive_recovery::ArchiveRecoveryBinding,
        ArchiveRecoveryReadRefusal,
    > {
        self.read.observe_recovery_binding()
    }
    pub fn archive_origin_failure(
        &self,
    ) -> Option<&solstone_core_journal_io::archive_origin::OriginObservationFailure> {
        self.read.archive_origin_failure()
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
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.clock
    }
    pub fn revalidate_admission(&self) -> Result<(), ReadbackError> {
        self.read.revalidate_admission()
    }
    pub fn offload_required_bytes(
        &self,
        max_payload: u64,
        metadata: u64,
    ) -> Result<u64, ReadbackError> {
        self.read.offload_required_bytes(max_payload, metadata)
    }
    pub fn discover_owned_snapshot(
        &mut self,
        snapshot_id: &str,
        source: &ArchiveSource<'_>,
    ) -> Result<ExactArchiveFile, ReadbackError> {
        self.read.discover_owned_snapshot(snapshot_id, source)
    }
    pub fn discover_historical_snapshot(
        &mut self,
        snapshot_selector: &str,
        files: &[HistoricalArchiveFile<'_>],
    ) -> Result<HistoricalSnapshotFiles, ReadbackError> {
        self.read
            .discover_historical_snapshot(snapshot_selector, files)
    }
    pub fn read_owned_snapshot(
        &mut self,
        snapshot_id: &str,
        source: &ArchiveSource<'_>,
        target: OwnedMediaTarget,
    ) -> Result<(ExactArchiveFile, PreparedMedia), ReadbackFailure> {
        self.read.read_owned_snapshot(snapshot_id, source, target)
    }
    pub fn read_exact(
        &mut self,
        file: &ExactArchiveFile,
        target: OwnedMediaTarget,
    ) -> Result<PreparedMedia, ReadbackFailure> {
        self.read.read_exact(file, target)
    }
    pub fn into_owned_hold(self) -> OwnedDirectReadHold {
        close_owned_read(self.read, self.clock)
    }
    pub fn settle_direct(self) -> Result<OwnedSettledDirectRead, OwnedHeldDirectRead> {
        let Self { read, clock } = self;
        match read.settle_direct() {
            Ok(settled) => Ok(OwnedSettledDirectRead {
                read: close_owned_read(settled.read, clock),
            }),
            Err(held) => {
                let HeldDirectRead { reason, read } = *held;
                Err(OwnedHeldDirectRead {
                    reason,
                    read: Box::new(close_owned_read(read, clock)),
                })
            }
        }
    }
}

/// Genuine qualification of an owned read, with a total consuming close.
#[must_use]
pub struct OwnedSettledDirectRead {
    read: OwnedDirectReadHold,
}
impl OwnedSettledDirectRead {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.read.carrier()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        self.read.owned_clock()
    }
    pub fn revalidate(&self) -> Result<(), ReadbackError> {
        #[cfg(unix)]
        {
            self.read.operation.revalidate()?;
            self.read._tool.revalidate()
        }
        #[cfg(windows)]
        {
            self.read.operation.revalidate()?;
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(ReadbackError::Unsupported)
        }
    }
    pub fn into_owned_hold(self) -> OwnedDirectReadHold {
        self.read
    }
    pub fn release(self) -> DirectReadCompletion {
        let OwnedDirectReadHold {
            operation,
            _root: root,
            _execution: execution,
            ..
        } = self.read;
        let OwnedPreparedArchiveHold {
            activity,
            _configuration: configuration,
            _budget: budget,
            clock,
        } = operation;
        drop(activity);
        DirectReadCompletion {
            _execution: execution,
            root,
            configuration,
            budget,
            clock,
        }
    }
}

fn close_owned_read(
    read: ReadOperation<'static>,
    clock: Arc<dyn Clock + Send + Sync>,
) -> OwnedDirectReadHold {
    let ReadOperation {
        operation,
        root,
        #[cfg(all(unix, feature = "full-tests"))]
            source_observations: _,
        #[cfg(unix)]
        tool,
        #[cfg(windows)]
        restic_path,
        #[cfg(windows)]
        executor,
        #[cfg(any(unix, windows))]
        env,
        repository,
        execution,
    } = read;
    OwnedDirectReadHold {
        operation: close_owned_prepared(operation, clock),
        _root: root,
        #[cfg(unix)]
        _tool: tool,
        #[cfg(windows)]
        _restic_path: restic_path,
        #[cfg(windows)]
        _executor: executor,
        #[cfg(any(unix, windows))]
        _env: env,
        _repository: repository,
        _execution: execution.into_inner(),
    }
}

fn close_owned_admission(
    failure: ReadbackAdmissionError<'static>,
    clock: Arc<dyn Clock + Send + Sync>,
) -> OwnedReadbackAdmissionError {
    let ReadbackAdmissionError { reason, owned } = failure;
    let owned = match owned {
        AdmissionOwner::Input(input) => {
            let ReadAdmission {
                operation,
                destination,
                password,
                repository,
                expected_restic_path,
                predecessor,
                #[cfg(any(
                    target_os = "ios",
                    all(
                        any(
                            all(target_os = "linux", target_arch = "x86_64"),
                            all(target_os = "macos", target_arch = "aarch64")
                        ),
                        feature = "native-archive-host-tests"
                    )
                ))]
                native,
            } = *input;
            ClosedAdmissionOwner::Input(Box::new(OwnedReadInput {
                operation: close_owned_prepared(operation, clock),
                _destination: destination,
                _password: password,
                _repository: repository,
                _expected_restic_path: expected_restic_path,
                _predecessor: predecessor,
                #[cfg(any(
                    target_os = "ios",
                    all(
                        any(
                            all(target_os = "linux", target_arch = "x86_64"),
                            all(target_os = "macos", target_arch = "aarch64")
                        ),
                        feature = "native-archive-host-tests"
                    )
                ))]
                _native: native,
            }))
        }
        AdmissionOwner::Admitted(read) => {
            ClosedAdmissionOwner::Admitted(Box::new(close_owned_read(*read, clock)))
        }
    };
    OwnedReadbackAdmissionError { reason, owned }
}

#[derive(Clone)]
enum WriterState {
    NotLaunched,
    #[cfg(windows)]
    Pending(Arc<dyn WindowsArchiveCleanup>),
    #[cfg(any(unix, windows))]
    Unknown,
    #[cfg(any(unix, windows))]
    QualifiedCompletion,
}

enum OwnedReadback {
    Target(OwnedMediaTarget),
    #[cfg(any(unix, windows))]
    Prepared(PreparedMedia),
}

pub struct ReadbackFailure {
    carrier: Arc<RootActivityCarrier>,
    reason: ReadbackError,
    writer: WriterState,
    owned: Box<OwnedReadback>,
}

impl fmt::Debug for ReadbackFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadbackFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for ReadbackFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(f)
    }
}
impl std::error::Error for ReadbackFailure {}

impl ReadbackFailure {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn reason(&self) -> ReadbackError {
        self.reason
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[doc(hidden)]
    pub fn prepared_digest_for_test(&self) -> Option<&MediaDigest> {
        match self.owned.as_ref() {
            OwnedReadback::Prepared(prepared) => Some(prepared.digest()),
            OwnedReadback::Target(_) => None,
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[doc(hidden)]
    pub fn target_transport_issued_for_test(&self) -> Option<bool> {
        match self.owned.as_ref() {
            OwnedReadback::Target(target) => Some(target.helper_transport_issued()),
            OwnedReadback::Prepared(_) => None,
        }
    }

    /// Unknown writer lifetime never grants cleanup, including on Drop.
    pub fn cleanup(&mut self) -> Cleanup {
        #[cfg(windows)]
        if let WriterState::Pending(cleanup) = &self.writer {
            if !cleanup.quiescent() {
                return Cleanup::Pending;
            }
            self.writer = WriterState::QualifiedCompletion;
        }
        match (self.owned.as_mut(), &self.writer) {
            #[cfg(any(unix, windows))]
            (_, WriterState::Unknown) => Cleanup::Refused,
            (OwnedReadback::Target(target), _) => target.cleanup_after_helper_quiescence(),
            #[cfg(any(unix, windows))]
            (OwnedReadback::Prepared(prepared), _) => prepared.cleanup(),
        }
    }

    fn target(
        reason: ReadbackError,
        writer: WriterState,
        target: OwnedMediaTarget,
        carrier: Arc<RootActivityCarrier>,
    ) -> Self {
        Self {
            carrier,
            reason,
            writer,
            owned: Box::new(OwnedReadback::Target(target)),
        }
    }
}

pub fn full_hex_identity(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn plain_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
}

/// Validate exact restic spelling without normalization, including its narrow
/// Windows snapshot-volume first component.
pub fn exact_archive_path(value: &str) -> bool {
    let Some(path) = value.strip_prefix('/') else {
        return false;
    };
    let mut components = path.split('/');
    let Some(first) = components.next() else {
        return false;
    };
    if snapshot_volume(first) {
        let Some(next) = components.next() else {
            return false;
        };
        plain_name(next) && components.all(plain_name)
    } else {
        plain_name(first) && components.all(plain_name)
    }
}

fn snapshot_volume(value: &str) -> bool {
    let Some(guid) = value
        .strip_prefix(r"\\?\Volume{")
        .and_then(|guid| guid.strip_suffix('}'))
    else {
        return false;
    };
    guid.len() == 36
        && guid.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

impl ExactArchiveFile {
    pub fn validate(&self) -> Result<(), ReadbackError> {
        if !full_hex_identity(&self.repository_id)
            || !full_hex_identity(&self.snapshot_id)
            || !full_hex_identity(&self.sha256)
            || !plain_name(&self.name)
            || !exact_archive_path(&self.archive_path)
            || self.archive_path.rsplit('/').next() != Some(self.name.as_str())
        {
            return Err(ReadbackError::InvalidSelection);
        }
        Ok(())
    }
}

#[cfg(any(unix, windows))]
fn direct_environment(
    destination: &Destination,
    password: &str,
) -> Result<std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>, ReadbackError> {
    // restic 0.19.0 cmd_dump.go creates the target in the root process;
    // internal/dump/common.go joins its in-process writers before returning.
    // The direct S3/B2 backends do not launch an external transport. The only
    // password-command execution in internal/global is excluded by this fixed
    // environment and argv. This restriction does not qualify arbitrary tools.
    let scheme = match destination.backend.as_str() {
        "s3" => "s3:",
        "b2" => "b2:",
        _ => return Err(ReadbackError::UnsupportedBackend),
    };
    if !destination.repository.starts_with(scheme)
        || destination.repository[scheme.len()..].trim().is_empty()
        || destination.repository.chars().any(char::is_control)
    {
        return Err(ReadbackError::UnsupportedBackend);
    }
    if password.is_empty() {
        return Err(ReadbackError::CredentialsUnavailable);
    }
    let backend = crate::assemble_backend_env(destination)
        .map_err(|_| ReadbackError::CredentialsUnavailable)?;
    if backend
        .values()
        .any(|value| value.as_str().is_none_or(|s| s.trim().is_empty()))
    {
        // Empty AWS credentials fall through to ambient credential providers.
        return Err(ReadbackError::CredentialsUnavailable);
    }
    let mut env = std::collections::BTreeMap::from([
        (
            "RESTIC_REPOSITORY".into(),
            destination.repository.as_str().into(),
        ),
        ("RESTIC_PASSWORD".into(), password.into()),
    ]);
    for (key, value) in backend {
        env.insert(key.into(), value.as_str().expect("assembled string").into());
    }
    Ok(env)
}

impl<'a> ReadOperation<'a> {
    pub fn owned_clock(&self) -> Option<&Arc<dyn Clock + Send + Sync>> {
        self.operation.owned_clock()
    }
    pub fn into_owned_hold(self) -> Result<OwnedDirectReadHold, Box<Self>> {
        if self.operation.owned_clock().is_none() {
            return Err(Box::new(self));
        }
        let Self {
            operation,
            root,
            #[cfg(all(unix, feature = "full-tests"))]
                source_observations: _,
            #[cfg(unix)]
            tool,
            #[cfg(windows)]
            restic_path,
            #[cfg(windows)]
            executor,
            #[cfg(any(unix, windows))]
            env,
            repository,
            execution,
        } = self;
        let operation = operation
            .into_owned_hold()
            .ok()
            .expect("checked owned clock");
        Ok(OwnedDirectReadHold {
            operation,
            _root: root,
            #[cfg(unix)]
            _tool: tool,
            #[cfg(windows)]
            _restic_path: restic_path,
            #[cfg(windows)]
            _executor: executor,
            #[cfg(any(unix, windows))]
            _env: env,
            _repository: repository,
            _execution: execution.into_inner(),
        })
    }
    fn qualify_direct(&self) -> Result<(), ReadbackError> {
        self.revalidate_admission()?;
        if self.operation.owned_clock().is_none() {
            return Err(ReadbackError::Unsupported);
        }
        #[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
        {
            pinned_member(&self.tool)?;
            let execution = self.execution.borrow();
            if let Some(predecessor) = &execution.predecessor {
                predecessor.qualify_settlement()?;
            }
            execution.qualify_read_state()
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            self.tool.revalidate()?;
            let execution = self.execution.borrow();
            if let Some(predecessor) = &execution.predecessor {
                predecessor.qualify_settlement()?;
            }
            execution.qualify_read_state()
        }
        #[cfg(windows)]
        {
            let execution = self.execution.borrow();
            if let Some(predecessor) = &execution.predecessor {
                predecessor.qualify_settlement()?;
            }
            execution.qualify_read_state()
        }
        #[cfg(not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            target_os = "macos",
            target_os = "android",
            target_os = "ios",
            windows
        )))]
        {
            Err(ReadbackError::Unsupported)
        }
    }

    pub fn settle_direct(self) -> Result<SettledDirectRead<'a>, Box<HeldDirectRead<'a>>> {
        let qualify = self.qualify_direct();
        match qualify {
            Ok(()) => Ok(SettledDirectRead { read: self }),
            Err(reason) => Err(Box::new(HeldDirectRead { reason, read: self })),
        }
    }
}

impl<'a> ReadOperation<'a> {
    #[cfg(not(any(unix, windows)))]
    pub fn admit(input: ReadAdmission<'a>) -> Result<Self, ReadbackAdmissionError<'a>> {
        Err(ReadbackAdmissionError {
            reason: ReadbackError::Unsupported,
            owned: AdmissionOwner::Input(Box::new(input)),
        })
    }
    #[cfg(windows)]
    pub fn admit(
        input: ReadAdmission<'a>,
        executor: Option<Arc<dyn WindowsArchiveExecutor>>,
    ) -> Result<Self, ReadbackAdmissionError<'a>> {
        let executor = match executor {
            Some(executor) => executor,
            None => {
                return Err(ReadbackAdmissionError {
                    reason: ReadbackError::Unsupported,
                    owned: AdmissionOwner::Input(Box::new(input)),
                });
            }
        };
        let admission = (|| {
            input.operation.revalidate()?;
            let configuration = input.operation.configuration();
            if !configuration.enabled() {
                return Err(ReadbackError::NotConfigured);
            }
            let keys = configuration.keys()?.ok_or(ReadbackError::NotConfigured)?;
            if input.password != keys.daily_key {
                return Err(ReadbackError::CredentialsUnavailable);
            }
            match configuration.mode() {
                Some("byo") if configuration.destination().as_ref() == Some(&input.destination) => {
                }
                Some("operated")
                    if configuration.binding().is_some() && input.destination.backend == "s3" => {}
                _ => return Err(ReadbackError::NotConfigured),
            }
            if matches!(&input.repository, RepositoryBinding::Expected(id) if !full_hex_identity(id))
            {
                return Err(ReadbackError::InvalidRepositoryIdentity);
            }
            if let Some(predecessor) = &input.predecessor {
                predecessor.validate_input(&input)?;
            }
            let env = direct_environment(&input.destination, &input.password)?;
            let restic_path = input.expected_restic_path.clone();
            Ok((env, restic_path))
        })();
        let (env, restic_path) = match admission {
            Ok(admitted) => admitted,
            Err(reason) => {
                return Err(ReadbackAdmissionError {
                    reason,
                    owned: AdmissionOwner::Input(Box::new(input)),
                });
            }
        };
        let mut admitted = Self {
            root: Arc::clone(input.operation.journal_root()),
            operation: input.operation,
            #[cfg(all(unix, feature = "full-tests"))]
            source_observations: std::cell::Cell::new((0, 0)),
            restic_path,
            executor,
            env,
            repository: input.repository,
            execution: std::cell::RefCell::new(DirectExecution {
                predecessor: input.predecessor,
                ..DirectExecution::default()
            }),
        };
        let qualification = (|| {
            let version_output = admitted.query(vec!["version".into()])?;
            let version = std::str::from_utf8(&version_output.stdout)
                .map_err(|_| ReadbackError::UnsupportedToolVersion)?;
            let mut fields = version.split_whitespace();
            if fields.next() != Some("restic") || fields.next() != Some(SUPPORTED_RESTIC_VERSION) {
                return Err(ReadbackError::UnsupportedToolVersion);
            }
            admitted
                .execution
                .borrow_mut()
                .accept_windows(DirectInvocation::Version, version_output.successful());
            admitted.confirm_repository()?;
            Ok(())
        })();
        match qualification {
            Ok(()) => Ok(admitted),
            Err(reason) => Err(ReadbackAdmissionError {
                reason,
                owned: AdmissionOwner::Admitted(Box::new(admitted)),
            }),
        }
    }

    #[cfg(unix)]
    pub fn admit(input: ReadAdmission<'a>) -> Result<Self, ReadbackAdmissionError<'a>> {
        let admission = (|| {
            input.operation.revalidate()?;
            let configuration = input.operation.configuration();
            if !configuration.enabled() {
                return Err(ReadbackError::NotConfigured);
            }
            let keys = configuration.keys()?.ok_or(ReadbackError::NotConfigured)?;
            if input.password != keys.daily_key {
                return Err(ReadbackError::CredentialsUnavailable);
            }
            match configuration.mode() {
                Some("byo") if configuration.destination().as_ref() == Some(&input.destination) => {
                }
                Some("operated")
                    if configuration.binding().is_some() && input.destination.backend == "s3" => {}
                _ => return Err(ReadbackError::NotConfigured),
            }
            if matches!(&input.repository, RepositoryBinding::Expected(id) if !full_hex_identity(id))
            {
                return Err(ReadbackError::InvalidRepositoryIdentity);
            }
            if let Some(predecessor) = &input.predecessor {
                predecessor.validate_input(&input)?;
            }
            #[cfg(all(
                any(
                    all(target_os = "linux", target_arch = "x86_64"),
                    all(target_os = "macos", target_arch = "aarch64")
                ),
                feature = "native-archive-host-tests"
            ))]
            let native_local_control = input
                .native
                .as_ref()
                .is_some_and(|context| context.is_local_host_control());
            #[cfg(not(all(
                any(
                    all(target_os = "linux", target_arch = "x86_64"),
                    all(target_os = "macos", target_arch = "aarch64")
                ),
                feature = "native-archive-host-tests"
            )))]
            let native_local_control = false;
            let env = if native_local_control {
                std::collections::BTreeMap::new()
            } else {
                direct_environment(&input.destination, &input.password)?
            };
            let tool = admit_tool(&input)?;
            Ok((env, tool))
        })();
        let (env, tool) = match admission {
            Ok(admitted) => admitted,
            Err(reason) => {
                return Err(ReadbackAdmissionError {
                    reason,
                    owned: AdmissionOwner::Input(Box::new(input)),
                });
            }
        };
        let mut admitted = Self {
            root: Arc::clone(input.operation.journal_root()),
            operation: input.operation,
            #[cfg(feature = "full-tests")]
            source_observations: std::cell::Cell::new((0, 0)),
            tool,
            env,
            repository: input.repository,
            execution: std::cell::RefCell::new(DirectExecution {
                predecessor: input.predecessor,
                ..DirectExecution::default()
            }),
        };
        let qualification = (|| {
            let version_child = admitted.query(vec!["version".into()])?;
            let version = std::str::from_utf8(&version_child.output().stdout)
                .map_err(|_| ReadbackError::UnsupportedToolVersion)?;
            let mut fields = version.split_whitespace();
            if fields.next() != Some("restic") || fields.next() != Some(SUPPORTED_RESTIC_VERSION) {
                return Err(ReadbackError::UnsupportedToolVersion);
            }
            admitted
                .execution
                .borrow_mut()
                .accept(DirectInvocation::Version, version_child);
            admitted.confirm_repository()?;
            Ok(())
        })();
        match qualification {
            Ok(()) => Ok(admitted),
            Err(reason) => Err(ReadbackAdmissionError {
                reason,
                owned: AdmissionOwner::Admitted(Box::new(admitted)),
            }),
        }
    }

    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.operation.carrier()
    }
    /// Forward DATA from this read's original owners. No work is settled or released.
    pub fn observe_recovery_binding(
        &self,
    ) -> Result<
        solstone_core_journal_io::archive_recovery::ArchiveRecoveryBinding,
        ArchiveRecoveryReadRefusal,
    > {
        self.qualify_direct()
            .map_err(ArchiveRecoveryReadRefusal::Qualification)?;
        let origin = self
            .operation
            .activity
            .origin()
            .map_err(|_| ArchiveRecoveryReadRefusal::CapturedOriginUnavailable)?;
        let binding = self
            .carrier()
            .observe_recovery_binding(origin, self.repository_id())
            .map_err(ArchiveRecoveryReadRefusal::Observation)?;
        self.qualify_direct()
            .map_err(ArchiveRecoveryReadRefusal::Qualification)?;
        Ok(binding)
    }
    pub fn archive_origin_failure(
        &self,
    ) -> Option<&solstone_core_journal_io::archive_origin::OriginObservationFailure> {
        self.operation.activity.origin().err()
    }

    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.operation.activity()
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn repository_id(&self) -> &str {
        match &self.repository {
            RepositoryBinding::Confirmed(id) => id,
            _ => unreachable!("only confirmed read operations leave admission"),
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[doc(hidden)]
    pub fn source_observations_for_test(&self) -> (usize, usize) {
        self.source_observations.get()
    }

    /// Recheck retained local admission under the original budget and cancellation.
    /// This does not query the repository or establish readback or release authority.
    /// Callers still check canonical binding and current proof at their effects.
    pub fn revalidate_admission(&self) -> Result<(), ReadbackError> {
        #[cfg(any(unix, windows))]
        {
            self.revalidate_local()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(ReadbackError::Unsupported)
        }
    }

    pub fn offload_required_bytes(
        &self,
        max_payload: u64,
        metadata: u64,
    ) -> Result<u64, ReadbackError> {
        self.revalidate_admission()?;
        Ok(self
            .operation
            .configuration()
            .offload_required_bytes(max_payload, metadata)?)
    }

    #[cfg(unix)]
    fn revalidate_local(&self) -> Result<(), ReadbackError> {
        self.operation.revalidate()?;
        self.tool.revalidate()
    }

    #[cfg(windows)]
    fn revalidate_local(&self) -> Result<(), ReadbackError> {
        Ok(self.operation.revalidate()?)
    }

    /// A failure reports whether nothing of the run is still alive.
    #[cfg(unix)]
    fn run_child(
        &self,
        request: &Request<'_>,
        capture_limit: usize,
    ) -> Result<CompletedChild, (ReadbackError, bool)> {
        let creator = self
            .execution
            .borrow()
            .predecessor
            .as_ref()
            .is_some_and(ArchivePredecessorCustody::creator_executor);
        self.execution.borrow_mut().attempt();
        #[cfg(not(target_os = "macos"))]
        let completed = if creator {
            bounded_child::run_owned_archive_creator(
                request,
                Some(capture_limit),
                self.operation.budget().cancellation(),
            )
        } else {
            bounded_child::run_owned(
                request,
                Some(capture_limit),
                self.operation.budget().cancellation(),
            )
        };
        // macOS reads run as ordinary children, as in restore; only the
        // creator's `backup` runs under the parent watch.
        #[cfg(target_os = "macos")]
        let completed = {
            let _ = creator;
            bounded_child::run_owned(
                request,
                Some(capture_limit),
                self.operation.budget().cancellation(),
            )
        };
        match completed {
            Ok(completed) => {
                self.execution.borrow_mut().returned();
                Ok(completed)
            }
            Err(failure) => {
                if failure.settled() {
                    self.execution.borrow_mut().returned();
                }
                Err((self.execution_error(), failure.settled()))
            }
        }
    }

    #[cfg(unix)]
    fn query(&self, argv: Vec<std::ffi::OsString>) -> Result<QueryCompletion, ReadbackError> {
        self.query_inner(argv)
            .inspect_err(|reason| self.execution.borrow_mut().fail(*reason))
    }
    #[cfg(unix)]
    fn query_inner(&self, argv: Vec<std::ffi::OsString>) -> Result<QueryCompletion, ReadbackError> {
        #[cfg(any(
            target_os = "ios",
            all(
                any(
                    all(target_os = "linux", target_arch = "x86_64"),
                    all(target_os = "macos", target_arch = "aarch64")
                ),
                feature = "native-archive-host-tests"
            )
        ))]
        {
            #[cfg(target_os = "ios")]
            let AdmittedTool::Linked(context) = &self.tool;
            #[cfg(target_os = "ios")]
            return context.query(self, &argv).map(QueryCompletion::Native);
            #[cfg(not(target_os = "ios"))]
            if let AdmittedTool::Linked(context) = &self.tool {
                return context.query(self, &argv).map(QueryCompletion::Native);
            }
        }
        #[cfg(not(target_os = "ios"))]
        {
            self.revalidate_local()?;
            let timeout = self
                .operation
                .budget()
                .remaining(self.operation.clock())?
                .min(Duration::from_secs(ARCHIVE_LS_TIMEOUT_SECONDS));
            let output = self
                .run_child(
                    &Request {
                        program: self.tool.program()?.as_os_str().to_owned(),
                        argv: query_argv(argv),
                        env: self.env.clone(),
                        timeout: Some(timeout),
                        pass_fds: vec![],
                    },
                    MAX_QUERY_BYTES,
                )
                .map_err(|(reason, _)| reason)?;
            self.revalidate_local()?;
            if !output.successful() {
                return Err(ReadbackError::ToolFailed);
            }
            Ok(output.into())
        }
    }

    #[cfg(windows)]
    fn query(
        &self,
        argv: Vec<std::ffi::OsString>,
    ) -> Result<WindowsExecutionOutput, ReadbackError> {
        self.query_inner(argv)
            .inspect_err(|reason| self.execution.borrow_mut().fail(*reason))
    }
    #[cfg(windows)]
    fn query_inner(
        &self,
        argv: Vec<std::ffi::OsString>,
    ) -> Result<WindowsExecutionOutput, ReadbackError> {
        self.revalidate_local()?;
        let timeout = self
            .operation
            .budget()
            .remaining(self.operation.clock())?
            .min(Duration::from_secs(ARCHIVE_LS_TIMEOUT_SECONDS));
        let argv = query_argv(argv);
        self.execution.borrow_mut().attempt();
        let cancel = self.operation.budget().cancellation();
        let output = self.executor.run_cancellable(
            WindowsArchiveRequest {
                program: &self.restic_path,
                argv: &argv,
                env: &self.env,
                timeout,
                cancellation: cancel,
                retained: vec![Arc::clone(self.carrier()) as Arc<dyn Send + Sync>],
            },
            MAX_QUERY_BYTES,
        );
        {
            let mut execution = self.execution.borrow_mut();
            execution.returned();
            execution.retain_cleanup(&output);
        }
        if !output.quiescent() {
            return Err(self.execution_error());
        }
        self.revalidate_local()?;
        if !output.successful() {
            return Err(ReadbackError::ToolFailed);
        }
        Ok(output)
    }

    #[cfg(any(unix, windows))]
    fn execution_error(&self) -> ReadbackError {
        match self.operation.budget().remaining(self.operation.clock()) {
            Err(error) => error.into(),
            Ok(_) => ReadbackError::ToolFailed,
        }
    }

    #[cfg(unix)]
    fn confirm_repository(&mut self) -> Result<(), ReadbackError> {
        if let Some(predecessor) = self.execution.borrow_mut().predecessor.as_mut() {
            predecessor.mark_read_config();
        }
        let result = self.query(vec!["cat".into(), "config".into(), "--no-cache".into()])?;
        let parsed: Value = serde_json::from_slice(&result.output().stdout)
            .map_err(|_| ReadbackError::InvalidRepositoryIdentity)?;
        let id = parsed
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| full_hex_identity(id))
            .ok_or(ReadbackError::InvalidRepositoryIdentity)?;
        if matches!(&self.repository, RepositoryBinding::Expected(expected) if expected != id) {
            return Err(ReadbackError::RepositoryIdentityChanged);
        }
        self.revalidate_local()?;
        self.repository = RepositoryBinding::Confirmed(id.to_owned());
        if let Some(predecessor) = self.execution.borrow_mut().predecessor.as_mut() {
            predecessor.confirm_read_config(id);
        }
        self.execution
            .borrow_mut()
            .accept(DirectInvocation::Config, result);
        Ok(())
    }

    #[cfg(windows)]
    fn confirm_repository(&mut self) -> Result<(), ReadbackError> {
        if let Some(predecessor) = self.execution.borrow_mut().predecessor.as_mut() {
            predecessor.mark_read_config();
        }
        let result = self.query(vec!["cat".into(), "config".into(), "--no-cache".into()])?;
        let parsed: Value = serde_json::from_slice(&result.stdout)
            .map_err(|_| ReadbackError::InvalidRepositoryIdentity)?;
        let id = parsed
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| full_hex_identity(id))
            .ok_or(ReadbackError::InvalidRepositoryIdentity)?;
        if matches!(&self.repository, RepositoryBinding::Expected(expected) if expected != id) {
            return Err(ReadbackError::RepositoryIdentityChanged);
        }
        self.revalidate_local()?;
        self.repository = RepositoryBinding::Confirmed(id.to_owned());
        if let Some(predecessor) = self.execution.borrow_mut().predecessor.as_mut() {
            predecessor.confirm_read_config(id);
        }
        self.execution
            .borrow_mut()
            .accept_windows(DirectInvocation::Config, result.successful());
        Ok(())
    }

    #[cfg(unix)]
    fn check_repository(&self) -> Result<(), ReadbackError> {
        {
            let result = self.query(vec!["cat".into(), "config".into(), "--no-cache".into()])?;
            let parsed: Value = serde_json::from_slice(&result.output().stdout)
                .map_err(|_| ReadbackError::InvalidRepositoryIdentity)?;
            let id = parsed
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| full_hex_identity(id))
                .ok_or(ReadbackError::InvalidRepositoryIdentity)?;
            if id != self.repository_id() {
                return Err(ReadbackError::RepositoryIdentityChanged);
            }
            self.execution
                .borrow_mut()
                .accept(DirectInvocation::Config, result);
            Ok(())
        }
    }

    #[cfg(windows)]
    fn check_repository(&self) -> Result<(), ReadbackError> {
        let result = self.query(vec!["cat".into(), "config".into(), "--no-cache".into()])?;
        let parsed: Value = serde_json::from_slice(&result.stdout)
            .map_err(|_| ReadbackError::InvalidRepositoryIdentity)?;
        let id = parsed
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| full_hex_identity(id))
            .ok_or(ReadbackError::InvalidRepositoryIdentity)?;
        if id != self.repository_id() {
            return Err(ReadbackError::RepositoryIdentityChanged);
        }
        self.execution
            .borrow_mut()
            .accept_windows(DirectInvocation::Config, result.successful());
        Ok(())
    }

    #[cfg(unix)]
    fn check_node(&self, file: &ExactArchiveFile) -> Result<(), ReadbackError> {
        let output = self.query(vec![
            "ls".into(),
            "--json".into(),
            file.snapshot_id.as_str().into(),
            file.archive_path.as_str().into(),
            "--no-cache".into(),
        ])?;
        exact_regular_node(&output.output().stdout, file)?;
        self.execution
            .borrow_mut()
            .accept(DirectInvocation::List, output);
        Ok(())
    }

    #[cfg(windows)]
    fn check_node(&self, file: &ExactArchiveFile) -> Result<(), ReadbackError> {
        let output = self.query(vec![
            "ls".into(),
            "--json".into(),
            file.snapshot_id.as_str().into(),
            file.archive_path.as_str().into(),
            "--no-cache".into(),
        ])?;
        exact_regular_node(&output.stdout, file)?;
        self.execution
            .borrow_mut()
            .accept_windows(DirectInvocation::List, output.successful());
        Ok(())
    }

    #[cfg(any(unix, windows))]
    fn revalidate_source(&self, source: &ArchiveSource<'_>) -> Result<(), ReadbackError> {
        self.revalidate_local()?;
        let result = source.revalidate();
        #[cfg(all(unix, feature = "full-tests"))]
        {
            let (observed, failed) = self.source_observations.get();
            self.source_observations
                .set((observed + 1, failed + usize::from(result.is_err())));
        }
        self.revalidate_local()?;
        result.map_err(|_| ReadbackError::NativeFailure)
    }

    /// Observe one retained source's exact snapshot path. This checks metadata;
    /// content verification still requires an independent read of its bytes.
    pub fn discover_owned_snapshot(
        &mut self,
        snapshot_id: &str,
        source: &ArchiveSource<'_>,
    ) -> Result<ExactArchiveFile, ReadbackError> {
        self.discover_owned_snapshot_inner(snapshot_id, source)
            .inspect_err(|reason| self.execution.borrow_mut().fail(*reason))
    }

    fn discover_owned_snapshot_inner(
        &mut self,
        snapshot_id: &str,
        source: &ArchiveSource<'_>,
    ) -> Result<ExactArchiveFile, ReadbackError> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (snapshot_id, source);
            Err(ReadbackError::Unsupported)
        }
        #[cfg(windows)]
        {
            self.revalidate_source(source)?;
            let name = source
                .name()
                .to_str()
                .filter(|name| plain_name(name))
                .ok_or(ReadbackError::InvalidSelection)?;
            if !full_hex_identity(snapshot_id) || !full_hex_identity(&source.digest().sha256) {
                return Err(ReadbackError::InvalidSelection);
            }
            self.check_repository()?;
            self.revalidate_source(source)?;
            let output = self.query(vec![
                "ls".into(),
                "--json".into(),
                snapshot_id.into(),
                "--no-cache".into(),
            ])?;
            let file = owned_snapshot_file(
                &output.stdout,
                self.repository_id(),
                snapshot_id,
                name,
                source.digest(),
            )?;
            self.revalidate_source(source)?;
            self.execution
                .borrow_mut()
                .accept_windows(DirectInvocation::List, output.successful());
            Ok(file)
        }
        #[cfg(unix)]
        {
            self.revalidate_source(source)?;
            let name = source
                .name()
                .to_str()
                .filter(|name| plain_name(name))
                .ok_or(ReadbackError::InvalidSelection)?;
            if !full_hex_identity(snapshot_id) || !full_hex_identity(&source.digest().sha256) {
                return Err(ReadbackError::InvalidSelection);
            }
            self.check_repository()?;
            self.revalidate_source(source)?;
            let output = self.query(vec![
                "ls".into(),
                "--json".into(),
                snapshot_id.into(),
                "--no-cache".into(),
            ])?;
            let file = owned_snapshot_file(
                &output.output().stdout,
                self.repository_id(),
                snapshot_id,
                name,
                source.digest(),
            )?;
            self.revalidate_source(source)?;
            self.execution
                .borrow_mut()
                .accept(DirectInvocation::List, output);
            Ok(file)
        }
    }

    /// Discover one retained source's exact snapshot path and independently read
    /// its bytes. Other group members must have distinct original basenames.
    pub fn read_owned_snapshot(
        &mut self,
        snapshot_id: &str,
        source: &ArchiveSource<'_>,
        target: OwnedMediaTarget,
    ) -> Result<(ExactArchiveFile, PreparedMedia), ReadbackFailure> {
        self.read_owned_snapshot_inner(snapshot_id, source, target)
            .inspect_err(|failure| self.execution.borrow_mut().fail(failure.reason))
    }

    fn read_owned_snapshot_inner(
        &mut self,
        snapshot_id: &str,
        source: &ArchiveSource<'_>,
        target: OwnedMediaTarget,
    ) -> Result<(ExactArchiveFile, PreparedMedia), ReadbackFailure> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (snapshot_id, source);
            Err(ReadbackFailure::target(
                ReadbackError::Unsupported,
                WriterState::NotLaunched,
                target,
                Arc::clone(self.carrier()),
            ))
        }
        #[cfg(windows)]
        {
            if target.helper_transport_issued() {
                return Err(ReadbackFailure::target(
                    ReadbackError::NativeFailure,
                    WriterState::Unknown,
                    target,
                    Arc::clone(self.carrier()),
                ));
            }
            let discovery = self.discover_owned_snapshot(snapshot_id, source);
            let file = match discovery {
                Ok(file) => file,
                Err(reason) => {
                    return Err(ReadbackFailure::target(
                        reason,
                        WriterState::NotLaunched,
                        target,
                        Arc::clone(self.carrier()),
                    ));
                }
            };
            let prepared = self.read_exact(&file, target)?;
            if let Err(reason) = self.revalidate_source(source) {
                return Err(ReadbackFailure {
                    carrier: Arc::clone(self.carrier()),
                    reason,
                    writer: WriterState::QualifiedCompletion,
                    owned: Box::new(OwnedReadback::Prepared(prepared)),
                });
            }
            Ok((file, prepared))
        }
        #[cfg(unix)]
        {
            if target.helper_transport_issued() {
                return Err(ReadbackFailure::target(
                    ReadbackError::NativeFailure,
                    WriterState::Unknown,
                    target,
                    Arc::clone(self.carrier()),
                ));
            }
            let discovery = self.discover_owned_snapshot(snapshot_id, source);
            let file = match discovery {
                Ok(file) => file,
                Err(reason) => {
                    return Err(ReadbackFailure::target(
                        reason,
                        WriterState::NotLaunched,
                        target,
                        Arc::clone(self.carrier()),
                    ));
                }
            };
            let prepared = self.read_exact(&file, target)?;
            if let Err(reason) = self.revalidate_source(source) {
                return Err(ReadbackFailure {
                    carrier: Arc::clone(self.carrier()),
                    reason,
                    writer: WriterState::QualifiedCompletion,
                    owned: Box::new(OwnedReadback::Prepared(prepared)),
                });
            }
            Ok((file, prepared))
        }
    }

    pub fn read_exact(
        &mut self,
        file: &ExactArchiveFile,
        target: OwnedMediaTarget,
    ) -> Result<PreparedMedia, ReadbackFailure> {
        self.read_exact_inner(file, target)
            .inspect_err(|failure| self.execution.borrow_mut().fail(failure.reason))
    }

    fn read_exact_inner(
        &mut self,
        file: &ExactArchiveFile,
        target: OwnedMediaTarget,
    ) -> Result<PreparedMedia, ReadbackFailure> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            Err(ReadbackFailure::target(
                ReadbackError::Unsupported,
                WriterState::NotLaunched,
                target,
                Arc::clone(self.carrier()),
            ))
        }
        #[cfg(windows)]
        {
            let mut target = target;
            if target.helper_transport_issued() {
                return Err(ReadbackFailure::target(
                    ReadbackError::NativeFailure,
                    WriterState::Unknown,
                    target,
                    Arc::clone(self.carrier()),
                ));
            }
            let mut writer = WriterState::NotLaunched;
            let preflight = (|| {
                file.validate()?;
                if file.repository_id != self.repository_id() {
                    return Err(ReadbackError::RepositoryIdentityChanged);
                }
                if !Arc::ptr_eq(target.journal_root(), &self.root) {
                    return Err(ReadbackError::TargetRootChanged);
                }
                target
                    .journal_root()
                    .revalidate_canonical_binding()
                    .map_err(|_| ReadbackError::TargetRootChanged)?;
                self.check_repository()?;
                self.check_node(file)?;
                Ok(())
            })();
            if let Err(reason) = preflight {
                return Err(ReadbackFailure::target(
                    reason,
                    writer,
                    target,
                    Arc::clone(self.carrier()),
                ));
            }
            let execution = (|| {
                self.revalidate_local()?;
                let timeout = self.operation.budget().remaining(self.operation.clock())?;
                let retained = target
                    .retain_for_helper()
                    .map_err(|_| ReadbackError::NativeFailure)?;
                let stage_file = target
                    .issue_stage_file()
                    .map_err(|_| ReadbackError::NativeFailure)?;
                let argv = waiting_for_repository(vec![
                    "dump".into(),
                    file.snapshot_id.as_str().into(),
                    file.archive_path.as_str().into(),
                    "--no-cache".into(),
                ]);
                writer = WriterState::Unknown;
                self.execution.borrow_mut().attempt();
                let cancel = self.operation.budget().cancellation();
                let completed = self.executor.run_dump(
                    WindowsArchiveRequest {
                        program: &self.restic_path,
                        argv: &argv,
                        env: &self.env,
                        timeout,
                        cancellation: cancel,
                        retained: vec![
                            Arc::clone(self.carrier()) as Arc<dyn Send + Sync>,
                            Arc::new(retained),
                        ],
                    },
                    stage_file,
                    file.bytes,
                );
                {
                    let mut execution = self.execution.borrow_mut();
                    execution.returned();
                    execution.retain_cleanup(&completed);
                }
                if let Some(cleanup) = &completed.cleanup {
                    writer = WriterState::Pending(Arc::clone(cleanup));
                } else {
                    writer = WriterState::QualifiedCompletion;
                }
                if !completed.quiescent() {
                    return Err(ReadbackError::ToolFailed);
                }
                writer = WriterState::QualifiedCompletion;
                self.operation.budget().remaining(self.operation.clock())?;
                if !completed.successful() {
                    return Err(ReadbackError::ToolFailed);
                }
                self.check_repository()?;
                self.revalidate_local()?;
                Ok(completed)
            })();
            let completed = match execution {
                Ok(completed) => completed,
                Err(reason) => {
                    return Err(ReadbackFailure::target(
                        reason,
                        writer,
                        target,
                        Arc::clone(self.carrier()),
                    ));
                }
            };
            let expected = MediaDigest {
                size: file.bytes,
                sha256: file.sha256.clone(),
            };
            let prepared =
                target
                    .prepare_after_successful_helper(&expected)
                    .map_err(|failure| {
                        ReadbackFailure::target(
                            ReadbackError::NativeFailure,
                            writer.clone(),
                            failure.owned,
                            Arc::clone(self.carrier()),
                        )
                    })?;
            if let Err(reason) = self.revalidate_local() {
                return Err(ReadbackFailure {
                    carrier: Arc::clone(self.carrier()),
                    reason,
                    writer,
                    owned: Box::new(OwnedReadback::Prepared(prepared)),
                });
            }
            self.execution
                .borrow_mut()
                .accept_windows(DirectInvocation::Dump, completed.successful());
            Ok(prepared)
        }
        #[cfg(unix)]
        {
            let mut target = target;
            // Issuance is refusal evidence only: another writer may still hold
            // this target even when this operation never launched a child.
            if target.helper_transport_issued() {
                return Err(ReadbackFailure::target(
                    ReadbackError::NativeFailure,
                    WriterState::Unknown,
                    target,
                    Arc::clone(self.carrier()),
                ));
            }
            let mut writer = WriterState::NotLaunched;
            let preflight = (|| {
                file.validate()?;
                if file.repository_id != self.repository_id() {
                    return Err(ReadbackError::RepositoryIdentityChanged);
                }
                if !Arc::ptr_eq(target.journal_root(), &self.root) {
                    return Err(ReadbackError::TargetRootChanged);
                }
                target
                    .journal_root()
                    .revalidate_canonical_binding()
                    .map_err(|_| ReadbackError::TargetRootChanged)?;
                self.check_repository()?;
                self.check_node(file)?;
                Ok(())
            })();
            if let Err(reason) = preflight {
                return Err(ReadbackFailure::target(
                    reason,
                    writer,
                    target,
                    Arc::clone(self.carrier()),
                ));
            }
            #[cfg(any(
                target_os = "ios",
                all(
                    any(
                        all(target_os = "linux", target_arch = "x86_64"),
                        all(target_os = "macos", target_arch = "aarch64")
                    ),
                    feature = "native-archive-host-tests"
                )
            ))]
            let native_target_identity = format!("{:?}", target.identity());
            let execution = (|| {
                self.revalidate_local()?;
                let timeout = self.operation.budget().remaining(self.operation.clock())?;
                let transport = target
                    .helper_transport()
                    .map_err(|_| ReadbackError::NativeFailure)?;
                #[cfg(any(
                    target_os = "ios",
                    all(
                        any(
                            all(target_os = "linux", target_arch = "x86_64"),
                            all(target_os = "macos", target_arch = "aarch64")
                        ),
                        feature = "native-archive-host-tests"
                    )
                ))]
                {
                    let _ = timeout;
                    #[cfg(target_os = "ios")]
                    let AdmittedTool::Linked(context) = &self.tool;
                    #[cfg(not(target_os = "ios"))]
                    let native_context = match &self.tool {
                        AdmittedTool::Linked(context) => Some(context),
                        _ => None,
                    };
                    #[cfg(target_os = "ios")]
                    let native_context = Some(context);
                    if let Some(context) = native_context {
                        writer = WriterState::Unknown;
                        let result = context.download(
                            self,
                            &file.snapshot_id,
                            &file.archive_path,
                            transport.descriptor,
                            file.bytes,
                            &native_target_identity,
                        );
                        // This fixed backend returns only after wait/free; unknown
                        // native drain parks the actual lifetime instead of returning.
                        writer = WriterState::QualifiedCompletion;
                        let completed = QueryCompletion::Native(result?);
                        self.operation.budget().remaining(self.operation.clock())?;
                        self.check_repository()?;
                        self.revalidate_local()?;
                        return Ok(completed);
                    }
                    #[cfg(target_os = "ios")]
                    return Err(ReadbackError::Unsupported);
                }
                #[cfg(not(target_os = "ios"))]
                {
                    let request = Request {
                        program: self.tool.program()?.as_os_str().to_owned(),
                        argv: waiting_for_repository(vec![
                            "dump".into(),
                            "--target".into(),
                            transport.target.into_os_string(),
                            file.snapshot_id.as_str().into(),
                            file.archive_path.as_str().into(),
                            "--no-cache".into(),
                        ]),
                        env: self.env.clone(),
                        timeout: Some(timeout),
                        pass_fds: vec![transport.descriptor],
                    };
                    writer = WriterState::Unknown;
                    let completed = match self.run_child(&request, MAX_QUERY_BYTES) {
                        Ok(completed) => completed,
                        Err((reason, settled)) => {
                            // A cancelled or refused run that was reaped, or never
                            // spawned, leaves no writer either.
                            if settled && self.tool.revalidate().is_ok() {
                                writer = WriterState::QualifiedCompletion;
                            }
                            return Err(reason);
                        }
                    };
                    self.tool.revalidate()?;
                    // Qualification is restricted to this admitted direct restic
                    // profile; killpg alone does not prove descendant cessation.
                    // The runner returns only after reaping the root, so this holds
                    // for a failed or timed-out dump as for a successful one.
                    writer = WriterState::QualifiedCompletion;
                    self.operation.budget().remaining(self.operation.clock())?;
                    if !completed.successful() {
                        return Err(ReadbackError::ToolFailed);
                    }
                    self.check_repository()?;
                    self.revalidate_local()?;
                    Ok(QueryCompletion::Child(completed))
                }
            })();
            let completed = match execution {
                Ok(completed) => completed,
                Err(reason) => {
                    return Err(ReadbackFailure::target(
                        reason,
                        writer,
                        target,
                        Arc::clone(self.carrier()),
                    ));
                }
            };
            let expected = MediaDigest {
                size: file.bytes,
                sha256: file.sha256.clone(),
            };
            let prepared =
                target
                    .prepare_after_successful_helper(&expected)
                    .map_err(|failure| {
                        ReadbackFailure::target(
                            ReadbackError::NativeFailure,
                            writer.clone(),
                            failure.owned,
                            Arc::clone(self.carrier()),
                        )
                    })?;
            if let Err(reason) = self.revalidate_local() {
                return Err(ReadbackFailure {
                    carrier: Arc::clone(self.carrier()),
                    reason,
                    writer,
                    owned: Box::new(OwnedReadback::Prepared(prepared)),
                });
            }
            self.execution
                .borrow_mut()
                .accept(DirectInvocation::Dump, completed);
            Ok(prepared)
        }
    }
}

#[cfg(unix)]
fn admit_tool(input: &ReadAdmission<'_>) -> Result<AdmittedTool, ReadbackError> {
    #[cfg(all(
        any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        ),
        feature = "native-archive-host-tests"
    ))]
    if let Some(context) = &input.native {
        if !context.binds(
            input.operation.journal_root(),
            &input.destination,
            &input.password,
        ) {
            return Err(ReadbackError::ToolChanged);
        }
        context.revalidate()?;
        return Ok(AdmittedTool::Linked(Arc::clone(context)));
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let member = solstone_core_installed_payload::resolve_current_member(RESTIC_MEMBER)
            .map_err(|_| ReadbackError::ToolUnavailable)?;
        if member.path() != input.expected_restic_path {
            return Err(ReadbackError::ToolChanged);
        }
        if let Some(predecessor) = &input.predecessor {
            predecessor.validate_member(&member)?;
        }
        Ok(AdmittedTool::Installed(Box::new(member)))
    }
    #[cfg(target_os = "android")]
    {
        use std::os::unix::fs::MetadataExt;
        let path = input.expected_restic_path.clone();
        let meta = std::fs::metadata(&path).map_err(|_| ReadbackError::ToolUnavailable)?;
        Ok(AdmittedTool::Service {
            path,
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
    #[cfg(target_os = "ios")]
    {
        let context = input
            .native
            .as_ref()
            .ok_or(ReadbackError::ToolUnavailable)?;
        if !context.binds(
            input.operation.journal_root(),
            &input.destination,
            &input.password,
        ) {
            return Err(ReadbackError::ToolChanged);
        }
        context.revalidate()?;
        Ok(AdmittedTool::Linked(Arc::clone(context)))
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn pinned_member(tool: &AdmittedTool) -> Result<(), ReadbackError> {
    #[cfg(feature = "native-archive-host-tests")]
    if let AdmittedTool::Linked(context) = tool {
        return context.revalidate();
    }
    #[allow(irrefutable_let_patterns)]
    let AdmittedTool::Installed(member) = tool else {
        return Err(ReadbackError::ToolChanged);
    };
    if member.bytes() != 30_924_962
        || member.sha256() != "ae7fe58ab3511f830fd31d157158620b209522ff1332b119199d2e938d72338c"
    {
        return Err(ReadbackError::UnsupportedToolVersion);
    }
    Ok(())
}

/// Signing rewrites the macOS restic's bytes every release, so there is no byte
/// pin there: the member is the signed package's own manifest record, revalidated
/// around every use, and it reported the supported version at admission.
#[cfg(target_os = "macos")]
fn pinned_member(tool: &AdmittedTool) -> Result<(), ReadbackError> {
    tool.revalidate()
}

#[cfg(any(unix, windows, test))]
fn owned_snapshot_file(
    bytes: &[u8],
    repository_id: &str,
    snapshot_id: &str,
    name: &str,
    digest: &MediaDigest,
) -> Result<ExactArchiveFile, ReadbackError> {
    let unavailable = ReadbackError::SnapshotNodeUnavailable;
    let mut snapshots = 0usize;
    let mut selected = None;
    for value in serde_json::Deserializer::from_slice(bytes).into_iter::<Value>() {
        let value = value.map_err(|_| unavailable)?;
        match value.get("message_type").and_then(Value::as_str) {
            Some("snapshot") => {
                snapshots += 1;
                if value.get("id").and_then(Value::as_str) != Some(snapshot_id) {
                    return Err(unavailable);
                }
            }
            Some("node") => {
                // Every node must disclose a name so it cannot hide ambiguity.
                let node_name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(unavailable)?;
                if node_name != name {
                    continue;
                }
                if selected.is_some()
                    || value.get("type").and_then(Value::as_str) != Some("file")
                    || value.get("size").and_then(Value::as_u64) != Some(digest.size)
                {
                    return Err(unavailable);
                }
                let path = value
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or(unavailable)?;
                let file = ExactArchiveFile {
                    repository_id: repository_id.to_owned(),
                    snapshot_id: snapshot_id.to_owned(),
                    name: name.to_owned(),
                    archive_path: path.to_owned(),
                    bytes: digest.size,
                    sha256: digest.sha256.clone(),
                };
                file.validate().map_err(|_| unavailable)?;
                selected = Some(file);
            }
            _ => return Err(unavailable),
        }
    }
    if snapshots != 1 {
        return Err(unavailable);
    }
    selected.ok_or(unavailable)
}

#[cfg(any(unix, windows, test))]
fn exact_regular_node(bytes: &[u8], file: &ExactArchiveFile) -> Result<(), ReadbackError> {
    let mut snapshots = 0usize;
    let mut nodes = 0usize;
    for value in serde_json::Deserializer::from_slice(bytes).into_iter::<Value>() {
        let value = value.map_err(|_| ReadbackError::SnapshotNodeUnavailable)?;
        match value.get("message_type").and_then(Value::as_str) {
            Some("snapshot") => {
                snapshots += 1;
                if value.get("id").and_then(Value::as_str) != Some(file.snapshot_id.as_str()) {
                    return Err(ReadbackError::SnapshotNodeUnavailable);
                }
            }
            Some("node") => {
                // Equality is over the exact stored snapshot-native spelling.
                if value.get("path").and_then(Value::as_str) == Some(file.archive_path.as_str()) {
                    nodes += 1;
                    if value.get("type").and_then(Value::as_str) != Some("file")
                        || value.get("name").and_then(Value::as_str) != Some(file.name.as_str())
                        || value.get("size").and_then(Value::as_u64) != Some(file.bytes)
                    {
                        return Err(ReadbackError::SnapshotNodeUnavailable);
                    }
                }
            }
            _ => return Err(ReadbackError::SnapshotNodeUnavailable),
        }
    }
    if snapshots != 1 || nodes != 1 {
        return Err(ReadbackError::SnapshotNodeUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_repository_read_waits_out_an_exclusive_lock() {
        for read in [
            vec!["cat", "config", "--no-cache"],
            vec!["ls", "--json", "snapshot", "--no-cache"],
            vec!["ls", "--json", "snapshot", "/archive/path", "--no-cache"],
            vec!["dump", "snapshot", "/archive/path", "--no-cache"],
        ] {
            let argv =
                super::waiting_for_repository(read.iter().map(std::ffi::OsString::from).collect());
            let flag = argv
                .iter()
                .position(|argument| argument == "--retry-lock")
                .unwrap_or_else(|| panic!("{read:?} refuses under another run's lock"));
            assert_eq!(argv[flag + 1], super::REPOSITORY_RETRY_LOCK, "{read:?}");
            // The wait notice would otherwise lead the output parsed here.
            assert!(
                argv.iter().any(|argument| argument == "--quiet"),
                "{read:?}"
            );
            assert_eq!(argv[..read.len()], read[..], "{read:?}");
        }
    }

    use super::*;

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn direct_read_state_requires_metadata_and_preserves_first_failure() {
        let mut state = DirectExecution::default();
        assert_eq!(state.qualify_read_state(), Err(ReadbackError::ToolFailed));
        state.completed[DirectInvocation::Version as usize] = 1;
        assert!(state.qualify_read_state().is_err());
        state.completed[DirectInvocation::Config as usize] = 1;
        assert_eq!(state.qualify_read_state(), Ok(()));
        state.pending = 1;
        assert!(state.qualify_read_state().is_err());
        state.pending = 0;
        state.fail(ReadbackError::Admission(AdmissionError::Cancelled));
        state.fail(ReadbackError::ToolFailed);
        assert_eq!(
            state.qualify_read_state(),
            Err(ReadbackError::Admission(AdmissionError::Cancelled))
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn refused_release_requires_every_launched_child_reaped_and_never_qualifies() {
        let mut state = DirectExecution::default();
        assert!(state.quiescent());
        state.attempt();
        assert!(!state.quiescent());
        // A reaped child releases whatever its exit status; it never qualifies.
        state.returned();
        state.fail(ReadbackError::ToolFailed);
        assert!(state.quiescent());
        assert_eq!(state.qualify_read_state(), Err(ReadbackError::ToolFailed));
        // A run that errored (cancel, kill or reap failure) stays pending.
        state.attempt();
        state.fail(ReadbackError::Admission(AdmissionError::Cancelled));
        assert!(!state.quiescent());
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn reaped_writer_failure_removes_its_unpublished_copy_and_unknown_keeps_it() {
        use solstone_core_journal_io::media::{MediaDirectory, MediaStage};
        let temp = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
        let carrier = Arc::new(RootActivityCarrier::prepare(Arc::clone(&activity)).unwrap());
        let parent = MediaDirectory::open(Arc::clone(&root), std::path::Path::new("")).unwrap();
        let issued = || {
            let stage = MediaStage::create(Arc::clone(&parent), Arc::clone(&activity)).unwrap();
            let path = stage.diagnostic_path();
            let mut target = stage.create_target().unwrap();
            let transport = target.helper_transport().unwrap();
            std::fs::write(&transport.target, b"partial archive bytes").unwrap();
            (path, target)
        };
        for writer in [WriterState::QualifiedCompletion, WriterState::Unknown] {
            let (stage, target) = issued();
            let mut failure = ReadbackFailure::target(
                ReadbackError::ToolFailed,
                writer.clone(),
                target,
                Arc::clone(&carrier),
            );
            if matches!(writer, WriterState::QualifiedCompletion) {
                assert_eq!(failure.cleanup(), Cleanup::Complete);
                assert!(!stage.exists());
            } else {
                assert_eq!(failure.cleanup(), Cleanup::Refused);
                assert_eq!(
                    std::fs::read(stage.join("payload")).unwrap(),
                    b"partial archive bytes"
                );
            }
        }
        assert!(carrier.diagnostic_path().is_dir());
    }

    // Reduced output captured from restic 0.19.0 with a two-file local
    // repository. Parser fields are verbatim; unrelated host metadata is omitted.
    const FULL_LS: &[u8] =
        include_bytes!("../tests/fixtures/archive_readback/restic-0.19.0-full-ls.jsonl");
    const EXACT_LS: &[u8] =
        include_bytes!("../tests/fixtures/archive_readback/restic-0.19.0-exact-ls.jsonl");
    const CAPTURED_SNAPSHOT: &str =
        "61cc73a4bf7d520b6c28f68ab7f5402df357899c4613a4a6ea6dd9ea00b1804c";

    fn captured_values() -> Vec<Value> {
        serde_json::Deserializer::from_slice(FULL_LS)
            .into_iter::<Value>()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn encode_values(values: &[Value]) -> Vec<u8> {
        values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    }

    fn captured_source() -> MediaDigest {
        solstone_core_journal_io::media::digest_reader(
            &mut b"actual owned discovery audio\n".as_slice(),
        )
        .unwrap()
    }

    fn discover(bytes: &[u8]) -> Result<ExactArchiveFile, ReadbackError> {
        owned_snapshot_file(
            bytes,
            &"a".repeat(64),
            CAPTURED_SNAPSHOT,
            "audio.flac",
            &captured_source(),
        )
    }

    #[test]
    fn captured_full_and_exact_lists_support_both_readback_parsers() {
        let file = discover(FULL_LS).unwrap();
        let values = captured_values();
        let node = values
            .iter()
            .find(|value| value["name"] == "audio.flac")
            .unwrap();
        assert_eq!(file.archive_path, node["path"].as_str().unwrap());
        assert_eq!(file.bytes, captured_source().size);
        assert_eq!(file.sha256, captured_source().sha256);
        exact_regular_node(FULL_LS, &file).unwrap();
        exact_regular_node(EXACT_LS, &file).unwrap();

        let screen = solstone_core_journal_io::media::digest_reader(
            &mut b"actual owned discovery screen\n".as_slice(),
        )
        .unwrap();
        let file = owned_snapshot_file(
            FULL_LS,
            &"a".repeat(64),
            CAPTURED_SNAPSHOT,
            "screen.mp4",
            &screen,
        )
        .unwrap();
        assert_eq!(file.name, "screen.mp4");
        assert_eq!(file.bytes, screen.size);
    }

    #[test]
    fn full_list_discovery_refuses_ambiguous_or_undisclosed_names() {
        let values = captured_values();
        let source = values
            .iter()
            .find(|value| value["name"] == "audio.flac")
            .unwrap();
        for replacement in [
            serde_json::json!(29),
            serde_json::json!(30),
            serde_json::json!(null),
        ] {
            let mut duplicate = source.clone();
            duplicate["path"] = serde_json::json!("/another/audio.flac");
            duplicate["size"] = replacement;
            let mut changed = values.clone();
            changed.push(duplicate);
            assert_eq!(
                discover(&encode_values(&changed)).err(),
                Some(ReadbackError::SnapshotNodeUnavailable)
            );
        }
        for kind in ["dir", "symlink"] {
            let mut duplicate = source.clone();
            duplicate["path"] = serde_json::json!("/another/audio.flac");
            duplicate["type"] = serde_json::json!(kind);
            let mut changed = values.clone();
            changed.push(duplicate);
            assert_eq!(
                discover(&encode_values(&changed)).err(),
                Some(ReadbackError::SnapshotNodeUnavailable)
            );
        }
        // An unrelated node with no usable name could hide a duplicate.
        for name in [Value::Null, serde_json::json!(7), serde_json::json!([])] {
            let mut changed = values.clone();
            changed[1]["name"] = name;
            assert_eq!(
                discover(&encode_values(&changed)).err(),
                Some(ReadbackError::SnapshotNodeUnavailable)
            );
        }
        let mut changed = values;
        changed[1].as_object_mut().unwrap().remove("name");
        assert_eq!(
            discover(&encode_values(&changed)).err(),
            Some(ReadbackError::SnapshotNodeUnavailable)
        );
    }

    #[test]
    fn full_list_discovery_requires_one_snapshot_and_exact_regular_candidate() {
        let values = captured_values();
        let candidate = values
            .iter()
            .position(|value| value["name"] == "audio.flac")
            .unwrap();
        for (key, replacement) in [
            ("type", serde_json::json!("dir")),
            ("name", serde_json::json!("other.flac")),
            ("size", serde_json::json!(30)),
            ("size", serde_json::json!(-1)),
            ("path", serde_json::json!("/a/../audio.flac")),
            ("path", serde_json::json!("/a/other.flac")),
            ("message_type", serde_json::json!("other")),
        ] {
            let mut changed = values.clone();
            changed[candidate][key] = replacement;
            assert_eq!(
                discover(&encode_values(&changed)).err(),
                Some(ReadbackError::SnapshotNodeUnavailable)
            );
        }
        for key in ["type", "size", "path"] {
            let mut changed = values.clone();
            changed[candidate].as_object_mut().unwrap().remove(key);
            assert_eq!(
                discover(&encode_values(&changed)).err(),
                Some(ReadbackError::SnapshotNodeUnavailable)
            );
        }
        let mut wrong = values.clone();
        wrong[0]["id"] = serde_json::json!("b".repeat(64));
        let mut duplicate = values.clone();
        duplicate.push(values[0].clone());
        let missing = values[1..].to_vec();
        for changed in [wrong, duplicate, missing] {
            assert_eq!(
                discover(&encode_values(&changed)).err(),
                Some(ReadbackError::SnapshotNodeUnavailable)
            );
        }
        let mut truncated = FULL_LS.to_vec();
        truncated.push(b'{');
        assert_eq!(
            discover(&truncated).err(),
            Some(ReadbackError::SnapshotNodeUnavailable)
        );
    }

    #[test]
    fn discovery_preserves_literal_snapshot_volume_path() {
        let mut values = captured_values();
        let candidate = values
            .iter_mut()
            .find(|value| value["name"] == "audio.flac")
            .unwrap();
        let literal = r"/\\?\Volume{08fa948d-3336-40fd-b369-dd44a1c89601}/stage/audio.flac";
        candidate["path"] = serde_json::json!(literal);
        let encoded = encode_values(&values);
        let file = discover(&encoded).unwrap();
        assert_eq!(file.archive_path, literal);
        exact_regular_node(&encoded, &file).unwrap();
    }

    fn selected() -> ExactArchiveFile {
        ExactArchiveFile {
            repository_id: "a".repeat(64),
            snapshot_id: "b".repeat(64),
            name: "audio.flac".into(),
            archive_path: r"/\\?\Volume{08fa948d-3336-40fd-b369-dd44a1c89601}/stage/audio.flac"
                .into(),
            bytes: 7,
            sha256: "c".repeat(64),
        }
    }
    #[test]
    fn exact_node_requires_file_kind_snapshot_size_and_unmodified_path() {
        let file = selected();
        file.validate().unwrap();
        let snapshot = serde_json::json!({"message_type":"snapshot","id":file.snapshot_id});
        let node = serde_json::json!({"message_type":"node","path":file.archive_path,"name":file.name,"type":"file","size":7});
        let encode = |node: &Value| format!("{snapshot}\n{node}\n").into_bytes();
        exact_regular_node(&encode(&node), &file).unwrap();
        for (key, replacement) in [
            ("type", serde_json::json!("dir")),
            ("path", serde_json::json!("/stage/audio.flac")),
            ("size", serde_json::json!(8)),
        ] {
            let mut wrong = node.clone();
            wrong[key] = replacement;
            assert_eq!(
                exact_regular_node(&encode(&wrong), &file),
                Err(ReadbackError::SnapshotNodeUnavailable)
            );
        }
        assert_eq!(
            exact_regular_node(format!("{snapshot}\n{node}\n{node}").as_bytes(), &file),
            Err(ReadbackError::SnapshotNodeUnavailable)
        );
    }
    #[test]
    fn invalid_selector_refuses_instead_of_cleaning_it() {
        for path in [
            "/a/../audio.flac",
            "/a/./audio.flac",
            "relative/audio.flac",
            "/a/other.flac",
            "//a/audio.flac",
            r"/stage\\?\Volume{08fa948d-3336-40fd-b369-dd44a1c89601}/audio.flac",
            r"/\\?\Volume{08fa948d3336-40fd-b369-dd44a1c89601}/audio.flac",
            r"/\\server\share/audio.flac",
        ] {
            let mut file = selected();
            file.archive_path = path.into();
            assert_eq!(file.validate(), Err(ReadbackError::InvalidSelection));
        }
        let mut file = selected();
        file.snapshot_id = "latest".into();
        assert_eq!(file.validate(), Err(ReadbackError::InvalidSelection));
    }
    #[cfg(unix)]
    #[test]
    fn direct_profile_rejects_mismatched_scheme_empty_credentials_and_ambient_keys() {
        let mut destination = Destination {
            backend: "s3".into(),
            repository: "s3:https://example.invalid/bucket".into(),
            credentials: serde_json::from_value(
                serde_json::json!({"access_key_id":"access","secret_access_key":"secret"}),
            )
            .unwrap(),
        };
        let env = direct_environment(&destination, "password").unwrap();
        assert_eq!(env.len(), 4);
        assert!(!env.contains_key(std::ffi::OsStr::new("PATH")));
        for repository in ["sftp:host:path", "rclone:remote:path", "/local"] {
            destination.repository = repository.into();
            assert_eq!(
                direct_environment(&destination, "password"),
                Err(ReadbackError::UnsupportedBackend)
            );
        }
        destination.repository = "s3:https://example.invalid/bucket".into();
        destination
            .credentials
            .insert("access_key_id".into(), Value::String(String::new()));
        assert_eq!(
            direct_environment(&destination, "password"),
            Err(ReadbackError::CredentialsUnavailable)
        );
    }
}
