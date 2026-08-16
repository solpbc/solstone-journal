// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Produce-time refusal for an archive entry that would escape its prefix.
///
/// Install-time `install.sh` must use this same named set. A repository
/// contract in commit #7 asserts the shell scanner names match these
/// variants exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArchiveEscape {
    AbsolutePath,
    ParentTraversal,
    SymlinkEscape,
    HardlinkEscape,
    SymlinkThenChild,
}

impl ArchiveEscape {
    pub const ALL: [Self; 5] = [
        Self::AbsolutePath,
        Self::ParentTraversal,
        Self::SymlinkEscape,
        Self::HardlinkEscape,
        Self::SymlinkThenChild,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AbsolutePath => "archive-absolute-path",
            Self::ParentTraversal => "archive-parent-traversal",
            Self::SymlinkEscape => "archive-symlink-escape",
            Self::HardlinkEscape => "archive-hardlink-escape",
            Self::SymlinkThenChild => "archive-symlink-then-child",
        }
    }
}

pub fn refuse_escape(dest: &str) -> Result<(), ArchiveEscape> {
    if dest.starts_with('/') {
        return Err(ArchiveEscape::AbsolutePath);
    }
    for component in dest.split('/') {
        if component == ".." {
            return Err(ArchiveEscape::ParentTraversal);
        }
    }
    Ok(())
}

pub fn refuse_link_target(
    kind: ArchiveEscape,
    dest: &str,
    target: &str,
) -> Result<(), ArchiveEscape> {
    refuse_escape(dest)?;
    refuse_escape(target).map_err(|_| kind)?;
    if target.starts_with('/') {
        return Err(kind);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ArchiveEscape;

    #[test]
    fn archive_escape_names_are_stable() {
        let names = ArchiveEscape::ALL
            .into_iter()
            .map(ArchiveEscape::as_str)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "archive-absolute-path",
                "archive-parent-traversal",
                "archive-symlink-escape",
                "archive-hardlink-escape",
                "archive-symlink-then-child",
            ]
        );
    }
}
