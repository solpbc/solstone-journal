// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic inputs for the controlled Windows ONNX Runtime build slot.
//!
//! The driver turns a clean pinned Git checkout and the explicitly pinned
//! CMake dependency mirror into two auditable archives before either reaches
//! Windows. The native slot only verifies and extracts these archives; it does
//! not clone, initialize a submodule, or download a compiler dependency.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use serde::Deserialize;
use tar::{Archive, Builder, EntryType, Header};

use crate::acquire;
use crate::controlled_build::{DependencySource, InputIdentityEntry};
use crate::digest::sha256_hex;
use crate::onnx_windows::{
    ONNX_RUNTIME_COMMIT, ONNX_RUNTIME_REPOSITORY, ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256,
    PYANNOTE_FILENAME, PYANNOTE_SHA256, PYANNOTE_SIZE_BYTES, SILERO_VAD_FILENAME,
    SILERO_VAD_SHA256, SILERO_VAD_SIZE_BYTES, WESPEAKER_FILENAME, WESPEAKER_SHA256,
    WESPEAKER_SIZE_BYTES,
};

const ONNX_WINDOWS_INPUTS_PATH: &str = "core/distribution/onnx-windows-inputs.toml";
const ONNX_WINDOWS_INPUTS_SCHEMA: &str = "solstone-onnx-windows-inputs-v1";
const ONNX_WINDOWS_SOURCE_ARCHIVE_SCHEMA: &str = "solstone.onnx-windows-source-archive.v1";
const ONNX_WINDOWS_MIRROR_ARCHIVE_SCHEMA: &str = "solstone.onnx-windows-mirror-archive.v1";
const ONNX_WINDOWS_SOURCE_ARCHIVE_MANIFEST: &str = "onnx-windows-source.json";
const ONNX_WINDOWS_MIRROR_ARCHIVE_MANIFEST: &str = "onnx-windows-mirror.json";
const ONNX_WINDOWS_SOURCE_ARCHIVE_LABEL: &str = "sources/onnxruntime-with-submodules.tar.gz";
const ONNX_WINDOWS_MIRROR_ARCHIVE_LABEL: &str = "sources/onnxruntime-cmake-mirror.tar.gz";
const ONNX_WINDOWS_CMAKE_ARCHIVE_LABEL: &str = "tools/cmake-windows-x86_64.zip";
const ONNX_WINDOWS_PYTHON_ARCHIVE_LABEL: &str = "tools/python-windows-x86_64.zip";
const ONNX_WINDOWS_PROTOC_ARCHIVE_LABEL: &str = "tools/protoc-windows-x86_64.zip";
const ONNX_WINDOWS_REDUCED_OPS_CONFIG_LABEL: &str = "build/required-operators.config";

const ONNX_SUBMODULES: &[(&str, &str, &str)] = &[
    (
        "cmake/external/emsdk",
        "https://github.com/emscripten-core/emsdk.git",
        "c0bb220cb6e6f4e0fabb6f6db9efd53390ef5e56",
    ),
    (
        "cmake/external/libprotobuf-mutator",
        "https://github.com/google/libprotobuf-mutator.git",
        "7a2ed51a6b682a83e345ff49fc4cfd7ca47550db",
    ),
    (
        "cmake/external/onnx",
        "https://github.com/onnx/onnx.git",
        "be2b5fde82d9c8874f3d19328bdfe3b6962dc67b",
    ),
];

const CPU_MIRROR_DEPENDENCIES: &[&str] = &[
    "abseil_cpp",
    "date",
    "eigen",
    "flatbuffers",
    "json",
    "microsoft_gsl",
    "microsoft_wil",
    "mp11",
    "onnx",
    "protobuf",
    "pytorch_cpuinfo",
    "re2",
    "safeint",
];

#[derive(Debug)]
pub struct OnnxWindowsSourceError {
    message: String,
}

impl OnnxWindowsSourceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for OnnxWindowsSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OnnxWindowsSourceError {}

impl From<std::io::Error> for OnnxWindowsSourceError {
    fn from(source: std::io::Error) -> Self {
        Self::new(source.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnnxWindowsSourceArchiveManifest {
    pub schema: String,
    pub onnxruntime: DependencySource,
    pub submodules: BTreeMap<String, DependencySource>,
    pub reduced_ops_config: InputIdentityEntry,
    pub models: Vec<InputIdentityEntry>,
    pub files: Vec<OnnxWindowsArchiveFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnnxWindowsArchiveFile {
    pub path: String,
    pub mode: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnnxWindowsSourceArchive {
    pub archive: InputIdentityEntry,
    pub onnxruntime: DependencySource,
    pub submodules: BTreeMap<String, DependencySource>,
    pub reduced_ops_config: InputIdentityEntry,
    pub models: Vec<InputIdentityEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnnxWindowsMirrorArchiveManifest {
    pub schema: String,
    pub dependencies: Vec<OnnxWindowsMirroredDependency>,
    pub files: Vec<OnnxWindowsArchiveFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnnxWindowsMirroredDependency {
    pub name: String,
    pub url: String,
    pub sha1: String,
    pub sha256: String,
    pub size: u64,
    pub mirror_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnnxWindowsMirrorArchive {
    pub archive: InputIdentityEntry,
    pub dependencies: Vec<OnnxWindowsMirroredDependency>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnnxWindowsInputs {
    schema: String,
    reduced_ops_config: String,
    reduced_ops_config_sha256: String,
    protoc_windows_x86_64: ProtocInput,
    cmake_dependency: Vec<MirrorInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocInput {
    version: String,
    filename: String,
    url: String,
    sha1: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorInput {
    name: String,
    url: String,
    sha1: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone)]
struct SourceFile {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
}

impl SourceFile {
    fn record(&self) -> OnnxWindowsArchiveFile {
        OnnxWindowsArchiveFile {
            path: self.path.clone(),
            mode: self.mode,
            sha256: sha256_hex(&self.bytes),
        }
    }
}

pub fn usage() -> &'static str {
    "usage: solstone-distribution onnx-windows <source-archive|mirror-archive|verify-inputs|help> [FLAG]"
}

pub fn run_cli(args: &[String]) -> Result<String, OnnxWindowsSourceError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(OnnxWindowsSourceError::new(usage()));
    };
    let flags = parse_flags(rest)?;
    match operation.as_str() {
        "help" | "--help" | "-h" => {
            require_only(&flags, &[])?;
            Ok(usage().to_owned())
        }
        "source-archive" => {
            let source = required_path(&flags, "--source")?;
            let output = required_path(&flags, "--out")?;
            require_only(&flags, &["--source", "--out"])?;
            let archive =
                materialize_onnx_windows_source_archive(&source, &repository_root()?, &output)?;
            Ok(format!(
                "ONNX_WINDOWS_SOURCE_ARCHIVE_OK path={} sha256={} size={}",
                output.display(),
                archive.archive.sha256,
                archive.archive.size
            ))
        }
        "mirror-archive" => {
            let source = required_path(&flags, "--source")?;
            let input_dir = required_path(&flags, "--input-dir")?;
            let output = required_path(&flags, "--out")?;
            require_only(&flags, &["--source", "--input-dir", "--out"])?;
            let archive = materialize_onnx_windows_mirror_archive(
                &source,
                &repository_root()?,
                &input_dir,
                &output,
            )?;
            Ok(format!(
                "ONNX_WINDOWS_MIRROR_ARCHIVE_OK path={} sha256={} size={} dependencies={}",
                output.display(),
                archive.archive.sha256,
                archive.archive.size,
                archive.dependencies.len()
            ))
        }
        "verify-inputs" => {
            let source_archive = required_path(&flags, "--source-archive")?;
            let mirror_archive = required_path(&flags, "--mirror-archive")?;
            let cmake_archive = required_path(&flags, "--cmake-archive")?;
            let python_archive = required_path(&flags, "--python-archive")?;
            let protoc_archive = required_path(&flags, "--protoc-archive")?;
            require_only(
                &flags,
                &[
                    "--source-archive",
                    "--mirror-archive",
                    "--cmake-archive",
                    "--python-archive",
                    "--protoc-archive",
                ],
            )?;
            let repo = repository_root()?;
            let source = inspect_onnx_windows_source_archive(&source_archive)?;
            let mirror = inspect_onnx_windows_mirror_archive(&repo, &mirror_archive)?;
            let cmake = inspect_cmake_archive(&repo, &cmake_archive)?;
            let python = inspect_python_archive(&repo, &python_archive)?;
            let protoc = inspect_protoc_archive(&repo, &protoc_archive)?;
            Ok(format!(
                "ONNX_WINDOWS_INPUTS_OK source_sha256={} source_size={} mirror_sha256={} mirror_size={} dependencies={} cmake_sha256={} python_sha256={} protoc_sha256={}",
                source.archive.sha256,
                source.archive.size,
                mirror.archive.sha256,
                mirror.archive.size,
                mirror.dependencies.len(),
                cmake.sha256,
                python.sha256,
                protoc.sha256,
            ))
        }
        other => Err(OnnxWindowsSourceError::new(format!(
            "unknown ONNX Windows command {other:?}\n{}",
            usage()
        ))),
    }
}

pub fn materialize_onnx_windows_source_archive(
    source_root: &Path,
    repo_root: &Path,
    destination: &Path,
) -> Result<OnnxWindowsSourceArchive, OnnxWindowsSourceError> {
    refuse_existing(destination, "ONNX source archive")?;
    verify_source_checkout(source_root)?;
    let inputs = load_inputs(repo_root)?;
    let config = read_reduced_ops_config(repo_root, &inputs)?;
    let models = read_models(repo_root)?;

    let mut root_files = git_archive_files(source_root, "")?;
    let source_content_sha256 = digest_files(&root_files);
    let mut submodules = BTreeMap::new();
    for (path, repository, revision) in ONNX_SUBMODULES {
        let root = source_root.join(path);
        let files = git_archive_files(&root, &format!("{path}/"))?;
        let digest = digest_files(&files);
        submodules.insert(
            (*path).to_owned(),
            DependencySource {
                repository: (*repository).to_owned(),
                revision: (*revision).to_owned(),
                content_sha256: digest,
            },
        );
        root_files.extend(files);
    }
    root_files.push(SourceFile {
        path: "required-operators.config".to_owned(),
        mode: 0o644,
        bytes: fs::read(repo_root.join(&inputs.reduced_ops_config))?,
    });
    root_files.sort_by(|left, right| left.path.cmp(&right.path));
    ensure_distinct_files(&root_files, "ONNX source archive")?;
    let manifest = OnnxWindowsSourceArchiveManifest {
        schema: ONNX_WINDOWS_SOURCE_ARCHIVE_SCHEMA.to_owned(),
        onnxruntime: DependencySource {
            repository: ONNX_RUNTIME_REPOSITORY.to_owned(),
            revision: ONNX_RUNTIME_COMMIT.to_owned(),
            content_sha256: source_content_sha256,
        },
        submodules,
        reduced_ops_config: config,
        models,
        files: root_files.iter().map(SourceFile::record).collect(),
    };
    let manifest_bytes = canonical_json(&manifest)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    write_archive(
        destination,
        &root_files,
        ONNX_WINDOWS_SOURCE_ARCHIVE_MANIFEST,
        &manifest_bytes,
    )?;
    inspect_onnx_windows_source_archive(destination)
}

pub fn materialize_onnx_windows_mirror_archive(
    source_root: &Path,
    repo_root: &Path,
    input_dir: &Path,
    destination: &Path,
) -> Result<OnnxWindowsMirrorArchive, OnnxWindowsSourceError> {
    refuse_existing(destination, "ONNX mirror archive")?;
    verify_source_checkout(source_root)?;
    let inputs = load_inputs(repo_root)?;
    verify_cpu_mirror_matches_source(source_root, &inputs)?;
    let mut dependencies = Vec::new();
    let mut files = Vec::new();
    for input in &inputs.cmake_dependency {
        let path = input_dir.join(&input.name);
        let bytes = fs::read(&path).map_err(|source| {
            OnnxWindowsSourceError::new(format!(
                "read ONNX mirror input {}: {source}",
                path.display()
            ))
        })?;
        verify_input_bytes(&bytes, &input.sha1, &input.sha256, input.size, &input.name)?;
        let mirror_path = mirror_path(&input.url)?;
        dependencies.push(OnnxWindowsMirroredDependency {
            name: input.name.clone(),
            url: input.url.clone(),
            sha1: input.sha1.clone(),
            sha256: input.sha256.clone(),
            size: input.size,
            mirror_path: mirror_path.clone(),
        });
        files.push(SourceFile {
            path: mirror_path,
            mode: 0o644,
            bytes,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    ensure_distinct_files(&files, "ONNX mirror archive")?;
    let manifest = OnnxWindowsMirrorArchiveManifest {
        schema: ONNX_WINDOWS_MIRROR_ARCHIVE_SCHEMA.to_owned(),
        dependencies,
        files: files.iter().map(SourceFile::record).collect(),
    };
    let manifest_bytes = canonical_json(&manifest)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    write_archive(
        destination,
        &files,
        ONNX_WINDOWS_MIRROR_ARCHIVE_MANIFEST,
        &manifest_bytes,
    )?;
    inspect_onnx_windows_mirror_archive(repo_root, destination)
}

pub fn inspect_onnx_windows_source_archive(
    path: &Path,
) -> Result<OnnxWindowsSourceArchive, OnnxWindowsSourceError> {
    let bytes = fs::read(path)?;
    let manifest = validate_source_archive(&bytes)?;
    Ok(OnnxWindowsSourceArchive {
        archive: input_identity(ONNX_WINDOWS_SOURCE_ARCHIVE_LABEL, &bytes),
        onnxruntime: manifest.onnxruntime,
        submodules: manifest.submodules,
        reduced_ops_config: manifest.reduced_ops_config,
        models: manifest.models,
    })
}

pub fn inspect_onnx_windows_mirror_archive(
    repo_root: &Path,
    path: &Path,
) -> Result<OnnxWindowsMirrorArchive, OnnxWindowsSourceError> {
    let bytes = fs::read(path)?;
    let manifest = validate_mirror_archive(&bytes, &load_inputs(repo_root)?)?;
    Ok(OnnxWindowsMirrorArchive {
        archive: input_identity(ONNX_WINDOWS_MIRROR_ARCHIVE_LABEL, &bytes),
        dependencies: manifest.dependencies,
    })
}

fn inspect_cmake_archive(
    repo_root: &Path,
    path: &Path,
) -> Result<InputIdentityEntry, OnnxWindowsSourceError> {
    let expected = acquire::windows_cmake_archive_input(repo_root)
        .map_err(|source| OnnxWindowsSourceError::new(source.to_string()))?;
    inspect_exact_input(
        path,
        ONNX_WINDOWS_CMAKE_ARCHIVE_LABEL,
        &expected.filename,
        &expected.sha256,
        expected.size,
    )
}

fn inspect_python_archive(
    repo_root: &Path,
    path: &Path,
) -> Result<InputIdentityEntry, OnnxWindowsSourceError> {
    let expected = acquire::windows_python_archive_input(repo_root)
        .map_err(|source| OnnxWindowsSourceError::new(source.to_string()))?;
    inspect_exact_input(
        path,
        ONNX_WINDOWS_PYTHON_ARCHIVE_LABEL,
        &expected.filename,
        &expected.sha256,
        expected.size,
    )
}

fn inspect_protoc_archive(
    repo_root: &Path,
    path: &Path,
) -> Result<InputIdentityEntry, OnnxWindowsSourceError> {
    let expected = load_inputs(repo_root)?.protoc_windows_x86_64;
    if expected.version != "21.12" {
        return Err(OnnxWindowsSourceError::new(format!(
            "unexpected ONNX Windows protoc version {}",
            expected.version
        )));
    }
    let bytes = fs::read(path)?;
    verify_input_bytes(
        &bytes,
        &expected.sha1,
        &expected.sha256,
        expected.size,
        &expected.filename,
    )?;
    Ok(input_identity(ONNX_WINDOWS_PROTOC_ARCHIVE_LABEL, &bytes))
}

fn inspect_exact_input(
    path: &Path,
    label: &str,
    filename: &str,
    sha256: &str,
    size: u64,
) -> Result<InputIdentityEntry, OnnxWindowsSourceError> {
    let bytes = fs::read(path)?;
    if bytes.len() as u64 != size || sha256_hex(&bytes) != sha256 {
        return Err(OnnxWindowsSourceError::new(format!(
            "ONNX Windows input {filename} has an unexpected identity"
        )));
    }
    Ok(input_identity(label, &bytes))
}

fn validate_source_archive(
    bytes: &[u8],
) -> Result<OnnxWindowsSourceArchiveManifest, OnnxWindowsSourceError> {
    let members = read_archive_members(bytes)?;
    let (_, manifest_bytes) = members
        .get(ONNX_WINDOWS_SOURCE_ARCHIVE_MANIFEST)
        .ok_or_else(|| OnnxWindowsSourceError::new("ONNX source archive lacks manifest"))?;
    let manifest = serde_json::from_slice::<OnnxWindowsSourceArchiveManifest>(manifest_bytes)
        .map_err(|source| OnnxWindowsSourceError::new(source.to_string()))?;
    if manifest.schema != ONNX_WINDOWS_SOURCE_ARCHIVE_SCHEMA {
        return Err(OnnxWindowsSourceError::new(format!(
            "unexpected ONNX source archive schema {}",
            manifest.schema
        )));
    }
    require_dependency(
        &manifest.onnxruntime,
        ONNX_RUNTIME_REPOSITORY,
        ONNX_RUNTIME_COMMIT,
        "onnxruntime",
    )?;
    let expected_submodules: BTreeSet<&str> =
        ONNX_SUBMODULES.iter().map(|(path, _, _)| *path).collect();
    let actual_submodules: BTreeSet<&str> =
        manifest.submodules.keys().map(String::as_str).collect();
    require_exact_set("ONNX submodules", expected_submodules, actual_submodules)?;
    for (path, repository, revision) in ONNX_SUBMODULES {
        require_dependency(
            manifest
                .submodules
                .get(*path)
                .expect("submodule set checked"),
            repository,
            revision,
            path,
        )?;
    }
    require_models(&manifest.models)?;
    if manifest.reduced_ops_config.label != ONNX_WINDOWS_REDUCED_OPS_CONFIG_LABEL
        || manifest.reduced_ops_config.sha256 != ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256
        || manifest.reduced_ops_config.size != 711
    {
        return Err(OnnxWindowsSourceError::new(
            "ONNX source archive has an unexpected reduced-operator config identity",
        ));
    }
    validate_files_against_manifest(
        &members,
        ONNX_WINDOWS_SOURCE_ARCHIVE_MANIFEST,
        &manifest.files,
        "ONNX source archive",
    )?;
    let config = members
        .get("required-operators.config")
        .ok_or_else(|| OnnxWindowsSourceError::new("ONNX source archive lacks operator config"))?;
    if sha256_hex(&config.1) != ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256 {
        return Err(OnnxWindowsSourceError::new(
            "ONNX source archive operator config does not match its locked digest",
        ));
    }
    Ok(manifest)
}

fn validate_mirror_archive(
    bytes: &[u8],
    inputs: &OnnxWindowsInputs,
) -> Result<OnnxWindowsMirrorArchiveManifest, OnnxWindowsSourceError> {
    let members = read_archive_members(bytes)?;
    let (_, manifest_bytes) = members
        .get(ONNX_WINDOWS_MIRROR_ARCHIVE_MANIFEST)
        .ok_or_else(|| OnnxWindowsSourceError::new("ONNX mirror archive lacks manifest"))?;
    let manifest = serde_json::from_slice::<OnnxWindowsMirrorArchiveManifest>(manifest_bytes)
        .map_err(|source| OnnxWindowsSourceError::new(source.to_string()))?;
    if manifest.schema != ONNX_WINDOWS_MIRROR_ARCHIVE_SCHEMA {
        return Err(OnnxWindowsSourceError::new(format!(
            "unexpected ONNX mirror archive schema {}",
            manifest.schema
        )));
    }
    let expected = inputs
        .cmake_dependency
        .iter()
        .map(|input| (input.name.as_str(), input))
        .collect::<BTreeMap<_, _>>();
    let actual = manifest
        .dependencies
        .iter()
        .map(|dependency| (dependency.name.as_str(), dependency))
        .collect::<BTreeMap<_, _>>();
    if actual.len() != manifest.dependencies.len() {
        return Err(OnnxWindowsSourceError::new(
            "ONNX mirror archive has duplicate dependency names",
        ));
    }
    require_exact_set(
        "ONNX mirror dependencies",
        expected.keys().copied().collect(),
        actual.keys().copied().collect(),
    )?;
    for (name, input) in expected {
        let dependency = actual.get(name).expect("mirror dependency set checked");
        if dependency.url != input.url
            || dependency.sha1 != input.sha1
            || dependency.sha256 != input.sha256
            || dependency.size != input.size
            || dependency.mirror_path != mirror_path(&input.url)?
        {
            return Err(OnnxWindowsSourceError::new(format!(
                "ONNX mirror archive has an unexpected identity for {name}"
            )));
        }
    }
    validate_files_against_manifest(
        &members,
        ONNX_WINDOWS_MIRROR_ARCHIVE_MANIFEST,
        &manifest.files,
        "ONNX mirror archive",
    )?;
    for dependency in &manifest.dependencies {
        let (_, bytes) = members.get(&dependency.mirror_path).ok_or_else(|| {
            OnnxWindowsSourceError::new(format!(
                "ONNX mirror archive lacks {}",
                dependency.mirror_path
            ))
        })?;
        verify_input_bytes(
            bytes,
            &dependency.sha1,
            &dependency.sha256,
            dependency.size,
            &dependency.name,
        )?;
    }
    Ok(manifest)
}

fn read_archive_members(
    bytes: &[u8],
) -> Result<BTreeMap<String, (u32, Vec<u8>)>, OnnxWindowsSourceError> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let mut members = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(OnnxWindowsSourceError::new(
                "ONNX archive contains a non-file member",
            ));
        }
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        crate::archive::refuse_escape(&path)
            .map_err(|source| OnnxWindowsSourceError::new(source.as_str()))?;
        let mode = entry.header().mode()? & 0o777;
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents)?;
        if members.insert(path.clone(), (mode, contents)).is_some() {
            return Err(OnnxWindowsSourceError::new(format!(
                "ONNX archive contains duplicate member {path}"
            )));
        }
    }
    Ok(members)
}

fn validate_files_against_manifest(
    members: &BTreeMap<String, (u32, Vec<u8>)>,
    manifest_name: &str,
    files: &[OnnxWindowsArchiveFile],
    label: &str,
) -> Result<(), OnnxWindowsSourceError> {
    let expected = files
        .iter()
        .map(|file| (file.path.clone(), (file.mode, file.sha256.clone())))
        .collect::<BTreeMap<_, _>>();
    if expected.len() != files.len() {
        return Err(OnnxWindowsSourceError::new(format!(
            "{label} manifest has duplicate file paths"
        )));
    }
    let actual = members
        .iter()
        .filter(|(path, _)| path.as_str() != manifest_name)
        .map(|(path, (mode, bytes))| (path.clone(), (*mode, sha256_hex(bytes))))
        .collect::<BTreeMap<_, _>>();
    if actual != expected {
        return Err(OnnxWindowsSourceError::new(format!(
            "{label} members differ from its manifest"
        )));
    }
    Ok(())
}

fn read_models(repo_root: &Path) -> Result<Vec<InputIdentityEntry>, OnnxWindowsSourceError> {
    let mut models = Vec::new();
    for (name, size, sha256) in [
        (PYANNOTE_FILENAME, PYANNOTE_SIZE_BYTES, PYANNOTE_SHA256),
        (
            SILERO_VAD_FILENAME,
            SILERO_VAD_SIZE_BYTES,
            SILERO_VAD_SHA256,
        ),
        (WESPEAKER_FILENAME, WESPEAKER_SIZE_BYTES, WESPEAKER_SHA256),
    ] {
        let bytes = fs::read(repo_root.join("core/models/assets").join(name))?;
        if bytes.len() as u64 != size || sha256_hex(&bytes) != sha256 {
            return Err(OnnxWindowsSourceError::new(format!(
                "ONNX raw model {name} has an unexpected identity"
            )));
        }
        models.push(InputIdentityEntry {
            label: format!("models/{name}"),
            sha256: sha256.to_owned(),
            size,
        });
    }
    Ok(models)
}

fn require_models(models: &[InputIdentityEntry]) -> Result<(), OnnxWindowsSourceError> {
    let expected = BTreeMap::from([
        (
            format!("models/{PYANNOTE_FILENAME}"),
            (PYANNOTE_SIZE_BYTES, PYANNOTE_SHA256),
        ),
        (
            format!("models/{SILERO_VAD_FILENAME}"),
            (SILERO_VAD_SIZE_BYTES, SILERO_VAD_SHA256),
        ),
        (
            format!("models/{WESPEAKER_FILENAME}"),
            (WESPEAKER_SIZE_BYTES, WESPEAKER_SHA256),
        ),
    ]);
    let actual = models
        .iter()
        .map(|model| (model.label.as_str(), (model.size, model.sha256.as_str())))
        .collect::<BTreeMap<_, _>>();
    if actual.len() != models.len() || actual.len() != expected.len() {
        return Err(OnnxWindowsSourceError::new(
            "ONNX source archive has an unexpected raw-model set",
        ));
    }
    for (label, (size, digest)) in expected {
        if actual.get(label.as_str()) != Some(&(size, digest)) {
            return Err(OnnxWindowsSourceError::new(format!(
                "ONNX source archive has an unexpected raw-model identity for {label}"
            )));
        }
    }
    Ok(())
}

fn read_reduced_ops_config(
    repo_root: &Path,
    inputs: &OnnxWindowsInputs,
) -> Result<InputIdentityEntry, OnnxWindowsSourceError> {
    if inputs.reduced_ops_config_sha256 != ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256 {
        return Err(OnnxWindowsSourceError::new(
            "ONNX Windows input table has an unexpected reduced-operator digest",
        ));
    }
    let path = repo_root.join(&inputs.reduced_ops_config);
    let bytes = fs::read(&path)?;
    if sha256_hex(&bytes) != ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256 || bytes.len() != 711 {
        return Err(OnnxWindowsSourceError::new(format!(
            "ONNX reduced-operator config has an unexpected identity: {}",
            path.display()
        )));
    }
    Ok(input_identity(
        ONNX_WINDOWS_REDUCED_OPS_CONFIG_LABEL,
        &bytes,
    ))
}

fn load_inputs(repo_root: &Path) -> Result<OnnxWindowsInputs, OnnxWindowsSourceError> {
    let path = repo_root.join(ONNX_WINDOWS_INPUTS_PATH);
    let text = fs::read_to_string(&path)?;
    let inputs = toml_edit::de::from_str::<OnnxWindowsInputs>(&text)
        .map_err(|source| OnnxWindowsSourceError::new(format!("{}: {source}", path.display())))?;
    if inputs.schema != ONNX_WINDOWS_INPUTS_SCHEMA {
        return Err(OnnxWindowsSourceError::new(format!(
            "unexpected ONNX Windows inputs schema {}",
            inputs.schema
        )));
    }
    if inputs.protoc_windows_x86_64.version != "21.12"
        || inputs.protoc_windows_x86_64.filename != "protoc-21.12-win64.zip"
    {
        return Err(OnnxWindowsSourceError::new(
            "ONNX Windows input table has an unexpected protoc identity",
        ));
    }
    require_hex(&inputs.protoc_windows_x86_64.sha1, 40, "protoc SHA-1")?;
    require_hex(&inputs.protoc_windows_x86_64.sha256, 64, "protoc SHA-256")?;
    if inputs.protoc_windows_x86_64.size == 0
        || !inputs.protoc_windows_x86_64.url.starts_with("https://")
    {
        return Err(OnnxWindowsSourceError::new(
            "ONNX Windows input table has an invalid protoc archive",
        ));
    }
    let expected: BTreeSet<&str> = CPU_MIRROR_DEPENDENCIES.iter().copied().collect();
    let actual: BTreeSet<&str> = inputs
        .cmake_dependency
        .iter()
        .map(|input| input.name.as_str())
        .collect();
    require_exact_set("ONNX CMake mirror inputs", expected, actual)?;
    if inputs.cmake_dependency.len() != CPU_MIRROR_DEPENDENCIES.len() {
        return Err(OnnxWindowsSourceError::new(
            "ONNX Windows input table has duplicate CMake dependency names",
        ));
    }
    for input in &inputs.cmake_dependency {
        if input.size == 0 || !input.url.starts_with("https://") {
            return Err(OnnxWindowsSourceError::new(format!(
                "ONNX Windows input table has an invalid mirror entry {}",
                input.name
            )));
        }
        require_hex(&input.sha1, 40, &format!("{} SHA-1", input.name))?;
        require_hex(&input.sha256, 64, &format!("{} SHA-256", input.name))?;
        let _ = mirror_path(&input.url)?;
    }
    Ok(inputs)
}

fn verify_cpu_mirror_matches_source(
    source_root: &Path,
    inputs: &OnnxWindowsInputs,
) -> Result<(), OnnxWindowsSourceError> {
    let text = fs::read_to_string(source_root.join("cmake/deps.txt"))?;
    let source = text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let fields = line.split(';').collect::<Vec<_>>();
            (fields.len() == 3).then_some((fields[0], (fields[1], fields[2])))
        })
        .collect::<BTreeMap<_, _>>();
    for input in &inputs.cmake_dependency {
        if source.get(input.name.as_str()) != Some(&(input.url.as_str(), input.sha1.as_str())) {
            return Err(OnnxWindowsSourceError::new(format!(
                "ONNX source deps.txt does not match mirrored input {}",
                input.name
            )));
        }
    }
    Ok(())
}

fn verify_source_checkout(source_root: &Path) -> Result<(), OnnxWindowsSourceError> {
    verify_git_checkout(
        source_root,
        "https://github.com/microsoft/onnxruntime.git",
        ONNX_RUNTIME_COMMIT,
    )?;
    let expected_paths = ONNX_SUBMODULES
        .iter()
        .map(|(path, _, _)| *path)
        .collect::<BTreeSet<_>>();
    let status = git_output(source_root, &["submodule", "status", "--recursive"])?;
    let actual_paths = status
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(parse_submodule_status)
        .collect::<Result<BTreeSet<_>, _>>()?;
    require_exact_set("ONNX source submodules", expected_paths, actual_paths)?;
    for (path, repository, revision) in ONNX_SUBMODULES {
        let root = source_root.join(path);
        verify_git_checkout(&root, repository, revision)?;
        let gitlink = git_text(source_root, &["ls-tree", "HEAD", path])?;
        let expected = format!("160000 commit {revision}\t{path}");
        if gitlink != expected {
            return Err(OnnxWindowsSourceError::new(format!(
                "ONNX source has an unexpected {path} gitlink"
            )));
        }
    }
    Ok(())
}

fn parse_submodule_status(line: &[u8]) -> Result<&str, OnnxWindowsSourceError> {
    let text = std::str::from_utf8(line)
        .map_err(|source| OnnxWindowsSourceError::new(source.to_string()))?;
    if !text.starts_with(' ') {
        return Err(OnnxWindowsSourceError::new(
            "ONNX source has an uninitialized, conflicted, or dirty submodule",
        ));
    }
    let trimmed = text.trim_start();
    let mut fields = trimmed.split_whitespace();
    let revision = fields
        .next()
        .ok_or_else(|| OnnxWindowsSourceError::new("malformed ONNX submodule status"))?;
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(OnnxWindowsSourceError::new(
            "malformed ONNX submodule revision",
        ));
    }
    fields
        .next()
        .ok_or_else(|| OnnxWindowsSourceError::new("malformed ONNX submodule path"))
}

fn verify_git_checkout(
    root: &Path,
    expected_repository: &str,
    expected_commit: &str,
) -> Result<(), OnnxWindowsSourceError> {
    let status = git_output(
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if !status.is_empty() {
        return Err(OnnxWindowsSourceError::new(format!(
            "ONNX source checkout is dirty: {}",
            root.display()
        )));
    }
    if git_text(root, &["rev-parse", "HEAD"])? != expected_commit {
        return Err(OnnxWindowsSourceError::new(format!(
            "ONNX source checkout has unexpected commit: {}",
            root.display()
        )));
    }
    if git_text(root, &["remote", "get-url", "origin"])? != expected_repository {
        return Err(OnnxWindowsSourceError::new(format!(
            "ONNX source checkout has unexpected origin: {}",
            root.display()
        )));
    }
    Ok(())
}

fn git_archive_files(root: &Path, prefix: &str) -> Result<Vec<SourceFile>, OnnxWindowsSourceError> {
    let bytes = git_output(root, &["archive", "--format=tar", "HEAD"])?;
    let mut archive = Archive::new(Cursor::new(bytes));
    let mut files = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir()
            || entry_type.is_pax_global_extensions()
            || entry_type.is_pax_local_extensions()
        {
            continue;
        }
        if !entry_type.is_file() {
            return Err(OnnxWindowsSourceError::new(format!(
                "ONNX Git archive contains a non-file member under {}",
                root.display()
            )));
        }
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        crate::archive::refuse_escape(&path)
            .map_err(|source| OnnxWindowsSourceError::new(source.as_str()))?;
        // `git archive` applies the local archive umask to non-executable
        // files (for example, Git mode 100644 can appear as tar mode 664).
        // The build input stores only the Git-relevant executable bit so the
        // output remains identical regardless of the host that archived it.
        let raw_mode = entry.header().mode()? & 0o777;
        let mode = if raw_mode & 0o111 == 0 { 0o644 } else { 0o755 };
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents)?;
        files.push(SourceFile {
            path: format!("{prefix}{path}"),
            mode,
            bytes: contents,
        });
    }
    Ok(files)
}

fn write_archive(
    destination: &Path,
    files: &[SourceFile],
    manifest_name: &str,
    manifest: &[u8],
) -> Result<(), OnnxWindowsSourceError> {
    ensure_manifest_is_not_source_member(files, manifest_name)?;
    let temporary = destination.with_extension("partial");
    if temporary.exists() {
        return Err(OnnxWindowsSourceError::new(format!(
            "refusing to replace interrupted ONNX archive {}",
            temporary.display()
        )));
    }
    let result = (|| {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let encoder = crate::tar::deterministic_gzip(file);
        let mut builder = Builder::new(encoder);
        let mut all = BTreeMap::new();
        for source in files {
            all.insert(source.path.as_str(), (source.mode, source.bytes.as_slice()));
        }
        all.insert(manifest_name, (0o644, manifest));
        for (path, (mode, bytes)) in all {
            append_source_regular(&mut builder, path, bytes, mode)?;
        }
        builder.finish()?;
        builder.into_inner()?.finish()?;
        Ok::<(), OnnxWindowsSourceError>(())
    })();
    match result {
        Ok(()) => match fs::rename(&temporary, destination) {
            Ok(()) => Ok(()),
            Err(error) => {
                if let Err(cleanup_error) = fs::remove_file(&temporary) {
                    if cleanup_error.kind() != std::io::ErrorKind::NotFound {
                        return Err(OnnxWindowsSourceError::new(format!(
                            "could not publish ONNX archive {}: {error}; additionally could not remove incomplete archive {}: {cleanup_error}",
                            destination.display(),
                            temporary.display()
                        )));
                    }
                }
                Err(OnnxWindowsSourceError::new(format!(
                    "could not publish ONNX archive {}: {error}",
                    destination.display()
                )))
            }
        },
        Err(error) => {
            if let Err(cleanup_error) = fs::remove_file(&temporary) {
                if cleanup_error.kind() != std::io::ErrorKind::NotFound {
                    return Err(OnnxWindowsSourceError::new(format!(
                        "{error}; additionally could not remove incomplete ONNX archive {}: {cleanup_error}",
                        temporary.display()
                    )));
                }
            }
            Err(error)
        }
    }
}

fn ensure_manifest_is_not_source_member(
    files: &[SourceFile],
    manifest_name: &str,
) -> Result<(), OnnxWindowsSourceError> {
    if files.iter().any(|file| file.path == manifest_name) {
        Err(OnnxWindowsSourceError::new(format!(
            "ONNX archive source collides with its manifest member {manifest_name}"
        )))
    } else {
        Ok(())
    }
}

fn append_source_regular<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    bytes: &[u8],
    mode: u32,
) -> Result<(), OnnxWindowsSourceError> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_username("")?;
    header.set_groupname("")?;
    header.set_mtime(0);
    // `append_data` emits a GNU long-name record when necessary. Setting the
    // path directly would reject a valid tracked ONNX path before that happens.
    builder.append_data(&mut header, path, bytes)?;
    Ok(())
}

fn ensure_distinct_files(files: &[SourceFile], label: &str) -> Result<(), OnnxWindowsSourceError> {
    let paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    if paths.len() != files.len() {
        return Err(OnnxWindowsSourceError::new(format!(
            "{label} has duplicate member paths"
        )));
    }
    Ok(())
}

fn digest_files(files: &[SourceFile]) -> String {
    let records = files.iter().map(SourceFile::record).collect::<Vec<_>>();
    sha256_hex(&serde_json::to_vec(&records).expect("source archive records serialize"))
}

fn verify_input_bytes(
    bytes: &[u8],
    expected_sha1: &str,
    expected_sha256: &str,
    expected_size: u64,
    label: &str,
) -> Result<(), OnnxWindowsSourceError> {
    if bytes.len() as u64 != expected_size
        || sha1_hex(bytes) != expected_sha1
        || sha256_hex(bytes) != expected_sha256
    {
        return Err(OnnxWindowsSourceError::new(format!(
            "ONNX input {label} has an unexpected identity"
        )));
    }
    Ok(())
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut message = bytes.to_vec();
    let bit_len = (message.len() as u64)
        .checked_mul(8)
        .expect("SHA-1 message length fits u64 bits");
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let mut h0 = 0x6745_2301u32;
    let mut h1 = 0xefcd_ab89u32;
    let mut h2 = 0x98ba_dcfeu32;
    let mut h3 = 0x1032_5476u32;
    let mut h4 = 0xc3d2_e1f0u32;
    for block in message.chunks_exact(64) {
        let mut words = [0u32; 80];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes(block[index * 4..index * 4 + 4].try_into().expect("word"));
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (index, word) in words.into_iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }
    format!("{h0:08x}{h1:08x}{h2:08x}{h3:08x}{h4:08x}")
}

fn mirror_path(url: &str) -> Result<String, OnnxWindowsSourceError> {
    let path = url.strip_prefix("https://").ok_or_else(|| {
        OnnxWindowsSourceError::new(format!("ONNX mirror URL must use HTTPS: {url}"))
    })?;
    if path.is_empty() || path.contains('?') || path.contains('#') {
        return Err(OnnxWindowsSourceError::new(format!(
            "ONNX mirror URL cannot be represented safely: {url}"
        )));
    }
    crate::archive::refuse_escape(path)
        .map_err(|source| OnnxWindowsSourceError::new(source.as_str()))?;
    Ok(path.to_owned())
}

fn require_dependency(
    dependency: &DependencySource,
    repository: &str,
    revision: &str,
    label: &str,
) -> Result<(), OnnxWindowsSourceError> {
    if dependency.repository != repository || dependency.revision != revision {
        return Err(OnnxWindowsSourceError::new(format!(
            "ONNX source archive has an unexpected {label} identity"
        )));
    }
    require_hex(
        &dependency.content_sha256,
        64,
        &format!("{label} content SHA-256"),
    )
}

fn require_exact_set(
    label: &str,
    expected: BTreeSet<&str>,
    actual: BTreeSet<&str>,
) -> Result<(), OnnxWindowsSourceError> {
    if expected == actual {
        Ok(())
    } else {
        Err(OnnxWindowsSourceError::new(format!(
            "{label} are not the reviewed set"
        )))
    }
}

fn require_hex(value: &str, length: usize, label: &str) -> Result<(), OnnxWindowsSourceError> {
    if value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(OnnxWindowsSourceError::new(format!(
            "{label} must be {length} hexadecimal characters"
        )))
    }
}

fn input_identity(label: &str, bytes: &[u8]) -> InputIdentityEntry {
    InputIdentityEntry {
        label: label.to_owned(),
        sha256: sha256_hex(bytes),
        size: bytes.len() as u64,
    }
}

fn canonical_json(value: &impl serde::Serialize) -> Result<Vec<u8>, OnnxWindowsSourceError> {
    serde_json::to_vec(value).map_err(|source| OnnxWindowsSourceError::new(source.to_string()))
}

fn refuse_existing(path: &Path, label: &str) -> Result<(), OnnxWindowsSourceError> {
    if path.exists() {
        Err(OnnxWindowsSourceError::new(format!(
            "refusing to replace existing {label} {}",
            path.display()
        )))
    } else {
        Ok(())
    }
}

fn repository_root() -> Result<PathBuf, OnnxWindowsSourceError> {
    let mut cursor = std::env::current_dir()?;
    loop {
        if cursor.join(ONNX_WINDOWS_INPUTS_PATH).is_file() {
            return Ok(cursor);
        }
        cursor = cursor.parent().map(Path::to_path_buf).ok_or_else(|| {
            OnnxWindowsSourceError::new(format!("could not find {ONNX_WINDOWS_INPUTS_PATH}"))
        })?;
    }
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, OnnxWindowsSourceError> {
    let value = git_output(root, args)?;
    let text = std::str::from_utf8(&value)
        .map_err(|source| OnnxWindowsSourceError::new(source.to_string()))?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>, OnnxWindowsSourceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|source| OnnxWindowsSourceError::new(source.to_string()))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(OnnxWindowsSourceError::new(format!(
            "git -C {} {} failed: {}",
            root.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim_end()
        )))
    }
}

fn parse_flags(args: &[String]) -> Result<BTreeMap<String, String>, OnnxWindowsSourceError> {
    if !args.len().is_multiple_of(2) {
        return Err(OnnxWindowsSourceError::new(
            "ONNX Windows command flags must be name/value pairs",
        ));
    }
    let mut flags = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        let name = &pair[0];
        if !name.starts_with("--") {
            return Err(OnnxWindowsSourceError::new(format!(
                "invalid ONNX Windows flag {name:?}"
            )));
        }
        if flags.insert(name.clone(), pair[1].clone()).is_some() {
            return Err(OnnxWindowsSourceError::new(format!(
                "duplicate ONNX Windows flag {name}"
            )));
        }
    }
    Ok(flags)
}

fn required_path(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<PathBuf, OnnxWindowsSourceError> {
    let value = flags
        .get(name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OnnxWindowsSourceError::new(format!("missing required {name}")))?;
    Ok(PathBuf::from(value))
}

fn require_only(
    flags: &BTreeMap<String, String>,
    admitted: &[&str],
) -> Result<(), OnnxWindowsSourceError> {
    if let Some(unexpected) = flags.keys().find(|name| !admitted.contains(&name.as_str())) {
        Err(OnnxWindowsSourceError::new(format!(
            "unknown ONNX Windows flag {unexpected}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onnx_windows_inputs_are_exact_and_parseable() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("repo")
            .to_path_buf();
        let inputs = load_inputs(&root).expect("committed ONNX Windows inputs");
        assert_eq!(inputs.cmake_dependency.len(), CPU_MIRROR_DEPENDENCIES.len());
        assert_eq!(inputs.protoc_windows_x86_64.sha256.len(), 64);
        let config = read_reduced_ops_config(&root, &inputs).expect("locked config");
        assert_eq!(config.sha256, ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256);
        assert_eq!(config.size, 711);
    }

    #[test]
    fn mirror_paths_are_https_relative_and_safe() {
        assert_eq!(
            mirror_path("https://github.com/onnx/onnx/archive/refs/tags/v1.21.0.zip").unwrap(),
            "github.com/onnx/onnx/archive/refs/tags/v1.21.0.zip"
        );
        assert!(mirror_path("http://example.test/a.zip").is_err());
        assert!(mirror_path("https://example.test/a.zip?next=x").is_err());
    }

    #[test]
    fn sha1_is_verified_without_a_new_workspace_dependency() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn source_members_cannot_replace_the_archive_manifest() {
        let files = vec![SourceFile {
            path: ONNX_WINDOWS_SOURCE_ARCHIVE_MANIFEST.to_owned(),
            mode: 0o644,
            bytes: Vec::new(),
        }];
        assert!(
            ensure_manifest_is_not_source_member(&files, ONNX_WINDOWS_SOURCE_ARCHIVE_MANIFEST)
                .is_err()
        );
    }
}
