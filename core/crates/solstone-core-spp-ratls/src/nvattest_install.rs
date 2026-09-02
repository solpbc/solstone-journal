// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use crate::NvattestEnsureStatus;

#[cfg(unix)]
use crate::check_nvattest_readiness;
#[cfg(unix)]
use crate::nvattest_authority::{
    AuthorityError, NVATTEST_AUTHORITY_JSON, NvattestArtifactSpec, nvattest_platform_key,
    parse_nvattest_target,
};
#[cfg(unix)]
use solstone_core_artifact_download::{
    ArchiveError, DownloadHostPolicy, PRODUCTION_DOWNLOAD_POLICY, clear_macos_quarantine,
    download_verified_origin, make_executable,
};

#[cfg(not(unix))]
pub fn ensure_nvattest_installed(_nvattest_dir: &Path) -> NvattestEnsureStatus {
    NvattestEnsureStatus::PlatformUnsupported
}

#[cfg(unix)]
pub fn ensure_nvattest_installed(nvattest_dir: &Path) -> NvattestEnsureStatus {
    match nvattest_platform_key(std::env::consts::OS, std::env::consts::ARCH) {
        None => NvattestEnsureStatus::PlatformUnsupported,
        Some(platform) => ensure_nvattest_installed_with(
            nvattest_dir,
            platform,
            NVATTEST_AUTHORITY_JSON,
            &PRODUCTION_DOWNLOAD_POLICY,
        ),
    }
}

#[cfg(unix)]
pub(crate) fn ensure_nvattest_installed_with(
    nvattest_dir: &Path,
    platform: &str,
    authority_json: &str,
    policy: &DownloadHostPolicy<'_>,
) -> NvattestEnsureStatus {
    let spec = match parse_nvattest_target(authority_json, platform) {
        Ok(spec) => spec,
        Err(AuthorityError::PlatformUnsupported) => {
            return NvattestEnsureStatus::PlatformUnsupported;
        }
        Err(AuthorityError::Malformed) => return NvattestEnsureStatus::Unavailable,
    };
    ensure_with(nvattest_dir, spec, policy)
}

#[cfg(all(unix, feature = "test-hooks"))]
#[doc(hidden)]
pub fn ensure_nvattest_installed_with_for_tests(
    nvattest_dir: &Path,
    platform: &str,
    authority_json: &str,
    policy: &DownloadHostPolicy<'_>,
) -> NvattestEnsureStatus {
    ensure_nvattest_installed_with(nvattest_dir, platform, authority_json, policy)
}

#[cfg(unix)]
fn ensure_with(
    nvattest_dir: &Path,
    spec: NvattestArtifactSpec,
    policy: &DownloadHostPolicy<'_>,
) -> NvattestEnsureStatus {
    if payload_matches(nvattest_dir, &spec) {
        return NvattestEnsureStatus::AlreadyInstalled;
    }
    let parent = parent_dir(nvattest_dir);
    let _lock = match acquire_lock(&parent) {
        Ok(Some(lock)) => lock,
        Ok(None) => return NvattestEnsureStatus::InstallInFlight,
        Err(_) => return NvattestEnsureStatus::InstallFailed,
    };
    if payload_matches(nvattest_dir, &spec) {
        return NvattestEnsureStatus::AlreadyInstalled;
    }
    reclaim_orphans(&parent, live_target(nvattest_dir).as_deref());
    let attempt_id = new_attempt_id();
    let install_target = parent.join(format!(".nvattest-install-{attempt_id}"));
    if fs::create_dir_all(&install_target).is_err() {
        return NvattestEnsureStatus::InstallFailed;
    }
    match install_into(&install_target, &spec, policy, &attempt_id) {
        Ok(()) => match publish_symlink(nvattest_dir, &parent, &attempt_id) {
            Ok(()) => {
                reclaim_orphans(&parent, live_target(nvattest_dir).as_deref());
                NvattestEnsureStatus::Installed
            }
            Err(()) => abort_attempt(&install_target, &parent, &attempt_id),
        },
        Err(()) => abort_attempt(&install_target, &parent, &attempt_id),
    }
}

#[cfg(unix)]
use std::fs::{self, File, OpenOptions};
#[cfg(unix)]
use std::io::{BufReader, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::path::{Component, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
#[allow(deprecated)]
use nix::fcntl::{FlockArg, flock};
#[cfg(unix)]
use serde::{Deserialize, Serialize};

#[cfg(unix)]
static ATTEMPT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
struct InstallLock {
    _file: File,
}

#[cfg(unix)]
#[derive(Serialize, Deserialize)]
struct InstallSidecar {
    schema_version: u64,
    platform: String,
    version: String,
    artifact_name: String,
    artifact_sha256: String,
    artifact_size_bytes: u64,
    attempt_id: String,
    installed_at_unix: u64,
}

#[cfg(unix)]
fn parent_dir(nvattest_dir: &Path) -> PathBuf {
    match nvattest_dir.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(unix)]
fn new_attempt_id() -> String {
    format!(
        "{:x}-{:x}",
        std::process::id(),
        ATTEMPT_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(unix)]
#[allow(deprecated)]
fn acquire_lock(parent: &Path) -> std::io::Result<Option<InstallLock>> {
    fs::create_dir_all(parent)?;
    let path = parent.join(".nvattest-install.lock");
    let deadline = Instant::now() + Duration::from_millis(250);
    for attempt in 0..5 {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        match flock(file.as_raw_fd(), FlockArg::LockExclusiveNonblock) {
            Ok(()) => return Ok(Some(InstallLock { _file: file })),
            Err(nix::errno::Errno::EWOULDBLOCK) => {
                if attempt == 4 || Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(std::io::Error::other(error)),
        }
    }
    Ok(None)
}

#[cfg(unix)]
fn payload_matches(nvattest_dir: &Path, spec: &NvattestArtifactSpec) -> bool {
    if check_nvattest_readiness(nvattest_dir) != NvattestEnsureStatus::AlreadyInstalled {
        return false;
    }
    let Ok(bytes) = fs::read(nvattest_dir.join(".nvattest-install.json")) else {
        return false;
    };
    let Ok(sidecar) = serde_json::from_slice::<InstallSidecar>(&bytes) else {
        return false;
    };
    sidecar.schema_version == 1
        && sidecar.artifact_sha256 == spec.sha256
        && sidecar.platform == spec.platform
}

#[cfg(unix)]
fn live_target(nvattest_dir: &Path) -> Option<PathBuf> {
    let meta = fs::symlink_metadata(nvattest_dir).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let target = fs::read_link(nvattest_dir).ok()?;
    Some(parent_dir(nvattest_dir).join(target))
}

#[cfg(unix)]
fn reclaim_orphans(parent: &Path, live: Option<&Path>) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !(name.starts_with(".nvattest-install-") || name.starts_with(".nvattest-link-")) {
            continue;
        }
        let path = entry.path();
        if let Some(live) = live
            && (path == live
                || path
                    .canonicalize()
                    .ok()
                    .zip(live.canonicalize().ok())
                    .is_some_and(|(left, right)| left == right))
        {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() || file_type.is_file() {
            let _ = fs::remove_file(&path);
        } else if file_type.is_dir() {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

#[cfg(unix)]
fn abort_attempt(install_target: &Path, parent: &Path, attempt_id: &str) -> NvattestEnsureStatus {
    let _ = fs::remove_dir_all(install_target);
    let _ = fs::remove_file(parent.join(format!(".nvattest-link-{attempt_id}")));
    NvattestEnsureStatus::InstallFailed
}

#[cfg(unix)]
fn install_into(
    install_target: &Path,
    spec: &NvattestArtifactSpec,
    policy: &DownloadHostPolicy<'_>,
    attempt_id: &str,
) -> Result<(), ()> {
    let stage = install_target.join(".stage");
    fs::create_dir_all(&stage).map_err(|_| ())?;
    let archive = stage.join(&spec.name);
    download_verified_origin(
        &spec.origin_key,
        &spec.sha256,
        Some(spec.size_bytes),
        &archive,
        policy,
        |_, _| {},
    )
    .map_err(|_| ())?;
    let tar_path = stage.join("payload.tar");
    decompress_xz(&archive, &tar_path)?;
    extract_tar(&tar_path, install_target).map_err(|_| ())?;
    for entry in &spec.inventory {
        let path = install_target.join(&entry.relpath);
        if !path.exists() {
            return Err(());
        }
        if entry.executable {
            make_executable(&path).map_err(|_| ())?;
        }
    }
    if check_nvattest_readiness(install_target) != NvattestEnsureStatus::AlreadyInstalled {
        return Err(());
    }
    clear_macos_quarantine(install_target).map_err(|_| ())?;
    write_sidecar(install_target, spec, attempt_id)?;
    let _ = fs::remove_dir_all(&stage);
    Ok(())
}

#[cfg(unix)]
fn decompress_xz(archive: &Path, tar_path: &Path) -> Result<(), ()> {
    let file = File::open(archive).map_err(|_| ())?;
    let mut input = BufReader::new(file);
    let mut output = File::create(tar_path).map_err(|_| ())?;
    lzma_rs::xz_decompress(&mut input, &mut output).map_err(|_| ())?;
    output.sync_all().map_err(|_| ())?;
    Ok(())
}

#[cfg(unix)]
fn extract_tar(archive: &Path, destination: &Path) -> Result<(), ArchiveError> {
    let mut tar = tar::Archive::new(File::open(archive)?);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if escapes_destination(&path)
            || path == Path::new(".nvattest-install.json")
            || path.starts_with(".stage")
        {
            return Err(ArchiveError::PathEscape(path.display().to_string()));
        }
        let output = destination.join(&path);
        if !output.starts_with(destination) {
            return Err(ArchiveError::PathEscape(path.display().to_string()));
        }
        if (entry.header().entry_type().is_symlink() || entry.header().entry_type().is_hard_link())
            && let Some(link) = entry.link_name()?
            && !link_stays_within(
                destination,
                if entry.header().entry_type().is_symlink() {
                    output.parent().unwrap_or(destination)
                } else {
                    destination
                },
                &link,
            )
        {
            return Err(ArchiveError::PathEscape(format!(
                "{} -> {}",
                path.display(),
                link.display()
            )));
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(output)?;
    }
    Ok(())
}

#[cfg(unix)]
fn escapes_destination(path: &Path) -> bool {
    path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

#[cfg(unix)]
fn link_stays_within(root: &Path, base: &Path, link: &Path) -> bool {
    if link.is_absolute() {
        return false;
    }
    let Ok(relative_base) = base.strip_prefix(root) else {
        return false;
    };
    let mut components: Vec<_> = relative_base
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            _ => None,
        })
        .collect();
    for component in link.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => components.push(value),
            Component::ParentDir => {
                if components.pop().is_none() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(unix)]
fn write_sidecar(
    install_target: &Path,
    spec: &NvattestArtifactSpec,
    attempt_id: &str,
) -> Result<(), ()> {
    let sidecar = InstallSidecar {
        schema_version: 1,
        platform: spec.platform.clone(),
        version: spec.version.clone(),
        artifact_name: spec.name.clone(),
        artifact_sha256: spec.sha256.clone(),
        artifact_size_bytes: spec.size_bytes,
        attempt_id: attempt_id.to_owned(),
        installed_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    let path = install_target.join(".nvattest-install.json");
    let mut file = File::create(&path).map_err(|_| ())?;
    file.write_all(
        serde_json::to_vec_pretty(&sidecar)
            .map_err(|_| ())?
            .as_slice(),
    )
    .map_err(|_| ())?;
    file.write_all(b"\n").map_err(|_| ())?;
    file.sync_all().map_err(|_| ())?;
    Ok(())
}

#[cfg(unix)]
fn publish_symlink(nvattest_dir: &Path, parent: &Path, attempt_id: &str) -> Result<(), ()> {
    let relative = format!(".nvattest-install-{attempt_id}");
    let temp_link = parent.join(format!(".nvattest-link-{attempt_id}"));
    let _ = fs::remove_file(&temp_link);
    std::os::unix::fs::symlink(&relative, &temp_link).map_err(|_| ())?;
    match fs::symlink_metadata(nvattest_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(meta) if meta.file_type().is_symlink() => {}
        Ok(_) | Err(_) => {
            let displaced = parent.join(format!(".nvattest-install-displaced-{attempt_id}"));
            fs::rename(nvattest_dir, &displaced).map_err(|_| ())?;
        }
    }
    fs::rename(&temp_link, nvattest_dir).map_err(|_| ())?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    use sha2::{Digest, Sha256};
    use solstone_core_artifact_download::{DownloadHostPolicy, PRODUCTION_DOWNLOAD_POLICY};

    use super::{acquire_lock, ensure_nvattest_installed_with, parent_dir, payload_matches};
    use crate::NvattestEnsureStatus;
    use crate::nvattest_authority::NVATTEST_AUTHORITY_JSON;
    use crate::test_support::TempDir;

    const PLATFORM: &str = "linux-x86_64";
    const ARTIFACT: &str = "payload.tar.xz";

    fn digest(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn pack_tar_xz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (name, data) in files {
                let mut header = tar::Header::new_gnu();
                header.set_path(*name).unwrap();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append(&header, *data).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut tar_bytes.as_slice(), &mut xz).unwrap();
        xz
    }

    fn valid_archive() -> Vec<u8> {
        pack_tar_xz(&[
            ("bin/nvattest", b"placeholder"),
            ("lib/.keep", b""),
            ("share/ca/ca-bundle.pem", b"CA"),
        ])
    }

    fn authority(sha256: &str, size: u64) -> String {
        format!(
            r#"{{"targets":{{"{PLATFORM}":{{"artifact":{{"name":"{ARTIFACT}","sha256":"{sha256}","size_bytes":{size},"url":"https://updates.solstone.app/providers/nvattest/{ARTIFACT}"}},"inventory":[{{"executable":true,"kind":"regular","relpath":"bin/nvattest","symlink_target":null}},{{"executable":false,"kind":"regular","relpath":"share/ca/ca-bundle.pem","symlink_target":null}}],"source":{{"version":"test"}}}}}}}}"#
        )
    }

    fn policy<'a>(origin: &'a str) -> DownloadHostPolicy<'a> {
        DownloadHostPolicy {
            allowed_hosts: &["127.0.0.1"],
            allow_http: true,
            origin_base_url: origin,
        }
    }

    fn stage_ready(dir: &std::path::Path, sha256: &str) {
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::create_dir_all(dir.join("share/ca")).unwrap();
        fs::write(dir.join("bin/nvattest"), "placeholder").unwrap();
        fs::write(dir.join("share/ca/ca-bundle.pem"), "CA").unwrap();
        fs::write(
            dir.join(".nvattest-install.json"),
            format!(
                r#"{{"schema_version":1,"platform":"{PLATFORM}","version":"test","artifact_name":"{ARTIFACT}","artifact_sha256":"{sha256}","artifact_size_bytes":1,"attempt_id":"0","installed_at_unix":0}}"#
            ),
        )
        .unwrap();
    }

    fn attempt_dirs(parent: &std::path::Path) -> Vec<String> {
        fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name();
                let name = name.to_str()?.to_owned();
                name.starts_with(".nvattest-install-").then_some(name)
            })
            .collect()
    }

    #[test]
    fn missing_platform_does_not_fetch() {
        let root = TempDir::new("platform");
        let dest = root.path().join("nvattest");
        assert_eq!(
            ensure_nvattest_installed_with(
                &dest,
                "windows-x86_64",
                NVATTEST_AUTHORITY_JSON,
                &PRODUCTION_DOWNLOAD_POLICY,
            ),
            NvattestEnsureStatus::PlatformUnsupported
        );
        assert!(!dest.exists());
    }

    #[test]
    fn malformed_authority_is_unavailable_not_install_failed() {
        let root = TempDir::new("malformed");
        let dest = root.path().join("nvattest");
        assert_eq!(
            ensure_nvattest_installed_with(
                &dest,
                PLATFORM,
                "not json",
                &policy("http://127.0.0.1:1")
            ),
            NvattestEnsureStatus::Unavailable
        );
        assert!(!dest.exists());
        assert!(attempt_dirs(&parent_dir(&dest)).is_empty());
    }

    #[test]
    fn matching_sidecar_short_circuits() {
        let root = TempDir::new("ready");
        let dest = root.path().join("nvattest");
        let archive = valid_archive();
        let sha = digest(&archive);
        stage_ready(&dest, &sha);
        let json = authority(&sha, archive.len() as u64);
        let before = fs::metadata(dest.join("bin/nvattest")).unwrap();
        assert_eq!(
            ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy("http://127.0.0.1:1")),
            NvattestEnsureStatus::AlreadyInstalled
        );
        let after = fs::metadata(dest.join("bin/nvattest")).unwrap();
        assert_eq!(before.ino(), after.ino());
        assert!(payload_matches(
            &dest,
            &crate::nvattest_authority::parse_nvattest_target(&json, PLATFORM).unwrap()
        ));
    }

    #[test]
    fn held_lock_is_install_in_flight() {
        let root = TempDir::new("lock");
        let dest = root.path().join("nvattest");
        let archive = valid_archive();
        let json = authority(&digest(&archive), archive.len() as u64);
        let _held = acquire_lock(&parent_dir(&dest)).unwrap().unwrap();
        assert_eq!(
            ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy("http://127.0.0.1:1")),
            NvattestEnsureStatus::InstallInFlight
        );
        assert!(!dest.exists());
        assert!(attempt_dirs(&parent_dir(&dest)).is_empty());
    }
}
