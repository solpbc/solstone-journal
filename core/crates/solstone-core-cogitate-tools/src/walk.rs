// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::denylist::{Classification, classify};
use crate::paths::contained_path;

#[derive(Clone, Debug)]
pub(crate) struct WalkEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

pub(crate) fn journal_rel(path: &Path, root_real: &Path) -> Option<String> {
    path.strip_prefix(root_real)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn walk_allowed(
    root: &Path,
    root_real: &Path,
    start: &Path,
    include_hidden: bool,
) -> AllowedWalk {
    AllowedWalk {
        root: root.to_path_buf(),
        root_real: root_real.to_path_buf(),
        include_hidden,
        pending_directories: vec![start.to_path_buf()],
        queued: VecDeque::new(),
        seen: HashSet::new(),
        #[cfg(test)]
        directory_reads: 0,
    }
}

pub(crate) struct AllowedWalk {
    root: PathBuf,
    root_real: PathBuf,
    include_hidden: bool,
    pending_directories: Vec<PathBuf>,
    queued: VecDeque<WalkEntry>,
    seen: HashSet<PathBuf>,
    #[cfg(test)]
    directory_reads: usize,
}

impl Iterator for AllowedWalk {
    type Item = WalkEntry;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(entry) = self.queued.pop_front() {
                return Some(entry);
            }
            let directory = self.pending_directories.pop()?;
            self.read_directory(&directory);
        }
    }
}

impl AllowedWalk {
    fn read_directory(&mut self, directory: &Path) {
        #[cfg(test)]
        {
            self.directory_reads += 1;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        let mut recurse = Vec::new();
        let mut files = Vec::new();
        for entry in entries {
            let source = entry.path();
            let is_dir = fs::metadata(&source)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false);
            let Some(resolved) =
                resolve_entry(&self.root, &self.root_real, &source, self.include_hidden)
            else {
                continue;
            };
            if is_dir {
                if self.seen.insert(resolved.clone()) {
                    self.queued.push_back(WalkEntry {
                        path: resolved,
                        is_dir: true,
                    });
                    if !source.is_symlink() {
                        recurse.push(source);
                    }
                }
            } else if self.seen.insert(resolved.clone()) {
                files.push(WalkEntry {
                    path: resolved,
                    is_dir: false,
                });
            }
        }
        self.queued.extend(files);
        self.pending_directories.extend(recurse.into_iter().rev());
    }
}

pub(crate) fn iter_allowed_children(
    root: &Path,
    root_real: &Path,
    start: &Path,
    include_hidden: bool,
) -> Vec<WalkEntry> {
    let Ok(entries) = fs::read_dir(start) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for entry in entries {
        let source = entry.path();
        let Some(resolved) = resolve_entry(root, root_real, &source, include_hidden) else {
            continue;
        };
        if !seen.insert(resolved.clone()) {
            continue;
        }
        let Ok(metadata) = fs::metadata(&resolved) else {
            continue;
        };
        output.push(WalkEntry {
            path: resolved,
            is_dir: metadata.is_dir(),
        });
    }
    output
}

fn resolve_entry(
    root: &Path,
    root_real: &Path,
    source: &Path,
    include_hidden: bool,
) -> Option<PathBuf> {
    if !include_hidden
        && source
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    {
        return None;
    }
    let rel = source
        .strip_prefix(root_real)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    let resolved = contained_path(root, &rel).ok()?;
    (classify(&resolved, root_real) == Classification::Allowed).then_some(resolved)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("cogitate-tools-walk-{name}-"))
            .tempdir()
            .expect("tempdir")
    }

    #[test]
    fn dangling_symlink_is_a_file_walk_entry() {
        let root = root("dangling");
        fs::create_dir_all(root.path().join("allowed")).expect("allowed");
        symlink("missing.txt", root.path().join("allowed/dangling")).expect("link");
        let root_real = crate::paths::journal_root_real(root.path()).expect("real root");
        let paths: Vec<_> =
            walk_allowed(root.path(), &root_real, &root_real.join("allowed"), false)
                .map(|entry| journal_rel(&entry.path, &root_real).expect("relative"))
                .collect();
        assert_eq!(paths, ["allowed/missing.txt"]);
    }

    #[test]
    fn recursive_walk_is_lazy() {
        let root = root("lazy");
        for directory in 0..30 {
            for file in 0..30 {
                let path = root.path().join(format!("d{directory:02}/f{file:02}.txt"));
                fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
                fs::write(path, "x").expect("file");
            }
        }
        let root_real = crate::paths::journal_root_real(root.path()).expect("real root");
        let mut walk = walk_allowed(root.path(), &root_real, &root_real, false);
        assert_eq!(walk.directory_reads, 0);
        assert!(walk.next().is_some());
        assert_eq!(walk.directory_reads, 1);
        assert!(walk.seen.len() <= 30);
    }
}
