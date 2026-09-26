// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep the warm release cache to the generation the last produce used.
//!
//! The distribution target directory persists across produces so third-party
//! crates stay warm. Cargo never removes an artifact whose unit hash has been
//! superseded, and every release bumps the workspace version, which rehashes
//! every workspace crate and everything built on one, the patched
//! `ffmpeg-sys-next` build tree among them. Left alone, each produce leaves
//! one more full generation behind.
//!
//! After a successful produce, the unit graph cargo reported across all of the
//! run's lanes, fresh units included, is the cache. An entry in `deps/`,
//! `build/` or `.fingerprint/` whose hash no lane reported is removed. So a
//! re-produce of the same tree is warm in both lanes, and a crate that has
//! left the graph, such as a removed workspace member, does not linger.
//! Superseded here is a statement about cargo's graph, never about file
//! access times, which a reused artifact does not update.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const UNIT_DIRS: [&str; 3] = ["deps", "build", ".fingerprint"];
const HASH_LEN: usize = 16;

#[derive(Debug)]
pub struct Generations {
    target_dir: PathBuf,
    hashes: BTreeSet<String>,
    /// Units whose current hash could not be recovered; never pruned.
    pinned: BTreeSet<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub entries: usize,
    pub bytes: u64,
}

impl Generations {
    pub fn new(target_dir: &Path) -> Self {
        Self {
            target_dir: target_dir.to_path_buf(),
            hashes: BTreeSet::new(),
            pinned: BTreeSet::new(),
        }
    }

    /// Record every unit one lane's `cargo build --message-format=json`
    /// reported. Cargo reports fresh units as well as rebuilt ones.
    pub fn record(&mut self, cargo_json: &str) {
        #[derive(serde::Deserialize)]
        struct Message {
            reason: Option<String>,
            target: Option<Target>,
            filenames: Option<Vec<PathBuf>>,
            executable: Option<PathBuf>,
            out_dir: Option<PathBuf>,
        }
        #[derive(serde::Deserialize)]
        struct Target {
            name: String,
        }
        for line in cargo_json.lines() {
            let Ok(message) = serde_json::from_str::<Message>(line) else {
                continue;
            };
            match message.reason.as_deref() {
                Some("compiler-artifact") => {
                    for path in message.filenames.iter().flatten() {
                        self.record_path(path);
                    }
                    if let (Some(executable), Some(target)) = (message.executable, message.target) {
                        self.record_binary(&executable, &target.name);
                    }
                }
                Some("build-script-executed") => {
                    if let Some(out_dir) = message.out_dir {
                        self.record_path(&out_dir);
                    }
                }
                _ => {}
            }
        }
    }

    /// A path under `<layout>/deps/` or `<layout>/build/` names its unit's
    /// generation in the component that follows.
    fn record_path(&mut self, path: &Path) {
        let Ok(relative) = path.strip_prefix(&self.target_dir) else {
            return;
        };
        let components: Vec<_> = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect();
        for pair in components.windows(2) {
            if (pair[0] == "deps" || pair[0] == "build")
                && let Some((_, hash)) = generation(&pair[1])
            {
                self.hashes.insert(hash.to_owned());
                return;
            }
        }
    }

    /// Cargo reports a binary only at its uplifted path, which is a hard link
    /// to the hashed copy in `deps/`. Without that link the binary's current
    /// generation is unknown, so none of its generations are pruned.
    fn record_binary(&mut self, executable: &Path, name: &str) {
        let name = unit_name(name);
        match linked_generation(executable, &name) {
            Some(hash) => {
                self.hashes.insert(hash);
            }
            None => {
                self.pinned.insert(name);
            }
        }
    }

    /// Remove what the run's graph no longer holds under each layout
    /// directory, such as `<target>/release` and `<target>/<triple>/release`.
    /// A run that recorded no unit at all removes nothing.
    pub fn prune(&self, layouts: &[PathBuf]) -> io::Result<Pruned> {
        let mut pruned = Pruned::default();
        if self.hashes.is_empty() {
            return Ok(pruned);
        }
        for layout in layouts {
            for unit_dir in UNIT_DIRS {
                let dir = layout.join(unit_dir);
                let entries = match fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                for entry in entries {
                    let entry = entry?;
                    let file_name = entry.file_name();
                    let Some((names, hash)) = file_name.to_str().and_then(generation) else {
                        continue;
                    };
                    if self.hashes.contains(hash)
                        || names.iter().any(|name| self.pinned.contains(name))
                    {
                        continue;
                    }
                    pruned.bytes += remove_entry(&entry.path())?;
                    pruned.entries += 1;
                }
            }
        }
        Ok(pruned)
    }
}

/// `<name>-<16 hex>[.<extension>]` → the unit names the entry can belong to,
/// and its hash. `deps/` prefixes library files with `lib`, so both readings
/// are offered; they matter only for a pinned binary, and the hash alone
/// decides what the run used.
fn generation(entry: &str) -> Option<(Vec<String>, &str)> {
    let stem = entry.split('.').next()?;
    let (name, hash) = stem.rsplit_once('-')?;
    if name.is_empty()
        || hash.len() != HASH_LEN
        || !hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut names = vec![unit_name(name)];
    if let Some(bare) = name.strip_prefix("lib").filter(|bare| !bare.is_empty()) {
        names.push(unit_name(bare));
    }
    Some((names, hash))
}

/// `deps/` spells a unit with underscores; `build/` and `.fingerprint/` use
/// the package's hyphens.
fn unit_name(name: &str) -> String {
    name.replace('-', "_")
}

#[cfg(unix)]
fn linked_generation(executable: &Path, name: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let linked = fs::metadata(executable).ok()?;
    let deps = executable.parent()?.join("deps");
    for entry in fs::read_dir(deps).ok()? {
        let entry = entry.ok()?;
        let file_name = entry.file_name();
        let Some((names, hash)) = file_name.to_str().and_then(generation) else {
            continue;
        };
        if names.first().is_some_and(|candidate| candidate == name)
            && let Ok(metadata) = entry.metadata()
            && metadata.dev() == linked.dev()
            && metadata.ino() == linked.ino()
        {
            return Some(hash.to_owned());
        }
    }
    None
}

#[cfg(not(unix))]
fn linked_generation(_executable: &Path, _name: &str) -> Option<String> {
    None
}

fn remove_entry(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        fs::remove_file(path)?;
        return Ok(metadata.len());
    }
    let bytes = tree_bytes(path)?;
    fs::remove_dir_all(path)?;
    Ok(bytes)
}

fn tree_bytes(dir: &Path) -> io::Result<u64> {
    let mut bytes = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        bytes += if metadata.is_dir() {
            tree_bytes(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIPLE: &str = "x86_64-unknown-linux-gnu";

    /// `cargo 1.97.1 build --release --target <triple> --message-format=json`
    /// over a workspace with a proc-macro, a library with a build script and a
    /// binary, captured on a fresh rebuild and re-rooted at `{T}`.
    const CAPTURED: &str = include_str!("../../fixtures/cargo-build-two-layouts.jsonl");

    /// The current generation (1.0.1) beside the one it superseded (1.0.0),
    /// the file layout cargo left after both builds.
    const CURRENT: &[&str] = &[
        "release/build/my-lb-af719d058e49ecbc/build-script-build",
        "release/deps/libmy_pm-c28d059dd0952f2a.so",
        "release/deps/my_pm-c28d059dd0952f2a.d",
        "release/.fingerprint/my-lb-af719d058e49ecbc/build",
        "release/.fingerprint/my-pm-c28d059dd0952f2a/lib-my_pm",
        "x86_64-unknown-linux-gnu/release/build/my-lb-b14daca33bdd2e03/out/generated",
        "x86_64-unknown-linux-gnu/release/deps/libmy_lb-f71d97512fc889c8.rlib",
        "x86_64-unknown-linux-gnu/release/deps/libmy_lb-f71d97512fc889c8.rmeta",
        "x86_64-unknown-linux-gnu/release/deps/my_lb-f71d97512fc889c8.d",
        "x86_64-unknown-linux-gnu/release/deps/my_app-e0b1c60d01d9d59a.d",
        "x86_64-unknown-linux-gnu/release/.fingerprint/my-app-e0b1c60d01d9d59a/bin-my-app",
        "x86_64-unknown-linux-gnu/release/.fingerprint/my-lb-b14daca33bdd2e03/run-build-script",
        "x86_64-unknown-linux-gnu/release/.fingerprint/my-lb-f71d97512fc889c8/lib-my_lb",
    ];
    const SUPERSEDED: &[&str] = &[
        "release/build/my-lb-71ef8e4039236a1b/build-script-build",
        "release/deps/libmy_pm-92c4517236c21b8c.so",
        "release/deps/my_pm-92c4517236c21b8c.d",
        "release/.fingerprint/my-lb-71ef8e4039236a1b/build",
        "release/.fingerprint/my-pm-92c4517236c21b8c/lib-my_pm",
        "x86_64-unknown-linux-gnu/release/build/my-lb-6c6ccf36eb944c2d/out/generated",
        "x86_64-unknown-linux-gnu/release/deps/libmy_lb-c3f241db5fdcfaaa.rlib",
        "x86_64-unknown-linux-gnu/release/deps/libmy_lb-c3f241db5fdcfaaa.rmeta",
        "x86_64-unknown-linux-gnu/release/deps/my_lb-c3f241db5fdcfaaa.d",
        "x86_64-unknown-linux-gnu/release/deps/my_app-6fb0cba689dc6604",
        "x86_64-unknown-linux-gnu/release/deps/my_app-6fb0cba689dc6604.d",
        "x86_64-unknown-linux-gnu/release/.fingerprint/my-app-6fb0cba689dc6604/bin-my-app",
        "x86_64-unknown-linux-gnu/release/.fingerprint/my-lb-6c6ccf36eb944c2d/run-build-script",
        "x86_64-unknown-linux-gnu/release/.fingerprint/my-lb-c3f241db5fdcfaaa/lib-my_lb",
    ];
    /// A crate that has left the graph, in several generations.
    const DEPARTED: &[&str] = &[
        "release/deps/libother-0123456789abcdef.rlib",
        "release/deps/libother-fedcba9876543210.rlib",
        "x86_64-unknown-linux-gnu/release/build/other-0123456789abcdef/out/generated",
    ];

    fn write(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, relative).unwrap();
    }

    fn cache(link_binary: bool) -> (tempfile::TempDir, Generations, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for relative in CURRENT.iter().chain(SUPERSEDED).chain(DEPARTED) {
            write(root, relative);
        }
        let bin = root
            .join(TRIPLE)
            .join("release/deps/my_app-e0b1c60d01d9d59a");
        let uplifted = root.join(TRIPLE).join("release/my-app");
        fs::write(&bin, "current").unwrap();
        if link_binary {
            fs::hard_link(&bin, &uplifted).unwrap();
        } else {
            fs::copy(&bin, &uplifted).unwrap();
        }
        let mut generations = Generations::new(root);
        generations.record(&CAPTURED.replace("{T}", &root.display().to_string()));
        let layouts = vec![root.join("release"), root.join(TRIPLE).join("release")];
        (dir, generations, layouts)
    }

    fn present(root: &Path, relative: &str) -> bool {
        root.join(relative).exists()
    }

    #[cfg(unix)]
    #[test]
    fn prune_keeps_exactly_the_units_the_run_reported() {
        let (dir, generations, layouts) = cache(true);
        let pruned = generations.prune(&layouts).unwrap();
        let root = dir.path();
        for relative in CURRENT {
            assert!(present(root, relative), "kept {relative}");
        }
        assert!(present(
            root,
            &format!("{TRIPLE}/release/deps/my_app-e0b1c60d01d9d59a")
        ));
        for relative in SUPERSEDED.iter().chain(DEPARTED) {
            assert!(!present(root, relative), "removed {relative}");
        }
        assert_eq!(pruned.entries, SUPERSEDED.len() + DEPARTED.len());
        assert!(pruned.bytes > 0);
        assert_eq!(generations.prune(&layouts).unwrap(), Pruned::default());
    }

    #[test]
    fn a_binary_without_its_hard_link_keeps_every_generation() {
        let (dir, generations, layouts) = cache(false);
        generations.prune(&layouts).unwrap();
        let root = dir.path();
        assert!(present(
            root,
            &format!("{TRIPLE}/release/deps/my_app-e0b1c60d01d9d59a")
        ));
        assert!(present(
            root,
            &format!("{TRIPLE}/release/deps/my_app-6fb0cba689dc6604")
        ));
        assert!(present(
            root,
            &format!("{TRIPLE}/release/.fingerprint/my-app-6fb0cba689dc6604/bin-my-app")
        ));
        assert!(!present(
            root,
            &format!("{TRIPLE}/release/deps/libmy_lb-c3f241db5fdcfaaa.rlib")
        ));
    }

    #[test]
    fn a_run_that_recorded_nothing_removes_nothing() {
        let (dir, _, layouts) = cache(true);
        let empty = Generations::new(dir.path());
        assert_eq!(empty.prune(&layouts).unwrap(), Pruned::default());
        for relative in CURRENT.iter().chain(SUPERSEDED).chain(DEPARTED) {
            assert!(present(dir.path(), relative), "kept {relative}");
        }
    }

    #[test]
    fn generation_reads_every_spelling_cargo_uses() {
        assert_eq!(
            generation("libmy_lb-f71d97512fc889c8.rlib"),
            Some((
                vec!["libmy_lb".to_owned(), "my_lb".to_owned()],
                "f71d97512fc889c8"
            ))
        );
        assert_eq!(
            generation("ffmpeg-sys-next-a800e6902c240986"),
            Some((vec!["ffmpeg_sys_next".to_owned()], "a800e6902c240986"))
        );
        assert_eq!(
            generation("liblibc-0123456789abcdef.rlib").map(|(names, _)| names),
            Some(vec!["liblibc".to_owned(), "libc".to_owned()])
        );
        assert_eq!(generation("my-app"), None);
        assert_eq!(generation("my_app-0123456789ABCDEF"), None);
        assert_eq!(generation("rmeta0123456789abcdef"), None);
    }
}
