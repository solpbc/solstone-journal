// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Replay the original, bounded native capture. Named-program denial is not
//! process-tree isolation; the receipt preserves that distinction.

use crate::controlled_build::InputIdentityEntry;
use crate::digest::sha256_hex;
use crate::llama_windows_config as config;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

type Result<T> = std::result::Result<T, String>;
const SHADER_DIR: &str =
    "engine-build/ggml/src/ggml-vulkan/vulkan-shaders-gen-prefix/src/vulkan-shaders-gen-build";
const DRIVER_SHA: &str = "70c122cc962b3dd5191150dfabe3d0d5ad1b65f843e4ca5bb696bc8ca6205ee7";
const HELPER_SHA: &str = "a4959a5aca1b346a915e87c4019e64169a153f27af2a724012eb01dca13fe605";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inputs {
    product_commit: String,
    cargo_lock_sha256: String,
    driver_sha256: String,
    capture_helper_sha256: String,
    source_sha256: String,
    sdk_sha256: String,
    cmake_sha256: String,
    tools: Vec<Tool>,
    kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tool {
    path: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Execution {
    exit_code: i32,
    pending_reconciliation: bool,
    elapsed_ms: u64,
    fence_retained: bool,
    owner_token: String,
    firewall_rules_retained: Vec<String>,
    failures: Vec<String>,
    process_ownership: String,
    whole_tree_quiescence_proven: bool,
    incomplete_logs_are_original_prefixes: bool,
    kind: String,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Step {
    label: String,
    argv: Vec<String>,
    cwd: String,
    exit_code: i32,
    stdout_path: String,
    stderr_path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepExecution {
    label: String,
    argv: Vec<String>,
    cwd: String,
    launch_attempted: bool,
    started: bool,
    pid: u32,
    exit_code: i32,
    completed: bool,
    elapsed_ms: u64,
    wait_budget_ms: u64,
    invocation_elapsed_ms: u64,
    error: Option<String>,
    stdout_path: String,
    stderr_path: String,
}

fn insist(condition: bool, what: &str) -> Result<()> {
    if condition { Ok(()) } else { Err(what.into()) }
}
fn hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn norm(s: &str) -> String {
    s.replace('\\', "/")
}
fn json<T: serde::de::DeserializeOwned>(bytes: &[u8], label: &str) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| format!("{label}: {e}"))
}
fn text(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| "capture text is not UTF-8".into())
}

/// Read a retained tree once, with regular-file, membership and byte budgets.
/// Callers derive all parsed values and digests from these same bytes.
pub(crate) fn read_capture(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    let mut total = 0_u64;
    let mut entries = 0;
    while let Some((path, label)) = pending.pop() {
        entries += 1;
        insist(entries <= 2000, "capture entry budget exceeded")?;
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.is_dir() {
            for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "non-UTF-8 capture path")?;
                insist(
                    !name.is_empty()
                        && !name.contains(['\\', ':'])
                        && !name.chars().any(char::is_control)
                        && !matches!(name.as_str(), "." | ".."),
                    "invalid capture member",
                )?;
                insist(
                    entries + pending.len() < 2000,
                    "capture entry budget exceeded",
                )?;
                pending.push((
                    entry.path(),
                    if label.is_empty() {
                        name
                    } else {
                        format!("{label}/{name}")
                    },
                ));
            }
        } else {
            insist(
                meta.file_type().is_file(),
                "capture link or special file refused",
            )?;
            let limit = if label.starts_with("output/") {
                128 * 1024 * 1024
            } else {
                64 * 1024 * 1024
            };
            insist(meta.len() <= limit, "capture member size exceeded")?;
            let mut bytes = Vec::new();
            fs::File::open(&path)
                .map_err(|e| e.to_string())?
                .take(limit + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            insist(
                bytes.len() as u64 <= limit,
                "capture member grew beyond limit",
            )?;
            total += bytes.len() as u64;
            insist(total <= 256 * 1024 * 1024, "capture total size exceeded")?;
            insist(
                files.insert(label, bytes).is_none(),
                "duplicate capture member",
            )?;
        }
    }
    Ok(files)
}
fn member<'a>(files: &'a BTreeMap<String, Vec<u8>>, label: &str) -> Result<&'a [u8]> {
    files
        .get(label)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("capture member missing: {label}"))
}

#[derive(Serialize)]
pub(crate) struct CaptureEvidence {
    pub product: crate::provenance::Provenance,
    pub files: Vec<InputIdentityEntry>,
    pub named_program_network_denial: bool,
    pub whole_tree_quiescence_proven: bool,
}

pub(crate) fn inspect(
    files: &BTreeMap<String, Vec<u8>>,
    source_members: &BTreeMap<String, (u64, String)>,
    source: &InputIdentityEntry,
    sdk: &InputIdentityEntry,
    cmake: &InputIdentityEntry,
) -> Result<CaptureEvidence> {
    let inputs: Inputs = json(member(files, "report/inputs.json")?, "inputs")?;
    insist(
        inputs.product_commit == "2b5b0dfddba46ca66bd75d41e1a3c4fc1df9dcbc"
            && inputs.cargo_lock_sha256
                == "1ffd8dfd4f7debd9189561e2b6bb19184affa3349b5eb14040bdf38bf0594423",
        "invalid product provenance",
    )?;
    insist(
        inputs.driver_sha256 == DRIVER_SHA && inputs.capture_helper_sha256 == HELPER_SHA,
        "unreviewed capture driver/helper",
    )?;
    insist(
        inputs.source_sha256 == source.sha256
            && inputs.sdk_sha256 == sdk.sha256
            && inputs.cmake_sha256 == cmake.sha256,
        "capture input identities disagree with verified archives",
    )?;
    insist(
        inputs.kind == "native capture; receipt admission pending",
        "unknown capture kind",
    )?;
    insist(
        inputs.tools.len() == 5 && inputs.tools.iter().all(|t| hex(&t.sha256, 64)),
        "missing or malformed tool identities",
    )?;
    let exe: Execution = json(member(files, "report/execution.json")?, "execution")?;
    insist(
        exe.exit_code == 0
            && !exe.pending_reconciliation
            && !exe.fence_retained
            && exe.firewall_rules_retained.is_empty()
            && exe.failures.is_empty()
            && !exe.incomplete_logs_are_original_prefixes
            && exe.elapsed_ms > 0
            && exe.elapsed_ms <= 14_400_000,
        "native capture incomplete, failed or retained authority",
    )?;
    insist(
        exe.process_ownership == "Unowned"
            && !exe.whole_tree_quiescence_proven
            && exe.kind == "native capture; not an admitted receipt"
            && hex(&exe.owner_token, 32),
        "invalid ownership/capture claim",
    )?;
    let expected_members: BTreeSet<String> =
        include_str!("../../../distribution/llama-windows-capture-members.txt")
            .lines()
            .map(|p| p.replace("{owner_token}", &exe.owner_token))
            .collect();
    insist(
        files.keys().cloned().collect::<BTreeSet<_>>() == expected_members,
        "capture file membership differs from the retained recipe: missing or unexpected supporting artifact",
    )?;
    insist(
        text(member(files, "report/exit.txt")?)?.trim() == "0",
        "terminal exit disagrees",
    )?;
    for label in ["host-before-fence", "host-after-fence", "host-terminal"] {
        let census: Vec<Value> = json(member(files, &format!("report/{label}.json"))?, label)?;
        insist(census.is_empty(), "native tool census was not empty")?;
    }
    insist(
        member(files, "report/sdk-state-before.json")?
            == member(files, "report/sdk-state-after.json")?,
        "monitored SDK environment or system loader changed",
    )?;
    validate_source_census(files, source_members)?;
    let steps: Vec<Step> = json(
        member(files, "report/subprocess-evidence.json")?,
        "subprocesses",
    )?;
    let labels = [
        "host-fence-acquire",
        "product-head-before",
        "product-status-before",
        "extract-source",
        "check-shader-patch",
        "apply-shader-patch",
        "network-positive",
        "sdk-copy-only",
        "vswhere",
        "build-environment",
        "network-negative-before",
        "engine-configure",
        "engine-build",
        "loader-configure",
        "loader-build",
        "network-negative-after",
        "product-head-after",
        "product-status-after",
        "engine-imports",
        "loader-imports",
    ];
    insist(
        steps.iter().map(|s| s.label.as_str()).eq(labels),
        "subprocess sequence missing, duplicated or reordered",
    )?;
    let run_root = &steps[0].cwd;
    insist(
        run_root.len() > 3
            && run_root.as_bytes()[1] == b':'
            && !run_root.contains(['\n', '\r', '"']),
        "invalid native run root",
    )?;
    let mut elapsed = 0;
    for step in &steps {
        insist(
            step.exit_code == 0
                && !step.argv.is_empty()
                && step.stdout_path == format!("logs/{}.stdout", step.label)
                && step.stderr_path == format!("logs/{}.stderr", step.label),
            "failed subprocess or unbound stream path",
        )?;
        member(files, &format!("report/{}", step.stdout_path))?;
        member(files, &format!("report/{}", step.stderr_path))?;
        let d: StepExecution = json(
            member(files, &format!("report/logs/{}.execution.json", step.label))?,
            &step.label,
        )?;
        insist(
            d.label == step.label
                && d.argv == step.argv
                && d.cwd == step.cwd
                && d.exit_code == step.exit_code
                && d.stdout_path == step.stdout_path
                && d.stderr_path == step.stderr_path,
            "step record disagrees with original capture",
        )?;
        insist(
            d.launch_attempted
                && d.started
                && d.completed
                && d.pid > 0
                && d.error.is_none()
                && d.wait_budget_ms > 0
                && d.wait_budget_ms <= 14_400_000
                && d.elapsed_ms <= d.wait_budget_ms + 1000
                && d.invocation_elapsed_ms >= elapsed
                && d.invocation_elapsed_ms <= exe.elapsed_ms,
            "incomplete or inconsistent subprocess timing",
        )?;
        elapsed = d.invocation_elapsed_ms;
    }
    for phase in ["before", "after"] {
        insist(
            text(member(
                files,
                &format!("report/logs/product-head-{phase}.stdout"),
            )?)?
            .trim()
                == inputs.product_commit,
            "original product HEAD disagrees",
        )?;
        insist(
            member(files, &format!("report/logs/product-status-{phase}.stdout"))?.is_empty(),
            "original product checkout dirty",
        )?;
    }
    let cmake_exe = format!("{run_root}/cmake/cmake-3.31.12-windows-x86_64/bin/cmake.exe");
    insist(
        norm(&inputs.tools[0].path) == norm(&cmake_exe)
            && norm(&inputs.tools[4].path) == format!("{}/sdk/Bin/glslc.exe", norm(run_root)),
        "tools did not use captured CMake/SDK",
    )?;
    for (index, name) in [(1, "cl.exe"), (2, "link.exe"), (3, "ml64.exe")] {
        insist(
            norm(&inputs.tools[index].path).ends_with(&format!(
                "/VC/Tools/MSVC/14.44.35207/bin/HostX64/x64/{name}"
            )),
            "unreviewed MSVC tool identity",
        )?;
    }
    for (tool, expected) in inputs.tools.iter().zip([
        "f3a124ad60459b56b2a5b4e312b0cd5de56d5b1fcf2921f8f602fb531b0da10d",
        "88c8344236a27a6e727e0a8edc49aaa2690bdc7a9464b9d18cc7abe70a9f1c0d",
        "ca11e6c45debd34bf652dfe984c5360a531a005ed78bf72852330c9c2590cf0d",
        "a776cf32777d2df2e201a769bb5bd6502bb72bac98f60c6f53a4bfcecd0db8ed",
        "f8c97ee2c8bfcd31da87b602622c6e742389f98a83693b504cf538de4c75d3fa",
    ]) {
        insist(
            tool.sha256 == expected,
            "captured build tool bytes differ from the reviewed toolchain",
        )?;
    }
    let compiler = norm(&inputs.tools[1].path);
    let vs_root = compiler
        .strip_suffix("/VC/Tools/MSVC/14.44.35207/bin/HostX64/x64/cl.exe")
        .ok_or("unreviewed compiler root")?;
    for (build, source) in [
        ("engine-build", "source/llama"),
        ("loader-build", "source/loader-wrapper"),
        (
            SHADER_DIR,
            "source/llama/ggml/src/ggml-vulkan/vulkan-shaders",
        ),
    ] {
        config::validate_cache_binding(
            member(files, &format!("{build}/CMakeCache.txt"))?,
            run_root,
            source,
            build,
            vs_root,
        )?;
    }
    for build in ["engine-build", "loader-build", SHADER_DIR] {
        let values = config::cache(member(files, &format!("{build}/CMakeCache.txt"))?)?;
        // Visual Studio caches bind C/C++ through the generator instance and
        // toolset; they do not contain CMAKE_C_COMPILER/CMAKE_CXX_COMPILER.
        let mut bindings = vec![("CMAKE_LINKER", &inputs.tools[2].path)];
        if build == "engine-build" {
            bindings.push(("CMAKE_ASM_COMPILER", &inputs.tools[1].path));
        } else if build == "loader-build" {
            bindings.push(("CMAKE_ASM_MASM_COMPILER", &inputs.tools[3].path));
        }
        for (key, expected) in bindings {
            insist(
                values.get(key).map(|p| norm(p).to_ascii_lowercase())
                    == Some(norm(expected).to_ascii_lowercase()),
                "CMake tool differs from denied tool",
            )?;
        }
    }
    // Admission of this reviewed completed capture is deliberately closed over
    // all generated project bytes, including imports, targets and commands.
    // Semantic checks explain the bounds; they are not an MSBuild interpreter.
    // A fresh native run whose generated projects differ requires a new review
    // and an explicit update of this manifest before receipt admission.
    let reviewed_projects: BTreeMap<String, String> = serde_json::from_str(include_str!(
        "../../../distribution/llama-windows-reviewed-projects.json"
    ))
    .map_err(|e| e.to_string())?;
    let actual_projects: BTreeMap<String, String> = files
        .iter()
        .filter(|(name, _)| name.ends_with(".vcxproj"))
        .map(|(name, bytes)| (name.clone(), sha256_hex(bytes)))
        .collect();
    insist(
        actual_projects == reviewed_projects,
        "generated projects differ from the reviewed native capture",
    )?;
    config::validate_custom_serialization(member(
        files,
        "engine-build/ggml/src/ggml-vulkan/ggml-vulkan.vcxproj",
    )?)?;
    validate_commands(&steps, run_root, &cmake_exe, files)?;
    validate_denial(files, &exe.owner_token, &inputs, &steps, run_root)?;
    config::validate_engine_cache(member(files, "engine-build/CMakeCache.txt")?, run_root)?;
    config::validate_loader_cache(member(files, "loader-build/CMakeCache.txt")?, run_root)?;
    config::validate_shader_cache(member(files, &format!("{SHADER_DIR}/CMakeCache.txt"))?)?;
    let sdk_options = format!(
        "%(AdditionalOptions) /external:I \"{}/sdk/Include\"",
        norm(run_root)
    );
    for (label, options, bounded) in [
        (
            "loader-build/upstream/loader/vulkan.vcxproj".to_owned(),
            sdk_options.clone(),
            false,
        ),
        (
            "loader-build/upstream/loader/asm_offset.vcxproj".to_owned(),
            sdk_options.clone(),
            false,
        ),
        (
            "engine-build/ggml/src/ggml-vulkan/ggml-vulkan.vcxproj".to_owned(),
            format!("{sdk_options} /utf-8 /bigobj"),
            true,
        ),
        (
            format!("{SHADER_DIR}/vulkan-shaders-gen.vcxproj"),
            String::new(),
            false,
        ),
        (
            "engine-build/tools/server/llama-server.vcxproj".to_owned(),
            "%(AdditionalOptions) /utf-8 /bigobj".into(),
            false,
        ),
    ] {
        config::validate_compiler(member(files, &label)?, &options, bounded)
            .map_err(|e| format!("{label}: {e}"))?;
    }
    for label in [
        "engine-build/common/llama-common-base.vcxproj",
        "engine-build/common/llama-common.vcxproj",
        "engine-build/src/llama.vcxproj",
        "engine-build/tools/server/llama-server-impl.vcxproj",
        "engine-build/tools/server/server-context.vcxproj",
        "engine-build/tools/mtmd/mtmd.vcxproj",
        "engine-build/ggml/src/ggml.vcxproj",
        "engine-build/ggml/src/ggml-base.vcxproj",
    ] {
        config::validate_compiler(
            member(files, label)?,
            "%(AdditionalOptions) /utf-8 /bigobj",
            false,
        )
        .map_err(|e| format!("{label}: {e}"))?;
    }
    config::validate_compiler(
        member(files, "engine-build/vendor/cpp-httplib/cpp-httplib.vcxproj")?,
        "%(AdditionalOptions) /utf-8 /bigobj /w",
        false,
    )?;
    let cpu = config::release_compiler(member(files, "engine-build/ggml/src/ggml-cpu.vcxproj")?)?;
    insist(
        cpu.get("EnableEnhancedInstructionSet").map(String::as_str)
            == Some("AdvancedVectorExtensions2")
            && cpu.get("AdditionalOptions").map(String::as_str)
                == Some("%(AdditionalOptions) /utf-8 /bigobj")
            && !cpu.contains_key("MultiProcessorCompilation")
            && !cpu.contains_key("ProcessorNumber"),
        "CPU ancillary compilation diverges from the fixed AVX2 recipe",
    )?;
    config::validate_shader_commands(
        member(
            files,
            "engine-build/ggml/src/ggml-vulkan/vulkan-shaders-gen.vcxproj",
        )?,
        &cmake_exe,
        &format!("{run_root}/{SHADER_DIR}"),
    )?;
    validate_outputs(files)?;
    Ok(CaptureEvidence {
        product: crate::provenance::Provenance {
            commit: inputs.product_commit,
            lock_sha256: inputs.cargo_lock_sha256,
        },
        files: files
            .iter()
            .map(|(label, bytes)| InputIdentityEntry {
                label: label.clone(),
                sha256: sha256_hex(bytes),
                size: bytes.len() as u64,
            })
            .collect(),
        named_program_network_denial: true,
        whole_tree_quiescence_proven: false,
    })
}

fn validate_source_census(
    files: &BTreeMap<String, Vec<u8>>,
    source_members: &BTreeMap<String, (u64, String)>,
) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Row {
        path: String,
        size: u64,
        sha256: String,
    }
    let mut expected = source_members.clone();
    for phase in ["original", "patched", "after"] {
        if phase == "patched" {
            for (path, size, hash) in [
                (
                    "llama/ggml/src/ggml-vulkan/CMakeLists.txt",
                    11150,
                    "3fa1145ac58b6d6f20b067054d8539b978c447a90f68855dd1f9fd226511291e",
                ),
                (
                    "llama/ggml/src/ggml-vulkan/vulkan-shaders/vulkan-shaders-gen.cpp",
                    82518,
                    "6e53453098b2bb7ff01f3903382cf7e7742cec3b802e6ae06ec42ef516690fef",
                ),
            ] {
                insist(
                    expected.insert(path.into(), (size, hash.into())).is_some(),
                    "patch target missing",
                )?;
            }
        }
        let rows: Vec<Row> = json(
            member(files, &format!("report/source-{phase}.json"))?,
            phase,
        )?;
        let mut found = BTreeMap::new();
        for row in rows {
            insist(
                found.insert(row.path, (row.size, row.sha256)).is_none(),
                "duplicate source census member",
            )?;
        }
        insist(
            found == expected,
            &format!("{phase} source census disagrees with pinned archive and fixed patch"),
        )?;
    }
    Ok(())
}

fn validate_commands(
    steps: &[Step],
    root: &str,
    cmake: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for (label, dir, target) in [
        ("engine-build", "engine-build", "llama-server"),
        ("loader-build", "loader-build", "vulkan"),
    ] {
        let step = steps
            .iter()
            .find(|s| s.label == label)
            .ok_or("missing build step")?;
        let expected = [
            norm(cmake),
            "--build".into(),
            format!("{}/{dir}", norm(root)),
            "--config".into(),
            "Release".into(),
            "--target".into(),
            target.into(),
            "--parallel".into(),
            "1".into(),
        ];
        insist(
            step.argv.iter().map(|s| norm(s)).eq(expected)
                && norm(&step.cwd) == format!("{}/source", norm(root)),
            "build command is not the serial Release target",
        )?;
    }
    for (label, build, source) in [
        ("engine-configure", "engine-build", "llama"),
        ("loader-configure", "loader-build", "loader-wrapper"),
    ] {
        let step = steps
            .iter()
            .find(|s| s.label == label)
            .ok_or("missing configure")?;
        let values = config::cache(member(files, &format!("{build}/CMakeCache.txt"))?)?;
        let expected_prefix = [
            norm(cmake),
            "-S".into(),
            format!("{}/source/{source}", norm(root)),
            "-B".into(),
            format!("{}/{build}", norm(root)),
            "-G".into(),
            "Visual Studio 17 2022".into(),
            "-A".into(),
            "x64".into(),
            "-T".into(),
            "host=x64,version=14.44.35207".into(),
        ];
        insist(
            step.argv.len() > expected_prefix.len()
                && step.argv[..expected_prefix.len()]
                    .iter()
                    .map(|s| norm(s))
                    .eq(expected_prefix),
            "configure generator/source/build binding disagrees",
        )?;
        let mut names = BTreeSet::new();
        for flag in &step.argv[11..] {
            let (key, value) = flag
                .strip_prefix("-D")
                .and_then(|s| s.split_once('='))
                .ok_or("unexpected configure argument")?;
            insist(names.insert(key), "duplicate configure flag")?;
            insist(
                values.get(key).map(|s| norm(s)) == Some(norm(value)),
                &format!("configure/cache mismatch: {key}"),
            )?;
        }
        let allowed: BTreeSet<&str> = if label == "engine-configure" {
            [
                "CMAKE_GENERATOR_INSTANCE",
                "CMAKE_MSVC_RUNTIME_LIBRARY",
                "BUILD_SHARED_LIBS",
                "GGML_BACKEND_DL",
                "GGML_CPU_ALL_VARIANTS",
                "GGML_CPU",
                "GGML_VULKAN",
                "GGML_NATIVE",
                "GGML_CCACHE",
                "GGML_LLAMAFILE",
                "GGML_CUDA",
                "GGML_HIP",
                "GGML_METAL",
                "GGML_OPENCL",
                "GGML_SYCL",
                "GGML_RPC",
                "GGML_BLAS",
                "GGML_AVX",
                "GGML_AVX2",
                "GGML_AVX512",
                "GGML_FMA",
                "GGML_F16C",
                "GGML_BMI2",
                "LLAMA_BUILD_COMMON",
                "LLAMA_BUILD_TOOLS",
                "LLAMA_BUILD_SERVER",
                "LLAMA_BUILD_APP",
                "LLAMA_BUILD_TESTS",
                "LLAMA_BUILD_EXAMPLES",
                "LLAMA_BUILD_UI",
                "LLAMA_USE_PREBUILT_UI",
                "LLAMA_OPENSSL",
                "LLAMA_BUILD_BORINGSSL",
                "LLAMA_BUILD_LIBRESSL",
                "LLAMA_LLGUIDANCE",
                "Vulkan_GLSLC_EXECUTABLE",
                "Vulkan_INCLUDE_DIR",
                "Vulkan_LIBRARY",
                "SPIRV-Headers_DIR",
            ]
            .into_iter()
            .collect()
        } else {
            [
                "CMAKE_GENERATOR_INSTANCE",
                "VulkanHeaders_DIR",
                "UPDATE_DEPS",
                "BUILD_TESTS",
                "LOADER_CODEGEN",
                "CODE_COVERAGE",
                "USE_MASM",
                "USE_GAS",
                "ENABLE_WIN10_ONECORE",
                "LOADER_USE_UNSAFE_FILE_SEARCH",
            ]
            .into_iter()
            .collect()
        };
        insist(
            names == allowed,
            "configure flag set differs from the reviewed recipe",
        )?;
        insist(
            norm(&step.cwd) == format!("{}/source", norm(root)),
            "configure cwd disagrees",
        )?;
    }
    let sdk = &steps[7];
    insist(
        sdk.argv.len() == 8
            && sdk.argv[1..]
                == [
                    "--root",
                    &format!("{root}\\sdk"),
                    "--accept-licenses",
                    "--default-answer",
                    "--confirm-command",
                    "install",
                    "copy_only=1",
                ],
        "SDK extraction must remain copy-only",
    )?;
    for (label, denied) in [
        ("network-positive", false),
        ("network-negative-before", true),
        ("network-negative-after", true),
    ] {
        let step = steps
            .iter()
            .find(|s| s.label == label)
            .ok_or("missing network control")?;
        insist(
            step.argv.len() == 5
                && norm(&step.argv[0]) == norm(cmake)
                && step.argv[1] == format!("-DEXPECT_DENIED={}", if denied { "ON" } else { "OFF" })
                && norm(&step.argv[2]) == format!("-DOUTPUT={}/{label}.txt", norm(root))
                && step.argv[3] == "-P"
                && norm(&step.argv[4]) == format!("{}/network-control.cmake", norm(root)),
            "network control command disagrees",
        )?;
        let observed = text(member(files, &format!("report/logs/{label}.stdout"))?)?.trim();
        let code = observed
            .strip_prefix("-- transfer-status=")
            .and_then(|s| s.split_once(';'))
            .and_then(|(s, _)| s.parse::<i32>().ok())
            .ok_or("missing transfer observation")?;
        insist(
            (code != 0) == denied,
            "transfer control does not establish expected denial",
        )?;
    }
    Ok(())
}

fn validate_denial(
    files: &BTreeMap<String, Vec<u8>>,
    token: &str,
    inputs: &Inputs,
    steps: &[Step],
    root: &str,
) -> Result<()> {
    #[derive(Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Rule {
        name: String,
        program: String,
        enabled: String,
        direction: String,
        action: String,
        profile: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Profile {
        name: String,
        enabled: i32,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Denial {
        utc: String,
        profiles: Vec<Profile>,
        rules: Vec<Rule>,
    }
    let tool_bin = norm(&inputs.tools[1].path)
        .strip_suffix("/cl.exe")
        .ok_or("invalid compiler path")?
        .to_owned();
    let vs = tool_bin
        .split("/VC/Tools/")
        .next()
        .ok_or("invalid VS path")?;
    let programs = vec![
        norm(&inputs.tools[0].path),
        norm(&steps[7].argv[0]),
        norm(&inputs.tools[1].path),
        norm(&inputs.tools[2].path),
        norm(&inputs.tools[3].path),
        norm(&steps[1].argv[0]),
        format!("{vs}/MSBuild/Current/Bin/MSBuild.exe"),
        format!("{vs}/MSBuild/Current/Bin/amd64/MSBuild.exe"),
        norm(&inputs.tools[4].path),
        "C:/Program Files/Git/mingw64/bin/git.exe".into(),
        format!("{}/engine-build/Release/vulkan-shaders-gen.exe", norm(root)),
        format!(
            "{}/loader-build/upstream/loader/Release/asm_offset.exe",
            norm(root)
        ),
    ];
    let mut previous = None;
    for (phase, count) in [("sdk-before", 2), ("before", 12), ("after", 12)] {
        let denial: Denial = json(
            member(files, &format!("report/denial-{phase}.json"))?,
            phase,
        )?;
        insist(
            !denial.utc.is_empty()
                && denial.profiles.len() == 3
                && denial
                    .profiles
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<BTreeSet<_>>()
                    == BTreeSet::from(["Domain", "Private", "Public"])
                && denial.profiles.iter().all(|p| p.enabled == 1),
            "firewall profiles not observed enabled",
        )?;
        insist(denial.rules.len() == count, "incomplete program denial set")?;
        for (index, rule) in denial.rules.iter().enumerate() {
            insist(
                rule.name == format!("solstone-llama-build-{token}-{index}")
                    && norm(&rule.program) == programs[index]
                    && rule.enabled == "True"
                    && rule.direction == "Outbound"
                    && rule.action == "Block"
                    && rule.profile == "Any",
                "effective deny rule disagrees with owned program intent",
            )?;
            let intent: Value = json(
                member(files, &format!("report/{}.json", rule.name))?,
                "deny intent",
            )?;
            insist(
                intent["name"] == rule.name && intent["program"] == rule.program,
                "denial intent disagrees",
            )?;
        }
        if phase == "after" {
            insist(
                previous.as_ref() == Some(&denial.rules),
                "denial rules changed during build",
            )?;
        }
        previous = Some(denial.rules);
    }
    Ok(())
}

fn validate_outputs(files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Output {
        path: String,
        bytes: u64,
        sha256: String,
    }
    let outputs: Vec<Output> = json(member(files, "report/outputs.json")?, "outputs")?;
    let expected = BTreeSet::from(["bin/llama-server.exe", "bin/vulkan-1.dll"]);
    insist(
        outputs.len() == 2
            && outputs
                .iter()
                .map(|o| o.path.as_str())
                .collect::<BTreeSet<_>>()
                == expected
            && files
                .keys()
                .filter_map(|s| s.strip_prefix("output/"))
                .collect::<BTreeSet<_>>()
                == expected,
        "output census must contain exactly the engine and loader",
    )?;
    for output in outputs {
        let bytes = member(files, &format!("output/{}", output.path))?;
        insist(
            bytes.len() as u64 == output.bytes && sha256_hex(bytes) == output.sha256,
            "output differs from original pre-sign capture",
        )?;
        let info = crate::pe_dependencies::inspect_dependencies(bytes)?;
        insist(
            info.is_dll == (output.path == "bin/vulkan-1.dll"),
            "output executable/DLL kind mismatch",
        )?;
        let mut names = BTreeSet::new();
        for name in info
            .imports
            .iter()
            .chain(&info.delay_imports)
            .chain(&info.forwarders)
        {
            let name = crate::pe_dependencies::dll_name(name)?;
            insist(
                allowed_dependency(&name, info.is_dll),
                &format!("unaccounted engine/loader import: {name}"),
            )?;
            names.insert(name);
        }
        insist(
            names.contains("vcruntime140.dll"),
            "output lacks dynamic Microsoft runtime import",
        )?;
        if !info.is_dll {
            insist(
                names.contains("vulkan-1.dll"),
                "engine does not import its app-local Vulkan loader",
            )?;
        }
    }
    Ok(())
}

// This is the pre-sign dependency boundary, not package closure. The producer
// must still supply and verify each Microsoft redistributable alongside the
// loader. No vendor ICD, Vulkan SDK or debug runtime is admitted here.
fn allowed_dependency(name: &str, loader: bool) -> bool {
    const COMMON: &[&str] = &[
        "kernel32.dll",
        "advapi32.dll",
        "vcruntime140.dll",
        "api-ms-win-crt-runtime-l1-1-0.dll",
        "api-ms-win-crt-heap-l1-1-0.dll",
        "api-ms-win-crt-convert-l1-1-0.dll",
        "api-ms-win-crt-stdio-l1-1-0.dll",
        "api-ms-win-crt-string-l1-1-0.dll",
        "api-ms-win-crt-filesystem-l1-1-0.dll",
    ];
    COMMON.contains(&name)
        || if loader {
            // Windows Configuration Manager. The captured loader imports the
            // CM_Get/Locate/Open device-node APIs.
            // https://learn.microsoft.com/en-us/windows/win32/api/cfgmgr32/
            name == "cfgmgr32.dll"
        } else {
            [
                "vulkan-1.dll",
                "shell32.dll",
                "ws2_32.dll",
                "msvcp140.dll",
                "vcruntime140_1.dll",
                "vcomp140.dll",
                "api-ms-win-crt-locale-l1-1-0.dll",
                "api-ms-win-crt-math-l1-1-0.dll",
                "api-ms-win-crt-environment-l1-1-0.dll",
                "api-ms-win-crt-time-l1-1-0.dll",
                "api-ms-win-crt-utility-l1-1-0.dll",
            ]
            .contains(&name)
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dependency_admission_keeps_packaged_loader_and_vendor_drivers_distinct() {
        assert!(allowed_dependency("vulkan-1.dll", false));
        assert!(!allowed_dependency("vulkan-1.dll", true));
        assert!(allowed_dependency("cfgmgr32.dll", true));
        for name in [
            "ggml-cuda.dll",
            "ggml-vulkan.dll",
            "amdvlk64.dll",
            "nvoglv64.dll",
            "vcruntime140d.dll",
            "msvcp140d.dll",
            "api-ms-win-unknown.dll",
        ] {
            assert!(!allowed_dependency(name, false));
            assert!(!allowed_dependency(name, true));
        }
    }
}

pub(crate) fn receipt(
    evidence: &CaptureEvidence,
    files: &BTreeMap<String, Vec<u8>>,
    inputs: Vec<InputIdentityEntry>,
    builder_host: &str,
) -> Result<(crate::controlled_build::ControlledBuildReceipt, Vec<u8>)> {
    use crate::controlled_build::*;
    insist(
        !builder_host.is_empty()
            && builder_host.len() <= 255
            && builder_host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
        "invalid builder host",
    )?;
    let evidence_bytes = serde_json::to_vec_pretty(evidence).map_err(|e| e.to_string())?;
    let outputs = census_outputs(&[
        (
            "bin/llama-server.exe",
            member(files, "output/bin/llama-server.exe")?,
        ),
        (
            "bin/vulkan-1.dll",
            member(files, "output/bin/vulkan-1.dll")?,
        ),
    ])
    .map_err(|e| e.to_string())?;
    let source = inputs
        .iter()
        .find(|i| i.label == "sources/llama-windows.tar.gz")
        .ok_or("missing verified source identity")?;
    let draft = ControlledBuildReceiptDraft {
        schema: Some(CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into()),
        source: Some(SourceIdentity {
            product: evidence.product.clone(),
            windows_dependency: DependencySource {
                repository: "https://github.com/ggml-org/llama.cpp.git".into(),
                revision: crate::llama_windows_source::LLAMA_COMMIT.into(),
                content_sha256: source.sha256.clone(),
            },
        }),
        inputs: Some(inputs),
        builder: Some(BuilderIdentity { host: builder_host.into(), toolchain: "MSVC 14.44.35207 x64; CMake 3.31.12; Vulkan SDK 1.4.357.0".into() }),
        configuration: Some(BuildConfiguration {
            target_triple: "x86_64-pc-windows-msvc".into(), profile: "Release".into(),
            flags: vec!["Vulkan; static ggml; app-local Vulkan-Loader".into(), "outer/nested --parallel 1; Vulkan /MP1; shader helper slots=2; CMP0147 OLD".into(), "engine/loader/shader-helper Microsoft DLL runtime".into(), "named-program outbound denial observed; no whole-tree isolation claim".into()],
            network_access_denied: evidence.named_program_network_denial,
        }),
        outputs: Some(outputs),
        supporting: Some(vec![SupportingArtifactRef { label: "llama-windows-build-evidence.json".into(), sha256: sha256_hex(&evidence_bytes) }]),
        validation: Some(ValidationReference { description: "Original native capture terminal record; all supporting bytes bound by the replayed evidence".into(), sha256: sha256_hex(member(files, "report/execution.json")?) }),
    };
    Ok((draft.validate().map_err(|e| e.to_string())?, evidence_bytes))
}

pub(crate) fn publish(
    receipt: &crate::controlled_build::ControlledBuildReceipt,
    evidence: &[u8],
    receipt_path: &Path,
    evidence_path: &Path,
) -> Result<String> {
    use crate::controlled_build::{
        ControlledBuildReceiptPublication, write_controlled_build_receipt_exclusive,
    };
    use solstone_core_journal_io::{
        AtomicWriteOptions, FinalNameConfirmation, MetadataDurability, StageCleanup,
        write_bytes_exclusive_detailed,
    };
    insist(
        receipt_path != evidence_path,
        "receipt and evidence paths collide",
    )?;
    for path in [receipt_path, evidence_path] {
        insist(
            !path.try_exists().map_err(|e| e.to_string())?,
            "refusing to overwrite an existing admission record",
        )?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let e = write_bytes_exclusive_detailed(evidence_path, evidence, AtomicWriteOptions::default())
        .map_err(|e| e.to_string())?;
    let evidence_durable = e.is_fully_confirmed();
    if !evidence_durable
        && !(matches!(e.final_name, FinalNameConfirmation::Confirmed { .. })
            && matches!(e.cleanup, StageCleanup::Removed)
            && matches!(e.durability, MetadataDurability::Unproven { .. }))
    {
        return Err(format!(
            "evidence publication unconfirmed; preserve destination: {e:?}"
        ));
    }
    let publication = write_controlled_build_receipt_exclusive(receipt_path, receipt)
        .map_err(|e| e.to_string())?;
    match publication {
        ControlledBuildReceiptPublication::Durable {..} if evidence_durable => Ok("recorded: durable; original pre-sign bytes; signing requires package admission".into()),
        ControlledBuildReceiptPublication::Durable {..} | ControlledBuildReceiptPublication::PublishedButNotDurable {..} => Ok("recorded: published-but-not-durable; metadata durability unproven; signing requires package admission".into()),
        ControlledBuildReceiptPublication::PublicationUnconfirmed {publication, ..} => Err(format!("receipt publication unconfirmed; preserve destination: {publication:?}")),
    }
}

pub(crate) fn verify_record(
    expected: &crate::controlled_build::ControlledBuildReceipt,
    evidence: &[u8],
    receipt_path: &Path,
    evidence_path: &Path,
) -> Result<()> {
    fn read(path: &Path) -> Result<Vec<u8>> {
        let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        insist(
            m.file_type().is_file() && m.len() <= 8 * 1024 * 1024,
            "admission record is not a bounded regular file",
        )?;
        let mut b = Vec::new();
        fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut b)
            .map_err(|e| e.to_string())?;
        insist(
            b.len() <= 8 * 1024 * 1024,
            "admission record grew beyond bound",
        )?;
        Ok(b)
    }
    let persisted = crate::controlled_build::decode_controlled_build_receipt(&read(receipt_path)?)
        .map_err(|e| e.to_string())?;
    insist(
        &persisted == expected,
        "persisted receipt disagrees with original inputs, capture or pre-sign outputs",
    )?;
    insist(
        read(evidence_path)? == evidence,
        "persisted evidence disagrees with rehashed capture",
    )
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use crate::pe::{FixtureSpec, ImportSpec, PeSymbolSpec, fixture};

    fn pair(extra: Option<&str>, machine: u16) -> BTreeMap<String, Vec<u8>> {
        let symbols = [PeSymbolSpec::Named("entry")];
        let mut imports = vec![
            ImportSpec {
                name: "vcruntime140.dll",
                symbols: &symbols,
            },
            ImportSpec {
                name: "vulkan-1.dll",
                symbols: &symbols,
            },
        ];
        if let Some(name) = extra {
            imports.push(ImportSpec {
                name,
                symbols: &symbols,
            });
        }
        let mut engine = fixture(&FixtureSpec {
            machine,
            imports: &imports,
            ..FixtureSpec::default()
        });
        let mut loader = fixture(&FixtureSpec {
            dll: true,
            imports: &[ImportSpec {
                name: "vcruntime140.dll",
                symbols: &symbols,
            }],
            ..FixtureSpec::default()
        });
        // The general PE fixture omits SizeOfHeaders; the dependency census
        // deliberately requires this extent before admitting file-backed RVAs.
        for bytes in [&mut engine, &mut loader] {
            let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
            let optional = pe + 24;
            let optional_size =
                u16::from_le_bytes(bytes[pe + 20..pe + 22].try_into().unwrap()) as usize;
            let headers = (optional + optional_size + 40) as u32;
            bytes[optional + 60..optional + 64].copy_from_slice(&headers.to_le_bytes());
        }
        let report = serde_json::json!([
            {"path": "bin/llama-server.exe", "bytes": engine.len(), "sha256": sha256_hex(&engine)},
            {"path": "bin/vulkan-1.dll", "bytes": loader.len(), "sha256": sha256_hex(&loader)},
        ]);
        BTreeMap::from([
            ("output/bin/llama-server.exe".into(), engine),
            ("output/bin/vulkan-1.dll".into(), loader),
            (
                "report/outputs.json".into(),
                serde_json::to_vec(&report).unwrap(),
            ),
        ])
    }
    #[test]
    fn exact_pair_is_censused_from_original_pe_bytes() {
        let files = pair(None, crate::pe::machine_amd64());
        validate_outputs(&files).unwrap();
        let mut changed = files.clone();
        changed.remove("output/bin/vulkan-1.dll");
        assert!(validate_outputs(&changed).is_err());
        let mut changed = files.clone();
        changed.insert("output/bin/ggml-cuda.dll".into(), vec![]);
        assert!(validate_outputs(&changed).is_err());
        let mut changed = files;
        changed
            .get_mut("output/bin/llama-server.exe")
            .unwrap()
            .push(1);
        assert!(validate_outputs(&changed).is_err());
        assert!(validate_outputs(&pair(Some("nvoglv64.dll"), crate::pe::machine_amd64())).is_err());
        assert!(validate_outputs(&pair(None, crate::pe::machine_arm64())).is_err());
    }
}
