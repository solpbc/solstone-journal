// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Target-neutral process vocabulary shared by platform implementations.

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::events::ProcessEventSink;
use crate::lifecycle::HostedServiceKind;

/// PID together with the native birth token observed for that PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInstance {
    pub pid: u32,
    pub birth: ProcessBirth,
}

/// Opaque start-time identity. Equality is exact (tick-level on Linux,
/// microsecond-level proc_bsdinfo on macOS). [`ProcessBirth::epoch_seconds`] is only for
/// supervisor pid-file identity, which applies `START_TIME_TOLERANCE_SECONDS`.
#[derive(Debug, Clone, Copy)]
pub struct ProcessBirth {
    inner: ProcessBirthInner,
}

#[derive(Debug, Clone, Copy)]
enum ProcessBirthInner {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Linux {
        start_ticks: u64,
        btime: u64,
        clk_tck: u64,
    },
    #[allow(dead_code)]
    Macos {
        epoch_micros: i64,
    },
    #[allow(dead_code)]
    Windows {
        filetime: u64,
    },
    Unknown,
}

impl PartialEq for ProcessBirth {
    fn eq(&self, other: &Self) -> bool {
        match (self.inner, other.inner) {
            (
                ProcessBirthInner::Linux {
                    start_ticks: left_ticks,
                    btime: left_btime,
                    ..
                },
                ProcessBirthInner::Linux {
                    start_ticks: right_ticks,
                    btime: right_btime,
                    ..
                },
            ) => left_ticks == right_ticks && left_btime == right_btime,
            (
                ProcessBirthInner::Macos { epoch_micros: left },
                ProcessBirthInner::Macos {
                    epoch_micros: right,
                },
            ) => left == right,
            (
                ProcessBirthInner::Windows { filetime: left },
                ProcessBirthInner::Windows { filetime: right },
            ) => left == right,
            _ => false,
        }
    }
}

impl Eq for ProcessBirth {}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum ProcessBirthWire {
    Linux {
        start_ticks: u64,
        btime: u64,
        clk_tck: u64,
    },
    Macos {
        epoch_micros: i64,
    },
    Windows {
        filetime: u64,
    },
    #[serde(other)]
    Unknown,
}

impl Serialize for ProcessBirth {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = match self.inner {
            ProcessBirthInner::Linux {
                start_ticks,
                btime,
                clk_tck,
            } => ProcessBirthWire::Linux {
                start_ticks,
                btime,
                clk_tck,
            },
            ProcessBirthInner::Macos { epoch_micros } => ProcessBirthWire::Macos { epoch_micros },
            ProcessBirthInner::Windows { filetime } => ProcessBirthWire::Windows { filetime },
            ProcessBirthInner::Unknown => ProcessBirthWire::Unknown,
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProcessBirth {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = match ProcessBirthWire::deserialize(deserializer)? {
            ProcessBirthWire::Linux {
                start_ticks,
                btime,
                clk_tck,
            } => ProcessBirthInner::Linux {
                start_ticks,
                btime,
                clk_tck,
            },
            ProcessBirthWire::Macos { epoch_micros } => ProcessBirthInner::Macos { epoch_micros },
            ProcessBirthWire::Windows { filetime } => ProcessBirthInner::Windows { filetime },
            ProcessBirthWire::Unknown => ProcessBirthInner::Unknown,
        };
        Ok(Self { inner })
    }
}

impl ProcessBirth {
    pub fn epoch_seconds(&self) -> Option<f64> {
        match self.inner {
            ProcessBirthInner::Linux {
                start_ticks,
                btime,
                clk_tck,
            } => Some(btime as f64 + start_ticks as f64 / clk_tck as f64),
            ProcessBirthInner::Macos { epoch_micros } => Some(epoch_micros as f64 / 1_000_000.0),
            ProcessBirthInner::Windows { filetime } => {
                Some(windows_filetime_epoch_seconds(filetime))
            }
            ProcessBirthInner::Unknown => None,
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn linux(start_ticks: u64, btime: u64, clk_tck: u64) -> Self {
        Self {
            inner: ProcessBirthInner::Linux {
                start_ticks,
                btime,
                clk_tck,
            },
        }
    }

    #[allow(dead_code)]
    pub(crate) fn macos(epoch_micros: i64) -> Self {
        Self {
            inner: ProcessBirthInner::Macos { epoch_micros },
        }
    }

    pub fn windows(filetime: u64) -> Self {
        Self {
            inner: ProcessBirthInner::Windows { filetime },
        }
    }

    pub fn windows_filetime(&self) -> Option<u64> {
        match self.inner {
            ProcessBirthInner::Windows { filetime } => Some(filetime),
            _ => None,
        }
    }

    pub fn is_verifiable(&self) -> bool {
        !matches!(self.inner, ProcessBirthInner::Unknown)
    }
}

/// Convert the Windows FILETIME epoch (1601-01-01) to Unix epoch seconds.
pub(crate) fn windows_filetime_epoch_seconds(filetime: u64) -> f64 {
    const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;
    (filetime as i128 - WINDOWS_TO_UNIX_EPOCH_100NS as i128) as f64 / 10_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNKNOWN_BIRTH_JSON: &str = r#"{"kind":"totally-unrecognized-future-kind","token":1}"#;

    struct FixedAbsentSource;

    impl ProcessInstanceSource for FixedAbsentSource {
        fn inspect(&self, _pid: u32) -> InspectResult {
            InspectResult::Absent
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    #[test]
    fn windows_filetime_epoch_controls() {
        const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;
        assert_eq!(
            windows_filetime_epoch_seconds(WINDOWS_TO_UNIX_EPOCH_100NS),
            0.0
        );
        assert_eq!(
            windows_filetime_epoch_seconds(WINDOWS_TO_UNIX_EPOCH_100NS + 15_000_000),
            1.5
        );
    }

    #[test]
    fn windows_process_birth_wire_round_trip_preserves_filetime_above_f64_precision() {
        const FILETIME: u64 = 9_007_199_254_740_993;
        let birth = ProcessBirth::windows(FILETIME);
        let decoded: ProcessBirth =
            serde_json::from_slice(&serde_json::to_vec(&birth).expect("serialize Windows birth"))
                .expect("deserialize Windows birth");

        assert_eq!(decoded.windows_filetime(), Some(FILETIME));
        assert_eq!(decoded, ProcessBirth::windows(FILETIME));
    }

    #[test]
    fn foreign_process_birth_kind_deserializes_to_unknown_and_never_compares_equal() {
        let left: ProcessBirth = serde_json::from_str(UNKNOWN_BIRTH_JSON)
            .expect("unknown process-birth kind remains decodable");
        let right: ProcessBirth = serde_json::from_str(UNKNOWN_BIRTH_JSON)
            .expect("unknown process-birth kind remains decodable");

        assert!(!left.is_verifiable());
        assert_ne!(left, right);
    }

    #[test]
    fn default_observe_returns_unverifiable_for_unknown_expected_birth() {
        let birth: ProcessBirth = serde_json::from_str(UNKNOWN_BIRTH_JSON)
            .expect("unknown process-birth kind remains decodable");
        let expected = ProcessInstance { pid: 42, birth };

        assert_eq!(
            FixedAbsentSource.observe(&expected),
            InstanceVerdict::Unverifiable
        );
    }
}

/// Live execution state. Zombies are not live and surface as [`InspectResult::Absent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    Running,
    Stopped,
}

/// Result of comparing a remembered identity against one native sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceVerdict {
    SameLive { execution: ExecutionState },
    NotSameOrExited,
    Unverifiable,
}

/// One PID sample without a remembered identity to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectResult {
    Present {
        instance: ProcessInstance,
        uid: u32,
        execution: ExecutionState,
        ppid: Option<u32>,
        pgid: Option<i32>,
    },
    Absent,
    Unverifiable,
}

/// One live row from a process-table sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CensusRow {
    pub instance: ProcessInstance,
    pub uid: u32,
    pub ppid: u32,
    pub pgid: i32,
    pub execution: ExecutionState,
}

/// Process-table sample. Incomplete must never be treated as an empty table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceCensus {
    Complete(Vec<CensusRow>),
    Incomplete(Vec<CensusRow>),
}

/// Injected process-instance source. Production uses [`SystemProcessInstanceSource`].
pub trait ProcessInstanceSource: Send + Sync {
    fn inspect(&self, pid: u32) -> InspectResult;
    fn census(&self) -> InstanceCensus;

    fn census_until(&self, deadline: Instant) -> InstanceCensus {
        if Instant::now() >= deadline {
            return InstanceCensus::Incomplete(Vec::new());
        }
        self.census()
    }

    fn census_tree(&self, root_pid: u32, deadline: Option<Instant>) -> InstanceCensus {
        let _ = root_pid;
        deadline.map_or_else(|| self.census(), |deadline| self.census_until(deadline))
    }

    fn observe(&self, expected: &ProcessInstance) -> InstanceVerdict {
        if !expected.birth.is_verifiable() {
            return InstanceVerdict::Unverifiable;
        }
        match self.inspect(expected.pid) {
            InspectResult::Unverifiable => InstanceVerdict::Unverifiable,
            InspectResult::Absent => InstanceVerdict::NotSameOrExited,
            InspectResult::Present { instance, .. } if !instance.birth.is_verifiable() => {
                InstanceVerdict::Unverifiable
            }
            InspectResult::Present {
                instance,
                execution,
                ..
            } if instance.birth == expected.birth => InstanceVerdict::SameLive { execution },
            InspectResult::Present { .. } => InstanceVerdict::NotSameOrExited,
        }
    }
}

/// Native observer for the current target.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcessInstanceSource;

/// A descendant's exact identity and provenance observed before signaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Descendant {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: Option<i32>,
    pub uid: u32,
}

/// Process tree captured before any termination signal is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTreeSnapshot {
    pub parent_pid: i32,
    pub parent_pgid: Option<i32>,
    pub descendants: Vec<Descendant>,
    pub descendant_births: HashMap<i32, ProcessBirth>,
}

/// How a launched child is owned and when it is expected to end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    IndependentLongLived,
    IndependentBoundedHelper { timeout: Duration },
    InheritedParentScope,
    ExplicitlyUnowned { reason: String },
}

/// Boundary-facing launch and termination failures.
#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("host capability unavailable: {needed}")]
    CapabilityUnavailable { needed: &'static str },
    #[error("ExplicitlyUnowned reason must be nonempty")]
    EmptyUnownedReason,
    #[error("failed to spawn child: {0}")]
    Spawn(#[source] io::Error),
    #[error(transparent)]
    SpawnManaged(SpawnError),
    #[error("post-spawn confirmation failed for pid {pid}: {source}")]
    ConfirmationFailed {
        pid: u32,
        #[source]
        source: io::Error,
    },
    #[error("failed to terminate child: {0}")]
    Terminate(#[source] io::Error),
    #[error("child output is unavailable")]
    OutputUnavailable,
    #[error("this authority is not explicitly unowned")]
    NotExplicitlyUnowned,
    #[error("hosted launch admission failed: {0}")]
    Admission(String),
}

pub type BoxedTerminateFn = Box<dyn FnMut(&mut Child, Duration) -> Result<(), LaunchError> + Send>;

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("host capability unavailable: {needed}")]
    CapabilityUnavailable { needed: &'static str },
    #[error("cannot spawn an empty command")]
    EmptyCommand,
    #[error("failed to prepare operational log: {0}")]
    Log(#[source] io::Error),
    #[error("failed to spawn child: {0}")]
    Spawn(#[source] io::Error),
    #[error("failed to capture birth-bound identity for spawned pid {pid}")]
    ExactInstanceUnavailable { pid: u32 },
}

/// Inputs owned by the caller rather than process-global state.
#[derive(Clone)]
pub struct SpawnOptions {
    pub journal_root: PathBuf,
    pub reference: String,
    pub day: Option<String>,
    pub sink: Option<Arc<dyn ProcessEventSink>>,
    pub environment: BTreeMap<OsString, OsString>,
}

/// Task cap enforcement's bounded graceful window.
pub const CAP_TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Future TaskQueue shutdown's distinct default window.
pub const TASK_QUEUE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Long-lived service shutdown's distinct default window.
pub const SERVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
/// Unconditional bounded reap window after SIGKILL escalation.
pub const KILL_REAP_GRACE: Duration = Duration::from_millis(500);
/// Bounded drain-thread join after the child and descendants are reaped.
pub const DRAIN_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationOutcome {
    Graceful { exit_code: Option<i32> },
    EscalatedAndReaped { exit_code: Option<i32> },
}

#[derive(Debug, Error)]
pub enum TerminationError {
    #[error("managed parent missed the graceful termination window")]
    ParentGraceTimeout,
    #[error("process tree not reaped: {reason}; survivors={survivors:?}")]
    ProcessTreeNotReaped {
        reason: &'static str,
        survivors: Vec<Descendant>,
    },
    #[error("descendant coverage unavailable on this platform")]
    DescendantCoverageUnavailable,
    #[error("exact process identity is unavailable")]
    ExactInstanceUnavailable,
    #[error("process lifecycle I/O failed: {0}")]
    Io(#[from] io::Error),
}

/// The reason exact descendant coverage could not be proven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescendantObservationFailure {
    #[error("descendant census was incomplete")]
    CensusIncomplete,
    #[error("service root was not the remembered live instance")]
    RootNotSameOrExited,
    #[error("service root could not be observed")]
    RootUnverifiable,
    #[error("service root was missing from a complete census")]
    Missing,
    #[error("descendant observation became stale")]
    Stale,
    #[error("descendant PID was reused")]
    Reused,
    #[error("descendant UID changed")]
    WrongUid,
    #[error("descendant could not be observed")]
    Unverifiable,
}

/// The terminal result of exact descendant-only cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescendantTerminationOutcome {
    Graceful,
    EscalatedAndReaped,
}

/// Portable signal vocabulary for exact-instance operations.
#[derive(Debug, Clone, Copy)]
pub enum SignalKind {
    Terminate,
    Kill,
}

pub(crate) fn require_managed_process_capability() -> Result<(), &'static str> {
    #[cfg(unix)]
    {
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err("process-groups")
    }
}

/// Declarative raw-command construction.  Production callers name a program,
/// arguments, environment and stdio policy; the platform launch boundary is
/// the only place that converts this request into a real child process.
#[derive(Clone, Debug)]
pub struct CommandLaunchRequest {
    pub program: OsString,
    pub arguments: Vec<OsString>,
    pub environment: BTreeMap<OsString, OsString>,
    pub current_dir: Option<PathBuf>,
    /// Start a separate process group when the helper owns a descendant tree.
    pub process_group: bool,
    pub stdin_piped: bool,
    pub stdout_piped: bool,
    pub stderr_piped: bool,
}

/// Declarative managed launch inputs.  Hosted provenance may only be used
/// with exact managed launch; the boundary captures PID/birth/UID itself.
#[derive(Clone)]
pub struct ManagedLaunchRequest {
    pub command: Vec<String>,
    pub options: SpawnOptions,
}

/// The generation-scoped provenance required for every gated hosted launch.
#[derive(Clone, Debug)]
pub struct HostedLaunchProvenance {
    pub journal: PathBuf,
    pub generation: u64,
    pub launch_id: String,
    pub service: Option<HostedServiceKind>,
    pub parent_launch_id: Option<String>,
    pub acknowledgement_timeout: Duration,
}

/// The single process-table sample captured for an exact launch.  Keeping UID
/// beside the birth-bound instance prevents a later authority from widening a
/// signal decision to a same-PID or wrong-user process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchedProcessIdentity {
    pub instance: ProcessInstance,
    pub uid: u32,
}

/// Test-only failure points at the exact admission-rejection boundary.
///
/// They deliberately model a failure to prove termination, rather than
/// changing ordinary spawned-child behavior.  Production callers cannot
/// enable either outcome.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub enum HostedAdmissionTestFault {
    ExactReap,
    ExitProof,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static HOSTED_ADMISSION_TEST_FAULT: Cell<Option<HostedAdmissionTestFault>> = const { Cell::new(None) };
}

#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn set_hosted_admission_test_fault(fault: Option<HostedAdmissionTestFault>) {
    HOSTED_ADMISSION_TEST_FAULT.with(|slot| slot.set(fault));
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn hosted_admission_test_fault() -> Option<HostedAdmissionTestFault> {
    HOSTED_ADMISSION_TEST_FAULT.with(Cell::get)
}
