// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows Parakeet (`parakeet.cpp`) controlled-provider admission facts.
//!
//! Pins, validators, and a fail-closed admission draft. No build, fetch,
//! spawn, or probe is performed here. This is a sibling of `ced_windows`,
//! not an extension of `ControlledBuildReceiptDraft`: the surface includes
//! package placement, notices, SBOM, and a static server configuration that
//! a DLL producer does not have.
//!
//! `None` on a server bound means unbounded; `Some(0)` / `Duration::ZERO`
//! means a zero bound. Those are distinct rejections. Category absence is
//! `Missing`. Package-relative paths are intended layout, not yet verified
//! against a produced tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use crate::controlled_build::DependencySource;
use crate::digest::sha256_hex;
use crate::import_policy::disallowed_imports;
use crate::pe::PeInfo;
use crate::provenance::{self, Provenance};

pub const PARAKEET_CPP_VERSION: &str = "v0.5.0";
pub const PARAKEET_CPP_COMMIT: &str = "1bfbebfaaf493866f49597cd3b7901959d395c60";
/// Currently the same revision as `ced_windows::GGML_COMMIT` because both
/// producers pin the same upstream ggml; the pins are independent and may
/// diverge.
pub const GGML_COMMIT: &str = "e705c5fed490514458bdd2eaddc43bd098fcce9b";

pub const PARAKEET_WINDOWS_TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
pub const PARAKEET_WINDOWS_BUILD_PROFILE: &str = "Release";
// Intended CPU-only/static-ggml CMake inputs for a future controlled build;
// not yet verified against upstream CMakeLists.txt (no build has run).
// Admission checks `ParakeetBuildConfiguration`, not this list.
pub const PARAKEET_WINDOWS_BUILD_FLAGS: &[&str] = &[
    "-DBUILD_SHARED_LIBS=OFF",
    "-DGGML_NATIVE=OFF",
    "-DGGML_CUDA=OFF",
    "-DGGML_VULKAN=OFF",
    "-DGGML_OPENCL=OFF",
    "-DGGML_HIP=OFF",
    "-DGGML_METAL=OFF",
];

pub const PARAKEET_ATT_CONTEXT_ENV: &str = "PARAKEET_ATT_CONTEXT";
pub const PARAKEET_ATT_CONTEXT: u32 = 128;

/// Catalog identity for the GGUF model. Unlike `ced_windows::CED_MODEL_*`,
/// sha256 is declared locally: this admission surface must compare a digest,
/// and `solstone-core-distribution` has no `solstone-core-assets` /
/// `solstone-core-local` dependency to defer to. Values are cross-checked
/// against `solstone-core-assets::ARTIFACTS`'s `parakeet-model` row and
/// `solstone-core-local::install::pins::PARAKEET_MODEL`.
pub const PARAKEET_MODEL_ORIGIN: &str = "mudler/parakeet-cpp-gguf";
pub const PARAKEET_MODEL_REVISION: &str = "bf0af9f425fa01809cadec671b3cb672709d13e9";
pub const PARAKEET_MODEL_FILENAME: &str = "tdt-0.6b-v3-q8_0.gguf";
pub const PARAKEET_MODEL_SIZE_BYTES: u64 = 940_663_680;
pub const PARAKEET_MODEL_SHA256: &str =
    "4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757";

pub const PARAKEET_SERVER_OUTPUT_LABEL: &str = "bin/parakeet-server.exe";
pub const PARAKEET_MODEL_OUTPUT_LABEL: &str = "models/tdt-0.6b-v3-q8_0.gguf";

pub const PARAKEET_WINDOWS_IMPORT_ALLOWLIST: &[&str] = &[
    "KERNEL32.dll",
    "ADVAPI32.dll",
    "WS2_32.dll",
    "VCRUNTIME140.dll",
    "VCRUNTIME140_1.dll",
    "ucrtbase.dll",
    "ntdll.dll",
    "bcrypt.dll",
];

pub const PARAKEET_WINDOWS_NOTICE_NAMES: &[&str] =
    &["parakeet.cpp-LICENSE.txt", "ggml-LICENSE.txt"];

pub const PARAKEET_WINDOWS_SBOM_COMPONENTS: &[&str] = &["parakeet.cpp", "ggml", "parakeet-model"];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GgmlLinkage {
    Static,
    Shared,
}

impl GgmlLinkage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Static => "Static",
            Self::Shared => "Shared",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionContext {
    Forced(u32),
    Inherited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParakeetModelSource {
    Packaged,
    Ambient,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetSourceIdentity {
    pub version: String,
    pub product: Provenance,
    pub ggml: DependencySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetBuildConfiguration {
    pub target_triple: String,
    pub profile: String,
    pub cmake: bool,
    pub msvc: bool,
    pub runtime_library: RuntimeLibrary,
    pub ggml_linkage: GgmlLinkage,
    pub ggml_native: bool,
    pub cpu_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetModelIdentity {
    pub origin: String,
    pub revision: String,
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub source: ParakeetModelSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedModelFacts {
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetPackageLayout {
    pub server_relative_path: String,
    pub model_relative_path: String,
    pub packaged_model: Option<PackagedModelFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbomComponent {
    pub name: String,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetServerConfiguration {
    pub host: Option<String>,
    pub auth_token: Option<String>,
    pub auth_nonce: Option<String>,
    pub authenticated_health: Option<bool>,
    pub request_size: Option<u64>,
    pub audio_duration: Option<Duration>,
    pub concurrency: Option<u32>,
    pub queue_depth: Option<u32>,
    pub output_size: Option<u64>,
    pub deadline: Option<Duration>,
    pub stdout: Option<u64>,
    pub stderr: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParakeetServerSource {
    Controlled(ParakeetServerConfiguration),
    Ambient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParakeetAdmissionCategory {
    SourceIdentity,
    BuildConfiguration,
    AttentionContext,
    ModelIdentity,
    PackageLayout,
    ImportClosure,
    Notices,
    Provenance,
    Sbom,
    ServerConfiguration,
}

impl ParakeetAdmissionCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::SourceIdentity => "source identity",
            Self::BuildConfiguration => "build configuration",
            Self::AttentionContext => "attention context",
            Self::ModelIdentity => "model identity",
            Self::PackageLayout => "package layout",
            Self::ImportClosure => "import closure",
            Self::Notices => "notices",
            Self::Provenance => "provenance",
            Self::Sbom => "sbom",
            Self::ServerConfiguration => "server configuration",
        }
    }
}

impl fmt::Display for ParakeetAdmissionCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParakeetWindowsAdmissionDraft {
    pub source: Option<ParakeetSourceIdentity>,
    pub build: Option<ParakeetBuildConfiguration>,
    pub attention_context: Option<AttentionContext>,
    pub model: Option<ParakeetModelIdentity>,
    pub layout: Option<ParakeetPackageLayout>,
    pub import_census: Option<PeInfo>,
    pub notices: Option<BTreeMap<String, Vec<u8>>>,
    pub provenance: Option<Provenance>,
    pub sbom: Option<Vec<SbomComponent>>,
    pub server: Option<ParakeetServerSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetWindowsAdmission {
    pub source: ParakeetSourceIdentity,
    pub build: ParakeetBuildConfiguration,
    pub attention_context: AttentionContext,
    pub model: ParakeetModelIdentity,
    pub layout: ParakeetPackageLayout,
    pub import_census: PeInfo,
    pub notices: BTreeMap<String, Vec<u8>>,
    pub provenance: Provenance,
    pub sbom: Vec<SbomComponent>,
    pub server: ParakeetServerConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParakeetWindowsError {
    Missing {
        category: ParakeetAdmissionCategory,
    },
    Ambient {
        category: ParakeetAdmissionCategory,
    },
    MissingRequired {
        role: &'static str,
    },
    Unbounded {
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
}

impl fmt::Display for ParakeetWindowsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { category } => {
                write!(formatter, "missing required:\n  {category}")
            }
            Self::Ambient { category } => {
                write!(formatter, "unexpected:\n  ambient {category}")
            }
            Self::MissingRequired { role } => {
                write!(formatter, "missing required:\n  {role}")
            }
            Self::Unbounded { role } => {
                write!(formatter, "unexpected:\n  unbounded {role}")
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
        }
    }
}

impl std::error::Error for ParakeetWindowsError {}

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
) -> ParakeetWindowsError {
    ParakeetWindowsError::Unexpected {
        role,
        expected: expected.into(),
        observed: observed.into(),
    }
}

fn missing(category: ParakeetAdmissionCategory) -> ParakeetWindowsError {
    ParakeetWindowsError::Missing { category }
}

fn require_nonempty_string(
    value: &str,
    role: &'static str,
    expected: &str,
) -> Result<(), ParakeetWindowsError> {
    if value.is_empty() {
        return Err(ParakeetWindowsError::MissingRequired { role });
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
) -> Result<(), ParakeetWindowsError> {
    if actual.is_empty() {
        return Err(ParakeetWindowsError::MissingRequired { role });
    }
    provenance::require_commit(expected, actual).map_err(|_| unexpected(role, expected, actual))
}

#[must_use]
pub fn parakeet_windows_build_configuration() -> ParakeetBuildConfiguration {
    ParakeetBuildConfiguration {
        target_triple: PARAKEET_WINDOWS_TARGET_TRIPLE.to_owned(),
        profile: PARAKEET_WINDOWS_BUILD_PROFILE.to_owned(),
        cmake: true,
        msvc: true,
        runtime_library: RuntimeLibrary::Md,
        ggml_linkage: GgmlLinkage::Static,
        ggml_native: false,
        cpu_only: true,
    }
}

pub fn verify_source_identity(
    identity: &ParakeetSourceIdentity,
) -> Result<(), ParakeetWindowsError> {
    require_nonempty_string(
        &identity.version,
        "parakeet.cpp version",
        PARAKEET_CPP_VERSION,
    )?;
    require_pinned_commit(
        PARAKEET_CPP_COMMIT,
        &identity.product.commit,
        "parakeet.cpp commit",
    )?;
    require_pinned_commit(GGML_COMMIT, &identity.ggml.revision, "ggml commit")?;
    Ok(())
}

pub fn verify_build_configuration(
    configuration: &ParakeetBuildConfiguration,
) -> Result<(), ParakeetWindowsError> {
    let pinned = parakeet_windows_build_configuration();
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
    if configuration.cmake != pinned.cmake {
        return Err(unexpected(
            "cmake",
            pinned.cmake.to_string(),
            configuration.cmake.to_string(),
        ));
    }
    if configuration.msvc != pinned.msvc {
        return Err(unexpected(
            "msvc",
            pinned.msvc.to_string(),
            configuration.msvc.to_string(),
        ));
    }
    if configuration.runtime_library != pinned.runtime_library {
        return Err(unexpected(
            "runtime library",
            pinned.runtime_library.as_str(),
            configuration.runtime_library.as_str(),
        ));
    }
    if configuration.ggml_linkage != pinned.ggml_linkage {
        return Err(unexpected(
            "ggml linkage",
            pinned.ggml_linkage.as_str(),
            configuration.ggml_linkage.as_str(),
        ));
    }
    if configuration.ggml_native != pinned.ggml_native {
        return Err(unexpected(
            "ggml native",
            pinned.ggml_native.to_string(),
            configuration.ggml_native.to_string(),
        ));
    }
    if configuration.cpu_only != pinned.cpu_only {
        return Err(unexpected(
            "cpu only",
            pinned.cpu_only.to_string(),
            configuration.cpu_only.to_string(),
        ));
    }
    Ok(())
}

pub fn verify_attention_context(context: &AttentionContext) -> Result<(), ParakeetWindowsError> {
    match *context {
        AttentionContext::Forced(value) if value == PARAKEET_ATT_CONTEXT => Ok(()),
        AttentionContext::Forced(value) => Err(unexpected(
            "PARAKEET_ATT_CONTEXT",
            PARAKEET_ATT_CONTEXT.to_string(),
            value.to_string(),
        )),
        AttentionContext::Inherited => Err(unexpected(
            "PARAKEET_ATT_CONTEXT",
            PARAKEET_ATT_CONTEXT.to_string(),
            "inherited",
        )),
    }
}

pub fn verify_model_identity(identity: &ParakeetModelIdentity) -> Result<(), ParakeetWindowsError> {
    if identity.source == ParakeetModelSource::Ambient {
        return Err(ParakeetWindowsError::Ambient {
            category: ParakeetAdmissionCategory::ModelIdentity,
        });
    }
    require_nonempty_string(&identity.origin, "model origin", PARAKEET_MODEL_ORIGIN)?;
    require_nonempty_string(
        &identity.revision,
        "model revision",
        PARAKEET_MODEL_REVISION,
    )?;
    require_nonempty_string(
        &identity.filename,
        "model filename",
        PARAKEET_MODEL_FILENAME,
    )?;
    if identity.size_bytes != PARAKEET_MODEL_SIZE_BYTES {
        return Err(unexpected(
            "model size",
            PARAKEET_MODEL_SIZE_BYTES.to_string(),
            identity.size_bytes.to_string(),
        ));
    }
    require_nonempty_string(&identity.sha256, "model digest", PARAKEET_MODEL_SHA256)?;
    Ok(())
}

pub fn verify_packaged_model(facts: &PackagedModelFacts) -> Result<(), ParakeetWindowsError> {
    if facts.size_bytes != PARAKEET_MODEL_SIZE_BYTES {
        return Err(unexpected(
            "packaged model size",
            PARAKEET_MODEL_SIZE_BYTES.to_string(),
            facts.size_bytes.to_string(),
        ));
    }
    if facts.sha256 != PARAKEET_MODEL_SHA256 {
        return Err(unexpected(
            "packaged model digest",
            PARAKEET_MODEL_SHA256,
            &facts.sha256,
        ));
    }
    Ok(())
}

pub fn verify_model_bytes(bytes: &[u8]) -> Result<(), ParakeetWindowsError> {
    let size = bytes.len() as u64;
    if size != PARAKEET_MODEL_SIZE_BYTES {
        return Err(unexpected(
            "model size",
            PARAKEET_MODEL_SIZE_BYTES.to_string(),
            size.to_string(),
        ));
    }
    let digest = sha256_hex(bytes);
    if digest != PARAKEET_MODEL_SHA256 {
        return Err(unexpected("model digest", PARAKEET_MODEL_SHA256, digest));
    }
    Ok(())
}

pub fn verify_package_layout(layout: &ParakeetPackageLayout) -> Result<(), ParakeetWindowsError> {
    if layout.server_relative_path != PARAKEET_SERVER_OUTPUT_LABEL {
        return Err(unexpected(
            "server path",
            PARAKEET_SERVER_OUTPUT_LABEL,
            &layout.server_relative_path,
        ));
    }
    if layout.model_relative_path != PARAKEET_MODEL_OUTPUT_LABEL {
        return Err(unexpected(
            "model path",
            PARAKEET_MODEL_OUTPUT_LABEL,
            &layout.model_relative_path,
        ));
    }
    let facts = layout
        .packaged_model
        .as_ref()
        .ok_or(ParakeetWindowsError::MissingRequired {
            role: "packaged model",
        })?;
    verify_packaged_model(facts)
}

pub fn verify_import_closure(census: &PeInfo) -> Result<(), ParakeetWindowsError> {
    let unexpected_imports: Vec<String> =
        disallowed_imports(&census.imports, PARAKEET_WINDOWS_IMPORT_ALLOWLIST)
            .into_iter()
            .map(str::to_owned)
            .collect();
    if unexpected_imports.is_empty() {
        Ok(())
    } else {
        Err(ParakeetWindowsError::SetMismatch {
            role: "import closure",
            missing: Vec::new(),
            unexpected: unexpected_imports,
        })
    }
}

pub fn verify_notices(notices: &BTreeMap<String, Vec<u8>>) -> Result<(), ParakeetWindowsError> {
    let expected: BTreeSet<&str> = PARAKEET_WINDOWS_NOTICE_NAMES.iter().copied().collect();
    let actual: BTreeSet<&str> = notices.keys().map(String::as_str).collect();
    if actual != expected {
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
        return Err(ParakeetWindowsError::SetMismatch {
            role: "notices",
            missing,
            unexpected: unexpected_names,
        });
    }
    for name in PARAKEET_WINDOWS_NOTICE_NAMES {
        // Presence is guaranteed by the set-equality check above.
        let bytes = notices
            .get(*name)
            .expect("notice name verified present above");
        if bytes.is_empty() {
            return Err(unexpected("notice bytes", "non-empty", *name));
        }
    }
    Ok(())
}

pub fn verify_provenance(provenance: &Provenance) -> Result<(), ParakeetWindowsError> {
    require_pinned_commit(PARAKEET_CPP_COMMIT, &provenance.commit, "provenance commit")?;
    if provenance.lock_sha256.is_empty() {
        return Err(ParakeetWindowsError::MissingRequired {
            role: "lock sha256",
        });
    }
    Ok(())
}

pub fn verify_sbom(components: &[SbomComponent]) -> Result<(), ParakeetWindowsError> {
    let expected: BTreeSet<&str> = PARAKEET_WINDOWS_SBOM_COMPONENTS.iter().copied().collect();
    let actual: BTreeSet<&str> = components
        .iter()
        .map(|component| component.name.as_str())
        .collect();
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
    Err(ParakeetWindowsError::SetMismatch {
        role: "sbom",
        missing,
        unexpected: unexpected_names,
    })
}

fn require_nonempty_option(
    value: &Option<String>,
    role: &'static str,
) -> Result<(), ParakeetWindowsError> {
    match value {
        None => Err(ParakeetWindowsError::MissingRequired { role }),
        Some(text) if text.is_empty() => Err(unexpected(role, "non-empty", "")),
        Some(_) => Ok(()),
    }
}

fn require_positive_u64(
    value: Option<u64>,
    role: &'static str,
) -> Result<(), ParakeetWindowsError> {
    match value {
        None => Err(ParakeetWindowsError::Unbounded { role }),
        Some(0) => Err(unexpected(role, "nonzero bound", "0")),
        Some(_) => Ok(()),
    }
}

fn require_positive_u32(
    value: Option<u32>,
    role: &'static str,
) -> Result<(), ParakeetWindowsError> {
    match value {
        None => Err(ParakeetWindowsError::Unbounded { role }),
        Some(0) => Err(unexpected(role, "nonzero bound", "0")),
        Some(_) => Ok(()),
    }
}

fn require_positive_duration(
    value: Option<Duration>,
    role: &'static str,
) -> Result<(), ParakeetWindowsError> {
    match value {
        None => Err(ParakeetWindowsError::Unbounded { role }),
        Some(duration) if duration.is_zero() => Err(unexpected(role, "nonzero bound", "0")),
        Some(_) => Ok(()),
    }
}

pub fn verify_server_configuration(
    configuration: &ParakeetServerConfiguration,
) -> Result<(), ParakeetWindowsError> {
    match configuration.host.as_deref() {
        Some("127.0.0.1") | Some("::1") => {}
        None => {
            return Err(ParakeetWindowsError::MissingRequired { role: "host" });
        }
        Some(observed) => {
            return Err(unexpected("host", "127.0.0.1 or ::1", observed));
        }
    }
    require_nonempty_option(&configuration.auth_token, "auth token")?;
    require_nonempty_option(&configuration.auth_nonce, "auth nonce")?;
    match configuration.authenticated_health {
        Some(true) => {}
        None => {
            return Err(ParakeetWindowsError::MissingRequired {
                role: "authenticated health",
            });
        }
        Some(false) => {
            return Err(unexpected("authenticated health", "true", "false"));
        }
    }
    require_positive_u64(configuration.request_size, "request size")?;
    require_positive_duration(configuration.audio_duration, "audio duration")?;
    require_positive_u32(configuration.concurrency, "concurrency")?;
    require_positive_u32(configuration.queue_depth, "queue depth")?;
    require_positive_u64(configuration.output_size, "output size")?;
    require_positive_duration(configuration.deadline, "deadline")?;
    require_positive_u64(configuration.stdout, "stdout")?;
    require_positive_u64(configuration.stderr, "stderr")?;
    Ok(())
}

pub fn admit(
    draft: ParakeetWindowsAdmissionDraft,
) -> Result<ParakeetWindowsAdmission, ParakeetWindowsError> {
    let source = draft
        .source
        .ok_or_else(|| missing(ParakeetAdmissionCategory::SourceIdentity))?;
    verify_source_identity(&source)?;
    let build = draft
        .build
        .ok_or_else(|| missing(ParakeetAdmissionCategory::BuildConfiguration))?;
    verify_build_configuration(&build)?;
    let attention_context = draft
        .attention_context
        .ok_or_else(|| missing(ParakeetAdmissionCategory::AttentionContext))?;
    verify_attention_context(&attention_context)?;
    let model = draft
        .model
        .ok_or_else(|| missing(ParakeetAdmissionCategory::ModelIdentity))?;
    verify_model_identity(&model)?;
    let layout = draft
        .layout
        .ok_or_else(|| missing(ParakeetAdmissionCategory::PackageLayout))?;
    verify_package_layout(&layout)?;
    let import_census = draft
        .import_census
        .ok_or_else(|| missing(ParakeetAdmissionCategory::ImportClosure))?;
    verify_import_closure(&import_census)?;
    let notices = draft
        .notices
        .ok_or_else(|| missing(ParakeetAdmissionCategory::Notices))?;
    verify_notices(&notices)?;
    let provenance = draft
        .provenance
        .ok_or_else(|| missing(ParakeetAdmissionCategory::Provenance))?;
    verify_provenance(&provenance)?;
    let sbom = draft
        .sbom
        .ok_or_else(|| missing(ParakeetAdmissionCategory::Sbom))?;
    verify_sbom(&sbom)?;
    let server = match draft
        .server
        .ok_or_else(|| missing(ParakeetAdmissionCategory::ServerConfiguration))?
    {
        ParakeetServerSource::Ambient => {
            return Err(ParakeetWindowsError::Ambient {
                category: ParakeetAdmissionCategory::ServerConfiguration,
            });
        }
        ParakeetServerSource::Controlled(configuration) => {
            verify_server_configuration(&configuration)?;
            configuration
        }
    };
    Ok(ParakeetWindowsAdmission {
        source,
        build,
        attention_context,
        model,
        layout,
        import_census,
        notices,
        provenance,
        sbom,
        server,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::{FixtureSpec, ImportSpec, fixture, parse_pe};

    fn pinned_source() -> ParakeetSourceIdentity {
        ParakeetSourceIdentity {
            version: PARAKEET_CPP_VERSION.to_owned(),
            product: Provenance {
                commit: PARAKEET_CPP_COMMIT.to_owned(),
                lock_sha256: String::new(),
            },
            ggml: DependencySource {
                repository: "ggml".into(),
                revision: GGML_COMMIT.to_owned(),
                content_sha256: String::new(),
            },
        }
    }

    fn pinned_model() -> ParakeetModelIdentity {
        ParakeetModelIdentity {
            origin: PARAKEET_MODEL_ORIGIN.to_owned(),
            revision: PARAKEET_MODEL_REVISION.to_owned(),
            filename: PARAKEET_MODEL_FILENAME.to_owned(),
            size_bytes: PARAKEET_MODEL_SIZE_BYTES,
            sha256: PARAKEET_MODEL_SHA256.to_owned(),
            source: ParakeetModelSource::Packaged,
        }
    }

    fn pinned_packaged_model() -> PackagedModelFacts {
        PackagedModelFacts {
            size_bytes: PARAKEET_MODEL_SIZE_BYTES,
            sha256: PARAKEET_MODEL_SHA256.to_owned(),
        }
    }

    fn pinned_layout() -> ParakeetPackageLayout {
        ParakeetPackageLayout {
            server_relative_path: PARAKEET_SERVER_OUTPUT_LABEL.to_owned(),
            model_relative_path: PARAKEET_MODEL_OUTPUT_LABEL.to_owned(),
            packaged_model: Some(pinned_packaged_model()),
        }
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
            "parakeet.cpp-LICENSE.txt".into(),
            b"parakeet-license".to_vec(),
        );
        notices.insert("ggml-LICENSE.txt".into(), b"ggml-license".to_vec());
        notices
    }

    fn pinned_provenance() -> Provenance {
        Provenance {
            commit: PARAKEET_CPP_COMMIT.to_owned(),
            lock_sha256: "lock-digest".into(),
        }
    }

    fn pinned_sbom() -> Vec<SbomComponent> {
        vec![
            SbomComponent {
                name: "parakeet.cpp".into(),
                revision: PARAKEET_CPP_VERSION.to_owned(),
            },
            SbomComponent {
                name: "ggml".into(),
                revision: GGML_COMMIT.to_owned(),
            },
            SbomComponent {
                name: "parakeet-model".into(),
                revision: PARAKEET_MODEL_REVISION.to_owned(),
            },
        ]
    }

    fn canonical_server() -> ParakeetServerConfiguration {
        ParakeetServerConfiguration {
            host: Some("127.0.0.1".into()),
            auth_token: Some("token".into()),
            auth_nonce: Some("nonce".into()),
            authenticated_health: Some(true),
            request_size: Some(1_048_576),
            audio_duration: Some(Duration::from_secs(60)),
            concurrency: Some(1),
            queue_depth: Some(1),
            output_size: Some(1_048_576),
            deadline: Some(Duration::from_secs(300)),
            stdout: Some(65_536),
            stderr: Some(65_536),
        }
    }

    fn canonical_draft() -> ParakeetWindowsAdmissionDraft {
        ParakeetWindowsAdmissionDraft {
            source: Some(pinned_source()),
            build: Some(parakeet_windows_build_configuration()),
            attention_context: Some(AttentionContext::Forced(PARAKEET_ATT_CONTEXT)),
            model: Some(pinned_model()),
            layout: Some(pinned_layout()),
            import_census: Some(census_with_imports(&[ImportSpec {
                name: "KERNEL32.dll",
                symbols: &[],
            }])),
            notices: Some(pinned_notices()),
            provenance: Some(pinned_provenance()),
            sbom: Some(pinned_sbom()),
            server: Some(ParakeetServerSource::Controlled(canonical_server())),
        }
    }

    type AdmissionCase = (
        &'static str,
        fn(&mut ParakeetWindowsAdmissionDraft),
        ParakeetWindowsError,
    );

    fn admit_err(mutate: impl FnOnce(&mut ParakeetWindowsAdmissionDraft)) -> ParakeetWindowsError {
        let mut draft = canonical_draft();
        mutate(&mut draft);
        admit(draft).expect_err("must reject")
    }

    fn server_config(
        draft: &mut ParakeetWindowsAdmissionDraft,
    ) -> &mut ParakeetServerConfiguration {
        match draft.server.as_mut() {
            Some(ParakeetServerSource::Controlled(configuration)) => configuration,
            other => panic!("canonical server is Controlled, got {other:?}"),
        }
    }

    #[test]
    fn parakeet_admission_accepts_canonical_controlled_inputs() {
        let admitted = admit(canonical_draft()).expect("canonical inputs");
        assert_eq!(admitted.build, parakeet_windows_build_configuration());
        assert_eq!(
            admitted.attention_context,
            AttentionContext::Forced(PARAKEET_ATT_CONTEXT)
        );
        assert_eq!(admitted.model.source, ParakeetModelSource::Packaged);
        assert_eq!(admitted.server.host.as_deref(), Some("127.0.0.1"));

        let mut loopback_v6 = canonical_server();
        loopback_v6.host = Some("::1".into());
        verify_server_configuration(&loopback_v6).expect("::1 is loopback");
    }

    #[test]
    fn parakeet_admission_rejects_identity_and_package_variants() {
        let cases: &[AdmissionCase] = &[
            (
                "missing source",
                |draft| draft.source = None,
                ParakeetWindowsError::Missing {
                    category: ParakeetAdmissionCategory::SourceIdentity,
                },
            ),
            (
                "empty producer commit",
                |draft| {
                    if let Some(source) = &mut draft.source {
                        source.product.commit.clear();
                    }
                },
                ParakeetWindowsError::MissingRequired {
                    role: "parakeet.cpp commit",
                },
            ),
            (
                "wrong producer commit",
                |draft| {
                    if let Some(source) = &mut draft.source {
                        source.product.commit = "wrong-commit".into();
                    }
                },
                unexpected("parakeet.cpp commit", PARAKEET_CPP_COMMIT, "wrong-commit"),
            ),
            (
                "wrong ggml commit",
                |draft| {
                    if let Some(source) = &mut draft.source {
                        source.ggml.revision = "wrong-ggml".into();
                    }
                },
                unexpected("ggml commit", GGML_COMMIT, "wrong-ggml"),
            ),
            (
                "wrong version",
                |draft| {
                    if let Some(source) = &mut draft.source {
                        source.version = "v0.0.0".into();
                    }
                },
                unexpected("parakeet.cpp version", PARAKEET_CPP_VERSION, "v0.0.0"),
            ),
            (
                "wrong model origin",
                |draft| {
                    if let Some(model) = &mut draft.model {
                        model.origin = "other/repo".into();
                    }
                },
                unexpected("model origin", PARAKEET_MODEL_ORIGIN, "other/repo"),
            ),
            (
                "wrong model revision",
                |draft| {
                    if let Some(model) = &mut draft.model {
                        model.revision = "wrong-revision".into();
                    }
                },
                unexpected("model revision", PARAKEET_MODEL_REVISION, "wrong-revision"),
            ),
            (
                "wrong model filename",
                |draft| {
                    if let Some(model) = &mut draft.model {
                        model.filename = "other.gguf".into();
                    }
                },
                unexpected("model filename", PARAKEET_MODEL_FILENAME, "other.gguf"),
            ),
            (
                "wrong model size",
                |draft| {
                    if let Some(model) = &mut draft.model {
                        model.size_bytes = 1;
                    }
                },
                unexpected("model size", PARAKEET_MODEL_SIZE_BYTES.to_string(), "1"),
            ),
            (
                "wrong model digest",
                |draft| {
                    if let Some(model) = &mut draft.model {
                        model.sha256 = "0".repeat(64);
                    }
                },
                unexpected("model digest", PARAKEET_MODEL_SHA256, "0".repeat(64)),
            ),
            (
                "ambient model source",
                |draft| {
                    if let Some(model) = &mut draft.model {
                        model.source = ParakeetModelSource::Ambient;
                    }
                },
                ParakeetWindowsError::Ambient {
                    category: ParakeetAdmissionCategory::ModelIdentity,
                },
            ),
            (
                "missing package layout",
                |draft| draft.layout = None,
                ParakeetWindowsError::Missing {
                    category: ParakeetAdmissionCategory::PackageLayout,
                },
            ),
            (
                "wrong server path",
                |draft| {
                    if let Some(layout) = &mut draft.layout {
                        layout.server_relative_path = "bin/other.exe".into();
                    }
                },
                unexpected("server path", PARAKEET_SERVER_OUTPUT_LABEL, "bin/other.exe"),
            ),
            (
                "wrong model path",
                |draft| {
                    if let Some(layout) = &mut draft.layout {
                        layout.model_relative_path = "models/other.gguf".into();
                    }
                },
                unexpected(
                    "model path",
                    PARAKEET_MODEL_OUTPUT_LABEL,
                    "models/other.gguf",
                ),
            ),
            (
                "absent packaged model",
                |draft| {
                    if let Some(layout) = &mut draft.layout {
                        layout.packaged_model = None;
                    }
                },
                ParakeetWindowsError::MissingRequired {
                    role: "packaged model",
                },
            ),
            (
                "wrong packaged digest",
                |draft| {
                    if let Some(layout) = &mut draft.layout
                        && let Some(facts) = &mut layout.packaged_model
                    {
                        facts.sha256 = "0".repeat(64);
                    }
                },
                unexpected(
                    "packaged model digest",
                    PARAKEET_MODEL_SHA256,
                    "0".repeat(64),
                ),
            ),
            (
                "wrong packaged size",
                |draft| {
                    if let Some(layout) = &mut draft.layout
                        && let Some(facts) = &mut layout.packaged_model
                    {
                        facts.size_bytes = 1;
                    }
                },
                unexpected(
                    "packaged model size",
                    PARAKEET_MODEL_SIZE_BYTES.to_string(),
                    "1",
                ),
            ),
            (
                "missing attention context",
                |draft| draft.attention_context = None,
                ParakeetWindowsError::Missing {
                    category: ParakeetAdmissionCategory::AttentionContext,
                },
            ),
            (
                "inherited attention context",
                |draft| draft.attention_context = Some(AttentionContext::Inherited),
                unexpected("PARAKEET_ATT_CONTEXT", "128", "inherited"),
            ),
            (
                "non-128 attention context",
                |draft| draft.attention_context = Some(AttentionContext::Forced(256)),
                unexpected("PARAKEET_ATT_CONTEXT", "128", "256"),
            ),
            (
                "unadmitted import",
                |draft| {
                    draft.import_census = Some(census_with_imports(&[ImportSpec {
                        name: "nvcuda.dll",
                        symbols: &[],
                    }]));
                },
                ParakeetWindowsError::SetMismatch {
                    role: "import closure",
                    missing: Vec::new(),
                    unexpected: vec!["nvcuda.dll".into()],
                },
            ),
            (
                "missing notices",
                |draft| draft.notices = None,
                ParakeetWindowsError::Missing {
                    category: ParakeetAdmissionCategory::Notices,
                },
            ),
            (
                "missing notice name",
                |draft| {
                    if let Some(notices) = &mut draft.notices {
                        notices.remove("ggml-LICENSE.txt");
                    }
                },
                ParakeetWindowsError::SetMismatch {
                    role: "notices",
                    missing: vec!["ggml-LICENSE.txt".into()],
                    unexpected: Vec::new(),
                },
            ),
            (
                "extra notice name",
                |draft| {
                    if let Some(notices) = &mut draft.notices {
                        notices.insert("extra.txt".into(), b"extra".to_vec());
                    }
                },
                ParakeetWindowsError::SetMismatch {
                    role: "notices",
                    missing: Vec::new(),
                    unexpected: vec!["extra.txt".into()],
                },
            ),
            (
                "empty notice bytes",
                |draft| {
                    if let Some(notices) = &mut draft.notices {
                        notices.insert("parakeet.cpp-LICENSE.txt".into(), Vec::new());
                    }
                },
                unexpected("notice bytes", "non-empty", "parakeet.cpp-LICENSE.txt"),
            ),
            (
                "missing provenance",
                |draft| draft.provenance = None,
                ParakeetWindowsError::Missing {
                    category: ParakeetAdmissionCategory::Provenance,
                },
            ),
            (
                "wrong provenance commit",
                |draft| {
                    if let Some(provenance) = &mut draft.provenance {
                        provenance.commit = "wrong-commit".into();
                    }
                },
                unexpected("provenance commit", PARAKEET_CPP_COMMIT, "wrong-commit"),
            ),
            (
                "empty lock",
                |draft| {
                    if let Some(provenance) = &mut draft.provenance {
                        provenance.lock_sha256.clear();
                    }
                },
                ParakeetWindowsError::MissingRequired {
                    role: "lock sha256",
                },
            ),
            (
                "missing sbom",
                |draft| draft.sbom = None,
                ParakeetWindowsError::Missing {
                    category: ParakeetAdmissionCategory::Sbom,
                },
            ),
            (
                "missing sbom component",
                |draft| {
                    if let Some(sbom) = &mut draft.sbom {
                        sbom.retain(|component| component.name != "ggml");
                    }
                },
                ParakeetWindowsError::SetMismatch {
                    role: "sbom",
                    missing: vec!["ggml".into()],
                    unexpected: Vec::new(),
                },
            ),
            (
                "extra sbom component",
                |draft| {
                    if let Some(sbom) = &mut draft.sbom {
                        sbom.push(SbomComponent {
                            name: "extra".into(),
                            revision: "1".into(),
                        });
                    }
                },
                ParakeetWindowsError::SetMismatch {
                    role: "sbom",
                    missing: Vec::new(),
                    unexpected: vec!["extra".into()],
                },
            ),
            (
                "debug profile",
                |draft| {
                    if let Some(build) = &mut draft.build {
                        build.profile = "Debug".into();
                    }
                },
                unexpected("profile", "Release", "Debug"),
            ),
            (
                "ggml native on",
                |draft| {
                    if let Some(build) = &mut draft.build {
                        build.ggml_native = true;
                    }
                },
                unexpected("ggml native", "false", "true"),
            ),
            (
                "cpu only false",
                |draft| {
                    if let Some(build) = &mut draft.build {
                        build.cpu_only = false;
                    }
                },
                unexpected("cpu only", "true", "false"),
            ),
            (
                "runtime library mt",
                |draft| {
                    if let Some(build) = &mut draft.build {
                        build.runtime_library = RuntimeLibrary::Mt;
                    }
                },
                unexpected("runtime library", "Md", "Mt"),
            ),
            (
                "shared ggml",
                |draft| {
                    if let Some(build) = &mut draft.build {
                        build.ggml_linkage = GgmlLinkage::Shared;
                    }
                },
                unexpected("ggml linkage", "Static", "Shared"),
            ),
        ];

        for (name, mutate, expected) in cases {
            assert_eq!(&admit_err(mutate), expected, "{name}");
        }

        match verify_model_bytes(b"not-the-model") {
            Err(ParakeetWindowsError::Unexpected {
                role: "model size", ..
            }) => {}
            other => panic!("expected model size mismatch for fixture bytes, got {other:?}"),
        }
    }

    #[test]
    fn parakeet_admission_rejects_ambient_or_unbounded_server_configuration() {
        let cases: &[AdmissionCase] = &[
            (
                "ambient server",
                |draft| draft.server = Some(ParakeetServerSource::Ambient),
                ParakeetWindowsError::Ambient {
                    category: ParakeetAdmissionCategory::ServerConfiguration,
                },
            ),
            (
                "missing server",
                |draft| draft.server = None,
                ParakeetWindowsError::Missing {
                    category: ParakeetAdmissionCategory::ServerConfiguration,
                },
            ),
            (
                "host 0.0.0.0",
                |draft| server_config(draft).host = Some("0.0.0.0".into()),
                unexpected("host", "127.0.0.1 or ::1", "0.0.0.0"),
            ),
            (
                "host ::",
                |draft| server_config(draft).host = Some("::".into()),
                unexpected("host", "127.0.0.1 or ::1", "::"),
            ),
            (
                "host localhost",
                |draft| server_config(draft).host = Some("localhost".into()),
                unexpected("host", "127.0.0.1 or ::1", "localhost"),
            ),
            (
                "empty host",
                |draft| server_config(draft).host = Some(String::new()),
                unexpected("host", "127.0.0.1 or ::1", ""),
            ),
            (
                "host lan",
                |draft| server_config(draft).host = Some("192.168.1.1".into()),
                unexpected("host", "127.0.0.1 or ::1", "192.168.1.1"),
            ),
            (
                "host none",
                |draft| server_config(draft).host = None,
                ParakeetWindowsError::MissingRequired { role: "host" },
            ),
            (
                "missing auth token",
                |draft| server_config(draft).auth_token = None,
                ParakeetWindowsError::MissingRequired { role: "auth token" },
            ),
            (
                "empty auth token",
                |draft| server_config(draft).auth_token = Some(String::new()),
                unexpected("auth token", "non-empty", ""),
            ),
            (
                "missing auth nonce",
                |draft| server_config(draft).auth_nonce = None,
                ParakeetWindowsError::MissingRequired { role: "auth nonce" },
            ),
            (
                "empty auth nonce",
                |draft| server_config(draft).auth_nonce = Some(String::new()),
                unexpected("auth nonce", "non-empty", ""),
            ),
            (
                "authenticated health false",
                |draft| server_config(draft).authenticated_health = Some(false),
                unexpected("authenticated health", "true", "false"),
            ),
            (
                "authenticated health none",
                |draft| server_config(draft).authenticated_health = None,
                ParakeetWindowsError::MissingRequired {
                    role: "authenticated health",
                },
            ),
            (
                "unbounded request size",
                |draft| server_config(draft).request_size = None,
                ParakeetWindowsError::Unbounded {
                    role: "request size",
                },
            ),
            (
                "zero request size",
                |draft| server_config(draft).request_size = Some(0),
                unexpected("request size", "nonzero bound", "0"),
            ),
            (
                "unbounded audio duration",
                |draft| server_config(draft).audio_duration = None,
                ParakeetWindowsError::Unbounded {
                    role: "audio duration",
                },
            ),
            (
                "zero audio duration",
                |draft| server_config(draft).audio_duration = Some(Duration::ZERO),
                unexpected("audio duration", "nonzero bound", "0"),
            ),
            (
                "unbounded concurrency",
                |draft| server_config(draft).concurrency = None,
                ParakeetWindowsError::Unbounded {
                    role: "concurrency",
                },
            ),
            (
                "zero concurrency",
                |draft| server_config(draft).concurrency = Some(0),
                unexpected("concurrency", "nonzero bound", "0"),
            ),
            (
                "unbounded queue depth",
                |draft| server_config(draft).queue_depth = None,
                ParakeetWindowsError::Unbounded {
                    role: "queue depth",
                },
            ),
            (
                "zero queue depth",
                |draft| server_config(draft).queue_depth = Some(0),
                unexpected("queue depth", "nonzero bound", "0"),
            ),
            (
                "unbounded output size",
                |draft| server_config(draft).output_size = None,
                ParakeetWindowsError::Unbounded {
                    role: "output size",
                },
            ),
            (
                "zero output size",
                |draft| server_config(draft).output_size = Some(0),
                unexpected("output size", "nonzero bound", "0"),
            ),
            (
                "unbounded deadline",
                |draft| server_config(draft).deadline = None,
                ParakeetWindowsError::Unbounded { role: "deadline" },
            ),
            (
                "zero deadline",
                |draft| server_config(draft).deadline = Some(Duration::ZERO),
                unexpected("deadline", "nonzero bound", "0"),
            ),
            (
                "unbounded stdout",
                |draft| server_config(draft).stdout = None,
                ParakeetWindowsError::Unbounded { role: "stdout" },
            ),
            (
                "zero stdout",
                |draft| server_config(draft).stdout = Some(0),
                unexpected("stdout", "nonzero bound", "0"),
            ),
            (
                "unbounded stderr",
                |draft| server_config(draft).stderr = None,
                ParakeetWindowsError::Unbounded { role: "stderr" },
            ),
            (
                "zero stderr",
                |draft| server_config(draft).stderr = Some(0),
                unexpected("stderr", "nonzero bound", "0"),
            ),
        ];

        for (name, mutate, expected) in cases {
            assert_eq!(&admit_err(mutate), expected, "{name}");
        }
    }
}
