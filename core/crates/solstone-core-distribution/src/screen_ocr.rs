// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Acquisition and staging for the independent screen OCR helper closure.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;

use crate::digest::sha256_hex;
use crate::inventory::{Entry, Inventory, Target};
use crate::select::ArtifactId;
use crate::stage;

pub const HELPER_PACKAGE: &str = "solstone-screen-ocr-helper";
pub const HELPER_BIN: &str = "solstone-screen-ocr-helper";
pub const BUNDLE_DEST: &str = "lib/solstone-screen-ocr";
pub const CACHE_ROOT: &str = "target/screen-ocr-cache";
pub const HELPER_MANIFEST: &str = "core/screen-ocr-helper/Cargo.toml";
pub const HELPER_LOCK: &str = "core/screen-ocr-helper/Cargo.lock";
pub const RECIPE: &str = "core/screen-ocr-helper/recipe.toml";
pub const CARGO_NOTICES: &str = "core/distribution/screen-ocr-rust-NOTICES.txt";
pub const CARGO_SOURCE_INDEX: &str = "core/distribution/screen-ocr-rust-sources.json";

const DETECTOR: &str = "detector.onnx";
const RECOGNIZER: &str = "recognizer.onnx";
const DICTIONARY_SOURCE: &str = "inference.yml";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub protocol_version: u16,
    pub source: Source,
    pub settings: serde_json::Value,
    pub limits: serde_json::Value,
    pub models: Models,
    pub runtime: Runtime,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub oar_git: String,
    pub oar_revision: String,
    pub oar_default_features: bool,
    pub oar_features: Vec<String>,
    pub ort_version: String,
    pub ort_features: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub detector: Model,
    pub recognizer: Model,
    pub dictionary: Dictionary,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub filename: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dictionary {
    pub filename: String,
    pub source_url: String,
    pub source_sha256: String,
    pub materialized_entries: usize,
    pub materialized_final_newline: bool,
    pub materialized_sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub version: String,
    pub linux_max_glibc: String,
    pub linux_build_lane: String,
    pub linux_x86_64: Archive,
    pub macos_arm64: Archive,
    pub linux_aarch64: Archive,
    pub windows_x86_64: Archive,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Archive {
    pub filename: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetSpec {
    pub id: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub triple: &'static str,
    pub runtime_member: &'static str,
    pub runtime_name: &'static str,
    pub dependency_member: Option<&'static str>,
    pub dependency_name: Option<&'static str>,
}

pub const TARGETS: &[TargetSpec] = &[
    TargetSpec {
        id: "linux-x86_64",
        os: "linux",
        arch: "x86_64",
        triple: "x86_64-unknown-linux-gnu",
        runtime_member: "onnxruntime/capi/libonnxruntime.so.1.31.0",
        runtime_name: "libonnxruntime.so.1.31.0",
        dependency_member: Some("onnxruntime/capi/libonnxruntime_providers_shared.so"),
        dependency_name: Some("libonnxruntime_providers_shared.so"),
    },
    TargetSpec {
        id: "linux-aarch64",
        os: "linux",
        arch: "aarch64",
        triple: "aarch64-unknown-linux-gnu",
        runtime_member: "onnxruntime/capi/libonnxruntime.so.1.31.0",
        runtime_name: "libonnxruntime.so.1.31.0",
        dependency_member: Some("onnxruntime/capi/libonnxruntime_providers_shared.so"),
        dependency_name: Some("libonnxruntime_providers_shared.so"),
    },
    TargetSpec {
        id: "macos-arm64",
        os: "macos",
        arch: "arm64",
        triple: "aarch64-apple-darwin",
        runtime_member: "onnxruntime/capi/libonnxruntime.1.31.0.dylib",
        runtime_name: "libonnxruntime.1.31.0.dylib",
        dependency_member: None,
        dependency_name: None,
    },
    TargetSpec {
        id: "windows-x86_64",
        os: "windows",
        arch: "x86_64",
        triple: "x86_64-pc-windows-msvc",
        runtime_member: "onnxruntime/capi/onnxruntime.dll",
        runtime_name: "onnxruntime.dll",
        dependency_member: Some("onnxruntime/capi/onnxruntime_providers_shared.dll"),
        dependency_name: Some("onnxruntime_providers_shared.dll"),
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedFile {
    pub dest: String,
    pub mode: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedBundle {
    pub target: String,
    pub files: Vec<StagedFile>,
}

#[derive(Deserialize)]
struct CargoLock {
    package: Vec<CargoLockPackage>,
}

#[derive(Deserialize)]
struct CargoLockPackage {
    name: String,
    source: Option<String>,
}

impl Recipe {
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "screen OCR recipe is not UTF-8")?;
        let recipe: Self =
            toml_edit::de::from_str(text).map_err(|error| format!("screen OCR recipe: {error}"))?;
        if recipe.schema_version != 1 || recipe.protocol_version != 1 {
            return Err("unsupported screen OCR recipe schema".into());
        }
        Ok(recipe)
    }

    pub fn runtime_archive(&self, target: &str) -> Option<&Archive> {
        match target {
            "linux-x86_64" => Some(&self.runtime.linux_x86_64),
            "linux-aarch64" => Some(&self.runtime.linux_aarch64),
            "macos-arm64" => Some(&self.runtime.macos_arm64),
            "windows-x86_64" => Some(&self.runtime.windows_x86_64),
            _ => None,
        }
    }
}

pub fn verify_oar_revision(recipe_bytes: &[u8], helper_lock: &[u8]) -> Result<(), String> {
    let recipe = Recipe::load(recipe_bytes)?;
    let lock_text =
        std::str::from_utf8(helper_lock).map_err(|_| "screen OCR Cargo.lock is not UTF-8")?;
    let lock: CargoLock = toml_edit::de::from_str(lock_text)
        .map_err(|error| format!("screen OCR Cargo.lock: {error}"))?;
    let expected = format!(
        "git+{}?rev={}#{}",
        recipe.source.oar_git, recipe.source.oar_revision, recipe.source.oar_revision
    );
    let oar_sources = lock
        .package
        .iter()
        .filter(|package| package.name == "oar-ocr")
        .filter_map(|package| package.source.as_deref())
        .collect::<Vec<_>>();
    if oar_sources.len() != 1 || oar_sources[0] != expected {
        return Err("screen OCR Cargo.lock does not bind the pinned OAR revision".into());
    }
    Ok(())
}

pub fn target_spec(target: &str) -> Option<&'static TargetSpec> {
    TARGETS.iter().find(|item| item.id == target)
}

/// Fetch exactly the helper's pinned models, dictionary source, and target
/// runtime archive. `fetch` is the only origin boundary and is injectable for
/// fixture tests.
pub fn acquire_with(
    recipe_bytes: &[u8],
    target: &str,
    cache: &Path,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    let recipe = Recipe::load(recipe_bytes)?;
    let archive = recipe
        .runtime_archive(target)
        .ok_or_else(|| format!("unsupported screen OCR target {target}"))?;
    let mut inputs = vec![
        (
            &recipe.models.detector.url,
            &recipe.models.detector.sha256,
            DETECTOR,
        ),
        (
            &recipe.models.recognizer.url,
            &recipe.models.recognizer.sha256,
            RECOGNIZER,
        ),
        (
            &recipe.models.dictionary.source_url,
            &recipe.models.dictionary.source_sha256,
            DICTIONARY_SOURCE,
        ),
        (&archive.url, &archive.sha256, archive.filename.as_str()),
    ];
    fs::create_dir_all(cache).map_err(|error| error.to_string())?;
    for (url, expected, name) in inputs.drain(..) {
        let path = cache.join(name);
        let bytes = match fs::read(&path) {
            Ok(bytes) if sha256_hex(&bytes) == *expected => bytes,
            _ => fetch(url)?,
        };
        if sha256_hex(&bytes) != *expected {
            return Err(format!("screen OCR input digest mismatch: {name}"));
        }
        let temporary = cache.join(format!(".{name}.part"));
        fs::write(&temporary, &bytes).map_err(|error| error.to_string())?;
        fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn acquire_from_repo(repo: &Path, target: &str, cache: &Path) -> Result<(), String> {
    let recipe = fs::read(repo.join(RECIPE)).map_err(|error| error.to_string())?;
    let target_cache = cache.join(target);
    acquire_with(&recipe, target, &target_cache, |url| {
        let pin = Recipe::load(&recipe)?;
        let archive = pin.runtime_archive(target).filter(|item| item.url == url);
        let (sha, size) = if let Some(archive) = archive {
            (&archive.sha256, None)
        } else if pin.models.detector.url == url {
            (&pin.models.detector.sha256, None)
        } else if pin.models.recognizer.url == url {
            (&pin.models.recognizer.sha256, None)
        } else if pin.models.dictionary.source_url == url {
            (&pin.models.dictionary.source_sha256, None)
        } else {
            return Err("screen OCR fetch refused unknown origin".into());
        };
        let temp = target_cache.join("download.part");
        solstone_core_artifact_download::ensure_verified_url(url, sha, size, &temp, |_, _| {})
            .map_err(|error| error.to_string())?;
        let bytes = fs::read(&temp).map_err(|error| error.to_string())?;
        let _ = fs::remove_file(temp);
        Ok(bytes)
    })
}

/// Materialize the pinned Paddle `character_dict` YAML sequence. The caller
/// verifies both the source and resulting digest against the recipe.
pub fn materialize_dictionary(yaml: &[u8]) -> Result<Vec<u8>, String> {
    let text = std::str::from_utf8(yaml).map_err(|_| "dictionary source is not UTF-8")?;
    let mut in_dictionary = false;
    let mut indent = 0;
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !in_dictionary {
            if trimmed == "character_dict:" {
                in_dictionary = true;
                indent = line.len() - trimmed.len();
            }
            continue;
        }
        let current_indent = line.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // The pinned Paddle YAML uses an indentless block sequence.
        if current_indent < indent || (current_indent == indent && !trimmed.starts_with("- ")) {
            break;
        }
        let Some(value) = trimmed.strip_prefix("- ") else {
            return Err("invalid dictionary sequence".into());
        };
        entries.push(parse_yaml_scalar(value.trim_matches([' ', '\t']))?);
    }
    if entries.is_empty() {
        return Err("dictionary sequence missing".into());
    }
    let mut out = Vec::new();
    for entry in entries {
        if entry.contains(['\n', '\r', '\0']) {
            return Err("invalid dictionary entry".into());
        }
        out.extend_from_slice(entry.as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

fn parse_yaml_scalar(value: &str) -> Result<String, String> {
    if value.starts_with('"') {
        serde_json::from_str(value).map_err(|_| "invalid quoted dictionary scalar".into())
    } else if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        Ok(value[1..value.len() - 1].replace("''", "'"))
    } else if value.is_empty() || value.contains(" #") {
        Err("invalid plain dictionary scalar".into())
    } else {
        Ok(value.to_owned())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn stage_bundle(
    recipe_bytes: &[u8],
    target: &str,
    helper_triple: &str,
    helper: &[u8],
    detector: &[u8],
    recognizer: &[u8],
    dictionary_yaml: &[u8],
    wheel: &[u8],
    helper_manifest: &[u8],
    helper_lock: &[u8],
    helper_license: &[u8],
    apache_license: &[u8],
    source_directions: &[u8],
    attribution: &[u8],
    rust_notices: &[u8],
    rust_source_index: &[u8],
    bundle_dest: &str,
) -> Result<StagedBundle, String> {
    let recipe = Recipe::load(recipe_bytes)?;
    let spec =
        target_spec(target).ok_or_else(|| format!("unsupported screen OCR target {target}"))?;
    verify_oar_revision(recipe_bytes, helper_lock)?;
    if helper_triple != spec.triple || helper.is_empty() {
        return Err("screen OCR helper target/artifact mismatch".into());
    }
    if sha256_hex(detector) != recipe.models.detector.sha256
        || sha256_hex(recognizer) != recipe.models.recognizer.sha256
        || sha256_hex(dictionary_yaml) != recipe.models.dictionary.source_sha256
    {
        return Err("screen OCR model or dictionary source digest mismatch".into());
    }
    let dictionary = materialize_dictionary(dictionary_yaml)?;
    if dictionary.iter().filter(|byte| **byte == b'\n').count()
        != recipe.models.dictionary.materialized_entries
        || !dictionary.ends_with(b"\n")
        || sha256_hex(&dictionary) != recipe.models.dictionary.materialized_sha256
    {
        return Err("screen OCR materialized dictionary mismatch".into());
    }
    let archive = recipe
        .runtime_archive(target)
        .ok_or_else(|| format!("missing screen OCR runtime pin for {target}"))?;
    if sha256_hex(wheel) != archive.sha256 {
        return Err("screen OCR runtime archive digest mismatch".into());
    }
    let members = crate::zip::read_members(wheel).map_err(|error| error.to_string())?;
    let runtime = crate::zip::member(&members, spec.runtime_member)
        .map_err(|_| "screen OCR runtime library missing")?;
    let dependency = spec
        .dependency_member
        .map(|member| {
            crate::zip::member(&members, member)
                .map_err(|_| "screen OCR runtime dependency missing")
        })
        .transpose()?;
    let license = crate::zip::member(&members, "onnxruntime/LICENSE")
        .map_err(|_| "screen OCR ORT license missing")?;
    let third_party = crate::zip::member(&members, "onnxruntime/ThirdPartyNotices.txt")
        .map_err(|_| "screen OCR ORT third-party notices missing")?;
    if [
        helper_license,
        apache_license,
        source_directions,
        attribution,
        rust_notices,
        rust_source_index,
    ]
    .into_iter()
    .any(<[u8]>::is_empty)
    {
        return Err("screen OCR license or Cargo notice closure missing".into());
    }
    verify_rust_source_index(
        rust_source_index,
        helper_manifest,
        helper_lock,
        recipe_bytes,
        source_directions,
        rust_notices,
        &recipe,
    )?;
    let mut files = vec![
        file(bundle_dest, "recipe.toml", 0o644, recipe_bytes),
        file(bundle_dest, "models/detector.onnx", 0o644, detector),
        file(bundle_dest, "models/recognizer.onnx", 0o644, recognizer),
        file(bundle_dest, "models/dictionary.txt", 0o644, &dictionary),
        file(
            bundle_dest,
            &format!("runtime/{}", spec.runtime_name),
            0o755,
            runtime,
        ),
        file(bundle_dest, "runtime/LICENSE", 0o644, license),
        file(
            bundle_dest,
            "runtime/ThirdPartyNotices.txt",
            0o644,
            third_party,
        ),
        file(
            bundle_dest,
            "licenses/AGPL-3.0-only.txt",
            0o644,
            helper_license,
        ),
        file(
            bundle_dest,
            "licenses/Apache-2.0.txt",
            0o644,
            apache_license,
        ),
        file(
            bundle_dest,
            "licenses/SOURCE-DIRECTIONS.md",
            0o644,
            source_directions,
        ),
        file(bundle_dest, "licenses/NOTICE", 0o644, attribution),
        file(
            bundle_dest,
            "licenses/rust-NOTICES.txt",
            0o644,
            rust_notices,
        ),
        file(
            bundle_dest,
            "provenance/rust-sources.json",
            0o644,
            rust_source_index,
        ),
    ];
    if let (Some(name), Some(bytes)) = (spec.dependency_name, dependency) {
        files.push(file(bundle_dest, &format!("runtime/{name}"), 0o755, bytes));
    }
    let provenance = json!({
        "schema": "solstone.screen-ocr-bundle.v1",
        "target": target,
        "helper_triple": helper_triple,
        "helper_sha256": sha256_hex(helper),
        "helper_bytes": helper.len(),
        "helper_manifest_sha256": sha256_hex(helper_manifest),
        "helper_lock_sha256": sha256_hex(helper_lock),
        "rust_notices_sha256": sha256_hex(rust_notices),
        "rust_source_index_sha256": sha256_hex(rust_source_index),
        "recipe_sha256": sha256_hex(recipe_bytes),
        "oar_git": recipe.source.oar_git,
        "oar_revision": recipe.source.oar_revision,
        "ort_version": recipe.runtime.version,
        "detector_url": recipe.models.detector.url,
        "detector_sha256": sha256_hex(detector),
        "recognizer_url": recipe.models.recognizer.url,
        "recognizer_sha256": sha256_hex(recognizer),
        "dictionary_source_url": recipe.models.dictionary.source_url,
        "dictionary_source_sha256": sha256_hex(dictionary_yaml),
        "dictionary_sha256": sha256_hex(&dictionary),
        "runtime_archive_url": archive.url,
        "runtime_archive_sha256": sha256_hex(wheel),
        "runtime_member": spec.runtime_member,
        "runtime_sha256": sha256_hex(runtime),
        "runtime_dependency_member": spec.dependency_member,
        "runtime_dependency_sha256": dependency.map(sha256_hex),
        "settings": recipe.settings,
        "limits": recipe.limits,
    });
    let provenance = serde_json::to_vec_pretty(&provenance).map_err(|error| error.to_string())?;
    files.push(file(
        bundle_dest,
        "provenance/bundle.json",
        0o644,
        &provenance,
    ));
    files.sort_by(|left, right| left.dest.cmp(&right.dest));
    Ok(StagedBundle {
        target: target.into(),
        files,
    })
}

fn verify_rust_source_index(
    bytes: &[u8],
    helper_manifest: &[u8],
    helper_lock: &[u8],
    recipe_bytes: &[u8],
    source_directions: &[u8],
    rust_notices: &[u8],
    recipe: &Recipe,
) -> Result<(), String> {
    let index: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "screen OCR Cargo source index is invalid")?;
    if index["schema"] != "solstone.screen-ocr-rust-notices.v1"
        || index["oar_git"] != recipe.source.oar_git
        || index["oar_revision"] != recipe.source.oar_revision
    {
        return Err("screen OCR Cargo source index identity mismatch".into());
    }
    for (field, input) in [
        ("helper_manifest_sha256", helper_manifest),
        ("cargo_lock_sha256", helper_lock),
        ("recipe_sha256", recipe_bytes),
        ("source_directions_sha256", source_directions),
        ("notices_sha256", rust_notices),
    ] {
        let expected = sha256_hex(input);
        if index[field].as_str() != Some(expected.as_str()) {
            return Err("screen OCR Cargo source index digest mismatch".into());
        }
    }
    Ok(())
}

fn file(base: &str, relative: &str, mode: u32, bytes: &[u8]) -> StagedFile {
    StagedFile {
        dest: format!("{base}/{relative}"),
        mode,
        bytes: bytes.to_vec(),
    }
}

pub fn prepare_from_cache(
    repo: &Path,
    cache_root: &Path,
    target: &str,
    helper_path: &Path,
    helper_triple: &str,
) -> Result<StagedBundle, String> {
    let helper = fs::read(helper_path).map_err(|_| "screen OCR helper artifact missing")?;
    prepare_with_helper_bytes(repo, cache_root, target, &helper, helper_triple)
}

pub fn prepare_with_helper_bytes(
    repo: &Path,
    cache_root: &Path,
    target: &str,
    helper: &[u8],
    helper_triple: &str,
) -> Result<StagedBundle, String> {
    let recipe_bytes = fs::read(repo.join(RECIPE)).map_err(|_| "screen OCR recipe missing")?;
    let recipe = Recipe::load(&recipe_bytes)?;
    let archive = recipe
        .runtime_archive(target)
        .ok_or_else(|| format!("unsupported screen OCR target {target}"))?;
    let cache = cache_root.join(target);
    let detector = fs::read(cache.join(DETECTOR)).map_err(|_| "screen OCR detector missing")?;
    let recognizer =
        fs::read(cache.join(RECOGNIZER)).map_err(|_| "screen OCR recognizer missing")?;
    let dictionary = fs::read(cache.join(DICTIONARY_SOURCE))
        .map_err(|_| "screen OCR dictionary source missing")?;
    let wheel = fs::read(cache.join(&archive.filename))
        .map_err(|_| "screen OCR runtime archive missing")?;
    let helper_lock =
        fs::read(repo.join(HELPER_LOCK)).map_err(|_| "screen OCR Cargo.lock missing")?;
    let helper_license = fs::read(repo.join("core/screen-ocr-helper/LICENSE"))
        .map_err(|_| "screen OCR AGPL license missing")?;
    let apache_license = fs::read(repo.join("core/screen-ocr-helper/LICENSE-APACHE"))
        .map_err(|_| "screen OCR Apache license missing")?;
    let source_directions = fs::read(repo.join("core/screen-ocr-helper/SOURCE-DIRECTIONS.md"))
        .map_err(|_| "screen OCR source directions missing")?;
    let attribution = fs::read(repo.join("core/screen-ocr-helper/NOTICE"))
        .map_err(|_| "screen OCR attribution missing")?;
    let rust_notices =
        fs::read(repo.join(CARGO_NOTICES)).map_err(|_| "screen OCR Cargo notices missing")?;
    let rust_source_index = fs::read(repo.join(CARGO_SOURCE_INDEX))
        .map_err(|_| "screen OCR Cargo source index missing")?;
    let helper_manifest =
        fs::read(repo.join(HELPER_MANIFEST)).map_err(|_| "screen OCR helper manifest missing")?;
    stage_bundle(
        &recipe_bytes,
        target,
        helper_triple,
        helper,
        &detector,
        &recognizer,
        &dictionary,
        &wheel,
        &helper_manifest,
        &helper_lock,
        &helper_license,
        &apache_license,
        &source_directions,
        &attribution,
        &rust_notices,
        &rust_source_index,
        BUNDLE_DEST,
    )
}

pub fn stage_files(bundle: &StagedBundle, root: &Path) -> Result<(), String> {
    for member in &bundle.files {
        stage::write_staged_file_mode(root, &member.dest, &member.bytes, member.mode)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn declared_files(bundle: &StagedBundle) -> Vec<(String, u32, String)> {
    bundle
        .files
        .iter()
        .map(|member| (member.dest.clone(), member.mode, sha256_hex(&member.bytes)))
        .collect()
}

pub fn helper_target_path(root: &Path, target: &str) -> PathBuf {
    root.join(target)
        .join("release")
        .join(if target == "windows-x86_64" {
            format!("{HELPER_BIN}.exe")
        } else {
            HELPER_BIN.to_owned()
        })
}

pub fn prepare_for_inventory(
    repo: &Path,
    catalog_cache: &Path,
    inventory: &Inventory,
    target_id: &str,
    artifacts: &BTreeMap<ArtifactId, PathBuf>,
) -> Result<Option<StagedBundle>, String> {
    if !inventory.entry.iter().any(|entry| {
        matches!(entry, Entry::ScreenOcr { targets, .. } if targets.iter().any(|target| target == target_id))
    }) {
        return Ok(None);
    }
    let target = inventory
        .target
        .iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| format!("screen OCR inventory target missing: {target_id}"))?;
    let triple = helper_triple_for(inventory, target)?;
    let path = artifacts
        .get(&ArtifactId {
            package: HELPER_PACKAGE.into(),
            bin: HELPER_BIN.into(),
            triple: triple.clone(),
        })
        .ok_or_else(|| format!("screen OCR helper artifact missing: {triple}"))?;
    if !path.is_file() {
        return Err(format!(
            "screen OCR helper artifact missing: {}",
            path.display()
        ));
    }
    prepare_from_cache(
        repo,
        &catalog_cache.join("screen-ocr"),
        target_id,
        path,
        &triple,
    )
    .map(Some)
}

fn helper_triple_for(inventory: &Inventory, target: &Target) -> Result<String, String> {
    let entry = inventory
        .entry
        .iter()
        .find(|entry| match entry {
            Entry::Bin {
                package,
                bin,
                targets,
                ..
            } => package == HELPER_PACKAGE && bin == HELPER_BIN && targets.contains(&target.id),
            _ => false,
        })
        .ok_or_else(|| format!("screen OCR helper inventory entry missing: {}", target.id))?;
    let Entry::Bin { lane, .. } = entry else {
        unreachable!("filtered helper entry is a bin")
    };
    Ok(target.triple_for_lane(target.lane_for(lane)).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_has_all_four_immutable_runtime_archive_pins() {
        let recipe =
            Recipe::load(include_bytes!("../../../screen-ocr-helper/recipe.toml")).unwrap();
        verify_oar_revision(
            include_bytes!("../../../screen-ocr-helper/recipe.toml"),
            include_bytes!("../../../screen-ocr-helper/Cargo.lock"),
        )
        .unwrap();
        let expected = [
            (
                "linux-x86_64",
                "37fd78cee5160c7a43a1730ccb3682ffd880af9c9e80385d625c0c2f8b125809",
            ),
            (
                "linux-aarch64",
                "ad663106f6eeff3d454f24a786450459d07f30e74863851104fc1b8b3f368127",
            ),
            (
                "macos-arm64",
                "0ba02a44acb6203040354d9a1f160e3f37a43feac7bb05caa3e0ea545efed505",
            ),
            (
                "windows-x86_64",
                "73e0165d58ece068c2a8a1c477c90b38e5a8adbbd399fdfdfd4bd79cbc28ff8d",
            ),
        ];
        for (target, digest) in expected {
            assert_eq!(recipe.runtime_archive(target).unwrap().sha256, digest);
            assert!(target_spec(target).is_some());
        }
        assert_eq!(recipe.runtime.version, "1.31.0");
        assert_eq!(recipe.runtime.linux_max_glibc, "2.34");
        assert_eq!(recipe.runtime.linux_build_lane, "GNU Zig");
    }

    #[test]
    fn dictionary_materializer_handles_yaml_scalars_and_final_newline() {
        let bytes = materialize_dictionary(
            "PostProcess:\n  character_dict:\n    - a\n    - '中'\n    - \"\\u4e2d\"\nOther:\n  - ignored\n".as_bytes(),
        )
        .unwrap();
        assert_eq!(bytes, "a\n中\n中\n".as_bytes());
    }

    #[test]
    fn dictionary_accepts_upstream_indentless_sequence_and_stops_at_the_next_key() {
        let bytes = materialize_dictionary(
            br#"PostProcess:
  name: CTCLabelDecode
  character_dict:
  - '!'
  - '"'
  - ''''
  - ' '
  use_space_char: true
"#,
        )
        .unwrap();
        assert_eq!(bytes, b"!\n\"\n'\n \n");
        assert_eq!(
            materialize_dictionary("character_dict:\n- \u{3000}\n".as_bytes()).unwrap(),
            "\u{3000}\n".as_bytes(),
        );
    }

    #[test]
    fn target_and_missing_archive_member_fail_closed() {
        let wheel = crate::zip::write_stored_zip(&[("onnxruntime/LICENSE", b"L")]).unwrap();
        let result = crate::zip::read_members(&wheel).unwrap();
        assert!(crate::zip::member(&result, TARGETS[0].runtime_member).is_err());
        assert!(target_spec("linux-i686").is_none());
    }

    struct Fixture {
        recipe: Vec<u8>,
        detector: Vec<u8>,
        recognizer: Vec<u8>,
        dictionary_yaml: Vec<u8>,
        wheel: Vec<u8>,
        helper_manifest: Vec<u8>,
        helper_lock: Vec<u8>,
        source_directions: Vec<u8>,
        rust_notices: Vec<u8>,
        rust_source_index: Vec<u8>,
        runtime_url: String,
    }

    fn fixture(target: &str, include_dependency: bool, include_third_party: bool) -> Fixture {
        let spec = target_spec(target).unwrap();
        let detector = b"fixture detector model".to_vec();
        let recognizer = b"fixture recognizer model".to_vec();
        let dictionary_yaml = b"PostProcess:\n  character_dict:\n    - A\n    - B\n".to_vec();
        let dictionary = materialize_dictionary(&dictionary_yaml).unwrap();
        let mut wheel_members = vec![
            (spec.runtime_member, b"fixture ORT runtime".as_slice()),
            ("onnxruntime/LICENSE", b"ORT license".as_slice()),
        ];
        if include_dependency && let Some(member) = spec.dependency_member {
            wheel_members.push((member, b"fixture CPU provider dependency".as_slice()));
        }
        if include_third_party {
            wheel_members.push((
                "onnxruntime/ThirdPartyNotices.txt",
                b"ORT third party notices".as_slice(),
            ));
        }
        let wheel = crate::zip::write_stored_zip(&wheel_members).unwrap();
        let wheel_sha = sha256_hex(&wheel);
        let runtime_url = format!("https://example.invalid/{}", target);
        let archives = [
            ("linux_x86_64", "linux-x86_64", "linux-x86_64.whl"),
            ("linux_aarch64", "linux-aarch64", "linux-aarch64.whl"),
            ("macos_arm64", "macos-arm64", "macos-arm64.whl"),
            ("windows_x86_64", "windows-x86_64", "windows-x86_64.whl"),
        ]
        .into_iter()
        .map(|(table, id, filename)| {
            let url = if id == target {
                runtime_url.clone()
            } else {
                format!("https://example.invalid/{id}")
            };
            format!(
                "[runtime.{table}]\nfilename = \"{filename}\"\nurl = \"{url}\"\nsha256 = \"{wheel_sha}\"\n"
            )
        })
        .collect::<String>();
        let recipe = format!(
            "schema_version = 1\nprotocol_version = 1\n\
             [source]\noar_git = \"https://github.com/GreatV/oar-ocr\"\noar_revision = \"84817f58d7a0932d5368a9fa18105ba5d842af29\"\noar_default_features = false\noar_features = [\"simd\"]\nort_version = \"2.0.0-rc.13\"\nort_features = [\"std\", \"load-dynamic\"]\n\
             [settings]\ncpu_only = true\ntelemetry = false\n\
             [limits]\nmax_regions = 3000\n\
             [models.detector]\nfilename = \"detector.onnx\"\nurl = \"https://example.invalid/detector\"\nsha256 = \"{}\"\n\
             [models.recognizer]\nfilename = \"recognizer.onnx\"\nurl = \"https://example.invalid/recognizer\"\nsha256 = \"{}\"\n\
             [models.dictionary]\nfilename = \"dictionary.txt\"\nsource_url = \"https://example.invalid/dictionary\"\nsource_sha256 = \"{}\"\nmaterialized_entries = 2\nmaterialized_final_newline = true\nmaterialized_sha256 = \"{}\"\n\
             [runtime]\nversion = \"1.31.0\"\nlinux_max_glibc = \"2.34\"\nlinux_build_lane = \"GNU Zig\"\n{}",
            sha256_hex(&detector),
            sha256_hex(&recognizer),
            sha256_hex(&dictionary_yaml),
            sha256_hex(&dictionary),
            archives,
        )
        .into_bytes();
        let helper_lock = b"version = 4\n\n[[package]]\nname = \"oar-ocr\"\nversion = \"0.10.0\"\nsource = \"git+https://github.com/GreatV/oar-ocr?rev=84817f58d7a0932d5368a9fa18105ba5d842af29#84817f58d7a0932d5368a9fa18105ba5d842af29\"\n".to_vec();
        let helper_manifest = b"[package]\nname = \"solstone-screen-ocr-helper\"\n".to_vec();
        let source_directions = b"ORT and Eigen source directions".to_vec();
        let rust_notices = b"Cargo dependency notices".to_vec();
        let recipe_value = Recipe::load(&recipe).unwrap();
        let rust_source_index = serde_json::to_vec(&json!({
            "schema": "solstone.screen-ocr-rust-notices.v1",
            "helper_manifest_sha256": sha256_hex(&helper_manifest),
            "cargo_lock_sha256": sha256_hex(&helper_lock),
            "recipe_sha256": sha256_hex(&recipe),
            "source_directions_sha256": sha256_hex(&source_directions),
            "oar_git": recipe_value.source.oar_git,
            "oar_revision": recipe_value.source.oar_revision,
            "notices_sha256": sha256_hex(&rust_notices),
        }))
        .unwrap();
        Fixture {
            recipe,
            detector,
            recognizer,
            dictionary_yaml,
            wheel,
            helper_manifest,
            helper_lock,
            source_directions,
            rust_notices,
            rust_source_index,
            runtime_url,
        }
    }

    fn stage_fixture(target: &str, fixture: &Fixture) -> Result<StagedBundle, String> {
        stage_bundle(
            &fixture.recipe,
            target,
            target_spec(target).unwrap().triple,
            b"fixture helper executable",
            &fixture.detector,
            &fixture.recognizer,
            &fixture.dictionary_yaml,
            &fixture.wheel,
            &fixture.helper_manifest,
            &fixture.helper_lock,
            b"AGPL license",
            b"Apache license",
            &fixture.source_directions,
            b"OAR and model attribution",
            &fixture.rust_notices,
            &fixture.rust_source_index,
            BUNDLE_DEST,
        )
    }

    #[test]
    fn fixture_acquisition_uses_and_rechecks_all_pinned_inputs() {
        let fixture = fixture("linux-x86_64", true, true);
        let cache = tempfile::tempdir().unwrap();
        let mut calls = Vec::new();
        let fetch = |url: &str, calls: &mut Vec<String>| {
            calls.push(url.to_owned());
            match url {
                "https://example.invalid/detector" => Ok(fixture.detector.clone()),
                "https://example.invalid/recognizer" => Ok(fixture.recognizer.clone()),
                "https://example.invalid/dictionary" => Ok(fixture.dictionary_yaml.clone()),
                url if url == fixture.runtime_url => Ok(fixture.wheel.clone()),
                _ => Err("unexpected fixture URL".into()),
            }
        };
        acquire_with(&fixture.recipe, "linux-x86_64", cache.path(), |url| {
            fetch(url, &mut calls)
        })
        .unwrap();
        assert_eq!(calls.len(), 4);
        let cached_calls = calls.len();
        acquire_with(&fixture.recipe, "linux-x86_64", cache.path(), |_| {
            panic!("digest-valid cache should not fetch")
        })
        .unwrap();
        assert_eq!(calls.len(), cached_calls);

        let bad_cache = tempfile::tempdir().unwrap();
        let error = acquire_with(&fixture.recipe, "linux-x86_64", bad_cache.path(), |url| {
            if url == "https://example.invalid/detector" {
                Ok(b"wrong model".to_vec())
            } else {
                fetch(url, &mut Vec::new())
            }
        })
        .unwrap_err();
        assert!(error.contains("digest mismatch"));
    }

    #[test]
    fn fixture_bundle_stages_private_runtime_notices_and_bound_provenance() {
        let fixture = fixture("windows-x86_64", true, true);
        let bundle = stage_fixture("windows-x86_64", &fixture).unwrap();
        let files = declared_files(&bundle);
        let paths = files
            .iter()
            .map(|(path, _, _)| path.as_str())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"lib/solstone-screen-ocr/runtime/onnxruntime.dll"));
        assert!(
            paths.contains(&"lib/solstone-screen-ocr/runtime/onnxruntime_providers_shared.dll")
        );
        assert!(paths.contains(&"lib/solstone-screen-ocr/runtime/LICENSE"));
        assert!(paths.contains(&"lib/solstone-screen-ocr/runtime/ThirdPartyNotices.txt"));
        assert!(paths.contains(&"lib/solstone-screen-ocr/licenses/rust-NOTICES.txt"));
        assert!(paths.contains(&"lib/solstone-screen-ocr/provenance/rust-sources.json"));
        let provenance = bundle
            .files
            .iter()
            .find(|file| file.dest.ends_with("provenance/bundle.json"))
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&provenance.bytes).unwrap();
        assert_eq!(value["target"], "windows-x86_64");
        assert_eq!(
            value["helper_lock_sha256"],
            sha256_hex(&fixture.helper_lock)
        );
        assert_eq!(value["runtime_archive_sha256"], sha256_hex(&fixture.wheel));
        assert_eq!(value["runtime_sha256"], sha256_hex(b"fixture ORT runtime"));
        assert_eq!(
            value["helper_manifest_sha256"],
            sha256_hex(&fixture.helper_manifest)
        );
        assert_eq!(
            value["rust_notices_sha256"],
            sha256_hex(&fixture.rust_notices)
        );
        assert_eq!(
            value["rust_source_index_sha256"],
            sha256_hex(&fixture.rust_source_index)
        );
        assert_eq!(
            value["runtime_dependency_sha256"],
            sha256_hex(b"fixture CPU provider dependency")
        );
    }

    #[test]
    fn macos_bundle_stages_versioned_runtime_without_missing_provider_member() {
        let fixture = fixture("macos-arm64", false, true);
        let bundle = stage_fixture("macos-arm64", &fixture).unwrap();
        let paths = declared_files(&bundle)
            .into_iter()
            .map(|(path, _, _)| path)
            .collect::<Vec<_>>();
        assert!(paths.contains(&format!(
            "{BUNDLE_DEST}/runtime/libonnxruntime.1.31.0.dylib"
        )));
        assert!(!paths.iter().any(|path| path.contains("providers_shared")));
        assert!(paths.contains(&format!("{BUNDLE_DEST}/runtime/LICENSE")));
        assert!(paths.contains(&format!("{BUNDLE_DEST}/runtime/ThirdPartyNotices.txt")));
    }

    #[test]
    fn fixture_bundle_rejects_wrong_helper_model_runtime_and_oar_pins() {
        let base_fixture = fixture("linux-x86_64", true, true);
        assert!(
            stage_bundle(
                &base_fixture.recipe,
                "linux-x86_64",
                "wrong-triple",
                b"helper",
                &base_fixture.detector,
                &base_fixture.recognizer,
                &base_fixture.dictionary_yaml,
                &base_fixture.wheel,
                &base_fixture.helper_manifest,
                &base_fixture.helper_lock,
                b"AGPL",
                b"Apache",
                b"directions",
                b"attribution",
                b"notices",
                b"sources",
                BUNDLE_DEST,
            )
            .is_err()
        );

        let mut wrong_model = fixture("linux-x86_64", true, true);
        wrong_model.detector = b"wrong detector".to_vec();
        assert!(stage_fixture("linux-x86_64", &wrong_model).is_err());

        let missing_dependency = fixture("linux-x86_64", false, true);
        assert!(
            stage_fixture("linux-x86_64", &missing_dependency)
                .unwrap_err()
                .contains("runtime dependency missing")
        );
        let missing_notices = fixture("linux-x86_64", true, false);
        assert!(
            stage_fixture("linux-x86_64", &missing_notices)
                .unwrap_err()
                .contains("third-party notices missing")
        );

        let mut wrong_lock = fixture("linux-x86_64", true, true);
        wrong_lock.helper_lock = String::from_utf8(wrong_lock.helper_lock)
            .unwrap()
            .replace(
                "84817f58d7a0932d5368a9fa18105ba5d842af29",
                "94817f58d7a0932d5368a9fa18105ba5d842af29",
            )
            .into_bytes();
        assert!(
            stage_fixture("linux-x86_64", &wrong_lock)
                .unwrap_err()
                .contains("pinned OAR revision")
        );

        let mut changed_notices = fixture("linux-x86_64", true, true);
        changed_notices.rust_notices.push(b'!');
        assert!(
            stage_fixture("linux-x86_64", &changed_notices)
                .unwrap_err()
                .contains("source index digest mismatch")
        );
    }

    #[test]
    fn main_onnx_runtime_fixture_keeps_its_original_member_path_and_bytes() {
        let (spec, wheel) = crate::onnx_runtime::identity_fixture_wheel();
        let staged = crate::onnx_runtime::stage_from_bytes(&spec, &wheel).unwrap();
        assert_eq!(spec.runtime_staged_name, "libonnxruntime.so.1");
        assert_eq!(
            spec.runtime_member,
            "onnxruntime/capi/libonnxruntime.so.1.25.0"
        );
        assert_eq!(staged.library, b"fixture-runtime-bytes");
        assert_eq!(
            crate::onnx_runtime::staged_member_names(&spec),
            ["libonnxruntime.so.1"].into_iter().collect()
        );
        let destination = tempfile::tempdir().unwrap();
        crate::onnx_runtime::write_staged_runtime(&spec, &staged, destination.path()).unwrap();
        assert_eq!(
            std::fs::read(destination.path().join("libonnxruntime.so.1")).unwrap(),
            b"fixture-runtime-bytes"
        );
    }
}
