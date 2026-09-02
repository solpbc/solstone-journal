// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_journal_io::{StagedDirOptions, StagedWriteError, publish_staged_dir};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetVolumeInformationByHandleW,
    OPEN_EXISTING,
};

const MANIFEST: &[u8] = b"{\"complete\":true}\n";
const PAYLOAD: &[u8] = b"complete-payload";
const SIBLING: &[u8] = b"sibling-bytes";

#[derive(Debug, Eq, PartialEq)]
enum TreeEntry {
    Directory,
    File(Vec<u8>),
}

fn write_complete_set(staging: &Path) -> io::Result<()> {
    fs::write(staging.join("manifest.json"), MANIFEST)?;
    fs::write(staging.join("payload.bin"), PAYLOAD)?;
    Ok(())
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, TreeEntry> {
    fn walk(root: &Path, current: &Path, result: &mut BTreeMap<PathBuf, TreeEntry>) {
        let mut entries: Vec<_> = fs::read_dir(current)
            .unwrap_or_else(|error| panic!("read {}: {error}", current.display()))
            .map(|entry| entry.unwrap())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                result.insert(relative, TreeEntry::Directory);
                walk(root, &path, result);
            } else if file_type.is_file() {
                result.insert(relative, TreeEntry::File(fs::read(&path).unwrap()));
            } else {
                panic!(
                    "unexpected non-file/non-directory entry: {}",
                    path.display()
                );
            }
        }
    }

    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

fn complete_parent_tree() -> BTreeMap<PathBuf, TreeEntry> {
    BTreeMap::from([
        (PathBuf::from("bundle"), TreeEntry::Directory),
        (
            PathBuf::from("bundle/manifest.json"),
            TreeEntry::File(MANIFEST.to_vec()),
        ),
        (
            PathBuf::from("bundle/payload.bin"),
            TreeEntry::File(PAYLOAD.to_vec()),
        ),
        (PathBuf::from("keep.txt"), TreeEntry::File(SIBLING.to_vec())),
    ])
}

fn create_case(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).unwrap();
    fs::write(path.join("keep.txt"), SIBLING).unwrap();
    path
}

fn is_staging_candidate(name: &OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    (bytes.starts_with(b".stage_") || bytes.starts_with(b"_stage_")) && bytes.ends_with(b".tmp")
}

fn staging_candidates(parent: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| is_staging_candidate(&entry.file_name()))
        .map(|entry| entry.path())
        .collect();
    candidates.sort();
    candidates
}

fn assert_no_staging_candidate(parent: &Path) {
    assert!(
        staging_candidates(parent).is_empty(),
        "session staging residue remained under {}",
        parent.display()
    );
}

fn exercise_success(root: &Path) {
    let parent = create_case(root, "success");
    let destination = parent.join("bundle");
    publish_staged_dir(
        &destination,
        StagedDirOptions::default(),
        write_complete_set,
    )
    .unwrap();
    assert_eq!(snapshot_tree(&parent), complete_parent_tree());
    assert_no_staging_candidate(&parent);
}

fn exercise_preexisting_refusal(root: &Path) {
    let parent = create_case(root, "preexisting");
    let destination = parent.join("bundle");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("existing.txt"), b"preexisting").unwrap();
    let before = snapshot_tree(&parent);

    let error = publish_staged_dir(
        &destination,
        StagedDirOptions::default(),
        write_complete_set,
    )
    .expect_err("a destination observed before staging must be refused");
    assert!(
        matches!(error, StagedWriteError::Io { ref source, .. } if source.kind() == io::ErrorKind::AlreadyExists),
        "unexpected preexisting-destination error: {error}"
    );
    assert_eq!(snapshot_tree(&parent), before);
    assert_no_staging_candidate(&parent);
}

fn exercise_population_cleanup(root: &Path) {
    let parent = create_case(root, "population-error");
    let destination = parent.join("bundle");
    let foreign = parent.join(".stage_foreign.tmp");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("marker"), b"foreign").unwrap();
    let before = snapshot_tree(&parent);

    let error = publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
        fs::write(staging.join("partial.bin"), b"partial")?;
        Err::<(), _>(io::Error::other("injected population failure"))
    })
    .expect_err("population failure must be returned");
    assert!(matches!(error, StagedWriteError::Populate { .. }));
    assert_eq!(snapshot_tree(&parent), before);
    assert_eq!(staging_candidates(&parent), vec![foreign]);
}

#[derive(Clone, Copy)]
enum Competitor {
    EmptyDirectory,
    RegularFile,
    NonEmptyDirectory,
}

fn exercise_late_collision(root: &Path, name: &str, competitor: Competitor) -> &'static str {
    let parent = create_case(root, name);
    let destination = parent.join("bundle");
    let result = publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
        match competitor {
            Competitor::EmptyDirectory => fs::create_dir(&destination)?,
            Competitor::RegularFile => fs::write(&destination, b"late-file")?,
            Competitor::NonEmptyDirectory => {
                fs::create_dir(&destination)?;
                fs::write(destination.join("existing.txt"), b"late-directory")?;
            }
        }
        write_complete_set(staging)
    });

    let outcome = match result {
        Ok(()) => {
            assert_eq!(snapshot_tree(&parent), complete_parent_tree());
            "replaced"
        }
        Err(error) => {
            assert!(matches!(error, StagedWriteError::Io { .. }), "{error}");
            let expected = match competitor {
                Competitor::EmptyDirectory => BTreeMap::from([
                    (PathBuf::from("bundle"), TreeEntry::Directory),
                    (PathBuf::from("keep.txt"), TreeEntry::File(SIBLING.to_vec())),
                ]),
                Competitor::RegularFile => BTreeMap::from([
                    (
                        PathBuf::from("bundle"),
                        TreeEntry::File(b"late-file".to_vec()),
                    ),
                    (PathBuf::from("keep.txt"), TreeEntry::File(SIBLING.to_vec())),
                ]),
                Competitor::NonEmptyDirectory => BTreeMap::from([
                    (PathBuf::from("bundle"), TreeEntry::Directory),
                    (
                        PathBuf::from("bundle/existing.txt"),
                        TreeEntry::File(b"late-directory".to_vec()),
                    ),
                    (PathBuf::from("keep.txt"), TreeEntry::File(SIBLING.to_vec())),
                ]),
            };
            assert_eq!(snapshot_tree(&parent), expected);
            "refused"
        }
    };
    assert_no_staging_candidate(&parent);
    outcome
}

fn exercise_retained_stage_boundary(root: &Path) {
    let parent = create_case(root, "cleanup-blocked");
    let destination = parent.join("bundle");
    let mut held: Option<File> = None;
    let mut staging_path: Option<PathBuf> = None;

    let error = publish_staged_dir(&destination, StagedDirOptions::default(), |staging| {
        let child = staging.join("held.bin");
        fs::write(&child, b"held-stage")?;
        held = Some(
            OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .open(&child)?,
        );
        staging_path = Some(staging.to_path_buf());
        fs::create_dir(&destination)?;
        fs::write(destination.join("existing.txt"), b"late-directory")?;
        Ok::<_, io::Error>(())
    })
    .expect_err("the non-empty competitor must refuse directory publication");
    assert!(matches!(error, StagedWriteError::Io { .. }), "{error}");

    let staging = staging_path.expect("population closure records the private staging path");
    assert!(
        staging.exists(),
        "a no-delete-share child must make best-effort cleanup observably retain the stage"
    );
    assert_eq!(fs::read(staging.join("held.bin")).unwrap(), b"held-stage");
    assert_eq!(fs::read(parent.join("keep.txt")).unwrap(), SIBLING);
    assert_eq!(
        fs::read(destination.join("existing.txt")).unwrap(),
        b"late-directory"
    );
    assert_eq!(staging_candidates(&parent), vec![staging.clone()]);
    let staging_name = staging
        .strip_prefix(&parent)
        .expect("private staging path is below its parent")
        .to_path_buf();
    assert_eq!(
        snapshot_tree(&parent),
        BTreeMap::from([
            (PathBuf::from("bundle"), TreeEntry::Directory),
            (
                PathBuf::from("bundle/existing.txt"),
                TreeEntry::File(b"late-directory".to_vec()),
            ),
            (PathBuf::from("keep.txt"), TreeEntry::File(SIBLING.to_vec())),
            (staging_name.clone(), TreeEntry::Directory),
            (
                staging_name.join("held.bin"),
                TreeEntry::File(b"held-stage".to_vec()),
            ),
        ])
    );

    drop(held.take());
    fs::remove_dir_all(&staging).unwrap();
    assert_no_staging_candidate(&parent);
    assert_eq!(
        snapshot_tree(&parent),
        BTreeMap::from([
            (PathBuf::from("bundle"), TreeEntry::Directory),
            (
                PathBuf::from("bundle/existing.txt"),
                TreeEntry::File(b"late-directory".to_vec()),
            ),
            (PathBuf::from("keep.txt"), TreeEntry::File(SIBLING.to_vec()),),
        ])
    );
}

fn wait_for_marker(child: &mut Child, marker: &Path, step: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if fs::read(marker).ok().as_deref() == Some(step.as_bytes()) {
            assert!(
                child.try_wait().unwrap().is_none(),
                "staged helper exited before kill at {step}"
            );
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("staged helper did not pause at {step}");
}

fn exercise_crash_boundary(root: &Path, name: &str, step: &str, published: bool) {
    let parent = create_case(root, name);
    let destination = parent.join("bundle");
    let marker = parent.join("pause-marker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "staged_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_STAGED_HELPER_DESTINATION", &destination)
        .env("JOURNAL_IO_TEST_PAUSE_AT", step)
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    wait_for_marker(&mut child, &marker, step);
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(
        !status.success(),
        "killed staged helper unexpectedly succeeded"
    );
    assert_eq!(fs::read(parent.join("keep.txt")).unwrap(), SIBLING);

    if published {
        let mut expected = complete_parent_tree();
        expected.insert(
            PathBuf::from("pause-marker"),
            TreeEntry::File(step.as_bytes().to_vec()),
        );
        assert_eq!(snapshot_tree(&parent), expected);
        assert_no_staging_candidate(&parent);
    } else {
        assert!(
            !destination.exists(),
            "destination appeared before the rename boundary"
        );
        let candidates = staging_candidates(&parent);
        assert_eq!(
            candidates.len(),
            1,
            "killed helper must leave its private stage"
        );
        assert_eq!(
            fs::read(candidates[0].join("manifest.json")).unwrap(),
            MANIFEST
        );
        assert_eq!(
            fs::read(candidates[0].join("payload.bin")).unwrap(),
            PAYLOAD
        );
        let candidate_name = candidates[0]
            .strip_prefix(&parent)
            .expect("private staging path is below its parent")
            .to_path_buf();
        assert_eq!(
            snapshot_tree(&parent),
            BTreeMap::from([
                (PathBuf::from("keep.txt"), TreeEntry::File(SIBLING.to_vec())),
                (
                    PathBuf::from("pause-marker"),
                    TreeEntry::File(step.as_bytes().to_vec()),
                ),
                (candidate_name.clone(), TreeEntry::Directory),
                (
                    candidate_name.join("manifest.json"),
                    TreeEntry::File(MANIFEST.to_vec()),
                ),
                (
                    candidate_name.join("payload.bin"),
                    TreeEntry::File(PAYLOAD.to_vec()),
                ),
            ])
        );
        fs::remove_dir_all(&candidates[0]).unwrap();
    }
}

fn exercise_filesystem(root: &Path) -> [&'static str; 3] {
    exercise_success(root);
    exercise_preexisting_refusal(root);
    exercise_population_cleanup(root);
    let outcomes = [
        exercise_late_collision(root, "late-empty", Competitor::EmptyDirectory),
        exercise_late_collision(root, "late-file", Competitor::RegularFile),
        exercise_late_collision(root, "late-nonempty", Competitor::NonEmptyDirectory),
    ];
    exercise_retained_stage_boundary(root);
    exercise_crash_boundary(root, "crash-before", "after-populate", false);
    exercise_crash_boundary(root, "crash-after", "after-rename", true);
    outcomes
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn filesystem_name(path: &Path) -> io::Result<String> {
    let wide_path = wide(path.as_os_str());
    // SAFETY: `wide_path` is NUL-terminated and a successful handle is owned exactly once.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` passed the invalid-handle sentinel check and is uniquely owned here.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut filesystem = [0u16; 256];
    let mut volume_name = [0u16; 256];
    let mut serial = 0;
    let mut maximum_component_length = 0;
    let mut flags = 0;
    // SAFETY: the output buffer is writable for its supplied length and the handle is valid.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetVolumeInformationByHandleW(
            handle.as_raw_handle(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            &mut serial,
            &mut maximum_component_length,
            &mut flags,
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    let terminator = filesystem
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "filesystem name is not terminated",
            )
        })?;
    String::from_utf16(&filesystem[..terminator])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "filesystem name is not UTF-16"))
}

fn windows_version() -> String {
    let output = Command::new("cmd.exe")
        .args(["/C", "ver"])
        .output()
        .expect("query native Windows version");
    assert!(output.status.success(), "cmd.exe /C ver failed");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn staged_pause_helper() {
    let Some(destination) = std::env::var_os("JOURNAL_IO_STAGED_HELPER_DESTINATION") else {
        return;
    };
    publish_staged_dir(
        Path::new(&destination),
        StagedDirOptions::default(),
        write_complete_set,
    )
    .unwrap();
}

#[test]
fn staged_protocol_covers_ntfs_and_refs() {
    let ntfs = tempfile::Builder::new()
        .prefix("solstone-staged-ntfs-")
        .tempdir()
        .unwrap();
    assert_eq!(filesystem_name(ntfs.path()).unwrap(), "NTFS");

    let refs_root = std::env::var_os("SOLSTONE_JOURNAL_WIN_REFS_ROOT")
        .map(PathBuf::from)
        .expect("staged protocol requires SOLSTONE_JOURNAL_WIN_REFS_ROOT");
    assert_eq!(filesystem_name(&refs_root).unwrap(), "ReFS");
    let refs = tempfile::Builder::new()
        .prefix("solstone-staged-refs-")
        .tempdir_in(refs_root)
        .unwrap();
    assert_eq!(filesystem_name(refs.path()).unwrap(), "ReFS");

    let ntfs_outcomes = exercise_filesystem(ntfs.path());
    let refs_outcomes = exercise_filesystem(refs.path());

    println!("JOURNAL_WIN_CI_STAGED_OS={}", windows_version());
    println!(
        "JOURNAL_WIN_CI_STAGED_NTFS_OUTCOMES=empty:{}/file:{}/nonempty:{}",
        ntfs_outcomes[0], ntfs_outcomes[1], ntfs_outcomes[2]
    );
    println!(
        "JOURNAL_WIN_CI_STAGED_REFS_OUTCOMES=empty:{}/file:{}/nonempty:{}",
        refs_outcomes[0], refs_outcomes[1], refs_outcomes[2]
    );
    println!("JOURNAL_WIN_CI_STAGED=publish/race/crash/cleanup/pass");
}
