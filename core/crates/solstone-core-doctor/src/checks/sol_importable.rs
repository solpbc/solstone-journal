// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_journal::{
    python_site_packages_from_executable_dir, resolve_installation_root_from_executable_dir,
};

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

const REINSTALL: &str = "rm -rf .venv .installed && make install";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let Some(root) = resolve_installation_root_from_executable_dir(&context.install_bin_dir) else {
        if let Some(site_packages) =
            python_site_packages_from_executable_dir(&context.install_bin_dir)
        {
            return Ok(packaged_result(check, &site_packages));
        }
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "could not resolve source checkout or site-packages from install bin directory: {}",
                context.install_bin_dir.display()
            ),
            None::<String>,
        ));
    };
    if is_source_checkout(&root) {
        let python = root.join(".venv/bin/python");
        if !python.exists() {
            return Ok(make_result(
                check,
                Status::Skip,
                ".venv absent; run make install",
                None::<String>,
            ));
        }
        return Ok(make_result(
            check,
            Status::Ok,
            format!(
                "solstone package and .venv found under {} (native check verifies presence only; it does not execute an import)",
                root.display()
            ),
            None::<String>,
        ));
    }
    Ok(packaged_result(check, &root))
}

fn is_source_checkout(root: &Path) -> bool {
    root.join("pyproject.toml").is_file() && root.join(".git").exists()
}

fn packaged_result(check: Check, site_packages: &Path) -> crate::vocabulary::CheckResult {
    if site_packages.join("solstone/__init__.py").is_file() {
        return make_result(
            check,
            Status::Ok,
            format!(
                "solstone package found at {}/solstone (native check verifies presence only; it does not execute an import)",
                site_packages.display()
            ),
            None::<String>,
        );
    }
    make_result(
        check,
        Status::Fail,
        format!(
            "solstone package not found under {}",
            site_packages.display()
        ),
        Some(REINSTALL),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{
        checks::test_support::{check, context, site_packages},
        vocabulary::{Severity, Status},
    };

    #[test]
    fn reports_honest_packaged_presence_and_not_found_results() {
        let staged = context();
        site_packages(&staged, "python3.12");
        let check = check("sol_importable", Severity::Blocker);
        let result = run(&staged, check).unwrap();
        assert_eq!(result.status, Status::Ok);
        assert!(result.detail.contains("does not execute an import"));

        let missing_context = context();
        let missing = missing_context
            .install_bin_dir
            .parent()
            .expect("staged install prefix")
            .join("lib/python3.12/site-packages");
        fs::create_dir_all(&missing).expect("create empty site-packages");
        let result = run(&missing_context, check).unwrap();
        assert_eq!(result.status, Status::Fail);
        assert_eq!(
            result.detail,
            format!("solstone package not found under {}", missing.display())
        );

        let unresolved = context();
        let result = run(&unresolved, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert!(
            result
                .detail
                .contains("could not resolve source checkout or site-packages")
        );
    }

    #[test]
    fn checks_staged_checkout_venv_presence_without_executing_python() {
        let mut staged = context();
        let root = staged
            .install_bin_dir
            .parent()
            .and_then(Path::parent)
            .expect("staged root")
            .to_path_buf();
        fs::create_dir_all(root.join("solstone")).expect("create checkout package");
        fs::create_dir(root.join(".git")).expect("create checkout marker");
        fs::write(root.join("pyproject.toml"), "").expect("write checkout marker");
        let bin = root.join(".venv/bin");
        fs::create_dir_all(&bin).expect("create staged venv bin");
        staged.context.install_bin_dir = bin.clone();
        let check = check("sol_importable", Severity::Blocker);
        assert_eq!(run(&staged, check).unwrap().status, Status::Skip);

        fs::write(bin.join("python"), "poison must not run").expect("write staged python");
        let result = run(&staged, check).unwrap();
        assert_eq!(result.status, Status::Ok);
        assert!(result.detail.contains("does not execute an import"));
    }
}
