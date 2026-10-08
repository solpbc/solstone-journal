// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::apple;
#[cfg(test)]
use crate::apple::ArchiveMemberSigner;
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
    VerifyInstalled,
    Notarize,
    Checksums,
    Manifest,
    Revalidate,
    Rename,
}

impl PromoteStep {
    pub const ALL: [Self; 12] = [
        Self::Compile,
        Self::Stage,
        Self::Sign,
        Self::Tar,
        Self::Deb,
        Self::Rpm,
        Self::VerifyInstalled,
        Self::Notarize,
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
        let macos_only = |step: &Self| matches!(step, Self::Sign | Self::Notarize);
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
            OS_WINDOWS => Err("windows archive/signing is not implemented on this platform"),
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
            Self::VerifyInstalled => "verify-installed",
            Self::Notarize => "notarize",
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
                "windows archive/signing is not implemented on this platform",
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
                "windows archive/signing is not implemented on this platform",
            ));
        }
        other => return Err(PromoteError::new(format!("unexpected os {other}"))),
    };

    let partial = request.work.join("out.partial");
    let _ = fs::remove_dir_all(&partial);
    fs::create_dir_all(&partial).map_err(|error| PromoteError::new(error.to_string()))?;

    // macOS signs the staged tree BEFORE the tarball is written, because the
    // `.tar.gz` Journal.app embeds must carry the signed bytes and notarization
    // registers tickets for exactly those bytes. Signing after the fact would
    // leave the tarball's copies unsigned and identical-looking.
    let mut signing = match request.os.as_str() {
        OS_MACOS => Some(sign_macos_tree(request, &stage)?),
        OS_LINUX => None,
        OS_WINDOWS => {
            return Err(PromoteError::new(
                "windows archive/signing is not implemented on this platform",
            ));
        }
        other => return Err(PromoteError::new(format!("unexpected os {other}"))),
    };
    checkpoint(request, PromoteStep::Sign)?;
    render_installed_manifest(request, &stage)?;

    let tar_name = format!("{}.tar.gz", request.basename);
    crate::tar::write_tar_gz(&stage, &partial.join(&tar_name))
        .map_err(|error| PromoteError::new(error.to_string()))?;
    checkpoint(request, PromoteStep::Tar)?;

    if request.os == OS_LINUX {
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
    verify_installed_containers(request, &partial, &tar_name)?;
    checkpoint(request, PromoteStep::VerifyInstalled)?;
    if request.os == OS_MACOS
        && let Some(signing) = signing.as_mut()
    {
        notarize_macos_tree(request, &stage, signing)?;
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
                "    {{\"path\": {:?}, \"kind\": {:?}, \"sha256\": {:?}, \"authority\": {:?}, \"team_identifier\": {:?}, \"hardened_runtime\": {}, \"trusted_timestamp\": {}, \"library_validation_disabled\": {}}}{comma}\n",
                member.relative,
                if member.payload { "payload" } else { "executable" },
                member.sha256,
                member.authority,
                member.team_identifier,
                member.hardened_runtime,
                member.trusted_timestamp,
                member.library_validation_disabled,
            ));
        }
        out.push_str("  ],\n");
        out.push_str(&format!("  \"payload_count\": {},\n", self.payload_count()));
        out.push_str(&format!(
            "  \"executable_count\": {},\n",
            self.executable_count()
        ));
        match &self.notarization {
            Some(receipt) => out.push_str(&format!(
                "  \"notarization\": {{\"submission_id\": {:?}, \"status\": {:?}}}\n",
                receipt.submission_id, receipt.status
            )),
            None => out.push_str("  \"notarization\": null\n"),
        }
        out.push_str("}\n");
        out
    }
}

fn sign_macos_tree(request: &PromoteRequest, stage: &Path) -> Result<MacosSigning, PromoteError> {
    #[cfg(test)]
    if fake_macos_sign_enabled() {
        return fake_sign_macos_tree(stage);
    }
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
        notarization: None,
    })
}

/// Notarize the signed tree and keep Apple's receipt. `Accepted` is asserted,
/// never assumed from the submission returning.
fn notarize_macos_tree(
    request: &PromoteRequest,
    stage: &Path,
    signing: &mut MacosSigning,
) -> Result<(), PromoteError> {
    #[cfg(test)]
    if fake_macos_sign_enabled() {
        signing.notarization = Some(apple::NotarizationReceipt {
            submission_id: "fake-submission".to_owned(),
            status: "Accepted".to_owned(),
        });
        return checkpoint(request, PromoteStep::Notarize);
    }
    let apple_config = request.apple.as_ref().ok_or_else(|| {
        PromoteError::new("missing required:\n  [apple] signing contract for a macos target")
    })?;
    let receipt = apple::notarize_tree(stage, &request.work.join("notarize"), apple_config)
        .map_err(|error| PromoteError::new(error.to_string()))?;
    signing.notarization = Some(receipt);
    checkpoint(request, PromoteStep::Notarize)
}

fn render_installed_manifest(request: &PromoteRequest, stage: &Path) -> Result<(), PromoteError> {
    let target = installed_target(request)?;
    let bytes = solstone_core_installed_payload::render_installed_payload(
        stage,
        solstone_core_installed_payload::PRODUCT,
        &request.version,
        target,
        &request.expected.commit,
    )
    .map_err(|error| PromoteError::new(error.to_string()))?;
    crate::stage::write_staged_file(
        stage,
        solstone_core_installed_payload::INSTALLED_PAYLOAD_MANIFEST,
        &bytes,
    )
    .map_err(|error| PromoteError::new(error.to_string()))?;
    Ok(())
}

fn verify_installed_containers(
    request: &PromoteRequest,
    partial: &Path,
    tar_name: &str,
) -> Result<(), PromoteError> {
    let target = installed_target(request)?;
    let mut containers = Vec::new();
    let tar_bytes =
        fs::read(partial.join(tar_name)).map_err(|error| PromoteError::new(error.to_string()))?;
    containers.push((
        "tar",
        crate::tar::tar_members(&tar_bytes)
            .map_err(|error| PromoteError::new(error.to_string()))?,
    ));
    if request.os == OS_LINUX {
        let [_tar, deb_name, rpm_name] = artifact_archives(&request.basename);
        containers.push((
            "deb",
            crate::deb::deb_members(&partial.join(deb_name))
                .map_err(|error| PromoteError::new(error.to_string()))?,
        ));
        containers.push((
            "rpm",
            crate::rpm::rpm_members(&partial.join(rpm_name))
                .map_err(|error| PromoteError::new(error.to_string()))?,
        ));
    }
    for (name, members) in containers {
        let root = request.work.join("installed-verify").join(name);
        let _ = fs::remove_dir_all(&root);
        materialize_members(&root, &members)?;
        solstone_core_installed_payload::verify_installed_package(&root, &request.version, target)
            .map_err(|error| PromoteError::new(error.to_string()))?;
    }
    Ok(())
}

fn installed_target(request: &PromoteRequest) -> Result<&'static str, PromoteError> {
    solstone_core_installed_payload::canonical_target(&request.arch).ok_or_else(|| {
        PromoteError::new(format!("unknown installed-payload target {}", request.arch))
    })
}

fn materialize_members(
    root: &Path,
    members: &[crate::tar::MemberBytes],
) -> Result<(), PromoteError> {
    for member in members {
        crate::archive::refuse_escape(&member.path)
            .map_err(|error| PromoteError::new(error.as_str()))?;
        let path = root.join(&member.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| PromoteError::new(error.to_string()))?;
        }
        fs::write(&path, &member.bytes).map_err(|error| PromoteError::new(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    static FAKE_MACOS_SIGN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) struct FakeMacosSignGuard;

#[cfg(test)]
impl Drop for FakeMacosSignGuard {
    fn drop(&mut self) {
        FAKE_MACOS_SIGN.with(|cell| cell.set(false));
    }
}

#[cfg(test)]
pub(crate) fn install_fake_macos_sign() -> FakeMacosSignGuard {
    FAKE_MACOS_SIGN.with(|cell| cell.set(true));
    FakeMacosSignGuard
}

#[cfg(test)]
fn fake_macos_sign_enabled() -> bool {
    FAKE_MACOS_SIGN.with(|cell| cell.get())
}

#[cfg(test)]
fn fake_sign_macos_tree(stage: &Path) -> Result<MacosSigning, PromoteError> {
    let signer = apple::FakeArchiveMemberSigner::new("promote");
    let mut members = Vec::new();
    match apple::discover_macho_members(stage) {
        Ok(found) if !found.is_empty() => {
            for member in found {
                let mut signed = signer
                    .sign_executable(&member.path, &member.relative)
                    .map_err(|error| PromoteError::new(error.to_string()))?;
                signed.payload = member.payload;
                members.push(signed);
            }
        }
        _ => {
            for record in crate::stage::staged_records(stage)
                .map_err(|error| PromoteError::new(error.to_string()))?
            {
                let path = stage.join(&record.dest);
                let bytes =
                    fs::read(&path).map_err(|error| PromoteError::new(error.to_string()))?;
                if !crate::macho::looks_like_macho(&bytes) {
                    continue;
                }
                let mut signed = signer
                    .sign_executable(&path, &record.dest)
                    .map_err(|error| PromoteError::new(error.to_string()))?;
                signed.payload = record.dest.starts_with("lib/");
                members.push(signed);
            }
        }
    }
    if members.iter().all(|member| !member.payload) {
        return Err(PromoteError::new(
            "missing required:\n  a signed loaded payload in the macos tree\n  a binaries-only signing census is exactly the gap this step exists to close",
        ));
    }
    Ok(MacosSigning {
        members,
        notarization: None,
    })
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
            std::path::Path::new("/home/builder/out/linux-x86_64"),
            "Invalid cross-device link (os error 18)",
        );
        let message = error.to_string();
        assert!(
            message
                .contains("/var/tmp/solstone-distribution-work/linux-x86_64/promote/out.partial")
        );
        assert!(message.contains("/home/builder/out/linux-x86_64"));
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
                .contains("windows archive/signing is not implemented on this platform"),
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
            error.contains("windows archive/signing is not implemented on this platform"),
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

    fn promotion_root(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("solstone-installed-{label}-"))
            .tempdir_in("/var/tmp")
            .expect("promotion root")
    }

    fn linux_tree() -> Vec<(String, Vec<u8>, u32)> {
        vec![
            ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
            (
                "lib/solstone-demo/model.bin".into(),
                b"model-bytes".to_vec(),
                0o644,
            ),
            ("share/LICENSE".into(), b"license-text".to_vec(), 0o644),
        ]
    }

    fn linux_request(
        root: &std::path::Path,
        target: &str,
        deb_arch: &str,
        rpm_arch: &str,
    ) -> super::PromoteRequest {
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let arch = target.strip_prefix("linux-").expect("linux target id");
        super::PromoteRequest {
            dest: root.join("dest"),
            work: root.join("work"),
            tree: linux_tree(),
            version: version.to_owned(),
            basename: format!("solstone-journal-{version}-linux-{arch}"),
            os: "linux".into(),
            arch: target.to_owned(),
            deb_arch: deb_arch.into(),
            rpm_arch: rpm_arch.into(),
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
        }
    }

    fn manifest_bytes(members: &[crate::tar::MemberBytes]) -> Vec<u8> {
        members
            .iter()
            .find(|member| {
                member.path == solstone_core_installed_payload::INSTALLED_PAYLOAD_MANIFEST
            })
            .expect("installed payload manifest")
            .bytes
            .clone()
    }

    fn assert_rendered_identity(bytes: &[u8], target: &str) {
        let value: serde_json::Value = serde_json::from_slice(bytes).expect("manifest json");
        assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(value["target"], target);
        assert_eq!(value["source_commit"], "aaa");
        assert_eq!(
            value["schema"],
            solstone_core_installed_payload::INSTALLED_PAYLOAD_SCHEMA
        );
        let files = value["files"].as_array().expect("files");
        assert!(files.iter().all(|file| {
            file["path"] != solstone_core_installed_payload::INSTALLED_PAYLOAD_MANIFEST
                && file["path"] != solstone_core_installed_payload::INSTALLED_PAYLOAD_SIGNATURE
        }));
        assert!(files.windows(2).all(|pair| {
            pair[0]["path"].as_str().expect("path") < pair[1]["path"].as_str().expect("path")
        }));
    }

    #[test]
    fn render_installed_manifest_bytes_match_across_linux_containers() {
        for (target, deb_arch, rpm_arch, constant) in [
            (
                "linux-x86_64",
                "amd64",
                "x86_64",
                solstone_core_installed_payload::TARGET_LINUX_X86_64,
            ),
            (
                "linux-aarch64",
                "arm64",
                "aarch64",
                solstone_core_installed_payload::TARGET_LINUX_AARCH64,
            ),
        ] {
            let root = promotion_root(target);
            let request = linux_request(root.path(), target, deb_arch, rpm_arch);
            super::promote(&request).expect(target);
            let tar =
                fs::read(request.dest.join(format!("{}.tar.gz", request.basename))).expect("tar");
            let tar_manifest = manifest_bytes(&crate::tar::tar_members(&tar).expect("tar members"));
            let deb_manifest = manifest_bytes(
                &crate::deb::deb_members(&request.dest.join(format!("{}.deb", request.basename)))
                    .expect("deb members"),
            );
            let rpm_manifest = manifest_bytes(
                &crate::rpm::rpm_members(&request.dest.join(format!("{}.rpm", request.basename)))
                    .expect("rpm members"),
            );
            assert_eq!(tar_manifest, deb_manifest, "{target}");
            assert_eq!(tar_manifest, rpm_manifest, "{target}");
            assert_rendered_identity(&tar_manifest, constant);
        }
    }

    #[test]
    fn self_check_seams_fail_promote_and_check_installed_names_the_share_flip() {
        use crate::container_seam::{ContainerSeam, ContainerSeamKind};

        let cases = [
            (
                "lib/solstone-demo/model.bin",
                ContainerSeamKind::Drop,
                "member-missing",
            ),
            (
                "lib/solstone-demo/model.bin",
                ContainerSeamKind::Grow,
                "member-changed",
            ),
            (
                "lib/solstone-demo/model.bin",
                ContainerSeamKind::Flip,
                "member-changed",
            ),
            ("share/LICENSE", ContainerSeamKind::Flip, "member-changed"),
        ];
        for (index, (path, kind, needle)) in cases.into_iter().enumerate() {
            let root = promotion_root(&format!("seam-{index}"));
            let request = linux_request(root.path(), "linux-x86_64", "amd64", "x86_64");
            fs::create_dir_all(&request.dest).expect("dest");
            fs::write(request.dest.join("marker"), b"prior").expect("marker");
            let before = super::snapshot_dir(&request.dest).expect("before");
            let _guard = crate::container_seam::install(ContainerSeam { path, kind });
            let error = super::promote(&request).expect_err(path);
            assert!(
                error.to_string().contains(needle) && error.to_string().contains(path),
                "{error}"
            );
            assert_eq!(
                super::snapshot_dir(&request.dest).expect("after"),
                before,
                "{path}"
            );
            if path == "share/LICENSE" {
                let tar_name = format!("{}.tar.gz", request.basename);
                let tar = fs::read(request.work.join("out.partial").join(&tar_name))
                    .expect("partial tar");
                let members = crate::tar::tar_members(&tar).expect("partial members");
                let subject = root.path().join("subject");
                super::materialize_members(&subject, &members).expect("materialize");
                let subject_before = super::snapshot_dir(&subject).expect("subject before");
                let refusal = crate::installed_check::check_installed(
                    &subject,
                    env!("CARGO_PKG_VERSION"),
                    solstone_core_installed_payload::TARGET_LINUX_X86_64,
                )
                .expect_err("check-installed");
                assert_eq!(
                    crate::installed_check::refusal_line(&refusal),
                    "member-changed share/LICENSE\n"
                );
                assert_eq!(
                    super::snapshot_dir(&subject).expect("subject after"),
                    subject_before
                );
            }
        }
    }

    #[test]
    fn macos_signer_seam_records_the_post_sign_digest() {
        use crate::archive_contract::{DeliveryContract, PrebuildInputIdentity};
        use crate::macho::{FixtureSpec, MH_DYLIB, fixture};
        use crate::provenance::Provenance;

        let root = promotion_root("macos-sign");
        let stage = root.path().join("chain-stage");
        let executable = fixture(&FixtureSpec::default());
        let dylib = fixture(&FixtureSpec {
            filetype: MH_DYLIB,
            install_name: Some("@rpath/libdemo.dylib"),
            ..FixtureSpec::default()
        });
        crate::stage::write_staged_file_mode(&stage, "bin/solstone", &executable, 0o755)
            .expect("stage executable");
        crate::stage::write_staged_file_mode(
            &stage,
            "lib/solstone-runtime/libdemo.dylib",
            &dylib,
            0o644,
        )
        .expect("stage dylib");
        let prebuild = PrebuildInputIdentity {
            target_id: "macos-arm64".into(),
            commit: "aaa".into(),
            lock_sha256: "bbb".into(),
            inventory_sha256: "ab".repeat(32),
            slots: Vec::new(),
        };
        let delivery = DeliveryContract {
            target_id: prebuild.target_id.clone(),
            prebuild_input_sha256: prebuild.digest(),
            slots: Vec::new(),
        };
        crate::archive_contract::stage_chain(&stage, &prebuild, &delivery, "aaa", "bbb")
            .expect("stage chain");
        let mut tree = Vec::new();
        for record in crate::stage::staged_records(&stage).expect("staged records") {
            let bytes = fs::read(stage.join(&record.dest)).expect("staged bytes");
            tree.push((record.dest, bytes, record.mode));
        }
        let version = env!("CARGO_PKG_VERSION");
        let basename = format!("solstone-journal-{version}-macos-arm64");
        let _guard = super::install_fake_macos_sign();
        let request = super::PromoteRequest {
            dest: root.path().join("dest"),
            work: root.path().join("work"),
            tree,
            version: version.to_owned(),
            basename: basename.clone(),
            os: "macos".into(),
            arch: "macos-arm64".into(),
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
        };
        super::promote(&request).expect("macos promote");
        let tar = fs::read(request.dest.join(format!("{basename}.tar.gz"))).expect("tar");
        let members = crate::tar::tar_members(&tar).expect("tar members");
        let manifest = manifest_bytes(&members);
        assert_rendered_identity(
            &manifest,
            solstone_core_installed_payload::TARGET_MACOS_ARM64,
        );
        let shipped = members
            .iter()
            .find(|member| member.path == "lib/solstone-runtime/libdemo.dylib")
            .expect("dylib");
        let value: serde_json::Value = serde_json::from_slice(&manifest).expect("json");
        let listed = value["files"]
            .as_array()
            .expect("files")
            .iter()
            .find(|file| file["path"] == "lib/solstone-runtime/libdemo.dylib")
            .expect("listed dylib");
        assert_eq!(
            listed["sha256"].as_str().expect("sha"),
            crate::digest::sha256_hex(&shipped.bytes)
        );
        assert_ne!(shipped.bytes, dylib);
        assert!(
            shipped
                .bytes
                .windows(b"SOLSTONE-FAKE-ARCHIVE-SIGNATURE:promote:".len())
                .any(|window| window == b"SOLSTONE-FAKE-ARCHIVE-SIGNATURE:promote:")
        );
    }
}
