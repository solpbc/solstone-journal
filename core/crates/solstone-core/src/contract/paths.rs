// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
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
        let Some(root) =
            solstone_core_journal::resolve_installation_root_from_executable_dir(executable_dir)
        else {
            return Err(format!(
                "contract: could not locate installed solstone package or source checkout from {}",
                executable.display()
            ));
        };
        Self::from_root(root)
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

    pub(crate) fn fixture_journal(&self) -> Result<&Path, String> {
        if !self.fixture.is_dir() {
            return Err(format!(
                "contract: fixture journal missing: {}",
                self.fixture.display()
            ));
        }
        Ok(&self.fixture)
    }
}

pub(crate) fn package_roots_from_executable_dir(
    executable_dir: &Path,
) -> Option<(PathBuf, PathBuf)> {
    let root =
        solstone_core_journal::resolve_installation_root_from_executable_dir(executable_dir)?;
    let talent = root.join("solstone/talent");
    let apps = root.join("solstone/apps");
    (talent.is_dir() && apps.is_dir()).then_some((talent, apps))
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
        std::fs::create_dir_all(package.join("solstone")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(package.join("solstone/__init__.py"), "").unwrap();
        let found =
            solstone_core_journal::installed_site_packages_from_executable_dir(&bin).unwrap();
        let paths = ContractPaths::from_root(found).unwrap();
        assert_eq!(
            paths.solstone,
            std::fs::canonicalize(package.join("solstone")).unwrap()
        );
        assert!(paths.fixture_journal().is_err());
        assert!(
            paths
                .fixture_journal()
                .unwrap_err()
                .contains(&paths.fixture.display().to_string())
        );
    }

    #[test]
    fn share_layout_resolves_and_fails_when_anchor_removed() {
        let temp = TempDir::new().unwrap();
        let prefix = temp.path().join("tree");
        let bin = prefix.join("bin");
        let share = prefix.join("share");
        std::fs::create_dir_all(&bin).unwrap();
        for relative in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, relative).unwrap();
        }
        std::fs::create_dir_all(share.join("solstone/apps")).unwrap();
        let paths = ContractPaths::resolve_from_executable(&bin.join("solstone-core")).unwrap();
        assert_eq!(paths.root, share);
        assert!(package_roots_from_executable_dir(&bin).is_some());
        std::fs::remove_file(share.join(solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR)).unwrap();
        assert!(ContractPaths::resolve_from_executable(&bin.join("solstone-core")).is_err());
        assert!(package_roots_from_executable_dir(&bin).is_none());
    }
}
