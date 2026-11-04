// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-account privacy of the journal root. No descendant migration or ACL policy.
use serde::Serialize;
use std::os::unix::fs::PermissionsExt;
use std::{fs, io, path::Path};

pub const WARNING: &str = "other accounts on this computer may be able to read your journal. its folder could not be made private on this drive. you can keep it on this drive or move it to a drive that keeps folders private.";
pub const ACCEPTED: &str = "you chose to keep your journal on this drive, where other accounts on this computer may be able to read it.";
pub const PRIVATE: &str = "your journal's folder is private to your account on this computer.";
pub const UNAVAILABLE: &str = "your journal's folder privacy could not be checked.";

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
pub fn set_private_umask() {
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
}

/// Reapply only the root mode. A storage choice never prevents journal startup.
pub fn apply_private_root(root: &Path) {
    if let Err(error) = fs::set_permissions(root, fs::Permissions::from_mode(0o700)) {
        log::debug!("journal root privacy mode: {error}");
    }
    match inspect(root) {
        Ok(status) if !status.private && status.consented => {
            log::info!("journal root privacy: {ACCEPTED}")
        }
        Ok(status) if !status.private => log::warn!("journal root privacy: {WARNING}"),
        Err(error) => log::warn!("journal root privacy could not be checked: {error}"),
        _ => {}
    }
}

#[derive(Debug, Serialize)]
pub struct StoragePrivacy {
    pub private: bool,
    pub consented: bool,
    pub filesystem_type: String,
    pub message: &'static str,
}

/// Read-only: health and doctor must not repair modes or record consent.
pub fn inspect(root: &Path) -> io::Result<StoragePrivacy> {
    let metadata = fs::metadata(root)?;
    let (filesystem_type, ignores_ownership) = volume(root)?;
    let private =
        metadata.is_dir() && mode_is_private(metadata.permissions().mode(), ignores_ownership);
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
        message: if private {
            PRIVATE
        } else if consented {
            ACCEPTED
        } else {
            WARNING
        },
    })
}

fn mode_is_private(mode: u32, ignores_ownership: bool) -> bool {
    mode & 0o777 == 0o700 && !ignores_ownership
}

fn consent_matches(consent: &serde_json::Value, filesystem_type: &str, root: &Path) -> bool {
    consent["filesystem_type"].as_str() == Some(filesystem_type)
        && consent["root"].as_str() == root.to_str()
        && consent["accepted_at"]
            .as_str()
            .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
}

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
    #[test]
    fn actual_mode_and_ownership_policy_decide_privacy() {
        assert!(mode_is_private(0o40700, false));
        for mode in [0o755, 0o777, 0o750, 0o770] {
            assert!(!mode_is_private(mode, false));
        }
        assert!(!mode_is_private(0o700, true));
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
    #[cfg(feature = "full-tests")]
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
