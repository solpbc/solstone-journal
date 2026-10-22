// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact restic source, host tool and offline module archive inspection.
//! All archive digests are checked before any container is parsed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

use crate::controlled_build::InputIdentityEntry;
use crate::digest::sha256_hex;
use crate::restic_windows as recipe;
use solstone_core_installed_payload::WindowsArchitecture;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleManifest {
    schema: String,
    restic_version: String,
    go_mod_sha256: String,
    go_sum_sha256: String,
    files: Vec<ModuleFile>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug)]
pub struct InspectedInputs {
    pub tools: Vec<InputIdentityEntry>,
    pub go_sum: Vec<u8>,
}

pub fn require_pin(bytes: &[u8], expected: &InputIdentityEntry) -> Result<(), String> {
    if bytes.len() as u64 != expected.size || sha256_hex(bytes) != expected.sha256 {
        return Err(format!(
            "restic input differs from its pinned bytes: {}",
            expected.label
        ));
    }
    Ok(())
}

pub fn inspect(source: &[u8], go: &[u8], modules: &[u8]) -> Result<InspectedInputs, String> {
    for (bytes, pin) in [source, go, modules]
        .into_iter()
        .zip(recipe::pinned_inputs())
    {
        require_pin(bytes, &pin)?;
    }
    let source_members = inspect_source_members(source)?;
    let source_file = |path: &str, expected: &str| -> Result<Vec<u8>, String> {
        let matches: Vec<_> = source_members.iter().filter(|m| m.path == path).collect();
        let [member] = matches.as_slice() else {
            return Err(format!("restic source member absent or duplicated: {path}"));
        };
        if sha256_hex(&member.bytes) != expected {
            return Err(format!("restic source metadata differs: {path}"));
        }
        Ok(member.bytes.clone())
    };
    let _ = source_file("restic-0.19.0/go.mod", recipe::GO_MOD_SHA256)?;
    let go_sum = source_file("restic-0.19.0/go.sum", recipe::GO_SUM_SHA256)?;
    inspect_modules(modules)?;
    let tools = inspect_go_tools(go)?;
    Ok(InspectedInputs { tools, go_sum })
}

// The official git archive carries one global PAX commit comment. Admit that
// exact metadata here; the general payload tar reader remains unchanged.
fn inspect_source_members(bytes: &[u8]) -> Result<Vec<crate::tar::MemberBytes>, String> {
    let mut archive = ::tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut members = Vec::new();
    let mut revision_seen = false;
    let mut seen = BTreeSet::new();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if kind.is_pax_global_extensions() {
            let mut body = Vec::new();
            entry.read_to_end(&mut body).map_err(|e| e.to_string())?;
            if revision_seen || body != format!("52 comment={}\n", recipe::REVISION).as_bytes() {
                return Err("restic source archive commit metadata differs".into());
            }
            revision_seen = true;
            continue;
        }
        if !kind.is_file() {
            return Err("restic source archive has a non-file member".into());
        }
        let path = std::str::from_utf8(&entry.path_bytes())
            .map_err(|e| e.to_string())?
            .to_owned();
        crate::archive::refuse_escape(&path).map_err(|e| e.as_str().to_owned())?;
        if !path.is_ascii() || path.contains('\\') || !seen.insert(path.clone()) {
            return Err("unsafe or duplicate restic source member".into());
        }
        let mut body = Vec::new();
        entry.read_to_end(&mut body).map_err(|e| e.to_string())?;
        members.push(crate::tar::MemberBytes { path, bytes: body });
    }
    if !revision_seen {
        return Err("restic source archive commit metadata absent".into());
    }
    Ok(members)
}

pub fn inspect_go_tools(bytes: &[u8]) -> Result<Vec<InputIdentityEntry>, String> {
    require_pin(bytes, &recipe::pinned_inputs()[1])?;
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let (mut zip, names) = crate::zip_names::prove_open_zip(zip).map_err(|e| e.to_string())?;
    let mut tools = Vec::new();
    for (index, path) in names.into_iter().enumerate() {
        if !(path.starts_with("go/bin/") || path.starts_with("go/pkg/tool/windows_amd64/"))
            || !path.ends_with(".exe")
        {
            continue;
        }
        let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
        if entry.is_dir() || entry.size() > 64 * 1024 * 1024 {
            return Err("invalid Go host executable".into());
        }
        let mut body = Vec::new();
        entry.read_to_end(&mut body).map_err(|e| e.to_string())?;
        if crate::pe_dependencies::inspect_dependencies(&body, WindowsArchitecture::X86_64)?.is_dll
        {
            return Err("Go host tool is a DLL".into());
        }
        tools.push(InputIdentityEntry {
            label: path,
            size: body.len() as u64,
            sha256: sha256_hex(&body),
        });
    }
    tools.sort_by(|a, b| a.label.cmp(&b.label));
    for required in [
        "go/bin/go.exe",
        "go/pkg/tool/windows_amd64/asm.exe",
        "go/pkg/tool/windows_amd64/compile.exe",
        "go/pkg/tool/windows_amd64/link.exe",
    ] {
        if !tools.iter().any(|t| t.label == required) {
            return Err(format!("sealed Go host tool missing: {required}"));
        }
    }
    Ok(tools)
}

fn inspect_modules(bytes: &[u8]) -> Result<(), String> {
    let mut members = BTreeMap::new();
    let mut folded = BTreeSet::new();
    for member in crate::tar::tar_members(bytes).map_err(|e| e.to_string())? {
        crate::archive::refuse_escape(&member.path).map_err(|e| e.as_str().to_owned())?;
        if !member.path.is_ascii()
            || member.path.contains('\\')
            || member.path.contains(':')
            || !folded.insert(member.path.to_ascii_lowercase())
            || members.insert(member.path, member.bytes).is_some()
        {
            return Err("unsafe or duplicate restic module member".into());
        }
    }
    let manifest_bytes = members
        .remove("offline-modules.json")
        .ok_or("restic module manifest missing")?;
    if sha256_hex(&manifest_bytes) != recipe::MODULE_MANIFEST_SHA256 {
        return Err("restic module manifest differs".into());
    }
    let manifest: ModuleManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| e.to_string())?;
    if manifest.schema != "solstone.restic-offline-modules.v1"
        || manifest.restic_version != recipe::VERSION
        || manifest.go_mod_sha256 != recipe::GO_MOD_SHA256
        || manifest.go_sum_sha256 != recipe::GO_SUM_SHA256
        || manifest.files.len() != members.len()
    {
        return Err("restic module source/census differs".into());
    }
    for item in manifest.files {
        let body = members
            .remove(&item.path)
            .ok_or("restic module member absent or duplicated")?;
        if body.len() as u64 != item.bytes || sha256_hex(&body) != item.sha256 {
            return Err(format!("changed restic offline module: {}", item.path));
        }
    }
    if !members.is_empty() {
        return Err("unexpected restic offline module member".into());
    }
    Ok(())
}

/// Validate the native producer's `go version -m` output against the source
/// checksums. The output path varies by exclusive build root and is not a pin.
pub fn validate_build_info(bytes: &[u8], go_sum: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let sum = std::str::from_utf8(go_sum).map_err(|e| e.to_string())?;
    if !text
        .lines()
        .next()
        .is_some_and(|line| line.ends_with(": go1.26.4"))
    {
        return Err("restic binary Go version differs".into());
    }
    let lines: BTreeSet<_> = text.lines().map(str::trim).collect();
    for required in [
        "path\tgithub.com/restic/restic/cmd/restic",
        "build\t-buildmode=exe",
        "build\t-compiler=gc",
        "build\t-tags=selfupdate,disable_grpc_modules",
        "build\t-trimpath=true",
        "build\tDefaultGODEBUG=cryptocustomrand=1,tlssecpmlkem=0,urlstrictcolons=0,winsymlink=0",
        "build\tCGO_ENABLED=0",
        "build\tGOARCH=arm64",
        "build\tGOOS=windows",
        "build\tGOARM64=v8.0",
    ] {
        if !lines.contains(required) {
            return Err(format!("restic binary build setting absent: {required}"));
        }
    }
    let mut dependency_count = 0;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.first() == Some(&"=>") {
            return Err("restic binary has an unqualified module replacement".into());
        }
        if fields.first() != Some(&"dep") {
            continue;
        }
        if fields.len() != 4
            || !sum
                .lines()
                .any(|entry| entry == format!("{} {} {}", fields[1], fields[2], fields[3]))
        {
            return Err("restic binary module differs from pinned source checksums".into());
        }
        dependency_count += 1;
    }
    if dependency_count == 0 {
        return Err("restic binary has no dependency build information".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_reader_admits_only_the_exact_global_commit_and_regular_files() {
        let archive = |revision: &str, link: bool| {
            let mut tar = ::tar::Builder::new(Vec::new());
            let comment = format!("52 comment={revision}\n");
            let mut header = ::tar::Header::new_gnu();
            header.set_entry_type(::tar::EntryType::XGlobalHeader);
            header.set_size(comment.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "pax_global_header", comment.as_bytes())
                .unwrap();
            let mut header = ::tar::Header::new_gnu();
            header.set_entry_type(if link {
                ::tar::EntryType::Symlink
            } else {
                ::tar::EntryType::Regular
            });
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "restic-0.19.0/go.mod", &[][..])
                .unwrap();
            crate::tar::gzip_bytes(&tar.into_inner().unwrap()).unwrap()
        };
        let members = inspect_source_members(&archive(recipe::REVISION, false)).unwrap();
        assert_eq!(members[0].path, "restic-0.19.0/go.mod");
        assert!(inspect_source_members(&archive(&"f".repeat(40), false)).is_err());
        assert!(inspect_source_members(&archive(recipe::REVISION, true)).is_err());
    }
    #[test]
    fn archive_identity_refuses_unpinned_bytes_before_container_parsing() {
        assert!(
            inspect(
                b"source substitution",
                b"tool substitution",
                b"module substitution"
            )
            .unwrap_err()
            .contains(recipe::SOURCE_LABEL)
        );
        assert!(
            inspect_go_tools(b"not a ZIP")
                .unwrap_err()
                .contains(recipe::GO_LABEL)
        );
    }
    #[test]
    fn build_information_binds_target_tags_and_module_checksums() {
        let text = "C:\\output\\restic.exe: go1.26.4\n\tpath\tgithub.com/restic/restic/cmd/restic\n\tdep\texample.test/mod\tv1.2.3\th1:fixture\n\tbuild\t-buildmode=exe\n\tbuild\t-compiler=gc\n\tbuild\t-tags=selfupdate,disable_grpc_modules\n\tbuild\t-trimpath=true\n\tbuild\tDefaultGODEBUG=cryptocustomrand=1,tlssecpmlkem=0,urlstrictcolons=0,winsymlink=0\n\tbuild\tCGO_ENABLED=0\n\tbuild\tGOARCH=arm64\n\tbuild\tGOOS=windows\n\tbuild\tGOARM64=v8.0\n";
        let sum = b"example.test/mod v1.2.3 h1:fixture\n";
        validate_build_info(text.as_bytes(), sum).unwrap();
        for (old, new) in [
            ("GOARCH=arm64", "GOARCH=amd64"),
            ("selfupdate,disable_grpc_modules", "selfupdate"),
            ("go1.26.4", "go1.26.5"),
            ("h1:fixture", "h1:foreign"),
            ("winsymlink=0", "winsymlink=1"),
        ] {
            assert!(validate_build_info(text.replace(old, new).as_bytes(), sum).is_err());
        }
        assert!(
            validate_build_info(
                format!("{text}\t=>\tforeign.test/replacement\tv9.0.0\n").as_bytes(),
                sum
            )
            .is_err()
        );
    }
}
