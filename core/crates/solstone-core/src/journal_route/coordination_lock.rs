// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Validation for the shell-joinable per-prefix journal-route lock.

use std::fs::{self, File};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[cfg(test)]
use std::fs::OpenOptions;
#[cfg(test)]
use std::io::Write;
#[cfg(test)]
use std::os::unix::fs::OpenOptionsExt;

use nix::fcntl::{AtFlags, OFlag, open, openat};
use nix::sys::stat::{Mode, SFlag, fstat, fstatat};

const DIRECTORY_NAME: &str = ".solstone-route.lock";
const OWNER_NAME: &str = "owner";
const MAGIC: &str = "solstone-route-lock-v1";
const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const OWNER_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_NONBLOCK);

/// The on-disk route-lock state visible to the repair record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RouteLockState {
    Validated,
    Missing,
    Invalid,
    OwnerMismatch,
}

#[must_use]
pub(crate) fn lock_path(prefix: &Path) -> std::path::PathBuf {
    prefix.join(DIRECTORY_NAME)
}

/// Validates a lock owned by the calling shell without acquiring or changing it.
#[must_use]
pub(crate) fn validate(prefix: &Path, owner_token: &str) -> RouteLockState {
    if !is_valid_owner_token(owner_token) {
        return RouteLockState::Invalid;
    }
    let directory = match open_directory(&lock_path(prefix)) {
        Ok(directory) => directory,
        Err(state) => return state,
    };
    let owner = match open_owner(&directory) {
        Ok(owner) => owner,
        Err(state) => return state,
    };
    let mut bytes = Vec::new();
    let file = File::from(owner);
    if file.take(128).read_to_end(&mut bytes).is_err() {
        return RouteLockState::Invalid;
    }
    let Ok(contents) = std::str::from_utf8(&bytes) else {
        return RouteLockState::Invalid;
    };
    let Some((magic, token)) = contents.split_once('\n') else {
        return RouteLockState::Invalid;
    };
    if magic != MAGIC || !token.ends_with('\n') || token.matches('\n').count() != 1 {
        return RouteLockState::Invalid;
    }
    let token = &token[..token.len() - 1];
    if !is_valid_owner_token(token) {
        return RouteLockState::Invalid;
    }
    if token == owner_token {
        RouteLockState::Validated
    } else {
        RouteLockState::OwnerMismatch
    }
}

#[must_use]
pub(crate) fn is_valid_owner_token(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn open_directory(path: &Path) -> Result<OwnedFd, RouteLockState> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(RouteLockState::Missing);
        }
        Err(_) => return Err(RouteLockState::Invalid),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(RouteLockState::Invalid);
    }
    let directory = open(path, DIRECTORY_FLAGS, Mode::empty()).map_err(|error| {
        if error == nix::errno::Errno::ENOENT {
            RouteLockState::Missing
        } else {
            RouteLockState::Invalid
        }
    })?;
    if !is_directory_with_mode(&directory, 0o700) {
        return Err(RouteLockState::Invalid);
    }
    Ok(directory)
}

fn open_owner(directory: &OwnedFd) -> Result<OwnedFd, RouteLockState> {
    let metadata =
        fstatat(directory, OWNER_NAME, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(|error| {
            if error == nix::errno::Errno::ENOENT {
                RouteLockState::Missing
            } else {
                RouteLockState::Invalid
            }
        })?;
    if !is_regular_with_mode(metadata.st_mode, 0o600) {
        return Err(RouteLockState::Invalid);
    }
    let owner = openat(directory, OWNER_NAME, OWNER_FLAGS, Mode::empty()).map_err(|error| {
        if error == nix::errno::Errno::ENOENT {
            RouteLockState::Missing
        } else {
            RouteLockState::Invalid
        }
    })?;
    let metadata = fstat(&owner).map_err(|_| RouteLockState::Invalid)?;
    if !is_regular_with_mode(metadata.st_mode, 0o600) {
        return Err(RouteLockState::Invalid);
    }
    Ok(owner)
}

fn is_directory_with_mode(directory: &OwnedFd, mode: nix::libc::mode_t) -> bool {
    fstat(directory).is_ok_and(|metadata| {
        SFlag::from_bits_truncate(metadata.st_mode).contains(SFlag::S_IFDIR)
            && metadata.st_mode & 0o7777 == mode
    })
}

fn is_regular_with_mode(mode: nix::libc::mode_t, expected: nix::libc::mode_t) -> bool {
    SFlag::from_bits_truncate(mode).contains(SFlag::S_IFREG) && mode & 0o7777 == expected
}

#[cfg(test)]
pub(crate) fn acquire_for_test(prefix: &Path, owner_token: &str) -> std::io::Result<()> {
    assert!(
        is_valid_owner_token(owner_token),
        "test lock owner token is valid"
    );
    let directory = lock_path(prefix);
    fs::create_dir(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let owner_path = directory.join(OWNER_NAME);
    let mut owner = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&owner_path)?;
    owner.set_permissions(fs::Permissions::from_mode(0o600))?;
    owner.write_all(format!("{MAGIC}\n{owner_token}\n").as_bytes())?;
    owner.sync_all()?;
    File::open(&directory)?.sync_all()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::tempdir;

    use super::{RouteLockState, acquire_for_test, lock_path, validate};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn prefix() -> tempfile::TempDir {
        tempdir().expect("temporary prefix")
    }

    #[test]
    fn valid_lock_validates() {
        let prefix = prefix();
        acquire_for_test(prefix.path(), TOKEN).expect("create lock");
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Validated);
    }

    #[test]
    fn missing_lock_directory_or_owner_is_missing() {
        let prefix = prefix();
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Missing);
        acquire_for_test(prefix.path(), TOKEN).expect("create lock");
        fs::remove_file(lock_path(prefix.path()).join("owner")).expect("remove owner");
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Missing);
    }

    #[test]
    fn unsafe_modes_and_symlinks_are_invalid() {
        let prefix = prefix();
        acquire_for_test(prefix.path(), TOKEN).expect("create lock");
        let directory = lock_path(prefix.path());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("wrong mode");
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Invalid);

        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("restore mode");
        let owner = directory.join("owner");
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o644)).expect("wrong owner mode");
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Invalid);

        fs::remove_dir_all(&directory).expect("remove lock");
        let outside = prefix.path().join("outside");
        fs::create_dir(&outside).expect("create outside");
        symlink(&outside, &directory).expect("symlink directory");
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Invalid);

        fs::remove_file(&directory).expect("remove directory symlink");
        acquire_for_test(prefix.path(), TOKEN).expect("recreate lock");
        let owner = directory.join("owner");
        fs::remove_file(&owner).expect("remove owner");
        let outside_owner = prefix.path().join("outside-owner");
        fs::write(&outside_owner, "owner").expect("write outside owner");
        symlink(&outside_owner, &owner).expect("symlink owner");
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Invalid);
    }

    #[test]
    fn malformed_owner_content_is_invalid_and_mismatch_is_distinct() {
        let prefix = prefix();
        acquire_for_test(prefix.path(), TOKEN).expect("create lock");
        let owner = lock_path(prefix.path()).join("owner");
        fs::write(&owner, format!("wrong\n{TOKEN}\n")).expect("wrong magic");
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o600)).expect("owner mode");
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Invalid);

        fs::write(
            &owner,
            "solstone-route-lock-v1\nzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\n",
        )
        .expect("non-hex token");
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o600)).expect("owner mode");
        assert_eq!(validate(prefix.path(), TOKEN), RouteLockState::Invalid);

        acquire_for_test(prefix.path(), TOKEN).expect_err("existing lock remains exclusive");
        fs::write(
            &owner,
            "solstone-route-lock-v1\nfedcba9876543210fedcba9876543210\n",
        )
        .expect("other valid owner");
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o600)).expect("owner mode");
        assert_eq!(
            validate(prefix.path(), TOKEN),
            RouteLockState::OwnerMismatch
        );
    }

    #[test]
    fn uppercase_caller_token_is_invalid_before_comparison() {
        let prefix = prefix();
        acquire_for_test(prefix.path(), TOKEN).expect("create lock");
        assert_eq!(
            validate(prefix.path(), "0123456789ABCDEF0123456789ABCDEF"),
            RouteLockState::Invalid
        );
    }
}
