// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Structural guards for the system library's intentional boundaries.

use std::collections::BTreeSet;

use solstone_core_system::TASK_VERB_TOKENS;

const JOURNAL_PROCESSES: &str = include_str!("../../solstone-core-journal-cli/src/processes.rs");
const LIB: &str = include_str!("../src/lib.rs");
const CAP: &str = include_str!("../src/cap.rs");
const ERROR: &str = include_str!("../src/error.rs");
const PARTITION: &str = include_str!("../src/partition.rs");
const QUEUE: &str = include_str!("../src/queue.rs");
const REQUEST: &str = include_str!("../src/request.rs");
const PROCESS_MOD: &str = include_str!("../src/process/mod.rs");
const EVENTS: &str = include_str!("../src/process/events.rs");
const RESTART: &str = include_str!("../src/process/restart.rs");
const LOG: &str = include_str!("../src/process/log.rs");
const SPAWN: &str = include_str!("../src/process/spawn.rs");
const TERMINATE: &str = include_str!("../src/process/terminate.rs");
const DESCENDANTS: &str = include_str!("../src/process/descendants.rs");
const LIFECYCLE: &str = include_str!("../src/lifecycle/mod.rs");
const LIFECYCLE_ADMISSION: &str = include_str!("../src/lifecycle/admission.rs");
const LIFECYCLE_READINESS: &str = include_str!("../src/lifecycle/readiness.rs");
const LIFECYCLE_SHUTDOWN: &str = include_str!("../src/lifecycle/shutdown.rs");
const LIFECYCLE_STATE: &str = include_str!("../src/lifecycle/state.rs");
const LIFECYCLE_SWEEP: &str = include_str!("../src/lifecycle/sweep.rs");
const LIFECYCLE_SYNC: &str = include_str!("../src/lifecycle/sync.rs");
const LIFECYCLE_WATCHER: &str = include_str!("../src/lifecycle/watcher.rs");
const STT_BACKEND_CHOICE: &str = include_str!("../src/stt_backend_choice.rs");
const SCHEDULE: &str = include_str!("../src/schedule/mod.rs");
const SCHEDULE_CAPS: &str = include_str!("../src/schedule/caps.rs");
const SCHEDULE_COMPLETION: &str = include_str!("../src/schedule/completion.rs");
const SCHEDULE_CONFIG: &str = include_str!("../src/schedule/config.rs");
const SCHEDULE_DUE: &str = include_str!("../src/schedule/due.rs");
const SCHEDULE_ENGINE: &str = include_str!("../src/schedule/engine.rs");
const SCHEDULE_STATUS: &str = include_str!("../src/schedule/status.rs");
const SCHEDULE_SUBMISSION: &str = include_str!("../src/schedule/submission.rs");
const PROVIDER_RUNTIME: &str = include_str!("../src/provider_runtime/mod.rs");
const PROVIDER_RUNTIME_ADMISSION: &str = include_str!("../src/provider_runtime/admission.rs");
const PROVIDER_RUNTIME_EVENTS: &str = include_str!("../src/provider_runtime/events.rs");
const PROVIDER_RUNTIME_GATE: &str = include_str!("../src/provider_runtime/gate.rs");
const PROVIDER_RUNTIME_LAUNCH: &str = include_str!("../src/provider_runtime/launch.rs");
const PROVIDER_RUNTIME_MODEL: &str = include_str!("../src/provider_runtime/model.rs");
const PROVIDER_RUNTIME_PARAKEET: &str = include_str!("../src/provider_runtime/parakeet.rs");
const PROVIDER_RUNTIME_PARAKEET_TRUTH: &str =
    include_str!("../src/provider_runtime/parakeet_truth.rs");
const PROVIDER_RUNTIME_PLACEMENT: &str = include_str!("../src/provider_runtime/placement.rs");
const PROVIDER_RUNTIME_RECONCILE: &str = include_str!("../src/provider_runtime/reconcile.rs");
const PROVIDER_RUNTIME_RETRY: &str = include_str!("../src/provider_runtime/retry.rs");
const PROVIDER_RUNTIME_SEAMS: &str = include_str!("../src/provider_runtime/seams.rs");
const PROVIDER_RUNTIME_STORE: &str = include_str!("../src/provider_runtime/store.rs");
const PROVIDER_RUNTIME_STOP: &str = include_str!("../src/provider_runtime/stop.rs");
const PROVIDER_RUNTIME_WEDGE: &str = include_str!("../src/provider_runtime/wedge.rs");

fn declared_modules(source: &str) -> BTreeSet<&str> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("pub mod ")
                .or_else(|| line.strip_prefix("mod "))
                .and_then(|declaration| declaration.strip_suffix(';'))
        })
        .collect()
}

#[test]
fn ac1_every_declared_task_verb_is_in_the_native_journal_census_with_empty_preset_argv() {
    for token in TASK_VERB_TOKENS {
        let marker = format!("token: \"{token}\"");
        let start = JOURNAL_PROCESSES
            .find(&marker)
            .unwrap_or_else(|| panic!("missing task token {token} in PROCESS_SPECS"));
        let entry = &JOURNAL_PROCESSES[start..];
        let end = entry.find("ProcessSpec {").unwrap_or(entry.len());
        assert!(
            entry[..end].contains("preset_argv: EMPTY"),
            "task token {token} must retain an empty native preset argv"
        );
    }
}

#[test]
fn ac4_ios_descendant_coverage_is_explicitly_cfg_gated() {
    assert!(DESCENDANTS.contains("#[cfg(any(target_os = \"linux\", target_os = \"macos\"))]"));
    assert!(TERMINATE.contains("#[cfg(not(any(target_os = \"linux\", target_os = \"macos\")))]"));
    assert!(TERMINATE.contains("DescendantCoverageUnavailable"));
}

#[test]
fn ac7_bus_decode_is_typed_to_bus_requests_not_scheduled_execution_requests() {
    // The ordinary bus decoder returns `BusTaskRequest`, not `ExecutionRequest`.
    // Therefore it cannot construct `ExecutionRequest::Scheduled`; scheduler work
    // must enter via `ScheduledRequest::new` and the explicit enum wrapper.
    let decode_start = REQUEST
        .find("pub fn decode(")
        .expect("ordinary bus decoder exists");
    let decode = &REQUEST[decode_start..REQUEST.len().min(decode_start + 900)];
    assert!(decode.contains("Result<Self, WireRequestError>"));
    assert!(!decode.contains("ExecutionRequest"));
    assert!(REQUEST.contains("pub enum ExecutionRequest"));
    assert!(REQUEST.contains("Scheduled(ScheduledRequest)"));
}

#[test]
fn ac21_only_operational_log_module_names_write_primitives() {
    let root_modules = [
        ("cap", CAP),
        ("error", ERROR),
        ("lifecycle", LIFECYCLE),
        ("partition", PARTITION),
        ("process", PROCESS_MOD),
        ("queue", QUEUE),
        ("request", REQUEST),
        ("provider_runtime", PROVIDER_RUNTIME),
        ("schedule", SCHEDULE),
        ("stt_backend_choice", STT_BACKEND_CHOICE),
    ];
    let process_modules = [
        ("descendants", DESCENDANTS),
        ("events", EVENTS),
        ("log", LOG),
        ("restart", RESTART),
        ("spawn", SPAWN),
        ("terminate", TERMINATE),
    ];
    let lifecycle_modules = [
        ("admission", LIFECYCLE_ADMISSION),
        ("readiness", LIFECYCLE_READINESS),
        ("shutdown", LIFECYCLE_SHUTDOWN),
        ("state", LIFECYCLE_STATE),
        ("sweep", LIFECYCLE_SWEEP),
        ("sync", LIFECYCLE_SYNC),
        ("watcher", LIFECYCLE_WATCHER),
    ];
    let schedule_modules = [
        ("caps", SCHEDULE_CAPS),
        ("completion", SCHEDULE_COMPLETION),
        ("config", SCHEDULE_CONFIG),
        ("due", SCHEDULE_DUE),
        ("engine", SCHEDULE_ENGINE),
        ("status", SCHEDULE_STATUS),
        ("submission", SCHEDULE_SUBMISSION),
    ];
    let provider_runtime_modules = [
        ("admission", PROVIDER_RUNTIME_ADMISSION),
        ("events", PROVIDER_RUNTIME_EVENTS),
        ("gate", PROVIDER_RUNTIME_GATE),
        ("launch", PROVIDER_RUNTIME_LAUNCH),
        ("model", PROVIDER_RUNTIME_MODEL),
        ("parakeet", PROVIDER_RUNTIME_PARAKEET),
        ("parakeet_truth", PROVIDER_RUNTIME_PARAKEET_TRUTH),
        ("placement", PROVIDER_RUNTIME_PLACEMENT),
        ("reconcile", PROVIDER_RUNTIME_RECONCILE),
        ("retry", PROVIDER_RUNTIME_RETRY),
        ("seams", PROVIDER_RUNTIME_SEAMS),
        ("store", PROVIDER_RUNTIME_STORE),
        ("stop", PROVIDER_RUNTIME_STOP),
        ("wedge", PROVIDER_RUNTIME_WEDGE),
    ];
    assert_eq!(
        declared_modules(LIB),
        root_modules.iter().map(|(name, _)| *name).collect()
    );
    assert_eq!(
        declared_modules(LIFECYCLE),
        lifecycle_modules.iter().map(|(name, _)| *name).collect()
    );
    assert_eq!(
        declared_modules(PROCESS_MOD),
        process_modules.iter().map(|(name, _)| *name).collect()
    );
    assert_eq!(
        declared_modules(SCHEDULE),
        schedule_modules.iter().map(|(name, _)| *name).collect()
    );
    assert_eq!(
        declared_modules(PROVIDER_RUNTIME),
        provider_runtime_modules
            .iter()
            .map(|(name, _)| *name)
            .collect()
    );

    for (name, source) in root_modules
        .into_iter()
        .chain(process_modules)
        .chain(lifecycle_modules)
        .chain(schedule_modules)
        .chain(provider_runtime_modules)
        .filter(|(name, _)| {
            *name != "log" && *name != "state" && *name != "completion" && *name != "store"
        })
    {
        for primitive in [
            "File::",
            "OpenOptions",
            "fs::write",
            "fs::rename",
            "create_dir_all",
        ] {
            assert!(
                !source.contains(primitive),
                "{name} must not write journal data through {primitive}"
            );
        }
    }
    assert!(LOG.contains("OpenOptions"));
    assert!(LOG.contains("create_dir_all"));
    assert!(LOG.contains("join(\"health\")"));
    assert!(LOG.contains("CHRONICLE_DIR"));
    assert!(LIFECYCLE_STATE.contains("OpenOptions"));
    assert!(LIFECYCLE_STATE.contains("create_dir_all"));
    assert!(LIFECYCLE_STATE.contains("join(\"health\")"));
    assert!(SCHEDULE_COMPLETION.contains("record_completion"));
    assert!(SCHEDULE_COMPLETION.contains("atomic_replace"));
    assert!(PROVIDER_RUNTIME_STORE.contains("health"));
    assert!(PROVIDER_RUNTIME_STORE.contains("providers"));
    assert!(PROVIDER_RUNTIME_STORE.contains("runtime"));
    // The durable record is one write path per provider, not a literal
    // "local"-only filename -- health_path/retry_path/port_path all format!
    // on self.provider (or its mapped port service name), so the evidence
    // here is the parameterization itself, not a hardcoded "local.json".
    assert!(PROVIDER_RUNTIME_STORE.contains(".json\", self.provider.as_str()"));
    assert!(PROVIDER_RUNTIME_STORE.contains(".retry-token.json\", self.provider.as_str()"));
    assert!(PROVIDER_RUNTIME_STORE.contains("port_service_name(self.provider)"));
    assert!(PROVIDER_RUNTIME_STORE.contains("\"parakeet-cpp\""));
    assert!(PROVIDER_RUNTIME_STORE.contains("hold_lock"));
    assert!(PROVIDER_RUNTIME_STORE.contains("write_json"));
}

#[test]
fn ac17_running_slots_are_released_by_the_worker_lease_drop_path() {
    assert!(QUEUE.contains("struct WorkerLease"));
    assert!(QUEUE.contains("impl Drop for WorkerLease"));
    assert!(QUEUE.contains("finish_worker(&self.inner, &self.partition, &self.reference)"));
    assert!(QUEUE.contains("let _lease = WorkerLease"));
}

#[test]
fn ac25_ios_process_state_probe_is_explicit_and_returns_unknown() {
    assert!(QUEUE.contains("#[cfg(target_os = \"ios\")]"));
    assert!(QUEUE.contains("iOS has neither Linux procfs"));
    assert!(QUEUE.contains(
        "fn system_process_state(_pid: u32) -> ProcessState {\n    ProcessState::Unknown"
    ));
}

#[test]
fn ac26_lifecycle_sweep_and_identity_have_explicit_platform_absence() {
    assert!(LIFECYCLE_SWEEP.contains("#[cfg(target_os = \"linux\")]"));
    assert!(LIFECYCLE_SWEEP.contains("UnsupportedPlatform"));
    assert!(LIFECYCLE_SWEEP.contains("Other targets refuse"));
    assert!(LIFECYCLE_STATE.contains("#[cfg(target_os = \"linux\")]"));
    assert!(LIFECYCLE_STATE.contains("no non-Linux creation-time implementation"));
}
