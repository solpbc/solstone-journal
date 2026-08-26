// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! codesign · pkgbuild · notarytool · stapler, driven from the producer.
//!
//! This is the Python-free replacement for `scripts/sign-and-notarize-helper.sh`,
//! which read its pins through an interpreter and emitted its receipt through
//! one. `P-distribution` forbids Python in the machinery that builds the
//! product, not only in the product, so the signing step moves here with the
//! rest of the producer. The pins it used to import now live in the inventory's
//! `[apple]` table, which is the declarative surface the plate asks for.
//!
//! ⛔ What does NOT move here is Apple's own toolchain. `codesign`, `pkgbuild`,
//! `notarytool` and `stapler` are the only way to produce a signed, notarized
//! and stapled macOS artifact; they ship with the OS and carry no interpreter.
//! The plate's ban on external packaging toolchains names `dpkg-deb`,
//! `rpmbuild`, `maturin`, `setuptools` and `twine` — third-party build systems
//! we replaced. Apple's platform tools are not in that class and there is no
//! alternative to them that Gatekeeper would accept.

use std::fs;
use std::io;
#[cfg(any(test, feature = "test-hooks"))]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::inventory::Apple;
use crate::macho::{self, MachoInfo};

#[derive(Debug)]
pub struct AppleError {
    pub message: String,
}

impl AppleError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AppleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppleError {}

impl From<io::Error> for AppleError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// A signable item found in the staged tree. `payload` is true for anything
/// that is loaded rather than launched — the half of the Gatekeeper obligation
/// that a "the executables run" proof does not touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachoMember {
    pub relative: String,
    pub path: PathBuf,
    pub info: MachoInfo,
    pub payload: bool,
}

/// Walk a staged tree and return every Mach-O in it, keyed by magic bytes
/// rather than by name or extension.
///
/// ⛔ Extension-keyed discovery is the defect this exists to avoid: the tree's
/// executables have no extension at all, the ONNX runtime arrives as
/// `libonnxruntime.1.25.0.dylib` AND as `libonnxruntime.dylib`, and a future
/// payload could carry neither shape. Reading the magic makes the census a
/// property of the bytes.
pub fn discover_macho_members(stage: &Path) -> Result<Vec<MachoMember>, AppleError> {
    let mut found = Vec::new();
    walk(stage, stage, &mut found)?;
    found.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(found)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<MachoMember>) -> Result<(), AppleError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out)?;
            continue;
        }
        let bytes = fs::read(&path)?;
        if !macho::looks_like_macho(&bytes) {
            continue;
        }
        let info = macho::parse_macho(&bytes)
            .map_err(|error| AppleError::new(format!("{}: {error}", path.display())))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|error| AppleError::new(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let payload = info.filetype != macho::MH_EXECUTE;
        out.push(MachoMember {
            relative,
            path,
            info,
            payload,
        });
    }
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<String, AppleError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| AppleError::new(format!("{program}: {error}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(AppleError::new(format!(
            "{program} {} failed ({}):\n{stderr}{stdout}",
            args.join(" "),
            output
                .status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
        )));
    }
    // codesign writes its report to stderr; notarytool writes JSON to stdout.
    Ok(format!("{stdout}{stderr}"))
}

/// A credential problem is a blocker, never something to work around. Each of
/// these refusals names the exact missing thing so an alert can quote it.
pub fn require_credentials(apple: &Apple) -> Result<(), AppleError> {
    let keychain = apple.keychain_path();
    if !keychain.is_file() {
        return Err(AppleError::new(format!(
            "missing required:\n  signing keychain {}",
            keychain.display()
        )));
    }
    // ⚠ Two policies, deliberately. `-p codesigning` is the narrow question —
    // can this identity sign CODE — and the Developer ID Installer cert is not
    // a codesigning identity, so it is absent from that list on a perfectly
    // healthy keychain. Asking one list for both is a refusal that reads as a
    // missing credential and is not one.
    let signing = run(
        "security",
        &[
            "find-identity",
            "-v",
            "-p",
            "codesigning",
            &keychain.to_string_lossy(),
        ],
    )?;
    let all = run(
        "security",
        &["find-identity", "-v", &keychain.to_string_lossy()],
    )?;
    let mut missing = Vec::new();
    if !signing.contains(apple.app_identity.as_str()) {
        missing.push(format!("codesigning identity {}", apple.app_identity));
    }
    if !all.contains(apple.installer_identity.as_str()) {
        missing.push(format!("installer identity {}", apple.installer_identity));
    }
    if !missing.is_empty() {
        return Err(AppleError::new(format!(
            "missing required:\n  {}",
            missing.join("\n  ")
        )));
    }
    Ok(())
}

/// Refuse a toolchain that is not the one the release policy pins. The pins
/// moved out of `scripts/release_tool_pins.py` and into the inventory; the
/// check itself is unchanged in force.
pub fn require_tool_pins(apple: &Apple) -> Result<(), AppleError> {
    let mut unexpected = Vec::new();
    if !apple.xcode.is_empty() {
        let xcode = run("xcodebuild", &["-version"])?;
        let flattened = xcode.split_whitespace().collect::<Vec<_>>().join(" ");
        if !flattened
            .to_lowercase()
            .contains(&apple.xcode.to_lowercase())
        {
            unexpected.push(format!("xcode {flattened:?} (want {:?})", apple.xcode));
        }
    }
    if !apple.notarytool.is_empty() {
        let notarytool = run("xcrun", &["notarytool", "--version"])?;
        if !notarytool.contains(&apple.notarytool) {
            unexpected.push(format!(
                "notarytool {:?} (want {:?})",
                notarytool.trim(),
                apple.notarytool
            ));
        }
    }
    if !apple.codesign_path.is_empty() && !Path::new(&apple.codesign_path).is_file() {
        unexpected.push(format!("codesign path {}", apple.codesign_path));
    }
    if unexpected.is_empty() {
        return Ok(());
    }
    Err(AppleError::new(format!(
        "unexpected:\n  {}",
        unexpected.join("\n  ")
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedMember {
    pub relative: String,
    pub payload: bool,
    pub sha256: String,
    pub authority: String,
    pub team_identifier: String,
    pub hardened_runtime: bool,
    pub trusted_timestamp: bool,
}

pub(crate) trait ArchiveMemberSigner {
    fn sign_executable(
        &self,
        path: &Path,
        relative_member_path: &str,
    ) -> Result<SignedMember, AppleError>;
}

pub(crate) struct RealArchiveMemberSigner<'a> {
    pub apple: &'a Apple,
}

impl ArchiveMemberSigner for RealArchiveMemberSigner<'_> {
    fn sign_executable(
        &self,
        path: &Path,
        relative_member_path: &str,
    ) -> Result<SignedMember, AppleError> {
        codesign_at(path, self.apple)?;
        verify_signed(relative_member_path, path, false, self.apple)
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) struct FakeArchiveMemberSigner {
    marker: String,
    mutate_mode: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
impl FakeArchiveMemberSigner {
    pub(crate) fn new(marker: impl Into<String>) -> Self {
        Self {
            marker: marker.into(),
            mutate_mode: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_mode_mutation(marker: impl Into<String>) -> Self {
        Self {
            marker: marker.into(),
            mutate_mode: true,
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl ArchiveMemberSigner for FakeArchiveMemberSigner {
    fn sign_executable(
        &self,
        path: &Path,
        relative_member_path: &str,
    ) -> Result<SignedMember, AppleError> {
        let mut bytes = fs::read(path)?;
        bytes.extend_from_slice(b"\nSOLSTONE-FAKE-ARCHIVE-SIGNATURE:");
        bytes.extend_from_slice(self.marker.as_bytes());
        bytes.extend_from_slice(b":");
        bytes.extend_from_slice(relative_member_path.as_bytes());
        fs::write(path, &bytes)?;
        if self.mutate_mode {
            let mut permissions = fs::metadata(path)?.permissions();
            permissions.set_mode(0o644);
            fs::set_permissions(path, permissions)?;
        }
        Ok(SignedMember {
            relative: relative_member_path.to_owned(),
            payload: false,
            sha256: crate::digest::sha256_hex(&bytes),
            authority: format!("fake:{}", self.marker),
            team_identifier: "fake".to_owned(),
            hardened_runtime: true,
            trusted_timestamp: true,
        })
    }
}

/// Sign every Mach-O in the staged tree, loaded payloads first.
///
/// 🔴 Payload-first is not cosmetic. Under the hardened runtime a signed
/// executable refuses to load a dylib that is not signed by the same team, so
/// signing the executable while leaving `libonnxruntime.1.25.0.dylib` bare
/// produces a tree whose binaries all pass `codesign --verify` and whose
/// speakers helper cannot start. That is precisely the disjoint observable the
/// retired Python `warm` verb used to hold, restated as a producer step.
pub fn sign_tree(stage: &Path, apple: &Apple) -> Result<Vec<SignedMember>, AppleError> {
    let members = discover_macho_members(stage)?;
    if members.is_empty() {
        return Err(AppleError::new(
            "missing required:\n  mach-o members in staged tree",
        ));
    }
    let mut signed = Vec::new();
    for member in members
        .iter()
        .filter(|member| member.payload)
        .chain(members.iter().filter(|member| !member.payload))
    {
        codesign_at(&member.path, apple)?;
        signed.push(verify_signed(
            &member.relative,
            &member.path,
            member.payload,
            apple,
        )?);
    }
    Ok(signed)
}

fn codesign_at(path: &Path, apple: &Apple) -> Result<(), AppleError> {
    let keychain = apple.keychain_path().to_string_lossy().into_owned();
    let codesign = if apple.codesign_path.is_empty() {
        "codesign".to_owned()
    } else {
        apple.codesign_path.clone()
    };
    let display = path.to_string_lossy().into_owned();
    run(
        &codesign,
        &[
            "--force",
            "--options",
            "runtime",
            "--timestamp",
            "--keychain",
            &keychain,
            "--sign",
            &apple.app_identity,
            &display,
        ],
    )?;
    Ok(())
}

/// Assert the properties we wanted, never merely the absence of an error.
///
/// ⚠ `codesign --verify` answers "is this signature valid", and an ad-hoc
/// linker signature is a valid signature — which is how an unsigned binary once
/// reported success twice over on this very pipeline. Every field below is read
/// out of `codesign -dv` and compared, and `--verify` is kept only as the
/// structural leg.
pub fn verify_signed(
    relative: &str,
    path: &Path,
    payload: bool,
    apple: &Apple,
) -> Result<SignedMember, AppleError> {
    let codesign = if apple.codesign_path.is_empty() {
        "codesign".to_owned()
    } else {
        apple.codesign_path.clone()
    };
    let display = path.to_string_lossy().into_owned();
    run(
        &codesign,
        &["--verify", "--strict", "--verbose=2", &display],
    )?;
    let report = run(&codesign, &["-dv", "--verbose=4", &display])?;

    let authority = field(&report, "Authority=").unwrap_or_default();
    let team_identifier = field(&report, "TeamIdentifier=").unwrap_or_default();
    let hardened_runtime = report
        .lines()
        .find_map(|line| line.strip_prefix("CodeDirectory "))
        .is_some_and(|_| report.contains("(runtime)"))
        || report.contains("flags=0x10000(runtime)")
        || report.contains(",runtime)")
        || report.contains("(runtime,");
    let trusted_timestamp = report
        .lines()
        .any(|line| line.starts_with("Timestamp=") && !line.contains("none"));

    let mut missing = Vec::new();
    if authority != apple.app_identity {
        missing.push(format!("Authority={authority:?}"));
    }
    if team_identifier != apple.team_id {
        missing.push(format!("TeamIdentifier={team_identifier:?}"));
    }
    if !hardened_runtime {
        missing.push("hardened runtime".to_owned());
    }
    if !trusted_timestamp {
        missing.push("trusted timestamp".to_owned());
    }
    if !missing.is_empty() {
        return Err(AppleError::new(format!(
            "missing required:\n  {relative}\n  {}",
            missing.join("\n  ")
        )));
    }
    Ok(SignedMember {
        relative: relative.to_owned(),
        payload,
        sha256: crate::digest::sha256_hex(&fs::read(path)?),
        authority,
        team_identifier,
        hardened_runtime,
        trusted_timestamp,
    })
}

fn field(report: &str, key: &str) -> Option<String> {
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix(key))
        .map(|value| value.trim().to_owned())
}

/// Build and sign the installer package over the already-signed tree.
///
/// The `.pkg` is macOS's answer to `.deb`/`.rpm`: it relocates the same tree
/// under a system prefix and puts `bin/` on `PATH`. It is also the only
/// container in this set that can be stapled — a `.tar.gz` cannot carry a
/// notarization ticket, so the tarball's binaries are covered by the online
/// check against the tickets this submission registers.
///
/// 🔴 **Two steps, and the split is forced by the credential rather than by
/// taste.** `pkgbuild --sign` fails against our Developer ID Installer key with
/// `errSecInteractionNotAllowed (-25308)`, which reads exactly like a locked
/// keychain or a cleared partition list and is neither: the key was imported
/// with `-T /usr/bin/codesign -T /usr/bin/productbuild`, so `pkgbuild` is not
/// an admitted tool for it and `productsign` is. Measured 2026-08-17 on pro5e
/// in one session — `pkgbuild --sign` refused while `productsign` and
/// `productbuild --sign` both succeeded with the same identity, keychain and
/// unlock state. ⛔ Do not "fix" this by re-running the partition-list grant.
pub fn build_pkg(stage: &Path, out: &Path, version: &str, apple: &Apple) -> Result<(), AppleError> {
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    let keychain = apple.keychain_path().to_string_lossy().into_owned();
    let unsigned = out.with_extension("unsigned.pkg");
    let _ = fs::remove_file(&unsigned);
    run(
        "pkgbuild",
        &[
            "--root",
            &stage.to_string_lossy(),
            "--identifier",
            &apple.pkg_identifier,
            "--version",
            version,
            "--install-location",
            &apple.install_location,
            &unsigned.to_string_lossy(),
        ],
    )?;
    let _ = fs::remove_file(out);
    let signed = run(
        "productsign",
        &[
            "--sign",
            &apple.installer_identity,
            "--keychain",
            &keychain,
            &unsigned.to_string_lossy(),
            &out.to_string_lossy(),
        ],
    );
    // The unsigned component package is an intermediate, never an artifact. It
    // must not survive into the promoted set, where it would sit beside the
    // signed one looking like a second container.
    let _ = fs::remove_file(&unsigned);
    signed?;
    if !out.is_file() {
        return Err(AppleError::new(format!(
            "missing required:\n  signed package {}",
            out.display()
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotarizationReceipt {
    pub submission_id: String,
    pub status: String,
    pub stapled: bool,
}

/// Submit, wait, and refuse anything that is not `Accepted`.
pub fn notarize(path: &Path, apple: &Apple) -> Result<NotarizationReceipt, AppleError> {
    let keychain = apple.keychain_path().to_string_lossy().into_owned();
    let output = run(
        "xcrun",
        &[
            "notarytool",
            "submit",
            &path.to_string_lossy(),
            "--keychain-profile",
            &apple.notary_profile,
            "--keychain",
            &keychain,
            "--wait",
            "--output-format",
            "json",
        ],
    )?;
    let value: serde_json::Value = serde_json::from_str(output.trim().lines().last().unwrap_or(""))
        .map_err(|error| AppleError::new(format!("notarytool json: {error}\n{output}")))?;
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let submission_id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if status != "Accepted" {
        return Err(AppleError::new(format!(
            "unexpected:\n  notarization {status:?} for {}\n  submission {submission_id}",
            path.display()
        )));
    }
    Ok(NotarizationReceipt {
        submission_id,
        status,
        stapled: false,
    })
}

/// Staple, then read the ticket back off the file. `staple` succeeding is the
/// act; `validate` is the observation, and only the second one survives the
/// file being copied somewhere else.
pub fn staple(path: &Path) -> Result<(), AppleError> {
    run("xcrun", &["stapler", "staple", &path.to_string_lossy()])?;
    run("xcrun", &["stapler", "validate", &path.to_string_lossy()])?;
    Ok(())
}

/// `spctl` assessment, the same evaluation Gatekeeper performs.
pub fn assess(path: &Path, kind: &str) -> Result<String, AppleError> {
    let report = run(
        "spctl",
        &["-a", "-vvv", "-t", kind, &path.to_string_lossy()],
    )?;
    if !report.contains("accepted") {
        return Err(AppleError::new(format!(
            "unexpected:\n  spctl {kind} {}\n{report}",
            path.display()
        )));
    }
    Ok(report)
}

// ⛔ There is deliberately no `quarantine()` helper here, and an earlier draft
// of this file had one whose rationale was wrong in a way worth recording:
// *"marking the tree is what makes the Gatekeeper check able to fail."*
//
// Measured 2026-08-17 on pro5e, both halves:
//   · a fresh `tar -xzf` of our own tarball carries NO `com.apple.quarantine`
//     at all, and `curl` sets none either — so the owner's real path is never
//     marked, and a rung that marks it is testing a state no install produces;
//   · executing a marked binary from a headless shell BLOCKS INDEFINITELY.
//     `solstone-core --version` returned in 1s unmarked and had not returned
//     after four minutes marked; a 7.9 MB binary behaved identically, so it is
//     not size. The first-launch assessment wants a GUI session.
//
// What actually makes the assessment falsifiable is the ad-hoc control:
// `spctl -a -t open --context context:primary-signature` and
// `codesign -R="notarized"` both accept our binaries and both reject an
// ad-hoc-signed copy that `codesign --verify` happily accepts. That contrast is
// the proof; the xattr was theatre in the other direction.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macho::{FixtureSpec, MH_DYLIB, fixture};

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn discovery_reads_magic_not_names_and_separates_payload_from_executable() {
        let root = tempfile::tempdir().unwrap();
        let stage = root.path();
        write(
            stage,
            "bin/solstone-core",
            &fixture(&FixtureSpec::default()),
        );
        write(
            stage,
            "lib/solstone-core-speakers-analyze/libonnxruntime.1.25.0.dylib",
            &fixture(&FixtureSpec {
                filetype: MH_DYLIB,
                install_name: Some(macho::HELPER_INSTALL_NAME),
                ..FixtureSpec::default()
            }),
        );
        // A shell launcher and a model asset are not code and must not appear.
        write(
            stage,
            "bin/journal",
            b"#!/bin/sh\nexec solstone-core \"$@\"\n",
        );
        write(
            stage,
            "lib/solstone_journal_models/assets/a.onnx",
            b"\x08\x01model",
        );
        write(stage, "share/LICENSE", b"AGPL");

        let members = discover_macho_members(stage).unwrap();
        let names = members
            .iter()
            .map(|member| member.relative.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "bin/solstone-core",
                "lib/solstone-core-speakers-analyze/libonnxruntime.1.25.0.dylib",
            ]
        );
        assert!(!members[0].payload);
        assert!(members[1].payload);
    }

    #[test]
    fn discovery_returns_the_payload_half_a_binaries_only_census_would_miss() {
        // The control: a tree with only executables. If discovery ever stops
        // seeing dylibs, this pair diverges and the payload assertion is the
        // one that goes red.
        let root = tempfile::tempdir().unwrap();
        let stage = root.path();
        write(stage, "bin/one", &fixture(&FixtureSpec::default()));
        write(stage, "bin/two", &fixture(&FixtureSpec::default()));
        let members = discover_macho_members(stage).unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members.iter().filter(|member| member.payload).count(), 0);

        write(
            stage,
            "lib/libpayload.dylib",
            &fixture(&FixtureSpec {
                filetype: MH_DYLIB,
                install_name: Some("@rpath/libpayload.dylib"),
                ..FixtureSpec::default()
            }),
        );
        let members = discover_macho_members(stage).unwrap();
        assert_eq!(members.len(), 3);
        assert_eq!(members.iter().filter(|member| member.payload).count(), 1);
    }

    #[test]
    fn field_reads_the_codesign_report_shape() {
        let report = concat!(
            "Executable=/tmp/solstone-core\n",
            "Identifier=solstone-core\n",
            "CodeDirectory v=20500 size=1234 flags=0x10000(runtime) hashes=1+2\n",
            "Authority=Developer ID Application: sol pbc (7QCG8V4M6H)\n",
            "Authority=Developer ID Certification Authority\n",
            "TeamIdentifier=7QCG8V4M6H\n",
            "Timestamp=17 Aug 2026 at 08:00:00\n",
        );
        assert_eq!(
            field(report, "Authority=").as_deref(),
            Some("Developer ID Application: sol pbc (7QCG8V4M6H)")
        );
        assert_eq!(
            field(report, "TeamIdentifier=").as_deref(),
            Some("7QCG8V4M6H")
        );
        assert_eq!(field(report, "NotThere=").as_deref(), None);

        // The ad-hoc, linker-signed state every unsigned arm64 binary carries.
        let adhoc = concat!(
            "Executable=/tmp/solstone-core\n",
            "CodeDirectory v=20400 size=999 flags=0x20002(adhoc,linker-signed)\n",
            "Signature=adhoc\n",
        );
        assert_eq!(field(adhoc, "Authority=").as_deref(), None);
        assert_eq!(field(adhoc, "TeamIdentifier=").as_deref(), None);
    }

    #[test]
    fn fake_archive_signer_is_deterministic_and_marker_distinct() {
        let temporary = tempfile::tempdir().expect("temporary signer files");
        let first = temporary.path().join("first");
        let second = temporary.path().join("second");
        let third = temporary.path().join("third");
        for path in [&first, &second, &third] {
            fs::write(path, b"unsigned member").expect("write member");
        }

        let first_signature = FakeArchiveMemberSigner::new("one")
            .sign_executable(&first, "bin/member")
            .expect("first signature");
        let second_signature = FakeArchiveMemberSigner::new("one")
            .sign_executable(&second, "bin/member")
            .expect("second signature");
        let third_signature = FakeArchiveMemberSigner::new("two")
            .sign_executable(&third, "bin/member")
            .expect("third signature");

        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        assert_eq!(first_signature.sha256, second_signature.sha256);
        assert_ne!(fs::read(&first).unwrap(), fs::read(&third).unwrap());
        assert_ne!(first_signature.sha256, third_signature.sha256);
    }
}
