// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A package around a checkout's own build, for running that build where an
//! installed journal would run.
//!
//! Backup, confidential processing and sound tagging run only members of the
//! installed package beside the executable (`lib/solstone-restic/restic`, the
//! nvattest tree, the CED runtime), verified against the installed-payload
//! manifest. A checkout build has no package, so those features refuse there.
//! This writes `<out>/bin` (the build's own executables, under their build
//! names, beside the `solstone` and `journal` launchers that exec them), every pinned member the host target ships, staged and verified
//! exactly as `produce` stages them, and the manifest over the result. The
//! executables still find the checkout's talent and app roots by walking up.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use solstone_core_installed_payload::{
    COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, PRODUCT, compiled_target,
    render_installed_payload,
};

use crate::inventory::{Entry, load_inventory};
use crate::produce::{ProduceError, stage_pinned_entry};

const INVENTORY: &str = "core/distribution/inventory.toml";

pub fn usage() -> &'static str {
    "usage: solstone-distribution dev-package <out> --bin-dir <dir> --runtime-root <dir> --source-commit <sha>"
}

#[derive(Debug, PartialEq, Eq)]
pub struct DevPackageArgs {
    pub out: PathBuf,
    pub bin_dir: PathBuf,
    /// Where the build staged its runtime directories (the Cargo target dir):
    /// the ONNX Runtime the VAD and speaker helpers load is read from
    /// `<runtime_root>/<dest_dir>`, the same relative path the package uses.
    pub runtime_root: PathBuf,
    pub source_commit: String,
}

pub fn parse(args: &[String]) -> Result<DevPackageArgs, ProduceError> {
    let usage = || ProduceError::new(usage());
    let (out, rest) = args.split_first().ok_or_else(usage)?;
    let (mut bin_dir, mut runtime_root, mut source_commit) = (None, None, None);
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or_else(usage)?;
        match flag.as_str() {
            "--bin-dir" => bin_dir = Some(PathBuf::from(value)),
            "--runtime-root" => runtime_root = Some(PathBuf::from(value)),
            "--source-commit" => source_commit = Some(value.clone()),
            _ => return Err(usage()),
        }
    }
    Ok(DevPackageArgs {
        out: PathBuf::from(out),
        bin_dir: bin_dir.ok_or_else(usage)?,
        runtime_root: runtime_root.ok_or_else(usage)?,
        source_commit: source_commit.ok_or_else(usage)?,
    })
}

/// Build the package. The caller has already fetched the host target's pinned
/// inputs into the catalog cache (`acquire catalog-inputs`).
pub fn run(repo: &Path, args: &DevPackageArgs) -> Result<(), ProduceError> {
    let inventory = load_inventory(&repo.join(INVENTORY))
        .map_err(|error| ProduceError::new(error.to_string()))?;
    build(repo, &inventory.entry, compiled_target(), args)
}

pub(crate) fn build(
    repo: &Path,
    entries: &[Entry],
    target_id: &str,
    args: &DevPackageArgs,
) -> Result<(), ProduceError> {
    let out = &args.out;
    if out.parent().is_none() || out.as_os_str().is_empty() {
        return Err(ProduceError::new(
            "dev-package refuses an empty or root output",
        ));
    }
    match fs::remove_dir_all(out) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    fs::create_dir_all(out.join("bin"))?;
    let catalog_cache = crate::pinned_stage::catalog_input_cache_dir(repo);
    let mut placed = 0;
    for entry in entries {
        match entry {
            Entry::Bin { bin, targets, .. } if targets.iter().any(|t| t == target_id) => {
                let built = args.bin_dir.join(bin);
                if built.is_file() {
                    link_or_copy(&built, &out.join("bin").join(bin))?;
                    placed += 1;
                }
            }
            Entry::Launcher {
                source,
                dest,
                mode,
                targets,
                ..
            } if targets.iter().any(|t| t == target_id) => {
                let bytes = fs::read(repo.join(source))?;
                crate::stage::write_staged_file_mode(out, dest, &bytes, *mode)?;
            }
            Entry::OnnxRuntime {
                dest_dir, targets, ..
            } if targets.iter().any(|t| t == target_id) => {
                copy_runtime_dir(&args.runtime_root.join(dest_dir), &out.join(dest_dir))?;
            }
            Entry::PinnedNative { .. } | Entry::PinnedMembers { .. } => {
                stage_pinned_entry(entry, repo, &catalog_cache, target_id, out)?;
            }
            _ => {}
        }
    }
    if placed == 0 {
        return Err(ProduceError::new(format!(
            "dev-package found no built executable in {}",
            args.bin_dir.display()
        )));
    }
    let manifest = render_installed_payload(
        out,
        PRODUCT,
        COMPILED_VERSION,
        target_id,
        &args.source_commit,
    )
    .map_err(|refusal| ProduceError::new(format!("dev-package manifest: {}", refusal.code)))?;
    let path = out.join(INSTALLED_PAYLOAD_MANIFEST);
    fs::create_dir_all(path.parent().expect("manifest has a parent"))?;
    fs::write(path, manifest)?;
    Ok(())
}

fn copy_runtime_dir(from: &Path, to: &Path) -> Result<(), ProduceError> {
    let entries = fs::read_dir(from).map_err(|error| {
        ProduceError::new(format!(
            "dev-package needs the staged runtime {} ({error}); run make build-sandbox-processing",
            from.display()
        ))
    })?;
    fs::create_dir_all(to)?;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn link_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    if fs::hard_link(from, to).is_err() {
        fs::copy(from, to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_installed_payload::{InstalledPackage, TARGET_LINUX_X86_64};

    fn args(rest: &[&str]) -> Vec<String> {
        rest.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parse_requires_the_bin_dir_and_the_commit() {
        assert_eq!(
            parse(&args(&[
                "out",
                "--bin-dir",
                "b",
                "--runtime-root",
                "r",
                "--source-commit",
                "c"
            ]))
            .unwrap(),
            DevPackageArgs {
                out: "out".into(),
                bin_dir: "b".into(),
                runtime_root: "r".into(),
                source_commit: "c".into()
            }
        );
        assert!(parse(&args(&["out", "--bin-dir", "b", "--source-commit", "c"])).is_err());
        assert!(parse(&args(&["out", "--source-commit", "c", "--other", "x"])).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn the_package_admits_and_carries_the_built_executables_under_their_build_names() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("debug");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("solstone-core"), b"core").unwrap();
        fs::write(bin_dir.join("unlisted-tool"), b"not shipped").unwrap();
        let bin = |package: &str, name: &str| Entry::Bin {
            class: None,
            package: package.into(),
            bin: name.into(),
            dest: format!("bin/{name}-packaged"),
            mode: 0o755,
            lane: "native".into(),
            targets: vec![TARGET_LINUX_X86_64.into()],
        };
        fs::create_dir_all(tmp.path().join("scripts")).unwrap();
        fs::write(tmp.path().join("scripts/journal"), b"#!/bin/sh\n").unwrap();
        let entries = [
            bin("solstone-core", "solstone-core"),
            bin("solstone-core-journal-bin", "solstone-core-journal"),
            Entry::Launcher {
                class: None,
                source: "scripts/journal".into(),
                dest: "bin/journal".into(),
                mode: 0o755,
                targets: vec![TARGET_LINUX_X86_64.into()],
            },
            Entry::OnnxRuntime {
                class: None,
                dest_dir: "lib/onnx-helper".into(),
                mode: 0o755,
                identities: vec![],
                targets: vec![TARGET_LINUX_X86_64.into()],
            },
        ];
        fs::create_dir_all(tmp.path().join("target/lib/onnx-helper")).unwrap();
        fs::write(tmp.path().join("target/lib/onnx-helper/libort.so"), b"ort").unwrap();
        let out = tmp.path().join("pkg");
        fs::create_dir_all(out.join("stale")).unwrap();
        build(
            tmp.path(),
            &entries,
            TARGET_LINUX_X86_64,
            &DevPackageArgs {
                out: out.clone(),
                bin_dir,
                runtime_root: tmp.path().join("target"),
                source_commit: "0123abc".into(),
            },
        )
        .unwrap();
        assert!(!out.join("stale").exists());
        assert_eq!(fs::read(out.join("bin/solstone-core")).unwrap(), b"core");
        assert!(!out.join("bin/unlisted-tool").exists());
        assert_eq!(fs::read(out.join("bin/journal")).unwrap(), b"#!/bin/sh\n");
        assert_eq!(
            fs::read(out.join("lib/onnx-helper/libort.so")).unwrap(),
            b"ort"
        );
        let package = InstalledPackage::admit(&out, COMPILED_VERSION, TARGET_LINUX_X86_64).unwrap();
        assert_eq!(
            package.member("bin/solstone-core").unwrap(),
            out.join("bin/solstone-core")
        );
    }
}
