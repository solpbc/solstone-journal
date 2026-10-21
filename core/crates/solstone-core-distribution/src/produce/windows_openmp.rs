// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retain the original OpenMP receipt and its separately bound development bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::windows_build::read_bounded;
use super::windows_inputs::{AdmittedControlledInput, verify_receipt_document_binding};
use crate::controlled_build::{
    ControlledBuildReceipt, ValidationReference, decode_controlled_build_receipt,
};
use crate::digest::sha256_hex;
use crate::openmp_windows::{self as recipe, OpenMpBuildEvidence};

pub struct OpenMpArchivePaths<'a> {
    pub source: &'a Path,
    pub llvm: &'a Path,
    pub cmake: &'a Path,
    pub python: &'a Path,
}

impl OpenMpArchivePaths<'_> {
    pub fn verify(&self) -> Result<(), String> {
        for (path, pin) in [self.source, self.llvm, self.cmake, self.python]
            .into_iter()
            .zip(recipe::pinned_inputs())
        {
            let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            if !metadata.is_file() || metadata.len() != pin.size {
                return Err(format!(
                    "OpenMP archive differs from its pinned size: {}",
                    pin.label
                ));
            }
            let mut digest = Sha256::new();
            let mut buffer = vec![0; 1024 * 1024];
            let mut total = 0;
            loop {
                let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
                if count == 0 {
                    break;
                }
                total += count as u64;
                if total > pin.size {
                    return Err("OpenMP archive grew during hashing".into());
                }
                digest.update(&buffer[..count]);
            }
            if total != pin.size || format!("{:x}", digest.finalize()) != pin.sha256 {
                return Err(format!(
                    "OpenMP archive differs from its pinned SHA-256: {}",
                    pin.label
                ));
            }
        }
        Ok(())
    }
}

pub struct OpenMpInputPaths<'a> {
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub validation: &'a Path,
    pub archives: OpenMpArchivePaths<'a>,
    pub output_root: &'a Path,
    pub development_root: &'a Path,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMpFiles {
    pub receipt: std::path::PathBuf,
    pub evidence: std::path::PathBuf,
    pub validation: std::path::PathBuf,
    pub source_archive: std::path::PathBuf,
    pub llvm_archive: std::path::PathBuf,
    pub cmake_archive: std::path::PathBuf,
    pub python_archive: std::path::PathBuf,
    pub output_root: std::path::PathBuf,
    pub development_root: std::path::PathBuf,
}

impl OpenMpFiles {
    pub fn paths(&self) -> Result<OpenMpInputPaths<'_>, String> {
        for path in [
            &self.receipt,
            &self.evidence,
            &self.validation,
            &self.source_archive,
            &self.llvm_archive,
            &self.cmake_archive,
            &self.python_archive,
            &self.output_root,
            &self.development_root,
        ] {
            if !path.is_absolute()
                || path
                    .components()
                    .any(|p| p == std::path::Component::ParentDir)
            {
                return Err("ordinary absolute OpenMP input paths required".into());
            }
        }
        Ok(OpenMpInputPaths {
            receipt: &self.receipt,
            evidence: &self.evidence,
            validation: &self.validation,
            archives: OpenMpArchivePaths {
                source: &self.source_archive,
                llvm: &self.llvm_archive,
                cmake: &self.cmake_archive,
                python: &self.python_archive,
            },
            output_root: &self.output_root,
            development_root: &self.development_root,
        })
    }
}

pub struct AdmittedOpenMp {
    runtime: AdmittedControlledInput,
    header: Vec<u8>,
    import_library: Vec<u8>,
}

impl AdmittedOpenMp {
    pub fn runtime(&self) -> &AdmittedControlledInput {
        &self.runtime
    }
    pub fn header(&self) -> &[u8] {
        &self.header
    }
    pub fn import_library(&self) -> &[u8] {
        &self.import_library
    }
    /// Bind a consumer to this original runtime and development cohort.
    /// These are input identities; the development members are not payload files.
    pub fn build_inputs(&self) -> Vec<crate::controlled_build::InputIdentityEntry> {
        let members: [(&str, &[u8]); 6] = [
            (
                "dependencies/openmp/receipt.json",
                self.runtime.receipt_bytes(),
            ),
            (
                "dependencies/openmp/build-evidence.json",
                self.runtime.evidence_bytes(),
            ),
            (
                "dependencies/openmp/validation.log",
                self.runtime.validation_bytes(),
            ),
            (
                "dependencies/openmp/libomp.dll",
                &self.runtime.outputs()[recipe::OUTPUT_LABEL],
            ),
            ("dependencies/openmp/include/omp.h", &self.header),
            ("dependencies/openmp/lib/libomp.lib", &self.import_library),
        ];
        members
            .into_iter()
            .map(|(label, bytes)| recipe::identity(label, bytes.len() as u64, &sha256_hex(bytes)))
            .collect()
    }
    pub fn into_runtime(self) -> AdmittedControlledInput {
        self.runtime
    }
}

pub use crate::windows_clang::{
    BuildValidation as OpenMpValidation, NetworkDeny as OpenMpNetworkDeny,
};
use crate::windows_clang::{parse_cache, windows_path};

pub fn validate_capture(
    evidence: &OpenMpBuildEvidence,
    validation: &OpenMpValidation,
) -> Result<(), String> {
    evidence.validate()?;
    if validation.schema != "solstone.openmp-windows-validation.v1"
        || sha256_hex(validation.cache.as_bytes()) != evidence.cache_sha256
        || sha256_hex(validation.build_log.as_bytes()) != evidence.build_log_sha256
        || sha256_hex(validation.install_manifest.as_bytes()) != evidence.install_manifest_sha256
        || !validation
            .build_log
            .lines()
            .any(|line| line == "OPENMP_WINDOWS_BUILD_EXIT=0")
    {
        return Err("OpenMP validation does not bind the successful original build".into());
    }
    let work = windows_path(&validation.work_root)?;
    let cache = parse_cache(&validation.cache)?;
    for (key, value) in recipe::cache_configuration() {
        if cache.get(key.as_str()).copied() != Some(value.as_str()) {
            return Err(format!("OpenMP cache differs at {key}"));
        }
    }
    for (key, suffix) in [
        (
            "CMAKE_HOME_DIRECTORY",
            "/source/llvm-project-22.1.6.src/openmp",
        ),
        ("CMAKE_INSTALL_PREFIX", "/install"),
    ] {
        if windows_path(cache.get(key).ok_or("OpenMP source/install path missing")?)?
            != format!("{work}{suffix}")
        {
            return Err("OpenMP cache source or install root differs".into());
        }
    }
    let python_paths: Vec<_> = validation
        .python_path_configuration
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.replace('\\', "/").to_ascii_lowercase())
        .collect();
    if python_paths
        != [
            "python312.zip".to_owned(),
            ".".to_owned(),
            format!("{work}/source/llvm-project-22.1.6.src/openmp/runtime/tools"),
        ]
    {
        return Err("OpenMP embedded Python search path differs".into());
    }
    let installed: BTreeSet<_> = validation
        .install_manifest
        .lines()
        .filter(|line| !line.is_empty())
        .map(windows_path)
        .collect::<Result<_, _>>()?;
    if installed
        .iter()
        .any(|path| !path.starts_with(&format!("{work}/install/")))
        || ["bin/libomp.dll", "lib/libomp.lib", "include/omp.h"]
            .iter()
            .any(|path| !installed.contains(&format!("{work}/install/{path}")))
    {
        return Err(
            "OpenMP install manifest is outside its prefix or lacks a required member".into(),
        );
    }
    crate::windows_clang::validate_host_capture(
        &crate::windows_clang::ToolCensus {
            archive_tools: &evidence.archive_tools,
            system_tools: &evidence.system_tools,
            network_denied_tools: &evidence.network_denied_tools,
        },
        validation,
    )
}

fn flat_files(root: &Path, names: &[&str]) -> Result<(), String> {
    let mut observed = BTreeSet::new();
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err("non-regular OpenMP output".into());
        }
        observed.insert(entry.file_name());
    }
    if observed != names.iter().map(std::ffi::OsString::from).collect() {
        return Err("OpenMP output population differs".into());
    }
    Ok(())
}

fn require_import_library(bytes: &[u8]) -> Result<(), String> {
    if bytes.get(..8) != Some(b"!<arch>\n") {
        return Err("OpenMP import library is not a regular ar archive".into());
    }
    let mut offset = 8usize;
    let mut objects = 0;
    let mut members = 0;
    while offset < bytes.len() {
        members += 1;
        if members > 8192 {
            return Err("OpenMP import-library member limit exceeded".into());
        }
        let header = bytes
            .get(offset..offset.checked_add(60).ok_or("ar offset overflow")?)
            .ok_or("truncated ar header")?;
        if &header[58..] != b"`\n" {
            return Err("invalid ar member header".into());
        }
        let name = std::str::from_utf8(&header[..16])
            .map_err(|e| e.to_string())?
            .trim();
        let size: usize = std::str::from_utf8(&header[48..58])
            .map_err(|e| e.to_string())?
            .trim()
            .parse::<usize>()
            .map_err(|e| e.to_string())?;
        offset += 60;
        let end = offset.checked_add(size).ok_or("ar size overflow")?;
        let body = bytes.get(offset..end).ok_or("truncated ar member")?;
        if !matches!(name, "/" | "//" | "/SYM64/") {
            if body.len() < 20 {
                return Err("truncated OpenMP COFF member".into());
            }
            if body[..4] == [0, 0, 255, 255] {
                if body[4..6] != [0, 0] || body[6..8] != [0x64, 0xaa] {
                    return Err("foreign or unsupported OpenMP import object".into());
                }
                if u32::from_le_bytes(body[12..16].try_into().unwrap()) as usize != body.len() - 20
                    || body.last() != Some(&0)
                {
                    return Err("invalid OpenMP import-object data extent".into());
                }
                let mut strings = body[20..].split(|b| *b == 0);
                let symbol = strings.next().ok_or("missing import symbol")?;
                let dll = strings.next().ok_or("missing import DLL")?;
                if symbol.is_empty() || !dll.eq_ignore_ascii_case(b"libomp.dll") {
                    return Err("OpenMP import object names a foreign DLL".into());
                }
            } else if body[..2] != [0x64, 0xaa] {
                return Err("foreign OpenMP COFF-object machine".into());
            }
            objects += 1;
        }
        offset = end;
        if size % 2 == 1 {
            if bytes.get(offset) != Some(&b'\n') {
                return Err("invalid ar member padding".into());
            }
            offset += 1;
        }
    }
    if objects == 0 {
        return Err("OpenMP import library has no native objects".into());
    }
    Ok(())
}

fn read_outputs(root: &Path) -> Result<(Vec<u8>, Vec<u8>), String> {
    flat_files(root, &[recipe::OUTPUT_LABEL, "LICENSE.txt"])?;
    let dll = read_bounded(&root.join(recipe::OUTPUT_LABEL), 16 * 1024 * 1024)?;
    let license = read_bounded(&root.join("LICENSE.txt"), recipe::LICENSE_BYTES)?;
    if license.len() as u64 != recipe::LICENSE_BYTES
        || sha256_hex(&license) != recipe::LICENSE_SHA256
    {
        return Err("OpenMP original licence differs".into());
    }
    Ok((dll, license))
}

fn read_development(
    root: &Path,
    evidence: &OpenMpBuildEvidence,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    evidence.validate()?;
    let mut directories = BTreeSet::new();
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            return Err("non-directory OpenMP development member".into());
        }
        directories.insert(entry.file_name());
    }
    if directories
        != ["include", "lib"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect()
    {
        return Err("OpenMP development directory population differs".into());
    }
    flat_files(&root.join("include"), &["omp.h"])?;
    flat_files(&root.join("lib"), &["libomp.lib"])?;
    let mut members = Vec::new();
    for pin in &evidence.development {
        let bytes = read_bounded(
            &super::windows_stage::join_components(root, &pin.label),
            pin.size,
        )?;
        if bytes.len() as u64 != pin.size || sha256_hex(&bytes) != pin.sha256 {
            return Err("OpenMP development bytes differ from the original build".into());
        }
        members.push(bytes);
    }
    let [header, library]: [Vec<u8>; 2] = members
        .try_into()
        .map_err(|_| "OpenMP development population differs")?;
    require_import_library(&library)?;
    Ok((header, library))
}

pub fn expected_receipt(
    original: &ControlledBuildReceipt,
    evidence: &OpenMpBuildEvidence,
    validation_bytes: &[u8],
    dll: &[u8],
) -> Result<ControlledBuildReceipt, String> {
    let mut draft = recipe::assemble_receipt_draft(
        original.source.clone(),
        evidence,
        original.builder.clone(),
        dll,
    )?;
    draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into());
    draft.validation = Some(ValidationReference {
        description:
            "OpenMP Windows ARM64 network-denied source build and PE/development inspection".into(),
        sha256: sha256_hex(validation_bytes),
    });
    draft.validate().map_err(|e| e.to_string())
}

// Windows inventory counts flat leaves at depth one. The two allowed
// files are libomp.dll and its pinned licence; read_outputs rejects extras.
pub fn admit(paths: OpenMpInputPaths<'_>) -> Result<AdmittedOpenMp, String> {
    paths.archives.verify()?;
    let receipt_bytes = read_bounded(paths.receipt, 4 * 1024 * 1024)?;
    let receipt = decode_controlled_build_receipt(&receipt_bytes).map_err(|e| e.to_string())?;
    let evidence_bytes = read_bounded(paths.evidence, 4 * 1024 * 1024)?;
    let evidence: OpenMpBuildEvidence =
        serde_json::from_slice(&evidence_bytes).map_err(|e| e.to_string())?;
    let validation_bytes = read_bounded(paths.validation, 4 * 1024 * 1024)?;
    let validation: OpenMpValidation =
        serde_json::from_slice(&validation_bytes).map_err(|e| e.to_string())?;
    verify_receipt_document_binding(
        &receipt,
        &evidence_bytes,
        &validation_bytes,
        recipe::EVIDENCE_LABEL,
    )?;
    validate_capture(&evidence, &validation)?;
    crate::artifact_verify::verify_controlled_build_artifacts(
        paths.output_root,
        &receipt,
        crate::artifact_verify::ControlledBuildArtifactVerificationLimits::new(
            2,
            1,
            16 * 1024 * 1024,
        ),
    )
    .map_err(|e| e.to_string())?;
    let (dll, license) = read_outputs(paths.output_root)?;
    let (header, import_library) = read_development(paths.development_root, &evidence)?;
    if expected_receipt(&receipt, &evidence, &validation_bytes, &dll)? != receipt {
        return Err("OpenMP original receipt differs from the admitted source/configuration/evidence/output".into());
    }
    Ok(AdmittedOpenMp {
        runtime: AdmittedControlledInput::new(
            receipt,
            receipt_bytes,
            evidence_bytes,
            validation_bytes,
            BTreeMap::from([
                (recipe::OUTPUT_LABEL.into(), dll),
                ("LICENSE.txt".into(), license),
            ]),
        ),
        header,
        import_library,
    })
}

const USAGE: &str = "openmp-windows verify-inputs|record|verify --source-archive ABS --llvm-archive ABS --cmake-archive ABS --python-archive ABS [FLAG VALUE]";

fn flags(args: &[String], allowed: &[&str]) -> Result<BTreeMap<String, String>, String> {
    if !args.len().is_multiple_of(2) {
        return Err(USAGE.into());
    }
    let mut values = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str())
            || pair[1].is_empty()
            || values.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err("unknown, duplicate or empty OpenMP flag".into());
        }
    }
    if allowed.iter().any(|key| !values.contains_key(*key)) {
        return Err("missing OpenMP build flag".into());
    }
    Ok(values)
}

fn absolute(values: &BTreeMap<String, String>, key: &str) -> Result<std::path::PathBuf, String> {
    let p = std::path::PathBuf::from(&values[key]);
    if !p.is_absolute() || p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(format!(
            "absolute OpenMP path without parent traversal required: {key}"
        ));
    }
    Ok(p)
}

pub(crate) fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    output.write_all(bytes).map_err(|e| e.to_string())?;
    output.sync_all().map_err(|e| e.to_string())
}

pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let (operation, rest) = args.split_first().ok_or(USAGE)?;
    let mut allowed = vec![
        "--source-archive",
        "--llvm-archive",
        "--cmake-archive",
        "--python-archive",
    ];
    match operation.as_str() {
        "verify-inputs" => {}
        "verify" | "record" => allowed.extend([
            "--validation",
            "--output-root",
            "--development-root",
            "--receipt",
            "--evidence",
        ]),
        _ => return Err(USAGE.into()),
    }
    if operation == "record" {
        allowed.extend([
            "--capture",
            "--product-commit",
            "--cargo-lock-sha256",
            "--builder-host",
        ]);
    }
    let values = flags(rest, &allowed)?;
    let source = absolute(&values, "--source-archive")?;
    let llvm = absolute(&values, "--llvm-archive")?;
    let cmake = absolute(&values, "--cmake-archive")?;
    let python = absolute(&values, "--python-archive")?;
    let archives = OpenMpArchivePaths {
        source: &source,
        llvm: &llvm,
        cmake: &cmake,
        python: &python,
    };
    if operation == "verify-inputs" {
        archives.verify()?;
        return serde_json::to_string(&serde_json::json!({
            "inputs": recipe::pinned_inputs(), "tools": recipe::pinned_tools(),
            "configuration": recipe::cache_configuration(), "configuration_flags": recipe::build_configuration().flags,
        })).map_err(|e| e.to_string());
    }
    let validation_path = absolute(&values, "--validation")?;
    let output_root = absolute(&values, "--output-root")?;
    let development_root = absolute(&values, "--development-root")?;
    let receipt_path = absolute(&values, "--receipt")?;
    let evidence_path = absolute(&values, "--evidence")?;
    if operation == "verify" {
        let admitted = admit(OpenMpInputPaths {
            receipt: &receipt_path,
            evidence: &evidence_path,
            validation: &validation_path,
            archives,
            output_root: &output_root,
            development_root: &development_root,
        })?;
        return Ok(format!(
            "OPENMP_WINDOWS_VERIFY_PASS output_sha256={} header_sha256={} import_library_sha256={}",
            admitted.runtime.receipt().outputs[0].pre_signing_sha256,
            sha256_hex(&admitted.header),
            sha256_hex(&admitted.import_library)
        ));
    }
    archives.verify()?;
    let inventory = crate::inventory::repository_inventory_path(start)
        .ok_or("OpenMP recorder needs its product checkout")?;
    let repo = inventory
        .ancestors()
        .nth(3)
        .ok_or("missing product checkout root")?;
    let before = super::windows_build::capture_source(repo)?;
    if before.commit != values["--product-commit"]
        || before.lock_sha256 != values["--cargo-lock-sha256"]
    {
        return Err("OpenMP recorder source differs from its source-bound driver".into());
    }
    let capture = read_bounded(&absolute(&values, "--capture")?, 4 * 1024 * 1024)?;
    let evidence: OpenMpBuildEvidence =
        serde_json::from_slice(&capture).map_err(|e| e.to_string())?;
    let validation_bytes = read_bounded(&validation_path, 4 * 1024 * 1024)?;
    let validation: OpenMpValidation =
        serde_json::from_slice(&validation_bytes).map_err(|e| e.to_string())?;
    validate_capture(&evidence, &validation)?;
    let (dll, _license) = read_outputs(&output_root)?;
    read_development(&development_root, &evidence)?;
    let mut draft = recipe::assemble_receipt_draft(
        recipe::source_identity(before.clone()),
        &evidence,
        crate::controlled_build::BuilderIdentity {
            host: values["--builder-host"].clone(),
            toolchain: "LLVM22.1.6 clang-cl/NMake x64 -> ARM64; CMake3.31.12; Python3.12.10".into(),
        },
        &dll,
    )?;
    draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into());
    draft.validation = Some(ValidationReference {
        description:
            "OpenMP Windows ARM64 network-denied source build and PE/development inspection".into(),
        sha256: sha256_hex(&validation_bytes),
    });
    let receipt = draft.validate().map_err(|e| e.to_string())?;
    crate::artifact_verify::verify_controlled_build_artifacts(
        &output_root,
        &receipt,
        crate::artifact_verify::ControlledBuildArtifactVerificationLimits::new(
            2,
            1,
            16 * 1024 * 1024,
        ),
    )
    .map_err(|e| e.to_string())?;
    if super::windows_build::capture_source(repo)? != before {
        return Err("product source changed during OpenMP recording".into());
    }
    publish(
        &evidence_path,
        &serde_json::to_vec(&evidence).map_err(|e| e.to_string())?,
    )?;
    let publication =
        crate::controlled_build::write_controlled_build_receipt_exclusive(&receipt_path, &receipt)
            .map_err(|e| e.to_string())?;
    if matches!(
        publication,
        crate::controlled_build::ControlledBuildReceiptPublication::PublicationUnconfirmed { .. }
    ) {
        return Err("OpenMP receipt publication unconfirmed".into());
    }
    Ok(format!(
        "OPENMP_WINDOWS_RECORD_PASS output_sha256={} publication={publication:?}",
        receipt.outputs[0].pre_signing_sha256
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> (OpenMpBuildEvidence, OpenMpValidation) {
        let work = "c:/openmp/work";
        let system_tools: Vec<_> = [
            "msvc/link.exe",
            "msvc/lib.exe",
            "msvc/nmake.exe",
            "sdk/rc.exe",
            "sdk/mt.exe",
            "windows/cmd.exe",
            "windows/powershell.exe",
            "windows/git.exe",
            "windows/git-backend.exe",
            "windows/git-core.exe",
            "windows/git-http.exe",
            "windows/recorder.exe",
        ]
        .into_iter()
        .map(|label| recipe::identity(label, 1, &"a".repeat(64)))
        .collect();
        let archive_tools = recipe::pinned_tools();
        let mut tool_paths: BTreeMap<_, _> = archive_tools
            .iter()
            .chain(&system_tools)
            .map(|tool| (tool.label.clone(), format!("{work}/{}", tool.label)))
            .collect();
        for (label, path) in [
            ("windows/git.exe", "c:/git/cmd/git.exe"),
            ("windows/git-backend.exe", "c:/git/ucrt64/bin/git.exe"),
            (
                "windows/git-core.exe",
                "c:/git/ucrt64/libexec/git-core/git.exe",
            ),
            (
                "windows/git-http.exe",
                "c:/git/ucrt64/libexec/git-core/git-remote-https.exe",
            ),
        ] {
            tool_paths.insert(label.into(), path.into());
        }
        let network_denied_tools: Vec<_> = tool_paths.keys().cloned().collect();
        let network_denies = tool_paths
            .iter()
            .map(|(label, path)| OpenMpNetworkDeny {
                label: label.clone(),
                program: path.clone(),
                enabled: true,
                direction: "Outbound".into(),
                action: "Block".into(),
                profile: "Any".into(),
                protocol: "Any".into(),
                local_address: vec!["Any".into()],
                remote_address: vec!["Any".into()],
                local_port: vec!["Any".into()],
                remote_port: vec!["Any".into()],
            })
            .collect();
        let mut cache: String = recipe::cache_configuration()
            .into_iter()
            .map(|(key, value)| format!("{key}:STRING={value}\n"))
            .collect();
        for (key, value) in [
            (
                "CMAKE_HOME_DIRECTORY",
                format!("{work}/source/llvm-project-22.1.6.src/openmp"),
            ),
            ("CMAKE_INSTALL_PREFIX", format!("{work}/install")),
            ("CMAKE_C_COMPILER", format!("{work}/llvm/bin/clang-cl.exe")),
            (
                "CMAKE_CXX_COMPILER",
                format!("{work}/llvm/bin/clang-cl.exe"),
            ),
            (
                "CMAKE_ASM_COMPILER",
                format!("{work}/llvm/bin/clang-cl.exe"),
            ),
            ("Python3_EXECUTABLE", format!("{work}/python/python.exe")),
            ("CMAKE_MAKE_PROGRAM", tool_paths["msvc/nmake.exe"].clone()),
            (
                "GIT_EXECUTABLE",
                tool_paths["windows/git-backend.exe"].clone(),
            ),
        ] {
            cache.push_str(&format!("{key}:FILEPATH={value}\n"));
        }
        let validation = OpenMpValidation {
            schema: "solstone.openmp-windows-validation.v1".into(),
            work_root: work.into(),
            tool_paths,
            network_denies,
            cache,
            build_log: "OPENMP_WINDOWS_BUILD_EXIT=0\n".into(),
            install_manifest: ["bin/libomp.dll", "lib/libomp.lib", "include/omp.h"]
                .iter()
                .map(|path| format!("{work}/install/{path}\n"))
                .collect(),
            python_path_configuration: format!(
                "python312.zip\n.\n{work}/source/llvm-project-22.1.6.src/openmp/runtime/tools\n"
            ),
        };
        let evidence = OpenMpBuildEvidence {
            schema: recipe::EVIDENCE_SCHEMA.into(),
            inputs: recipe::pinned_inputs(),
            archive_tools,
            system_tools,
            network_denied_tools,
            configuration: recipe::cache_configuration(),
            development: recipe::DEVELOPMENT_LABELS
                .iter()
                .map(|label| recipe::identity(label, 1, &"b".repeat(64)))
                .collect(),
            cache_sha256: sha256_hex(validation.cache.as_bytes()),
            build_log_sha256: sha256_hex(validation.build_log.as_bytes()),
            install_manifest_sha256: sha256_hex(validation.install_manifest.as_bytes()),
        };
        (evidence, validation)
    }

    #[test]
    fn launcher_only_git_census_cannot_claim_complete_denial() {
        let (mut evidence, mut validation) = capture();
        for label in [
            "windows/git-backend.exe",
            "windows/git-core.exe",
            "windows/git-http.exe",
        ] {
            evidence.system_tools.retain(|tool| tool.label != label);
            evidence.network_denied_tools.retain(|tool| tool != label);
            validation.tool_paths.remove(label);
            validation.network_denies.retain(|rule| rule.label != label);
        }
        assert!(
            validate_capture(&evidence, &validation)
                .unwrap_err()
                .contains("windows/git-backend.exe")
        );
    }

    #[test]
    fn rebinding_git_backend_to_its_launcher_or_foreign_transport_refuses() {
        for label in ["windows/git-backend.exe", "windows/git-http.exe"] {
            let (mut evidence, mut validation) = capture();
            let original = validation.tool_paths[label].clone();
            let replacement = validation.tool_paths["windows/git.exe"].clone();
            validation
                .tool_paths
                .insert(label.into(), replacement.clone());
            validation
                .network_denies
                .iter_mut()
                .find(|rule| rule.label == label)
                .unwrap()
                .program = replacement.clone();
            validation.cache = validation.cache.replace(&original, &replacement);
            evidence.cache_sha256 = sha256_hex(validation.cache.as_bytes());
            let error = validate_capture(&evidence, &validation).unwrap_err();
            assert!(
                error.contains("Git"),
                "{label} rebinding reached the wrong refusal: {error}"
            );
        }
    }

    #[test]
    fn capture_refuses_rebound_foreign_paths_python_and_firewall_changes() {
        let (evidence, good) = capture();
        validate_capture(&evidence, &good).unwrap();
        let mut bad = good.clone();
        bad.network_denies[0].action = "Allow".into();
        assert!(validate_capture(&evidence, &bad).is_err());
        let mut bad = good.clone();
        bad.network_denies[0].remote_address = vec!["127.0.0.1".into()];
        assert!(validate_capture(&evidence, &bad).is_err());
        let mut bad = good.clone();
        bad.tool_paths
            .insert("foreign/tool.exe".into(), "c:/foreign/tool.exe".into());
        assert!(validate_capture(&evidence, &bad).is_err());
        let mut bad = good.clone();
        bad.python_path_configuration.push_str("import site\n");
        assert!(validate_capture(&evidence, &bad).is_err());
        let mut bad = good.clone();
        bad.cache = bad.cache.replace(
            "c:/openmp/work/llvm/bin/clang-cl.exe",
            "c:/foreign/clang-cl.exe",
        );
        let mut rebound = evidence.clone();
        rebound.cache_sha256 = sha256_hex(bad.cache.as_bytes());
        assert!(validate_capture(&rebound, &bad).is_err());
        let mut bad = good.clone();
        bad.install_manifest.push_str("c:/outside/extra.dll\n");
        let mut rebound = evidence.clone();
        rebound.install_manifest_sha256 = sha256_hex(bad.install_manifest.as_bytes());
        assert!(validate_capture(&rebound, &bad).is_err());
        let mut bad = good;
        bad.cache
            .push_str("CMAKE_C_COMPILER_TARGET:STRING=x86_64-pc-windows-msvc\n");
        let mut rebound = evidence;
        rebound.cache_sha256 = sha256_hex(bad.cache.as_bytes());
        assert!(validate_capture(&rebound, &bad).is_err());
    }

    fn library(machine: u16, dll: &[u8]) -> Vec<u8> {
        let mut body = vec![0; 20];
        body[2..4].copy_from_slice(&0xffffu16.to_le_bytes());
        body[6..8].copy_from_slice(&machine.to_le_bytes());
        body.extend_from_slice(b"omp_get_num_threads\0");
        body.extend_from_slice(dll);
        body.push(0);
        let size = (body.len() - 20) as u32;
        body[12..16].copy_from_slice(&size.to_le_bytes());
        let mut header = [b' '; 60];
        header[..11].copy_from_slice(b"libomp.dll/");
        let size = body.len().to_string();
        header[48..48 + size.len()].copy_from_slice(size.as_bytes());
        header[58..].copy_from_slice(b"`\n");
        let mut result = b"!<arch>\n".to_vec();
        result.extend_from_slice(&header);
        result.extend_from_slice(&body);
        if body.len() % 2 == 1 {
            result.push(b'\n');
        }
        result
    }

    #[test]
    fn development_import_library_refuses_foreign_machine_dll_and_truncation() {
        let good = library(0xaa64, b"libomp.dll");
        require_import_library(&good).unwrap();
        for bad in [
            library(0x8664, b"libomp.dll"),
            library(0xaa64, b"vcomp140.dll"),
            good[..good.len() - 2].to_vec(),
            b"!<arch>\n".to_vec(),
        ] {
            assert!(require_import_library(&bad).is_err());
        }
    }
}
