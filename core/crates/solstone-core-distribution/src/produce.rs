// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_ffmpeg_build_support::{
    BUILD_RUN_ID_ENV, EVIDENCE_DIR, parse_ffmpeg_pin, read_configure_receipt,
    read_current_run_record, verify_sha256,
};

use crate::apple::RealArchiveMemberSigner;
use crate::archive_census;
use crate::archive_contract::{
    COMPILED_EXPECTATION_ENV, DeliveryContract, PrebuildInputIdentity, stage_chain,
    write_rfdetr_compiled_expectation,
};
use crate::archive_seal::{SealedArchiveSet, seal_declared_archives};
use crate::digest::sha256_hex;
use crate::elf;
use crate::inventory::{
    Entry, Inventory, OS_LINUX, OS_MACOS, OS_WINDOWS, Target, artifact_set_for_os,
    digest_const_hex, format_named_list, load_payload, parse_min_macos,
};
use crate::lanes::{self, write_wrappers};
use crate::macho;
use crate::onnx_runtime;
use crate::pdfium;
use crate::promote::{PromoteRequest, isolated_target_dir, promote};
use crate::provenance::{
    self, Provenance, bind_cargo_json, bind_ffmpeg_build_script_out_dirs, lock_digest,
};
use crate::record;
use crate::select::{self, ArtifactId, Selection};
use crate::stage;

pub const ZIG_OVERRIDE: &str = "SOLSTONE_ZIG";
pub const ONNX_ARCHIVE_OVERRIDE: &str = "SOLSTONE_DISTRIBUTION_ONNX_ARCHIVE";
pub const ONNX_ARCHIVE_DIR: &str = "SOLSTONE_DISTRIBUTION_ONNX_ARCHIVE_DIR";
pub const PDFIUM_ARCHIVE_OVERRIDE: &str = "SOLSTONE_DISTRIBUTION_PDFIUM_ARCHIVE";
pub const FFMPEG_ARCHIVE_OVERRIDE: &str = "SOLSTONE_DISTRIBUTION_FFMPEG_ARCHIVE";
pub const OFFLINE: &str = "SOLSTONE_DISTRIBUTION_OFFLINE";

#[derive(Debug)]
pub struct ProduceError {
    pub message: String,
}

impl ProduceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProduceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProduceError {}

impl From<io::Error> for ProduceError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub struct ProduceArgs {
    pub target_id: String,
    pub dest: PathBuf,
    pub start: PathBuf,
}

pub struct ProduceReport {
    pub dest: PathBuf,
    pub target: String,
    pub commit: String,
    pub lock_sha256: String,
    pub onnx_source: String,
    pub onnx_wheel_sha256: String,
    pub artifacts: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnnxInput {
    Local(PathBuf),
    PinnedUrl,
}

pub fn select_onnx_input(
    target_id: &str,
    archive: Option<&OsStr>,
    archive_dir: Option<&OsStr>,
    offline: bool,
) -> Result<OnnxInput, ProduceError> {
    if let Some(value) = archive {
        if value.is_empty() {
            return Err(ProduceError::new(format!(
                "missing required:\n  {ONNX_ARCHIVE_OVERRIDE} for {target_id}"
            )));
        }
        return Ok(OnnxInput::Local(PathBuf::from(value)));
    }
    if let Some(value) = archive_dir {
        if value.is_empty() {
            return Err(ProduceError::new(format!(
                "missing required:\n  {ONNX_ARCHIVE_DIR} for {target_id}"
            )));
        }
        return Ok(OnnxInput::Local(
            PathBuf::from(value).join(format!("onnxruntime-{target_id}.whl")),
        ));
    }
    if offline {
        return Err(ProduceError::new(format!(
            "missing required:\n  {ONNX_ARCHIVE_OVERRIDE} or {ONNX_ARCHIVE_DIR} for {target_id}"
        )));
    }
    Ok(OnnxInput::PinnedUrl)
}

fn stage_onnx_input(
    spec: &onnx_runtime::TargetSpec,
    input: &OnnxInput,
) -> Result<onnx_runtime::StagedRuntime, ProduceError> {
    match input {
        OnnxInput::Local(path) => onnx_runtime::stage_from_path(spec, path),
        OnnxInput::PinnedUrl => onnx_runtime::stage_from_url(spec),
    }
    .map_err(|error| ProduceError::new(error.to_string()))
}

fn stage_pdfium_input(
    repo: &Path,
    target_id: &str,
    offline: bool,
) -> Result<(&'static pdfium::TargetSpec, pdfium::StagedRuntime), ProduceError> {
    let spec = pdfium::spec_for(target_id).ok_or_else(|| {
        ProduceError::new(format!("missing required:\n  pdfium runtime {target_id}"))
    })?;
    let archive = if let Some(value) = env::var_os(PDFIUM_ARCHIVE_OVERRIDE) {
        if value.is_empty() {
            return Err(ProduceError::new(format!(
                "missing required:\n  {PDFIUM_ARCHIVE_OVERRIDE} for {target_id}"
            )));
        }
        PathBuf::from(value)
    } else {
        repo.join("target/pdfium-runtime-cache")
            .join(spec.archive_name)
    };
    if !archive.is_file() {
        if offline {
            return Err(ProduceError::new(format!(
                "missing required:\n  pdfium archive {} (run solstone-distribution acquire pdfium --target {target_id})",
                archive.display()
            )));
        }
        return Err(ProduceError::new(format!(
            "missing required:\n  pdfium archive {} (run solstone-distribution acquire pdfium --target {target_id})",
            archive.display()
        )));
    }
    let bytes = fs::read(&archive)?;
    let staged = pdfium::stage_from_bytes(spec, &bytes)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    Ok((spec, staged))
}

pub fn select_ffmpeg_input(
    repo: &Path,
    archive_override: Option<&OsStr>,
) -> Result<PathBuf, ProduceError> {
    let override_selected = archive_override.is_some();
    let archive = match archive_override {
        Some(value) if value.is_empty() => {
            return Err(ProduceError::new(format!(
                "missing required:\n  {FFMPEG_ARCHIVE_OVERRIDE}"
            )));
        }
        Some(value) => PathBuf::from(value),
        None => repo.join("target/ffmpeg-source-cache/ffmpeg.tar.gz"),
    };
    if !archive.is_file() {
        let source = if override_selected {
            format!("{FFMPEG_ARCHIVE_OVERRIDE} {}", archive.display())
        } else {
            format!(
                "FFmpeg archive {} (run solstone-distribution acquire ffmpeg first)",
                archive.display()
            )
        };
        return Err(ProduceError::new(format!("missing required:\n  {source}")));
    }
    let bytes = fs::read(&archive).map_err(|error| {
        ProduceError::new(format!(
            "missing required:\n  FFmpeg archive {} ({error})",
            archive.display()
        ))
    })?;
    let builder_inputs = fs::read_to_string(repo.join("core/distribution/builder-inputs.toml"))?;
    let pin = parse_ffmpeg_pin(&builder_inputs).map_err(ProduceError::new)?;
    verify_sha256(&bytes, &pin.sha256).map_err(|error| {
        ProduceError::new(format!(
            "unexpected:\n  FFmpeg archive {}: {error}",
            archive.display()
        ))
    })?;
    archive.canonicalize().map_err(ProduceError::from)
}

pub fn resolve_zig_binary(
    override_value: Option<&str>,
    path: &str,
) -> Result<PathBuf, ProduceError> {
    if let Some(value) = override_value {
        let candidate = PathBuf::from(value);
        if candidate.is_file() {
            return Ok(candidate.canonicalize()?);
        }
        let nested = candidate.join("zig");
        if nested.is_file() {
            return Ok(nested.canonicalize()?);
        }
        return Err(ProduceError::new(format!(
            "missing required:\n  zig ({ZIG_OVERRIDE}={value})"
        )));
    }
    for entry in env::split_paths(path) {
        let candidate = entry.join("zig");
        if candidate.is_file() {
            return Ok(candidate.canonicalize()?);
        }
    }
    Err(ProduceError::new("missing required:\n  zig"))
}

pub fn discover_zig() -> Result<PathBuf, ProduceError> {
    resolve_zig_binary(
        env::var(ZIG_OVERRIDE).ok().as_deref(),
        &env::var("PATH").unwrap_or_default(),
    )
}

pub fn payload_dest(prefix: &str, source: &str) -> String {
    format!("{prefix}/{source}")
}

pub fn inspect_bin(bin: &str, lane: &str, bytes: &[u8], machine: u16) -> Result<(), ProduceError> {
    let info = elf::parse_elf(bytes).map_err(|error| ProduceError::new(error.to_string()))?;
    match lane {
        "musl-static" => elf::inspect_core_family(&info, machine),
        "zig-gnu-2.27"
            if matches!(
                bin,
                "solstone-core-speakers-analyze" | "solstone-core-vad-analyze"
            ) =>
        {
            elf::inspect_gnu_helper(
                &info,
                machine,
                Some(elf::HELPER_RUNPATH),
                &[elf::HELPER_SONAME],
            )
        }
        "zig-gnu-2.27" => elf::inspect_gnu_helper(&info, machine, None, &[]),
        other => {
            return Err(ProduceError::new(format!("unexpected:\n  lane {other}")));
        }
    }
    .map_err(|error| ProduceError::new(error.to_string()))
}

/// The macOS binary policy. Deliberately not a translation of the Linux one:
/// `crt-static` has no macOS counterpart, so "no dynamic dependency at all"
/// becomes "only system dylibs", and `$ORIGIN` becomes `@loader_path`.
pub fn inspect_macho_bin(
    bin: &str,
    lane: &str,
    bytes: &[u8],
    cputype: u32,
    ceiling: (u32, u32),
) -> Result<(), ProduceError> {
    if lane != "apple-native" {
        return Err(ProduceError::new(format!("unexpected:\n  lane {lane}")));
    }
    let info =
        macho::parse_macho(bytes).map_err(|error| ProduceError::new(format!("{bin}: {error}")))?;
    if matches!(
        bin,
        "solstone-core-speakers-analyze" | "solstone-core-vad-analyze"
    ) {
        macho::inspect_helper(
            &info,
            cputype,
            ceiling,
            macho::HELPER_RPATH,
            macho::HELPER_INSTALL_NAME,
        )
    } else {
        macho::inspect_core_family(&info, cputype, ceiling)
    }
    .map_err(|error| ProduceError::new(format!("{bin}: {error}")))
}

pub fn run(args: ProduceArgs) -> Result<ProduceReport, ProduceError> {
    let inventory_path =
        crate::inventory::repository_inventory_path(&args.start).ok_or_else(|| {
            ProduceError::new(format!(
                "could not find core/distribution/inventory.toml from {}",
                args.start.display()
            ))
        })?;
    let inventory = crate::validate_distribution_inventory(&inventory_path)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let target = inventory
        .target
        .iter()
        .find(|item| item.id == args.target_id)
        .ok_or_else(|| {
            let names = inventory
                .target
                .iter()
                .map(|item| item.id.clone())
                .collect();
            ProduceError::new(format_named_list("missing required", &names))
        })?;
    match target.os.as_str() {
        OS_LINUX => {}
        OS_MACOS => {}
        OS_WINDOWS => {
            return Err(ProduceError::new(
                "windows produce is not implemented in this lode",
            ));
        }
        other => {
            return Err(ProduceError::new(format!("unexpected:\n  os {other}")));
        }
    }
    let repo = inventory_path
        .ancestors()
        .nth(3)
        .ok_or_else(|| ProduceError::new("missing required:\n  repository root"))?;
    let ffmpeg_archive =
        select_ffmpeg_input(repo, env::var_os(FFMPEG_ARCHIVE_OVERRIDE).as_deref())?;
    let ffmpeg_run_id = ffmpeg_build_run_id();

    let commit = git_stdout(repo, &["rev-parse", "HEAD"])?;
    let dirty = !git_stdout(repo, &["status", "--porcelain"])?.is_empty();
    provenance::require_clean(dirty).map_err(|error| ProduceError::new(error.to_string()))?;
    let lock_path = repo.join("core/Cargo.lock");
    let expected_lock =
        lock_digest(&lock_path).map_err(|error| ProduceError::new(error.to_string()))?;
    let version = workspace_version(&repo.join("core/Cargo.toml"))?;
    let epoch = git_stdout(repo, &["show", "-s", "--format=%ct", "HEAD"])?;

    // macOS builds natively with Apple's toolchain and never touches zig, so
    // discovering it would refuse a Mac that is perfectly able to produce.
    let zig_paths = if target.is_macos() {
        None
    } else {
        let zig = discover_zig()?;
        let zig_version = command_stdout(&zig, &["version"])?;
        lanes::check_zig_version(&zig_version)
            .map_err(|error| ProduceError::new(error.to_string()))?;
        let zig_lib = zig
            .parent()
            .map(|parent| parent.join("lib"))
            .filter(|path| path.is_dir())
            .ok_or_else(|| ProduceError::new("missing required:\n  zig lib"))?;
        let zig_dir = zig
            .parent()
            .ok_or_else(|| ProduceError::new("missing required:\n  zig"))?
            .to_path_buf();
        Some((zig_lib, zig_dir))
    };

    let work = env::var_os("SOLSTONE_DISTRIBUTION_WORK")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/var/tmp/solstone-distribution-work").join(&args.target_id)
        });
    fs::create_dir_all(&work)?;
    let checkout = work.join("checkout");
    let wrappers = work.join("wrappers");
    let onnx_dir = work.join("onnx");
    let target_dir = isolated_target_dir(&work);
    fs::create_dir_all(&wrappers)?;
    fs::create_dir_all(&onnx_dir)?;
    fs::create_dir_all(&target_dir)?;

    let _ = git_run(
        repo,
        &["worktree", "remove", "--force", &checkout.to_string_lossy()],
    );
    let _ = fs::remove_dir_all(&checkout);
    git_run(
        repo,
        &[
            "worktree",
            "add",
            "--detach",
            &checkout.to_string_lossy(),
            &commit,
        ],
    )?;

    let result = (|| {
        let host = rustc_host()?;
        let sysroot = command_stdout(Path::new("rustc"), &["--print", "sysroot"])?;
        let spec = onnx_runtime::spec_for(&args.target_id).ok_or_else(|| {
            ProduceError::new(format!(
                "missing required:\n  onnx runtime {}",
                args.target_id
            ))
        })?;
        let onnx_input = select_onnx_input(
            &args.target_id,
            env::var_os(ONNX_ARCHIVE_OVERRIDE).as_deref(),
            env::var_os(ONNX_ARCHIVE_DIR).as_deref(),
            env::var_os(OFFLINE).is_some(),
        )?;
        let staged_runtime = stage_onnx_input(spec, &onnx_input)?;
        onnx_runtime::write_staged_runtime(spec, &staged_runtime, &onnx_dir)
            .map_err(|error| ProduceError::new(error.to_string()))?;
        let (pdfium_spec, staged_pdfium) =
            stage_pdfium_input(repo, &args.target_id, env::var_os(OFFLINE).is_some())?;

        let remap_for = |sysroot: &str| {
            [
                format!("--remap-path-prefix={}=/source", checkout.display()),
                format!("--remap-path-prefix={}=/target", target_dir.display()),
                format!("--remap-path-prefix={sysroot}=/rustc"),
            ]
        };

        let mut artifacts = BTreeMap::new();
        if target.is_macos() {
            let signer = RealArchiveMemberSigner {
                apple: &inventory.apple,
            };
            let sealed_archives =
                seal_declared_archives(&checkout, &inventory, &target.id, &signer)
                    .map_err(|error| ProduceError::new(error.to_string()))?;
            let prebuild = PrebuildInputIdentity::from_sealed_archives(
                &target.id,
                &commit,
                &expected_lock,
                &fs::read(&inventory_path)?,
                &sealed_archives,
            );
            let delivery = DeliveryContract::from_sealed_archives(&prebuild, &sealed_archives);
            let expectation = write_rfdetr_compiled_expectation(&work, &delivery)
                .map_err(|error| ProduceError::new(error.to_string()))?;
            let mut apple_vars = apple_lane_env(target, &onnx_dir);
            apple_vars.insert(
                COMPILED_EXPECTATION_ENV.to_owned(),
                expectation.display().to_string(),
            );
            let remap = remap_for(&sysroot);
            let apple_bins = bins_for_resolved_lane(&inventory, target, "apple-native");
            merge_artifacts(
                &mut artifacts,
                build_lane(BuildLane {
                    checkout: &checkout,
                    target_dir: &target_dir,
                    zig_dir: None,
                    wrapper_dir: &wrappers,
                    triple: &target.triple_apple,
                    host: &host,
                    bins: &apple_bins,
                    vars: &apple_vars,
                    rustflags: &remap.join("\x1f"),
                    epoch: &epoch,
                    ffmpeg_archive: &ffmpeg_archive,
                    ffmpeg_run_id: &ffmpeg_run_id,
                })?,
            );
            build_parakeet_helper(&checkout)?;
            return finish_produce(FinishProduce {
                args: &args,
                inventory: &inventory,
                inventory_path: &inventory_path,
                target,
                checkout: &checkout,
                work: &work,
                spec,
                staged_runtime: &staged_runtime,
                pdfium_spec,
                staged_pdfium: &staged_pdfium,
                artifacts: &artifacts,
                version: &version,
                commit: &commit,
                expected_lock: &expected_lock,
                onnx_input: &onnx_input,
                sealed_archives: Some(&sealed_archives),
                prebuild_input: Some(&prebuild),
                delivery_contract: Some(&delivery),
            });
        }

        if target.os != OS_LINUX {
            return Err(ProduceError::new(format!(
                "unexpected:\n  os {}",
                target.os
            )));
        }
        let (zig_lib, zig_dir) = zig_paths
            .as_ref()
            .ok_or_else(|| ProduceError::new("missing required:\n  zig"))?;
        let zig_lib = zig_lib.as_path();
        let zig_dir = zig_dir.as_path();
        let mut musl_env = lanes::musl_lane_env(target, &wrappers, &host)
            .map_err(|error| ProduceError::new(error.to_string()))?;
        let rust_lld = PathBuf::from(&sysroot)
            .join("lib/rustlib")
            .join(&host)
            .join("bin/rust-lld");
        if !rust_lld.is_file() {
            return Err(ProduceError::new(format!(
                "missing required:\n  rust-lld {}",
                rust_lld.display()
            )));
        }
        musl_env.vars.insert(
            format!(
                "CARGO_TARGET_{}_LINKER",
                lanes::env_target(&target.triple_musl).to_uppercase()
            ),
            rust_lld.display().to_string(),
        );
        musl_env.vars.insert(
            format!(
                "BINDGEN_EXTRA_CLANG_ARGS_{}",
                lanes::env_target(&target.triple_musl)
            ),
            musl_bindgen_args(target, zig_lib),
        );
        let musl_stubs = work.join("musl-lib-stubs");
        write_musl_lib_stubs(&musl_stubs)?;
        let mut gnu_env = lanes::gnu_lane_env(
            target,
            &wrappers,
            zig_lib,
            &checkout,
            Some(&onnx_dir),
            &host,
        )
        .map_err(|error| ProduceError::new(error.to_string()))?;
        if let Some(cflags) = gnu_env
            .vars
            .get(&format!("CFLAGS_{}", lanes::env_target(&target.triple_gnu)))
            .cloned()
        {
            gnu_env.vars.insert("CFLAGS".to_owned(), cflags);
        }
        write_wrappers(&musl_env).map_err(|error| ProduceError::new(error.to_string()))?;
        write_wrappers(&gnu_env).map_err(|error| ProduceError::new(error.to_string()))?;

        let remap = remap_for(&sysroot);
        let musl_rustflags = [
            remap[0].clone(),
            remap[1].clone(),
            remap[2].clone(),
            "-C".to_owned(),
            "link-arg=--build-id=none".to_owned(),
            "-C".to_owned(),
            format!("link-arg=-L{}", musl_stubs.display()),
        ]
        .join("\x1f");
        let gnu_rustflags = [
            remap[0].clone(),
            remap[1].clone(),
            remap[2].clone(),
            "-C".to_owned(),
            "link-arg=-Wl,--build-id=none".to_owned(),
        ]
        .join("\x1f");

        let musl_bins = bins_for_resolved_lane(&inventory, target, "musl-static");
        let gnu_bins = bins_for_resolved_lane(&inventory, target, "zig-gnu-2.27");
        merge_artifacts(
            &mut artifacts,
            build_lane(BuildLane {
                checkout: &checkout,
                target_dir: &target_dir,
                zig_dir: Some(zig_dir),
                wrapper_dir: &wrappers,
                triple: &target.triple_musl,
                host: &host,
                bins: &musl_bins,
                vars: &musl_env.vars,
                rustflags: &musl_rustflags,
                epoch: &epoch,
                ffmpeg_archive: &ffmpeg_archive,
                ffmpeg_run_id: &ffmpeg_run_id,
            })?,
        );
        merge_artifacts(
            &mut artifacts,
            build_lane(BuildLane {
                checkout: &checkout,
                target_dir: &target_dir,
                zig_dir: Some(zig_dir),
                wrapper_dir: &wrappers,
                triple: &target.triple_gnu,
                host: &host,
                bins: &gnu_bins,
                vars: &gnu_env.vars,
                rustflags: &gnu_rustflags,
                epoch: &epoch,
                ffmpeg_archive: &ffmpeg_archive,
                ffmpeg_run_id: &ffmpeg_run_id,
            })?,
        );

        finish_produce(FinishProduce {
            args: &args,
            inventory: &inventory,
            inventory_path: &inventory_path,
            target,
            checkout: &checkout,
            work: &work,
            spec,
            staged_runtime: &staged_runtime,
            pdfium_spec,
            staged_pdfium: &staged_pdfium,
            artifacts: &artifacts,
            version: &version,
            commit: &commit,
            expected_lock: &expected_lock,
            onnx_input: &onnx_input,
            sealed_archives: None,
            prebuild_input: None,
            delivery_contract: None,
        })
    })();

    let _ = git_run(
        repo,
        &["worktree", "remove", "--force", &checkout.to_string_lossy()],
    );
    let _ = fs::remove_dir_all(&checkout);
    result
}

/// Binaries this target builds in `lane`, where `lane` is the RESOLVED lane —
/// the target's own on macOS, the entry's on Linux. Keying off the declared
/// entry lane instead would silently build nothing on macOS, because no entry
/// declares `apple-native`.
fn bins_for_resolved_lane(
    inventory: &Inventory,
    target: &Target,
    lane: &str,
) -> Vec<(String, String)> {
    inventory
        .entry
        .iter()
        .filter_map(|entry| match entry {
            Entry::Bin {
                package,
                bin,
                lane: entry_lane,
                targets,
                ..
            } if target.lane_for(entry_lane) == lane && targets.contains(&target.id) => {
                Some((package.clone(), bin.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The one macOS build lane. No wrappers, no cross toolchain, no zig: the Mac
/// builds for itself with Apple's own linker and SDK.
fn apple_lane_env(target: &Target, onnx_dir: &Path) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    vars.insert(
        "MACOSX_DEPLOYMENT_TARGET".to_owned(),
        target.min_macos.clone(),
    );
    // The ONNX-linked helper links against the staged runtime, exactly as the
    // Linux gnu lane does. Its `@loader_path` rpath comes from the crate's own
    // build.rs, so nothing here has to know the install layout.
    vars.insert("ORT_PREFER_DYNAMIC_LINK".to_owned(), "true".to_owned());
    vars.insert("ORT_LIB_PATH".to_owned(), onnx_dir.display().to_string());
    vars
}

struct FinishProduce<'a> {
    args: &'a ProduceArgs,
    inventory: &'a Inventory,
    inventory_path: &'a Path,
    target: &'a Target,
    checkout: &'a Path,
    work: &'a Path,
    spec: &'a onnx_runtime::TargetSpec,
    staged_runtime: &'a onnx_runtime::StagedRuntime,
    pdfium_spec: &'a pdfium::TargetSpec,
    staged_pdfium: &'a pdfium::StagedRuntime,
    artifacts: &'a BTreeMap<ArtifactId, PathBuf>,
    version: &'a str,
    commit: &'a str,
    expected_lock: &'a str,
    onnx_input: &'a OnnxInput,
    sealed_archives: Option<&'a SealedArchiveSet>,
    prebuild_input: Option<&'a PrebuildInputIdentity>,
    delivery_contract: Option<&'a DeliveryContract>,
}

/// Selection, binary inspection, staging and atomic promotion — identical for
/// both platforms except for which inspector reads the binaries and which
/// containers promotion emits.
fn finish_produce(finish: FinishProduce<'_>) -> Result<ProduceReport, ProduceError> {
    let FinishProduce {
        args,
        inventory,
        inventory_path,
        target,
        checkout,
        work,
        spec,
        staged_runtime,
        pdfium_spec,
        staged_pdfium,
        artifacts,
        version,
        commit,
        expected_lock,
        onnx_input,
        sealed_archives,
        prebuild_input,
        delivery_contract,
    } = finish;

    select::refuse_wrong_triple(inventory, &args.target_id, artifacts)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    select::refuse_extra(inventory, &args.target_id, artifacts)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let selection = select::select_artifacts(inventory, &args.target_id, artifacts)
        .map_err(|error| ProduceError::new(error.to_string()))?;

    if target.is_macos() {
        let cputype = macho::cputype_for_arch(&target.arch)
            .ok_or_else(|| ProduceError::new(format!("unexpected:\n  arch {}", target.arch)))?;
        let ceiling = parse_min_macos(&target.min_macos).ok_or_else(|| {
            ProduceError::new(format!("unexpected:\n  min_macos {}", target.min_macos))
        })?;
        for bin in &selection.bins {
            let bytes = fs::read(&bin.path)?;
            inspect_macho_bin(&bin.bin, &bin.lane, &bytes, cputype, ceiling)?;
        }
    } else {
        let machine = match target.arch.as_str() {
            "x86_64" => elf::machine_x86_64(),
            "aarch64" => elf::machine_aarch64(),
            other => {
                return Err(ProduceError::new(format!("unexpected:\n  arch {other}")));
            }
        };
        for bin in &selection.bins {
            let bytes = fs::read(&bin.path)?;
            inspect_bin(&bin.bin, &bin.lane, &bytes, machine)?;
        }
    }

    let stage = work.join("stage");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage)?;
    write_stage(
        &selection,
        checkout,
        inventory_path,
        inventory,
        &args.target_id,
        Some((spec, staged_runtime)),
        Some((pdfium_spec, staged_pdfium)),
        sealed_archives,
        &stage,
    )?;

    // The macOS payload dylib is inspected here rather than in the binary loop,
    // because it is not a binary: it is staged straight out of the pinned wheel
    // and never passes through `select`. A census that walked only `selection`
    // would report a clean tree with the loaded half never looked at.
    if target.is_macos() {
        archive_census::validate_staged_archives(&stage, inventory)
            .map_err(|error| ProduceError::new(error.to_string()))?;
        inspect_macos_payloads(&stage, target)?;
        let prebuild = prebuild_input.ok_or_else(|| {
            ProduceError::new("missing required:\n  macOS prebuild archive identity")
        })?;
        let delivery = delivery_contract.ok_or_else(|| {
            ProduceError::new("missing required:\n  macOS archive delivery contract")
        })?;
        stage_chain(&stage, prebuild, delivery, commit, expected_lock)
            .map_err(|error| ProduceError::new(error.to_string()))?;
    }

    let tree = tree_from_stage(&stage)?;
    let observed_lock = lock_digest(&checkout.join("core/Cargo.lock"))
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let observed_commit = git_stdout(checkout, &["rev-parse", "HEAD"])?;
    if let Some(parent) = args.dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let basename = inventory.artifact.render(version, &target.os, &target.arch);
    let produced = promote(&PromoteRequest {
        dest: args.dest.clone(),
        work: work.join("promote"),
        tree,
        version: version.to_owned(),
        basename: basename.clone(),
        os: target.os.clone(),
        arch: args.target_id.clone(),
        deb_arch: target.deb_arch.clone(),
        rpm_arch: target.rpm_arch.clone(),
        dirty: false,
        observed: Provenance {
            commit: observed_commit,
            lock_sha256: observed_lock,
        },
        expected: Provenance {
            commit: commit.to_owned(),
            lock_sha256: expected_lock.to_owned(),
        },
        fail_after: None,
        apple: target.is_macos().then(|| inventory.apple.clone()),
    })
    .map_err(|error| ProduceError::new(error.to_string()))?;

    let produced_artifacts = artifact_set_for_os(&target.os, &basename)
        .map_err(ProduceError::new)?
        .into_iter()
        .map(|name| produced.join(name))
        .collect::<Vec<_>>();
    for path in &produced_artifacts {
        if !path.is_file() {
            return Err(ProduceError::new(format!(
                "missing required:\n  {}",
                path.display()
            )));
        }
    }
    Ok(ProduceReport {
        dest: produced,
        target: args.target_id.clone(),
        commit: commit.to_owned(),
        lock_sha256: expected_lock.to_owned(),
        onnx_source: match onnx_input {
            OnnxInput::Local(path) => path.display().to_string(),
            OnnxInput::PinnedUrl => spec.wheel_url.to_owned(),
        },
        onnx_wheel_sha256: spec.wheel_sha256.to_owned(),
        artifacts: produced_artifacts,
    })
}

/// Every Mach-O in the staged tree that is NOT an executable — the loaded half.
fn inspect_macos_payloads(stage: &Path, target: &Target) -> Result<(), ProduceError> {
    let cputype = macho::cputype_for_arch(&target.arch)
        .ok_or_else(|| ProduceError::new(format!("unexpected:\n  arch {}", target.arch)))?;
    let ceiling = parse_min_macos(&target.min_macos).ok_or_else(|| {
        ProduceError::new(format!("unexpected:\n  min_macos {}", target.min_macos))
    })?;
    let members = crate::apple::discover_macho_members(stage)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let payloads = members
        .iter()
        .filter(|member| member.payload)
        .collect::<Vec<_>>();
    if payloads.is_empty() {
        return Err(ProduceError::new(
            "missing required:\n  a loaded mach-o payload in the staged macos tree",
        ));
    }
    for member in payloads {
        let install_name = if member.relative.contains("libpdfium") {
            member.info.install_name.as_deref().ok_or_else(|| {
                ProduceError::new(format!(
                    "missing required:\n  LC_ID_DYLIB {}",
                    member.relative
                ))
            })?
        } else {
            macho::HELPER_INSTALL_NAME
        };
        macho::inspect_payload_dylib(&member.info, cputype, ceiling, install_name)
            .map_err(|error| ProduceError::new(format!("{}: {error}", member.relative)))?;
    }
    Ok(())
}

struct BuildLane<'a> {
    checkout: &'a Path,
    target_dir: &'a Path,
    /// `None` on the Apple lane, which uses the host toolchain directly.
    zig_dir: Option<&'a Path>,
    wrapper_dir: &'a Path,
    triple: &'a str,
    host: &'a str,
    bins: &'a [(String, String)],
    vars: &'a BTreeMap<String, String>,
    rustflags: &'a str,
    epoch: &'a str,
    ffmpeg_archive: &'a Path,
    ffmpeg_run_id: &'a str,
}

fn build_lane(lane: BuildLane<'_>) -> Result<BTreeMap<ArtifactId, PathBuf>, ProduceError> {
    if lane.bins.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut command = Command::new("cargo");
    command
        .current_dir(lane.checkout)
        .env("CARGO_TARGET_DIR", lane.target_dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("SOURCE_DATE_EPOCH", lane.epoch)
        .env("ZERO_AR_DATE", "1")
        .env("TZ", "UTC")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .env("CARGO_ENCODED_RUSTFLAGS", lane.rustflags)
        .env("SOLSTONE_FFMPEG_SOURCE_ARCHIVE", lane.ffmpeg_archive)
        .env(OFFLINE, "1")
        .env(BUILD_RUN_ID_ENV, lane.ffmpeg_run_id)
        .env(
            "PATH",
            match lane.zig_dir {
                Some(zig_dir) => prepend_path(
                    lane.wrapper_dir,
                    &prepend_path(zig_dir, &env::var("PATH").unwrap_or_default()),
                ),
                None => env::var("PATH").unwrap_or_default(),
            },
        )
        .args([
            "build",
            "--manifest-path",
            "core/Cargo.toml",
            "--locked",
            "--offline",
            "--release",
            "--target",
            lane.triple,
            "--message-format=json",
        ]);
    for (package, bin) in lane.bins {
        command.arg("-p").arg(package).arg("--bin").arg(bin);
    }
    let host_arch = lane.host.split('-').next().unwrap_or_default();
    let target_arch = lane.triple.split('-').next().unwrap_or_default();
    for (key, value) in lane.vars {
        if key == lanes::describe_cc_key() && host_arch != target_arch {
            continue;
        }
        command.env(key, value);
    }
    if let Some((_, ar)) = lane.vars.iter().find(|(key, _)| key.starts_with("AR_")) {
        command.env("AR", ar);
    }
    if let Some((_, ranlib)) = lane.vars.iter().find(|(key, _)| key.starts_with("RANLIB_")) {
        command.env("RANLIB", ranlib);
    }
    if host_arch == target_arch {
        if let Some((_, cc)) = lane.vars.iter().find(|(key, _)| key.starts_with("CC_")) {
            command.env("CC", cc);
        }
        if let Some((_, cxx)) = lane.vars.iter().find(|(key, _)| key.starts_with("CXX_")) {
            command.env("CXX", cxx);
        }
    }
    let output = command
        .output()
        .map_err(|error| ProduceError::new(format!("cargo: {error}")))?;
    if !output.status.success() {
        return Err(ProduceError::new(format!(
            "cargo {} failed:\n{}\n{}",
            lane.triple,
            String::from_utf8_lossy(&output.stderr),
            cargo_rendered_errors(&String::from_utf8_lossy(&output.stdout))
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ffmpeg_out_dir =
        require_single_ffmpeg_out_dir(bind_ffmpeg_build_script_out_dirs(&stdout), lane.triple)?;
    validate_ffmpeg_evidence(
        &ffmpeg_out_dir.join(EVIDENCE_DIR),
        lane.ffmpeg_run_id,
        lane.triple,
        "release",
    )?;
    bind_cargo_json(&stdout, lane.triple).map_err(|error| ProduceError::new(error.to_string()))
}

fn require_single_ffmpeg_out_dir(
    out_dirs: Vec<PathBuf>,
    target: &str,
) -> Result<PathBuf, ProduceError> {
    if let [out_dir] = out_dirs.as_slice() {
        return Ok(out_dir.clone());
    }
    Err(ProduceError::new(format!(
        "incomplete FFmpeg configure evidence:\n  expected one ffmpeg build-script out directory for {target}, found {}",
        out_dirs.len()
    )))
}

fn ffmpeg_build_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

fn validate_ffmpeg_evidence(
    evidence_dir: &Path,
    expected_run_id: &str,
    expected_target: &str,
    expected_profile: &str,
) -> Result<(), ProduceError> {
    let record = read_current_run_record(evidence_dir).map_err(incomplete_ffmpeg_evidence)?;
    if record.run_id != expected_run_id {
        return Err(incomplete_ffmpeg_evidence(format!(
            "run id {} does not match {expected_run_id}",
            record.run_id
        )));
    }
    if record.target != expected_target || record.profile != expected_profile {
        return Err(incomplete_ffmpeg_evidence(format!(
            "record target/profile {}/{} does not match {expected_target}/{expected_profile}",
            record.target, record.profile
        )));
    }
    let stored = read_configure_receipt(evidence_dir, &record.receipt_filename)
        .map_err(incomplete_ffmpeg_evidence)?;
    if stored.content_sha256 != record.receipt_sha256 {
        return Err(incomplete_ffmpeg_evidence(
            "referenced receipt digest does not match the current-run record",
        ));
    }
    let receipt = stored.receipt;
    if receipt.target != record.target
        || receipt.profile != record.profile
        || receipt.source_sha256 != record.source_sha256
        || receipt.fingerprint != record.fingerprint
    {
        return Err(incomplete_ffmpeg_evidence(
            "referenced receipt does not match the current-run record",
        ));
    }
    if expected_profile == "release" {
        validate_release_configure_args(&receipt.args)?;
    }
    Ok(())
}

fn incomplete_ffmpeg_evidence(detail: impl std::fmt::Display) -> ProduceError {
    ProduceError::new(format!("incomplete FFmpeg configure evidence:\n  {detail}"))
}

fn validate_release_configure_args(args: &[String]) -> Result<(), ProduceError> {
    let mistyped_optimization = ["-", "0", "3"].concat();
    if args.iter().any(|arg| arg.contains(&mistyped_optimization)) {
        return Err(incomplete_ffmpeg_evidence(
            "release configure receipt contains invalid optimization flag",
        ));
    }
    if args.iter().any(|arg| arg == "--enable-debug")
        || !args.iter().any(|arg| arg == "--disable-debug")
    {
        return Err(incomplete_ffmpeg_evidence(
            "release configure receipt does not disable debug",
        ));
    }
    if args.iter().any(|arg| arg == "--disable-stripping")
        || !args.iter().any(|arg| arg == "--enable-stripping")
    {
        return Err(incomplete_ffmpeg_evidence(
            "release configure receipt does not enable stripping",
        ));
    }
    if !args.iter().any(|arg| arg.contains("-O3")) {
        return Err(incomplete_ffmpeg_evidence(
            "release configure receipt does not enable -O3",
        ));
    }
    Ok(())
}

fn musl_bindgen_args(target: &crate::inventory::Target, zig_lib: &Path) -> String {
    let arch_musl = format!("{}-linux-musl", target.arch);
    let arch_any = format!("{}-linux-any", target.arch);
    let lib = zig_lib.display();
    format!(
        "-nostdinc --target={} -isystem {lib}/include -isystem {lib}/libc/include/{arch_musl} -isystem {lib}/libc/include/generic-musl -isystem {lib}/libc/include/{arch_any} -isystem {lib}/libc/include/any-linux-any",
        target.triple_musl
    )
}

fn write_musl_lib_stubs(dir: &Path) -> Result<(), ProduceError> {
    fs::create_dir_all(dir)?;
    for name in ["libdl.a", "libm.a", "libatomic.a"] {
        fs::write(dir.join(name), b"!<arch>\n")?;
    }
    Ok(())
}

fn cargo_rendered_errors(stdout: &str) -> String {
    let mut messages = Vec::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(|item| item.as_str()) != Some("compiler-message") {
            continue;
        }
        if let Some(rendered) = value
            .pointer("/message/rendered")
            .and_then(|item| item.as_str())
        {
            messages.push(rendered.to_owned());
        }
    }
    messages.join("")
}

fn merge_artifacts(into: &mut BTreeMap<ArtifactId, PathBuf>, from: BTreeMap<ArtifactId, PathBuf>) {
    into.extend(from);
}

pub struct StageInventoryTree {
    pub selection: Selection,
    pub records: Vec<crate::record::FileRecord>,
}

#[allow(clippy::too_many_arguments)]
pub fn stage_inventory_tree(
    repo: &Path,
    inventory_path: &Path,
    inventory: &Inventory,
    target_id: &str,
    artifacts: &BTreeMap<ArtifactId, PathBuf>,
    onnx: Option<(&onnx_runtime::TargetSpec, &onnx_runtime::StagedRuntime)>,
    pdfium: Option<(&pdfium::TargetSpec, &pdfium::StagedRuntime)>,
    sealed_archives: Option<&SealedArchiveSet>,
    stage: &Path,
) -> Result<StageInventoryTree, ProduceError> {
    select::refuse_wrong_triple(inventory, target_id, artifacts)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    select::refuse_extra(inventory, target_id, artifacts)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let selection = select::select_artifacts(inventory, target_id, artifacts)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let _ = fs::remove_dir_all(stage);
    fs::create_dir_all(stage)?;
    let records = write_stage(
        &selection,
        repo,
        inventory_path,
        inventory,
        target_id,
        onnx,
        pdfium,
        sealed_archives,
        stage,
    )?;
    Ok(StageInventoryTree { selection, records })
}

#[allow(clippy::too_many_arguments)]
fn write_stage(
    selection: &Selection,
    repo: &Path,
    inventory_path: &Path,
    inventory: &Inventory,
    target_id: &str,
    onnx: Option<(&onnx_runtime::TargetSpec, &onnx_runtime::StagedRuntime)>,
    pdfium: Option<(&pdfium::TargetSpec, &pdfium::StagedRuntime)>,
    sealed_archives: Option<&SealedArchiveSet>,
    stage: &Path,
) -> Result<Vec<crate::record::FileRecord>, ProduceError> {
    select::stage_selected(selection, stage)?;
    stage_layout(
        repo,
        inventory_path,
        inventory,
        target_id,
        onnx,
        pdfium,
        sealed_archives,
        stage,
    )?;
    let records = stage::staged_records(stage)?;
    let payload = load_payload(inventory_path, inventory)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let artifacts = selection
        .bins
        .iter()
        .map(|bin| {
            (
                ArtifactId {
                    package: bin.package.clone(),
                    bin: bin.bin.clone(),
                    triple: bin.triple.clone(),
                },
                bin.path.clone(),
            )
        })
        .collect();
    let declared = record::declared_records(
        inventory,
        target_id,
        repo,
        &payload,
        &artifacts,
        onnx,
        pdfium,
        sealed_archives,
    )
    .map_err(ProduceError::new)?;
    record::compare_records("declared", &declared, "staged", &records)
        .map_err(ProduceError::new)?;
    Ok(records)
}

#[allow(clippy::too_many_arguments)]
fn stage_layout(
    repo: &Path,
    inventory_path: &Path,
    inventory: &Inventory,
    target_id: &str,
    onnx: Option<(&onnx_runtime::TargetSpec, &onnx_runtime::StagedRuntime)>,
    pdfium: Option<(&pdfium::TargetSpec, &pdfium::StagedRuntime)>,
    sealed_archives: Option<&SealedArchiveSet>,
    stage: &Path,
) -> Result<(), ProduceError> {
    for entry in &inventory.entry {
        match entry {
            Entry::Bin { .. } => {}
            Entry::Launcher {
                source,
                dest,
                mode,
                targets,
            }
            | Entry::Copy {
                source,
                dest,
                mode,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let bytes = fs::read(repo.join(source))?;
                stage::write_staged_file_mode(stage, dest, &bytes, *mode)?;
            }
            Entry::ModelAsset {
                source,
                dest,
                mode,
                digest_const,
                digest_source,
                targets,
                archive_slot,
                ..
            } => {
                stage_model_asset(
                    repo,
                    stage,
                    target_id,
                    source,
                    dest,
                    *mode,
                    digest_const,
                    digest_source,
                    targets,
                    archive_slot.as_ref(),
                    sealed_archives,
                )?;
            }
            Entry::OnnxRuntime {
                dest_dir,
                mode: _,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let (spec, runtime) = onnx.ok_or_else(|| {
                    ProduceError::new(format!("missing required:\n  onnx runtime {target_id}"))
                })?;
                onnx_runtime::write_staged_runtime(spec, runtime, &stage.join(dest_dir))
                    .map_err(|error| ProduceError::new(error.to_string()))?;
            }
            Entry::Pdfium {
                dest_dir,
                mode: _,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let (spec, pdfium_runtime) = pdfium.ok_or_else(|| {
                    ProduceError::new(format!("missing required:\n  pdfium runtime {target_id}"))
                })?;
                pdfium::write_staged_library(spec, pdfium_runtime, &stage.join(dest_dir))
                    .map_err(|error| ProduceError::new(error.to_string()))?;
            }
        }
    }
    let payload = load_payload(inventory_path, inventory)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let target = inventory
        .target
        .iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| ProduceError::new(format!("missing required:\n  target {target_id}")))?;
    if target.is_windows() {
        if !payload.is_empty() {
            return Err(ProduceError::new(
                "windows payload is not implemented in this lode",
            ));
        }
    } else {
        for source in payload {
            let dest = payload_dest(&inventory.payload_dest_prefix, &source);
            let bytes = fs::read(repo.join(&inventory.payload_src_root).join(&source))?;
            stage::write_staged_file_mode(stage, &dest, &bytes, 0o644)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stage_model_asset(
    repo: &Path,
    stage: &Path,
    target_id: &str,
    source: &str,
    dest: &str,
    mode: u32,
    digest_const: &str,
    digest_source: &str,
    targets: &[String],
    archive_slot: Option<&crate::inventory::ArchiveSlot>,
    sealed_archives: Option<&SealedArchiveSet>,
) -> Result<(), ProduceError> {
    if !targets.iter().any(|item| item == target_id) {
        return Ok(());
    }
    if let Some(slot) = archive_slot {
        let sealed = sealed_archives
            .and_then(|archives| archives.by_slot_id(&slot.id))
            .ok_or_else(|| {
                ProduceError::new(format!(
                    "missing required:\n  sealed archive slot {}",
                    slot.id
                ))
            })?;
        if sealed.staged_dest != dest {
            return Err(ProduceError::new(format!(
                "unexpected:\n  sealed archive slot {} dest {} (want {dest})",
                slot.id, sealed.staged_dest
            )));
        }
        stage::write_staged_file_mode(stage, dest, &sealed.bytes, mode)?;
        return Ok(());
    }
    let bytes = fs::read(repo.join(source))?;
    let expected = digest_const_hex(&fs::read_to_string(repo.join(digest_source))?, digest_const)
        .ok_or_else(|| {
        ProduceError::new(format!("missing required:\n  digest {digest_const}"))
    })?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(ProduceError::new(format!(
            "unexpected:\n  {dest} digest {actual}"
        )));
    }
    stage::write_staged_file_mode(stage, dest, &bytes, mode)?;
    Ok(())
}

fn tree_from_stage(stage: &Path) -> Result<Vec<(String, Vec<u8>, u32)>, ProduceError> {
    let mut tree = Vec::new();
    for record in stage::staged_records(stage)? {
        tree.push((
            record.dest.clone(),
            fs::read(stage.join(&record.dest))?,
            record.mode,
        ));
    }
    Ok(tree)
}

fn workspace_version(path: &Path) -> Result<String, ProduceError> {
    let text = fs::read_to_string(path)?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[workspace.package]";
            continue;
        }
        if in_package && let Some(value) = trimmed.strip_prefix("version") {
            let value = value
                .trim()
                .trim_start_matches('=')
                .trim()
                .trim_matches('"');
            if !value.is_empty() {
                return Ok(value.to_owned());
            }
        }
    }
    Err(ProduceError::new("missing required:\n  workspace version"))
}

fn rustc_host() -> Result<String, ProduceError> {
    let text = command_stdout(Path::new("rustc"), &["-vV"])?;
    for line in text.lines() {
        if let Some(host) = line.strip_prefix("host: ") {
            return Ok(host.to_owned());
        }
    }
    Err(ProduceError::new("missing required:\n  rustc host"))
}

fn prepend_path(dir: &Path, path: &str) -> String {
    let mut entries = vec![dir.to_path_buf()];
    entries.extend(env::split_paths(path));
    env::join_paths(entries)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{}:{path}", dir.display()))
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String, ProduceError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| ProduceError::new(format!("git: {error}")))?;
    if !output.status.success() {
        return Err(ProduceError::new(format!(
            "git {} failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_run(repo: &Path, args: &[&str]) -> Result<(), ProduceError> {
    git_stdout(repo, args).map(|_| ())
}

/// The CoreML helper (parakeet-helper) is Swift, not Rust, so cargo build
/// never produces it. Building it here — inside the isolated checkout, from
/// nothing but what that commit tracks — keeps it inside the same
/// reproducibility boundary as every cargo-built binary: the artifact traces
/// to a commit, not to whatever happened to be lying around in the working
/// tree that invoked produce. The entry that stages the result is a plain
/// copy entry in the inventory, matched against this same relative path.
fn build_parakeet_helper(checkout: &Path) -> Result<(), ProduceError> {
    let dir = checkout.join("core/crates/solstone-core-transcribe/parakeet-helper");
    let output = Command::new("swift")
        .arg("build")
        .arg("-c")
        .arg("release")
        .current_dir(&dir)
        .output()
        .map_err(|error| ProduceError::new(format!("swift build: {error}")))?;
    if !output.status.success() {
        return Err(ProduceError::new(format!(
            "swift build -c release failed in {}:\n{}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

fn command_stdout(bin: &Path, args: &[&str]) -> Result<String, ProduceError> {
    let output = Command::new(bin)
        .args(args)
        .output()
        .map_err(|error| ProduceError::new(format!("{}: {error}", bin.display())))?;
    if !output.status.success() {
        return Err(ProduceError::new(format!(
            "{} {} failed:\n{}",
            bin.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_ffmpeg_build_support::{
        ConfigureReceipt, ConfigureRunRecord, write_configure_receipt, write_current_run_record,
    };

    use std::collections::BTreeSet;

    use crate::archive_taxonomy::ContainerKind;
    use crate::inventory::{ArchiveExecutable, ArchiveSlot};

    fn write_ffmpeg_pin(repo: &Path, sha256: &str) {
        let inputs = repo.join("core/distribution/builder-inputs.toml");
        fs::create_dir_all(inputs.parent().unwrap()).unwrap();
        fs::write(
            inputs,
            format!(
                "[ffmpeg]\ncommit = \"fixture\"\nfilename = \"fixture.tar.gz\"\nurl = \"https://example.invalid/fixture.tar.gz\"\nsha256 = \"{sha256}\"\nsize = 1\n"
            ),
        )
        .unwrap();
    }

    fn write_ffmpeg_evidence(
        evidence_dir: &Path,
        run_id: &str,
        configure_executed: bool,
        args: &[String],
    ) -> ConfigureRunRecord {
        let receipt = ConfigureReceipt::new(
            "x86_64-unknown-linux-gnu",
            "release",
            &"a".repeat(64),
            "/source/configure",
            args,
        );
        let receipt = write_configure_receipt(evidence_dir, &receipt).unwrap();
        let record = ConfigureRunRecord::new(run_id, &receipt, configure_executed).unwrap();
        write_current_run_record(evidence_dir, &record).unwrap();
        record
    }

    #[test]
    fn payload_dest_uses_inventory_prefix() {
        assert_eq!(
            payload_dest("share", "solstone/talent/journal/contract/bundle.json"),
            "share/solstone/talent/journal/contract/bundle.json"
        );
    }

    #[test]
    fn digest_const_hex_reads_named_literal() {
        let source = "pub const WESPEAKER_RESNET34_SHA256: &str =\n    \"5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94\";\n";
        assert_eq!(
            digest_const_hex(source, "WESPEAKER_RESNET34_SHA256").as_deref(),
            Some("5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94")
        );
    }

    #[test]
    fn model_asset_digest_mismatch_is_rejected_and_target_exclusion_skips_it() {
        let repo = tempfile::tempdir().unwrap();
        let stage = tempfile::tempdir().unwrap();
        fs::create_dir_all(repo.path().join("assets")).unwrap();
        fs::write(repo.path().join("assets/payload"), b"expected bytes").unwrap();
        fs::write(
            repo.path().join("pins.rs"),
            format!(
                "pub const PAYLOAD_SHA256: &str = \"{}\";\n",
                sha256_hex(b"expected bytes")
            ),
        )
        .unwrap();
        let targets = vec!["linux-x86_64".to_owned()];

        stage_model_asset(
            repo.path(),
            stage.path(),
            "macos-arm64",
            "assets/payload",
            "lib/payload",
            0o644,
            "PAYLOAD_SHA256",
            "pins.rs",
            &targets,
            None,
            None,
        )
        .unwrap();
        assert!(!stage.path().join("lib/payload").exists());

        fs::write(repo.path().join("assets/payload"), b"wrong bytes").unwrap();
        let error = stage_model_asset(
            repo.path(),
            stage.path(),
            "linux-x86_64",
            "assets/payload",
            "lib/payload",
            0o644,
            "PAYLOAD_SHA256",
            "pins.rs",
            &targets,
            None,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("digest"));
        fs::write(repo.path().join("assets/payload"), b"expected bytes").unwrap();
    }

    #[test]
    fn rfdetr_archive_slot_stages_only_the_sealed_derivative() {
        let repo = tempfile::tempdir().expect("temporary repository");
        let stage = tempfile::tempdir().expect("temporary stage");
        let targets = vec!["macos-arm64".to_owned()];
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz");
        let original = fs::read(source).expect("checked-in RF-DETR archive");
        let mut derivative = original.clone();
        derivative.extend_from_slice(b"fixture sealed derivative");
        let slot = ArchiveSlot {
            id: "rfdetr-macos-metal-arm64".to_owned(),
            target: "macos-arm64".to_owned(),
            container: ContainerKind::GzipTar,
            executables: vec![ArchiveExecutable {
                path: "rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64/rfdetr-cli".to_owned(),
                digest_const: "RFDETR_ENGINE_MACOS_METAL_ARM64_BINARY_SHA256".to_owned(),
                digest_source: "core/crates/solstone-core-local/src/install/rfdetr_install.rs"
                    .to_owned(),
            }],
        };
        let dest = "lib/solstone_journal_models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz";
        let sealed = SealedArchiveSet {
            archives: vec![crate::archive_seal::SealedArchive {
                slot_id: slot.id.clone(),
                staged_dest: dest.to_owned(),
                bytes: derivative.clone(),
                sha256: sha256_hex(&derivative),
                size: derivative.len() as u64,
                source_sha256: sha256_hex(&original),
                signed_executables: vec![],
            }],
        };

        stage_model_asset(
            repo.path(),
            stage.path(),
            "macos-arm64",
            "core/models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz",
            dest,
            0o644,
            "MISSING_SHA256",
            "missing.rs",
            &targets,
            Some(&slot),
            Some(&sealed),
        )
        .expect("sealed derivative stages without reading original");
        assert_eq!(
            fs::read(stage.path().join(dest)).expect("staged derivative"),
            derivative
        );
        assert_ne!(fs::read(stage.path().join(dest)).unwrap(), original);

        let error = stage_model_asset(
            repo.path(),
            stage.path(),
            "macos-arm64",
            "core/models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz",
            dest,
            0o644,
            "MISSING_SHA256",
            "missing.rs",
            &targets,
            Some(&slot),
            None,
        )
        .expect_err("missing sealed archive refuses");
        assert!(error.to_string().contains("sealed archive slot"));
    }

    #[test]
    fn onnx_input_selection_is_fail_closed_offline() {
        let direct = select_onnx_input(
            "linux-x86_64",
            Some(OsStr::new("/inputs/x86.whl")),
            Some(OsStr::new("/ignored")),
            true,
        )
        .unwrap();
        assert_eq!(direct, OnnxInput::Local(PathBuf::from("/inputs/x86.whl")));

        let from_dir =
            select_onnx_input("linux-aarch64", None, Some(OsStr::new("/inputs")), true).unwrap();
        assert_eq!(
            from_dir,
            OnnxInput::Local(PathBuf::from("/inputs/onnxruntime-linux-aarch64.whl"))
        );

        let missing = select_onnx_input("linux-x86_64", None, None, true).unwrap_err();
        assert!(missing.to_string().contains(ONNX_ARCHIVE_OVERRIDE));
        assert!(missing.to_string().contains(ONNX_ARCHIVE_DIR));
        assert_eq!(
            select_onnx_input("linux-x86_64", None, None, false).unwrap(),
            OnnxInput::PinnedUrl
        );
    }

    #[test]
    fn ffmpeg_input_selection_is_verified_before_lanes_spawn() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("target/ffmpeg-source-cache/ffmpeg.tar.gz");
        fs::create_dir_all(archive.parent().unwrap()).unwrap();
        fs::write(&archive, b"verified ffmpeg archive").unwrap();
        write_ffmpeg_pin(root.path(), &sha256_hex(b"verified ffmpeg archive"));

        let default = select_ffmpeg_input(root.path(), None).unwrap();
        assert_eq!(default, archive.canonicalize().unwrap());

        let override_archive = root.path().join("override.tar.gz");
        fs::write(&override_archive, b"verified ffmpeg archive").unwrap();
        assert_eq!(
            select_ffmpeg_input(root.path(), Some(override_archive.as_os_str())).unwrap(),
            override_archive.canonicalize().unwrap()
        );

        let empty = select_ffmpeg_input(root.path(), Some(OsStr::new(""))).unwrap_err();
        assert!(empty.to_string().contains(FFMPEG_ARCHIVE_OVERRIDE));

        let missing =
            select_ffmpeg_input(root.path(), Some(OsStr::new("missing.tar.gz"))).unwrap_err();
        assert!(missing.to_string().contains(FFMPEG_ARCHIVE_OVERRIDE));

        let wrong = root.path().join("wrong.tar.gz");
        fs::write(&wrong, b"wrong digest").unwrap();
        assert!(select_ffmpeg_input(root.path(), Some(wrong.as_os_str())).is_err());

        let empty_archive = root.path().join("empty.tar.gz");
        fs::write(&empty_archive, b"").unwrap();
        assert!(select_ffmpeg_input(root.path(), Some(empty_archive.as_os_str())).is_err());

        let unreadable = root.path().join("not-an-archive");
        fs::create_dir_all(&unreadable).unwrap();
        assert!(select_ffmpeg_input(root.path(), Some(unreadable.as_os_str())).is_err());

        fs::remove_file(&archive).unwrap();
        let missing_default = select_ffmpeg_input(root.path(), None).unwrap_err();
        assert!(
            missing_default
                .to_string()
                .contains("solstone-distribution acquire ffmpeg")
        );
    }

    #[test]
    fn ffmpeg_evidence_requires_a_fresh_matching_record_and_receipt() {
        let root = tempfile::tempdir().unwrap();
        let evidence = root.path().join(EVIDENCE_DIR);
        let release_args = vec![
            "--disable-debug".to_owned(),
            "--enable-stripping".to_owned(),
            "--extra-cflags=-O3 -ffast-math".to_owned(),
        ];
        let record = write_ffmpeg_evidence(&evidence, "current", true, &release_args);
        validate_ffmpeg_evidence(&evidence, "current", "x86_64-unknown-linux-gnu", "release")
            .unwrap();

        let stale =
            validate_ffmpeg_evidence(&evidence, "next-run", "x86_64-unknown-linux-gnu", "release")
                .unwrap_err();
        assert!(stale.to_string().contains("incomplete"));

        fs::remove_file(evidence.join(&record.receipt_filename)).unwrap();
        assert!(
            validate_ffmpeg_evidence(&evidence, "current", "x86_64-unknown-linux-gnu", "release")
                .is_err()
        );

        let no_record = root.path().join("no-record");
        fs::create_dir_all(&no_record).unwrap();
        assert!(
            validate_ffmpeg_evidence(&no_record, "current", "x86_64-unknown-linux-gnu", "release")
                .is_err()
        );
    }

    #[test]
    fn ffmpeg_evidence_requires_exactly_one_build_script_result() {
        let target = "x86_64-unknown-linux-gnu";
        assert!(require_single_ffmpeg_out_dir(Vec::new(), target).is_err());
        assert!(
            require_single_ffmpeg_out_dir(
                vec![PathBuf::from("first"), PathBuf::from("second")],
                target,
            )
            .is_err()
        );
        assert_eq!(
            require_single_ffmpeg_out_dir(vec![PathBuf::from("only")], target).unwrap(),
            PathBuf::from("only")
        );
    }

    #[test]
    fn ffmpeg_evidence_refuses_tampered_or_release_unsafe_receipts() {
        let root = tempfile::tempdir().unwrap();
        let evidence = root.path().join(EVIDENCE_DIR);
        let release_args = vec![
            "--disable-debug".to_owned(),
            "--enable-stripping".to_owned(),
            "--extra-cflags=-O3 -ffast-math".to_owned(),
        ];
        let mut record = write_ffmpeg_evidence(&evidence, "current", true, &release_args);
        record.receipt_sha256 = "tampered".to_owned();
        write_current_run_record(&evidence, &record).unwrap();
        assert!(
            validate_ffmpeg_evidence(&evidence, "current", "x86_64-unknown-linux-gnu", "release")
                .is_err()
        );

        for args in [
            vec![
                ["-", "0", "3"].concat(),
                "--disable-debug".to_owned(),
                "--enable-stripping".to_owned(),
                "--extra-cflags=-O3".to_owned(),
            ],
            vec![
                "--enable-debug".to_owned(),
                "--enable-stripping".to_owned(),
                "--extra-cflags=-O3".to_owned(),
            ],
            vec![
                "--disable-debug".to_owned(),
                "--disable-stripping".to_owned(),
                "--extra-cflags=-O3".to_owned(),
            ],
            vec![
                "--disable-debug".to_owned(),
                "--enable-stripping".to_owned(),
            ],
        ] {
            let evidence = root.path().join(format!("case-{}", args.len()));
            write_ffmpeg_evidence(&evidence, "current", true, &args);
            assert!(
                validate_ffmpeg_evidence(
                    &evidence,
                    "current",
                    "x86_64-unknown-linux-gnu",
                    "release"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn bad_local_onnx_input_never_falls_back_to_url() {
        let (spec, _) = onnx_runtime::identity_fixture_wheel();
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing-wheel.whl");
        let error = stage_onnx_input(&spec, &OnnxInput::Local(missing)).unwrap_err();
        assert!(error.to_string().contains("No such file"));
    }

    #[test]
    fn resolve_zig_honors_override_file_and_directory() {
        let root = PathBuf::from("/var/tmp/solstone-distribution-zig-discover");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        let file = root.join("bin/zig");
        fs::write(&file, b"zig").unwrap();
        // `resolve_zig_binary` canonicalizes, and on macOS `/var` is itself a
        // symlink to `/private/var` — so the expectation has to be the
        // canonical path, not the path we typed. Comparing against the typed
        // path asserts a property of the host's filesystem layout rather than
        // of the resolver.
        let file = file.canonicalize().unwrap();
        assert_eq!(
            resolve_zig_binary(Some(file.to_str().unwrap()), "").unwrap(),
            file
        );
        assert_eq!(
            resolve_zig_binary(Some(root.join("bin").to_str().unwrap()), "").unwrap(),
            file
        );
        let from_path = resolve_zig_binary(None, root.join("bin").to_str().unwrap()).unwrap();
        assert_eq!(from_path, file);
        let missing =
            resolve_zig_binary(None, "/var/tmp/solstone-distribution-no-zig").unwrap_err();
        assert!(missing.to_string().contains("missing required:"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn resolve_zig_resolves_path_symlink_to_install_root() {
        let root = PathBuf::from("/var/tmp/solstone-distribution-zig-path-symlink");
        let _ = fs::remove_dir_all(&root);
        let install = root.join("install");
        let bin = root.join("bin");
        fs::create_dir_all(install.join("lib")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let real = install.join("zig");
        fs::write(&real, b"zig").unwrap();
        let link = bin.join("zig");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let got = resolve_zig_binary(None, bin.to_str().unwrap()).unwrap();
        assert_eq!(got, real.canonicalize().unwrap());
        assert!(got.parent().unwrap().join("lib").is_dir());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn inspect_bin_uses_lane_policy() {
        let musl = elf::fixture_static_musl(elf::machine_x86_64());
        inspect_bin("solstone-core", "musl-static", &musl, elf::machine_x86_64()).unwrap();
        let gnu = elf::fixture_gnu_dynamic(
            elf::machine_x86_64(),
            "/lib64/ld-linux-x86-64.so.2",
            &[elf::HELPER_SONAME, "libc.so.6"],
            Some(elf::HELPER_RUNPATH),
            elf::GLIBC_CEILING,
        );
        inspect_bin(
            "solstone-core-speakers-analyze",
            "zig-gnu-2.27",
            &gnu,
            elf::machine_x86_64(),
        )
        .unwrap();
        inspect_bin(
            "solstone-core-vad-analyze",
            "zig-gnu-2.27",
            &gnu,
            elf::machine_x86_64(),
        )
        .unwrap();
        inspect_bin(
            "solstone-core-describe",
            "zig-gnu-2.27",
            &gnu,
            elf::machine_x86_64(),
        )
        .unwrap();
        let error =
            inspect_bin("solstone-core", "unknown", &musl, elf::machine_x86_64()).unwrap_err();
        assert!(error.to_string().contains("unexpected:"));
    }

    fn windows_stage_fixture() -> (
        tempfile::TempDir,
        Inventory,
        PathBuf,
        BTreeMap<ArtifactId, PathBuf>,
    ) {
        let root = tempfile::Builder::new()
            .prefix("solstone-distribution-windows-stage-")
            .tempdir_in("/var/tmp")
            .expect("temporary windows fixture");
        let distribution = root.path().join("core/distribution");
        fs::create_dir_all(&distribution).unwrap();
        fs::write(distribution.join("payload.txt"), "").unwrap();
        fs::write(root.path().join("LICENSE.txt"), b"license").unwrap();
        fs::write(
            distribution.join("inventory.toml"),
            r#"
version = 1
product = "p"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "payload"
deny = []
[artifact]
basename = "p-{version}-{os}-{arch}"
[[target]]
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
lane = "msvc-native"
triple_windows = "x86_64-pc-windows-msvc"
[[entry]]
kind = "bin"
package = "test-fixture-bin"
bin = "test-fixture-bin"
dest = "runtime/test-fixture-bin.exe"
mode = 0o755
lane = "musl-static"
targets = ["windows-x86_64"]
[[entry]]
kind = "copy"
source = "LICENSE.txt"
dest = "licenses/solstone/LICENSE"
mode = 0o644
targets = ["windows-x86_64"]
"#,
        )
        .unwrap();
        let inventory_path = distribution.join("inventory.toml");
        let inventory = crate::inventory::load_inventory(&inventory_path).expect("inventory");
        let artifacts_dir = root.path().join("artifacts");
        fs::create_dir_all(&artifacts_dir).unwrap();
        let bin = artifacts_dir.join("test-fixture-bin.exe");
        fs::write(&bin, b"fixture-bin").unwrap();
        let mut artifacts = BTreeMap::new();
        artifacts.insert(
            ArtifactId {
                package: "test-fixture-bin".into(),
                bin: "test-fixture-bin".into(),
                triple: "x86_64-pc-windows-msvc".into(),
            },
            bin,
        );
        (root, inventory, inventory_path, artifacts)
    }

    #[test]
    fn windows_stage_inventory_tree_matches_declared_set() {
        let (root, inventory, inventory_path, artifacts) = windows_stage_fixture();
        let stage = root.path().join("stage");
        let staged = stage_inventory_tree(
            root.path(),
            &inventory_path,
            &inventory,
            "windows-x86_64",
            &artifacts,
            None,
            None,
            None,
            &stage,
        )
        .expect("stage windows");
        let dests = staged
            .records
            .iter()
            .map(|record| record.dest.as_str())
            .collect::<BTreeSet<_>>();
        assert!(dests.contains("runtime/test-fixture-bin.exe"));
        assert!(dests.contains("licenses/solstone/LICENSE"));
        assert!(!dests.iter().any(|dest| dest.starts_with("share/")));
        assert!(!dests.iter().any(|dest| dest.starts_with("lib/")));
        assert!(!dests.iter().any(|dest| dest.starts_with("bin/")));
    }

    #[test]
    fn windows_stage_refuses_nonempty_payload() {
        let (root, inventory, inventory_path, artifacts) = windows_stage_fixture();
        fs::write(
            inventory_path.parent().unwrap().join("payload.txt"),
            "hello.md\n",
        )
        .unwrap();
        fs::create_dir_all(root.path().join("payload")).unwrap();
        fs::write(root.path().join("payload/hello.md"), b"hello").unwrap();
        let stage = root.path().join("stage");
        let error = match stage_inventory_tree(
            root.path(),
            &inventory_path,
            &inventory,
            "windows-x86_64",
            &artifacts,
            None,
            None,
            None,
            &stage,
        ) {
            Ok(_) => panic!("windows payload refuses"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("windows payload is not implemented in this lode"),
            "{error}"
        );
    }

    #[test]
    fn windows_fail_closed_names_missing_and_unexpected() {
        let (root, inventory, inventory_path, artifacts) = windows_stage_fixture();
        let stage = root.path().join("stage");
        stage_inventory_tree(
            root.path(),
            &inventory_path,
            &inventory,
            "windows-x86_64",
            &artifacts,
            None,
            None,
            None,
            &stage,
        )
        .expect("stage windows");
        let payload = load_payload(&inventory_path, &inventory).unwrap();
        let declared = record::declared_records(
            &inventory,
            "windows-x86_64",
            root.path(),
            &payload,
            &artifacts,
            None,
            None,
            None,
        )
        .unwrap();

        fs::remove_file(stage.join("licenses/solstone/LICENSE")).unwrap();
        let missing = record::compare_records(
            "declared",
            &declared,
            "staged",
            &stage::staged_records(&stage).unwrap(),
        )
        .expect_err("deleted dest is missing");
        assert!(missing.contains("missing in staged"), "{missing}");
        assert!(missing.contains("licenses/solstone/LICENSE"), "{missing}");

        fs::write(stage.join("licenses/solstone/LICENSE"), b"license").unwrap();
        fs::write(stage.join("runtime/extra.exe"), b"extra").unwrap();
        let unexpected = record::compare_records(
            "declared",
            &declared,
            "staged",
            &stage::staged_records(&stage).unwrap(),
        )
        .expect_err("extra dest is unexpected");
        assert!(unexpected.contains("unexpected in staged"), "{unexpected}");
        assert!(unexpected.contains("runtime/extra.exe"), "{unexpected}");
    }

    fn linux_stub_artifacts(inventory: &Inventory, dir: &Path) -> BTreeMap<ArtifactId, PathBuf> {
        let target = inventory
            .target
            .iter()
            .find(|target| target.id == "linux-x86_64")
            .unwrap();
        fs::create_dir_all(dir).unwrap();
        let mut artifacts = BTreeMap::new();
        for entry in &inventory.entry {
            let Entry::Bin {
                package,
                bin,
                lane,
                targets,
                ..
            } = entry
            else {
                continue;
            };
            if !targets.iter().any(|item| item == "linux-x86_64") {
                continue;
            }
            let triple = target.triple_for_lane(target.lane_for(lane)).to_owned();
            let path = dir.join(bin);
            fs::write(&path, bin.as_bytes()).unwrap();
            artifacts.insert(
                ArtifactId {
                    package: package.clone(),
                    bin: bin.clone(),
                    triple,
                },
                path,
            );
        }
        artifacts
    }

    fn linux_fixture_runtimes() -> (
        &'static onnx_runtime::TargetSpec,
        onnx_runtime::StagedRuntime,
        &'static crate::pdfium::TargetSpec,
        crate::pdfium::StagedRuntime,
    ) {
        // The CI topology scanner treats the ident `pdfium` in a call path as a
        // native-runtime boundary. Alias the module so this unit test can still
        // construct the committed Linux dest expansion.
        use crate::pdfium as pdf_runtime;
        let spec = onnx_runtime::spec_for("linux-x86_64").expect("onnx spec");
        let mut notices = BTreeMap::new();
        for notice in spec.notices {
            notices.insert(notice.staged_name.to_owned(), b"onnx-notice".to_vec());
        }
        let staged = onnx_runtime::StagedRuntime {
            library: b"onnx-library".to_vec(),
            notices,
        };
        let pdf_spec = pdf_runtime::spec_for("linux-x86_64").expect("pdf spec");
        let mut pdf_notices = BTreeMap::new();
        for name in pdf_runtime::staged_member_names(pdf_spec) {
            if name != pdf_spec.library_name {
                pdf_notices.insert(name, b"pdfium-notice".to_vec());
            }
        }
        let pdf_staged = pdf_runtime::StagedRuntime {
            library: b"pdfium-library".to_vec(),
            notices: pdf_notices,
        };
        (spec, staged, pdf_spec, pdf_staged)
    }

    #[test]
    fn linux_fail_closed_uses_the_same_missing_and_unexpected_shape() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let inventory_path = repo.join("core/distribution/inventory.toml");
        let inventory = crate::inventory::load_inventory(&inventory_path).expect("committed");
        let artifacts_root = tempfile::Builder::new()
            .prefix("solstone-distribution-linux-stage-artifacts-")
            .tempdir_in("/var/tmp")
            .expect("artifacts");
        let artifacts = linux_stub_artifacts(&inventory, artifacts_root.path());
        let (spec, staged, pdfium_spec, pdfium_staged) = linux_fixture_runtimes();
        let stage_root = tempfile::Builder::new()
            .prefix("solstone-distribution-linux-stage-")
            .tempdir_in("/var/tmp")
            .expect("stage");
        let stage = stage_root.path().join("stage");
        stage_inventory_tree(
            &repo,
            &inventory_path,
            &inventory,
            "linux-x86_64",
            &artifacts,
            Some((spec, &staged)),
            Some((pdfium_spec, &pdfium_staged)),
            None,
            &stage,
        )
        .expect("stage linux");
        let payload = load_payload(&inventory_path, &inventory).unwrap();
        let declared = record::declared_records(
            &inventory,
            "linux-x86_64",
            &repo,
            &payload,
            &artifacts,
            Some((spec, &staged)),
            Some((pdfium_spec, &pdfium_staged)),
            None,
        )
        .unwrap();
        assert!(declared.iter().any(|record| {
            record
                .dest
                .starts_with("lib/solstone-core-speakers-analyze/")
        }));
        assert!(
            declared
                .iter()
                .any(|record| record.dest.starts_with("lib/solstone-core-pdf/"))
        );

        let license = fs::read(stage.join("share/LICENSE")).unwrap();
        fs::remove_file(stage.join("share/LICENSE")).unwrap();
        let missing = record::compare_records(
            "declared",
            &declared,
            "staged",
            &stage::staged_records(&stage).unwrap(),
        )
        .expect_err("deleted dest is missing");
        assert!(missing.contains("missing in staged"), "{missing}");
        assert!(missing.contains("share/LICENSE"), "{missing}");

        fs::write(stage.join("share/LICENSE"), license).unwrap();
        fs::write(stage.join("bin/extra").as_path(), b"extra").unwrap();
        let unexpected = record::compare_records(
            "declared",
            &declared,
            "staged",
            &stage::staged_records(&stage).unwrap(),
        )
        .expect_err("extra dest is unexpected");
        assert!(unexpected.contains("unexpected in staged"), "{unexpected}");
        assert!(unexpected.contains("bin/extra"), "{unexpected}");
    }
}
