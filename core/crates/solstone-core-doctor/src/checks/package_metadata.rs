// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use solstone_core_journal::{
    InstalledDistribution, installed_distributions, installed_site_packages_from_executable_dir,
};

use crate::{context::CheckContext, vocabulary::ExecutionError};

pub(crate) const SOLSTONE: &str = "solstone";
pub(crate) const JOURNAL: &str = "solstone-journal";
pub(crate) const JOURNAL_CUDA: &str = "solstone-journal-cuda";
pub(crate) const JOURNAL_HOST: &str = "solstone-journal-host";
pub(crate) const JOURNAL_PACKAGES: &[&str] = &[SOLSTONE, JOURNAL, JOURNAL_CUDA, JOURNAL_HOST];

pub(crate) fn installed(
    context: &CheckContext,
    targets: &[&str],
) -> Result<Option<BTreeMap<String, InstalledDistribution>>, ExecutionError> {
    let Some(site_packages) = installed_site_packages_from_executable_dir(&context.install_bin_dir)
    else {
        return Ok(None);
    };
    installed_distributions(&site_packages, targets)
        .map(Some)
        .map_err(|error| ExecutionError {
            kind: "DistributionMetadataError".into(),
            message: error.to_string(),
        })
}

pub(crate) fn unavailable_detail(context: &CheckContext) -> String {
    format!(
        "could not resolve site-packages from install bin directory: {}",
        context.install_bin_dir.display()
    )
}
