// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only inspection of source-checkout router-skill links.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const ROUTER_SKILL_NAMES: [&str; 2] = ["solstone", "journal"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterSkillLinkState {
    Installed,
    Missing,
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterSkillLink {
    pub name: String,
    pub source: PathBuf,
    pub link: PathBuf,
    pub expected_target: String,
    pub state: RouterSkillLinkState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleRouterSkillLink {
    pub name: OsString,
    pub link: PathBuf,
}

/// Locate the two canonical router-skill source directories.
pub fn discover_project_sources(project_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut sources = Vec::new();
    for name in ROUTER_SKILL_NAMES {
        let source = project_root.join("solstone").join("talent").join(name);
        let skill_file = source.join("SKILL.md");
        if !skill_file.is_file() {
            return Err(format!(
                "expected project skill at {}",
                skill_file.display()
            ));
        }
        sources.push(source);
    }
    sources.sort();
    Ok(sources)
}

/// Return the lexical relative target used for project skill symlinks.
pub fn expected_link_target(source: &Path, link_parent: &Path) -> String {
    lexical_relpath(source, link_parent)
}

/// Inspect each expected router-skill link without modifying the project.
pub fn inspect_router_skill_links(
    project_root: &Path,
    link_parent: &Path,
) -> Result<Vec<RouterSkillLink>, String> {
    discover_project_sources(project_root).map(|sources| {
        sources
            .into_iter()
            .map(|source| {
                let name = source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned();
                let link = link_parent.join(&name);
                let expected_target = expected_link_target(&source, link_parent);
                let state = if fs::symlink_metadata(&link)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                    && fs::read_link(&link)
                        .is_ok_and(|target| target == Path::new(&expected_target))
                {
                    RouterSkillLinkState::Installed
                } else if link.exists() || fs::symlink_metadata(&link).is_ok() {
                    RouterSkillLinkState::Foreign
                } else {
                    RouterSkillLinkState::Missing
                };
                RouterSkillLink {
                    name,
                    source,
                    link,
                    expected_target,
                    state,
                }
            })
            .collect()
    })
}

/// Enumerate symlink entries that are not canonical router skills.
pub fn stale_router_skill_links(link_parent: &Path) -> Result<Vec<StaleRouterSkillLink>, String> {
    let mut entries = match fs::read_dir(link_parent) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    entries.sort();
    Ok(entries
        .into_iter()
        .filter_map(|link| {
            let name = link.file_name()?.to_os_string();
            (!ROUTER_SKILL_NAMES.iter().any(|skill| name == *skill)
                && fs::symlink_metadata(&link)
                    .ok()
                    .is_some_and(|metadata| metadata.file_type().is_symlink()))
            .then_some(StaleRouterSkillLink { name, link })
        })
        .collect())
}

fn lexical_relpath(target: &Path, base: &Path) -> String {
    let (target_root, target_parts) = lexical_parts(target);
    let (base_root, base_parts) = lexical_parts(base);
    if target_root != base_root {
        return target.to_string_lossy().to_string();
    }
    let mut common = 0;
    while common < target_parts.len()
        && common < base_parts.len()
        && target_parts[common] == base_parts[common]
    {
        common += 1;
    }
    let mut out = PathBuf::new();
    for _ in common..base_parts.len() {
        out.push("..");
    }
    for part in &target_parts[common..] {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        out.to_string_lossy().to_string()
    }
}

fn lexical_parts(path: &Path) -> (bool, Vec<OsString>) {
    let mut rooted = false;
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => parts.push(prefix.as_os_str().to_os_string()),
            Component::RootDir => rooted = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|part| part != "..") {
                    parts.pop();
                } else {
                    parts.push(OsString::from(".."));
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
        }
    }
    (rooted, parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn classifies_expected_foreign_and_stale_links() {
        let root = std::env::temp_dir().join(format!("skill-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for name in ROUTER_SKILL_NAMES {
            fs::create_dir_all(root.join("solstone/talent").join(name)).unwrap();
            fs::write(
                root.join("solstone/talent").join(name).join("SKILL.md"),
                "x",
            )
            .unwrap();
        }
        let links = root.join("project/.claude/skills");
        fs::create_dir_all(&links).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            expected_link_target(&root.join("solstone/talent/solstone"), &links),
            links.join("solstone"),
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("elsewhere", links.join("old")).unwrap();
        let rows = inspect_router_skill_links(&root, &links).unwrap();
        assert!(
            rows.iter()
                .any(|row| row.name == "solstone" && row.state == RouterSkillLinkState::Installed)
        );
        assert_eq!(stale_router_skill_links(&links).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lexical_relpath_emits_multi_parent_string() {
        let root = PathBuf::from("/tmp/solstone-root");
        let target = expected_link_target(
            &root.join("solstone/talent/journal"),
            &root.join("scratch/skills-oracle/casework/p/deep/.claude/skills"),
        );
        assert!(target.starts_with("../../.."), "{target}");
        assert!(target.ends_with("solstone/talent/journal"), "{target}");
    }
}
