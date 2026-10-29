// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-owned Android native service-tool admission, manifest verification,
//! and process-local tool registry.
//!
//! Android applications do not use the desktop installed package layout
//! (`bin/`, `lib/`, `share/`). Instead, app-owned service tools are delivered
//! and admitted through `solstone.android-service-tools.v2`.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use solstone_core_distribution::elf::{ElfInfo, parse_elf};
use solstone_core_installed_payload::{
    COMPILED_VERSION, InstalledPayloadRefusal, PRODUCT, code, digest_matches, read_member,
    resolve_member, sha256_hex, validate_relative,
};

pub const ANDROID_SERVICE_TOOLS_SCHEMA: &str = "solstone.android-service-tools.v2";
pub const ANDROID_SERVICE_TOOLS_TARGET: &str = "android-aarch64-app";

pub const ANDROID_RESTIC_FILENAME: &str = "librestic.so";
pub const ANDROID_RCLONE_FILENAME: &str = "librclone.so";
pub const ANDROID_NVATTEST_FILENAME: &str = "libnvattest.so";
pub const ANDROID_NVAT_FILENAME: &str = "libnvat.so";
pub const ANDROID_CA_BUNDLE_FILENAME: &str = "ca-bundle.pem";

/// Single path component of the private appraisal directory under the admitted
/// resource root. [`create_android_appraisal_staging`] joins and checks it.
pub const ANDROID_APPRAISAL_STAGING_DIR: &str = "appraisal-staging";

const fn staging_component_is_single(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || is_dot_component(bytes) {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'/' || byte == b'\\' || byte == 0 {
            return false;
        }
        index += 1;
    }
    true
}

const fn is_dot_component(bytes: &[u8]) -> bool {
    (bytes.len() == 1 && bytes[0] == b'.')
        || (bytes.len() == 2 && bytes[0] == b'.' && bytes[1] == b'.')
}

const _: () = assert!(staging_component_is_single(ANDROID_APPRAISAL_STAGING_DIR));

const ANDROID_SERVICE_TOOL_FILE_COUNT: usize = 8;

const NVATTEST_CLI_NEEDED: &[&str] = &["libnvat.so", "libm.so", "libdl.so", "libc.so"];
const NVAT_LIBRARY_NEEDED: &[&str] = &["libz.so", "liblog.so", "libdl.so", "libm.so", "libc.so"];
const ANDROID_OS_LIBRARY_SHADOWS: &[&str] =
    &["libc.so", "libm.so", "libdl.so", "liblog.so", "libz.so"];

const MOBILE_NATIVE_DRIVER: &[u8] = include_bytes!("../../../../scripts/mobile_native_build.py");
const MOBILE_NATIVE_INPUTS: &[u8] = include_bytes!("../../../distribution/mobile/inputs.json");
const MOBILE_NVATTEST_PATCH: &[u8] =
    include_bytes!("../../../distribution/mobile/nvattest-mobile.patch");

/// Driver-consumed Android native inputs. Unrelated components such as llama,
/// Parakeet, PDFium, or `nvattest-zlib` may be absent.
pub const ANDROID_NATIVE_RECEIPT_INPUTS: &[&str] = &[
    "restic",
    "rclone",
    "go-android",
    "nvattest",
    "nvattest-corrosion",
    "nvattest-regorus",
    "nvattest-cli11",
    "nvattest-jwt-cpp",
    "nvattest-fmt",
    "nvattest-spdlog",
    "nvattest-json",
    "nvattest-openssl",
    "nvattest-libxml2",
    "nvattest-xmlsec1",
    "nvattest-curl",
];

pub use solstone_core_distribution::read_test_ca_bundle_pem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AndroidFileRoot {
    NativeLibrary,
    Resource,
}

impl AndroidFileRoot {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeLibrary => "native-library",
            Self::Resource => "resource",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AndroidServiceToolRole {
    Restic,
    Rclone,
    Nvattest,
    Nvat,
}

impl AndroidServiceToolRole {
    #[must_use]
    pub const fn filename(self) -> &'static str {
        match self {
            Self::Restic => ANDROID_RESTIC_FILENAME,
            Self::Rclone => ANDROID_RCLONE_FILENAME,
            Self::Nvattest => ANDROID_NVATTEST_FILENAME,
            Self::Nvat => ANDROID_NVAT_FILENAME,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidApplicationPcrEntry {
    pub register: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidPackagedRimEntry {
    pub filename: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidToolPinEntry {
    pub revision: String,
    pub archive_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidApplicationPcrProfile {
    pub pcr_sha256: String,
    pub registers: Vec<AndroidApplicationPcrEntry>,
}

fn application_profiles_manifest(
    profiles: &[(&str, &crate::pins::ApplicationPcrValues)],
) -> Vec<AndroidApplicationPcrProfile> {
    profiles
        .iter()
        .map(|(pin, registers)| AndroidApplicationPcrProfile {
            pcr_sha256: (*pin).to_owned(),
            registers: registers
                .iter()
                .map(|(register, digest)| AndroidApplicationPcrEntry {
                    register: *register,
                    sha256: hex_lower(digest),
                })
                .collect(),
        })
        .collect()
}

fn validate_application_profiles(
    actual: &[AndroidApplicationPcrProfile],
    expected: &[(&str, &crate::pins::ApplicationPcrValues)],
) -> Result<(), AndroidServiceToolsRefusal> {
    if actual != application_profiles_manifest(expected) {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "appraisal-policy",
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidIdentityManifest {
    pub sdk_revision: String,
    pub regorus_revision: String,
    pub ca_bundle_sha256: String,
    pub pcr_sha256_pins: Vec<String>,
    pub id_key_digest_pins: Vec<String>,
    pub quote_pcr_selection: Vec<u32>,
    pub application_profiles: Vec<AndroidApplicationPcrProfile>,
    pub rims: Vec<AndroidPackagedRimEntry>,
    pub restic: AndroidToolPinEntry,
    pub rclone: AndroidToolPinEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidServiceToolFile {
    pub root: AndroidFileRoot,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<AndroidServiceToolRole>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAndroidServiceToolFile {
    root: AndroidFileRoot,
    path: String,
    bytes: u64,
    sha256: String,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidServiceToolsManifest {
    pub schema: String,
    pub product: String,
    pub version: String,
    pub target: String,
    pub identity: AndroidIdentityManifest,
    pub files: Vec<AndroidServiceToolFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAndroidServiceToolsManifest {
    schema: String,
    product: String,
    version: String,
    target: String,
    identity: AndroidIdentityManifest,
    files: Vec<RawAndroidServiceToolFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AndroidServiceToolsRefusal {
    #[error("wrong schema: expected {expected}, found {found}")]
    WrongSchema { expected: String, found: String },
    #[error("wrong product: expected {expected}, found {found}")]
    WrongProduct { expected: String, found: String },
    #[error("wrong version: expected {expected}, found {found}")]
    WrongVersion { expected: String, found: String },
    #[error("wrong target: expected {expected}, found {found}")]
    WrongTarget { expected: String, found: String },
    #[error("manifest invalid: {detail}")]
    ManifestInvalid { detail: String },
    #[error("unsafe path: {path}")]
    UnsafePath { path: String },
    #[error("member missing: {path}")]
    MemberMissing {
        path: String,
        role: Option<AndroidServiceToolRole>,
    },
    #[error("member changed: {path}")]
    MemberChanged {
        path: String,
        role: Option<AndroidServiceToolRole>,
    },
    #[error("duplicate role: {role:?}")]
    DuplicateRole { role: AndroidServiceToolRole },
    #[error("duplicate path: {path}")]
    DuplicatePath { root: AndroidFileRoot, path: String },
    #[error("unknown role: {role}")]
    UnknownRole { role: String },
    #[error("role missing: {role:?}")]
    RoleMissing { role: AndroidServiceToolRole },
    #[error("root substituted: {root:?}")]
    RootSubstituted { root: AndroidFileRoot },
    #[error("pin mismatch for {field}")]
    PinMismatch { field: &'static str },
    #[error("already installed in process")]
    AlreadyInstalled,
    #[error("load unrepresentable: {needed} tag {tag} absent {absent}")]
    LoadUnrepresentable {
        needed: String,
        tag: &'static str,
        absent: &'static str,
    },
    #[error("receipt invalid: {detail}")]
    ReceiptInvalid { detail: String },
    #[error("library shadow: {name}")]
    LibraryShadow { name: &'static str },
}

impl AndroidServiceToolsRefusal {
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::WrongSchema { .. } => "wrong-schema",
            Self::WrongProduct { .. } => "wrong-product",
            Self::WrongVersion { .. } => "wrong-version",
            Self::WrongTarget { .. } => "wrong-target",
            Self::ManifestInvalid { .. } => "manifest-invalid",
            Self::UnsafePath { .. } => "unsafe-path",
            Self::MemberMissing { .. } => "member-missing",
            Self::MemberChanged { .. } => "member-changed",
            Self::DuplicateRole { .. } => "duplicate-role",
            Self::DuplicatePath { .. } => "duplicate-path",
            Self::UnknownRole { .. } => "unknown-role",
            Self::RoleMissing { .. } => "role-missing",
            Self::RootSubstituted { .. } => "root-substituted",
            Self::PinMismatch { .. } => "pin-mismatch",
            Self::AlreadyInstalled => "already-installed",
            Self::LoadUnrepresentable { .. } => "load-unrepresentable",
            Self::ReceiptInvalid { .. } => "receipt-invalid",
            Self::LibraryShadow { .. } => "library-shadow",
        }
    }

    #[must_use]
    pub fn role(&self) -> Option<AndroidServiceToolRole> {
        match self {
            Self::MemberMissing { role, .. } => *role,
            Self::MemberChanged { role, .. } => *role,
            Self::DuplicateRole { role } => Some(*role),
            Self::RoleMissing { role } => Some(*role),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootIdentity {
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub ino: u64,
}

impl RootIdentity {
    pub fn inspect(path: &Path) -> Result<Self, AndroidServiceToolsRefusal> {
        let meta =
            fs::symlink_metadata(path).map_err(|_| AndroidServiceToolsRefusal::UnsafePath {
                path: path.display().to_string(),
            })?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(AndroidServiceToolsRefusal::UnsafePath {
                path: path.display().to_string(),
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                dev: meta.dev(),
                ino: meta.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    pub fn matches(&self, current: &Self) -> bool {
        #[cfg(unix)]
        {
            self.dev == current.dev && self.ino == current.ino
        }
        #[cfg(not(unix))]
        {
            let _ = current;
            true
        }
    }
}

#[derive(Debug, Clone)]
pub struct AndroidServiceToolsAdmitted {
    native_library_dir: PathBuf,
    resource_root: PathBuf,
    native_identity: RootIdentity,
    resource_identity: RootIdentity,
    manifest_bytes: Vec<u8>,
    manifest: AndroidServiceToolsManifest,
}

impl AndroidServiceToolsAdmitted {
    #[must_use]
    pub fn native_library_dir(&self) -> &Path {
        &self.native_library_dir
    }

    #[must_use]
    pub fn resource_root(&self) -> &Path {
        &self.resource_root
    }

    #[must_use]
    pub fn manifest(&self) -> &AndroidServiceToolsManifest {
        &self.manifest
    }

    pub fn resolve_tool(
        &self,
        role: AndroidServiceToolRole,
    ) -> Result<PathBuf, AndroidServiceToolsRefusal> {
        let current_native = RootIdentity::inspect(&self.native_library_dir).map_err(|_| {
            AndroidServiceToolsRefusal::RootSubstituted {
                root: AndroidFileRoot::NativeLibrary,
            }
        })?;
        if !self.native_identity.matches(&current_native) {
            return Err(AndroidServiceToolsRefusal::RootSubstituted {
                root: AndroidFileRoot::NativeLibrary,
            });
        }

        let filename = role.filename();
        let full_path = resolve_member(&self.native_library_dir, filename)
            .map_err(|e| map_payload_refusal(e, filename, Some(role)))?;
        let bytes = read_member(&full_path, filename)
            .map_err(|e| map_payload_refusal(e, filename, Some(role)))?;

        let expected_file = self
            .manifest
            .files
            .iter()
            .find(|f| f.root == AndroidFileRoot::NativeLibrary && f.path == filename)
            .ok_or_else(|| AndroidServiceToolsRefusal::MemberMissing {
                path: filename.to_owned(),
                role: Some(role),
            })?;

        if !digest_matches(&bytes, expected_file.bytes, &expected_file.sha256) {
            return Err(AndroidServiceToolsRefusal::MemberChanged {
                path: filename.to_owned(),
                role: Some(role),
            });
        }

        Ok(full_path)
    }

    pub fn revalidate(&self) -> Result<(), AndroidServiceToolsRefusal> {
        let current_native = RootIdentity::inspect(&self.native_library_dir).map_err(|_| {
            AndroidServiceToolsRefusal::RootSubstituted {
                root: AndroidFileRoot::NativeLibrary,
            }
        })?;
        if !self.native_identity.matches(&current_native) {
            return Err(AndroidServiceToolsRefusal::RootSubstituted {
                root: AndroidFileRoot::NativeLibrary,
            });
        }

        let current_resource = RootIdentity::inspect(&self.resource_root).map_err(|_| {
            AndroidServiceToolsRefusal::RootSubstituted {
                root: AndroidFileRoot::Resource,
            }
        })?;
        if !self.resource_identity.matches(&current_resource) {
            return Err(AndroidServiceToolsRefusal::RootSubstituted {
                root: AndroidFileRoot::Resource,
            });
        }

        for file in &self.manifest.files {
            let root = match file.root {
                AndroidFileRoot::NativeLibrary => &self.native_library_dir,
                AndroidFileRoot::Resource => &self.resource_root,
            };
            let full_path = resolve_member(root, &file.path)
                .map_err(|e| map_payload_refusal(e, &file.path, file.role))?;
            let bytes = read_member(&full_path, &file.path)
                .map_err(|e| map_payload_refusal(e, &file.path, file.role))?;
            if !digest_matches(&bytes, file.bytes, &file.sha256) {
                return Err(AndroidServiceToolsRefusal::MemberChanged {
                    path: file.path.clone(),
                    role: file.role,
                });
            }
        }
        Ok(())
    }
}

fn map_payload_refusal(
    refusal: InstalledPayloadRefusal,
    path: &str,
    role: Option<AndroidServiceToolRole>,
) -> AndroidServiceToolsRefusal {
    if refusal.code == code::UNSAFE_PATH {
        AndroidServiceToolsRefusal::UnsafePath {
            path: path.to_owned(),
        }
    } else {
        AndroidServiceToolsRefusal::MemberMissing {
            path: path.to_owned(),
            role,
        }
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    result
}

fn load_unrepresentable(
    needed: impl Into<String>,
    tag: &'static str,
    absent: &'static str,
) -> AndroidServiceToolsRefusal {
    AndroidServiceToolsRefusal::LoadUnrepresentable {
        needed: needed.into(),
        tag,
        absent,
    }
}

fn pin_mismatch(field: &'static str) -> AndroidServiceToolsRefusal {
    AndroidServiceToolsRefusal::PinMismatch { field }
}

fn receipt_invalid(detail: &str) -> AndroidServiceToolsRefusal {
    AndroidServiceToolsRefusal::ReceiptInvalid {
        detail: detail.to_owned(),
    }
}

/// Exact retained nvattest CLI and `libnvat.so` load closure.
///
/// The CLI needs `libnvat.so` plus OS `libm`, `libdl`, and `libc`. `libnvat.so`
/// needs only OS `libz`, `liblog`, `libdl`, `libm`, and `libc`, and its SONAME
/// is `libnvat.so`. Neither file may carry `DT_RPATH` or `DT_RUNPATH`.
pub fn verifier_load_closure(
    cli: &ElfInfo,
    library: &ElfInfo,
) -> Result<(), AndroidServiceToolsRefusal> {
    check_needed(&cli.needed, NVATTEST_CLI_NEEDED)?;
    check_no_loader_path(cli)?;
    check_needed(&library.needed, NVAT_LIBRARY_NEEDED)?;
    check_no_loader_path(library)?;
    if library.soname.as_deref() != Some(ANDROID_NVAT_FILENAME) {
        return Err(load_unrepresentable(
            library
                .soname
                .clone()
                .unwrap_or_else(|| ANDROID_NVAT_FILENAME.to_owned()),
            "DT_SONAME",
            ANDROID_NVAT_FILENAME,
        ));
    }
    Ok(())
}

fn check_needed(actual: &[String], expected: &[&str]) -> Result<(), AndroidServiceToolsRefusal> {
    let mut seen = BTreeSet::new();
    for name in actual {
        if !seen.insert(name.as_str()) || !expected.contains(&name.as_str()) {
            return Err(load_unrepresentable(
                name.clone(),
                "DT_NEEDED",
                "allowlisted-os-library",
            ));
        }
    }
    for name in expected {
        if !seen.contains(name) {
            return Err(load_unrepresentable(
                (*name).to_owned(),
                "DT_NEEDED",
                "allowlisted-os-library",
            ));
        }
    }
    Ok(())
}

fn check_no_loader_path(info: &ElfInfo) -> Result<(), AndroidServiceToolsRefusal> {
    if info.runpath.is_some() {
        return Err(load_unrepresentable(
            ANDROID_NVAT_FILENAME,
            "DT_RUNPATH",
            "absent",
        ));
    }
    if info.rpath.is_some() {
        return Err(load_unrepresentable(
            ANDROID_NVAT_FILENAME,
            "DT_RPATH",
            "absent",
        ));
    }
    Ok(())
}

fn parse_closure_elf(bytes: &[u8], name: &str) -> Result<ElfInfo, AndroidServiceToolsRefusal> {
    parse_elf(bytes).map_err(|_| load_unrepresentable(name, "ELF", "ELF64"))
}

/// Refuses a non-absolute loader directory, a path separator, or an app-local
/// copy of an OS library the nvattest closure is allowed to need.
pub fn validate_android_loader_directory(dir: &Path) -> Result<(), AndroidServiceToolsRefusal> {
    let text = dir
        .to_str()
        .ok_or_else(|| AndroidServiceToolsRefusal::UnsafePath {
            path: dir.display().to_string(),
        })?;
    if !dir.is_absolute() || text.contains(':') || text.contains('\0') {
        return Err(AndroidServiceToolsRefusal::UnsafePath {
            path: text.to_owned(),
        });
    }
    RootIdentity::inspect(dir)?;
    for name in ANDROID_OS_LIBRARY_SHADOWS {
        if fs::symlink_metadata(dir.join(name)).is_ok() {
            return Err(AndroidServiceToolsRefusal::LibraryShadow { name });
        }
    }
    Ok(())
}

struct ReceiptArtifacts {
    restic: String,
    rclone: String,
    nvattest: String,
    nvat: String,
    ca_bundle: String,
}

fn require_sha(
    actual: &str,
    expected: &str,
    field: &'static str,
) -> Result<(), AndroidServiceToolsRefusal> {
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(pin_mismatch(field))
    }
}

fn receipt_input_fields(name: &str) -> (&'static str, &'static str) {
    match name {
        "restic" => ("restic-revision", "restic-archive"),
        "rclone" => ("rclone-revision", "rclone-archive"),
        "nvattest" => ("sdk-revision", "nvattest-archive"),
        "go-android" => ("go-android", "go-android"),
        "nvattest-corrosion" => ("nvattest-corrosion", "nvattest-corrosion"),
        "nvattest-regorus" => ("nvattest-regorus", "nvattest-regorus"),
        "nvattest-cli11" => ("nvattest-cli11", "nvattest-cli11"),
        "nvattest-jwt-cpp" => ("nvattest-jwt-cpp", "nvattest-jwt-cpp"),
        "nvattest-fmt" => ("nvattest-fmt", "nvattest-fmt"),
        "nvattest-spdlog" => ("nvattest-spdlog", "nvattest-spdlog"),
        "nvattest-json" => ("nvattest-json", "nvattest-json"),
        "nvattest-openssl" => ("nvattest-openssl", "nvattest-openssl"),
        "nvattest-libxml2" => ("nvattest-libxml2", "nvattest-libxml2"),
        "nvattest-xmlsec1" => ("nvattest-xmlsec1", "nvattest-xmlsec1"),
        "nvattest-curl" => ("nvattest-curl", "nvattest-curl"),
        _ => ("inputs", "inputs"),
    }
}

fn require_regorus_build_revision(
    patches: &[serde_json::Value],
) -> Result<(), AndroidServiceToolsRefusal> {
    let pin = solstone_core_distribution::regorus_build_revision_pin();
    let compiled = solstone_core_distribution::mobile_input_pin("nvattest-regorus");
    if pin.revision != compiled.revision {
        return Err(pin_mismatch("regorus-build-revision"));
    }
    let mut found = 0;
    let mut matched = false;
    for entry in patches {
        if entry.get("component").and_then(|value| value.as_str()) != Some("regorus-build-revision")
        {
            continue;
        }
        found += 1;
        let revision = entry
            .get("revision")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let original = entry
            .get("original_sha256")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let patched = entry
            .get("patched_sha256")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        matched = revision == pin.revision
            && original.eq_ignore_ascii_case(pin.original_sha256)
            && patched.eq_ignore_ascii_case(pin.patched_sha256);
    }
    if found != 1 || !matched {
        return Err(pin_mismatch("regorus-build-revision"));
    }
    Ok(())
}

fn require_input_pin(
    inputs: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    revision: &str,
    archive_sha256: &str,
    revision_field: &'static str,
    archive_field: &'static str,
) -> Result<(), AndroidServiceToolsRefusal> {
    let Some(entry) = inputs.get(name).and_then(|value| value.as_object()) else {
        return Err(pin_mismatch(revision_field));
    };
    let got_revision = entry
        .get("revision")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if got_revision != revision {
        return Err(pin_mismatch(revision_field));
    }
    let got_sha = entry
        .get("sha256")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    require_sha(got_sha, archive_sha256, archive_field)
}

fn artifact_sha256(
    artifacts: &[serde_json::Value],
    component: &str,
    field: &'static str,
) -> Result<String, AndroidServiceToolsRefusal> {
    let mut found = None;
    for entry in artifacts {
        if entry.get("component").and_then(|value| value.as_str()) != Some(component) {
            continue;
        }
        if found.is_some() {
            return Err(pin_mismatch(field));
        }
        let Some(sha) = entry.get("sha256").and_then(|value| value.as_str()) else {
            return Err(pin_mismatch(field));
        };
        found = Some(sha.to_owned());
    }
    found.ok_or_else(|| pin_mismatch(field))
}

fn validate_mobile_native_receipt(
    bytes: &[u8],
) -> Result<ReceiptArtifacts, AndroidServiceToolsRefusal> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| receipt_invalid("json"))?;
    let Some(obj) = value.as_object() else {
        return Err(receipt_invalid("object"));
    };
    // The maintained driver leaves `complete` false for component builds.
    // Admit only the exact service-tool subset; never relabel it full-stack.
    let service_tool_subset = obj.get("complete").and_then(|value| value.as_bool()) == Some(false)
        && obj
            .get("scope")
            .and_then(|value| value.as_array())
            .is_some_and(|scope| {
                scope.len() == 3
                    && ["restic", "rclone", "nvattest"].iter().all(|component| {
                        scope.iter().any(|value| value.as_str() == Some(component))
                    })
            });
    if obj.get("status").and_then(|value| value.as_str()) != Some("passed")
        || (obj.get("complete").and_then(|value| value.as_bool()) != Some(true)
            && !service_tool_subset)
        || obj.get("exit").and_then(|value| value.as_i64()) != Some(0)
        || obj.get("platform").and_then(|value| value.as_str()) != Some("android")
    {
        return Err(receipt_invalid("status"));
    }
    let driver = obj
        .get("driver_sha256")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    require_sha(driver, &sha256_hex(MOBILE_NATIVE_DRIVER), "driver")?;
    let inputs_sha = obj
        .get("inputs_sha256")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    require_sha(inputs_sha, &sha256_hex(MOBILE_NATIVE_INPUTS), "inputs")?;
    let patch_sha = sha256_hex(MOBILE_NVATTEST_PATCH);
    let Some(patches) = obj.get("patches").and_then(|value| value.as_array()) else {
        return Err(pin_mismatch("patch"));
    };
    let patch_ok = patches.iter().any(|entry| {
        entry.get("component").and_then(|value| value.as_str()) == Some("nvattest")
            && entry
                .get("sha256")
                .and_then(|value| value.as_str())
                .is_some_and(|sha| sha.eq_ignore_ascii_case(&patch_sha))
    });
    if !patch_ok {
        return Err(pin_mismatch("patch"));
    }
    require_regorus_build_revision(patches)?;
    let Some(inputs) = obj.get("inputs").and_then(|value| value.as_object()) else {
        return Err(pin_mismatch("inputs"));
    };
    for name in ANDROID_NATIVE_RECEIPT_INPUTS {
        let (revision_field, archive_field) = receipt_input_fields(name);
        let pin = solstone_core_distribution::mobile_input_pin(name);
        let revision = if *name == "nvattest" {
            if pin.revision != solstone_core_distribution::NVATTEST_SDK_REVISION {
                return Err(pin_mismatch("sdk-revision"));
            }
            solstone_core_distribution::NVATTEST_SDK_REVISION
        } else {
            pin.revision
        };
        require_input_pin(
            inputs,
            name,
            revision,
            pin.archive_sha256,
            revision_field,
            archive_field,
        )?;
    }
    let Some(artifacts) = obj.get("artifacts").and_then(|value| value.as_array()) else {
        return Err(receipt_invalid("artifacts"));
    };
    let ca_bundle = artifact_sha256(artifacts, "nvattest-ca-bundle", "ca-bundle")?;
    require_sha(
        &ca_bundle,
        solstone_core_distribution::pinned_ca_bundle_sha256(),
        "ca-bundle",
    )?;
    Ok(ReceiptArtifacts {
        restic: artifact_sha256(artifacts, "restic-cli", "restic-artifact")?,
        rclone: artifact_sha256(artifacts, "rclone-cli", "rclone-artifact")?,
        nvattest: artifact_sha256(artifacts, "nvattest-cli", "nvattest-artifact")?,
        nvat: artifact_sha256(artifacts, "nvattest-library", "nvat-artifact")?,
        ca_bundle,
    })
}

fn read_named(
    root: &Path,
    filename: &str,
    role: Option<AndroidServiceToolRole>,
) -> Result<Vec<u8>, AndroidServiceToolsRefusal> {
    let path =
        resolve_member(root, filename).map_err(|err| map_payload_refusal(err, filename, role))?;
    read_member(&path, filename).map_err(|err| map_payload_refusal(err, filename, role))
}

fn push_native_role(
    files: &mut Vec<AndroidServiceToolFile>,
    root: &Path,
    role: AndroidServiceToolRole,
    artifact_sha: &str,
    artifact_field: &'static str,
) -> Result<Vec<u8>, AndroidServiceToolsRefusal> {
    let filename = role.filename();
    let bytes = read_named(root, filename, Some(role))?;
    require_sha(&sha256_hex(&bytes), artifact_sha, artifact_field)?;
    files.push(AndroidServiceToolFile {
        root: AndroidFileRoot::NativeLibrary,
        path: filename.to_owned(),
        bytes: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
        role: Some(role),
    });
    Ok(bytes)
}

fn validate_relative_path(path: &str) -> Result<(), AndroidServiceToolsRefusal> {
    validate_relative(path).map_err(|_| AndroidServiceToolsRefusal::UnsafePath {
        path: path.to_owned(),
    })
}

/// Renders `solstone.android-service-tools.v2` for one native-library directory
/// and one resource root.
///
/// `receipt_bytes` is a mobile-native-build receipt. It must record a passed
/// Android build: either a complete stack or the exact restic/rclone/nvattest
/// component scope, which retains `complete=false`. Its driver, inputs file,
/// nvattest patch, Regorus
/// build-revision patch, and every driver-consumed native input match the
/// compiled sources. Its artifact hashes must equal the packaged
/// `librestic.so`, `librclone.so`, `libnvattest.so`, `libnvat.so`, and
/// `ca-bundle.pem` bytes. Current pin labels are not applied to any other
/// bytes. Unrelated components may be absent.
///
/// Do not take the receipt or either root from Intent extras or from
/// `JournalService` Binder fields `op`, `root`, and `count`.
pub fn render_android_service_tools_manifest(
    native_library_dir: &Path,
    resource_root: &Path,
    receipt_bytes: &[u8],
) -> Result<Vec<u8>, AndroidServiceToolsRefusal> {
    RootIdentity::inspect(native_library_dir)?;
    RootIdentity::inspect(resource_root)?;
    let receipt = validate_mobile_native_receipt(receipt_bytes)?;

    let restic_pin = solstone_core_distribution::restic_input_pin();
    let stripped_restic_rev = restic_pin
        .revision
        .strip_prefix('v')
        .unwrap_or(restic_pin.revision);
    if stripped_restic_rev != solstone_core_backup::SUPPORTED_RESTIC_VERSION {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "restic-revision",
        });
    }
    let rclone_pin = solstone_core_distribution::rclone_input_pin();

    let identity = AndroidIdentityManifest {
        sdk_revision: solstone_core_distribution::NVATTEST_SDK_REVISION.to_owned(),
        regorus_revision: solstone_core_distribution::pinned_regorus_revision().to_owned(),
        ca_bundle_sha256: solstone_core_distribution::pinned_ca_bundle_sha256().to_owned(),
        pcr_sha256_pins: crate::pins::PRODUCTION_PCR_SHA256_PINS
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        id_key_digest_pins: crate::pins::PRODUCTION_ID_KEY_DIGEST_PINS
            .iter()
            .map(|b| hex_lower(b))
            .collect(),
        quote_pcr_selection: crate::pins::PUBLISHED_QUOTE_PCR_SELECTION.to_vec(),
        application_profiles: application_profiles_manifest(
            crate::pins::PUBLISHED_APPLICATION_PCR_PROFILES,
        ),
        rims: crate::nvgpu::ManifestSet::QUALIFIED_595_71_05
            .entries()
            .iter()
            .map(|rim| AndroidPackagedRimEntry {
                filename: rim.filename().to_owned(),
                sha256: rim.sha256().to_owned(),
            })
            .collect(),
        restic: AndroidToolPinEntry {
            revision: restic_pin.revision.to_owned(),
            archive_sha256: restic_pin.archive_sha256.to_owned(),
        },
        rclone: AndroidToolPinEntry {
            revision: rclone_pin.revision.to_owned(),
            archive_sha256: rclone_pin.archive_sha256.to_owned(),
        },
    };

    let mut files = Vec::new();

    push_native_role(
        &mut files,
        native_library_dir,
        AndroidServiceToolRole::Restic,
        &receipt.restic,
        "restic-artifact",
    )?;
    push_native_role(
        &mut files,
        native_library_dir,
        AndroidServiceToolRole::Rclone,
        &receipt.rclone,
        "rclone-artifact",
    )?;
    let nvattest_bytes = push_native_role(
        &mut files,
        native_library_dir,
        AndroidServiceToolRole::Nvattest,
        &receipt.nvattest,
        "nvattest-artifact",
    )?;
    let nvat_bytes = push_native_role(
        &mut files,
        native_library_dir,
        AndroidServiceToolRole::Nvat,
        &receipt.nvat,
        "nvat-artifact",
    )?;
    verifier_load_closure(
        &parse_closure_elf(&nvattest_bytes, ANDROID_NVATTEST_FILENAME)?,
        &parse_closure_elf(&nvat_bytes, ANDROID_NVAT_FILENAME)?,
    )?;

    // 2. Resource root members
    let ca_path = resolve_member(resource_root, ANDROID_CA_BUNDLE_FILENAME)
        .map_err(|e| map_payload_refusal(e, ANDROID_CA_BUNDLE_FILENAME, None))?;
    let ca_bytes = read_member(&ca_path, ANDROID_CA_BUNDLE_FILENAME)
        .map_err(|e| map_payload_refusal(e, ANDROID_CA_BUNDLE_FILENAME, None))?;
    let ca_sha = sha256_hex(&ca_bytes);
    if ca_sha != solstone_core_distribution::pinned_ca_bundle_sha256() {
        return Err(AndroidServiceToolsRefusal::PinMismatch { field: "ca-bundle" });
    }
    files.push(AndroidServiceToolFile {
        root: AndroidFileRoot::Resource,
        path: ANDROID_CA_BUNDLE_FILENAME.to_owned(),
        bytes: ca_bytes.len() as u64,
        sha256: ca_sha,
        role: None,
    });

    for rim in crate::nvgpu::ManifestSet::QUALIFIED_595_71_05.entries() {
        let rim_path = resolve_member(resource_root, rim.filename())
            .map_err(|e| map_payload_refusal(e, rim.filename(), None))?;
        let rim_bytes = read_member(&rim_path, rim.filename())
            .map_err(|e| map_payload_refusal(e, rim.filename(), None))?;
        let rim_sha = sha256_hex(&rim_bytes);
        if rim_sha != rim.sha256() {
            return Err(AndroidServiceToolsRefusal::PinMismatch { field: "rim" });
        }
        files.push(AndroidServiceToolFile {
            root: AndroidFileRoot::Resource,
            path: rim.filename().to_owned(),
            bytes: rim_bytes.len() as u64,
            sha256: rim_sha,
            role: None,
        });
    }

    let manifest = AndroidServiceToolsManifest {
        schema: ANDROID_SERVICE_TOOLS_SCHEMA.to_owned(),
        product: PRODUCT.to_owned(),
        version: COMPILED_VERSION.to_owned(),
        target: ANDROID_SERVICE_TOOLS_TARGET.to_owned(),
        identity,
        files,
    };

    serde_json::to_vec_pretty(&manifest).map_err(|e| AndroidServiceToolsRefusal::ManifestInvalid {
        detail: e.to_string(),
    })
}

pub fn admit_android_service_tools(
    native_library_dir: &Path,
    resource_root: &Path,
    manifest_bytes: &[u8],
    receipt_bytes: &[u8],
) -> Result<AndroidServiceToolsAdmitted, AndroidServiceToolsRefusal> {
    let native_identity = RootIdentity::inspect(native_library_dir)?;
    let resource_identity = RootIdentity::inspect(resource_root)?;
    let receipt = validate_mobile_native_receipt(receipt_bytes)?;

    let raw: RawAndroidServiceToolsManifest =
        serde_json::from_slice(manifest_bytes).map_err(|e| {
            AndroidServiceToolsRefusal::ManifestInvalid {
                detail: e.to_string(),
            }
        })?;

    if raw.schema != ANDROID_SERVICE_TOOLS_SCHEMA {
        return Err(AndroidServiceToolsRefusal::WrongSchema {
            expected: ANDROID_SERVICE_TOOLS_SCHEMA.to_owned(),
            found: raw.schema,
        });
    }
    if raw.product != PRODUCT {
        return Err(AndroidServiceToolsRefusal::WrongProduct {
            expected: PRODUCT.to_owned(),
            found: raw.product,
        });
    }
    if raw.version != COMPILED_VERSION {
        return Err(AndroidServiceToolsRefusal::WrongVersion {
            expected: COMPILED_VERSION.to_owned(),
            found: raw.version,
        });
    }
    if raw.target != ANDROID_SERVICE_TOOLS_TARGET {
        return Err(AndroidServiceToolsRefusal::WrongTarget {
            expected: ANDROID_SERVICE_TOOLS_TARGET.to_owned(),
            found: raw.target,
        });
    }

    // Identity verification against compiled constants
    let restic_pin = solstone_core_distribution::restic_input_pin();
    let rclone_pin = solstone_core_distribution::rclone_input_pin();

    if raw.identity.sdk_revision != solstone_core_distribution::NVATTEST_SDK_REVISION {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "sdk-revision",
        });
    }
    if raw.identity.regorus_revision != solstone_core_distribution::pinned_regorus_revision() {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "regorus-revision",
        });
    }
    if raw.identity.ca_bundle_sha256 != solstone_core_distribution::pinned_ca_bundle_sha256() {
        return Err(AndroidServiceToolsRefusal::PinMismatch { field: "ca-bundle" });
    }
    if raw.identity.pcr_sha256_pins
        != crate::pins::PRODUCTION_PCR_SHA256_PINS
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>()
    {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "appraisal-policy",
        });
    }
    if raw.identity.id_key_digest_pins
        != crate::pins::PRODUCTION_ID_KEY_DIGEST_PINS
            .iter()
            .map(|b| hex_lower(b))
            .collect::<Vec<_>>()
    {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "appraisal-policy",
        });
    }
    if raw.identity.quote_pcr_selection != crate::pins::PUBLISHED_QUOTE_PCR_SELECTION {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "appraisal-policy",
        });
    }
    validate_application_profiles(
        &raw.identity.application_profiles,
        crate::pins::PUBLISHED_APPLICATION_PCR_PROFILES,
    )?;

    let qualified_rims = crate::nvgpu::ManifestSet::QUALIFIED_595_71_05.entries();
    if raw.identity.rims.len() != qualified_rims.len() {
        return Err(AndroidServiceToolsRefusal::PinMismatch { field: "rim" });
    }
    for (entry, rim) in raw.identity.rims.iter().zip(qualified_rims.iter()) {
        if entry.filename != rim.filename() || entry.sha256 != rim.sha256() {
            return Err(AndroidServiceToolsRefusal::PinMismatch { field: "rim" });
        }
    }

    if raw.identity.restic.revision != restic_pin.revision {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "restic-revision",
        });
    }
    if raw.identity.restic.archive_sha256 != restic_pin.archive_sha256 {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "restic-archive",
        });
    }
    if raw.identity.rclone.revision != rclone_pin.revision {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "rclone-revision",
        });
    }
    if raw.identity.rclone.archive_sha256 != rclone_pin.archive_sha256 {
        return Err(AndroidServiceToolsRefusal::PinMismatch {
            field: "rclone-archive",
        });
    }

    // Convert raw files and check paths and roles
    let mut files = Vec::new();
    let mut seen_roles = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();

    let mut restic_found = false;
    let mut rclone_found = false;
    let mut nvattest_found = false;
    let mut nvat_found = false;
    let mut ca_found = false;
    let mut rims_found = BTreeSet::new();
    let mut nvattest_info = None;
    let mut nvat_info = None;

    for raw_file in raw.files {
        validate_relative_path(&raw_file.path)?;
        if !seen_paths.insert((raw_file.root, raw_file.path.clone())) {
            return Err(AndroidServiceToolsRefusal::DuplicatePath {
                root: raw_file.root,
                path: raw_file.path,
            });
        }

        let role = match raw_file.role {
            Some(role_str) => match role_str.as_str() {
                "restic" => Some(AndroidServiceToolRole::Restic),
                "rclone" => Some(AndroidServiceToolRole::Rclone),
                "nvattest" => Some(AndroidServiceToolRole::Nvattest),
                "nvat" => Some(AndroidServiceToolRole::Nvat),
                _ => return Err(AndroidServiceToolsRefusal::UnknownRole { role: role_str }),
            },
            None => None,
        };

        if let Some(r) = role {
            if raw_file.root != AndroidFileRoot::NativeLibrary {
                return Err(AndroidServiceToolsRefusal::ManifestInvalid {
                    detail: format!("role {:?} forbidden on resource root", r),
                });
            }
            if !seen_roles.insert(r) {
                return Err(AndroidServiceToolsRefusal::DuplicateRole { role: r });
            }
            match r {
                AndroidServiceToolRole::Restic => {
                    if raw_file.path != ANDROID_RESTIC_FILENAME {
                        return Err(AndroidServiceToolsRefusal::ManifestInvalid {
                            detail: "restic role must have path librestic.so".to_owned(),
                        });
                    }
                    restic_found = true;
                }
                AndroidServiceToolRole::Rclone => {
                    if raw_file.path != ANDROID_RCLONE_FILENAME {
                        return Err(AndroidServiceToolsRefusal::ManifestInvalid {
                            detail: "rclone role must have path librclone.so".to_owned(),
                        });
                    }
                    rclone_found = true;
                }
                AndroidServiceToolRole::Nvattest => {
                    if raw_file.path != ANDROID_NVATTEST_FILENAME {
                        return Err(AndroidServiceToolsRefusal::ManifestInvalid {
                            detail: "nvattest role must have path libnvattest.so".to_owned(),
                        });
                    }
                    nvattest_found = true;
                }
                AndroidServiceToolRole::Nvat => {
                    if raw_file.path != ANDROID_NVAT_FILENAME {
                        return Err(AndroidServiceToolsRefusal::ManifestInvalid {
                            detail: "nvat role must have path libnvat.so".to_owned(),
                        });
                    }
                    nvat_found = true;
                }
            }
        } else {
            if raw_file.root == AndroidFileRoot::NativeLibrary {
                return Err(AndroidServiceToolsRefusal::ManifestInvalid {
                    detail: format!(
                        "native library file {} must have an assigned role",
                        raw_file.path
                    ),
                });
            }
            if raw_file.path == ANDROID_CA_BUNDLE_FILENAME {
                if raw_file.sha256 != solstone_core_distribution::pinned_ca_bundle_sha256() {
                    return Err(AndroidServiceToolsRefusal::PinMismatch { field: "ca-bundle" });
                }
                ca_found = true;
            } else if let Some(rim) = qualified_rims
                .iter()
                .find(|rim| rim.filename() == raw_file.path)
            {
                if raw_file.sha256 != rim.sha256() {
                    return Err(AndroidServiceToolsRefusal::PinMismatch { field: "rim" });
                }
                rims_found.insert(rim.filename());
            } else {
                return Err(AndroidServiceToolsRefusal::ManifestInvalid {
                    detail: format!("unexpected resource file {}", raw_file.path),
                });
            }
        }

        files.push(AndroidServiceToolFile {
            root: raw_file.root,
            path: raw_file.path,
            bytes: raw_file.bytes,
            sha256: raw_file.sha256,
            role,
        });
    }

    if !restic_found {
        return Err(AndroidServiceToolsRefusal::RoleMissing {
            role: AndroidServiceToolRole::Restic,
        });
    }
    if !rclone_found {
        return Err(AndroidServiceToolsRefusal::RoleMissing {
            role: AndroidServiceToolRole::Rclone,
        });
    }
    if !nvattest_found {
        return Err(AndroidServiceToolsRefusal::RoleMissing {
            role: AndroidServiceToolRole::Nvattest,
        });
    }
    if !nvat_found {
        return Err(AndroidServiceToolsRefusal::RoleMissing {
            role: AndroidServiceToolRole::Nvat,
        });
    }
    if !ca_found {
        return Err(AndroidServiceToolsRefusal::MemberMissing {
            path: ANDROID_CA_BUNDLE_FILENAME.to_owned(),
            role: None,
        });
    }
    if rims_found.len() != qualified_rims.len() {
        for rim in qualified_rims {
            if !rims_found.contains(rim.filename()) {
                return Err(AndroidServiceToolsRefusal::MemberMissing {
                    path: rim.filename().to_owned(),
                    role: None,
                });
            }
        }
    }
    if files.len() != ANDROID_SERVICE_TOOL_FILE_COUNT {
        return Err(AndroidServiceToolsRefusal::ManifestInvalid {
            detail: format!(
                "manifest files set must contain exactly {ANDROID_SERVICE_TOOL_FILE_COUNT} entries, found {}",
                files.len()
            ),
        });
    }

    // Verify on-disk file bytes against manifest
    for file in &files {
        let root = match file.root {
            AndroidFileRoot::NativeLibrary => native_library_dir,
            AndroidFileRoot::Resource => resource_root,
        };
        let full_path = resolve_member(root, &file.path)
            .map_err(|e| map_payload_refusal(e, &file.path, file.role))?;
        let bytes = read_member(&full_path, &file.path)
            .map_err(|e| map_payload_refusal(e, &file.path, file.role))?;
        if !digest_matches(&bytes, file.bytes, &file.sha256) {
            return Err(AndroidServiceToolsRefusal::MemberChanged {
                path: file.path.clone(),
                role: file.role,
            });
        }
        match (file.root, file.path.as_str()) {
            (AndroidFileRoot::NativeLibrary, ANDROID_RESTIC_FILENAME) => {
                require_sha(&file.sha256, &receipt.restic, "restic-artifact")?;
            }
            (AndroidFileRoot::NativeLibrary, ANDROID_RCLONE_FILENAME) => {
                require_sha(&file.sha256, &receipt.rclone, "rclone-artifact")?;
            }
            (AndroidFileRoot::NativeLibrary, ANDROID_NVATTEST_FILENAME) => {
                require_sha(&file.sha256, &receipt.nvattest, "nvattest-artifact")?;
                nvattest_info = Some(parse_closure_elf(&bytes, ANDROID_NVATTEST_FILENAME)?);
            }
            (AndroidFileRoot::NativeLibrary, ANDROID_NVAT_FILENAME) => {
                require_sha(&file.sha256, &receipt.nvat, "nvat-artifact")?;
                nvat_info = Some(parse_closure_elf(&bytes, ANDROID_NVAT_FILENAME)?);
            }
            (AndroidFileRoot::Resource, ANDROID_CA_BUNDLE_FILENAME) => {
                require_sha(&file.sha256, &receipt.ca_bundle, "ca-bundle")?;
            }
            _ => {}
        }
    }
    verifier_load_closure(
        nvattest_info
            .as_ref()
            .ok_or_else(|| load_unrepresentable(ANDROID_NVATTEST_FILENAME, "ELF", "ELF64"))?,
        nvat_info
            .as_ref()
            .ok_or_else(|| load_unrepresentable(ANDROID_NVAT_FILENAME, "ELF", "ELF64"))?,
    )?;

    Ok(AndroidServiceToolsAdmitted {
        native_library_dir: native_library_dir.to_path_buf(),
        resource_root: resource_root.to_path_buf(),
        native_identity,
        resource_identity,
        manifest_bytes: manifest_bytes.to_vec(),
        manifest: AndroidServiceToolsManifest {
            schema: raw.schema,
            product: raw.product,
            version: raw.version,
            target: raw.target,
            identity: raw.identity,
            files,
        },
    })
}

static INSTALLED_TOOLS: OnceLock<AndroidServiceToolsAdmitted> = OnceLock::new();

/// Installs admitted Android service tools into the process-global registry.
///
/// Read `android.content.pm.ApplicationInfo.nativeLibraryDir` in the application
/// process. That absolute directory is the native library root. Packaged members
/// there are `librestic.so`, `librclone.so`, `libnvattest.so`, and `libnvat.so`.
/// `libnvattest.so` is the retained nvattest CLI. Its load closure is `libnvat.so`
/// plus the OS libraries `libm`, `libdl`, and `libc`. `libnvat.so` needs only the
/// OS libraries `libz`, `liblog`, `libdl`, `libm`, and `libc`.
///
/// Read the manifest, the mobile-native-build receipt, `ca-bundle.pem`, and the
/// three RIM XML files from the application's own packaged assets or files
/// directory. That directory is the resource root. The receipt must name the
/// current driver, inputs file, nvattest patch, Regorus build-revision patch,
/// and every driver-consumed native input. The packaged `librestic.so`,
/// `librclone.so`, `libnvattest.so`, `libnvat.so`, and `ca-bundle.pem` bytes
/// must equal its artifact hashes.
///
/// The nvattest child environment is cleared and then contains only
/// `LD_LIBRARY_PATH` set to this revalidated absolute `nativeLibraryDir`.
///
/// Appraisal inputs are staged in a private directory under this admitted
/// resource root. [`create_android_appraisal_staging`] revalidates the
/// installed authority and returns that directory. It is not taken from
/// Android `Intent` extras or from `JournalService` Binder fields.
///
/// Call this function once during application startup, before backup or
/// confidential work. An identical later call is a no-op and returns `Ok(())`.
/// A different later call returns [`AndroidServiceToolsRefusal::AlreadyInstalled`]
/// and does not replace the authority.
///
/// Do not take the manifest, the receipt, tool paths, either root, or the child
/// loader path from Android `Intent` extras or from `JournalService` Binder
/// fields `op`, `root`, and `count` (`Native.run` / `journal_run`).
///
/// Surface [`AndroidServiceToolsRefusal`]. A refusal is not a success result.
pub fn install_android_service_tools(
    native_library_dir: &Path,
    resource_root: &Path,
    manifest_bytes: &[u8],
    receipt_bytes: &[u8],
) -> Result<(), AndroidServiceToolsRefusal> {
    let admitted = admit_android_service_tools(
        native_library_dir,
        resource_root,
        manifest_bytes,
        receipt_bytes,
    )?;

    if let Some(existing) = INSTALLED_TOOLS.get() {
        if existing.native_identity.matches(&admitted.native_identity)
            && existing
                .resource_identity
                .matches(&admitted.resource_identity)
            && existing.manifest_bytes == manifest_bytes
        {
            return Ok(());
        }
        return Err(AndroidServiceToolsRefusal::AlreadyInstalled);
    }

    match INSTALLED_TOOLS.set(admitted.clone()) {
        Ok(()) => Ok(()),
        Err(_) => {
            if INSTALLED_TOOLS.get().is_some_and(|existing| {
                existing.native_identity.matches(&admitted.native_identity)
                    && existing
                        .resource_identity
                        .matches(&admitted.resource_identity)
                    && existing.manifest_bytes == manifest_bytes
            }) {
                Ok(())
            } else {
                Err(AndroidServiceToolsRefusal::AlreadyInstalled)
            }
        }
    }
}

pub fn resolve_admitted_android_service_tool(
    role: AndroidServiceToolRole,
) -> Result<PathBuf, AndroidServiceToolsRefusal> {
    if let Some(tools) = INSTALLED_TOOLS.get() {
        tools.resolve_tool(role)
    } else {
        Err(AndroidServiceToolsRefusal::MemberMissing {
            path: role.filename().to_owned(),
            role: Some(role),
        })
    }
}

#[must_use]
pub fn has_admitted_android_service_tools() -> bool {
    INSTALLED_TOOLS.get().is_some()
}

pub fn revalidate_installed_android_service_tools() -> Result<(), AndroidServiceToolsRefusal> {
    if let Some(tools) = INSTALLED_TOOLS.get() {
        tools.revalidate()
    } else {
        Err(AndroidServiceToolsRefusal::MemberMissing {
            path: "android service tools not installed in process".to_owned(),
            role: None,
        })
    }
}

/// Admitted nvattest CLI, `libnvat.so`, native-library directory, and CA bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidVerifierPaths {
    pub binary: PathBuf,
    pub library: PathBuf,
    pub lib_dir: PathBuf,
    pub ca_bundle: PathBuf,
}

/// Revalidates the process authority and returns the admitted verifier closure.
///
/// The native-library directory is absolute, contains no path separator, and
/// does not shadow an OS library. Caller must not take `lib_dir` from Binder
/// or Intent extras.
pub fn admitted_android_verifier() -> Result<AndroidVerifierPaths, AndroidServiceToolsRefusal> {
    let Some(tools) = INSTALLED_TOOLS.get() else {
        return Err(AndroidServiceToolsRefusal::MemberMissing {
            path: ANDROID_NVATTEST_FILENAME.to_owned(),
            role: Some(AndroidServiceToolRole::Nvattest),
        });
    };
    tools.revalidate()?;
    validate_android_loader_directory(&tools.native_library_dir)?;
    let binary = tools.resolve_tool(AndroidServiceToolRole::Nvattest)?;
    let library = tools.resolve_tool(AndroidServiceToolRole::Nvat)?;
    let cli_bytes = read_member(&binary, ANDROID_NVATTEST_FILENAME).map_err(|err| {
        map_payload_refusal(
            err,
            ANDROID_NVATTEST_FILENAME,
            Some(AndroidServiceToolRole::Nvattest),
        )
    })?;
    let lib_bytes = read_member(&library, ANDROID_NVAT_FILENAME).map_err(|err| {
        map_payload_refusal(
            err,
            ANDROID_NVAT_FILENAME,
            Some(AndroidServiceToolRole::Nvat),
        )
    })?;
    verifier_load_closure(
        &parse_closure_elf(&cli_bytes, ANDROID_NVATTEST_FILENAME)?,
        &parse_closure_elf(&lib_bytes, ANDROID_NVAT_FILENAME)?,
    )?;
    let ca_bundle = resolve_member(&tools.resource_root, ANDROID_CA_BUNDLE_FILENAME)
        .map_err(|err| map_payload_refusal(err, ANDROID_CA_BUNDLE_FILENAME, None))?;
    Ok(AndroidVerifierPaths {
        binary,
        library,
        lib_dir: tools.native_library_dir.clone(),
        ca_bundle,
    })
}

/// Private appraisal staging directory under the admitted resource root.
///
/// The directory is `<resource root>/appraisal-staging`. First-party startup
/// supplies that resource root to [`install_android_service_tools`]. This
/// function revalidates the installed authority before creating or returning
/// the directory. On Unix the directory is mode `0700`. A symlink, a
/// non-directory, or a directory with group or other permissions is refused.
///
/// The path is not taken from Android `Intent` extras or from `JournalService`
/// Binder fields `op`, `root`, and `count`.
pub fn create_android_appraisal_staging() -> Result<PathBuf, AndroidServiceToolsRefusal> {
    let Some(tools) = INSTALLED_TOOLS.get() else {
        return Err(AndroidServiceToolsRefusal::MemberMissing {
            path: "android service tools not installed in process".to_owned(),
            role: None,
        });
    };
    tools.revalidate()?;
    let staging = tools.resource_root.join(ANDROID_APPRAISAL_STAGING_DIR);
    if staging.file_name() != Some(OsStr::new(ANDROID_APPRAISAL_STAGING_DIR))
        || staging.parent().map(Path::as_os_str) != Some(tools.resource_root.as_os_str())
    {
        return Err(AndroidServiceToolsRefusal::UnsafePath {
            path: staging.display().to_string(),
        });
    }
    ensure_private_staging_dir(&staging)?;
    Ok(staging)
}

fn ensure_private_staging_dir(path: &Path) -> Result<(), AndroidServiceToolsRefusal> {
    match fs::symlink_metadata(path) {
        Ok(meta) => validate_private_staging(&meta, path),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder
            };
            #[cfg(not(unix))]
            let builder = fs::DirBuilder::new();
            match builder.create(path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(_) => {
                    return Err(AndroidServiceToolsRefusal::UnsafePath {
                        path: path.display().to_string(),
                    });
                }
            }
            let meta =
                fs::symlink_metadata(path).map_err(|_| AndroidServiceToolsRefusal::UnsafePath {
                    path: path.display().to_string(),
                })?;
            validate_private_staging(&meta, path)
        }
        Err(_) => Err(AndroidServiceToolsRefusal::UnsafePath {
            path: path.display().to_string(),
        }),
    }
}

fn validate_private_staging(
    meta: &fs::Metadata,
    path: &Path,
) -> Result<(), AndroidServiceToolsRefusal> {
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(AndroidServiceToolsRefusal::UnsafePath {
            path: path.display().to_string(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(AndroidServiceToolsRefusal::UnsafePath {
                path: path.display().to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn identity_binds_distinct_profiles_to_their_fingerprints() {
        let first = crate::pins::IMAGE_4_APPLICATION_PCRS;
        let mut second = first;
        second[0].1 = [0x33; 32];
        let expected = [("first", &first), ("second", &second)];
        let valid = super::application_profiles_manifest(&expected);
        assert!(super::validate_application_profiles(&valid, &expected).is_ok());
        let mut swapped = valid.clone();
        swapped[0].registers = valid[1].registers.clone();
        swapped[1].registers = valid[0].registers.clone();
        assert!(super::validate_application_profiles(&swapped, &expected).is_err());
        let mut changed = valid.clone();
        changed[0].registers[0].sha256 = "00".repeat(32);
        assert!(super::validate_application_profiles(&changed, &expected).is_err());
        assert!(super::validate_application_profiles(&valid[..1], &expected).is_err());
        let mut extra = valid.clone();
        extra.push(valid[0].clone());
        assert!(super::validate_application_profiles(&extra, &expected).is_err());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn generated_identity_profile_mutations_fail_actual_admission() {
        let native = tempdir().unwrap();
        let resources = tempdir().unwrap();
        setup_fixture_roots(native.path(), resources.path());
        let (bytes, receipt) = rendered(native.path(), resources.path());
        admit_android_service_tools(native.path(), resources.path(), &bytes, &receipt).unwrap();
        for mutation in 0..4 {
            let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let profiles = value["identity"]["application_profiles"]
                .as_array_mut()
                .unwrap();
            match mutation {
                0 => {
                    profiles.pop();
                }
                1 => {
                    profiles[0]["pcr_sha256"] = "00".repeat(32).into();
                }
                2 => {
                    profiles[0]["registers"][0]["sha256"] = "00".repeat(32).into();
                }
                _ => {
                    profiles.push(profiles[0].clone());
                }
            }
            let error = admit_android_service_tools(
                native.path(),
                resources.path(),
                &serde_json::to_vec(&value).unwrap(),
                &receipt,
            )
            .unwrap_err();
            assert!(matches!(
                error,
                AndroidServiceToolsRefusal::PinMismatch {
                    field: "appraisal-policy"
                }
            ));
        }
    }

    use super::*;
    use tempfile::tempdir;

    fn shared_elf(needed: &[&str], soname: Option<&str>, runpath: Option<&str>) -> Vec<u8> {
        solstone_core_distribution::elf::fixture_shared_needed(
            solstone_core_distribution::elf::machine_aarch64(),
            needed,
            soname,
            runpath,
        )
    }

    fn setup_fixture_roots(
        native_dir: &Path,
        resource_dir: &Path,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<Vec<u8>>) {
        let restic_bytes = b"\x7fELFdummy-restic-binary".to_vec();
        let rclone_bytes = b"\x7fELFdummy-rclone-binary".to_vec();
        let nvattest_bytes = shared_elf(super::NVATTEST_CLI_NEEDED, None, None);
        let nvat_bytes = shared_elf(
            super::NVAT_LIBRARY_NEEDED,
            Some(ANDROID_NVAT_FILENAME),
            None,
        );
        fs::write(native_dir.join(ANDROID_RESTIC_FILENAME), &restic_bytes).unwrap();
        fs::write(native_dir.join(ANDROID_RCLONE_FILENAME), &rclone_bytes).unwrap();
        fs::write(native_dir.join(ANDROID_NVATTEST_FILENAME), &nvattest_bytes).unwrap();
        fs::write(native_dir.join(ANDROID_NVAT_FILENAME), &nvat_bytes).unwrap();

        let ca_bytes = solstone_core_distribution::read_test_ca_bundle_pem().unwrap();
        fs::write(resource_dir.join(ANDROID_CA_BUNDLE_FILENAME), &ca_bytes).unwrap();

        let mut rim_bytes = Vec::new();
        for rim in crate::nvgpu::ManifestSet::QUALIFIED_595_71_05.entries() {
            let bytes = rim.xml().to_vec();
            fs::write(resource_dir.join(rim.filename()), &bytes).unwrap();
            rim_bytes.push(bytes);
        }

        (restic_bytes, rclone_bytes, ca_bytes, rim_bytes)
    }

    fn artifact(component: &str, bytes: &[u8]) -> serde_json::Value {
        serde_json::json!({
            "component": component,
            "path": component,
            "sha256": super::sha256_hex(bytes),
            "size_bytes": bytes.len(),
        })
    }

    fn current_native_inputs() -> serde_json::Value {
        let mut inputs = serde_json::Map::new();
        for name in ANDROID_NATIVE_RECEIPT_INPUTS {
            let pin = solstone_core_distribution::mobile_input_pin(name);
            let revision = if *name == "nvattest" {
                solstone_core_distribution::NVATTEST_SDK_REVISION
            } else {
                pin.revision
            };
            inputs.insert(
                (*name).to_owned(),
                serde_json::json!({
                    "revision": revision,
                    "sha256": pin.archive_sha256,
                }),
            );
        }
        serde_json::Value::Object(inputs)
    }

    fn current_native_patches() -> serde_json::Value {
        let regorus = solstone_core_distribution::regorus_build_revision_pin();
        serde_json::json!([
            {
                "component": "nvattest",
                "sha256": super::sha256_hex(super::MOBILE_NVATTEST_PATCH),
            },
            {
                "component": "regorus-build-revision",
                "revision": regorus.revision,
                "original_sha256": regorus.original_sha256,
                "patched_sha256": regorus.patched_sha256,
            }
        ])
    }

    fn receipt_for_tree(native_dir: &Path, resource_dir: &Path) -> Vec<u8> {
        let restic = fs::read(native_dir.join(ANDROID_RESTIC_FILENAME)).unwrap();
        let rclone = fs::read(native_dir.join(ANDROID_RCLONE_FILENAME)).unwrap();
        let nvattest = fs::read(native_dir.join(ANDROID_NVATTEST_FILENAME)).unwrap();
        let nvat = fs::read(native_dir.join(ANDROID_NVAT_FILENAME)).unwrap();
        let ca = fs::read(resource_dir.join(ANDROID_CA_BUNDLE_FILENAME)).unwrap();
        serde_json::to_vec(&serde_json::json!({
            "platform": "android",
            "scope": ["restic", "rclone", "nvattest"],
            "complete": true,
            "status": "passed",
            "exit": 0,
            "driver_sha256": super::sha256_hex(super::MOBILE_NATIVE_DRIVER),
            "inputs_sha256": super::sha256_hex(super::MOBILE_NATIVE_INPUTS),
            "inputs": current_native_inputs(),
            "patches": current_native_patches(),
            "artifacts": [
                artifact("restic-cli", &restic),
                artifact("rclone-cli", &rclone),
                artifact("nvattest-cli", &nvattest),
                artifact("nvattest-library", &nvat),
                artifact("nvattest-ca-bundle", &ca),
            ],
        }))
        .unwrap()
    }

    fn rendered(native_dir: &Path, resource_dir: &Path) -> (Vec<u8>, Vec<u8>) {
        let receipt = receipt_for_tree(native_dir, resource_dir);
        let manifest =
            render_android_service_tools_manifest(native_dir, resource_dir, &receipt).unwrap();
        (manifest, receipt)
    }

    #[test]
    fn successful_service_tool_subset_keeps_full_stack_incomplete() {
        let native = tempdir().unwrap();
        let resources = tempdir().unwrap();
        setup_fixture_roots(native.path(), resources.path());
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&receipt_for_tree(native.path(), resources.path())).unwrap();
        receipt["complete"] = false.into();
        let bytes = serde_json::to_vec(&receipt).unwrap();
        let manifest =
            render_android_service_tools_manifest(native.path(), resources.path(), &bytes).unwrap();
        let admitted =
            admit_android_service_tools(native.path(), resources.path(), &manifest, &bytes)
                .unwrap();
        admitted.revalidate().unwrap();

        for (field, value, code) in [
            (
                "driver_sha256",
                serde_json::json!("0".repeat(64)),
                "pin-mismatch",
            ),
            ("status", serde_json::json!("failed"), "receipt-invalid"),
            ("exit", serde_json::json!(7), "receipt-invalid"),
            ("complete", serde_json::Value::Null, "receipt-invalid"),
        ] {
            let previous = receipt[field].take();
            receipt[field] = value;
            let err = render_android_service_tools_manifest(
                native.path(),
                resources.path(),
                &serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap_err();
            assert_eq!(err.code(), code);
            receipt[field] = previous;
        }

        for scope in [
            serde_json::json!(["restic", "rclone"]),
            serde_json::json!(["restic", "rclone", "restic"]),
            serde_json::json!(["restic", "rclone", "nvattest", "rust"]),
            serde_json::json!(["restic", "rclone", null]),
            serde_json::Value::Null,
        ] {
            receipt["scope"] = scope;
            let err = render_android_service_tools_manifest(
                native.path(),
                resources.path(),
                &serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap_err();
            assert_eq!(err.code(), "receipt-invalid");
        }
    }

    #[test]
    fn appraisal_staging_requires_installed_authority() {
        let err = create_android_appraisal_staging().unwrap_err();
        assert_eq!(err.code(), "member-missing");
        assert!(err.role().is_none());
    }

    #[test]
    fn producer_and_admit_roundtrip_valid_tree() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());

        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());
        let admitted = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &manifest_bytes,
            &receipt,
        )
        .unwrap();

        assert_eq!(admitted.native_library_dir(), temp_native.path());
        assert_eq!(admitted.resource_root(), temp_resource.path());
        assert_eq!(
            admitted
                .resolve_tool(AndroidServiceToolRole::Restic)
                .unwrap(),
            temp_native.path().join(ANDROID_RESTIC_FILENAME)
        );
        assert_eq!(
            admitted
                .resolve_tool(AndroidServiceToolRole::Rclone)
                .unwrap(),
            temp_native.path().join(ANDROID_RCLONE_FILENAME)
        );
        admitted.revalidate().unwrap();
    }

    #[test]
    fn header_refusals_check_codes() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();

        // Wrong schema
        val["schema"] = "solstone.other.v1".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "wrong-schema");

        // Wrong product
        val["schema"] = ANDROID_SERVICE_TOOLS_SCHEMA.into();
        val["product"] = "other-product".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "wrong-product");

        // Wrong version
        val["product"] = PRODUCT.into();
        val["version"] = "99.99.99".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "wrong-version");

        // Wrong target
        val["version"] = COMPILED_VERSION.into();
        val["target"] = "linux-x86_64".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "wrong-target");
    }

    #[test]
    fn member_missing_and_changed_refusals() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());

        // Tamper with restic
        fs::write(
            temp_native.path().join(ANDROID_RESTIC_FILENAME),
            b"tampered-bytes",
        )
        .unwrap();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &manifest_bytes,
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "member-changed");
        assert_eq!(err.role(), Some(AndroidServiceToolRole::Restic));

        // Delete restic
        fs::remove_file(temp_native.path().join(ANDROID_RESTIC_FILENAME)).unwrap();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &manifest_bytes,
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "member-missing");
        assert_eq!(err.role(), Some(AndroidServiceToolRole::Restic));
    }

    #[test]
    fn role_validation_and_unknown_role() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();

        // Assign unknown role nvattest
        val["files"][0]["role"] = "other-tool".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "unknown-role");
    }

    #[test]
    fn pin_mismatches_refuse_with_field_id() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());

        // sdk-revision mismatch
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["identity"]["sdk_revision"] = "bad-sdk".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        match err {
            AndroidServiceToolsRefusal::PinMismatch { field } => {
                assert_eq!(field, "sdk-revision");
            }
            _ => panic!("expected PinMismatch"),
        }

        // regorus-revision mismatch
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["identity"]["regorus_revision"] = "bad-regorus".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        match err {
            AndroidServiceToolsRefusal::PinMismatch { field } => {
                assert_eq!(field, "regorus-revision");
            }
            _ => panic!("expected PinMismatch"),
        }

        // ca-bundle mismatch in identity
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["identity"]["ca_bundle_sha256"] = "0".repeat(64).into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        match err {
            AndroidServiceToolsRefusal::PinMismatch { field } => {
                assert_eq!(field, "ca-bundle");
            }
            _ => panic!("expected PinMismatch"),
        }

        // rim mismatch in identity
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["identity"]["rims"][0]["sha256"] = "0".repeat(64).into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        match err {
            AndroidServiceToolsRefusal::PinMismatch { field } => {
                assert_eq!(field, "rim");
            }
            _ => panic!("expected PinMismatch"),
        }

        // restic-revision mismatch in identity
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["identity"]["restic"]["revision"] = "v999.0.0".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        match err {
            AndroidServiceToolsRefusal::PinMismatch { field } => {
                assert_eq!(field, "restic-revision");
            }
            _ => panic!("expected PinMismatch"),
        }

        // rclone-archive mismatch in identity
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["identity"]["rclone"]["archive_sha256"] = "0".repeat(64).into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        match err {
            AndroidServiceToolsRefusal::PinMismatch { field } => {
                assert_eq!(field, "rclone-archive");
            }
            _ => panic!("expected PinMismatch"),
        }

        // pcr_sha256_pins mismatch -> appraisal-policy
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["identity"]["pcr_sha256_pins"][0] = "0".repeat(64).into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        match err {
            AndroidServiceToolsRefusal::PinMismatch { field } => {
                assert_eq!(field, "appraisal-policy");
            }
            _ => panic!("expected PinMismatch"),
        }

        // rclone-revision mismatch in identity
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["identity"]["rclone"]["revision"] = "v0.0.0".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        match err {
            AndroidServiceToolsRefusal::PinMismatch { field } => {
                assert_eq!(field, "rclone-revision");
            }
            _ => panic!("expected PinMismatch"),
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn assert_receipt_pin(
        native: &Path,
        resource: &Path,
        manifest: &[u8],
        receipt: &[u8],
        field: &str,
    ) {
        for err in [
            render_android_service_tools_manifest(native, resource, receipt).unwrap_err(),
            admit_android_service_tools(native, resource, manifest, receipt).unwrap_err(),
        ] {
            assert_eq!(err.code(), "pin-mismatch");
            match err {
                AndroidServiceToolsRefusal::PinMismatch { field: got } => {
                    assert_eq!(got, field);
                }
                _ => panic!("expected PinMismatch"),
            }
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn native_receipt_pins_refuse_when_stale_or_missing() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let good = receipt_for_tree(temp_native.path(), temp_resource.path());
        let (manifest, _) = rendered(temp_native.path(), temp_resource.path());

        for name in ANDROID_NATIVE_RECEIPT_INPUTS {
            let (revision_field, archive_field) = match *name {
                "restic" => ("restic-revision", "restic-archive"),
                "rclone" => ("rclone-revision", "rclone-archive"),
                "nvattest" => ("sdk-revision", "nvattest-archive"),
                other => (other, other),
            };
            assert_eq!(receipt_input_fields(name), (revision_field, archive_field));

            let mut stale_revision: serde_json::Value = serde_json::from_slice(&good).unwrap();
            stale_revision["inputs"][name]["revision"] = "stale-revision".into();
            assert_receipt_pin(
                temp_native.path(),
                temp_resource.path(),
                &manifest,
                &serde_json::to_vec(&stale_revision).unwrap(),
                revision_field,
            );

            let mut missing: serde_json::Value = serde_json::from_slice(&good).unwrap();
            missing["inputs"].as_object_mut().unwrap().remove(*name);
            assert_receipt_pin(
                temp_native.path(),
                temp_resource.path(),
                &manifest,
                &serde_json::to_vec(&missing).unwrap(),
                revision_field,
            );

            let mut stale_archive: serde_json::Value = serde_json::from_slice(&good).unwrap();
            stale_archive["inputs"][name]["sha256"] = "0".repeat(64).into();
            assert_receipt_pin(
                temp_native.path(),
                temp_resource.path(),
                &manifest,
                &serde_json::to_vec(&stale_archive).unwrap(),
                archive_field,
            );
        }

        let mut patches_only_nvattest: serde_json::Value = serde_json::from_slice(&good).unwrap();
        patches_only_nvattest["patches"] = serde_json::json!([{
            "component": "nvattest",
            "sha256": super::sha256_hex(super::MOBILE_NVATTEST_PATCH),
        }]);
        assert_receipt_pin(
            temp_native.path(),
            temp_resource.path(),
            &manifest,
            &serde_json::to_vec(&patches_only_nvattest).unwrap(),
            "regorus-build-revision",
        );

        for (key, value) in [
            (
                "revision",
                serde_json::Value::String("stale-revision".to_owned()),
            ),
            ("original_sha256", serde_json::Value::String("0".repeat(64))),
            ("patched_sha256", serde_json::Value::String("1".repeat(64))),
        ] {
            let mut stale: serde_json::Value = serde_json::from_slice(&good).unwrap();
            for entry in stale["patches"].as_array_mut().unwrap() {
                if entry["component"] == "regorus-build-revision" {
                    entry[key] = value.clone();
                }
            }
            assert_receipt_pin(
                temp_native.path(),
                temp_resource.path(),
                &manifest,
                &serde_json::to_vec(&stale).unwrap(),
                "regorus-build-revision",
            );
        }

        let mut duplicate: serde_json::Value = serde_json::from_slice(&good).unwrap();
        let regorus_entry = duplicate["patches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["component"] == "regorus-build-revision")
            .unwrap()
            .clone();
        duplicate["patches"]
            .as_array_mut()
            .unwrap()
            .push(regorus_entry);
        assert_receipt_pin(
            temp_native.path(),
            temp_resource.path(),
            &manifest,
            &serde_json::to_vec(&duplicate).unwrap(),
            "regorus-build-revision",
        );

        let mut unrelated: serde_json::Value = serde_json::from_slice(&good).unwrap();
        for name in ["llama", "openssl", "nvattest-zlib", "go-ios"] {
            unrelated["inputs"][name] = serde_json::json!({
                "revision": "stale-unrelated",
                "sha256": "2".repeat(64),
            });
        }
        let curl_sha = unrelated["inputs"]["nvattest-curl"]["sha256"]
            .as_str()
            .unwrap()
            .to_ascii_uppercase();
        unrelated["inputs"]["nvattest-curl"]["sha256"] = curl_sha.into();
        for entry in unrelated["patches"].as_array_mut().unwrap() {
            if entry["component"] == "regorus-build-revision" {
                let original = entry["original_sha256"]
                    .as_str()
                    .unwrap()
                    .to_ascii_uppercase();
                entry["original_sha256"] = original.into();
            }
        }
        let unrelated_bytes = serde_json::to_vec(&unrelated).unwrap();
        render_android_service_tools_manifest(
            temp_native.path(),
            temp_resource.path(),
            &unrelated_bytes,
        )
        .unwrap();
        admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &manifest,
            &unrelated_bytes,
        )
        .unwrap();
    }

    #[test]
    fn path_traversal_and_absolute_paths_refuse_unsafe_path() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());

        // Relative path with ..
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["files"][0]["path"] = "../librestic.so".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "unsafe-path");

        // Absolute path
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["files"][0]["path"] = "/data/app/librestic.so".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "unsafe-path");
    }

    #[test]
    fn duplicate_role_refuses() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["files"][1]["role"] = "restic".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "duplicate-role");
        assert_eq!(err.role(), Some(AndroidServiceToolRole::Restic));
    }

    #[test]
    fn duplicate_path_refuses() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["files"][1]["path"] = "librestic.so".into();
        val["files"][1]["role"] = "rclone".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "duplicate-path");
    }

    #[test]
    fn symlink_root_at_admit_refuses_unsafe_path() {
        let real_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(real_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(real_native.path(), temp_resource.path());

        let sym_parent = tempdir().unwrap();
        let sym_native = sym_parent.path().join("sym_native");
        #[cfg(unix)]
        std::os::unix::fs::symlink(real_native.path(), &sym_native).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(real_native.path(), &sym_native).unwrap();

        let err = admit_android_service_tools(
            &sym_native,
            temp_resource.path(),
            &manifest_bytes,
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "unsafe-path");
    }

    #[test]
    fn symlink_file_at_admit_refuses_unsafe_path() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        let (restic_bytes, ..) = setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());

        let other = tempdir().unwrap();
        let real_file = other.path().join("real_restic.so");
        fs::write(&real_file, &restic_bytes).unwrap();
        fs::remove_file(temp_native.path().join(ANDROID_RESTIC_FILENAME)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, temp_native.path().join(ANDROID_RESTIC_FILENAME))
            .unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(
            &real_file,
            temp_native.path().join(ANDROID_RESTIC_FILENAME),
        )
        .unwrap();

        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &manifest_bytes,
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "unsafe-path");
    }

    // Root identity is the directory's device and inode, which only Unix
    // hosts have; Android, the one place these tools are admitted, is one.
    #[cfg(unix)]
    #[test]
    fn substituted_native_root_refuses_root_substituted() {
        let parent = tempdir().unwrap();
        let native_dir = parent.path().join("native");
        fs::create_dir(&native_dir).unwrap();
        let resource_dir = tempdir().unwrap();
        let (restic_bytes, rclone_bytes, ..) =
            setup_fixture_roots(&native_dir, resource_dir.path());
        let (manifest_bytes, receipt) = rendered(&native_dir, resource_dir.path());
        let admitted = admit_android_service_tools(
            &native_dir,
            resource_dir.path(),
            &manifest_bytes,
            &receipt,
        )
        .unwrap();

        // Keep the original inode live while substituting a new directory.
        fs::rename(&native_dir, parent.path().join("original-native")).unwrap();
        fs::create_dir(&native_dir).unwrap();
        fs::write(native_dir.join(ANDROID_RESTIC_FILENAME), &restic_bytes).unwrap();
        fs::write(native_dir.join(ANDROID_RCLONE_FILENAME), &rclone_bytes).unwrap();

        let err_resolve = admitted
            .resolve_tool(AndroidServiceToolRole::Restic)
            .unwrap_err();
        assert_eq!(err_resolve.code(), "root-substituted");

        let err_reval = admitted.revalidate().unwrap_err();
        assert_eq!(err_reval.code(), "root-substituted");
    }

    // Root identity is the directory's device and inode, which only Unix
    // hosts have; Android, the one place these tools are admitted, is one.
    #[cfg(unix)]
    #[test]
    fn substituted_resource_root_refuses_root_substituted() {
        let native_dir = tempdir().unwrap();
        let parent = tempdir().unwrap();
        let resource_dir = parent.path().join("resource");
        fs::create_dir(&resource_dir).unwrap();
        setup_fixture_roots(native_dir.path(), &resource_dir);
        let (manifest_bytes, receipt) = rendered(native_dir.path(), &resource_dir);
        let admitted = admit_android_service_tools(
            native_dir.path(),
            &resource_dir,
            &manifest_bytes,
            &receipt,
        )
        .unwrap();

        // Keep the original inode live while substituting a new directory.
        fs::rename(&resource_dir, parent.path().join("original-resource")).unwrap();
        fs::create_dir(&resource_dir).unwrap();
        let ca_bytes = solstone_core_distribution::read_test_ca_bundle_pem().unwrap();
        fs::write(resource_dir.join(ANDROID_CA_BUNDLE_FILENAME), &ca_bytes).unwrap();
        for rim in crate::nvgpu::ManifestSet::QUALIFIED_595_71_05.entries() {
            fs::write(resource_dir.join(rim.filename()), rim.xml()).unwrap();
        }

        let err_reval = admitted.revalidate().unwrap_err();
        assert_eq!(err_reval.code(), "root-substituted");
    }

    #[test]
    fn file_replaced_with_symlink_or_truncated_after_admit() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        let (restic_bytes, ..) = setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());
        let admitted = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &manifest_bytes,
            &receipt,
        )
        .unwrap();

        // Symlink replacement
        let other = tempdir().unwrap();
        let real_file = other.path().join("real_restic.so");
        fs::write(&real_file, &restic_bytes).unwrap();
        fs::remove_file(temp_native.path().join(ANDROID_RESTIC_FILENAME)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_file, temp_native.path().join(ANDROID_RESTIC_FILENAME))
            .unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(
            &real_file,
            temp_native.path().join(ANDROID_RESTIC_FILENAME),
        )
        .unwrap();

        let err_sym = admitted
            .resolve_tool(AndroidServiceToolRole::Restic)
            .unwrap_err();
        assert_eq!(err_sym.code(), "unsafe-path");

        // Truncate file to a proper prefix
        fs::remove_file(temp_native.path().join(ANDROID_RESTIC_FILENAME)).unwrap();
        fs::write(
            temp_native.path().join(ANDROID_RESTIC_FILENAME),
            &restic_bytes[..restic_bytes.len() / 2],
        )
        .unwrap();

        let err_trunc = admitted
            .resolve_tool(AndroidServiceToolRole::Restic)
            .unwrap_err();
        assert_eq!(err_trunc.code(), "member-changed");
        assert_eq!(err_trunc.role(), Some(AndroidServiceToolRole::Restic));
    }

    #[test]
    fn role_on_resource_and_unexpected_seventh_resource_refuse_manifest_invalid() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());

        // Role on resource path
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val["files"][4]["role"] = "restic".into();
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "manifest-invalid");

        // Seventh unexpected resource file
        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        let extra = serde_json::json!({
            "root": "resource",
            "path": "unexpected.xml",
            "bytes": 10,
            "sha256": "0".repeat(64)
        });
        val["files"].as_array_mut().unwrap().push(extra);
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "manifest-invalid");
    }

    #[test]
    fn omitted_identity_refuses_manifest_invalid() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let (manifest_bytes, receipt) = rendered(temp_native.path(), temp_resource.path());

        let mut val: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        val.as_object_mut().unwrap().remove("identity");
        let err = admit_android_service_tools(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&val).unwrap(),
            &receipt,
        )
        .unwrap_err();
        assert_eq!(err.code(), "manifest-invalid");
    }

    #[test]
    fn closure_refuses_unexpected_needed_missing_soname_and_loader_path() {
        fn expect_load(native: &Path, resource: &Path, needed: &str, tag: &str, absent: &str) {
            let receipt = receipt_for_tree(native, resource);
            let err =
                render_android_service_tools_manifest(native, resource, &receipt).unwrap_err();
            assert_eq!(err.code(), "load-unrepresentable");
            match err {
                AndroidServiceToolsRefusal::LoadUnrepresentable {
                    needed: got_needed,
                    tag: got_tag,
                    absent: got_absent,
                } => {
                    assert_eq!(got_needed, needed);
                    assert_eq!(got_tag, tag);
                    assert_eq!(got_absent, absent);
                }
                other => panic!("expected LoadUnrepresentable, got {other:?}"),
            }
        }

        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let mut needed = super::NVATTEST_CLI_NEEDED.to_vec();
        needed.push("libfoo.so");
        fs::write(
            temp_native.path().join(ANDROID_NVATTEST_FILENAME),
            shared_elf(&needed, None, None),
        )
        .unwrap();
        expect_load(
            temp_native.path(),
            temp_resource.path(),
            "libfoo.so",
            "DT_NEEDED",
            "allowlisted-os-library",
        );

        setup_fixture_roots(temp_native.path(), temp_resource.path());
        fs::write(
            temp_native.path().join(ANDROID_NVAT_FILENAME),
            shared_elf(super::NVAT_LIBRARY_NEEDED, None, None),
        )
        .unwrap();
        expect_load(
            temp_native.path(),
            temp_resource.path(),
            ANDROID_NVAT_FILENAME,
            "DT_SONAME",
            ANDROID_NVAT_FILENAME,
        );

        setup_fixture_roots(temp_native.path(), temp_resource.path());
        fs::write(
            temp_native.path().join(ANDROID_NVATTEST_FILENAME),
            shared_elf(super::NVATTEST_CLI_NEEDED, None, Some("$ORIGIN")),
        )
        .unwrap();
        expect_load(
            temp_native.path(),
            temp_resource.path(),
            ANDROID_NVAT_FILENAME,
            "DT_RUNPATH",
            "absent",
        );

        let cli = solstone_core_distribution::elf::parse_elf(&shared_elf(
            super::NVATTEST_CLI_NEEDED,
            None,
            None,
        ))
        .unwrap();
        let mut library = solstone_core_distribution::elf::parse_elf(&shared_elf(
            super::NVAT_LIBRARY_NEEDED,
            Some(ANDROID_NVAT_FILENAME),
            None,
        ))
        .unwrap();
        library.rpath = Some("/data/app/lib".to_owned());
        let err = verifier_load_closure(&cli, &library).unwrap_err();
        assert_eq!(err.code(), "load-unrepresentable");
        match err {
            AndroidServiceToolsRefusal::LoadUnrepresentable {
                needed,
                tag,
                absent,
            } => {
                assert_eq!(needed, ANDROID_NVAT_FILENAME);
                assert_eq!(tag, "DT_RPATH");
                assert_eq!(absent, "absent");
            }
            other => panic!("expected LoadUnrepresentable, got {other:?}"),
        }
    }

    #[test]
    fn receipt_stamp_and_structure_refuse() {
        let temp_native = tempdir().unwrap();
        let temp_resource = tempdir().unwrap();
        setup_fixture_roots(temp_native.path(), temp_resource.path());
        let mut receipt: serde_json::Value =
            serde_json::from_slice(&receipt_for_tree(temp_native.path(), temp_resource.path()))
                .unwrap();
        for artifact in receipt["artifacts"].as_array_mut().unwrap() {
            if artifact["component"] == "restic-cli" {
                artifact["sha256"] = "0".repeat(64).into();
            }
        }
        let err = render_android_service_tools_manifest(
            temp_native.path(),
            temp_resource.path(),
            &serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        assert!(matches!(
            err,
            AndroidServiceToolsRefusal::PinMismatch {
                field: "restic-artifact"
            }
        ));

        let mut driver = receipt_for_tree(temp_native.path(), temp_resource.path());
        let mut value: serde_json::Value = serde_json::from_slice(&driver).unwrap();
        value["driver_sha256"] = "0".repeat(64).into();
        driver = serde_json::to_vec(&value).unwrap();
        let err = render_android_service_tools_manifest(
            temp_native.path(),
            temp_resource.path(),
            &driver,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pin-mismatch");
        assert!(matches!(
            err,
            AndroidServiceToolsRefusal::PinMismatch { field: "driver" }
        ));

        for payload in [
            r#"{"status":"failed","complete":true,"exit":0,"platform":"android"}"#,
            r#"{"status":"passed","complete":false,"exit":0,"platform":"android"}"#,
            r#"{"status":"passed","complete":true,"exit":"0","platform":"android"}"#,
            r#"{"status":"passed","complete":true,"exit":7,"platform":"android"}"#,
        ] {
            let err = render_android_service_tools_manifest(
                temp_native.path(),
                temp_resource.path(),
                payload.as_bytes(),
            )
            .unwrap_err();
            assert_eq!(err.code(), "receipt-invalid");
        }
    }

    #[test]
    fn loader_directory_rejects_separators_and_os_shadows() {
        assert_eq!(
            validate_android_loader_directory(Path::new("relative"))
                .unwrap_err()
                .code(),
            "unsafe-path"
        );
        assert_eq!(
            validate_android_loader_directory(Path::new("/data/app/lib:extra"))
                .unwrap_err()
                .code(),
            "unsafe-path"
        );
        assert_eq!(
            validate_android_loader_directory(Path::new("/data/app/lib\0extra"))
                .unwrap_err()
                .code(),
            "unsafe-path"
        );
        let dir = tempdir().unwrap();
        // A drive-letter path carries the loader's `:` separator, so no Windows
        // directory can be a loader directory.
        #[cfg(windows)]
        assert_eq!(
            validate_android_loader_directory(dir.path())
                .unwrap_err()
                .code(),
            "unsafe-path"
        );
        #[cfg(unix)]
        assert_loader_directory_refuses_os_shadows(dir.path());
    }

    #[cfg(unix)]
    fn assert_loader_directory_refuses_os_shadows(dir: &Path) {
        assert!(validate_android_loader_directory(dir).is_ok());
        fs::write(dir.join("libc.so"), b"shadow").unwrap();
        let err = validate_android_loader_directory(dir).unwrap_err();
        assert_eq!(err.code(), "library-shadow");
        assert!(matches!(
            err,
            AndroidServiceToolsRefusal::LibraryShadow { name: "libc.so" }
        ));
        fs::remove_file(dir.join("libc.so")).unwrap();
        fs::write(dir.join("libz.so"), b"shadow").unwrap();
        let err = validate_android_loader_directory(dir).unwrap_err();
        assert_eq!(err.code(), "library-shadow");
        assert!(matches!(
            err,
            AndroidServiceToolsRefusal::LibraryShadow { name: "libz.so" }
        ));
    }
}
