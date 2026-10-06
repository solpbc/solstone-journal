// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure admission and evidence verification for the Windows NVIDIA GPU
//! attestation verifier (`nvattest.exe`).
//!
//! Input files provide no authority, digests, destinations, or overrides.
//! Output identity, source revision, offline tools, and notices are strictly
//! bound to committed pins and inventory configuration.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use crate::controlled_build::{ControlledBuildReceipt, decode_controlled_build_receipt};
use crate::digest::sha256_hex;
use crate::pe_dependencies::inspect_dependencies;

pub const NVATTEST_BUILD_EVIDENCE_SCHEMA_V1: &str = "solstone.nvattest-windows-build-evidence.v1";
pub const NVATTEST_BUILD_EVIDENCE_LABEL: &str =
    "provenance/windows-x86_64/nvattest-build-evidence.json";
pub const NVATTEST_TOOL_CENSUS_SCHEMA_V1: &str = "solstone.nvattest-windows-tool-census.v1";
pub const NVATTEST_NOTICES_INDEX_SCHEMA_V1: &str = "solstone.nvattest-windows-notices.v1";
pub const NVATTEST_EXE_OUTPUT_LABEL: &str = "bin/nvattest.exe";
pub const NVATTEST_CA_BUNDLE_LABEL: &str = "share/ca/ca-bundle.pem";
pub const NVATTEST_LICENSE_LABEL: &str = "LICENSE";

pub const IMPORT_ALLOWLIST: &[&str] = &[
    "advapi32.dll",
    "bcrypt.dll",
    "bcryptprimitives.dll",
    "crypt32.dll",
    "kernel32.dll",
    "ntdll.dll",
    "shell32.dll",
    "user32.dll",
    "ws2_32.dll",
    "api-ms-win-core-synch-l1-2-0.dll",
    "api-ms-win-crt-convert-l1-1-0.dll",
    "api-ms-win-crt-environment-l1-1-0.dll",
    "api-ms-win-crt-filesystem-l1-1-0.dll",
    "api-ms-win-crt-heap-l1-1-0.dll",
    "api-ms-win-crt-locale-l1-1-0.dll",
    "api-ms-win-crt-math-l1-1-0.dll",
    "api-ms-win-crt-runtime-l1-1-0.dll",
    "api-ms-win-crt-stdio-l1-1-0.dll",
    "api-ms-win-crt-string-l1-1-0.dll",
    "api-ms-win-crt-time-l1-1-0.dll",
    "api-ms-win-crt-utility-l1-1-0.dll",
    "msvcp140.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pins {
    pub sdk_repo: &'static str,
    pub sdk_revision: &'static str,
    pub source_archive: ArchivePin,
    pub bundle_archive: ArchivePin,
    pub manifest_sha256: &'static str,
    pub ca_bundle: FilePin,
    pub license: FilePin,
    pub regorus_cargo_lock: FilePin,
    pub notices_body_sha256: &'static str,
    pub body_assembly_revision: &'static str,
    pub regorus_build_revision: RegorusBuildRevisionPin,
    pub msvc_runtime: MsvcRuntimePins,
    pub native_sources: &'static [DownloadSourcePin],
    pub build_tools: &'static [DownloadSourcePin],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchivePin {
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePin {
    pub member: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegorusBuildRevisionPin {
    pub revision: &'static str,
    pub original_sha256: &'static str,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsvcRuntimePins {
    pub msvcp140: FilePin,
    pub vcruntime140: FilePin,
    pub vcruntime140_1: FilePin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadSourcePin {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

pub fn production_pins() -> Pins {
    Pins {
        sdk_repo: "https://github.com/solpbc/attestation-sdk",
        sdk_revision: "8fdbb0f8c10594a5f88f77fdec4766803b4e6d59",
        source_archive: ArchivePin {
            bytes: 5171200,
            sha256: "8cfac3ecbdf370a9bc383690909d2489cfe0e9adc27c8dd403dde91aa7378d58",
        },
        bundle_archive: ArchivePin {
            bytes: 451225600,
            sha256: "fda9ddc90ead6a20e928d42c04a1eb1bb687f14bf484ff475ba4f11ba4cbddf2",
        },
        manifest_sha256: "6fe151b377b80c894135b4b42e65d4bdbfdcd4e170fea9c197b16fdaa833921f",
        ca_bundle: FilePin {
            member: "ca-bundle.pem",
            bytes: 186446,
            sha256: "3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91",
        },
        license: FilePin {
            member: "LICENSE",
            bytes: 11348,
            sha256: "82d36972a71088e8d4a4793313e64e18340c60de08e3175a58360a277c962a33",
        },
        regorus_cargo_lock: FilePin {
            member: "sol/release/regorus-Cargo.lock",
            bytes: 45645,
            sha256: "b8352c80ef609b602bfd25e393ad774f74894bab1ab60a9afbeb61d6aae3c910",
        },
        notices_body_sha256: "36c2cec38a03bfd53b35833598ce8f4129493d036e309adb6e395a91a4acf017",
        body_assembly_revision: "7db176e058ca749f7c02a2867bbf3ca1caaf41cc",
        regorus_build_revision: RegorusBuildRevisionPin {
            revision: "c7bf460bc160c96e38048296e5708943d2e43909",
            original_sha256: "7dc931d2a3cc9203b9cf63c9da29e85122f8b10ae2b1a4318494eacc178a3956",
            sha256: "bef3c5f151c9f48f2e0bcf9a71ceb7c4d7ad9c5aec723d483076d4247ba86c15",
        },
        msvc_runtime: MsvcRuntimePins {
            msvcp140: FilePin {
                member: "msvcp140.dll",
                bytes: 557728,
                sha256: "0f885b509a685d2bbfa652fed26b5fb31d88fbdab0a978c641d1c7b8aa460aa9",
            },
            vcruntime140: FilePin {
                member: "vcruntime140.dll",
                bytes: 124544,
                sha256: "d5e4d9a3e835fa679450145d6a7d94e36573a509317111904d9b3712c30d9066",
            },
            vcruntime140_1: FilePin {
                member: "vcruntime140_1.dll",
                bytes: 49792,
                sha256: "1f2d41c4aa5db0bc33ebf7b66d72943a817d7ce6cbe880502a9403823633093f",
            },
        },
        native_sources: &[
            DownloadSourcePin {
                name: "openssl-3.6.1.tar.gz",
                url: "https://github.com/openssl/openssl/releases/download/openssl-3.6.1/openssl-3.6.1.tar.gz",
                sha256: "b1bfedcd5b289ff22aee87c9d600f515767ebf45f77168cb6d64f231f518a82e",
            },
            DownloadSourcePin {
                name: "libxml2-2.11.9.tar.xz",
                url: "https://download.gnome.org/sources/libxml2/2.11/libxml2-2.11.9.tar.xz",
                sha256: "780157a1efdb57188ec474dca87acaee67a3a839c2525b2214d318228451809f",
            },
            DownloadSourcePin {
                name: "xmlsec1-1.2.39.tar.gz",
                url: "https://github.com/lsh123/xmlsec/releases/download/xmlsec-1_2_39/xmlsec1-1.2.39.tar.gz",
                sha256: "15f2f55ea5968e578fcd24b3b427e553876c86c147dc7f03923e98fc2768a1fa",
            },
            DownloadSourcePin {
                name: "curl-7.88.1.tar.gz",
                url: "https://github.com/curl/curl/releases/download/curl-7_88_1/curl-7.88.1.tar.gz",
                sha256: "cdb38b72e36bc5d33d5b8810f8018ece1baa29a8f215b4495e495ded82bbf3c7",
            },
            DownloadSourcePin {
                name: "zlib-1.3.1.tar.gz",
                url: "https://zlib.net/fossils/zlib-1.3.1.tar.gz",
                sha256: "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
            },
            DownloadSourcePin {
                name: "cli11-bfffd37e1f804ca4fae1caae106935791696b6a9.tar.gz",
                url: "https://codeload.github.com/CLIUtils/CLI11/tar.gz/bfffd37e1f804ca4fae1caae106935791696b6a9",
                sha256: "03c9b7921b8f99ca39ae660b03ebf9bd5a3f4280201f7d04bc5639b1e0496401",
            },
            DownloadSourcePin {
                name: "corrosion-6be991bb34c348dfb8344be22f3606288ea5c7fd.tar.gz",
                url: "https://codeload.github.com/corrosion-rs/corrosion/tar.gz/6be991bb34c348dfb8344be22f3606288ea5c7fd",
                sha256: "84d8fbc2810af9a42e250411dcd30b8e8cc59deba196c1dcd1fb44fec459a793",
            },
            DownloadSourcePin {
                name: "regorus-c7bf460bc160c96e38048296e5708943d2e43909.tar.gz",
                url: "https://codeload.github.com/microsoft/regorus/tar.gz/c7bf460bc160c96e38048296e5708943d2e43909",
                sha256: "188805b3b44b1cd2f9e8fd9b74a281b5f8eaa1c1b4087fd442afc88eccfef7cd",
            },
            DownloadSourcePin {
                name: "jwt-cpp-e71e0c2d584baff06925bbb3aad683f677e4d498.tar.gz",
                url: "https://codeload.github.com/Thalhammer/jwt-cpp/tar.gz/e71e0c2d584baff06925bbb3aad683f677e4d498",
                sha256: "1988cbe1c930638ac4341578fb0fc0616fe39cf468a9087624b27581d81f8b51",
            },
            DownloadSourcePin {
                name: "fmt-e69e5f977d458f2650bb346dadf2ad30c5320281.tar.gz",
                url: "https://codeload.github.com/fmtlib/fmt/tar.gz/e69e5f977d458f2650bb346dadf2ad30c5320281",
                sha256: "1723f27eed50e751037f49dcdf73e33b17658f1178ea1c1f829a30bb02335745",
            },
            DownloadSourcePin {
                name: "spdlog-27cb4c76708608465c413f6d0e6b8d99a4d84302.tar.gz",
                url: "https://codeload.github.com/gabime/spdlog/tar.gz/27cb4c76708608465c413f6d0e6b8d99a4d84302",
                sha256: "7d512b37019b61646cd6fd1e48f52a3cf3e098f36b33ba9c059fe51301ff40b3",
            },
            DownloadSourcePin {
                name: "json-3.12.0.tar.xz",
                url: "https://github.com/nlohmann/json/releases/download/v3.12.0/json.tar.xz",
                sha256: "42f6e95cad6ec532fd372391373363b62a14af6d771056dbfc86160e6dfff7aa",
            },
        ],
        build_tools: &[
            DownloadSourcePin {
                name: "strawberry-perl-5.40.0.1-64bit-portable.zip",
                url: "https://github.com/StrawberryPerl/Perl-Dist-Strawberry/releases/download/SP_54001_64bit_UCRT/strawberry-perl-5.40.0.1-64bit-portable.zip",
                sha256: "754f3e2a8e473dc68d1540c7802fb166a025f35ef18960c4564a31f8b5933907",
            },
            DownloadSourcePin {
                name: "cmake-3.31.12-windows-x86_64.zip",
                url: "https://cmake.org/files/v3.31/cmake-3.31.12-windows-x86_64.zip",
                sha256: "0c4baa40f28b3f8225eb3fdf6946c987b4fe901403b4eaf2fbbd9378100aaa0c",
            },
        ],
    }
}

pub struct AdmissionBytes<'a> {
    pub receipt: &'a [u8],
    pub evidence: &'a [u8],
    pub validation: &'a [u8],
    pub notices_body: &'a [u8],
    pub source_archive: &'a [u8],
    pub bundle_archive: &'a [u8],
    pub output_exe: &'a [u8],
    pub output_license: &'a [u8],
    pub output_msvcp140: &'a [u8],
    pub output_vcruntime140: &'a [u8],
    pub output_vcruntime140_1: &'a [u8],
}

pub struct MsvcRuntimeBytes<'a> {
    pub msvcp140: &'a [u8],
    pub vcruntime140: &'a [u8],
    pub vcruntime140_1: &'a [u8],
}

#[derive(Debug)]
pub struct AdmittedNvattest {
    receipt: ControlledBuildReceipt,
    receipt_bytes: Vec<u8>,
    evidence_bytes: Vec<u8>,
    validation_bytes: Vec<u8>,
    outputs: BTreeMap<String, Vec<u8>>,
}

impl AdmittedNvattest {
    pub fn receipt(&self) -> &ControlledBuildReceipt {
        &self.receipt
    }
    pub fn receipt_bytes(&self) -> &[u8] {
        &self.receipt_bytes
    }
    pub fn evidence_bytes(&self) -> &[u8] {
        &self.evidence_bytes
    }
    pub fn validation_bytes(&self) -> &[u8] {
        &self.validation_bytes
    }
    pub fn outputs(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.outputs
    }

    pub fn into_admitted_controlled_input(
        self,
    ) -> crate::produce::windows_inputs::AdmittedControlledInput {
        crate::produce::windows_inputs::AdmittedControlledInput::new(
            self.receipt,
            self.receipt_bytes,
            self.evidence_bytes,
            self.validation_bytes,
            self.outputs,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestWindowsBuildEvidence {
    pub schema: String,
    pub report_base64: String,
    pub census: NvattestToolCensus,
    pub invocation: NvattestInvocation,
    pub refusals: NvattestRefusals,
    pub network: NvattestNetworkEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestToolCensus {
    pub schema: String,
    pub rustc: NvattestToolVersionPath,
    pub cargo: NvattestToolVersionPath,
    pub cmake: NvattestToolVersionPath,
    pub msvc: NvattestMsvcToolsVersion,
    pub windows_sdk: NvattestWindowsSdkVersion,
    #[serde(default)]
    pub vs: Option<NvattestVsInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestToolVersionPath {
    #[serde(default)]
    pub version: Option<String>,
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestMsvcToolsVersion {
    #[serde(default)]
    pub vc_tools_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestWindowsSdkVersion {
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestVsInfo {
    #[serde(default)]
    pub product_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestInvocation {
    pub offline: bool,
    pub bundle_path: String,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestRefusals {
    pub manifest_digest: NvattestRefusalEntry,
    pub reuse_dependencies: NvattestRefusalEntry,
    pub corrupt_member: NvattestRefusalEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestRefusalEntry {
    pub exit_code: i32,
    pub boundary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestNetworkEvidence {
    pub positive_control: String,
    pub negative_control: String,
    pub rules_remaining: u32,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestBuildReport {
    pub schema: u32,
    pub source_commit: String,
    pub source_clean: Option<bool>,
    pub tools: NvattestReportTools,
    pub sources: Vec<NvattestReportSourceEntry>,
    pub build_tools: Vec<NvattestReportBuildToolEntry>,
    pub ca_bundle: NvattestReportCaBundle,
    pub imports: Vec<String>,
    pub outputs: Vec<NvattestReportOutputEntry>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestReportTools {
    pub msvc: String,
    pub vs: serde_json::Value,
    pub windows_sdk: String,
    pub cmake: String,
    pub rustc: String,
    pub cargo: String,
    pub crt_redist: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestReportSourceEntry {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub original_sha256: Option<String>,
    pub sha256: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestReportBuildToolEntry {
    pub name: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestReportCaBundle {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestReportOutputEntry {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub file_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NoticesIndex {
    pub schema: String,
    pub notices_body_sha256: String,
    pub body_assembly_revision: String,
    pub admitted_sdk_revision: String,
    pub regorus_cargo_lock: RegorusCargoLockEntry,
    pub windows_population_marker: String,
    pub rust_standard_library: String,
    pub mozilla_ca_notice: bool,
    pub notices_live_in: String,
    pub native_sources: Vec<NvattestReportSourceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegorusCargoLockEntry {
    pub member: String,
    pub sha256: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleManifest {
    pub schema: u32,
    pub files: Vec<BundleManifestFile>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleManifestFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

pub fn admit(
    pins: &Pins,
    index: &NoticesIndex,
    input: &AdmissionBytes<'_>,
    msvc: &MsvcRuntimeBytes<'_>,
) -> Result<AdmittedNvattest, String> {
    // 1. Validate notices index structure and pins
    validate_notices_index(pins, index)?;
    let notices_body_sha = sha256_hex(input.notices_body);
    if notices_body_sha != pins.notices_body_sha256 || notices_body_sha != index.notices_body_sha256
    {
        return Err("notices-body: notices body SHA-256 differs from pin or index".into());
    }

    // 2. Validate uncompressed archive sizes and SHA-256 hashes
    if input.source_archive.len() as u64 != pins.source_archive.bytes
        || sha256_hex(input.source_archive) != pins.source_archive.sha256
    {
        return Err("source-archive size or SHA-256 differs from pin".into());
    }
    if input.bundle_archive.len() as u64 != pins.bundle_archive.bytes
        || sha256_hex(input.bundle_archive) != pins.bundle_archive.sha256
    {
        return Err("bundle-archive size or SHA-256 differs from pin".into());
    }

    // 3. Inspect PE dependencies on the output exe before receipt verification
    let pe = inspect_dependencies(input.output_exe)?;
    if pe.is_dll {
        return Err("pe-kind: nvattest.exe must be an executable, not a DLL".into());
    }
    for import in pe
        .imports
        .iter()
        .chain(&pe.delay_imports)
        .chain(&pe.forwarders)
    {
        let lower = import.to_ascii_lowercase();
        if !IMPORT_ALLOWLIST.contains(&lower.as_str()) {
            return Err(format!(
                "import-allowlist: unrecognized PE import dependency: {import}"
            ));
        }
    }

    // 4. Decode and validate ControlledBuildReceipt
    let receipt = decode_controlled_build_receipt(input.receipt).map_err(|e| e.to_string())?;
    crate::produce::windows_inputs::verify_receipt_document_binding(
        &receipt,
        input.evidence,
        input.validation,
        NVATTEST_BUILD_EVIDENCE_LABEL,
    )?;

    if !receipt.configuration.network_access_denied {
        return Err(
            "network-rules: receipt configuration must declare network_access_denied=true".into(),
        );
    }
    if receipt.source.windows_dependency.repository != pins.sdk_repo
        || receipt.source.windows_dependency.revision != pins.sdk_revision
        || receipt.source.windows_dependency.content_sha256 != pins.source_archive.sha256
    {
        return Err("source-revision: receipt windows_dependency differs from pin".into());
    }
    if receipt.outputs.len() != 1 || receipt.outputs[0].label != NVATTEST_EXE_OUTPUT_LABEL {
        return Err("native receipt has an unexpected output set".into());
    }

    let output_entry = &receipt.outputs[0];
    let output_sha256 = sha256_hex(input.output_exe);
    if input.output_exe.len() as u64 != output_entry.size
        || output_sha256 != output_entry.pre_signing_sha256
    {
        return Err("report-output: output exe does not match receipt output identity".into());
    }

    // 5. Decode evidence document
    let evidence: NvattestWindowsBuildEvidence =
        serde_json::from_slice(input.evidence).map_err(|e| e.to_string())?;

    if evidence.schema != NVATTEST_BUILD_EVIDENCE_SCHEMA_V1 {
        return Err(format!(
            "evidence schema must be {NVATTEST_BUILD_EVIDENCE_SCHEMA_V1}"
        ));
    }
    if !evidence.invocation.offline {
        return Err("evidence invocation must declare offline=true".into());
    }
    if evidence.invocation.manifest_sha256 != pins.manifest_sha256 {
        return Err("bundle-archive: invocation manifest_sha256 differs from pin".into());
    }

    // Validate refusals
    validate_refusals(&evidence.refusals)?;

    // Validate network rules
    if evidence.network.positive_control != "connected"
        || evidence.network.negative_control != "refused"
        || evidence.network.rules_remaining != 0
    {
        return Err(
            "network-rules: positive control must connect, negative must refuse, rules_remaining must be 0".into(),
        );
    }

    // 6. Decode report from evidence report_base64
    let report_bytes = base64_decode_report(&evidence.report_base64)?;
    let report = decode_build_report(&report_bytes)?;

    if report.source_commit != pins.sdk_revision {
        return Err("source-revision: report source_commit differs from pin".into());
    }

    // Validate census vs report
    validate_census(pins, &evidence.census, &report)?;

    // Validate report output vs receipt output
    let exe_report_output = report
        .outputs
        .iter()
        .find(|o| o.path == NVATTEST_EXE_OUTPUT_LABEL)
        .ok_or_else(|| "report-output: report missing bin/nvattest.exe output".to_string())?;
    if exe_report_output.bytes != output_entry.size
        || exe_report_output.sha256 != output_entry.pre_signing_sha256
    {
        return Err("report-output: report exe output differs from receipt output".into());
    }

    // 7. Extract and validate members from source tar
    let (source_pax_comment, source_license) = extract_source_tar_members(input.source_archive)?;
    if source_pax_comment.as_deref() != Some(pins.sdk_revision) {
        return Err("source-revision: source pax comment differs from pin".into());
    }
    if source_license.len() as u64 != pins.license.bytes
        || sha256_hex(&source_license) != pins.license.sha256
    {
        return Err("source-archive: source LICENSE member differs from pin".into());
    }

    // Output license must match source archive license
    if input.output_license != source_license.as_slice() {
        return Err("output-license: output root LICENSE differs from archive member".into());
    }

    // 8. Extract and validate members from bundle tar
    let (bundle_manifest_bytes, bundle_ca_bytes) =
        extract_bundle_tar_members(input.bundle_archive)?;
    if sha256_hex(&bundle_manifest_bytes) != pins.manifest_sha256 {
        return Err("bundle-archive: bundle offline-manifest.json differs from pin".into());
    }
    let bundle_manifest: BundleManifest =
        serde_json::from_slice(&bundle_manifest_bytes).map_err(|e| e.to_string())?;
    if bundle_manifest.schema != 1 {
        return Err("bundle-archive: bundle offline-manifest schema must be 1".into());
    }

    // Validate CA bundle against pin and report
    let bundle_ca_sha256 = sha256_hex(&bundle_ca_bytes);
    if bundle_ca_bytes.len() as u64 != pins.ca_bundle.bytes
        || bundle_ca_sha256 != pins.ca_bundle.sha256
    {
        if bundle_ca_sha256 == report.ca_bundle.sha256 {
            return Err("ca-pin: bundle CA matches report but differs from pin".into());
        }
        return Err("ca-pin: bundle ca-bundle.pem differs from pin".into());
    }
    if bundle_ca_sha256 != report.ca_bundle.sha256 {
        return Err("ca-report: bundle CA matches pin but differs from report".into());
    }

    // Validate manifest files against report sources and build_tools
    validate_manifest_and_sources(pins, &bundle_manifest, &report)?;

    // 9. Validate CRT outputs against MSVC package members and pins
    validate_crt_members(
        pins,
        msvc,
        input.output_msvcp140,
        input.output_vcruntime140,
        input.output_vcruntime140_1,
    )?;

    let mut outputs = BTreeMap::new();
    outputs.insert(NVATTEST_EXE_OUTPUT_LABEL.into(), input.output_exe.to_vec());
    outputs.insert(NVATTEST_CA_BUNDLE_LABEL.into(), bundle_ca_bytes);
    outputs.insert(NVATTEST_LICENSE_LABEL.into(), source_license);

    Ok(AdmittedNvattest {
        receipt,
        receipt_bytes: input.receipt.to_vec(),
        evidence_bytes: input.evidence.to_vec(),
        validation_bytes: input.validation.to_vec(),
        outputs,
    })
}

fn validate_notices_index(pins: &Pins, index: &NoticesIndex) -> Result<(), String> {
    if index.schema != NVATTEST_NOTICES_INDEX_SCHEMA_V1 {
        return Err("notices-index: schema mismatch".into());
    }
    if index.notices_body_sha256 != pins.notices_body_sha256 {
        return Err("notices-body: notices body SHA-256 differs from pin".into());
    }
    if index.body_assembly_revision != pins.body_assembly_revision {
        return Err("notices-index: body assembly revision mismatch".into());
    }
    if index.admitted_sdk_revision != pins.sdk_revision {
        return Err("notices-index: admitted SDK revision mismatch".into());
    }
    if index.regorus_cargo_lock.member != pins.regorus_cargo_lock.member
        || index.regorus_cargo_lock.sha256 != pins.regorus_cargo_lock.sha256
    {
        return Err("notices-index: regorus cargo lock pin mismatch".into());
    }
    if index.windows_population_marker != "windows-link 0.2.1"
        || index.rust_standard_library != "1.97.1"
        || !index.mozilla_ca_notice
        || index.notices_live_in != "journal"
    {
        return Err("notices-index: notice markers mismatch".into());
    }

    if index.native_sources.len() != pins.native_sources.len() + 1 {
        return Err("notices-sources: native sources count mismatch".into());
    }
    for expected in pins.native_sources {
        let entry = index
            .native_sources
            .iter()
            .find(|s| s.name == expected.name)
            .ok_or_else(|| {
                format!(
                    "notices-sources: missing source {} in notices index",
                    expected.name
                )
            })?;
        if entry.url.as_deref() != Some(expected.url) || entry.sha256 != expected.sha256 {
            return Err(format!(
                "notices-sources: source {} url or digest differs from pin",
                expected.name
            ));
        }
    }
    let regorus_entry = index
        .native_sources
        .iter()
        .find(|s| s.name == "regorus-build-revision")
        .ok_or_else(|| {
            "notices-sources: missing regorus-build-revision in notices index".to_string()
        })?;
    if regorus_entry.url.is_some()
        || regorus_entry.revision.as_deref() != Some(pins.regorus_build_revision.revision)
        || regorus_entry.original_sha256.as_deref()
            != Some(pins.regorus_build_revision.original_sha256)
        || regorus_entry.sha256 != pins.regorus_build_revision.sha256
    {
        return Err("regorus-build-revision: index entry differs from pin".into());
    }
    Ok(())
}

fn validate_refusals(refusals: &NvattestRefusals) -> Result<(), String> {
    if refusals.manifest_digest.exit_code == 0
        || !refusals
            .manifest_digest
            .boundary
            .contains("offline manifest does not match the caller-bound digest")
    {
        return Err("refusals: manifest_digest refusal boundary token mismatch".into());
    }
    if refusals.reuse_dependencies.exit_code == 0
        || !refusals
            .reuse_dependencies
            .boundary
            .contains("offline builds cannot reuse dependencies")
    {
        return Err("refusals: reuse_dependencies refusal boundary token mismatch".into());
    }
    if refusals.corrupt_member.exit_code == 0
        || !refusals
            .corrupt_member
            .boundary
            .contains("missing or changed offline input")
    {
        return Err("refusals: corrupt_member refusal boundary token mismatch".into());
    }
    Ok(())
}

fn base64_decode_report(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut quartet = [0_u8; 4];
    let mut len = 0;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        quartet[len] = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err(format!("invalid base64 byte: {byte}")),
        };
        len += 1;
        if len == 4 {
            out.push((quartet[0] << 2) | (quartet[1] >> 4));
            if quartet[2] != 64 {
                out.push((quartet[1] << 4) | (quartet[2] >> 2));
            }
            if quartet[3] != 64 {
                out.push((quartet[2] << 6) | quartet[3]);
            }
            len = 0;
        }
    }
    if len != 0 {
        return Err("invalid unpadded base64 length".into());
    }
    Ok(out)
}

fn decode_build_report(bytes: &[u8]) -> Result<NvattestBuildReport, String> {
    // Strip optional leading UTF-8 BOM only for JSON parsing
    let clean_bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    serde_json::from_slice(clean_bytes)
        .map_err(|e| format!("failed to parse build-report.json: {e}"))
}

fn validate_census(
    _pins: &Pins,
    census: &NvattestToolCensus,
    report: &NvattestBuildReport,
) -> Result<(), String> {
    if census.schema != NVATTEST_TOOL_CENSUS_SCHEMA_V1 {
        return Err("census-version: tool census schema mismatch".into());
    }

    // 1. Rustc
    let rustc_ver = census
        .rustc
        .version
        .as_deref()
        .ok_or_else(|| "census-version: rustc version is missing or null".to_string())?;
    validate_sha256_hex(&census.rustc.sha256, "rustc path sha256")?;
    validate_rustc_version_token(rustc_ver)?;
    if rustc_ver != report.tools.rustc {
        return Err("tool-census: census rustc version differs from report".into());
    }

    // 2. Cargo
    let cargo_ver = census
        .cargo
        .version
        .as_deref()
        .ok_or_else(|| "census-version: cargo version is missing or null".to_string())?;
    validate_sha256_hex(&census.cargo.sha256, "cargo path sha256")?;
    validate_cargo_version_token(cargo_ver)?;
    if cargo_ver != report.tools.cargo {
        return Err("tool-census: census cargo version differs from report".into());
    }

    // 3. CMake
    let cmake_ver = census
        .cmake
        .version
        .as_deref()
        .ok_or_else(|| "census-version: cmake version is missing or null".to_string())?;
    validate_sha256_hex(&census.cmake.sha256, "cmake path sha256")?;
    validate_cmake_version_token(cmake_ver)?;
    if cmake_ver != report.tools.cmake {
        return Err("tool-census: census cmake version differs from report".into());
    }

    // 4. MSVC
    let msvc_ver =
        census.msvc.vc_tools_version.as_deref().ok_or_else(|| {
            "census-version: msvc vc_tools_version is missing or null".to_string()
        })?;
    if msvc_ver != "14.44.35207" {
        return Err("census-version: msvc vc_tools_version does not equal 14.44.35207".into());
    }
    if msvc_ver != report.tools.msvc {
        return Err("tool-census: census msvc version differs from report".into());
    }

    // 5. Windows SDK
    let sdk_ver = census
        .windows_sdk
        .version
        .as_deref()
        .ok_or_else(|| "census-version: windows_sdk version is missing or null".to_string())?;
    if !sdk_ver.ends_with('\\') || sdk_ver.ends_with("\\\\") {
        return Err(
            "census-version: windows_sdk version must end with exactly one trailing backslash"
                .into(),
        );
    }
    let sdk_trimmed = &sdk_ver[..sdk_ver.len() - 1];
    if sdk_trimmed != "10.0.26100.0" {
        return Err(
            "census-version: windows_sdk version remainder does not equal 10.0.26100.0".into(),
        );
    }
    if sdk_ver != report.tools.windows_sdk {
        return Err("tool-census: census windows_sdk version differs from report".into());
    }

    Ok(())
}

fn validate_sha256_hex(s: &str, label: &str) -> Result<(), String> {
    if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("census-version: {label} must be 64-hex: {s}"));
    }
    Ok(())
}

fn validate_rustc_version_token(s: &str) -> Result<(), String> {
    // e.g. "rustc 1.97.1 (8bab26f4f 2026-07-14)"
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() < 3 || tokens[0] != "rustc" || tokens[1] != "1.97.1" {
        return Err(format!("census-version: rustc version token mismatch: {s}"));
    }
    let commit = tokens[2].trim_start_matches('(');
    if commit != "8bab26f4f" {
        return Err(format!("census-version: rustc commit token mismatch: {s}"));
    }
    Ok(())
}

fn validate_cargo_version_token(s: &str) -> Result<(), String> {
    // e.g. "cargo 1.97.1 (c980f4866 2026-06-30)"
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() < 2 || tokens[0] != "cargo" || tokens[1] != "1.97.1" {
        return Err(format!("census-version: cargo version token mismatch: {s}"));
    }
    Ok(())
}

fn validate_cmake_version_token(s: &str) -> Result<(), String> {
    // e.g. "cmake version 3.31.12"
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() < 3 || tokens[0] != "cmake" || tokens[1] != "version" || tokens[2] != "3.31.12"
    {
        return Err(format!("census-version: cmake version token mismatch: {s}"));
    }
    Ok(())
}

fn extract_source_tar_members(archive_bytes: &[u8]) -> Result<(Option<String>, Vec<u8>), String> {
    let mut archive = tar::Archive::new(Cursor::new(archive_bytes));
    let mut pax_comment = None;
    let mut license = None;

    for entry in archive.entries().map_err(|e| e.to_string())?.raw(true) {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_pax_global_extensions()
            || entry_type.is_pax_local_extensions()
            || entry_type.as_byte() == b'g'
            || entry_type.as_byte() == b'x'
        {
            let mut body = Vec::new();
            entry.read_to_end(&mut body).map_err(|e| e.to_string())?;
            if let Some(c) = parse_pax_comment(&body) {
                pax_comment = Some(c);
            }
        } else if entry_type.is_file() {
            let path = entry.path().map_err(|e| e.to_string())?;
            if path == std::path::Path::new("LICENSE") {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                license = Some(bytes);
            }
        }
    }

    let license_bytes =
        license.ok_or_else(|| "source-archive: missing LICENSE in source archive".to_string())?;
    Ok((pax_comment, license_bytes))
}

fn parse_pax_comment(bytes: &[u8]) -> Option<String> {
    // PAX format: <len> <key>=<value>\n
    let text = std::str::from_utf8(bytes).ok()?;
    for line in text.lines() {
        if let Some((_len, rest)) = line.split_once(' ')
            && let Some((key, val)) = rest.split_once('=')
            && key == "comment"
        {
            return Some(val.trim().to_string());
        }
    }
    None
}

fn extract_bundle_tar_members(archive_bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut archive = tar::Archive::new(Cursor::new(archive_bytes));
    let mut manifest = None;
    let mut ca = None;

    for entry in archive.entries().map_err(|e| e.to_string())?.raw(true) {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if entry.header().entry_type().is_file() {
            let path = entry.path().map_err(|e| e.to_string())?;
            if path == std::path::Path::new("offline-manifest.json") {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                manifest = Some(bytes);
            } else if path == std::path::Path::new("ca-bundle.pem") {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                ca = Some(bytes);
            }
        }
    }

    let manifest_bytes = manifest
        .ok_or_else(|| "bundle-archive: missing offline-manifest.json in bundle".to_string())?;
    let ca_bytes =
        ca.ok_or_else(|| "bundle-archive: missing ca-bundle.pem in bundle".to_string())?;
    Ok((manifest_bytes, ca_bytes))
}

fn validate_manifest_and_sources(
    pins: &Pins,
    manifest: &BundleManifest,
    report: &NvattestBuildReport,
) -> Result<(), String> {
    // Report sources vs manifest
    for source in &report.sources {
        if source.name == "regorus-build-revision" {
            if source.revision.as_deref() != Some(pins.regorus_build_revision.revision)
                || source.original_sha256.as_deref()
                    != Some(pins.regorus_build_revision.original_sha256)
                || source.sha256 != pins.regorus_build_revision.sha256
            {
                return Err(
                    "regorus-build-revision: report regorus-build-revision differs from pin".into(),
                );
            }
            continue;
        }
        let manifest_file = manifest
            .files
            .iter()
            .find(|f| f.path == source.name)
            .ok_or_else(|| {
                format!(
                    "bundle-archive: report source {} not found in bundle manifest",
                    source.name
                )
            })?;
        if manifest_file.sha256 != source.sha256 {
            return Err(format!(
                "bundle-archive: manifest SHA-256 for {} differs from report",
                source.name
            ));
        }
    }

    // Report build_tools vs manifest
    for tool in &report.build_tools {
        let manifest_file = manifest
            .files
            .iter()
            .find(|f| f.path == tool.name)
            .ok_or_else(|| {
                format!(
                    "bundle-archive: report build tool {} not found in bundle manifest",
                    tool.name
                )
            })?;
        if manifest_file.sha256 != tool.sha256 {
            return Err(format!(
                "bundle-archive: manifest SHA-256 for {} differs from report",
                tool.name
            ));
        }
    }

    if let Some(manifest_ca) = manifest.files.iter().find(|f| f.path == "ca-bundle.pem")
        && manifest_ca.size != pins.ca_bundle.bytes
    {
        return Err("bundle-archive: manifest ca-bundle.pem size mismatch".into());
    }

    Ok(())
}

fn validate_crt_members(
    pins: &Pins,
    msvc: &MsvcRuntimeBytes<'_>,
    out_msvcp: &[u8],
    out_vcruntime: &[u8],
    out_vcruntime_1: &[u8],
) -> Result<(), String> {
    for (name, pin, msvc_bytes, out_bytes) in [
        (
            "msvcp140.dll",
            &pins.msvc_runtime.msvcp140,
            msvc.msvcp140,
            out_msvcp,
        ),
        (
            "vcruntime140.dll",
            &pins.msvc_runtime.vcruntime140,
            msvc.vcruntime140,
            out_vcruntime,
        ),
        (
            "vcruntime140_1.dll",
            &pins.msvc_runtime.vcruntime140_1,
            msvc.vcruntime140_1,
            out_vcruntime_1,
        ),
    ] {
        if msvc_bytes.len() as u64 != pin.bytes || sha256_hex(msvc_bytes) != pin.sha256 {
            return Err(format!("runtime-dll: msvc input {name} differs from pin"));
        }
        if out_bytes != msvc_bytes {
            return Err(format!(
                "runtime-dll: output {name} differs from msvc package member"
            ));
        }
    }
    Ok(())
}

pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controlled_build::{
        BuildConfiguration, BuilderIdentity, DependencySource, OutputIdentityEntry, SourceIdentity,
        SupportingArtifactRef, ValidationReference,
    };
    use crate::pe;
    use crate::provenance::Provenance;

    fn make_test_pe(dll: bool, imports: &[&str]) -> Vec<u8> {
        let import_specs: Vec<pe::ImportSpec<'_>> = imports
            .iter()
            .map(|name| pe::ImportSpec {
                name,
                symbols: &[pe::PeSymbolSpec::Named("TestSymbol")],
            })
            .collect();
        let mut bytes = pe::fixture(&pe::FixtureSpec {
            dll,
            imports: &import_specs,
            ..pe::FixtureSpec::default()
        });
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let opt_offset = pe_offset + 24;
        let headers_size = (opt_offset + 240 + 40) as u32;
        bytes[opt_offset + 60..opt_offset + 64].copy_from_slice(&headers_size.to_le_bytes());
        bytes
    }

    fn make_source_tar(comment: &str, license: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let pax_comment_str = format!("comment={comment}\n");
        let pax_len = pax_comment_str.len() + 3 + pax_comment_str.len().to_string().len();
        let pax_payload = format!("{pax_len} {pax_comment_str}");

        let mut global_header = tar::Header::new_gnu();
        global_header.set_entry_type(tar::EntryType::XGlobalHeader);
        global_header.set_size(pax_payload.len() as u64);
        global_header.set_mode(0o644);
        global_header.set_path("pax_global_header").unwrap();
        global_header.set_cksum();
        builder
            .append(&global_header, pax_payload.as_bytes())
            .unwrap();

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(license.len() as u64);
        header.set_mode(0o644);
        header.set_path("LICENSE").unwrap();
        header.set_cksum();
        builder.append(&header, license).unwrap();
        builder.into_inner().unwrap()
    }

    fn make_bundle_tar(manifest_json: &[u8], ca_pem: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(manifest_json.len() as u64);
        header.set_mode(0o644);
        header.set_path("offline-manifest.json").unwrap();
        header.set_cksum();
        builder.append(&header, manifest_json).unwrap();

        let mut header_ca = tar::Header::new_gnu();
        header_ca.set_entry_type(tar::EntryType::Regular);
        header_ca.set_size(ca_pem.len() as u64);
        header_ca.set_mode(0o644);
        header_ca.set_path("ca-bundle.pem").unwrap();
        header_ca.set_cksum();
        builder.append(&header_ca, ca_pem).unwrap();

        builder.into_inner().unwrap()
    }

    struct TestFixture {
        pins: Pins,
        index: NoticesIndex,
        receipt: Vec<u8>,
        evidence: Vec<u8>,
        validation: Vec<u8>,
        notices_body: Vec<u8>,
        source_archive: Vec<u8>,
        bundle_archive: Vec<u8>,
        output_exe: Vec<u8>,
        output_license: Vec<u8>,
        output_msvcp140: Vec<u8>,
        output_vcruntime140: Vec<u8>,
        output_vcruntime140_1: Vec<u8>,
        msvc_msvcp140: Vec<u8>,
        msvc_vcruntime140: Vec<u8>,
        msvc_vcruntime140_1: Vec<u8>,
    }

    fn setup_fixture() -> TestFixture {
        let pins = production_pins();
        let index: NoticesIndex = serde_json::from_slice(include_bytes!(
            "../../../distribution/nvattest-windows-sources.json"
        ))
        .unwrap();
        let notices_body =
            include_bytes!("../../../distribution/nvattest-windows-NOTICES.md").to_vec();

        let license = b"NVIDIA ATTESTATION SDK LICENSE TEXT\n";
        let ca_pem = b"-----BEGIN CERTIFICATE-----\nTEST CA\n-----END CERTIFICATE-----\n";

        let mut custom_pins = pins.clone();
        custom_pins.license.bytes = license.len() as u64;
        custom_pins.license.sha256 = Box::leak(sha256_hex(license).into_boxed_str());
        custom_pins.ca_bundle.bytes = ca_pem.len() as u64;
        custom_pins.ca_bundle.sha256 = Box::leak(sha256_hex(ca_pem).into_boxed_str());

        let msvcp = vec![1u8; 557728];
        let vcruntime = vec![2u8; 124544];
        let vcruntime_1 = vec![3u8; 49792];

        custom_pins.msvc_runtime.msvcp140.sha256 = Box::leak(sha256_hex(&msvcp).into_boxed_str());
        custom_pins.msvc_runtime.vcruntime140.sha256 =
            Box::leak(sha256_hex(&vcruntime).into_boxed_str());
        custom_pins.msvc_runtime.vcruntime140_1.sha256 =
            Box::leak(sha256_hex(&vcruntime_1).into_boxed_str());

        let source_archive = make_source_tar(pins.sdk_revision, license);
        custom_pins.source_archive.bytes = source_archive.len() as u64;
        custom_pins.source_archive.sha256 = Box::leak(sha256_hex(&source_archive).into_boxed_str());

        let manifest_json = serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "files": pins.native_sources.iter().chain(pins.build_tools).map(|s| {
                serde_json::json!({
                    "path": s.name,
                    "sha256": s.sha256,
                    "size": 1000
                })
            }).chain(std::iter::once(serde_json::json!({
                "path": "ca-bundle.pem",
                "sha256": custom_pins.ca_bundle.sha256,
                "size": ca_pem.len()
            }))).collect::<Vec<_>>()
        }))
        .unwrap();

        custom_pins.manifest_sha256 = Box::leak(sha256_hex(&manifest_json).into_boxed_str());

        let bundle_archive = make_bundle_tar(&manifest_json, ca_pem);
        custom_pins.bundle_archive.bytes = bundle_archive.len() as u64;
        custom_pins.bundle_archive.sha256 = Box::leak(sha256_hex(&bundle_archive).into_boxed_str());

        let output_exe = make_test_pe(false, &["kernel32.dll", "crypt32.dll"]);
        let exe_census = pe::parse_pe(&output_exe).unwrap();

        let validation = b"schema=solstone.nvattest-windows-validation.v1\n".to_vec();

        let report = NvattestBuildReport {
            schema: 1,
            source_commit: pins.sdk_revision.into(),
            source_clean: None,
            tools: NvattestReportTools {
                msvc: "14.44.35207".into(),
                vs: serde_json::json!({}),
                windows_sdk: "10.0.26100.0\\".into(),
                cmake: "cmake version 3.31.12".into(),
                rustc: "rustc 1.97.1 (8bab26f4f 2026-07-14)".into(),
                cargo: "cargo 1.97.1 (c980f4866 2026-06-30)".into(),
                crt_redist: "Microsoft.VC143.CRT".into(),
            },
            sources: pins
                .native_sources
                .iter()
                .map(|s| NvattestReportSourceEntry {
                    name: s.name.into(),
                    url: Some(s.url.into()),
                    revision: None,
                    original_sha256: None,
                    sha256: s.sha256.into(),
                })
                .chain(std::iter::once(NvattestReportSourceEntry {
                    name: "regorus-build-revision".into(),
                    url: None,
                    revision: Some(pins.regorus_build_revision.revision.into()),
                    original_sha256: Some(pins.regorus_build_revision.original_sha256.into()),
                    sha256: pins.regorus_build_revision.sha256.into(),
                }))
                .collect(),
            build_tools: pins
                .build_tools
                .iter()
                .map(|s| NvattestReportBuildToolEntry {
                    name: s.name.into(),
                    url: s.url.into(),
                    sha256: s.sha256.into(),
                })
                .collect(),
            ca_bundle: NvattestReportCaBundle {
                url: "https://curl.se/ca/cacert-2026-07-16.pem".into(),
                sha256: custom_pins.ca_bundle.sha256.into(),
            },
            imports: vec!["kernel32.dll".into(), "crypt32.dll".into()],
            outputs: vec![NvattestReportOutputEntry {
                path: NVATTEST_EXE_OUTPUT_LABEL.into(),
                bytes: output_exe.len() as u64,
                sha256: sha256_hex(&output_exe),
                file_version: None,
            }],
        };

        let mut report_raw = vec![0xEF, 0xBB, 0xBF];
        report_raw.extend(serde_json::to_vec(&report).unwrap());
        let report_b64 = base64_encode(&report_raw);

        let evidence = NvattestWindowsBuildEvidence {
            schema: NVATTEST_BUILD_EVIDENCE_SCHEMA_V1.into(),
            report_base64: report_b64,
            census: NvattestToolCensus {
                schema: NVATTEST_TOOL_CENSUS_SCHEMA_V1.into(),
                rustc: NvattestToolVersionPath {
                    version: Some("rustc 1.97.1 (8bab26f4f 2026-07-14)".into()),
                    path: "C:\\rustc.exe".into(),
                    sha256: "a".repeat(64),
                },
                cargo: NvattestToolVersionPath {
                    version: Some("cargo 1.97.1 (c980f4866 2026-06-30)".into()),
                    path: "C:\\cargo.exe".into(),
                    sha256: "b".repeat(64),
                },
                cmake: NvattestToolVersionPath {
                    version: Some("cmake version 3.31.12".into()),
                    path: "C:\\cmake.exe".into(),
                    sha256: "c".repeat(64),
                },
                msvc: NvattestMsvcToolsVersion {
                    vc_tools_version: Some("14.44.35207".into()),
                },
                windows_sdk: NvattestWindowsSdkVersion {
                    version: Some("10.0.26100.0\\".into()),
                },
                vs: Some(NvattestVsInfo {
                    product_version: Some("17.14.60".into()),
                }),
            },
            invocation: NvattestInvocation {
                offline: true,
                bundle_path: "C:\\bundle.tar".into(),
                manifest_sha256: custom_pins.manifest_sha256.into(),
            },
            refusals: NvattestRefusals {
                manifest_digest: NvattestRefusalEntry {
                    exit_code: 1,
                    boundary: "offline manifest does not match the caller-bound digest".into(),
                },
                reuse_dependencies: NvattestRefusalEntry {
                    exit_code: 1,
                    boundary: "offline builds cannot reuse dependencies".into(),
                },
                corrupt_member: NvattestRefusalEntry {
                    exit_code: 1,
                    boundary: "missing or changed offline input: foo".into(),
                },
            },
            network: NvattestNetworkEvidence {
                positive_control: "connected".into(),
                negative_control: "refused".into(),
                rules_remaining: 0,
            },
        };

        let evidence_bytes = serde_json::to_vec(&evidence).unwrap();

        let receipt = ControlledBuildReceipt {
            schema: crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into(),
            source: SourceIdentity {
                product: Provenance {
                    commit: "0".repeat(40),
                    lock_sha256: "0".repeat(64),
                },
                windows_dependency: DependencySource {
                    repository: pins.sdk_repo.into(),
                    revision: pins.sdk_revision.into(),
                    content_sha256: custom_pins.source_archive.sha256.into(),
                },
            },
            inputs: vec![
                crate::controlled_build::InputIdentityEntry {
                    label: "source.tar".into(),
                    sha256: custom_pins.source_archive.sha256.into(),
                    size: custom_pins.source_archive.bytes,
                },
                crate::controlled_build::InputIdentityEntry {
                    label: "bundle.tar".into(),
                    sha256: custom_pins.bundle_archive.sha256.into(),
                    size: custom_pins.bundle_archive.bytes,
                },
            ],
            builder: BuilderIdentity {
                host: "test-builder".into(),
                toolchain: "MSVC 14.44.35207".into(),
            },
            configuration: BuildConfiguration {
                target_triple: "x86_64-pc-windows-msvc".into(),
                profile: "Release".into(),
                flags: vec![],
                network_access_denied: true,
            },
            outputs: vec![OutputIdentityEntry {
                pre_signing_sha256: sha256_hex(&output_exe),
                label: NVATTEST_EXE_OUTPUT_LABEL.into(),
                size: output_exe.len() as u64,
                census: exe_census,
            }],
            supporting: vec![SupportingArtifactRef {
                label: NVATTEST_BUILD_EVIDENCE_LABEL.into(),
                sha256: sha256_hex(&evidence_bytes),
            }],
            validation: ValidationReference {
                description: "nvattest-windows-validation".into(),
                sha256: sha256_hex(&validation),
            },
        };

        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();

        TestFixture {
            pins: custom_pins,
            index,
            receipt: receipt_bytes,
            evidence: evidence_bytes,
            validation,
            notices_body,
            source_archive,
            bundle_archive,
            output_exe,
            output_license: license.to_vec(),
            output_msvcp140: msvcp.clone(),
            output_vcruntime140: vcruntime.clone(),
            output_vcruntime140_1: vcruntime_1.clone(),
            msvc_msvcp140: msvcp,
            msvc_vcruntime140: vcruntime,
            msvc_vcruntime140_1: vcruntime_1,
        }
    }

    fn admit_fixture(f: &TestFixture) -> Result<AdmittedNvattest, String> {
        let input = AdmissionBytes {
            receipt: &f.receipt,
            evidence: &f.evidence,
            validation: &f.validation,
            notices_body: &f.notices_body,
            source_archive: &f.source_archive,
            bundle_archive: &f.bundle_archive,
            output_exe: &f.output_exe,
            output_license: &f.output_license,
            output_msvcp140: &f.output_msvcp140,
            output_vcruntime140: &f.output_vcruntime140,
            output_vcruntime140_1: &f.output_vcruntime140_1,
        };
        let msvc = MsvcRuntimeBytes {
            msvcp140: &f.msvc_msvcp140,
            vcruntime140: &f.msvc_vcruntime140,
            vcruntime140_1: &f.msvc_vcruntime140_1,
        };
        admit(&f.pins, &f.index, &input, &msvc)
    }

    #[test]
    fn intact_synthesized_fixture_admits() {
        let f = setup_fixture();
        let admitted = admit_fixture(&f).unwrap();
        assert_eq!(
            admitted.outputs().keys().cloned().collect::<Vec<_>>(),
            vec![
                NVATTEST_LICENSE_LABEL,
                NVATTEST_EXE_OUTPUT_LABEL,
                NVATTEST_CA_BUNDLE_LABEL
            ]
        );
        assert_eq!(admitted.outputs()[NVATTEST_EXE_OUTPUT_LABEL], f.output_exe);
        assert_eq!(admitted.outputs()[NVATTEST_LICENSE_LABEL], f.output_license);
    }

    #[test]
    fn source_revision_refuses_on_mismatch() {
        let mut f = setup_fixture();
        f.source_archive = make_source_tar("wrong_rev", &f.output_license);
        f.pins.source_archive.bytes = f.source_archive.len() as u64;
        f.pins.source_archive.sha256 = Box::leak(sha256_hex(&f.source_archive).into_boxed_str());
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("source-revision"), "{err}");
    }

    #[test]
    fn source_archive_digest_mismatch_refuses() {
        let mut f = setup_fixture();
        f.pins.source_archive.sha256 = "0".repeat(64).leak();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("source-archive"), "{err}");
    }

    #[test]
    fn bundle_archive_digest_mismatch_refuses() {
        let mut f = setup_fixture();
        f.pins.bundle_archive.sha256 = "0".repeat(64).leak();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("bundle-archive"), "{err}");
    }

    #[test]
    fn unbound_evidence_refuses() {
        let mut f = setup_fixture();
        f.evidence[10] ^= 1;
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("unbound-evidence"), "{err}");
    }

    #[test]
    fn report_output_sha256_mismatch_refuses() {
        let mut f = setup_fixture();
        f.output_exe[100] ^= 1;
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("report-output"), "{err}");
    }

    #[test]
    fn ca_pin_mismatch_refuses() {
        let mut f = setup_fixture();
        f.pins.ca_bundle.sha256 = "0".repeat(64).leak();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("ca-pin"), "{err}");
    }

    #[test]
    fn output_license_mismatch_refuses() {
        let mut f = setup_fixture();
        f.output_license = b"tampered license".to_vec();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("output-license"), "{err}");
    }

    #[test]
    fn runtime_dll_mismatch_refuses() {
        let mut f = setup_fixture();
        f.output_msvcp140[0] ^= 1;
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("runtime-dll"), "{err}");
    }

    #[test]
    fn import_allowlist_unrecognized_import_refuses() {
        let mut f = setup_fixture();
        f.output_exe = make_test_pe(false, &["kernel32.dll", "unauthorized.dll"]);
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("import-allowlist"), "{err}");
    }

    #[test]
    fn delay_import_outside_allowlist_refuses() {
        let mut f = setup_fixture();
        f.output_exe = crate::pe_dependencies::tests::with_import(true);
        let pe_offset = u32::from_le_bytes(f.output_exe[0x3c..0x40].try_into().unwrap()) as usize;
        let mut charac = u16::from_le_bytes(
            f.output_exe[pe_offset + 22..pe_offset + 24]
                .try_into()
                .unwrap(),
        );
        charac &= !0x2000;
        f.output_exe[pe_offset + 22..pe_offset + 24].copy_from_slice(&charac.to_le_bytes());
        // Change the imported DLL name to unauthorized.dll
        f.output_exe[0x280..0x280 + 17].copy_from_slice(b"unauthorized.dll\0");
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("import-allowlist"), "{err}");
    }

    #[test]
    fn forwarder_outside_allowlist_refuses() {
        let mut f = setup_fixture();
        f.output_exe = crate::pe_dependencies::tests::with_forwarder(b"unauthorized.TestFunc\0");
        let pe_offset = u32::from_le_bytes(f.output_exe[0x3c..0x40].try_into().unwrap()) as usize;
        let mut charac = u16::from_le_bytes(
            f.output_exe[pe_offset + 22..pe_offset + 24]
                .try_into()
                .unwrap(),
        );
        charac &= !0x2000;
        f.output_exe[pe_offset + 22..pe_offset + 24].copy_from_slice(&charac.to_le_bytes());
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("import-allowlist"), "{err}");
    }

    #[test]
    fn pe_kind_dll_refuses() {
        let mut f = setup_fixture();
        f.output_exe = make_test_pe(true, &["kernel32.dll"]);
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("pe-kind"), "{err}");
    }

    #[test]
    fn notices_body_mismatch_refuses() {
        let mut f = setup_fixture();
        f.pins.notices_body_sha256 = "0".repeat(64).leak();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("notices-body"), "{err}");
    }

    #[test]
    fn notices_sources_mismatch_refuses() {
        let mut f = setup_fixture();
        f.index.native_sources.pop();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("notices-sources"), "{err}");
    }

    #[test]
    fn notices_index_mismatch_refuses() {
        let mut f = setup_fixture();
        f.index.schema = "wrong.schema".into();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("notices-index"), "{err}");
    }

    #[test]
    fn tool_census_disagreement_refuses() {
        let mut f = setup_fixture();
        let mut evidence: NvattestWindowsBuildEvidence =
            serde_json::from_slice(&f.evidence).unwrap();
        evidence.census.msvc.vc_tools_version = Some("14.44.35207".into());
        // Modify report msvc
        let report_bytes = base64_decode_report(&evidence.report_base64).unwrap();
        let mut report = decode_build_report(&report_bytes).unwrap();
        report.tools.msvc = "14.44.99999".into();
        let mut report_raw = vec![0xEF, 0xBB, 0xBF];
        report_raw.extend(serde_json::to_vec(&report).unwrap());
        evidence.report_base64 = base64_encode(&report_raw);
        f.evidence = serde_json::to_vec(&evidence).unwrap();
        // Update receipt supporting artifact hash to match
        let mut receipt: ControlledBuildReceipt = serde_json::from_slice(&f.receipt).unwrap();
        receipt.supporting[0].sha256 = sha256_hex(&f.evidence);
        f.receipt = serde_json::to_vec(&receipt).unwrap();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("tool-census"), "{err}");
    }

    #[test]
    fn census_version_null_missing_or_token_mismatch_refuses() {
        let mut f = setup_fixture();
        let mut evidence: NvattestWindowsBuildEvidence =
            serde_json::from_slice(&f.evidence).unwrap();
        evidence.census.rustc.version = Some("rustc 1.97.10 (8bab26f4f 2026-07-14)".into());
        f.evidence = serde_json::to_vec(&evidence).unwrap();
        let mut receipt: ControlledBuildReceipt = serde_json::from_slice(&f.receipt).unwrap();
        receipt.supporting[0].sha256 = sha256_hex(&f.evidence);
        f.receipt = serde_json::to_vec(&receipt).unwrap();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("census-version"), "{err}");

        // Test Windows SDK version format
        let mut f2 = setup_fixture();
        let mut evidence2: NvattestWindowsBuildEvidence =
            serde_json::from_slice(&f2.evidence).unwrap();
        evidence2.census.windows_sdk.version = Some("10.0.26100.01\\".into());
        f2.evidence = serde_json::to_vec(&evidence2).unwrap();
        let mut receipt2: ControlledBuildReceipt = serde_json::from_slice(&f2.receipt).unwrap();
        receipt2.supporting[0].sha256 = sha256_hex(&f2.evidence);
        f2.receipt = serde_json::to_vec(&receipt2).unwrap();
        let err2 = admit_fixture(&f2).unwrap_err();
        assert!(err2.contains("census-version"), "{err2}");
    }

    #[test]
    fn regorus_build_revision_mismatch_refuses() {
        let mut f = setup_fixture();
        let mut evidence: NvattestWindowsBuildEvidence =
            serde_json::from_slice(&f.evidence).unwrap();
        let report_bytes = base64_decode_report(&evidence.report_base64).unwrap();
        let mut report = decode_build_report(&report_bytes).unwrap();
        for s in &mut report.sources {
            if s.name == "regorus-build-revision" {
                s.sha256 = "0".repeat(64);
            }
        }
        let mut report_raw = vec![0xEF, 0xBB, 0xBF];
        report_raw.extend(serde_json::to_vec(&report).unwrap());
        evidence.report_base64 = base64_encode(&report_raw);
        f.evidence = serde_json::to_vec(&evidence).unwrap();
        let mut receipt: ControlledBuildReceipt = serde_json::from_slice(&f.receipt).unwrap();
        receipt.supporting[0].sha256 = sha256_hex(&f.evidence);
        f.receipt = serde_json::to_vec(&receipt).unwrap();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("regorus-build-revision"), "{err}");
    }

    #[test]
    fn captured_fixture_import_names_admit() {
        let mut f = setup_fixture();
        let captured_imports = [
            "ADVAPI32.dll",
            "USER32.dll",
            "bcrypt.dll",
            "CRYPT32.dll",
            "kernel32.dll",
            "api-ms-win-core-synch-l1-2-0.dll",
            "bcryptprimitives.dll",
            "ntdll.dll",
            "WS2_32.dll",
            "SHELL32.dll",
            "MSVCP140.dll",
            "VCRUNTIME140.dll",
            "VCRUNTIME140_1.dll",
            "api-ms-win-crt-runtime-l1-1-0.dll",
            "api-ms-win-crt-stdio-l1-1-0.dll",
            "api-ms-win-crt-filesystem-l1-1-0.dll",
            "api-ms-win-crt-heap-l1-1-0.dll",
            "api-ms-win-crt-convert-l1-1-0.dll",
            "api-ms-win-crt-environment-l1-1-0.dll",
            "api-ms-win-crt-string-l1-1-0.dll",
            "api-ms-win-crt-locale-l1-1-0.dll",
            "api-ms-win-crt-math-l1-1-0.dll",
            "api-ms-win-crt-time-l1-1-0.dll",
            "api-ms-win-crt-utility-l1-1-0.dll",
        ];
        f.output_exe = make_test_pe(false, &captured_imports);
        let mut receipt: ControlledBuildReceipt = serde_json::from_slice(&f.receipt).unwrap();
        receipt.outputs[0].size = f.output_exe.len() as u64;
        receipt.outputs[0].pre_signing_sha256 = sha256_hex(&f.output_exe);
        receipt.outputs[0].census = pe::parse_pe(&f.output_exe).unwrap();
        f.receipt = serde_json::to_vec(&receipt).unwrap();

        let mut evidence: NvattestWindowsBuildEvidence =
            serde_json::from_slice(&f.evidence).unwrap();
        let report_bytes = base64_decode_report(&evidence.report_base64).unwrap();
        let mut report = decode_build_report(&report_bytes).unwrap();
        report.outputs[0].bytes = f.output_exe.len() as u64;
        report.outputs[0].sha256 = sha256_hex(&f.output_exe);
        let mut report_raw = vec![0xEF, 0xBB, 0xBF];
        report_raw.extend(serde_json::to_vec(&report).unwrap());
        evidence.report_base64 = base64_encode(&report_raw);
        f.evidence = serde_json::to_vec(&evidence).unwrap();
        let mut receipt_obj: ControlledBuildReceipt = serde_json::from_slice(&f.receipt).unwrap();
        receipt_obj.supporting[0].sha256 = sha256_hex(&f.evidence);
        f.receipt = serde_json::to_vec(&receipt_obj).unwrap();

        assert!(admit_fixture(&f).is_ok());
    }

    #[test]
    fn parse_test_with_real_fixture_bytes() {
        let report_bytes = include_bytes!("../fixtures/nvattest-windows-build/build-report.json");
        let report = decode_build_report(report_bytes).unwrap();
        assert_eq!(
            report.source_commit,
            "8fdbb0f8c10594a5f88f77fdec4766803b4e6d59"
        );
        assert_eq!(report.tools.msvc, "14.44.35207");
        assert_eq!(report.tools.windows_sdk, "10.0.26100.0\\");
        assert_eq!(
            report
                .outputs
                .iter()
                .find(|o| o.path == "bin/nvattest.exe")
                .unwrap()
                .sha256,
            "220849fea69d60563fc6ef0ea7c020450d842565d08e7cbd2eecfecbd41ecdc8"
        );
    }

    #[test]
    fn production_pins_match_committed_constants() {
        let pins = production_pins();
        let _ = std::env::var("NVAT_SOURCE_COMMIT");
        let _ = std::env::var("SOLSTONE_NVATTEST_REVISION");
        assert_eq!(
            pins.sdk_revision,
            "8fdbb0f8c10594a5f88f77fdec4766803b4e6d59"
        );
        assert_eq!(pins.source_archive.bytes, 5171200);
        assert_eq!(pins.bundle_archive.bytes, 451225600);
        assert_eq!(pins.ca_bundle.bytes, 186446);
        assert_eq!(pins.license.bytes, 11348);
    }

    #[test]
    fn pe32_output_fails_census() {
        let mut f = setup_fixture();
        f.output_exe = pe::fixture_pe32();
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("dependency census requires PE32+"), "{err}");
    }

    #[test]
    fn arm64_output_fails_census() {
        let mut f = setup_fixture();
        f.output_exe = pe::fixture(&pe::FixtureSpec {
            machine: pe::IMAGE_FILE_MACHINE_ARM64,
            dll: false,
            ..pe::FixtureSpec::default()
        });
        let err = admit_fixture(&f).unwrap_err();
        assert!(
            err.contains("Windows payload requires AMD64 PE images"),
            "{err}"
        );
    }

    #[test]
    fn report_ca_mismatch_fails_ca_report() {
        let mut f = setup_fixture();
        let mut evidence: NvattestWindowsBuildEvidence =
            serde_json::from_slice(&f.evidence).unwrap();
        let report_bytes = base64_decode_report(&evidence.report_base64).unwrap();
        let mut report = decode_build_report(&report_bytes).unwrap();
        report.ca_bundle.sha256 = "0".repeat(64);
        let mut report_raw = vec![0xEF, 0xBB, 0xBF];
        report_raw.extend(serde_json::to_vec(&report).unwrap());
        evidence.report_base64 = base64_encode(&report_raw);
        f.evidence = serde_json::to_vec(&evidence).unwrap();

        let mut receipt: ControlledBuildReceipt = serde_json::from_slice(&f.receipt).unwrap();
        receipt.supporting[0].sha256 = sha256_hex(&f.evidence);
        f.receipt = serde_json::to_vec(&receipt).unwrap();

        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("ca-report"), "{err}");
    }

    #[test]
    fn missing_or_null_census_version_fails() {
        let mut f = setup_fixture();
        let mut evidence: NvattestWindowsBuildEvidence =
            serde_json::from_slice(&f.evidence).unwrap();
        evidence.census.rustc.version = None;
        f.evidence = serde_json::to_vec(&evidence).unwrap();
        let mut receipt: ControlledBuildReceipt = serde_json::from_slice(&f.receipt).unwrap();
        receipt.supporting[0].sha256 = sha256_hex(&f.evidence);
        f.receipt = serde_json::to_vec(&receipt).unwrap();

        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("census-version"), "{err}");

        // Second case: delete version key from census rustc object in evidence JSON
        let mut evidence_val: serde_json::Value = serde_json::from_slice(&f.evidence).unwrap();
        evidence_val["census"]["rustc"]
            .as_object_mut()
            .unwrap()
            .remove("version");
        f.evidence = serde_json::to_vec(&evidence_val).unwrap();
        let mut receipt2: ControlledBuildReceipt = serde_json::from_slice(&f.receipt).unwrap();
        receipt2.supporting[0].sha256 = sha256_hex(&f.evidence);
        f.receipt = serde_json::to_vec(&receipt2).unwrap();

        let err2 = admit_fixture(&f).unwrap_err();
        assert!(err2.contains("census-version"), "{err2}");
    }

    #[test]
    fn flipped_notices_body_fails() {
        let mut f = setup_fixture();
        f.notices_body[0] ^= 0xff;
        let err = admit_fixture(&f).unwrap_err();
        assert!(err.contains("notices-body"), "{err}");
    }

    #[test]
    #[ignore = "requires real retained archives SOLSTONE_NVATTEST_SOURCE_ARCHIVE, SOLSTONE_NVATTEST_BUNDLE_ARCHIVE, SOLSTONE_NVATTEST_ADMISSION_FIXTURE"]
    fn nvattest_retained_archives_admit() {
        let source_path = std::env::var("SOLSTONE_NVATTEST_SOURCE_ARCHIVE").unwrap();
        let bundle_path = std::env::var("SOLSTONE_NVATTEST_BUNDLE_ARCHIVE").unwrap();
        let fixture_path = std::env::var("SOLSTONE_NVATTEST_ADMISSION_FIXTURE").unwrap();
        let fixture_bytes = std::fs::read(fixture_path).unwrap();
        let values: BTreeMap<String, String> = serde_json::from_slice(&fixture_bytes).unwrap();

        let receipt = std::fs::read(&values["receipt"]).unwrap();
        let evidence = std::fs::read(&values["evidence"]).unwrap();
        let validation = std::fs::read(&values["validation"]).unwrap();
        let source_archive = std::fs::read(source_path).unwrap();
        let bundle_archive = std::fs::read(bundle_path).unwrap();
        let output_exe = std::fs::read(&values["output_exe"]).unwrap();
        let output_license = std::fs::read(&values["output_license"]).unwrap();
        let output_msvcp140 = std::fs::read(&values["output_msvcp140"]).unwrap();
        let output_vcruntime140 = std::fs::read(&values["output_vcruntime140"]).unwrap();
        let output_vcruntime140_1 = std::fs::read(&values["output_vcruntime140_1"]).unwrap();

        let msvc_msvcp140 = std::fs::read(&values["msvc_msvcp140"]).unwrap();
        let msvc_vcruntime140 = std::fs::read(&values["msvc_vcruntime140"]).unwrap();
        let msvc_vcruntime140_1 = std::fs::read(&values["msvc_vcruntime140_1"]).unwrap();

        let notices_body = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../distribution/nvattest-windows-NOTICES.md"),
        )
        .unwrap();

        let index: NoticesIndex = serde_json::from_slice(include_bytes!(
            "../../../distribution/nvattest-windows-sources.json"
        ))
        .unwrap();

        let input = AdmissionBytes {
            receipt: &receipt,
            evidence: &evidence,
            validation: &validation,
            notices_body: &notices_body,
            source_archive: &source_archive,
            bundle_archive: &bundle_archive,
            output_exe: &output_exe,
            output_license: &output_license,
            output_msvcp140: &output_msvcp140,
            output_vcruntime140: &output_vcruntime140,
            output_vcruntime140_1: &output_vcruntime140_1,
        };
        let msvc = MsvcRuntimeBytes {
            msvcp140: &msvc_msvcp140,
            vcruntime140: &msvc_vcruntime140,
            vcruntime140_1: &msvc_vcruntime140_1,
        };

        admit(&production_pins(), &index, &input, &msvc).unwrap();
    }
}
