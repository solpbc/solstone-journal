// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::package_metadata::{
        JOURNAL, JOURNAL_CUDA, JOURNAL_PACKAGES, installed, unavailable_detail,
    },
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let Some(versions) = installed(context, JOURNAL_PACKAGES)? else {
        return Ok(make_result(
            check,
            Status::Skip,
            unavailable_detail(context),
            None::<String>,
        ));
    };
    let cpu = versions.get(JOURNAL);
    let cuda = versions.get(JOURNAL_CUDA);
    match (cpu, cuda) {
        (None, None) => Ok(make_result(
            check,
            Status::Skip,
            "no journal leaf installed",
            None::<String>,
        )),
        (Some(leaf), None) => Ok(make_result(
            check,
            Status::Ok,
            format!("single journal leaf installed: {JOURNAL} {}", leaf.version),
            None::<String>,
        )),
        (None, Some(leaf)) => Ok(make_result(
            check,
            Status::Ok,
            format!(
                "single journal leaf installed: {JOURNAL_CUDA} {}",
                leaf.version
            ),
            None::<String>,
        )),
        (Some(cpu), Some(cuda)) => Ok(make_result(
            check,
            Status::Fail,
            format!(
                "both journal leaves are installed: {JOURNAL} {}, {JOURNAL_CUDA} {}; CPU and CUDA ONNX runtimes own the same files",
                cpu.version, cuda.version
            ),
            Some(
                "uninstall both journal leaves, then reinstall exactly one: pip uninstall -y solstone-journal solstone-journal-cuda; then pip install solstone-journal OR pip install solstone-journal-cuda",
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        checks::test_support::{check, context, metadata, site_packages},
        vocabulary::{Severity, Status},
    };

    #[test]
    fn reports_staged_leaf_states_and_missing_site_packages() {
        let staged = context();
        let site_packages = site_packages(&staged, "python3.12");
        metadata(
            &site_packages,
            "solstone_journal-1.2.3.dist-info",
            JOURNAL,
            "1.2.3",
            None,
        );
        let check = check("journal_leaf_exclusivity", Severity::Blocker);
        assert_eq!(run(&staged, check).unwrap().status, Status::Ok);

        metadata(
            &site_packages,
            "solstone_journal_cuda-1.2.3.dist-info",
            JOURNAL_CUDA,
            "1.2.3",
            None,
        );
        assert_eq!(run(&staged, check).unwrap().status, Status::Fail);

        let unresolved = context();
        let result = run(&unresolved, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert!(result.detail.contains("could not resolve site-packages"));
    }
}
