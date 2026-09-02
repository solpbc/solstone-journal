// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows ONNX Runtime controlled-build admission facts.
//!
//! Static admission-record schema slice: pins and fail-closed validators
//! only. No build, fetch, spawn, probe, DLL-load, or model-load is performed
//! here. This is a sibling of `parakeet_windows` and `ced_windows`, not an
//! extension of `ControlledBuildReceiptDraft`, and it is structurally
//! separate from `onnx_runtime`'s `TARGETS` wheel-staging table — there is
//! no `wheel_url` field and no extraction route.
//!
//! The ORT source-tree digest, submodule names and digests, and
//! builder-input versions and digests are an invented plausible fixture,
//! not a verified ONNX Runtime inventory or real captured hashes. The three
//! model digests are hardcoded local consts rather than a
//! `solstone-core-transcribe` dependency, for the same reason
//! `parakeet_windows` states for its own model digests: this crate has no
//! `solstone-core-assets` / `solstone-core-local` dependency to defer to,
//! and this surface must compare a digest.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::controlled_build::DependencySource;
use crate::import_policy::is_ordinary_import;
use crate::pe::PeInfo;
use crate::provenance;

pub const ONNX_RUNTIME_TAG: &str = "v1.25.0";
pub const ONNX_RUNTIME_COMMIT: &str = "7a71bc575b189cdedea7fa2c0f87389f870bd10e";
pub const ONNX_RUNTIME_REPOSITORY: &str = "microsoft/onnxruntime";
pub const ONNX_RUNTIME_CONTENT_SHA256: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

pub const ONNX_WINDOWS_TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
pub const ONNX_WINDOWS_BUILD_PROFILE: &str = "Release";
pub const ONNX_WINDOWS_API_LEVEL: u32 = 24;

pub const ONNX_WINDOWS_BUILD_FLAGS: &[&str] = &[
    "--build_shared_lib",
    "--config=Release",
    "--disable_telemetry",
    "--include_ops_by_config=solstone-ort-reduced-ops.config",
];

pub const ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256: &str =
    "d624f0c4902d58df3a5513535450ad4a5c2fffdd95676b23d44c925886b2654d";

/// Invented 4-name fixture; not a verified ONNX Runtime submodule inventory.
pub const ONNX_WINDOWS_REQUIRED_SUBMODULES: &[&str] =
    &["onnx", "protobuf", "abseil-cpp", "flatbuffers"];
pub const ONNX_WINDOWS_SUBMODULE_ONNX_SHA256: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";
pub const ONNX_WINDOWS_SUBMODULE_PROTOBUF_SHA256: &str =
    "2222222222222222222222222222222222222222222222222222222222222222";
pub const ONNX_WINDOWS_SUBMODULE_ABSEIL_SHA256: &str =
    "3333333333333333333333333333333333333333333333333333333333333333";
pub const ONNX_WINDOWS_SUBMODULE_FLATBUFFERS_SHA256: &str =
    "4444444444444444444444444444444444444444444444444444444444444444";

pub const ONNX_WINDOWS_PYTHON_VERSION: &str = "3.12.8";
pub const ONNX_WINDOWS_PYTHON_SHA256: &str =
    "5151515151515151515151515151515151515151515151515151515151515151";
pub const ONNX_WINDOWS_CMAKE_VERSION: &str = "3.31.6";
pub const ONNX_WINDOWS_CMAKE_SHA256: &str =
    "5252525252525252525252525252525252525252525252525252525252525252";
pub const ONNX_WINDOWS_MSVC_VERSION: &str = "19.40";
pub const ONNX_WINDOWS_MSVC_SHA256: &str =
    "5353535353535353535353535353535353535353535353535353535353535353";

pub const WESPEAKER_FILENAME: &str = "wespeaker-resnet34-256.onnx";
pub const WESPEAKER_SIZE_BYTES: u64 = 26_534_365;
pub const WESPEAKER_SHA256: &str =
    "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94";
pub const PYANNOTE_FILENAME: &str = "pyannote-segmentation-3.0.onnx";
pub const PYANNOTE_SIZE_BYTES: u64 = 5_986_908;
pub const PYANNOTE_SHA256: &str =
    "057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25";
pub const SILERO_VAD_FILENAME: &str = "silero_vad_v6.onnx";
pub const SILERO_VAD_SIZE_BYTES: u64 = 1_245_151;
pub const SILERO_VAD_SHA256: &str =
    "4cbf549b8326f60f80f2536d9eefeb450a9abe83365a098031c89719f1be17d2";

pub const ONNX_WINDOWS_PACKAGE_CLOSURE: &[&str] = &["onnxruntime.dll"];
pub const ONNX_WINDOWS_NOTICE_NAMES: &[&str] = &[
    "onnxruntime-LICENSE.txt",
    "onnxruntime-ThirdPartyNotices.txt",
];
pub const ONNX_WINDOWS_API_POLICY: u32 = 25;
pub const ONNX_WINDOWS_ORDINARY_IMPORT: &str = "onnxruntime.dll";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedSource {
    pub tag: String,
    pub dependency: DependencySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLibrary {
    Md,
    Mt,
}

impl RuntimeLibrary {
    fn as_str(self) -> &'static str {
        match self {
            Self::Md => "Md",
            Self::Mt => "Mt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuilderInputRole {
    Cmake,
    Msvc,
    Python,
}

impl BuilderInputRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cmake => "cmake",
            Self::Msvc => "msvc",
            Self::Python => "python",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderInputIdentity {
    pub version: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnnxWindowsBuildConfiguration {
    pub target_triple: String,
    pub profile: String,
    pub flags: Vec<String>,
    pub runtime_library: RuntimeLibrary,
    pub cpu_only: bool,
    pub telemetry_enabled: bool,
    pub network_access_denied: bool,
    pub api_level: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelRole {
    Pyannote,
    SileroVad,
    Wespeaker,
}

impl ModelRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pyannote => "pyannote",
            Self::SileroVad => "silero_vad",
            Self::Wespeaker => "wespeaker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnnxModelSource {
    Raw,
    Converted,
    OrtFormat,
}

impl OnnxModelSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "Raw",
            Self::Converted => "Converted",
            Self::OrtFormat => "OrtFormat",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInputIdentity {
    pub role: ModelRole,
    pub source: OnnxModelSource,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnnxWindowsAdmissionCategory {
    SourceIdentity,
    Submodules,
    BuilderInputs,
    BuildConfiguration,
    ReducedOpsConfig,
    ModelInputs,
}

impl OnnxWindowsAdmissionCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::SourceIdentity => "source identity",
            Self::Submodules => "submodules",
            Self::BuilderInputs => "builder inputs",
            Self::BuildConfiguration => "build configuration",
            Self::ReducedOpsConfig => "reduced ops config",
            Self::ModelInputs => "model inputs",
        }
    }
}

impl fmt::Display for OnnxWindowsAdmissionCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OnnxWindowsAdmissionDraft {
    pub source: Option<TaggedSource>,
    pub submodules: Option<BTreeMap<String, DependencySource>>,
    pub builder_inputs: Option<BTreeMap<BuilderInputRole, BuilderInputIdentity>>,
    pub build: Option<OnnxWindowsBuildConfiguration>,
    pub reduced_ops_config_sha256: Option<String>,
    pub models: Option<Vec<ModelInputIdentity>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnnxWindowsAdmission {
    pub source: TaggedSource,
    pub submodules: BTreeMap<String, DependencySource>,
    pub builder_inputs: BTreeMap<BuilderInputRole, BuilderInputIdentity>,
    pub build: OnnxWindowsBuildConfiguration,
    pub reduced_ops_config_sha256: String,
    pub models: Vec<ModelInputIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperRole {
    Speaker,
    Vad,
}

impl HelperRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Speaker => "speaker",
            Self::Vad => "vad",
        }
    }
}

impl fmt::Display for HelperRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OnnxHelperCensusDraft {
    pub role: Option<HelperRole>,
    pub import_census: Option<PeInfo>,
    pub package_closure: Option<Vec<String>>,
    pub notices: Option<BTreeMap<String, Vec<u8>>>,
    pub api_policy: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnnxHelperCensusAdmission {
    pub role: HelperRole,
    pub import_census: PeInfo,
    pub package_closure: Vec<String>,
    pub notices: BTreeMap<String, Vec<u8>>,
    pub api_policy: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnnxWindowsError {
    Missing {
        category: OnnxWindowsAdmissionCategory,
    },
    MissingRequired {
        role: &'static str,
    },
    Unexpected {
        role: &'static str,
        expected: String,
        observed: String,
    },
    SetMismatch {
        role: &'static str,
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    Census {
        helper: HelperRole,
        inner: Box<Self>,
    },
}

impl fmt::Display for OnnxWindowsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { category } => {
                write!(formatter, "missing required:\n  {category}")
            }
            Self::MissingRequired { role } => {
                write!(formatter, "missing required:\n  {role}")
            }
            Self::Unexpected {
                role,
                expected,
                observed,
            } => write!(
                formatter,
                "unexpected:\n  {role}\n  expected: {expected}\n  observed: {observed}"
            ),
            Self::SetMismatch {
                role,
                missing,
                unexpected,
            } => write!(
                formatter,
                "{role} is not the reviewed set\n  missing: {}\n  unexpected: {}",
                format_diff_side(missing),
                format_diff_side(unexpected)
            ),
            Self::Census { helper, inner } => {
                write!(formatter, "{helper} helper: {inner}")
            }
        }
    }
}

impl std::error::Error for OnnxWindowsError {}

fn format_diff_side(names: &[String]) -> String {
    if names.is_empty() {
        "<none>".to_owned()
    } else {
        names.join(", ")
    }
}

fn unexpected(
    role: &'static str,
    expected: impl Into<String>,
    observed: impl Into<String>,
) -> OnnxWindowsError {
    OnnxWindowsError::Unexpected {
        role,
        expected: expected.into(),
        observed: observed.into(),
    }
}

fn missing(category: OnnxWindowsAdmissionCategory) -> OnnxWindowsError {
    OnnxWindowsError::Missing { category }
}

fn require_nonempty_string(
    value: &str,
    role: &'static str,
    expected: &str,
) -> Result<(), OnnxWindowsError> {
    if value.is_empty() {
        return Err(OnnxWindowsError::MissingRequired { role });
    }
    if value != expected {
        return Err(unexpected(role, expected, value));
    }
    Ok(())
}

fn require_pinned_commit(
    expected: &str,
    actual: &str,
    role: &'static str,
) -> Result<(), OnnxWindowsError> {
    if actual.is_empty() {
        return Err(OnnxWindowsError::MissingRequired { role });
    }
    provenance::require_commit(expected, actual).map_err(|_| unexpected(role, expected, actual))
}

fn telemetry_state(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

fn set_mismatch(
    role: &'static str,
    expected: BTreeSet<&str>,
    actual: BTreeSet<&str>,
) -> Result<(), OnnxWindowsError> {
    if actual == expected {
        return Ok(());
    }
    let missing: Vec<String> = expected
        .iter()
        .filter(|name| !actual.contains(*name))
        .map(|name| (*name).to_owned())
        .collect();
    let unexpected_names: Vec<String> = actual
        .iter()
        .filter(|name| !expected.contains(*name))
        .map(|name| (*name).to_owned())
        .collect();
    Err(OnnxWindowsError::SetMismatch {
        role,
        missing,
        unexpected: unexpected_names,
    })
}

#[must_use]
pub fn onnx_windows_build_configuration() -> OnnxWindowsBuildConfiguration {
    OnnxWindowsBuildConfiguration {
        target_triple: ONNX_WINDOWS_TARGET_TRIPLE.to_owned(),
        profile: ONNX_WINDOWS_BUILD_PROFILE.to_owned(),
        flags: ONNX_WINDOWS_BUILD_FLAGS
            .iter()
            .map(|flag| (*flag).to_owned())
            .collect(),
        runtime_library: RuntimeLibrary::Md,
        cpu_only: true,
        telemetry_enabled: false,
        network_access_denied: true,
        api_level: ONNX_WINDOWS_API_LEVEL,
    }
}

fn verify_source_identity(source: &TaggedSource) -> Result<(), OnnxWindowsError> {
    require_nonempty_string(&source.tag, "onnxruntime tag", ONNX_RUNTIME_TAG)?;
    require_nonempty_string(
        &source.dependency.repository,
        "onnxruntime repository",
        ONNX_RUNTIME_REPOSITORY,
    )?;
    require_pinned_commit(
        ONNX_RUNTIME_COMMIT,
        &source.dependency.revision,
        "onnxruntime commit",
    )?;
    require_nonempty_string(
        &source.dependency.content_sha256,
        "onnxruntime digest",
        ONNX_RUNTIME_CONTENT_SHA256,
    )
}

fn submodule_digest(name: &str) -> Option<&'static str> {
    match name {
        "onnx" => Some(ONNX_WINDOWS_SUBMODULE_ONNX_SHA256),
        "protobuf" => Some(ONNX_WINDOWS_SUBMODULE_PROTOBUF_SHA256),
        "abseil-cpp" => Some(ONNX_WINDOWS_SUBMODULE_ABSEIL_SHA256),
        "flatbuffers" => Some(ONNX_WINDOWS_SUBMODULE_FLATBUFFERS_SHA256),
        _ => None,
    }
}

fn submodule_revision_role(name: &str) -> &'static str {
    match name {
        "onnx" => "onnx submodule revision",
        "protobuf" => "protobuf submodule revision",
        "abseil-cpp" => "abseil-cpp submodule revision",
        "flatbuffers" => "flatbuffers submodule revision",
        _ => "submodule revision",
    }
}

fn submodule_digest_role(name: &str) -> &'static str {
    match name {
        "onnx" => "onnx submodule digest",
        "protobuf" => "protobuf submodule digest",
        "abseil-cpp" => "abseil-cpp submodule digest",
        "flatbuffers" => "flatbuffers submodule digest",
        _ => "submodule digest",
    }
}

fn verify_submodules(
    submodules: &BTreeMap<String, DependencySource>,
) -> Result<(), OnnxWindowsError> {
    let expected: BTreeSet<&str> = ONNX_WINDOWS_REQUIRED_SUBMODULES.iter().copied().collect();
    let actual: BTreeSet<&str> = submodules.keys().map(String::as_str).collect();
    set_mismatch("submodules", expected, actual)?;
    for name in ONNX_WINDOWS_REQUIRED_SUBMODULES {
        let identity = submodules
            .get(*name)
            .expect("required submodule present after set check");
        if identity.revision.is_empty() {
            return Err(OnnxWindowsError::MissingRequired {
                role: submodule_revision_role(name),
            });
        }
        let digest = submodule_digest(name).expect("required submodule has a pinned digest");
        require_nonempty_string(
            &identity.content_sha256,
            submodule_digest_role(name),
            digest,
        )?;
    }
    Ok(())
}

fn builder_version_role(role: BuilderInputRole) -> &'static str {
    match role {
        BuilderInputRole::Cmake => "cmake version",
        BuilderInputRole::Msvc => "msvc version",
        BuilderInputRole::Python => "python version",
    }
}

fn builder_digest_role(role: BuilderInputRole) -> &'static str {
    match role {
        BuilderInputRole::Cmake => "cmake digest",
        BuilderInputRole::Msvc => "msvc digest",
        BuilderInputRole::Python => "python digest",
    }
}

fn builder_pin(role: BuilderInputRole) -> (&'static str, &'static str) {
    match role {
        BuilderInputRole::Cmake => (ONNX_WINDOWS_CMAKE_VERSION, ONNX_WINDOWS_CMAKE_SHA256),
        BuilderInputRole::Msvc => (ONNX_WINDOWS_MSVC_VERSION, ONNX_WINDOWS_MSVC_SHA256),
        BuilderInputRole::Python => (ONNX_WINDOWS_PYTHON_VERSION, ONNX_WINDOWS_PYTHON_SHA256),
    }
}

fn verify_builder_inputs(
    inputs: &BTreeMap<BuilderInputRole, BuilderInputIdentity>,
) -> Result<(), OnnxWindowsError> {
    let expected: BTreeSet<&str> = [
        BuilderInputRole::Cmake,
        BuilderInputRole::Msvc,
        BuilderInputRole::Python,
    ]
    .into_iter()
    .map(BuilderInputRole::as_str)
    .collect();
    let actual: BTreeSet<&str> = inputs
        .keys()
        .copied()
        .map(BuilderInputRole::as_str)
        .collect();
    set_mismatch("builder inputs", expected, actual)?;
    for role in [
        BuilderInputRole::Cmake,
        BuilderInputRole::Msvc,
        BuilderInputRole::Python,
    ] {
        let identity = inputs
            .get(&role)
            .expect("required builder input present after set check");
        let (version, digest) = builder_pin(role);
        require_nonempty_string(&identity.version, builder_version_role(role), version)?;
        require_nonempty_string(&identity.content_sha256, builder_digest_role(role), digest)?;
    }
    Ok(())
}

fn verify_build_configuration(
    configuration: &OnnxWindowsBuildConfiguration,
) -> Result<(), OnnxWindowsError> {
    let pinned = onnx_windows_build_configuration();
    if configuration.target_triple != pinned.target_triple {
        return Err(unexpected(
            "target triple",
            pinned.target_triple,
            &configuration.target_triple,
        ));
    }
    if configuration.profile != pinned.profile {
        return Err(unexpected(
            "profile",
            pinned.profile,
            &configuration.profile,
        ));
    }
    if configuration.runtime_library != pinned.runtime_library {
        return Err(unexpected(
            "runtime library",
            pinned.runtime_library.as_str(),
            configuration.runtime_library.as_str(),
        ));
    }
    if configuration.cpu_only != pinned.cpu_only {
        return Err(unexpected(
            "cpu only",
            pinned.cpu_only.to_string(),
            configuration.cpu_only.to_string(),
        ));
    }
    if configuration.telemetry_enabled != pinned.telemetry_enabled {
        return Err(unexpected(
            "telemetry",
            telemetry_state(pinned.telemetry_enabled),
            telemetry_state(configuration.telemetry_enabled),
        ));
    }
    if configuration.network_access_denied != pinned.network_access_denied {
        return Err(unexpected(
            "network access denied",
            pinned.network_access_denied.to_string(),
            configuration.network_access_denied.to_string(),
        ));
    }
    if configuration.api_level != pinned.api_level {
        return Err(unexpected(
            "api level",
            pinned.api_level.to_string(),
            configuration.api_level.to_string(),
        ));
    }
    let expected: BTreeSet<&str> = ONNX_WINDOWS_BUILD_FLAGS.iter().copied().collect();
    let actual: BTreeSet<&str> = configuration.flags.iter().map(String::as_str).collect();
    set_mismatch("build flags", expected, actual)
}

fn verify_reduced_ops_config(digest: &str) -> Result<(), OnnxWindowsError> {
    require_nonempty_string(
        digest,
        "reduced ops config digest",
        ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256,
    )
}

fn model_pin(role: ModelRole) -> (&'static str, u64, &'static str) {
    match role {
        ModelRole::Wespeaker => (WESPEAKER_FILENAME, WESPEAKER_SIZE_BYTES, WESPEAKER_SHA256),
        ModelRole::Pyannote => (PYANNOTE_FILENAME, PYANNOTE_SIZE_BYTES, PYANNOTE_SHA256),
        ModelRole::SileroVad => (
            SILERO_VAD_FILENAME,
            SILERO_VAD_SIZE_BYTES,
            SILERO_VAD_SHA256,
        ),
    }
}

fn model_source_role(role: ModelRole) -> &'static str {
    match role {
        ModelRole::Wespeaker => "wespeaker source",
        ModelRole::Pyannote => "pyannote source",
        ModelRole::SileroVad => "silero_vad source",
    }
}

fn model_filename_role(role: ModelRole) -> &'static str {
    match role {
        ModelRole::Wespeaker => "wespeaker filename",
        ModelRole::Pyannote => "pyannote filename",
        ModelRole::SileroVad => "silero_vad filename",
    }
}

fn model_size_role(role: ModelRole) -> &'static str {
    match role {
        ModelRole::Wespeaker => "wespeaker size",
        ModelRole::Pyannote => "pyannote size",
        ModelRole::SileroVad => "silero_vad size",
    }
}

fn model_sha256_role(role: ModelRole) -> &'static str {
    match role {
        ModelRole::Wespeaker => "wespeaker sha256",
        ModelRole::Pyannote => "pyannote sha256",
        ModelRole::SileroVad => "silero_vad sha256",
    }
}

fn verify_model_inputs(models: &[ModelInputIdentity]) -> Result<(), OnnxWindowsError> {
    let mut counts: BTreeMap<ModelRole, u32> = BTreeMap::new();
    for model in models {
        *counts.entry(model.role).or_insert(0) += 1;
    }
    let required = [
        ModelRole::Pyannote,
        ModelRole::SileroVad,
        ModelRole::Wespeaker,
    ];
    let missing: Vec<String> = required
        .into_iter()
        .filter(|role| counts.get(role).copied().unwrap_or(0) == 0)
        .map(|role| role.as_str().to_owned())
        .collect();
    let unexpected_names: Vec<String> = required
        .into_iter()
        .filter(|role| counts.get(role).copied().unwrap_or(0) > 1)
        .map(|role| role.as_str().to_owned())
        .collect();
    if !missing.is_empty() || !unexpected_names.is_empty() {
        return Err(OnnxWindowsError::SetMismatch {
            role: "model inputs",
            missing,
            unexpected: unexpected_names,
        });
    }
    for role in required {
        let identity = models
            .iter()
            .find(|model| model.role == role)
            .expect("role present exactly once after count check");
        if identity.source != OnnxModelSource::Raw {
            return Err(unexpected(
                model_source_role(role),
                OnnxModelSource::Raw.as_str(),
                identity.source.as_str(),
            ));
        }
        let (filename, size, sha256) = model_pin(role);
        if identity.filename != filename {
            return Err(unexpected(
                model_filename_role(role),
                filename,
                &identity.filename,
            ));
        }
        if identity.size != size {
            return Err(unexpected(
                model_size_role(role),
                size.to_string(),
                identity.size.to_string(),
            ));
        }
        if identity.sha256 != sha256 {
            return Err(unexpected(
                model_sha256_role(role),
                sha256,
                &identity.sha256,
            ));
        }
    }
    Ok(())
}

pub fn admit(draft: OnnxWindowsAdmissionDraft) -> Result<OnnxWindowsAdmission, OnnxWindowsError> {
    let source = draft
        .source
        .ok_or_else(|| missing(OnnxWindowsAdmissionCategory::SourceIdentity))?;
    verify_source_identity(&source)?;
    let submodules = draft
        .submodules
        .ok_or_else(|| missing(OnnxWindowsAdmissionCategory::Submodules))?;
    verify_submodules(&submodules)?;
    let builder_inputs = draft
        .builder_inputs
        .ok_or_else(|| missing(OnnxWindowsAdmissionCategory::BuilderInputs))?;
    verify_builder_inputs(&builder_inputs)?;
    let build = draft
        .build
        .ok_or_else(|| missing(OnnxWindowsAdmissionCategory::BuildConfiguration))?;
    verify_build_configuration(&build)?;
    let reduced_ops_config_sha256 = draft
        .reduced_ops_config_sha256
        .ok_or_else(|| missing(OnnxWindowsAdmissionCategory::ReducedOpsConfig))?;
    verify_reduced_ops_config(&reduced_ops_config_sha256)?;
    let models = draft
        .models
        .ok_or_else(|| missing(OnnxWindowsAdmissionCategory::ModelInputs))?;
    verify_model_inputs(&models)?;
    Ok(OnnxWindowsAdmission {
        source,
        submodules,
        builder_inputs,
        build,
        reduced_ops_config_sha256,
        models,
    })
}

fn verify_helper_notices(notices: &BTreeMap<String, Vec<u8>>) -> Result<(), OnnxWindowsError> {
    let expected: BTreeSet<&str> = ONNX_WINDOWS_NOTICE_NAMES.iter().copied().collect();
    let actual: BTreeSet<&str> = notices.keys().map(String::as_str).collect();
    set_mismatch("notices", expected, actual)?;
    for name in ONNX_WINDOWS_NOTICE_NAMES {
        let bytes = notices
            .get(*name)
            .expect("notice name verified present above");
        if bytes.is_empty() {
            return Err(unexpected("notice bytes", "non-empty", *name));
        }
    }
    Ok(())
}

fn admit_helper_census_inner(
    expected: HelperRole,
    draft: &OnnxHelperCensusDraft,
) -> Result<OnnxHelperCensusAdmission, OnnxWindowsError> {
    let role = draft.role.ok_or(OnnxWindowsError::MissingRequired {
        role: "helper role",
    })?;
    if role != expected {
        return Err(unexpected("helper role", expected.as_str(), role.as_str()));
    }
    let import_census = draft
        .import_census
        .clone()
        .ok_or(OnnxWindowsError::MissingRequired {
            role: "import census",
        })?;
    if is_ordinary_import(&import_census.imports, ONNX_WINDOWS_ORDINARY_IMPORT) {
        return Err(unexpected(
            "ordinary import",
            "absent",
            ONNX_WINDOWS_ORDINARY_IMPORT,
        ));
    }
    let package_closure =
        draft
            .package_closure
            .clone()
            .ok_or(OnnxWindowsError::MissingRequired {
                role: "package closure",
            })?;
    let expected_closure: BTreeSet<&str> = ONNX_WINDOWS_PACKAGE_CLOSURE.iter().copied().collect();
    let actual_closure: BTreeSet<&str> = package_closure.iter().map(String::as_str).collect();
    set_mismatch("package closure", expected_closure, actual_closure)?;
    let notices = draft
        .notices
        .clone()
        .ok_or(OnnxWindowsError::MissingRequired { role: "notices" })?;
    verify_helper_notices(&notices)?;
    let api_policy = draft
        .api_policy
        .ok_or(OnnxWindowsError::MissingRequired { role: "api policy" })?;
    if api_policy != ONNX_WINDOWS_API_POLICY {
        return Err(unexpected(
            "api policy",
            ONNX_WINDOWS_API_POLICY.to_string(),
            api_policy.to_string(),
        ));
    }
    Ok(OnnxHelperCensusAdmission {
        role: expected,
        import_census,
        package_closure,
        notices,
        api_policy,
    })
}

pub fn admit_helper_census(
    expected: HelperRole,
    draft: &OnnxHelperCensusDraft,
) -> Result<OnnxHelperCensusAdmission, OnnxWindowsError> {
    admit_helper_census_inner(expected, draft).map_err(|inner| OnnxWindowsError::Census {
        helper: expected,
        inner: Box::new(inner),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::{FixtureSpec, ImportSpec, fixture, parse_pe};

    fn pinned_source() -> TaggedSource {
        TaggedSource {
            tag: ONNX_RUNTIME_TAG.to_owned(),
            dependency: DependencySource {
                repository: ONNX_RUNTIME_REPOSITORY.to_owned(),
                revision: ONNX_RUNTIME_COMMIT.to_owned(),
                content_sha256: ONNX_RUNTIME_CONTENT_SHA256.to_owned(),
            },
        }
    }

    fn pinned_submodules() -> BTreeMap<String, DependencySource> {
        let mut submodules = BTreeMap::new();
        for (name, digest) in [
            ("onnx", ONNX_WINDOWS_SUBMODULE_ONNX_SHA256),
            ("protobuf", ONNX_WINDOWS_SUBMODULE_PROTOBUF_SHA256),
            ("abseil-cpp", ONNX_WINDOWS_SUBMODULE_ABSEIL_SHA256),
            ("flatbuffers", ONNX_WINDOWS_SUBMODULE_FLATBUFFERS_SHA256),
        ] {
            submodules.insert(
                name.to_owned(),
                DependencySource {
                    repository: name.to_owned(),
                    revision: "rev".to_owned(),
                    content_sha256: digest.to_owned(),
                },
            );
        }
        submodules
    }

    fn pinned_builder_inputs() -> BTreeMap<BuilderInputRole, BuilderInputIdentity> {
        let mut inputs = BTreeMap::new();
        for role in [
            BuilderInputRole::Cmake,
            BuilderInputRole::Msvc,
            BuilderInputRole::Python,
        ] {
            let (version, digest) = builder_pin(role);
            inputs.insert(
                role,
                BuilderInputIdentity {
                    version: version.to_owned(),
                    content_sha256: digest.to_owned(),
                },
            );
        }
        inputs
    }

    fn pinned_model(role: ModelRole) -> ModelInputIdentity {
        let (filename, size, sha256) = model_pin(role);
        ModelInputIdentity {
            role,
            source: OnnxModelSource::Raw,
            filename: filename.to_owned(),
            size,
            sha256: sha256.to_owned(),
        }
    }

    fn pinned_models() -> Vec<ModelInputIdentity> {
        vec![
            pinned_model(ModelRole::Wespeaker),
            pinned_model(ModelRole::Pyannote),
            pinned_model(ModelRole::SileroVad),
        ]
    }

    fn census_with_imports(imports: &[ImportSpec<'_>]) -> PeInfo {
        parse_pe(&fixture(&FixtureSpec {
            imports,
            ..FixtureSpec::default()
        }))
        .expect("fixture parses")
    }

    fn pinned_notices() -> BTreeMap<String, Vec<u8>> {
        let mut notices = BTreeMap::new();
        notices.insert(
            "onnxruntime-LICENSE.txt".into(),
            b"onnxruntime-license".to_vec(),
        );
        notices.insert(
            "onnxruntime-ThirdPartyNotices.txt".into(),
            b"onnxruntime-notices".to_vec(),
        );
        notices
    }

    fn canonical_draft() -> OnnxWindowsAdmissionDraft {
        OnnxWindowsAdmissionDraft {
            source: Some(pinned_source()),
            submodules: Some(pinned_submodules()),
            builder_inputs: Some(pinned_builder_inputs()),
            build: Some(onnx_windows_build_configuration()),
            reduced_ops_config_sha256: Some(ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256.to_owned()),
            models: Some(pinned_models()),
        }
    }

    fn canonical_census(role: HelperRole) -> OnnxHelperCensusDraft {
        OnnxHelperCensusDraft {
            role: Some(role),
            import_census: Some(census_with_imports(&[ImportSpec {
                name: "KERNEL32.dll",
                symbols: &[],
            }])),
            package_closure: Some(vec!["onnxruntime.dll".into()]),
            notices: Some(pinned_notices()),
            api_policy: Some(ONNX_WINDOWS_API_POLICY),
        }
    }

    type AdmissionCase = (
        &'static str,
        fn(&mut OnnxWindowsAdmissionDraft),
        OnnxWindowsError,
    );

    fn admit_err(mutate: impl FnOnce(&mut OnnxWindowsAdmissionDraft)) -> OnnxWindowsError {
        let mut draft = canonical_draft();
        mutate(&mut draft);
        admit(draft).expect_err("must reject")
    }

    fn census_wrap(helper: HelperRole, inner: OnnxWindowsError) -> OnnxWindowsError {
        OnnxWindowsError::Census {
            helper,
            inner: Box::new(inner),
        }
    }

    type CensusCase = (
        &'static str,
        HelperRole,
        fn(&mut OnnxHelperCensusDraft),
        OnnxWindowsError,
    );

    fn mutate_source(draft: &mut OnnxWindowsAdmissionDraft) -> &mut TaggedSource {
        draft.source.as_mut().expect("canonical source")
    }

    fn mutate_build(draft: &mut OnnxWindowsAdmissionDraft) -> &mut OnnxWindowsBuildConfiguration {
        draft.build.as_mut().expect("canonical build")
    }

    fn mutate_models(draft: &mut OnnxWindowsAdmissionDraft) -> &mut Vec<ModelInputIdentity> {
        draft.models.as_mut().expect("canonical models")
    }

    #[test]
    fn onnx_windows_admission_accepts_canonical_controlled_inputs() {
        let admitted = admit(canonical_draft()).expect("canonical inputs");
        assert_eq!(admitted.build.api_level, ONNX_WINDOWS_API_LEVEL);
        assert!(!admitted.build.telemetry_enabled);
        assert!(admitted.build.cpu_only);
        assert!(admitted.build.network_access_denied);
        assert_eq!(admitted.build.runtime_library, RuntimeLibrary::Md);
        assert_eq!(admitted.models.len(), 3);
        admit_helper_census(HelperRole::Speaker, &canonical_census(HelperRole::Speaker))
            .expect("canonical speaker census");
        admit_helper_census(HelperRole::Vad, &canonical_census(HelperRole::Vad))
            .expect("canonical vad census");
    }

    #[test]
    fn onnx_windows_admission_rejects_source_model_and_build_variants() {
        let cases: &[AdmissionCase] = &[
            (
                "missing source",
                |draft| draft.source = None,
                missing(OnnxWindowsAdmissionCategory::SourceIdentity),
            ),
            (
                "missing submodules",
                |draft| draft.submodules = None,
                missing(OnnxWindowsAdmissionCategory::Submodules),
            ),
            (
                "missing builder inputs",
                |draft| draft.builder_inputs = None,
                missing(OnnxWindowsAdmissionCategory::BuilderInputs),
            ),
            (
                "missing build",
                |draft| draft.build = None,
                missing(OnnxWindowsAdmissionCategory::BuildConfiguration),
            ),
            (
                "missing reduced ops config",
                |draft| draft.reduced_ops_config_sha256 = None,
                missing(OnnxWindowsAdmissionCategory::ReducedOpsConfig),
            ),
            (
                "missing models",
                |draft| draft.models = None,
                missing(OnnxWindowsAdmissionCategory::ModelInputs),
            ),
            (
                "wrong tag",
                |draft| mutate_source(draft).tag = "v1.24.0".into(),
                unexpected("onnxruntime tag", ONNX_RUNTIME_TAG, "v1.24.0"),
            ),
            (
                "wrong repository",
                |draft| mutate_source(draft).dependency.repository = "other/onnxruntime".into(),
                unexpected(
                    "onnxruntime repository",
                    ONNX_RUNTIME_REPOSITORY,
                    "other/onnxruntime",
                ),
            ),
            (
                "wheel url repository",
                |draft| {
                    mutate_source(draft).dependency.repository = "https://pypi.org/simple/onnxruntime/onnxruntime-1.25.0-cp312-win_amd64.whl".into();
                },
                unexpected(
                    "onnxruntime repository",
                    ONNX_RUNTIME_REPOSITORY,
                    "https://pypi.org/simple/onnxruntime/onnxruntime-1.25.0-cp312-win_amd64.whl",
                ),
            ),
            (
                "empty commit",
                |draft| mutate_source(draft).dependency.revision.clear(),
                OnnxWindowsError::MissingRequired {
                    role: "onnxruntime commit",
                },
            ),
            (
                "wrong commit",
                |draft| mutate_source(draft).dependency.revision = "wrong-commit".into(),
                unexpected("onnxruntime commit", ONNX_RUNTIME_COMMIT, "wrong-commit"),
            ),
            (
                "wrong source digest",
                |draft| mutate_source(draft).dependency.content_sha256 = "0".repeat(64),
                unexpected(
                    "onnxruntime digest",
                    ONNX_RUNTIME_CONTENT_SHA256,
                    "0".repeat(64),
                ),
            ),
            (
                "missing submodule name",
                |draft| {
                    if let Some(submodules) = &mut draft.submodules {
                        submodules.remove("onnx");
                    }
                },
                OnnxWindowsError::SetMismatch {
                    role: "submodules",
                    missing: vec!["onnx".into()],
                    unexpected: Vec::new(),
                },
            ),
            (
                "extra submodule name",
                |draft| {
                    if let Some(submodules) = &mut draft.submodules {
                        submodules.insert(
                            "eigen".into(),
                            DependencySource {
                                repository: "eigen".into(),
                                revision: "rev".into(),
                                content_sha256: "0".repeat(64),
                            },
                        );
                    }
                },
                OnnxWindowsError::SetMismatch {
                    role: "submodules",
                    missing: Vec::new(),
                    unexpected: vec!["eigen".into()],
                },
            ),
            (
                "changed submodule digest",
                |draft| {
                    if let Some(submodules) = &mut draft.submodules
                        && let Some(onnx) = submodules.get_mut("onnx")
                    {
                        onnx.content_sha256 = "0".repeat(64);
                    }
                },
                unexpected(
                    "onnx submodule digest",
                    ONNX_WINDOWS_SUBMODULE_ONNX_SHA256,
                    "0".repeat(64),
                ),
            ),
            (
                "empty submodule revision",
                |draft| {
                    if let Some(submodules) = &mut draft.submodules
                        && let Some(onnx) = submodules.get_mut("onnx")
                    {
                        onnx.revision.clear();
                    }
                },
                OnnxWindowsError::MissingRequired {
                    role: "onnx submodule revision",
                },
            ),
            (
                "missing builder-input role",
                |draft| {
                    if let Some(inputs) = &mut draft.builder_inputs {
                        inputs.remove(&BuilderInputRole::Python);
                    }
                },
                OnnxWindowsError::SetMismatch {
                    role: "builder inputs",
                    missing: vec!["python".into()],
                    unexpected: Vec::new(),
                },
            ),
            (
                "wrong python version",
                |draft| {
                    if let Some(inputs) = &mut draft.builder_inputs
                        && let Some(python) = inputs.get_mut(&BuilderInputRole::Python)
                    {
                        python.version = "3.11.0".into();
                    }
                },
                unexpected("python version", ONNX_WINDOWS_PYTHON_VERSION, "3.11.0"),
            ),
            (
                "wrong python digest",
                |draft| {
                    if let Some(inputs) = &mut draft.builder_inputs
                        && let Some(python) = inputs.get_mut(&BuilderInputRole::Python)
                    {
                        python.content_sha256 = "0".repeat(64);
                    }
                },
                unexpected("python digest", ONNX_WINDOWS_PYTHON_SHA256, "0".repeat(64)),
            ),
            (
                "wrong target triple",
                |draft| mutate_build(draft).target_triple = "aarch64-pc-windows-msvc".into(),
                unexpected(
                    "target triple",
                    ONNX_WINDOWS_TARGET_TRIPLE,
                    "aarch64-pc-windows-msvc",
                ),
            ),
            (
                "debug profile",
                |draft| mutate_build(draft).profile = "Debug".into(),
                unexpected("profile", "Release", "Debug"),
            ),
            (
                "runtime library mt",
                |draft| mutate_build(draft).runtime_library = RuntimeLibrary::Mt,
                unexpected("runtime library", "Md", "Mt"),
            ),
            (
                "cpu only false",
                |draft| mutate_build(draft).cpu_only = false,
                unexpected("cpu only", "true", "false"),
            ),
            (
                "telemetry enabled",
                |draft| mutate_build(draft).telemetry_enabled = true,
                unexpected("telemetry", "disabled", "enabled"),
            ),
            (
                "network access allowed",
                |draft| mutate_build(draft).network_access_denied = false,
                unexpected("network access denied", "true", "false"),
            ),
            (
                "wrong api level",
                |draft| mutate_build(draft).api_level = 23,
                unexpected("api level", "24", "23"),
            ),
            (
                "missing required flag",
                |draft| {
                    mutate_build(draft)
                        .flags
                        .retain(|flag| flag != "--build_shared_lib");
                },
                OnnxWindowsError::SetMismatch {
                    role: "build flags",
                    missing: vec!["--build_shared_lib".into()],
                    unexpected: Vec::new(),
                },
            ),
            (
                "extra unexpected flag",
                |draft| mutate_build(draft).flags.push("--minimal_build".into()),
                OnnxWindowsError::SetMismatch {
                    role: "build flags",
                    missing: Vec::new(),
                    unexpected: vec!["--minimal_build".into()],
                },
            ),
            (
                "empty reduced ops digest",
                |draft| draft.reduced_ops_config_sha256 = Some(String::new()),
                OnnxWindowsError::MissingRequired {
                    role: "reduced ops config digest",
                },
            ),
            (
                "wrong reduced ops digest",
                |draft| draft.reduced_ops_config_sha256 = Some("0".repeat(64)),
                unexpected(
                    "reduced ops config digest",
                    ONNX_WINDOWS_REDUCED_OPS_CONFIG_SHA256,
                    "0".repeat(64),
                ),
            ),
            (
                "missing wespeaker model",
                |draft| {
                    mutate_models(draft).retain(|model| model.role != ModelRole::Wespeaker);
                },
                OnnxWindowsError::SetMismatch {
                    role: "model inputs",
                    missing: vec!["wespeaker".into()],
                    unexpected: Vec::new(),
                },
            ),
            (
                "duplicate pyannote model",
                |draft| {
                    let models = mutate_models(draft);
                    let pyannote = models
                        .iter()
                        .find(|model| model.role == ModelRole::Pyannote)
                        .cloned()
                        .expect("canonical pyannote");
                    models.push(pyannote);
                },
                OnnxWindowsError::SetMismatch {
                    role: "model inputs",
                    missing: Vec::new(),
                    unexpected: vec!["pyannote".into()],
                },
            ),
            (
                "partial silero filename",
                |draft| {
                    if let Some(model) = mutate_models(draft)
                        .iter_mut()
                        .find(|model| model.role == ModelRole::SileroVad)
                    {
                        model.filename = "silero_vad_v5.onnx".into();
                    }
                },
                unexpected(
                    "silero_vad filename",
                    SILERO_VAD_FILENAME,
                    "silero_vad_v5.onnx",
                ),
            ),
            (
                "changed wespeaker sha256",
                |draft| {
                    if let Some(model) = mutate_models(draft)
                        .iter_mut()
                        .find(|model| model.role == ModelRole::Wespeaker)
                    {
                        model.sha256 = "0".repeat(64);
                    }
                },
                unexpected("wespeaker sha256", WESPEAKER_SHA256, "0".repeat(64)),
            ),
            (
                "changed pyannote size",
                |draft| {
                    if let Some(model) = mutate_models(draft)
                        .iter_mut()
                        .find(|model| model.role == ModelRole::Pyannote)
                    {
                        model.size = 1;
                    }
                },
                unexpected("pyannote size", PYANNOTE_SIZE_BYTES.to_string(), "1"),
            ),
            (
                "converted pyannote source",
                |draft| {
                    if let Some(model) = mutate_models(draft)
                        .iter_mut()
                        .find(|model| model.role == ModelRole::Pyannote)
                    {
                        model.source = OnnxModelSource::Converted;
                    }
                },
                unexpected("pyannote source", "Raw", "Converted"),
            ),
            (
                "ort-format silero source",
                |draft| {
                    if let Some(model) = mutate_models(draft)
                        .iter_mut()
                        .find(|model| model.role == ModelRole::SileroVad)
                    {
                        model.source = OnnxModelSource::OrtFormat;
                    }
                },
                unexpected("silero_vad source", "Raw", "OrtFormat"),
            ),
        ];

        for (name, mutate, expected) in cases {
            assert_eq!(&admit_err(mutate), expected, "{name}");
        }
    }

    #[test]
    fn onnx_windows_admission_rejects_package_closure_and_static_import_variants() {
        let cases: &[CensusCase] = &[
            (
                "missing helper role",
                HelperRole::Speaker,
                |draft| draft.role = None,
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::MissingRequired {
                        role: "helper role",
                    },
                ),
            ),
            (
                "missing import census",
                HelperRole::Speaker,
                |draft| draft.import_census = None,
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::MissingRequired {
                        role: "import census",
                    },
                ),
            ),
            (
                "missing package closure",
                HelperRole::Speaker,
                |draft| draft.package_closure = None,
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::MissingRequired {
                        role: "package closure",
                    },
                ),
            ),
            (
                "missing notices",
                HelperRole::Speaker,
                |draft| draft.notices = None,
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::MissingRequired { role: "notices" },
                ),
            ),
            (
                "missing api policy",
                HelperRole::Speaker,
                |draft| draft.api_policy = None,
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::MissingRequired { role: "api policy" },
                ),
            ),
            (
                "ordinary onnxruntime.dll import",
                HelperRole::Speaker,
                |draft| {
                    draft.import_census = Some(census_with_imports(&[ImportSpec {
                        name: "onnxruntime.dll",
                        symbols: &[],
                    }]));
                },
                census_wrap(
                    HelperRole::Speaker,
                    unexpected("ordinary import", "absent", "onnxruntime.dll"),
                ),
            ),
            (
                "ordinary ONNXRUNTIME.DLL import",
                HelperRole::Vad,
                |draft| {
                    draft.import_census = Some(census_with_imports(&[ImportSpec {
                        name: "ONNXRUNTIME.DLL",
                        symbols: &[],
                    }]));
                },
                census_wrap(
                    HelperRole::Vad,
                    unexpected("ordinary import", "absent", "onnxruntime.dll"),
                ),
            ),
            (
                "package-closure extra member",
                HelperRole::Speaker,
                |draft| {
                    if let Some(closure) = &mut draft.package_closure {
                        closure.push("onnxruntime.lib".into());
                    }
                },
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::SetMismatch {
                        role: "package closure",
                        missing: Vec::new(),
                        unexpected: vec!["onnxruntime.lib".into()],
                    },
                ),
            ),
            (
                "package-closure missing dll",
                HelperRole::Speaker,
                |draft| draft.package_closure = Some(Vec::new()),
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::SetMismatch {
                        role: "package closure",
                        missing: vec!["onnxruntime.dll".into()],
                        unexpected: Vec::new(),
                    },
                ),
            ),
            (
                "missing notice name",
                HelperRole::Speaker,
                |draft| {
                    if let Some(notices) = &mut draft.notices {
                        notices.remove("onnxruntime-LICENSE.txt");
                    }
                },
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::SetMismatch {
                        role: "notices",
                        missing: vec!["onnxruntime-LICENSE.txt".into()],
                        unexpected: Vec::new(),
                    },
                ),
            ),
            (
                "extra notice name",
                HelperRole::Speaker,
                |draft| {
                    if let Some(notices) = &mut draft.notices {
                        notices.insert("extra.txt".into(), b"extra".to_vec());
                    }
                },
                census_wrap(
                    HelperRole::Speaker,
                    OnnxWindowsError::SetMismatch {
                        role: "notices",
                        missing: Vec::new(),
                        unexpected: vec!["extra.txt".into()],
                    },
                ),
            ),
            (
                "empty notice bytes",
                HelperRole::Speaker,
                |draft| {
                    if let Some(notices) = &mut draft.notices {
                        notices.insert("onnxruntime-LICENSE.txt".into(), Vec::new());
                    }
                },
                census_wrap(
                    HelperRole::Speaker,
                    unexpected("notice bytes", "non-empty", "onnxruntime-LICENSE.txt"),
                ),
            ),
            (
                "wrong api policy",
                HelperRole::Speaker,
                |draft| draft.api_policy = Some(24),
                census_wrap(HelperRole::Speaker, unexpected("api policy", "25", "24")),
            ),
            (
                "helper-role mismatch",
                HelperRole::Speaker,
                |draft| draft.role = Some(HelperRole::Vad),
                census_wrap(
                    HelperRole::Speaker,
                    unexpected("helper role", "speaker", "vad"),
                ),
            ),
        ];

        for (name, expected_role, mutate, expected) in cases {
            let mut draft = canonical_census(*expected_role);
            mutate(&mut draft);
            let error = admit_helper_census(*expected_role, &draft).expect_err("must reject");
            match &error {
                OnnxWindowsError::Census { helper, .. } => {
                    assert_eq!(helper, expected_role, "{name}");
                }
                other => panic!("{name}: expected Census wrapper, got {other:?}"),
            }
            assert_eq!(&error, expected, "{name}");
        }
    }
}
