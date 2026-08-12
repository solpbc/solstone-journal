// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_journal::installed_site_packages_from_executable_dir;

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

const HOST_DEPENDENCY_MODULES: &[(&str, &str)] = &[
    ("frontmatter", "python-frontmatter"),
    ("flask", "Flask"),
    ("onnxruntime", "ONNX runtime"),
];
const REINSTALL_GUIDANCE: &str = "Reinstall the journal host stack: pip install --upgrade solstone-journal  |  uv tool install --upgrade solstone-journal  |  pipx install --force solstone-journal. On an NVIDIA host use solstone-journal-cuda instead — never install both.";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let Some(site_packages) = installed_site_packages_from_executable_dir(&context.install_bin_dir)
    else {
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "could not resolve site-packages from install bin directory: {}",
                context.install_bin_dir.display()
            ),
            None::<String>,
        ));
    };
    let missing = HOST_DEPENDENCY_MODULES
        .iter()
        .filter_map(|(module, label)| {
            let package = site_packages.join(module);
            let single_file = site_packages.join(format!("{module}.py"));
            (!package.is_dir() && !single_file.is_file()).then_some(*label)
        })
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(make_result(
            check,
            Status::Ok,
            "journal host dependencies present: python-frontmatter, Flask, ONNX runtime",
            None::<String>,
        ));
    }
    Ok(make_result(
        check,
        Status::Fail,
        format!(
            "journal host stack incomplete — missing {}; the journal host is not installed or is incomplete.",
            missing.join(", ")
        ),
        Some(REINSTALL_GUIDANCE),
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{
        checks::test_support::{check, context, site_packages},
        registry::{self, Battery},
        vocabulary::{Severity, Status},
    };

    #[test]
    fn checks_staged_module_shapes_and_both_battery_bindings() {
        let staged = context();
        let site_packages = site_packages(&staged, "python3.12");
        fs::create_dir(site_packages.join("frontmatter")).expect("create frontmatter package");
        fs::write(site_packages.join("flask.py"), "").expect("create Flask module");
        fs::create_dir(site_packages.join("onnxruntime")).expect("create onnxruntime package");
        let check = check("host_dependencies", Severity::Blocker);
        assert_eq!(run(&staged, check).unwrap().status, Status::Ok);

        fs::remove_dir_all(site_packages.join("onnxruntime")).expect("remove onnxruntime package");
        assert_eq!(run(&staged, check).unwrap().status, Status::Fail);

        let unresolved = context();
        let result = run(&unresolved, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert!(result.detail.contains("could not resolve site-packages"));

        for battery in [Battery::Journal, Battery::JournalReadiness] {
            let entry = registry::lookup(battery, "host_dependencies")
                .expect("host dependency check is registered");
            assert_eq!(entry.check.severity, Severity::Blocker);
            assert!(entry.deferred.is_none());
        }
    }
}
