// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Regenerates the FFmpeg bindings committed under `vendor/ffmpeg-sys-next/bindings`.
//!
//! Ordinary builds install those files instead of running bindgen, so they need
//! neither bindgen nor libclang. Each file is bound to the pinned FFmpeg source
//! digest; after the pin moves, this command rebuilds FFmpeg for every Rust
//! triple a distribution target ships, with the same toolchains `produce` uses,
//! and rewrites that target's files.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{
    FFMPEG_ARCHIVE_OVERRIDE, OFFLINE, OS_LINUX, OS_MACOS, ProduceError, apply_lane_toolchain,
    discover_zig, lanes, musl_bindgen_args, rustc_host, select_ffmpeg_input,
};

const BINDINGS_OUT_ENV: &str = "SOLSTONE_FFMPEG_BINDINGS_OUT";
const IOS_TRIPLE: &str = "aarch64-apple-ios";

/// A one-crate project outside the workspace that asks for exactly the headers the
/// committed bindings cover. Cargo cannot select features of the vendored crate
/// from the workspace, because it is a patched dependency rather than a member.
fn regeneration_manifest(vendor: &Path) -> String {
    format!(
        r#"[package]
name = "solstone-ffmpeg-bindings-regeneration"
version = "0.0.0"
edition = "2024"
publish = false

[lib]
path = "lib.rs"

[dependencies.ffmpeg-sys-next]
path = "{}"
default-features = false
features = ["generate-bindings", "build", "build-portable", "avcodec", "avformat", "swresample", "swscale"]

[workspace]
"#,
        vendor.display()
    )
}

/// Rewrites the committed bindings for every triple of `target_id`, returning the files written.
pub fn run(target_id: &str, start: &Path) -> Result<Vec<PathBuf>, ProduceError> {
    let inventory_path = crate::inventory::repository_inventory_path(start).ok_or_else(|| {
        ProduceError::new(format!(
            "could not find core/distribution/inventory.toml from {}",
            start.display()
        ))
    })?;
    let inventory = crate::validate_distribution_inventory(&inventory_path)
        .map_err(|error| ProduceError::new(error.to_string()))?;
    let target = inventory
        .target
        .iter()
        .find(|item| item.id == target_id)
        .ok_or_else(|| ProduceError::new(format!("missing required:\n  target {target_id}")))?;
    let repo = inventory_path
        .ancestors()
        .nth(3)
        .ok_or_else(|| ProduceError::new("missing required:\n  repository root"))?;
    let archive = select_ffmpeg_input(repo, env::var_os(FFMPEG_ARCHIVE_OVERRIDE).as_deref())?;
    let out = repo.join("core/vendor/ffmpeg-sys-next/bindings");
    let work = repo.join("target/ffmpeg-bindings").join(target_id);
    let wrappers = work.join("wrappers");
    let project = work.join("project");
    // A fresh target directory makes cargo rerun the build script that writes the files.
    let _ = fs::remove_dir_all(work.join("target"));
    fs::create_dir_all(&out)?;
    fs::create_dir_all(&wrappers)?;
    fs::create_dir_all(&project)?;
    fs::write(
        project.join("Cargo.toml"),
        regeneration_manifest(&repo.join("core/vendor/ffmpeg-sys-next").canonicalize()?),
    )?;
    fs::write(project.join("lib.rs"), "")?;
    // Seed the lock from the workspace so every crate resolves to the version it ships with.
    fs::copy(repo.join("core/Cargo.lock"), project.join("Cargo.lock"))?;
    let host = rustc_host()?;

    let mut lanes_to_build: Vec<(String, BTreeMap<String, String>, Option<PathBuf>)> = Vec::new();
    match target.os.as_str() {
        OS_MACOS => {
            let mut vars = BTreeMap::new();
            vars.insert(
                "MACOSX_DEPLOYMENT_TARGET".to_owned(),
                target.min_macos.clone(),
            );
            lanes_to_build.push((target.triple_apple.clone(), vars.clone(), None));
            // The macOS host also checks the workspace for iOS (`make check-rust-ios`),
            // which compiles FFmpeg for that triple, so its bindings travel with the Mac's.
            lanes_to_build.push((IOS_TRIPLE.to_owned(), BTreeMap::new(), None));
        }
        OS_LINUX => {
            let zig = discover_zig()?;
            let zig_dir = zig
                .parent()
                .ok_or_else(|| ProduceError::new("missing required:\n  zig"))?
                .to_path_buf();
            let zig_lib = zig_dir.join("lib");
            let mut musl = lanes::musl_lane_env(target, &wrappers, &host)
                .map_err(|error| ProduceError::new(error.to_string()))?;
            musl.vars.insert(
                format!(
                    "BINDGEN_EXTRA_CLANG_ARGS_{}",
                    lanes::env_target(&target.triple_musl)
                ),
                musl_bindgen_args(target, &zig_lib),
            );
            let mut gnu = lanes::gnu_lane_env(target, &wrappers, &zig_lib, repo, None, &host)
                .map_err(|error| ProduceError::new(error.to_string()))?;
            // As in produce: FFmpeg's configure reads the lane's glibc shim flags from CFLAGS.
            if let Some(cflags) = gnu
                .vars
                .get(&format!("CFLAGS_{}", lanes::env_target(&target.triple_gnu)))
                .cloned()
            {
                gnu.vars.insert("CFLAGS".to_owned(), cflags);
            }
            lanes::write_wrappers(&musl).map_err(|error| ProduceError::new(error.to_string()))?;
            lanes::write_wrappers(&gnu).map_err(|error| ProduceError::new(error.to_string()))?;
            lanes_to_build.push((target.triple_musl.clone(), musl.vars, Some(zig_dir.clone())));
            lanes_to_build.push((target.triple_gnu.clone(), gnu.vars, Some(zig_dir)));
        }
        other => {
            return Err(ProduceError::new(format!(
                "unexpected:\n  bindings are generated by bindgen at build time on {other}"
            )));
        }
    }

    let mut written = Vec::new();
    for (triple, vars, zig_dir) in lanes_to_build {
        let path = out.join(format!("{triple}.rs"));
        let _ = fs::remove_file(&path);
        let mut command = Command::new("cargo");
        command
            .current_dir(&project)
            .env("CARGO_TARGET_DIR", work.join("target"))
            .env("SOLSTONE_FFMPEG_SOURCE_ARCHIVE", &archive)
            .env(OFFLINE, "1")
            .env(BINDINGS_OUT_ENV, &out)
            .args(["build", "--offline", "--release", "--target", &triple]);
        apply_lane_toolchain(
            &mut command,
            &host,
            &triple,
            &vars,
            zig_dir.as_deref(),
            &wrappers,
        );
        let status = command
            .status()
            .map_err(|error| ProduceError::new(format!("cargo: {error}")))?;
        if !status.success() {
            return Err(ProduceError::new(format!(
                "cargo failed generating FFmpeg bindings for {triple}"
            )));
        }
        if !path.is_file() {
            return Err(ProduceError::new(format!(
                "missing required:\n  generated bindings {}",
                path.display()
            )));
        }
        written.push(path);
    }
    Ok(written)
}
