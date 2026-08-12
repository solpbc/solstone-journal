// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Provider-runtime vocabulary and state records.

use std::fmt;

use serde_json::Value;

/// The cadence shared by truth observation and ready-state probing.
pub const GATE_TICK_INTERVAL_SECONDS: f64 = 60.0;
pub const PROVIDER_RETRY_SCHEDULE_SECONDS: [f64; 6] = [0.0, 2.0, 4.0, 8.0, 16.0, 30.0];
pub const PROVIDER_CLEANUP_RETRY_SCHEDULE_SECONDS: [f64; 5] = [2.0, 4.0, 8.0, 16.0, 30.0];
pub const PROVIDER_ADMISSION_STOP_TIMEOUT_SECONDS: f64 = 5.0;
pub const PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS: f64 = GATE_TICK_INTERVAL_SECONDS;
pub const PROVIDER_PROBE_INTERVAL_SECONDS: f64 = GATE_TICK_INTERVAL_SECONDS;
pub const PROVIDER_STARTUP_GATE_WINDOW_SECONDS: f64 = 60.0;
pub const PROVIDER_STARTUP_GATE_CEILING_SECONDS: f64 = 330.0;
pub const LOCAL_WEDGE_THRESHOLD: usize = 3;
pub const LOCAL_WEDGE_RECYCLE_GRACE_SECONDS: f64 = 120.0;
pub const LOCAL_WEDGE_PROVIDER_MAP_CAP: usize = 512;

/// A provider runtime is reconciled independently for each provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderName {
    Local,
    Parakeet,
}

impl ProviderName {
    pub const ALL: [Self; 2] = [Self::Local, Self::Parakeet];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Parakeet => "parakeet",
        }
    }
}

/// A runtime-health phase. This is intentionally closed: the reconciler owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuntimePhase {
    NotDesired,
    Observing,
    ArtifactNotReady,
    HostBlocked,
    Starting,
    Warming,
    Backoff,
    RetryRequested,
    Ready,
    ReadyProofUnavailable,
    StopDeferred,
    Stopping,
    Stopped,
    Failed,
    CleanupFailed,
    StateCorrupt,
    StateUnavailable,
}

impl RuntimePhase {
    pub const ALL: [Self; 17] = [
        Self::NotDesired,
        Self::Observing,
        Self::ArtifactNotReady,
        Self::HostBlocked,
        Self::Starting,
        Self::Warming,
        Self::Backoff,
        Self::RetryRequested,
        Self::Ready,
        Self::ReadyProofUnavailable,
        Self::StopDeferred,
        Self::Stopping,
        Self::Stopped,
        Self::Failed,
        Self::CleanupFailed,
        Self::StateCorrupt,
        Self::StateUnavailable,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotDesired => "not-desired",
            Self::Observing => "observing",
            Self::ArtifactNotReady => "artifact-not-ready",
            Self::HostBlocked => "host-blocked",
            Self::Starting => "starting",
            Self::Warming => "warming",
            Self::Backoff => "backoff",
            Self::RetryRequested => "retry-requested",
            Self::Ready => "ready",
            Self::ReadyProofUnavailable => "ready-proof-unavailable",
            Self::StopDeferred => "stop-deferred",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::CleanupFailed => "cleanup-failed",
            Self::StateCorrupt => "state-corrupt",
            Self::StateUnavailable => "state-unavailable",
        }
    }
}

pub const PROVIDER_STARTUP_TERMINAL_PHASES: [RuntimePhase; 8] = [
    RuntimePhase::Ready,
    RuntimePhase::ReadyProofUnavailable,
    RuntimePhase::NotDesired,
    RuntimePhase::ArtifactNotReady,
    RuntimePhase::HostBlocked,
    RuntimePhase::Failed,
    RuntimePhase::StateCorrupt,
    RuntimePhase::StateUnavailable,
];
pub const PROVIDER_START_CANCEL_PHASES: [RuntimePhase; 3] = [
    RuntimePhase::NotDesired,
    RuntimePhase::StateCorrupt,
    RuntimePhase::StateUnavailable,
];
pub const PROVIDER_TRUTH_PRESERVED_PHASES: [RuntimePhase; 5] = [
    RuntimePhase::Ready,
    RuntimePhase::ReadyProofUnavailable,
    RuntimePhase::StopDeferred,
    RuntimePhase::Stopping,
    RuntimePhase::CleanupFailed,
];
/// A RAM-floor observation blocks a new admission, not an already-live provider.
pub const ADMISSION_ONLY_REASON_CODES: [&str; 1] = ["ram-insufficient"];

pub fn phase_in(phases: &[RuntimePhase], phase: RuntimePhase) -> bool {
    let mut index = 0;
    while index < phases.len() {
        if phases[index] == phase {
            return true;
        }
        index += 1;
    }
    false
}

/// Why one of the intentionally-unclassified phases is excluded from all frozen sets.
pub const fn unclassified_phase_reason(phase: RuntimePhase) -> Option<&'static str> {
    match phase {
        // A truth worker is in flight; it cannot complete the startup condition.
        RuntimePhase::Observing => Some("truth refresh is in flight"),
        // A launch remains eligible once its retry deadline has arrived.
        RuntimePhase::Starting => Some("launch-eligible and retry-gated"),
        // Present in Python vocabulary but not produced by its runtime state machine.
        RuntimePhase::Warming => Some("dormant Python vocabulary value"),
        // A finite launch retry budget remains active.
        RuntimePhase::Backoff => Some("budgeted retry state"),
        // A durable retry token requests a fresh observation.
        RuntimePhase::RetryRequested => Some("forces fresh observation"),
        // A replacement may start after cleanup converges.
        RuntimePhase::Stopped => Some("replacement-eligible post-cleanup state"),
        _ => None,
    }
}

/// All reason codes recognized by the cross-language runtime-health contract.
pub const KNOWN_REASON_CODES: [&str; 42] = [
    "intent-disabled",
    "intent-enabled",
    "provider-not-needed",
    "truth-observation-started",
    "truth-observation-failed",
    "observation-raced",
    "proof-observation-unavailable",
    "install-idle",
    "install-in-progress",
    "artifact-missing",
    "artifact-stale",
    "artifact-proof-failed",
    "host-admission-blocked",
    "platform-unsupported",
    "package-unavailable",
    "openmp-runtime-unavailable",
    "ram-insufficient",
    "gpu-probe-failed",
    "gpu-unavailable",
    "confidential-backend-selected",
    "launch-requested",
    "launch-spawned",
    "launch-failed",
    "warmup-timeout",
    "process-exited",
    "probe-not-ready",
    "retry-scheduled",
    "retry-token-requested",
    "launch-budget-exhausted",
    "local-wedge-provider-unavailable",
    "target-changed",
    "intent-removed",
    "duplicate-owned-process",
    "admission-exclusive-stop",
    "cleanup-succeeded",
    "cleanup-attempt-failed",
    "probe-ready",
    "ready-existing-owned-process",
    "ready-with-proof-observation-unavailable",
    "record-malformed",
    "record-unavailable",
    "stale-result-ignored",
];

/// A lossless runtime reason. Unknown wire values remain representable for forward compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReasonCode {
    value: String,
    recognized: bool,
}

impl ReasonCode {
    pub fn from_wire(value: impl Into<String>) -> Self {
        let value = value.into();
        let recognized = KNOWN_REASON_CODES.contains(&value.as_str());
        Self { value, recognized }
    }

    pub fn known(value: &'static str) -> Self {
        debug_assert!(KNOWN_REASON_CODES.contains(&value));
        Self {
            value: value.to_owned(),
            recognized: true,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
    pub const fn is_recognized(&self) -> bool {
        self.recognized
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchOutcomeStatus {
    Ready,
    NotReady,
    HostBlocked,
    Exited,
    WarmupTimeout,
    LaunchFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCleanupStatus {
    Stopped,
    StopDeferred,
    CleanupFailed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStatus {
    Ready,
    NotReady,
    Unavailable,
}

/// Caller-supplied monotonic time for all runtime decisions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProviderRuntimeNow {
    pub monotonic_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFence {
    pub incarnation: String,
    pub generation: u64,
    pub fingerprint: Option<String>,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedProcess {
    pub id: String,
    pub name: String,
    pub running: bool,
    /// The accepted launch fence, absent for intermediate launch outcomes.
    pub fence: Option<ProviderFence>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderTruthObservation {
    pub provider: ProviderName,
    pub phase: RuntimePhase,
    pub reason_code: Option<ReasonCode>,
    pub desired_fingerprint: Option<String>,
    pub has_plan: bool,
    pub boot_required: bool,
    /// Provider-specific payload the durable health record carries alongside
    /// phase/reason_code -- e.g. Parakeet's `{"remote_mode": true}` /
    /// `{"platform": ...}` / `{"stt_admission_latch": ...}`. Local has none
    /// today, hence `None` at every existing call site; this is the
    /// provider-specific-payload-inside-one-type the durable record needs
    /// rather than a second store.
    pub detail: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLaunchOutcome {
    pub status: LaunchOutcomeStatus,
    pub reason_code: ReasonCode,
    pub managed: Option<ManagedProcess>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStopCleanupRequest {
    pub managed: ManagedProcess,
    pub reason_code: ReasonCode,
    pub target_phase: RuntimePhase,
    pub target_reason_code: Option<ReasonCode>,
    pub admission_exclusive: bool,
    /// Cleanup for a stale/cancelled start result must not transiently overwrite a newer phase.
    pub orphaned_start_outcome: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStopCleanupOutcome {
    pub status: StopCleanupStatus,
    pub reason_code: ReasonCode,
    pub managed: Option<ManagedProcess>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProbeOutcome {
    pub status: ProbeStatus,
    pub reason_code: ReasonCode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRetryState {
    pub attempt_count: u32,
    pub next_at: f64,
    pub desired_fingerprint: Option<String>,
}

impl Default for ProviderRetryState {
    fn default() -> Self {
        Self {
            attempt_count: 0,
            next_at: 0.0,
            desired_fingerprint: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlight<T> {
    pub fence: ProviderFence,
    pub result: Option<T>,
}

#[derive(Debug, Clone)]
pub struct ProviderRuntimeState {
    pub provider: ProviderName,
    pub truth: Option<InFlight<ProviderTruthObservation>>,
    pub start: Option<InFlight<ProviderLaunchOutcome>>,
    pub start_cancelled: bool,
    pub stop_cleanup: Option<InFlight<ProviderStopCleanupOutcome>>,
    pub stop_cancelled: bool,
    pub pending_stop_request: Option<ProviderStopCleanupRequest>,
    /// Orphaned start outcomes can arrive while another cleanup is in flight; retain both handles.
    pub orphaned_stop_requests: Vec<ProviderStopCleanupRequest>,
    pub pending_stop_target_phase: RuntimePhase,
    pub pending_stop_target_reason_code: Option<ReasonCode>,
    pub pending_stop_admission_exclusive: bool,
    pub cleanup_attempt_count: u32,
    pub cleanup_next_at: f64,
    pub probe: Option<InFlight<ProviderProbeOutcome>>,
    pub retry: ProviderRetryState,
    pub generation: u64,
    pub desired_fingerprint: Option<String>,
    pub replacement_artifact_not_ready_fingerprint: Option<String>,
    pub has_plan: bool,
    pub latest_phase: RuntimePhase,
    pub latest_reason_code: Option<ReasonCode>,
    pub latest_detail: Option<Value>,
    pub boot_required: bool,
    pub startup_terminal: bool,
    pub next_truth_at: f64,
    pub next_probe_at: f64,
}

impl ProviderRuntimeState {
    pub fn new(provider: ProviderName) -> Self {
        Self {
            provider,
            truth: None,
            start: None,
            start_cancelled: false,
            stop_cleanup: None,
            stop_cancelled: false,
            pending_stop_request: None,
            orphaned_stop_requests: Vec::new(),
            pending_stop_target_phase: RuntimePhase::Stopped,
            pending_stop_target_reason_code: Some(ReasonCode::known("cleanup-succeeded")),
            pending_stop_admission_exclusive: false,
            cleanup_attempt_count: 0,
            cleanup_next_at: 0.0,
            probe: None,
            retry: ProviderRetryState::default(),
            generation: 0,
            desired_fingerprint: None,
            replacement_artifact_not_ready_fingerprint: None,
            has_plan: false,
            latest_phase: RuntimePhase::Stopped,
            latest_reason_code: Some(ReasonCode::known("cleanup-succeeded")),
            latest_detail: None,
            boot_required: false,
            startup_terminal: false,
            next_truth_at: 0.0,
            next_probe_at: 0.0,
        }
    }
}
