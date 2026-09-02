// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod advisory_audit;
pub mod ci;
#[cfg(test)]
mod payload_inventory;
pub mod release_manifest;
pub mod windows_crosscheck;

#[cfg(test)]
#[path = "contracts/backup_admission_caller_purity.rs"]
mod backup_admission_caller_purity;
#[cfg(test)]
#[path = "contracts/ci_gate_purity.rs"]
mod ci_gate_purity;
#[cfg(test)]
#[path = "contracts/client_ingest_contract_bundle.rs"]
mod client_ingest_contract_bundle;
#[cfg(test)]
#[path = "contracts/convey_shell_assets.rs"]
mod convey_shell_assets;
#[cfg(test)]
#[path = "contracts/distribution_install_archive_refusals.rs"]
mod distribution_install_archive_refusals;
#[cfg(test)]
#[path = "contracts/distribution_install_basename.rs"]
mod distribution_install_basename;
#[cfg(test)]
#[path = "contracts/distribution_install_tmpdir.rs"]
mod distribution_install_tmpdir;
#[cfg(test)]
#[path = "contracts/distribution_launchers.rs"]
mod distribution_launchers;
#[cfg(test)]
#[path = "contracts/distribution_model_digests.rs"]
mod distribution_model_digests;
#[cfg(test)]
#[path = "contracts/distribution_no_independent_resolvers.rs"]
mod distribution_no_independent_resolvers;
#[cfg(test)]
#[path = "contracts/distribution_onnx_runtime_pins.rs"]
mod distribution_onnx_runtime_pins;
#[cfg(test)]
#[path = "contracts/distribution_payload.rs"]
mod distribution_payload;
#[cfg(test)]
#[path = "contracts/distribution_workspace_bins.rs"]
mod distribution_workspace_bins;
#[cfg(test)]
#[path = "contracts/hosted_launch_admission_boundary.rs"]
mod hosted_launch_admission_boundary;
#[cfg(test)]
#[path = "contracts/journal_windows_target_gate.rs"]
mod journal_windows_target_gate;
#[cfg(test)]
#[path = "contracts/mcp_audit_boundary.rs"]
mod mcp_audit_boundary;
#[cfg(test)]
#[path = "contracts/mcp_endpoint_exclusion_coherence.rs"]
mod mcp_endpoint_exclusion_coherence;
#[cfg(test)]
#[path = "contracts/mcp_endpoint_gate_purity.rs"]
mod mcp_endpoint_gate_purity;
#[cfg(test)]
#[path = "contracts/paired_stream_allocator_governance.rs"]
mod paired_stream_allocator_governance;
#[cfg(test)]
#[path = "contracts/pairing_contract_bundle.rs"]
mod pairing_contract_bundle;

#[cfg(test)]
#[path = "contracts/bound_read_race_closure.rs"]
mod bound_read_race_closure;
#[cfg(test)]
#[path = "contracts/mcp_endpoint_production_source_purity.rs"]
mod mcp_endpoint_production_source_purity;
#[cfg(test)]
#[path = "contracts/retention_client_contracts.rs"]
mod retention_client_contracts;
#[cfg(test)]
#[path = "contracts/retention_projection_architecture.rs"]
mod retention_projection_architecture;
#[cfg(test)]
#[path = "contracts/rust_solstone_compile_inputs.rs"]
mod rust_solstone_compile_inputs;
#[cfg(test)]
#[path = "contracts/schedule_read_only_architecture.rs"]
mod schedule_read_only_architecture;
#[cfg(test)]
#[path = "contracts/service_legacy_gate_purity.rs"]
mod service_legacy_gate_purity;
#[cfg(test)]
#[path = "contracts/settings_devices_python_web_cut.rs"]
mod settings_devices_python_web_cut;
#[cfg(test)]
#[path = "contracts/speaker_native_routes.rs"]
mod speaker_native_routes;
#[cfg(test)]
#[path = "contracts/spl_source_coherence.rs"]
mod spl_source_coherence;
#[cfg(test)]
#[path = "contracts/stats_dispatch_audit.rs"]
mod stats_dispatch_audit;
#[cfg(test)]
#[path = "contracts/stream_name_identity_consumers.rs"]
mod stream_name_identity_consumers;
#[cfg(test)]
#[path = "contracts/talent_config_reader_architecture.rs"]
mod talent_config_reader_architecture;
#[cfg(test)]
#[path = "contracts/workspace_reachability.rs"]
mod workspace_reachability;
