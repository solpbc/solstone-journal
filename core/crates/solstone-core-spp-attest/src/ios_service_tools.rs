// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unchanged, no edit to the function: `thinking::offered_on_this_platform`,
//! `brain_owner::spp_prerequisite`, `generate_wire::confidential_generate`,
//! `dispatch` `LaneOutcome::ConfidentialEndpoint`,
//! `transcribe::backend::confidential::transcribe`,
//! `fresh::establish_fresh_production_channel`, `refuse_confidential_egress`,
//! `stage::dispatch_after_egress_gate`, `tools/android-runtime` service-tools,
//! `NvattestGpuAppraiser::appraise_installation`. `CompositeVerifier::verify`,
//! `confidential_generate`, and `transcribe` gained no cancellation token.
//!
//! iOS-adapted: `check_nvattest_readiness`,
//! `resolve_nvattest_from_current_exe`, `confidential_verifier_offered`,
//! `confidential_verifier_on_this_platform`,
//! `establish_production_attested_channel_with_clock`.
//!
//! iOS is not a shipped release target.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use solstone_core_installed_payload::sha256_hex;

pub const IOS_HELPER_RELATIVE_PATH: &str = "Extensions/Appraisal.appex";
pub const IOS_HELPER_EXECUTABLE_NAME: &str = "Appraisal";
// Each signed app admits only its own appraisal extension. The owner app is a
// separate identity chosen when the native library is built, never at run time.
#[cfg(not(feature = "ios-owner-app"))]
pub const IOS_BUNDLE_IDENTIFIER: &str = "app.solstone.journal.runtime.dev.gpu-appraisal";
#[cfg(not(feature = "ios-owner-app"))]
pub const IOS_EXTENSION_POINT_HOST: &str = "app.solstone.journal.runtime.dev";
#[cfg(feature = "ios-owner-app")]
pub const IOS_BUNDLE_IDENTIFIER: &str = "app.solstone.journal.owner.dev.gpu-appraisal";
#[cfg(feature = "ios-owner-app")]
pub const IOS_EXTENSION_POINT_HOST: &str = "app.solstone.journal.owner.dev";
pub const IOS_EXTENSION_POINT_NAME: &str = "gpu-appraisal";
const IOS_RIM_DIRECTORY: &str = "RIMs";

const MOBILE_NATIVE_DRIVER: &[u8] = include_bytes!("../../../../scripts/mobile_native_build.py");
const MOBILE_NATIVE_INPUTS: &[u8] = include_bytes!("../../../distribution/mobile/inputs.json");
const MOBILE_NVATTEST_PATCH: &[u8] =
    include_bytes!("../../../distribution/mobile/nvattest-mobile.patch");

const PLATFORM_IOS: u32 = 2;
const CPUTYPE_ARM64: u32 = 0x0100_000c;
const MH_EXECUTE: u32 = 2;
const LC_BUILD_VERSION: u32 = 0x32;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const MAX_IMAGE_SIZE_BYTES: usize = 128 * 1024 * 1024;
const MAX_RECEIPT_SIZE_BYTES: u64 = 1024 * 1024;
const MAX_BUNDLE_MEMBER_SIZE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BUNDLE_MEMBERS: usize = 64;
const MAX_LOAD_COMMANDS: u32 = 512;
const MAX_SIZEOFCMDS_BYTES: usize = 1024 * 1024;

/// Packaged reference integrity manifest digest record from the receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IosPackagedRimEntry {
    pub filename: String,
    pub sha256: String,
}

/// Receipt-bound regular file in the appraisal extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IosPackagedMemberEntry {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Signed companion build receipt for the iOS GPU appraisal helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IosGpuAppraisalReceipt {
    pub status: String,
    pub complete: bool,
    pub exit: i32,
    pub platform: String,
    pub arch: String,
    pub driver_sha256: String,
    pub inputs_sha256: String,
    pub patch_sha256: String,
    pub nvattest_revision: String,
    pub regorus_revision: String,
    pub regorus_original_sha256: String,
    pub regorus_patched_sha256: String,
    pub ca_bundle_sha256: String,
    pub pcr_sha256_pins: Vec<String>,
    pub rims: Vec<IosPackagedRimEntry>,
    pub needed: Vec<String>,
    pub helper_sha256: String,
    pub schema: String,
    pub build_record_sha256: String,
    pub package_members: Vec<IosPackagedMemberEntry>,
    #[serde(default)]
    pub steps: Vec<serde_json::Value>,
}

/// Parsed structural facts of a Mach-O 64-bit executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosMachoFacts {
    pub platform: u32,
    pub cputype: u32,
    pub filetype: u32,
    pub image_size: usize,
    pub ncmds: u32,
    pub sizeofcmds: usize,
    pub needed: Vec<String>,
    pub rpaths: Vec<String>,
    pub install_name: Option<String>,
    pub upward_dylibs: Vec<String>,
    pub lazy_dylibs: Vec<String>,
    pub dyld_environments: Vec<String>,
}

/// A scanned bundle member file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosBundleMember {
    pub relative_path: String,
    pub is_regular_file: bool,
    pub is_symlink: bool,
    pub size_bytes: u64,
    pub sha256: String,
    pub macho_facts: Option<IosMachoFacts>,
}

/// Admitted iOS GPU verifier authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedIosVerifier {
    pub bundle_path: PathBuf,
    pub helper_path: PathBuf,
    pub receipt: IosGpuAppraisalReceipt,
    pub bundle_id: String,
    pub extension_point_host: String,
    pub extension_point_name: String,
    pub helper_sha256: String,
    pub needed: Vec<String>,
}

/// Fail-closed refusal reasons for iOS verifier admission.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IosVerifierRefusal {
    #[error("ios verifier unavailable: {0}")]
    Unavailable(String),
    #[error("ios verifier already installed in process")]
    AlreadyInstalled,
    #[error("bad platform: expected ios, found {0}")]
    BadPlatform(String),
    #[error("bad architecture: expected arm64, found {0}")]
    BadArchitecture(String),
    #[error("receipt invalid: {0}")]
    ReceiptInvalid(String),
    #[error("pin mismatch: {0}")]
    PinMismatch(String),
    #[error("substituted identity: {0}")]
    SubstitutedIdentity(String),
    #[error("closure mismatch: {0}")]
    ClosureMismatch(String),
    #[error("integrity failed: {0}")]
    IntegrityFailed(String),
}

/// Maps an iOS verifier refusal to the stable reason code expected by SPP-RATLS.
#[must_use]
pub fn ios_verifier_refusal_reason(refusal: &IosVerifierRefusal) -> &'static str {
    match refusal {
        IosVerifierRefusal::IntegrityFailed(_)
        | IosVerifierRefusal::ClosureMismatch(_)
        | IosVerifierRefusal::PinMismatch(_)
        | IosVerifierRefusal::SubstitutedIdentity(_) => "nvattest_integrity_failed",
        IosVerifierRefusal::Unavailable(_)
        | IosVerifierRefusal::AlreadyInstalled
        | IosVerifierRefusal::BadPlatform(_)
        | IosVerifierRefusal::BadArchitecture(_)
        | IosVerifierRefusal::ReceiptInvalid(_) => "nvattest_unavailable",
    }
}

/// Parses Mach-O 64-bit header and load commands for iOS admission checks.
pub fn inspect_ios_macho(bytes: &[u8]) -> Result<IosMachoFacts, IosVerifierRefusal> {
    if bytes.len() > MAX_IMAGE_SIZE_BYTES {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho image size {} exceeds cap {}",
            bytes.len(),
            MAX_IMAGE_SIZE_BYTES
        )));
    }
    if bytes.len() < 32 {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "truncated mach-o header".into(),
        ));
    }
    let ncmds = u32::from_le_bytes(
        bytes[16..20]
            .try_into()
            .map_err(|_| IosVerifierRefusal::IntegrityFailed("header slice error".into()))?,
    );
    let sizeofcmds = u32::from_le_bytes(
        bytes[20..24]
            .try_into()
            .map_err(|_| IosVerifierRefusal::IntegrityFailed("header slice error".into()))?,
    ) as usize;

    if ncmds > MAX_LOAD_COMMANDS {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho ncmds {ncmds} exceeds cap {MAX_LOAD_COMMANDS}"
        )));
    }
    if sizeofcmds > MAX_SIZEOFCMDS_BYTES {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho sizeofcmds {sizeofcmds} exceeds cap {MAX_SIZEOFCMDS_BYTES}"
        )));
    }

    // Call distribution's parse_macho for standard fields
    let info = solstone_core_distribution::macho::parse_macho(bytes)
        .map_err(|e| IosVerifierRefusal::IntegrityFailed(format!("parse_macho failed: {e}")))?;

    // Manually inspect LC_BUILD_VERSION (0x32) command at offset 8 for platform
    let commands = bytes
        .get(32..32 + sizeofcmds)
        .ok_or_else(|| IosVerifierRefusal::IntegrityFailed("truncated load commands".into()))?;

    let mut cursor = 0usize;
    let mut detected_platform: Option<u32> = None;
    for _ in 0..ncmds {
        if cursor + 8 > commands.len() {
            return Err(IosVerifierRefusal::IntegrityFailed(
                "truncated load command".into(),
            ));
        }
        let cmd = u32::from_le_bytes(
            commands[cursor..cursor + 4]
                .try_into()
                .map_err(|_| IosVerifierRefusal::IntegrityFailed("cmd slice error".into()))?,
        );
        let cmdsize = u32::from_le_bytes(
            commands[cursor + 4..cursor + 8]
                .try_into()
                .map_err(|_| IosVerifierRefusal::IntegrityFailed("cmdsize slice error".into()))?,
        ) as usize;
        if cmdsize < 8 || cursor + cmdsize > commands.len() {
            return Err(IosVerifierRefusal::IntegrityFailed(
                "invalid load command size".into(),
            ));
        }
        if cmd == LC_BUILD_VERSION && cmdsize >= 12 {
            let platform =
                u32::from_le_bytes(commands[cursor + 8..cursor + 12].try_into().map_err(|_| {
                    IosVerifierRefusal::IntegrityFailed("platform slice error".into())
                })?);
            detected_platform = Some(platform);
        }
        cursor += cmdsize;
    }

    let platform = detected_platform
        .ok_or_else(|| IosVerifierRefusal::IntegrityFailed("missing LC_BUILD_VERSION".into()))?;

    Ok(IosMachoFacts {
        platform,
        cputype: info.cputype,
        filetype: info.filetype,
        image_size: bytes.len(),
        ncmds,
        sizeofcmds,
        needed: info.needed,
        rpaths: info.rpaths,
        install_name: info.install_name,
        upward_dylibs: info.upward_dylibs,
        lazy_dylibs: info.lazy_dylibs,
        dyld_environments: info.dyld_environments,
    })
}

/// Hashes all Mach-O bytes except the embedded Apple code-signature blob.
///
/// The blob covers the receipt through CodeResources, so including it in a
/// digest stored inside that receipt would be self-referential. The load
/// command remains covered, as do every code and data byte.
pub fn ios_macho_content_sha256(bytes: &[u8]) -> Result<String, IosVerifierRefusal> {
    if bytes.len() < 32 || bytes.len() > MAX_IMAGE_SIZE_BYTES {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "invalid mach-o size for content digest".into(),
        ));
    }
    if u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .map_err(|_| IosVerifierRefusal::IntegrityFailed("magic slice error".into()))?,
    ) != 0xfeed_facf
    {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "helper is not a little-endian mach-o 64 image".into(),
        ));
    }
    let ncmds = u32::from_le_bytes(
        bytes[16..20]
            .try_into()
            .map_err(|_| IosVerifierRefusal::IntegrityFailed("header slice error".into()))?,
    );
    let sizeofcmds = u32::from_le_bytes(
        bytes[20..24]
            .try_into()
            .map_err(|_| IosVerifierRefusal::IntegrityFailed("header slice error".into()))?,
    ) as usize;
    if ncmds > MAX_LOAD_COMMANDS || sizeofcmds > MAX_SIZEOFCMDS_BYTES {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "mach-o command bounds exceeded for content digest".into(),
        ));
    }
    let commands = bytes
        .get(32..32 + sizeofcmds)
        .ok_or_else(|| IosVerifierRefusal::IntegrityFailed("truncated load commands".into()))?;
    let mut cursor = 0usize;
    let mut signature = None;
    for _ in 0..ncmds {
        let header = commands
            .get(cursor..cursor + 8)
            .ok_or_else(|| IosVerifierRefusal::IntegrityFailed("truncated load command".into()))?;
        let cmd =
            u32::from_le_bytes(header[0..4].try_into().map_err(|_| {
                IosVerifierRefusal::IntegrityFailed("load command slice error".into())
            })?);
        let cmdsize = u32::from_le_bytes(header[4..8].try_into().map_err(|_| {
            IosVerifierRefusal::IntegrityFailed("load command size slice error".into())
        })?) as usize;
        if cmdsize < 8 || cursor + cmdsize > commands.len() {
            return Err(IosVerifierRefusal::IntegrityFailed(
                "invalid load command size".into(),
            ));
        }
        if cmd == LC_CODE_SIGNATURE {
            if cmdsize < 16 || signature.is_some() {
                return Err(IosVerifierRefusal::IntegrityFailed(
                    "invalid code-signature load command".into(),
                ));
            }
            let dataoff =
                u32::from_le_bytes(commands[cursor + 8..cursor + 12].try_into().map_err(|_| {
                    IosVerifierRefusal::IntegrityFailed("code-signature offset slice error".into())
                })?) as usize;
            let datasize =
                u32::from_le_bytes(commands[cursor + 12..cursor + 16].try_into().map_err(|_| {
                    IosVerifierRefusal::IntegrityFailed("code-signature size slice error".into())
                })?) as usize;
            let end = dataoff.checked_add(datasize).ok_or_else(|| {
                IosVerifierRefusal::IntegrityFailed("code-signature range overflow".into())
            })?;
            if dataoff < 32 + sizeofcmds || end > bytes.len() {
                return Err(IosVerifierRefusal::IntegrityFailed(
                    "code-signature range is outside mach-o".into(),
                ));
            }
            signature = Some((dataoff, end));
        }
        cursor += cmdsize;
    }
    let (start, end) = signature.ok_or_else(|| {
        IosVerifierRefusal::IntegrityFailed("missing code-signature load command".into())
    })?;
    let mut content = Vec::with_capacity(bytes.len() - (end - start));
    content.extend_from_slice(&bytes[..start]);
    content.extend_from_slice(&bytes[end..]);
    Ok(sha256_hex(&content))
}

/// Pure decision logic for admitting an iOS confidential GPU appraisal helper.
///
/// Validates receipt pins against compiled constants, verifies helper identity
/// and member regularity, and checks Mach-O closure constraints.
pub fn ios_admission_decision(
    receipt: &IosGpuAppraisalReceipt,
    members: &[IosBundleMember],
    bundle_id: &str,
    extension_point_host: &str,
    extension_point_name: &str,
) -> Result<AdmittedIosVerifier, IosVerifierRefusal> {
    // 1. Receipt status and execution metadata
    if receipt.schema != "gpu-appraisal-receipt-v2"
        || receipt.status != "passed"
        || !receipt.complete
        || receipt.exit != 0
    {
        return Err(IosVerifierRefusal::ReceiptInvalid(format!(
            "receipt did not pass: schema={}, status={}, complete={}, exit={}",
            receipt.schema, receipt.status, receipt.complete, receipt.exit
        )));
    }
    if receipt.platform != "ios" {
        return Err(IosVerifierRefusal::BadPlatform(receipt.platform.clone()));
    }
    if receipt.arch != "arm64" {
        return Err(IosVerifierRefusal::BadArchitecture(receipt.arch.clone()));
    }

    // 2. Hash and revision pin comparisons against compiled constants
    let expected_driver_sha = sha256_hex(MOBILE_NATIVE_DRIVER);
    if receipt.driver_sha256 != expected_driver_sha {
        return Err(IosVerifierRefusal::PinMismatch(
            "driver_sha256 does not match compiled mobile_native_build.py".into(),
        ));
    }
    let expected_inputs_sha = sha256_hex(MOBILE_NATIVE_INPUTS);
    if receipt.inputs_sha256 != expected_inputs_sha {
        return Err(IosVerifierRefusal::PinMismatch(
            "inputs_sha256 does not match compiled inputs.json".into(),
        ));
    }
    let expected_patch_sha = sha256_hex(MOBILE_NVATTEST_PATCH);
    if receipt.patch_sha256 != expected_patch_sha {
        return Err(IosVerifierRefusal::PinMismatch(
            "patch_sha256 does not match compiled nvattest-mobile.patch".into(),
        ));
    }

    let expected_nvattest = solstone_core_distribution::NVATTEST_SDK_REVISION;
    let expected_nvattest_pin = solstone_core_distribution::mobile_input_pin("nvattest").revision;
    if receipt.nvattest_revision != expected_nvattest
        || receipt.nvattest_revision != expected_nvattest_pin
    {
        return Err(IosVerifierRefusal::PinMismatch(
            "nvattest_revision does not match compiled SDK revision pin".into(),
        ));
    }

    let regorus_pin = solstone_core_distribution::regorus_build_revision_pin();
    let regorus_input_pin = solstone_core_distribution::mobile_input_pin("nvattest-regorus");
    let pinned_regorus = solstone_core_distribution::pinned_regorus_revision();
    if regorus_pin.revision != regorus_input_pin.revision || regorus_pin.revision != pinned_regorus
    {
        return Err(IosVerifierRefusal::PinMismatch(
            "compiled regorus revisions differ".into(),
        ));
    }
    if receipt.regorus_revision != regorus_pin.revision {
        return Err(IosVerifierRefusal::PinMismatch(
            "regorus_revision does not match compiled regorus revision pin".into(),
        ));
    }
    if !receipt
        .regorus_original_sha256
        .eq_ignore_ascii_case(regorus_pin.original_sha256)
    {
        return Err(IosVerifierRefusal::PinMismatch(
            "regorus_original_sha256 does not match compiled regorus build revision pin".into(),
        ));
    }
    if !receipt
        .regorus_patched_sha256
        .eq_ignore_ascii_case(regorus_pin.patched_sha256)
    {
        return Err(IosVerifierRefusal::PinMismatch(
            "regorus_patched_sha256 does not match compiled regorus build revision pin".into(),
        ));
    }

    if receipt.ca_bundle_sha256 != solstone_core_distribution::pinned_ca_bundle_sha256() {
        return Err(IosVerifierRefusal::PinMismatch(
            "ca_bundle_sha256 does not match compiled CA bundle pin".into(),
        ));
    }

    if receipt.pcr_sha256_pins.as_slice() != crate::pins::PRODUCTION_PCR_SHA256_PINS {
        return Err(IosVerifierRefusal::PinMismatch(
            "pcr_sha256_pins does not match compiled PRODUCTION_PCR_SHA256_PINS".into(),
        ));
    }

    let qualified_rims = crate::nvgpu::ManifestSet::QUALIFIED_595_71_05.entries();
    let receipt_rims = receipt
        .rims
        .iter()
        .map(|rim| (rim.filename.as_str(), rim.sha256.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    if receipt.rims.len() != qualified_rims.len() || receipt_rims.len() != receipt.rims.len() {
        return Err(IosVerifierRefusal::PinMismatch(format!(
            "rim count mismatch: expected {}, got {}",
            qualified_rims.len(),
            receipt.rims.len()
        )));
    }
    for expected in qualified_rims {
        if receipt_rims.get(expected.filename()).copied() != Some(expected.sha256()) {
            return Err(IosVerifierRefusal::PinMismatch(format!(
                "rim mismatch for {}: expected {}, got {:?}",
                expected.filename(),
                expected.sha256(),
                receipt_rims.get(expected.filename())
            )));
        }
    }

    // 3. Discovered extension identity validation
    if bundle_id != IOS_BUNDLE_IDENTIFIER
        || extension_point_host != IOS_EXTENSION_POINT_HOST
        || extension_point_name != IOS_EXTENSION_POINT_NAME
    {
        return Err(IosVerifierRefusal::SubstitutedIdentity(format!(
            "expected {IOS_BUNDLE_IDENTIFIER} on {IOS_EXTENSION_POINT_HOST}:{IOS_EXTENSION_POINT_NAME}, got {bundle_id} on {extension_point_host}:{extension_point_name}"
        )));
    }

    // 4. Bundle member regular file validation and closure completeness
    let helper_exe_path = format!("{IOS_HELPER_RELATIVE_PATH}/{IOS_HELPER_EXECUTABLE_NAME}");
    let receipt_rel_path = format!("{IOS_HELPER_RELATIVE_PATH}/gpu-appraisal-receipt.json");
    let code_resources_rel_path =
        format!("{IOS_HELPER_RELATIVE_PATH}/_CodeSignature/CodeResources");
    let ca_rel_path = format!("{IOS_HELPER_RELATIVE_PATH}/ca-bundle.pem");
    let info_rel_path = format!("{IOS_HELPER_RELATIVE_PATH}/Info.plist");
    let build_record_rel_path = format!("{IOS_HELPER_RELATIVE_PATH}/gpu-appraisal-build.json");

    let mut expected_paths = BTreeSet::new();
    expected_paths.insert(receipt_rel_path.clone());
    expected_paths.insert(code_resources_rel_path.clone());
    let mut packaged = std::collections::BTreeMap::new();
    let mut allowed_members = BTreeSet::from([
        IOS_HELPER_EXECUTABLE_NAME.to_owned(),
        "Info.plist".to_owned(),
        "embedded.mobileprovision".to_owned(),
        "gpu-appraisal-build.json".to_owned(),
        "ca-bundle.pem".to_owned(),
    ]);
    allowed_members.extend(
        qualified_rims
            .iter()
            .map(|rim| format!("{IOS_RIM_DIRECTORY}/{}", rim.filename())),
    );
    for member in &receipt.package_members {
        let path = Path::new(&member.path);
        if path.is_absolute()
            || member.path.is_empty()
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || member.path == "gpu-appraisal-receipt.json"
            || member.path == "_CodeSignature/CodeResources"
            || !allowed_members.contains(&member.path)
            || member.size_bytes > MAX_BUNDLE_MEMBER_SIZE_BYTES
            || !member.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || member.sha256.len() != 64
        {
            return Err(IosVerifierRefusal::ReceiptInvalid(format!(
                "invalid package member record: {}",
                member.path
            )));
        }
        let full = format!("{IOS_HELPER_RELATIVE_PATH}/{}", member.path);
        if packaged.insert(full.clone(), member).is_some() {
            return Err(IosVerifierRefusal::ReceiptInvalid(format!(
                "duplicate package member record: {}",
                member.path
            )));
        }
        expected_paths.insert(full);
    }
    if receipt.package_members.len() > MAX_BUNDLE_MEMBERS {
        return Err(IosVerifierRefusal::ReceiptInvalid(format!(
            "package member count {} exceeds cap {MAX_BUNDLE_MEMBERS}",
            receipt.package_members.len()
        )));
    }
    for required in [
        &helper_exe_path,
        &ca_rel_path,
        &info_rel_path,
        &build_record_rel_path,
    ] {
        if !expected_paths.contains(required) {
            return Err(IosVerifierRefusal::ReceiptInvalid(format!(
                "required package member absent from receipt: {required}"
            )));
        }
    }
    for rim in qualified_rims {
        let path = format!(
            "{IOS_HELPER_RELATIVE_PATH}/{IOS_RIM_DIRECTORY}/{}",
            rim.filename()
        );
        if !expected_paths.contains(&path) {
            return Err(IosVerifierRefusal::ReceiptInvalid(format!(
                "RIM absent from package member receipt: {}",
                rim.filename()
            )));
        }
    }

    let mut found_paths = BTreeSet::new();
    let mut helper_member: Option<&IosBundleMember> = None;

    for member in members {
        if member.is_symlink {
            return Err(IosVerifierRefusal::IntegrityFailed(format!(
                "symlink forbidden: {}",
                member.relative_path
            )));
        }
        if !member.is_regular_file {
            return Err(IosVerifierRefusal::IntegrityFailed(format!(
                "not a regular file: {}",
                member.relative_path
            )));
        }
        if !expected_paths.contains(&member.relative_path) {
            return Err(IosVerifierRefusal::IntegrityFailed(format!(
                "unexpected extra member: {}",
                member.relative_path
            )));
        }
        if !found_paths.insert(member.relative_path.clone()) {
            return Err(IosVerifierRefusal::IntegrityFailed(format!(
                "duplicate member: {}",
                member.relative_path
            )));
        }
        if let Some(expected) = packaged.get(&member.relative_path)
            && (member.sha256 != expected.sha256 || member.size_bytes != expected.size_bytes)
        {
            return Err(IosVerifierRefusal::IntegrityFailed(format!(
                "receipt-bound member drifted: {}",
                member.relative_path
            )));
        }
        if member.relative_path == helper_exe_path {
            helper_member = Some(member);
        } else if member.relative_path == ca_rel_path {
            if member.sha256 != solstone_core_distribution::pinned_ca_bundle_sha256() {
                return Err(IosVerifierRefusal::IntegrityFailed(
                    "on-disk ca-bundle.pem sha256 mismatch".into(),
                ));
            }
        } else {
            // Check if it's an expected RIM
            for rim in qualified_rims {
                if member.relative_path
                    == format!(
                        "{IOS_HELPER_RELATIVE_PATH}/{IOS_RIM_DIRECTORY}/{}",
                        rim.filename()
                    )
                    && member.sha256 != rim.sha256()
                {
                    return Err(IosVerifierRefusal::IntegrityFailed(format!(
                        "on-disk RIM sha256 mismatch for {}",
                        rim.filename()
                    )));
                }
            }
        }
    }

    let build_record = members
        .iter()
        .find(|member| member.relative_path == build_record_rel_path)
        .ok_or_else(|| {
            IosVerifierRefusal::IntegrityFailed("missing appraisal build record".into())
        })?;
    if build_record.sha256 != receipt.build_record_sha256 {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "appraisal build record digest mismatch".into(),
        ));
    }

    for expected in &expected_paths {
        if !found_paths.contains(expected) {
            return Err(IosVerifierRefusal::IntegrityFailed(format!(
                "missing expected member: {expected}"
            )));
        }
    }

    let helper = helper_member.ok_or_else(|| {
        IosVerifierRefusal::IntegrityFailed(format!("missing helper binary at {helper_exe_path}"))
    })?;

    // 5. Mach-O facts validation
    let facts = helper.macho_facts.as_ref().ok_or_else(|| {
        IosVerifierRefusal::IntegrityFailed("helper binary missing Mach-O facts".into())
    })?;

    if facts.platform != PLATFORM_IOS {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho platform {} != {PLATFORM_IOS} (iOS)",
            facts.platform
        )));
    }
    if facts.cputype != CPUTYPE_ARM64 {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho cputype {:#x} != {CPUTYPE_ARM64:#x} (arm64)",
            facts.cputype
        )));
    }
    if facts.filetype != MH_EXECUTE {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho filetype {} != {MH_EXECUTE} (execute)",
            facts.filetype
        )));
    }
    if facts.image_size > MAX_IMAGE_SIZE_BYTES {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho image size {} exceeds {MAX_IMAGE_SIZE_BYTES}",
            facts.image_size
        )));
    }
    if facts.ncmds > MAX_LOAD_COMMANDS {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho ncmds {} exceeds {MAX_LOAD_COMMANDS}",
            facts.ncmds
        )));
    }
    if facts.sizeofcmds > MAX_SIZEOFCMDS_BYTES {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "macho sizeofcmds {} exceeds {MAX_SIZEOFCMDS_BYTES}",
            facts.sizeofcmds
        )));
    }
    if !facts.rpaths.is_empty() {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "rpaths present in helper executable".into(),
        ));
    }
    if facts.install_name.is_some() {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "install_name present in helper executable".into(),
        ));
    }
    if !facts.upward_dylibs.is_empty() {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "upward dylibs present in helper executable".into(),
        ));
    }
    if !facts.lazy_dylibs.is_empty() {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "lazy dylibs present in helper executable".into(),
        ));
    }
    if !facts.dyld_environments.is_empty() {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "dyld environments present in helper executable".into(),
        ));
    }

    // 6. Load closure validation: every needed dependency must be absolute system library
    for dep in &facts.needed {
        if !dep.starts_with("/usr/lib/") && !dep.starts_with("/System/Library/") {
            return Err(IosVerifierRefusal::ClosureMismatch(format!(
                "disallowed non-system dependency: {dep}"
            )));
        }
    }
    if facts.needed != receipt.needed {
        return Err(IosVerifierRefusal::ClosureMismatch(format!(
            "macho needed list {:?} does not match receipt needed list {:?}",
            facts.needed, receipt.needed
        )));
    }

    // 7. Helper binary digest check against receipt (mandatory signed helper sha256)
    if receipt.helper_sha256.is_empty() || helper.sha256 != receipt.helper_sha256 {
        return Err(IosVerifierRefusal::IntegrityFailed(format!(
            "helper binary sha256 {} does not match receipt {}",
            helper.sha256, receipt.helper_sha256
        )));
    }

    Ok(AdmittedIosVerifier {
        bundle_path: PathBuf::new(),
        helper_path: PathBuf::from(IOS_HELPER_RELATIVE_PATH),
        receipt: receipt.clone(),
        bundle_id: bundle_id.to_string(),
        extension_point_host: extension_point_host.to_string(),
        extension_point_name: extension_point_name.to_string(),
        helper_sha256: helper.sha256.clone(),
        needed: facts.needed.clone(),
    })
}

static ADMITTED_IOS_VERIFIER: OnceLock<AdmittedIosVerifier> = OnceLock::new();

/// Finds the enclosing `.app` bundle directory from `std::env::current_exe()`.
fn discover_app_bundle() -> Result<PathBuf, IosVerifierRefusal> {
    let current_exe = std::env::current_exe()
        .map_err(|e| IosVerifierRefusal::Unavailable(format!("current_exe failed: {e}")))?;

    let mut current = current_exe.as_path();
    while let Some(parent) = current.parent() {
        if parent.extension().and_then(|s| s.to_str()) == Some("app") {
            let canonical = parent.canonicalize().map_err(|e| {
                IosVerifierRefusal::IntegrityFailed(format!("canonicalize .app failed: {e}"))
            })?;
            return Ok(canonical);
        }
        current = parent;
    }
    Err(IosVerifierRefusal::Unavailable(
        "process is not running inside an app bundle".into(),
    ))
}

/// Scans bundle members under `Extensions/Appraisal.appex` using symlink metadata.
fn scan_bundle_members(
    app_root: &Path,
) -> Result<(IosGpuAppraisalReceipt, Vec<IosBundleMember>), IosVerifierRefusal> {
    let appraisal_dir = app_root.join(IOS_HELPER_RELATIVE_PATH);
    if !appraisal_dir.is_dir() {
        return Err(IosVerifierRefusal::Unavailable(format!(
            "missing {IOS_HELPER_RELATIVE_PATH} directory"
        )));
    }

    // One receipt path only: Extensions/Appraisal.appex/gpu-appraisal-receipt.json
    let receipt_path = appraisal_dir.join("gpu-appraisal-receipt.json");
    let receipt_metadata = std::fs::symlink_metadata(&receipt_path).map_err(|error| {
        IosVerifierRefusal::Unavailable(format!("missing gpu-appraisal-receipt.json: {error}"))
    })?;
    if !receipt_metadata.is_file() || receipt_metadata.file_type().is_symlink() {
        return Err(IosVerifierRefusal::Unavailable(
            "gpu-appraisal-receipt.json is not a regular file".into(),
        ));
    }
    if receipt_metadata.len() > MAX_RECEIPT_SIZE_BYTES {
        return Err(IosVerifierRefusal::ReceiptInvalid(format!(
            "receipt size {} exceeds cap {MAX_RECEIPT_SIZE_BYTES}",
            receipt_metadata.len()
        )));
    }

    let receipt_bytes = std::fs::read(&receipt_path)
        .map_err(|e| IosVerifierRefusal::IntegrityFailed(format!("failed reading receipt: {e}")))?;
    let receipt: IosGpuAppraisalReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|e| IosVerifierRefusal::ReceiptInvalid(format!("failed parsing receipt: {e}")))?;

    let mut members = Vec::new();
    let mut pending = vec![appraisal_dir.clone()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|e| {
            IosVerifierRefusal::Unavailable(format!(
                "failed reading appraisal bundle directory {}: {e}",
                directory.display()
            ))
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| {
                IosVerifierRefusal::IntegrityFailed(format!("directory entry error: {e}"))
            })?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| {
                IosVerifierRefusal::IntegrityFailed(format!(
                    "symlink_metadata error for {}: {e}",
                    path.display()
                ))
            })?;
            let relative = path.strip_prefix(&appraisal_dir).map_err(|e| {
                IosVerifierRefusal::IntegrityFailed(format!("appraisal member escaped bundle: {e}"))
            })?;
            // Manifest coordinates use slash separators on every host. Preserve
            // component text so a Unix backslash filename is not reinterpreted.
            let relative_path = format!(
                "{IOS_HELPER_RELATIVE_PATH}/{}",
                relative
                    .iter()
                    .map(|part| part.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            );
            let is_symlink = meta.file_type().is_symlink();
            if is_symlink {
                members.push(IosBundleMember {
                    relative_path,
                    is_regular_file: false,
                    is_symlink: true,
                    size_bytes: 0,
                    sha256: String::new(),
                    macho_facts: None,
                });
                continue;
            }
            if meta.is_dir() {
                if relative == Path::new("_CodeSignature")
                    || relative == Path::new(IOS_RIM_DIRECTORY)
                {
                    pending.push(path);
                    continue;
                }
                members.push(IosBundleMember {
                    relative_path,
                    is_regular_file: false,
                    is_symlink: false,
                    size_bytes: 0,
                    sha256: String::new(),
                    macho_facts: None,
                });
                continue;
            }
            if !meta.is_file() {
                members.push(IosBundleMember {
                    relative_path,
                    is_regular_file: false,
                    is_symlink: false,
                    size_bytes: 0,
                    sha256: String::new(),
                    macho_facts: None,
                });
                continue;
            }
            if meta.len() > MAX_BUNDLE_MEMBER_SIZE_BYTES {
                return Err(IosVerifierRefusal::IntegrityFailed(format!(
                    "bundle member size {} exceeds cap {MAX_BUNDLE_MEMBER_SIZE_BYTES}: {}",
                    meta.len(),
                    path.display()
                )));
            }
            let bytes = std::fs::read(&path).map_err(|e| {
                IosVerifierRefusal::IntegrityFailed(format!(
                    "failed reading {}: {e}",
                    path.display()
                ))
            })?;
            let macho_facts = if relative == Path::new(IOS_HELPER_EXECUTABLE_NAME) {
                Some(inspect_ios_macho(&bytes)?)
            } else {
                None
            };
            let sha256 = if macho_facts.is_some() {
                ios_macho_content_sha256(&bytes)?
            } else {
                sha256_hex(&bytes)
            };
            members.push(IosBundleMember {
                relative_path,
                is_regular_file: true,
                is_symlink: false,
                size_bytes: meta.len(),
                sha256,
                macho_facts,
            });
            if members.len() > MAX_BUNDLE_MEMBERS + 2 {
                return Err(IosVerifierRefusal::IntegrityFailed(format!(
                    "bundle member count exceeds cap {}",
                    MAX_BUNDLE_MEMBERS + 2
                )));
            }
        }
    }

    let info = plist::Value::from_file(appraisal_dir.join("Info.plist")).map_err(|error| {
        IosVerifierRefusal::SubstitutedIdentity(format!("Info.plist parse failed: {error}"))
    })?;
    let dictionary = info.as_dictionary().ok_or_else(|| {
        IosVerifierRefusal::SubstitutedIdentity("Info.plist root is not a dictionary".into())
    })?;
    let string_value = |key: &str| {
        dictionary
            .get(key)
            .and_then(plist::Value::as_string)
            .ok_or_else(|| {
                IosVerifierRefusal::SubstitutedIdentity(format!(
                    "Info.plist is missing string {key}"
                ))
            })
    };
    if string_value("CFBundleIdentifier")? != IOS_BUNDLE_IDENTIFIER
        || string_value("CFBundleExecutable")? != IOS_HELPER_EXECUTABLE_NAME
        || string_value("CFBundlePackageType")? != "XPC!"
    {
        return Err(IosVerifierRefusal::SubstitutedIdentity(
            "Appraisal extension bundle identity drifted".into(),
        ));
    }
    let extension_identifier = dictionary
        .get("EXAppExtensionAttributes")
        .and_then(plist::Value::as_dictionary)
        .and_then(|attributes| attributes.get("EXExtensionPointIdentifier"))
        .and_then(plist::Value::as_string)
        .ok_or_else(|| {
            IosVerifierRefusal::SubstitutedIdentity(
                "Info.plist is missing extension point identity".into(),
            )
        })?;
    if extension_identifier != IOS_BUNDLE_IDENTIFIER {
        return Err(IosVerifierRefusal::SubstitutedIdentity(format!(
            "unexpected extension point identifier: {extension_identifier}"
        )));
    }

    Ok((receipt, members))
}

/// Discovers the app bundle, validates the companion appraisal helper, and
/// records the admitted authority in process state.
pub fn admit_ios_verifier() -> Result<AdmittedIosVerifier, IosVerifierRefusal> {
    let app_root = discover_app_bundle()?;
    let (receipt, members) = scan_bundle_members(&app_root)?;

    let mut admitted = ios_admission_decision(
        &receipt,
        &members,
        IOS_BUNDLE_IDENTIFIER,
        IOS_EXTENSION_POINT_HOST,
        IOS_EXTENSION_POINT_NAME,
    )?;
    admitted.bundle_path = app_root.clone();
    admitted.helper_path = app_root.join(IOS_HELPER_RELATIVE_PATH);

    if let Some(existing) = ADMITTED_IOS_VERIFIER.get() {
        if existing.bundle_id == admitted.bundle_id
            && existing.helper_sha256 == admitted.helper_sha256
            && existing.bundle_path == admitted.bundle_path
        {
            Ok(existing.clone())
        } else {
            Err(IosVerifierRefusal::AlreadyInstalled)
        }
    } else {
        let _ = ADMITTED_IOS_VERIFIER.set(admitted.clone());
        Ok(admitted)
    }
}

/// Re-validates that the admitted iOS verifier remains unaltered.
///
/// Refuses if the verifier was never admitted, or if any bundle member has drifted.
pub fn admitted_ios_verifier() -> Result<AdmittedIosVerifier, IosVerifierRefusal> {
    let existing = ADMITTED_IOS_VERIFIER.get().ok_or_else(|| {
        IosVerifierRefusal::Unavailable("ios verifier has not been admitted in this process".into())
    })?;

    let (receipt, members) = scan_bundle_members(&existing.bundle_path)?;
    let rechecked = ios_admission_decision(
        &receipt,
        &members,
        &existing.bundle_id,
        &existing.extension_point_host,
        &existing.extension_point_name,
    )
    .map_err(|e| IosVerifierRefusal::IntegrityFailed(format!("re-admission check failed: {e}")))?;

    if rechecked.helper_sha256 != existing.helper_sha256 {
        return Err(IosVerifierRefusal::IntegrityFailed(
            "helper binary digest changed".into(),
        ));
    }

    Ok(existing.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_receipt() -> IosGpuAppraisalReceipt {
        let qualified_rims: Vec<IosPackagedRimEntry> =
            crate::nvgpu::ManifestSet::QUALIFIED_595_71_05
                .entries()
                .iter()
                .map(|rim| IosPackagedRimEntry {
                    filename: rim.filename().to_string(),
                    sha256: rim.sha256().to_string(),
                })
                .collect();

        let regorus_pin = solstone_core_distribution::regorus_build_revision_pin();

        let helper_sha = "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";
        let build_sha = "1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd";
        let mut package_members = vec![
            IosPackagedMemberEntry {
                path: "Appraisal".into(),
                sha256: helper_sha.into(),
                size_bytes: 2048,
            },
            IosPackagedMemberEntry {
                path: "Info.plist".into(),
                sha256: "11".repeat(32),
                size_bytes: 1,
            },
            IosPackagedMemberEntry {
                path: "embedded.mobileprovision".into(),
                sha256: "22".repeat(32),
                size_bytes: 1,
            },
            IosPackagedMemberEntry {
                path: "gpu-appraisal-build.json".into(),
                sha256: build_sha.into(),
                size_bytes: 1,
            },
            IosPackagedMemberEntry {
                path: "ca-bundle.pem".into(),
                sha256: solstone_core_distribution::pinned_ca_bundle_sha256().into(),
                size_bytes: 1,
            },
        ];
        package_members.extend(qualified_rims.iter().map(|rim| IosPackagedMemberEntry {
            path: format!("{IOS_RIM_DIRECTORY}/{}", rim.filename),
            sha256: rim.sha256.clone(),
            size_bytes: 1,
        }));

        IosGpuAppraisalReceipt {
            status: "passed".into(),
            complete: true,
            exit: 0,
            platform: "ios".into(),
            arch: "arm64".into(),
            driver_sha256: sha256_hex(MOBILE_NATIVE_DRIVER),
            inputs_sha256: sha256_hex(MOBILE_NATIVE_INPUTS),
            patch_sha256: sha256_hex(MOBILE_NVATTEST_PATCH),
            nvattest_revision: solstone_core_distribution::NVATTEST_SDK_REVISION.into(),
            regorus_revision: solstone_core_distribution::pinned_regorus_revision().into(),
            regorus_original_sha256: regorus_pin.original_sha256.into(),
            regorus_patched_sha256: regorus_pin.patched_sha256.into(),
            ca_bundle_sha256: solstone_core_distribution::pinned_ca_bundle_sha256().into(),
            pcr_sha256_pins: crate::pins::PRODUCTION_PCR_SHA256_PINS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            rims: qualified_rims,
            needed: vec![
                "/usr/lib/libSystem.B.dylib".into(),
                "/System/Library/Frameworks/Foundation.framework/Foundation".into(),
            ],
            helper_sha256: helper_sha.into(),
            schema: "gpu-appraisal-receipt-v2".into(),
            build_record_sha256: build_sha.into(),
            package_members,
            steps: Vec::new(),
        }
    }

    fn valid_helper_member(helper_sha: &str) -> IosBundleMember {
        IosBundleMember {
            relative_path: "Extensions/Appraisal.appex/Appraisal".into(),
            is_regular_file: true,
            is_symlink: false,
            size_bytes: 2048,
            sha256: helper_sha.into(),
            macho_facts: Some(IosMachoFacts {
                platform: PLATFORM_IOS,
                cputype: CPUTYPE_ARM64,
                filetype: MH_EXECUTE,
                image_size: 2048,
                ncmds: 10,
                sizeofcmds: 1024,
                needed: vec![
                    "/usr/lib/libSystem.B.dylib".into(),
                    "/System/Library/Frameworks/Foundation.framework/Foundation".into(),
                ],
                rpaths: Vec::new(),
                install_name: None,
                upward_dylibs: Vec::new(),
                lazy_dylibs: Vec::new(),
                dyld_environments: Vec::new(),
            }),
        }
    }

    fn valid_members(helper_sha: &str) -> Vec<IosBundleMember> {
        let receipt = valid_receipt();
        let mut members = vec![
            valid_helper_member(helper_sha),
            IosBundleMember {
                relative_path: "Extensions/Appraisal.appex/gpu-appraisal-receipt.json".into(),
                is_regular_file: true,
                is_symlink: false,
                size_bytes: 1,
                sha256: "receiptsha".into(),
                macho_facts: None,
            },
            IosBundleMember {
                relative_path: "Extensions/Appraisal.appex/_CodeSignature/CodeResources".into(),
                is_regular_file: true,
                is_symlink: false,
                size_bytes: 1,
                sha256: "signature".into(),
                macho_facts: None,
            },
        ];
        for expected in receipt.package_members {
            if expected.path == "Appraisal" {
                continue;
            }
            members.push(IosBundleMember {
                relative_path: format!("Extensions/Appraisal.appex/{}", expected.path),
                is_regular_file: true,
                is_symlink: false,
                size_bytes: expected.size_bytes,
                sha256: expected.sha256,
                macho_facts: None,
            });
        }
        members
    }

    #[test]
    fn pure_decision_accepts_valid_receipt_and_bundle() {
        let receipt = valid_receipt();
        let members = valid_members(&receipt.helper_sha256);
        let admitted = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .expect("admission should succeed");

        assert_eq!(admitted.bundle_id, IOS_BUNDLE_IDENTIFIER);
        assert_eq!(admitted.needed.len(), 2);
    }

    #[test]
    fn pure_decision_refuses_failed_receipt() {
        let mut receipt = valid_receipt();
        receipt.status = "failed".into();
        let members = valid_members(&receipt.helper_sha256);
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::ReceiptInvalid(_)));
        assert_eq!(ios_verifier_refusal_reason(&err), "nvattest_unavailable");
    }

    #[test]
    fn pure_decision_refuses_wrong_platform_or_arch() {
        let mut receipt = valid_receipt();
        receipt.platform = "android".into();
        let members = valid_members(&receipt.helper_sha256);
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::BadPlatform(_)));

        let mut receipt = valid_receipt();
        receipt.arch = "x86_64".into();
        let members = valid_members(&receipt.helper_sha256);
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::BadArchitecture(_)));
    }

    #[test]
    fn pure_decision_refuses_pin_mismatch() {
        let mut receipt = valid_receipt();
        receipt.driver_sha256 =
            "0000000000000000000000000000000000000000000000000000000000000000".into();
        let members = valid_members(&receipt.helper_sha256);
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::PinMismatch(_)));
        assert_eq!(
            ios_verifier_refusal_reason(&err),
            "nvattest_integrity_failed"
        );
    }

    #[test]
    fn pure_decision_refuses_regorus_patch_hash_mismatch() {
        let mut receipt = valid_receipt();
        receipt.regorus_original_sha256 =
            "0000000000000000000000000000000000000000000000000000000000000000".into();
        let members = valid_members(&receipt.helper_sha256);
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::PinMismatch(_)));
    }

    #[test]
    fn pure_decision_refuses_substituted_identity() {
        let receipt = valid_receipt();
        let members = valid_members(&receipt.helper_sha256);
        let err = ios_admission_decision(
            &receipt,
            &members,
            "app.solstone.journal.runtime.dev.different-id",
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::SubstitutedIdentity(_)));
        assert_eq!(
            ios_verifier_refusal_reason(&err),
            "nvattest_integrity_failed"
        );
    }

    #[test]
    fn pure_decision_refuses_symlink_or_non_regular_file() {
        let receipt = valid_receipt();
        let mut members = valid_members(&receipt.helper_sha256);
        members[0].is_symlink = true;
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::IntegrityFailed(_)));
        assert_eq!(
            ios_verifier_refusal_reason(&err),
            "nvattest_integrity_failed"
        );
    }

    #[test]
    fn pure_decision_refuses_extra_or_missing_member() {
        let receipt = valid_receipt();
        let mut members = valid_members(&receipt.helper_sha256);

        // Extra member
        members.push(IosBundleMember {
            relative_path: "Extensions/Appraisal.appex/extra_file.txt".into(),
            is_regular_file: true,
            is_symlink: false,
            size_bytes: 1,
            sha256: "somehash".into(),
            macho_facts: None,
        });
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();
        assert!(matches!(err, IosVerifierRefusal::IntegrityFailed(_)));

        // Missing expected member
        let mut members = valid_members(&receipt.helper_sha256);
        members.retain(|m| !m.relative_path.ends_with("ca-bundle.pem"));
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();
        assert!(matches!(err, IosVerifierRefusal::IntegrityFailed(_)));
    }

    #[test]
    fn pure_decision_refuses_self_declared_extra_member() {
        let mut receipt = valid_receipt();
        let mut members = valid_members(&receipt.helper_sha256);
        receipt.package_members.push(IosPackagedMemberEntry {
            path: "unqualified.xml".into(),
            sha256: "11".repeat(32),
            size_bytes: 1,
        });
        members.push(IosBundleMember {
            relative_path: format!("{IOS_HELPER_RELATIVE_PATH}/unqualified.xml"),
            is_regular_file: true,
            is_symlink: false,
            size_bytes: 1,
            sha256: "11".repeat(32),
            macho_facts: None,
        });
        assert!(matches!(
            ios_admission_decision(
                &receipt,
                &members,
                IOS_BUNDLE_IDENTIFIER,
                IOS_EXTENSION_POINT_HOST,
                IOS_EXTENSION_POINT_NAME,
            ),
            Err(IosVerifierRefusal::ReceiptInvalid(_))
        ));
    }

    #[test]
    fn pure_decision_refuses_on_disk_ca_or_rim_hash_mismatch() {
        let receipt = valid_receipt();
        let mut members = valid_members(&receipt.helper_sha256);

        // Mutate CA hash
        for m in &mut members {
            if m.relative_path.ends_with("ca-bundle.pem") {
                m.sha256 = "mutated_ca".into();
            }
        }
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();
        assert!(matches!(err, IosVerifierRefusal::IntegrityFailed(_)));
    }

    #[test]
    fn pure_decision_refuses_helper_hash_mismatch() {
        let receipt = valid_receipt();
        let members =
            valid_members("wrong_helper_hash_11111111111111111111111111111111111111111111");
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();
        assert!(matches!(err, IosVerifierRefusal::IntegrityFailed(_)));
    }

    #[test]
    fn pure_decision_refuses_non_system_dependency() {
        let receipt = valid_receipt();
        let mut members = valid_members(&receipt.helper_sha256);
        members[0]
            .macho_facts
            .as_mut()
            .unwrap()
            .needed
            .push("@rpath/libforbidden.dylib".into());
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::ClosureMismatch(_)));
        assert_eq!(
            ios_verifier_refusal_reason(&err),
            "nvattest_integrity_failed"
        );
    }

    #[test]
    fn pure_decision_refuses_non_ios_macho_platform() {
        let receipt = valid_receipt();
        let mut members = valid_members(&receipt.helper_sha256);
        members[0].macho_facts.as_mut().unwrap().platform = 1; // macOS
        let err = ios_admission_decision(
            &receipt,
            &members,
            IOS_BUNDLE_IDENTIFIER,
            IOS_EXTENSION_POINT_HOST,
            IOS_EXTENSION_POINT_NAME,
        )
        .unwrap_err();

        assert!(matches!(err, IosVerifierRefusal::IntegrityFailed(_)));
    }

    #[test]
    fn macho_content_digest_excludes_only_signature_blob() {
        let mut image = vec![0u8; 52];
        image[0..4].copy_from_slice(&0xfeed_facfu32.to_le_bytes());
        image[16..20].copy_from_slice(&1u32.to_le_bytes());
        image[20..24].copy_from_slice(&16u32.to_le_bytes());
        image[32..36].copy_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
        image[36..40].copy_from_slice(&16u32.to_le_bytes());
        image[40..44].copy_from_slice(&48u32.to_le_bytes());
        image[44..48].copy_from_slice(&4u32.to_le_bytes());
        image[48..52].copy_from_slice(b"aaaa");
        let first = ios_macho_content_sha256(&image).unwrap();
        image[48..52].copy_from_slice(b"bbbb");
        assert_eq!(first, ios_macho_content_sha256(&image).unwrap());
        image[8] = 1;
        assert_ne!(first, ios_macho_content_sha256(&image).unwrap());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn bundle_scanner_descends_only_into_packaged_rim_directory() {
        let temp = tempfile::tempdir().unwrap();
        let appraisal = temp.path().join(IOS_HELPER_RELATIVE_PATH);
        let rims = appraisal.join(IOS_RIM_DIRECTORY);
        std::fs::create_dir_all(&rims).unwrap();
        std::fs::write(
            appraisal.join("gpu-appraisal-receipt.json"),
            serde_json::to_vec(&valid_receipt()).unwrap(),
        )
        .unwrap();
        std::fs::write(rims.join("qualified.xml"), b"rim").unwrap();

        write_appraisal_info(&appraisal, IOS_BUNDLE_IDENTIFIER);

        let (_, members) = scan_bundle_members(temp.path()).unwrap();
        assert!(members.iter().any(|member| {
            member.relative_path
                == format!("{IOS_HELPER_RELATIVE_PATH}/{IOS_RIM_DIRECTORY}/qualified.xml")
                && member.is_regular_file
        }));
        assert!(!members.iter().any(|member| {
            member.relative_path == format!("{IOS_HELPER_RELATIVE_PATH}/{IOS_RIM_DIRECTORY}")
        }));
    }

    #[cfg(feature = "full-tests")]
    fn write_appraisal_info(appraisal: &Path, identifier: &str) {
        let mut extension_attributes = plist::Dictionary::new();
        extension_attributes.insert("EXExtensionPointIdentifier".into(), identifier.into());
        let mut info = plist::Dictionary::new();
        info.insert("CFBundleIdentifier".into(), identifier.into());
        info.insert(
            "CFBundleExecutable".into(),
            IOS_HELPER_EXECUTABLE_NAME.into(),
        );
        info.insert("CFBundlePackageType".into(), "XPC!".into());
        info.insert(
            "EXAppExtensionAttributes".into(),
            plist::Value::Dictionary(extension_attributes),
        );
        plist::to_file_xml(
            appraisal.join("Info.plist"),
            &plist::Value::Dictionary(info),
        )
        .unwrap();
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn bundle_scanner_refuses_the_other_apps_appraisal_identity() {
        #[cfg(not(feature = "ios-owner-app"))]
        let other = "app.solstone.journal.owner.dev.gpu-appraisal";
        #[cfg(feature = "ios-owner-app")]
        let other = "app.solstone.journal.runtime.dev.gpu-appraisal";
        assert_ne!(other, IOS_BUNDLE_IDENTIFIER);
        assert_eq!(
            IOS_BUNDLE_IDENTIFIER,
            format!("{IOS_EXTENSION_POINT_HOST}.{IOS_EXTENSION_POINT_NAME}")
        );

        let temp = tempfile::tempdir().unwrap();
        let appraisal = temp.path().join(IOS_HELPER_RELATIVE_PATH);
        std::fs::create_dir_all(&appraisal).unwrap();
        std::fs::write(
            appraisal.join("gpu-appraisal-receipt.json"),
            serde_json::to_vec(&valid_receipt()).unwrap(),
        )
        .unwrap();
        write_appraisal_info(&appraisal, other);

        let err = scan_bundle_members(temp.path()).unwrap_err();
        assert!(matches!(err, IosVerifierRefusal::SubstitutedIdentity(_)));

        write_appraisal_info(&appraisal, IOS_BUNDLE_IDENTIFIER);
        scan_bundle_members(temp.path()).unwrap();
    }
}
