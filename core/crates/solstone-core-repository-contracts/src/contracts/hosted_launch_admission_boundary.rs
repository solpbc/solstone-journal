// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Hosted-generation service descendants may enter the process boundary only
//! through provenance-aware authority launch APIs. This intentionally scans
//! the closed service-like inventory rather than one-shot implementation
//! helpers owned by an already-admitted descendant.

use std::collections::BTreeSet;

const CORE_MAIN: &str = include_str!("../../../solstone-core/src/main.rs");
const SUPERVISOR_MOD: &str = include_str!("../../../solstone-core/src/supervisor/mod.rs");
const CORTEX_MOD: &str = include_str!("../../../solstone-core-cortex/src/lib.rs");
const SENSE_MOD: &str = include_str!("../../../solstone-core-sense/src/lib.rs");
const CONVEY_SHELL: &str = include_str!("../../../solstone-core-convey-shell/src/lib.rs");
const SPL_MOD: &str = include_str!("../../../solstone-core-spl/src/lib.rs");

// Keep this complete with the direct module declarations in the crate roots
// above. The coverage test below is deliberately bidirectional: a new module
// cannot silently become a hosted-launch blind spot.
const SCANNED_MODULE_SOURCES: &[(&str, &str, &str)] = &[
    (
        "supervisor",
        "bus",
        include_str!("../../../solstone-core/src/supervisor/bus.rs"),
    ),
    (
        "supervisor",
        "config",
        include_str!("../../../solstone-core/src/supervisor/config.rs"),
    ),
    (
        "supervisor",
        "host",
        include_str!("../../../solstone-core/src/supervisor/host.rs"),
    ),
    (
        "supervisor",
        "receipt",
        include_str!("../../../solstone-core/src/supervisor/receipt.rs"),
    ),
    (
        "supervisor",
        "runtime",
        include_str!("../../../solstone-core/src/supervisor/runtime.rs"),
    ),
    (
        "supervisor",
        "shutdown",
        include_str!("../../../solstone-core/src/supervisor/shutdown.rs"),
    ),
    (
        "supervisor",
        "tick",
        include_str!("../../../solstone-core/src/supervisor/tick.rs"),
    ),
    (
        "cortex",
        "process",
        include_str!("../../../solstone-core-cortex/src/process.rs"),
    ),
    (
        "cortex",
        "renewal",
        include_str!("../../../solstone-core-cortex/src/renewal.rs"),
    ),
    (
        "cortex",
        "service",
        include_str!("../../../solstone-core-cortex/src/service.rs"),
    ),
    (
        "cortex",
        "state",
        include_str!("../../../solstone-core-cortex/src/state.rs"),
    ),
    (
        "cortex",
        "storage",
        include_str!("../../../solstone-core-cortex/src/storage.rs"),
    ),
    (
        "cortex",
        "test_hooks",
        include_str!("../../../solstone-core-cortex/src/test_hooks.rs"),
    ),
    (
        "sense",
        "batch",
        include_str!("../../../solstone-core-sense/src/batch.rs"),
    ),
    (
        "sense",
        "beacon",
        include_str!("../../../solstone-core-sense/src/beacon.rs"),
    ),
    (
        "sense",
        "config",
        include_str!("../../../solstone-core-sense/src/config.rs"),
    ),
    (
        "sense",
        "dispatch",
        include_str!("../../../solstone-core-sense/src/dispatch.rs"),
    ),
    (
        "sense",
        "events",
        include_str!("../../../solstone-core-sense/src/events.rs"),
    ),
    (
        "sense",
        "memory",
        include_str!("../../../solstone-core-sense/src/memory.rs"),
    ),
    (
        "sense",
        "registry",
        include_str!("../../../solstone-core-sense/src/registry.rs"),
    ),
    (
        "sense",
        "service",
        include_str!("../../../solstone-core-sense/src/service.rs"),
    ),
    (
        "sense",
        "work",
        include_str!("../../../solstone-core-sense/src/work.rs"),
    ),
    (
        "convey",
        "assets",
        include_str!("../../../solstone-core-convey-shell/src/assets.rs"),
    ),
    (
        "convey",
        "authorization_gate",
        include_str!("../../../solstone-core-convey-shell/src/authorization_gate.rs"),
    ),
    (
        "convey",
        "body",
        include_str!("../../../solstone-core-convey-shell/src/body.rs"),
    ),
    (
        "convey",
        "clients",
        include_str!("../../../solstone-core-convey-shell/src/clients.rs"),
    ),
    (
        "convey",
        "door",
        include_str!("../../../solstone-core-convey-shell/src/door.rs"),
    ),
    (
        "convey",
        "entities",
        include_str!("../../../solstone-core-convey-shell/src/entities.rs"),
    ),
    (
        "convey",
        "link_health_cache",
        include_str!("../../../solstone-core-convey-shell/src/link_health_cache.rs"),
    ),
    (
        "convey",
        "network",
        include_str!("../../../solstone-core-convey-shell/src/network.rs"),
    ),
    (
        "convey",
        "network_status",
        include_str!("../../../solstone-core-convey-shell/src/network_status.rs"),
    ),
    (
        "convey",
        "network_writes",
        include_str!("../../../solstone-core-convey-shell/src/network_writes.rs"),
    ),
    (
        "convey",
        "pair_window_manager",
        include_str!("../../../solstone-core-convey-shell/src/pair_window_manager.rs"),
    ),
    (
        "convey",
        "refusal",
        include_str!("../../../solstone-core-convey-shell/src/refusal.rs"),
    ),
    (
        "convey",
        "registry",
        include_str!("../../../solstone-core-convey-shell/src/registry.rs"),
    ),
    (
        "convey",
        "relay_admission",
        include_str!("../../../solstone-core-convey-shell/src/relay_admission.rs"),
    ),
    (
        "convey",
        "session",
        include_str!("../../../solstone-core-convey-shell/src/session.rs"),
    ),
    (
        "convey",
        "session_gate",
        include_str!("../../../solstone-core-convey-shell/src/session_gate.rs"),
    ),
    (
        "convey",
        "speakers",
        include_str!("../../../solstone-core-convey-shell/src/speakers.rs"),
    ),
    (
        "convey",
        "speakers_analyze_client",
        include_str!("../../../solstone-core-convey-shell/src/speakers_analyze_client.rs"),
    ),
    (
        "convey",
        "speakers_attribution",
        include_str!("../../../solstone-core-convey-shell/src/speakers_attribution.rs"),
    ),
    (
        "convey",
        "speakers_calendar",
        include_str!("../../../solstone-core-convey-shell/src/speakers_calendar.rs"),
    ),
    (
        "convey",
        "speakers_cli_discovery",
        include_str!("../../../solstone-core-convey-shell/src/speakers_cli_discovery.rs"),
    ),
    (
        "convey",
        "speakers_cli_entities",
        include_str!("../../../solstone-core-convey-shell/src/speakers_cli_entities.rs"),
    ),
    (
        "convey",
        "speakers_cli_maintenance",
        include_str!("../../../solstone-core-convey-shell/src/speakers_cli_maintenance.rs"),
    ),
    (
        "convey",
        "speakers_cli_owner",
        include_str!("../../../solstone-core-convey-shell/src/speakers_cli_owner.rs"),
    ),
    (
        "convey",
        "speakers_cli_reads",
        include_str!("../../../solstone-core-convey-shell/src/speakers_cli_reads.rs"),
    ),
    (
        "convey",
        "speakers_discovery",
        include_str!("../../../solstone-core-convey-shell/src/speakers_discovery.rs"),
    ),
    (
        "convey",
        "speakers_discovery_write",
        include_str!("../../../solstone-core-convey-shell/src/speakers_discovery_write.rs"),
    ),
    (
        "convey",
        "speakers_known",
        include_str!("../../../solstone-core-convey-shell/src/speakers_known.rs"),
    ),
    (
        "convey",
        "speakers_media",
        include_str!("../../../solstone-core-convey-shell/src/speakers_media.rs"),
    ),
    (
        "convey",
        "speakers_npz",
        include_str!("../../../solstone-core-convey-shell/src/speakers_npz.rs"),
    ),
    (
        "convey",
        "speakers_owner",
        include_str!("../../../solstone-core-convey-shell/src/speakers_owner.rs"),
    ),
    (
        "convey",
        "speakers_owner_write",
        include_str!("../../../solstone-core-convey-shell/src/speakers_owner_write.rs"),
    ),
    (
        "convey",
        "speakers_quality",
        include_str!("../../../solstone-core-convey-shell/src/speakers_quality.rs"),
    ),
    (
        "convey",
        "speakers_review",
        include_str!("../../../solstone-core-convey-shell/src/speakers_review.rs"),
    ),
    (
        "convey",
        "speakers_segment_catalog",
        include_str!("../../../solstone-core-convey-shell/src/speakers_segment_catalog.rs"),
    ),
    (
        "convey",
        "sse",
        include_str!("../../../solstone-core-convey-shell/src/sse.rs"),
    ),
    (
        "convey",
        "status_mark",
        include_str!("../../../solstone-core-convey-shell/src/status_mark.rs"),
    ),
    (
        "convey",
        "system",
        include_str!("../../../solstone-core-convey-shell/src/system.rs"),
    ),
    (
        "convey",
        "thinking",
        include_str!("../../../solstone-core-convey-shell/src/thinking.rs"),
    ),
    (
        "convey",
        "thinking_sol_reads",
        include_str!("../../../solstone-core-convey-shell/src/thinking_sol_reads.rs"),
    ),
    (
        "convey",
        "thinking_sol_reads_contract",
        include_str!("../../../solstone-core-convey-shell/src/thinking_sol_reads_contract.rs"),
    ),
    (
        "convey",
        "thinking_sol_writes",
        include_str!("../../../solstone-core-convey-shell/src/thinking_sol_writes.rs"),
    ),
    (
        "convey",
        "thinking_sol_writes_contract",
        include_str!("../../../solstone-core-convey-shell/src/thinking_sol_writes_contract.rs"),
    ),
    (
        "spl",
        "admission",
        include_str!("../../../solstone-core-spl/src/admission.rs"),
    ),
    (
        "spl",
        "callosum",
        include_str!("../../../solstone-core-spl/src/callosum.rs"),
    ),
    (
        "spl",
        "health",
        include_str!("../../../solstone-core-spl/src/health.rs"),
    ),
    (
        "spl",
        "link_state_files",
        include_str!("../../../solstone-core-spl/src/link_state_files.rs"),
    ),
    (
        "spl",
        "loopback_pipe",
        include_str!("../../../solstone-core-spl/src/loopback_pipe.rs"),
    ),
    (
        "spl",
        "pair_window_client",
        include_str!("../../../solstone-core-spl/src/pair_window_client.rs"),
    ),
    (
        "spl",
        "posture_gate",
        include_str!("../../../solstone-core-spl/src/posture_gate.rs"),
    ),
    (
        "spl",
        "private_link",
        include_str!("../../../solstone-core-spl/src/private_link.rs"),
    ),
    (
        "spl",
        "reconnect_backoff",
        include_str!("../../../solstone-core-spl/src/reconnect_backoff.rs"),
    ),
    (
        "spl",
        "relay_client",
        include_str!("../../../solstone-core-spl/src/relay_client.rs"),
    ),
    (
        "spl",
        "relay_control",
        include_str!("../../../solstone-core-spl/src/relay_control.rs"),
    ),
    (
        "spl",
        "relay_health",
        include_str!("../../../solstone-core-spl/src/relay_health.rs"),
    ),
    (
        "spl",
        "relay_status_failure",
        include_str!("../../../solstone-core-spl/src/relay_status_failure.rs"),
    ),
    (
        "spl",
        "relay_websocket",
        include_str!("../../../solstone-core-spl/src/relay_websocket.rs"),
    ),
    (
        "spl",
        "service",
        include_str!("../../../solstone-core-spl/src/service.rs"),
    ),
    (
        "spl",
        "service_process",
        include_str!("../../../solstone-core-spl/src/service_process.rs"),
    ),
    (
        "spl",
        "service_shutdown",
        include_str!("../../../solstone-core-spl/src/service_shutdown.rs"),
    ),
    (
        "spl",
        "service_transition",
        include_str!("../../../solstone-core-spl/src/service_transition.rs"),
    ),
    (
        "spl",
        "tunnel_route",
        include_str!("../../../solstone-core-spl/src/tunnel_route.rs"),
    ),
    (
        "spl",
        "ws_buffer",
        include_str!("../../../solstone-core-spl/src/ws_buffer.rs"),
    ),
    (
        "spl",
        "ws_sink",
        include_str!("../../../solstone-core-spl/src/ws_sink.rs"),
    ),
];

const RAW_BYPASS_EXEMPT_SOURCES: &[(&str, &str)] = &[
    // The one-shot Sense memory helper is explicitly outside the service-like
    // generation boundary; it remains enumerated so new modules cannot evade
    // the inventory check.
    ("sense", "memory"),
    // These modules contain source-code examples for their own contracts, not
    // production command construction.
    ("convey", "thinking_sol_reads_contract"),
    ("convey", "thinking_sol_writes_contract"),
];

fn declared_modules(source: &str) -> BTreeSet<&str> {
    source
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("pub mod ")
                .or_else(|| line.strip_prefix("mod "))
                .and_then(|rest| rest.strip_suffix(';'))
        })
        .collect()
}

fn scanned_modules(crate_name: &str) -> BTreeSet<&str> {
    SCANNED_MODULE_SOURCES
        .iter()
        .filter(|(owner, _, _)| *owner == crate_name)
        .map(|(_, module, _)| *module)
        .collect()
}

fn raw_launch_surface(source: &str) -> Option<&'static str> {
    [
        "Command::new",
        "std::process::Command",
        "tokio::process::Command",
        "ManagedProcess::spawn",
        "ManagedProcess::spawn_exact",
    ]
    .into_iter()
    .find(|forbidden| source.contains(forbidden))
}

fn production_source(source: &str) -> &str {
    source
        .split_once("mod tests")
        .map_or(source, |(production, _)| production)
}

fn raw_bypass_exempt(owner: &str, module: &str) -> bool {
    RAW_BYPASS_EXEMPT_SOURCES.contains(&(owner, module))
}

#[test]
fn scan_covers_every_declared_hosted_launch_module() {
    for (owner, root) in [
        ("supervisor", SUPERVISOR_MOD),
        ("cortex", CORTEX_MOD),
        ("sense", SENSE_MOD),
        ("convey", CONVEY_SHELL),
        ("spl", SPL_MOD),
    ] {
        assert_eq!(
            declared_modules(root),
            scanned_modules(owner),
            "{owner} crate-root declarations and hosted-launch scan diverged"
        );
    }
}

#[test]
fn hosted_service_sources_reject_raw_process_launches() {
    assert!(raw_launch_surface(production_source(CORE_MAIN)).is_none());
    for (owner, module, source) in SCANNED_MODULE_SOURCES {
        if raw_bypass_exempt(owner, module) {
            continue;
        }
        let source = production_source(source);
        assert!(
            raw_launch_surface(source).is_none(),
            "{owner}/{module} bypasses the hosted launch admission boundary with {}",
            raw_launch_surface(source).expect("rejected source identifies its raw surface")
        );
    }
}

#[test]
fn every_service_like_hosted_launch_selects_provenance() {
    let source = |owner, module| {
        SCANNED_MODULE_SOURCES
            .iter()
            .find_map(|(source_owner, source_module, source)| {
                (*source_owner == owner && *source_module == module).then_some(*source)
            })
            .expect("hosted launch source is scanned")
    };
    let supervisor_runtime = production_source(source("supervisor", "runtime"));
    let cortex_process = production_source(source("cortex", "process"));
    let sense_dispatch = production_source(source("sense", "dispatch"));
    let convey_speakers_analyze = production_source(source("convey", "speakers_analyze_client"));
    let spl_service_process = production_source(source("spl", "service_process"));

    assert!(supervisor_runtime.contains("launch_managed_hosted("));
    assert!(supervisor_runtime.contains("HostedLaunchProvenance {"));
    assert!(cortex_process.contains("process::launch_command_hosted("));
    assert!(cortex_process.contains("child_launch_provenance("));
    assert!(sense_dispatch.contains("launch_managed_hosted("));
    assert!(sense_dispatch.contains("child_launch_provenance("));
    assert!(convey_speakers_analyze.contains("launch_command_hosted("));
    assert!(convey_speakers_analyze.contains("child_launch_provenance("));

    // SPL has no service-like child-launch boundary in the closed inventory.
    for forbidden in ["launch_command", "launch_managed", "CommandLaunchRequest"] {
        assert!(
            !spl_service_process.contains(forbidden),
            "SPL unexpectedly gained a child launch path: {forbidden}"
        );
    }
}

#[test]
fn raw_launch_rejection_is_load_bearing() {
    assert_eq!(
        raw_launch_surface("Command::new(\"child\")"),
        Some("Command::new")
    );
}
