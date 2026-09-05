// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::apple;
use crate::archive_contract;
use crate::deb::{DebMeta, write_deb};
use crate::inspect::{ArchiveChainDigests, ReleaseInfo, write_sidecars};
use crate::inventory::{Apple, OS_LINUX, OS_MACOS, OS_WINDOWS, artifact_archives};
use crate::provenance::{Provenance, require_clean, require_commit, require_lock};
use crate::rpm::{RpmMeta, write_rpm};
use crate::stage::write_staged_file_mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoteStep {
    Compile,
    Stage,
    Sign,
    Tar,
    Deb,
    Rpm,
    Pkg,
    Notarize,
    Staple,
    Checksums,
    Manifest,
    Revalidate,
    Rename,
}

impl PromoteStep {
    pub const ALL: [Self; 13] = [
        Self::Compile,
        Self::Stage,
        Self::Sign,
        Self::Tar,
        Self::Deb,
        Self::Rpm,
        Self::Pkg,
        Self::Notarize,
        Self::Staple,
        Self::Checksums,
        Self::Manifest,
        Self::Revalidate,
        Self::Rename,
    ];

    /// The steps a run for `os` actually reaches, in order.
    ///
    /// ⚠ The atomicity proof injects a failure after each step and asserts the
    /// destination is untouched. Handing it a step this platform never executes
    /// makes the injection a no-op, the promotion succeed, and the assertion
    /// fail — which is the honest outcome, but the useful one is a per-os list
    /// so every step that DOES run is still covered on both platforms.
    pub fn for_os(os: &str) -> Result<Vec<Self>, &'static str> {
        let macos_only =
            |step: &Self| matches!(step, Self::Sign | Self::Pkg | Self::Notarize | Self::Staple);
        let linux_only = |step: &Self| matches!(step, Self::Deb | Self::Rpm);
        match os {
            OS_MACOS => Ok(Self::ALL
                .into_iter()
                .filter(|step| !linux_only(step))
                .collect()),
            OS_LINUX => Ok(Self::ALL
                .into_iter()
                .filter(|step| !macos_only(step))
                .collect()),
            OS_WINDOWS => Err("windows archive/signing is not implemented in this lode"),
            other => panic!("unexpected distribution os {other}"),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compile => "compile",
            Self::Stage => "stage",
            Self::Sign => "sign",
            Self::Tar => "tar",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Pkg => "pkg",
            Self::Notarize => "notarize",
            Self::Staple => "staple",
            Self::Checksums => "checksums",
            Self::Manifest => "manifest",
            Self::Revalidate => "revalidate",
            Self::Rename => "rename",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromoteRequest {
    pub dest: PathBuf,
    pub work: PathBuf,
    pub tree: Vec<(String, Vec<u8>, u32)>,
    pub version: String,
    pub basename: String,
    pub os: String,
    pub arch: String,
    pub deb_arch: String,
    pub rpm_arch: String,
    pub dirty: bool,
    pub observed: Provenance,
    pub expected: Provenance,
    pub fail_after: Option<String>,
    /// Present for a macOS target. `None` on Linux.
    ///
    /// ⛔ There is deliberately no "produce it unsigned" escape. An unsigned
    /// macOS tree is not a producible artifact — its binaries cannot start
    /// under Gatekeeper — and a flag that emitted one would be the single
    /// easiest way for a proof harness to go green over nothing. A macOS run
    /// without credentials fails closed with a named missing-credential
    /// refusal, which is a blocker to raise rather than a mode to select.
    pub apple: Option<Apple>,
}

#[derive(Debug)]
pub struct PromoteError {
    pub message: String,
}

impl PromoteError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PromoteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PromoteError {}

fn fail_after(request: &PromoteRequest) -> Option<String> {
    request
        .fail_after
        .clone()
        .or_else(|| env::var("SOLSTONE_DISTRIBUTION_FAIL_AFTER").ok())
}

fn checkpoint(request: &PromoteRequest, step: PromoteStep) -> Result<(), PromoteError> {
    if fail_after(request).as_deref() == Some(step.as_str()) {
        return Err(PromoteError::new(format!(
            "injected-failure {}",
            step.as_str()
        )));
    }
    Ok(())
}

#[must_use]
pub fn isolated_target_dir(work: &Path) -> PathBuf {
    work.join("distribution-target")
}

pub fn promote(request: &PromoteRequest) -> Result<PathBuf, PromoteError> {
    require_clean(request.dirty).map_err(|error| PromoteError::new(error.to_string()))?;
    match request.os.as_str() {
        OS_LINUX => {}
        OS_MACOS => {}
        OS_WINDOWS => {
            return Err(PromoteError::new(
                "windows archive/signing is not implemented in this lode",
            ));
        }
        other => {
            return Err(PromoteError::new(format!("unexpected os {other}")));
        }
    }
    checkpoint(request, PromoteStep::Compile)?;

    let stage = request.work.join("stage");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).map_err(|error| PromoteError::new(error.to_string()))?;
    for (dest, bytes, mode) in &request.tree {
        write_staged_file_mode(&stage, dest, bytes, *mode)
            .map_err(|error| PromoteError::new(error.to_string()))?;
    }
    checkpoint(request, PromoteStep::Stage)?;
    let archive_chain = match request.os.as_str() {
        OS_MACOS => Some(
            archive_contract::validate_staged_chain(
                &stage,
                &request.arch,
                &request.expected.commit,
                &request.expected.lock_sha256,
            )
            .map_err(|error| PromoteError::new(error.to_string()))?,
        ),
        OS_LINUX => None,
        OS_WINDOWS => {
            return Err(PromoteError::new(
                "windows archive/signing is not implemented in this lode",
            ));
        }
        other => return Err(PromoteError::new(format!("unexpected os {other}"))),
    };

    let partial = request.work.join("out.partial");
    let _ = fs::remove_dir_all(&partial);
    fs::create_dir_all(&partial).map_err(|error| PromoteError::new(error.to_string()))?;

    // macOS signs the staged tree BEFORE any container is written, because both
    // containers must carry the signed bytes: the `.pkg` so notarization
    // registers tickets for what it encloses, and the `.tar.gz` so a bootstrap
    // install lands binaries Gatekeeper will admit. Signing a container after
    // the fact would leave the tarball's copies unsigned and identical-looking.
    let mut signing = match request.os.as_str() {
        OS_MACOS => Some(sign_macos_tree(request, &stage)?),
        OS_LINUX => None,
        OS_WINDOWS => {
            return Err(PromoteError::new(
                "windows archive/signing is not implemented in this lode",
            ));
        }
        other => return Err(PromoteError::new(format!("unexpected os {other}"))),
    };
    checkpoint(request, PromoteStep::Sign)?;

    let tar_name = format!("{}.tar.gz", request.basename);
    crate::tar::write_tar_gz(&stage, &partial.join(&tar_name))
        .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Tar)?;

    match request.os.as_str() {
        OS_MACOS => {
            if let Some(signing) = signing.as_mut() {
                write_pkg(request, &stage, &partial, signing)?;
            }
        }
        OS_LINUX => {
            let [_tar, deb_name, rpm_name] = artifact_archives(&request.basename);
            write_deb(
                &stage,
                &partial.join(deb_name),
                DebMeta {
                    version: &request.version,
                    arch: &request.deb_arch,
                },
            )
            .map_err(|error| PromoteError::new(error.to_string()))?;
            checkpoint(request, PromoteStep::Deb)?;
            write_rpm(
                &stage,
                &partial.join(rpm_name),
                RpmMeta {
                    version: &request.version,
                    arch: &request.rpm_arch,
                },
            )
            .map_err(|error| PromoteError::new(error.to_string()))?;
            checkpoint(request, PromoteStep::Rpm)?;
        }
        OS_WINDOWS => {
            return Err(PromoteError::new(
                "windows archive/signing is not implemented in this lode",
            ));
        }
        other => return Err(PromoteError::new(format!("unexpected os {other}"))),
    }

    let release = ReleaseInfo {
        product: "solstone-journal",
        version: &request.version,
        target: &request.arch,
        commit: &request.expected.commit,
        lock_sha256: &request.expected.lock_sha256,
        archive_chain: archive_chain.as_ref().map(|chain| ArchiveChainDigests {
            prebuild_input_sha256: &chain.prebuild_input_sha256,
            delivery_contract_sha256: &chain.delivery_contract_sha256,
            final_invocation_sha256: &chain.final_invocation_sha256,
        }),
    };
    if let Some(signing) = &signing {
        fs::write(
            partial.join(format!("{}.signing.json", request.basename)),
            signing.render(),
        )
        .map_err(|error| PromoteError::new(error.to_string()))?;
    }
    write_sidecars(&partial, &request.os, &release, &request.basename)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Checksums)?;
    checkpoint(request, PromoteStep::Manifest)?;

    require_commit(&request.expected.commit, &request.observed.commit)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    require_lock(&request.expected.lock_sha256, &request.observed.lock_sha256)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Revalidate)?;

    checkpoint(request, PromoteStep::Rename)?;
    if let Some(parent) = request.dest.parent() {
        fs::create_dir_all(parent).map_err(|error| PromoteError::new(error.to_string()))?;
    }
    if request.dest.exists() {
        let displaced = request.work.join("dest.displaced");
        let _ = fs::remove_dir_all(&displaced);
        rename_or_copy(&request.dest, &displaced)?;
    }
    rename_or_copy(&partial, &request.dest)?;
    Ok(request.dest.clone())
}

fn rename_error(src: &Path, dest: &Path, error: impl std::fmt::Display) -> PromoteError {
    PromoteError::new(format!(
        "could not move {} to {}: {error}. Set SOLSTONE_DISTRIBUTION_WORK to a directory on the same filesystem as the output.",
        src.display(),
        dest.display()
    ))
}

fn rename_or_copy(src: &Path, dest: &Path) -> Result<(), PromoteError> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_recursively(src, dest).map_err(|copy_error| {
                let _ = fs::remove_dir_all(dest);
                let _ = fs::remove_file(dest);
                rename_error(src, dest, format!("cross-device copy failed: {copy_error}"))
            })?;
            if src.is_dir() {
                fs::remove_dir_all(src).map_err(|error| rename_error(src, dest, error))?;
            } else {
                fs::remove_file(src).map_err(|error| rename_error(src, dest, error))?;
            }
            Ok(())
        }
        Err(error) => Err(rename_error(src, dest, error)),
    }
}

fn copy_recursively(src: &Path, dest: &Path) -> io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name();
            copy_recursively(&entry.path(), &dest.join(name))?;
        }
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dest)?;
    Ok(())
}

/// What the producer signed, and what Apple said about it. Written beside the
/// containers as `<basename>.signing.json` — the macOS half of provenance,
/// replacing the record `scripts/record_macos_native_wheel.py` used to emit.
#[derive(Debug, Clone)]
pub struct MacosSigning {
    pub members: Vec<apple::SignedMember>,
    pub pkg: Option<String>,
    pub notarization: Option<apple::NotarizationReceipt>,
}

impl MacosSigning {
    #[must_use]
    pub fn payload_count(&self) -> usize {
        self.members.iter().filter(|member| member.payload).count()
    }

    #[must_use]
    pub fn executable_count(&self) -> usize {
        self.members.iter().filter(|member| !member.payload).count()
    }

    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from("{\n  \"members\": [\n");
        for (index, member) in self.members.iter().enumerate() {
            let comma = if index + 1 == self.members.len() {
                ""
            } else {
                ","
            };
            out.push_str(&format!(
                "    {{\"path\": {:?}, \"kind\": {:?}, \"sha256\": {:?}, \"authority\": {:?}, \"team_identifier\": {:?}, \"hardened_runtime\": {}, \"trusted_timestamp\": {}}}{comma}\n",
                member.relative,
                if member.payload { "payload" } else { "executable" },
                member.sha256,
                member.authority,
                member.team_identifier,
                member.hardened_runtime,
                member.trusted_timestamp,
            ));
        }
        out.push_str("  ],\n");
        out.push_str(&format!("  \"payload_count\": {},\n", self.payload_count()));
        out.push_str(&format!(
            "  \"executable_count\": {},\n",
            self.executable_count()
        ));
        match &self.pkg {
            Some(pkg) => out.push_str(&format!("  \"pkg\": {pkg:?},\n")),
            None => out.push_str("  \"pkg\": null,\n"),
        }
        match &self.notarization {
            Some(receipt) => out.push_str(&format!(
                "  \"notarization\": {{\"submission_id\": {:?}, \"status\": {:?}, \"stapled\": {}}}\n",
                receipt.submission_id, receipt.status, receipt.stapled
            )),
            None => out.push_str("  \"notarization\": null\n"),
        }
        out.push_str("}\n");
        out
    }
}

fn sign_macos_tree(request: &PromoteRequest, stage: &Path) -> Result<MacosSigning, PromoteError> {
    let apple_config = request.apple.as_ref().ok_or_else(|| {
        PromoteError::new("missing required:\n  [apple] signing contract for a macos target")
    })?;
    apple::require_credentials(apple_config)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    apple::require_tool_pins(apple_config).map_err(|error| PromoteError::new(error.to_string()))?;
    let members = apple::sign_tree(stage, apple_config)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    if members.iter().all(|member| !member.payload) {
        return Err(PromoteError::new(
            "missing required:\n  a signed loaded payload in the macos tree\n  a binaries-only signing census is exactly the gap this step exists to close",
        ));
    }
    Ok(MacosSigning {
        members,
        pkg: None,
        notarization: None,
    })
}

/// Build the installer package, notarize it, staple the ticket, and read the
/// ticket back. Every one of those four is asserted; none is assumed from the
/// previous one succeeding.
fn write_pkg(
    request: &PromoteRequest,
    stage: &Path,
    partial: &Path,
    signing: &mut MacosSigning,
) -> Result<(), PromoteError> {
    let apple_config = request.apple.as_ref().ok_or_else(|| {
        PromoteError::new("missing required:\n  [apple] signing contract for a macos target")
    })?;
    let pkg_name = format!("{}.pkg", request.basename);
    let pkg_path = partial.join(&pkg_name);
    apple::build_pkg(stage, &pkg_path, &request.version, apple_config)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    signing.pkg = Some(pkg_name);
    checkpoint(request, PromoteStep::Pkg)?;

    let mut receipt = apple::notarize(&pkg_path, apple_config)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Notarize)?;

    apple::staple(&pkg_path).map_err(|error| PromoteError::new(error.to_string()))?;
    receipt.stapled = true;
    checkpoint(request, PromoteStep::Staple)?;

    // The assessment Gatekeeper itself performs, over the finished container.
    apple::assess(&pkg_path, "install").map_err(|error| PromoteError::new(error.to_string()))?;
    signing.notarization = Some(receipt);
    Ok(())
}

pub fn snapshot_dir(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, PromoteError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let mut files = BTreeMap::new();
    collect(path, path, &mut files)?;
    Ok(files)
}

fn collect(
    root: &Path,
    dir: &Path,
    files: &mut std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), PromoteError> {
    for entry in fs::read_dir(dir).map_err(|error| PromoteError::new(error.to_string()))? {
        let entry = entry.map_err(|error| PromoteError::new(error.to_string()))?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| PromoteError::new(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        files.insert(
            relative,
            fs::read(&path).map_err(|error| PromoteError::new(error.to_string()))?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{copy_recursively, rename_error, rename_or_copy};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "solstone-promote-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("scratch");
        root
    }

    #[test]
    fn rename_error_names_both_paths_and_the_work_variable() {
        let error = rename_error(
            std::path::Path::new(
                "/var/tmp/solstone-distribution-work/linux-x86_64/promote/out.partial",
            ),
            std::path::Path::new("/home/jer/out/linux-x86_64"),
            "Invalid cross-device link (os error 18)",
        );
        let message = error.to_string();
        assert!(
            message
                .contains("/var/tmp/solstone-distribution-work/linux-x86_64/promote/out.partial")
        );
        assert!(message.contains("/home/jer/out/linux-x86_64"));
        assert!(message.contains("SOLSTONE_DISTRIBUTION_WORK"));
    }

    #[test]
    fn copy_recursively_preserves_nested_files() {
        let root = scratch("copy");
        let src = root.join("src");
        fs::create_dir_all(src.join("nested")).expect("src");
        fs::write(src.join("nested/file.txt"), "payload").expect("write");
        let dest = root.join("dest");
        copy_recursively(&src, &dest).expect("copy");
        assert_eq!(
            fs::read_to_string(dest.join("nested/file.txt")).expect("read"),
            "payload"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn promote_writes_unsigned_six_file_set_without_a_minisig() {
        use crate::inventory;
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let basename = format!("solstone-journal-{version}-linux-x86_64");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dest = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-sign-env-dest-{}-{nanos}",
            std::process::id()
        ));
        let work = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-sign-env-work-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        promote(&PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![("bin/solstone-core".into(), b"core".to_vec(), 0o755)],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: "linux-x86_64".into(),
            deb_arch: "amd64".into(),
            rpm_arch: "x86_64".into(),
            dirty: false,
            observed: Provenance {
                commit: "aaa".into(),
                lock_sha256: "bbb".into(),
            },
            expected: Provenance {
                commit: "aaa".into(),
                lock_sha256: "bbb".into(),
            },
            fail_after: None,
            apple: None,
        })
        .expect("promote");
        let found = fs::read_dir(&dest)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let mut expected = inventory::artifact_set(&basename).to_vec();
        expected.sort();
        let mut found_sorted = found.clone();
        found_sorted.sort();
        assert_eq!(found_sorted, expected);
        assert!(!found.iter().any(|name| name.ends_with(".minisig")));
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn promote_refuses_windows_archives() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let dest = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-windows-refuse-dest-{}",
            std::process::id()
        ));
        let work = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-windows-refuse-work-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&dest).expect("dest");
        fs::write(dest.join("marker"), b"prior").expect("marker");
        let error = promote(&PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![(
                "runtime/test-fixture-bin.exe".into(),
                b"core".to_vec(),
                0o755,
            )],
            version: "1.0.22".into(),
            basename: "solstone-journal-1.0.22-windows-x86_64".into(),
            os: "windows".into(),
            arch: "windows-x86_64".into(),
            deb_arch: String::new(),
            rpm_arch: String::new(),
            dirty: false,
            observed: Provenance {
                commit: "aaa".into(),
                lock_sha256: "bbb".into(),
            },
            expected: Provenance {
                commit: "aaa".into(),
                lock_sha256: "bbb".into(),
            },
            fail_after: None,
            apple: None,
        })
        .expect_err("windows promote refuses");
        assert!(
            error
                .to_string()
                .contains("windows archive/signing is not implemented in this lode"),
            "{error}"
        );
        assert_eq!(
            fs::read(dest.join("marker")).expect("marker remains"),
            b"prior"
        );
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn promote_step_for_os_refuses_windows() {
        let error = super::PromoteStep::for_os("windows").unwrap_err();
        assert!(
            error.contains("windows archive/signing is not implemented in this lode"),
            "{error}"
        );
        assert!(
            !super::PromoteStep::for_os("linux")
                .expect("linux")
                .is_empty()
        );
        assert!(
            !super::PromoteStep::for_os("macos")
                .expect("macos")
                .is_empty()
        );
    }

    #[test]
    fn rename_or_copy_moves_a_directory_on_the_same_device() {
        let root = scratch("same-device");
        let src = root.join("src");
        fs::create_dir_all(&src).expect("src");
        fs::write(src.join("marker"), "ok").expect("write");
        let dest = root.join("dest");
        rename_or_copy(&src, &dest).expect("rename");
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(dest.join("marker")).expect("read"), "ok");
        let _ = fs::remove_dir_all(root);
    }
}
