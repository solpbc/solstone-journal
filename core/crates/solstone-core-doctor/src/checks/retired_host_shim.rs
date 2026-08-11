// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::package_metadata::{
        JOURNAL, JOURNAL_CUDA, JOURNAL_HOST, JOURNAL_PACKAGES, installed, unavailable_detail,
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
    let Some(host) = versions.get(JOURNAL_HOST) else {
        return Ok(make_result(
            check,
            Status::Ok,
            "retired solstone-journal-host not installed",
            None::<String>,
        ));
    };
    let leaf = versions
        .get(JOURNAL_CUDA)
        .map(|leaf| (JOURNAL_CUDA, leaf))
        .or_else(|| versions.get(JOURNAL).map(|leaf| (JOURNAL, leaf)));
    let Some((leaf_name, leaf)) = leaf else {
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "solstone-journal-host {} installed without a journal leaf; journal service commands will show migration guidance",
                host.version
            ),
            None::<String>,
        ));
    };
    Ok(make_result(
        check,
        Status::Warn,
        format!(
            "retired solstone-journal-host {} is installed alongside {leaf_name} {}",
            host.version, leaf.version
        ),
        Some("pip uninstall solstone-journal-host"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        checks::test_support::{check, context, metadata, site_packages},
        vocabulary::{Severity, Status},
    };

    #[test]
    fn reports_retired_shim_states_from_staged_metadata() {
        let absent = context();
        let absent_site = site_packages(&absent, "python3.12");
        let check = check("retired_host_shim", Severity::Advisory);
        assert_eq!(run(&absent, check).unwrap().status, Status::Ok);

        metadata(
            &absent_site,
            "solstone_journal_host-1.2.3.dist-info",
            JOURNAL_HOST,
            "1.2.3",
            None,
        );
        metadata(
            &absent_site,
            "solstone_journal-1.2.3.dist-info",
            JOURNAL,
            "1.2.3",
            None,
        );
        assert_eq!(run(&absent, check).unwrap().status, Status::Warn);

        let unresolved = context();
        let result = run(&unresolved, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert!(result.detail.contains("could not resolve site-packages"));
    }
}
