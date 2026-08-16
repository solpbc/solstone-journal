// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::digest::sha256_hex;
use crate::elf;
use crate::inventory::{Entry, Inventory, format_named_list, load_payload};
use crate::lanes::{self, write_wrappers};
use crate::onnx_runtime;
use crate::promote::{PromoteRequest, isolated_target_dir, promote};
use crate::provenance::{self, Provenance, bind_cargo_json, lock_digest};
use crate::select::{self, ArtifactId};
use crate::stage;

pub const ZIG_OVERRIDE: &str = "SOLSTONE_ZIG";

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
    pub artifacts: Vec<PathBuf>,
}

pub fn resolve_zig_binary(
    override_value: Option<&str>,
    path: &str,
) -> Result<PathBuf, ProduceError> {
    if let Some(value) = override_value {
        let candidate = PathBuf::from(value);
        if candidate.is_file() {
            return Ok(candidate);
        }
        let nested = candidate.join("zig");
        if nested.is_file() {
            return Ok(nested);
        }
        return Err(ProduceError::new(format!(
            "missing required:\n  zig ({ZIG_OVERRIDE}={value})"
        )));
    }
    for entry in env::split_paths(path) {
        let candidate = entry.join("zig");
        if candidate.is_file() {
            return Ok(candidate);
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

pub fn digest_const_hex(source: &str, name: &str) -> Option<String> {
    let mut pending: Option<&str> = None;
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pub const ") {
            let Some((const_name, after)) = rest.split_once(':') else {
                continue;
            };
            if !after.contains("&str") {
                continue;
            }
            if const_name.trim() != name {
                pending = None;
                continue;
            }
            if let Some((_, literal)) = trimmed.split_once('=') {
                let hex = literal
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .trim_matches('"');
                if hex.len() == 64 {
                    return Some(hex.to_owned());
                }
            }
            pending = Some(name);
            continue;
        }
        if pending.take() == Some(name) {
            let hex = trimmed.trim_end_matches(';').trim().trim_matches('"');
            if hex.len() == 64 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Some(hex.to_owned());
            }
        }
    }
    None
}

pub fn inspect_bin(bin: &str, lane: &str, bytes: &[u8], machine: u16) -> Result<(), ProduceError> {
    let info = elf::parse_elf(bytes).map_err(|error| ProduceError::new(error.to_string()))?;
    match lane {
        "musl-static" => elf::inspect_core_family(&info, machine),
        "zig-gnu-2.27" if bin == "solstone-core-speakers-analyze" => elf::inspect_gnu_helper(
            &info,
            machine,
            Some(elf::HELPER_RUNPATH),
            &[elf::HELPER_SONAME],
        ),
        "zig-gnu-2.27" => elf::inspect_gnu_helper(&info, machine, None, &[]),
        other => {
            return Err(ProduceError::new(format!("unexpected:\n  lane {other}")));
        }
    }
    .map_err(|error| ProduceError::new(error.to_string()))
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
    let repo = inventory_path
        .ancestors()
        .nth(3)
        .ok_or_else(|| ProduceError::new("missing required:\n  repository root"))?;

    let commit = git_stdout(repo, &["rev-parse", "HEAD"])?;
    let dirty = !git_stdout(repo, &["status", "--porcelain"])?.is_empty();
    provenance::require_clean(dirty).map_err(|error| ProduceError::new(error.to_string()))?;
    let lock_path = repo.join("core/Cargo.lock");
    let expected_lock =
        lock_digest(&lock_path).map_err(|error| ProduceError::new(error.to_string()))?;
    let version = workspace_version(&repo.join("core/Cargo.toml"))?;
    let epoch = git_stdout(repo, &["show", "-s", "--format=%ct", "HEAD"])?;

    let zig = discover_zig()?;
    let zig_version = command_stdout(&zig, &["version"])?;
    lanes::check_zig_version(&zig_version).map_err(|error| ProduceError::new(error.to_string()))?;
    let zig_lib = zig
        .parent()
        .map(|parent| parent.join("lib"))
        .filter(|path| path.is_dir())
        .ok_or_else(|| ProduceError::new("missing required:\n  zig lib"))?;
    let zig_dir = zig
        .parent()
        .ok_or_else(|| ProduceError::new("missing required:\n  zig"))?;

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
        let staged_runtime = onnx_runtime::stage_from_url(spec)
            .map_err(|error| ProduceError::new(error.to_string()))?;
        onnx_runtime::write_staged_runtime(spec, &staged_runtime, &onnx_dir)
            .map_err(|error| ProduceError::new(error.to_string()))?;

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
        let gnu_env = lanes::gnu_lane_env(
            target,
            &wrappers,
            &zig_lib,
            &checkout,
            Some(&onnx_dir),
            &host,
        )
        .map_err(|error| ProduceError::new(error.to_string()))?;
        write_wrappers(&musl_env).map_err(|error| ProduceError::new(error.to_string()))?;
        write_wrappers(&gnu_env).map_err(|error| ProduceError::new(error.to_string()))?;

        let remap = [
            format!("--remap-path-prefix={}=/source", checkout.display()),
            format!("--remap-path-prefix={}=/target", target_dir.display()),
            format!("--remap-path-prefix={sysroot}=/rustc"),
        ];
        let musl_rustflags = [
            remap[0].clone(),
            remap[1].clone(),
            remap[2].clone(),
            "-C".to_owned(),
            "link-arg=--build-id=none".to_owned(),
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

        let mut artifacts = BTreeMap::new();
        let musl_bins = bins_for_lane(&inventory, &args.target_id, "musl-static");
        let gnu_bins = bins_for_lane(&inventory, &args.target_id, "zig-gnu-2.27");
        merge_artifacts(
            &mut artifacts,
            build_lane(BuildLane {
                checkout: &checkout,
                target_dir: &target_dir,
                zig_dir,
                wrapper_dir: &wrappers,
                triple: &target.triple_musl,
                bins: &musl_bins,
                vars: &musl_env.vars,
                rustflags: &musl_rustflags,
                epoch: &epoch,
            })?,
        );
        merge_artifacts(
            &mut artifacts,
            build_lane(BuildLane {
                checkout: &checkout,
                target_dir: &target_dir,
                zig_dir,
                wrapper_dir: &wrappers,
                triple: &target.triple_gnu,
                bins: &gnu_bins,
                vars: &gnu_env.vars,
                rustflags: &gnu_rustflags,
                epoch: &epoch,
            })?,
        );

        select::refuse_wrong_triple(&inventory, &args.target_id, &artifacts)
            .map_err(|error| ProduceError::new(error.to_string()))?;
        select::refuse_extra(&inventory, &args.target_id, &artifacts)
            .map_err(|error| ProduceError::new(error.to_string()))?;
        let selection = select::select_artifacts(&inventory, &args.target_id, &artifacts)
            .map_err(|error| ProduceError::new(error.to_string()))?;

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

        let stage = work.join("stage");
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir_all(&stage)?;
        select::stage_selected(&selection, &stage)?;
        stage_layout(
            &checkout,
            &inventory_path,
            &inventory,
            &args.target_id,
            spec,
            &staged_runtime,
            &stage,
        )?;

        let tree = tree_from_stage(&stage)?;
        let observed_lock = lock_digest(&checkout.join("core/Cargo.lock"))
            .map_err(|error| ProduceError::new(error.to_string()))?;
        let observed_commit = git_stdout(&checkout, &["rev-parse", "HEAD"])?;
        if let Some(parent) = args.dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let dest = args.dest.clone();
        let produced = promote(&PromoteRequest {
            dest,
            work: work.join("promote"),
            tree,
            version,
            arch: args.target_id.clone(),
            deb_arch: target.deb_arch.clone(),
            rpm_arch: target.rpm_arch.clone(),
            dirty: false,
            observed: Provenance {
                commit: observed_commit,
                lock_sha256: observed_lock,
            },
            expected: Provenance {
                commit: commit.clone(),
                lock_sha256: expected_lock.clone(),
            },
            fail_after: None,
        })
        .map_err(|error| ProduceError::new(error.to_string()))?;

        let artifacts = [
            "tree.tar.gz",
            "tree.deb",
            "tree.rpm",
            ".sha256",
            ".manifest.json",
            ".release",
        ]
        .into_iter()
        .map(|name| produced.join(name))
        .collect::<Vec<_>>();
        for path in &artifacts {
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
            commit,
            lock_sha256: expected_lock,
            artifacts,
        })
    })();

    let _ = git_run(
        repo,
        &["worktree", "remove", "--force", &checkout.to_string_lossy()],
    );
    let _ = fs::remove_dir_all(&checkout);
    result
}

fn bins_for_lane(inventory: &Inventory, target_id: &str, lane: &str) -> Vec<(String, String)> {
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
            } if entry_lane == lane && targets.iter().any(|item| item == target_id) => {
                Some((package.clone(), bin.clone()))
            }
            _ => None,
        })
        .collect()
}

struct BuildLane<'a> {
    checkout: &'a Path,
    target_dir: &'a Path,
    zig_dir: &'a Path,
    wrapper_dir: &'a Path,
    triple: &'a str,
    bins: &'a [(String, String)],
    vars: &'a BTreeMap<String, String>,
    rustflags: &'a str,
    epoch: &'a str,
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
        .env(
            "PATH",
            prepend_path(
                lane.wrapper_dir,
                &prepend_path(lane.zig_dir, &env::var("PATH").unwrap_or_default()),
            ),
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
    for (key, value) in lane.vars {
        command.env(key, value);
    }
    if let Some((_, ar)) = lane.vars.iter().find(|(key, _)| key.starts_with("AR_")) {
        command.env("AR", ar);
    }
    if let Some((_, ranlib)) = lane.vars.iter().find(|(key, _)| key.starts_with("RANLIB_")) {
        command.env("RANLIB", ranlib);
    }
    if let Some((_, cc)) = lane.vars.iter().find(|(key, _)| key.starts_with("CC_")) {
        command.env("CC", cc);
    }
    if let Some((_, cxx)) = lane.vars.iter().find(|(key, _)| key.starts_with("CXX_")) {
        command.env("CXX", cxx);
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
    bind_cargo_json(&String::from_utf8_lossy(&output.stdout))
        .map_err(|error| ProduceError::new(error.to_string()))
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

fn stage_layout(
    repo: &Path,
    inventory_path: &Path,
    inventory: &Inventory,
    target_id: &str,
    spec: &onnx_runtime::TargetSpec,
    runtime: &onnx_runtime::StagedRuntime,
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
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                let bytes = fs::read(repo.join(source))?;
                let expected = digest_const_hex(
                    &fs::read_to_string(
                        repo.join("core/crates/solstone-core-transcribe/src/model_assets.rs"),
                    )?,
                    digest_const,
                )
                .ok_or_else(|| {
                    ProduceError::new(format!("missing required:\n  digest {digest_const}"))
                })?;
                let actual = sha256_hex(&bytes);
                if actual != expected {
                    return Err(ProduceError::new(format!(
                        "unexpected:\n  {dest} digest {actual}"
                    )));
                }
                stage::write_staged_file_mode(stage, dest, &bytes, *mode)?;
            }
            Entry::OnnxRuntime {
                dest_dir,
                mode: _,
                targets,
            } => {
                if !targets.iter().any(|item| item == target_id) {
                    continue;
                }
                onnx_runtime::write_staged_runtime(spec, runtime, &stage.join(dest_dir))
                    .map_err(|error| ProduceError::new(error.to_string()))?;
            }
        }
    }
    for source in load_payload(inventory_path, inventory)
        .map_err(|error| ProduceError::new(error.to_string()))?
    {
        let dest = payload_dest(&inventory.payload_dest_prefix, &source);
        let bytes = fs::read(repo.join(&source))?;
        stage::write_staged_file_mode(stage, &dest, &bytes, 0o644)?;
    }
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
    fn resolve_zig_honors_override_file_and_directory() {
        let root = PathBuf::from("/var/tmp/solstone-distribution-zig-discover");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        let file = root.join("bin/zig");
        fs::write(&file, b"zig").unwrap();
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
}
