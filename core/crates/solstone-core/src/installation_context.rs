// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared executable/home-derived installation identity inputs.

use std::path::PathBuf;

use solstone_core_installation_identity::{IdentityError, OwnerBase, PlatformTag};
use solstone_core_journal::resolve_identity_root_from_executable_dir;

pub fn project_root_from_current_executable() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|executable| {
        executable
            .ancestors()
            .find(|path| path.join("pyproject.toml").is_file())
            .map(std::path::Path::to_path_buf)
    })
}

pub fn identity_root_from_current_executable() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not resolve current executable: {error}"))?;
    let executable_dir = executable
        .parent()
        .ok_or_else(|| "current executable has no containing directory".to_owned())?;
    let resolved = resolve_identity_root_from_executable_dir(executable_dir)
        .or_else(project_root_from_current_executable);
    #[cfg(debug_assertions)]
    let resolved = resolved.or_else(supervisor_fixture_project_root);
    resolved.ok_or_else(|| {
        format!(
            "could not resolve installation identity root from {}",
            executable_dir.display()
        )
    })
}

/// Integration-test-only root seam for binaries emitted to an external Cargo
/// target directory. Release builds do not compile this fallback, and debug
/// builds require the explicit supervisor app-fixture gate.
#[cfg(debug_assertions)]
fn supervisor_fixture_project_root() -> Option<PathBuf> {
    (std::env::var("SOLSTONE_SUPERVISOR_APP_FIXTURE").as_deref() == Ok("1"))
        .then(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .and_then(|root| root.canonicalize().ok())
}

pub fn owner_base_at_home(home: PathBuf) -> Result<OwnerBase, IdentityError> {
    OwnerBase::at_home(home, PlatformTag::current())
}

/// Locked recovery guidance shared by direct service and hosted-supervisor
/// installation-binding refusals.
pub fn installation_recovery_copy(detail: &str) -> String {
    let detail = solstone_core_system_health::sanitize_str_for_terminal_bounded(detail);
    format!(
        "this installation couldn't be verified.\nrun `journal setup` to check it. if setup finishes successfully, try again.\ndetails: {detail}"
    )
}

#[cfg(test)]
mod tests {
    use super::installation_recovery_copy;

    #[test]
    fn installation_recovery_copy_is_locked() {
        let copy = installation_recovery_copy("binding\\detail\n\x1b");
        assert!(copy.starts_with(
            "this installation couldn't be verified.\nrun `journal setup` to check it. if setup finishes successfully, try again.\ndetails: "
        ));
        assert!(copy.ends_with("binding\\\\detail\\n\\x1b"));

        let oversized = installation_recovery_copy(&"\x1b".repeat(1025));
        assert!(oversized.ends_with("…[truncated]"));
        let detail = oversized.strip_prefix(
            "this installation couldn't be verified.\nrun `journal setup` to check it. if setup finishes successfully, try again.\ndetails: "
        ).expect("locked recovery prefix");
        assert_eq!(detail.chars().count(), 2048);
    }
}
