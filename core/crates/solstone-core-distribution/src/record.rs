// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use crate::inventory::format_named_list;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileRecord {
    pub dest: String,
    pub kind: String,
    pub mode: u32,
    pub digest: String,
}

impl FileRecord {
    #[must_use]
    pub fn file(dest: impl Into<String>, mode: u32, digest: impl Into<String>) -> Self {
        Self {
            dest: dest.into(),
            kind: "file".to_owned(),
            mode,
            digest: digest.into(),
        }
    }

    #[must_use]
    pub fn key(&self) -> String {
        format!("{} {} {:04o} {}", self.kind, self.dest, self.mode, self.digest)
    }
}

pub fn compare_records(
    _left_label: &str,
    left: &[FileRecord],
    right_label: &str,
    right: &[FileRecord],
) -> Result<(), String> {
    let left_keys = left.iter().map(FileRecord::key).collect::<BTreeSet<_>>();
    let right_keys = right.iter().map(FileRecord::key).collect::<BTreeSet<_>>();
    let missing = left_keys
        .difference(&right_keys)
        .cloned()
        .collect::<BTreeSet<_>>();
    let unexpected = right_keys
        .difference(&left_keys)
        .cloned()
        .collect::<BTreeSet<_>>();
    if missing.is_empty() && unexpected.is_empty() {
        return Ok(());
    }
    let mut sections = Vec::new();
    if !missing.is_empty() {
        sections.push(format_named_list(
            &format!("missing in {right_label}"),
            &missing,
        ));
    }
    if !unexpected.is_empty() {
        sections.push(format_named_list(
            &format!("unexpected in {right_label}"),
            &unexpected,
        ));
    }
    Err(sections.join("\n"))
}
