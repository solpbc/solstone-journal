// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use crate::digest::sha256_hex;
use crate::inventory::StagedMember;
use crate::produce::ProduceError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedPin {
    pub sha256_hex: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemberPlan {
    pub inner_path: String,
    pub alias: Option<String>,
    pub dest: String,
    pub mode: u32,
    pub extracted_sha256: String,
}

#[derive(Debug, Clone)]
enum ArchiveEntryKind {
    Regular,
    Symlink(String),
    NonRegular,
}

pub(crate) fn plan_pinned_input(
    entry: &str,
    bytes: &[u8],
    pin: &ResolvedPin,
    filename: &str,
    staged: &[StagedMember],
    ignored: &[String],
) -> Result<Vec<MemberPlan>, ProduceError> {
    if bytes.len() as u64 != pin.size {
        return Err(ProduceError::new(format!(
            "{entry}: size mismatch: expected {}, got {}",
            pin.size,
            bytes.len()
        )));
    }

    let expected_hex = pin
        .sha256_hex
        .strip_prefix("sha256:")
        .unwrap_or(&pin.sha256_hex);
    let actual_hex = sha256_hex(bytes);
    if !actual_hex.eq_ignore_ascii_case(expected_hex) {
        return Err(ProduceError::new(format!(
            "{entry}: pin sha256 mismatch: expected sha256:{expected_hex}, got sha256:{actual_hex}"
        )));
    }

    let members = collect_archive_members(entry, bytes, filename)?;

    let mut staged_set = BTreeSet::new();
    for s in staged {
        if !staged_set.insert(&s.relpath) {
            return Err(ProduceError::new(format!(
                "{entry}: duplicate staged member relpath {}",
                s.relpath
            )));
        }
    }

    let mut ignored_set = BTreeSet::new();
    for ign in ignored {
        if !ignored_set.insert(ign) {
            return Err(ProduceError::new(format!(
                "{entry}: duplicate ignored relpath {ign}"
            )));
        }
        if staged_set.contains(ign) {
            return Err(ProduceError::new(format!(
                "{entry}: member {ign} is both staged and ignored"
            )));
        }
    }

    for s in staged {
        if !members.contains_key(&s.relpath) {
            return Err(ProduceError::new(format!(
                "{entry}: missing staged member relpath {}",
                s.relpath
            )));
        }
    }

    for ign in ignored {
        if !members.contains_key(ign) {
            return Err(ProduceError::new(format!(
                "{entry}: missing ignored member {ign}"
            )));
        }
    }

    for (member_name, kind) in &members {
        if matches!(kind, ArchiveEntryKind::NonRegular) {
            continue;
        }
        if !staged_set.contains(member_name) && !ignored_set.contains(member_name) {
            return Err(ProduceError::new(format!(
                "{entry}: undeclared archive member {member_name}"
            )));
        }
    }

    let mut plans = Vec::new();
    for s in staged {
        let kind = &members[&s.relpath];
        match kind {
            ArchiveEntryKind::Regular => {
                plans.push(MemberPlan {
                    inner_path: s.relpath.clone(),
                    alias: None,
                    dest: s.dest.clone(),
                    mode: s.mode,
                    extracted_sha256: s.extracted_sha256.clone(),
                });
            }
            ArchiveEntryKind::Symlink(target) => {
                let inner_path = resolve_symlink(entry, &s.relpath, target, &members)?;
                plans.push(MemberPlan {
                    inner_path,
                    alias: Some(s.relpath.clone()),
                    dest: s.dest.clone(),
                    mode: s.mode,
                    extracted_sha256: s.extracted_sha256.clone(),
                });
            }
            ArchiveEntryKind::NonRegular => {
                return Err(ProduceError::new(format!(
                    "{entry}: staged member {} is not a regular file",
                    s.relpath
                )));
            }
        }
    }

    Ok(plans)
}

fn collect_archive_members(
    entry: &str,
    bytes: &[u8],
    filename: &str,
) -> Result<BTreeMap<String, ArchiveEntryKind>, ProduceError> {
    let mut members = BTreeMap::new();

    if filename.ends_with(".tar.gz") {
        let gz = flate2::read::GzDecoder::new(bytes);
        let mut tar = tar::Archive::new(gz);
        parse_tar_members(entry, &mut tar, &mut members)?;
    } else if filename.ends_with(".tar.xz") {
        let mut decompressed = Vec::new();
        lzma_rs::xz_decompress(&mut &bytes[..], &mut decompressed).map_err(|e| {
            ProduceError::new(format!("{entry}: xz decompress failed for {filename}: {e}"))
        })?;
        let mut tar = tar::Archive::new(&decompressed[..]);
        parse_tar_members(entry, &mut tar, &mut members)?;
    } else if filename.ends_with(".zip") {
        let cursor = std::io::Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(cursor).map_err(|e| {
            ProduceError::new(format!("{entry}: zip open failed for {filename}: {e}"))
        })?;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| {
                ProduceError::new(format!("{entry}: zip read failed at index {i}: {e}"))
            })?;
            let name = file.name().to_string();
            let norm_name = normalize_archive_path(&name);
            if norm_name.is_empty() {
                continue;
            }
            if file.is_dir() || name.ends_with('/') {
                members
                    .entry(norm_name)
                    .or_insert(ArchiveEntryKind::NonRegular);
                continue;
            }
            if members.contains_key(&norm_name) {
                return Err(ProduceError::new(format!(
                    "{entry}: duplicate member {norm_name}"
                )));
            }
            let mode = file.unix_mode().unwrap_or(0);
            if (mode & 0o170000) == 0o120000 {
                let mut target = String::new();
                file.read_to_string(&mut target).map_err(|e| {
                    ProduceError::new(format!("{entry}: zip read symlink target {norm_name}: {e}"))
                })?;
                members.insert(norm_name, ArchiveEntryKind::Symlink(target));
            } else if (mode & 0o170000) != 0 && (mode & 0o170000) != 0o100000 {
                members.insert(norm_name, ArchiveEntryKind::NonRegular);
            } else {
                members.insert(norm_name, ArchiveEntryKind::Regular);
            }
        }
    } else if filename.ends_with(".bz2") && !filename.ends_with(".tar.bz2") {
        let mut decoder = bzip2_rs::DecoderReader::new(bytes);
        let mut _sink = Vec::new();
        decoder.read_to_end(&mut _sink).map_err(|e| {
            ProduceError::new(format!(
                "{entry}: bz2 decompress failed for {filename}: {e}"
            ))
        })?;
        members.insert(String::new(), ArchiveEntryKind::Regular);
    } else {
        members.insert(String::new(), ArchiveEntryKind::Regular);
    }

    Ok(members)
}

fn parse_tar_members<R: Read>(
    entry: &str,
    tar: &mut tar::Archive<R>,
    members: &mut BTreeMap<String, ArchiveEntryKind>,
) -> Result<(), ProduceError> {
    let entries = tar
        .entries()
        .map_err(|e| ProduceError::new(format!("{entry}: tar entries read failed: {e}")))?;
    for entry_res in entries {
        let entry_file = entry_res
            .map_err(|e| ProduceError::new(format!("{entry}: tar entry read failed: {e}")))?;
        let header = entry_file.header();
        let entry_type = header.entry_type();
        let path = entry_file
            .path()
            .map_err(|e| ProduceError::new(format!("{entry}: tar path read failed: {e}")))?;
        let path_str = path
            .to_str()
            .ok_or_else(|| ProduceError::new(format!("{entry}: tar path is not valid utf-8")))?;
        let norm_name = normalize_archive_path(path_str);
        if norm_name.is_empty() {
            continue;
        }
        if entry_type == tar::EntryType::Directory {
            members
                .entry(norm_name)
                .or_insert(ArchiveEntryKind::NonRegular);
            continue;
        }
        if members.contains_key(&norm_name) {
            return Err(ProduceError::new(format!(
                "{entry}: duplicate member {norm_name}"
            )));
        }
        if entry_type == tar::EntryType::Symlink {
            let link_target = entry_file
                .link_name()
                .map_err(|e| {
                    ProduceError::new(format!("{entry}: tar link_name read {norm_name}: {e}"))
                })?
                .ok_or_else(|| {
                    ProduceError::new(format!("{entry}: missing symlink target for {norm_name}"))
                })?;
            let link_str = link_target
                .to_str()
                .ok_or_else(|| {
                    ProduceError::new(format!(
                        "{entry}: symlink target for {norm_name} is not valid utf-8"
                    ))
                })?
                .to_owned();
            members.insert(norm_name, ArchiveEntryKind::Symlink(link_str));
        } else if entry_type == tar::EntryType::Regular || entry_type.is_file() {
            members.insert(norm_name, ArchiveEntryKind::Regular);
        } else {
            return Err(ProduceError::new(format!(
                "{entry}: unsupported tar entry type for {norm_name}"
            )));
        }
    }
    Ok(())
}

fn normalize_archive_path(p: &str) -> String {
    let mut s = p.trim_start_matches('/');
    while let Some(stripped) = s.strip_prefix("./") {
        s = stripped.trim_start_matches('/');
    }
    s.trim_end_matches('/').to_string()
}

fn resolve_symlink(
    entry: &str,
    alias: &str,
    target: &str,
    members: &BTreeMap<String, ArchiveEntryKind>,
) -> Result<String, ProduceError> {
    if target.is_empty()
        || target.starts_with('/')
        || target.starts_with('\\')
        || (target.len() > 1 && target.as_bytes()[1] == b':')
    {
        return Err(ProduceError::new(format!(
            "{entry}: symlink {alias} has absolute target {target}"
        )));
    }
    let parent = Path::new(alias).parent().unwrap_or_else(|| Path::new(""));
    let mut parts: Vec<&str> = parent
        .iter()
        .filter_map(|p| p.to_str())
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    for seg in target.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            if parts.is_empty() {
                return Err(ProduceError::new(format!(
                    "{entry}: symlink {alias} target escapes archive root: {target}"
                )));
            }
            parts.pop();
        } else {
            parts.push(seg);
        }
    }
    let resolved = parts.join("/");
    let target_kind = members.get(&resolved).ok_or_else(|| {
        ProduceError::new(format!(
            "{entry}: symlink {alias} has dangling target {target} (resolved to {resolved})"
        ))
    })?;
    match target_kind {
        ArchiveEntryKind::Symlink(_) => Err(ProduceError::new(format!(
            "{entry}: symlink {alias} points to symlink {resolved} (symlink-to-symlink chain)"
        ))),
        ArchiveEntryKind::NonRegular => Err(ProduceError::new(format!(
            "{entry}: symlink {alias} target is not a regular file: {resolved}"
        ))),
        ArchiveEntryKind::Regular => Ok(resolved),
    }
}

pub(crate) fn extract_and_verify_plans(
    entry: &str,
    bytes: &[u8],
    filename: &str,
    plans: &[MemberPlan],
) -> Result<BTreeMap<String, Vec<u8>>, ProduceError> {
    let needed_paths: BTreeSet<&str> = plans.iter().map(|p| p.inner_path.as_str()).collect();
    let contents = extract_needed_members(entry, bytes, filename, &needed_paths)?;

    for plan in plans {
        let file_bytes = contents.get(&plan.inner_path).ok_or_else(|| {
            ProduceError::new(format!(
                "{entry}: missing extracted member bytes for {}",
                plan.inner_path
            ))
        })?;
        let expected_sha = plan
            .extracted_sha256
            .strip_prefix("sha256:")
            .unwrap_or(&plan.extracted_sha256);
        let actual_sha = sha256_hex(file_bytes);
        if !actual_sha.eq_ignore_ascii_case(expected_sha) {
            return Err(ProduceError::new(format!(
                "{entry}: extracted sha256 mismatch for {}: expected sha256:{expected_sha}, got sha256:{actual_sha}",
                plan.dest
            )));
        }
    }

    Ok(contents)
}

pub(crate) fn stage_pinned_plans(
    entry: &str,
    stage_dir: &Path,
    bytes: &[u8],
    filename: &str,
    plans: &[MemberPlan],
) -> Result<(), ProduceError> {
    let contents = extract_and_verify_plans(entry, bytes, filename, plans)?;
    for plan in plans {
        let file_bytes = &contents[&plan.inner_path];
        crate::stage::write_staged_file_mode(stage_dir, &plan.dest, file_bytes, plan.mode)?;
    }
    Ok(())
}

pub(crate) fn extract_needed_members(
    entry: &str,
    bytes: &[u8],
    filename: &str,
    needed: &BTreeSet<&str>,
) -> Result<BTreeMap<String, Vec<u8>>, ProduceError> {
    let mut contents = BTreeMap::new();

    if filename.ends_with(".tar.gz") {
        let gz = flate2::read::GzDecoder::new(bytes);
        let mut tar = tar::Archive::new(gz);
        extract_tar_members(entry, &mut tar, needed, &mut contents)?;
    } else if filename.ends_with(".tar.xz") {
        let mut decompressed = Vec::new();
        lzma_rs::xz_decompress(&mut &bytes[..], &mut decompressed).map_err(|e| {
            ProduceError::new(format!("{entry}: xz decompress failed for {filename}: {e}"))
        })?;
        let mut tar = tar::Archive::new(&decompressed[..]);
        extract_tar_members(entry, &mut tar, needed, &mut contents)?;
    } else if filename.ends_with(".zip") {
        let cursor = std::io::Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(cursor).map_err(|e| {
            ProduceError::new(format!("{entry}: zip open failed for {filename}: {e}"))
        })?;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| {
                ProduceError::new(format!("{entry}: zip read failed at index {i}: {e}"))
            })?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().to_string();
            let norm_name = normalize_archive_path(&name);
            if needed.contains(norm_name.as_str()) {
                let mut data = Vec::new();
                file.read_to_end(&mut data).map_err(|e| {
                    ProduceError::new(format!("{entry}: zip extract {norm_name}: {e}"))
                })?;
                contents.insert(norm_name, data);
            }
        }
    } else if filename.ends_with(".bz2") && !filename.ends_with(".tar.bz2") {
        if needed.contains("") {
            let mut decoder = bzip2_rs::DecoderReader::new(bytes);
            let mut data = Vec::new();
            decoder.read_to_end(&mut data).map_err(|e| {
                ProduceError::new(format!(
                    "{entry}: bz2 decompress failed for {filename}: {e}"
                ))
            })?;
            contents.insert(String::new(), data);
        }
    } else {
        if needed.contains("") {
            contents.insert(String::new(), bytes.to_vec());
        }
    }

    Ok(contents)
}

fn extract_tar_members<R: Read>(
    entry: &str,
    tar: &mut tar::Archive<R>,
    needed: &BTreeSet<&str>,
    contents: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), ProduceError> {
    let entries = tar
        .entries()
        .map_err(|e| ProduceError::new(format!("{entry}: tar entries read failed: {e}")))?;
    for entry_res in entries {
        let mut entry_file = entry_res
            .map_err(|e| ProduceError::new(format!("{entry}: tar entry read failed: {e}")))?;
        let header = entry_file.header();
        let entry_type = header.entry_type();
        if entry_type == tar::EntryType::Directory {
            continue;
        }
        let path = entry_file
            .path()
            .map_err(|e| ProduceError::new(format!("{entry}: tar path read failed: {e}")))?;
        let path_str = path
            .to_str()
            .ok_or_else(|| ProduceError::new(format!("{entry}: tar path is not valid utf-8")))?;
        let norm_name = normalize_archive_path(path_str);
        if needed.contains(norm_name.as_str()) {
            let mut data = Vec::new();
            entry_file
                .read_to_end(&mut data)
                .map_err(|e| ProduceError::new(format!("{entry}: tar extract {norm_name}: {e}")))?;
            contents.insert(norm_name, data);
        }
    }
    Ok(())
}

pub(crate) fn resolve_pinned_input(
    entry: &str,
    repo: &Path,
    target_id: &str,
    input: &crate::inventory::PinnedInput,
) -> Result<(Vec<u8>, ResolvedPin, String), ProduceError> {
    resolve_pinned_input_with_catalog(
        entry,
        repo,
        target_id,
        input,
        solstone_core_assets::catalog(),
    )
}

pub(crate) fn resolve_pinned_input_with_catalog(
    entry: &str,
    repo: &Path,
    target_id: &str,
    input: &crate::inventory::PinnedInput,
    catalog: &[solstone_core_assets::Artifact],
) -> Result<(Vec<u8>, ResolvedPin, String), ProduceError> {
    match input {
        crate::inventory::PinnedInput::Inline { source, digest } => {
            let file_path = repo.join(source);
            let bytes = std::fs::read(&file_path).map_err(|e| {
                ProduceError::new(format!(
                    "{entry}: failed to read {}: {e}",
                    file_path.display()
                ))
            })?;
            let filename = Path::new(source)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(source)
                .to_string();
            let size = bytes.len() as u64;
            let pin = ResolvedPin {
                sha256_hex: digest.clone(),
                size,
            };
            Ok((bytes, pin, filename))
        }
        crate::inventory::PinnedInput::CatalogCommitted {
            unit,
            filename,
            path,
        } => {
            let artifact = catalog
                .iter()
                .find(|a| a.unit == unit && a.filename == filename)
                .ok_or_else(|| {
                    ProduceError::new(format!(
                        "{entry}: catalog item not found for ({unit}, {filename})"
                    ))
                })?;
            if artifact.extracted_binary_sha256.is_some() {
                return Err(ProduceError::new(format!(
                    "{entry}: catalog item ({unit}, {filename}) has extracted_binary_sha256: Some, which is forbidden"
                )));
            }
            let file_path = repo.join(path);
            let bytes = std::fs::read(&file_path).map_err(|e| {
                ProduceError::new(format!(
                    "{entry}: failed to read {}: {e}",
                    file_path.display()
                ))
            })?;
            if bytes.len() as u64 != artifact.size_bytes
                || !sha256_hex(&bytes).eq_ignore_ascii_case(artifact.sha256)
            {
                return Err(ProduceError::new(format!(
                    "{entry}: catalog committed file verification failed for ({unit}, {filename})"
                )));
            }
            let pin = ResolvedPin {
                sha256_hex: artifact.sha256.to_string(),
                size: artifact.size_bytes,
            };
            Ok((bytes, pin, filename.clone()))
        }
        crate::inventory::PinnedInput::CatalogAcquired { unit, filename } => {
            let artifact = catalog
                .iter()
                .find(|a| a.unit == unit && a.filename == filename)
                .ok_or_else(|| {
                    ProduceError::new(format!(
                        "{entry}: catalog item not found for ({unit}, {filename})"
                    ))
                })?;
            if artifact.extracted_binary_sha256.is_some() {
                return Err(ProduceError::new(format!(
                    "{entry}: catalog item ({unit}, {filename}) has extracted_binary_sha256: Some, which is forbidden"
                )));
            }
            crate::acquire::check_cache_path_components(unit, artifact.version, filename)
                .map_err(|e| ProduceError::new(format!("{entry}: {e}")))?;
            let cache_path = repo
                .join("target/catalog-input-cache")
                .join(unit)
                .join(artifact.version)
                .join(filename);
            let bytes = match std::fs::read(&cache_path) {
                Ok(b) => b,
                Err(_) => {
                    return Err(ProduceError::new(format!(
                        "{entry}: missing catalog input cache file for ({unit}, {filename}) at {}; run `solstone-distribution acquire catalog-inputs --target {target_id}`",
                        cache_path.display()
                    )));
                }
            };
            if bytes.len() as u64 != artifact.size_bytes
                || !sha256_hex(&bytes).eq_ignore_ascii_case(artifact.sha256)
            {
                return Err(ProduceError::new(format!(
                    "{entry}: catalog input cache verification failed for ({unit}, {filename}); run `solstone-distribution acquire catalog-inputs --target {target_id}`"
                )));
            }
            let pin = ResolvedPin {
                sha256_hex: artifact.sha256.to_string(),
                size: artifact.size_bytes,
            };
            Ok((bytes, pin, filename.clone()))
        }
        crate::inventory::PinnedInput::AuthorityCommitted { platform, path } => {
            let authority = solstone_core_nvattest_authority::parse(
                solstone_core_nvattest_authority::AUTHORITY_JSON,
            )
            .map_err(|e| ProduceError::new(e.to_string()))?;
            let spec = solstone_core_nvattest_authority::artifact_spec(&authority, platform)
                .map_err(|e| ProduceError::new(e.to_string()))?;
            let file_path = repo.join(path);
            let bytes = std::fs::read(&file_path).map_err(|e| {
                ProduceError::new(format!(
                    "{entry}: failed to read {}: {e}",
                    file_path.display()
                ))
            })?;
            if bytes.len() as u64 != spec.size_bytes
                || !sha256_hex(&bytes).eq_ignore_ascii_case(&spec.sha256)
            {
                return Err(ProduceError::new(format!(
                    "{entry}: authority committed file verification failed for {platform}"
                )));
            }
            let pin = ResolvedPin {
                sha256_hex: spec.sha256.clone(),
                size: spec.size_bytes,
            };
            Ok((bytes, pin, spec.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const EM_X86_64: u16 = 62;

    const TEST_BZ2: &[u8] = &[
        66, 90, 104, 57, 49, 65, 89, 38, 83, 89, 252, 208, 76, 212, 0, 0, 2, 25, 128, 64, 0, 16, 0,
        18, 68, 128, 16, 32, 0, 49, 12, 8, 32, 15, 40, 54, 104, 195, 226, 238, 72, 167, 10, 18, 31,
        154, 9, 154, 128,
    ];

    fn test_staged_member(
        relpath: &str,
        dest: &str,
        mode: u32,
        extracted_sha256: &str,
    ) -> StagedMember {
        StagedMember {
            relpath: relpath.into(),
            dest: dest.into(),
            mode,
            extracted_sha256: extracted_sha256.into(),
            identity: None,
        }
    }

    fn assert_no_symlinks_recursive(dir: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let ft = entry.file_type().unwrap();
            assert!(!ft.is_symlink(), "symlink found at {:?}", entry.path());
            if ft.is_dir() {
                assert_no_symlinks_recursive(&entry.path());
            }
        }
    }

    fn make_tar_gz(entries: &[(&str, &[u8], tar::EntryType, Option<&str>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data, entry_type, link_target) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*entry_type);
            header.set_mode(0o644);
            if let Some(target) = link_target {
                header.set_size(0);
                header.set_link_name(target).unwrap();
            } else {
                header.set_size(data.len() as u64);
            }
            header.set_cksum();
            builder.append_data(&mut header, *path, *data).unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn make_tar_xz(entries: &[(&str, &[u8], tar::EntryType, Option<&str>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data, entry_type, link_target) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*entry_type);
            header.set_mode(0o644);
            if let Some(target) = link_target {
                header.set_size(0);
                header.set_link_name(target).unwrap();
            } else {
                header.set_size(data.len() as u64);
            }
            header.set_cksum();
            builder.append_data(&mut header, *path, *data).unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut xz_bytes = Vec::new();
        lzma_rs::xz_compress(&mut &tar_bytes[..], &mut xz_bytes).unwrap();
        xz_bytes
    }

    fn make_zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        for (path, data, mode) in entries {
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(*mode);
            zip.start_file(*path, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        let mut bytes = zip.finish().unwrap().into_inner();
        let cd_sig = [0x50, 0x4b, 0x01, 0x02];
        for (path, _, mode) in entries {
            let path_bytes = path.as_bytes();
            let mut i = 0;
            while i + 46 + path_bytes.len() <= bytes.len() {
                if bytes[i..i + 4] == cd_sig {
                    let name_len = u16::from_le_bytes([bytes[i + 28], bytes[i + 29]]) as usize;
                    if name_len == path_bytes.len()
                        && &bytes[i + 46..i + 46 + name_len] == path_bytes
                    {
                        bytes[i + 5] = 3; // Unix
                        let mode_bytes = (*mode << 16).to_le_bytes();
                        bytes[i + 38..i + 42].copy_from_slice(&mode_bytes);
                        break;
                    }
                }
                i += 1;
            }
        }
        bytes
    }

    #[test]
    fn plan_pinned_input_formats_and_refusals() {
        let plain_bytes = b"hello plain contents";
        let plain_pin = ResolvedPin {
            sha256_hex: sha256_hex(plain_bytes),
            size: plain_bytes.len() as u64,
        };

        // Size mismatch names entry
        let bad_size_pin = ResolvedPin {
            sha256_hex: plain_pin.sha256_hex.clone(),
            size: 999,
        };
        let err = plan_pinned_input(
            "entry-size-err",
            plain_bytes,
            &bad_size_pin,
            "test.bin",
            &[],
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("entry-size-err"));
        assert!(err.to_string().contains("size mismatch"));

        // SHA mismatch names entry
        let bad_sha_pin = ResolvedPin {
            sha256_hex: "00".repeat(32),
            size: plain_bytes.len() as u64,
        };
        let err = plan_pinned_input(
            "entry-sha-err",
            plain_bytes,
            &bad_sha_pin,
            "test.bin",
            &[],
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("entry-sha-err"));
        assert!(err.to_string().contains("pin sha256 mismatch"));

        // Missing staged member names entry
        let missing_staged = vec![test_staged_member(
            "nonexistent",
            "dest",
            0o644,
            &"00".repeat(32),
        )];
        let err = plan_pinned_input(
            "entry-missing-err",
            plain_bytes,
            &plain_pin,
            "test.bin",
            &missing_staged,
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("entry-missing-err"));
        assert!(err.to_string().contains("missing staged member"));

        // Undeclared archive member names entry
        let err = plan_pinned_input(
            "entry-undeclared-err",
            plain_bytes,
            &plain_pin,
            "test.bin",
            &[],
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("entry-undeclared-err"));
        assert!(err.to_string().contains("undeclared archive member"));

        // Extracted sha mismatch names entry
        let staged = vec![test_staged_member("", "out.bin", 0o644, &"00".repeat(32))];
        let plans = plan_pinned_input(
            "entry-extr-err",
            plain_bytes,
            &plain_pin,
            "test.bin",
            &staged,
            &[],
        )
        .unwrap();
        let err = extract_and_verify_plans("entry-extr-err", plain_bytes, "test.bin", &plans)
            .unwrap_err();
        assert!(err.to_string().contains("entry-extr-err"));
        assert!(err.to_string().contains("extracted sha256 mismatch"));

        // .bz2 format
        let bz2_pin = ResolvedPin {
            sha256_hex: sha256_hex(TEST_BZ2),
            size: TEST_BZ2.len() as u64,
        };
        let bz2_staged = vec![test_staged_member(
            "",
            "out.txt",
            0o644,
            &sha256_hex(b"hello bz2"),
        )];
        let bz2_plans = plan_pinned_input(
            "entry-bz2",
            TEST_BZ2,
            &bz2_pin,
            "test.bz2",
            &bz2_staged,
            &[],
        )
        .unwrap();
        let bz2_extracted =
            extract_and_verify_plans("entry-bz2", TEST_BZ2, "test.bz2", &bz2_plans).unwrap();
        assert_eq!(bz2_extracted[""], b"hello bz2");

        // Duplicate member in zip names entry
        let dup_zip = make_zip(&[
            ("file1.txt", b"a", 0o100644),
            ("./file1.txt", b"b", 0o100644),
        ]);
        let dup_zip_pin = ResolvedPin {
            sha256_hex: sha256_hex(&dup_zip),
            size: dup_zip.len() as u64,
        };
        let err = plan_pinned_input(
            "entry-zip-dup",
            &dup_zip,
            &dup_zip_pin,
            "test.zip",
            &[],
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("entry-zip-dup"));
        assert!(err.to_string().contains("duplicate member file1.txt"));

        // Duplicate member in tar names entry
        let dup_tar = make_tar_gz(&[
            ("dup.txt", b"a", tar::EntryType::Regular, None),
            ("dup.txt", b"b", tar::EntryType::Regular, None),
        ]);
        let dup_tar_pin = ResolvedPin {
            sha256_hex: sha256_hex(&dup_tar),
            size: dup_tar.len() as u64,
        };
        let err = plan_pinned_input(
            "entry-tar-dup",
            &dup_tar,
            &dup_tar_pin,
            "test.tar.gz",
            &[],
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("entry-tar-dup"));
        assert!(err.to_string().contains("duplicate member dup.txt"));

        // .tar.xz format
        let xz_tar = make_tar_xz(&[("xz_file.txt", b"xz data", tar::EntryType::Regular, None)]);
        let xz_pin = ResolvedPin {
            sha256_hex: sha256_hex(&xz_tar),
            size: xz_tar.len() as u64,
        };
        let xz_staged = vec![test_staged_member(
            "xz_file.txt",
            "staged/xz.txt",
            0o644,
            &sha256_hex(b"xz data"),
        )];
        let xz_plans =
            plan_pinned_input("entry-xz", &xz_tar, &xz_pin, "test.tar.xz", &xz_staged, &[])
                .unwrap();
        let xz_extracted =
            extract_and_verify_plans("entry-xz", &xz_tar, "test.tar.xz", &xz_plans).unwrap();
        assert_eq!(xz_extracted["xz_file.txt"], b"xz data");
    }

    #[test]
    fn alias_symlink_resolution_and_refusals() {
        let zip_bytes = make_zip(&[
            ("target.txt", b"target content", 0o100644),
            ("link.txt", b"target.txt", 0o120777),
        ]);
        let zip_pin = ResolvedPin {
            sha256_hex: sha256_hex(&zip_bytes),
            size: zip_bytes.len() as u64,
        };
        let staged = vec![test_staged_member(
            "link.txt",
            "resolved/link.txt",
            0o644,
            &sha256_hex(b"target content"),
        )];
        let ignored = vec!["target.txt".into()];
        let plans = plan_pinned_input(
            "zip-alias",
            &zip_bytes,
            &zip_pin,
            "archive.zip",
            &staged,
            &ignored,
        )
        .unwrap();
        assert_eq!(plans[0].inner_path, "target.txt");
        assert_eq!(plans[0].alias.as_deref(), Some("link.txt"));

        let stage_dir = tempfile::tempdir().unwrap();
        stage_pinned_plans(
            "zip-alias",
            stage_dir.path(),
            &zip_bytes,
            "archive.zip",
            &plans,
        )
        .unwrap();
        assert_no_symlinks_recursive(stage_dir.path());
        assert_eq!(
            std::fs::read(stage_dir.path().join("resolved/link.txt")).unwrap(),
            b"target content"
        );

        // Tar symlink resolution
        let tar_bytes = make_tar_gz(&[
            (
                "target.txt",
                b"tar target content",
                tar::EntryType::Regular,
                None,
            ),
            (
                "tar_link.txt",
                b"",
                tar::EntryType::Symlink,
                Some("target.txt"),
            ),
        ]);
        let tar_pin = ResolvedPin {
            sha256_hex: sha256_hex(&tar_bytes),
            size: tar_bytes.len() as u64,
        };
        let tar_staged = vec![test_staged_member(
            "tar_link.txt",
            "resolved/tar_link.txt",
            0o644,
            &sha256_hex(b"tar target content"),
        )];
        let tar_ignored = vec!["target.txt".into()];
        let tar_plans = plan_pinned_input(
            "tar-alias",
            &tar_bytes,
            &tar_pin,
            "archive.tar.gz",
            &tar_staged,
            &tar_ignored,
        )
        .unwrap();
        assert_eq!(tar_plans[0].inner_path, "target.txt");
        assert_eq!(tar_plans[0].alias.as_deref(), Some("tar_link.txt"));

        let stage_dir_tar = tempfile::tempdir().unwrap();
        stage_pinned_plans(
            "tar-alias",
            stage_dir_tar.path(),
            &tar_bytes,
            "archive.tar.gz",
            &tar_plans,
        )
        .unwrap();
        assert_no_symlinks_recursive(stage_dir_tar.path());
        assert_eq!(
            std::fs::read(stage_dir_tar.path().join("resolved/tar_link.txt")).unwrap(),
            b"tar target content"
        );

        // Refusals: dangling target
        let dangling_tar = make_tar_gz(&[(
            "dangling.txt",
            b"",
            tar::EntryType::Symlink,
            Some("nonexistent.txt"),
        )]);
        let dangling_pin = ResolvedPin {
            sha256_hex: sha256_hex(&dangling_tar),
            size: dangling_tar.len() as u64,
        };
        let dangling_staged = vec![test_staged_member(
            "dangling.txt",
            "dest.txt",
            0o644,
            &"00".repeat(32),
        )];
        let err = plan_pinned_input(
            "dangling-entry",
            &dangling_tar,
            &dangling_pin,
            "dangling.tar.gz",
            &dangling_staged,
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("dangling-entry"));
        assert!(err.to_string().contains("dangling.txt"));
        assert!(err.to_string().contains("dangling target"));

        // Refusals: absolute target
        let abs_tar =
            make_tar_gz(&[("abs.txt", b"", tar::EntryType::Symlink, Some("/etc/passwd"))]);
        let abs_pin = ResolvedPin {
            sha256_hex: sha256_hex(&abs_tar),
            size: abs_tar.len() as u64,
        };
        let abs_staged = vec![test_staged_member(
            "abs.txt",
            "dest.txt",
            0o644,
            &"00".repeat(32),
        )];
        let err = plan_pinned_input(
            "abs-entry",
            &abs_tar,
            &abs_pin,
            "abs.tar.gz",
            &abs_staged,
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("abs-entry"));
        assert!(err.to_string().contains("abs.txt"));
        assert!(err.to_string().contains("absolute target"));

        // Refusals: .. escape
        let escape_tar = make_tar_gz(&[(
            "escape.txt",
            b"",
            tar::EntryType::Symlink,
            Some("../secret"),
        )]);
        let escape_pin = ResolvedPin {
            sha256_hex: sha256_hex(&escape_tar),
            size: escape_tar.len() as u64,
        };
        let escape_staged = vec![test_staged_member(
            "escape.txt",
            "dest.txt",
            0o644,
            &"00".repeat(32),
        )];
        let err = plan_pinned_input(
            "escape-entry",
            &escape_tar,
            &escape_pin,
            "escape.tar.gz",
            &escape_staged,
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("escape-entry"));
        assert!(err.to_string().contains("escape.txt"));
        assert!(err.to_string().contains("escapes archive root"));

        // Refusals: symlink-to-symlink chain
        let chain_tar = make_tar_gz(&[
            ("target.txt", b"content", tar::EntryType::Regular, None),
            (
                "link1.txt",
                b"",
                tar::EntryType::Symlink,
                Some("target.txt"),
            ),
            ("link2.txt", b"", tar::EntryType::Symlink, Some("link1.txt")),
        ]);
        let chain_pin = ResolvedPin {
            sha256_hex: sha256_hex(&chain_tar),
            size: chain_tar.len() as u64,
        };
        let chain_staged = vec![test_staged_member(
            "link2.txt",
            "dest.txt",
            0o644,
            &"00".repeat(32),
        )];
        let chain_ignored = vec!["target.txt".into(), "link1.txt".into()];
        let err = plan_pinned_input(
            "chain-entry",
            &chain_tar,
            &chain_pin,
            "chain.tar.gz",
            &chain_staged,
            &chain_ignored,
        )
        .unwrap_err();
        assert!(err.to_string().contains("chain-entry"));
        assert!(err.to_string().contains("link2.txt"));
        assert!(err.to_string().contains("symlink-to-symlink"));

        // Refusals: symlink to non-regular member (directory)
        let dir_tar = make_tar_gz(&[
            ("sub_dir", b"", tar::EntryType::Directory, None),
            (
                "link_dir.txt",
                b"",
                tar::EntryType::Symlink,
                Some("sub_dir"),
            ),
        ]);
        let dir_pin = ResolvedPin {
            sha256_hex: sha256_hex(&dir_tar),
            size: dir_tar.len() as u64,
        };
        let dir_staged = vec![test_staged_member(
            "link_dir.txt",
            "dest.txt",
            0o644,
            &"00".repeat(32),
        )];
        let err = plan_pinned_input(
            "dir-entry",
            &dir_tar,
            &dir_pin,
            "dir.tar.gz",
            &dir_staged,
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("dir-entry"));
        assert!(err.to_string().contains("link_dir.txt"));
        assert!(err.to_string().contains("not a regular file"));
    }

    #[test]
    fn plan_and_stage_pinned_input() {
        let content = b"staged file contents for plan test";
        let zip_bytes = make_zip(&[("my/inner.txt", content, 0o100644)]);
        let pin = ResolvedPin {
            sha256_hex: sha256_hex(&zip_bytes),
            size: zip_bytes.len() as u64,
        };
        let staged = vec![test_staged_member(
            "my/inner.txt",
            "target/dest.txt",
            0o644,
            &sha256_hex(content),
        )];
        // Call without stage dir
        let plans =
            plan_pinned_input("plan-test", &zip_bytes, &pin, "test.zip", &staged, &[]).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].inner_path, "my/inner.txt");
        assert_eq!(plans[0].alias, None);
        assert_eq!(plans[0].dest, "target/dest.txt");
        assert_eq!(plans[0].mode, 0o644);
        assert_eq!(plans[0].extracted_sha256, sha256_hex(content));

        // Call stage_pinned_plans into stage dir
        let tmp = tempfile::tempdir().unwrap();
        stage_pinned_plans("plan-test", tmp.path(), &zip_bytes, "test.zip", &plans).unwrap();
        let staged_bytes = std::fs::read(tmp.path().join("target/dest.txt")).unwrap();
        assert_eq!(sha256_hex(&staged_bytes), plans[0].extracted_sha256);
    }

    #[test]
    fn real_catalog_row_ced_engine() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let row = solstone_core_assets::catalog()
            .iter()
            .find(|a| a.unit == "ced-engine" && a.filename == "ced-v0.1.0-lib-linux-cpu-x64.tar.gz")
            .expect("ced-engine catalog row");
        assert_eq!(
            row.sha256,
            "915e0573bc4e17197a7a893d0eb98e1a851abb64451b2e1a8ad51f5f99040360"
        );
        assert_eq!(row.size_bytes, 788651);

        let file_path =
            repo_root.join("core/models/assets/ced/ced-v0.1.0-lib-linux-cpu-x64.tar.gz");
        let bytes = std::fs::read(&file_path).expect("read committed ced tar.gz");
        assert_eq!(bytes.len() as u64, 788651);
        assert_eq!(sha256_hex(&bytes), row.sha256);
    }

    #[test]
    fn ced_tar_gz_resolve_and_stage_elf_admit() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let input = crate::inventory::PinnedInput::CatalogCommitted {
            unit: "ced-engine".into(),
            filename: "ced-v0.1.0-lib-linux-cpu-x64.tar.gz".into(),
            path: "core/models/assets/ced/ced-v0.1.0-lib-linux-cpu-x64.tar.gz".into(),
        };
        let (bytes, pin, filename) =
            resolve_pinned_input("ced-test", repo_root, "linux-x86_64", &input).unwrap();

        let needed_set = BTreeSet::from(["ced-v0.1.0-lib-linux-cpu-x64/libced.so"]);
        let contents = extract_needed_members("ced-test", &bytes, &filename, &needed_set).unwrap();
        let libced_bytes = &contents["ced-v0.1.0-lib-linux-cpu-x64/libced.so"];
        let libced_sha = sha256_hex(libced_bytes);

        let staged = vec![test_staged_member(
            "ced-v0.1.0-lib-linux-cpu-x64/libced.so",
            "lib/solstone-ced/libced.so",
            0o755,
            &libced_sha,
        )];
        let ignored = vec![
            "ced-v0.1.0-lib-linux-cpu-x64/LICENSE".into(),
            "ced-v0.1.0-lib-linux-cpu-x64/README.md".into(),
            "ced-v0.1.0-lib-linux-cpu-x64/ced_capi.h".into(),
        ];
        let plans =
            plan_pinned_input("ced-test", &bytes, &pin, &filename, &staged, &ignored).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        stage_pinned_plans("ced-test", tmp.path(), &bytes, &filename, &plans).unwrap();
        let staged_so = std::fs::read(tmp.path().join("lib/solstone-ced/libced.so")).unwrap();
        assert_eq!(staged_so, *libced_bytes);

        // admit_elf passes
        crate::elf::admit_elf(
            "lib/solstone-ced/libced.so",
            &staged_so,
            EM_X86_64,
            "lib/solstone-ced",
            None,
            &[],
        )
        .unwrap();

        // Copy archive to a temp file, flip one byte, point CatalogCommitted.path at that temp file
        let mut corrupted_bytes = bytes.clone();
        corrupted_bytes[100] ^= 0x55;
        let corrupt_dir = tempfile::tempdir().unwrap();
        let corrupt_file = corrupt_dir.path().join("corrupted.tar.gz");
        std::fs::write(&corrupt_file, &corrupted_bytes).unwrap();
        let corrupt_input = crate::inventory::PinnedInput::CatalogCommitted {
            unit: "ced-engine".into(),
            filename: "ced-v0.1.0-lib-linux-cpu-x64.tar.gz".into(),
            path: corrupt_file.to_str().unwrap().into(),
        };
        let err = resolve_pinned_input("ced-test", repo_root, "linux-x86_64", &corrupt_input)
            .unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("ced-test"));
        assert!(err_msg.contains("(ced-engine, ced-v0.1.0-lib-linux-cpu-x64.tar.gz)"));
    }

    #[test]
    fn nvattest_linux_authority_committed_resolve_and_stage() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let input = crate::inventory::PinnedInput::AuthorityCommitted {
            platform: "linux-x86_64".into(),
            path: "core/models/assets/nvattest/libnvat-linux-x86_64-1.2.2-sol.6-archive.tar.xz"
                .into(),
        };
        let (bytes, pin, filename) =
            resolve_pinned_input("nvattest-linux", repo_root, "linux-x86_64", &input).unwrap();

        let needed_set = BTreeSet::from([
            "bin/nvattest",
            "lib/libnvat.so.1.2.2",
            "share/ca/ca-bundle.pem",
            "LICENSE",
            "share/THIRD_PARTY_NOTICES.md",
        ]);
        let contents =
            extract_needed_members("nvattest-linux", &bytes, &filename, &needed_set).unwrap();

        let staged = vec![
            test_staged_member(
                "bin/nvattest",
                "lib/solstone-nvattest/bin/nvattest",
                0o755,
                &sha256_hex(&contents["bin/nvattest"]),
            ),
            test_staged_member(
                "lib/libnvat.so.1",
                "lib/solstone-nvattest/lib/libnvat.so.1",
                0o755,
                &sha256_hex(&contents["lib/libnvat.so.1.2.2"]),
            ),
            test_staged_member(
                "share/ca/ca-bundle.pem",
                "lib/solstone-nvattest/share/ca/ca-bundle.pem",
                0o644,
                &sha256_hex(&contents["share/ca/ca-bundle.pem"]),
            ),
            test_staged_member(
                "LICENSE",
                "share/solstone-journal/licenses/nvattest/LICENSE",
                0o644,
                &sha256_hex(&contents["LICENSE"]),
            ),
            test_staged_member(
                "share/THIRD_PARTY_NOTICES.md",
                "share/solstone-journal/licenses/nvattest/THIRD_PARTY_NOTICES.md",
                0o644,
                &sha256_hex(&contents["share/THIRD_PARTY_NOTICES.md"]),
            ),
        ];
        let ignored = vec!["lib/libnvat.so".into(), "lib/libnvat.so.1.2.2".into()];

        let plans = plan_pinned_input("nvattest-linux", &bytes, &pin, &filename, &staged, &ignored)
            .unwrap();
        assert_eq!(plans[1].alias.as_deref(), Some("lib/libnvat.so.1"));
        assert_eq!(plans[1].inner_path, "lib/libnvat.so.1.2.2");

        let tmp = tempfile::tempdir().unwrap();
        stage_pinned_plans("nvattest-linux", tmp.path(), &bytes, &filename, &plans).unwrap();
        assert_no_symlinks_recursive(tmp.path());

        let bin_bytes =
            std::fs::read(tmp.path().join("lib/solstone-nvattest/bin/nvattest")).unwrap();
        let so_bytes =
            std::fs::read(tmp.path().join("lib/solstone-nvattest/lib/libnvat.so.1")).unwrap();
        crate::elf::admit_elf(
            "lib/solstone-nvattest/bin/nvattest",
            &bin_bytes,
            EM_X86_64,
            "lib/solstone-nvattest/bin",
            None,
            &[("lib/solstone-nvattest/lib/libnvat.so.1", &so_bytes)],
        )
        .unwrap();
    }

    #[test]
    fn macos_real_bytes() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let input = crate::inventory::PinnedInput::AuthorityCommitted {
            platform: "macos-arm64".into(),
            path: "core/models/assets/nvattest/libnvat-macos-arm64-1.2.2-sol.6-archive.tar.xz"
                .into(),
        };
        let (bytes, pin, filename) =
            resolve_pinned_input("nvattest-mac", repo_root, "macos-arm64", &input).unwrap();

        let needed_set = BTreeSet::from([
            "bin/nvattest",
            "lib/libnvat.1.2.2.dylib",
            "share/ca/ca-bundle.pem",
            "LICENSE",
            "share/THIRD_PARTY_NOTICES.md",
        ]);
        let contents =
            extract_needed_members("nvattest-mac", &bytes, &filename, &needed_set).unwrap();

        let staged = vec![
            test_staged_member(
                "bin/nvattest",
                "lib/solstone-nvattest/bin/nvattest",
                0o755,
                &sha256_hex(&contents["bin/nvattest"]),
            ),
            test_staged_member(
                "lib/libnvat.1.dylib",
                "lib/solstone-nvattest/lib/libnvat.1.dylib",
                0o755,
                &sha256_hex(&contents["lib/libnvat.1.2.2.dylib"]),
            ),
            test_staged_member(
                "share/ca/ca-bundle.pem",
                "lib/solstone-nvattest/share/ca/ca-bundle.pem",
                0o644,
                &sha256_hex(&contents["share/ca/ca-bundle.pem"]),
            ),
            test_staged_member(
                "LICENSE",
                "share/solstone-journal/licenses/nvattest/LICENSE",
                0o644,
                &sha256_hex(&contents["LICENSE"]),
            ),
            test_staged_member(
                "share/THIRD_PARTY_NOTICES.md",
                "share/solstone-journal/licenses/nvattest/THIRD_PARTY_NOTICES.md",
                0o644,
                &sha256_hex(&contents["share/THIRD_PARTY_NOTICES.md"]),
            ),
        ];
        let ignored = vec!["lib/libnvat.dylib".into(), "lib/libnvat.1.2.2.dylib".into()];
        let plans =
            plan_pinned_input("nvattest-mac", &bytes, &pin, &filename, &staged, &ignored).unwrap();
        assert_eq!(plans[1].alias.as_deref(), Some("lib/libnvat.1.dylib"));
        assert_eq!(plans[1].inner_path, "lib/libnvat.1.2.2.dylib");

        let tmp = tempfile::tempdir().unwrap();
        stage_pinned_plans("nvattest-mac", tmp.path(), &bytes, &filename, &plans).unwrap();
        assert_no_symlinks_recursive(tmp.path());

        let bin_bytes =
            std::fs::read(tmp.path().join("lib/solstone-nvattest/bin/nvattest")).unwrap();
        crate::macho::admit_macho(
            "lib/solstone-nvattest/bin/nvattest",
            &bin_bytes,
            crate::macho::cputype_arm64(),
            (15, 0),
            "lib/solstone-nvattest/bin",
            None,
            &["lib/solstone-nvattest/lib/libnvat.1.dylib"],
        )
        .unwrap();

        // CED macOS Metal ARM64
        let ced_mac_path =
            repo_root.join("core/models/assets/ced/ced-v0.1.0-lib-macos-metal-arm64.tar.gz");
        let ced_mac_bytes = std::fs::read(&ced_mac_path).unwrap();
        assert_eq!(ced_mac_bytes.len(), 686952);
        assert_eq!(
            sha256_hex(&ced_mac_bytes),
            "4c913ba0ece1d06ba2210da9fcaee3d8199ca3c62697c331810f224444e4054b"
        );
        let ced_needed = BTreeSet::from(["ced-v0.1.0-lib-macos-metal-arm64/libced.dylib"]);
        let ced_contents = extract_needed_members(
            "ced-mac",
            &ced_mac_bytes,
            "ced-v0.1.0-lib-macos-metal-arm64.tar.gz",
            &ced_needed,
        )
        .unwrap();
        let ced_dylib = &ced_contents["ced-v0.1.0-lib-macos-metal-arm64/libced.dylib"];
        crate::macho::admit_macho(
            "lib/solstone-ced/libced.dylib",
            ced_dylib,
            crate::macho::cputype_arm64(),
            (15, 0),
            "lib/solstone-ced",
            None,
            &[],
        )
        .unwrap();
    }

    #[test]
    fn extracted_binary_sha256_refusal() {
        let bad_artifact = solstone_core_assets::Artifact {
            unit: "bad-unit",
            version: "1.0.0",
            filename: "bad.tar.gz",
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            size_bytes: 10,
            upstream_url: "https://example.com/bad.tar.gz",
            origin_key: "bad-key",
            artifact_key: None,
            platform: None,
            backend: None,
            extracted_binary_sha256: Some("ab"),
        };
        let catalog = vec![bad_artifact];
        let input = crate::inventory::PinnedInput::CatalogCommitted {
            unit: "bad-unit".into(),
            filename: "bad.tar.gz".into(),
            path: "dummy".into(),
        };
        let err = resolve_pinned_input_with_catalog(
            "my-entry",
            Path::new("."),
            "linux-x86_64",
            &input,
            &catalog,
        )
        .unwrap_err();
        assert!(err.to_string().contains("bad-unit"));
        assert!(err.to_string().contains("bad.tar.gz"));
    }

    #[test]
    fn nvattest_inventory_targets_staged_and_admitted() {
        const EM_AARCH64: u16 = 183;
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let inventory =
            crate::inventory::load_inventory(&repo_root.join("core/distribution/inventory.toml"))
                .unwrap();

        for target_id in ["linux-x86_64", "linux-aarch64", "macos-arm64"] {
            let entry = inventory
                .entry
                .iter()
                .find(|e| match e {
                    crate::inventory::Entry::PinnedMembers {
                        component, targets, ..
                    } => {
                        component.as_deref() == Some("nvattest")
                            && targets.iter().any(|t| t == target_id)
                    }
                    _ => false,
                })
                .unwrap_or_else(|| panic!("find nvattest entry for {target_id}"));

            let (input, staged, ignored) = match entry {
                crate::inventory::Entry::PinnedMembers {
                    component,
                    input,
                    staged,
                    ignored,
                    ..
                } => {
                    assert_eq!(component.as_deref(), Some("nvattest"));
                    (input, staged.as_slice(), ignored.as_slice())
                }
                _ => unreachable!(),
            };

            for m in staged {
                assert!(
                    !m.dest.starts_with("bin/"),
                    "dest {} must not be under package-root bin/",
                    m.dest
                );
                assert!(
                    !m.dest.starts_with("share/licenses/"),
                    "dest {} must not be under share/licenses/",
                    m.dest
                );
                assert!(
                    !m.dest.starts_with("share/provenance/"),
                    "dest {} must not be under share/provenance/",
                    m.dest
                );
            }

            if target_id.starts_with("linux") {
                assert!(ignored.contains(&"lib/libnvat.so".to_string()));
                assert!(ignored.contains(&"lib/libnvat.so.1.2.2".to_string()));
            } else {
                assert!(ignored.contains(&"lib/libnvat.dylib".to_string()));
                assert!(ignored.contains(&"lib/libnvat.1.2.2.dylib".to_string()));
            }
            assert_eq!(ignored.len(), 2);
            for ign in ignored {
                assert!(
                    !staged.iter().any(|m| m.dest == *ign || m.relpath == *ign),
                    "ignored name {ign} must not be a dest or relpath in staged"
                );
            }

            let (bytes, pin, filename) =
                resolve_pinned_input("nvattest", repo_root, target_id, input).unwrap();
            let plans =
                plan_pinned_input("nvattest", &bytes, &pin, &filename, staged, ignored).unwrap();
            let tmp = tempfile::tempdir().unwrap();
            stage_pinned_plans("nvattest", tmp.path(), &bytes, &filename, &plans).unwrap();

            let expected_files = if target_id.starts_with("linux") {
                vec![
                    "lib/solstone-nvattest/bin/nvattest",
                    "lib/solstone-nvattest/lib/libnvat.so.1",
                    "lib/solstone-nvattest/share/ca/ca-bundle.pem",
                    "share/solstone-journal/licenses/nvattest/LICENSE",
                    "share/solstone-journal/licenses/nvattest/THIRD_PARTY_NOTICES.md",
                ]
            } else {
                vec![
                    "lib/solstone-nvattest/bin/nvattest",
                    "lib/solstone-nvattest/lib/libnvat.1.dylib",
                    "lib/solstone-nvattest/share/ca/ca-bundle.pem",
                    "share/solstone-journal/licenses/nvattest/LICENSE",
                    "share/solstone-journal/licenses/nvattest/THIRD_PARTY_NOTICES.md",
                ]
            };

            for rel in &expected_files {
                let p = tmp.path().join(rel);
                assert!(p.is_file(), "expected file {} does not exist", p.display());
                let meta = std::fs::symlink_metadata(&p).unwrap();
                assert!(
                    !meta.file_type().is_symlink(),
                    "{} must not be a symlink",
                    p.display()
                );
            }

            let bin_meta =
                std::fs::metadata(tmp.path().join("lib/solstone-nvattest/bin/nvattest")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(bin_meta.permissions().mode() & 0o777, 0o755);
            }

            assert_no_symlinks_recursive(tmp.path());
            assert!(!tmp.path().join("bin/nvattest").exists());

            fn count_files(p: &Path) -> usize {
                let mut c = 0;
                for entry in std::fs::read_dir(p).unwrap() {
                    let entry = entry.unwrap();
                    let ft = entry.file_type().unwrap();
                    if ft.is_file() {
                        c += 1;
                    } else if ft.is_dir() {
                        c += count_files(&entry.path());
                    }
                }
                c
            }
            assert_eq!(count_files(tmp.path()), 5);

            for m in staged {
                let staged_bytes = std::fs::read(tmp.path().join(&m.dest)).unwrap();
                let actual_hash = sha256_hex(&staged_bytes);
                assert_eq!(
                    actual_hash, m.extracted_sha256,
                    "staged file hash mismatch for {}",
                    m.dest
                );

                let expected_oracle = match (target_id, m.dest.as_str()) {
                    ("linux-x86_64", "lib/solstone-nvattest/bin/nvattest") => {
                        "41d65c4ae56aab9e17802cdc376017c64cd1e10a3b273a212c096307e513ce28"
                    }
                    ("linux-x86_64", "lib/solstone-nvattest/lib/libnvat.so.1") => {
                        "b0c5d7031700845f49fa6dae1ab33dcc427965df691354466aaf67c868ec0c29"
                    }
                    ("linux-aarch64", "lib/solstone-nvattest/bin/nvattest") => {
                        "02032d5bc77c2ff8b76e0a7e735c268253b37251a09eb4ddc13085a74b7a89c7"
                    }
                    ("linux-aarch64", "lib/solstone-nvattest/lib/libnvat.so.1") => {
                        "ee0d57e5b6e79beb5e43ec86c4b0e7512705e7c0edf7db8b8b3652ce2a745a86"
                    }
                    ("macos-arm64", "lib/solstone-nvattest/bin/nvattest") => {
                        "f9b22f299477545df5537a53b3b26bfaf9738759a49c1def4ca1d08422071497"
                    }
                    ("macos-arm64", "lib/solstone-nvattest/lib/libnvat.1.dylib") => {
                        "f2519b32b31b36ca62538611910f2142bf9fcf654ed386785429d8ed5de414e8"
                    }
                    (_, "lib/solstone-nvattest/share/ca/ca-bundle.pem") => {
                        "3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91"
                    }
                    (_, "share/solstone-journal/licenses/nvattest/LICENSE") => {
                        "82d36972a71088e8d4a4793313e64e18340c60de08e3175a58360a277c962a33"
                    }
                    (
                        "linux-x86_64" | "linux-aarch64",
                        "share/solstone-journal/licenses/nvattest/THIRD_PARTY_NOTICES.md",
                    ) => "b6f7785f37de5e10aeda2f86435f3e6b7fdabda603b3d04200d91ed00ef7312a",
                    (
                        "macos-arm64",
                        "share/solstone-journal/licenses/nvattest/THIRD_PARTY_NOTICES.md",
                    ) => "199a52ad0726e35c4ae82ed6b8fb80b75ac89aa0d2303e38755a67136a15db56",
                    other => panic!("unexpected (target, dest): {other:?}"),
                };
                assert_eq!(
                    m.extracted_sha256, expected_oracle,
                    "inventory extracted_sha256 oracle mismatch for {}",
                    m.dest
                );
            }

            let mut flipped_bytes = bytes.clone();
            flipped_bytes[64] ^= 0xaa;
            let corrupt_dir = tempfile::tempdir().unwrap();
            let corrupt_archive = corrupt_dir.path().join("flipped.tar.xz");
            std::fs::write(&corrupt_archive, &flipped_bytes).unwrap();
            let corrupt_input = crate::inventory::PinnedInput::AuthorityCommitted {
                platform: target_id.into(),
                path: corrupt_archive.to_str().unwrap().into(),
            };
            assert!(
                resolve_pinned_input("nvattest-corrupt", repo_root, target_id, &corrupt_input)
                    .is_err()
            );

            let bin_bytes =
                std::fs::read(tmp.path().join("lib/solstone-nvattest/bin/nvattest")).unwrap();
            if target_id == "linux-x86_64" || target_id == "linux-aarch64" {
                let machine = if target_id == "linux-x86_64" {
                    EM_X86_64
                } else {
                    EM_AARCH64
                };
                let so_bytes =
                    std::fs::read(tmp.path().join("lib/solstone-nvattest/lib/libnvat.so.1"))
                        .unwrap();
                crate::elf::admit_elf(
                    "lib/solstone-nvattest/bin/nvattest",
                    &bin_bytes,
                    machine,
                    "lib/solstone-nvattest/bin",
                    None,
                    &[("lib/solstone-nvattest/lib/libnvat.so.1", &so_bytes)],
                )
                .unwrap();

                let bin_info = crate::elf::parse_elf(&bin_bytes).unwrap();
                let so_info = crate::elf::parse_elf(&so_bytes).unwrap();

                for need in &bin_info.verneed {
                    for name in &need.names {
                        if let Some(rest) = name.strip_prefix("GLIBC_") {
                            if !name.starts_with("GLIBC_PRIVATE") && !name.starts_with("GLIBC_ABI_")
                            {
                                let parts: Vec<u32> =
                                    rest.split('.').filter_map(|s| s.parse().ok()).collect();
                                if parts.len() >= 2 {
                                    assert!(
                                        parts[0] < 2 || (parts[0] == 2 && parts[1] <= 28),
                                        "glibc version {name} exceeds 2.28 in {target_id}"
                                    );
                                }
                            }
                        }
                    }
                }

                assert!(
                    so_info.needed.iter().any(|n| n == "libutil.so.1"),
                    "libnvat in {target_id} must need libutil.so.1"
                );
                assert!(
                    so_info
                        .verneed
                        .iter()
                        .any(|v| v.file == "libz.so.1"
                            && v.names.iter().any(|n| n == "ZLIB_1.2.3.4")),
                    "libnvat in {target_id} must need ZLIB_1.2.3.4"
                );
                assert!(
                    so_info
                        .verneed
                        .iter()
                        .any(|v| v.names.iter().any(|n| n == "GLIBCXX_3.4.21"))
                        || bin_info
                            .verneed
                            .iter()
                            .any(|v| v.names.iter().any(|n| n == "GLIBCXX_3.4.21")),
                    "GLIBCXX_3.4.21 must be present in {target_id}"
                );
            } else if target_id == "macos-arm64" {
                crate::macho::admit_macho(
                    "lib/solstone-nvattest/bin/nvattest",
                    &bin_bytes,
                    crate::macho::cputype_arm64(),
                    (15, 0),
                    "lib/solstone-nvattest/bin",
                    None,
                    &["lib/solstone-nvattest/lib/libnvat.1.dylib"],
                )
                .unwrap();
            }

            if target_id == "linux-x86_64" {
                let out_dir = tempfile::tempdir().unwrap();
                let basename = "solstone-journal-2.0.0-linux-x86_64";
                crate::write_containers(
                    tmp.path(),
                    out_dir.path(),
                    crate::ContainerMeta {
                        version: "2.0.0",
                        basename,
                        deb_arch: "amd64",
                        rpm_arch: "x86_64",
                    },
                )
                .unwrap();
                let [tar_name, deb_name, rpm_name] = crate::inventory::artifact_archives(basename);
                let tar_records =
                    crate::tar::tar_records(&std::fs::read(out_dir.path().join(tar_name)).unwrap())
                        .unwrap();
                let deb_records = crate::deb::deb_records(&out_dir.path().join(deb_name)).unwrap();
                let rpm_records = crate::rpm::rpm_records(&out_dir.path().join(rpm_name)).unwrap();

                let tar_bin = tar_records
                    .iter()
                    .find(|r| r.dest == "lib/solstone-nvattest/bin/nvattest")
                    .expect("tar bin");
                let deb_bin = deb_records
                    .iter()
                    .find(|r| r.dest == "lib/solstone-nvattest/bin/nvattest")
                    .expect("deb bin");
                let rpm_bin = rpm_records
                    .iter()
                    .find(|r| r.dest == "lib/solstone-nvattest/bin/nvattest")
                    .expect("rpm bin");

                assert_eq!(tar_bin.mode, 0o755);
                assert_eq!(deb_bin.mode, 0o755);
                assert_eq!(rpm_bin.mode, 0o755);
            }
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn nvattest_version_staged_runs_exit_0() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let inventory =
            crate::inventory::load_inventory(&repo_root.join("core/distribution/inventory.toml"))
                .unwrap();
        let entry = inventory
            .entry
            .iter()
            .find(|e| match e {
                crate::inventory::Entry::PinnedMembers {
                    component, targets, ..
                } => {
                    component.as_deref() == Some("nvattest")
                        && targets.iter().any(|t| t == "linux-x86_64")
                }
                _ => false,
            })
            .expect("find nvattest entry for linux-x86_64");

        let (input, staged, ignored) = match entry {
            crate::inventory::Entry::PinnedMembers {
                input,
                staged,
                ignored,
                ..
            } => (input, staged.as_slice(), ignored.as_slice()),
            _ => unreachable!(),
        };

        let (bytes, pin, filename) =
            resolve_pinned_input("nvattest", repo_root, "linux-x86_64", input).unwrap();
        let plans =
            plan_pinned_input("nvattest", &bytes, &pin, &filename, staged, ignored).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        stage_pinned_plans("nvattest", tmp.path(), &bytes, &filename, &plans).unwrap();

        let exe = tmp.path().join("lib/solstone-nvattest/bin/nvattest");
        let output = std::process::Command::new(&exe)
            .arg("version")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("execute staged nvattest version");
        assert!(
            output.status.success(),
            "status: {:?}, stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
