// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod ci;
#[cfg(test)]
mod payload_inventory;
pub mod windows_crosscheck;

#[cfg(test)]
#[path = "contracts/ci_gate_purity.rs"]
mod ci_gate_purity;
#[cfg(test)]
#[path = "contracts/convergence_unwired.rs"]
mod convergence_unwired;
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
#[path = "contracts/observer_client_contract_bundle.rs"]
mod observer_client_contract_bundle;
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
#[path = "contracts/stats_dispatch_audit.rs"]
mod stats_dispatch_audit;
#[cfg(test)]
#[path = "contracts/talent_config_reader_architecture.rs"]
mod talent_config_reader_architecture;
#[cfg(test)]
#[path = "contracts/workspace_reachability.rs"]
mod workspace_reachability;
