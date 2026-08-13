// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct ContractPaths {
    pub(crate) root: PathBuf,
    pub(crate) solstone: PathBuf,
    pub(crate) layout: PathBuf,
    pub(crate) artifact: PathBuf,
    pub(crate) fixture: PathBuf,
}

impl ContractPaths {
    pub(crate) fn resolve(override_root: Option<&Path>) -> Result<Self, String> {
        if let Some(root) = override_root {
            return Self::from_root(root.to_path_buf());
        }
        if let Some(root) = env::var_os("SOLSTONE_CONTRACT_ROOT").filter(|root| !root.is_empty()) {
            return Self::from_root(PathBuf::from(root));
        }
        let executable = env::current_exe()
            .map_err(|error| format!("contract: could not inspect current executable: {error}"))?;
        Self::resolve_from_executable(&executable)
    }

    fn resolve_from_executable(executable: &Path) -> Result<Self, String> {
        let Some(executable_dir) = executable.parent() else {
            return Err(format!(
                "contract: could not locate installed solstone package or source checkout from {}",
                executable.display()
            ));
        };
        if let Some(site_packages) = installed_site_packages_from_executable_dir(executable_dir) {
            return Self::from_root(site_packages);
        }
        for candidate in executable_dir.ancestors() {
            if is_solstone_checkout_root(candidate) {
                return Self::from_root(candidate.to_path_buf());
            }
        }
        Err(format!(
            "contract: could not locate installed solstone package or source checkout from {}",
            executable.display()
        ))
    }

    pub(crate) fn from_root(root: PathBuf) -> Result<Self, String> {
        let solstone = root.join("solstone");
        if !solstone.is_dir() {
            return Err(format!(
                "contract: solstone package tree not found: {}",
                solstone.display()
            ));
        }
        Ok(Self {
            layout: solstone.join("think/contract/layout.json"),
            artifact: solstone.join("talent/journal/contract/bundle.json"),
            fixture: root.join("tests/fixtures/journal"),
            root,
            solstone,
        })
    }
}

fn installed_site_packages_from_executable_dir(executable_dir: &Path) -> Option<PathBuf> {
    let prefix = executable_dir.parent()?;
    let entries = fs::read_dir(prefix).ok()?;
    let mut candidates = Vec::new();
    for lib_entry in entries.flatten() {
        let lib_path = lib_entry.path();
        if !lib_path.is_dir()
            || !lib_path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with("lib"))
        {
            continue;
        }
        let Ok(python_entries) = fs::read_dir(&lib_path) else {
            continue;
        };
        for python_entry in python_entries.flatten() {
            let python_path = python_entry.path();
            if !python_path.is_dir()
                || !python_path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| name.starts_with("python"))
            {
                continue;
            }
            for name in ["site-packages", "dist-packages"] {
                let package_dir = python_path.join(name);
                if package_dir.join("solstone/__init__.py").is_file() {
                    candidates.push(package_dir);
                }
            }
        }
    }
    let mut canonical = candidates
        .into_iter()
        .filter_map(|candidate| fs::canonicalize(candidate).ok())
        .collect::<Vec<_>>();
    canonical.sort();
    canonical.dedup();
    canonical.into_iter().next()
}

fn is_solstone_checkout_root(candidate: &Path) -> bool {
    candidate.join("pyproject.toml").is_file()
        && candidate.join(".git").exists()
        && candidate.join("solstone").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn installed_site_packages_is_the_contract_root() {
        let temp = TempDir::new().unwrap();
        let bin = temp.path().join("bin");
        let package = temp.path().join("lib/python3.12/site-packages");
        fs::create_dir_all(package.join("solstone")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(package.join("solstone/__init__.py"), "").unwrap();
        let found = installed_site_packages_from_executable_dir(&bin).unwrap();
        let paths = ContractPaths::from_root(found).unwrap();
        assert_eq!(
            paths.solstone,
            fs::canonicalize(package.join("solstone")).unwrap()
        );
    }
}
