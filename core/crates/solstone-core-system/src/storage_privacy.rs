// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-account privacy of the journal root. No descendant migration or ACL policy.
use serde::Serialize;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::{io, path::Path};

#[cfg(windows)]
#[path = "storage_privacy_windows.rs"]
mod windows;

use solstone_core_window_catalog::{ENGLISH, text};

const WARNING_ID: &str = "journal.window.health.storage.privacy.warning";
const ACCEPTED_ID: &str = "journal.window.health.storage.privacy.accepted";
const PRIVATE_ID: &str = "journal.window.health.storage.privacy.private";
const UNAVAILABLE_ID: &str = "journal.window.health.storage.privacy.unavailable";

pub fn unavailable_message(locale: &str) -> String {
    text(locale, UNAVAILABLE_ID)
}

pub const DRIVE_CANNOT_RUN: &str = "your journal can't run on this drive because the drive can't save files the way your journal needs. use a mac-formatted drive for your journal.";

/// Whether a Mac drive lacks the two operations journal saves are built on: an exclusive rename and
/// a hard link. It asks the drive by trying both in `root`, or in the closest folder that exists yet.
/// exFAT refuses the rename and FAT refuses the link, and a volume's advertised capability doesn't
/// tell them apart. The link only counts on a local drive: that is where it has been measured to mean
/// the journal can't run. Anything but a refusal of the operation leaves the answer to startup.
#[cfg(target_os = "macos")]
#[must_use]
pub fn drive_cannot_run_journal(root: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Some(directory) = root.ancestors().find(|path| path.is_dir()) else {
        return false;
    };
    let unique = format!(".solstone-drive-check-{}", std::process::id());
    let from = directory.join(format!("{unique}-a"));
    let to = directory.join(format!("{unique}-b"));
    let linked = directory.join(format!("{unique}-c"));
    if fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&from)
        .is_err()
    {
        return false;
    }
    let refused = match (
        std::ffi::CString::new(from.as_os_str().as_bytes()),
        std::ffi::CString::new(to.as_os_str().as_bytes()),
    ) {
        (Ok(from_c), Ok(to_c)) => {
            rename_excl_refused(&from_c, &to_c)
                || (volume_is_local(directory)
                    && fs::hard_link(&to, &linked)
                        .is_err_and(|error| error.raw_os_error() == Some(libc::ENOTSUP)))
        }
        _ => false,
    };
    let _ = fs::remove_file(&from);
    let _ = fs::remove_file(&to);
    let _ = fs::remove_file(&linked);
    refused
}

#[cfg(target_os = "macos")]
fn volume_is_local(directory: &Path) -> bool {
    nix::sys::statfs::statfs(directory)
        .is_ok_and(|stats| stats.flags().contains(nix::mount::MntFlags::MNT_LOCAL))
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn rename_excl_refused(from: &std::ffi::CStr, to: &std::ffi::CStr) -> bool {
    // SAFETY: both paths are valid NUL-terminated strings that outlive the call.
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ENOTSUP)
}

/// Other Unix drives run the journal under the storage consent.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn drive_cannot_run_journal(_root: &Path) -> bool {
    false
}

/// Set once at process entry, before starting threads or writing journal files.
#[cfg(unix)]
pub fn set_private_umask() {
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
}

/// A root others can reach gets its one pass, off to the side: a large journal
/// takes a while, and a storage choice never prevents journal startup.
#[cfg(windows)]
pub fn apply_private_root(root: &Path) {
    match windows::root_is_private(root) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            log::warn!("journal root privacy could not be checked: {error}");
            return;
        }
    }
    let root = root.to_owned();
    std::thread::spawn(move || {
        match windows::make_private(&root) {
            Ok(0) => {}
            Ok(left) => log::warn!("journal root privacy: {left} items could not be changed"),
            Err(error) => log::debug!("journal root privacy: {error}"),
        }
        match inspect(&root) {
            Ok(status) if !status.private && status.consented => {
                log::info!("journal root privacy: {}", status.message)
            }
            Ok(status) if !status.private => log::warn!("journal root privacy: {}", status.message),
            Err(error) => log::warn!("journal root privacy could not be checked: {error}"),
            _ => {}
        }
    });
}

/// Reapply only the root mode. A storage choice never prevents journal startup.
#[cfg(unix)]
pub fn apply_private_root(root: &Path) {
    if let Err(error) = fs::set_permissions(root, fs::Permissions::from_mode(0o700)) {
        log::debug!("journal root privacy mode: {error}");
    }
    match inspect(root) {
        Ok(status) if !status.private && status.consented => {
            log::info!("journal root privacy: {}", status.message)
        }
        Ok(status) if !status.private => log::warn!("journal root privacy: {}", status.message),
        Err(error) => log::warn!("journal root privacy could not be checked: {error}"),
        _ => {}
    }
}

#[derive(Debug, Serialize)]
pub struct StoragePrivacy {
    pub private: bool,
    pub consented: bool,
    pub filesystem_type: String,
    pub message: String,
}

/// Ensure `path` is a plain (non-symlink, non-reparse) directory owned by the
/// current OS account and restricted to private owner-only access (`0o700` on
/// Unix, protected owner-only DACL on Windows).
///
/// Fails closed if `path` is a symlink/reparse point/regular file, owned by a
/// different account, or lies on a volume that cannot enforce private access.
#[cfg(unix)]
pub fn ensure_private_owner_dir(path: &Path) -> io::Result<()> {
    let euid = nix::unistd::geteuid().as_raw();
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.file_type().is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "private directory path is not a plain directory",
                ));
            }
            if meta.uid() != euid {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "private directory is not owned by the current user",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => {}
                Err(create_err) if create_err.kind() == io::ErrorKind::AlreadyExists => {
                    let meta = fs::symlink_metadata(path)?;
                    if !meta.file_type().is_dir() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "private directory path is not a plain directory",
                        ));
                    }
                    if meta.uid() != euid {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "private directory is not owned by the current user",
                        ));
                    }
                }
                Err(create_err) => return Err(create_err),
            }
        }
        Err(error) => return Err(error),
    }

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    let verified = fs::symlink_metadata(path)?;
    if !verified.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory path is no longer a plain directory",
        ));
    }
    if verified.uid() != euid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private directory is not owned by the current user",
        ));
    }
    let (_, ignores_ownership) = volume(path)?;
    if !mode_is_private(verified.permissions().mode(), ignores_ownership) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private directory permissions are not owner-only",
        ));
    }
    Ok(())
}

/// Ensure `path` is a plain (non-reparse) directory owned by the current OS
/// account with a protected owner-only DACL.
#[cfg(windows)]
pub fn ensure_private_owner_dir(path: &Path) -> io::Result<()> {
    windows::ensure_private_owner_dir(path)
}

/// Read-only: health and doctor must not repair modes or record consent.
pub fn inspect(root: &Path) -> io::Result<StoragePrivacy> {
    inspect_in(root, ENGLISH)
}

pub fn inspect_in(root: &Path, locale: &str) -> io::Result<StoragePrivacy> {
    #[cfg(unix)]
    let (private, filesystem_type) = {
        let metadata = fs::metadata(root)?;
        let (filesystem_type, ignores_ownership) = volume(root)?;
        (
            metadata.is_dir() && mode_is_private(metadata.permissions().mode(), ignores_ownership),
            filesystem_type,
        )
    };
    #[cfg(windows)]
    let (private, filesystem_type) = (
        windows::root_is_private(root)?,
        windows::filesystem_type(root)?,
    );
    let config = solstone_core_journal_config::read_journal_config(root)
        .map_err(io::Error::other)?
        .config;
    let consented = config
        .as_ref()
        .and_then(|c| c.get("storage_privacy_consent"))
        .is_some_and(|c| consent_matches(c, &filesystem_type, root));
    Ok(StoragePrivacy {
        private,
        consented,
        filesystem_type,
        message: text(
            locale,
            if private {
                PRIVATE_ID
            } else if consented {
                ACCEPTED_ID
            } else {
                WARNING_ID
            },
        ),
    })
}

#[cfg(unix)]
fn mode_is_private(mode: u32, ignores_ownership: bool) -> bool {
    mode & 0o777 == 0o700 && !ignores_ownership
}

fn consent_matches(consent: &serde_json::Value, filesystem_type: &str, root: &Path) -> bool {
    consent["filesystem_type"].as_str() == Some(filesystem_type)
        && consent["root"].as_str() == root.to_str()
        && consent["accepted_at"]
            .as_str()
            .is_some_and(|s| solstone_core_time::parse_rfc3339(s).is_ok())
}

#[cfg(unix)]
fn volume(root: &Path) -> io::Result<(String, bool)> {
    let stats = nix::sys::statfs::statfs(root).map_err(io::Error::from)?;
    #[cfg(target_vendor = "apple")]
    {
        Ok((
            stats.filesystem_type_name().to_owned(),
            stats
                .flags()
                .contains(nix::mount::MntFlags::MNT_IGNORE_OWNERSHIP),
        ))
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        Ok((format!("0x{:x}", stats.filesystem_type().0), false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn checking_a_drive_that_can_run_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        assert!(!drive_cannot_run_journal(root.path()));
        assert!(!drive_cannot_run_journal(&root.path().join("not/yet/made")));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[cfg(unix)]
    #[test]
    fn actual_mode_and_ownership_policy_decide_privacy() {
        assert!(mode_is_private(0o40700, false));
        for mode in [0o755, 0o777, 0o750, 0o770] {
            assert!(!mode_is_private(mode, false));
        }
        assert!(!mode_is_private(0o700, true));
    }
    #[cfg(unix)]
    #[test]
    fn ensure_private_owner_dir_enforces_mode_and_rejects_symlinks_and_files() {
        let work = tempfile::tempdir().unwrap();
        let fresh = work.path().join("fresh");
        ensure_private_owner_dir(&fresh).unwrap();
        assert_eq!(
            fs::symlink_metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let broad = work.path().join("broad");
        fs::create_dir(&broad).unwrap();
        fs::set_permissions(&broad, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_owner_dir(&broad).unwrap();
        assert_eq!(
            fs::symlink_metadata(&broad).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let regular = work.path().join("file");
        fs::write(&regular, b"x").unwrap();
        assert!(ensure_private_owner_dir(&regular).is_err());

        let link = work.path().join("link");
        std::os::unix::fs::symlink(&fresh, &link).unwrap();
        assert!(ensure_private_owner_dir(&link).is_err());
    }

    #[test]
    fn consent_is_explicit_and_bound_to_the_storage_choice() {
        let consent = serde_json::json!({"accepted_at":"2026-01-01T00:00:00Z", "filesystem_type":"fat", "root":"/journal"});
        assert!(consent_matches(&consent, "fat", Path::new("/journal")));
        assert!(!consent_matches(&consent, "apfs", Path::new("/journal")));
        assert!(!consent_matches(&consent, "fat", Path::new("/elsewhere")));
        assert!(!consent_matches(
            &serde_json::json!({"filesystem_type":"fat", "root":"/journal"}),
            "fat",
            Path::new("/journal")
        ));
    }
    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn startup_repairs_only_the_root() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("existing"), "unchanged").unwrap();
        fs::set_permissions(
            root.path().join("existing"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        apply_private_root(root.path());
        assert!(inspect(root.path()).unwrap().private);
        assert_eq!(
            fs::metadata(root.path().join("existing"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
mod process_tests {
    use super::*;
    #[test]
    fn process_umask_is_inherited_by_new_files_and_children() {
        const CHILD: &str = "SOLSTONE_TEST_PRIVATE_UMASK_CHILD";
        if std::env::var_os(CHILD).is_some() {
            set_private_umask();
            let root = tempfile::tempdir().unwrap();
            fs::write(root.path().join("output"), "private").unwrap();
            fs::create_dir(root.path().join("directory")).unwrap();
            assert_eq!(
                fs::metadata(root.path().join("output"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(root.path().join("directory"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", "umask"])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .trim()
                    .trim_start_matches('0'),
                "77"
            );
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "storage_privacy::process_tests::process_umask_is_inherited_by_new_files_and_children", "--nocapture"])
            .env(CHILD, "1").status().unwrap();
        assert!(status.success());
    }
}
