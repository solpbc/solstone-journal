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
fn journal_leaf_exclusivity(c: &CheckContext) -> RunnerResult {
    checks::journal_leaf_exclusivity::run(c, CHECK_JOURNAL_LEAF_EXCLUSIVITY)
}
fn journal_package_version(c: &CheckContext) -> RunnerResult {
    checks::journal_package_version::run(c, CHECK_JOURNAL_PACKAGE_VERSION)
}
fn retired_host_shim(c: &CheckContext) -> RunnerResult {
    checks::retired_host_shim::run(c, CHECK_RETIRED_HOST_SHIM)
}
fn host_dependencies(c: &CheckContext) -> RunnerResult {
    checks::host_dependencies::run(c, CHECK_HOST_DEPENDENCIES)
}
fn disk_space(c: &CheckContext) -> RunnerResult {
    checks::disk_space::run(c, CHECK_DISK_SPACE)
}
fn python_version(c: &CheckContext) -> RunnerResult {
    checks::python_version::run(c, CHECK_PYTHON_VERSION)
}
fn service_identity(c: &CheckContext) -> RunnerResult {
    checks::service_identity::run(c, CHECK_SERVICE_IDENTITY)
}
fn stale_alias_journal(c: &CheckContext) -> RunnerResult {
    checks::stale_alias_symlink::run(c, CHECK_STALE_ALIAS_SYMLINK, "journal")
}
fn stale_alias_sol(c: &CheckContext) -> RunnerResult {
    checks::stale_alias_symlink::run(c, CHECK_STALE_ALIAS_SYMLINK, "sol")
}
fn sol_importable(c: &CheckContext) -> RunnerResult {
    checks::sol_importable::run(c, CHECK_SOL_IMPORTABLE)
}
fn local_bin_sol_reachable(c: &CheckContext) -> RunnerResult {
    checks::local_bin_sol_reachable::run(c, CHECK_LOCAL_BIN_SOL_REACHABLE)
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
const CHECK_JOURNAL_LEAF_EXCLUSIVITY: Check = Check {
    name: "journal_leaf_exclusivity",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_JOURNAL_PACKAGE_VERSION: Check = Check {
    name: "journal_package_version",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_RETIRED_HOST_SHIM: Check = Check {
    name: "retired_host_shim",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_HOST_DEPENDENCIES: Check = Check {
    name: "host_dependencies",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_DISK_SPACE: Check = Check {
    name: "disk_space",
    severity: Severity::Advisory,
    platforms: BOTH,
};
const CHECK_PYTHON_VERSION: Check = Check {
    name: "python_version",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_SERVICE_IDENTITY: Check = Check {
    name: "service_identity",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_STALE_ALIAS_SYMLINK: Check = Check {
    name: "stale_alias_symlink",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_SOL_IMPORTABLE: Check = Check {
    name: "sol_importable",
    severity: Severity::Blocker,
    platforms: BOTH,
};
const CHECK_LOCAL_BIN_SOL_REACHABLE: Check = Check {
    name: "local_bin_sol_reachable",
    severity: Severity::Advisory,
    platforms: BOTH,
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
    RegistryEntry {
        check: CHECK_JOURNAL_LEAF_EXCLUSIVITY,
        runner: journal_leaf_exclusivity,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_JOURNAL_PACKAGE_VERSION,
        runner: journal_package_version,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_RETIRED_HOST_SHIM,
        runner: retired_host_shim,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_HOST_DEPENDENCIES,
        runner: host_dependencies,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_DISK_SPACE,
        runner: disk_space,
        deferred: None,
        feature: None,
    },
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
    RegistryEntry {
        check: CHECK_SERVICE_IDENTITY,
        runner: service_identity,
        deferred: None,
        feature: None,
    },
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
    RegistryEntry {
        check: CHECK_STALE_ALIAS_SYMLINK,
        runner: stale_alias_journal,
        deferred: None,
        feature: None,
    },
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
        Severity::Advisory,
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
    RegistryEntry {
        check: CHECK_HOST_DEPENDENCIES,
        runner: host_dependencies,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_PYTHON_VERSION,
        runner: python_version,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_SOL_IMPORTABLE,
        runner: sol_importable,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_LOCAL_BIN_SOL_REACHABLE,
        runner: local_bin_sol_reachable,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_STALE_ALIAS_SYMLINK,
        runner: stale_alias_sol,
        deferred: None,
        feature: None,
    },
    RegistryEntry {
        check: CHECK_DISK_SPACE,
        runner: disk_space,
        deferred: None,
        feature: None,
    },
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
        Severity::Advisory,
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
