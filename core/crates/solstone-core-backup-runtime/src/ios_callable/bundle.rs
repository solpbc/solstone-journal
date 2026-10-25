// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixed signed app resource discovery. No caller-selected bundle or receipt.

use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::io::{Cursor, Read};
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use nix::fcntl::{OFlag, openat};
use nix::sys::stat::Mode;
use solstone_core_journal_io::{FlatDirectory, JournalRoot, list_flat_directory};

use super::Refusal;
use super::build_identity::{ValidatedLinkedBuild, validate_linked_build};

const MAX_RECORD: u64 = 1024 * 1024;

fn stamp(metadata: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

pub(super) fn read_bound(
    directory: &impl AsFd,
    name: &OsStr,
    maximum: u64,
) -> Result<(File, Vec<u8>), Refusal> {
    let fd = openat(
        directory,
        name,
        OFlag::O_RDONLY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
        Mode::empty(),
    )
    .map_err(|_| Refusal::Bundle)?;
    let mut file = File::from(fd);
    let before = file.metadata().map_err(|_| Refusal::Bundle)?;
    if !before.is_file() || before.nlink() != 1 || before.len() > maximum {
        return Err(Refusal::Bundle);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Refusal::Bundle)?;
    let after = file.metadata().map_err(|_| Refusal::Bundle)?;
    if bytes.len() as u64 > maximum
        || bytes.len() as u64 != before.len()
        || stamp(&before) != stamp(&after)
    {
        return Err(Refusal::Bundle);
    }
    Ok((file, bytes))
}

pub(super) struct Bundle {
    root: JournalRoot,
    executable: PathBuf,
    info: File,
    info_bytes: Vec<u8>,
    record: File,
    record_directory: FlatDirectory,
    pub build: ValidatedLinkedBuild,
}

impl Bundle {
    pub fn discover() -> Result<Self, Refusal> {
        let executable = std::env::current_exe().map_err(|_| Refusal::Bundle)?;
        if executable.file_name() != Some(OsStr::new("JournalRuntimeDev")) {
            return Err(Refusal::Bundle);
        }
        let path = executable.parent().ok_or(Refusal::Bundle)?;
        if path.extension() != Some(OsStr::new("app")) {
            return Err(Refusal::Bundle);
        }
        let root = JournalRoot::open(path).map_err(|_| Refusal::Bundle)?;
        root.revalidate_canonical_binding()
            .map_err(|_| Refusal::Bundle)?;
        if root.canonical_path().join("JournalRuntimeDev") != executable {
            return Err(Refusal::Bundle);
        }
        let (info, info_bytes) = read_bound(&root, OsStr::new("Info.plist"), MAX_RECORD)?;
        let plist =
            plist::Value::from_reader(Cursor::new(&info_bytes)).map_err(|_| Refusal::Bundle)?;
        let dictionary = plist.as_dictionary().ok_or(Refusal::Bundle)?;
        for (key, value) in [
            (
                "CFBundleIdentifier",
                solstone_core_spp_attest::ios_service_tools::IOS_EXTENSION_POINT_HOST,
            ),
            ("CFBundleExecutable", "JournalRuntimeDev"),
            ("CFBundlePackageType", "APPL"),
        ] {
            if dictionary.get(key).and_then(plist::Value::as_string) != Some(value) {
                return Err(Refusal::Bundle);
            }
        }
        let record_directory =
            FlatDirectory::open(&root, Path::new("BackupRuntime")).map_err(|_| Refusal::Bundle)?;
        let members = list_flat_directory(&record_directory, 2)
            .map_err(|_| Refusal::Bundle)?
            .ok_or(Refusal::Bundle)?;
        if members.len() != 1 || members[0].name != OsStr::new("backup-build.json") {
            return Err(Refusal::Bundle);
        }
        let (record, bytes) = read_bound(
            &record_directory,
            OsStr::new("backup-build.json"),
            MAX_RECORD,
        )?;
        let build = validate_linked_build(&bytes).map_err(|_| Refusal::Build)?;
        let bundle = Self {
            root,
            executable,
            info,
            info_bytes,
            record,
            record_directory,
            build,
        };
        bundle.revalidate()?;
        Ok(bundle)
    }

    pub fn revalidate(&self) -> Result<(), Refusal> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|_| Refusal::Bundle)?;
        if std::env::current_exe().map_err(|_| Refusal::Bundle)? != self.executable {
            return Err(Refusal::Bundle);
        }
        let (info, info_bytes) = read_bound(&self.root, OsStr::new("Info.plist"), MAX_RECORD)?;
        if stamp(&info.metadata().map_err(|_| Refusal::Bundle)?)
            != stamp(&self.info.metadata().map_err(|_| Refusal::Bundle)?)
            || info_bytes != self.info_bytes
        {
            return Err(Refusal::Bundle);
        }
        let directory = FlatDirectory::open(&self.root, Path::new("BackupRuntime"))
            .map_err(|_| Refusal::Bundle)?;
        if directory.identity() != self.record_directory.identity() {
            return Err(Refusal::Bundle);
        }
        let members = list_flat_directory(&directory, 2)
            .map_err(|_| Refusal::Bundle)?
            .ok_or(Refusal::Bundle)?;
        if members.len() != 1 || members[0].name != OsStr::new("backup-build.json") {
            return Err(Refusal::Bundle);
        }
        let (file, bytes) = read_bound(&directory, OsStr::new("backup-build.json"), MAX_RECORD)?;
        if stamp(&file.metadata().map_err(|_| Refusal::Bundle)?)
            != stamp(&self.record.metadata().map_err(|_| Refusal::Bundle)?)
            || validate_linked_build(&bytes).map_err(|_| Refusal::Build)? != self.build
        {
            return Err(Refusal::Bundle);
        }
        Ok(())
    }

    pub fn same_installation(&self, other: &Self) -> bool {
        self.root.identity() == other.root.identity() && self.build == other.build
    }
}
