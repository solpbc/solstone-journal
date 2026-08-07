// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::layout::installed_site_packages_from_executable_dir;

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

fn installed_versions(
    site_packages: &Path,
) -> Result<BTreeMap<&'static str, String>, CoherenceError> {
    let entries = fs::read_dir(site_packages).map_err(|error| {
        CoherenceError::new(format!(
            "native journal package coherence check failed: could not read {}: {error}",
            site_packages.display()
        ))
    })?;
    let mut versions = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            CoherenceError::new(format!(
                "native journal package coherence check failed: could not read {}: {error}",
                site_packages.display()
            ))
        })?;
        let path = entry.path();
        if !path.is_dir()
            || !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".dist-info"))
        {
            continue;
        }
        let directory_target = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(target_from_dist_info_directory);
        let metadata = match fs::read_to_string(path.join("METADATA")) {
            Ok(metadata) => metadata,
            Err(error) => {
                if directory_target.is_some() {
                    return Err(metadata_error(&path, error.to_string()));
                }
                continue;
            }
        };
        let (name, version) = metadata_headers(&metadata);
        let target = name
            .as_deref()
            .map(normalize_name)
            .and_then(|name| target_name(&name))
            .or(directory_target);
        let Some(target) = target else {
            continue;
        };
        let Some(name) = name else {
            return Err(metadata_error(&path, "missing Name header"));
        };
        let Some(version) = version else {
            return Err(metadata_error(&path, "missing Version header"));
        };
        if normalize_name(&name) != target {
            return Err(metadata_error(
                &path,
                "Name header does not match target package",
            ));
        }
        match versions.get(target) {
            Some(existing) if existing == &version => {}
            Some(existing) => {
                return Err(CoherenceError::new(format!(
                    "native journal package coherence check failed: conflicting installed versions for {target}: {existing} and {version}"
                )));
            }
            None => {
                versions.insert(target, version);
            }
        }
    }
    Ok(versions)
}

fn metadata_headers(metadata: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut version = None;
    for line in metadata.lines() {
        if line.is_empty() {
            break;
        }
        if name.is_none() {
            name = line.strip_prefix("Name:").map(str::trim).map(str::to_owned);
        }
        if version.is_none() {
            version = line
                .strip_prefix("Version:")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
        }
    }
    (name.filter(|value| !value.is_empty()), version)
}

fn target_from_dist_info_directory(name: &str) -> Option<&'static str> {
    let stem = name.strip_suffix(".dist-info")?;
    let normalized = normalize_name(stem);
    TARGETS
        .iter()
        .copied()
        .filter(|target| {
            normalized
                .strip_prefix(target)
                .is_some_and(|suffix| suffix.starts_with('-'))
        })
        .max_by_key(|target| target.len())
}

fn target_name(name: &str) -> Option<&'static str> {
    TARGETS.iter().copied().find(|target| *target == name)
}

fn normalize_name(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut separator = false;
    for character in value.chars() {
        if matches!(character, '-' | '_' | '.') {
            if !separator {
                normalized.push('-');
                separator = true;
            }
        } else {
            normalized.extend(character.to_lowercase());
            separator = false;
        }
    }
    normalized
}

fn metadata_error(path: &Path, reason: impl std::fmt::Display) -> CoherenceError {
    CoherenceError::new(format!(
        "native journal package coherence check failed: invalid metadata at {}: {reason}",
        path.join("METADATA").display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should be available")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-core-journal-cli-coherence-{}-{stamp}",
                std::process::id()
            ));
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
