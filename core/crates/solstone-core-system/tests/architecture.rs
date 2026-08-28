// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Structural guards for the system library's intentional boundaries.

use std::collections::BTreeSet;

use solstone_core_system::TASK_VERB_TOKENS;

const JOURNAL_PROCESSES: &str = include_str!("../../solstone-core-journal-cli/src/processes.rs");
const JOURNAL_MANIFEST: &str = include_str!("../../solstone-core-journal-cli/src/manifest.rs");
const CORE_MAIN: &str = include_str!("../../solstone-core/src/main.rs");
const LIB: &str = include_str!("../src/lib.rs");
const ACTIVITY_STATE: &str = include_str!("../src/activity_state.rs");
const CAP: &str = include_str!("../src/cap.rs");
const CATCHUP: &str = include_str!("../src/catchup.rs");
const DIRECT_DOOR: &str = include_str!("../src/direct_door.rs");
const ERROR: &str = include_str!("../src/error.rs");
const OPERATIONAL_LOG_PARSE: &str = include_str!("../src/operational_log_parse.rs");
const PARTITION: &str = include_str!("../src/partition.rs");
const QUEUE: &str = include_str!("../src/queue.rs");
const REQUEST: &str = include_str!("../src/request.rs");
const PROCESS_MOD: &str = include_str!("../src/process/mod.rs");
const PROCESS_COMMON: &str = include_str!("../src/process/common.rs");
const EVENTS: &str = include_str!("../src/process/events.rs");
const RESTART: &str = include_str!("../src/process/restart.rs");
const LOG: &str = include_str!("../src/process/log.rs");
const OBSERVATION: &str = include_str!("../src/process/observation.rs");
const PROCESS_UNIX: &str = include_str!("../src/process/unix/mod.rs");
const AUTHORITY: &str = include_str!("../src/process/unix/authority.rs");
const DESCENDANTS: &str = include_str!("../src/process/unix/descendants.rs");
const INSTANCE: &str = include_str!("../src/process/unix/instance.rs");
const MACOS_PROC: &str = include_str!("../src/process/unix/macos_proc.rs");
const PDEATHSIG: &str = include_str!("../src/process/unix/pdeathsig.rs");
const SPAWN: &str = include_str!("../src/process/unix/spawn.rs");
const TERMINATE: &str = include_str!("../src/process/unix/terminate.rs");
const PROCESS_WINDOWS: &str = include_str!("../src/process/windows.rs");
const LIFECYCLE: &str = include_str!("../src/lifecycle/mod.rs");
const LIFECYCLE_CLOCK: &str = include_str!("../src/lifecycle/clock.rs");
const LIFECYCLE_DARWIN_PARENT_WATCH: &str = include_str!("../src/lifecycle/darwin_parent_watch.rs");
const LIFECYCLE_HOSTED_SERVICE: &str = include_str!("../src/lifecycle/hosted_service.rs");
const LIFECYCLE_READINESS: &str = include_str!("../src/lifecycle/readiness.rs");
const LIFECYCLE_PARENT: &str = include_str!("../src/lifecycle/parent.rs");
const LIFECYCLE_PARENT_LOSS_ADMISSION: &str =
    include_str!("../src/lifecycle/parent_loss_admission.rs");
const LIFECYCLE_PARENT_LOSS_COORDINATOR: &str =
    include_str!("../src/lifecycle/parent_loss_coordinator.rs");
const LIFECYCLE_PARENT_LOSS_LEDGER: &str = include_str!("../src/lifecycle/parent_loss_ledger.rs");
const LIFECYCLE_SHUTDOWN: &str = include_str!("../src/lifecycle/shutdown.rs");
const LIFECYCLE_STARTUP: &str = include_str!("../src/lifecycle/startup.rs");
const LIFECYCLE_STATE: &str = include_str!("../src/lifecycle/state.rs");
const LIFECYCLE_SWEEP: &str = include_str!("../src/lifecycle/sweep.rs");
const LIFECYCLE_SYNC: &str = include_str!("../src/lifecycle/sync.rs");
const LIFECYCLE_WINDOWS: &str = include_str!("../src/lifecycle/windows.rs");
const MEMORY_ADMISSION: &str = include_str!("../src/memory_admission.rs");
const STATUS_WIRE: &str = include_str!("../src/status_wire.rs");
const STT_BACKEND_CHOICE: &str = include_str!("../src/stt_backend_choice.rs");
const SCHEDULE: &str = include_str!("../src/schedule/mod.rs");
const SCHEDULE_CAPS: &str = include_str!("../src/schedule/caps.rs");
const SCHEDULE_COMPLETION: &str = include_str!("../src/schedule/completion.rs");
const SCHEDULE_CONFIG: &str = include_str!("../src/schedule/config.rs");
const SCHEDULE_DUE: &str = include_str!("../src/schedule/due.rs");
const SCHEDULE_ENGINE: &str = include_str!("../src/schedule/engine.rs");
const SCHEDULE_REPORT: &str = include_str!("../src/schedule/report.rs");
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
const PROVIDER_RUNTIME_PARAKEET_TRUTH_SEAM: &str =
    include_str!("../src/provider_runtime/parakeet_truth_seam.rs");
const PROVIDER_RUNTIME_PLACEMENT: &str = include_str!("../src/provider_runtime/placement.rs");
const PROVIDER_RUNTIME_RECONCILE: &str = include_str!("../src/provider_runtime/reconcile.rs");
const PROVIDER_RUNTIME_READINESS: &str = include_str!("../src/provider_runtime/readiness.rs");
const PROVIDER_RUNTIME_RETRY: &str = include_str!("../src/provider_runtime/retry.rs");
const PROVIDER_RUNTIME_SEAMS: &str = include_str!("../src/provider_runtime/seams.rs");
const PROVIDER_RUNTIME_STORE: &str = include_str!("../src/provider_runtime/store.rs");
const PROVIDER_RUNTIME_STOP: &str = include_str!("../src/provider_runtime/stop.rs");
const PROVIDER_RUNTIME_WEDGE: &str = include_str!("../src/provider_runtime/wedge.rs");
const TRANSCRIPT_DELETE: &str = include_str!("../../solstone-core-transcripts-web/src/delete.rs");
const SUPERVISOR_RUNTIME: &str = include_str!("../../solstone-core/src/supervisor/runtime.rs");

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
fn ac1_every_declared_task_verb_has_root_native_or_empty_preset_process_authority() {
    let root_commands_end = JOURNAL_MANIFEST
        .find("pub(crate) struct LocalPath")
        .expect("journal root command boundary");
    let root_commands = &JOURNAL_MANIFEST[..root_commands_end];
    for token in TASK_VERB_TOKENS {
        let marker = format!("token: \"{token}\"");
        if root_commands.contains(&format!("\"{token}\"")) {
            continue;
        }
        // A token satisfies this three ways, and the name says so: a native
        // root command, a NATIVE dispatch spec, or a Python ProcessSpec whose
        // preset is empty so the task queue's own argv passes through intact.
        //
        // Search the two tables separately. A bare `find` returns whichever
        // occurrence comes first in the file, and NATIVE_PROCESS_SPECS is
        // declared above PROCESS_SPECS — so once a verb is cut native, the
        // native row is what a single search finds, and native rows legitimately
        // carry a preset (that is how the subcommand is named). Judging a
        // natively-dispatched verb by the empty-preset rule fails it for
        // satisfying the criterion a different, allowed way.
        let native_table = JOURNAL_PROCESSES
            .find("NATIVE_PROCESS_SPECS")
            .map(|start| {
                let rest = &JOURNAL_PROCESSES[start..];
                &rest[..rest.find("\n];").map_or(rest.len(), |end| end)]
            })
            .unwrap_or("");
        if native_table.contains(&marker) {
            continue;
        }
        let python_table_start = JOURNAL_PROCESSES
            .find("const PROCESS_SPECS")
            .unwrap_or_else(|| {
                panic!("journal process table boundary");
            });
        let python_table = &JOURNAL_PROCESSES[python_table_start..];
        let start = python_table.find(&marker).unwrap_or_else(|| {
            panic!("task token {token} has no native root or process authority")
        });
        let entry = &python_table[start..];
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
        ("activity_state", ACTIVITY_STATE),
        ("cap", CAP),
        ("catchup", CATCHUP),
        ("direct_door", DIRECT_DOOR),
        ("error", ERROR),
        ("lifecycle", LIFECYCLE),
        ("memory_admission", MEMORY_ADMISSION),
        ("operational_log_parse", OPERATIONAL_LOG_PARSE),
        ("partition", PARTITION),
        ("process", PROCESS_MOD),
        ("queue", QUEUE),
        ("request", REQUEST),
        ("provider_runtime", PROVIDER_RUNTIME),
        ("schedule", SCHEDULE),
        ("status_wire", STATUS_WIRE),
        ("stt_backend_choice", STT_BACKEND_CHOICE),
    ];
    let process_modules = [
        ("common", PROCESS_COMMON),
        ("events", EVENTS),
        ("log", LOG),
        ("observation", OBSERVATION),
        ("restart", RESTART),
        ("platform", PROCESS_UNIX),
    ];
    let unix_process_modules = [
        ("authority", AUTHORITY),
        ("descendants", DESCENDANTS),
        ("instance", INSTANCE),
        ("macos_proc", MACOS_PROC),
        ("pdeathsig", PDEATHSIG),
        ("spawn", SPAWN),
        ("terminate", TERMINATE),
    ];
    let lifecycle_modules = [
        ("clock", LIFECYCLE_CLOCK),
        ("darwin_parent_watch", LIFECYCLE_DARWIN_PARENT_WATCH),
        ("hosted_service", LIFECYCLE_HOSTED_SERVICE),
        ("parent", LIFECYCLE_PARENT),
        ("parent_loss_admission", LIFECYCLE_PARENT_LOSS_ADMISSION),
        ("parent_loss_coordinator", LIFECYCLE_PARENT_LOSS_COORDINATOR),
        ("parent_loss_ledger", LIFECYCLE_PARENT_LOSS_LEDGER),
        ("readiness", LIFECYCLE_READINESS),
        ("shutdown", LIFECYCLE_SHUTDOWN),
        ("startup", LIFECYCLE_STARTUP),
        ("state", LIFECYCLE_STATE),
        ("sweep", LIFECYCLE_SWEEP),
        ("sync", LIFECYCLE_SYNC),
        ("windows", LIFECYCLE_WINDOWS),
    ];
    let schedule_modules = [
        ("caps", SCHEDULE_CAPS),
        ("completion", SCHEDULE_COMPLETION),
        ("config", SCHEDULE_CONFIG),
        ("due", SCHEDULE_DUE),
        ("engine", SCHEDULE_ENGINE),
        ("report", SCHEDULE_REPORT),
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
        ("parakeet_truth_seam", PROVIDER_RUNTIME_PARAKEET_TRUTH_SEAM),
        ("placement", PROVIDER_RUNTIME_PLACEMENT),
        (
            "readiness",
            PROVIDER_RUNTIME_READINESS
                .split_once("mod tests")
                .map_or(PROVIDER_RUNTIME_READINESS, |(production, _)| production),
        ),
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
        declared_modules(PROCESS_UNIX),
        unix_process_modules.iter().map(|(name, _)| *name).collect()
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
        .chain(unix_process_modules)
        .chain([("windows", PROCESS_WINDOWS)])
        .chain(lifecycle_modules)
        .chain(schedule_modules)
        .chain(provider_runtime_modules)
        .filter(|(name, _)| {
            *name != "log"
                && *name != "state"
                && *name != "completion"
                && *name != "store"
                && *name != "catchup"
                // These lifecycle modules own the isolated admission and
                // witness drops, generation ledger, and terminal coordinator
                // record under health/parent-loss.
                && *name != "parent_loss_admission"
                && *name != "parent_loss_coordinator"
                && *name != "parent_loss_ledger"
        })
    {
        // Checked against PRODUCTION source only -- everything before a trailing
        // `mod tests`. The claim is "must not write journal data", and a unit
        // test building a fixture tree under the OS temp dir is not that. This
        // mirrors the `catchup` treatment immediately below, which established
        // the pattern for exactly this shape; scanning whole files instead made
        // `activity_state` (0 write primitives in production, 4 `fs::write` and
        // 3 `create_dir_all` in its tests) and `schedule/config` red on test
        // code. Production is a subset of the whole file, so this removes false
        // positives without weakening any module's real coverage.
        let production = source
            .split_once("mod tests")
            .map_or(source, |(production, _)| production);
        for primitive in [
            "File::",
            "OpenOptions",
            "fs::write",
            "fs::rename",
            "create_dir_all",
        ] {
            assert!(
                !production.contains(primitive),
                "{name} must not write journal data through {primitive}"
            );
        }
    }
    // catchup owns the narrow operational catchup-state write path, while its
    // unit tests build real fixture trees under the OS temp dir. Check only
    // production source (everything before its trailing `mod tests`) rather
    // than skipping the module entirely.
    let catchup_production = CATCHUP
        .split_once("mod tests")
        .map_or(CATCHUP, |(production, _)| production);
    for primitive in [
        "File::",
        "OpenOptions",
        "fs::write",
        "fs::rename",
        "create_dir_all",
    ] {
        assert!(
            !catchup_production.contains(primitive),
            "catchup must not write journal data through {primitive}"
        );
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
fn ac27_spawn_plan_and_spawn_parakeet_apply_parent_death_kill() {
    assert!(PROVIDER_RUNTIME_LAUNCH.contains("fn spawn_plan"));
    assert!(PROVIDER_RUNTIME_LAUNCH.contains("apply_parent_death_kill"));
    assert!(PROVIDER_RUNTIME_PARAKEET.contains("fn spawn_parakeet"));
    assert!(PROVIDER_RUNTIME_PARAKEET.contains("apply_parent_death_kill"));
}

#[test]
fn ac1_all_hosted_service_routes_use_shared_parent_admission() {
    for function in [
        "fn run_convey(",
        "fn run_spl_service(",
        "fn run_sense_service(",
        "fn run_cortex_service(",
    ] {
        assert!(function_body(CORE_MAIN, function).contains("run_hosted_service("));
    }
    assert!(
        function_body(CORE_MAIN, "fn run_hosted_service<").contains("admit_hosted_service_parent(")
    );
}

fn function_body<'a>(source: &'a str, function: &str) -> &'a str {
    let start = source
        .find(function)
        .unwrap_or_else(|| panic!("missing function {function}"));
    let body = &source[start..];
    let end = body.find("\nfn ").unwrap_or(body.len());
    &body[..end]
}

#[test]
fn ac25_ios_process_state_probe_is_explicit_and_returns_unknown() {
    assert!(INSTANCE.contains("#[cfg(target_os = \"ios\")]"));
    assert!(INSTANCE.contains("iOS has neither Linux procfs"));
    assert!(INSTANCE.contains("InspectResult::Unverifiable"));
}

#[test]
fn ac26_lifecycle_sweep_and_identity_have_explicit_platform_support() {
    assert!(LIFECYCLE_SWEEP.contains("#[cfg(target_os = \"linux\")]"));
    assert!(LIFECYCLE_SWEEP.contains("#[cfg(target_os = \"macos\")]"));
    assert!(LIFECYCLE_SWEEP.contains("OrphanSweepOutcome::UnsupportedPlatform"));
    assert!(INSTANCE.contains("#[cfg(target_os = \"linux\")]"));
    assert!(INSTANCE.contains("#[cfg(target_os = \"macos\")]"));
    assert!(LIFECYCLE_STATE.contains("iOS still has no supported process-start-time source"));
}

#[test]
fn ac28_process_common_and_non_unix_facade_are_unix_free() {
    for (name, source) in [("common", PROCESS_COMMON), ("windows", PROCESS_WINDOWS)] {
        assert!(!source.contains("nix::"), "{name} must not name nix");
        assert!(
            !source.contains("std::os::unix"),
            "{name} must not name Unix std extensions"
        );
    }
    assert!(
        !PROCESS_WINDOWS.contains("Command::spawn"),
        "the non-Unix process facade must not spawn owned children"
    );
}

#[test]
fn ac29_liveness_and_instance_consumers_preserve_unverifiable_outcomes() {
    let production_consumers = [
        ("transcript delete", TRANSCRIPT_DELETE),
        ("supervisor runtime", SUPERVISOR_RUNTIME),
    ];
    assert_eq!(
        production_consumers.len(),
        2,
        "census has a nonzero denominator"
    );
    assert!(TRANSCRIPT_DELETE.contains("SupervisorLiveness"));
    assert!(TRANSCRIPT_DELETE.contains("SupervisorLiveness::Unverifiable"));
    assert!(SUPERVISOR_RUNTIME.contains("InspectResult::Unverifiable"));
}
