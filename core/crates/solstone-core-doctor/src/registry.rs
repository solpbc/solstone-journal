// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks,
    context::CheckContext,
    vocabulary::{Check, Platform, RunnerResult, Severity},
};
use std::collections::BTreeSet;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Battery {
    Journal,
    JournalReadiness,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredCheckSet {
    EarlierSet,
    LaterSet,
}
impl std::fmt::Display for DeferredCheckSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::EarlierSet => "earlier check set",
                Self::LaterSet => "later check set",
            }
        )
    }
}
pub type Runner = fn(&CheckContext) -> RunnerResult;
#[derive(Clone, Copy)]
pub struct RegistryEntry {
    pub check: Check,
    pub runner: Runner,
    pub deferred: Option<DeferredCheckSet>,
}
const BOTH: &[Platform] = &[Platform::Linux, Platform::Darwin];
const DARWIN: &[Platform] = &[Platform::Darwin];
fn config(c: &CheckContext) -> RunnerResult {
    checks::config_dir_readable::run(c, CHECK_CONFIG)
}
fn journal_writable(c: &CheckContext) -> RunnerResult {
    checks::journal_dir_writable::journal(c, CHECK_WRITABLE)
}
fn readiness_writable(c: &CheckContext) -> RunnerResult {
    checks::journal_dir_writable::readiness(c, CHECK_WRITABLE)
}
fn service(c: &CheckContext) -> RunnerResult {
    checks::service_running::run(c, CHECK_SERVICE)
}
fn conflict(c: &CheckContext) -> RunnerResult {
    checks::supervisor_conflict::run(c, CHECK_CONFLICT)
}
fn plist(c: &CheckContext) -> RunnerResult {
    checks::launchd_stale_plist::run(c, CHECK_PLIST)
}
fn disk_space(c: &CheckContext) -> RunnerResult {
    checks::disk_space::run(c, CHECK_DISK_SPACE)
}
fn service_identity(c: &CheckContext) -> RunnerResult {
    checks::service_identity::run(c, CHECK_SERVICE_IDENTITY)
}
fn local_bin_solstone_reachable(c: &CheckContext) -> RunnerResult {
    checks::local_bin_solstone_reachable::run(c, CHECK_LOCAL_BIN_SOLSTONE_REACHABLE)
}
fn journal_sync(c: &CheckContext) -> RunnerResult {
    checks::journal_sync::run(c, CHECK_SYNC)
}
fn caught_up(c: &CheckContext) -> RunnerResult {
    checks::journal_caught_up::run(c, CHECK_CAUGHT_UP)
}
fn task_pace(c: &CheckContext) -> RunnerResult {
    checks::task_pace::run(c, CHECK_TASK_PACE)
}
fn brain(c: &CheckContext) -> RunnerResult {
    checks::brain::run(c, CHECK_BRAIN)
}
fn capture(c: &CheckContext) -> RunnerResult {
    checks::capture_health::run(c, CHECK_CAPTURE)
}
fn client_binding(c: &CheckContext) -> RunnerResult {
    checks::client_binding::run(c, CHECK_CLIENT_BINDING)
}
fn client_delivery(c: &CheckContext) -> RunnerResult {
    checks::client_delivery_stall::run(c, CHECK_CLIENT_DELIVERY)
}
fn client_ingest(c: &CheckContext) -> RunnerResult {
    checks::client_ingest_health::run(c, CHECK_CLIENT_INGEST)
}
fn orphan(c: &CheckContext) -> RunnerResult {
    checks::orphan_segment_pdf::run(c, CHECK_ORPHAN)
}
fn default_stt(c: &CheckContext) -> RunnerResult {
    checks::default_stt_ready::run(c, CHECK_DEFAULT_STT)
}
fn cpp_stt(c: &CheckContext) -> RunnerResult {
    checks::parakeet_cpp_stt_ready::run(c, CHECK_CPP_STT)
}
fn speakers(c: &CheckContext) -> RunnerResult {
    checks::speakers_analyze_installation::run(c, CHECK_SPEAKERS)
}
fn vad_runtime(c: &CheckContext) -> RunnerResult {
    checks::vad_runtime_ready::run(c, CHECK_VAD_RUNTIME)
}
fn skills(c: &CheckContext) -> RunnerResult {
    checks::skill_state::run(c, CHECK_SKILLS)
}
const CHECK_CONFIG: Check = Check {
    name: "config_dir_readable",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_WRITABLE: Check = Check {
    name: "journal_dir_writable",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_SERVICE: Check = Check {
    name: "service_running",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_CONFLICT: Check = Check {
    name: "supervisor_conflict",
    severity: Severity::Blocker,
    platforms: DARWIN,
};
const CHECK_PLIST: Check = Check {
    name: "launchd_stale_plist",
    severity: Severity::Advisory,
    platforms: DARWIN,
};
const CHECK_DISK_SPACE: Check = Check {
    name: "disk_space",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_SERVICE_IDENTITY: Check = Check {
    name: "service_identity",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_LOCAL_BIN_SOLSTONE_REACHABLE: Check = Check {
    name: "local_bin_solstone_reachable",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_SYNC: Check = Check {
    name: "journal_sync",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_CAUGHT_UP: Check = Check {
    name: "journal_caught_up",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_TASK_PACE: Check = Check {
    name: "task_pace",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_BRAIN: Check = Check {
    name: "brain",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_CAPTURE: Check = Check {
    name: "capture_health",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_CLIENT_BINDING: Check = Check {
    name: "client_binding",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_CLIENT_DELIVERY: Check = Check {
    name: "client_delivery_stall",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_CLIENT_INGEST: Check = Check {
    name: "client_ingest_health",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_ORPHAN: Check = Check {
    name: "orphan_segment_pdf",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_DEFAULT_STT: Check = Check {
    name: "default_stt_ready",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_CPP_STT: Check = Check {
    name: "parakeet_cpp_stt_ready",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_SPEAKERS: Check = Check {
    name: "speakers_analyze_installation",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_VAD_RUNTIME: Check = Check {
    name: "vad_runtime_ready",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_SKILLS: Check = Check {
    name: "skill_state",
    severity: Severity::Advisory,
    platforms: BOTH,
};
pub static JOURNAL: &[RegistryEntry] = &[
    RegistryEntry {
        check: CHECK_DISK_SPACE,
        runner: disk_space,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CONFIG,
        runner: config,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_WRITABLE,
        runner: journal_writable,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CONFLICT,
        runner: conflict,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_SERVICE_IDENTITY,
        runner: service_identity,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_SERVICE,
        runner: service,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_SYNC,
        runner: journal_sync,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CAUGHT_UP,
        runner: caught_up,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_TASK_PACE,
        runner: task_pace,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_BRAIN,
        runner: brain,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CAPTURE,
        runner: capture,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CLIENT_BINDING,
        runner: client_binding,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CLIENT_DELIVERY,
        runner: client_delivery,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CLIENT_INGEST,
        runner: client_ingest,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_ORPHAN,
        runner: orphan,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_PLIST,
        runner: plist,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_DEFAULT_STT,
        runner: default_stt,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CPP_STT,
        runner: cpp_stt,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_SPEAKERS,
        runner: speakers,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_VAD_RUNTIME,
        runner: vad_runtime,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_SKILLS,
        runner: skills,
        deferred: None,
    },
];
pub static READINESS: &[RegistryEntry] = &[
    RegistryEntry {
        check: CHECK_LOCAL_BIN_SOLSTONE_REACHABLE,
        runner: local_bin_solstone_reachable,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_DISK_SPACE,
        runner: disk_space,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_WRITABLE,
        runner: readiness_writable,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_DEFAULT_STT,
        runner: default_stt,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_CPP_STT,
        runner: cpp_stt,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_SPEAKERS,
        runner: speakers,
        deferred: None,
    },
    RegistryEntry {
        check: CHECK_VAD_RUNTIME,
        runner: vad_runtime,
        deferred: None,
    },
];
pub fn entries(battery: Battery) -> &'static [RegistryEntry] {
    match battery {
        Battery::Journal => JOURNAL,
        Battery::JournalReadiness => READINESS,
    }
}
pub fn lookup(b: Battery, name: &str) -> Option<&'static RegistryEntry> {
    entries(b).iter().find(|e| e.check.name == name)
}
pub fn union_names() -> BTreeSet<&'static str> {
    JOURNAL
        .iter()
        .chain(READINESS)
        .map(|e| e.check.name)
        .collect()
}
