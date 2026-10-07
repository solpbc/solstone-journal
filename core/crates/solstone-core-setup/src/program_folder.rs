// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::{Path, PathBuf};

use solstone_core_installation_identity::{IdentityError, PROGRAM_FOLDER_JOURNAL_REFUSAL};

/// Refuses a journal path if it is located inside the Velopack program folder
/// of the specified executable or executable directory.
pub fn refuse_journal_in_program_folder(
    journal: &Path,
    executable_or_dir: &Path,
) -> Result<(), IdentityError> {
    let Some(root) = velopack_program_root(executable_or_dir)? else {
        return Ok(());
    };

    if lexical_overlap(journal, &root) {
        return Err(IdentityError::AdmissionRefused(
            PROGRAM_FOLDER_JOURNAL_REFUSAL,
        ));
    }

    let resolved_journal = resolve_nearest_ancestor(journal)?;
    let resolved_root = resolve_nearest_ancestor(&root)?;

    if lexical_overlap(&resolved_journal, &resolved_root) {
        return Err(IdentityError::AdmissionRefused(
            PROGRAM_FOLDER_JOURNAL_REFUSAL,
        ));
    }

    Ok(())
}

fn velopack_program_root(executable_or_dir: &Path) -> Result<Option<PathBuf>, IdentityError> {
    let exe_dir = if executable_or_dir.is_dir() {
        executable_or_dir.to_path_buf()
    } else {
        match executable_or_dir.parent() {
            Some(parent) => parent.to_path_buf(),
            None => PathBuf::from("."),
        }
    };

    if is_update_exe_file(&exe_dir.join("Update.exe"))? {
        return Ok(Some(exe_dir));
    }

    if let Some(candidate) = find_current_ancestor_candidate(executable_or_dir)
        && is_update_exe_file(&candidate.join("Update.exe"))?
    {
        return Ok(Some(candidate));
    }

    Ok(None)
}

fn is_update_exe_file(path: &Path) -> Result<bool, IdentityError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(IdentityError::AdmissionRefused(
            PROGRAM_FOLDER_JOURNAL_REFUSAL,
        )),
    }
}

fn find_current_ancestor_candidate(path: &Path) -> Option<PathBuf> {
    let norm = parse_normalized(path);
    let rightmost_idx = norm
        .components
        .iter()
        .rposition(|comp| comp.eq_ignore_ascii_case("current"))?;

    let ancestor_components = &norm.components[..rightmost_idx];
    Some(norm.flavor.format_path(ancestor_components))
}

fn resolve_nearest_ancestor(path: &Path) -> Result<PathBuf, IdentityError> {
    let mut current = path.to_path_buf();
    let mut trailing = Vec::new();
    loop {
        match std::fs::symlink_metadata(&current) {
            Ok(_) => {
                let canonical = std::fs::canonicalize(&current)
                    .map_err(|_| IdentityError::AdmissionRefused(PROGRAM_FOLDER_JOURNAL_REFUSAL))?;
                let mut resolved = canonical;
                for component in trailing.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file_name = current.file_name().map(|n| n.to_os_string());
                let parent = current.parent().map(|p| p.to_path_buf());
                match (parent, file_name) {
                    (Some(parent), Some(name)) => {
                        trailing.push(name);
                        current = parent;
                    }
                    _ => {
                        return Err(IdentityError::AdmissionRefused(
                            PROGRAM_FOLDER_JOURNAL_REFUSAL,
                        ));
                    }
                }
            }
            Err(_) => {
                return Err(IdentityError::AdmissionRefused(
                    PROGRAM_FOLDER_JOURNAL_REFUSAL,
                ));
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PathFlavor {
    WindowsDrive(char),
    WindowsUnc(String, String),
    WindowsRelative,
    PosixAbsolute,
    PosixRelative,
}

impl PathFlavor {
    fn format_path(&self, components: &[String]) -> PathBuf {
        match self {
            Self::WindowsDrive(d) => {
                if components.is_empty() {
                    PathBuf::from(format!("{d}:\\"))
                } else {
                    PathBuf::from(format!("{d}:\\{}", components.join("\\")))
                }
            }
            Self::WindowsUnc(server, share) => {
                if components.is_empty() {
                    PathBuf::from(format!("\\\\{server}\\{share}"))
                } else {
                    PathBuf::from(format!("\\\\{server}\\{share}\\{}", components.join("\\")))
                }
            }
            Self::WindowsRelative => {
                if components.is_empty() {
                    PathBuf::from(".")
                } else {
                    PathBuf::from(components.join("\\"))
                }
            }
            Self::PosixAbsolute => {
                if components.is_empty() {
                    PathBuf::from("/")
                } else {
                    PathBuf::from(format!("/{}", components.join("/")))
                }
            }
            Self::PosixRelative => {
                if components.is_empty() {
                    PathBuf::from(".")
                } else {
                    PathBuf::from(components.join("/"))
                }
            }
        }
    }
}

struct NormalizedPath {
    flavor: PathFlavor,
    components: Vec<String>,
}

fn parse_normalized(path: &Path) -> NormalizedPath {
    let s = path.to_string_lossy();
    let mut raw = s.as_ref();

    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        let folded = rest.replace('/', "\\");
        let mut parts = folded.split('\\').filter(|p| !p.is_empty());
        let server = parts.next().unwrap_or("").to_string();
        let share = parts.next().unwrap_or("").to_string();
        let mut components = Vec::new();
        for part in parts {
            if part == "." {
                continue;
            } else if part == ".." {
                components.pop();
            } else {
                components.push(part.to_string());
            }
        }
        return NormalizedPath {
            flavor: PathFlavor::WindowsUnc(server, share),
            components,
        };
    }

    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        raw = rest;
    }

    let folded = raw.replace('/', "\\");

    if let Some(rest) = folded.strip_prefix(r"\\") {
        let mut parts = rest.split('\\').filter(|p| !p.is_empty());
        let server = parts.next().unwrap_or("").to_string();
        let share = parts.next().unwrap_or("").to_string();
        let mut components = Vec::new();
        for part in parts {
            if part == "." {
                continue;
            } else if part == ".." {
                components.pop();
            } else {
                components.push(part.to_string());
            }
        }
        return NormalizedPath {
            flavor: PathFlavor::WindowsUnc(server, share),
            components,
        };
    }

    let bytes = folded.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_uppercase();
        let rest = &folded[2..];
        let mut components = Vec::new();
        for part in rest.split('\\').filter(|p| !p.is_empty()) {
            if part == "." {
                continue;
            } else if part == ".." {
                components.pop();
            } else {
                components.push(part.to_string());
            }
        }
        return NormalizedPath {
            flavor: PathFlavor::WindowsDrive(drive),
            components,
        };
    }

    let is_posix_abs = raw.starts_with('/');
    let mut components = Vec::new();
    for part in raw.split(['/', '\\']).filter(|p| !p.is_empty()) {
        if part == "." {
            continue;
        } else if part == ".." {
            components.pop();
        } else {
            components.push(part.to_string());
        }
    }

    let flavor = if is_posix_abs {
        PathFlavor::PosixAbsolute
    } else if cfg!(windows) {
        PathFlavor::WindowsRelative
    } else {
        PathFlavor::PosixRelative
    };

    NormalizedPath { flavor, components }
}

pub(crate) fn lexical_overlap(journal: &Path, root: &Path) -> bool {
    let norm_journal = parse_normalized(journal);
    let norm_root = parse_normalized(root);

    match (&norm_journal.flavor, &norm_root.flavor) {
        (PathFlavor::WindowsDrive(d1), PathFlavor::WindowsDrive(d2)) => {
            if d1 != d2 {
                return false;
            }
            has_containment(&norm_journal.components, &norm_root.components, false)
        }
        (PathFlavor::WindowsUnc(s1, sh1), PathFlavor::WindowsUnc(s2, sh2)) => {
            if !s1.eq_ignore_ascii_case(s2) || !sh1.eq_ignore_ascii_case(sh2) {
                return false;
            }
            has_containment(&norm_journal.components, &norm_root.components, false)
        }
        (PathFlavor::PosixAbsolute, PathFlavor::PosixAbsolute) => {
            has_containment(&norm_journal.components, &norm_root.components, true)
        }
        (PathFlavor::PosixRelative, PathFlavor::PosixRelative) => {
            has_containment(&norm_journal.components, &norm_root.components, true)
        }
        (PathFlavor::WindowsRelative, PathFlavor::WindowsRelative) => {
            has_containment(&norm_journal.components, &norm_root.components, false)
        }
        _ => false,
    }
}

fn has_containment(a: &[String], b: &[String], case_sensitive: bool) -> bool {
    is_prefix_of(a, b, case_sensitive) || is_prefix_of(b, a, case_sensitive)
}

fn is_prefix_of(prefix: &[String], full: &[String], case_sensitive: bool) -> bool {
    if prefix.len() > full.len() {
        return false;
    }
    for (p, f) in prefix.iter().zip(full.iter()) {
        if case_sensitive {
            if p != f {
                return false;
            }
        } else if !p.eq_ignore_ascii_case(f) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    fn test_dir(label: &str) -> PathBuf {
        let path = PathBuf::from("/var/tmp").join(format!(
            "program-folder-{}-{}-{}",
            label,
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test directory");
        path
    }

    #[test]
    fn program_folder_lexical_containment_both_directions_and_parent() {
        assert!(lexical_overlap(
            Path::new(r"C:\App\SolstoneJournal\journal"),
            Path::new(r"C:\App\SolstoneJournal")
        ));
        assert!(lexical_overlap(
            Path::new(r"C:\App"),
            Path::new(r"C:\App\SolstoneJournal")
        ));
        assert!(lexical_overlap(
            Path::new("/var/tmp/app/journal"),
            Path::new("/var/tmp/app")
        ));
        assert!(lexical_overlap(
            Path::new("/var/tmp/app"),
            Path::new("/var/tmp/app/journal")
        ));
    }

    #[test]
    fn program_folder_lexical_windows_case_and_verbatim() {
        assert!(lexical_overlap(
            Path::new(r"\\?\c:\app\solstonejournal\journal"),
            Path::new(r"C:\APP\SolstoneJournal")
        ));
        assert!(lexical_overlap(
            Path::new(r"\\?\UNC\server\share\app\journal"),
            Path::new(r"\\SERVER\SHARE\app")
        ));
    }

    #[test]
    fn program_folder_lexical_dot_segments_collapse() {
        assert!(lexical_overlap(
            Path::new(r"C:\App\SolstoneJournal\sub\..\journal"),
            Path::new(r"C:\App\SolstoneJournal")
        ));
        assert!(lexical_overlap(
            Path::new(r"C:\App\.\SolstoneJournal\journal"),
            Path::new(r"C:\App\SolstoneJournal\.")
        ));
    }

    #[test]
    fn program_folder_lexical_sibling_allowed() {
        assert!(!lexical_overlap(
            Path::new(r"C:\App\SolstoneJournalBackup"),
            Path::new(r"C:\App\SolstoneJournal")
        ));
        assert!(!lexical_overlap(
            Path::new("/var/tmp/SolstoneJournalBackup"),
            Path::new("/var/tmp/SolstoneJournal")
        ));
    }

    #[test]
    fn program_folder_lexical_app_state_webview_allowed() {
        assert!(!lexical_overlap(
            Path::new(r"C:\Users\User\AppData\Local\solstone-journal\journal-app-webview"),
            Path::new(r"C:\Users\User\AppData\Local\SolstoneJournal")
        ));
    }

    #[test]
    fn program_folder_layout_no_update_exe_allows_journal() {
        let dir = test_dir("no-update");
        let versions = dir.join("versions");
        let current = dir.join("current");
        let bin = current.join("bin");
        fs::create_dir_all(&versions).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("solstone");
        fs::write(&exe, b"").unwrap();

        assert_eq!(velopack_program_root(&exe).unwrap(), None);
        assert!(refuse_journal_in_program_folder(&dir.join("journal"), &exe).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn program_folder_layout_update_exe_in_parent() {
        let dir = test_dir("direct-parent");
        let exe = dir.join("solstone.exe");
        let update = dir.join("Update.exe");
        fs::write(&exe, b"").unwrap();
        fs::write(&update, b"").unwrap();

        assert_eq!(velopack_program_root(&exe).unwrap(), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn program_folder_layout_current_bin_nested() {
        let dir = test_dir("nested-current");
        let update = dir.join("Update.exe");
        let bin = dir.join("current").join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("solstone.exe");
        fs::write(&update, b"").unwrap();
        fs::write(&exe, b"").unwrap();

        assert_eq!(velopack_program_root(&exe).unwrap(), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn program_folder_layout_current_component_case_insensitive() {
        let dir = test_dir("case-current");
        let update = dir.join("Update.exe");
        let bin = dir.join("Current").join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("solstone.exe");
        fs::write(&update, b"").unwrap();
        fs::write(&exe, b"").unwrap();

        assert_eq!(velopack_program_root(&exe).unwrap(), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn program_folder_symlink_into_root_refuses() {
        let dir = test_dir("symlink-into-root");
        let root = dir.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Update.exe"), b"").unwrap();
        let bin = root.join("current").join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("solstone");
        fs::write(&exe, b"").unwrap();

        let inner_journal = root.join("inner_journal");
        fs::create_dir_all(&inner_journal).unwrap();
        let outer_symlink = dir.join("outer_symlink");
        std::os::unix::fs::symlink(&inner_journal, &outer_symlink).unwrap();

        let result = refuse_journal_in_program_folder(&outer_symlink, &exe);
        assert!(matches!(
            result,
            Err(IdentityError::AdmissionRefused(msg)) if msg == PROGRAM_FOLDER_JOURNAL_REFUSAL
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn program_folder_missing_leaf_under_alias_refuses() {
        let dir = test_dir("missing-leaf");
        let root = dir.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Update.exe"), b"").unwrap();
        let bin = root.join("current").join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("solstone");
        fs::write(&exe, b"").unwrap();

        let inner_dir = root.join("inner_dir");
        fs::create_dir_all(&inner_dir).unwrap();
        let outer_symlink = dir.join("outer_symlink");
        std::os::unix::fs::symlink(&inner_dir, &outer_symlink).unwrap();

        let missing_journal = outer_symlink.join("uncreated_child");
        let result = refuse_journal_in_program_folder(&missing_journal, &exe);
        assert!(matches!(
            result,
            Err(IdentityError::AdmissionRefused(msg)) if msg == PROGRAM_FOLDER_JOURNAL_REFUSAL
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn program_folder_unresolvable_alias_refuses() {
        let dir = test_dir("unresolvable-alias");
        let root = dir.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Update.exe"), b"").unwrap();
        let bin = root.join("current").join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("solstone");
        fs::write(&exe, b"").unwrap();

        let loop_a = dir.join("loop_a");
        let loop_b = dir.join("loop_b");
        std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
        std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();

        let result = refuse_journal_in_program_folder(&loop_a, &exe);
        assert!(matches!(
            result,
            Err(IdentityError::AdmissionRefused(msg)) if msg == PROGRAM_FOLDER_JOURNAL_REFUSAL
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
