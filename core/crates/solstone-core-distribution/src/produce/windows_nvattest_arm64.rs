// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Original terminal capture admission for the ARM verifier producer. Native
//! UTC/SNP/G23 execution and authority activation remain separate release gates.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::windows_build::{capture_source, read_bounded};
use super::windows_inputs::{AdmittedControlledInput, snapshot_file, with_snapshot};
use crate::controlled_build::*;
use crate::digest::sha256_hex;
use crate::nvattest_windows as common;
use crate::nvattest_windows_arm64 as arm;
use crate::provenance::Provenance;
use crate::windows_clang::windows_path;

pub const CAPTURE_SCHEMA: &str = "solstone.nvattest-windows-arm64-capture.v1";
pub const EVIDENCE_SCHEMA: &str = "solstone.nvattest-windows-arm64-evidence.v1";
pub const EVIDENCE_LABEL: &str = "provenance/windows-aarch64/nvattest-build-evidence.json";
const DRIVER: &str = "core/distribution/nvattest-windows-arm64-build.ps1";
const HELPERS: [&str; 3] = [
    "core/distribution/rfdetr-windows-capture.ps1",
    "core/distribution/windows-rust-toolchain.ps1",
    "core/distribution/windows-git-programs.ps1",
];
const STEPS: [&str; 22] = [
    "vswhere",
    "rust-installed-targets",
    "rust-installed-components",
    "rust-selected-cargo",
    "rust-pinned-cargo",
    "rust-selected-rustc",
    "rust-pinned-rustc",
    "recorder-environment",
    "recorder-build",
    "verify-inputs",
    "source-extract",
    "bundle-extract",
    "census-probe",
    "cmake-extract",
    "cmake-version",
    "refusal-manifest-digest",
    "refusal-reuse-dependencies",
    "refusal-changed-input",
    "sdk-build",
    "dumpbin-nvattest-exe",
    "dumpbin-msvcp140-dll",
    "dumpbin-vcruntime140-dll",
];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureHeader {
    pub schema: String,
    pub product: Provenance,
    pub driver_sha256: String,
    pub helpers: BTreeMap<String, String>,
    pub recorder_sha256: String,
    pub work_root: String,
    pub repository_root: String,
    pub report_root: String,
    pub output_root: String,
    pub source_archive: String,
    pub bundle_archive: String,
    pub system_before: Vec<InputIdentityEntry>,
    pub system_after: Vec<InputIdentityEntry>,
    pub tool_paths: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeProcess {
    pid: u32,
    started: bool,
    exit_code: i32,
    completed: bool,
    elapsed_ms: u64,
    error: Option<String>,
}

#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Terminal {
    exit_code: i32,
    failures: Vec<String>,
    firewall_rules_added: u32,
    firewall_rules_remaining: i32,
    children_stopped_before_lifting_denial: u32,
    firewall_rules: Vec<String>,
    captured_processes: Vec<NativeProcess>,
    product_commit: String,
    cargo_lock_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeStep {
    label: String,
    argv: Vec<String>,
    cwd: String,
    cmd_argument_line: String,
    launch_attempted: bool,
    started: bool,
    pid: u32,
    exit_code: i32,
    completed: bool,
    elapsed_ms: u64,
    error: Option<String>,
    stdout_sha256: String,
    stderr_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refusal {
    pub exit_code: i32,
    pub boundary: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriverControls {
    pub schema: String,
    pub invocation: common::NvattestInvocation,
    pub refusals: BTreeMap<String, Refusal>,
    pub network: common::NvattestNetworkEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildEvidence {
    pub schema: String,
    pub product: Provenance,
    pub driver_sha256: String,
    pub helpers: BTreeMap<String, String>,
    pub inputs: Vec<InputIdentityEntry>,
    pub capture_members: Vec<InputIdentityEntry>,
    pub output_members: Vec<InputIdentityEntry>,
    pub artifact_facts: Vec<InputIdentityEntry>,
    pub native_execution_pending: bool,
}

pub struct InputPaths<'a> {
    pub source_archive: &'a Path,
    pub bundle_archive: &'a Path,
    pub capture_root: &'a Path,
    pub output_root: &'a Path,
    pub receipt: &'a Path,
    pub evidence: &'a Path,
}

fn member<'a>(files: &'a BTreeMap<String, Vec<u8>>, label: &str) -> Result<&'a [u8], String> {
    files
        .get(label)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("ARM nvattest original capture lacks {label}"))
}
fn decode<T: serde::de::DeserializeOwned>(
    files: &BTreeMap<String, Vec<u8>>,
    label: &str,
) -> Result<T, String> {
    serde_json::from_slice(member(files, label)?).map_err(|e| format!("{label}: {e}"))
}
fn identities(files: &BTreeMap<String, Vec<u8>>) -> Vec<InputIdentityEntry> {
    files
        .iter()
        .map(|(label, bytes)| InputIdentityEntry {
            label: label.clone(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        })
        .collect()
}

struct SourceMaterial {
    inputs: Vec<InputIdentityEntry>,
    files: BTreeMap<String, Vec<u8>>,
}

fn source_material(source: &Path, bundle: &Path) -> Result<SourceMaterial, String> {
    let pins = arm::pins();
    with_snapshot(|root| {
        let source = snapshot_file(root, "source.tar", source, pins.source_archive.bytes)?;
        let bundle = snapshot_file(root, "bundle.tar", bundle, pins.bundle_archive.bytes)?;
        let source_bytes = read_bounded(&source, pins.source_archive.bytes)?;
        let bundle_bytes = read_bounded(&bundle, pins.bundle_archive.bytes)?;
        let mut material = common::admitted_archive_material(&pins, &source_bytes, &bundle_bytes)?;
        let index_bytes =
            include_bytes!("../../../../distribution/nvattest-windows-arm64-sources.json");
        let index: common::NoticesIndex =
            serde_json::from_slice(index_bytes).map_err(|e| e.to_string())?;
        common::validate_notices_index(&pins, &index)?;
        let notices = include_bytes!("../../../../distribution/nvattest-windows-NOTICES.md");
        if sha256_hex(notices) != pins.notices_body_sha256 {
            return Err("ARM nvattest notice body differs from pin".into());
        }
        material.insert("notices/nvattest-NOTICES.md".into(), notices.to_vec());
        material.insert("notices/nvattest-sources.json".into(), index_bytes.to_vec());
        Ok(SourceMaterial {
            inputs: vec![
                InputIdentityEntry {
                    label: "source.tar".into(),
                    size: source_bytes.len() as u64,
                    sha256: sha256_hex(&source_bytes),
                },
                InputIdentityEntry {
                    label: "bundle.tar".into(),
                    size: bundle_bytes.len() as u64,
                    sha256: sha256_hex(&bundle_bytes),
                },
            ],
            files: material,
        })
    })
}

fn validate_source_header(repo: &Path, header: &CaptureHeader) -> Result<(), String> {
    if header.schema != CAPTURE_SCHEMA
        || header.product.commit.len() != 40
        || !header
            .product
            .commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("ARM nvattest original product identity is malformed".into());
    }
    let git_hash = |file: &str| -> Result<String, String> {
        crate::rfdetr_windows_source::git_checked(
            repo,
            &[
                "show",
                "--no-textconv",
                &format!("{}:{file}", header.product.commit),
            ],
        )
        .map(|text| sha256_hex(text.as_bytes()))
        .map_err(|e| e.to_string())
    };
    if git_hash("core/Cargo.lock")? != header.product.lock_sha256
        || git_hash(DRIVER)? != header.driver_sha256
    {
        return Err(
            "ARM nvattest original product/driver bytes differ from their Git objects".into(),
        );
    }
    let expected = HELPERS
        .into_iter()
        .map(|name| Ok((name.to_owned(), git_hash(name)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    if header.helpers != expected {
        return Err("ARM nvattest original driver helper identities differ".into());
    }
    Ok(())
}

fn validate_system(
    header: &CaptureHeader,
    census: &common::NvattestToolCensus,
) -> Result<(), String> {
    if header.system_before != header.system_after || header.system_after.is_empty() {
        return Err("ARM nvattest system tools changed during capture".into());
    }
    let mut labels = BTreeSet::new();
    let mut previous = None;
    for row in &header.system_after {
        if previous.is_some_and(|p: &str| p >= row.label.as_str())
            || row.size == 0
            || row.sha256.len() != 64
            || !row
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !labels.insert(row.label.as_str())
        {
            return Err("ARM nvattest system census is malformed, unsorted or duplicated".into());
        }
        previous = Some(row.label.as_str());
    }
    if header
        .tool_paths
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != labels
    {
        return Err("ARM nvattest system-tool path population differs".into());
    }
    for path in header.tool_paths.values() {
        windows_path(path)?;
    }
    for required in [
        "windows/cmd.exe",
        "windows/tar.exe",
        "windows/powershell.exe",
        "windows/vswhere.exe",
        "windows/rustup.exe",
        "windows/recorder.exe",
        "windows/dumpbin.exe",
        "windows/git.exe",
        "windows/git-backend.exe",
        "windows/git-core.exe",
        "windows/git-http.exe",
        "msvc/cl.exe",
        "msvc/link.exe",
        "msvc/nmake.exe",
        "msvc/msbuild.exe",
        "windows/cargo.exe",
        "windows/cmake.exe",
        "windows/rustc.exe",
    ] {
        if !labels.contains(required) {
            return Err(format!("ARM nvattest system census lacks {required}"));
        }
    }
    for (label, tool) in [
        ("msvc/cl.exe", &census.cl),
        ("msvc/link.exe", &census.link),
        ("msvc/nmake.exe", &census.nmake),
        ("msvc/msbuild.exe", &census.msbuild),
        ("windows/cargo.exe", &census.cargo),
        ("windows/cmake.exe", &census.cmake),
        ("windows/rustc.exe", &census.rustc),
    ] {
        let row = header
            .system_after
            .iter()
            .find(|r| r.label == label)
            .unwrap();
        if row.sha256 != tool.sha256
            || windows_path(&header.tool_paths[label])? != windows_path(&tool.path)?
        {
            return Err(format!(
                "ARM nvattest selected tool differs from the complete census: {label}"
            ));
        }
    }
    if header
        .system_after
        .iter()
        .find(|r| r.label == "windows/recorder.exe")
        .unwrap()
        .sha256
        != header.recorder_sha256
    {
        return Err("ARM nvattest original recorder identity differs".into());
    }
    let backend = windows_path(&header.tool_paths["windows/git-backend.exe"])?;
    let prefix = backend
        .strip_suffix("/bin/git.exe")
        .ok_or("ARM nvattest Git backend path differs")?;
    let root = prefix
        .rsplit_once('/')
        .ok_or("ARM nvattest Git root missing")?
        .0;
    if windows_path(&header.tool_paths["windows/git-core.exe"])?
        != format!("{prefix}/libexec/git-core/git.exe")
        || windows_path(&header.tool_paths["windows/git-http.exe"])?
            != format!("{prefix}/libexec/git-core/git-remote-https.exe")
        || ![
            format!("{root}/cmd/git.exe"),
            backend.clone(),
            format!("{prefix}/libexec/git-core/git.exe"),
        ]
        .contains(&windows_path(&header.tool_paths["windows/git.exe"])?)
    {
        return Err("ARM nvattest installed Git executable layout differs".into());
    }
    Ok(())
}

fn validate_terminal(
    header: &CaptureHeader,
    controls: &DriverControls,
    terminal: &Terminal,
) -> Result<(), String> {
    if terminal.exit_code != 0
        || !terminal.failures.is_empty()
        || terminal.firewall_rules_remaining != 0
        || terminal.children_stopped_before_lifting_denial != 0
        || terminal.product_commit != header.product.commit
        || terminal.cargo_lock_sha256 != header.product.lock_sha256
        || terminal.firewall_rules_added != controls.network.rules_added
        || terminal.firewall_rules.len() != terminal.firewall_rules_added as usize
        || terminal
            .firewall_rules
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != terminal.firewall_rules.len()
        || terminal.captured_processes.len() != STEPS.len()
        || controls.schema != CAPTURE_SCHEMA
    {
        return Err(
            "ARM nvattest capture did not complete successfully with removed authority".into(),
        );
    }
    common::validate_network(&controls.network)?;
    Ok(())
}

fn inspect_capture(
    repo: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    outputs: &BTreeMap<String, Vec<u8>>,
    inputs: Vec<InputIdentityEntry>,
) -> Result<BuildEvidence, String> {
    let header: CaptureHeader = decode(files, "capture-header.json")?;
    validate_source_header(repo, &header)?;
    let census: common::NvattestToolCensus = decode(files, "tool-census.json")?;
    validate_system(&header, &census)?;
    let terminal: Terminal = decode(files, "execution.json")?;
    let controls: DriverControls = decode(files, "driver-controls.json")?;
    validate_terminal(&header, &controls, &terminal)?;
    let pins = arm::pins();
    common::validate_invocation(&pins, &controls.invocation)?;
    if controls
        .refusals
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != BTreeSet::from(["manifest_digest", "reuse_dependencies", "corrupt_member"])
    {
        return Err("ARM nvattest refusal population differs".into());
    }
    let legacy_refusal = |name: &str| common::NvattestRefusalEntry {
        exit_code: controls.refusals[name].exit_code,
        boundary: controls.refusals[name].boundary.clone(),
    };
    common::validate_refusals(&common::NvattestRefusals {
        manifest_digest: legacy_refusal("manifest_digest"),
        reuse_dependencies: legacy_refusal("reuse_dependencies"),
        corrupt_member: legacy_refusal("corrupt_member"),
    })?;
    for path in [
        &header.repository_root,
        &header.report_root,
        &header.output_root,
        &header.source_archive,
        &header.bundle_archive,
    ] {
        windows_path(path)?;
    }
    let work = &header.work_root;
    windows_path(work)?;
    let root = work.replace('\\', "/");
    let script = format!("{root}/source/sol/windows/build.ps1");
    let bundle = format!("{root}/offline-inputs");
    if windows_path(&controls.invocation.bundle_path)? != windows_path(&bundle)? {
        return Err("ARM nvattest native bundle path differs".into());
    }
    let mut expected_members: BTreeSet<String> = [
        "capture-header.json",
        "execution.json",
        "tool-census.json",
        "rust-toolchain.json",
        "driver-controls.json",
        "build-report.json",
        "nvattest-build-validation.log",
        "validation/nvat_windows_utc_test.exe",
        "validation/utc-vectors.json",
        "dumpbin/nvattest.exe.dependents.txt",
        "dumpbin/msvcp140.dll.dependents.txt",
        "dumpbin/vcruntime140.dll.dependents.txt",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for (label, terminal_process) in STEPS.iter().zip(&terminal.captured_processes) {
        let step: NativeStep = decode(files, &format!("logs/{label}.execution.json"))?;
        for suffix in ["stdout", "stderr", "execution.json"] {
            expected_members.insert(format!("logs/{label}.{suffix}"));
        }
        let stdout = member(files, &format!("logs/{label}.stdout"))?;
        let stderr = member(files, &format!("logs/{label}.stderr"))?;
        let process = NativeProcess {
            pid: step.pid,
            started: step.started,
            exit_code: step.exit_code,
            completed: step.completed,
            elapsed_ms: step.elapsed_ms,
            error: step.error.clone(),
        };
        if step.label != *label
            || !step.launch_attempted
            || !step.started
            || !step.completed
            || step.pid == 0
            || step.error.is_some()
            || step.elapsed_ms > 7_201_000
            || step.stdout_sha256 != sha256_hex(stdout)
            || step.stderr_sha256 != sha256_hex(stderr)
            || &process != terminal_process
            || step.argv.is_empty()
        {
            return Err(format!(
                "ARM nvattest original command is incomplete or changed: {label}"
            ));
        }
        windows_path(&step.argv[0])?;
        windows_path(&step.cwd)?;
        if !label.starts_with("refusal-") && *label != "sdk-build" {
            let (argv, cwd, raw) = expected_step(&header, label)?;
            if step.argv != argv
                || windows_path(&step.cwd)? != windows_path(&cwd)?
                || step.cmd_argument_line != raw
            {
                return Err(format!(
                    "ARM nvattest original native command differs: {label}"
                ));
            }
        } else if !step.cmd_argument_line.is_empty() {
            return Err("ARM nvattest SDK command has a raw argument override".into());
        }
        let refusal = match *label {
            "refusal-manifest-digest" => {
                Some(("manifest_digest", "refusal-manifest", "0".repeat(64), false))
            }
            "refusal-reuse-dependencies" => Some((
                "reuse_dependencies",
                "refusal-reuse",
                pins.manifest_sha256.into(),
                true,
            )),
            "refusal-changed-input" => Some((
                "corrupt_member",
                "refusal-changed",
                pins.manifest_sha256.into(),
                false,
            )),
            _ => None,
        };
        if let Some((key, directory, digest, reuse)) = refusal {
            let entry = &controls.refusals[key];
            if step.exit_code == 0
                || step.exit_code != entry.exit_code
                || step.argv != entry.argv
                || !(String::from_utf8_lossy(stdout).to_string() + &String::from_utf8_lossy(stderr))
                    .contains(&entry.boundary)
            {
                return Err(format!(
                    "ARM nvattest refusal does not match its original process: {key}"
                ));
            }
            validate_sdk_step(
                &step,
                &header,
                &script,
                &format!("{root}/{directory}"),
                &bundle,
                &digest,
                reuse,
            )?;
        } else {
            if step.exit_code != 0 {
                return Err(format!("ARM nvattest command failed: {label}"));
            }
            if *label == "sdk-build" {
                if controls.invocation.argv != step.argv {
                    return Err(
                        "ARM nvattest real invocation differs from its command record".into(),
                    );
                }
                validate_sdk_step(
                    &step,
                    &header,
                    &script,
                    &format!("{root}/build"),
                    &bundle,
                    pins.manifest_sha256,
                    false,
                )?;
            }
        }
    }
    if files.keys().cloned().collect::<BTreeSet<_>>() != expected_members {
        return Err("ARM nvattest original terminal capture membership differs".into());
    }
    let facts = arm::inspect_artifacts(
        member(files, "build-report.json")?,
        &census,
        outputs,
        member(files, "validation/nvat_windows_utc_test.exe")?,
        member(files, "validation/utc-vectors.json")?,
    )?;
    for name in ["nvattest.exe", "msvcp140.dll", "vcruntime140.dll"] {
        let label = name.replace('.', "-");
        if member(files, &format!("dumpbin/{name}.dependents.txt"))?
            != member(files, &format!("logs/dumpbin-{label}.stdout"))?
        {
            return Err("ARM nvattest dumpbin copy differs from its original stream".into());
        }
    }
    Ok(BuildEvidence {
        schema: EVIDENCE_SCHEMA.into(),
        product: header.product,
        driver_sha256: header.driver_sha256,
        helpers: header.helpers,
        inputs,
        capture_members: identities(files),
        output_members: identities(outputs),
        artifact_facts: facts,
        native_execution_pending: true,
    })
}

fn expected_step(
    header: &CaptureHeader,
    label: &str,
) -> Result<(Vec<String>, String, String), String> {
    let work = &header.work_root;
    let path = |suffix: &str| format!("{}\\{}", work.trim_end_matches('\\'), suffix);
    let tool = |name: &str| {
        header
            .tool_paths
            .get(name)
            .cloned()
            .ok_or_else(|| format!("ARM nvattest selected program missing: {name}"))
    };
    let mut cwd = work.clone();
    let mut raw = String::new();
    let argv: Vec<String> = match label {
        "vswhere" => [
            tool("windows/vswhere.exe")?,
            "-latest".into(),
            "-products".into(),
            "*".into(),
            "-requires".into(),
            "Microsoft.VisualStudio.Component.VC.Tools.ARM64".into(),
            "-property".into(),
            "installationPath".into(),
        ]
        .into(),
        "rust-installed-targets" | "rust-installed-components" => {
            cwd = header.report_root.clone();
            vec![
                tool("windows/rustup.exe")?,
                if label == "rust-installed-targets" {
                    "target"
                } else {
                    "component"
                }
                .into(),
                "list".into(),
                "--installed".into(),
                "--toolchain".into(),
                "1.97.1-x86_64-pc-windows-msvc".into(),
            ]
        }
        "rust-selected-cargo"
        | "rust-selected-rustc"
        | "rust-pinned-cargo"
        | "rust-pinned-rustc" => {
            cwd = header.repository_root.clone();
            let mut args = vec![tool("windows/rustup.exe")?, "which".into()];
            if label.starts_with("rust-pinned-") {
                args.extend(["--toolchain".into(), "1.97.1-x86_64-pc-windows-msvc".into()]);
            }
            args.push(
                if label.ends_with("cargo") {
                    "cargo"
                } else {
                    "rustc"
                }
                .into(),
            );
            args
        }
        "recorder-environment" => {
            raw = format!("/d /s /c \"\"{}\"\"", path("recorder-environment.cmd"));
            vec![tool("windows/cmd.exe")?]
        }
        "recorder-build" => {
            cwd = header.repository_root.clone();
            vec![
                tool("windows/cargo.exe")?,
                "build".into(),
                "--manifest-path".into(),
                "core\\Cargo.toml".into(),
                "-p".into(),
                "solstone-core-distribution".into(),
                "--bin".into(),
                "solstone-distribution".into(),
                "--target".into(),
                "x86_64-pc-windows-msvc".into(),
                "--target-dir".into(),
                path("recorder-target"),
                "--locked".into(),
                "--offline".into(),
                "-j".into(),
                "2".into(),
            ]
        }
        "verify-inputs" => {
            cwd = header.repository_root.clone();
            vec![
                tool("windows/recorder.exe")?,
                "nvattest-windows-arm64".into(),
                "verify-inputs".into(),
                "--source-archive".into(),
                header.source_archive.clone(),
                "--bundle-archive".into(),
                header.bundle_archive.clone(),
            ]
        }
        "source-extract" | "bundle-extract" => vec![
            tool("windows/tar.exe")?,
            "-xf".into(),
            if label == "source-extract" {
                header.source_archive.clone()
            } else {
                header.bundle_archive.clone()
            },
            "-C".into(),
            path(if label == "source-extract" {
                "source"
            } else {
                "offline-inputs"
            }),
        ],
        "census-probe" => vec![
            tool("windows/powershell.exe")?,
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            path("census-probe.ps1"),
            "-Out".into(),
            path("census-probe.json"),
        ],
        "cmake-extract" => vec![
            tool("windows/tar.exe")?,
            "-xf".into(),
            path("offline-inputs\\cmake-3.31.12-windows-x86_64.zip"),
            "-C".into(),
            path("tools-cmake"),
        ],
        "cmake-version" => vec![
            path("tools-cmake\\cmake-3.31.12-windows-x86_64\\bin\\cmake.exe"),
            "--version".into(),
        ],
        "dumpbin-nvattest-exe" | "dumpbin-msvcp140-dll" | "dumpbin-vcruntime140-dll" => {
            let name = match label {
                "dumpbin-nvattest-exe" => "nvattest.exe",
                "dumpbin-msvcp140-dll" => "msvcp140.dll",
                _ => "vcruntime140.dll",
            };
            vec![
                tool("windows/dumpbin.exe")?,
                "/nologo".into(),
                "/dependents".into(),
                path(&format!("build\\dist\\nvattest\\bin\\{name}")),
            ]
        }
        _ => return Err("ARM nvattest command label is not in its native recipe".into()),
    };
    Ok((argv, cwd, raw))
}

fn validate_sdk_step(
    step: &NativeStep,
    header: &CaptureHeader,
    script: &str,
    root: &str,
    bundle: &str,
    digest: &str,
    reuse: bool,
) -> Result<(), String> {
    if windows_path(&step.argv[0])? != windows_path(&header.tool_paths["windows/powershell.exe"])?
        || windows_path(&step.cwd)? != windows_path(&header.work_root)?
    {
        return Err("ARM nvattest SDK program or working directory differs".into());
    }
    // The caller records native Windows separators, so compare after binding
    // each expected path to that same spelling rather than changing raw argv.
    let backward = header.work_root.contains('\\');
    let spell = |path: &str| {
        if backward {
            path.replace('/', "\\")
        } else {
            path.to_owned()
        }
    };
    arm::validate_sdk_argv(
        &step.argv[1..],
        &spell(script),
        &spell(root),
        &spell(bundle),
        digest,
        reuse,
    )
}

fn receipt(
    evidence: &BuildEvidence,
    files: &BTreeMap<String, Vec<u8>>,
    outputs: &BTreeMap<String, Vec<u8>>,
    builder: BuilderIdentity,
) -> Result<ControlledBuildReceipt, String> {
    ControlledBuildReceiptDraft {
        schema: Some(CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into()),
        source: Some(SourceIdentity {product: evidence.product.clone(), windows_dependency: DependencySource {repository: "https://github.com/solpbc/attestation-sdk".into(), revision: arm::SDK_REVISION.into(), content_sha256: arm::SOURCE_SHA256.into()}}),
        inputs: Some(evidence.inputs.clone()), builder: Some(builder),
        configuration: Some(BuildConfiguration {target_triple: arm::TARGET.into(), profile: "Release".into(), flags: vec!["SDK -TargetArchitecture arm64; native UTC execution retained as a separate pending gate".into(), "global outbound denial except the recorded operator transport peer".into()], network_access_denied: true}),
        outputs: Some(census_outputs(&[("bin/nvattest.exe", outputs["bin/nvattest.exe"].as_slice())]).map_err(|e| e.to_string())?),
        validation: Some(ValidationReference {description: "Original ARM nvattest terminal capture after child census and firewall cleanup".into(), sha256: sha256_hex(&files["execution.json"])}),
        supporting: Some(vec![SupportingArtifactRef {label: EVIDENCE_LABEL.into(), sha256: sha256_hex(&serde_json::to_vec(evidence).map_err(|e| e.to_string())?)}]),
    }.validate().map_err(|e| e.to_string())
}

pub fn admit(repo: &Path, paths: InputPaths<'_>) -> Result<AdmittedControlledInput, String> {
    let SourceMaterial {
        inputs,
        files: mut material,
    } = source_material(paths.source_archive, paths.bundle_archive)?;
    let files = crate::llama_windows_capture::read_capture(paths.capture_root)?;
    let outputs = crate::llama_windows_capture::read_capture(paths.output_root)?;
    let evidence = inspect_capture(repo, &files, &outputs, inputs)?;
    let evidence_bytes = read_bounded(paths.evidence, 16 * 1024 * 1024)?;
    let receipt_bytes = read_bounded(paths.receipt, 16 * 1024 * 1024)?;
    let original = decode_controlled_build_receipt(&receipt_bytes).map_err(|e| e.to_string())?;
    if evidence_bytes != serde_json::to_vec(&evidence).map_err(|e| e.to_string())?
        || original != receipt(&evidence, &files, &outputs, original.builder.clone())?
    {
        return Err(
            "ARM nvattest original receipt/evidence differs from its terminal capture".into(),
        );
    }
    for (label, bytes) in &material {
        if !label.starts_with("notices/") && outputs.get(label) != Some(bytes) {
            return Err("ARM nvattest staged source material differs".into());
        }
    }
    material.insert(
        "bin/nvattest.exe".into(),
        outputs["bin/nvattest.exe"].clone(),
    );
    Ok(AdmittedControlledInput::new(
        original,
        receipt_bytes,
        evidence_bytes,
        files["execution.json"].clone(),
        material,
    ))
}

pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    const USAGE: &str = "nvattest-windows-arm64 verify-inputs|record|verify --source-archive ABS --bundle-archive ABS [FLAG VALUE]";
    let Some((operation, rest)) = args.split_first() else {
        return Err(USAGE.into());
    };
    let mut allowed = vec!["--source-archive", "--bundle-archive"];
    match operation.as_str() {
        "verify-inputs" => (),
        "record" | "verify" => allowed.extend([
            "--capture-root",
            "--output-root",
            "--receipt",
            "--evidence",
            "--repo",
        ]),
        _ => return Err(USAGE.into()),
    }
    if operation == "record" {
        allowed.extend(["--product-commit", "--cargo-lock-sha256", "--builder-host"]);
    }
    if rest.len() != allowed.len() * 2 {
        return Err(USAGE.into());
    }
    let mut flags = BTreeMap::new();
    for pair in rest.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str())
            || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("unknown or duplicate ARM nvattest control flag".into());
        }
    }
    let absolute = |key: &str| -> Result<PathBuf, String> {
        let path = PathBuf::from(flags[key]);
        if !path.is_absolute() {
            return Err(format!("absolute ARM nvattest path required: {key}"));
        }
        Ok(path)
    };
    let source = absolute("--source-archive")?;
    let bundle = absolute("--bundle-archive")?;
    if operation == "verify-inputs" {
        let SourceMaterial { inputs, .. } = source_material(&source, &bundle)?;
        return serde_json::to_string(&serde_json::json!({"inputs":inputs,"sdk_revision":arm::SDK_REVISION,"manifest_sha256":arm::pins().manifest_sha256})).map_err(|e| e.to_string());
    }
    let repo = absolute("--repo")?;
    let capture = absolute("--capture-root")?;
    let output = absolute("--output-root")?;
    let receipt_path = absolute("--receipt")?;
    let evidence_path = absolute("--evidence")?;
    if operation == "verify" {
        admit(
            &repo,
            InputPaths {
                source_archive: &source,
                bundle_archive: &bundle,
                capture_root: &capture,
                output_root: &output,
                receipt: &receipt_path,
                evidence: &evidence_path,
            },
        )?;
        return Ok("NVATTEST_ARM64_WINDOWS_VERIFY_PASS".into());
    }
    let before = capture_source(&repo)?;
    if before.commit != flags["--product-commit"]
        || before.lock_sha256 != flags["--cargo-lock-sha256"]
    {
        return Err("ARM nvattest recorder source differs from expected product".into());
    }
    if std::fs::canonicalize(start).map_err(|e| e.to_string())?
        != std::fs::canonicalize(&repo).map_err(|e| e.to_string())?
    {
        return Err("ARM nvattest record must run from its exact product checkout".into());
    }
    let canonical_capture = std::fs::canonicalize(&capture).map_err(|e| e.to_string())?;
    let canonical_output = std::fs::canonicalize(&output).map_err(|e| e.to_string())?;
    for path in [&receipt_path, &evidence_path] {
        let parent = std::fs::canonicalize(
            path.parent()
                .ok_or("ARM nvattest publication parent missing")?,
        )
        .map_err(|e| e.to_string())?;
        if parent.starts_with(&canonical_capture) || parent.starts_with(&canonical_output) {
            return Err(
                "ARM nvattest admission records must stay outside original captures".into(),
            );
        }
    }
    let SourceMaterial { inputs, .. } = source_material(&source, &bundle)?;
    let files = crate::llama_windows_capture::read_capture(&capture)?;
    let outputs = crate::llama_windows_capture::read_capture(&output)?;
    let evidence = inspect_capture(&repo, &files, &outputs, inputs)?;
    if evidence.product != before || capture_source(&repo)? != before {
        return Err("ARM nvattest original product changed or is being restamped".into());
    }
    let original = receipt(
        &evidence,
        &files,
        &outputs,
        BuilderIdentity {
            host: flags["--builder-host"].into(),
            toolchain: "MSVC14.44 ARM64 / pinned Rust1.97.1 / CMake3.31.12".into(),
        },
    )?;
    super::windows_openmp::publish(
        &evidence_path,
        &serde_json::to_vec(&evidence).map_err(|e| e.to_string())?,
    )?;
    let publication = write_controlled_build_receipt_exclusive(&receipt_path, &original)
        .map_err(|e| e.to_string())?;
    if matches!(
        publication,
        ControlledBuildReceiptPublication::PublicationUnconfirmed { .. }
    ) {
        return Err("ARM nvattest receipt publication unconfirmed".into());
    }
    Ok("NVATTEST_ARM64_WINDOWS_RECORD_PASS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_sdk_exit_cannot_hide_failed_cleanup_or_restamped_product() {
        let header:CaptureHeader=serde_json::from_value(serde_json::json!({
            "schema":CAPTURE_SCHEMA,"product":{"commit":"a".repeat(40),"lock_sha256":"b".repeat(64)},
            "driver_sha256":"c".repeat(64),"helpers":{},"recorder_sha256":"d".repeat(64),
            "work_root":"C:/work","repository_root":"C:/source","report_root":"C:/report","output_root":"C:/output",
            "source_archive":"C:/source.tar","bundle_archive":"C:/bundle.tar","system_before":[],"system_after":[],"tool_paths":{}
        })).unwrap();
        let controls:DriverControls=serde_json::from_value(serde_json::json!({
            "schema":CAPTURE_SCHEMA,"invocation":{"offline":true,"bundle_path":"C:/bundle","manifest_sha256":"a".repeat(64),"source_commit":arm::SDK_REVISION,"environment":[],"argv":[]},"refusals":{},
            "network":{"transport_peer":"192.0.2.1","ipv4":{"target":"1.1.1.1:443","positive_control":"connected","negative_control":"refused"},"ipv6":{"target":"[2606:4700:4700::1111]:443","positive_control":"unreachable","negative_control":"refused"},"rules_added":2,"rules_remaining":0}
        })).unwrap();
        let terminal = Terminal {
            exit_code: 0,
            failures: vec![],
            firewall_rules_added: 2,
            firewall_rules_remaining: 0,
            children_stopped_before_lifting_denial: 0,
            firewall_rules: vec!["one".into(), "two".into()],
            captured_processes: vec![
                NativeProcess {
                    pid: 123,
                    started: true,
                    exit_code: 0,
                    completed: true,
                    elapsed_ms: 10,
                    error: None
                };
                STEPS.len()
            ],
            product_commit: header.product.commit.clone(),
            cargo_lock_sha256: header.product.lock_sha256.clone(),
        };
        validate_terminal(&header, &controls, &terminal).unwrap();
        for field in [
            "exit",
            "failure",
            "remaining",
            "children",
            "commit",
            "lock",
            "rules",
            "processes",
        ] {
            let mut changed = terminal.clone();
            match field {
                "exit" => changed.exit_code = 1,
                "failure" => changed.failures.push("late cleanup failure".into()),
                "remaining" => changed.firewall_rules_remaining = 1,
                "children" => changed.children_stopped_before_lifting_denial = 1,
                "commit" => changed.product_commit = "f".repeat(40),
                "lock" => changed.cargo_lock_sha256 = "f".repeat(64),
                "rules" => {
                    changed.firewall_rules.pop();
                }
                "processes" => {
                    changed.captured_processes.pop();
                }
                _ => unreachable!(),
            }
            assert!(
                validate_terminal(&header, &controls, &changed).is_err(),
                "{field}"
            );
        }
        let mut failed_probe = controls.clone();
        failed_probe.network.ipv4.positive_control = "unreachable".into();
        assert!(validate_terminal(&header, &failed_probe, &terminal).is_err());
        let mut foreign_schema = controls;
        foreign_schema.schema = common::NVATTEST_DRIVER_CONTROLS_SCHEMA_V1.into();
        assert!(validate_terminal(&header, &foreign_schema, &terminal).is_err());
    }
}
