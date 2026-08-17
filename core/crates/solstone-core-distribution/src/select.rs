// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::inventory::{Entry, Inventory, format_named_list};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArtifactId {
    pub package: String,
    pub bin: String,
    pub triple: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedBin {
    pub dest: String,
    pub bin: String,
    pub package: String,
    pub triple: String,
    pub path: PathBuf,
    pub mode: u32,
    /// The lane that actually built this binary — the target's lane on macOS,
    /// the entry's lane on Linux. `produce` keys its binary inspection policy
    /// off this, so it must be the resolved value and never the declared one.
    pub lane: String,
    pub os: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub admitted: BTreeSet<String>,
    pub bins: Vec<SelectedBin>,
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

/// Inventory-driven selection. Never reads a Cargo output directory.
pub fn select_artifacts(
    inventory: &Inventory,
    target_id: &str,
    artifacts: &BTreeMap<ArtifactId, PathBuf>,
) -> Result<Selection, SelectError> {
    let target = inventory
        .target
        .iter()
        .find(|target| target.id == target_id);
    let Some(target) = target else {
        return Err(SelectError {
            missing_required: inventory.required_bins(),
            admitted_forbidden: BTreeSet::new(),
        });
    };

    let forbidden = inventory.forbidden_bins();
    let mut admitted_forbidden = BTreeSet::new();
    for id in artifacts.keys() {
        if forbidden.contains(&id.bin) {
            admitted_forbidden.insert(id.bin.clone());
        }
    }

    let mut admitted = BTreeSet::new();
    let mut bins = Vec::new();
    let mut missing_required = BTreeSet::new();
    for entry in &inventory.entry {
        let Entry::Bin {
            package,
            bin,
            dest,
            mode,
            lane,
            targets,
        } = entry
        else {
            continue;
        };
        if !targets.iter().any(|item| item == target_id) {
            continue;
        }
        let lane = target.lane_for(lane).to_owned();
        let triple = target.triple_for_lane(&lane);
        let id = ArtifactId {
            package: package.clone(),
            bin: bin.clone(),
            triple: triple.to_owned(),
        };
        match artifacts.get(&id) {
            Some(path) if path.is_file() => {
                admitted.insert(bin.clone());
                bins.push(SelectedBin {
                    dest: dest.clone(),
                    bin: bin.clone(),
                    package: package.clone(),
                    triple: triple.to_owned(),
                    path: path.clone(),
                    mode: *mode,
                    lane: lane.clone(),
                    os: target.os.clone(),
                });
            }
            _ => {
                missing_required.insert(bin.clone());
            }
        }
    }

    if !missing_required.is_empty() || !admitted_forbidden.is_empty() {
        return Err(SelectError {
            missing_required,
            admitted_forbidden,
        });
    }
    Ok(Selection { admitted, bins })
}

pub fn refuse_extra(
    inventory: &Inventory,
    target_id: &str,
    artifacts: &BTreeMap<ArtifactId, PathBuf>,
) -> Result<(), SelectError> {
    let required = inventory
        .entry
        .iter()
        .filter_map(|entry| match entry {
            Entry::Bin {
                package,
                bin,
                targets,
                ..
            } if targets.iter().any(|item| item == target_id) => {
                Some((package.as_str(), bin.as_str()))
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let extra = artifacts
        .keys()
        .filter(|id| !required.contains(&(id.package.as_str(), id.bin.as_str())))
        .map(|id| format!("{} {}", id.package, id.bin))
        .collect::<BTreeSet<_>>();
    if extra.is_empty() {
        return Ok(());
    }
    Err(SelectError {
        missing_required: BTreeSet::new(),
        admitted_forbidden: extra,
    })
}

pub fn refuse_wrong_triple(
    inventory: &Inventory,
    target_id: &str,
    artifacts: &BTreeMap<ArtifactId, PathBuf>,
) -> Result<(), SelectError> {
    let Some(target) = inventory.target.iter().find(|item| item.id == target_id) else {
        return Ok(());
    };
    let allowed = target.triples();
    let unexpected = artifacts
        .keys()
        .filter(|id| !allowed.contains(&id.triple.as_str()))
        .map(|id| format!("{} {} {}", id.package, id.bin, id.triple))
        .collect::<BTreeSet<_>>();
    if unexpected.is_empty() {
        return Ok(());
    }
    Err(SelectError {
        missing_required: BTreeSet::new(),
        admitted_forbidden: unexpected,
    })
}

pub fn stage_selected(selection: &Selection, stage: &Path) -> std::io::Result<()> {
    for bin in &selection.bins {
        let bytes = std::fs::read(&bin.path)?;
        crate::stage::write_staged_file_mode(stage, &bin.dest, &bytes, bin.mode)?;
    }
    Ok(())
}
