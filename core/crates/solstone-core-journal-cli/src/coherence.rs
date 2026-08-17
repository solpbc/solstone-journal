// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use solstone_core_journal::{installed_distributions, installed_site_packages_from_executable_dir};

const SOLSTONE: &str = "solstone";
const JOURNAL: &str = "solstone-journal";
const JOURNAL_CUDA: &str = "solstone-journal-cuda";
const JOURNAL_HOST: &str = "solstone-journal-host";
const TARGETS: &[&str] = &[SOLSTONE, JOURNAL, JOURNAL_CUDA, JOURNAL_HOST];

#[derive(Debug)]
pub(crate) struct CoherenceError {
    message: String,
}

impl CoherenceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CoherenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CoherenceError {}

pub(crate) fn guard_current_installation() -> Result<(), CoherenceError> {
    let executable = std::env::current_exe().map_err(|error| {
        CoherenceError::new(format!(
            "native journal package coherence check failed: could not inspect current executable: {error}"
        ))
    })?;
    let Some(executable_dir) = executable.parent() else {
        return Ok(());
    };
    guard_site_packages(installed_site_packages_from_executable_dir(executable_dir).as_deref())
}

pub(crate) fn guard_site_packages(site_packages: Option<&Path>) -> Result<(), CoherenceError> {
    let Some(site_packages) = site_packages else {
        return Ok(());
    };
    let versions = installed_versions(site_packages)?;
    let Some(solstone_version) = versions.get(SOLSTONE) else {
        return Ok(());
    };
    let leaves = [
        (JOURNAL, versions.get(JOURNAL)),
        (JOURNAL_CUDA, versions.get(JOURNAL_CUDA)),
    ];
    if leaves
        .iter()
        .any(|(_, version)| version.is_some_and(|version| version == solstone_version))
    {
        return Ok(());
    }
    if leaves.iter().any(|(_, version)| version.is_some()) {
        let (leaf_name, leaf_version) = versions
            .get(JOURNAL_CUDA)
            .map(|version| (JOURNAL_CUDA, version))
            .or_else(|| versions.get(JOURNAL).map(|version| (JOURNAL, version)))
            .expect("a present leaf version was checked above");
        return Err(CoherenceError::new(format!(
            "Journal package versions are out of sync.\n\nsolstone is installed at {solstone_version}, but {leaf_name} is installed at {leaf_version}.\nThis usually happens when a bare `pip install --upgrade solstone` upgraded the\nthin client without upgrading the journal package.\n\nUpgrade the installed journal package:\n    pip install --upgrade {leaf_name}\n    uv tool install --upgrade {leaf_name}\n"
        )));
    }
    if versions.contains_key(JOURNAL_HOST) {
        return Err(CoherenceError::new(
            "solstone[journal] and solstone-journal-host have moved.\n\nThe journal is now its own package:\n\n    pip install solstone-journal          # the journal (CPU)\n    pip install solstone-journal-cuda     # the journal on NVIDIA CUDA\n\nOne-time migration for uv tool installs:\n\n    uv tool uninstall solstone && uv tool install solstone-journal && uv tool install solstone\n\nNothing was changed by this failed command.\nSee https://github.com/solpbc/solstone-journal/blob/main/INSTALL.md\n",
        ));
    }
    Ok(())
}

fn installed_versions(site_packages: &Path) -> Result<BTreeMap<String, String>, CoherenceError> {
    installed_distributions(site_packages, TARGETS)
        .map_err(|error| {
            CoherenceError::new(format!(
                "native journal package coherence check failed: {error}"
            ))
        })
        .map(|distributions| {
            distributions
                .into_iter()
                .map(|(name, distribution)| (name, distribution.version))
                .collect()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use std::fs;

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let path = reserve_temp_path("solstone-core-journal-cli-coherence");
            fs::create_dir(&path).expect("create temporary test directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn metadata(site: &Path, directory: &str, name: &str, version: &str) {
        let dist_info = site.join(directory);
        fs::create_dir(&dist_info).expect("create dist-info fixture");
        fs::write(
            dist_info.join("METADATA"),
            format!("Name: {name}\nVersion: {version}\n\n"),
        )
        .expect("write metadata fixture");
    }

    fn site_packages(temp: &TempDir) -> std::path::PathBuf {
        let site = temp.path.join("site-packages");
        fs::create_dir(&site).expect("create site-packages fixture");
        site
    }

    #[test]
    fn matching_leaf_and_absent_installation_pass() {
        assert!(guard_site_packages(None).is_ok());

        let temp = TempDir::new();
        let site = site_packages(&temp);
        metadata(&site, "solstone-1.2.3.dist-info", SOLSTONE, "1.2.3");
        metadata(&site, "solstone_journal-1.2.3.dist-info", JOURNAL, "1.2.3");
        assert!(guard_site_packages(Some(&site)).is_ok());
    }

    #[test]
    fn matching_cpu_leaf_allows_a_mismatched_cuda_leaf() {
        let temp = TempDir::new();
        let site = site_packages(&temp);
        metadata(&site, "solstone-1.2.3.dist-info", SOLSTONE, "1.2.3");
        metadata(&site, "solstone_journal-1.2.3.dist-info", JOURNAL, "1.2.3");
        metadata(
            &site,
            "solstone_journal_cuda-1.2.2.dist-info",
            JOURNAL_CUDA,
            "1.2.2",
        );
        assert!(guard_site_packages(Some(&site)).is_ok());
    }

    #[test]
    fn mismatched_leaf_prefers_cuda_in_the_error() {
        let temp = TempDir::new();
        let site = site_packages(&temp);
        metadata(&site, "solstone-1.2.3.dist-info", SOLSTONE, "1.2.3");
        metadata(&site, "solstone_journal-1.2.2.dist-info", JOURNAL, "1.2.2");
        metadata(
            &site,
            "solstone_journal_cuda-1.2.1.dist-info",
            JOURNAL_CUDA,
            "1.2.1",
        );
        let error = guard_site_packages(Some(&site)).expect_err("mismatch must fail");
        assert!(error.to_string().contains(JOURNAL_CUDA));
    }

    #[test]
    fn retired_shim_without_a_leaf_fails() {
        let temp = TempDir::new();
        let site = site_packages(&temp);
        metadata(&site, "solstone-1.2.3.dist-info", SOLSTONE, "1.2.3");
        metadata(
            &site,
            "solstone_journal_host-1.2.3.dist-info",
            JOURNAL_HOST,
            "1.2.3",
        );
        let error = guard_site_packages(Some(&site)).expect_err("retired shim must fail");
        assert!(error.to_string().contains("have moved"));
    }

    #[test]
    fn duplicate_or_malformed_target_metadata_fails() {
        let temp = TempDir::new();
        let site = site_packages(&temp);
        metadata(&site, "solstone-1.2.3.dist-info", SOLSTONE, "1.2.3");
        metadata(&site, "solstone_1.2.4.dist-info", SOLSTONE, "1.2.4");
        assert!(
            guard_site_packages(Some(&site))
                .expect_err("conflicting duplicate must fail")
                .to_string()
                .contains("conflicting")
        );

        fs::remove_dir_all(&site).expect("remove duplicate fixture");
        fs::create_dir(&site).expect("recreate site-packages fixture");
        let malformed = site.join("solstone_journal-1.2.3.dist-info");
        fs::create_dir(&malformed).expect("create malformed dist-info fixture");
        fs::write(malformed.join("METADATA"), "Name: solstone-journal\n\n")
            .expect("write malformed metadata fixture");
        assert!(
            guard_site_packages(Some(&site))
                .expect_err("malformed target must fail")
                .to_string()
                .contains("missing Version")
        );
    }
}
