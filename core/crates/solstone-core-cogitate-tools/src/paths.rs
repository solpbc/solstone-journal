// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use solstone_core_format::paths::resolve_journal_path;

#[derive(Debug)]
pub(crate) enum ContainedPathError {
    Invalid,
    Escape,
    Io,
}

pub(crate) fn journal_root_real(root: &Path) -> Result<PathBuf, io::Error> {
    realpath_non_strict(root)
}

pub(crate) fn contained_path(root: &Path, rel: &str) -> Result<PathBuf, ContainedPathError> {
    if root_alias(rel) {
        return journal_root_real(root).map_err(|_| ContainedPathError::Io);
    }
    let normalized = pathlib_normalize(rel);
    let lexical =
        resolve_journal_path(root, &normalized).map_err(|_| ContainedPathError::Invalid)?;
    let root_real = journal_root_real(root).map_err(|_| ContainedPathError::Io)?;
    let candidate = realpath_non_strict(&lexical).map_err(|_| ContainedPathError::Io)?;
    if candidate.starts_with(&root_real) {
        Ok(candidate)
    } else {
        Err(ContainedPathError::Escape)
    }
}

pub(crate) fn resolve_target(root: &Path, rel: &str) -> Result<PathBuf, ContainedPathError> {
    contained_path(root, rel)
}

/// Normalise a journal-relative path the way `pathlib.Path(rel).parts` does.
///
/// The reference validates `Path(rel).parts`, and pathlib drops trailing
/// slashes, repeated separators and leading `./` before the guard ever runs --
/// so `chronicle/` reaches it as `("chronicle",)` and is accepted. Splitting the
/// string literally instead sees an empty final component and refuses with
/// bad_path, which is a different refusal carrying different repair guidance.
///
/// This matters against a live model, not just in theory: a real provider asked
/// for `chronicle/` on the first tool call, as models routinely do for
/// directories. `..` is deliberately preserved so the guard still rejects it.
fn pathlib_normalize(rel: &str) -> String {
    if rel.contains('\\') || Path::new(rel).is_absolute() {
        return rel.to_owned();
    }
    let parts: Vec<&str> = rel
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        return rel.to_owned();
    }
    parts.join("/")
}

fn root_alias(rel: &str) -> bool {
    !Path::new(rel).is_absolute() && rel.split('/').all(|part| part.is_empty() || part == ".")
}

/// Resolve symlinks while retaining missing trailing components, like `os.path.realpath`.
pub(crate) fn realpath_non_strict(path: &Path) -> Result<PathBuf, io::Error> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut pending: VecDeque<OsString> = absolute
        .components()
        .map(|part| part.as_os_str().to_os_string())
        .collect();
    let mut resolved = PathBuf::new();
    let mut links = 0usize;
    while let Some(part) = pending.pop_front() {
        if part == OsStr::new("/") {
            resolved = PathBuf::from("/");
            continue;
        }
        if part == OsStr::new(".") {
            continue;
        }
        if part == OsStr::new("..") {
            resolved.pop();
            continue;
        }
        let candidate = resolved.join(&part);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                links += 1;
                if links > 40 {
                    return Err(io::Error::other("too many symlinks"));
                }
                let target = fs::read_link(&candidate)?;
                if target.is_absolute() {
                    resolved = PathBuf::new();
                }
                let target_parts: Vec<OsString> = target
                    .components()
                    .map(|component| component.as_os_str().to_os_string())
                    .collect();
                for component in target_parts.into_iter().rev() {
                    pending.push_front(component);
                }
            }
            Ok(_) => resolved.push(part),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                resolved.push(part);
                while let Some(rest) = pending.pop_front() {
                    if rest == OsStr::new(".") {
                        continue;
                    }
                    if rest == OsStr::new("..") {
                        resolved.pop();
                    } else if rest == OsStr::new("/") {
                        resolved = PathBuf::from("/");
                    } else {
                        resolved.push(rest);
                    }
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(resolved)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn containment_keeps_missing_inside_and_rejects_escape_symlinks() {
        let base = tempfile::tempdir().expect("tempdir");
        let root = base.path().join("journal");
        let outside = base.path().join("outside");
        std::fs::create_dir_all(root.join("facets")).expect("root");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(root.join("facets/work.md"), "work").expect("work");
        std::fs::write(outside.join("leak.txt"), "leak").expect("leak");
        symlink(root.join("facets/work.md"), root.join("inside_link")).expect("inside link");
        symlink(outside.join("leak.txt"), root.join("escape")).expect("escape link");
        let root_real = journal_root_real(&root).expect("real root");

        assert_eq!(
            contained_path(&root, "inside_link").expect("inside"),
            root_real.join("facets/work.md")
        );
        assert_eq!(
            contained_path(&root, "missing.md").expect("missing"),
            root_real.join("missing.md")
        );
        assert!(matches!(
            contained_path(&root, "escape"),
            Err(ContainedPathError::Escape)
        ));
    }
}
