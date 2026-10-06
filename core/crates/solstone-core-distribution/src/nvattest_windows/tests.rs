// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::test_support::*;
use super::*;
use crate::pe;

const CAPTURED_REPORT: &[u8] =
    include_bytes!("../../fixtures/nvattest-windows-build/build-report.json");
const CAPTURED_DUMPBIN: &[u8] =
    include_bytes!("../../fixtures/nvattest-windows-build/nvattest.exe.dumpbin.txt");

#[track_caller]
fn refuses(fixture: &Fixture, boundary: &str) -> String {
    let error = fixture.admit().unwrap_err();
    assert!(
        error.starts_with(boundary) || error.contains(boundary),
        "expected {boundary}, got {error}"
    );
    error
}

fn source_named(name: &str) -> NvattestReportSourceEntry {
    production_pins()
        .native_sources
        .iter()
        .map(NativeSourcePin::report_entry)
        .find(|entry| entry.name == name)
        .unwrap()
}

#[test]
fn intact_fixture_admits_with_an_older_product_identity() {
    let fixture = Fixture::new();
    assert_eq!(fixture.receipt.source.product.commit, "0".repeat(40));
    let admitted = fixture.admit().unwrap();
    assert_eq!(
        admitted.outputs().keys().cloned().collect::<Vec<_>>(),
        [
            NVATTEST_LICENSE_LABEL,
            NVATTEST_EXE_OUTPUT_LABEL,
            NVATTEST_CA_BUNDLE_LABEL
        ]
    );
    assert_eq!(
        admitted.outputs()[NVATTEST_EXE_OUTPUT_LABEL],
        fixture.output_exe
    );
    assert_eq!(admitted.outputs()[NVATTEST_CA_BUNDLE_LABEL], FIXTURE_CA);
    assert_eq!(
        sha256_hex(&admitted.outputs()[NVATTEST_LICENSE_LABEL]),
        fixture.pins.license.sha256
    );
    assert!(
        admitted
            .outputs()
            .keys()
            .all(|label| !label.ends_with(".dll"))
    );
}

#[test]
fn bundle_members_with_and_without_a_dot_prefix_admit() {
    let mut fixture = Fixture::new();
    assert!(
        fixture
            .bundle_archive
            .windows(25)
            .any(|w| w == b"./offline-manifest.json\0\0")
    );
    let manifest = manifest_json(&fixture.pins);
    fixture.set_bundle_archive(make_tar(
        None,
        &[
            (b"offline-manifest.json", &manifest),
            (b"ca-bundle.pem", FIXTURE_CA),
        ],
    ));
    fixture.admit().unwrap();
    fixture.set_bundle_archive(make_tar(
        None,
        &[
            (b"././offline-manifest.json", &manifest),
            (b"./ca-bundle.pem", FIXTURE_CA),
        ],
    ));
    fixture.admit().unwrap();
}

#[test]
fn bundle_member_names_that_are_unsafe_or_duplicated_refuse() {
    let manifest = manifest_json(&Fixture::new().pins);
    for members in [
        vec![
            (b"./offline-manifest.json".as_slice(), manifest.as_slice()),
            (b"./ca-bundle.pem", FIXTURE_CA),
            (b"ca-bundle.pem", FIXTURE_CA),
        ],
        vec![
            (b"./offline-manifest.json".as_slice(), manifest.as_slice()),
            (b"/ca-bundle.pem", FIXTURE_CA),
        ],
        vec![
            (b"./offline-manifest.json".as_slice(), manifest.as_slice()),
            (b"../ca-bundle.pem", FIXTURE_CA),
        ],
        vec![
            (b"./offline-manifest.json".as_slice(), manifest.as_slice()),
            (b"./x/../ca-bundle.pem", FIXTURE_CA),
        ],
    ] {
        let mut fixture = Fixture::new();
        fixture.set_bundle_archive(make_tar(None, &members));
        refuses(&fixture, "bundle-archive");
    }
}

#[test]
fn a_dot_prefixed_source_archive_admits() {
    let mut fixture = Fixture::new();
    let revision = fixture.pins.sdk_revision;
    fixture.set_source_archive(make_tar(
        Some(&[("comment", revision)]),
        &[
            (b"./LICENSE", FIXTURE_LICENSE),
            (b"./sol/release/regorus-Cargo.lock", REGORUS_LOCK),
        ],
    ));
    fixture.admit().unwrap();
}

#[test]
fn source_pax_global_header_commit_must_equal_the_revision_pin() {
    let mut fixture = Fixture::new();
    fixture.set_source_archive(source_tar(&"1".repeat(40), FIXTURE_LICENSE, REGORUS_LOCK));
    refuses(&fixture, "source-revision: source archive pax comment");
    fixture.set_source_archive(make_tar(
        None,
        &[
            (b"LICENSE", FIXTURE_LICENSE),
            (b"sol/release/regorus-Cargo.lock", REGORUS_LOCK),
        ],
    ));
    refuses(&fixture, "source-revision: source archive pax comment");
}

#[test]
fn report_source_commit_must_equal_the_revision_pin() {
    let mut fixture = Fixture::new();
    fixture.report.source_commit = "1".repeat(40);
    refuses(&fixture, "source-revision: report source_commit");
}

#[test]
fn receipt_windows_dependency_must_bind_the_pinned_source() {
    for change in [
        |r: &mut crate::controlled_build::DependencySource| {
            r.repository = "https://example.invalid".into()
        },
        |r: &mut crate::controlled_build::DependencySource| r.revision = "1".repeat(40),
        |r: &mut crate::controlled_build::DependencySource| r.content_sha256 = "1".repeat(64),
    ] {
        let mut fixture = Fixture::new();
        change(&mut fixture.receipt.source.windows_dependency);
        refuses(&fixture, "source-revision: receipt windows_dependency");
    }
}

#[test]
fn archive_identities_must_equal_their_pins() {
    let mut fixture = Fixture::new();
    fixture.pins.source_archive.sha256 = leak("0".repeat(64));
    refuses(&fixture, "source-archive");
    let mut fixture = Fixture::new();
    fixture.source_archive.push(0);
    refuses(&fixture, "source-archive");
    let mut fixture = Fixture::new();
    fixture.pins.bundle_archive.sha256 = leak("0".repeat(64));
    refuses(&fixture, "bundle-archive");
    let mut fixture = Fixture::new();
    *fixture.bundle_archive.last_mut().unwrap() ^= 1;
    refuses(&fixture, "bundle-archive");
}

#[test]
fn receipt_inputs_must_be_exactly_the_two_pinned_archives() {
    let mutations: [fn(&mut Vec<crate::controlled_build::InputIdentityEntry>); 6] = [
        |inputs| inputs.clear(),
        |inputs| {
            inputs.pop();
        },
        |inputs| inputs.reverse(),
        |inputs| inputs[0].label = "source.tar.gz".into(),
        |inputs| inputs[1].sha256 = "1".repeat(64),
        |inputs| {
            let extra = inputs[0].clone();
            inputs.push(extra);
        },
    ];
    for mutate in mutations {
        let mut fixture = Fixture::new();
        mutate(&mut fixture.receipt.inputs);
        refuses(&fixture, "receipt-inputs");
    }
}

#[test]
fn receipt_configuration_must_equal_the_recorder_constants() {
    let mutations: [fn(&mut crate::controlled_build::BuildConfiguration); 4] = [
        |c| c.target_triple = "aarch64-pc-windows-msvc".into(),
        |c| c.profile = "Debug".into(),
        |c| c.flags.push("-DNVAT_EXTRA=ON".into()),
        |c| c.network_access_denied = false,
    ];
    for mutate in mutations {
        let mut fixture = Fixture::new();
        mutate(&mut fixture.receipt.configuration);
        refuses(&fixture, "receipt-configuration");
    }
    let mut fixture = Fixture::new();
    fixture.receipt.validation.description = "other".into();
    refuses(&fixture, "receipt-validation");
}

#[test]
fn receipt_that_does_not_bind_its_evidence_or_validation_refuses() {
    let mut fixture = Fixture::new();
    fixture.bind_evidence = false;
    refuses(&fixture, "unbound-evidence");
    let mut fixture = Fixture::new();
    fixture.validation.push(b'\n');
    refuses(&fixture, "unbound-evidence");
    let mut fixture = Fixture::new();
    fixture.validation.clear();
    fixture.receipt.validation.sha256 = sha256_hex(b"");
    refuses(&fixture, "unbound-evidence");
}

#[test]
fn report_output_digest_differing_from_the_receipt_refuses() {
    // Only the embedded report changes; the evidence digest is re-bound.
    let mut fixture = Fixture::new();
    let exe = fixture
        .report
        .outputs
        .iter_mut()
        .find(|o| o.path == NVATTEST_EXE_OUTPUT_LABEL)
        .unwrap();
    exe.sha256 = "1".repeat(64);
    refuses(&fixture, "report-output");
    let mut fixture = Fixture::new();
    let exe = fixture
        .report
        .outputs
        .iter_mut()
        .find(|o| o.path == NVATTEST_EXE_OUTPUT_LABEL)
        .unwrap();
    exe.bytes += 1;
    refuses(&fixture, "report-output");
}

#[test]
fn report_output_set_must_be_the_staged_layout() {
    let mut fixture = Fixture::new();
    fixture.report.outputs.retain(|o| o.path != "LICENSE");
    refuses(&fixture, "report-output");
    let mut fixture = Fixture::new();
    let extra = fixture.report.outputs[0].clone();
    fixture.report.outputs.push(extra);
    refuses(&fixture, "report-output");
    let mut fixture = Fixture::new();
    fixture.report.outputs[0].sha256 = "1".repeat(64);
    refuses(&fixture, "report-output");
}

#[test]
fn an_executable_that_differs_from_the_receipt_refuses() {
    let mut fixture = Fixture::new();
    fixture.output_exe.push(0);
    refuses(&fixture, "receipt-output");
}

#[test]
fn ca_bytes_must_match_both_the_pin_and_the_report() {
    let mut fixture = Fixture::new();
    fixture.pins.ca_bundle.sha256 = leak("0".repeat(64));
    let ca = fixture
        .report
        .outputs
        .iter_mut()
        .find(|o| o.path == NVATTEST_CA_BUNDLE_LABEL);
    ca.unwrap().sha256 = "0".repeat(64);
    refuses(&fixture, "ca-pin: bundle CA matches report");
    let mut fixture = Fixture::new();
    fixture.report.ca_bundle.sha256 = "0".repeat(64);
    refuses(&fixture, "ca-report");
    let mut fixture = Fixture::new();
    let manifest = manifest_json(&fixture.pins);
    fixture.set_bundle_archive(bundle_tar(&manifest, b"another CA\n"));
    refuses(&fixture, "ca-pin: bundle ca-bundle.pem differs from pin");
}

#[test]
fn license_and_regorus_lock_members_must_equal_their_pins() {
    let mut fixture = Fixture::new();
    fixture.output_license = b"tampered license".to_vec();
    refuses(&fixture, "output-license");
    let mut fixture = Fixture::new();
    let revision = fixture.pins.sdk_revision;
    fixture.set_source_archive(source_tar(revision, b"another license\n", REGORUS_LOCK));
    refuses(&fixture, "source-archive: member LICENSE");
    let mut fixture = Fixture::new();
    let mut lock = REGORUS_LOCK.to_vec();
    lock.extend_from_slice(b"\n");
    fixture.set_source_archive(source_tar(revision, FIXTURE_LICENSE, &lock));
    refuses(
        &fixture,
        "source-archive: member sol/release/regorus-Cargo.lock",
    );
    let mut fixture = Fixture::new();
    fixture.set_source_archive(make_tar(
        Some(&[("comment", revision)]),
        &[(b"LICENSE", FIXTURE_LICENSE)],
    ));
    refuses(&fixture, "source-archive: missing regorus Cargo.lock");
}

#[test]
fn a_revision_or_lock_pin_change_without_a_matching_index_change_refuses() {
    let mut fixture = Fixture::new();
    fixture.pins.sdk_revision = leak("1".repeat(40));
    refuses(&fixture, "notices-index: admitted SDK revision");
    let mut fixture = Fixture::new();
    fixture.pins.regorus_cargo_lock.sha256 = leak("1".repeat(64));
    refuses(&fixture, "notices-index: regorus Cargo.lock");
}

#[test]
fn notices_body_changes_refuse_at_each_binding() {
    let mut fixture = Fixture::new();
    fixture.notices_body[0] ^= 0xff;
    let error = refuses(&fixture, "notices-body");
    assert!(error.contains("differs from pin"), "{error}");

    let mut fixture = Fixture::new();
    fixture.pins.notices_body_sha256 = leak("0".repeat(64));
    let error = refuses(&fixture, "notices-body");
    assert!(error.contains("index notices body"), "{error}");

    // The pin and index agree with each other but not with the body.
    let mut fixture = Fixture::new();
    let mut index: NoticesIndex = serde_json::from_slice(&fixture.notices_index).unwrap();
    index.notices_body_sha256 = "0".repeat(64);
    fixture.notices_index = serde_json::to_vec(&index).unwrap();
    fixture.pins.notices_body_sha256 = leak("0".repeat(64));
    let error = refuses(&fixture, "notices-body");
    assert!(
        error.contains("notices body SHA-256 differs from pin"),
        "{error}"
    );
}

#[test]
fn notices_index_must_carry_the_committed_source_list_and_markers() {
    let mutations: [fn(&mut NoticesIndex); 6] = [
        |i| {
            i.native_sources.pop();
        },
        |i| i.native_sources[0].url = Some("https://example.invalid/openssl.tar.gz".into()),
        |i| i.native_sources.swap(0, 1),
        |i| i.windows_population_marker = "windows-link 0.2.0".into(),
        |i| i.schema = "wrong.schema".into(),
        |i| i.body_assembly_revision = "1".repeat(40),
    ];
    for mutate in mutations {
        let mut fixture = Fixture::new();
        let mut index: NoticesIndex = serde_json::from_slice(&fixture.notices_index).unwrap();
        mutate(&mut index);
        fixture.notices_index = serde_json::to_vec(&index).unwrap();
        let error = fixture.admit().unwrap_err();
        assert!(
            error.starts_with("notices-sources") || error.starts_with("notices-index"),
            "{error}"
        );
    }
    let mut fixture = Fixture::new();
    let mut index: NoticesIndex = serde_json::from_slice(&fixture.notices_index).unwrap();
    for entry in &mut index.native_sources {
        if entry.name == "regorus-build-revision" {
            entry.original_sha256 = Some("1".repeat(64));
        }
    }
    fixture.notices_index = serde_json::to_vec(&index).unwrap();
    refuses(&fixture, "regorus-build-revision: notices index");
}

#[test]
fn report_source_list_must_equal_the_committed_list_exactly() {
    let mut fixture = Fixture::new();
    fixture
        .report
        .sources
        .retain(|s| s.name != "zlib-1.3.1.tar.gz");
    refuses(&fixture, "report-sources");

    let mut fixture = Fixture::new();
    fixture.report.sources.push(NvattestReportSourceEntry {
        name: BUILD_TOOLS[1].name.into(),
        url: Some(BUILD_TOOLS[1].url.into()),
        revision: None,
        original_sha256: None,
        sha256: BUILD_TOOLS[1].sha256.into(),
    });
    refuses(&fixture, "report-sources");

    let mut fixture = Fixture::new();
    fixture.report.sources[0].url = Some("https://example.invalid/openssl-3.6.1.tar.gz".into());
    refuses(&fixture, "report-sources");

    let mut fixture = Fixture::new();
    fixture.report.sources[0].sha256 = "1".repeat(64);
    refuses(&fixture, "report-sources");

    let mut fixture = Fixture::new();
    fixture
        .report
        .sources
        .retain(|s| s.name != "regorus-build-revision");
    refuses(
        &fixture,
        "regorus-build-revision: report has no regorus-build-revision",
    );

    for mutate in [
        |s: &mut NvattestReportSourceEntry| s.sha256 = "0".repeat(64),
        |s: &mut NvattestReportSourceEntry| s.revision = Some("1".repeat(40)),
        |s: &mut NvattestReportSourceEntry| s.original_sha256 = None,
        |s: &mut NvattestReportSourceEntry| s.url = Some("https://example.invalid".into()),
    ] {
        let mut fixture = Fixture::new();
        mutate(
            fixture
                .report
                .sources
                .iter_mut()
                .find(|s| s.name == "regorus-build-revision")
                .unwrap(),
        );
        refuses(&fixture, "regorus-build-revision: report entry differs");
    }
}

#[test]
fn report_build_tools_must_equal_the_committed_list() {
    let mut fixture = Fixture::new();
    fixture.report.build_tools.clear();
    refuses(&fixture, "report-tools");
    let mut fixture = Fixture::new();
    fixture.report.build_tools.pop();
    refuses(&fixture, "report-tools");
    let mut fixture = Fixture::new();
    fixture.report.build_tools[1].url = "https://example.invalid/cmake.zip".into();
    refuses(&fixture, "report-tools");
}

#[test]
fn regorus_build_revision_absent_from_the_bundle_manifest_admits() {
    let fixture = Fixture::new();
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_json(&fixture.pins)).unwrap();
    assert!(
        manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["path"] != "regorus-build-revision")
    );
    fixture.admit().unwrap();
}

#[test]
fn every_pinned_input_must_be_in_the_bundle_manifest_at_its_digest() {
    for (drop, change) in [
        ("zlib-1.3.1.tar.gz", false),
        ("cmake-3.31.12-windows-x86_64.zip", false),
        ("json-3.12.0.tar.xz", true),
        ("ca-bundle.pem", true),
    ] {
        let mut fixture = Fixture::new();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&manifest_json(&fixture.pins)).unwrap();
        let files = manifest["files"].as_array_mut().unwrap();
        if change {
            for file in files.iter_mut().filter(|f| f["path"] == drop) {
                file["sha256"] = serde_json::json!("1".repeat(64));
            }
        } else {
            files.retain(|f| f["path"] != drop);
        }
        fixture.set_manifest(serde_json::to_vec(&manifest).unwrap());
        refuses(&fixture, "bundle-manifest");
    }
}

#[test]
fn runtime_dlls_must_equal_the_admitted_msvc_members() {
    let mut fixture = Fixture::new();
    fixture.output_runtime[0][0] ^= 1;
    refuses(
        &fixture,
        "runtime-dll: output msvcp140.dll differs from msvc package member",
    );

    // The MSVC member itself differs from the build copy, under a pin it meets.
    let mut fixture = Fixture::new();
    fixture.msvc_runtime[1][0] ^= 1;
    fixture.pins.msvc_runtime.vcruntime140.sha256 = leak(sha256_hex(&fixture.msvc_runtime[1]));
    let exe = fixture.output_exe.clone();
    fixture.set_exe(exe);
    refuses(
        &fixture,
        "runtime-dll: output vcruntime140.dll differs from msvc package member",
    );

    let mut fixture = Fixture::new();
    fixture.msvc_runtime[2][0] ^= 1;
    refuses(
        &fixture,
        "runtime-dll: msvc input vcruntime140_1.dll differs from pin",
    );
}

#[test]
fn an_import_outside_the_allowlist_refuses_even_when_the_evidence_agrees() {
    let mut fixture = Fixture::new();
    let exe = make_test_pe(false, &["kernel32.dll", "unauthorized.dll"]);
    fixture.output_exe = exe;
    refuses(&fixture, "import-allowlist");

    // The same import added to the executable and to every evidence record.
    let mut fixture = Fixture::new();
    fixture.set_exe(make_test_pe(
        false,
        &["kernel32.dll", "crypt32.dll", "winhttp.dll"],
    ));
    assert!(fixture.report.imports.iter().any(|i| i == "winhttp.dll"));
    refuses(&fixture, "import-allowlist");
}

#[test]
fn report_and_dumpbin_imports_corroborate_the_admitted_bytes() {
    let mut fixture = Fixture::new();
    fixture.report.imports.push("user32.dll".into());
    refuses(&fixture, "report-imports");
    let mut fixture = Fixture::new();
    fixture.report.imports.push("winhttp.dll".into());
    refuses(&fixture, "import-allowlist: report");
    let mut fixture = Fixture::new();
    fixture.evidence.dumpbin_dependents.nvattest_exe =
        base64_encode(dumpbin_text(&["KERNEL32.dll"]).as_bytes());
    refuses(&fixture, "dumpbin");
    let mut fixture = Fixture::new();
    fixture.evidence.dumpbin_dependents.vcruntime140 = String::new();
    refuses(&fixture, "dumpbin");
}

#[test]
fn captured_import_names_in_mixed_case_admit() {
    let report = decode_build_report(CAPTURED_REPORT).unwrap();
    let names: Vec<&str> = report.imports.iter().map(String::as_str).collect();
    // The captured list names KERNEL32 twice in different case; one descriptor
    // per distinct DLL is what a linker writes.
    let mut distinct: Vec<&str> = Vec::new();
    for name in names {
        if !distinct.iter().any(|d| d.eq_ignore_ascii_case(name)) {
            distinct.push(name);
        }
    }
    assert!(
        distinct
            .iter()
            .any(|n| n.chars().any(|c| c.is_ascii_uppercase()))
    );
    let mut fixture = Fixture::new();
    fixture.set_exe(make_test_pe(false, &distinct));
    fixture.admit().unwrap();
    for extra in [
        "dbghelp.dll",
        "api-ms-win-crt-private-l1-1-0.dll",
        "Api-Ms-Win-Crt-Runtime-L1-1-1.dll",
    ] {
        let mut with_extra = distinct.clone();
        with_extra.push(extra);
        let mut fixture = Fixture::new();
        fixture.set_exe(make_test_pe(false, &with_extra));
        refuses(&fixture, "import-allowlist");
    }
}

#[test]
fn delay_imports_and_forwarders_outside_the_allowlist_refuse() {
    let clear_dll = |mut bytes: Vec<u8>| {
        let pe_offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let mut characteristics =
            u16::from_le_bytes(bytes[pe_offset + 22..pe_offset + 24].try_into().unwrap());
        characteristics &= !0x2000;
        bytes[pe_offset + 22..pe_offset + 24].copy_from_slice(&characteristics.to_le_bytes());
        bytes
    };
    let mut delayed = clear_dll(crate::pe_dependencies::tests::with_import(true));
    delayed[0x280..0x280 + 17].copy_from_slice(b"unauthorized.dll\0");
    assert_eq!(
        crate::pe_dependencies::inspect_dependencies(&delayed)
            .unwrap()
            .delay_imports,
        ["unauthorized.dll"]
    );
    let mut fixture = Fixture::new();
    fixture.output_exe = delayed;
    refuses(&fixture, "import-allowlist");

    let forwarder = clear_dll(crate::pe_dependencies::tests::with_forwarder(
        b"unauthorized.TestFunc\0",
    ));
    let mut fixture = Fixture::new();
    fixture.output_exe = forwarder;
    refuses(&fixture, "import-allowlist");
}

#[test]
fn a_dll_or_non_amd64_pe32plus_image_refuses_as_the_verifier() {
    let mut fixture = Fixture::new();
    fixture.set_exe(make_test_pe(true, &["kernel32.dll"]));
    refuses(&fixture, "pe-kind");
    let mut fixture = Fixture::new();
    fixture.output_exe = pe::fixture_pe32();
    refuses(&fixture, "dependency census requires PE32+");
    let mut fixture = Fixture::new();
    fixture.output_exe = pe::fixture(&pe::FixtureSpec {
        machine: pe::IMAGE_FILE_MACHINE_ARM64,
        ..pe::FixtureSpec::default()
    });
    refuses(&fixture, "Windows payload requires AMD64 PE images");
}

#[test]
fn census_and_report_disagreeing_on_any_pinned_tool_refuses() {
    let mutations: [fn(&mut NvattestReportTools); 5] = [
        |t| t.rustc = "rustc 1.97.1 (8bab26f4f 2026-07-15)".into(),
        |t| t.cargo = "cargo 1.97.1 (c980f4866 2026-07-01)".into(),
        |t| t.cmake = "cmake version 3.31.12-dirty".into(),
        |t| t.windows_sdk = "10.0.26100.0".into(),
        |t| t.msvc = "14.44.35208".into(),
    ];
    for mutate in mutations {
        let mut fixture = Fixture::new();
        mutate(&mut fixture.report.tools);
        refuses(&fixture, "tool-census");
    }
}

#[test]
fn census_with_a_null_missing_or_unparsable_version_refuses() {
    let mutations: [fn(&mut NvattestToolCensus); 10] = [
        |c| c.rustc.version = None,
        |c| c.cargo.version = Some(String::new()),
        |c| c.cmake.version = Some("cmake".into()),
        |c| c.cl.version = None,
        |c| c.link.version = Some(" ".into()),
        |c| c.nmake.version = None,
        |c| c.msbuild.version = None,
        |c| c.msvc.vc_tools_version = None,
        |c| c.windows_sdk.version = None,
        |c| c.rustc.sha256 = "A".repeat(64),
    ];
    for mutate in mutations {
        let mut fixture = Fixture::new();
        mutate(&mut fixture.evidence.census);
        refuses(&fixture, "census-version");
    }
    // A key deleted outright, not only nulled.
    let fixture = Fixture::new();
    let mut evidence: serde_json::Value =
        serde_json::from_slice(&fixture.evidence_bytes()).unwrap();
    evidence["census"]["cargo"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    let evidence = serde_json::to_vec(&evidence).unwrap();
    let receipt = fixture.receipt_bytes(&evidence);
    let error = admit(
        &fixture.pins,
        &AdmissionBytes {
            receipt: &receipt,
            evidence: &evidence,
            validation: &fixture.validation,
            notices_index: &fixture.notices_index,
            notices_body: &fixture.notices_body,
            source_archive: &fixture.source_archive,
            bundle_archive: &fixture.bundle_archive,
            output_exe: &fixture.output_exe,
            output_license: &fixture.output_license,
            output_msvcp140: &fixture.output_runtime[0],
            output_vcruntime140: &fixture.output_runtime[1],
            output_vcruntime140_1: &fixture.output_runtime[2],
        },
        &fixture.msvc(),
    )
    .unwrap_err();
    assert!(error.contains("census-version: cargo"), "{error}");
}

#[test]
fn census_versions_compare_exact_tokens_and_digests() {
    type Case = (fn(&mut Fixture), &'static str);
    let cases: [Case; 8] = [
        (
            |f| {
                f.evidence.census.rustc.version =
                    Some("rustc 1.97.10 (8bab26f4f 2026-07-14)".into());
                f.report.tools.rustc = "rustc 1.97.10 (8bab26f4f 2026-07-14)".into();
            },
            "census-version",
        ),
        (
            |f| {
                f.evidence.census.rustc.version = Some("rustc 1.97.1 (8bab26f4 2026-07-14)".into());
                f.report.tools.rustc = "rustc 1.97.1 (8bab26f4 2026-07-14)".into();
            },
            "census-version",
        ),
        (
            |f| {
                f.evidence.census.cargo.version = Some("cargo 1.97.1-nightly".into());
                f.report.tools.cargo = "cargo 1.97.1-nightly".into();
            },
            "census-version",
        ),
        (
            |f| {
                f.evidence.census.windows_sdk.version = Some("10.0.26100.01\\".into());
                f.report.tools.windows_sdk = "10.0.26100.01\\".into();
            },
            "census-version",
        ),
        (
            |f| {
                f.evidence.census.windows_sdk.version = Some("10.0.26100.0\\\\".into());
                f.report.tools.windows_sdk = "10.0.26100.0\\\\".into();
            },
            "census-version",
        ),
        (
            |f| {
                f.evidence.census.msvc.vc_tools_version = Some("14.44.3520".into());
                f.report.tools.msvc = "14.44.3520".into();
            },
            "census-version",
        ),
        (
            |f| f.evidence.census.rustc.sha256 = "1".repeat(64),
            "census-digest: rustc",
        ),
        (
            |f| f.evidence.census.cmake.sha256 = "1".repeat(64),
            "census-digest: cmake",
        ),
    ];
    for (mutate, boundary) in cases {
        let mut fixture = Fixture::new();
        mutate(&mut fixture);
        refuses(&fixture, boundary);
    }
    // The real value keeps one trailing backslash and admits.
    let fixture = Fixture::new();
    assert_eq!(
        fixture.evidence.census.windows_sdk.version.as_deref(),
        Some("10.0.26100.0\\")
    );
    fixture.admit().unwrap();
}

#[test]
fn invocation_must_be_offline_at_the_pins_in_the_committed_environment() {
    let mutations: [fn(&mut NvattestInvocation); 6] = [
        |i| i.offline = false,
        |i| i.manifest_sha256 = "0".repeat(64),
        |i| i.source_commit = "1".repeat(40),
        |i| i.environment.push("OPENSSL_CONF".into()),
        |i| i.environment.retain(|n| n != "NVAT_SOURCE_COMMIT"),
        |i| i.argv.clear(),
    ];
    for mutate in mutations {
        let mut fixture = Fixture::new();
        mutate(&mut fixture.evidence.invocation);
        refuses(&fixture, "invocation");
    }
}

#[test]
fn refusal_controls_must_refuse_at_their_named_boundaries() {
    let mutations: [fn(&mut NvattestRefusals); 4] = [
        |r| r.manifest_digest.exit_code = 0,
        |r| r.reuse_dependencies.boundary = "offline builds".into(),
        |r| {
            r.corrupt_member.boundary = "missing or changed offline input: zlib-1.3.1.tar.gz".into()
        },
        |r| r.corrupt_member.exit_code = 0,
    ];
    for mutate in mutations {
        let mut fixture = Fixture::new();
        mutate(&mut fixture.evidence.refusals);
        refuses(&fixture, "refusals");
    }
}

#[test]
fn network_controls_must_prove_ipv4_and_ipv6_denial_and_full_cleanup() {
    let mutations: [fn(&mut NvattestNetworkEvidence); 6] = [
        |n| n.ipv4.positive_control = "unavailable".into(),
        |n| n.ipv4.negative_control = NVATTEST_NETWORK_CONNECTED.into(),
        |n| n.ipv6.positive_control = "unavailable".into(),
        |n| n.ipv6.negative_control = NVATTEST_NETWORK_CONNECTED.into(),
        |n| n.rules_remaining = 1,
        |n| n.rules_added = 0,
    ];
    for mutate in mutations {
        let mut fixture = Fixture::new();
        mutate(&mut fixture.evidence.network);
        refuses(&fixture, "network-rules");
    }
}

#[test]
fn the_embedded_report_must_decode_and_parse() {
    let mut fixture = Fixture::new();
    fixture.report_raw = Some(b"\xEF\xBB\xBFnot json".to_vec());
    refuses(&fixture, "report");
    let mut fixture = Fixture::new();
    fixture.evidence.schema = "other".into();
    refuses(&fixture, "evidence");
}

#[test]
fn base64_round_trips_and_refuses_malformed_text() {
    for input in [&b""[..], b"a", b"ab", b"abc", b"\xEF\xBB\xBF{\r\n}"] {
        assert_eq!(base64_decode(&base64_encode(input)).unwrap(), input);
    }
    for bad in ["A", "AB=C", "A===", "AA==AA==", "AB*="] {
        assert!(base64_decode(bad).is_err(), "{bad}");
    }
}

#[test]
fn captured_report_meets_the_committed_source_tool_and_census_contract() {
    let pins = production_pins();
    let report = decode_build_report(CAPTURED_REPORT).unwrap();
    assert!(CAPTURED_REPORT.starts_with(&[0xEF, 0xBB, 0xBF]));
    assert_eq!(report.source_commit, pins.sdk_revision);
    assert_eq!(report.sources.len(), 13);
    validate_report_sources(&pins, &report).unwrap();
    assert_eq!(
        report
            .sources
            .iter()
            .find(|s| s.name == "regorus-build-revision"),
        Some(&source_named("regorus-build-revision"))
    );
    validate_census(&pins, &census(&pins), &report).unwrap();
    let index: NoticesIndex = serde_json::from_slice(NOTICES_INDEX).unwrap();
    assert_eq!(index.native_sources, report.sources);
    validate_notices_index(&pins, &index).unwrap();
}

#[test]
fn captured_dumpbin_lists_exactly_the_captured_report_imports() {
    let report = decode_build_report(CAPTURED_REPORT).unwrap();
    let listed = dumpbin_dependents(&String::from_utf8_lossy(CAPTURED_DUMPBIN));
    let reported: BTreeSet<String> = report
        .imports
        .iter()
        .map(|i| i.to_ascii_lowercase())
        .collect();
    assert_eq!(listed, reported);
    assert!(
        listed
            .iter()
            .all(|name| IMPORT_ALLOWLIST.contains(&name.as_str()))
    );
}

#[test]
fn production_pins_carry_the_committed_toolchain_and_archive_identities() {
    let pins = production_pins();
    assert_eq!(
        pins.sdk_revision,
        "8fdbb0f8c10594a5f88f77fdec4766803b4e6d59"
    );
    assert_eq!(pins.source_archive.bytes, 5171200);
    assert_eq!(pins.bundle_archive.bytes, 451225600);
    assert_eq!(pins.native_sources.len(), 13);
    assert_eq!(
        pins.native_sources
            .iter()
            .filter(|s| matches!(s, NativeSourcePin::BuildRevision(_)))
            .count(),
        1
    );
    assert_eq!(pins.toolchain.windows_sdk, "10.0.26100.0");
    assert_eq!(sha256_hex(REGORUS_LOCK), pins.regorus_cargo_lock.sha256);
    assert_eq!(REGORUS_LOCK.len() as u64, pins.regorus_cargo_lock.bytes);
}
