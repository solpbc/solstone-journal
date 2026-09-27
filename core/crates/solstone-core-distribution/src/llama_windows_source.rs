// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only admission of the source and builder inputs used by the Windows
//! llama capture driver. A source archive is not evidence of a completed build.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::controlled_build::InputIdentityEntry;
use crate::digest::sha256_hex;

pub const LLAMA_COMMIT: &str = "571d0d540df04f25298d0e159e520d9fc62ed121";
pub const LOADER_COMMIT: &str = "5f157b62e333c63260d05d81bf66faa216ab0fb8";
pub const SOURCE_SHA256: &str = "ea613b46d078609bdac8dc05f99959bd38e965e7a5abadf58eab023c83203828";
pub const SOURCE_BYTES: u64 = 37_227_459;
pub const SDK_SHA256: &str = "81f474711e9042f4cd22b31b2f7a8870db2e428b21586fb43dd80150be97310d";
pub const SDK_BYTES: u64 = 287_971_024;
const MANIFEST_SHA256: &str = "10741e5d2fceb9c6027c90dae67c6a36bde196e08b537e38d9c3ef009c09fe79";
const MAX_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MEMBERS: usize = 20_000;

#[derive(Debug)]
pub struct LlamaWindowsSourceError(String);

impl std::fmt::Display for LlamaWindowsSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LlamaWindowsSourceError {}
impl From<io::Error> for LlamaWindowsSourceError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}
fn refuse(message: impl Into<String>) -> LlamaWindowsSourceError {
    LlamaWindowsSourceError(message.into())
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceManifest {
    kind: String,
    sources: Vec<SourceInput>,
    members: Vec<SourceMember>,
}
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceInput {
    component: String,
    commit: String,
    sha256: String,
    bytes: u64,
}
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceMember {
    path: String,
    mode: u32,
    bytes: u64,
    sha256: String,
}
#[derive(Debug, PartialEq, Eq)]
struct MemberIdentity {
    mode: u32,
    bytes: u64,
    sha256: String,
}

/// Each tool has its own exact length, including the SDK which exceeds the
/// generic retained-artifact bound. Never widen all artifact limits for it.
pub fn inspect_sdk(path: &Path) -> Result<InputIdentityEntry, LlamaWindowsSourceError> {
    inspect_pinned_file(
        path,
        "tools/vulkan-sdk-windows-x64.exe",
        SDK_BYTES,
        SDK_SHA256,
    )
}

fn inspect_pinned_file(
    path: &Path,
    label: &str,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<InputIdentityEntry, LlamaWindowsSourceError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() != expected_bytes {
        return Err(refuse(format!(
            "{label}: expected a regular file of {expected_bytes} bytes"
        )));
    }
    let mut file = fs::File::open(path)?.take(expected_bytes + 1);
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        hasher.update(&buffer[..count]);
    }
    let sha256 = format!("{:x}", hasher.finalize());
    if bytes != expected_bytes || sha256 != expected_sha256 {
        return Err(refuse(format!("{label}: input length or SHA-256 mismatch")));
    }
    Ok(InputIdentityEntry {
        label: label.to_owned(),
        sha256,
        size: bytes,
    })
}

pub fn inspect_source(path: &Path) -> Result<InputIdentityEntry, LlamaWindowsSourceError> {
    inspect_source_with_census(path).map(|(identity, _)| identity)
}

pub(crate) type SourceCensus = BTreeMap<String, (u64, String)>;

pub(crate) fn inspect_source_with_census(
    path: &Path,
) -> Result<(InputIdentityEntry, SourceCensus), LlamaWindowsSourceError> {
    // Bound the read itself, then verify the bytes we parse (not a prior path read).
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(refuse("source archive must be a regular file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != SOURCE_BYTES || sha256_hex(&bytes) != SOURCE_SHA256 {
        return Err(refuse("llama source archive identity mismatch"));
    }
    let (manifest_bytes, members) = archive_members(&bytes)?;
    if sha256_hex(&manifest_bytes) != MANIFEST_SHA256 {
        return Err(refuse("llama source manifest identity mismatch"));
    }
    let manifest: SourceManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| refuse(format!("llama source manifest: {e}")))?;
    validate_manifest(&manifest, &members)?;
    let mut census: SourceCensus = members
        .into_iter()
        .map(|(path, member)| (path, (member.bytes, member.sha256)))
        .collect();
    census.insert(
        "source-manifest.json".into(),
        (manifest_bytes.len() as u64, sha256_hex(&manifest_bytes)),
    );
    Ok((
        InputIdentityEntry {
            label: "sources/llama-windows.tar.gz".to_owned(),
            sha256: SOURCE_SHA256.to_owned(),
            size: SOURCE_BYTES,
        },
        census,
    ))
}

fn canonical_member(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn archive_members(
    bytes: &[u8],
) -> Result<(Vec<u8>, BTreeMap<String, MemberIdentity>), LlamaWindowsSourceError> {
    let mut archive = tar::Archive::new(GzDecoder::new(bytes).take(MAX_UNPACKED_BYTES + 1));
    let mut members = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut manifest = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = std::str::from_utf8(&entry.path_bytes())
            .map_err(|_| refuse("source member path is not UTF-8"))?
            .to_owned();
        if !entry.header().entry_type().is_file() || !canonical_member(&path) {
            return Err(refuse(format!("unsupported source member: {path:?}")));
        }
        if !seen.insert(path.clone()) || seen.len() > MAX_MEMBERS {
            return Err(refuse("duplicate or excessive source members"));
        }
        let size = entry.header().size()?;
        total = total
            .checked_add(size)
            .ok_or_else(|| refuse("source length overflow"))?;
        if size > MAX_MEMBER_BYTES || total > MAX_UNPACKED_BYTES {
            return Err(refuse(
                "source member or aggregate size exceeds admission limit",
            ));
        }
        let mode = entry.header().mode()?;
        if !matches!(mode, 0o644 | 0o755) {
            return Err(refuse(format!("unsupported source member mode: {path}")));
        }
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        if content.len() as u64 != size {
            return Err(refuse(format!("truncated source member: {path}")));
        }
        if path == "source-manifest.json" {
            manifest = Some(content);
        } else {
            members.insert(
                path,
                MemberIdentity {
                    mode,
                    bytes: size,
                    sha256: sha256_hex(&content),
                },
            );
        }
    }
    Ok((
        manifest.ok_or_else(|| refuse("source manifest missing"))?,
        members,
    ))
}

fn validate_manifest(
    manifest: &SourceManifest,
    members: &BTreeMap<String, MemberIdentity>,
) -> Result<(), LlamaWindowsSourceError> {
    if manifest.kind != "draft-source-bundle-not-native-admission" {
        return Err(refuse("source manifest kind mismatch"));
    }
    let expected = [
        (
            "llama",
            LLAMA_COMMIT,
            "9c802144585b8102e78dc6942adfde273686a87b2efae9b99de98891a20c06d3",
            35_828_683,
        ),
        (
            "loader",
            LOADER_COMMIT,
            "549d19257e7334727547cf01829a92205a9a96e3a7f47472b119dc88e16f41f7",
            1_817_456,
        ),
    ];
    if manifest.sources.len() != expected.len() {
        return Err(refuse("source component census mismatch"));
    }
    for (source, (component, commit, sha256, size)) in manifest.sources.iter().zip(expected) {
        if source.component != component
            || source.commit != commit
            || source.sha256 != sha256
            || source.bytes != size
        {
            return Err(refuse(format!(
                "source component identity mismatch: {component}"
            )));
        }
    }
    let mut seen = BTreeSet::new();
    for member in &manifest.members {
        if !canonical_member(&member.path) || !seen.insert(member.path.as_str()) {
            return Err(refuse("duplicate or invalid manifest member"));
        }
        let expected = MemberIdentity {
            mode: member.mode,
            bytes: member.bytes,
            sha256: member.sha256.clone(),
        };
        if members.get(&member.path) != Some(&expected) {
            return Err(refuse(format!(
                "source member identity mismatch: {}",
                member.path
            )));
        }
    }
    if seen.len() != members.len() {
        return Err(refuse("unrecognized source archive members"));
    }
    Ok(())
}

pub fn run_cli(args: &[String]) -> Result<String, LlamaWindowsSourceError> {
    const USAGE: &str = "usage: solstone-distribution llama-windows <verify-inputs|inspect-capture|record|verify> --source-archive PATH --sdk-archive PATH --cmake-archive PATH [--capture-root PATH] [--receipt-file PATH --evidence-file PATH --builder-host HOST]";
    let Some((operation, rest)) = args.split_first() else {
        return Err(refuse(USAGE));
    };
    if matches!(operation.as_str(), "help" | "--help" | "-h") && rest.is_empty() {
        return Ok(USAGE.to_owned());
    }
    let record_mode = matches!(operation.as_str(), "record" | "verify");
    let capture_mode = record_mode || operation == "inspect-capture";
    if (!capture_mode && operation != "verify-inputs")
        || rest.len()
            != if record_mode {
                14
            } else if capture_mode {
                8
            } else {
                6
            }
    {
        return Err(refuse(USAGE));
    }
    let mut flags = BTreeMap::new();
    for pair in rest.chunks_exact(2) {
        if !matches!(
            pair[0].as_str(),
            "--source-archive"
                | "--sdk-archive"
                | "--cmake-archive"
                | "--capture-root"
                | "--receipt-file"
                | "--evidence-file"
                | "--builder-host"
        ) || pair[1].is_empty()
            || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(refuse("unknown, empty or duplicate llama input flag"));
        }
    }
    for required in ["--source-archive", "--sdk-archive", "--cmake-archive"] {
        if !flags.contains_key(required) {
            return Err(refuse(format!("missing {required}")));
        }
    }
    if capture_mode != flags.contains_key("--capture-root") {
        return Err(refuse(USAGE));
    }
    let (source, census) = inspect_source_with_census(Path::new(flags["--source-archive"]))?;
    let sdk = inspect_sdk(Path::new(flags["--sdk-archive"]))?;
    let mut root = std::env::current_dir()?;
    while !root.join("core/distribution/builder-inputs.toml").is_file() {
        root = root
            .parent()
            .ok_or_else(|| refuse("could not find builder-inputs.toml"))?
            .to_path_buf();
    }
    let (_, cmake) = crate::ced_windows_source::inspect_cmake_windows_archive(
        &root,
        Path::new(flags["--cmake-archive"]),
    )
    .map_err(|e| refuse(e.to_string()))?;
    if capture_mode {
        let files = crate::llama_windows_capture::read_capture(Path::new(flags["--capture-root"]))
            .map_err(refuse)?;
        let evidence =
            crate::llama_windows_capture::inspect(&files, &census, &source, &sdk, &cmake)
                .map_err(refuse)?;
        if record_mode {
            for required in ["--receipt-file", "--evidence-file", "--builder-host"] {
                if !flags.contains_key(required) {
                    return Err(refuse(format!("missing {required}")));
                }
            }
            let capture = fs::canonicalize(flags["--capture-root"])?;
            for output in ["--receipt-file", "--evidence-file"] {
                let path = Path::new(flags[output]);
                let parent = path
                    .parent()
                    .ok_or_else(|| refuse("record needs a parent directory"))?;
                // Existing separate record directory keeps receipts out of their own input census.
                let parent = fs::canonicalize(parent)?;
                if parent.starts_with(&capture) {
                    return Err(refuse(
                        "admission records must be outside the captured tree",
                    ));
                }
            }
            let (receipt, bytes) = crate::llama_windows_capture::receipt(
                &evidence,
                &files,
                vec![source, sdk, cmake],
                flags["--builder-host"],
            )
            .map_err(refuse)?;
            let receipt_path = Path::new(flags["--receipt-file"]);
            let evidence_path = Path::new(flags["--evidence-file"]);
            if operation == "record" {
                return crate::llama_windows_capture::publish(
                    &receipt,
                    &bytes,
                    receipt_path,
                    evidence_path,
                )
                .map_err(refuse);
            }
            crate::llama_windows_capture::verify_record(
                &receipt,
                &bytes,
                receipt_path,
                evidence_path,
            )
            .map_err(refuse)?;
            return Ok("verified original pre-sign receipt and all retained capture bytes; package/signing admission remains separate".into());
        }
        return serde_json::to_string_pretty(&evidence).map_err(|e| refuse(e.to_string()));
    }
    serde_json::to_string_pretty(&vec![source, sdk, cmake]).map_err(|e| refuse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, kind, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_entry_type(*kind);
            header.set_mode(0o644);
            header.set_size(content.len() as u64);
            header.set_cksum();
            builder.append(&header, *content).unwrap();
        }
        crate::tar::gzip_bytes(&builder.into_inner().unwrap()).unwrap()
    }

    #[test]
    fn source_census_preserves_unicode_and_refuses_duplicates_and_links() {
        let bytes = archive(&[
            ("source-manifest.json", tar::EntryType::Regular, b"{}"),
            (
                "loader/\u{6d4b}\u{8bd5}.def",
                tar::EntryType::Regular,
                b"EXPORTS",
            ),
        ]);
        let (_, members) = archive_members(&bytes).unwrap();
        assert_eq!(
            members["loader/\u{6d4b}\u{8bd5}.def"].sha256,
            sha256_hex(b"EXPORTS")
        );
        let duplicate = archive(&[
            ("source-manifest.json", tar::EntryType::Regular, b"{}"),
            ("source-manifest.json", tar::EntryType::Regular, b"{}"),
        ]);
        assert!(archive_members(&duplicate).is_err());
        let link = archive(&[("escape", tar::EntryType::Symlink, b"")]);
        assert!(archive_members(&link).is_err());
    }

    #[test]
    fn source_paths_refuse_windows_and_posix_escapes() {
        for path in [
            "", "/etc/a", "../a", "x/../a", "x//a", "./a", "C:/a", "x\\a", "a\0b",
        ] {
            assert!(!canonical_member(path), "{path:?}");
        }
        assert!(canonical_member("llama/common/a.cpp"));
    }

    fn manifest() -> SourceManifest {
        SourceManifest {
            kind: "draft-source-bundle-not-native-admission".to_owned(),
            sources: vec![
                SourceInput {
                    component: "llama".to_owned(),
                    commit: LLAMA_COMMIT.to_owned(),
                    sha256: "9c802144585b8102e78dc6942adfde273686a87b2efae9b99de98891a20c06d3"
                        .to_owned(),
                    bytes: 35_828_683,
                },
                SourceInput {
                    component: "loader".to_owned(),
                    commit: LOADER_COMMIT.to_owned(),
                    sha256: "549d19257e7334727547cf01829a92205a9a96e3a7f47472b119dc88e16f41f7"
                        .to_owned(),
                    bytes: 1_817_456,
                },
            ],
            members: vec![SourceMember {
                path: "llama/a".to_owned(),
                mode: 0o644,
                bytes: 1,
                sha256: sha256_hex(b"a"),
            }],
        }
    }

    #[test]
    fn manifest_binds_content_modes_membership_and_source_revision() {
        let members = BTreeMap::from([(
            "llama/a".to_owned(),
            MemberIdentity {
                mode: 0o644,
                bytes: 1,
                sha256: sha256_hex(b"a"),
            },
        )]);
        validate_manifest(&manifest(), &members).unwrap();
        let mut changed = manifest();
        changed.sources[0].commit = "0".repeat(40);
        assert!(validate_manifest(&changed, &members).is_err());
        let mut changed = manifest();
        changed.members[0].sha256 = sha256_hex(b"b");
        assert!(validate_manifest(&changed, &members).is_err());
        let mut changed = manifest();
        changed.members[0].mode = 0o755;
        assert!(validate_manifest(&changed, &members).is_err());
        let mut changed = manifest();
        changed.members.clear();
        assert!(validate_manifest(&changed, &members).is_err());
    }

    #[test]
    fn input_cli_refuses_duplicate_and_unknown_flags_before_io() {
        for flags in [
            [
                "--source-archive",
                "a",
                "--sdk-archive",
                "b",
                "--sdk-archive",
                "c",
            ],
            [
                "--source-archive",
                "a",
                "--sdk-archive",
                "b",
                "--unknown",
                "c",
            ],
        ] {
            let args = std::iter::once("verify-inputs")
                .chain(flags)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            assert!(run_cli(&args).is_err());
        }
    }
}
