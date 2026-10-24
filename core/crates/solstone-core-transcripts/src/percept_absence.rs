// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Absence is stronger than an empty rendered transcript: unreadable or empty
//! inputs must never turn a conditional talent skip into a completion receipt.

use solstone_core_journal_io::{JournalEntryKind, JournalRoot, paths::Segment};
use solstone_core_processing_record::{MediaKind, media_kind};
use std::path::{Component, Path};

#[cfg(unix)]
use solstone_core_journal_io::{
    FlatDirectory as Directory, list_flat_directory as list, open_flat_directory_bound as open,
};
#[cfg(windows)]
use solstone_core_journal_io::{
    WindowsFlatDirectory as Directory, list_windows_flat_directory as list,
    open_windows_flat_directory_bound as open,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PerceptFamily {
    Screen,
    Browser,
    ProtectedVideo,
    DesktopActivity,
}

impl PerceptFamily {
    pub(crate) fn matches(self, name: &str) -> bool {
        match self {
            Self::Screen => name.ends_with("screen.jsonl"),
            Self::Browser => name.starts_with("browser_") && name.ends_with(".jsonl"),
            Self::ProtectedVideo => name.ends_with("_protected_video.jsonl"),
            Self::DesktopActivity => name.ends_with("_desktop_activity.jsonl"),
        }
    }
}

fn acquire(root: &JournalRoot, relative: &Path) -> Result<Directory, String> {
    root.revalidate().map_err(|e| e.to_string())?;
    let mut parent: Option<Directory> = None;
    let mut diagnostic = root.canonical_path().to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("invalid segment path".into());
        };
        let next = match &parent {
            Some(parent) => open(parent, name, &diagnostic),
            None => open(root, name, &diagnostic),
        }
        .map_err(|e| e.to_string())?
        .ok_or("segment disappeared")?;
        parent = Some(next);
        diagnostic.push(name);
    }
    parent.ok_or("segment path is empty".into())
}

fn identity(directory: &Directory) -> solstone_core_journal_io::ObjectIdentity {
    #[cfg(unix)]
    {
        directory.identity()
    }
    #[cfg(windows)]
    {
        directory.object_identity()
    }
}

/// Establish genuine absence of all source families loaded as screen percepts.
/// Raw video also prevents absence: its Describe projection may still be owed.
/// Uses bound no-follow directories, bounded listings and a reopened path check.
pub fn screen_inputs_absent(journal: &Path, segment: &Segment) -> Result<bool, String> {
    checked_absence(journal, segment, || {})
}

fn checked_absence(
    journal: &Path,
    segment: &Segment,
    after_list: impl FnOnce(),
) -> Result<bool, String> {
    let root = JournalRoot::open(journal).map_err(|e| e.to_string())?;
    let relative = segment
        .path()
        .strip_prefix(journal)
        .map_err(|e| e.to_string())?;
    let directory = acquire(&root, relative)?;
    let entries = list(&directory, 1024)
        .map_err(|e| e.to_string())?
        .ok_or("segment inventory exceeds absence budget")?;
    let mut absent = true;
    for entry in &entries {
        let name = entry
            .name
            .to_str()
            .ok_or("segment inventory name is not UTF-8")?;
        // Rendering keeps its exact names. Absence is conservative about native
        // case aliases, including Unicode capitals of the known ASCII names.
        let folded = name.to_uppercase().to_ascii_lowercase();
        if [
            PerceptFamily::Screen,
            PerceptFamily::Browser,
            PerceptFamily::ProtectedVideo,
            PerceptFamily::DesktopActivity,
        ]
        .into_iter()
        .any(|family| family.matches(&folded))
            || Path::new(name)
                .extension()
                .and_then(|ext| ext.to_str())
                .and_then(media_kind)
                == Some(MediaKind::Video)
        {
            absent = false;
        }
        if !matches!(
            entry.kind,
            JournalEntryKind::RegularFile | JournalEntryKind::Directory
        ) {
            return Err("segment inventory contains a nonregular entry".into());
        }
    }
    after_list();
    let rebound = acquire(&root, relative)?;
    if identity(&rebound) != identity(&directory)
        || list(&directory, 1024).map_err(|e| e.to_string())?.as_ref() != Some(&entries)
        || list(&rebound, 1024).map_err(|e| e.to_string())?.as_ref() != Some(&entries)
    {
        return Err("segment inventory changed during absence observation".into());
    }
    root.revalidate().map_err(|e| e.to_string())?;
    Ok(absent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Segment) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260101/notes/090000_1")).unwrap();
        let segment = Segment::resolve_exact(root.path(), "20260101", "notes", "090000_1")
            .unwrap()
            .unwrap();
        (root, segment)
    }

    #[test]
    fn absence_requires_no_percept_family_or_unprocessed_video() {
        let (root, segment) = fixture();
        fs::create_dir(segment.path().join("talents")).unwrap();
        for name in ["audio.jsonl", "photo.png", "audio.wav"] {
            fs::write(segment.path().join(name), "input").unwrap();
        }
        assert_eq!(screen_inputs_absent(root.path(), &segment), Ok(true));
        for name in [
            "screen.jsonl",
            "xscreen.jsonl",
            "XSCREEN.JSONL",
            "xſcreen.JSONL",
            "browser_a.jsonl",
            "BROWSER_a.JSONL",
            "x_protected_video.jsonl",
            "x_desktop_activity.jsonl",
            "raw.MP4",
            "raw.mov",
            "raw.webm",
        ] {
            let path = segment.path().join(name);
            for bytes in ["", "malformed"] {
                fs::write(&path, bytes).unwrap();
                assert_eq!(
                    screen_inputs_absent(root.path(), &segment),
                    Ok(false),
                    "{name}"
                );
            }
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
            assert_eq!(
                screen_inputs_absent(root.path(), &segment),
                Ok(false),
                "{name}"
            );
            fs::remove_dir(path).unwrap();
        }
    }

    #[test]
    fn absence_refuses_inventory_and_path_replacement() {
        for replace in [false, true] {
            let (root, segment) = fixture();
            let result = checked_absence(root.path(), &segment, || {
                if replace {
                    fs::rename(segment.path(), segment.path().with_file_name("old")).unwrap();
                    fs::create_dir(segment.path()).unwrap();
                } else {
                    fs::write(segment.path().join("browser_new.jsonl"), "").unwrap();
                }
            });
            assert!(result.is_err());
        }
    }

    #[test]
    #[cfg(unix)]
    fn absence_refuses_aliases_and_invalid_names() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let (root, segment) = fixture();
        let path = segment.path().join("alias");
        symlink("missing", &path).unwrap();
        assert!(screen_inputs_absent(root.path(), &segment).is_err());
        fs::remove_file(path).unwrap();
        match fs::write(
            segment
                .path()
                .join(std::ffi::OsStr::from_bytes(b"invalid\xff")),
            "",
        ) {
            Ok(()) => assert!(screen_inputs_absent(root.path(), &segment).is_err()),
            // APFS stores only valid UTF-8 names and refuses this one with
            // EILSEQ, so the case cannot be set up there.
            Err(error) if cfg!(target_os = "macos") && error.raw_os_error() == Some(92) => {}
            Err(error) => panic!("{error}"),
        }
    }
}
