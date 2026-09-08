// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Path spelling comparison after signed payload admission.

use std::ffi::OsStr;
use std::path::{Component, Path, Prefix};

/// Compare ordinary/verbatim drive path components, then require equal resolved
/// canonical pathnames. This is not a native file-ID comparison or admission.
/// Callers obtain `declared` from fresh signed admission and consume that path.
pub fn matches_declared_member_path(requested: &Path, declared: &Path) -> bool {
    fn drive_components(path: &Path) -> Option<(u8, Vec<&OsStr>)> {
        let mut parts = path.components();
        let Some(Component::Prefix(prefix)) = parts.next() else {
            return None;
        };
        let drive = match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive.to_ascii_uppercase(),
            _ => return None,
        };
        if parts.next() != Some(Component::RootDir) {
            return None;
        }
        let names = parts
            .map(|part| match part {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        Some((drive, names))
    }
    let (Some((requested_drive, requested_names)), Some((declared_drive, declared_names))) =
        (drive_components(requested), drive_components(declared))
    else {
        return false;
    };
    // Matching components also refuses an outside junction/alias to this file.
    // Parse components before comparison: ordinary inventory joins can use '/'
    // where canonical Windows paths use a backslash.
    if requested_drive != declared_drive
        || requested_names.len() != declared_names.len()
        || !requested_names
            .iter()
            .zip(&declared_names)
            .all(|(requested, declared)| {
                requested == declared
                    || matches!((requested.to_str(), declared.to_str()),
                (Some(requested), Some(declared)) if requested.eq_ignore_ascii_case(declared))
            })
    {
        return false;
    }
    match (requested.canonicalize(), declared.canonicalize()) {
        (Ok(requested), Ok(declared)) => requested == declared,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_distribution::windows_payload::WINDOWS_CED_WORKER;

    #[test]
    fn ordinary_and_verbatim_member_paths_resolve_to_same_path() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("bin")).unwrap();
        let member = root.path().join(WINDOWS_CED_WORKER);
        let different = root.path().join("bin/different.exe");
        std::fs::write(&member, b"same bytes, different file").unwrap();
        std::fs::write(&different, b"same bytes, different file").unwrap();
        let verbatim = member.canonicalize().unwrap();
        let spelling = verbatim.to_str().unwrap();
        let ordinary = Path::new(spelling.strip_prefix(r"\\?\").unwrap());
        assert_ne!(ordinary.as_os_str(), verbatim.as_os_str());
        assert_eq!(ordinary.canonicalize().unwrap(), verbatim);
        assert!(matches_declared_member_path(ordinary, &verbatim));
        assert!(matches_declared_member_path(&verbatim, ordinary));
        assert!(matches_declared_member_path(&member, &verbatim));
        assert!(matches_declared_member_path(&verbatim, &member));
        let verbatim_root = root.path().canonicalize().unwrap();
        let ordinary_root = Path::new(
            verbatim_root
                .to_str()
                .unwrap()
                .strip_prefix(r"\\?\")
                .unwrap(),
        );
        let mixed_inventory_path = ordinary_root.join(WINDOWS_CED_WORKER);
        assert!(mixed_inventory_path.to_str().unwrap().contains('/'));
        assert!(matches_declared_member_path(
            &mixed_inventory_path,
            &verbatim
        ));
        assert!(matches_declared_member_path(
            &verbatim,
            &mixed_inventory_path
        ));
        assert!(!matches_declared_member_path(&different, &verbatim));
        assert!(!matches_declared_member_path(
            Path::new(r"\\.\NUL"),
            &verbatim
        ));
        assert!(!matches_declared_member_path(
            Path::new(r"\\server\share\member.bin"),
            &verbatim
        ));
        assert!(!matches_declared_member_path(
            Path::new("member.bin"),
            &verbatim
        ));
    }
}
