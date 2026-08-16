// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const KNOWN_LANES: &[&str] = &["musl-static", "zig-gnu-2.27"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub version: u32,
    pub product: String,
    pub payload: String,
    pub payload_dest_prefix: String,
    pub artifact: Artifact,
    pub target: Vec<Target>,
    pub entry: Vec<Entry>,
    pub deny: Vec<Deny>,
    #[serde(default)]
    pub cleanroom: Cleanroom,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub basename: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub os: String,
    pub arch: String,
    pub deb_arch: String,
    pub rpm_arch: String,
    pub triple_musl: String,
    pub triple_gnu: String,
    pub zig_gnu: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Entry {
    Bin {
        package: String,
        bin: String,
        dest: String,
        mode: u32,
        lane: String,
        targets: Vec<String>,
    },
    Launcher {
        source: String,
        dest: String,
        mode: u32,
        targets: Vec<String>,
    },
    ModelAsset {
        source: String,
        dest: String,
        mode: u32,
        digest_const: String,
        targets: Vec<String>,
    },
    OnnxRuntime {
        dest_dir: String,
        mode: u32,
        targets: Vec<String>,
    },
    Copy {
        source: String,
        dest: String,
        mode: u32,
        targets: Vec<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deny {
    pub bin: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cleanroom {
    #[serde(default)]
    pub subject: Vec<CleanroomSubject>,
    #[serde(default)]
    pub builder: Vec<CleanroomBuilder>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanroomSubject {
    pub id: String,
    pub image: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanroomBuilder {
    pub id: String,
    pub from_subject: String,
    pub rustc: String,
    pub zig: String,
}

#[derive(Debug)]
pub struct InventoryError {
    message: String,
}

impl InventoryError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for InventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for InventoryError {}

impl Inventory {
    #[must_use]
    pub fn required_bins(&self) -> BTreeSet<String> {
        self.entry
            .iter()
            .filter_map(|entry| match entry {
                Entry::Bin { bin, .. } => Some(bin.clone()),
                _ => None,
            })
            .collect()
    }

    #[must_use]
    pub fn forbidden_bins(&self) -> BTreeSet<String> {
        self.deny.iter().map(|deny| deny.bin.clone()).collect()
    }
}

pub fn load_inventory(path: &Path) -> Result<Inventory, InventoryError> {
    let text = fs::read_to_string(path).map_err(|error| {
        InventoryError::new(format!("read inventory {}: {error}", path.display()))
    })?;
    let inventory: Inventory = toml_edit::de::from_str(&text).map_err(|error| {
        InventoryError::new(format!("parse inventory {}: {error}", path.display()))
    })?;
    validate_inventory(path, &inventory)?;
    Ok(inventory)
}

pub fn load_payload(
    inventory_path: &Path,
    inventory: &Inventory,
) -> Result<Vec<String>, InventoryError> {
    let payload_path = inventory_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&inventory.payload);
    let text = fs::read_to_string(&payload_path).map_err(|error| {
        InventoryError::new(format!("read payload {}: {error}", payload_path.display()))
    })?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

fn validate_inventory(path: &Path, inventory: &Inventory) -> Result<(), InventoryError> {
    if inventory.version != 1 {
        return Err(InventoryError::new(format!(
            "unsupported inventory version {}; expected 1",
            inventory.version
        )));
    }
    let target_ids = inventory
        .target
        .iter()
        .map(|target| target.id.clone())
        .collect::<BTreeSet<_>>();
    if target_ids.len() != inventory.target.len() {
        return Err(InventoryError::new(
            "duplicate inventory target ids".to_owned(),
        ));
    }

    let mut dests_by_target: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut missing_targets = BTreeSet::new();
    let mut unexpected_lanes = BTreeSet::new();
    for entry in &inventory.entry {
        let (dests, targets, lane) = entry_fields(entry);
        for target in targets {
            if !target_ids.contains(target) {
                missing_targets.insert(target.to_owned());
            }
            for dest in &dests {
                if !dests_by_target
                    .entry(target.clone())
                    .or_default()
                    .insert((*dest).to_owned())
                {
                    return Err(InventoryError::new(format!(
                        "duplicate dest {dest} for target {target} in {}",
                        path.display()
                    )));
                }
            }
        }
        if let Some(lane) = lane
            && !KNOWN_LANES.contains(&lane.as_str())
        {
            unexpected_lanes.insert(lane.clone());
        }
    }
    if !missing_targets.is_empty() {
        return Err(InventoryError::new(format_named_list(
            "unexpected target",
            &missing_targets,
        )));
    }
    if !unexpected_lanes.is_empty() {
        return Err(InventoryError::new(format_named_list(
            "unexpected lane",
            &unexpected_lanes,
        )));
    }

    let payload_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&inventory.payload);
    if !payload_path.is_file() {
        return Err(InventoryError::new(format!(
            "payload list missing: {}",
            payload_path.display()
        )));
    }
    Ok(())
}

fn entry_fields(entry: &Entry) -> (Vec<&str>, &Vec<String>, Option<&String>) {
    match entry {
        Entry::Bin {
            dest,
            targets,
            lane,
            ..
        } => (vec![dest.as_str()], targets, Some(lane)),
        Entry::Launcher { dest, targets, .. }
        | Entry::ModelAsset { dest, targets, .. }
        | Entry::Copy { dest, targets, .. } => (vec![dest.as_str()], targets, None),
        Entry::OnnxRuntime {
            dest_dir, targets, ..
        } => (vec![dest_dir.as_str()], targets, None),
    }
}

pub fn format_named_list(label: &str, names: &BTreeSet<String>) -> String {
    let mut lines = vec![format!("{label}:")];
    for name in names {
        lines.push(format!("  {name}"));
    }
    lines.join("\n")
}

pub fn repository_inventory_path(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|ancestor| {
        let candidate = ancestor.join("core/distribution/inventory.toml");
        candidate.is_file().then_some(candidate)
    })
}
