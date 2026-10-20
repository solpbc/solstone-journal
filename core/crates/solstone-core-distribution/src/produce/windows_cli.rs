// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local Windows production from fixed, typed input paths. Input files provide
//! no digests, output destinations, trust overrides or new admission authority.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::windows_archives::{admit_msvc, admit_pdfium, admit_rclone, admit_restic};
use super::windows_build::{build_windows_product, capture_source, read_bounded};
use super::windows_inputs::{
    ControlledInputPaths, LlamaInputPaths, NvattestInputPaths, OnnxInputPaths, RfdetrInputPaths,
    admit_ced, admit_ffmpeg_notices, admit_llama, admit_nvattest, admit_onnx, admit_parakeet,
    admit_rfdetr,
};
use super::windows_stage::{AdmittedWindowsNativeInputs, stage_windows_payload};

const USAGE: &str = "produce windows-x86_64 DEST --inputs LOCAL_JSON --logs FRESH_DIRECTORY (absolute paths required)";

#[derive(Debug, Deserialize)]
struct LockPackage {
    name: String,
    version: String,
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Lock {
    #[serde(default)]
    package: Vec<LockPackage>,
}

/// The digest `population.source` binds to: every external (non-workspace)
/// `[[package]]` row in `Cargo.lock`, identified the same way the committed
/// index identifies its own rows (`name@version (source)`). A workspace
/// member has no `source` and is excluded, so a workspace-internal
/// dependency edge cannot move this digest -- only an added, removed, or
/// upgraded external package can. Path-patched Velopack is absent from this
/// digest on purpose; the no-source check accounts for it.
fn external_population_sha256(lock: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(lock).map_err(|e| e.to_string())?;
    let parsed: Lock = toml_edit::de::from_str(text).map_err(|e| e.to_string())?;
    let mut identities: Vec<String> = parsed
        .package
        .into_iter()
        .filter_map(|package| {
            package
                .source
                .map(|source| format!("{}@{} ({source})", package.name, package.version))
        })
        .collect();
    identities.sort();
    Ok(crate::digest::sha256_hex(identities.join("\n").as_bytes()))
}

/// Workspace members and the explicitly owned excluded evidence crate.
/// An arbitrary workspace exclusion is not evidence of first-party ownership.
const OWNED_EXCLUDED_CRATE_PATHS: &[&str] = &["crates/solstone-core-service-legacy-evidence"];

fn workspace_owned_crate_names(repo: &Path) -> Result<BTreeSet<String>, String> {
    let workspace_manifest = repo.join("core/Cargo.toml");
    let text = std::fs::read_to_string(&workspace_manifest)
        .map_err(|e| format!("{}: {e}", workspace_manifest.display()))?;
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    let workspace = doc
        .get("workspace")
        .and_then(|item| item.as_table())
        .ok_or_else(|| format!("{}: no [workspace] table", workspace_manifest.display()))?;
    let mut declared_paths: Vec<String> = workspace
        .get("members")
        .and_then(|item| item.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect();
    if let Some(excluded) = workspace.get("exclude").and_then(|item| item.as_array()) {
        declared_paths.extend(
            excluded
                .iter()
                .filter_map(|item| item.as_str())
                .filter(|member| OWNED_EXCLUDED_CRATE_PATHS.contains(member))
                .map(str::to_owned),
        );
    }
    let mut names = BTreeSet::new();
    for path in declared_paths {
        let member_manifest = repo.join("core").join(&path).join("Cargo.toml");
        let member_text = std::fs::read_to_string(&member_manifest)
            .map_err(|e| format!("{}: {e}", member_manifest.display()))?;
        let member_doc: toml_edit::DocumentMut = member_text
            .parse()
            .map_err(|e: toml_edit::TomlError| e.to_string())?;
        let name = member_doc
            .get("package")
            .and_then(|item| item.get("name"))
            .and_then(|item| item.as_str())
            .ok_or_else(|| format!("{}: no [package].name", member_manifest.display()))?;
        names.insert(name.to_string());
    }
    Ok(names)
}

/// Cargo.lock rows with no `source` that CLO's 2026-09-16 sign-off already
/// reviewed and accepted as third party despite that: vendored via
/// `[patch.crates-io]` (`core/Cargo.toml`), which strips `source` the same
/// way a workspace member's absence does. Adding a name here is itself the
/// loud, reviewed admission the invariant below requires -- not a silent
/// pass. The licence basis for redistributing `ffmpeg-sys-next` was reviewed
/// separately; it is not a condition on this check.
const KNOWN_THIRD_PARTY_NO_SOURCE_EXCEPTIONS: &[&str] = &["ffmpeg-sys-next"];

const VELOPACK_PUBLISHED_CRATE_SHA256: &str =
    "bd3fd8e7b771e0d0fc4ca67249273ad4eacfabe2562617b3ca42065a5972a58c";
const VELOPACK_DONOR_CRATE_SHA256: &str =
    "cbfbd9ee9818d1155813c4a1024a459763c3782405d5c1ad8f116bd8adaf0b88";
const VELOPACK_DONOR_VERSION: &str = "1.3.39-g8d2844f";
const VELOPACK_LICENSE_URL: &str = "https://raw.githubusercontent.com/velopack/velopack/f2edcbcafb81da5b3c884aaea330e225ad91d8b6/LICENSE";
const VELOPACK_LICENSE_COMMIT: &str = "f2edcbcafb81da5b3c884aaea330e225ad91d8b6";
const VELOPACK_LICENSE_SHA256: &str =
    "91845db83551c877ebbb1118e0fb92e4e527290d23b995c55dcd438b3293943f";
const VELOPACK_LICENSE_LENGTH: i64 = 1091;
const VELOPACK_NOT_THE_PUBLISHED_CRATE: &str =
    "This tree is not the published crates.io crate velopack-1.2.0.";

/// The condition CLO's 2026-09-16 sign-off puts on landing the rebind above:
/// a Cargo.lock row with no `source` that is not sol pbc's own must not
/// pass silently. This is a guard on the no-source population, not a
/// notices change -- it adds no package to `windows-rust-NOTICES.txt` and
/// leaves `population.source_sha256` untouched. A no-source `velopack` row
/// is accounted for by the vendored tree's ORIGIN.toml, not by the
/// name-only exception list.
fn assert_no_source_population_is_accounted_for(
    lock: &[u8],
    owned: &BTreeSet<String>,
    repo: &Path,
) -> Result<(), String> {
    let text = std::str::from_utf8(lock).map_err(|e| e.to_string())?;
    let parsed: Lock = toml_edit::de::from_str(text).map_err(|e| e.to_string())?;
    let mut unaccounted = Vec::new();
    for package in parsed.package {
        if package.source.is_some() {
            continue;
        }
        if owned.contains(&package.name) {
            continue;
        }
        if KNOWN_THIRD_PARTY_NO_SOURCE_EXCEPTIONS.contains(&package.name.as_str()) {
            continue;
        }
        if package.name == "velopack" {
            assert_vendored_velopack_is_accounted_for(repo)?;
            continue;
        }
        unaccounted.push(package.name);
    }
    if !unaccounted.is_empty() {
        return Err(format!(
            "Cargo.lock package(s) with no `source` are neither a declared \
             core/Cargo.toml [workspace] member or explicitly owned excluded crate nor a named \
             third-party exception -- not sol pbc's own by the checkable \
             definition, and must not pass silently: {}",
            unaccounted.join(", ")
        ));
    }
    Ok(())
}

fn origin_string(doc: &toml_edit::DocumentMut, key: &str) -> Result<String, String> {
    doc.get(key)
        .and_then(toml_edit::Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("core/vendor/velopack/ORIGIN.toml missing string {key}"))
}

fn origin_integer(doc: &toml_edit::DocumentMut, key: &str) -> Result<i64, String> {
    doc.get(key)
        .and_then(toml_edit::Item::as_integer)
        .ok_or_else(|| format!("core/vendor/velopack/ORIGIN.toml missing integer {key}"))
}

fn patched_velopack_tree_sha256(vendor: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect_vendor_files(vendor, vendor, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, bytes) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0_u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_vendor_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(current).map_err(|error| format!("{}: {error}", current.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", current.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if file_type.is_dir() {
            collect_vendor_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "ORIGIN.toml" {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        files.push((relative, bytes));
    }
    Ok(())
}

fn assert_vendored_velopack_is_accounted_for(repo: &Path) -> Result<(), String> {
    let vendor = repo.join("core/vendor/velopack");
    let manifest_text = std::fs::read_to_string(vendor.join("Cargo.toml"))
        .map_err(|error| format!("core/vendor/velopack/Cargo.toml: {error}"))?;
    let manifest: toml_edit::DocumentMut = manifest_text
        .parse()
        .map_err(|error: toml_edit::TomlError| error.to_string())?;
    let name = manifest
        .get("package")
        .and_then(|item| item.get("name"))
        .and_then(toml_edit::Item::as_str)
        .ok_or_else(|| "core/vendor/velopack/Cargo.toml: no [package].name".to_owned())?;
    let version = manifest
        .get("package")
        .and_then(|item| item.get("version"))
        .and_then(toml_edit::Item::as_str)
        .ok_or_else(|| "core/vendor/velopack/Cargo.toml: no [package].version".to_owned())?;
    if name != "velopack" || version != "1.2.0" {
        return Err(format!(
            "vendored velopack Cargo.toml must be name velopack version 1.2.0, got {name}@{version}"
        ));
    }
    let origin_text = std::fs::read_to_string(vendor.join("ORIGIN.toml"))
        .map_err(|error| format!("core/vendor/velopack/ORIGIN.toml: {error}"))?;
    let origin: toml_edit::DocumentMut = origin_text
        .parse()
        .map_err(|error: toml_edit::TomlError| error.to_string())?;
    let expected_tree = patched_velopack_tree_sha256(&vendor)?;
    let expected = [
        ("published_crate_sha256", VELOPACK_PUBLISHED_CRATE_SHA256),
        ("donor_crate_sha256", VELOPACK_DONOR_CRATE_SHA256),
        ("donor_version", VELOPACK_DONOR_VERSION),
        ("license_url", VELOPACK_LICENSE_URL),
        ("license_commit", VELOPACK_LICENSE_COMMIT),
        ("license_sha256", VELOPACK_LICENSE_SHA256),
        ("not_the_published_crate", VELOPACK_NOT_THE_PUBLISHED_CRATE),
        ("patched_tree_sha256", expected_tree.as_str()),
    ];
    for (key, value) in expected {
        let got = origin_string(&origin, key)?;
        if got != value {
            return Err(format!(
                "core/vendor/velopack/ORIGIN.toml {key} mismatch: {got}"
            ));
        }
    }
    let license_length = origin_integer(&origin, "license_length")?;
    if license_length != VELOPACK_LICENSE_LENGTH {
        return Err(format!(
            "core/vendor/velopack/ORIGIN.toml license_length mismatch: {license_length}"
        ));
    }
    let vendor_license = std::fs::read(vendor.join("LICENSE"))
        .map_err(|error| format!("core/vendor/velopack/LICENSE: {error}"))?;
    if vendor_license.len() as i64 != VELOPACK_LICENSE_LENGTH
        || crate::digest::sha256_hex(&vendor_license) != VELOPACK_LICENSE_SHA256
    {
        return Err("vendored velopack LICENSE digest or length mismatch".into());
    }
    let shipped = std::fs::read(repo.join("core/distribution/licenses/velopack/LICENSE"))
        .map_err(|error| format!("core/distribution/licenses/velopack/LICENSE: {error}"))?;
    if shipped != vendor_license {
        return Err(
            "core/distribution/licenses/velopack/LICENSE is not byte-identical to the vendor LICENSE"
                .into(),
        );
    }
    Ok(())
}

fn validate_rust_notices(
    index: &[u8],
    notices: &[u8],
    lock: &[u8],
    repo: &Path,
) -> Result<(), String> {
    let index: serde_json::Value = serde_json::from_slice(index).map_err(|e| e.to_string())?;
    let population_sha256 = external_population_sha256(lock)?;
    if index["schema"].as_str() != Some("solstone.windows-rust-notices.v2")
        || index["population"]["source_sha256"].as_str() != Some(&population_sha256)
        || index["notices_sha256"].as_str() != Some(&crate::digest::sha256_hex(notices))
    {
        return Err(
            "Windows Rust notices do not match the current external package population and notice bytes"
                .into(),
        );
    }
    let parsed: Lock =
        toml_edit::de::from_str(std::str::from_utf8(lock).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if parsed
        .package
        .iter()
        .any(|package| package.name == "velopack" && package.source.is_none())
    {
        assert_vendored_velopack_is_accounted_for(repo)?;
        let actual = patched_velopack_tree_sha256(&repo.join("core/vendor/velopack"))?;
        if index["path_dependency_trees"]["velopack@1.2.0 (path+core/vendor/velopack)"].as_str()
            != Some(actual.as_str())
        {
            return Err(
                "Windows Rust source evidence does not match the patched Velopack tree".into(),
            );
        }
    }
    Ok(())
}

#[derive(Debug)]
struct InputPath(PathBuf);

impl<'de> Deserialize<'de> for InputPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let path = PathBuf::deserialize(deserializer)?;
        require_absolute(&path).map_err(serde::de::Error::custom)?;
        Ok(Self(path))
    }
}

fn require_absolute(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(format!(
            "absolute path without parent traversal required: {}",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlledFiles {
    receipt: InputPath,
    evidence: InputPath,
    validation: InputPath,
    source_archive: InputPath,
    cmake_archive: InputPath,
    output_root: InputPath,
}

impl ControlledFiles {
    fn paths(&self) -> ControlledInputPaths<'_> {
        ControlledInputPaths {
            receipt: &self.receipt.0,
            evidence: &self.evidence.0,
            validation: &self.validation.0,
            source_archive: &self.source_archive.0,
            cmake_archive: &self.cmake_archive.0,
            output_root: &self.output_root.0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParakeetFiles {
    build: ControlledFiles,
    model: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnnxFiles {
    build: ControlledFiles,
    mirror_archive: InputPath,
    python_archive: InputPath,
    protoc_archive: InputPath,
    cmake_cache: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RfdetrFiles {
    build: ControlledFiles,
    ggml_bundle: InputPath,
    cmake_cache: InputPath,
    build_options: InputPath,
    subprocess_evidence: InputPath,
    license: InputPath,
    ggml_license: InputPath,
    stb_license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NvattestFiles {
    receipt: InputPath,
    evidence: InputPath,
    validation: InputPath,
    source_archive: InputPath,
    bundle_archive: InputPath,
    output_root: InputPath,
}

impl NvattestFiles {
    fn paths(&self) -> NvattestInputPaths<'_> {
        NvattestInputPaths {
            receipt: &self.receipt.0,
            evidence: &self.evidence.0,
            validation: &self.validation.0,
            source_archive: &self.source_archive.0,
            bundle_archive: &self.bundle_archive.0,
            output_root: &self.output_root.0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveWithLicense {
    archive: InputPath,
    license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MsvcFiles {
    archive: InputPath,
    runtime_license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LlamaFiles {
    receipt: InputPath,
    evidence: InputPath,
    source_archive: InputPath,
    sdk_archive: InputPath,
    cmake_archive: InputPath,
    capture_root: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalInputs {
    llama: LlamaFiles,
    ced: ControlledFiles,
    parakeet: ParakeetFiles,
    onnx: OnnxFiles,
    rfdetr: RfdetrFiles,
    nvattest: NvattestFiles,
    restic: ArchiveWithLicense,
    rclone: ArchiveWithLicense,
    msvc: MsvcFiles,
    pdfium_archive: InputPath,
    ffmpeg_archive: InputPath,
}

impl LocalInputs {
    fn admit(&self, repo: &Path) -> Result<AdmittedWindowsNativeInputs, String> {
        let msvc_input = admit_msvc(&self.msvc.archive.0, &self.msvc.runtime_license.0)?;
        let msvcp140 = msvc_input
            .outputs()
            .get("msvcp140.dll")
            .ok_or("missing admitted msvcp140.dll")?;
        let vcruntime140 = msvc_input
            .outputs()
            .get("vcruntime140.dll")
            .ok_or("missing admitted vcruntime140.dll")?;
        let vcruntime140_1 = msvc_input
            .outputs()
            .get("vcruntime140_1.dll")
            .ok_or("missing admitted vcruntime140_1.dll")?;
        let controlled = vec![
            admit_llama(
                repo,
                LlamaInputPaths {
                    receipt: &self.llama.receipt.0,
                    evidence: &self.llama.evidence.0,
                    source_archive: &self.llama.source_archive.0,
                    sdk_archive: &self.llama.sdk_archive.0,
                    cmake_archive: &self.llama.cmake_archive.0,
                    capture_root: &self.llama.capture_root.0,
                },
            )?,
            admit_ced(repo, self.ced.paths())?,
            admit_parakeet(repo, self.parakeet.build.paths(), &self.parakeet.model.0)?,
            admit_onnx(
                repo,
                OnnxInputPaths {
                    build: self.onnx.build.paths(),
                    mirror_archive: &self.onnx.mirror_archive.0,
                    python_archive: &self.onnx.python_archive.0,
                    protoc_archive: &self.onnx.protoc_archive.0,
                    cmake_cache: &self.onnx.cmake_cache.0,
                },
            )?,
            admit_rfdetr(
                repo,
                RfdetrInputPaths {
                    build: self.rfdetr.build.paths(),
                    ggml_bundle: &self.rfdetr.ggml_bundle.0,
                    cmake_cache: &self.rfdetr.cmake_cache.0,
                    build_options: &self.rfdetr.build_options.0,
                    subprocess_evidence: &self.rfdetr.subprocess_evidence.0,
                    license: &self.rfdetr.license.0,
                    ggml_license: &self.rfdetr.ggml_license.0,
                    stb_license: &self.rfdetr.stb_license.0,
                },
            )?,
            admit_nvattest(
                repo,
                self.nvattest.paths(),
                crate::nvattest_windows::MsvcRuntimeBytes {
                    msvcp140,
                    vcruntime140,
                    vcruntime140_1,
                },
            )?,
        ];
        AdmittedWindowsNativeInputs::from_controlled(controlled)?.with_archives(vec![
            admit_restic(&self.restic.archive.0, &self.restic.license.0)?,
            admit_rclone(&self.rclone.archive.0, &self.rclone.license.0)?,
            msvc_input,
            admit_pdfium(&self.pdfium_archive.0)?,
            admit_ffmpeg_notices(repo, &self.ffmpeg_archive.0)?,
        ])
    }
}

#[derive(Debug)]
struct Args {
    destination: PathBuf,
    inputs: PathBuf,
    logs: PathBuf,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let Some(destination) = args.first() else {
        return Err(USAGE.into());
    };
    let mut inputs = None;
    let mut logs = None;
    let mut flags = args[1..].chunks_exact(2);
    for flag in &mut flags {
        match flag[0].as_str() {
            "--inputs" if inputs.is_none() => inputs = Some(PathBuf::from(&flag[1])),
            "--logs" if logs.is_none() => logs = Some(PathBuf::from(&flag[1])),
            _ => {
                return Err(format!(
                    "unknown or duplicate Windows production option: {}; {USAGE}",
                    flag[0]
                ));
            }
        }
    }
    if !flags.remainder().is_empty() {
        return Err(USAGE.into());
    }
    let result = Args {
        destination: destination.into(),
        inputs: inputs.ok_or(USAGE)?,
        logs: logs.ok_or(USAGE)?,
    };
    for path in [&result.destination, &result.inputs, &result.logs] {
        require_absolute(path)?;
    }
    Ok(result)
}

/// The reviewed native wrapper owns the existing host fence, finite wait,
/// isolated tool environment and cleanup evidence. Build tools remain Unowned.
/// This command produces an unsigned local tree; it cannot sign or promote it.
pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let args = parse_args(args)?;
    if !cfg!(windows) {
        return Err("Windows production requires its native MSVC host".into());
    }
    let inventory_path = crate::inventory::repository_inventory_path(start)
        .ok_or("could not locate canonical distribution inventory")?;
    let inventory =
        crate::validate_distribution_inventory(&inventory_path).map_err(|e| e.to_string())?;
    let repo = inventory_path
        .ancestors()
        .nth(3)
        .ok_or("missing repository root")?;
    let before = capture_source(repo)?;
    let lock_bytes = read_bounded(
        &super::windows_stage::join_components(repo, "core/Cargo.lock"),
        4 * 1024 * 1024,
    )?;
    validate_rust_notices(
        &read_bounded(
            &super::windows_stage::join_components(
                repo,
                "core/distribution/windows-rust-sources.json",
            ),
            4 * 1024 * 1024,
        )?,
        &read_bounded(
            &super::windows_stage::join_components(
                repo,
                "core/distribution/windows-rust-NOTICES.txt",
            ),
            16 * 1024 * 1024,
        )?,
        &lock_bytes,
        repo,
    )?;
    assert_no_source_population_is_accounted_for(
        &lock_bytes,
        &workspace_owned_crate_names(repo)?,
        repo,
    )?;
    let input_bytes = read_bounded(&args.inputs, 1024 * 1024)?;
    let inputs: LocalInputs = serde_json::from_slice(&input_bytes).map_err(|e| e.to_string())?;
    let native = inputs.admit(repo)?;
    if capture_source(repo)? != before {
        return Err("product source changed during native input admission".into());
    }
    let candidate_version =
        super::workspace_version(&repo.join("core/Cargo.toml")).map_err(|e| e.to_string())?;
    super::windows_stage::refuse_unupdated_windows_predecessor(&candidate_version)?;
    let product = build_windows_product(repo, &inventory, &args.logs, &inputs.ffmpeg_archive.0)?;
    // Retain the actual operator file, including original spelling, independently
    // of the inventory and original dependency receipts. No input file is a grant.
    let mut retained_inputs = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(args.logs.join("input-paths.json"))
        .map_err(|e| e.to_string())?;
    retained_inputs
        .write_all(&input_bytes)
        .map_err(|e| e.to_string())?;
    retained_inputs.sync_all().map_err(|e| e.to_string())?;
    let (staged, published) = stage_windows_payload(repo, &product, &native, &args.destination)
        .map_err(|e| e.to_string())?;
    // Informational command output. The existing unsigned manifest remains the
    // payload identity; this summary never admits a replacement tree or bytes.
    serde_json::to_string(&serde_json::json!({
        "target": "windows-x86_64",
        "source": product.evidence().source,
        "destination": published.destination,
        "unsigned_manifest_sha256": staged.unsigned_manifest_sha256,
        "durability_proven": published.durability_proven,
        "signed": false,
        "files": staged.files.iter().map(|file| serde_json::json!({
            "path": file.dest, "sha256": file.digest, "mode": file.mode,
        })).collect::<Vec<_>>(),
        "runtime_edges": staged.runtime_edges.iter().map(|edge| serde_json::json!({
            "importer": edge.importer, "kind": edge.kind, "library": edge.library, "member": edge.member,
        })).collect::<Vec<_>>(),
    })).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // If this test is red, the external (non-workspace) Cargo.lock package
    // population moved and the Windows Rust notices no longer describe it.
    // The fix is to refresh them: rebuild and republish the
    // `dependency_source_companion` archive for the new lock, then
    // regenerate the index. `scripts/refresh_windows_rust_notices.py` does
    // this -- see its module docstring -- for the cases where the vendored
    // bytes provably cannot have moved; it refuses with a clear reason when
    // the external population moved, or when the Windows notice closure moved,
    // either of which needs a fresh `cargo vendor` acquisition instead. A
    // resolved-graph change that leaves that closure untouched is admitted, on
    // the measurement rather than on assertion.
    //
    // DO NOT hand-edit `population.source_sha256` or `cargo_lock_sha256` on
    // their own. `validate_rust_notices` above checks `population.source_sha256`
    // (recomputed from the current lock's external packages) and
    // `notices_sha256` -- `cargo_lock_sha256` is retained only as the anchor
    // `dependency_source_companion` is keyed to for the refresh tool's own
    // `--prior-archive` bookkeeping, and no longer gates this check. Editing
    // either by hand leaves the companion archive naming or contents stale
    // for bytes nobody produced. The shortcut is reachable, it is one line,
    // and it is the reason this comment is here rather than in a tracker.
    //
    // Reached 2026-09-12 by an ordinary dependency pin bump; the bump was
    // reverted rather than the binding weakened.
    //
    // IGNORED 2026-09-15: this check has zero Windows-specific compilation and
    // was running in the default `--workspace --lib --bins` sweep, so any
    // workspace-internal dependency-edge change anywhere in the ~620-package
    // lock could red ordinary, non-Windows journal dev. It is a deliberate
    // pre-release step in the Windows release procedure now, rather than new
    // automation. Run it explicitly with `cargo test -p
    // solstone-core-distribution --lib -- --ignored
    // committed_rust_notices_match_workspace_lock` before cutting a Windows
    // release.
    //
    // REBOUND 2026-09-16: the trigger above was the whole-workspace-lock
    // digest, so a workspace-internal-only dependency edge (no external
    // package touched) still reddened it on every workspace build where
    // someone happened to run the ignored test by hand. The attestation now
    // binds `population.source_sha256` -- the digest of the complete
    // Cargo.lock external-source set (`population.source`, 484 packages
    // today), strictly broader than the Windows notice closure itself
    // (`population.notices`, 396 packages, "conservative inclusion, not
    // exact PE link graph" per `population.metadata_feature_scope`) -- so a
    // workspace-internal edge no longer reds this check, and any added,
    // removed, or upgraded external package still does.
    #[ignore = "run manually before a Windows release per the release playbook, not on every workspace build (2026-09-15)"]
    #[test]
    fn committed_rust_notices_match_workspace_lock() {
        validate_rust_notices(
            include_bytes!("../../../../distribution/windows-rust-sources.json"),
            include_bytes!("../../../../distribution/windows-rust-NOTICES.txt"),
            include_bytes!("../../../../Cargo.lock"),
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../..")),
        )
        .expect("refresh Windows Rust notices when the external package population changes");
    }

    // Unlike the ignored test above, this one does not red on an ordinary
    // workspace-internal dependency edge -- it only reds when a no-`source`
    // row is neither a declared `core/Cargo.toml` `[workspace]` member/
    // exclude nor a named exception, which is a structural change to what
    // the workspace vendors or declares, not routine dependency churn. Runs
    // in the default `cargo test --lib` sweep.
    #[test]
    fn no_source_population_is_accounted_for_at_head() {
        let repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."));
        let owned = workspace_owned_crate_names(repo)
            .expect("resolve core/Cargo.toml's [workspace] members and explicitly owned exclusions to crate names");
        assert_no_source_population_is_accounted_for(
            include_bytes!("../../../../Cargo.lock"),
            &owned,
            repo,
        )
        .expect(
            "Cargo.lock has a no-source package that is neither a declared workspace \
                 member or explicitly owned excluded crate nor a named third-party exception (see \
                 KNOWN_THIRD_PARTY_NO_SOURCE_EXCEPTIONS)",
        );
    }

    const LOCK: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"
dependencies = [
 "external-crate",
]

[[package]]
name = "external-crate"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "deadbeef"
"#;

    // Same external population, a workspace-internal edge added (a second
    // workspace member, and a dependency edge onto it) -- the exact shape of
    // the defect this binding fixes: no external `[[package]]` moved.
    const WORKSPACE_EDGE_MOVED: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"
dependencies = [
 "external-crate",
 "another-workspace-crate",
]

[[package]]
name = "another-workspace-crate"
version = "0.1.0"

[[package]]
name = "external-crate"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "deadbeef"
"#;

    // The external package itself moved -- this must still red.
    const EXTERNAL_PACKAGE_UPGRADED: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"
dependencies = [
 "external-crate",
]

[[package]]
name = "external-crate"
version = "1.2.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cafebabe"
"#;

    #[test]
    fn stale_lock_or_changed_rust_notices_refuse_before_production() {
        let notices = b"original upstream notices";
        let index = serde_json::to_vec(&serde_json::json!({
            "schema": "solstone.windows-rust-notices.v2",
            "population": {
                "source_sha256": external_population_sha256(LOCK).unwrap(),
            },
            "notices_sha256": crate::digest::sha256_hex(notices),
        }))
        .unwrap();
        assert!(validate_rust_notices(&index, notices, LOCK, Path::new(".")).is_ok());

        // A workspace-internal-only edge move leaves the external population
        // -- and therefore the gate -- untouched. Demonstrated, not argued.
        assert!(
            validate_rust_notices(&index, notices, WORKSPACE_EDGE_MOVED, Path::new(".")).is_ok()
        );

        // An external package add/remove/upgrade moves the population digest
        // and reds, exactly as it should.
        assert!(
            validate_rust_notices(&index, notices, EXTERNAL_PACKAGE_UPGRADED, Path::new("."))
                .is_err()
        );

        assert!(validate_rust_notices(&index, b"replaced", LOCK, Path::new(".")).is_err());
        assert!(validate_rust_notices(b"{}", notices, LOCK, Path::new(".")).is_err());
        assert!(validate_rust_notices(b"not-json", notices, LOCK, Path::new(".")).is_err());
    }

    const LOCK_UNOWNED_NO_SOURCE: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"

[[package]]
name = "vendored-third-party"
version = "1.0.0"
"#;

    const LOCK_EXCEPTION_NO_SOURCE: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"

[[package]]
name = "ffmpeg-sys-next"
version = "9.0.0"
"#;

    const LOCK_VELOPACK_NO_SOURCE: &[u8] = br#"
version = 4

[[package]]
name = "workspace-crate"
version = "0.1.0"

[[package]]
name = "velopack"
version = "1.2.0"
"#;

    // The invariant CLO's 2026-09-16 sign-off conditions the rebind on: a
    // no-`source` row that is neither a declared workspace member/exclude
    // nor a named exception must not pass silently. This is the exact shape
    // of the gap the sign-off found -- a vendored `[patch.crates-io]`
    // package loses its `source` the same way a workspace member does.
    #[test]
    fn no_source_row_outside_workspace_and_exceptions_reds() {
        let owned = BTreeSet::from(["workspace-crate".to_string()]);
        let repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."));
        assert!(
            assert_no_source_population_is_accounted_for(LOCK_UNOWNED_NO_SOURCE, &owned, repo)
                .is_err()
        );
    }

    #[test]
    fn no_source_row_that_is_a_named_exception_is_ok() {
        let owned = BTreeSet::from(["workspace-crate".to_string()]);
        let repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."));
        assert!(
            assert_no_source_population_is_accounted_for(LOCK_EXCEPTION_NO_SOURCE, &owned, repo)
                .is_ok()
        );
    }

    #[test]
    fn no_source_velopack_passes_only_with_a_matching_tree() {
        let owned = BTreeSet::from(["workspace-crate".to_string()]);
        let repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."));
        assert_no_source_population_is_accounted_for(LOCK_VELOPACK_NO_SOURCE, &owned, repo)
            .expect("matching vendored velopack tree");

        let temp = tempfile::tempdir().unwrap();
        let temp_repo = temp.path();
        copy_tree(
            &repo.join("core/vendor/velopack"),
            &temp_repo.join("core/vendor/velopack"),
        );
        copy_tree(
            &repo.join("core/distribution/licenses/velopack"),
            &temp_repo.join("core/distribution/licenses/velopack"),
        );
        assert_no_source_population_is_accounted_for(LOCK_VELOPACK_NO_SOURCE, &owned, temp_repo)
            .expect("copied matching tree");

        let bundle = temp_repo.join("core/vendor/velopack/src/bundle.rs");
        let original = std::fs::read(&bundle).unwrap();
        let mut mutated = original.clone();
        mutated.push(b'\n');
        std::fs::write(&bundle, &mutated).unwrap();
        assert!(
            assert_no_source_population_is_accounted_for(
                LOCK_VELOPACK_NO_SOURCE,
                &owned,
                temp_repo
            )
            .is_err(),
            "changed vendor source bytes must fail"
        );
        std::fs::write(&bundle, original).unwrap();
        assert_no_source_population_is_accounted_for(LOCK_VELOPACK_NO_SOURCE, &owned, temp_repo)
            .expect("restored vendor source bytes");
    }

    #[test]
    fn arbitrary_workspace_exclusion_does_not_make_a_dependency_first_party() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("core/crates/owned")).unwrap();
        std::fs::create_dir_all(root.join("core/crates/solstone-core-service-legacy-evidence"))
            .unwrap();
        std::fs::write(root.join("core/Cargo.toml"),
            "[workspace]\nmembers = ['crates/owned']\nexclude = ['vendor/unreviewed', 'crates/solstone-core-service-legacy-evidence']\n").unwrap();
        std::fs::write(
            root.join("core/crates/owned/Cargo.toml"),
            "[package]\nname = 'owned'\n",
        )
        .unwrap();
        std::fs::write(
            root.join("core/crates/solstone-core-service-legacy-evidence/Cargo.toml"),
            "[package]\nname = 'solstone-core-service-legacy-evidence'\n",
        )
        .unwrap();
        let owned = workspace_owned_crate_names(root).unwrap();
        assert!(owned.contains("owned"));
        assert!(owned.contains("solstone-core-service-legacy-evidence"));
        assert!(!owned.contains("unreviewed"));
        assert!(
            assert_no_source_population_is_accounted_for(LOCK_UNOWNED_NO_SOURCE, &owned, root)
                .is_err()
        );
    }

    #[test]
    fn retained_notice_binding_refuses_a_changed_vendor_even_with_updated_origin() {
        let repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.."));
        let temp = tempfile::tempdir().unwrap();
        let candidate = temp.path();
        for relative in [
            "core/vendor/velopack",
            "core/distribution/licenses/velopack",
        ] {
            copy_tree(&repo.join(relative), &candidate.join(relative));
        }
        let notices = b"notice";
        let original_tree =
            patched_velopack_tree_sha256(&candidate.join("core/vendor/velopack")).unwrap();
        let index = serde_json::to_vec(&serde_json::json!({
            "schema": "solstone.windows-rust-notices.v2",
            "population": {"source_sha256": external_population_sha256(LOCK_VELOPACK_NO_SOURCE).unwrap()},
            "notices_sha256": crate::digest::sha256_hex(notices),
            "path_dependency_trees": {"velopack@1.2.0 (path+core/vendor/velopack)": original_tree}
        })).unwrap();
        validate_rust_notices(&index, notices, LOCK_VELOPACK_NO_SOURCE, candidate).unwrap();
        let source = candidate.join("core/vendor/velopack/src/bundle.rs");
        let mut bytes = std::fs::read(&source).unwrap();
        bytes.push(b'\n');
        std::fs::write(&source, bytes).unwrap();
        let origin_path = candidate.join("core/vendor/velopack/ORIGIN.toml");
        let origin_text = std::fs::read_to_string(&origin_path).unwrap();
        let origin: toml_edit::DocumentMut = origin_text.parse().unwrap();
        let previous_tree = origin_string(&origin, "patched_tree_sha256").unwrap();
        assert_eq!(origin_text.matches(&previous_tree).count(), 1);
        let changed_tree =
            patched_velopack_tree_sha256(&candidate.join("core/vendor/velopack")).unwrap();
        std::fs::write(
            origin_path,
            origin_text.replace(&previous_tree, &changed_tree),
        )
        .unwrap();
        assert_vendored_velopack_is_accounted_for(candidate).unwrap();
        assert!(
            validate_rust_notices(&index, notices, LOCK_VELOPACK_NO_SOURCE, candidate).is_err()
        );
    }

    fn copy_tree(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let to = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), to).unwrap();
            }
        }
    }

    fn arguments(root: &Path) -> Vec<String> {
        [
            root.join("payload").display().to_string(),
            "--inputs".into(),
            root.join("inputs.json").display().to_string(),
            "--logs".into(),
            root.join("logs").display().to_string(),
        ]
        .into()
    }

    #[test]
    fn command_requires_explicit_paths_and_refuses_extra_authorities() {
        let root = tempfile::tempdir().unwrap();
        let args = arguments(root.path());
        assert_eq!(parse_args(&args).unwrap().logs, root.path().join("logs"));
        for extra in [
            vec!["--inputs", "other"],
            vec!["--signature-key", "key"],
            vec!["trailing"],
        ] {
            let mut candidate = args.clone();
            candidate.extend(extra.into_iter().map(String::from));
            assert!(parse_args(&candidate).is_err());
        }
        assert!(parse_args(&args[..3]).is_err());
        let mut relative = args;
        relative[0] = "relative".into();
        assert!(parse_args(&relative).is_err());
    }

    #[test]
    fn local_input_objects_reject_relative_paths_and_unrecognized_evidence_fields() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input");
        let value = serde_json::json!({"archive": path, "license": path});
        assert!(serde_json::from_value::<ArchiveWithLicense>(value.clone()).is_ok());
        for field in ["sha256", "destination", "signature", "optional"] {
            let mut candidate = value.clone();
            candidate[field] = serde_json::json!("override");
            assert!(serde_json::from_value::<ArchiveWithLicense>(candidate).is_err());
        }
        let mut relative = value;
        relative["license"] = serde_json::json!("relative");
        assert!(serde_json::from_value::<ArchiveWithLicense>(relative).is_err());
    }

    #[test]
    fn local_inputs_rejects_missing_nvattest_and_unknown_fields() {
        let root = tempfile::tempdir().unwrap();
        let p = root.path().join("file");
        let controlled = serde_json::json!({
            "receipt": p,
            "evidence": p,
            "validation": p,
            "source_archive": p,
            "cmake_archive": p,
            "output_root": p,
        });
        let nvattest = serde_json::json!({
            "receipt": p,
            "evidence": p,
            "validation": p,
            "source_archive": p,
            "bundle_archive": p,
            "output_root": p,
        });
        let valid = serde_json::json!({
            "llama": {
                "receipt": p,
                "evidence": p,
                "source_archive": p,
                "sdk_archive": p,
                "cmake_archive": p,
                "capture_root": p,
            },
            "ced": controlled,
            "parakeet": {
                "build": controlled,
                "model": p,
            },
            "onnx": {
                "build": controlled,
                "mirror_archive": p,
                "python_archive": p,
                "protoc_archive": p,
                "cmake_cache": p,
            },
            "rfdetr": {
                "build": controlled,
                "ggml_bundle": p,
                "cmake_cache": p,
                "build_options": p,
                "subprocess_evidence": p,
                "license": p,
                "ggml_license": p,
                "stb_license": p,
            },
            "nvattest": nvattest,
            "restic": {"archive": p, "license": p},
            "rclone": {"archive": p, "license": p},
            "msvc": {"archive": p, "runtime_license": p},
            "pdfium_archive": p,
            "ffmpeg_archive": p,
        });
        assert!(serde_json::from_value::<LocalInputs>(valid.clone()).is_ok());

        let mut without_nvattest = valid.clone();
        without_nvattest.as_object_mut().unwrap().remove("nvattest");
        let error = serde_json::from_value::<LocalInputs>(without_nvattest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing field `nvattest`"), "{error}");

        let mut with_unknown = valid.clone();
        with_unknown["unknown_field"] = serde_json::json!("forbidden");
        assert!(serde_json::from_value::<LocalInputs>(with_unknown).is_err());

        // The verifier entry names paths only: no pin, digest, destination,
        // revision or override can reach admission through the input file.
        for field in [
            "source_sha256",
            "bundle_sha256",
            "manifest_sha256",
            "revision",
            "pins",
            "dest",
            "ca_bundle",
            "allow_imports",
        ] {
            let mut with_override = valid.clone();
            with_override["nvattest"][field] = serde_json::json!("0".repeat(64));
            let error = serde_json::from_value::<LocalInputs>(with_override)
                .unwrap_err()
                .to_string();
            assert!(error.contains("unknown field"), "{field}: {error}");
        }
        for field in [
            "receipt",
            "evidence",
            "validation",
            "source_archive",
            "bundle_archive",
            "output_root",
        ] {
            let mut relative = valid.clone();
            relative["nvattest"][field] = serde_json::json!("relative/path");
            assert!(
                serde_json::from_value::<LocalInputs>(relative).is_err(),
                "{field}"
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_native_run_refuses_before_input_read_or_output_creation() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            run_cli(root.path(), &arguments(root.path()))
                .unwrap_err()
                .contains("native MSVC host")
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
