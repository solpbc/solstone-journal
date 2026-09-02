// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Build-time acquisition of digest-pinned distribution inputs.
//!
//! This is the path `ci-full-prep` uses. It is not the owner-facing
//! download primitive: see `solstone_core_artifact_download::BUILDER_INPUT_DOWNLOAD_POLICY`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde::Serialize;
#[cfg(not(windows))]
use solstone_core_artifact_download::{BUILDER_INPUT_DOWNLOAD_POLICY, ensure_verified_url};

use crate::onnx_runtime;
use crate::pdfium;
use crate::stage;

const BUILDER_INPUTS: &str = "core/distribution/builder-inputs.toml";
const ONNX_CACHE: &str = "target/speakers-analyze-runtime-cache";
const ONNX_LINK_ROOT: &str = "target/speakers-analyze-runtime-link";
const PDF_CACHE: &str = "target/pdfium-runtime-cache";
const PDF_LINK_ROOT: &str = "target/pdfium-runtime-link";

#[derive(Debug)]
pub struct AcquireError {
    pub message: String,
}

impl AcquireError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AcquireError {}

impl From<io::Error> for AcquireError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<onnx_runtime::StageError> for AcquireError {
    fn from(error: onnx_runtime::StageError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<pdfium::StageError> for AcquireError {
    fn from(error: pdfium::StageError) -> Self {
        Self::new(error.to_string())
    }
}

#[cfg(not(windows))]
impl From<solstone_core_artifact_download::ArchiveError> for AcquireError {
    fn from(error: solstone_core_artifact_download::ArchiveError) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct BuilderInputsFile {
    schema: String,
    ffmpeg: FetchableInput,
    zig: FetchableInput,
    cmake_windows_x86_64: VersionedFetchableInput,
    #[serde(rename = "rust_std_aarch64_unknown_linux_gnu")]
    rust_std_aarch64_gnu: FetchableInput,
    #[serde(rename = "rust_std_aarch64_unknown_linux_musl")]
    rust_std_aarch64_musl: FetchableInput,
    #[serde(rename = "rust_std_x86_64_unknown_linux_musl")]
    rust_std_x86_64_musl: FetchableInput,
}

#[derive(Debug, Clone, Deserialize)]
struct FetchableInput {
    filename: String,
    url: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct VersionedFetchableInput {
    version: String,
    #[serde(flatten)]
    input: FetchableInput,
}

/// The exact CMake archive admitted into a Windows controlled-build slot.
///
/// Acquisition and the CED receipt path share this one parsed identity so a
/// future builder-input edit cannot silently desynchronize download admission
/// from the bytes recorded beside the produced DLL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsCmakeArchiveInput {
    pub version: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Default)]
struct Flags {
    dest: Option<PathBuf>,
    target: Option<String>,
    package_dir: Option<PathBuf>,
    receipt: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    wheel_only: bool,
}

pub fn usage() -> &'static str {
    "usage: solstone-distribution acquire <ffmpeg|onnx|pdfium|builder-inputs|cmake-windows> [FLAG]"
}

pub fn run(args: &[String]) -> Result<(), AcquireError> {
    let Some((command, rest)) = args.split_first() else {
        return Err(AcquireError::new(usage()));
    };
    let flags = parse_flags(rest)?;
    let repo = repository_root()?;
    match command.as_str() {
        "ffmpeg" => acquire_ffmpeg(&repo, flags.dest.as_deref())?,
        "onnx" => acquire_onnx(&repo, &flags)?,
        "pdfium" => acquire_pdfium(&repo, &flags)?,
        "builder-inputs" => acquire_builder_inputs(&repo, flags.dest.as_deref())?,
        "cmake-windows" => acquire_cmake_windows(&repo, flags.dest.as_deref())?,
        other => {
            return Err(AcquireError::new(format!(
                "unknown acquire command {other:?}\n{}",
                usage()
            )));
        }
    }
    Ok(())
}

fn parse_flags(args: &[String]) -> Result<Flags, AcquireError> {
    let mut flags = Flags::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let next = || {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| AcquireError::new(format!("missing value for {arg}")))
        };
        match arg.as_str() {
            "--dest" => flags.dest = Some(PathBuf::from(next()?)),
            "--target" => flags.target = Some(next()?),
            "--package-dir" => flags.package_dir = Some(PathBuf::from(next()?)),
            "--receipt" => flags.receipt = Some(PathBuf::from(next()?)),
            "--cache-dir" => flags.cache_dir = Some(PathBuf::from(next()?)),
            "--wheel-only" => {
                flags.wheel_only = true;
                index += 1;
                continue;
            }
            other => return Err(AcquireError::new(format!("unknown flag {other}"))),
        }
        index += 2;
    }
    Ok(flags)
}

fn repository_root() -> Result<PathBuf, AcquireError> {
    let cwd = std::env::current_dir()?;
    let mut cursor = cwd.as_path();
    loop {
        if cursor.join(BUILDER_INPUTS).is_file() {
            return Ok(cursor.to_path_buf());
        }
        cursor = cursor.parent().ok_or_else(|| {
            AcquireError::new("could not find core/distribution/builder-inputs.toml")
        })?;
    }
}

fn load_builder_inputs(repo: &Path) -> Result<BuilderInputsFile, AcquireError> {
    let path = repo.join(BUILDER_INPUTS);
    let text = fs::read_to_string(&path)?;
    let parsed: BuilderInputsFile = toml_edit::de::from_str(&text)
        .map_err(|error| AcquireError::new(format!("{BUILDER_INPUTS}: {error}")))?;
    if parsed.schema != "solstone-distribution-builder-inputs-v1" {
        return Err(AcquireError::new(format!(
            "unexpected builder-inputs schema: {}",
            parsed.schema
        )));
    }
    Ok(parsed)
}

pub(crate) fn windows_cmake_archive_input(
    repo: &Path,
) -> Result<WindowsCmakeArchiveInput, AcquireError> {
    let input = load_builder_inputs(repo)?.cmake_windows_x86_64;
    Ok(WindowsCmakeArchiveInput {
        version: input.version,
        filename: input.input.filename,
        url: input.input.url,
        sha256: input.input.sha256,
        size: input.input.size,
    })
}

fn fetch_verified(
    url: &str,
    sha256: &str,
    size: Option<u64>,
    dest: &Path,
) -> Result<bool, AcquireError> {
    #[cfg(not(windows))]
    {
        Ok(ensure_verified_url(
            url,
            sha256,
            size,
            dest,
            &BUILDER_INPUT_DOWNLOAD_POLICY,
            |_, _| {},
        )?)
    }
    #[cfg(windows)]
    {
        let _ = (url, sha256, size, dest);
        Err(AcquireError::new(
            "distribution acquire is not supported on windows",
        ))
    }
}

fn fetch_input(input: &FetchableInput, dest: &Path) -> Result<bool, AcquireError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fetch_verified(&input.url, &input.sha256, Some(input.size), dest)
}

fn acquire_ffmpeg(repo: &Path, dest: Option<&Path>) -> Result<(), AcquireError> {
    let inputs = load_builder_inputs(repo)?;
    let dest = dest
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo.join("target/ffmpeg-source-cache/ffmpeg.tar.gz"));
    let fetched = fetch_input(&inputs.ffmpeg, &dest)?;
    println!(
        "ffmpeg {} dest={}",
        if fetched { "fetched" } else { "cached" },
        dest.display()
    );
    Ok(())
}

fn acquire_builder_inputs(repo: &Path, dest: Option<&Path>) -> Result<(), AcquireError> {
    let inputs = load_builder_inputs(repo)?;
    let dest = dest
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo.join("core/distribution"));
    fs::create_dir_all(&dest)?;
    for input in [
        &inputs.ffmpeg,
        &inputs.zig,
        &inputs.rust_std_aarch64_gnu,
        &inputs.rust_std_aarch64_musl,
        &inputs.rust_std_x86_64_musl,
    ] {
        let path = dest.join(&input.filename);
        let fetched = fetch_input(input, &path)?;
        println!(
            "{} {} dest={}",
            input.filename,
            if fetched { "fetched" } else { "cached" },
            path.display()
        );
    }
    Ok(())
}

/// Acquire the CMake archive admitted for a Windows controlled-build slot.
///
/// This deliberately remains separate from `builder-inputs`: those bytes make
/// the Linux cleanroom image, while this archive is a verified driver-side
/// input that must be transferred into a Windows slot before its network deny
/// boundary is armed.
fn acquire_cmake_windows(repo: &Path, dest: Option<&Path>) -> Result<(), AcquireError> {
    let cmake = windows_cmake_archive_input(repo)?;
    let dest = dest.map(Path::to_path_buf).unwrap_or_else(|| {
        repo.join("target/windows-builder-inputs")
            .join(&cmake.filename)
    });
    let fetched = fetch_verified(&cmake.url, &cmake.sha256, Some(cmake.size), &dest)?;
    println!(
        "cmake {} version={} dest={}",
        if fetched { "fetched" } else { "cached" },
        cmake.version,
        dest.display()
    );
    Ok(())
}

fn acquire_onnx(repo: &Path, flags: &Flags) -> Result<(), AcquireError> {
    let target = flags
        .target
        .as_deref()
        .ok_or_else(|| AcquireError::new("acquire onnx requires --target"))?;
    let spec = onnx_runtime::spec_for(target)
        .ok_or_else(|| AcquireError::new(format!("missing required:\n  onnx runtime {target}")))?;
    let cache_dir = flags
        .cache_dir
        .clone()
        .unwrap_or_else(|| repo.join(ONNX_CACHE));
    fs::create_dir_all(&cache_dir)?;
    let wheel_name = spec
        .wheel_url
        .rsplit('/')
        .next()
        .ok_or_else(|| AcquireError::new("onnx wheel url has no file name"))?;
    let wheel_path = cache_dir.join(wheel_name);
    let alias = cache_dir.join(format!("onnxruntime-{target}.whl"));
    let fetched = fetch_verified(spec.wheel_url, spec.wheel_sha256, None, &wheel_path)?;
    if alias != wheel_path {
        fs::copy(&wheel_path, &alias)?;
    }
    println!(
        "onnx wheel {} dest={}",
        if fetched { "fetched" } else { "cached" },
        wheel_path.display()
    );
    if flags.wheel_only {
        return Ok(());
    }
    let staged = onnx_runtime::stage_from_path(spec, &wheel_path)?;
    let package_dir = flags
        .package_dir
        .clone()
        .unwrap_or_else(|| repo.join("target/runtime-package-staging/solstone-core-vad-analyze"));
    let package_name = package_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AcquireError::new("onnx --package-dir has no file name"))?;
    write_onnx_layout(spec, &staged, &package_dir, package_name)?;
    let link_dir = repo.join(ONNX_LINK_ROOT).join(spec.key);
    write_onnx_link_dir(spec, &staged.library, &link_dir)?;
    if let Some(receipt) = &flags.receipt {
        write_json(
            receipt,
            &OnnxReceipt {
                schema: "solstone.speakers-analyze-runtime-provenance.v1",
                package: package_name,
                target: spec.key,
                wheel: WheelReceipt {
                    url: spec.wheel_url,
                    sha256: spec.wheel_sha256,
                    path: relative_to(repo, &wheel_path),
                    size: fs::metadata(&wheel_path)?.len(),
                },
                runtime_library: RuntimeReceipt {
                    source_member: spec.runtime_member,
                    sha256: spec.runtime_sha256,
                    path: relative_to(
                        repo,
                        &package_dir
                            .join("wheel-data/data/lib")
                            .join(package_name)
                            .join(spec.runtime_staged_name),
                    ),
                    size: staged.library.len() as u64,
                },
                wheel_data_root: relative_to(repo, &package_dir.join("wheel-data")),
                link_dir: relative_to(repo, &link_dir),
            },
        )?;
    }
    println!("onnx staged link_dir={}", link_dir.display());
    Ok(())
}

fn write_onnx_layout(
    spec: &onnx_runtime::TargetSpec,
    staged: &onnx_runtime::StagedRuntime,
    package_dir: &Path,
    package_name: &str,
) -> Result<(), AcquireError> {
    let wheel_data = package_dir.join("wheel-data");
    let _ = fs::remove_dir_all(&wheel_data);
    let runtime_dir = wheel_data.join("data/lib").join(package_name);
    let notice_dir = wheel_data
        .join("data/share")
        .join(package_name)
        .join("licenses");
    stage::write_staged_file_mode(
        &runtime_dir,
        spec.runtime_staged_name,
        &staged.library,
        onnx_runtime::LIB_MODE,
    )?;
    for (name, bytes) in &staged.notices {
        stage::write_staged_file_mode(&notice_dir, name, bytes, onnx_runtime::NOTICE_MODE)?;
    }
    Ok(())
}

fn write_onnx_link_dir(
    spec: &onnx_runtime::TargetSpec,
    library: &[u8],
    link_dir: &Path,
) -> Result<(), AcquireError> {
    let _ = fs::remove_dir_all(link_dir);
    fs::create_dir_all(link_dir)?;
    let Some((primary, rest)) = spec.link_names.split_first() else {
        return Err(AcquireError::new("onnx target has no link names"));
    };
    let primary_path = link_dir.join(primary);
    fs::write(&primary_path, library)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&primary_path)?.permissions();
        permissions.set_mode(onnx_runtime::LIB_MODE);
        fs::set_permissions(&primary_path, permissions)?;
    }
    for name in rest {
        let link_path = link_dir.join(name);
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(primary, &link_path).is_ok();
        #[cfg(not(unix))]
        let linked = false;
        if !linked {
            fs::copy(&primary_path, &link_path)?;
        }
    }
    Ok(())
}

fn acquire_pdfium(repo: &Path, flags: &Flags) -> Result<(), AcquireError> {
    let target = flags
        .target
        .as_deref()
        .ok_or_else(|| AcquireError::new("acquire pdfium requires --target"))?;
    let spec = pdfium::spec_for(target).ok_or_else(|| {
        AcquireError::new(format!("missing required:\n  pdfium runtime {target}"))
    })?;
    let cache_dir = flags
        .cache_dir
        .clone()
        .unwrap_or_else(|| repo.join(PDF_CACHE));
    fs::create_dir_all(&cache_dir)?;
    let archive_path = cache_dir.join(spec.archive_name);
    let fetched = fetch_verified(
        &spec.archive_url(),
        spec.archive_sha256,
        None,
        &archive_path,
    )?;
    println!(
        "pdfium archive {} dest={}",
        if fetched { "fetched" } else { "cached" },
        archive_path.display()
    );
    let attestation_path = cache_dir.join(pdfium::ATTESTATION_NAME);
    fetch_verified(
        &pdfium::attestation_url(),
        pdfium::ATTESTATION_SHA256,
        None,
        &attestation_path,
    )?;
    let attestation = verify_pdfium_attestation(&archive_path)?;
    let archive = fs::read(&archive_path)?;
    let staged = pdfium::stage_from_bytes(spec, &archive)?;
    let package_dir = flags
        .package_dir
        .clone()
        .unwrap_or_else(|| repo.join("target/runtime-package-staging/solstone-core-pdf"));
    let package_name = package_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AcquireError::new("pdfium --package-dir has no file name"))?;
    let wheel_data = package_dir.join("wheel-data");
    let _ = fs::remove_dir_all(&wheel_data);
    let runtime_dir = wheel_data.join("data/lib").join(package_name);
    let notice_dir = wheel_data
        .join("data/share")
        .join(package_name)
        .join("licenses");
    stage::write_staged_file_mode(
        &runtime_dir,
        spec.library_name,
        &staged.library,
        pdfium::LIB_MODE,
    )?;
    for (name, bytes) in &staged.notices {
        stage::write_staged_file_mode(&notice_dir, name, bytes, pdfium::NOTICE_MODE)?;
    }
    let link_dir = repo.join(PDF_LINK_ROOT).join(spec.key);
    let _ = fs::remove_dir_all(&link_dir);
    fs::create_dir_all(&link_dir)?;
    stage::write_staged_file_mode(
        &link_dir,
        spec.library_name,
        &staged.library,
        pdfium::LIB_MODE,
    )?;
    if let Some(receipt) = &flags.receipt {
        write_json(
            receipt,
            &PdfiumReceipt {
                schema: "solstone.pdfium-runtime-provenance.v1",
                package: package_name,
                target: spec.key,
                release_tag: pdfium::RELEASE_TAG,
                archive: ArchiveReceipt {
                    url: spec.archive_url(),
                    sha256: spec.archive_sha256,
                    name: spec.archive_name,
                },
                attestation,
                runtime_library: RuntimeReceipt {
                    source_member: spec.library_member,
                    sha256: spec.library_sha256,
                    path: relative_to(repo, &runtime_dir.join(spec.library_name)),
                    size: staged.library.len() as u64,
                },
                wheel_data_root: relative_to(repo, &wheel_data),
                link_dir: relative_to(repo, &link_dir),
            },
        )?;
    }
    println!("pdfium staged link_dir={}", link_dir.display());
    Ok(())
}

fn verify_pdfium_attestation(archive: &Path) -> Result<BTreeMap<String, String>, AcquireError> {
    let output = Command::new("gh")
        .args([
            "attestation",
            "verify",
            &archive.to_string_lossy(),
            "--repo",
            pdfium::ATTESTATION_REPOSITORY,
        ])
        .output()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                AcquireError::new(
                    "GitHub CLI is required to verify the pinned PDFium build attestation; \
                     install and authenticate gh rather than bypassing this control",
                )
            } else {
                AcquireError::new(error.to_string())
            }
        })?;
    if !output.status.success() {
        return Err(AcquireError::new(format!(
            "PDFium build attestation verification failed\n  stdout: {}\n  stderr: {}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut record = BTreeMap::new();
    record.insert(
        "command".to_owned(),
        format!(
            "gh attestation verify {} --repo {}",
            archive.display(),
            pdfium::ATTESTATION_REPOSITORY
        ),
    );
    record.insert(
        "stdout".to_owned(),
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    );
    Ok(record)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), AcquireError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(value)
        .map_err(|error| AcquireError::new(error.to_string()))?;
    let mut file = fs::File::create(path)?;
    file.write_all(body.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string())
}

#[derive(Serialize)]
struct OnnxReceipt<'a> {
    schema: &'a str,
    package: &'a str,
    target: &'a str,
    wheel: WheelReceipt<'a>,
    runtime_library: RuntimeReceipt<'a>,
    wheel_data_root: String,
    link_dir: String,
}

#[derive(Serialize)]
struct WheelReceipt<'a> {
    url: &'a str,
    sha256: &'a str,
    path: String,
    size: u64,
}

#[derive(Serialize)]
struct RuntimeReceipt<'a> {
    source_member: &'a str,
    sha256: &'a str,
    path: String,
    size: u64,
}

#[derive(Serialize)]
struct PdfiumReceipt<'a> {
    schema: &'a str,
    package: &'a str,
    target: &'a str,
    release_tag: &'a str,
    archive: ArchiveReceipt,
    attestation: BTreeMap<String, String>,
    runtime_library: RuntimeReceipt<'a>,
    wheel_data_root: String,
    link_dir: String,
}

#[derive(Serialize)]
struct ArchiveReceipt {
    url: String,
    sha256: &'static str,
    name: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_inputs_parse_from_the_committed_file() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("repo")
            .to_path_buf();
        let inputs = load_builder_inputs(&root).expect("committed builder-inputs");
        assert_eq!(inputs.ffmpeg.sha256.len(), 64);
        assert!(inputs.ffmpeg.url.starts_with("https://github.com/"));
        assert!(inputs.zig.url.starts_with("https://ziglang.org/"));
        assert_eq!(inputs.cmake_windows_x86_64.version, "3.31.12");
        assert_eq!(
            inputs.cmake_windows_x86_64.input.filename,
            "cmake-3.31.12-windows-x86_64.zip"
        );
        assert_eq!(
            inputs.cmake_windows_x86_64.input.url,
            "https://cmake.org/files/v3.31/cmake-3.31.12-windows-x86_64.zip"
        );
        assert_eq!(
            inputs.cmake_windows_x86_64.input.sha256,
            "0c4baa40f28b3f8225eb3fdf6946c987b4fe901403b4eaf2fbbd9378100aaa0c"
        );
        assert_eq!(inputs.cmake_windows_x86_64.input.size, 46_666_397);
    }

    #[test]
    fn parse_flags_reads_wheel_only_without_a_value() {
        let flags = parse_flags(&[
            "--target".into(),
            "linux-x86_64".into(),
            "--wheel-only".into(),
            "--dest".into(),
            "out".into(),
        ])
        .expect("flags");
        assert!(flags.wheel_only);
        assert_eq!(flags.target.as_deref(), Some("linux-x86_64"));
        assert_eq!(flags.dest.as_deref(), Some(Path::new("out")));
    }
}
