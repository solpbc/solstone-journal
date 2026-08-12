// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use nix::sys::statvfs::statvfs;
use solstone_core_journal::resolve_installation_root_from_executable_dir;

use crate::{
    context::CheckContext,
    vocabulary::{Check, ExecutionError, RunnerResult, Status, make_result},
};

// Threshold retained from the Python doctor rationale: .venv = 7.88 GiB plus
// first-install uv-cache growth of about 1 GiB and a safety buffer. It has not
// been re-derived for a native-only installation.
pub const MIN_FREE_GIB: f64 = 10.0;

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let Some(root) = resolve_installation_root_from_executable_dir(&context.install_bin_dir) else {
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "could not resolve installation root from install bin directory: {}",
                context.install_bin_dir.display()
            ),
            None::<String>,
        ));
    };
    let stats = statvfs(&root).map_err(|error| ExecutionError {
        kind: "StatvfsError".into(),
        message: format!(
            "could not inspect free space at {}: {error}",
            root.display()
        ),
    })?;
    let free_gib =
        (stats.blocks_available() as f64 * stats.fragment_size() as f64) / 1024_f64.powi(3);
    if free_gib < MIN_FREE_GIB {
        return Ok(make_result(
            check,
            Status::Warn,
            format!("only {free_gib:.1} GiB free on the repo filesystem (<{MIN_FREE_GIB:.0} GiB)"),
            Some("free disk on the repo filesystem before `make install`"),
        ));
    }
    Ok(make_result(
        check,
        Status::Ok,
        format!("{free_gib:.1} GiB free (>= {MIN_FREE_GIB:.0} GiB)"),
        None::<String>,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        checks::test_support::{check, context, site_packages},
        vocabulary::{Severity, Status},
    };

    #[test]
    fn uses_the_retained_ten_gib_threshold_and_staged_resolution() {
        // Python derived this from a 7.88 GiB .venv, about 1 GiB of initial
        // uv-cache growth, and a buffer. Native-only installs have not caused
        // this threshold to be re-derived.
        assert_eq!(MIN_FREE_GIB, 10.0);

        let staged = context();
        site_packages(&staged, "python3.12");
        let check = check("disk_space", Severity::Advisory);
        let result = run(&staged, check).expect("statvfs staged filesystem");
        assert!(matches!(result.status, Status::Ok | Status::Warn));

        let unresolved = context();
        let result = run(&unresolved, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert!(
            result
                .detail
                .contains("could not resolve installation root")
        );
    }
}
