// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::inventory::Target;

pub const REQUIRED_ZIG: &str = "0.16.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneEnv {
    pub vars: BTreeMap<String, String>,
    pub wrappers: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct LaneError {
    pub message: String,
}

impl LaneError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LaneError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LaneError {}

pub fn check_zig_version(version: &str) -> Result<(), LaneError> {
    let trimmed = version.trim();
    if trimmed == REQUIRED_ZIG {
        return Ok(());
    }
    if trimmed.is_empty() {
        return Err(LaneError::new("missing required:\n  zig"));
    }
    Err(LaneError::new(format!(
        "unexpected:\n  zig {trimmed} (want {REQUIRED_ZIG})"
    )))
}

pub fn env_target(triple: &str) -> String {
    triple.replace('-', "_")
}

pub fn describe_cc_key() -> &'static str {
    "cc"
}

/// musl-static lane: zig `*-linux-musl` wrappers. No global RUSTFLAGS.
pub fn musl_lane_env(
    target: &Target,
    wrapper_dir: &Path,
    host_triple: &str,
) -> Result<LaneEnv, LaneError> {
    let zig_target = format!("{}-linux-musl", target.arch);
    lane_env(
        &target.triple_musl,
        &zig_target,
        wrapper_dir,
        host_triple,
        LaneOptional::default(),
    )
}

pub fn gnu_lane_env(
    target: &Target,
    wrapper_dir: &Path,
    zig_lib: &Path,
    repo: &Path,
    helper_lib: Option<&Path>,
    host_triple: &str,
) -> Result<LaneEnv, LaneError> {
    let include = repo.join("core/crates/solstone-core-describe/build-support/zig-glibc");
    lane_env(
        &target.triple_gnu,
        &target.zig_gnu,
        wrapper_dir,
        host_triple,
        LaneOptional {
            bindgen: Some(bindgen_args(target, zig_lib)),
            describe_include: Some(include),
            helper_lib,
            zig_lib: Some(zig_lib),
        },
    )
}

#[derive(Default)]
struct LaneOptional<'a> {
    bindgen: Option<String>,
    describe_include: Option<PathBuf>,
    helper_lib: Option<&'a Path>,
    zig_lib: Option<&'a Path>,
}

fn lane_env(
    triple: &str,
    zig_target: &str,
    wrapper_dir: &Path,
    host_triple: &str,
    optional: LaneOptional<'_>,
) -> Result<LaneEnv, LaneError> {
    let env_target = env_target(triple);
    let env_upper = env_target.to_uppercase();
    let cc_wrapper = wrapper_dir.join(format!("{triple}-gcc"));
    let cxx_wrapper = wrapper_dir.join(format!("{triple}-g++"));
    let ar_wrapper = wrapper_dir.join(format!("{triple}-ar"));
    let ranlib_wrapper = wrapper_dir.join(format!("{triple}-ranlib"));
    let mut wrappers = BTreeMap::new();
    wrappers.insert(
        cc_wrapper.display().to_string(),
        wrapper_script("cc", zig_target),
    );
    wrappers.insert(
        cxx_wrapper.display().to_string(),
        wrapper_script("c++", zig_target),
    );
    wrappers.insert(
        ar_wrapper.display().to_string(),
        wrapper_script("ar", zig_target),
    );
    wrappers.insert(
        ranlib_wrapper.display().to_string(),
        wrapper_script("ranlib", zig_target),
    );

    let mut vars = BTreeMap::new();
    vars.insert(
        format!("CARGO_TARGET_{env_upper}_LINKER"),
        cc_wrapper.display().to_string(),
    );
    vars.insert(format!("CC_{env_target}"), cc_wrapper.display().to_string());
    vars.insert(
        format!("CXX_{env_target}"),
        cxx_wrapper.display().to_string(),
    );
    vars.insert(format!("AR_{env_target}"), ar_wrapper.display().to_string());
    vars.insert(
        format!("RANLIB_{env_target}"),
        ranlib_wrapper.display().to_string(),
    );
    if let Some(zig_lib) = optional.zig_lib {
        vars.insert("ZIG_LIB_DIR".to_owned(), zig_lib.display().to_string());
    }
    if let Some(bindgen) = optional.bindgen {
        vars.insert(format!("BINDGEN_EXTRA_CLANG_ARGS_{env_target}"), bindgen);
    }
    if let Some(include) = optional.describe_include {
        vars.insert(
            format!("CFLAGS_{env_target}"),
            format!("-I{}", include.display()),
        );
        vars.insert(
            describe_cc_key().to_owned(),
            format!("zig cc -target {zig_target} -I{}", include.display()),
        );
    }
    if let Some(helper_lib) = optional.helper_lib {
        vars.insert("ORT_PREFER_DYNAMIC_LINK".to_owned(), "true".to_owned());
        vars.insert("ORT_LIB_PATH".to_owned(), helper_lib.display().to_string());
    }
    if host_triple == triple {
        vars.insert(
            "CARGO_UNSTABLE_TARGET_APPLIES_TO_HOST".to_owned(),
            "true".to_owned(),
        );
        vars.insert(
            "CARGO_TARGET_APPLIES_TO_HOST".to_owned(),
            "false".to_owned(),
        );
        vars.insert(
            "__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS".to_owned(),
            "nightly".to_owned(),
        );
    }
    Ok(LaneEnv { vars, wrappers })
}

fn wrapper_script(command: &str, zig_target: &str) -> String {
    match command {
        "ar" | "ranlib" => format!("#!/bin/sh\nexec zig {command} \"$@\"\n"),
        _ => format!(
            r#"#!/bin/sh
skip=
n=0
for arg in "$@"; do
  if [ -n "$skip" ]; then
    skip=
    continue
  fi
  case "$arg" in
    --target=*|-target=*) continue ;;
    --target|-target) skip=1; continue ;;
  esac
  n=$((n+1))
  eval "a$n=\$arg"
done
set --
i=1
while [ "$i" -le "$n" ]; do
  eval "set -- \"\$@\" \"\$a$i\""
  i=$((i+1))
done
exec zig {command} -g -fno-sanitize=all -target {zig_target} "$@"
"#
        ),
    }
}

fn bindgen_args(target: &Target, zig_lib: &Path) -> String {
    let arch_inc = match target.arch.as_str() {
        "x86_64" => "x86-linux-gnu",
        _ => "aarch64-linux-gnu",
    };
    let arch_any = match target.arch.as_str() {
        "x86_64" => "x86-linux-any",
        _ => "aarch64-linux-any",
    };
    let lib = zig_lib.display();
    format!(
        "-nostdinc --target={} -isystem {lib}/include -isystem {lib}/libc/include/{arch_inc} -isystem {lib}/libc/include/generic-glibc -isystem {lib}/libc/include/{arch_any} -isystem {lib}/libc/include/any-linux-any",
        target.triple_gnu
    )
}

pub fn write_wrappers(env: &LaneEnv) -> Result<(), LaneError> {
    for (path, body) in &env.wrappers {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| LaneError::new(error.to_string()))?;
        }
        std::fs::write(&path, body).map_err(|error| LaneError::new(error.to_string()))?;
        set_executable(&path)?;
    }
    if let Some(dir) = env
        .wrappers
        .keys()
        .next()
        .map(Path::new)
        .and_then(Path::parent)
    {
        write_executable(dir.join("ar"), &wrapper_script("ar", "unused"))?;
        write_executable(dir.join("ranlib"), &wrapper_script("ranlib", "unused"))?;
    }
    Ok(())
}

fn write_executable(path: PathBuf, body: &str) -> Result<(), LaneError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| LaneError::new(error.to_string()))?;
    }
    std::fs::write(&path, body).map_err(|error| LaneError::new(error.to_string()))?;
    set_executable(&path)
}

fn set_executable(path: &Path) -> Result<(), LaneError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .map_err(|error| LaneError::new(error.to_string()))?
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions)
            .map_err(|error| LaneError::new(error.to_string()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
