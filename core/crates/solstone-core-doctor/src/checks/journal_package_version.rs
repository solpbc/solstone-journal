// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::package_metadata::{
        JOURNAL, JOURNAL_CUDA, JOURNAL_PACKAGES, SOLSTONE, installed, unavailable_detail,
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
    let Some(solstone) = versions.get(SOLSTONE) else {
        return Ok(make_result(
            check,
            Status::Skip,
            "solstone distribution metadata unavailable",
            None::<String>,
        ));
    };
    for leaf_name in [JOURNAL, JOURNAL_CUDA] {
        if let Some(leaf) = versions.get(leaf_name)
            && leaf.version == solstone.version
        {
            return Ok(make_result(
                check,
                Status::Ok,
                format!(
                    "journal leaf version matches solstone: {SOLSTONE} {}, {leaf_name} {}",
                    solstone.version, leaf.version
                ),
                None::<String>,
            ));
        }
    }
    let Some((leaf_name, leaf)) = versions
        .get(JOURNAL_CUDA)
        .map(|leaf| (JOURNAL_CUDA, leaf))
        .or_else(|| versions.get(JOURNAL).map(|leaf| (JOURNAL, leaf)))
    else {
        return Ok(make_result(
            check,
            Status::Skip,
            "no journal leaf installed",
            None::<String>,
        ));
    };
    Ok(make_result(
        check,
        Status::Fail,
        format!(
            "journal package version mismatch: {SOLSTONE} {}, {leaf_name} {}; a bare solstone upgrade may have outrun the journal leaf",
            solstone.version, leaf.version
        ),
        Some(format!(
            "upgrade the installed journal leaf: pip install --upgrade {leaf_name}  |  uv tool install --upgrade {leaf_name}"
        )),
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
    fn compares_staged_versions_and_skips_when_site_packages_is_unresolved() {
        let matching = context();
        let matching_site = site_packages(&matching, "python3.12");
        metadata(
            &matching_site,
            "solstone-1.2.3.dist-info",
            SOLSTONE,
            "1.2.3",
            None,
        );
        metadata(
            &matching_site,
            "solstone_journal-1.2.3.dist-info",
            JOURNAL,
            "1.2.3",
            None,
        );
        let check = check("journal_package_version", Severity::Blocker);
        assert_eq!(run(&matching, check).unwrap().status, Status::Ok);

        let mismatched = context();
        let mismatched_site = site_packages(&mismatched, "python3.12");
        metadata(
            &mismatched_site,
            "solstone-1.2.3.dist-info",
            SOLSTONE,
            "1.2.3",
            None,
        );
        metadata(
            &mismatched_site,
            "solstone_journal-1.2.2.dist-info",
            JOURNAL,
            "1.2.2",
            None,
        );
        assert_eq!(run(&mismatched, check).unwrap().status, Status::Fail);

        let unresolved = context();
        let result = run(&unresolved, check).unwrap();
        assert_eq!(result.status, Status::Skip);
        assert!(result.detail.contains("could not resolve site-packages"));
    }
}
