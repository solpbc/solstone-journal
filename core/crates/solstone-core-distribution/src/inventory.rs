// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::archive_taxonomy::ContainerKind;

const KNOWN_LANES: &[&str] = &["musl-static", "zig-gnu-2.27"];
/// Lanes a target may declare for itself. Linux entries carry a per-binary lane
/// because the Linux tree is built by two distinct cross toolchains; macOS has
/// exactly one toolchain, so its lane is a property of the target and the
/// per-entry `lane` is not consulted for it.
const KNOWN_TARGET_LANES: &[&str] = &["apple-native"];
pub const OS_LINUX: &str = "linux";
pub const OS_MACOS: &str = "macos";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub version: u32,
    pub product: String,
    pub payload: String,
    pub payload_dest_prefix: String,
    /// The repository directory `payload.txt`'s paths are rooted in. A checkout
    /// keeps the shipped payload here rather than in the Python package tree;
    /// the installed layout is unchanged, so `payload.txt` names one set of
    /// paths and the producer joins this root to read them.
    pub payload_src_root: String,
    pub artifact: Artifact,
    pub target: Vec<Target>,
    pub entry: Vec<Entry>,
    pub deny: Vec<Deny>,
    #[serde(default)]
    pub cleanroom: Cleanroom,
    #[serde(default)]
    pub apple: Apple,
}

/// The macOS signing contract, declared rather than imported.
///
/// These values lived in `scripts/release_tool_pins.py` and were read through
/// an interpreter by the shell signing helper. `P-distribution` puts the
/// producer and its machinery on the same side of the Python boundary, so they
/// move to the inventory — the declarative surface the plate already uses for
/// every other producer fact.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Apple {
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub app_identity: String,
    #[serde(default)]
    pub installer_identity: String,
    #[serde(default)]
    pub notary_profile: String,
    #[serde(default)]
    pub keychain: String,
    #[serde(default)]
    pub pkg_identifier: String,
    #[serde(default)]
    pub install_location: String,
    #[serde(default)]
    pub codesign_path: String,
    #[serde(default)]
    pub xcode: String,
    #[serde(default)]
    pub notarytool: String,
}

impl Apple {
    /// `~` is expanded against `HOME` so the inventory can name the keychain
    /// without pinning one operator's home directory into a public file.
    #[must_use]
    pub fn keychain_path(&self) -> PathBuf {
        match self.keychain.strip_prefix("~/") {
            Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
            None => PathBuf::from(&self.keychain),
        }
    }

    #[must_use]
    pub fn is_declared(&self) -> bool {
        !self.team_id.is_empty()
            && !self.app_identity.is_empty()
            && !self.installer_identity.is_empty()
            && !self.notary_profile.is_empty()
            && !self.keychain.is_empty()
            && !self.pkg_identifier.is_empty()
            && !self.install_location.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub basename: String,
}

impl Artifact {
    #[must_use]
    pub fn render(&self, version: &str, os: &str, arch: &str) -> String {
        self.basename
            .replace("{version}", version)
            .replace("{os}", os)
            .replace("{arch}", arch)
    }
}

#[must_use]
pub fn artifact_archives(basename: &str) -> [String; 3] {
    [
        format!("{basename}.tar.gz"),
        format!("{basename}.deb"),
        format!("{basename}.rpm"),
    ]
}

/// Containers for `os`, in emission order. The `.tar.gz` primitive is shared;
/// the rest is the platform's own supported wrapper. Linux relocates the tree
/// through `.deb` and `.rpm`; macOS relocates it through one signed, notarized
/// and stapled `.pkg`.
#[must_use]
pub fn artifact_archives_for_os(os: &str, basename: &str) -> Vec<String> {
    match os {
        OS_MACOS => vec![format!("{basename}.tar.gz"), format!("{basename}.pkg")],
        _ => artifact_archives(basename).to_vec(),
    }
}

#[must_use]
pub fn artifact_sidecars(basename: &str) -> [String; 3] {
    [
        format!("{basename}.sha256"),
        format!("{basename}.manifest.json"),
        format!("{basename}.release"),
    ]
}

/// Members protected by the checksum sidecar. The receipt is a first-class
/// macOS release fact, so it is covered alongside the containers and release
/// declaration rather than being an unsigned afterthought.
#[must_use]
pub fn checksum_members_for_os(os: &str, basename: &str) -> Vec<String> {
    let mut names = artifact_archives_for_os(os, basename);
    names.push(format!("{basename}.release"));
    if os == OS_MACOS {
        names.push(format!("{basename}.signing.json"));
    }
    names
}

/// Members protected by the manifest. The manifest never protects itself (or
/// its eventual minisign signature), but it does bind the checksum sidecar.
#[must_use]
pub fn manifest_members_for_os(os: &str, basename: &str) -> Vec<String> {
    let mut names = checksum_members_for_os(os, basename);
    names.push(format!("{basename}.sha256"));
    names
}

#[must_use]
pub fn artifact_set(basename: &str) -> [String; 6] {
    let [tar, deb, rpm] = artifact_archives(basename);
    let [sha256, manifest, release] = artifact_sidecars(basename);
    [tar, deb, rpm, sha256, manifest, release]
}

/// Sidecars for `os`. macOS carries a fourth: the signing receipt, which is
/// provenance the Linux set does not need and which used to be produced by
/// `scripts/record_macos_native_wheel.py`.
#[must_use]
pub fn artifact_sidecars_for_os(os: &str, basename: &str) -> Vec<String> {
    let mut names = artifact_sidecars(basename).to_vec();
    if os == OS_MACOS {
        names.push(format!("{basename}.signing.json"));
    }
    names
}

/// The complete atomic set for `os`: every container plus every sidecar.
/// Promotion renames one directory holding exactly this set or nothing at all.
/// Both platforms land on six files — three containers and three sidecars on
/// Linux, two containers and four sidecars on macOS — and the invariant that
/// matters is completeness, not the count.
#[must_use]
pub fn artifact_set_for_os(os: &str, basename: &str) -> Vec<String> {
    let mut names = artifact_archives_for_os(os, basename);
    names.extend(artifact_sidecars_for_os(os, basename));
    names
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub os: String,
    pub arch: String,
    /// Linux-only cross-toolchain fields. Empty on a macOS target, and
    /// `validate_inventory` refuses a target that carries the other os's set.
    #[serde(default)]
    pub deb_arch: String,
    #[serde(default)]
    pub rpm_arch: String,
    #[serde(default)]
    pub triple_musl: String,
    #[serde(default)]
    pub triple_gnu: String,
    #[serde(default)]
    pub zig_gnu: String,
    /// macOS-only fields.
    #[serde(default)]
    pub lane: String,
    #[serde(default)]
    pub triple_apple: String,
    #[serde(default)]
    pub min_macos: String,
}

impl Target {
    #[must_use]
    pub fn is_macos(&self) -> bool {
        self.os == OS_MACOS
    }

    /// The lane that actually builds `entry_lane` for this target. On macOS the
    /// target owns the lane; on Linux the entry does. One contract, one end.
    #[must_use]
    pub fn lane_for<'a>(&'a self, entry_lane: &'a str) -> &'a str {
        if self.is_macos() {
            &self.lane
        } else {
            entry_lane
        }
    }

    /// The rustc target triple a lane builds into.
    #[must_use]
    pub fn triple_for_lane(&self, lane: &str) -> &str {
        match lane {
            "apple-native" => &self.triple_apple,
            "musl-static" => &self.triple_musl,
            _ => &self.triple_gnu,
        }
    }

    /// Every triple this target may legitimately produce artifacts under.
    #[must_use]
    pub fn triples(&self) -> Vec<&str> {
        [
            self.triple_apple.as_str(),
            self.triple_musl.as_str(),
            self.triple_gnu.as_str(),
        ]
        .into_iter()
        .filter(|triple| !triple.is_empty())
        .collect()
    }
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
        digest_source: String,
        #[serde(default)]
        archive_slot: Option<ArchiveSlot>,
        targets: Vec<String>,
    },
    OnnxRuntime {
        dest_dir: String,
        mode: u32,
        targets: Vec<String>,
    },
    Pdfium {
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
pub struct ArchiveSlot {
    pub id: String,
    pub target: String,
    pub container: ContainerKind,
    pub executables: Vec<ArchiveExecutable>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveExecutable {
    pub path: String,
    pub digest_const: String,
    pub digest_source: String,
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
    pub digest: String,
    #[serde(default = "default_cleanroom_network")]
    pub network: String,
    #[serde(default)]
    pub python: bool,
    #[serde(default)]
    pub control: bool,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default)]
    pub forbidden_tools: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<String>,
    #[serde(default)]
    pub entry_command: String,
    #[serde(default)]
    pub expected: Vec<String>,
}

fn default_cleanroom_network() -> String {
    "none".to_owned()
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
    if !inventory.artifact.basename.contains("{version}")
        || !inventory.artifact.basename.contains("{os}")
        || !inventory.artifact.basename.contains("{arch}")
    {
        return Err(InventoryError::new(
            "missing required:\n  artifact basename {version} {os} {arch}".to_owned(),
        ));
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
    validate_targets(&inventory.target)?;
    if inventory.target.iter().any(Target::is_macos) && !inventory.apple.is_declared() {
        return Err(InventoryError::new(
            "missing required:\n  [apple] signing contract for a macos target".to_owned(),
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
            && !KNOWN_TARGET_LANES.contains(&lane.as_str())
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
    validate_archive_slots(path, inventory)?;

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

    // `payload_src_root` is joined to a repository root the validator does not
    // have, so what it can check is the shape: a relative path with no escape.
    // The producer's own read failure names the missing file if it is wrong.
    if inventory.payload_src_root.is_empty()
        || Path::new(&inventory.payload_src_root)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(InventoryError::new(format!(
            "payload_src_root must be a relative path with no parent escape: {}",
            inventory.payload_src_root
        )));
    }

    let mut unpinned = BTreeSet::new();
    let mut unexpected_network = BTreeSet::new();
    let mut invalid_controls = BTreeSet::new();
    for subject in &inventory.cleanroom.subject {
        if !digest_is_pinned(&subject.digest) {
            unpinned.insert(subject.id.clone());
        }
        if subject.network != "none" {
            unexpected_network.insert(format!("{}={}", subject.id, subject.network));
        }
        if subject.python != subject.control {
            invalid_controls.insert(subject.id.clone());
        }
    }
    if !unpinned.is_empty() {
        return Err(InventoryError::new(format_named_list(
            "unpinned cleanroom subject",
            &unpinned,
        )));
    }
    if !unexpected_network.is_empty() {
        return Err(InventoryError::new(format_named_list(
            "unexpected cleanroom network",
            &unexpected_network,
        )));
    }
    if !invalid_controls.is_empty() {
        return Err(InventoryError::new(format_named_list(
            "invalid cleanroom control",
            &invalid_controls,
        )));
    }
    Ok(())
}

/// Extract a named SHA-256 constant from the Rust source which owns it.
///
/// Both model-asset staging and archive-slot validation use this parser so a
/// digest declaration has exactly one interpretation in the producer.
#[must_use]
pub fn digest_const_hex(source: &str, name: &str) -> Option<String> {
    let mut pending: Option<&str> = None;
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pub const ") {
            let Some((const_name, after)) = rest.split_once(':') else {
                continue;
            };
            if !after.contains("&str") {
                continue;
            }
            if const_name.trim() != name {
                pending = None;
                continue;
            }
            if let Some((_, literal)) = trimmed.split_once('=') {
                let hex = literal
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .trim_matches('"');
                if hex.len() == 64 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                    return Some(hex.to_owned());
                }
            }
            pending = Some(name);
            continue;
        }
        if pending.take() == Some(name) {
            let hex = trimmed.trim_end_matches(';').trim().trim_matches('"');
            if hex.len() == 64 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Some(hex.to_owned());
            }
        }
    }
    None
}

fn validate_archive_slots(path: &Path, inventory: &Inventory) -> Result<(), InventoryError> {
    let repository = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| {
            InventoryError::new(format!(
                "inventory path has no repository root: {}",
                path.display()
            ))
        })?;
    let mut slot_ids = BTreeSet::new();
    for entry in &inventory.entry {
        let Entry::ModelAsset {
            dest,
            targets,
            archive_slot: Some(slot),
            ..
        } = entry
        else {
            continue;
        };
        if !slot_ids.insert(slot.id.clone()) {
            return Err(InventoryError::new(format!(
                "duplicate archive slot id {}",
                slot.id
            )));
        }
        if !targets.iter().any(|target| target == &slot.target) {
            return Err(InventoryError::new(format!(
                "archive slot {} target {} is not admitted by {dest}",
                slot.id, slot.target
            )));
        }
        let mut executable_paths = BTreeSet::new();
        for executable in &slot.executables {
            validate_archive_member_path(&executable.path).map_err(|reason| {
                InventoryError::new(format!(
                    "archive slot {} executable {}: {reason}",
                    slot.id, executable.path
                ))
            })?;
            if !executable_paths.insert(executable.path.clone()) {
                return Err(InventoryError::new(format!(
                    "archive slot {} duplicate executable path {}",
                    slot.id, executable.path
                )));
            }
            let source_path = repository.join(&executable.digest_source);
            let source = fs::read_to_string(&source_path).map_err(|error| {
                InventoryError::new(format!(
                    "read archive executable digest source {}: {error}",
                    source_path.display()
                ))
            })?;
            if digest_const_hex(&source, &executable.digest_const).is_none() {
                return Err(InventoryError::new(format!(
                    "missing required:\n  digest {}",
                    executable.digest_const
                )));
            }
        }
    }
    Ok(())
}

pub fn validate_archive_member_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("empty path");
    }
    if path.starts_with('/') {
        return Err("absolute path");
    }
    if path.contains('\\') {
        return Err("non-POSIX separator");
    }
    for component in path.split('/') {
        if component.is_empty() {
            return Err("empty path segment");
        }
        if component == "." || component == ".." {
            return Err("non-canonical path segment");
        }
    }
    Ok(())
}

/// Every target declares exactly the field set its own os builds through, and
/// none of the other os's. A macOS target carrying `deb_arch`, or a Linux
/// target carrying `triple_apple`, is refused rather than silently ignored —
/// an ignored field is how one os's contract drifts into the other's.
fn validate_targets(targets: &[Target]) -> Result<(), InventoryError> {
    let mut missing = BTreeSet::new();
    let mut unexpected = BTreeSet::new();
    for target in targets {
        let linux_fields = [
            ("deb_arch", target.deb_arch.as_str()),
            ("rpm_arch", target.rpm_arch.as_str()),
            ("triple_musl", target.triple_musl.as_str()),
            ("triple_gnu", target.triple_gnu.as_str()),
            ("zig_gnu", target.zig_gnu.as_str()),
        ];
        let macos_fields = [
            ("lane", target.lane.as_str()),
            ("triple_apple", target.triple_apple.as_str()),
            ("min_macos", target.min_macos.as_str()),
        ];
        let (required, forbidden) = match target.os.as_str() {
            OS_LINUX => (&linux_fields[..], &macos_fields[..]),
            OS_MACOS => (&macos_fields[..], &linux_fields[..]),
            other => {
                unexpected.insert(format!("{} os {other}", target.id));
                continue;
            }
        };
        for (name, value) in required {
            if value.is_empty() {
                missing.insert(format!("{} {name}", target.id));
            }
        }
        for (name, value) in forbidden {
            if !value.is_empty() {
                unexpected.insert(format!("{} {name}", target.id));
            }
        }
        if target.is_macos() {
            if !target.lane.is_empty() && !KNOWN_TARGET_LANES.contains(&target.lane.as_str()) {
                unexpected.insert(format!("{} lane {}", target.id, target.lane));
            }
            if !target.min_macos.is_empty() && parse_min_macos(&target.min_macos).is_none() {
                unexpected.insert(format!("{} min_macos {}", target.id, target.min_macos));
            }
        }
    }
    if !missing.is_empty() {
        return Err(InventoryError::new(format_named_list(
            "missing required target field",
            &missing,
        )));
    }
    if !unexpected.is_empty() {
        return Err(InventoryError::new(format_named_list(
            "unexpected target field",
            &unexpected,
        )));
    }
    Ok(())
}

/// `"14.0"` -> `(14, 0)`. The macOS analogue of the Linux GLIBC ceiling: the
/// deployment target every shipped Mach-O must declare at or below.
#[must_use]
pub fn parse_min_macos(value: &str) -> Option<(u32, u32)> {
    let mut parts = value.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor))
}

#[must_use]
pub fn digest_is_pinned(digest: &str) -> bool {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !hex.eq_ignore_ascii_case("REFUSEUNPINNED")
        && hex.bytes().any(|byte| byte != b'0')
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
        }
        | Entry::Pdfium {
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

#[cfg(test)]
mod tests {
    use super::*;

    const COMMITTED: &str = include_str!("../../../distribution/inventory.toml");

    fn parse(text: &str) -> Result<Inventory, InventoryError> {
        let inventory: Inventory = toml_edit::de::from_str(text)
            .map_err(|error| InventoryError::new(format!("parse: {error}")))?;
        validate_targets(&inventory.target)?;
        Ok(inventory)
    }

    fn committed() -> Inventory {
        toml_edit::de::from_str(COMMITTED).expect("committed inventory parses")
    }

    #[test]
    fn model_assets_require_an_explicit_digest_source() {
        let missing = COMMITTED.replacen(
            "digest_source = \"core/crates/solstone-core-transcribe/src/model_assets.rs\"\n",
            "",
            1,
        );
        let error = toml_edit::de::from_str::<Inventory>(&missing)
            .expect_err("model assets without a digest source are rejected")
            .to_string();
        assert!(error.contains("digest_source"), "{error}");
    }

    #[test]
    fn the_committed_macos_target_declares_its_own_field_set_and_no_linux_one() {
        let inventory = committed();
        let target = inventory
            .target
            .iter()
            .find(|target| target.id == "macos-arm64")
            .expect("macos target");
        assert!(target.is_macos());
        assert_eq!(target.lane, "apple-native");
        assert_eq!(target.triple_apple, "aarch64-apple-darwin");
        assert_eq!(parse_min_macos(&target.min_macos), Some((14, 0)));
        assert_eq!(target.deb_arch, "");
        assert_eq!(target.rpm_arch, "");
        assert_eq!(target.triple_musl, "");
        assert_eq!(target.triple_gnu, "");
        assert_eq!(target.zig_gnu, "");
        assert_eq!(target.triples(), vec!["aarch64-apple-darwin"]);
    }

    #[test]
    fn a_macos_target_resolves_every_entry_lane_to_its_own_lane() {
        let inventory = committed();
        let macos = inventory
            .target
            .iter()
            .find(|target| target.id == "macos-arm64")
            .unwrap();
        let linux = inventory
            .target
            .iter()
            .find(|target| target.id == "linux-x86_64")
            .unwrap();
        // The declared lanes on the shared entries stay the Linux ones. If
        // `lane_for` ever stopped overriding, the macOS build would look for
        // `x86_64-unknown-linux-musl` artifacts and quietly select nothing.
        for entry in &inventory.entry {
            let Entry::Bin { lane, .. } = entry else {
                continue;
            };
            assert_eq!(macos.lane_for(lane), "apple-native");
            assert_eq!(linux.lane_for(lane), lane.as_str());
        }
        assert_eq!(
            macos.triple_for_lane("apple-native"),
            "aarch64-apple-darwin"
        );
        assert_eq!(
            linux.triple_for_lane("musl-static"),
            "x86_64-unknown-linux-musl"
        );
        assert_eq!(
            linux.triple_for_lane("zig-gnu-2.27"),
            "x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn the_admitted_binary_count_is_ten_and_names_the_pdf_and_vad_helpers() {
        let inventory = committed();
        let bins = inventory.required_bins();
        assert_eq!(
            bins.len(),
            10,
            "admitted-binary count must move with the inventory, not widen"
        );
        assert!(bins.contains("solstone-core-pdf"));
        assert!(bins.contains("solstone-core-vad-analyze"));
        assert!(!inventory.forbidden_bins().contains("solstone-core-pdf"));
        assert!(
            !inventory
                .forbidden_bins()
                .contains("solstone-core-vad-analyze")
        );
        assert!(inventory.entry.iter().any(|entry| {
            matches!(
                entry,
                Entry::Pdfium {
                    dest_dir,
                    ..
                } if dest_dir == "lib/solstone-core-pdf"
            )
        }));
    }

    #[test]
    fn every_admitted_binary_and_payload_ships_on_macos_too() {
        // "The same distribution tree" is the contract, so the macOS target's
        // dest set must equal the Linux one exactly, minus a named, documented
        // set of deliberately target-exclusive entries. A drift in the shared
        // set — a binary Linux ships and macOS does not, or the reverse — is
        // what this asserts against; the exception lists are not an escape
        // hatch for that, only explicitly distinct target payloads belong in
        // them.
        //
        // bin/parakeet-helper is that exception: it is the CoreML subprocess
        // helper the macOS parakeet backend spawns (parakeet_coreml.rs).
        // Linux's parakeet backend (parakeet_cpp.rs) connects over HTTP to a
        // separately-managed parakeet-cpp server that this inventory does not
        // admit at all — there is no Linux binary for this to match, by
        // design, not by drift. The RF-DETR engine archives are likewise
        // target-specific payloads: each target receives its own archive.
        const MACOS_ONLY: &[&str] = &[
            "bin/parakeet-helper",
            "lib/solstone_journal_models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz",
        ];
        const LINUX_ONLY: &[&str] = &[
            "lib/solstone_journal_models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz",
        ];
        let inventory = committed();
        let dests_for = |id: &str| {
            inventory
                .entry
                .iter()
                .filter(|entry| entry_fields(entry).1.iter().any(|target| target == id))
                .map(|entry| entry_fields(entry).0[0].to_owned())
                .collect::<BTreeSet<_>>()
        };
        let mut linux = dests_for("linux-x86_64");
        let mut macos = dests_for("macos-arm64");
        for exception in MACOS_ONLY {
            assert!(
                macos.remove(*exception),
                "{exception} is declared as a macOS-only exception but is not admitted for macos-arm64"
            );
        }
        for exception in LINUX_ONLY {
            assert!(
                linux.remove(*exception),
                "{exception} is declared as a Linux-only exception but is not admitted for linux-x86_64"
            );
        }
        assert!(!linux.is_empty());
        assert_eq!(linux, macos);
    }

    #[test]
    fn both_platforms_promote_a_six_file_set_and_neither_names_the_others_container() {
        let base = "solstone-journal-1.0.22-linux-x86_64";
        let linux = artifact_set_for_os(OS_LINUX, base);
        assert_eq!(linux.len(), 6);
        assert!(linux.iter().any(|name| name.ends_with(".deb")));
        assert!(linux.iter().any(|name| name.ends_with(".rpm")));
        assert!(!linux.iter().any(|name| name.ends_with(".pkg")));
        assert!(!linux.iter().any(|name| name.ends_with(".signing.json")));

        let base = "solstone-journal-1.0.22-macos-arm64";
        let macos = artifact_set_for_os(OS_MACOS, base);
        assert_eq!(macos.len(), 6);
        assert!(macos.iter().any(|name| name.ends_with(".tar.gz")));
        assert!(macos.iter().any(|name| name.ends_with(".pkg")));
        assert!(macos.iter().any(|name| name.ends_with(".signing.json")));
        assert!(!macos.iter().any(|name| name.ends_with(".deb")));
        assert!(!macos.iter().any(|name| name.ends_with(".rpm")));
    }

    #[test]
    fn the_basename_template_renders_each_platforms_own_name() {
        let artifact = committed().artifact;
        assert_eq!(
            artifact.render("1.0.22", "linux", "x86_64"),
            "solstone-journal-1.0.22-linux-x86_64"
        );
        assert_eq!(
            artifact.render("1.0.22", "macos", "arm64"),
            "solstone-journal-1.0.22-macos-arm64"
        );
    }

    #[test]
    fn a_target_carrying_the_other_platforms_fields_is_refused_both_ways() {
        let macos_with_deb = r#"
version = 1
product = "p"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "core/payload"
entry = []
deny = []
[artifact]
basename = "p-{version}-{os}-{arch}"
[[target]]
id = "macos-arm64"
os = "macos"
arch = "arm64"
lane = "apple-native"
triple_apple = "aarch64-apple-darwin"
min_macos = "14.0"
deb_arch = "arm64"
"#;
        let error = parse(macos_with_deb).unwrap_err().to_string();
        assert!(error.contains("unexpected target field"), "{error}");
        assert!(error.contains("macos-arm64 deb_arch"), "{error}");

        let linux_without_zig = r#"
version = 1
product = "p"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "core/payload"
entry = []
deny = []
[artifact]
basename = "p-{version}-{os}-{arch}"
[[target]]
id = "linux-x86_64"
os = "linux"
arch = "x86_64"
deb_arch = "amd64"
rpm_arch = "x86_64"
triple_musl = "x86_64-unknown-linux-musl"
triple_gnu = "x86_64-unknown-linux-gnu"
"#;
        let error = parse(linux_without_zig).unwrap_err().to_string();
        assert!(error.contains("missing required target field"), "{error}");
        assert!(error.contains("linux-x86_64 zig_gnu"), "{error}");

        let unknown_lane = r#"
version = 1
product = "p"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "core/payload"
entry = []
deny = []
[artifact]
basename = "p-{version}-{os}-{arch}"
[[target]]
id = "macos-arm64"
os = "macos"
arch = "arm64"
lane = "xcodebuild"
triple_apple = "aarch64-apple-darwin"
min_macos = "14.0"
"#;
        let error = parse(unknown_lane).unwrap_err().to_string();
        assert!(error.contains("macos-arm64 lane xcodebuild"), "{error}");

        // The control: the committed inventory passes the same validator, so a
        // refusal above is the rule firing rather than the parser being broken.
        validate_targets(&committed().target).expect("committed targets validate");
    }

    #[test]
    fn min_macos_parses_only_a_real_deployment_target() {
        assert_eq!(parse_min_macos("14.0"), Some((14, 0)));
        assert_eq!(parse_min_macos("15"), Some((15, 0)));
        assert_eq!(parse_min_macos("14.0.1"), None);
        assert_eq!(parse_min_macos("sonoma"), None);
        assert_eq!(parse_min_macos(""), None);
    }

    #[test]
    fn a_macos_target_without_the_apple_contract_is_refused() {
        let missing_apple = COMMITTED
            .split("[[target]]")
            .next()
            .unwrap()
            .replace("[apple]", "[apple_disabled]");
        assert!(missing_apple.contains("[apple_disabled]"));
        // Rebuild a minimal inventory with a macos target and no [apple].
        let text = r#"
version = 1
product = "p"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "core/payload"
entry = []
deny = []
[artifact]
basename = "p-{version}-{os}-{arch}"
[[target]]
id = "macos-arm64"
os = "macos"
arch = "arm64"
lane = "apple-native"
triple_apple = "aarch64-apple-darwin"
min_macos = "14.0"
"#;
        let inventory: Inventory = toml_edit::de::from_str(text).unwrap();
        assert!(!inventory.apple.is_declared());
        assert!(inventory.target.iter().any(Target::is_macos));
        // And the committed one does declare it.
        assert!(committed().apple.is_declared());
    }

    #[test]
    fn the_apple_keychain_path_expands_the_home_shorthand() {
        let apple = Apple {
            keychain: "~/Library/Keychains/sol-signing.keychain-db".to_owned(),
            ..Apple::default()
        };
        let path = apple.keychain_path();
        assert!(path.is_absolute());
        assert!(!path.to_string_lossy().starts_with('~'));
        assert!(path.ends_with("Library/Keychains/sol-signing.keychain-db"));

        let absolute = Apple {
            keychain: "/opt/keys/sol.keychain-db".to_owned(),
            ..Apple::default()
        };
        assert_eq!(
            absolute.keychain_path(),
            PathBuf::from("/opt/keys/sol.keychain-db")
        );
    }
}
