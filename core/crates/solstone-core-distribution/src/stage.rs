// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn write_staged_file(root: &Path, dest: &str, contents: &[u8]) -> io::Result<PathBuf> {
    let path = root.join(dest);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, contents)?;
    Ok(path)
}

pub fn staged_files(root: &Path) -> io::Result<Vec<String>> {
    let mut dests = Vec::new();
    collect(root, root, &mut dests)?;
    dests.sort();
    dests.dedup();
    Ok(dests)
}

fn collect(root: &Path, dir: &Path, dests: &mut Vec<String>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, dests)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| io::Error::other(error.to_string()))?;
        dests.push(relative.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}
