// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only inspection of source-checkout router-skill links.
//!
//! It also compares user-skill directory trees, still read-only.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
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

/// Match an installed user copy against the complete bundled skill tree.
/// Extra empty directories, links, reparse points and special entries cannot
/// establish ownership. Missing/unreadable reference data is never a match.
pub fn user_skill_copy_matches(bundled: &Path, installed: &Path) -> std::io::Result<bool> {
    fn is_link(metadata: &fs::Metadata) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
        }
        #[cfg(not(windows))]
        {
            metadata.file_type().is_symlink()
        }
    }
    fn compare(source: &Path, target: &Path) -> std::io::Result<bool> {
        let source_meta = fs::symlink_metadata(source)?;
        let target_meta = fs::symlink_metadata(target)?;
        if is_link(&source_meta) || is_link(&target_meta) {
            return Ok(false);
        }
        if source_meta.is_file() && target_meta.is_file() {
            return Ok(
                source_meta.len() == target_meta.len() && fs::read(source)? == fs::read(target)?
            );
        }
        if !source_meta.is_dir() || !target_meta.is_dir() {
            return Ok(false);
        }
        fn names(path: &Path) -> std::io::Result<Vec<OsString>> {
            let mut names = fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<std::io::Result<Vec<_>>>()?;
            names.sort();
            Ok(names)
        }
        let source_names = names(source)?;
        if source_names != names(target)? {
            return Ok(false);
        }
        for name in source_names {
            if !compare(&source.join(&name), &target.join(&name))? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    let source_meta = fs::symlink_metadata(bundled)?;
    let target_meta = fs::symlink_metadata(installed)?;
    let skill_meta = fs::symlink_metadata(bundled.join("SKILL.md"))?;
    if !source_meta.is_dir()
        || is_link(&source_meta)
        || !skill_meta.is_file()
        || is_link(&skill_meta)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bundled skill reference requires a directory with a regular SKILL.md",
        ));
    }
    if !target_meta.is_dir() || is_link(&target_meta) {
        return Ok(false);
    }
    compare(bundled, installed)
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

#[derive(Debug, PartialEq, Eq)]
enum SkillTreeEntry {
    Directory,
    File(Vec<u8>),
}

fn collect_skill_tree(root: &Path) -> io::Result<BTreeMap<PathBuf, SkillTreeEntry>> {
    let root_meta = fs::symlink_metadata(root)?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "root is not an ordinary directory",
        ));
    }
    let mut map = BTreeMap::new();
    collect_skill_tree_inner(root, Path::new(""), &mut map)?;
    Ok(map)
}

fn collect_skill_tree_inner(
    root: &Path,
    rel: &Path,
    map: &mut BTreeMap<PathBuf, SkillTreeEntry>,
) -> io::Result<()> {
    let current_dir = if rel.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    for entry in fs::read_dir(current_dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "symlinks are not permitted in user skill trees",
            ));
        }
        let child_rel = if rel.as_os_str().is_empty() {
            PathBuf::from(entry.file_name())
        } else {
            rel.join(entry.file_name())
        };
        if file_type.is_dir() {
            map.insert(child_rel.clone(), SkillTreeEntry::Directory);
            collect_skill_tree_inner(root, &child_rel, map)?;
        } else if file_type.is_file() {
            let bytes = fs::read(&path)?;
            map.insert(child_rel, SkillTreeEntry::File(bytes));
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "special files are not permitted in user skill trees",
            ));
        }
    }
    Ok(())
}

/// Compare two ordinary user-skill directory trees without following symlinks.
///
/// Both walks must succeed without encountering symlinks or special files.
/// Those entries are errors, so a refresh or install leaves them in place.
/// Directories, including empty ones, and regular-file bytes are compared.
///
/// Uninstall ownership stays on [`user_skill_copy_matches`].
pub fn user_skill_ordinary_copy_matches(left: &Path, right: &Path) -> io::Result<bool> {
    let left_tree = collect_skill_tree(left)?;
    let right_tree = collect_skill_tree(right)?;
    Ok(left_tree == right_tree)
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

    #[test]
    fn user_skill_ordinary_copy_matches_reports_true_for_identical_trees() {
        let temp =
            std::env::temp_dir().join(format!("skill-matches-identical-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let left = temp.join("left");
        let right = temp.join("right");
        fs::create_dir_all(left.join("sub")).unwrap();
        fs::create_dir_all(right.join("sub")).unwrap();
        fs::write(left.join("SKILL.md"), b"content").unwrap();
        fs::write(right.join("SKILL.md"), b"content").unwrap();
        fs::write(left.join("sub/doc.txt"), b"doc").unwrap();
        fs::write(right.join("sub/doc.txt"), b"doc").unwrap();

        assert_eq!(
            user_skill_ordinary_copy_matches(&left, &right).unwrap(),
            true
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn user_skill_ordinary_copy_matches_reports_false_for_byte_difference() {
        let temp =
            std::env::temp_dir().join(format!("skill-matches-diff-bytes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let left = temp.join("left");
        let right = temp.join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("SKILL.md"), b"content a").unwrap();
        fs::write(right.join("SKILL.md"), b"content b").unwrap();

        assert_eq!(
            user_skill_ordinary_copy_matches(&left, &right).unwrap(),
            false
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn user_skill_ordinary_copy_matches_reports_false_for_extra_empty_directory() {
        let temp =
            std::env::temp_dir().join(format!("skill-matches-extra-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let left = temp.join("left");
        let right = temp.join("right");
        fs::create_dir_all(left.join("empty")).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("SKILL.md"), b"content").unwrap();
        fs::write(right.join("SKILL.md"), b"content").unwrap();

        assert_eq!(
            user_skill_ordinary_copy_matches(&left, &right).unwrap(),
            false
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn user_skill_ordinary_copy_matches_reports_err_for_internal_symlink() {
        let temp =
            std::env::temp_dir().join(format!("skill-matches-symlink-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let left = temp.join("left");
        let right = temp.join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("SKILL.md"), b"content").unwrap();
        fs::write(right.join("SKILL.md"), b"content").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("target", left.join("link")).unwrap();

        #[cfg(unix)]
        assert!(user_skill_ordinary_copy_matches(&left, &right).is_err());
        fs::remove_dir_all(temp).unwrap();
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod user_copy_tests {
    use super::user_skill_copy_matches;
    use std::fs;
    use std::path::Path;

    fn tree(root: &Path) {
        fs::create_dir_all(root.join("nested/empty")).unwrap();
        fs::write(root.join("SKILL.md"), "published skill\n").unwrap();
        fs::write(root.join("nested/guide.txt"), "published guide\n").unwrap();
    }

    #[test]
    fn user_copy_exact_tree_and_owner_additions() {
        for change in ["none", "bytes", "file", "empty", "kind"] {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("source");
            let target = root.path().join("target");
            tree(&source);
            tree(&target);
            match change {
                "bytes" => fs::write(target.join("SKILL.md"), "owner skill\n").unwrap(),
                "file" => fs::write(target.join("owner.txt"), "owner bytes").unwrap(),
                "empty" => fs::create_dir(target.join("owner-empty")).unwrap(),
                "kind" => {
                    fs::remove_file(target.join("nested/guide.txt")).unwrap();
                    fs::create_dir(target.join("nested/guide.txt")).unwrap();
                }
                _ => {}
            }
            assert_eq!(
                user_skill_copy_matches(&source, &target).unwrap(),
                change == "none"
            );
        }
    }

    #[test]
    fn user_copy_invalid_reference_never_proves_ownership() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(user_skill_copy_matches(&source, &target).is_err());
        fs::create_dir(source.join("SKILL.md")).unwrap();
        assert!(user_skill_copy_matches(&source, &target).is_err());
        assert!(user_skill_copy_matches(&root.path().join("missing"), &target).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn user_copy_links_never_prove_ownership() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        tree(&source);
        tree(&target);
        let external = root.path().join("external");
        fs::write(&external, "published guide\n").unwrap();
        fs::remove_file(target.join("nested/guide.txt")).unwrap();
        symlink(&external, target.join("nested/guide.txt")).unwrap();
        assert!(!user_skill_copy_matches(&source, &target).unwrap());
        fs::remove_file(target.join("nested/guide.txt")).unwrap();
        fs::remove_file(source.join("nested/guide.txt")).unwrap();
        symlink(&external, source.join("nested/guide.txt")).unwrap();
        symlink(&external, target.join("nested/guide.txt")).unwrap();
        assert!(!user_skill_copy_matches(&source, &target).unwrap());
        fs::remove_file(source.join("SKILL.md")).unwrap();
        symlink(&external, source.join("SKILL.md")).unwrap();
        assert!(user_skill_copy_matches(&source, &target).is_err());
    }
}
