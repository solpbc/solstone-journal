// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ARM SDK report and artifact checks. These do not admit a controlled receipt
//! or activate verifier authority; original x64 report admission stays separate.

use std::collections::{BTreeMap, BTreeSet};

use crate::controlled_build::InputIdentityEntry;
use crate::digest::sha256_hex;
use crate::nvattest_windows::{self as legacy, NvattestToolCensus};
use solstone_core_installed_payload::WindowsArchitecture;

pub const SDK_REVISION: &str = "25ea26bc6e3cee09a344f70f29c03481420e8077";
pub const SOURCE_BYTES: u64 = 5_201_920;
pub const SOURCE_SHA256: &str = "f2328f87f26a4bc7d1911e94735e18e40035b0ce633d3b7cbe9586952733ddbb";
pub const UTC_VECTORS_SHA256: &str =
    "7109d4c0dab5d8b017975d150a4632aa22d85635df5c2ab7776fe62101679933";
pub const TARGET: &str = "aarch64-pc-windows-msvc";

pub(crate) fn pins() -> legacy::Pins {
    let mut pins = legacy::production_pins();
    pins.sdk_revision = SDK_REVISION;
    pins.source_archive = legacy::ArchivePin {
        bytes: SOURCE_BYTES,
        sha256: SOURCE_SHA256,
    };
    pins
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeValidation {
    pub status: String,
    pub executable_sha256: String,
    pub vectors_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReport {
    pub schema: u32,
    pub target_architecture: String,
    pub target_triple: String,
    pub native_validation: NativeValidation,
    pub source_commit: String,
    pub source_clean: Option<bool>,
    pub tools: legacy::NvattestReportTools,
    pub sources: Vec<legacy::NvattestReportSourceEntry>,
    pub build_tools: Vec<legacy::NvattestReportBuildToolEntry>,
    pub ca_bundle: legacy::NvattestReportCaBundle,
    pub imports: Vec<String>,
    pub outputs: Vec<legacy::NvattestReportOutputEntry>,
}

pub fn decode_report(bytes: &[u8]) -> Result<BuildReport, String> {
    let json = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let report: BuildReport = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    validate_report_identity(&report)?;
    Ok(report)
}

fn validate_report_identity(report: &BuildReport) -> Result<(), String> {
    if report.schema != 1
        || report.source_commit != SDK_REVISION
        || report.target_architecture != "arm64"
        || report.target_triple != TARGET
        || report.source_clean == Some(false)
        || report.native_validation.status != "pending"
        || report.native_validation.vectors_sha256 != UTC_VECTORS_SHA256
    {
        return Err(
            "ARM nvattest report identity or cross-build/native-validation boundary differs".into(),
        );
    }
    Ok(())
}

/// The entire SDK argument vector is closed, including every refusal. A
/// second, abbreviated or colon-form target flag cannot override the target.
pub fn validate_sdk_argv(
    argv: &[String],
    script: &str,
    root: &str,
    bundle: &str,
    manifest: &str,
    reuse: bool,
) -> Result<(), String> {
    for path in [script, root, bundle] {
        crate::windows_clang::windows_path(path)?;
    }
    if manifest.len() != 64
        || !manifest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("ARM nvattest invocation manifest is not a SHA-256".into());
    }
    let mut expected: Vec<String> = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        script,
        "-Root",
        root,
        "-OfflineBundle",
        bundle,
        "-OfflineManifestSha256",
        manifest,
        "-TargetArchitecture",
        "arm64",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if reuse {
        expected.push("-ReuseDependencies".into());
    }
    if argv != expected {
        return Err("ARM nvattest SDK argv differs from the closed native recipe".into());
    }
    Ok(())
}

pub fn inspect_artifacts(
    report_bytes: &[u8],
    census: &NvattestToolCensus,
    outputs: &BTreeMap<String, Vec<u8>>,
    utc_executable: &[u8],
    utc_vectors: &[u8],
) -> Result<Vec<InputIdentityEntry>, String> {
    let report = decode_report(report_bytes)?;
    let pins = pins();
    legacy::validate_census_tools(&pins, census, &report.tools)?;
    legacy::validate_report_source_lists(&pins, &report.sources, &report.build_tools)?;
    for (tool, suffix) in [
        (&census.cl, "/bin/hostx64/arm64/cl.exe"),
        (&census.link, "/bin/hostx64/arm64/link.exe"),
    ] {
        if !crate::windows_clang::windows_path(&tool.path)?.ends_with(suffix) {
            return Err("ARM nvattest census selects a foreign MSVC target".into());
        }
    }
    let expected = BTreeSet::from([
        "LICENSE",
        "bin/msvcp140.dll",
        "bin/nvattest.exe",
        "bin/vcruntime140.dll",
        "share/ca/ca-bundle.pem",
    ]);
    if outputs.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected
        || report.outputs.len() != expected.len()
        || report
            .outputs
            .iter()
            .map(|o| o.path.as_str())
            .collect::<BTreeSet<_>>()
            != expected
    {
        return Err("ARM nvattest requires its exact five-member native SDK output layout".into());
    }
    for row in &report.outputs {
        let bytes = &outputs[&row.path];
        if row.bytes != bytes.len() as u64 || row.sha256 != sha256_hex(bytes) {
            return Err(format!(
                "ARM nvattest report output bytes differ: {}",
                row.path
            ));
        }
    }
    for (label, size, digest) in [
        ("LICENSE", pins.license.bytes, pins.license.sha256),
        (
            "share/ca/ca-bundle.pem",
            pins.ca_bundle.bytes,
            pins.ca_bundle.sha256,
        ),
        (
            "bin/msvcp140.dll",
            1_372_216,
            "045faeab0b5710816ae160ea20dae8495ffc11bb51feca15d22aaf7ff3752cb3",
        ),
        (
            "bin/vcruntime140.dll",
            199_200,
            "d8a8513921544569837e400d37cc71302819967ae6defa932266a7ecb41dbaf9",
        ),
    ] {
        let bytes = &outputs[label];
        if bytes.len() as u64 != size || sha256_hex(bytes) != digest {
            return Err(format!(
                "ARM nvattest output differs from its pinned source/runtime: {label}"
            ));
        }
    }
    if report.ca_bundle.sha256 != pins.ca_bundle.sha256
        || sha256_hex(utc_vectors) != UTC_VECTORS_SHA256
        || report.native_validation.executable_sha256 != sha256_hex(utc_executable)
    {
        return Err("ARM nvattest CA or original pending UTC validation artifacts differ".into());
    }
    let exe = &outputs["bin/nvattest.exe"];
    let pe = inspect_executable(exe)?;
    for required in ["msvcp140.dll", "vcruntime140.dll"] {
        if !pe
            .imports
            .iter()
            .any(|name| name.eq_ignore_ascii_case(required))
        {
            return Err(format!(
                "ARM nvattest lacks its required dynamic CRT import: {required}"
            ));
        }
    }
    let declared: BTreeSet<_> = report
        .imports
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if declared != pe.imports.iter().map(|s| s.to_ascii_lowercase()).collect() {
        return Err("ARM nvattest declared imports differ from its actual PE".into());
    }
    inspect_executable(utc_executable)?;
    for label in ["bin/msvcp140.dll", "bin/vcruntime140.dll"] {
        if !crate::pe_dependencies::inspect_dependencies(
            &outputs[label],
            WindowsArchitecture::Aarch64,
        )?
        .is_dll
        {
            return Err("ARM nvattest CRT member is not a native DLL".into());
        }
    }
    let mut facts: Vec<_> = outputs
        .iter()
        .map(|(label, bytes)| identity(label, bytes))
        .collect();
    facts.extend([
        identity("report/build-report.json", report_bytes),
        identity("validation/nvat_windows_utc_test.exe", utc_executable),
        identity("validation/utc-vectors.json", utc_vectors),
    ]);
    Ok(facts)
}

fn identity(label: &str, bytes: &[u8]) -> InputIdentityEntry {
    InputIdentityEntry {
        label: label.into(),
        size: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    }
}

fn inspect_executable(bytes: &[u8]) -> Result<crate::pe_dependencies::PeDependencies, String> {
    let pe = crate::pe_dependencies::inspect_dependencies(bytes, WindowsArchitecture::Aarch64)?;
    if pe.is_dll || !pe.delay_imports.is_empty() || !pe.forwarders.is_empty() {
        return Err("ARM nvattest requires a classic native executable without delayed dependencies or forwarded exports".into());
    }
    for import in &pe.imports {
        let name = import.to_ascii_lowercase();
        if name == "vcruntime140_1.dll" || !legacy::IMPORT_ALLOWLIST.contains(&name.as_str()) {
            return Err(format!("ARM nvattest has a foreign dependency: {import}"));
        }
    }
    Ok(pe)
}

pub fn run_cli(args: &[String]) -> Result<String, String> {
    use std::path::Path;
    const USAGE: &str = "nvattest-windows-arm64 inspect-artifacts --report ABS --tool-census ABS --output-root ABS --utc-test ABS --utc-vectors ABS";
    let Some((operation, rest)) = args.split_first() else {
        return Err(USAGE.into());
    };
    if operation != "inspect-artifacts" || rest.len() != 10 {
        return Err(USAGE.into());
    }
    let mut flags = BTreeMap::new();
    for pair in rest.chunks_exact(2) {
        if ![
            "--report",
            "--tool-census",
            "--output-root",
            "--utc-test",
            "--utc-vectors",
        ]
        .contains(&pair[0].as_str())
            || !Path::new(&pair[1]).is_absolute()
            || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("unknown, duplicate or non-absolute ARM nvattest artifact flag".into());
        }
    }
    let read = |name: &str, limit| {
        crate::produce::windows_build::read_bounded(Path::new(flags[name]), limit)
    };
    let census: NvattestToolCensus =
        serde_json::from_slice(&read("--tool-census", 1024 * 1024)?).map_err(|e| e.to_string())?;
    // This reader snapshots only regular files under bounded entry/byte budgets.
    let outputs = crate::llama_windows_capture::read_capture(Path::new(flags["--output-root"]))?;
    let facts = inspect_artifacts(
        &read("--report", 4 * 1024 * 1024)?,
        &census,
        &outputs,
        &read("--utc-test", 16 * 1024 * 1024)?,
        &read("--utc-vectors", 1024 * 1024)?,
    )?;
    serde_json::to_string(&serde_json::json!({
        "schema": "solstone.nvattest-windows-arm64-artifact-inspection.v1",
        "artifacts": facts, "controlled_receipt_admitted": false,
        "native_arm_execution": false, "verifier_authority_activated": false,
    }))
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(machine: u16, dll: bool, import: &str) -> Vec<u8> {
        use crate::pe::{FixtureSpec, ImportSpec, PeSymbolSpec, fixture};
        let symbols = [PeSymbolSpec::Named("entry")];
        let mut bytes = fixture(&FixtureSpec {
            machine,
            dll,
            imports: &[ImportSpec {
                name: import,
                symbols: &symbols,
            }],
            ..FixtureSpec::default()
        });
        let at = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let optional_size =
            u16::from_le_bytes(bytes[at + 20..at + 22].try_into().unwrap()) as usize;
        bytes[at + 24 + 60..at + 24 + 64]
            .copy_from_slice(&((at + 24 + optional_size + 40) as u32).to_le_bytes());
        bytes
    }

    #[test]
    fn actual_pe_bytes_require_classic_arm_and_refuse_the_foreign_crt() {
        inspect_executable(&executable(0xaa64, false, "KERNEL32.dll")).unwrap();
        for machine in [0x8664, 0x14c, 0xa641, 0xa64e] {
            assert!(inspect_executable(&executable(machine, false, "KERNEL32.dll")).is_err());
        }
        assert!(inspect_executable(&executable(0xaa64, true, "KERNEL32.dll")).is_err());
        for import in ["vcruntime140_1.dll", "vcomp140.dll", "unknown.dll"] {
            assert!(inspect_executable(&executable(0xaa64, false, import)).is_err());
        }
    }

    fn synthetic_report() -> serde_json::Value {
        serde_json::json!({
            "schema": 1, "target_architecture": "arm64", "target_triple": TARGET,
            "native_validation": {"status": "pending", "executable_sha256": "a".repeat(64), "vectors_sha256": UTC_VECTORS_SHA256},
            "source_commit": SDK_REVISION, "source_clean": null,
            "tools": {"msvc": "synthetic", "vs": "synthetic", "windows_sdk": "synthetic", "cmake": "synthetic", "rustc": "synthetic", "cargo": "synthetic", "crt_redist": "synthetic"},
            "sources": [], "build_tools": [], "ca_bundle": {"url": "synthetic", "sha256": "a".repeat(64)},
            "imports": [], "outputs": []
        })
    }

    #[test]
    fn arm_report_header_never_claims_native_execution_or_accepts_x64_identity() {
        let good = synthetic_report();
        let bytes = serde_json::to_vec(&good).unwrap();
        decode_report(&bytes).unwrap();
        let mut bom = vec![0xef, 0xbb, 0xbf];
        bom.extend_from_slice(&bytes);
        decode_report(&bom).unwrap();
        assert!(legacy::decode_build_report(&bytes).is_err());
        for (field, value) in [
            ("target_architecture", serde_json::json!("x64")),
            ("target_triple", serde_json::json!("x86_64-pc-windows-msvc")),
            (
                "source_commit",
                serde_json::json!("69a71c859ec02b6e5f10616b8b8a873c230941c9"),
            ),
            ("source_clean", serde_json::json!(false)),
            ("schema", serde_json::json!(2)),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(
                decode_report(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "{field}"
            );
        }
        for field in ["status", "vectors_sha256"] {
            let mut bad = good.clone();
            bad["native_validation"][field] = serde_json::json!("passed");
            assert!(decode_report(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        let mut unknown = good;
        unknown["architecture_override"] = serde_json::json!("x64");
        assert!(decode_report(&serde_json::to_vec(&unknown).unwrap()).is_err());
    }

    #[test]
    fn every_real_and_refusal_invocation_selects_arm_exactly_once() {
        let manifest = "a".repeat(64);
        for reuse in [false, true] {
            let mut argv: Vec<String> = [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "C:/source/sol/windows/build.ps1",
                "-Root",
                "C:/build",
                "-OfflineBundle",
                "C:/bundle",
                "-OfflineManifestSha256",
                &manifest,
                "-TargetArchitecture",
                "arm64",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect();
            if reuse {
                argv.push("-ReuseDependencies".into());
            }
            let check = |args: &[String]| {
                validate_sdk_argv(
                    args,
                    "C:/source/sol/windows/build.ps1",
                    "C:/build",
                    "C:/bundle",
                    &manifest,
                    reuse,
                )
            };
            check(&argv).unwrap();
            for replacement in ["x64", "ARM64", "arm64ec"] {
                let mut bad = argv.clone();
                bad[13] = replacement.into();
                assert!(check(&bad).is_err());
            }
            for extra in [
                vec!["-TargetArchitecture", "x64"],
                vec!["-TargetArch", "x64"],
                vec!["-TargetArchitecture:arm64"],
                vec!["-File", "C:/foreign.ps1"],
            ] {
                let mut bad = argv.clone();
                bad.extend(extra.into_iter().map(str::to_owned));
                assert!(check(&bad).is_err());
            }
            argv.drain(12..14);
            assert!(check(&argv).is_err());
        }
    }

    #[test]
    fn arm_and_x64_build_from_one_sdk_source() {
        assert_eq!(
            legacy::production_pins().sdk_revision,
            "25ea26bc6e3cee09a344f70f29c03481420e8077"
        );
        assert_eq!(pins().sdk_revision, SDK_REVISION);
        assert_eq!(pins().sdk_revision, legacy::production_pins().sdk_revision);
        assert_eq!(
            pins().source_archive,
            legacy::production_pins().source_archive
        );
        assert_eq!(
            pins().bundle_archive,
            legacy::production_pins().bundle_archive
        );
    }

    #[test]
    fn artifact_cli_rejects_missing_relative_and_duplicate_flags() {
        assert!(run_cli(&[]).is_err());
        assert!(run_cli(&["record".into()]).is_err());
        let mut args = vec!["inspect-artifacts".into()];
        for name in [
            "--report",
            "--tool-census",
            "--output-root",
            "--utc-test",
            "--utc-vectors",
        ] {
            args.extend([name.into(), "relative".into()]);
        }
        assert!(run_cli(&args).unwrap_err().contains("non-absolute"));
        #[cfg(unix)]
        let absolute = "/capture";
        #[cfg(windows)]
        let absolute = "C:/capture";
        for value in args.iter_mut().skip(2).step_by(2) {
            *value = absolute.into();
        }
        args[9] = args[1].clone();
        assert!(run_cli(&args).unwrap_err().contains("duplicate"));
    }
}
