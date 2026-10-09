// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The inventory lists files, not an SBOM. Code compiled into an executable is
//! outside it: FFmpeg, the WebView2 loader, Velopack, the Rust crates, and
//! FluidAudio in `parakeet-helper`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::inventory::Inventory;

#[derive(Debug)]
pub struct EvidenceError {
    pub message: String,
}

impl EvidenceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for EvidenceError {}

pub fn evidence_directory_name(basename: &str) -> String {
    format!("{basename}.component-evidence")
}

pub fn components_file_name(version: &str, target: &str) -> String {
    format!("solstone-journal-{version}-{target}.components.json")
}

pub fn provenance_file_name(version: &str, target: &str) -> String {
    format!("solstone-journal-{version}-{target}.component-provenance.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentFile {
    pub product: String,
    pub version: String,
    pub target: String,
    pub components: Vec<ComponentRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentRow {
    pub id: String,
    pub version: String,
    pub delivery: DeliveryKind,
    pub source: String,
    pub inputs: Vec<InputRef>,
    pub members: Vec<MemberRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryKind {
    Bundled,
    RuntimeDownloaded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputRef {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberRef {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvenanceFile {
    pub product: String,
    pub version: String,
    pub target: String,
    pub basis: String,
    pub manifests: ProvenanceManifests,
    pub records: Vec<ProvenanceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProvenanceManifests {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_payload_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_manifest_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_manifest_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvenanceRecord {
    pub id: String,
    pub path: String,
    pub input: InputRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_signing_sha256: Option<String>,
    pub final_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamedInput {
    pub target: String,
    pub id: String,
    pub dest: String,
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InjectedFetch {
    pub unit: String,
    pub version: String,
    pub origin_key: String,
    pub sha256: String,
    pub filename: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEvidence {
    pub components: String,
    pub provenance: String,
}

#[derive(Debug, Clone)]
pub struct PosixEvidenceRequest<'a> {
    pub target: &'a str,
    pub version: &'a str,
    pub basename: &'a str,
    pub inventory: &'a Inventory,
    pub stage: &'a Path,
    pub release_manifest: &'a Path,
    pub pre_signing: BTreeMap<String, String>,
    pub checkout: &'a Path,
    pub injected: Vec<InjectedFetch>,
    pub archives: Vec<(String, Vec<u8>)>,
}

pub fn check_member_path(path: &str) -> Result<(), EvidenceError> {
    if path.contains('\\')
        || path
            .chars()
            .any(|c| (c as u32) < 0x20 || (0x7F..=0x9F).contains(&(c as u32)))
    {
        return Err(EvidenceError::new(format!("bad-member-path: {path}")));
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return Err(EvidenceError::new(format!("bad-member-path: {path}")));
    }
    if !matches!(parts[0], "bin" | "lib" | "share") {
        return Err(EvidenceError::new(format!("bad-member-path: {path}")));
    }
    for part in parts {
        if part.is_empty() || part == "." || part == ".." {
            return Err(EvidenceError::new(format!("bad-member-path: {path}")));
        }
    }
    Ok(())
}

fn read_llama_cpp_revision(checkout: &Path) -> Result<String, EvidenceError> {
    let pins_path = checkout.join("core/crates/solstone-core-local/src/install/pins.rs");
    let content = std::fs::read_to_string(&pins_path)
        .map_err(|e| EvidenceError::new(format!("failed to read {}: {e}", pins_path.display())))?;
    for line in content.lines() {
        if line.contains("\"llama_cpp_revision\"")
            && let Some(start) = line.find(':')
        {
            let rest = line[start + 1..].trim();
            let unquoted = rest.trim_matches(|c| c == '"' || c == ',' || c == ' ');
            if !unquoted.is_empty() {
                return Ok(unquoted.to_string());
            }
        }
    }
    Err(EvidenceError::new(format!(
        "could not find llama_cpp_revision in {}",
        pins_path.display()
    )))
}

/// Test-only replacement identity for a bundled component, keyed by (target, id).
#[cfg(test)]
type BundledIdentityOverride = BTreeMap<(String, String), (String, String, Vec<InputRef>)>;

#[cfg(test)]
thread_local! {
    static RESIDUAL_OVERRIDES: std::cell::RefCell<BTreeMap<(String, String, String), String>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
    static BUNDLED_IDENTITY_OVERRIDE: std::cell::RefCell<Option<BundledIdentityOverride>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn set_residual_override(target: &str, id: &str, fact: &str, value: &str) {
    RESIDUAL_OVERRIDES.with(|cell| {
        cell.borrow_mut().insert(
            (target.to_string(), id.to_string(), fact.to_string()),
            value.to_string(),
        );
    });
}

#[cfg(test)]
pub fn clear_residual_overrides() {
    RESIDUAL_OVERRIDES.with(|cell| {
        cell.borrow_mut().clear();
    });
}

#[cfg(test)]
pub fn set_bundled_identity_override(
    target: &str,
    id: &str,
    version: &str,
    source: &str,
    inputs: Vec<InputRef>,
) {
    BUNDLED_IDENTITY_OVERRIDE.with(|cell| {
        let mut map = cell.borrow_mut();
        let map = map.get_or_insert_with(BTreeMap::new);
        map.insert(
            (target.to_string(), id.to_string()),
            (version.to_string(), source.to_string(), inputs),
        );
    });
}

#[cfg(test)]
pub fn clear_bundled_identity_override() {
    BUNDLED_IDENTITY_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

fn residual_override(target: &str, id: &str, fact: &str) -> Option<String> {
    #[cfg(test)]
    {
        RESIDUAL_OVERRIDES.with(|cell| {
            cell.borrow()
                .get(&(target.to_string(), id.to_string(), fact.to_string()))
                .cloned()
        })
    }
    #[cfg(not(test))]
    {
        let _ = (target, id, fact);
        None
    }
}

fn check_residual(target: &str, id: &str, fact: &str, value: &str) -> Result<(), EvidenceError> {
    if let Some(planted) = residual_override(target, id, fact)
        && planted != value
    {
        return Err(EvidenceError::new(format!(
            "residual-disagrees: {target} {id} {fact}"
        )));
    }
    Ok(())
}

fn catalog_source(
    artifact: &solstone_core_assets::Artifact,
    id: &str,
    checkout: &Path,
) -> Result<String, EvidenceError> {
    if artifact.upstream_url.contains("updates.solstone.app") {
        if id == "llama-server-cuda" {
            let rev = read_llama_cpp_revision(checkout)?;
            return Ok(rev);
        }
        return Err(EvidenceError::new(format!("owner-origin-source: {id}")));
    }
    let url = artifact.upstream_url;
    if let Some(after_github) = url.strip_prefix("https://github.com/")
        && let Some(pos) = after_github.find("/releases/download/")
    {
        let prefix = &after_github[..pos];
        let after_download = &after_github[pos + "/releases/download/".len()..];
        if let Some(tag_end) = after_download.find('/') {
            let tag = &after_download[..tag_end];
            return Ok(format!(
                "https://github.com/{prefix}/releases/download/{tag}"
            ));
        }
    }
    if let Some(after_hf) = url.strip_prefix("https://huggingface.co/")
        && let Some(pos) = after_hf.find("/resolve/")
    {
        let repo = &after_hf[..pos];
        let after_resolve = &after_hf[pos + "/resolve/".len()..];
        if let Some(rev_end) = after_resolve.find('/') {
            let rev = &after_resolve[..rev_end];
            return Ok(format!("{repo}@{rev}"));
        }
    }
    Ok(url.to_string())
}

fn read_digest_const(path: &Path, const_name: &str) -> Result<String, EvidenceError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| EvidenceError::new(format!("read {}: {e}", path.display())))?;
    crate::inventory::digest_const_hex(&text, const_name).ok_or_else(|| {
        EvidenceError::new(format!(
            "const {const_name} not found in {}",
            path.display()
        ))
    })
}

fn bundled_identity(
    checkout: &Path,
    target: &str,
    id: &str,
) -> Result<(String, String, Vec<InputRef>), EvidenceError> {
    #[cfg(test)]
    {
        if let Some(res) = BUNDLED_IDENTITY_OVERRIDE.with(|cell| {
            cell.borrow()
                .as_ref()
                .and_then(|m| m.get(&(target.to_string(), id.to_string())).cloned())
        }) {
            return Ok(res);
        }
    }
    if target == "windows-x86_64" {
        match id {
            "ced-engine" => {
                let ver = crate::ced_windows::CED_CPP_COMMIT.to_string();
                let src = crate::ced_windows::CED_CPP_COMMIT.to_string();
                Ok((ver, src, Vec::new()))
            }
            "llama-server" => {
                let ver = crate::llama_windows_source::LLAMA_COMMIT.to_string();
                let src = crate::llama_windows_source::LLAMA_COMMIT.to_string();
                Ok((ver, src, Vec::new()))
            }
            "vulkan-loader" => {
                let ver = crate::llama_windows_source::LOADER_COMMIT.to_string();
                let src = crate::llama_windows_source::LOADER_COMMIT.to_string();
                Ok((ver, src, Vec::new()))
            }
            "parakeet-server" => {
                let ver = crate::parakeet_windows::PARAKEET_CPP_COMMIT.to_string();
                let src = crate::parakeet_windows::PARAKEET_CPP_COMMIT.to_string();
                Ok((ver, src, Vec::new()))
            }
            "parakeet-model" => {
                let ver = crate::parakeet_windows::PARAKEET_MODEL_REVISION.to_string();
                let src = format!(
                    "mudler/parakeet-cpp-gguf@{}",
                    crate::parakeet_windows::PARAKEET_MODEL_REVISION
                );
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: crate::parakeet_windows::PARAKEET_MODEL_FILENAME.to_string(),
                        sha256: crate::parakeet_windows::PARAKEET_MODEL_SHA256.to_string(),
                    }],
                ))
            }
            "rfdetr-engine" => {
                let ver = crate::rfdetr_windows::RF_DETR_COMMIT.to_string();
                let src = crate::rfdetr_windows::RF_DETR_COMMIT.to_string();
                Ok((ver, src, Vec::new()))
            }
            "onnxruntime" => {
                let ver = crate::onnx_windows::ONNX_RUNTIME_COMMIT.to_string();
                let src = crate::onnx_windows::ONNX_RUNTIME_COMMIT.to_string();
                Ok((ver, src, Vec::new()))
            }
            "nvattest" => {
                let ver = crate::nvattest_windows::NVATTEST_SDK_REVISION.to_string();
                let src = crate::nvattest_windows::NVATTEST_SDK_REVISION.to_string();
                Ok((ver, src, Vec::new()))
            }
            "restic" => {
                let ver = crate::produce::windows_archives::RESTIC.version.to_string();
                let src = "https://github.com/restic/restic/releases/download/v0.19.0".to_string();
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "restic_0.19.0_windows_amd64.zip".to_string(),
                        sha256: crate::produce::windows_archives::RESTIC.sha256.to_string(),
                    }],
                ))
            }
            "rclone" => {
                let ver = crate::produce::windows_archives::RCLONE.version.to_string();
                let src = "https://downloads.rclone.org/v1.74.4".to_string();
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "rclone-v1.74.4-windows-amd64.zip".to_string(),
                        sha256: crate::produce::windows_archives::RCLONE.sha256.to_string(),
                    }],
                ))
            }
            "msvc-runtime" => {
                let ver = crate::produce::windows_archives::MSVC.version.to_string();
                let src = crate::produce::windows_archives::MSVC
                    .url
                    .rsplit_once('/')
                    .map(|(base, _)| base)
                    .unwrap_or(crate::produce::windows_archives::MSVC.url)
                    .to_string();
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "Microsoft.VC.14.44.17.14.CRT.Redist.X64.base.vsix".to_string(),
                        sha256: crate::produce::windows_archives::MSVC.sha256.to_string(),
                    }],
                ))
            }
            "pdfium" => {
                let ver = crate::pdfium::RELEASE_TAG.to_string();
                let src = crate::pdfium::RELEASE_URL.to_string();
                let spec = crate::pdfium::TARGETS
                    .iter()
                    .find(|s| s.key == target)
                    .ok_or_else(|| {
                        EvidenceError::new(format!("missing pdfium spec for {target}"))
                    })?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: spec.archive_name.to_string(),
                        sha256: spec.archive_sha256.to_string(),
                    }],
                ))
            }
            "wespeaker-model" => {
                let ver = "resnet34-256".to_string();
                let src = "core/models/assets/wespeaker-resnet34-256.onnx".to_string();
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-transcribe/src/model_assets.rs"),
                    "WESPEAKER_RESNET34_SHA256",
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "wespeaker-resnet34-256.onnx".to_string(),
                        sha256: sha,
                    }],
                ))
            }
            "pyannote-model" => {
                let ver = "3.0".to_string();
                let src = "core/models/assets/pyannote-segmentation-3.0.onnx".to_string();
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-transcribe/src/model_assets.rs"),
                    "PYANNOTE_SEGMENTATION_SHA256",
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "pyannote-segmentation-3.0.onnx".to_string(),
                        sha256: sha,
                    }],
                ))
            }
            "silero-vad-model" => {
                let ver = "v6".to_string();
                let src = "core/models/assets/silero_vad_v6.onnx".to_string();
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-transcribe/src/model_assets.rs"),
                    "SILERO_VAD_V6_SHA256",
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "silero_vad_v6.onnx".to_string(),
                        sha256: sha,
                    }],
                ))
            }
            "rfdetr-model" => {
                let ver = "nano-f16".to_string();
                let src = "core/models/assets/rfdetr/rfdetr-nano-f16.gguf".to_string();
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-local/src/install/rfdetr_install.rs"),
                    "RFDETR_MODEL_SHA256",
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "rfdetr-nano-f16.gguf".to_string(),
                        sha256: sha,
                    }],
                ))
            }
            other => Err(EvidenceError::new(format!(
                "unnamed-input: {target} {other}"
            ))),
        }
    } else {
        // POSIX bundled
        match id {
            "onnxruntime" => {
                let spec = crate::onnx_runtime::TARGETS
                    .iter()
                    .find(|s| s.key == target)
                    .ok_or_else(|| {
                        EvidenceError::new(format!("missing onnxruntime spec for {target}"))
                    })?;
                let ver = "1.25.0".to_string();
                let wheel_filename = spec.wheel_url.rsplit('/').next().unwrap_or_default();
                let src = spec
                    .wheel_url
                    .strip_suffix(&format!("/{wheel_filename}"))
                    .unwrap_or(spec.wheel_url)
                    .to_string();
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: wheel_filename.to_string(),
                        sha256: spec.wheel_sha256.to_string(),
                    }],
                ))
            }
            "pdfium" => {
                let ver = crate::pdfium::RELEASE_TAG.to_string();
                let src = crate::pdfium::RELEASE_URL.to_string();
                let spec = crate::pdfium::TARGETS
                    .iter()
                    .find(|s| s.key == target)
                    .ok_or_else(|| {
                        EvidenceError::new(format!("missing pdfium spec for {target}"))
                    })?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: spec.archive_name.to_string(),
                        sha256: spec.archive_sha256.to_string(),
                    }],
                ))
            }
            "wespeaker-model" => {
                let ver = "resnet34-256".to_string();
                let src = "core/models/assets/wespeaker-resnet34-256.onnx".to_string();
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-transcribe/src/model_assets.rs"),
                    "WESPEAKER_RESNET34_SHA256",
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "wespeaker-resnet34-256.onnx".to_string(),
                        sha256: sha,
                    }],
                ))
            }
            "pyannote-model" => {
                let ver = "3.0".to_string();
                let src = "core/models/assets/pyannote-segmentation-3.0.onnx".to_string();
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-transcribe/src/model_assets.rs"),
                    "PYANNOTE_SEGMENTATION_SHA256",
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "pyannote-segmentation-3.0.onnx".to_string(),
                        sha256: sha,
                    }],
                ))
            }
            "silero-vad-model" => {
                let ver = "v6".to_string();
                let src = "core/models/assets/silero_vad_v6.onnx".to_string();
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-transcribe/src/model_assets.rs"),
                    "SILERO_VAD_V6_SHA256",
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "silero_vad_v6.onnx".to_string(),
                        sha256: sha,
                    }],
                ))
            }
            "rfdetr-model" => {
                let ver = "nano-f16".to_string();
                let src = "core/models/assets/rfdetr/rfdetr-nano-f16.gguf".to_string();
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-local/src/install/rfdetr_install.rs"),
                    "RFDETR_MODEL_SHA256",
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: "rfdetr-nano-f16.gguf".to_string(),
                        sha256: sha,
                    }],
                ))
            }
            "rfdetr-engine" => {
                let ver = "v0.1.0-solpbc.5".to_string();
                let src = crate::rfdetr_windows::RF_DETR_REPOSITORY
                    .trim_end_matches(".git")
                    .to_string();
                let (const_name, filename) = match target {
                    "linux-x86_64" => (
                        "RFDETR_ENGINE_LINUX_CPU_X64_TARBALL_SHA256",
                        "rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz",
                    ),
                    "linux-aarch64" => (
                        "RFDETR_ENGINE_LINUX_CPU_ARM64_TARBALL_SHA256",
                        "rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-arm64.tar.gz",
                    ),
                    "macos-arm64" => (
                        "RFDETR_ENGINE_MACOS_METAL_ARM64_TARBALL_SHA256",
                        "rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz",
                    ),
                    _ => {
                        return Err(EvidenceError::new(format!("unknown target {target}")));
                    }
                };
                let sha = read_digest_const(
                    &checkout.join("core/crates/solstone-core-local/src/install/rfdetr_install.rs"),
                    const_name,
                )?;
                Ok((
                    ver,
                    src,
                    vec![InputRef {
                        name: filename.to_string(),
                        sha256: sha,
                    }],
                ))
            }
            other => Err(EvidenceError::new(format!(
                "unnamed-input: {target} {other}"
            ))),
        }
    }
}

/// A pinned entry names its own identity: the catalog row its input pins, or,
/// for the nvattest verifier, the authority's source for that platform. The
/// version and source come from those committed records, never from the bytes.
fn pinned_identity(
    input: &crate::inventory::PinnedInput,
    id: &str,
    checkout: &Path,
) -> Result<(String, String, Vec<InputRef>), EvidenceError> {
    match input {
        crate::inventory::PinnedInput::CatalogCommitted { unit, filename, .. }
        | crate::inventory::PinnedInput::CatalogAcquired { unit, filename } => {
            let artifact = solstone_core_assets::catalog()
                .iter()
                .find(|a| a.unit == unit && a.filename == filename)
                .ok_or_else(|| EvidenceError::new(format!("unnamed-input: {id} {filename}")))?;
            Ok((
                artifact.version.to_string(),
                catalog_source(artifact, id, checkout)?,
                vec![InputRef {
                    name: filename.clone(),
                    sha256: artifact.sha256.to_string(),
                }],
            ))
        }
        crate::inventory::PinnedInput::AuthorityCommitted { platform, .. } => {
            let authority = solstone_core_nvattest_authority::parse(
                solstone_core_nvattest_authority::AUTHORITY_JSON,
            )
            .map_err(|e| EvidenceError::new(e.to_string()))?;
            let target = authority
                .targets
                .get(platform)
                .ok_or_else(|| EvidenceError::new(format!("unnamed-input: {id} {platform}")))?;
            Ok((
                target.source.version.clone(),
                target.source.fork_commit.clone(),
                vec![InputRef {
                    name: target.artifact.name.clone(),
                    sha256: target.artifact.sha256.clone(),
                }],
            ))
        }
        crate::inventory::PinnedInput::Inline { .. } => Err(EvidenceError::new(format!(
            "unnamed-input: {id} inline pin"
        ))),
    }
}

fn entry_targets(entry: &crate::inventory::Entry) -> &[String] {
    match entry {
        crate::inventory::Entry::WindowsBuildEvidence { targets, .. }
        | crate::inventory::Entry::WindowsNative { targets, .. }
        | crate::inventory::Entry::Bin { targets, .. }
        | crate::inventory::Entry::Launcher { targets, .. }
        | crate::inventory::Entry::ModelAsset { targets, .. }
        | crate::inventory::Entry::OnnxRuntime { targets, .. }
        | crate::inventory::Entry::Pdfium { targets, .. }
        | crate::inventory::Entry::Copy { targets, .. }
        | crate::inventory::Entry::PinnedNative { targets, .. }
        | crate::inventory::Entry::PinnedMembers { targets, .. }
        | crate::inventory::Entry::LicenceTree { targets, .. } => targets,
    }
}

/// Names a pinned-members entry's input by its pin alone: the catalog row, the
/// nvattest authority, or the inline digest. It never reads the input bytes.
fn pinned_input_ref(input: &crate::inventory::PinnedInput) -> Option<InputRef> {
    match input {
        crate::inventory::PinnedInput::Inline { source, digest } => Some(InputRef {
            name: Path::new(source)
                .file_name()?
                .to_string_lossy()
                .into_owned(),
            sha256: digest.clone(),
        }),
        crate::inventory::PinnedInput::CatalogCommitted { unit, filename, .. }
        | crate::inventory::PinnedInput::CatalogAcquired { unit, filename } => {
            let artifact = solstone_core_assets::catalog()
                .iter()
                .find(|a| a.unit == unit && a.filename == filename)?;
            Some(InputRef {
                name: filename.clone(),
                sha256: artifact.sha256.to_string(),
            })
        }
        crate::inventory::PinnedInput::AuthorityCommitted { platform, .. } => {
            let authority = solstone_core_nvattest_authority::parse(
                solstone_core_nvattest_authority::AUTHORITY_JSON,
            )
            .ok()?;
            let spec =
                solstone_core_nvattest_authority::artifact_spec(&authority, platform).ok()?;
            Some(InputRef {
                name: spec.name,
                sha256: spec.sha256,
            })
        }
    }
}

pub fn name_receipt_inputs(
    inventory: &Inventory,
    target: &str,
) -> Result<Vec<NamedInput>, EvidenceError> {
    let mut result = Vec::new();
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for entry in &inventory.entry {
        if !entry_targets(entry).iter().any(|t| t == target) {
            continue;
        }
        if entry.class() != Some(crate::inventory::DeliveryClass::Component) {
            continue;
        }
        let Some(id) = crate::inventory::entry_component_id(entry) else {
            continue;
        };
        let dest = entry_dest_str(entry);
        let inputs = match entry {
            crate::inventory::Entry::ModelAsset {
                source,
                digest_source,
                digest_const,
                ..
            } => {
                let name = Path::new(source)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let sha256 = read_digest_const(&repo_root.join(digest_source), digest_const)?;
                vec![InputRef { name, sha256 }]
            }
            crate::inventory::Entry::OnnxRuntime { .. } => {
                let spec = crate::onnx_runtime::TARGETS
                    .iter()
                    .find(|s| s.key == target)
                    .ok_or_else(|| {
                        EvidenceError::new(format!("unnamed-input: {target} {id} {dest}"))
                    })?;
                let name = spec
                    .wheel_url
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                vec![InputRef {
                    name,
                    sha256: spec.wheel_sha256.to_string(),
                }]
            }
            crate::inventory::Entry::Pdfium { .. } => {
                let spec = crate::pdfium::TARGETS
                    .iter()
                    .find(|s| s.key == target)
                    .ok_or_else(|| {
                        EvidenceError::new(format!("unnamed-input: {target} {id} {dest}"))
                    })?;
                vec![InputRef {
                    name: spec.archive_name.to_string(),
                    sha256: spec.archive_sha256.to_string(),
                }]
            }
            crate::inventory::Entry::PinnedMembers { input, .. } => {
                vec![pinned_input_ref(input).ok_or_else(|| {
                    EvidenceError::new(format!("unnamed-input: {target} {id} {dest}"))
                })?]
            }
            crate::inventory::Entry::WindowsNative { component, .. } => match *component {
                crate::inventory::WindowsNativeComponent::Ced
                | crate::inventory::WindowsNativeComponent::Rfdetr
                | crate::inventory::WindowsNativeComponent::Onnx => {
                    return Err(EvidenceError::new(format!(
                        "unnamed-input: {target} {id} {dest}"
                    )));
                }
                crate::inventory::WindowsNativeComponent::Llama => {
                    if id == "llama-server" {
                        vec![InputRef {
                            name: "sources/llama-windows.tar.gz".to_string(),
                            sha256: crate::llama_windows_source::SOURCE_SHA256.to_string(),
                        }]
                    } else if id == "vulkan-loader" {
                        vec![InputRef {
                            name: "tools/vulkan-sdk-windows-x64.exe".to_string(),
                            sha256: crate::llama_windows_source::SDK_SHA256.to_string(),
                        }]
                    } else {
                        return Err(EvidenceError::new(format!(
                            "unnamed-input: {target} {id} {dest}"
                        )));
                    }
                }
                crate::inventory::WindowsNativeComponent::Parakeet => {
                    if dest.ends_with(".gguf") {
                        vec![InputRef {
                            name: crate::parakeet_windows::PARAKEET_MODEL_FILENAME.to_string(),
                            sha256: crate::parakeet_windows::PARAKEET_MODEL_SHA256.to_string(),
                        }]
                    } else {
                        return Err(EvidenceError::new(format!(
                            "unnamed-input: {target} {id} {dest}"
                        )));
                    }
                }
                crate::inventory::WindowsNativeComponent::Nvattest => {
                    if dest.ends_with(".pem") {
                        let pins = crate::nvattest_windows::production_pins();
                        vec![InputRef {
                            name: "ca-bundle.pem".to_string(),
                            sha256: pins.ca_bundle.sha256.to_string(),
                        }]
                    } else if id == "nvattest" {
                        let pins = crate::nvattest_windows::production_pins();
                        vec![InputRef {
                            name: crate::nvattest_windows::NVATTEST_SOURCE_INPUT_LABEL.to_string(),
                            sha256: pins.source_archive.sha256.to_string(),
                        }]
                    } else {
                        return Err(EvidenceError::new(format!(
                            "unnamed-input: {target} {id} {dest}"
                        )));
                    }
                }
                crate::inventory::WindowsNativeComponent::Pdfium => {
                    let spec = crate::pdfium::TARGETS
                        .iter()
                        .find(|s| s.key == "windows-x86_64")
                        .ok_or_else(|| {
                            EvidenceError::new(format!("unnamed-input: {target} {id} {dest}"))
                        })?;
                    vec![InputRef {
                        name: spec.archive_name.to_string(),
                        sha256: spec.archive_sha256.to_string(),
                    }]
                }
                crate::inventory::WindowsNativeComponent::Restic => {
                    vec![InputRef {
                        name: "restic_0.19.0_windows_amd64.zip".to_string(),
                        sha256: crate::produce::windows_archives::RESTIC.sha256.to_string(),
                    }]
                }
                crate::inventory::WindowsNativeComponent::Rclone => {
                    vec![InputRef {
                        name: "rclone-v1.74.4-windows-amd64.zip".to_string(),
                        sha256: crate::produce::windows_archives::RCLONE.sha256.to_string(),
                    }]
                }
                crate::inventory::WindowsNativeComponent::Msvc => {
                    vec![InputRef {
                        name: "Microsoft.VC.14.44.17.14.CRT.Redist.X64.base.vsix".to_string(),
                        sha256: crate::produce::windows_archives::MSVC.sha256.to_string(),
                    }]
                }
                _ => {
                    return Err(EvidenceError::new(format!(
                        "unnamed-input: {target} {id} {dest}"
                    )));
                }
            },
            _ => {
                return Err(EvidenceError::new(format!(
                    "unnamed-input: {target} {id} {dest}"
                )));
            }
        };
        for input in inputs {
            result.push(NamedInput {
                target: target.to_string(),
                id: id.to_string(),
                dest: dest.clone(),
                name: input.name,
                sha256: input.sha256,
            });
        }
    }
    Ok(result)
}

pub fn render_runtime_rows(
    target: &str,
    extra: &[InjectedFetch],
) -> Result<Vec<ComponentRow>, EvidenceError> {
    let fetches = solstone_core_assets::runtime_fetch_set(target)
        .map_err(|e| EvidenceError::new(e.to_string()))?;

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");

    let mut by_unit: BTreeMap<String, Vec<solstone_core_assets::RuntimeFetch>> = BTreeMap::new();
    for f in fetches {
        let u_id = solstone_core_assets::unit_id(f.unit()).expect("fetch unit has an id");
        by_unit.entry(u_id.to_string()).or_default().push(f);
    }

    let mut rows = Vec::new();
    for (id, group) in by_unit {
        let first = &group[0];
        let (version, source, mut inputs) = if id == "nvattest" {
            let authority = solstone_core_nvattest_authority::parse(
                solstone_core_nvattest_authority::AUTHORITY_JSON,
            )
            .map_err(|e| EvidenceError::new(e.to_string()))?;
            let spec_platform = target;
            let tgt = authority
                .targets
                .get(spec_platform)
                .ok_or_else(|| EvidenceError::new(format!("unknown-target: {target}")))?;
            let spec = solstone_core_nvattest_authority::artifact_spec(&authority, spec_platform)
                .map_err(|e| EvidenceError::new(e.to_string()))?;
            let version = tgt.source.version.clone();
            let source = tgt.source.fork_commit.clone();
            let inputs = vec![InputRef {
                name: spec.name,
                sha256: spec.sha256,
            }];
            (version, source, inputs)
        } else {
            let cat = solstone_core_assets::catalog();
            let mut ver = String::new();
            let mut inputs = Vec::new();
            for item in &group {
                let cat_artifact = cat
                    .iter()
                    .find(|a| a.origin_key == item.origin_key())
                    .ok_or_else(|| {
                        EvidenceError::new(format!(
                            "catalog row not found for origin key {}",
                            item.origin_key()
                        ))
                    })?;
                if ver.is_empty() {
                    ver = cat_artifact.version.to_string();
                } else if ver != cat_artifact.version {
                    return Err(EvidenceError::new(format!("unit-version-mismatch: {id}")));
                }
                inputs.push(InputRef {
                    name: cat_artifact.filename.to_string(),
                    sha256: item.sha256().to_string(),
                });
            }
            let first_artifact = cat
                .iter()
                .find(|a| a.origin_key == first.origin_key())
                .expect("first artifact in catalog");
            let source = catalog_source(first_artifact, &id, &repo_root)?;
            (ver, source, inputs)
        };

        if source.contains("updates.solstone.app") {
            return Err(EvidenceError::new(format!("owner-origin-source: {id}")));
        }

        check_residual(target, &id, "source", &source)?;

        inputs.sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.name, &b.name));
        inputs.dedup_by(|a, b| a.name == b.name);

        rows.push(ComponentRow {
            id,
            version,
            delivery: DeliveryKind::RuntimeDownloaded,
            source,
            inputs,
            members: Vec::new(),
        });
    }

    let mut by_injected: BTreeMap<String, Vec<&InjectedFetch>> = BTreeMap::new();
    for f in extra {
        let u_id = solstone_core_assets::unit_id(&f.unit).expect("fetch unit has an id");
        by_injected.entry(u_id.to_string()).or_default().push(f);
    }
    for (id, group) in by_injected {
        let version = group[0].version.clone();
        for item in &group {
            if item.version != version {
                return Err(EvidenceError::new(format!("unit-version-mismatch: {id}")));
            }
        }
        let source = group[0].source.clone();
        for item in &group {
            if item.source != source {
                return Err(EvidenceError::new(format!("unit-source-mismatch: {id}")));
            }
        }
        let mut inputs: Vec<InputRef> = group
            .iter()
            .map(|f| InputRef {
                name: f.filename.clone(),
                sha256: f.sha256.clone(),
            })
            .collect();
        inputs.sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.name, &b.name));
        inputs.dedup_by(|a, b| a.name == b.name);
        rows.push(ComponentRow {
            id: id.clone(),
            version,
            delivery: DeliveryKind::RuntimeDownloaded,
            source,
            inputs,
            members: Vec::new(),
        });
    }

    rows.sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.id, &b.id));
    Ok(rows)
}

pub fn render_posix_evidence(
    request: &PosixEvidenceRequest<'_>,
) -> Result<RenderedEvidence, EvidenceError> {
    solstone_core_installed_payload::verify_installed_package(
        request.stage,
        request.version,
        request.target,
    )
    .map_err(|e| EvidenceError::new(format!("verification-failed: {e}")))?;

    let manifest_path = request
        .stage
        .join(solstone_core_installed_payload::INSTALLED_PAYLOAD_MANIFEST);
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| EvidenceError::new(format!("read manifest: {e}")))?;
    let parsed: serde_json::Value = serde_json::from_str(&manifest_text)
        .map_err(|e| EvidenceError::new(format!("parse manifest: {e}")))?;

    let manifest_version = parsed["version"].as_str().unwrap_or_default();
    if manifest_version != request.version {
        return Err(EvidenceError::new(format!(
            "version-mismatch: requested {} != manifest {}",
            request.version, manifest_version
        )));
    }
    let manifest_target = parsed["target"].as_str().unwrap_or_default();
    if manifest_target != request.target {
        return Err(EvidenceError::new(format!(
            "target-mismatch: requested {} != manifest {}",
            request.target, manifest_target
        )));
    }

    let installed_payload_sha256 = crate::digest::sha256_hex(manifest_text.as_bytes());
    let release_manifest_bytes = std::fs::read(request.release_manifest)
        .map_err(|e| EvidenceError::new(format!("read release manifest: {e}")))?;
    let release_manifest_sha256 = crate::digest::sha256_hex(&release_manifest_bytes);

    let runtime_rows = render_runtime_rows(request.target, &request.injected)?;

    let mut bundled_rows: BTreeMap<String, ComponentRow> = BTreeMap::new();
    let mut records: Vec<ProvenanceRecord> = Vec::new();

    let files_arr = parsed["files"]
        .as_array()
        .ok_or_else(|| EvidenceError::new("manifest missing files array"))?;

    for file_val in files_arr {
        let path = file_val["path"].as_str().unwrap_or_default();
        let final_sha256 = file_val["sha256"].as_str().unwrap_or_default();

        let entry_opt = request.inventory.entry.iter().find(|e| {
            entry_targets(e).iter().any(|t| t == request.target) && entry_dest_matches(e, path)
        });

        let Some(entry) = entry_opt else {
            continue;
        };

        if entry.class() != Some(crate::inventory::DeliveryClass::Component) {
            continue;
        }

        let Some(id) = crate::inventory::entry_component_id(entry) else {
            continue;
        };

        check_member_path(path)?;

        let staged_file = request.stage.join(path);
        let actual_bytes = std::fs::read(&staged_file)
            .map_err(|e| EvidenceError::new(format!("read staged file {path}: {e}")))?;
        let actual_sha = crate::digest::sha256_hex(&actual_bytes);
        if actual_sha != final_sha256 {
            return Err(EvidenceError::new(format!("digest-disagreement: {path}")));
        }

        let (row_version, row_source, row_inputs) = match entry {
            crate::inventory::Entry::PinnedMembers { input, .. }
            | crate::inventory::Entry::PinnedNative { input, .. } => {
                pinned_identity(input, id, request.checkout)?
            }
            _ => bundled_identity(request.checkout, request.target, id)?,
        };

        let pre_signing = request.pre_signing.get(path).cloned();

        let is_reseal = match entry {
            crate::inventory::Entry::ModelAsset {
                archive_slot: Some(slot),
                source,
                ..
            } if slot.id == "rfdetr-macos-metal-arm64" => Some((slot, source)),
            _ => None,
        };

        if let Some((slot, source)) = is_reseal {
            let input_tar = request
                .archives
                .iter()
                .find(|(name, _)| {
                    name == source
                        || Path::new(source)
                            .file_name()
                            .map(|f| f == name.as_str())
                            .unwrap_or(false)
                })
                .ok_or_else(|| {
                    EvidenceError::new(format!("missing input archive for reseal slot {}", slot.id))
                })?;

            let (input_name, input_bytes) = input_tar;
            let input_members =
                crate::pinned_stage::collect_archive_members(input_name, input_bytes, input_name)
                    .map_err(|e| EvidenceError::new(e.to_string()))?;

            let sealed_members =
                crate::pinned_stage::collect_archive_members(path, &actual_bytes, path)
                    .map_err(|e| EvidenceError::new(e.to_string()))?;

            let mut needed_input = std::collections::BTreeSet::new();
            for (m_path, kind) in &input_members {
                if matches!(kind, crate::pinned_stage::ArchiveEntryKind::Regular) {
                    needed_input.insert(m_path.as_str());
                }
            }
            let mut needed_sealed = std::collections::BTreeSet::new();
            for (m_path, kind) in &sealed_members {
                if matches!(kind, crate::pinned_stage::ArchiveEntryKind::Regular) {
                    needed_sealed.insert(m_path.as_str());
                }
            }

            let extracted_input = crate::pinned_stage::extract_needed_members(
                input_name,
                input_bytes,
                input_name,
                &needed_input,
            )
            .map_err(|e| EvidenceError::new(e.to_string()))?;

            let extracted_sealed = crate::pinned_stage::extract_needed_members(
                path,
                &actual_bytes,
                path,
                &needed_sealed,
            )
            .map_err(|e| EvidenceError::new(e.to_string()))?;

            let exec_paths: std::collections::BTreeSet<&str> =
                slot.executables.iter().map(|e| e.path.as_str()).collect();

            // Check non-executables for byte equality
            for (inner, in_kind) in &input_members {
                let Some(seal_kind) = sealed_members.get(inner) else {
                    return Err(EvidenceError::new(format!("reseal-mismatch: {inner}")));
                };
                if !exec_paths.contains(inner.as_str()) {
                    match (in_kind, seal_kind) {
                        (
                            crate::pinned_stage::ArchiveEntryKind::Regular,
                            crate::pinned_stage::ArchiveEntryKind::Regular,
                        ) => {
                            let inp_b = extracted_input.get(inner);
                            let seal_b = extracted_sealed.get(inner);
                            if inp_b != seal_b {
                                return Err(EvidenceError::new(format!(
                                    "reseal-mismatch: {inner}"
                                )));
                            }
                        }
                        (
                            crate::pinned_stage::ArchiveEntryKind::Symlink(t1),
                            crate::pinned_stage::ArchiveEntryKind::Symlink(t2),
                        ) => {
                            if t1 != t2 {
                                return Err(EvidenceError::new(format!(
                                    "reseal-mismatch: {inner}"
                                )));
                            }
                        }
                        _ => {
                            return Err(EvidenceError::new(format!("reseal-mismatch: {inner}")));
                        }
                    }
                }
            }
            for inner in sealed_members.keys() {
                if !input_members.contains_key(inner) {
                    return Err(EvidenceError::new(format!("reseal-mismatch: {inner}")));
                }
            }

            let input = row_inputs.first().cloned().unwrap_or_else(|| InputRef {
                name: Path::new(source)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                sha256: crate::digest::sha256_hex(input_bytes),
            });

            for exe in &slot.executables {
                let inp_b = extracted_input
                    .get(&exe.path)
                    .ok_or_else(|| EvidenceError::new(format!("reseal-mismatch: {}", exe.path)))?;
                let seal_b = extracted_sealed
                    .get(&exe.path)
                    .ok_or_else(|| EvidenceError::new(format!("reseal-mismatch: {}", exe.path)))?;
                let inp_sha = crate::digest::sha256_hex(inp_b);
                let seal_sha = crate::digest::sha256_hex(seal_b);
                records.push(ProvenanceRecord {
                    id: id.to_string(),
                    path: path.to_string(),
                    input: input.clone(),
                    inner_path: Some(exe.path.clone()),
                    alias: None,
                    extracted_sha256: Some(inp_sha.clone()),
                    pre_signing_sha256: Some(inp_sha),
                    final_sha256: seal_sha,
                });
            }

            let row = bundled_rows
                .entry(id.to_string())
                .or_insert_with(|| ComponentRow {
                    id: id.to_string(),
                    version: row_version,
                    delivery: DeliveryKind::Bundled,
                    source: row_source,
                    inputs: row_inputs,
                    members: Vec::new(),
                });
            row.members.push(MemberRef {
                path: path.to_string(),
                sha256: final_sha256.to_string(),
            });

            continue;
        }

        // Case 2: Archive member extraction. A pinned entry reads only its own
        // input: a single-file input (a bz2 or a model) answers to the empty
        // member name, so any other archive would also match it.
        let own_input_only = matches!(
            entry,
            crate::inventory::Entry::PinnedMembers { .. }
                | crate::inventory::Entry::PinnedNative { .. }
        );
        let mut extracted_info = None;
        for (archive_name, archive_bytes) in request
            .archives
            .iter()
            .filter(|(name, _)| !own_input_only || row_inputs.iter().any(|i| i.name == *name))
        {
            if let Ok(members) = crate::pinned_stage::collect_archive_members(
                archive_name,
                archive_bytes,
                archive_name,
            ) {
                let key_opt = if members.contains_key(path) {
                    Some(path.to_string())
                } else if let crate::inventory::Entry::PinnedMembers { staged, .. } = entry {
                    staged
                        .iter()
                        .find(|m| m.dest == path && members.contains_key(&m.relpath))
                        .map(|m| m.relpath.clone())
                } else if let Some(fname) = Path::new(path).file_name().and_then(|f| f.to_str()) {
                    if members.contains_key(fname) {
                        Some(fname.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(key) = key_opt {
                    let kind = &members[&key];
                    match kind {
                        crate::pinned_stage::ArchiveEntryKind::Symlink(target) => {
                            let resolved = crate::pinned_stage::resolve_symlink(
                                archive_name,
                                &key,
                                target,
                                &members,
                            )
                            .map_err(|e| EvidenceError::new(e.to_string()))?;

                            let mut needed = std::collections::BTreeSet::new();
                            needed.insert(resolved.as_str());
                            let extracted = crate::pinned_stage::extract_needed_members(
                                archive_name,
                                archive_bytes,
                                archive_name,
                                &needed,
                            )
                            .map_err(|e| EvidenceError::new(e.to_string()))?;
                            let reg_b = extracted.get(&resolved).ok_or_else(|| {
                                EvidenceError::new(format!(
                                    "missing extracted member {resolved} in {archive_name}"
                                ))
                            })?;
                            let reg_sha = crate::digest::sha256_hex(reg_b);
                            extracted_info =
                                Some((archive_name.as_str(), Some(key), Some(resolved), reg_sha));
                            break;
                        }
                        crate::pinned_stage::ArchiveEntryKind::Regular => {
                            let mut needed = std::collections::BTreeSet::new();
                            needed.insert(key.as_str());
                            let extracted = crate::pinned_stage::extract_needed_members(
                                archive_name,
                                archive_bytes,
                                archive_name,
                                &needed,
                            )
                            .map_err(|e| EvidenceError::new(e.to_string()))?;
                            let reg_b = extracted.get(&key).ok_or_else(|| {
                                EvidenceError::new(format!(
                                    "missing extracted member {key} in {archive_name}"
                                ))
                            })?;
                            let reg_sha = crate::digest::sha256_hex(reg_b);
                            extracted_info = Some((archive_name.as_str(), None, None, reg_sha));
                            break;
                        }
                        crate::pinned_stage::ArchiveEntryKind::NonRegular => {}
                    }
                }
            }
        }

        let mut extracted_sha256 = None;
        let mut inner_path = None;
        let mut alias = None;

        if let Some((_archive_name, symlink_alias, symlink_inner, reg_sha)) = &extracted_info {
            extracted_sha256 = Some(reg_sha.clone());
            alias = symlink_alias.clone();
            inner_path = symlink_inner.clone();

            if let Some(ref pre) = pre_signing {
                if pre != reg_sha {
                    return Err(EvidenceError::new(format!("digest-disagreement: {path}")));
                }
                if pre == final_sha256 {
                    return Err(EvidenceError::new(format!("digest-disagreement: {path}")));
                }
            } else {
                if reg_sha != final_sha256 {
                    return Err(EvidenceError::new(format!("digest-disagreement: {path}")));
                }
            }
        } else {
            // Case 3: Otherwise the member is verbatim.
            let file_name = Path::new(path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(path);

            let is_extracted_lib =
                path.contains(".so") || path.contains(".dylib") || path.contains(".dll");

            for inp in &row_inputs {
                let is_archive_or_wheel = inp.name.ends_with(".tar.gz")
                    || inp.name.ends_with(".tar.xz")
                    || inp.name.ends_with(".zip")
                    || inp.name.ends_with(".whl");

                if !(is_archive_or_wheel && is_extracted_lib) && inp.name == file_name {
                    let check_sha = pre_signing.as_deref().unwrap_or(final_sha256);
                    if check_sha != inp.sha256 {
                        return Err(EvidenceError::new(format!("digest-disagreement: {path}")));
                    }
                    if let Some(ref pre) = pre_signing
                        && pre == final_sha256
                    {
                        return Err(EvidenceError::new(format!("digest-disagreement: {path}")));
                    }
                }
            }
        }

        let file_name = Path::new(path)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(path);
        let input = row_inputs
            .iter()
            .find(|inp| {
                inp.name == file_name
                    || extracted_info
                        .as_ref()
                        .map(|(arch_name, _, _, _)| inp.name == *arch_name)
                        .unwrap_or(false)
            })
            .cloned()
            .or_else(|| row_inputs.first().cloned())
            .unwrap_or(InputRef {
                name: file_name.to_string(),
                sha256: final_sha256.to_string(),
            });

        records.push(ProvenanceRecord {
            id: id.to_string(),
            path: path.to_string(),
            input,
            inner_path,
            alias,
            extracted_sha256,
            pre_signing_sha256: pre_signing,
            final_sha256: final_sha256.to_string(),
        });

        let row = bundled_rows
            .entry(id.to_string())
            .or_insert_with(|| ComponentRow {
                id: id.to_string(),
                version: row_version,
                delivery: DeliveryKind::Bundled,
                source: row_source,
                inputs: row_inputs,
                members: Vec::new(),
            });
        row.members.push(MemberRef {
            path: path.to_string(),
            sha256: final_sha256.to_string(),
        });
    }

    for rt in &runtime_rows {
        if bundled_rows.contains_key(&rt.id) {
            return Err(EvidenceError::new(format!("mixed-delivery: {}", rt.id)));
        }
    }

    let mut all_components: Vec<ComponentRow> = bundled_rows.into_values().collect();
    for row in &mut all_components {
        row.members
            .sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.path, &b.path));
        row.inputs
            .sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.name, &b.name));
    }
    all_components.extend(runtime_rows);
    all_components.sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.id, &b.id));

    records.sort_by(|a, b| {
        let id_cmp = solstone_core_assets::cmp_utf16_code_units(&a.id, &b.id);
        if id_cmp.is_eq() {
            solstone_core_assets::cmp_utf16_code_units(&a.path, &b.path)
        } else {
            id_cmp
        }
    });

    let comp_file = ComponentFile {
        product: solstone_core_installed_payload::PRODUCT.to_string(),
        version: request.version.to_string(),
        target: request.target.to_string(),
        components: all_components,
    };
    let prov_file = ProvenanceFile {
        product: solstone_core_installed_payload::PRODUCT.to_string(),
        version: request.version.to_string(),
        target: request.target.to_string(),
        basis: "measured".to_string(),
        manifests: ProvenanceManifests {
            installed_payload_sha256: Some(installed_payload_sha256),
            release_manifest_sha256: Some(release_manifest_sha256),
            payload_manifest_sha256: None,
        },
        records,
    };

    let comp_json = serde_json::to_string_pretty(&comp_file)
        .map_err(|e| EvidenceError::new(e.to_string()))?
        + "\n";
    if comp_json.len() >= 1_048_576 {
        return Err(EvidenceError::new(format!(
            "components-too-large: {}",
            comp_json.len()
        )));
    }
    let prov_json = serde_json::to_string_pretty(&prov_file)
        .map_err(|e| EvidenceError::new(e.to_string()))?
        + "\n";

    Ok(RenderedEvidence {
        components: comp_json,
        provenance: prov_json,
    })
}

pub fn render_windows_component_evidence(
    payload_root: &Path,
    observed_head: &str,
    observed_dirty: bool,
    requested_version: Option<&str>,
    out_dir: &Path,
) -> Result<(), EvidenceError> {
    if observed_dirty {
        return Err(EvidenceError::new("windows-evidence-dirty-tree"));
    }
    let verified =
        solstone_core_installed_payload::windows_payload::verify_windows_payload(payload_root)
            .map_err(|e| EvidenceError::new(format!("verification-failed: {e}")))?;

    if verified.manifest().source_commit != observed_head {
        return Err(EvidenceError::new(format!(
            "windows-evidence-commit-mismatch: payload {} != observed {}",
            verified.manifest().source_commit,
            observed_head
        )));
    }

    if verified.manifest().target != "windows-x86_64" {
        return Err(EvidenceError::new(format!(
            "target-mismatch: manifest {}",
            verified.manifest().target
        )));
    }

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let workspace_ver = crate::produce::workspace_version(&repo_root.join("core/Cargo.toml"))
        .map_err(|e| EvidenceError::new(e.to_string()))?;

    if let Some(req) = requested_version
        && req != workspace_ver
    {
        return Err(EvidenceError::new(format!(
            "windows-evidence-version-mismatch: workspace {workspace_ver} != payload {req}"
        )));
    }

    let version = workspace_ver;
    let target = "windows-x86_64";

    let inv_path = repo_root.join("core/distribution/inventory.toml");
    let inventory = crate::inventory::load_inventory(&inv_path)
        .map_err(|e| EvidenceError::new(e.to_string()))?;

    let manifest_path = payload_root
        .join(solstone_core_installed_payload::windows_payload::WINDOWS_PAYLOAD_MANIFEST);
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|e| EvidenceError::new(format!("read manifest: {e}")))?;
    let payload_manifest_sha256 = crate::digest::sha256_hex(&manifest_bytes);

    let mut bundled_rows: BTreeMap<String, ComponentRow> = BTreeMap::new();
    let mut records: Vec<ProvenanceRecord> = Vec::new();

    for file in &verified.manifest().files {
        let dest = &file.path;

        let id_opt = inventory.entry.iter().find_map(|e| {
            if entry_targets(e).iter().any(|t| t == "windows-x86_64")
                && entry_dest_matches(e, dest)
                && e.class() == Some(crate::inventory::DeliveryClass::Component)
            {
                crate::inventory::entry_component_id(e)
            } else {
                None
            }
        });

        let Some(id) = id_opt else {
            continue;
        };

        check_member_path(dest)?;

        let (row_version, row_source, mut row_inputs) = bundled_identity(&repo_root, target, id)?;

        let mut pre_signing_sha256 = None;
        let receipt_rel = match id {
            "ced-engine" => Some("share/provenance/ced/receipt.json"),
            "llama-server" | "vulkan-loader" => Some("share/provenance/llama/receipt.json"),
            "parakeet-server" => Some("share/provenance/parakeet/receipt.json"),
            "rfdetr-engine" => Some("share/provenance/rfdetr/receipt.json"),
            "onnxruntime" => Some("share/provenance/onnx/receipt.json"),
            "nvattest" => {
                if dest.ends_with(".pem") {
                    None
                } else {
                    Some("share/provenance/nvattest/receipt.json")
                }
            }
            _ => None,
        };
        if let Some(rpath) = receipt_rel {
            let full_receipt = payload_root.join(rpath);
            if full_receipt.exists() {
                let bytes = std::fs::read(&full_receipt)
                    .map_err(|e| EvidenceError::new(format!("read {rpath}: {e}")))?;
                let receipt_sha = crate::digest::sha256_hex(&bytes);
                if row_inputs.is_empty() {
                    row_inputs = vec![InputRef {
                        name: "receipt.json".to_string(),
                        sha256: receipt_sha,
                    }];
                }
                if let Ok(receipt) =
                    crate::controlled_build::decode_controlled_build_receipt(&bytes)
                {
                    let member_filename = Path::new(dest)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    if let Some(out) = receipt
                        .outputs
                        .iter()
                        .find(|o| o.label == member_filename || o.label == *dest)
                    {
                        pre_signing_sha256 = Some(out.pre_signing_sha256.clone());
                    }
                }
            } else if row_inputs.is_empty() {
                return Err(EvidenceError::new(format!(
                    "unnamed-input: {target} {id} {dest}"
                )));
            }
        }

        let input = row_inputs
            .first()
            .cloned()
            .ok_or_else(|| EvidenceError::new(format!("unnamed-input: {target} {id} {dest}")))?;

        records.push(ProvenanceRecord {
            id: id.to_string(),
            path: dest.clone(),
            input,
            inner_path: None,
            alias: None,
            extracted_sha256: None,
            pre_signing_sha256,
            final_sha256: file.sha256.clone(),
        });

        let entry = bundled_rows
            .entry(id.to_string())
            .or_insert_with(|| ComponentRow {
                id: id.to_string(),
                version: row_version,
                delivery: DeliveryKind::Bundled,
                source: row_source,
                inputs: row_inputs,
                members: Vec::new(),
            });
        entry.members.push(MemberRef {
            path: dest.clone(),
            sha256: file.sha256.clone(),
        });
    }

    let runtime_rows = render_runtime_rows(target, &[])?;
    for rt in &runtime_rows {
        if bundled_rows.contains_key(&rt.id) {
            return Err(EvidenceError::new(format!("mixed-delivery: {}", rt.id)));
        }
    }

    let mut all_components: Vec<ComponentRow> = bundled_rows.into_values().collect();
    for row in &mut all_components {
        row.members
            .sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.path, &b.path));
        row.inputs
            .sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.name, &b.name));
    }
    all_components.extend(runtime_rows);
    all_components.sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.id, &b.id));

    records.sort_by(|a, b| {
        let id_cmp = solstone_core_assets::cmp_utf16_code_units(&a.id, &b.id);
        if id_cmp.is_eq() {
            solstone_core_assets::cmp_utf16_code_units(&a.path, &b.path)
        } else {
            id_cmp
        }
    });

    let comp_file = ComponentFile {
        product: solstone_core_installed_payload::PRODUCT.to_string(),
        version: version.clone(),
        target: target.to_string(),
        components: all_components,
    };
    let prov_file = ProvenanceFile {
        product: solstone_core_installed_payload::PRODUCT.to_string(),
        version: version.clone(),
        target: target.to_string(),
        basis: "producer-attested".to_string(),
        manifests: ProvenanceManifests {
            installed_payload_sha256: None,
            release_manifest_sha256: None,
            payload_manifest_sha256: Some(payload_manifest_sha256),
        },
        records,
    };

    let comp_json = serde_json::to_string_pretty(&comp_file)
        .map_err(|e| EvidenceError::new(e.to_string()))?
        + "\n";
    if comp_json.len() >= 1_048_576 {
        return Err(EvidenceError::new(format!(
            "components-too-large: {}",
            comp_json.len()
        )));
    }
    let prov_json = serde_json::to_string_pretty(&prov_file)
        .map_err(|e| EvidenceError::new(e.to_string()))?
        + "\n";

    let parent = out_dir.parent().unwrap_or(Path::new("."));
    let partial = parent.join(format!(
        "{}.partial",
        out_dir.file_name().unwrap_or_default().to_string_lossy()
    ));
    if partial.exists() {
        let _ = std::fs::remove_dir_all(&partial);
    }
    std::fs::create_dir_all(&partial).map_err(|e| EvidenceError::new(e.to_string()))?;
    std::fs::write(
        partial.join(components_file_name(&version, target)),
        &comp_json,
    )
    .map_err(|e| EvidenceError::new(e.to_string()))?;
    std::fs::write(
        partial.join(provenance_file_name(&version, target)),
        &prov_json,
    )
    .map_err(|e| EvidenceError::new(e.to_string()))?;

    if out_dir.exists() {
        let _ = std::fs::remove_dir_all(out_dir);
    }
    std::fs::rename(&partial, out_dir).map_err(|e| {
        let _ = std::fs::remove_dir_all(&partial);
        EvidenceError::new(format!(
            "evidence-install-failed: {}: {e}",
            out_dir.display()
        ))
    })?;

    Ok(())
}

pub fn install_evidence_directory(
    work: &Path,
    dest: &Path,
    basename: &str,
    fail_install: bool,
) -> Result<PathBuf, EvidenceError> {
    let partial = work.join("component-evidence.partial");
    let dir_name = evidence_directory_name(basename);
    let sibling = dest.parent().unwrap_or(Path::new(".")).join(&dir_name);

    if fail_install {
        if partial.exists() {
            let _ = std::fs::remove_dir_all(&partial);
        }
        if sibling.exists() {
            let _ = std::fs::remove_dir_all(&sibling);
        }
        return Err(EvidenceError::new(format!(
            "evidence-install-failed: {}: injected",
            sibling.display()
        )));
    }

    if sibling.exists() {
        let displaced = work.join("evidence.displaced");
        if displaced.exists() {
            let _ = std::fs::remove_dir_all(&displaced);
        }
        std::fs::rename(&sibling, &displaced).map_err(|e| {
            EvidenceError::new(format!(
                "evidence-install-failed: {}: {e}",
                sibling.display()
            ))
        })?;
    }

    if let Err(e) = std::fs::rename(&partial, &sibling) {
        if partial.exists() {
            let _ = std::fs::remove_dir_all(&partial);
        }
        return Err(EvidenceError::new(format!(
            "evidence-install-failed: {}: {e}",
            sibling.display()
        )));
    }

    Ok(sibling)
}

fn entry_dest_str(entry: &crate::inventory::Entry) -> String {
    match entry {
        crate::inventory::Entry::ModelAsset { dest, .. } => dest.clone(),
        crate::inventory::Entry::OnnxRuntime { dest_dir, .. } => dest_dir.clone(),
        crate::inventory::Entry::Pdfium { dest_dir, .. } => dest_dir.clone(),
        crate::inventory::Entry::WindowsNative { dest, .. } => dest.clone(),
        crate::inventory::Entry::Bin { dest, .. } => dest.clone(),
        crate::inventory::Entry::Launcher { dest, .. } => dest.clone(),
        crate::inventory::Entry::Copy { dest, .. } => dest.clone(),
        crate::inventory::Entry::PinnedNative { dest, .. } => dest.clone(),
        crate::inventory::Entry::PinnedMembers { staged, .. } => {
            staged.first().map(|s| s.dest.clone()).unwrap_or_default()
        }
        crate::inventory::Entry::LicenceTree { source, .. } => source.clone(),
        crate::inventory::Entry::WindowsBuildEvidence { dest, .. } => dest.clone(),
    }
}

fn entry_dest_matches(entry: &crate::inventory::Entry, path: &str) -> bool {
    match entry {
        crate::inventory::Entry::ModelAsset { dest, .. } => dest == path,
        crate::inventory::Entry::OnnxRuntime {
            dest_dir,
            identities,
            ..
        } => identities.iter().any(|id| {
            id.name
                .as_ref()
                .is_some_and(|name| format!("{dest_dir}/{name}") == path)
                || id.aliases.iter().any(|a| format!("{dest_dir}/{a}") == path)
        }),
        crate::inventory::Entry::Pdfium { dest_dir, .. } => path.starts_with(dest_dir),
        crate::inventory::Entry::WindowsNative { dest, .. } => dest == path,
        crate::inventory::Entry::Bin { dest, .. } => dest == path,
        crate::inventory::Entry::Launcher { dest, .. } => dest == path,
        crate::inventory::Entry::Copy { dest, .. } => dest == path,
        crate::inventory::Entry::PinnedNative { dest, .. } => dest == path,
        crate::inventory::Entry::PinnedMembers { staged, .. } => {
            staged.iter().any(|s| s.dest == path)
        }
        crate::inventory::Entry::LicenceTree { .. } => false,
        crate::inventory::Entry::WindowsBuildEvidence { dest, .. } => dest == path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_inventory() -> crate::inventory::Inventory {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let inv_path = repo_root.join("core/distribution/inventory.toml");
        let mut inv = crate::inventory::load_inventory(&inv_path).expect("load test inventory");
        inv.entry
            .retain(|e| e.class() != Some(crate::inventory::DeliveryClass::Component));
        inv
    }

    #[test]
    fn render_runtime_rows_posix_properties() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let expected_llama_rev = read_llama_cpp_revision(&repo_root).unwrap();

        for target in ["linux-x86_64", "linux-aarch64", "macos-arm64"] {
            let rows1 = render_runtime_rows(target, &[]).unwrap();
            let rows2 = render_runtime_rows(target, &[]).unwrap();

            // Byte-identical serialized ComponentFile
            let comp_file1 = ComponentFile {
                product: solstone_core_installed_payload::PRODUCT.to_string(),
                version: "2.0.37".to_string(),
                target: target.to_string(),
                components: rows1.clone(),
            };
            let json1 = serde_json::to_string_pretty(&comp_file1).unwrap() + "\n";
            let comp_file2 = ComponentFile {
                product: solstone_core_installed_payload::PRODUCT.to_string(),
                version: "2.0.37".to_string(),
                target: target.to_string(),
                components: rows2.clone(),
            };
            let json2 = serde_json::to_string_pretty(&comp_file2).unwrap() + "\n";
            assert_eq!(json1, json2, "mismatch for {target}");
            assert!(json1.len() < 1_048_576, "exceeds 1 MiB for {target}");

            // Properties
            assert!(!rows1.iter().any(|r| r.id == "llama-server-vulkan"));
            let cat = solstone_core_assets::catalog();
            for r in &rows1 {
                if r.id == "llama-server-cuda" {
                    assert_eq!(r.source, expected_llama_rev);
                    assert!(!r.source.contains("updates.solstone.app"));
                }
                if r.id != "nvattest" {
                    for inp in &r.inputs {
                        if let Some(art) = cat.iter().find(|a| a.filename == inp.name) {
                            assert_eq!(
                                r.version, art.version,
                                "version check for {} in {target}",
                                r.id
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn residual_override_rejection() {
        set_residual_override(
            "linux-x86_64",
            "llama-server-cuda",
            "source",
            "https://updates.solstone.app/nope",
        );
        let res = render_runtime_rows("linux-x86_64", &[]);
        clear_residual_overrides();
        let err = res.unwrap_err();
        assert!(err.message.starts_with("residual-disagrees:"));
    }

    #[test]
    fn injected_fetch_unit_version_mismatch() {
        let extra = vec![
            InjectedFetch {
                unit: "ced-model".to_string(),
                version: "v1.0".to_string(),
                origin_key: "k1".to_string(),
                sha256: "0".repeat(64),
                filename: "f1".to_string(),
                source: "s1".to_string(),
            },
            InjectedFetch {
                unit: "ced-model".to_string(),
                version: "v2.0".to_string(),
                origin_key: "k2".to_string(),
                sha256: "0".repeat(64),
                filename: "f2".to_string(),
                source: "s1".to_string(),
            },
        ];
        let res = render_runtime_rows("linux-x86_64", &extra);
        let err = res.unwrap_err();
        assert_eq!(err.message, "unit-version-mismatch: ced-model");
    }

    #[test]
    fn name_receipt_inputs_posix_and_windows() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let inv_path = repo_root.join("core/distribution/inventory.toml");
        let inv = crate::inventory::load_inventory(&inv_path).unwrap();

        for target in ["linux-x86_64", "linux-aarch64", "macos-arm64"] {
            let inputs = name_receipt_inputs(&inv, target).unwrap();
            assert!(!inputs.is_empty());
            for inp in &inputs {
                assert_eq!(inp.sha256.len(), 64);
                assert!(inp.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            }
            for entry in &inv.entry {
                if entry_targets(entry).iter().any(|t| t == target)
                    && entry.class() == Some(crate::inventory::DeliveryClass::Component)
                    && let Some(id) = crate::inventory::entry_component_id(entry)
                {
                    assert!(
                        inputs.iter().any(|i| i.id == id),
                        "missing component {id} for target {target}"
                    );
                }
            }
        }

        let win_res = name_receipt_inputs(&inv, "windows-x86_64");
        let err = win_res.unwrap_err();
        assert!(err.message.starts_with("unnamed-input: windows-x86_64"));
    }

    #[test]
    fn utf16_sorting_order() {
        let row1 = ComponentRow {
            id: "\u{10000}".to_string(),
            version: "1.0".to_string(),
            delivery: DeliveryKind::Bundled,
            source: "src1".to_string(),
            inputs: Vec::new(),
            members: Vec::new(),
        };
        let row2 = ComponentRow {
            id: "\u{E000}".to_string(),
            version: "1.0".to_string(),
            delivery: DeliveryKind::Bundled,
            source: "src2".to_string(),
            inputs: Vec::new(),
            members: Vec::new(),
        };
        let mut rows = vec![row2, row1];
        rows.sort_by(|a, b| solstone_core_assets::cmp_utf16_code_units(&a.id, &b.id));
        assert_eq!(rows[0].id, "\u{10000}");
        assert_eq!(rows[1].id, "\u{E000}");

        let file = ComponentFile {
            product: "solstone-journal".to_string(),
            version: "1.0".to_string(),
            target: "test".to_string(),
            components: rows,
        };
        let json = serde_json::to_string(&file).unwrap();
        let idx1 = json.find("\u{10000}").unwrap();
        let idx2 = json.find("\u{E000}").unwrap();
        assert!(idx1 < idx2);
    }

    #[test]
    fn promote_linux_aarch64_evidence_lifecycle() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let basename = format!("solstone-journal-{version}-linux-aarch64");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-evidence-root-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![("bin/solstone-core".into(), b"core".to_vec(), 0o755)],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: "linux-aarch64".into(),
            deb_arch: "arm64".into(),
            rpm_arch: "aarch64".into(),
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
            inventory: test_inventory(),
            archives: Vec::new(),
            fail_evidence_install: false,
            stage_mutator: None,
        };
        promote(&req).expect("promote linux-aarch64");

        let evidence_dir = dest
            .parent()
            .unwrap()
            .join(evidence_directory_name(&basename));
        assert!(evidence_dir.exists(), "evidence directory exists");
        assert!(
            !dest.join(evidence_directory_name(&basename)).exists(),
            "evidence directory is not inside dest"
        );

        let comp_name = components_file_name(version, "linux-aarch64");
        let comp_content =
            fs::read_to_string(evidence_dir.join(comp_name)).expect("read components");
        let comp_file: ComponentFile =
            serde_json::from_str(&comp_content).expect("parse components");

        let mut downloaded_input_names: BTreeSet<String> = BTreeSet::new();
        for comp in &comp_file.components {
            if comp.delivery == DeliveryKind::RuntimeDownloaded {
                for inp in &comp.inputs {
                    downloaded_input_names.insert(inp.name.clone());
                    assert!(
                        !inp.name.contains("x64") && !inp.name.contains("amd64"),
                        "input {} contains x64 or amd64",
                        inp.name
                    );
                }
            }
        }

        let fetch_set = solstone_core_assets::runtime_fetch_set("linux-aarch64").unwrap();
        let mut expected_filenames = BTreeSet::new();
        for f in &fetch_set {
            let cat = solstone_core_assets::catalog();
            if let Some(a) = cat.iter().find(|a| a.origin_key == f.origin_key()) {
                expected_filenames.insert(a.filename.to_string());
            } else if f.unit() == "nvattest" {
                let fname = f.origin_key().rsplit('/').next().unwrap().to_string();
                expected_filenames.insert(fname);
            }
        }
        assert_eq!(downloaded_input_names, expected_filenames);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn promote_fail_after_checkpoints_leave_dest_marker_without_evidence() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let basename = format!("solstone-journal-{version}-linux-aarch64");

        for stage in ["checksums", "manifest", "revalidate", "rename"] {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = PathBuf::from(format!(
                "/var/tmp/solstone-distribution-fail-after-root-{}-{nanos}",
                std::process::id()
            ));
            let dest = root.join("dest");
            let work = root.join("work");
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&dest).unwrap();
            let marker = dest.join("marker");
            fs::write(&marker, b"marker").unwrap();

            let req = PromoteRequest {
                dest: dest.clone(),
                work: work.clone(),
                tree: vec![("bin/solstone-core".into(), b"core".to_vec(), 0o755)],
                version: version.to_owned(),
                basename: basename.clone(),
                os: "linux".into(),
                arch: "linux-aarch64".into(),
                deb_arch: "arm64".into(),
                rpm_arch: "aarch64".into(),
                dirty: false,
                observed: Provenance {
                    commit: "aaa".into(),
                    lock_sha256: "bbb".into(),
                },
                expected: Provenance {
                    commit: "aaa".into(),
                    lock_sha256: "bbb".into(),
                },
                fail_after: Some(stage.to_string()),
                apple: None,
                inventory: test_inventory(),
                archives: Vec::new(),
                fail_evidence_install: false,
                stage_mutator: None,
            };
            assert!(
                promote(&req).is_err(),
                "expected error for fail_after {stage}"
            );
            assert!(
                marker.exists(),
                "marker still exists in dest for stage {stage}"
            );
            let evidence_dir = dest
                .parent()
                .unwrap()
                .join(evidence_directory_name(&basename));
            assert!(
                !evidence_dir.exists(),
                "evidence directory does not exist for stage {stage}"
            );

            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn promote_fail_evidence_install_retains_dest_release_set_without_evidence() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let basename = format!("solstone-journal-{version}-linux-aarch64");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-fail-ev-root-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![("bin/solstone-core".into(), b"core".to_vec(), 0o755)],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: "linux-aarch64".into(),
            deb_arch: "arm64".into(),
            rpm_arch: "aarch64".into(),
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
            inventory: test_inventory(),
            archives: Vec::new(),
            fail_evidence_install: true,
            stage_mutator: None,
        };
        let err = promote(&req).unwrap_err();
        assert!(err.message.contains("evidence-install-failed:"));
        assert!(dest.join(format!("{basename}.tar.gz")).exists());
        let evidence_dir = dest
            .parent()
            .unwrap()
            .join(evidence_directory_name(&basename));
        assert!(!evidence_dir.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mixed_delivery_refuses_when_bundled_and_runtime_collide() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let basename = format!("solstone-journal-{version}-linux-x86_64");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-distribution-mixed-root-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        // restic and rclone ship in the package, so they are not in the runtime
        // fetch set. parakeet-server still is, and a bundled copy of it collides.
        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::WindowsNative {
            class: Some(crate::inventory::DeliveryClass::Component),
            component: crate::inventory::WindowsNativeComponent::Parakeet,
            member: "parakeet-server.exe".to_string(),
            dest: "bin/parakeet-server.exe".to_string(),
            mode: 0o755,
            targets: vec!["linux-x86_64".to_string()],
        });

        set_bundled_identity_override(
            "linux-x86_64",
            "parakeet-server",
            "0.6.1",
            "https://github.com/mudler/parakeet.cpp",
            vec![InputRef {
                name: "parakeet-server.exe".to_string(),
                sha256: crate::digest::sha256_hex(b"parakeet"),
            }],
        );

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![
                ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
                (
                    "bin/parakeet-server.exe".into(),
                    b"parakeet".to_vec(),
                    0o755,
                ),
            ],
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
            inventory: inv,
            archives: Vec::new(),
            fail_evidence_install: false,
            stage_mutator: None,
        };

        let res = promote(&req);
        clear_bundled_identity_override();
        let err = res.unwrap_err();
        assert!(
            err.message.contains("mixed-delivery: parakeet-server"),
            "expected mixed-delivery: parakeet-server, got {}",
            err.message
        );

        let _ = fs::remove_dir_all(&root);
    }

    fn make_tar_gz(entries: &[(&str, &[u8], tar::EntryType, Option<&str>)]) -> Vec<u8> {
        use std::io::Write;
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data, entry_type, link_target) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*entry_type);
            header.set_mode(0o644);
            if let Some(target) = link_target {
                header.set_size(0);
                header.set_link_name(target).unwrap();
            } else {
                header.set_size(data.len() as u64);
            }
            header.set_cksum();
            builder.append_data(&mut header, *path, *data).unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn mutate_macos_macho(stage: &Path) {
        let p = stage.join("lib/solstone_journal_models/libsilero_vad.dylib");
        let diff_macho = crate::macho::fixture(&crate::macho::FixtureSpec {
            filetype: crate::macho::MH_DYLIB,
            install_name: Some("@rpath/libmutated.dylib"),
            ..crate::macho::FixtureSpec::default()
        });
        fs::write(&p, diff_macho).unwrap();
    }

    fn mutate_linux_payload(stage: &Path) {
        let p = stage.join("lib/solstone_journal_models/assets/payload.bin");
        let mut b = fs::read(&p).unwrap();
        b[0] ^= 0xff;
        fs::write(&p, b).unwrap();
    }

    #[test]
    fn unsigned_linux_extract_reextracts_member_digest() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let payload_bytes = b"sample unsigned payload binary content";
        let payload_sha = crate::digest::sha256_hex(payload_bytes);
        let archive_name = "silero_vad_member.tar.gz";
        let archive_bytes =
            make_tar_gz(&[("payload.bin", payload_bytes, tar::EntryType::Regular, None)]);

        let version = env!("CARGO_PKG_VERSION");
        let target = "linux-x86_64";
        let basename = format!("solstone-journal-{version}-{target}");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-dist-test-unsigned-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        let member_dest = "lib/solstone_journal_models/assets/payload.bin";

        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::ModelAsset {
            class: Some(crate::inventory::DeliveryClass::Component),
            source: format!("assets/{archive_name}"),
            dest: member_dest.to_string(),
            mode: 0o644,
            digest_const: "UNUSED".to_string(),
            digest_source: "unused".to_string(),
            targets: vec![target.to_string()],
            archive_slot: None,
        });

        set_bundled_identity_override(
            target,
            "silero-vad-model",
            "1.0.0",
            "https://example.com/silero",
            vec![InputRef {
                name: archive_name.to_string(),
                sha256: crate::digest::sha256_hex(&archive_bytes),
            }],
        );

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![
                ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
                (member_dest.into(), payload_bytes.to_vec(), 0o644),
            ],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: target.into(),
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
            inventory: inv,
            archives: vec![(archive_name.to_string(), archive_bytes)],
            fail_evidence_install: false,
            stage_mutator: None,
        };

        let res = promote(&req);
        clear_bundled_identity_override();
        res.expect("promote unsigned extract");

        let evidence_dir = dest
            .parent()
            .unwrap()
            .join(evidence_directory_name(&basename));
        let prov_file: ProvenanceFile = serde_json::from_str(
            &fs::read_to_string(evidence_dir.join(provenance_file_name(version, target))).unwrap(),
        )
        .unwrap();

        let rec = prov_file
            .records
            .iter()
            .find(|r| r.path == member_dest)
            .expect("record");
        assert_eq!(rec.final_sha256, payload_sha);
        assert_eq!(rec.extracted_sha256, Some(payload_sha));
        assert!(rec.inner_path.is_none());
        assert!(rec.alias.is_none());

        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&evidence_dir);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verbatim_linux_file_has_no_extracted_digest() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let vad_bytes = b"vad-bytes";
        let vad_sha = crate::digest::sha256_hex(vad_bytes);
        let file_name = "silero_vad_v6.onnx";
        let member_dest = format!("lib/solstone_journal_models/assets/{file_name}");

        let version = env!("CARGO_PKG_VERSION");
        let target = "linux-x86_64";
        let basename = format!("solstone-journal-{version}-{target}");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-dist-test-verbatim-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::ModelAsset {
            class: Some(crate::inventory::DeliveryClass::Component),
            source: format!("assets/{file_name}"),
            dest: member_dest.clone(),
            mode: 0o644,
            digest_const: "UNUSED".to_string(),
            digest_source: "unused".to_string(),
            targets: vec![target.to_string()],
            archive_slot: None,
        });

        set_bundled_identity_override(
            target,
            "silero-vad-model",
            "1.0.0",
            "https://example.com/silero",
            vec![InputRef {
                name: file_name.to_string(),
                sha256: vad_sha.clone(),
            }],
        );

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![
                ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
                (member_dest.clone(), vad_bytes.to_vec(), 0o644),
            ],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: target.into(),
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
            inventory: inv,
            archives: Vec::new(),
            fail_evidence_install: false,
            stage_mutator: None,
        };

        let res = promote(&req);
        clear_bundled_identity_override();
        res.expect("promote verbatim");

        let evidence_dir = dest
            .parent()
            .unwrap()
            .join(evidence_directory_name(&basename));
        let prov_file: ProvenanceFile = serde_json::from_str(
            &fs::read_to_string(evidence_dir.join(provenance_file_name(version, target))).unwrap(),
        )
        .unwrap();

        let rec = prov_file
            .records
            .iter()
            .find(|r| r.path == member_dest)
            .expect("record");
        assert_eq!(rec.final_sha256, vad_sha);
        assert!(rec.extracted_sha256.is_none());

        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&evidence_dir);
        let _ = fs::remove_dir_all(&root);
    }

    /// A pinned single-file input (a model, a bz2) answers to the empty member
    /// name, as does every other single-file input in the release. Its row is
    /// named from its own catalog pin and its member is read from its own input
    /// only, never from the first single-file input that happens to match.
    #[test]
    fn pinned_single_file_member_reads_its_own_input_and_names_its_catalog_pin() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let model = fs::read(repo.join("core/models/assets/ced/ced-tiny-q8_0.gguf")).unwrap();
        let model_sha = crate::digest::sha256_hex(&model);
        let row = solstone_core_assets::catalog()
            .iter()
            .find(|a| a.unit == "ced-model" && a.filename == "ced-tiny-q8_0.gguf")
            .expect("catalog row");
        assert_eq!(row.sha256, model_sha);
        let member_dest = "lib/solstone-ced/ced-tiny-q8_0.gguf".to_string();
        let version = env!("CARGO_PKG_VERSION");
        let target = "linux-x86_64";
        let basename = format!("solstone-journal-{version}-{target}");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-dist-test-pinned-single-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::PinnedMembers {
            class: Some(crate::inventory::DeliveryClass::Component),
            component: Some("ced-model".to_string()),
            input: crate::inventory::PinnedInput::CatalogCommitted {
                unit: "ced-model".into(),
                filename: "ced-tiny-q8_0.gguf".into(),
                path: "core/models/assets/ced/ced-tiny-q8_0.gguf".into(),
            },
            staged: vec![crate::inventory::StagedMember {
                relpath: String::new(),
                dest: member_dest.clone(),
                mode: 0o644,
                extracted_sha256: model_sha.clone(),
                identity: None,
            }],
            ignored: Vec::new(),
            targets: vec![target.to_string()],
        });

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![
                ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
                (member_dest.clone(), model.clone(), 0o644),
            ],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: target.into(),
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
            inventory: inv,
            archives: vec![
                (
                    "decoy-model.gguf".to_string(),
                    b"not this input".to_vec(),
                ),
                ("ced-tiny-q8_0.gguf".to_string(), model.clone()),
            ],
            fail_evidence_install: false,
            stage_mutator: None,
        };

        let res = promote(&req);
        let evidence_dir = dest
            .parent()
            .unwrap()
            .join(evidence_directory_name(&basename));
        let read = |name: String| fs::read_to_string(evidence_dir.join(name));
        let outcome = res.map(|_| {
            (
                read(components_file_name(version, target)).unwrap(),
                read(provenance_file_name(version, target)).unwrap(),
            )
        });
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&evidence_dir);
        let (components, provenance) = outcome.expect("promote pinned single-file member");

        let components: serde_json::Value = serde_json::from_str(&components).unwrap();
        let ced = components["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "ced-model")
            .expect("ced-model row");
        assert_eq!(ced["version"], row.version);
        assert_eq!(ced["inputs"][0]["name"], "ced-tiny-q8_0.gguf");
        assert_eq!(ced["inputs"][0]["sha256"], model_sha.as_str());
        let provenance: ProvenanceFile = serde_json::from_str(&provenance).unwrap();
        let rec = provenance
            .records
            .iter()
            .find(|r| r.path == member_dest)
            .expect("record");
        assert_eq!(rec.input.name, "ced-tiny-q8_0.gguf");
        assert_eq!(rec.final_sha256, model_sha);
    }

    #[test]
    fn archive_symlink_records_alias_and_inner_path() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let nvat_bytes = b"libnvat.so regular binary bytes";
        let nvat_sha = crate::digest::sha256_hex(nvat_bytes);
        let archive_name = "silero_vad_alias.tar.gz";
        let archive_bytes = make_tar_gz(&[
            (
                "lib/libnvat.so.1.2.2",
                nvat_bytes,
                tar::EntryType::Regular,
                None,
            ),
            (
                "lib/libnvat.so.1",
                &[],
                tar::EntryType::Symlink,
                Some("libnvat.so.1.2.2"),
            ),
        ]);

        let version = env!("CARGO_PKG_VERSION");
        let target = "linux-x86_64";
        let basename = format!("solstone-journal-{version}-{target}");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-dist-test-alias-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        let member_dest = "lib/libnvat.so.1";

        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::ModelAsset {
            class: Some(crate::inventory::DeliveryClass::Component),
            source: format!("assets/{archive_name}"),
            dest: member_dest.to_string(),
            mode: 0o755,
            digest_const: "UNUSED".to_string(),
            digest_source: "unused".to_string(),
            targets: vec![target.to_string()],
            archive_slot: None,
        });

        set_bundled_identity_override(
            target,
            "silero-vad-model",
            "1.0.0",
            "https://example.com/silero",
            vec![InputRef {
                name: archive_name.to_string(),
                sha256: crate::digest::sha256_hex(&archive_bytes),
            }],
        );

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![
                ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
                (member_dest.into(), nvat_bytes.to_vec(), 0o755),
            ],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: target.into(),
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
            inventory: inv,
            archives: vec![(archive_name.to_string(), archive_bytes)],
            fail_evidence_install: false,
            stage_mutator: None,
        };

        let res = promote(&req);
        clear_bundled_identity_override();
        res.expect("promote alias");

        let evidence_dir = dest
            .parent()
            .unwrap()
            .join(evidence_directory_name(&basename));
        let prov_file: ProvenanceFile = serde_json::from_str(
            &fs::read_to_string(evidence_dir.join(provenance_file_name(version, target))).unwrap(),
        )
        .unwrap();

        let rec = prov_file
            .records
            .iter()
            .find(|r| r.path == member_dest)
            .expect("record");
        assert_eq!(rec.alias, Some("lib/libnvat.so.1".to_string()));
        assert_eq!(rec.inner_path, Some("lib/libnvat.so.1.2.2".to_string()));
        assert_eq!(rec.extracted_sha256, Some(nvat_sha.clone()));
        assert_eq!(rec.final_sha256, nvat_sha);

        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&evidence_dir);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn macos_macho_extracted_records_presigning_and_differing_final() {
        use crate::archive_contract::{DeliveryContract, PrebuildInputIdentity};
        use crate::macho::{FixtureSpec, MH_DYLIB, fixture};
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let target = "macos-arm64";
        let basename = format!("solstone-journal-{version}-{target}");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-dist-test-macos-macho-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let stage = root.join("chain-stage");
        let _ = fs::remove_dir_all(&root);

        let executable = fixture(&FixtureSpec::default());
        let dylib = fixture(&FixtureSpec {
            filetype: MH_DYLIB,
            install_name: Some("@rpath/libdemo.dylib"),
            ..FixtureSpec::default()
        });
        crate::stage::write_staged_file_mode(&stage, "bin/solstone", &executable, 0o755).unwrap();
        crate::stage::write_staged_file_mode(
            &stage,
            "lib/solstone-runtime/libdemo.dylib",
            &dylib,
            0o644,
        )
        .unwrap();

        let component_macho = fixture(&FixtureSpec {
            filetype: MH_DYLIB,
            install_name: Some("@rpath/libsilero_vad.dylib"),
            ..FixtureSpec::default()
        });
        let component_macho_sha = crate::digest::sha256_hex(&component_macho);
        let component_dest = "lib/solstone_journal_models/libsilero_vad.dylib";
        let archive_name = "silero_vad_macho.tar.gz";
        let archive_bytes = make_tar_gz(&[(
            component_dest,
            &component_macho,
            tar::EntryType::Regular,
            None,
        )]);

        crate::stage::write_staged_file_mode(&stage, component_dest, &component_macho, 0o644)
            .unwrap();

        let prebuild = PrebuildInputIdentity {
            target_id: target.into(),
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
        crate::archive_contract::stage_chain(&stage, &prebuild, &delivery, "aaa", "bbb").unwrap();

        let mut tree = Vec::new();
        for record in crate::stage::staged_records(&stage).unwrap() {
            let bytes = fs::read(stage.join(&record.dest)).unwrap();
            tree.push((record.dest, bytes, record.mode));
        }

        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::ModelAsset {
            class: Some(crate::inventory::DeliveryClass::Component),
            source: format!("assets/{archive_name}"),
            dest: component_dest.to_string(),
            mode: 0o644,
            digest_const: "UNUSED".to_string(),
            digest_source: "unused".to_string(),
            targets: vec![target.to_string()],
            archive_slot: None,
        });

        set_bundled_identity_override(
            target,
            "silero-vad-model",
            "1.0.0",
            "https://example.com/silero",
            vec![InputRef {
                name: archive_name.to_string(),
                sha256: crate::digest::sha256_hex(&archive_bytes),
            }],
        );

        let _signer_guard = crate::promote::install_fake_macos_sign();

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree,
            version: version.to_owned(),
            basename: basename.clone(),
            os: "macos".into(),
            arch: target.into(),
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
            inventory: inv,
            archives: vec![(archive_name.to_string(), archive_bytes)],
            fail_evidence_install: false,
            stage_mutator: None,
        };

        let res = promote(&req);
        clear_bundled_identity_override();
        res.expect("promote macos macho");

        let evidence_dir = dest
            .parent()
            .unwrap()
            .join(evidence_directory_name(&basename));
        let prov_file: ProvenanceFile = serde_json::from_str(
            &fs::read_to_string(evidence_dir.join(provenance_file_name(version, target))).unwrap(),
        )
        .unwrap();

        let rec = prov_file
            .records
            .iter()
            .find(|r| r.path == component_dest)
            .expect("record");
        assert_eq!(rec.pre_signing_sha256, Some(component_macho_sha));
        assert_ne!(rec.final_sha256, rec.pre_signing_sha256.clone().unwrap());

        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&evidence_dir);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn macos_stage_mutator_triggers_digest_disagreement() {
        use crate::archive_contract::{DeliveryContract, PrebuildInputIdentity};
        use crate::macho::{FixtureSpec, MH_DYLIB, fixture};
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let target = "macos-arm64";
        let basename = format!("solstone-journal-{version}-{target}");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-dist-test-macos-mutator-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let stage = root.join("chain-stage");
        let _ = fs::remove_dir_all(&root);

        let executable = fixture(&FixtureSpec::default());
        let dylib = fixture(&FixtureSpec {
            filetype: MH_DYLIB,
            install_name: Some("@rpath/libdemo.dylib"),
            ..FixtureSpec::default()
        });
        crate::stage::write_staged_file_mode(&stage, "bin/solstone", &executable, 0o755).unwrap();
        crate::stage::write_staged_file_mode(
            &stage,
            "lib/solstone-runtime/libdemo.dylib",
            &dylib,
            0o644,
        )
        .unwrap();

        let component_macho = fixture(&FixtureSpec {
            filetype: MH_DYLIB,
            install_name: Some("@rpath/libsilero_vad.dylib"),
            ..FixtureSpec::default()
        });
        let component_dest = "lib/solstone_journal_models/libsilero_vad.dylib";
        let archive_name = "silero_vad_macho.tar.gz";
        let archive_bytes = make_tar_gz(&[(
            component_dest,
            &component_macho,
            tar::EntryType::Regular,
            None,
        )]);

        crate::stage::write_staged_file_mode(&stage, component_dest, &component_macho, 0o644)
            .unwrap();

        let prebuild = PrebuildInputIdentity {
            target_id: target.into(),
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
        crate::archive_contract::stage_chain(&stage, &prebuild, &delivery, "aaa", "bbb").unwrap();

        let mut tree = Vec::new();
        for record in crate::stage::staged_records(&stage).unwrap() {
            let bytes = fs::read(stage.join(&record.dest)).unwrap();
            tree.push((record.dest, bytes, record.mode));
        }

        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::ModelAsset {
            class: Some(crate::inventory::DeliveryClass::Component),
            source: format!("assets/{archive_name}"),
            dest: component_dest.to_string(),
            mode: 0o644,
            digest_const: "UNUSED".to_string(),
            digest_source: "unused".to_string(),
            targets: vec![target.to_string()],
            archive_slot: None,
        });

        set_bundled_identity_override(
            target,
            "silero-vad-model",
            "1.0.0",
            "https://example.com/silero",
            vec![InputRef {
                name: archive_name.to_string(),
                sha256: crate::digest::sha256_hex(&archive_bytes),
            }],
        );

        let _signer_guard = crate::promote::install_fake_macos_sign();

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree,
            version: version.to_owned(),
            basename: basename.clone(),
            os: "macos".into(),
            arch: target.into(),
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
            inventory: inv,
            archives: vec![(archive_name.to_string(), archive_bytes)],
            fail_evidence_install: false,
            stage_mutator: Some(mutate_macos_macho),
        };

        let res = promote(&req);
        clear_bundled_identity_override();
        let err = res.unwrap_err();
        assert!(
            err.message
                .contains(&format!("digest-disagreement: {component_dest}")),
            "expected digest-disagreement for {component_dest}, got: {}",
            err.message
        );

        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn linux_stage_mutator_triggers_digest_disagreement() {
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let payload_bytes = b"sample linux payload binary content";
        let archive_name = "silero_vad_member.tar.gz";
        let archive_bytes =
            make_tar_gz(&[("payload.bin", payload_bytes, tar::EntryType::Regular, None)]);

        let version = env!("CARGO_PKG_VERSION");
        let target = "linux-x86_64";
        let basename = format!("solstone-journal-{version}-{target}");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-dist-test-linux-mutator-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        let member_dest = "lib/solstone_journal_models/assets/payload.bin";

        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::ModelAsset {
            class: Some(crate::inventory::DeliveryClass::Component),
            source: format!("assets/{archive_name}"),
            dest: member_dest.to_string(),
            mode: 0o644,
            digest_const: "UNUSED".to_string(),
            digest_source: "unused".to_string(),
            targets: vec![target.to_string()],
            archive_slot: None,
        });

        set_bundled_identity_override(
            target,
            "silero-vad-model",
            "1.0.0",
            "https://example.com/silero",
            vec![InputRef {
                name: archive_name.to_string(),
                sha256: crate::digest::sha256_hex(&archive_bytes),
            }],
        );

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![
                ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
                (member_dest.into(), payload_bytes.to_vec(), 0o644),
            ],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: target.into(),
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
            inventory: inv,
            archives: vec![(archive_name.to_string(), archive_bytes)],
            fail_evidence_install: false,
            stage_mutator: Some(mutate_linux_payload),
        };

        let res = promote(&req);
        clear_bundled_identity_override();
        let err = res.unwrap_err();
        assert!(
            err.message
                .contains(&format!("digest-disagreement: {member_dest}")),
            "expected digest-disagreement for {member_dest}, got: {}",
            err.message
        );
        assert!(
            solstone_core_installed_payload::verify_installed_package(
                &work.join("stage"),
                version,
                target
            )
            .is_ok(),
            "verify_installed_package on stage succeeded"
        );

        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reseal_slot_detects_signed_inner_and_rejects_differing_non_executable() {
        use crate::apple::ArchiveMemberSigner;
        use crate::inventory::{ArchiveExecutable, ArchiveSlot};
        use crate::promote::{PromoteRequest, promote};
        use crate::provenance::Provenance;

        let version = env!("CARGO_PKG_VERSION");
        let target = "linux-x86_64";
        let basename = format!("solstone-journal-{version}-{target}");
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-dist-test-reseal-{}-{nanos}",
            std::process::id()
        ));
        let dest = root.join("dest");
        let work = root.join("work");
        let _ = fs::remove_dir_all(&root);

        let temp_dir = tempfile::tempdir().unwrap();
        let cli_file = temp_dir.path().join("rfdetr-cli");
        let cli_bytes = b"original rfdetr-cli binary content";
        fs::write(&cli_file, cli_bytes).unwrap();
        let signer = crate::apple::FakeArchiveMemberSigner::new("promote");
        signer.sign_executable(&cli_file, "rfdetr-cli").unwrap();
        let signed_cli_bytes = fs::read(&cli_file).unwrap();

        let notes_bytes = b"notes.txt content";
        let input_tar_bytes = make_tar_gz(&[
            ("notes.txt", notes_bytes, tar::EntryType::Regular, None),
            ("rfdetr-cli", cli_bytes, tar::EntryType::Regular, None),
        ]);
        let sealed_tar_bytes = make_tar_gz(&[
            ("notes.txt", notes_bytes, tar::EntryType::Regular, None),
            (
                "rfdetr-cli",
                &signed_cli_bytes,
                tar::EntryType::Regular,
                None,
            ),
        ]);

        let source_file_name = "rfdetr-fixture.tar.gz";
        let member_dest = "lib/solstone_journal_models/assets/rfdetr/rfdetr-fixture.tar.gz";

        let slot = ArchiveSlot {
            id: "rfdetr-macos-metal-arm64".into(),
            target: "macos-arm64".into(),
            container: crate::archive_taxonomy::ContainerKind::GzipTar,
            inspect_only: false,
            executables: vec![ArchiveExecutable {
                path: "rfdetr-cli".into(),
                digest_const: "UNUSED".into(),
                digest_source: "unused".into(),
            }],
        };

        let mut inv = test_inventory();
        inv.entry.push(crate::inventory::Entry::ModelAsset {
            class: Some(crate::inventory::DeliveryClass::Component),
            source: format!("assets/{source_file_name}"),
            dest: member_dest.to_string(),
            mode: 0o644,
            digest_const: "UNUSED".to_string(),
            digest_source: "unused".to_string(),
            targets: vec![target.to_string()],
            archive_slot: Some(slot),
        });

        set_bundled_identity_override(
            target,
            "rfdetr-engine",
            "1.0.0",
            "https://example.com/rfdetr",
            vec![InputRef {
                name: source_file_name.to_string(),
                sha256: crate::digest::sha256_hex(&input_tar_bytes),
            }],
        );

        let req = PromoteRequest {
            dest: dest.clone(),
            work: work.clone(),
            tree: vec![
                ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
                (member_dest.into(), sealed_tar_bytes, 0o644),
            ],
            version: version.to_owned(),
            basename: basename.clone(),
            os: "linux".into(),
            arch: target.into(),
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
            inventory: inv.clone(),
            archives: vec![(source_file_name.to_string(), input_tar_bytes.clone())],
            fail_evidence_install: false,
            stage_mutator: None,
        };

        let res = promote(&req);
        clear_bundled_identity_override();
        res.expect("promote reseal slot");

        let evidence_dir = dest
            .parent()
            .unwrap()
            .join(evidence_directory_name(&basename));
        let prov_file: ProvenanceFile = serde_json::from_str(
            &fs::read_to_string(evidence_dir.join(provenance_file_name(version, target))).unwrap(),
        )
        .unwrap();

        let rec = prov_file
            .records
            .iter()
            .find(|r| r.inner_path.as_deref() == Some("rfdetr-cli"))
            .expect("rfdetr-cli record");
        assert_eq!(rec.inner_path, Some("rfdetr-cli".to_string()));
        assert_ne!(rec.pre_signing_sha256, Some(rec.final_sha256.clone()));
        assert_eq!(rec.extracted_sha256, rec.pre_signing_sha256);

        // Case 2: Differing non-executable returns reseal-mismatch: notes.txt
        let bad_sealed_tar_bytes = make_tar_gz(&[
            (
                "notes.txt",
                b"differing notes content",
                tar::EntryType::Regular,
                None,
            ),
            (
                "rfdetr-cli",
                &signed_cli_bytes,
                tar::EntryType::Regular,
                None,
            ),
        ]);

        let bad_dest = root.join("bad_dest");
        let bad_work = root.join("bad_work");

        set_bundled_identity_override(
            target,
            "rfdetr-engine",
            "1.0.0",
            "https://example.com/rfdetr",
            vec![InputRef {
                name: source_file_name.to_string(),
                sha256: crate::digest::sha256_hex(&input_tar_bytes),
            }],
        );

        let bad_req = PromoteRequest {
            dest: bad_dest.clone(),
            work: bad_work.clone(),
            tree: vec![
                ("bin/solstone-core".into(), b"core".to_vec(), 0o755),
                (member_dest.into(), bad_sealed_tar_bytes, 0o644),
            ],
            version: version.to_owned(),
            basename: format!("{basename}-bad"),
            os: "linux".into(),
            arch: target.into(),
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
            inventory: inv,
            archives: vec![(source_file_name.to_string(), input_tar_bytes)],
            fail_evidence_install: false,
            stage_mutator: None,
        };

        let bad_res = promote(&bad_req);
        clear_bundled_identity_override();
        let err = bad_res.unwrap_err();
        assert!(
            err.message.contains("reseal-mismatch: notes.txt"),
            "expected reseal-mismatch: notes.txt, got: {}",
            err.message
        );

        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        let _ = fs::remove_dir_all(&bad_dest);
        let _ = fs::remove_dir_all(&bad_work);
        let _ = fs::remove_dir_all(&evidence_dir);
        let _ = fs::remove_dir_all(&root);
    }
}
