// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessKind {
    Service,
    Universal,
    Alias,
}

impl ProcessKind {
    #[allow(dead_code)]
    pub(crate) const fn surface(self) -> &'static str {
        match self {
            Self::Service | Self::Alias => "service",
            Self::Universal => "universal",
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn census_kind(self) -> &'static str {
        match self {
            Self::Alias => "alias",
            Self::Service | Self::Universal => "command",
        }
    }

    pub(crate) const fn requires_coherence(self) -> bool {
        matches!(self, Self::Service | Self::Alias)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessSpec {
    pub(crate) token: &'static str,
    pub(crate) module: &'static str,
    pub(crate) preset_argv: &'static [&'static str],
    pub(crate) kind: ProcessKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeProcessSpec {
    pub(crate) token: &'static str,
    pub(crate) binary: &'static str,
    pub(crate) preset_argv: &'static [&'static str],
}

const EMPTY: &[&str] = &[];
const UP: &[&str] = &["up"];
const DOWN: &[&str] = &["down"];
const SPL_SERVICE: &[&str] = &["spl", "service"];

/// Process verbs whose landed implementation can replace Python at the
/// journal boundary. Keep this mapping explicit: a native crate alone is not
/// proof that the owner-facing grammar is ready to cut over.
pub(crate) const NATIVE_PROCESS_SPECS: &[NativeProcessSpec] = &[
    NativeProcessSpec {
        token: "depict",
        binary: "solstone-core-depict",
        preset_argv: EMPTY,
    },
    NativeProcessSpec {
        token: "spl",
        binary: "solstone-core",
        preset_argv: SPL_SERVICE,
    },
    NativeProcessSpec {
        token: "grab",
        binary: "solstone-core",
        preset_argv: &["grab"],
    },
    NativeProcessSpec {
        token: "transfer",
        binary: "solstone-core",
        preset_argv: &["transfer"],
    },
    NativeProcessSpec {
        token: "observer",
        binary: "solstone-core",
        preset_argv: &["observer"],
    },
    NativeProcessSpec {
        token: "export",
        binary: "solstone-core",
        preset_argv: &["export"],
    },
    NativeProcessSpec {
        token: "transcribe",
        binary: "solstone-core",
        preset_argv: &["transcribe"],
    },
];

pub(crate) const PROCESS_SPECS: &[ProcessSpec] = &[
    ProcessSpec {
        token: "backup",
        module: "solstone.think.backup_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "importer",
        module: "solstone.think.importers.cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "think",
        module: "solstone.think.thinking",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "start",
        module: "solstone.think.start",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "supervisor",
        module: "solstone.think.supervisor",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "schedule",
        module: "solstone.think.scheduler",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "maintenance",
        module: "solstone.think.maintenance_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "top",
        module: "solstone.think.top",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "health",
        module: "solstone.think.health_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "doctor",
        module: "solstone.think.doctor",
        preset_argv: EMPTY,
        kind: ProcessKind::Universal,
    },
    ProcessSpec {
        token: "check",
        module: "solstone.think.check",
        preset_argv: EMPTY,
        kind: ProcessKind::Universal,
    },
    ProcessSpec {
        token: "contract",
        module: "solstone.think.contract_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Universal,
    },
    ProcessSpec {
        token: "config",
        module: "solstone.think.config_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "install-models",
        module: "solstone.think.install_models",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "install-provider",
        module: "solstone.think.install_provider",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "settings",
        module: "solstone.think.settings_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "streams",
        module: "solstone.think.streams",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "segment",
        module: "solstone.think.segment",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "journal-stats",
        module: "solstone.think.journal_stats",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "reprocess",
        module: "solstone.think.reprocess",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "backfill-processing-records",
        module: "solstone.think.backfill_processing_records",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "warm",
        module: "solstone.think.warm",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "transcribe",
        module: "solstone.observe.transcribe",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "describe",
        module: "solstone.observe.describe",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "depict",
        module: "solstone.observe.depict",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "sense",
        module: "solstone.observe.sense",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "transfer",
        module: "solstone.observe.transfer",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "export",
        module: "solstone.observe.export",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "grab",
        module: "solstone.observe.grab",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "observer",
        module: "solstone.observe.observer_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "brain",
        module: "solstone.think.brain_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "facet-candidates",
        module: "solstone.think.facet_candidates_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "cortex",
        module: "solstone.think.cortex",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "talent",
        module: "solstone.think.talent_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "spl",
        module: "solstone.think.spl_native",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "navigate",
        module: "solstone.think.tools.navigate",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "identity",
        module: "solstone.think.tools.sol",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "engage",
        module: "solstone.think.engage",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "heartbeat",
        module: "solstone.think.heartbeat",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "convey",
        module: "solstone.convey.cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "restart-convey",
        module: "solstone.convey.restart",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "maint",
        module: "solstone.convey.maint_cli",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "service",
        module: "solstone.think.service",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "setup",
        module: "solstone.think.setup",
        preset_argv: EMPTY,
        kind: ProcessKind::Service,
    },
    ProcessSpec {
        token: "up",
        module: "solstone.think.service",
        preset_argv: UP,
        kind: ProcessKind::Alias,
    },
    ProcessSpec {
        token: "down",
        module: "solstone.think.service",
        preset_argv: DOWN,
        kind: ProcessKind::Alias,
    },
];

pub(crate) fn process_spec_for(token: &str) -> Option<&'static ProcessSpec> {
    PROCESS_SPECS.iter().find(|spec| spec.token == token)
}

pub(crate) fn native_process_spec_for(token: &str) -> Option<&'static NativeProcessSpec> {
    NATIVE_PROCESS_SPECS.iter().find(|spec| spec.token == token)
}

pub(crate) fn process_tokens() -> impl Iterator<Item = &'static str> {
    PROCESS_SPECS.iter().map(|spec| spec.token)
}
