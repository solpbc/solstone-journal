// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks,
    context::CheckContext,
    vocabulary::{Check, Platform, RunnerResult, Severity, Status, make_result},
};
use std::collections::BTreeSet;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Battery {
    Journal,
    JournalReadiness,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredWave {
    W3b,
    W3c,
}
impl std::fmt::Display for DeferredWave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::W3b => "W3b",
                Self::W3c => "W3c",
            }
        )
    }
}
pub type Runner = fn(&CheckContext) -> RunnerResult;
#[derive(Clone, Copy)]
pub struct RegistryEntry {
    pub check: Check,
    pub runner: Runner,
    pub deferred: Option<DeferredWave>,
    pub feature: Option<&'static str>,
}
const BOTH: &[Platform] = &[Platform::Linux, Platform::Darwin];
const DARWIN: &[Platform] = &[Platform::Darwin];
fn stub_w3b(_context: &CheckContext) -> RunnerResult {
    Ok(make_result(
        Check {
            name: "stub",
            severity: Severity::Advisory,
            platforms: BOTH,
        },
        Status::Skip,
        "deferred to wave W3b",
        None::<String>,
    ))
}
fn stub_w3c(_context: &CheckContext) -> RunnerResult {
    Ok(make_result(
        Check {
            name: "stub",
            severity: Severity::Advisory,
            platforms: BOTH,
        },
        Status::Skip,
        "deferred to wave W3c",
        None::<String>,
    ))
}
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
const fn stub(
    name: &'static str,
    severity: Severity,
    wave: DeferredWave,
    feature: Option<&'static str>,
) -> RegistryEntry {
    RegistryEntry {
        check: Check {
            name,
            severity,
            platforms: BOTH,
        },
        runner: match wave {
            DeferredWave::W3b => stub_w3b,
            DeferredWave::W3c => stub_w3c,
        },
        deferred: Some(wave),
        feature,
    }
}
pub static JOURNAL: &[RegistryEntry] = &[
    stub(
        "journal_leaf_exclusivity",
        Severity::Blocker,
        DeferredWave::W3b,
        None,
    ),
    stub(
        "journal_package_version",
        Severity::Blocker,
        DeferredWave::W3b,
        None,
    ),
    stub(
        "retired_host_shim",
        Severity::Blocker,
        DeferredWave::W3b,
        None,
    ),
    stub(
        "host_dependencies",
        Severity::Blocker,
        DeferredWave::W3b,
        None,
    ),
    stub("disk_space", Severity::Advisory, DeferredWave::W3b, None),
    RegistryEntry {
        check: CHECK_CONFIG,
        runner: config,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_WRITABLE,
        runner: journal_writable,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_CONFLICT,
        runner: conflict,
        deferred: None,
        feature: None,
    },
    stub(
        "service_identity",
        Severity::Blocker,
        DeferredWave::W3b,
        None,
    ),
    RegistryEntry {
        check: CHECK_SERVICE,
        runner: service,
        deferred: None,
        feature: None,
    },
    stub("journal_sync", Severity::Blocker, DeferredWave::W3c, None),
    stub(
        "journal_caught_up",
        Severity::Advisory,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "journal_maint_tasks",
        Severity::Blocker,
        DeferredWave::W3c,
        None,
    ),
    stub("task_pace", Severity::Advisory, DeferredWave::W3c, None),
    stub("brain", Severity::Advisory, DeferredWave::W3c, None),
    stub(
        "capture_health",
        Severity::Advisory,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "observer_binding",
        Severity::Advisory,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "observer_delivery_stall",
        Severity::Advisory,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "observer_ingest_health",
        Severity::Advisory,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "orphan_segment_pdf",
        Severity::Advisory,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "stale_alias_symlink",
        Severity::Blocker,
        DeferredWave::W3b,
        None,
    ),
    RegistryEntry {
        check: CHECK_PLIST,
        runner: plist,
        deferred: None,
        feature: None,
    },
    stub(
        "default_stt_ready",
        Severity::Advisory,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "parakeet_cpp_stt_ready",
        Severity::Blocker,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "speakers_analyze_installation",
        Severity::Blocker,
        DeferredWave::W3c,
        None,
    ),
    stub("skill_state", Severity::Advisory, DeferredWave::W3c, None),
    stub(
        "feature:pdf-import",
        Severity::Advisory,
        DeferredWave::W3c,
        Some("pdf-import"),
    ),
    stub(
        "feature:pdf-export",
        Severity::Advisory,
        DeferredWave::W3c,
        Some("pdf-export"),
    ),
];
pub static READINESS: &[RegistryEntry] = &[
    stub(
        "host_dependencies",
        Severity::Blocker,
        DeferredWave::W3b,
        None,
    ),
    stub("python_version", Severity::Blocker, DeferredWave::W3b, None),
    stub("sol_importable", Severity::Blocker, DeferredWave::W3b, None),
    stub(
        "local_bin_sol_reachable",
        Severity::Advisory,
        DeferredWave::W3b,
        None,
    ),
    stub(
        "stale_alias_symlink",
        Severity::Blocker,
        DeferredWave::W3b,
        None,
    ),
    stub("disk_space", Severity::Advisory, DeferredWave::W3b, None),
    RegistryEntry {
        check: CHECK_WRITABLE,
        runner: readiness_writable,
        deferred: None,
        feature: None,
    },
    stub(
        "default_stt_ready",
        Severity::Advisory,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "parakeet_cpp_stt_ready",
        Severity::Blocker,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "speakers_analyze_installation",
        Severity::Blocker,
        DeferredWave::W3c,
        None,
    ),
    stub(
        "feature:pdf-import",
        Severity::Advisory,
        DeferredWave::W3c,
        Some("pdf-import"),
    ),
    stub(
        "feature:pdf-export",
        Severity::Advisory,
        DeferredWave::W3c,
        Some("pdf-export"),
    ),
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
pub fn feature_entries() -> impl Iterator<Item = &'static RegistryEntry> {
    JOURNAL.iter().filter(|e| e.feature.is_some())
}
pub fn union_names() -> BTreeSet<&'static str> {
    JOURNAL
        .iter()
        .chain(READINESS)
        .map(|e| e.check.name)
        .collect()
}
