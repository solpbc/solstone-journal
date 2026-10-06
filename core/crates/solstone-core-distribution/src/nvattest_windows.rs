// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure admission and evidence verification for the Windows NVIDIA GPU
//! attestation verifier (`nvattest.exe`).
//!
//! Input files provide no authority, digests, destinations, or overrides.
//! Output identity, source revision, offline tools, and notices are strictly
//! bound to committed pins and inventory configuration. The pins value is
//! crate-private: production builds it only from the constants below.

use std::collections::BTreeSet;
use std::io::{Cursor, Read};

use crate::controlled_build::{ControlledBuildReceipt, decode_controlled_build_receipt};
use crate::digest::sha256_hex;
use crate::pe_dependencies::{PeDependencies, inspect_dependencies};

pub const NVATTEST_BUILD_EVIDENCE_SCHEMA_V1: &str = "solstone.nvattest-windows-build-evidence.v1";
pub const NVATTEST_BUILD_EVIDENCE_LABEL: &str =
    "provenance/windows-x86_64/nvattest-build-evidence.json";
pub const NVATTEST_TOOL_CENSUS_SCHEMA_V1: &str = "solstone.nvattest-windows-tool-census.v1";
pub const NVATTEST_DRIVER_CONTROLS_SCHEMA_V1: &str = "solstone.nvattest-windows-driver-controls.v1";
pub const NVATTEST_NOTICES_INDEX_SCHEMA_V1: &str = "solstone.nvattest-windows-notices.v1";
pub const NVATTEST_EXE_OUTPUT_LABEL: &str = "bin/nvattest.exe";
pub const NVATTEST_CA_BUNDLE_LABEL: &str = "share/ca/ca-bundle.pem";
pub const NVATTEST_LICENSE_LABEL: &str = "LICENSE";
pub const NVATTEST_RUNTIME_OUTPUT_LABELS: [&str; 3] = [
    "bin/msvcp140.dll",
    "bin/vcruntime140.dll",
    "bin/vcruntime140_1.dll",
];

/// The receipt's inputs, as the recorder writes them: exactly these two, in
/// this order, at the pinned archive identities.
pub const NVATTEST_SOURCE_INPUT_LABEL: &str = "source.tar";
pub const NVATTEST_BUNDLE_INPUT_LABEL: &str = "bundle.tar";
/// The receipt's configuration, as the recorder writes it.
pub const NVATTEST_TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
pub const NVATTEST_BUILD_PROFILE: &str = "Release";
pub const NVATTEST_VALIDATION_DESCRIPTION: &str = "nvattest-windows-validation";

pub const NVATTEST_NOTICES_INDEX_PATH: &str = "core/distribution/nvattest-windows-sources.json";
pub const NVATTEST_NOTICES_BODY_PATH: &str = "core/distribution/nvattest-windows-NOTICES.md";
const NOTICES_WINDOWS_POPULATION_MARKER: &str = "windows-link 0.2.1";
const NOTICES_RUST_STANDARD_LIBRARY: &str = "1.97.1";
const NOTICES_LIVE_IN: &str = "journal";

const REGORUS_BUILD_REVISION_NAME: &str = "regorus-build-revision";
const BUNDLE_MANIFEST_MEMBER: &str = "offline-manifest.json";

/// Each refusal control must stop at exactly this SDK boundary message.
pub const NVATTEST_REFUSAL_MANIFEST_BOUNDARY: &str =
    "offline manifest does not match the caller-bound digest";
pub const NVATTEST_REFUSAL_REUSE_BOUNDARY: &str = "offline builds cannot reuse dependencies";
/// The driver flips one byte of this bundle member for the changed-input control.
pub const NVATTEST_REFUSAL_CORRUPT_MEMBER: &str = "openssl-3.6.1.tar.gz";
pub const NVATTEST_REFUSAL_CORRUPT_BOUNDARY: &str =
    "missing or changed offline input: openssl-3.6.1.tar.gz";

pub const NVATTEST_NETWORK_CONNECTED: &str = "connected";
pub const NVATTEST_NETWORK_REFUSED: &str = "refused";

/// The complete environment every SDK child receives, in ordinal order. The
/// driver clears everything else; `NVAT_SOURCE_COMMIT` is the committed
/// revision, `RUSTC` and the leading `PATH` entry are the selected toolchain,
/// and `USERPROFILE`/`TEMP`/`TMP` are fresh directories under the work root.
pub const NVATTEST_SDK_CHILD_ENVIRONMENT: &[&str] = &[
    "ComSpec",
    "NUMBER_OF_PROCESSORS",
    "NVAT_SOURCE_COMMIT",
    "PATH",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "RUSTC",
    "SystemDrive",
    "SystemRoot",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "windir",
];

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
pub(crate) struct Pins {
    pub(crate) sdk_repo: &'static str,
    pub(crate) sdk_revision: &'static str,
    pub(crate) source_archive: ArchivePin,
    pub(crate) bundle_archive: ArchivePin,
    pub(crate) manifest_sha256: &'static str,
    pub(crate) ca_bundle: FilePin,
    pub(crate) license: FilePin,
    pub(crate) regorus_cargo_lock: FilePin,
    pub(crate) notices_body_sha256: &'static str,
    pub(crate) body_assembly_revision: &'static str,
    /// The SDK report's native source list, in the report's order.
    pub(crate) native_sources: &'static [NativeSourcePin],
    pub(crate) build_tools: &'static [DownloadSourcePin],
    pub(crate) msvc_runtime: MsvcRuntimePins,
    pub(crate) toolchain: ToolchainPins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArchivePin {
    pub(crate) bytes: u64,
    pub(crate) sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FilePin {
    pub(crate) member: &'static str,
    pub(crate) bytes: u64,
    pub(crate) sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MsvcRuntimePins {
    pub(crate) msvcp140: FilePin,
    pub(crate) vcruntime140: FilePin,
    pub(crate) vcruntime140_1: FilePin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DownloadSourcePin {
    pub(crate) name: &'static str,
    pub(crate) url: &'static str,
    pub(crate) sha256: &'static str,
}

/// The archived build script substitution the SDK records in place of a Git
/// revision. It has no URL and is not a bundle member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BuildRevisionPin {
    pub(crate) revision: &'static str,
    pub(crate) original_sha256: &'static str,
    pub(crate) sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeSourcePin {
    Download(DownloadSourcePin),
    BuildRevision(BuildRevisionPin),
}

/// Version tokens are compared exactly. Executable digests are the files the
/// build resolved: the selected toolchain's `rustc.exe`/`cargo.exe` and the
/// `cmake.exe` inside the pinned bundle's CMake archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolchainPins {
    pub(crate) rustc_version: &'static str,
    pub(crate) rustc_commit: &'static str,
    pub(crate) rustc_exe_sha256: &'static str,
    pub(crate) cargo_version: &'static str,
    pub(crate) cargo_exe_sha256: &'static str,
    pub(crate) cmake_version: &'static str,
    pub(crate) cmake_exe_sha256: &'static str,
    pub(crate) msvc_toolset: &'static str,
    pub(crate) windows_sdk: &'static str,
}

const fn download(name: &'static str, url: &'static str, sha256: &'static str) -> NativeSourcePin {
    NativeSourcePin::Download(DownloadSourcePin { name, url, sha256 })
}

const NATIVE_SOURCES: &[NativeSourcePin] = &[
    download(
        "openssl-3.6.1.tar.gz",
        "https://github.com/openssl/openssl/releases/download/openssl-3.6.1/openssl-3.6.1.tar.gz",
        "b1bfedcd5b289ff22aee87c9d600f515767ebf45f77168cb6d64f231f518a82e",
    ),
    download(
        "libxml2-2.11.9.tar.xz",
        "https://download.gnome.org/sources/libxml2/2.11/libxml2-2.11.9.tar.xz",
        "780157a1efdb57188ec474dca87acaee67a3a839c2525b2214d318228451809f",
    ),
    download(
        "xmlsec1-1.2.39.tar.gz",
        "https://github.com/lsh123/xmlsec/releases/download/xmlsec-1_2_39/xmlsec1-1.2.39.tar.gz",
        "15f2f55ea5968e578fcd24b3b427e553876c86c147dc7f03923e98fc2768a1fa",
    ),
    download(
        "curl-7.88.1.tar.gz",
        "https://github.com/curl/curl/releases/download/curl-7_88_1/curl-7.88.1.tar.gz",
        "cdb38b72e36bc5d33d5b8810f8018ece1baa29a8f215b4495e495ded82bbf3c7",
    ),
    download(
        "zlib-1.3.1.tar.gz",
        "https://zlib.net/fossils/zlib-1.3.1.tar.gz",
        "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23",
    ),
    download(
        "cli11-bfffd37e1f804ca4fae1caae106935791696b6a9.tar.gz",
        "https://codeload.github.com/CLIUtils/CLI11/tar.gz/bfffd37e1f804ca4fae1caae106935791696b6a9",
        "03c9b7921b8f99ca39ae660b03ebf9bd5a3f4280201f7d04bc5639b1e0496401",
    ),
    download(
        "corrosion-6be991bb34c348dfb8344be22f3606288ea5c7fd.tar.gz",
        "https://codeload.github.com/corrosion-rs/corrosion/tar.gz/6be991bb34c348dfb8344be22f3606288ea5c7fd",
        "84d8fbc2810af9a42e250411dcd30b8e8cc59deba196c1dcd1fb44fec459a793",
    ),
    download(
        "regorus-c7bf460bc160c96e38048296e5708943d2e43909.tar.gz",
        "https://codeload.github.com/microsoft/regorus/tar.gz/c7bf460bc160c96e38048296e5708943d2e43909",
        "188805b3b44b1cd2f9e8fd9b74a281b5f8eaa1c1b4087fd442afc88eccfef7cd",
    ),
    NativeSourcePin::BuildRevision(BuildRevisionPin {
        revision: "c7bf460bc160c96e38048296e5708943d2e43909",
        original_sha256: "7dc931d2a3cc9203b9cf63c9da29e85122f8b10ae2b1a4318494eacc178a3956",
        sha256: "bef3c5f151c9f48f2e0bcf9a71ceb7c4d7ad9c5aec723d483076d4247ba86c15",
    }),
    download(
        "jwt-cpp-e71e0c2d584baff06925bbb3aad683f677e4d498.tar.gz",
        "https://codeload.github.com/Thalhammer/jwt-cpp/tar.gz/e71e0c2d584baff06925bbb3aad683f677e4d498",
        "1988cbe1c930638ac4341578fb0fc0616fe39cf468a9087624b27581d81f8b51",
    ),
    download(
        "fmt-e69e5f977d458f2650bb346dadf2ad30c5320281.tar.gz",
        "https://codeload.github.com/fmtlib/fmt/tar.gz/e69e5f977d458f2650bb346dadf2ad30c5320281",
        "1723f27eed50e751037f49dcdf73e33b17658f1178ea1c1f829a30bb02335745",
    ),
    download(
        "spdlog-27cb4c76708608465c413f6d0e6b8d99a4d84302.tar.gz",
        "https://codeload.github.com/gabime/spdlog/tar.gz/27cb4c76708608465c413f6d0e6b8d99a4d84302",
        "7d512b37019b61646cd6fd1e48f52a3cf3e098f36b33ba9c059fe51301ff40b3",
    ),
    download(
        "json-3.12.0.tar.xz",
        "https://github.com/nlohmann/json/releases/download/v3.12.0/json.tar.xz",
        "42f6e95cad6ec532fd372391373363b62a14af6d771056dbfc86160e6dfff7aa",
    ),
];

const BUILD_TOOLS: &[DownloadSourcePin] = &[
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
];

pub(crate) const fn production_pins() -> Pins {
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
        native_sources: NATIVE_SOURCES,
        build_tools: BUILD_TOOLS,
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
        toolchain: ToolchainPins {
            rustc_version: "1.97.1",
            rustc_commit: "8bab26f4f",
            rustc_exe_sha256: "cf79cfd77b0a144c56a0a6af6bf10bcdf095a73718cd4bf2b9d4fe2d2cbded55",
            cargo_version: "1.97.1",
            cargo_exe_sha256: "ddfbad20b31b918d3439d070945ec59bbfe037a6ec0ab5b584459e69c8b37d1b",
            cmake_version: "3.31.12",
            cmake_exe_sha256: "f3a124ad60459b56b2a5b4e312b0cd5de56d5b1fcf2921f8f602fb531b0da10d",
            msvc_toolset: "14.44.35207",
            windows_sdk: "10.0.26100.0",
        },
    }
}

pub(crate) struct AdmissionBytes<'a> {
    pub(crate) receipt: &'a [u8],
    pub(crate) evidence: &'a [u8],
    pub(crate) validation: &'a [u8],
    pub(crate) notices_index: &'a [u8],
    pub(crate) notices_body: &'a [u8],
    pub(crate) source_archive: &'a [u8],
    pub(crate) bundle_archive: &'a [u8],
    pub(crate) output_exe: &'a [u8],
    pub(crate) output_license: &'a [u8],
    pub(crate) output_msvcp140: &'a [u8],
    pub(crate) output_vcruntime140: &'a [u8],
    pub(crate) output_vcruntime140_1: &'a [u8],
}

/// The payload's admitted MSVC runtime members; the verifier's build copies
/// must equal these byte for byte.
pub struct MsvcRuntimeBytes<'a> {
    pub msvcp140: &'a [u8],
    pub vcruntime140: &'a [u8],
    pub vcruntime140_1: &'a [u8],
}

#[derive(Debug)]
pub(crate) struct AdmittedNvattest {
    receipt: ControlledBuildReceipt,
    receipt_bytes: Vec<u8>,
    evidence_bytes: Vec<u8>,
    validation_bytes: Vec<u8>,
    outputs: std::collections::BTreeMap<String, Vec<u8>>,
}

impl AdmittedNvattest {
    pub(crate) fn receipt(&self) -> &ControlledBuildReceipt {
        &self.receipt
    }

    #[cfg(test)]
    pub(crate) fn outputs(&self) -> &std::collections::BTreeMap<String, Vec<u8>> {
        &self.outputs
    }

    pub(crate) fn into_admitted_controlled_input(
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
    /// The SDK's `build-report.json` exactly as written, BOM included.
    pub report_base64: String,
    pub census: NvattestToolCensus,
    pub invocation: NvattestInvocation,
    pub refusals: NvattestRefusals,
    pub network: NvattestNetworkEvidence,
    pub dumpbin_dependents: NvattestDumpbinDependents,
}

/// What the driver measured around the SDK build, handed to the recorder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestDriverControls {
    pub schema: String,
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
    pub cl: NvattestToolVersionPath,
    pub link: NvattestToolVersionPath,
    pub nmake: NvattestToolVersionPath,
    pub msbuild: NvattestToolVersionPath,
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
    /// The `NVAT_SOURCE_COMMIT` value the driver set for every SDK child.
    pub source_commit: String,
    /// The names of every variable in the SDK children's environment.
    pub environment: Vec<String>,
    pub argv: Vec<String>,
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
    pub transport_peer: String,
    pub ipv4: NvattestNetworkProbe,
    pub ipv6: NvattestNetworkProbe,
    pub rules_added: u32,
    pub rules_remaining: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestNetworkProbe {
    pub target: String,
    pub positive_control: String,
    pub negative_control: String,
}

/// `dumpbin /dependents` text, base64-encoded so its bytes survive JSON.
/// Corroborating only; imports are always parsed from the admitted bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvattestDumpbinDependents {
    pub nvattest_exe: String,
    pub msvcp140: String,
    pub vcruntime140: String,
    pub vcruntime140_1: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_sha256: Option<String>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
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
    schema: u32,
    files: Vec<BundleManifestFile>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleManifestFile {
    path: String,
    sha256: String,
    size: u64,
}

impl NativeSourcePin {
    fn report_entry(&self) -> NvattestReportSourceEntry {
        match self {
            Self::Download(pin) => NvattestReportSourceEntry {
                name: pin.name.into(),
                url: Some(pin.url.into()),
                revision: None,
                original_sha256: None,
                sha256: pin.sha256.into(),
            },
            Self::BuildRevision(pin) => NvattestReportSourceEntry {
                name: REGORUS_BUILD_REVISION_NAME.into(),
                url: None,
                revision: Some(pin.revision.into()),
                original_sha256: Some(pin.original_sha256.into()),
                sha256: pin.sha256.into(),
            },
        }
    }
}

pub(crate) fn admit(
    pins: &Pins,
    input: &AdmissionBytes<'_>,
    msvc: &MsvcRuntimeBytes<'_>,
) -> Result<AdmittedNvattest, String> {
    // 1. The committed notices index and body bind the pins they were built for.
    let index: NoticesIndex =
        serde_json::from_slice(input.notices_index).map_err(|e| format!("notices-index: {e}"))?;
    validate_notices_index(pins, &index)?;
    let notices_body_sha = sha256_hex(input.notices_body);
    if notices_body_sha != pins.notices_body_sha256 {
        return Err("notices-body: notices body SHA-256 differs from pin".into());
    }
    if notices_body_sha != index.notices_body_sha256 {
        return Err("notices-body: notices body SHA-256 differs from index".into());
    }

    // 2. Both archives at their pinned identities before any member is read.
    require_archive("source-archive", input.source_archive, &pins.source_archive)?;
    require_archive("bundle-archive", input.bundle_archive, &pins.bundle_archive)?;

    // 3. The executable's own dependencies, from the admitted bytes.
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
        if !IMPORT_ALLOWLIST.contains(&import.to_ascii_lowercase().as_str()) {
            return Err(format!(
                "import-allowlist: unrecognized PE import dependency: {import}"
            ));
        }
    }

    // 4. The receipt binds its documents, the pinned source, inputs and
    //    configuration, and exactly the executable presented here.
    let receipt = decode_controlled_build_receipt(input.receipt).map_err(|e| e.to_string())?;
    crate::produce::windows_inputs::verify_receipt_document_binding(
        &receipt,
        input.evidence,
        input.validation,
        NVATTEST_BUILD_EVIDENCE_LABEL,
    )?;
    validate_receipt(pins, &receipt)?;
    let output_entry = &receipt.outputs[0];
    if input.output_exe.len() as u64 != output_entry.size
        || sha256_hex(input.output_exe) != output_entry.pre_signing_sha256
    {
        return Err("receipt-output: output exe does not match receipt output identity".into());
    }

    // 5. The evidence document the receipt binds.
    let evidence: NvattestWindowsBuildEvidence =
        serde_json::from_slice(input.evidence).map_err(|e| format!("evidence: {e}"))?;
    if evidence.schema != NVATTEST_BUILD_EVIDENCE_SCHEMA_V1 {
        return Err(format!(
            "evidence: schema must be {NVATTEST_BUILD_EVIDENCE_SCHEMA_V1}"
        ));
    }
    validate_invocation(pins, &evidence.invocation)?;
    validate_refusals(&evidence.refusals)?;
    validate_network(&evidence.network)?;
    validate_dumpbin(&evidence.dumpbin_dependents, &pe)?;

    // 6. The raw SDK report, decoded and re-parsed rather than summarized.
    let report_bytes = base64_decode(&evidence.report_base64)?;
    let report = decode_build_report(&report_bytes)?;
    if report.schema != 1 {
        return Err("report: SDK build report schema must be 1".into());
    }
    if report.source_commit != pins.sdk_revision {
        return Err("source-revision: report source_commit differs from pin".into());
    }
    validate_census(pins, &evidence.census, &report)?;
    validate_report_sources(pins, &report)?;
    validate_report_outputs(pins, &report, output_entry)?;
    validate_report_imports(&report, &pe)?;

    // 7. Source archive members: the git archive commit, the staged LICENSE
    //    and the regorus lock the notices index was built against.
    let source = extract_source_tar_members(input.source_archive, pins)?;
    if source.pax_comment.as_deref() != Some(pins.sdk_revision) {
        return Err("source-revision: source archive pax comment differs from pin".into());
    }
    require_file_pin("source-archive", &source.license, &pins.license)?;
    require_file_pin(
        "source-archive",
        &source.regorus_lock,
        &pins.regorus_cargo_lock,
    )?;
    if input.output_license != source.license.as_slice() {
        return Err("output-license: output root LICENSE differs from archive member".into());
    }

    // 8. Bundle members: the caller-bound manifest and the CA that is staged.
    let bundle = extract_bundle_tar_members(input.bundle_archive, pins)?;
    if sha256_hex(&bundle.manifest) != pins.manifest_sha256 {
        return Err("bundle-archive: bundle offline-manifest.json differs from pin".into());
    }
    let bundle_manifest: BundleManifest = serde_json::from_slice(&bundle.manifest)
        .map_err(|e| format!("bundle-archive: offline manifest: {e}"))?;
    if bundle_manifest.schema != 1 {
        return Err("bundle-archive: bundle offline-manifest schema must be 1".into());
    }
    let bundle_ca_sha256 = sha256_hex(&bundle.ca);
    if bundle.ca.len() as u64 != pins.ca_bundle.bytes || bundle_ca_sha256 != pins.ca_bundle.sha256 {
        if bundle_ca_sha256 == report.ca_bundle.sha256 {
            return Err("ca-pin: bundle CA matches report but differs from pin".into());
        }
        return Err("ca-pin: bundle ca-bundle.pem differs from pin".into());
    }
    if bundle_ca_sha256 != report.ca_bundle.sha256 {
        return Err("ca-report: bundle CA matches pin but differs from report".into());
    }
    validate_bundle_manifest(pins, &bundle_manifest)?;

    // 9. The build's runtime copies are the payload's admitted MSVC members.
    validate_crt_members(
        pins,
        msvc,
        input.output_msvcp140,
        input.output_vcruntime140,
        input.output_vcruntime140_1,
    )?;

    let mut outputs = std::collections::BTreeMap::new();
    outputs.insert(NVATTEST_EXE_OUTPUT_LABEL.into(), input.output_exe.to_vec());
    // The CA bytes compared above are the bytes that are staged.
    outputs.insert(NVATTEST_CA_BUNDLE_LABEL.into(), bundle.ca);
    outputs.insert(NVATTEST_LICENSE_LABEL.into(), source.license);

    Ok(AdmittedNvattest {
        receipt,
        receipt_bytes: input.receipt.to_vec(),
        evidence_bytes: input.evidence.to_vec(),
        validation_bytes: input.validation.to_vec(),
        outputs,
    })
}

fn require_archive(label: &str, bytes: &[u8], pin: &ArchivePin) -> Result<(), String> {
    if bytes.len() as u64 != pin.bytes || sha256_hex(bytes) != pin.sha256 {
        return Err(format!("{label}: size or SHA-256 differs from pin"));
    }
    Ok(())
}

fn require_file_pin(label: &str, bytes: &[u8], pin: &FilePin) -> Result<(), String> {
    if bytes.len() as u64 != pin.bytes || sha256_hex(bytes) != pin.sha256 {
        return Err(format!(
            "{label}: member {} size or SHA-256 differs from pin",
            pin.member
        ));
    }
    Ok(())
}

fn validate_notices_index(pins: &Pins, index: &NoticesIndex) -> Result<(), String> {
    if index.schema != NVATTEST_NOTICES_INDEX_SCHEMA_V1 {
        return Err("notices-index: schema mismatch".into());
    }
    if index.notices_body_sha256 != pins.notices_body_sha256 {
        return Err("notices-body: index notices body SHA-256 differs from pin".into());
    }
    if index.body_assembly_revision != pins.body_assembly_revision {
        return Err("notices-index: body assembly revision mismatch".into());
    }
    if index.admitted_sdk_revision != pins.sdk_revision {
        return Err("notices-index: admitted SDK revision differs from the revision pin".into());
    }
    if index.regorus_cargo_lock.member != pins.regorus_cargo_lock.member
        || index.regorus_cargo_lock.sha256 != pins.regorus_cargo_lock.sha256
    {
        return Err("notices-index: regorus Cargo.lock differs from the lock pin".into());
    }
    if index.windows_population_marker != NOTICES_WINDOWS_POPULATION_MARKER
        || index.rust_standard_library != NOTICES_RUST_STANDARD_LIBRARY
        || !index.mozilla_ca_notice
        || index.notices_live_in != NOTICES_LIVE_IN
    {
        return Err("notices-index: notice markers mismatch".into());
    }
    compare_source_list(
        "notices-sources",
        "notices index",
        &index.native_sources,
        pins,
    )
}

/// The list must equal the committed one exactly, entry for entry, in order.
/// The `regorus-build-revision` entry has its own boundary so a change to it
/// is named as such.
fn compare_source_list(
    boundary: &str,
    origin: &str,
    actual: &[NvattestReportSourceEntry],
    pins: &Pins,
) -> Result<(), String> {
    let expected: Vec<NvattestReportSourceEntry> = pins
        .native_sources
        .iter()
        .map(NativeSourcePin::report_entry)
        .collect();
    for entry in expected
        .iter()
        .filter(|e| e.name == REGORUS_BUILD_REVISION_NAME)
    {
        let found: Vec<_> = actual.iter().filter(|a| a.name == entry.name).collect();
        match found.as_slice() {
            [] => {
                return Err(format!(
                    "regorus-build-revision: {origin} has no regorus-build-revision entry"
                ));
            }
            [one] if *one == entry => {}
            _ => {
                return Err(format!(
                    "regorus-build-revision: {origin} entry differs from its committed values"
                ));
            }
        }
    }
    if actual != expected.as_slice() {
        return Err(format!(
            "{boundary}: {origin} native source list differs from the committed list"
        ));
    }
    Ok(())
}

fn validate_receipt(pins: &Pins, receipt: &ControlledBuildReceipt) -> Result<(), String> {
    let dependency = &receipt.source.windows_dependency;
    if dependency.repository != pins.sdk_repo
        || dependency.revision != pins.sdk_revision
        || dependency.content_sha256 != pins.source_archive.sha256
    {
        return Err("source-revision: receipt windows_dependency differs from pin".into());
    }
    let expected_inputs = [
        (NVATTEST_SOURCE_INPUT_LABEL, &pins.source_archive),
        (NVATTEST_BUNDLE_INPUT_LABEL, &pins.bundle_archive),
    ];
    if receipt.inputs.len() != expected_inputs.len()
        || receipt
            .inputs
            .iter()
            .zip(expected_inputs)
            .any(|(input, (label, pin))| {
                input.label != label || input.sha256 != pin.sha256 || input.size != pin.bytes
            })
    {
        return Err(
            "receipt-inputs: receipt inputs are not exactly the pinned source and bundle archives"
                .into(),
        );
    }
    let configuration = &receipt.configuration;
    if configuration.target_triple != NVATTEST_TARGET_TRIPLE
        || configuration.profile != NVATTEST_BUILD_PROFILE
        || !configuration.flags.is_empty()
        || !configuration.network_access_denied
    {
        return Err(
            "receipt-configuration: receipt configuration differs from the recorder's".into(),
        );
    }
    if receipt.validation.description != NVATTEST_VALIDATION_DESCRIPTION {
        return Err(
            "receipt-validation: validation description differs from the recorder's".into(),
        );
    }
    if receipt.outputs.len() != 1 || receipt.outputs[0].label != NVATTEST_EXE_OUTPUT_LABEL {
        return Err("receipt-output: native receipt has an unexpected output set".into());
    }
    Ok(())
}

fn validate_invocation(pins: &Pins, invocation: &NvattestInvocation) -> Result<(), String> {
    if !invocation.offline {
        return Err("invocation: evidence invocation must declare offline=true".into());
    }
    if invocation.manifest_sha256 != pins.manifest_sha256 {
        return Err("invocation: caller-bound manifest digest differs from pin".into());
    }
    if invocation.source_commit != pins.sdk_revision {
        return Err("invocation: NVAT_SOURCE_COMMIT differs from the revision pin".into());
    }
    if invocation.bundle_path.is_empty() || invocation.argv.is_empty() {
        return Err("invocation: bundle path and argv must be recorded".into());
    }
    let mut actual: Vec<&str> = invocation.environment.iter().map(String::as_str).collect();
    actual.sort_unstable();
    let mut expected = NVATTEST_SDK_CHILD_ENVIRONMENT.to_vec();
    expected.sort_unstable();
    if actual != expected {
        return Err(
            "invocation: SDK child environment is not exactly the committed allowlist".into(),
        );
    }
    Ok(())
}

fn validate_refusals(refusals: &NvattestRefusals) -> Result<(), String> {
    for (name, entry, boundary) in [
        (
            "manifest_digest",
            &refusals.manifest_digest,
            NVATTEST_REFUSAL_MANIFEST_BOUNDARY,
        ),
        (
            "reuse_dependencies",
            &refusals.reuse_dependencies,
            NVATTEST_REFUSAL_REUSE_BOUNDARY,
        ),
        (
            "corrupt_member",
            &refusals.corrupt_member,
            NVATTEST_REFUSAL_CORRUPT_BOUNDARY,
        ),
    ] {
        if entry.exit_code == 0 || entry.boundary != boundary {
            return Err(format!(
                "refusals: {name} did not refuse non-zero at its named boundary"
            ));
        }
    }
    Ok(())
}

fn validate_network(network: &NvattestNetworkEvidence) -> Result<(), String> {
    for (family, probe) in [("ipv4", &network.ipv4), ("ipv6", &network.ipv6)] {
        if probe.target.is_empty()
            || probe.positive_control != NVATTEST_NETWORK_CONNECTED
            || probe.negative_control != NVATTEST_NETWORK_REFUSED
        {
            return Err(format!(
                "network-rules: {family} positive control must connect and negative control must refuse"
            ));
        }
    }
    if network.transport_peer.is_empty() || network.rules_added == 0 {
        return Err("network-rules: transport peer and installed rules must be recorded".into());
    }
    if network.rules_remaining != 0 {
        return Err("network-rules: firewall rules remained after the build".into());
    }
    Ok(())
}

/// The DLL names a `dumpbin /dependents` text lists (as the SDK's own report
/// parses them), folded to lower case.
fn dumpbin_dependents(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            (line.starts_with(char::is_whitespace)
                && !trimmed.is_empty()
                && !trimmed.contains(char::is_whitespace)
                && trimmed.to_ascii_lowercase().ends_with(".dll"))
            .then(|| trimmed.to_ascii_lowercase())
        })
        .collect()
}

fn validate_dumpbin(
    dumpbin: &NvattestDumpbinDependents,
    pe: &PeDependencies,
) -> Result<(), String> {
    let mut decoded = Vec::new();
    for (name, text) in [
        ("nvattest.exe", &dumpbin.nvattest_exe),
        ("msvcp140.dll", &dumpbin.msvcp140),
        ("vcruntime140.dll", &dumpbin.vcruntime140),
        ("vcruntime140_1.dll", &dumpbin.vcruntime140_1),
    ] {
        let bytes = base64_decode(text)?;
        if bytes.is_empty() {
            return Err(format!("dumpbin: {name} dependents were not captured"));
        }
        decoded.push(bytes);
    }
    let listed = dumpbin_dependents(&String::from_utf8_lossy(&decoded[0]));
    if listed != pe_load_dependencies(pe) {
        return Err(
            "dumpbin: captured nvattest.exe dependents differ from the admitted PE imports".into(),
        );
    }
    Ok(())
}

fn pe_load_dependencies(pe: &PeDependencies) -> BTreeSet<String> {
    pe.imports
        .iter()
        .chain(&pe.delay_imports)
        .map(|name| name.to_ascii_lowercase())
        .collect()
}

fn validate_report_imports(
    report: &NvattestBuildReport,
    pe: &PeDependencies,
) -> Result<(), String> {
    let listed: BTreeSet<String> = report
        .imports
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    if let Some(name) = listed
        .iter()
        .find(|name| !IMPORT_ALLOWLIST.contains(&name.as_str()))
    {
        return Err(format!(
            "import-allowlist: report lists an unrecognized dependency: {name}"
        ));
    }
    if listed != pe_load_dependencies(pe) {
        return Err("report-imports: report imports differ from the admitted PE imports".into());
    }
    Ok(())
}

pub(crate) fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let digits: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if !digits.len().is_multiple_of(4) {
        return Err("base64: invalid length".into());
    }
    let mut out = Vec::with_capacity(digits.len() / 4 * 3);
    let quartets = digits.len() / 4;
    for (index, chunk) in digits.chunks(4).enumerate() {
        let mut values = [0_u8; 4];
        let mut padding = 0;
        for (slot, byte) in chunk.iter().enumerate() {
            values[slot] = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if slot >= 2 && index + 1 == quartets => {
                    padding += 1;
                    0
                }
                _ => return Err(format!("base64: invalid byte {byte}")),
            };
            if padding > 0 && *byte != b'=' {
                return Err("base64: data after padding".into());
            }
        }
        out.push((values[0] << 2) | (values[1] >> 4));
        if padding < 2 {
            out.push((values[1] << 4) | (values[2] >> 2));
        }
        if padding < 1 {
            out.push((values[2] << 6) | values[3]);
        }
    }
    Ok(out)
}

pub(crate) fn base64_encode(input: &[u8]) -> String {
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

pub(crate) fn decode_build_report(bytes: &[u8]) -> Result<NvattestBuildReport, String> {
    // Windows PowerShell 5.1 writes a UTF-8 BOM; the embedded bytes keep it.
    let json = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    serde_json::from_slice(json)
        .map_err(|e| format!("report: failed to parse build-report.json: {e}"))
}

fn validate_census(
    pins: &Pins,
    census: &NvattestToolCensus,
    report: &NvattestBuildReport,
) -> Result<(), String> {
    if census.schema != NVATTEST_TOOL_CENSUS_SCHEMA_V1 {
        return Err("census-version: tool census schema mismatch".into());
    }
    let toolchain = &pins.toolchain;

    for (name, tool, expected_sha256, reported) in [
        (
            "rustc",
            &census.rustc,
            toolchain.rustc_exe_sha256,
            &report.tools.rustc,
        ),
        (
            "cargo",
            &census.cargo,
            toolchain.cargo_exe_sha256,
            &report.tools.cargo,
        ),
        (
            "cmake",
            &census.cmake,
            toolchain.cmake_exe_sha256,
            &report.tools.cmake,
        ),
    ] {
        let version = census_tool(name, tool)?;
        match name {
            "rustc" => require_version_tokens(
                name,
                version,
                &["rustc", toolchain.rustc_version],
                Some(toolchain.rustc_commit),
            )?,
            "cargo" => {
                require_version_tokens(name, version, &["cargo", toolchain.cargo_version], None)?
            }
            _ => require_version_tokens(
                name,
                version,
                &["cmake", "version", toolchain.cmake_version],
                None,
            )?,
        }
        if tool.sha256 != expected_sha256 {
            return Err(format!(
                "census-digest: {name} executable digest differs from pin"
            ));
        }
        if version != reported {
            return Err(format!(
                "tool-census: census {name} version differs from report"
            ));
        }
    }
    // Recorded identities of the compiler and build drivers the SDK ran.
    for (name, tool) in [
        ("cl", &census.cl),
        ("link", &census.link),
        ("nmake", &census.nmake),
        ("msbuild", &census.msbuild),
    ] {
        census_tool(name, tool)?;
    }

    let msvc =
        census.msvc.vc_tools_version.as_deref().ok_or_else(|| {
            "census-version: msvc vc_tools_version is missing or null".to_string()
        })?;
    if msvc != toolchain.msvc_toolset {
        return Err(format!(
            "census-version: msvc vc_tools_version does not equal {}",
            toolchain.msvc_toolset
        ));
    }
    if msvc != report.tools.msvc {
        return Err("tool-census: census msvc version differs from report".into());
    }

    let sdk = census
        .windows_sdk
        .version
        .as_deref()
        .ok_or_else(|| "census-version: windows_sdk version is missing or null".to_string())?;
    // vcvars writes exactly one trailing backslash; strip it, then match exactly.
    let trimmed = sdk
        .strip_suffix('\\')
        .filter(|rest| !rest.ends_with('\\'))
        .ok_or_else(|| {
            "census-version: windows_sdk version must end with exactly one trailing backslash"
                .to_string()
        })?;
    if trimmed != toolchain.windows_sdk {
        return Err(format!(
            "census-version: windows_sdk version does not equal {}",
            toolchain.windows_sdk
        ));
    }
    if sdk != report.tools.windows_sdk {
        return Err("tool-census: census windows_sdk version differs from report".into());
    }
    Ok(())
}

fn census_tool<'a>(name: &str, tool: &'a NvattestToolVersionPath) -> Result<&'a str, String> {
    let version = tool
        .version
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("census-version: {name} version is missing or null"))?;
    if tool.path.is_empty() {
        return Err(format!("census-version: {name} path is missing"));
    }
    if tool.sha256.len() != 64
        || !tool
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(format!(
            "census-version: {name} sha256 must be 64 lowercase hex digits"
        ));
    }
    Ok(version)
}

/// Exact token comparison: `rustc 1.97.10` is not `rustc 1.97.1`.
fn require_version_tokens(
    name: &str,
    version: &str,
    expected: &[&str],
    commit: Option<&str>,
) -> Result<(), String> {
    let tokens: Vec<&str> = version.split_whitespace().collect();
    let prefix_ok = tokens.len() >= expected.len() && tokens[..expected.len()] == *expected;
    let commit_ok = commit.is_none_or(|commit| {
        tokens
            .get(expected.len())
            .and_then(|token| token.strip_prefix('('))
            == Some(commit)
    });
    if !prefix_ok || !commit_ok {
        return Err(format!(
            "census-version: {name} version token mismatch: {version}"
        ));
    }
    Ok(())
}

fn validate_report_sources(pins: &Pins, report: &NvattestBuildReport) -> Result<(), String> {
    compare_source_list("report-sources", "report", &report.sources, pins)?;
    let expected: Vec<NvattestReportBuildToolEntry> = pins
        .build_tools
        .iter()
        .map(|tool| NvattestReportBuildToolEntry {
            name: tool.name.into(),
            url: tool.url.into(),
            sha256: tool.sha256.into(),
        })
        .collect();
    if report.build_tools != expected {
        return Err("report-tools: report build tools differ from the committed list".into());
    }
    Ok(())
}

fn validate_report_outputs(
    pins: &Pins,
    report: &NvattestBuildReport,
    exe: &crate::controlled_build::OutputIdentityEntry,
) -> Result<(), String> {
    let runtime = &pins.msvc_runtime;
    let expected: [(&str, u64, &str); 6] = [
        ("LICENSE", pins.license.bytes, pins.license.sha256),
        (
            NVATTEST_RUNTIME_OUTPUT_LABELS[0],
            runtime.msvcp140.bytes,
            runtime.msvcp140.sha256,
        ),
        (NVATTEST_EXE_OUTPUT_LABEL, exe.size, &exe.pre_signing_sha256),
        (
            NVATTEST_RUNTIME_OUTPUT_LABELS[1],
            runtime.vcruntime140.bytes,
            runtime.vcruntime140.sha256,
        ),
        (
            NVATTEST_RUNTIME_OUTPUT_LABELS[2],
            runtime.vcruntime140_1.bytes,
            runtime.vcruntime140_1.sha256,
        ),
        (
            NVATTEST_CA_BUNDLE_LABEL,
            pins.ca_bundle.bytes,
            pins.ca_bundle.sha256,
        ),
    ];
    let paths: BTreeSet<&str> = report.outputs.iter().map(|o| o.path.as_str()).collect();
    if paths.len() != report.outputs.len()
        || paths != expected.iter().map(|(path, _, _)| *path).collect()
    {
        return Err("report-output: report output set differs from the staged layout".into());
    }
    for (path, bytes, sha256) in expected {
        let output = report.outputs.iter().find(|o| o.path == path).unwrap();
        if output.bytes != bytes || output.sha256 != sha256 {
            return Err(format!(
                "report-output: report {path} differs from its receipt or pin identity"
            ));
        }
    }
    Ok(())
}

/// A tar member name with any leading `./` removed. Absolute names, `..`,
/// drive or backslash spellings and empty interior components refuse.
fn normalized_member(raw: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(raw).map_err(|_| "archive member name is not UTF-8")?;
    if text.starts_with('/') || text.contains(['\\', ':', '\0']) {
        return Err(format!("unsafe archive member name: {text:?}"));
    }
    let trimmed = text.strip_suffix('/').unwrap_or(text);
    let mut normalized = Vec::new();
    for part in trimmed.split('/').skip_while(|part| *part == ".") {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("unsafe archive member name: {text:?}"));
        }
        normalized.push(part);
    }
    Ok(normalized.join("/"))
}

struct SourceMembers {
    pax_comment: Option<String>,
    license: Vec<u8>,
    regorus_lock: Vec<u8>,
}

struct BundleMembers {
    manifest: Vec<u8>,
    ca: Vec<u8>,
}

/// Walk every member once; return the regular-file bytes for `wanted`, refusing
/// unsafe or duplicate names and a wanted name carried by a non-file entry.
fn read_tar_members(
    label: &str,
    archive_bytes: &[u8],
    wanted: &[&str],
    mut on_global: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<Vec<Option<Vec<u8>>>, String> {
    let mut found: Vec<Option<Vec<u8>>> = vec![None; wanted.len()];
    let mut seen = BTreeSet::new();
    let mut archive = tar::Archive::new(Cursor::new(archive_bytes));
    for entry in archive.entries().map_err(|e| format!("{label}: {e}"))? {
        let mut entry = entry.map_err(|e| format!("{label}: {e}"))?;
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            let mut body = Vec::new();
            entry
                .read_to_end(&mut body)
                .map_err(|e| format!("{label}: {e}"))?;
            on_global(&body)?;
            continue;
        }
        let name = normalized_member(&entry.path_bytes()).map_err(|e| format!("{label}: {e}"))?;
        if name.is_empty() {
            if kind.is_dir() {
                continue;
            }
            return Err(format!("{label}: archive member has an empty name"));
        }
        if !seen.insert(name.clone()) {
            return Err(format!("{label}: duplicate archive member {name}"));
        }
        if let Some(slot) = wanted.iter().position(|w| *w == name) {
            if !kind.is_file() {
                return Err(format!("{label}: {name} is not a regular file"));
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("{label}: {e}"))?;
            found[slot] = Some(bytes);
        }
    }
    Ok(found)
}

fn extract_source_tar_members(archive_bytes: &[u8], pins: &Pins) -> Result<SourceMembers, String> {
    let mut pax_comment = None;
    let mut found = read_tar_members(
        "source-archive",
        archive_bytes,
        &[pins.license.member, pins.regorus_cargo_lock.member],
        |body| {
            if let Some(comment) = parse_pax_record(body, "comment")? {
                if pax_comment.is_some() {
                    return Err(
                        "source-revision: source archive has two pax commit comments".into(),
                    );
                }
                pax_comment = Some(comment);
            }
            Ok(())
        },
    )?;
    let regorus_lock = found[1]
        .take()
        .ok_or("source-archive: missing regorus Cargo.lock in source archive")?;
    let license = found[0]
        .take()
        .ok_or("source-archive: missing LICENSE in source archive")?;
    Ok(SourceMembers {
        pax_comment,
        license,
        regorus_lock,
    })
}

/// One pax record value by key: records are `<len> <key>=<value>\n`.
fn parse_pax_record(body: &[u8], key: &str) -> Result<Option<String>, String> {
    let mut rest = body;
    let mut value = None;
    while !rest.is_empty() {
        let space = rest
            .iter()
            .position(|b| *b == b' ')
            .ok_or("source-revision: malformed pax record")?;
        let length: usize = std::str::from_utf8(&rest[..space])
            .ok()
            .and_then(|digits| digits.parse().ok())
            .filter(|length| *length > space + 1 && *length <= rest.len())
            .ok_or("source-revision: malformed pax record length")?;
        let record = &rest[space + 1..length];
        let record = record
            .strip_suffix(b"\n")
            .ok_or("source-revision: pax record lacks its newline")?;
        let text =
            std::str::from_utf8(record).map_err(|_| "source-revision: pax record is not UTF-8")?;
        let (name, content) = text
            .split_once('=')
            .ok_or("source-revision: pax record lacks a key")?;
        if name == key {
            value = Some(content.to_string());
        }
        rest = &rest[length..];
    }
    Ok(value)
}

fn extract_bundle_tar_members(archive_bytes: &[u8], pins: &Pins) -> Result<BundleMembers, String> {
    let mut found = read_tar_members(
        "bundle-archive",
        archive_bytes,
        &[BUNDLE_MANIFEST_MEMBER, pins.ca_bundle.member],
        |_| Ok(()),
    )?;
    let ca = found[1]
        .take()
        .ok_or("bundle-archive: missing ca-bundle.pem in bundle")?;
    let manifest = found[0]
        .take()
        .ok_or("bundle-archive: missing offline-manifest.json in bundle")?;
    Ok(BundleMembers { manifest, ca })
}

/// Every pinned download and build tool, and the CA, is a manifest member at
/// its pinned digest. The `regorus-build-revision` entry is not a member.
fn validate_bundle_manifest(pins: &Pins, manifest: &BundleManifest) -> Result<(), String> {
    let lookup = |name: &str| -> Result<&BundleManifestFile, String> {
        let matches: Vec<_> = manifest.files.iter().filter(|f| f.path == name).collect();
        match matches.as_slice() {
            [one] => Ok(one),
            [] => Err(format!(
                "bundle-manifest: pinned input {name} is not in the bundle manifest"
            )),
            _ => Err(format!("bundle-manifest: duplicate manifest entry {name}")),
        }
    };
    let downloads = pins
        .native_sources
        .iter()
        .filter_map(|source| match source {
            NativeSourcePin::Download(pin) => Some(pin),
            NativeSourcePin::BuildRevision(_) => None,
        });
    for pin in downloads.chain(pins.build_tools) {
        if lookup(pin.name)?.sha256 != pin.sha256 {
            return Err(format!(
                "bundle-manifest: manifest SHA-256 for {} differs from pin",
                pin.name
            ));
        }
    }
    let ca = lookup(pins.ca_bundle.member)?;
    if ca.size != pins.ca_bundle.bytes || ca.sha256 != pins.ca_bundle.sha256 {
        return Err("bundle-manifest: manifest ca-bundle.pem differs from pin".into());
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
    for (pin, msvc_bytes, out_bytes) in [
        (&pins.msvc_runtime.msvcp140, msvc.msvcp140, out_msvcp),
        (
            &pins.msvc_runtime.vcruntime140,
            msvc.vcruntime140,
            out_vcruntime,
        ),
        (
            &pins.msvc_runtime.vcruntime140_1,
            msvc.vcruntime140_1,
            out_vcruntime_1,
        ),
    ] {
        if msvc_bytes.len() as u64 != pin.bytes || sha256_hex(msvc_bytes) != pin.sha256 {
            return Err(format!(
                "runtime-dll: msvc input {} differs from pin",
                pin.member
            ));
        }
        if out_bytes != msvc_bytes {
            return Err(format!(
                "runtime-dll: output {} differs from msvc package member",
                pin.member
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
