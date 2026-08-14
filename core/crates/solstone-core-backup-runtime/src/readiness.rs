// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::runner::{ToolRunner, run_restic};

pub const RESTIC_VERSION: &str = "0.19.0";
pub const RESTIC_SCHEMA_VERSION: u64 = 1;
pub const RESTIC_TOOL: &str = "restic";
pub const RESTIC_BUNDLE_ENV: &str = "SOLSTONE_RESTIC_BUNDLE";
pub const MAC_TOOL_DIR: &str = "Library/Application Support/solstone/restic";
pub const LINUX_TOOL_DIR: &str = ".cache/solstone/restic";
pub const ARCH_ALIASES: &[(&str, &str)] = &[
    ("x86_64", "amd64"),
    ("amd64", "amd64"),
    ("arm64", "arm64"),
    ("aarch64", "arm64"),
];
pub const RESTIC_BZ2_SHA256: [(&str, &str); 4] = [
    (
        "restic_0.19.0_darwin_amd64.bz2",
        "c9d9a71234bc0955fdba6da93cc9375f8793ec1e1cbce77a91014d536a969148",
    ),
    (
        "restic_0.19.0_darwin_arm64.bz2",
        "1475397bf759ef4be16a77b19dec650bdbfec00d2cacd82005553411cdd37997",
    ),
    (
        "restic_0.19.0_linux_amd64.bz2",
        "13176fe6d89d4357947a2cd107218ab2873a5f9d8e1ac2d4cd1c8e07e6839c21",
    ),
    (
        "restic_0.19.0_linux_arm64.bz2",
        "e522ce6bf748d753fee8093e8ec59359972cf5b6bc65fc7c7cf38ae952351d91",
    ),
];
pub const RESTIC_GITHUB_URL_TEMPLATE: &str =
    "https://github.com/restic/restic/releases/download/v0.19.0/restic_0.19.0_{os}_{arch}.bz2";

pub fn platform_info() -> Result<(String, String), String> {
    let os = match env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => other,
    };
    let machine = env::consts::ARCH;
    let arch = ARCH_ALIASES
        .iter()
        .find(|(raw, _)| *raw == machine)
        .map(|(_, normalized)| (*normalized).to_owned());
    match (os, arch) {
        ("darwin" | "linux", Some(arch)) => Ok((os.to_owned(), arch)),
        _ => Err(format!(
            "restic unsupported platform: {os}/{machine}; supported: darwin|linux on amd64|arm64"
        )),
    }
}

pub fn select_restic_asset(
    os: Option<&str>,
    arch: Option<&str>,
) -> Result<(String, String, String), String> {
    let (os, arch) = match (os, arch) {
        (Some(os), Some(arch)) => (os.to_owned(), arch.to_lowercase()),
        _ => platform_info()?,
    };
    let normalized = ARCH_ALIASES
        .iter()
        .find(|(raw, _)| *raw == arch)
        .map(|(_, normalized)| *normalized)
        .ok_or_else(|| {
            format!(
                "restic unsupported platform: {os}/{arch}; supported: darwin|linux on amd64|arm64"
            )
        })?;
    if !matches!(os.as_str(), "darwin" | "linux") {
        return Err(format!(
            "restic unsupported platform: {os}/{arch}; supported: darwin|linux on amd64|arm64"
        ));
    }
    let filename = format!("restic_{RESTIC_VERSION}_{os}_{normalized}.bz2");
    let sha = RESTIC_BZ2_SHA256
        .iter()
        .find(|(name, _)| *name == filename)
        .map(|(_, digest)| *digest)
        .expect("asset matrix complete");
    Ok((
        filename,
        RESTIC_GITHUB_URL_TEMPLATE
            .replace("{os}", &os)
            .replace("{arch}", normalized),
        sha.to_owned(),
    ))
}

pub fn tool_dir(os: &str) -> Result<PathBuf, String> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())?;
    match os {
        "darwin" => Ok(home.join(MAC_TOOL_DIR)),
        "linux" => Ok(home.join(LINUX_TOOL_DIR)),
        _ => Err(format!("restic unsupported platform: {os}")),
    }
}
pub fn binary_path(tool_dir: &Path) -> PathBuf {
    tool_dir.join(RESTIC_TOOL)
}
pub fn sentinel_path(tool_dir: &Path) -> PathBuf {
    tool_dir.join(".install-complete")
}
pub fn license_path(tool_dir: &Path) -> PathBuf {
    tool_dir.join("restic.LICENSE")
}
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn check_restic_ready_with(
    runner: &dyn ToolRunner,
    requested: Option<&Path>,
) -> Option<PathBuf> {
    let (os, arch) = platform_info().ok()?;
    let tool_dir = requested
        .map(Path::to_path_buf)
        .unwrap_or(tool_dir(&os).ok()?);
    let binary = binary_path(&tool_dir);
    let payload: serde_json::Value =
        serde_json::from_slice(&fs::read(sentinel_path(&tool_dir)).ok()?).ok()?;
    let expected = payload.get("sha256")?.as_str()?;
    if payload.get("schema_version") != Some(&serde_json::Value::from(RESTIC_SCHEMA_VERSION))
        || payload.get("tool") != Some(&serde_json::Value::from(RESTIC_TOOL))
        || payload.get("version") != Some(&serde_json::Value::from(RESTIC_VERSION))
        || payload.pointer("/platform/os")?.as_str()? != os
        || payload.pointer("/platform/arch")?.as_str()? != arch
        || payload.get("binary_path")?.as_str()? != binary.to_string_lossy()
    {
        return None;
    }
    if !binary.is_file() || file_sha256(&binary).ok()?.as_str() != expected {
        return None;
    }
    let result = run_restic(
        runner,
        &["version".into()],
        "unused",
        "unused",
        &binary,
        Some(&BTreeMap::new()),
        false,
        None,
        Some(Duration::from_secs(10)),
        &[],
    )
    .ok()?;
    (result.returncode == 0 && result.stdout.contains(&format!("restic {RESTIC_VERSION}")))
        .then_some(binary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{ToolOutput, ToolRequest};
    use std::io;

    struct VersionRunner {
        output: ToolOutput,
    }
    impl ToolRunner for VersionRunner {
        fn run(&self, _: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            Ok(self.output.clone())
        }
    }

    #[test]
    fn restic_asset_matrix_matches_every_pin_and_alias() {
        for (filename, digest) in RESTIC_BZ2_SHA256 {
            let parts = filename
                .trim_end_matches(".bz2")
                .split('_')
                .collect::<Vec<_>>();
            let (selected, url, actual) =
                select_restic_asset(Some(parts[2]), Some(parts[3])).unwrap();
            assert_eq!(selected, filename);
            assert_eq!(actual, digest);
            assert!(url.ends_with(filename));
        }
        assert_eq!(
            select_restic_asset(Some("linux"), Some("x86_64"))
                .unwrap()
                .0,
            "restic_0.19.0_linux_amd64.bz2"
        );
        assert!(select_restic_asset(Some("windows"), Some("amd64")).is_err());
    }

    #[test]
    fn readiness_requires_the_pinned_version_probe() {
        let directory = tempfile::tempdir().unwrap();
        let binary = binary_path(directory.path());
        fs::write(&binary, b"fixture").unwrap();
        let digest = file_sha256(&binary).unwrap();
        let (os, arch) = platform_info().unwrap();
        fs::write(sentinel_path(directory.path()), serde_json::to_vec(&serde_json::json!({"schema_version":RESTIC_SCHEMA_VERSION,"tool":RESTIC_TOOL,"version":RESTIC_VERSION,"sha256":digest,"platform":{"os":os,"arch":arch},"binary_path":binary})).unwrap()).unwrap();
        let runner = VersionRunner {
            output: ToolOutput {
                returncode: 0,
                stdout: b"restic 0.18.0".to_vec(),
                stderr: vec![],
            },
        };
        assert_eq!(
            check_restic_ready_with(&runner, Some(directory.path())),
            None
        );
    }
}
