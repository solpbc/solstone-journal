// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Synthesized admission inputs for routine tests.
//!
//! The archives, executable, runtime DLLs, receipt, evidence wrapper,
//! validation log and tool census below are synthesized: no run of the
//! controlled driver exists yet, and the real 451 MB bundle cannot be a
//! fixture. The notices index and body, the regorus `Cargo.lock` member and
//! the native source and build-tool lists are the committed or captured
//! bytes. The fixture pins differ from production only where a synthesized
//! byte stands in for a real one.

use std::path::{Path, PathBuf};

use super::*;
use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, CONTROLLED_BUILD_RECEIPT_SCHEMA_V1, DependencySource,
    InputIdentityEntry, OutputIdentityEntry, SourceIdentity, SupportingArtifactRef,
    ValidationReference, encode_controlled_build_receipt,
};
use crate::pe;
use crate::provenance::Provenance;

pub(crate) const FIXTURE_LICENSE: &[u8] = b"Synthesized Apache-2.0 LICENSE stand-in\n";
pub(crate) const FIXTURE_CA: &[u8] =
    b"-----BEGIN CERTIFICATE-----\nU1lOVEhFU0laRUQ=\n-----END CERTIFICATE-----\n";
pub(crate) const REGORUS_LOCK: &[u8] =
    include_bytes!("../../fixtures/nvattest-windows-build/regorus-Cargo.lock");
pub(crate) const NOTICES_INDEX: &[u8] =
    include_bytes!("../../../../distribution/nvattest-windows-sources.json");
pub(crate) const NOTICES_BODY: &[u8] =
    include_bytes!("../../../../distribution/nvattest-windows-NOTICES.md");
pub(crate) const FIXTURE_IMPORTS: &[&str] = &["kernel32.dll", "CRYPT32.dll"];

pub(crate) fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// A PE32+ AMD64 image with the given import descriptors.
pub(crate) fn make_test_pe(dll: bool, imports: &[&str]) -> Vec<u8> {
    let specs: Vec<pe::ImportSpec<'_>> = imports
        .iter()
        .map(|name| pe::ImportSpec {
            name,
            symbols: &[pe::PeSymbolSpec::Named("TestSymbol")],
        })
        .collect();
    let mut bytes = pe::fixture(&pe::FixtureSpec {
        dll,
        imports: &specs,
        ..pe::FixtureSpec::default()
    });
    let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    let opt_offset = pe_offset + 24;
    let headers_size = (opt_offset + 240 + 40) as u32;
    bytes[opt_offset + 60..opt_offset + 64].copy_from_slice(&headers_size.to_le_bytes());
    bytes
}

fn pax_global(records: &[(&str, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (key, value) in records {
        let content = format!(" {key}={value}\n");
        let mut length = content.len();
        while length != content.len() + length.to_string().len() {
            length = content.len() + length.to_string().len();
        }
        body.extend(format!("{length}{content}").into_bytes());
    }
    body
}

fn raw_header(name: &[u8], kind: tar::EntryType, size: u64) -> tar::Header {
    let mut header = tar::Header::new_ustar();
    header.as_ustar_mut().unwrap().name[..name.len()].copy_from_slice(name);
    header.set_entry_type(kind);
    header.set_size(size);
    header.set_mode(0o644);
    header.set_cksum();
    header
}

/// A tar of regular files whose names are written verbatim, so `./`,
/// absolute, `..` and duplicate names can be represented.
pub(crate) fn make_tar(global: Option<&[(&str, &str)]>, members: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    if let Some(records) = global {
        let body = pax_global(records);
        let header = raw_header(
            b"pax_global_header",
            tar::EntryType::XGlobalHeader,
            body.len() as u64,
        );
        builder.append(&header, body.as_slice()).unwrap();
    }
    for (name, bytes) in members {
        let kind = if name.ends_with(b"/") {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        };
        let header = raw_header(name, kind, bytes.len() as u64);
        builder.append(&header, *bytes).unwrap();
    }
    builder.into_inner().unwrap()
}

pub(crate) fn source_tar(comment: &str, license: &[u8], lock: &[u8]) -> Vec<u8> {
    make_tar(
        Some(&[("comment", comment)]),
        &[
            (b"LICENSE", license),
            (b"sol/", b""),
            (b"sol/release/", b""),
            (b"sol/release/regorus-Cargo.lock", lock),
        ],
    )
}

/// The offline manifest for the pinned downloads, build tools and the CA.
pub(crate) fn manifest_json(pins: &Pins) -> Vec<u8> {
    let downloads = pins.native_sources.iter().filter_map(|s| match s {
        NativeSourcePin::Download(pin) => Some(pin),
        NativeSourcePin::BuildRevision(_) => None,
    });
    let mut files: Vec<serde_json::Value> = downloads
        .chain(pins.build_tools)
        .map(|pin| serde_json::json!({"path": pin.name, "sha256": pin.sha256, "size": 1000}))
        .collect();
    files.push(serde_json::json!({
        "path": pins.ca_bundle.member,
        "sha256": pins.ca_bundle.sha256,
        "size": pins.ca_bundle.bytes,
    }));
    serde_json::to_vec(&serde_json::json!({"schema": 1, "files": files})).unwrap()
}

/// Real bundle members are `./`-prefixed, beneath a `./` directory entry.
pub(crate) fn bundle_tar(manifest: &[u8], ca: &[u8]) -> Vec<u8> {
    make_tar(
        None,
        &[
            (b"./", b""),
            (b"./offline-manifest.json", manifest),
            (b"./ca-bundle.pem", ca),
        ],
    )
}

pub(crate) fn dumpbin_text(imports: &[&str]) -> String {
    let mut text = String::from(
        "Microsoft (R) COFF/PE Dumper\r\n\r\nDump of file nvattest.exe\r\n\r\nFile Type: EXECUTABLE IMAGE\r\n\r\n  Image has the following dependencies:\r\n\r\n",
    );
    for import in imports {
        text.push_str(&format!("    {import}\r\n"));
    }
    text.push_str("\r\n  Summary\r\n\r\n        1000 .data\r\n");
    text
}

pub(crate) fn census(pins: &Pins) -> NvattestToolCensus {
    let tool = |version: &str, path: &str, sha256: &str| NvattestToolVersionPath {
        version: Some(version.into()),
        path: path.into(),
        sha256: sha256.into(),
    };
    let toolchain = &pins.toolchain;
    NvattestToolCensus {
        schema: NVATTEST_TOOL_CENSUS_SCHEMA_V1.into(),
        rustc: tool(
            "rustc 1.97.1 (8bab26f4f 2026-07-14)",
            r"C:\toolchain\bin\rustc.exe",
            toolchain.rustc_exe_sha256,
        ),
        cargo: tool(
            "cargo 1.97.1 (c980f4866 2026-06-30)",
            r"C:\toolchain\bin\cargo.exe",
            toolchain.cargo_exe_sha256,
        ),
        cmake: tool(
            "cmake version 3.31.12",
            r"C:\work\cmake\bin\cmake.exe",
            toolchain.cmake_exe_sha256,
        ),
        cl: tool("19.44.35229.0", r"C:\vs\cl.exe", &"1".repeat(64)),
        link: tool("14.44.35229.0", r"C:\vs\link.exe", &"2".repeat(64)),
        nmake: tool("14.44.35229.0", r"C:\vs\nmake.exe", &"3".repeat(64)),
        msbuild: tool("17.14.60.43110", r"C:\vs\MSBuild.exe", &"4".repeat(64)),
        msvc: NvattestMsvcToolsVersion {
            vc_tools_version: Some("14.44.35207".into()),
        },
        windows_sdk: NvattestWindowsSdkVersion {
            version: Some("10.0.26100.0\\".into()),
        },
        vs: Some(NvattestVsInfo {
            product_version: Some("17.0".into()),
        }),
    }
}

pub(crate) fn controls(pins: &Pins) -> NvattestDriverControls {
    NvattestDriverControls {
        schema: NVATTEST_DRIVER_CONTROLS_SCHEMA_V1.into(),
        invocation: NvattestInvocation {
            offline: true,
            bundle_path: r"C:\work\offline-inputs".into(),
            manifest_sha256: pins.manifest_sha256.into(),
            source_commit: pins.sdk_revision.into(),
            environment: NVATTEST_SDK_CHILD_ENVIRONMENT
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            argv: vec![
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
                "-File".into(),
                r"C:\work\source\sol\windows\build.ps1".into(),
            ],
        },
        refusals: NvattestRefusals {
            manifest_digest: NvattestRefusalEntry {
                exit_code: 1,
                boundary: NVATTEST_REFUSAL_MANIFEST_BOUNDARY.into(),
            },
            reuse_dependencies: NvattestRefusalEntry {
                exit_code: 1,
                boundary: NVATTEST_REFUSAL_REUSE_BOUNDARY.into(),
            },
            corrupt_member: NvattestRefusalEntry {
                exit_code: 1,
                boundary: NVATTEST_REFUSAL_CORRUPT_BOUNDARY.into(),
            },
        },
        network: NvattestNetworkEvidence {
            transport_peer: "192.0.2.10".into(),
            ipv4: NvattestNetworkProbe {
                target: "1.1.1.1:443".into(),
                positive_control: NVATTEST_NETWORK_CONNECTED.into(),
                negative_control: NVATTEST_NETWORK_REFUSED.into(),
            },
            ipv6: NvattestNetworkProbe {
                target: "[2606:4700:4700::1111]:443".into(),
                positive_control: NVATTEST_NETWORK_CONNECTED.into(),
                negative_control: NVATTEST_NETWORK_REFUSED.into(),
            },
            rules_added: 3,
            rules_remaining: 0,
        },
    }
}

/// The receipt the recorder writes, around the given output and evidence.
pub(crate) fn recorder_receipt(
    pins: &Pins,
    exe: &[u8],
    evidence: &[u8],
    validation: &[u8],
) -> ControlledBuildReceipt {
    ControlledBuildReceipt {
        schema: CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into(),
        source: SourceIdentity {
            // An older product identity is history, never a staleness test.
            product: Provenance {
                commit: "0".repeat(40),
                lock_sha256: "0".repeat(64),
            },
            windows_dependency: DependencySource {
                repository: pins.sdk_repo.into(),
                revision: pins.sdk_revision.into(),
                content_sha256: pins.source_archive.sha256.into(),
            },
        },
        inputs: vec![
            InputIdentityEntry {
                label: NVATTEST_SOURCE_INPUT_LABEL.into(),
                sha256: pins.source_archive.sha256.into(),
                size: pins.source_archive.bytes,
            },
            InputIdentityEntry {
                label: NVATTEST_BUNDLE_INPUT_LABEL.into(),
                sha256: pins.bundle_archive.sha256.into(),
                size: pins.bundle_archive.bytes,
            },
        ],
        builder: BuilderIdentity {
            host: "test-builder".into(),
            toolchain: "MSVC 14.44.35207".into(),
        },
        configuration: BuildConfiguration {
            target_triple: NVATTEST_TARGET_TRIPLE.into(),
            profile: NVATTEST_BUILD_PROFILE.into(),
            flags: vec![],
            network_access_denied: true,
        },
        outputs: vec![OutputIdentityEntry {
            pre_signing_sha256: sha256_hex(exe),
            label: NVATTEST_EXE_OUTPUT_LABEL.into(),
            size: exe.len() as u64,
            census: pe::parse_pe(exe).unwrap(),
        }],
        supporting: vec![SupportingArtifactRef {
            label: NVATTEST_BUILD_EVIDENCE_LABEL.into(),
            sha256: sha256_hex(evidence),
        }],
        validation: ValidationReference {
            description: NVATTEST_VALIDATION_DESCRIPTION.into(),
            sha256: sha256_hex(validation),
        },
    }
}

pub(crate) struct Fixture {
    pub(crate) pins: Pins,
    pub(crate) notices_index: Vec<u8>,
    pub(crate) notices_body: Vec<u8>,
    pub(crate) report: NvattestBuildReport,
    /// When set, embedded verbatim instead of serializing `report`.
    pub(crate) report_raw: Option<Vec<u8>>,
    pub(crate) evidence: NvattestWindowsBuildEvidence,
    pub(crate) receipt: ControlledBuildReceipt,
    /// When false, the receipt keeps whatever evidence digest it carries.
    pub(crate) bind_evidence: bool,
    pub(crate) validation: Vec<u8>,
    pub(crate) source_archive: Vec<u8>,
    pub(crate) bundle_archive: Vec<u8>,
    pub(crate) output_exe: Vec<u8>,
    pub(crate) output_license: Vec<u8>,
    pub(crate) output_runtime: [Vec<u8>; 3],
    pub(crate) msvc_runtime: [Vec<u8>; 3],
}

pub(crate) struct WrittenInputs {
    pub(crate) root: tempfile::TempDir,
    pub(crate) receipt: PathBuf,
    pub(crate) evidence: PathBuf,
    pub(crate) validation: PathBuf,
    pub(crate) source_archive: PathBuf,
    pub(crate) bundle_archive: PathBuf,
    pub(crate) output_root: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let mut pins = production_pins();
        pins.license.bytes = FIXTURE_LICENSE.len() as u64;
        pins.license.sha256 = leak(sha256_hex(FIXTURE_LICENSE));
        pins.ca_bundle.bytes = FIXTURE_CA.len() as u64;
        pins.ca_bundle.sha256 = leak(sha256_hex(FIXTURE_CA));
        let runtime = [
            vec![1u8; pins.msvc_runtime.msvcp140.bytes as usize],
            vec![2u8; pins.msvc_runtime.vcruntime140.bytes as usize],
            vec![3u8; pins.msvc_runtime.vcruntime140_1.bytes as usize],
        ];
        pins.msvc_runtime.msvcp140.sha256 = leak(sha256_hex(&runtime[0]));
        pins.msvc_runtime.vcruntime140.sha256 = leak(sha256_hex(&runtime[1]));
        pins.msvc_runtime.vcruntime140_1.sha256 = leak(sha256_hex(&runtime[2]));

        let source_archive = source_tar(pins.sdk_revision, FIXTURE_LICENSE, REGORUS_LOCK);
        pins.source_archive = ArchivePin {
            bytes: source_archive.len() as u64,
            sha256: leak(sha256_hex(&source_archive)),
        };
        let manifest = manifest_json(&pins);
        pins.manifest_sha256 = leak(sha256_hex(&manifest));
        let bundle_archive = bundle_tar(&manifest, FIXTURE_CA);
        pins.bundle_archive = ArchivePin {
            bytes: bundle_archive.len() as u64,
            sha256: leak(sha256_hex(&bundle_archive)),
        };

        let output_exe = make_test_pe(false, FIXTURE_IMPORTS);
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
                .map(NativeSourcePin::report_entry)
                .collect(),
            build_tools: pins
                .build_tools
                .iter()
                .map(|tool| NvattestReportBuildToolEntry {
                    name: tool.name.into(),
                    url: tool.url.into(),
                    sha256: tool.sha256.into(),
                })
                .collect(),
            ca_bundle: NvattestReportCaBundle {
                url: "https://curl.se/ca/cacert-2026-07-16.pem".into(),
                sha256: pins.ca_bundle.sha256.into(),
            },
            imports: vec![],
            outputs: vec![],
        };
        let controls = controls(&pins);
        let evidence = NvattestWindowsBuildEvidence {
            schema: NVATTEST_BUILD_EVIDENCE_SCHEMA_V1.into(),
            report_base64: String::new(),
            census: census(&pins),
            invocation: controls.invocation,
            refusals: controls.refusals,
            network: controls.network,
            dumpbin_dependents: NvattestDumpbinDependents {
                nvattest_exe: String::new(),
                msvcp140: base64_encode(b"    KERNEL32.dll\r\n"),
                vcruntime140: base64_encode(b"    KERNEL32.dll\r\n"),
                vcruntime140_1: base64_encode(b"    KERNEL32.dll\r\n"),
            },
        };
        let validation = b"schema=solstone.nvattest-windows-validation.v1\n".to_vec();
        let receipt = recorder_receipt(&pins, &output_exe, b"", &validation);
        let mut fixture = Self {
            pins,
            notices_index: NOTICES_INDEX.to_vec(),
            notices_body: NOTICES_BODY.to_vec(),
            report,
            report_raw: None,
            evidence,
            receipt,
            bind_evidence: true,
            validation,
            source_archive,
            bundle_archive,
            output_exe: Vec::new(),
            output_license: FIXTURE_LICENSE.to_vec(),
            output_runtime: runtime.clone(),
            msvc_runtime: runtime,
        };
        fixture.set_exe(output_exe);
        fixture
    }

    /// Present a different executable everywhere the recorder would: the
    /// receipt output, the report outputs and imports, and the dumpbin text.
    pub(crate) fn set_exe(&mut self, exe: Vec<u8>) {
        let imports: Vec<String> = match crate::pe_dependencies::inspect_dependencies(&exe) {
            Ok(pe) => pe.imports.into_iter().chain(pe.delay_imports).collect(),
            Err(_) => vec![],
        };
        self.receipt.outputs[0].pre_signing_sha256 = sha256_hex(&exe);
        self.receipt.outputs[0].size = exe.len() as u64;
        if let Ok(census) = pe::parse_pe(&exe) {
            self.receipt.outputs[0].census = census;
        }
        self.report.imports = imports.clone();
        let names: Vec<&str> = imports.iter().map(String::as_str).collect();
        self.evidence.dumpbin_dependents.nvattest_exe =
            base64_encode(dumpbin_text(&names).as_bytes());
        let runtime = &self.pins.msvc_runtime;
        let entry = |path: &str, bytes: u64, sha256: &str| NvattestReportOutputEntry {
            path: path.into(),
            bytes,
            sha256: sha256.into(),
            file_version: None,
        };
        self.report.outputs = vec![
            entry(
                "bin/msvcp140.dll",
                runtime.msvcp140.bytes,
                runtime.msvcp140.sha256,
            ),
            entry(
                NVATTEST_EXE_OUTPUT_LABEL,
                exe.len() as u64,
                &sha256_hex(&exe),
            ),
            entry(
                "bin/vcruntime140.dll",
                runtime.vcruntime140.bytes,
                runtime.vcruntime140.sha256,
            ),
            entry(
                "bin/vcruntime140_1.dll",
                runtime.vcruntime140_1.bytes,
                runtime.vcruntime140_1.sha256,
            ),
            entry("LICENSE", self.pins.license.bytes, self.pins.license.sha256),
            entry(
                NVATTEST_CA_BUNDLE_LABEL,
                self.pins.ca_bundle.bytes,
                self.pins.ca_bundle.sha256,
            ),
        ];
        self.output_exe = exe;
    }

    /// Replace the source archive and repin it, as a changed pin would.
    pub(crate) fn set_source_archive(&mut self, archive: Vec<u8>) {
        self.pins.source_archive = ArchivePin {
            bytes: archive.len() as u64,
            sha256: leak(sha256_hex(&archive)),
        };
        self.receipt.source.windows_dependency.content_sha256 =
            self.pins.source_archive.sha256.into();
        self.receipt.inputs[0].sha256 = self.pins.source_archive.sha256.into();
        self.receipt.inputs[0].size = self.pins.source_archive.bytes;
        self.source_archive = archive;
    }

    /// Replace the bundle archive and repin it, as a changed pin would.
    pub(crate) fn set_bundle_archive(&mut self, archive: Vec<u8>) {
        self.pins.bundle_archive = ArchivePin {
            bytes: archive.len() as u64,
            sha256: leak(sha256_hex(&archive)),
        };
        self.receipt.inputs[1].sha256 = self.pins.bundle_archive.sha256.into();
        self.receipt.inputs[1].size = self.pins.bundle_archive.bytes;
        self.bundle_archive = archive;
    }

    /// Rebuild the bundle around a new manifest and repin both.
    pub(crate) fn set_manifest(&mut self, manifest: Vec<u8>) {
        self.pins.manifest_sha256 = leak(sha256_hex(&manifest));
        self.evidence.invocation.manifest_sha256 = self.pins.manifest_sha256.into();
        self.set_bundle_archive(bundle_tar(&manifest, FIXTURE_CA));
    }

    pub(crate) fn report_bytes(&self) -> Vec<u8> {
        self.report_raw.clone().unwrap_or_else(|| {
            let mut raw = vec![0xEF, 0xBB, 0xBF];
            raw.extend(serde_json::to_vec_pretty(&self.report).unwrap());
            raw
        })
    }

    pub(crate) fn evidence_bytes(&self) -> Vec<u8> {
        let mut evidence = self.evidence.clone();
        evidence.report_base64 = base64_encode(&self.report_bytes());
        serde_json::to_vec_pretty(&evidence).unwrap()
    }

    pub(crate) fn receipt_bytes(&self, evidence: &[u8]) -> Vec<u8> {
        let mut receipt = self.receipt.clone();
        if self.bind_evidence {
            receipt.supporting[0].sha256 = sha256_hex(evidence);
        }
        encode_controlled_build_receipt(&receipt).unwrap()
    }

    pub(crate) fn admit(&self) -> Result<AdmittedNvattest, String> {
        let evidence = self.evidence_bytes();
        let receipt = self.receipt_bytes(&evidence);
        admit(
            &self.pins,
            &AdmissionBytes {
                receipt: &receipt,
                evidence: &evidence,
                validation: &self.validation,
                notices_index: &self.notices_index,
                notices_body: &self.notices_body,
                source_archive: &self.source_archive,
                bundle_archive: &self.bundle_archive,
                output_exe: &self.output_exe,
                output_license: &self.output_license,
                output_msvcp140: &self.output_runtime[0],
                output_vcruntime140: &self.output_runtime[1],
                output_vcruntime140_1: &self.output_runtime[2],
            },
            &self.msvc(),
        )
    }

    pub(crate) fn msvc(&self) -> MsvcRuntimeBytes<'_> {
        MsvcRuntimeBytes {
            msvcp140: &self.msvc_runtime[0],
            vcruntime140: &self.msvc_runtime[1],
            vcruntime140_1: &self.msvc_runtime[2],
        }
    }

    /// Lay the inputs out as the driver leaves them on disk.
    pub(crate) fn write_inputs(&self) -> WrittenInputs {
        let root = tempfile::tempdir().unwrap();
        let path = |name: &str| root.path().join(name);
        let evidence = self.evidence_bytes();
        let write = |target: &Path, bytes: &[u8]| {
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, bytes).unwrap();
        };
        write(&path("receipt.json"), &self.receipt_bytes(&evidence));
        write(&path("evidence.json"), &evidence);
        write(&path("validation.log"), &self.validation);
        write(&path("source.tar"), &self.source_archive);
        write(&path("bundle.tar"), &self.bundle_archive);
        let output = path("output");
        write(&output.join("bin/nvattest.exe"), &self.output_exe);
        write(&output.join("LICENSE"), &self.output_license);
        for (label, bytes) in NVATTEST_RUNTIME_OUTPUT_LABELS
            .iter()
            .zip(&self.output_runtime)
        {
            write(&output.join(label), bytes);
        }
        write(&output.join("share/ca/ca-bundle.pem"), FIXTURE_CA);
        WrittenInputs {
            receipt: path("receipt.json"),
            evidence: path("evidence.json"),
            validation: path("validation.log"),
            source_archive: path("source.tar"),
            bundle_archive: path("bundle.tar"),
            output_root: output,
            root,
        }
    }
}
