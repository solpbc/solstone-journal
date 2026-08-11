// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
pub mod config_dir_readable;
pub mod disk_space;
pub mod host_dependencies;
pub mod journal_dir_writable;
pub mod journal_leaf_exclusivity;
pub mod journal_package_version;
pub mod launchd_stale_plist;
pub mod local_bin_sol_reachable;
pub(crate) mod managed_wrapper;
pub mod package_metadata;
pub mod python_version;
pub mod retired_host_shim;
pub mod service_identity;
pub mod service_running;
pub mod service_status;
pub mod sol_importable;
pub mod stale_alias_symlink;
pub mod supervisor_conflict;

#[cfg(test)]
pub(crate) mod test_support;
