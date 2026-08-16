// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::Path;

use crate::inventory::{Inventory, format_named_list};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub admitted: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectError {
    pub missing_required: BTreeSet<String>,
    pub admitted_forbidden: BTreeSet<String>,
}

impl fmt::Display for SelectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sections = Vec::new();
        if !self.missing_required.is_empty() {
            sections.push(format_named_list(
                "missing required",
                &self.missing_required,
            ));
        }
        if !self.admitted_forbidden.is_empty() {
            sections.push(format_named_list(
                "admitted forbidden",
                &self.admitted_forbidden,
            ));
        }
        formatter.write_str(&sections.join("\n"))
    }
}

impl std::error::Error for SelectError {}

/// Naive directory listing. Commit #4 replaces this with inventory-driven
/// package+bin+triple selection.
pub fn select_from_directory(
    inventory: &Inventory,
    output_dir: &Path,
) -> Result<Selection, SelectError> {
    let mut present = BTreeSet::new();
    let entries = fs::read_dir(output_dir).map_err(|_| SelectError {
        missing_required: inventory.required_bins(),
        admitted_forbidden: BTreeSet::new(),
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            present.insert(name.to_owned());
        }
    }

    let required = inventory.required_bins();
    let forbidden = inventory.forbidden_bins();
    let missing_required = required
        .difference(&present)
        .cloned()
        .collect::<BTreeSet<_>>();
    let admitted_forbidden = present
        .intersection(&forbidden)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !missing_required.is_empty() || !admitted_forbidden.is_empty() {
        return Err(SelectError {
            missing_required,
            admitted_forbidden,
        });
    }
    Ok(Selection { admitted: present })
}
