// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Package admission for downloaded and cached update archives.
//! Every update must be a signed full package satisfying cryptographic,
//! structural, and metadata requirements before handoff to the installer.

use std::io::{Read, Seek};
use std::path::Path;
use std::sync::mpsc::Sender;

use velopack::VelopackAsset;

use super::{FirstPartyFeed, asset_name_is_plain, request_without_redirects};

const MAX_SIDECAR_BYTES: u64 = 4096;
const MAX_NUSPEC_BYTES: u64 = 65536;

#[cfg(windows)]
pub type FileIdentity = solstone_core_journal_io::WindowsFileIdentity;
#[cfg(not(windows))]
pub type FileIdentity = (u64, u64);

#[cfg(windows)]
pub fn file_handle_identity(file: &std::fs::File) -> Result<FileIdentity, velopack::Error> {
    solstone_core_journal_io::windows_file_identity(file)
        .map_err(|e| velopack::Error::Other(format!("could not query windows file identity: {e}")))
}

#[cfg(not(windows))]
pub fn file_handle_identity(file: &std::fs::File) -> Result<FileIdentity, velopack::Error> {
    use std::os::unix::fs::MetadataExt;
    let meta = file
        .metadata()
        .map_err(|e| velopack::Error::Other(format!("could not query file metadata: {e}")))?;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
pub fn path_file_identity(path: &Path) -> Result<FileIdentity, velopack::Error> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT
        .open(path)
        .map_err(|e| {
            velopack::Error::Other(format!(
                "could not open path for identity check {}: {e}",
                path.display()
            ))
        })?;
    file_handle_identity(&file)
}

#[cfg(not(windows))]
pub fn path_file_identity(path: &Path) -> Result<FileIdentity, velopack::Error> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        velopack::Error::Other(format!(
            "could not query symlink metadata for {}: {e}",
            path.display()
        ))
    })?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(velopack::Error::Other(format!(
            "{} is a symlink or non-regular file",
            path.display()
        )));
    }
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
pub fn open_regular_file(path: &Path) -> Result<std::fs::File, velopack::Error> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let symlink_meta = std::fs::symlink_metadata(path).map_err(|e| {
        velopack::Error::Other(format!(
            "could not query symlink metadata for {}: {e}",
            path.display()
        ))
    })?;
    if symlink_meta.file_type().is_symlink() || !symlink_meta.file_type().is_file() {
        return Err(velopack::Error::Other(format!(
            "{} is a symlink or non-regular file",
            path.display()
        )));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT
        .open(path)
        .map_err(|e| velopack::Error::Other(format!("could not open {}: {e}", path.display())))?;
    if !file.metadata()?.file_type().is_file() {
        return Err(velopack::Error::Other(
            "opened package is not a regular file".into(),
        ));
    }
    let handle_id = file_handle_identity(&file)?;
    let path_id = path_file_identity(path)?;
    if handle_id != path_id {
        return Err(velopack::Error::Other(format!(
            "file identity mismatch on open for {}",
            path.display()
        )));
    }
    Ok(file)
}

#[cfg(not(windows))]
pub fn open_regular_file(path: &Path) -> Result<std::fs::File, velopack::Error> {
    let symlink_meta = std::fs::symlink_metadata(path).map_err(|e| {
        velopack::Error::Other(format!(
            "could not query symlink metadata for {}: {e}",
            path.display()
        ))
    })?;
    if symlink_meta.file_type().is_symlink() || !symlink_meta.file_type().is_file() {
        return Err(velopack::Error::Other(format!(
            "{} is a symlink or non-regular file",
            path.display()
        )));
    }
    let file = std::fs::File::open(path)
        .map_err(|e| velopack::Error::Other(format!("could not open {}: {e}", path.display())))?;
    let handle_id = file_handle_identity(&file)?;
    let path_id = path_file_identity(path)?;
    if handle_id != path_id {
        return Err(velopack::Error::Other(format!(
            "file identity mismatch on open for {}",
            path.display()
        )));
    }
    Ok(file)
}

/// Run the prehash and trusted-comment gate on sidecar bytes before any package read.
pub fn check_sidecar_format(signature_path: &Path, bytes: &[u8]) -> Result<(), velopack::Error> {
    solstone_core_installed_payload::admit_prehashed_signature(signature_path, bytes).map_err(|e| {
        log::warn!("invalid sidecar format: {e}");
        velopack::Error::Other(e.to_string())
    })
}

/// Verify that the zip package contains exactly one .nuspec manifest,
/// and that the manifest contains the 7 required identity elements exactly once.
pub fn parse_and_validate_nuspec<R: Read + Seek>(
    reader: R,
    expected_version: &str,
) -> Result<(), velopack::Error> {
    parse_and_validate_nuspec_for_architecture(
        reader,
        expected_version,
        solstone_core_installed_payload::compiled_windows_architecture(),
    )
}

fn parse_and_validate_nuspec_for_architecture<R: Read + Seek>(
    mut reader: R,
    expected_version: &str,
    architecture: solstone_core_installed_payload::WindowsArchitecture,
) -> Result<(), velopack::Error> {
    use quick_xml::events::Event;

    let identity = super::package_identity(architecture);
    let mut archive = zip::ZipArchive::new(&mut reader).map_err(|e| {
        log::warn!("could not read package zip archive: {e}");
        velopack::Error::Other(format!("could not read package zip archive: {e}"))
    })?;

    let mut nuspec_index = None;
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|e| {
            log::warn!("could not read zip entry {i}: {e}");
            velopack::Error::Other(format!("could not read zip entry: {e}"))
        })?;
        let entry_name = file.name().map_err(|e| {
            log::warn!("could not get entry name {i}: {e}");
            velopack::Error::Other(format!("could not get entry name: {e}"))
        })?;
        if entry_name.to_ascii_lowercase().ends_with(".nuspec") {
            if nuspec_index.is_some() {
                log::warn!("multiple .nuspec entries found in package zip");
                return Err(velopack::Error::Other(
                    "multiple .nuspec entries found in package zip".into(),
                ));
            }
            nuspec_index = Some(i);
        }
    }
    let Some(index) = nuspec_index else {
        log::warn!("no .nuspec entry found in package zip");
        return Err(velopack::Error::Other(
            "no .nuspec entry found in package zip".into(),
        ));
    };

    let mut nuspec_file = archive.by_index(index).map_err(|e| {
        log::warn!("could not access nuspec entry: {e}");
        velopack::Error::Other(format!("could not access nuspec entry: {e}"))
    })?;

    if nuspec_file.size() > MAX_NUSPEC_BYTES {
        log::warn!(
            "nuspec uncompressed size {} exceeds limit",
            nuspec_file.size()
        );
        return Err(velopack::Error::Other(
            "nuspec entry exceeds size limit".into(),
        ));
    }

    let mut text = String::new();
    nuspec_file
        .by_ref()
        .take(MAX_NUSPEC_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| {
            log::warn!("could not read nuspec content: {e}");
            velopack::Error::Other(format!("could not read nuspec content: {e}"))
        })?;

    if text.len() as u64 > MAX_NUSPEC_BYTES {
        log::warn!("nuspec read bytes {} exceed limit", text.len());
        return Err(velopack::Error::Other(
            "nuspec entry exceeds size limit".into(),
        ));
    }

    let mut xml_reader = quick_xml::Reader::from_str(&text);
    let mut counts = NuspecCounts::default();

    let mut path = Vec::<String>::new();
    let mut current_tag: Option<String> = None;
    let mut value = String::new();
    let identity_tag = |tag: &str| {
        matches!(
            tag,
            "id" | "version" | "channel" | "os" | "rid" | "machineArchitecture" | "mainExe"
        )
    };
    loop {
        match xml_reader.read_event() {
            Ok(Event::Start(e)) => {
                if current_tag.is_some() {
                    return Err(velopack::Error::Other("nested identity element".into()));
                }
                let name = xml_reader
                    .decoder()
                    .decode(e.local_name().as_ref())
                    .map_err(|e| velopack::Error::Other(format!("xml decode error: {e}")))?
                    .into_owned();
                path.push(name.clone());
                if identity_tag(&name) {
                    if path.len() != 3 || path[0] != "package" || path[1] != "metadata" {
                        return Err(velopack::Error::Other(
                            "identity element outside package metadata".into(),
                        ));
                    }
                    current_tag = Some(name);
                    value.clear();
                }
            }
            Ok(Event::Empty(e)) => {
                let name = xml_reader
                    .decoder()
                    .decode(e.local_name().as_ref())
                    .map_err(|e| velopack::Error::Other(format!("xml decode error: {e}")))?
                    .into_owned();
                if identity_tag(&name) {
                    return Err(velopack::Error::Other("empty identity element".into()));
                }
            }
            Ok(Event::End(_)) => {
                if let Some(tag) = current_tag.take() {
                    process_nuspec_tag(
                        &tag,
                        value.trim(),
                        expected_version,
                        identity,
                        &mut counts,
                    )?;
                }
                path.pop();
            }
            Ok(Event::Text(e)) if current_tag.is_some() => {
                let decoded = xml_reader
                    .decoder()
                    .decode(e.as_ref())
                    .map_err(|e| velopack::Error::Other(format!("xml decode error: {e}")))?;
                value.push_str(
                    &quick_xml::escape::unescape(&decoded)
                        .map_err(|e| velopack::Error::Other(format!("xml unescape error: {e}")))?,
                );
            }
            Ok(Event::CData(e)) if current_tag.is_some() => {
                value.push_str(
                    &xml_reader
                        .decoder()
                        .decode(e.as_ref())
                        .map_err(|e| velopack::Error::Other(format!("xml decode error: {e}")))?,
                );
            }
            Ok(Event::DocType(_)) => {
                return Err(velopack::Error::Other(
                    "DTD is not valid package metadata".into(),
                ));
            }
            Ok(Event::Eof) => {
                if !path.is_empty() {
                    return Err(velopack::Error::Other("incomplete package metadata".into()));
                }
                break;
            }
            Err(e) => return Err(velopack::Error::Other(format!("xml parsing error: {e}"))),
            _ => {}
        }
    }

    if counts.id != 1
        || counts.version != 1
        || counts.channel != 1
        || counts.os != 1
        || counts.rid != 1
        || counts.machine_arch != 1
        || counts.main_exe != 1
    {
        log::warn!("missing required nuspec identity element(s)");
        return Err(velopack::Error::Other(
            "missing required nuspec identity element(s)".into(),
        ));
    }

    Ok(())
}

#[derive(Default)]
struct NuspecCounts {
    id: usize,
    version: usize,
    channel: usize,
    os: usize,
    rid: usize,
    machine_arch: usize,
    main_exe: usize,
}

fn process_nuspec_tag(
    tag: &str,
    val_trimmed: &str,
    expected_version: &str,
    identity: super::PackageIdentity,
    counts: &mut NuspecCounts,
) -> Result<(), velopack::Error> {
    match tag {
        "id" => {
            counts.id += 1;
            if counts.id > 1 || val_trimmed != "SolstoneJournal" {
                log::warn!("invalid or duplicate id: {val_trimmed}");
                return Err(velopack::Error::Other("invalid or duplicate id".into()));
            }
        }
        "version" => {
            counts.version += 1;
            if counts.version > 1 || val_trimmed != expected_version {
                log::warn!(
                    "invalid or duplicate version: {val_trimmed}, expected {expected_version}"
                );
                return Err(velopack::Error::Other(
                    "invalid or duplicate version".into(),
                ));
            }
        }
        "channel" => {
            counts.channel += 1;
            if counts.channel > 1 || val_trimmed != identity.channel {
                log::warn!("invalid or duplicate channel: {val_trimmed}");
                return Err(velopack::Error::Other(
                    "invalid or duplicate channel".into(),
                ));
            }
        }
        "os" => {
            counts.os += 1;
            if counts.os > 1 || val_trimmed != "win" {
                log::warn!("invalid or duplicate os: {val_trimmed}");
                return Err(velopack::Error::Other("invalid or duplicate os".into()));
            }
        }
        "rid" => {
            counts.rid += 1;
            if counts.rid > 1 || val_trimmed != identity.rid {
                log::warn!("invalid or duplicate rid: {val_trimmed}");
                return Err(velopack::Error::Other("invalid or duplicate rid".into()));
            }
        }
        "machineArchitecture" => {
            counts.machine_arch += 1;
            if counts.machine_arch > 1 || val_trimmed != identity.machine_architecture {
                log::warn!("invalid or duplicate machineArchitecture: {val_trimmed}");
                return Err(velopack::Error::Other(
                    "invalid or duplicate machineArchitecture".into(),
                ));
            }
        }
        "mainExe" => {
            counts.main_exe += 1;
            if counts.main_exe > 1 || val_trimmed != "bin/journal-app.exe" {
                log::warn!("invalid or duplicate mainExe: {val_trimmed}");
                return Err(velopack::Error::Other(
                    "invalid or duplicate mainExe".into(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Admit an already-present package and sidecar on disk (for preflight or apply).
pub fn admit_existing_package(
    package_path: &Path,
    sidecar_path: &Path,
    asset: &VelopackAsset,
) -> Result<std::fs::File, velopack::Error> {
    if !asset.Type.eq_ignore_ascii_case("Full") {
        log::warn!("asset type {} is not Full", asset.Type);
        return Err(velopack::Error::Other(
            "only full updates are supported".into(),
        ));
    }
    let sidecar_name = format!("{}.minisig", asset.FileName);
    if !asset_name_is_plain(&asset.FileName) || !asset_name_is_plain(&sidecar_name) {
        log::warn!("asset filename is not plain: {}", asset.FileName);
        return Err(velopack::Error::Other("asset filename is not plain".into()));
    }
    if asset.Size == 0 {
        log::warn!("asset size is zero");
        return Err(velopack::Error::Other("asset size is zero".into()));
    }

    let mut sidecar_file = open_regular_file(sidecar_path)?;
    let mut sidecar_bytes = Vec::new();
    sidecar_file
        .by_ref()
        .take(MAX_SIDECAR_BYTES + 1)
        .read_to_end(&mut sidecar_bytes)
        .map_err(|e| {
            log::warn!("could not read sidecar: {e}");
            velopack::Error::Other(format!("could not read sidecar: {e}"))
        })?;
    if sidecar_bytes.len() as u64 > MAX_SIDECAR_BYTES {
        log::warn!("sidecar length {} exceeds limit", sidecar_bytes.len());
        return Err(velopack::Error::Other("sidecar exceeds size limit".into()));
    }

    check_sidecar_format(sidecar_path, &sidecar_bytes)?;

    let mut package_file = open_regular_file(package_path)?;
    let meta = package_file.metadata().map_err(|e| {
        log::warn!("could not read package metadata: {e}");
        velopack::Error::Other(format!("could not read package metadata: {e}"))
    })?;
    if meta.len() != asset.Size {
        log::warn!("package size mismatch: {} vs {}", meta.len(), asset.Size);
        return Err(velopack::Error::Other("package size mismatch".into()));
    }

    solstone_core_installed_payload::verify_pinned_signature_reader(
        &mut package_file,
        sidecar_path,
        &sidecar_bytes,
    )
    .map_err(|e| {
        log::warn!("pinned signature verification failed: {e}");
        velopack::Error::Other(format!("signature verification failed: {e}"))
    })?;

    package_file
        .rewind()
        .map_err(|e| velopack::Error::Other(format!("could not rewind package file: {e}")))?;

    parse_and_validate_nuspec(&mut package_file, &asset.Version)?;

    package_file
        .rewind()
        .map_err(|e| velopack::Error::Other(format!("could not rewind package file: {e}")))?;

    Ok(package_file)
}

/// Download an HTTP package with sidecar verification.
pub fn download_http_package(
    feed: &FirstPartyFeed,
    asset: &VelopackAsset,
    local_file: &Path,
    progress_sender: Option<Sender<i16>>,
) -> Result<(), velopack::Error> {
    if !asset.Type.eq_ignore_ascii_case("Full") {
        log::warn!("asset type {} is not Full", asset.Type);
        return Err(velopack::Error::Other(
            "only full updates are supported".into(),
        ));
    }
    let sidecar_name = format!("{}.minisig", asset.FileName);
    if !asset_name_is_plain(&asset.FileName) || !asset_name_is_plain(&sidecar_name) {
        log::warn!("asset filename is not plain: {}", asset.FileName);
        return Err(velopack::Error::Other("asset filename is not plain".into()));
    }
    if asset.Size == 0 {
        log::warn!("asset size is zero");
        return Err(velopack::Error::Other("asset size is zero".into()));
    }

    let sidecar_url = feed.asset_url(&sidecar_name)?;
    let mut sidecar_response = request_without_redirects(&sidecar_url)?;
    let mut sidecar_bytes = Vec::new();
    sidecar_response
        .body_mut()
        .as_reader()
        .take(MAX_SIDECAR_BYTES + 1)
        .read_to_end(&mut sidecar_bytes)
        .map_err(|e| {
            log::warn!("could not read http sidecar: {e}");
            velopack::Error::Other(format!("could not read http sidecar: {e}"))
        })?;
    if sidecar_bytes.len() as u64 > MAX_SIDECAR_BYTES {
        log::warn!("sidecar length {} exceeds limit", sidecar_bytes.len());
        return Err(velopack::Error::Other("sidecar exceeds size limit".into()));
    }

    check_sidecar_format(Path::new(&sidecar_name), &sidecar_bytes)?;

    let package_url = feed.asset_url(&asset.FileName)?;
    let package_response = request_without_redirects(&package_url)?;
    let total = asset.Size;
    let mut received = 0_u64;
    let mut reader = package_response.into_body().into_reader();

    let parent = local_file
        .parent()
        .ok_or_else(|| velopack::Error::Other("local_file has no parent".into()))?;
    let sidecar_partial = parent.join(format!("{}.minisig.partial", asset.FileName));
    let sidecar_final = parent.join(&sidecar_name);

    let stream_result = (|| -> Result<(), velopack::Error> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(local_file)?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            use std::io::Write;
            let size = reader.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            file.write_all(&buffer[..size])?;
            received = received.saturating_add(size as u64);
            let percent = if total > 0 {
                ((u128::from(received) * 100 / u128::from(total)).min(100)) as i16
            } else {
                0
            };
            if let Some(sender) = &progress_sender {
                let _ = sender.send(percent);
            }
        }
        if received != total {
            log::warn!("download size mismatch: received {received}, expected {total}");
            return Err(velopack::Error::Other("download size mismatch".into()));
        }
        if let Some(sender) = &progress_sender {
            let _ = sender.send(100);
        }

        // Windows admission opens with FILE_SHARE_READ, so close the writer first.
        file.sync_all()?;
        drop(file);

        let mut regular_file = open_regular_file(local_file)?;
        solstone_core_installed_payload::verify_pinned_signature_reader(
            &mut regular_file,
            &sidecar_final,
            &sidecar_bytes,
        )
        .map_err(|e| {
            log::warn!("pinned signature verification failed: {e}");
            velopack::Error::Other(format!("signature verification failed: {e}"))
        })?;

        regular_file
            .rewind()
            .map_err(|e| velopack::Error::Other(format!("could not rewind package file: {e}")))?;

        parse_and_validate_nuspec(&mut regular_file, &asset.Version)?;

        {
            use std::io::Write;
            let mut sidecar = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&sidecar_partial)?;
            sidecar.write_all(&sidecar_bytes)?;
            sidecar.sync_all()?;
        }
        std::fs::rename(&sidecar_partial, &sidecar_final)?;
        Ok(())
    })();

    if stream_result.is_err() {
        let _ = std::fs::remove_file(local_file);
        let _ = std::fs::remove_file(&sidecar_partial);
    }
    stream_result
}

/// Download a directory-fed package with sidecar verification.
pub fn download_dir_package(
    dir: &Path,
    asset: &VelopackAsset,
    local_file: &Path,
    progress_sender: Option<Sender<i16>>,
) -> Result<(), velopack::Error> {
    if !asset.Type.eq_ignore_ascii_case("Full") {
        log::warn!("asset type {} is not Full", asset.Type);
        return Err(velopack::Error::Other(
            "only full updates are supported".into(),
        ));
    }
    let sidecar_name = format!("{}.minisig", asset.FileName);
    if !asset_name_is_plain(&asset.FileName) || !asset_name_is_plain(&sidecar_name) {
        log::warn!("asset filename is not plain: {}", asset.FileName);
        return Err(velopack::Error::Other("asset filename is not plain".into()));
    }
    if asset.Size == 0 {
        log::warn!("asset size is zero");
        return Err(velopack::Error::Other("asset size is zero".into()));
    }

    let source_sidecar = dir.join(&sidecar_name);
    let mut sidecar_file = open_regular_file(&source_sidecar)?;
    let mut sidecar_bytes = Vec::new();
    sidecar_file
        .by_ref()
        .take(MAX_SIDECAR_BYTES + 1)
        .read_to_end(&mut sidecar_bytes)
        .map_err(|e| {
            log::warn!("could not read sidecar: {e}");
            velopack::Error::Other(format!("could not read sidecar: {e}"))
        })?;
    if sidecar_bytes.len() as u64 > MAX_SIDECAR_BYTES {
        log::warn!("sidecar length {} exceeds limit", sidecar_bytes.len());
        return Err(velopack::Error::Other("sidecar exceeds size limit".into()));
    }

    check_sidecar_format(&source_sidecar, &sidecar_bytes)?;

    let source_package = dir.join(&asset.FileName);
    let mut source_file = open_regular_file(&source_package)?;
    let source_meta = source_file.metadata().map_err(|e| {
        velopack::Error::Other(format!("could not read source package metadata: {e}"))
    })?;
    if source_meta.len() != asset.Size {
        log::warn!(
            "source package size mismatch: {} vs {}",
            source_meta.len(),
            asset.Size
        );
        return Err(velopack::Error::Other(
            "source package size mismatch".into(),
        ));
    }

    let total = asset.Size;
    let mut received = 0_u64;
    let parent = local_file
        .parent()
        .ok_or_else(|| velopack::Error::Other("local_file has no parent".into()))?;
    let sidecar_partial = parent.join(format!("{}.minisig.partial", asset.FileName));
    let sidecar_final = parent.join(&sidecar_name);

    let copy_result = (|| -> Result<(), velopack::Error> {
        let mut dest_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(local_file)?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            use std::io::Write;
            let size = source_file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            dest_file.write_all(&buffer[..size])?;
            received = received.saturating_add(size as u64);
            let percent = if total > 0 {
                ((u128::from(received) * 100 / u128::from(total)).min(100)) as i16
            } else {
                0
            };
            if let Some(sender) = &progress_sender {
                let _ = sender.send(percent);
            }
        }
        if received != total {
            log::warn!("copy size mismatch: received {received}, expected {total}");
            return Err(velopack::Error::Other("copy size mismatch".into()));
        }
        if let Some(sender) = &progress_sender {
            let _ = sender.send(100);
        }

        // Windows admission opens with FILE_SHARE_READ, so close the writer first.
        dest_file.sync_all()?;
        drop(dest_file);

        let mut regular_dest = open_regular_file(local_file)?;
        solstone_core_installed_payload::verify_pinned_signature_reader(
            &mut regular_dest,
            &sidecar_final,
            &sidecar_bytes,
        )
        .map_err(|e| {
            log::warn!("pinned signature verification failed: {e}");
            velopack::Error::Other(format!("signature verification failed: {e}"))
        })?;

        regular_dest
            .rewind()
            .map_err(|e| velopack::Error::Other(format!("could not rewind package file: {e}")))?;

        parse_and_validate_nuspec(&mut regular_dest, &asset.Version)?;

        {
            use std::io::Write;
            let mut sidecar = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&sidecar_partial)?;
            sidecar.write_all(&sidecar_bytes)?;
            sidecar.sync_all()?;
        }
        std::fs::rename(&sidecar_partial, &sidecar_final)?;
        Ok(())
    })();

    if copy_result.is_err() {
        let _ = std::fs::remove_file(local_file);
        let _ = std::fs::remove_file(&sidecar_partial);
    }
    copy_result
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::path::Path;

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::{
        asset_name_is_plain, check_sidecar_format, parse_and_validate_nuspec_for_architecture,
    };

    // These pre-existing fixtures deliberately describe the historical x64
    // package. Test both trusted expectations independently below.
    fn parse_and_validate_nuspec<R: std::io::Read + std::io::Seek>(
        reader: R,
        version: &str,
    ) -> Result<(), velopack::Error> {
        parse_and_validate_nuspec_for_architecture(
            reader,
            version,
            solstone_core_installed_payload::WindowsArchitecture::X86_64,
        )
    }

    #[test]
    fn architecture_is_selected_by_the_caller_and_cannot_be_mixed() {
        use solstone_core_installed_payload::{WindowsArchitecture, compiled_windows_architecture};
        for architecture in WindowsArchitecture::ALL {
            let identity = super::super::package_identity(architecture);
            let nuspec = format!(
                "<package><metadata><id>SolstoneJournal</id><version>2.0.28</version><channel>{}</channel><os>win</os><rid>{}</rid><machineArchitecture>{}</machineArchitecture><mainExe>bin/journal-app.exe</mainExe></metadata></package>",
                identity.channel, identity.rid, identity.machine_architecture
            );
            let bytes = make_test_zip(&[("SolstoneJournal.nuspec", &nuspec)]);
            parse_and_validate_nuspec_for_architecture(Cursor::new(&bytes), "2.0.28", architecture)
                .unwrap();
            assert_eq!(
                super::parse_and_validate_nuspec(Cursor::new(&bytes), "2.0.28").is_ok(),
                architecture == compiled_windows_architecture()
            );
            let foreign = WindowsArchitecture::ALL
                .into_iter()
                .find(|other| *other != architecture)
                .unwrap();
            assert!(
                parse_and_validate_nuspec_for_architecture(Cursor::new(&bytes), "2.0.28", foreign)
                    .is_err()
            );
            let foreign_identity = super::super::package_identity(foreign);
            for (tag, foreign_value) in [
                ("channel", foreign_identity.channel),
                ("rid", foreign_identity.rid),
                ("machineArchitecture", foreign_identity.machine_architecture),
            ] {
                let start = nuspec.find(&format!("<{tag}>")).unwrap() + tag.len() + 2;
                let end = nuspec.find(&format!("</{tag}>")).unwrap();
                let changed = format!("{}{}{}", &nuspec[..start], foreign_value, &nuspec[end..]);
                let bytes = make_test_zip(&[("SolstoneJournal.nuspec", &changed)]);
                assert!(
                    parse_and_validate_nuspec_for_architecture(
                        Cursor::new(bytes),
                        "2.0.28",
                        architecture
                    )
                    .is_err(),
                    "{tag}"
                );
            }
        }
    }

    #[test]
    fn production_metadata_and_namespace_duplicates() {
        let stock = include_bytes!(
            "../../solstone-core-installed-payload/tests/fixtures/windows-update/journal-stock.nupkg"
        );
        parse_and_validate_nuspec(Cursor::new(stock), "2.0.49").unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(stock)).unwrap();
        let mut metadata = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("SolstoneJournal.nuspec").unwrap(),
            &mut metadata,
        )
        .unwrap();
        for extra in [
            "<foo:mainExe xmlns:foo=\"urn:test\">other.exe</foo:mainExe>",
            "<mainExe/>",
            "<foo:version xmlns:foo=\"urn:test\">2.0.50</foo:version>",
        ] {
            let changed = metadata.replace("</metadata>", &format!("{extra}</metadata>"));
            let bytes = make_test_zip(&[("SolstoneJournal.nuspec", &changed)]);
            assert!(parse_and_validate_nuspec(Cursor::new(bytes), "2.0.49").is_err());
        }
    }

    fn make_test_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut zip = ZipWriter::new(Cursor::new(&mut buf));
        for (name, content) in files {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        buf
    }

    const VALID_NUSPEC: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2010/07/nuspec.xsd">
<metadata>
  <id>SolstoneJournal</id>
  <version>2.0.28</version>
  <channel>win</channel>
  <os>win</os>
  <rid>win-x64</rid>
  <machineArchitecture>x64</machineArchitecture>
  <mainExe>bin/journal-app.exe</mainExe>
</metadata>
</package>"#;

    #[test]
    fn one_valid_nuspec_admits() {
        let zip_bytes = make_test_zip(&[("SolstoneJournal.nuspec", VALID_NUSPEC)]);
        assert!(parse_and_validate_nuspec(Cursor::new(zip_bytes), "2.0.28").is_ok());
    }

    #[test]
    fn two_nuspec_entries_refuse() {
        let zip_bytes = make_test_zip(&[
            ("SolstoneJournal.nuspec", VALID_NUSPEC),
            ("Extra.nuspec", VALID_NUSPEC),
        ]);
        let err = parse_and_validate_nuspec(Cursor::new(zip_bytes), "2.0.28");
        assert!(err.is_err());
    }

    #[test]
    fn duplicated_id_refuses() {
        let nuspec = r#"<?xml version="1.0" encoding="utf-8"?>
<package>
<metadata>
  <id>SolstoneJournal</id>
  <id>SolstoneJournal</id>
  <version>2.0.28</version>
  <channel>win</channel>
  <os>win</os>
  <rid>win-x64</rid>
  <machineArchitecture>x64</machineArchitecture>
  <mainExe>bin/journal-app.exe</mainExe>
</metadata>
</package>"#;
        let zip_bytes = make_test_zip(&[("SolstoneJournal.nuspec", nuspec)]);
        assert!(parse_and_validate_nuspec(Cursor::new(zip_bytes), "2.0.28").is_err());
    }

    #[test]
    fn duplicated_version_refuses() {
        let nuspec = r#"<?xml version="1.0" encoding="utf-8"?>
<package>
<metadata>
  <id>SolstoneJournal</id>
  <version>2.0.28</version>
  <version>2.0.28</version>
  <channel>win</channel>
  <os>win</os>
  <rid>win-x64</rid>
  <machineArchitecture>x64</machineArchitecture>
  <mainExe>bin/journal-app.exe</mainExe>
</metadata>
</package>"#;
        let zip_bytes = make_test_zip(&[("SolstoneJournal.nuspec", nuspec)]);
        assert!(parse_and_validate_nuspec(Cursor::new(zip_bytes), "2.0.28").is_err());
    }

    #[test]
    fn well_formed_identity_then_malformed_xml_refuses() {
        let nuspec = r#"<?xml version="1.0" encoding="utf-8"?>
<package>
<metadata>
  <id>SolstoneJournal</id>
  <version>2.0.28</version>
  <channel>win</channel>
  <os>win</os>
  <rid>win-x64</rid>
  <machineArchitecture>x64</machineArchitecture>
  <mainExe>bin/journal-app.exe</mainExe>
  <malformed>
</metadata>
</package>"#;
        let zip_bytes = make_test_zip(&[("SolstoneJournal.nuspec", nuspec)]);
        assert!(parse_and_validate_nuspec(Cursor::new(zip_bytes), "2.0.28").is_err());
    }

    #[test]
    fn main_exe_solstone_exe_refuses() {
        let nuspec = r#"<?xml version="1.0" encoding="utf-8"?>
<package>
<metadata>
  <id>SolstoneJournal</id>
  <version>2.0.28</version>
  <channel>win</channel>
  <os>win</os>
  <rid>win-x64</rid>
  <machineArchitecture>x64</machineArchitecture>
  <mainExe>solstone.exe</mainExe>
</metadata>
</package>"#;
        let zip_bytes = make_test_zip(&[("SolstoneJournal.nuspec", nuspec)]);
        assert!(parse_and_validate_nuspec(Cursor::new(zip_bytes), "2.0.28").is_err());
    }

    struct PanicReader;
    impl std::io::Read for PanicReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            panic!("reader was read!");
        }
    }
    impl std::io::Seek for PanicReader {
        fn seek(&mut self, _pos: std::io::SeekFrom) -> std::io::Result<u64> {
            panic!("reader was seeked!");
        }
    }

    #[test]
    fn legacy_ed_refuses_without_reading_or_seeking() {
        // Algorithm "Ed" (0x45, 0x64), 74 bytes total, base64 encoded.
        let legacy_sig = "untrusted comment: signature from test secret key\n\
RWQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n\
trusted comment: timestamp:0\n\
AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==\n";
        assert!(check_sidecar_format(Path::new("test.minisig"), legacy_sig.as_bytes()).is_err());

        let result = solstone_core_installed_payload::verify_pinned_signature_reader(
            PanicReader,
            Path::new("test.minisig"),
            legacy_sig.as_bytes(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn plain_name_checks() {
        assert!(asset_name_is_plain("SolstoneJournal-2.0.28-full.nupkg"));
        assert!(asset_name_is_plain(
            "SolstoneJournal-2.0.28-full.nupkg.minisig"
        ));
        assert!(!asset_name_is_plain(""));
        assert!(!asset_name_is_plain(".hidden"));
        assert!(!asset_name_is_plain("../traversal.nupkg"));
        assert!(!asset_name_is_plain("sub/file.nupkg"));
        assert!(!asset_name_is_plain("sub\\file.nupkg"));
    }
}
