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

pub const LLAMA_COMMIT: &str = "d81235049384534c167caea52b85a694f6103d14";
pub const LOADER_COMMIT: &str = "5f157b62e333c63260d05d81bf66faa216ab0fb8";
/// The archive `prepare-source` produces.
pub const SOURCE_SHA256: &str = "16318b04ce7b32f67366d3ce41d7f9f96ce6ad7f9f450341510ee96171be9a17";
pub const SOURCE_BYTES: u64 = 39595062;
pub const SDK_SHA256: &str = "81f474711e9042f4cd22b31b2f7a8870db2e428b21586fb43dd80150be97310d";
pub const SDK_BYTES: u64 = 287_971_024;
const MANIFEST_SHA256: &str = "3996105a6ad7ac14edc28f767a4760052d58dbd9f030c8ba69de66460fa7af14";
const MAX_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MEMBERS: usize = 20_000;

/// The exact upstream trees and the reviewed recipe files (under
/// `core/distribution/llama-windows/`) that make up the prepared archive.
/// The tarball identities are the upstream `git archive` inputs recorded in
/// the source manifest; `prepare_source` re-derives the same trees from the
/// pinned commits and admission then holds the result to `SOURCE_SHA256`.
struct UpstreamInput {
    component: &'static str,
    repository: &'static str,
    commit: &'static str,
    tarball_sha256: &'static str,
    tarball_bytes: u64,
}
const UPSTREAM: [UpstreamInput; 2] = [
    UpstreamInput {
        component: "llama",
        repository: "https://github.com/ggml-org/llama.cpp.git",
        commit: LLAMA_COMMIT,
        tarball_sha256: "7d79e94bc257d9dfa2a97a194bba612aaca85a811de52b536aafa90e37763084",
        tarball_bytes: 37759820,
    },
    UpstreamInput {
        component: "loader",
        repository: "https://github.com/KhronosGroup/Vulkan-Loader.git",
        commit: LOADER_COMMIT,
        tarball_sha256: "549d19257e7334727547cf01829a92205a9a96e3a7f47472b119dc88e16f41f7",
        tarball_bytes: 1_817_456,
    },
];
const RECIPE_DIR: &str = "core/distribution/llama-windows";
const RECIPES: [(&str, &str); 4] = [
    (
        "0001-bounded-shader-build.patch",
        "patches/0001-bounded-shader-build.patch",
    ),
    (
        "loader-wrapper/CMakeLists.txt",
        "loader-wrapper/CMakeLists.txt",
    ),
    (
        "vulkan-headers/VulkanHeadersConfig.cmake",
        "vulkan-headers/VulkanHeadersConfig.cmake",
    ),
    (
        "vulkan-headers/VulkanHeadersConfigVersion.cmake",
        "vulkan-headers/VulkanHeadersConfigVersion.cmake",
    ),
];
const MAX_RECIPE_BYTES: u64 = 1024 * 1024;

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
    let source_sha256 = sha256_hex(&bytes);
    let source_bytes = bytes.len() as u64;
    if source_sha256 != SOURCE_SHA256 || source_bytes != SOURCE_BYTES {
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
            sha256: source_sha256,
            size: source_bytes,
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
    let expected = UPSTREAM.map(|input| {
        (
            input.component,
            input.commit,
            input.tarball_sha256,
            input.tarball_bytes,
        )
    });
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

type PreparedMembers = BTreeMap<String, (u32, Vec<u8>)>;

/// Read one upstream tree out of a `git archive` tar stream. Directories and
/// the pax global header are dropped; anything that is not a plain file is
/// refused, as are non-canonical or duplicate paths and over-limit sizes.
fn read_upstream_tree(
    component: &str,
    tar_bytes: impl Read,
    members: &mut PreparedMembers,
    total: &mut u64,
) -> Result<(), LlamaWindowsSourceError> {
    let mut archive = tar::Archive::new(tar_bytes);
    let mut seen = BTreeSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_dir() || kind.is_pax_global_extensions() {
            continue;
        }
        let path = std::str::from_utf8(&entry.path_bytes())
            .map_err(|_| refuse("source member path is not UTF-8"))?
            .to_owned();
        if !kind.is_file() || !canonical_member(&path) {
            return Err(refuse(format!("unsupported source member: {path:?}")));
        }
        if !seen.insert(path.clone()) || seen.len() > MAX_MEMBERS {
            return Err(refuse("duplicate or excessive source members"));
        }
        let size = entry.header().size()?;
        *total = total
            .checked_add(size)
            .ok_or_else(|| refuse("source length overflow"))?;
        if size > MAX_MEMBER_BYTES || *total > MAX_UNPACKED_BYTES {
            return Err(refuse(
                "source member or aggregate size exceeds admission limit",
            ));
        }
        let mode = if entry.header().mode()? & 0o111 != 0 {
            0o755
        } else {
            0o644
        };
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        if content.len() as u64 != size {
            return Err(refuse(format!("truncated source member: {path}")));
        }
        members.insert(format!("{component}/{path}"), (mode, content));
    }
    Ok(())
}

fn fetch_upstream_tree(
    input: &UpstreamInput,
    scratch: &Path,
    members: &mut PreparedMembers,
    total: &mut u64,
) -> Result<(), LlamaWindowsSourceError> {
    let git_dir = scratch.join(format!("{}.git", input.component));
    let git = |args: &[&str]| -> io::Result<std::process::Output> {
        let mut command = std::process::Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .arg("--git-dir")
            .arg(&git_dir)
            .args(args)
            .output()
    };
    let run = |args: &[&str]| -> Result<Vec<u8>, LlamaWindowsSourceError> {
        let output = git(args)?;
        if !output.status.success() {
            return Err(refuse(format!(
                "{}: git {} failed: {}",
                input.component,
                args[0],
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(output.stdout)
    };
    let init = std::process::Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg("--quiet")
        .arg(&git_dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()?;
    if !init.success() {
        return Err(refuse("git init failed"));
    }
    run(&[
        "fetch",
        "--quiet",
        "--depth",
        "1",
        input.repository,
        input.commit,
    ])?;
    let resolved = run(&[
        "rev-parse",
        "--verify",
        &format!("{}^{{commit}}", input.commit),
    ])?;
    if String::from_utf8_lossy(&resolved).trim() != input.commit {
        return Err(refuse(format!(
            "{}: fetched commit does not match the pin",
            input.component
        )));
    }
    let tree = run(&["archive", "--format=tar", input.commit])?;
    read_upstream_tree(input.component, tree.as_slice(), members, total)
}

fn read_recipes(root: &Path, members: &mut PreparedMembers) -> Result<(), LlamaWindowsSourceError> {
    for (relative, destination) in RECIPES {
        let path = root.join(RECIPE_DIR).join(relative);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_RECIPE_BYTES {
            return Err(refuse(format!(
                "recipe is not a regular file within bounds: {}",
                path.display()
            )));
        }
        members.insert(destination.to_owned(), (0o644, fs::read(&path)?));
    }
    Ok(())
}

// Field order is the sorted key order the pinned manifest bytes use.
#[derive(serde::Serialize)]
struct ManifestMemberOut<'a> {
    bytes: u64,
    mode: u32,
    path: &'a str,
    sha256: String,
}
#[derive(serde::Serialize)]
struct ManifestSourceOut<'a> {
    bytes: u64,
    commit: &'a str,
    component: &'a str,
    sha256: &'a str,
}
#[derive(serde::Serialize)]
struct ManifestOut<'a> {
    kind: &'a str,
    members: Vec<ManifestMemberOut<'a>>,
    sources: Vec<ManifestSourceOut<'a>>,
}

/// Python's default JSON output escapes every non-ASCII character; the
/// pinned manifest was written that way, so match it.
fn ascii_json(value: &impl serde::Serialize) -> Result<Vec<u8>, LlamaWindowsSourceError> {
    let text = serde_json::to_string(value).map_err(|e| refuse(e.to_string()))?;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0_u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out.push('\n');
    Ok(out.into_bytes())
}

/// Deterministic prepared archive: sorted members, zeroed metadata, manifest
/// last in sort order alongside the rest.
fn assemble_archive(mut members: PreparedMembers) -> Result<Vec<u8>, LlamaWindowsSourceError> {
    let manifest = ManifestOut {
        kind: "draft-source-bundle-not-native-admission",
        members: members
            .iter()
            .map(|(path, (mode, data))| ManifestMemberOut {
                bytes: data.len() as u64,
                mode: *mode,
                path,
                sha256: sha256_hex(data),
            })
            .collect(),
        sources: UPSTREAM
            .iter()
            .map(|input| ManifestSourceOut {
                bytes: input.tarball_bytes,
                commit: input.commit,
                component: input.component,
                sha256: input.tarball_sha256,
            })
            .collect(),
    };
    let manifest = ascii_json(&manifest)?;
    members.insert("source-manifest.json".to_owned(), (0o644, manifest));
    let mut builder = tar::Builder::new(Vec::new());
    for (name, (mode, data)) in &members {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_username("")?;
        header.set_groupname("")?;
        header.set_mtime(0);
        // Names beyond ASCII or the ustar field travel as a PAX `path` record,
        // which extractors read as UTF-8 whatever their process locale is. The
        // ustar name is an ASCII stand-in for readers that ignore PAX.
        if name.is_ascii() && name.len() <= 100 {
            header.set_path(name)?;
        } else {
            builder.append_pax_extensions([("path", name.as_bytes())])?;
            let fallback: Vec<u8> = name
                .bytes()
                .map(|byte| if byte.is_ascii() { byte } else { b'_' })
                .rev()
                .take(100)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let field = &mut header.as_old_mut().name;
            field.fill(0);
            field[..fallback.len()].copy_from_slice(&fallback);
        }
        header.set_cksum();
        builder.append(&header, data.as_slice())?;
    }
    Ok(crate::tar::gzip_bytes(&builder.into_inner()?)?)
}

/// `llama-windows prepare-source --dest PATH`: fetch both pinned upstream
/// commits, overlay the reviewed recipe files, write the archive create-only,
/// then admit it through `inspect_source`. A tree that does not admit is
/// removed rather than left where a later step could pick it up.
fn prepare_source(dest: &Path) -> Result<String, LlamaWindowsSourceError> {
    let root = repository_root()?;
    let scratch = tempfile::tempdir()?;
    let mut members = PreparedMembers::new();
    let mut total = 0_u64;
    for input in &UPSTREAM {
        fetch_upstream_tree(input, scratch.path(), &mut members, &mut total)?;
    }
    read_recipes(&root, &mut members)?;
    let archive = assemble_archive(members)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    io::Write::write_all(&mut file, &archive)?;
    file.sync_all()?;
    drop(file);
    match inspect_source(dest) {
        Ok(identity) if identity.sha256 != SOURCE_SHA256 => Err(refuse(
            "prepared archive matches the legacy packing, not the current pin",
        )),
        Ok(identity) => Ok(format!(
            "prepared {} sha256={} bytes={} (admitted)",
            dest.display(),
            identity.sha256,
            identity.size
        )),
        Err(error) => {
            let _ = fs::remove_file(dest);
            Err(refuse(format!(
                "prepared archive was not admitted and was removed: {error}; built sha256={} bytes={}",
                sha256_hex(&archive),
                archive.len()
            )))
        }
    }
}

fn repository_root() -> Result<std::path::PathBuf, LlamaWindowsSourceError> {
    let mut root = std::env::current_dir()?;
    while !root.join("core/distribution/builder-inputs.toml").is_file() {
        root = root
            .parent()
            .ok_or_else(|| refuse("could not find builder-inputs.toml"))?
            .to_path_buf();
    }
    Ok(root)
}

pub fn run_cli(args: &[String]) -> Result<String, LlamaWindowsSourceError> {
    const USAGE: &str = "usage: solstone-distribution llama-windows <prepare-source|verify-inputs|inspect-capture|record|verify> --source-archive PATH --sdk-archive PATH --cmake-archive PATH [--capture-root PATH] [--receipt-file PATH --evidence-file PATH --builder-host HOST]";
    let Some((operation, rest)) = args.split_first() else {
        return Err(refuse(USAGE));
    };
    if matches!(operation.as_str(), "help" | "--help" | "-h") && rest.is_empty() {
        return Ok(USAGE.to_owned());
    }
    if operation == "prepare-source" {
        return match rest {
            [flag, dest] if flag == "--dest" && !dest.is_empty() => prepare_source(Path::new(dest)),
            _ => Err(refuse(
                "usage: solstone-distribution llama-windows prepare-source --dest PATH",
            )),
        };
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
    let root = repository_root()?;
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
                    sha256: UPSTREAM[0].tarball_sha256.to_owned(),
                    bytes: UPSTREAM[0].tarball_bytes,
                },
                SourceInput {
                    component: "loader".to_owned(),
                    commit: LOADER_COMMIT.to_owned(),
                    sha256: UPSTREAM[1].tarball_sha256.to_owned(),
                    bytes: UPSTREAM[1].tarball_bytes,
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
        changed.sources[0].sha256 =
            "9c802144585b8102e78dc6942adfde273686a87b2efae9b99de98891a20c06d3".to_owned();
        changed.sources[0].bytes = 35_828_683;
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

    #[test]
    fn prepared_archive_is_deterministic_and_self_consistent() {
        let members = || {
            PreparedMembers::from([
                ("llama/b.txt".to_owned(), (0o644, b"b".to_vec())),
                ("llama/a\u{6d4b}.sh".to_owned(), (0o755, b"a".to_vec())),
                (
                    format!("loader/{}", "d/".repeat(60) + "long.c"),
                    (0o644, b"c".to_vec()),
                ),
                ("patches/p".to_owned(), (0o644, b"p".to_vec())),
            ])
        };
        let first = assemble_archive(members()).unwrap();
        assert_eq!(first, assemble_archive(members()).unwrap());
        let (manifest_bytes, census) = archive_members(&first).unwrap();
        let manifest: SourceManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        validate_manifest(&manifest, &census).unwrap();
        assert!(census.contains_key("llama/a\u{6d4b}.sh"));
        assert!(std::str::from_utf8(&manifest_bytes).unwrap().is_ascii());
        // Non-ASCII names must be carried by a PAX record, not raw ustar bytes.
        let raw = crate::tar::gunzip_bytes(&first).unwrap();
        let pax = "path=llama/a\u{6d4b}.sh\n".as_bytes();
        assert!(raw.windows(pax.len()).any(|window| window == pax));
        assert_eq!(census["llama/a\u{6d4b}.sh"].mode, 0o755);
    }

    #[test]
    fn upstream_tree_refuses_links_and_drops_directories_and_global_header() {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, kind, mode) in [
            ("d", tar::EntryType::Directory, 0o775),
            ("d/run.sh", tar::EntryType::Regular, 0o775),
            ("d/plain", tar::EntryType::Regular, 0o664),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_mode(mode);
            header.set_size(if kind.is_dir() { 0 } else { 1 });
            builder
                .append_data(
                    &mut header,
                    name,
                    if kind.is_dir() { &b""[..] } else { &b"x"[..] },
                )
                .unwrap();
        }
        let good = builder.into_inner().unwrap();
        let mut members = PreparedMembers::new();
        read_upstream_tree("llama", good.as_slice(), &mut members, &mut 0).unwrap();
        assert_eq!(members["llama/d/run.sh"].0, 0o755);
        assert_eq!(members["llama/d/plain"].0, 0o644);
        assert_eq!(members.len(), 2);
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder.append_link(&mut header, "l", "target").unwrap();
        let link = builder.into_inner().unwrap();
        assert!(
            read_upstream_tree(
                "llama",
                link.as_slice(),
                &mut PreparedMembers::new(),
                &mut 0
            )
            .is_err()
        );
    }
}
