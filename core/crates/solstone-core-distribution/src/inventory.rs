// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::archive_taxonomy::ContainerKind;

/// `zig-gnu-2.27` is the build baseline, not the shipped dependency ceiling.
const KNOWN_LANES: &[&str] = &["musl-static", "zig-gnu-2.27"];
/// Lanes a target may declare for itself. Linux entries carry a per-binary lane
/// because the Linux tree is built by two distinct cross toolchains; macOS and
/// Windows each have exactly one native toolchain, so the lane is a property of
/// the target and the per-entry `lane` is not consulted for them.
const KNOWN_TARGET_LANES: &[&str] = &["apple-native", "msvc-native"];
pub const OS_LINUX: &str = "linux";
pub const OS_MACOS: &str = "macos";
pub const OS_WINDOWS: &str = "windows";

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
    pub notary_profile: String,
    #[serde(default)]
    pub keychain: String,
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
            && !self.notary_profile.is_empty()
            && !self.keychain.is_empty()
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
/// through `.deb` and `.rpm`. macOS has no wrapper: Journal.app embeds the
/// `.tar.gz` of the signed, notarized tree and is the only way a Mac gets this
/// runtime.
pub fn artifact_archives_for_os(os: &str, basename: &str) -> Result<Vec<String>, &'static str> {
    match os {
        OS_MACOS => Ok(vec![format!("{basename}.tar.gz")]),
        OS_LINUX => Ok(artifact_archives(basename).to_vec()),
        OS_WINDOWS => Err("windows archive/signing is not implemented on this platform"),
        other => panic!("unexpected distribution os {other}"),
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

#[must_use]
pub fn versioned_bootstrap_member(version: &str) -> String {
    format!("solstone-journal-{version}-install.sh")
}

pub fn extract_version_from_basename(basename: &str) -> Option<&str> {
    let rest = basename.strip_prefix("solstone-journal-")?;
    let (version, _) = rest.split_once('-')?;
    Some(version)
}

/// Members protected by the checksum sidecar. The receipt is a first-class
/// macOS release fact, so it is covered alongside the containers and release
/// declaration rather than being an unsigned afterthought.
pub fn checksum_members_for_os(os: &str, basename: &str) -> Result<Vec<String>, &'static str> {
    let version =
        extract_version_from_basename(basename).ok_or("cannot derive version from basename")?;
    let bootstrap = versioned_bootstrap_member(version);
    match os {
        OS_MACOS => {
            let mut names = artifact_archives_for_os(os, basename)?;
            names.push(format!("{basename}.release"));
            names.push(format!("{basename}.signing.json"));
            names.push(bootstrap);
            Ok(names)
        }
        OS_LINUX => {
            let mut names = artifact_archives_for_os(os, basename)?;
            names.push(format!("{basename}.release"));
            names.push(bootstrap);
            Ok(names)
        }
        OS_WINDOWS => Err("windows archive/signing is not implemented on this platform"),
        other => panic!("unexpected distribution os {other}"),
    }
}

/// Members protected by the manifest. The manifest never protects itself (or
/// its eventual minisign signature), but it does bind the checksum sidecar.
pub fn manifest_members_for_os(os: &str, basename: &str) -> Result<Vec<String>, &'static str> {
    match os {
        OS_MACOS | OS_LINUX => {
            let mut names = checksum_members_for_os(os, basename)?;
            names.push(format!("{basename}.sha256"));
            Ok(names)
        }
        OS_WINDOWS => Err("windows archive/signing is not implemented on this platform"),
        other => panic!("unexpected distribution os {other}"),
    }
}

#[must_use]
pub fn artifact_set(basename: &str) -> Vec<String> {
    let [tar, deb, rpm] = artifact_archives(basename);
    let [sha256, manifest, release] = artifact_sidecars(basename);
    let mut set = vec![tar, deb, rpm, sha256, manifest, release];
    if let Some(version) = extract_version_from_basename(basename) {
        set.push(versioned_bootstrap_member(version));
    }
    set
}

/// Sidecars for `os`. macOS carries a fourth: the signing receipt, which is
/// provenance the Linux set does not need and which used to be produced by
/// `scripts/record_macos_native_wheel.py`.
pub fn artifact_sidecars_for_os(os: &str, basename: &str) -> Result<Vec<String>, &'static str> {
    let version =
        extract_version_from_basename(basename).ok_or("cannot derive version from basename")?;
    let bootstrap = versioned_bootstrap_member(version);
    match os {
        OS_MACOS => {
            let mut names = artifact_sidecars(basename).to_vec();
            names.push(format!("{basename}.signing.json"));
            names.push(bootstrap);
            Ok(names)
        }
        OS_LINUX => {
            let mut names = artifact_sidecars(basename).to_vec();
            names.push(bootstrap);
            Ok(names)
        }
        OS_WINDOWS => Err("windows archive/signing is not implemented on this platform"),
        other => panic!("unexpected distribution os {other}"),
    }
}

/// The complete atomic set for `os`: every container plus every sidecar.
/// Promotion renames one directory holding exactly this set or nothing at all.
/// Completeness is the invariant, not the count.
pub fn artifact_set_for_os(os: &str, basename: &str) -> Result<Vec<String>, &'static str> {
    match os {
        OS_MACOS => {
            let mut names = artifact_archives_for_os(os, basename)?;
            names.extend(artifact_sidecars_for_os(os, basename)?);
            Ok(names)
        }
        OS_LINUX => {
            let mut names = artifact_archives_for_os(os, basename)?;
            names.extend(artifact_sidecars_for_os(os, basename)?);
            Ok(names)
        }
        OS_WINDOWS => Err("windows archive/signing is not implemented on this platform"),
        other => panic!("unexpected distribution os {other}"),
    }
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
    /// Target-owned lane for macOS and Windows. Empty on Linux.
    #[serde(default)]
    pub lane: String,
    /// macOS-only fields.
    #[serde(default)]
    pub triple_apple: String,
    #[serde(default)]
    pub min_macos: String,
    /// Windows-only field.
    #[serde(default)]
    pub triple_windows: String,
}

impl Target {
    #[must_use]
    pub fn is_macos(&self) -> bool {
        self.os == OS_MACOS
    }

    #[must_use]
    pub fn is_windows(&self) -> bool {
        self.os == OS_WINDOWS
    }

    /// The lane that actually builds `entry_lane` for this target. On macOS and
    /// Windows the target owns the lane; on Linux the entry does.
    #[must_use]
    pub fn lane_for<'a>(&'a self, entry_lane: &'a str) -> &'a str {
        if self.is_macos() || self.is_windows() {
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
            "msvc-native" => &self.triple_windows,
            "musl-static" => &self.triple_musl,
            _ => &self.triple_gnu,
        }
    }

    /// Every triple this target may legitimately produce artifacts under.
    #[must_use]
    pub fn triples(&self) -> Vec<&str> {
        [
            self.triple_apple.as_str(),
            self.triple_windows.as_str(),
            self.triple_musl.as_str(),
            self.triple_gnu.as_str(),
        ]
        .into_iter()
        .filter(|triple| !triple.is_empty())
        .collect()
    }
}

/// Native input namespace only; installation destinations stay in each entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowsNativeComponent {
    Llama,
    Ced,
    Onnx,
    Parakeet,
    Rfdetr,
    Pdfium,
    Msvc,
    Restic,
    Rclone,
    Ffmpeg,
    Nvattest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Entry {
    WindowsBuildEvidence {
        dest: String,
        mode: u32,
        targets: Vec<String>,
    },
    WindowsNative {
        component: WindowsNativeComponent,
        member: String,
        dest: String,
        mode: u32,
        targets: Vec<String>,
    },
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
        #[serde(default)]
        identities: Vec<NativeIdentity>,
        targets: Vec<String>,
    },
    Pdfium {
        dest_dir: String,
        mode: u32,
        #[serde(default)]
        identities: Vec<NativeIdentity>,
        targets: Vec<String>,
    },
    Copy {
        source: String,
        dest: String,
        mode: u32,
        targets: Vec<String>,
    },
    PinnedNative {
        #[serde(default)]
        component: Option<String>,
        input: PinnedInput,
        dest: String,
        mode: u32,
        identity: NativeIdentity,
        targets: Vec<String>,
    },
    PinnedMembers {
        #[serde(default)]
        component: Option<String>,
        input: PinnedInput,
        staged: Vec<StagedMember>,
        #[serde(default)]
        ignored: Vec<String>,
        targets: Vec<String>,
    },
    LicenceTree {
        source: String,
        #[serde(default)]
        component: Option<String>,
        targets: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PinnedInput {
    Inline {
        source: String,
        digest: String,
    },
    CatalogCommitted {
        unit: String,
        filename: String,
        path: String,
    },
    CatalogAcquired {
        unit: String,
        filename: String,
    },
    AuthorityCommitted {
        platform: String,
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedMember {
    pub relpath: String,
    pub dest: String,
    pub mode: u32,
    pub extracted_sha256: String,
    #[serde(default)]
    pub identity: Option<NativeIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeIdentity {
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveSlot {
    pub id: String,
    pub target: String,
    pub container: ContainerKind,
    #[serde(default)]
    pub inspect_only: bool,
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

fn is_valid_component(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
}

pub(crate) fn collect_licence_relative_paths(
    source_dir: &Path,
    source_name: &str,
) -> Result<Vec<String>, InventoryError> {
    if !source_dir.is_dir() {
        return Err(InventoryError::new(format!(
            "licence-tree {source_name}: missing source directory {}",
            source_dir.display()
        )));
    }
    let mut files = Vec::new();
    let mut dirs = vec![source_dir.to_path_buf()];
    while let Some(current_dir) = dirs.pop() {
        let entries = fs::read_dir(&current_dir).map_err(|e| {
            InventoryError::new(format!(
                "licence-tree {source_name}: read dir {}: {e}",
                current_dir.display()
            ))
        })?;
        for entry_res in entries {
            let entry = entry_res.map_err(|e| {
                InventoryError::new(format!(
                    "licence-tree {source_name}: read entry in {}: {e}",
                    current_dir.display()
                ))
            })?;
            let p = entry.path();
            let meta = fs::symlink_metadata(&p).map_err(|e| {
                InventoryError::new(format!(
                    "licence-tree {source_name}: metadata {}: {e}",
                    p.display()
                ))
            })?;
            let rel = p
                .strip_prefix(source_dir)
                .map_err(|e| InventoryError::new(e.to_string()))?;
            let rel_str = rel.to_str().ok_or_else(|| {
                InventoryError::new(format!("licence-tree {source_name}: non-utf8 path"))
            })?;
            if meta.file_type().is_symlink() {
                return Err(InventoryError::new(format!(
                    "licence-tree {source_name}: licence tree contains symlink at {rel_str}"
                )));
            }
            if meta.is_dir() {
                dirs.push(p);
            } else if meta.is_file() {
                files.push(rel_str.to_owned());
            }
        }
    }
    if files.is_empty() {
        return Err(InventoryError::new(format!(
            "licence-tree {source_name}: empty source directory"
        )));
    }
    files.sort();
    Ok(files)
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
    let mut unexpected_dests = BTreeSet::new();
    let mut native_members = BTreeSet::new();
    let mut windows_build_evidence = false;
    for entry in &inventory.entry {
        if let Entry::WindowsBuildEvidence { mode, targets, .. } = entry {
            if targets.as_slice() != ["windows-x86_64"] || *mode != 0o644 || windows_build_evidence
            {
                return Err(InventoryError::new(
                    "invalid or duplicate Windows build evidence entry",
                ));
            }
            windows_build_evidence = true;
        }
        if let Entry::WindowsNative {
            component,
            member,
            mode,
            targets,
            ..
        } = entry
        {
            if targets.as_slice() != ["windows-x86_64"]
                || !matches!(*mode, 0o644 | 0o755)
                || member.is_empty()
                || !member.is_ascii()
                || member.contains(['\\', ':'])
                || member
                    .split('/')
                    .any(|part| part.is_empty() || matches!(part, "." | ".."))
            {
                return Err(InventoryError::new("invalid Windows native entry"));
            }
            if !native_members.insert((*component, member.as_str())) {
                return Err(InventoryError::new("duplicate Windows native input member"));
            }
        }
        if let Entry::PinnedNative {
            component,
            dest,
            identity,
            targets,
            ..
        } = entry
        {
            if let Some(comp) = component
                && !is_valid_component(comp)
            {
                return Err(InventoryError::new(format!(
                    "pinned-native {dest}: invalid-component {comp}"
                )));
            }
            if dest.starts_with("bin/")
                || (dest.starts_with("lib/") && !dest["lib/".len()..].contains('/'))
                || dest.starts_with("share/")
                || dest.starts_with("lib/solstone_journal_models/")
            {
                return Err(InventoryError::new(format!(
                    "invalid pinned-native dest {dest}"
                )));
            }
            let Some(after_lib) = dest.strip_prefix("lib/") else {
                return Err(InventoryError::new(format!(
                    "pinned-native dest must be under lib/solstone-<component>/: {dest}"
                )));
            };
            let Some((component_part, rest)) = after_lib.split_once('/') else {
                return Err(InventoryError::new(format!(
                    "pinned-native dest cannot be directly under lib/: {dest}"
                )));
            };
            if !component_part.starts_with("solstone-")
                || component_part == "solstone-"
                || component_part == "solstone_journal_models"
                || rest.is_empty()
            {
                return Err(InventoryError::new(format!(
                    "pinned-native dest must be under lib/solstone-<component>/: {dest}"
                )));
            }
            if identity.os.is_some() {
                return Err(InventoryError::new(
                    "pinned-native identity cannot declare os",
                ));
            }
            for target_id in targets {
                if let Some(target) = inventory.target.iter().find(|t| &t.id == target_id)
                    && target.is_windows()
                {
                    return Err(InventoryError::new(format!(
                        "pinned-native cannot target Windows: {target_id}"
                    )));
                }
            }
        }
        if let Entry::PinnedMembers {
            component,
            staged,
            targets,
            ..
        } = entry
        {
            if let Some(comp) = component
                && !is_valid_component(comp)
            {
                return Err(InventoryError::new(format!(
                    "pinned-members: invalid-component {comp}"
                )));
            }
            for target_id in targets {
                let is_windows = inventory
                    .target
                    .iter()
                    .find(|t| &t.id == target_id)
                    .map(Target::is_windows)
                    .unwrap_or(false);
                for member in staged {
                    let dest = &member.dest;
                    if let Some(ref identity) = member.identity
                        && identity.os.is_some()
                    {
                        return Err(InventoryError::new(
                            "pinned-members identity cannot declare os",
                        ));
                    }
                    if is_windows {
                        let valid = if let Some(after_lib) = dest.strip_prefix("lib/") {
                            if let Some(rest) = after_lib.strip_prefix("solstone_journal_models/") {
                                !rest.is_empty()
                            } else if let Some((comp, rest)) = after_lib.split_once('/') {
                                comp.starts_with("solstone-")
                                    && comp != "solstone-"
                                    && !rest.is_empty()
                            } else {
                                false
                            }
                        } else if let Some(after_share) = dest.strip_prefix("share/licenses/") {
                            if let Some((comp, rest)) = after_share.split_once('/') {
                                !comp.is_empty() && !rest.is_empty()
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if !valid || dest.starts_with("share/provenance/") {
                            return Err(InventoryError::new(format!(
                                "invalid pinned-members dest for Windows: {dest}"
                            )));
                        }
                    } else {
                        // POSIX (Linux or macOS)
                        if dest.starts_with("bin/")
                            || dest.starts_with("share/licenses/")
                            || dest.starts_with("share/provenance/")
                            || dest.starts_with("lib/solstone_journal_models/")
                        {
                            return Err(InventoryError::new(format!(
                                "invalid pinned-members dest {dest} for {target_id}"
                            )));
                        }
                        let valid = if let Some(after_lib) = dest.strip_prefix("lib/") {
                            if let Some((comp, rest)) = after_lib.split_once('/') {
                                comp.starts_with("solstone-")
                                    && comp != "solstone-"
                                    && comp != "solstone_journal_models"
                                    && !rest.is_empty()
                            } else {
                                false
                            }
                        } else if let Some(after_lic) =
                            dest.strip_prefix("share/solstone-journal/licenses/")
                        {
                            if let Some((comp, rest)) = after_lic.split_once('/') {
                                !comp.is_empty() && !rest.is_empty()
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if !valid {
                            return Err(InventoryError::new(format!(
                                "invalid pinned-members dest {dest} for {target_id}"
                            )));
                        }
                    }
                }
            }
        }
        if let Entry::LicenceTree {
            source,
            component,
            targets,
        } = entry
        {
            let Some(comp) = component else {
                return Err(InventoryError::new(format!(
                    "licence-tree {source}: missing-component"
                )));
            };
            if !is_valid_component(comp) {
                return Err(InventoryError::new(format!(
                    "licence-tree {source}: invalid-component {comp}"
                )));
            }
            let repo = path.ancestors().nth(3).unwrap_or_else(|| Path::new("."));
            let source_dir = repo.join(source);
            let rel_files = collect_licence_relative_paths(&source_dir, source)?;
            for target_id in targets {
                if !target_ids.contains(target_id) {
                    missing_targets.insert(target_id.to_owned());
                }
                let target_obj = inventory.target.iter().find(|t| &t.id == target_id);
                let is_windows = target_obj.map(Target::is_windows).unwrap_or(false);
                for rel in &rel_files {
                    let dest = if is_windows {
                        format!("share/licenses/{comp}/{rel}")
                    } else {
                        format!("share/solstone-journal/licenses/{comp}/{rel}")
                    };
                    let key = if target_id == "windows-x86_64" {
                        dest.to_ascii_lowercase()
                    } else {
                        dest.clone()
                    };
                    if !dests_by_target
                        .entry(target_id.clone())
                        .or_default()
                        .insert(key)
                    {
                        return Err(InventoryError::new(format!(
                            "duplicate dest {dest} for target {target_id} in {}",
                            path.display()
                        )));
                    }
                    if let Some(declared) = target_obj
                        && let Err(error) = crate::layout::admit_dest(&declared.os, &dest)
                    {
                        unexpected_dests.insert(format!("{target_id} {dest}: {error}"));
                    }
                }
            }
        }
        if let Entry::OnnxRuntime {
            identities,
            targets,
            ..
        }
        | Entry::Pdfium {
            identities,
            targets,
            ..
        } = entry
        {
            let has_linux = targets.iter().any(|tid| {
                inventory
                    .target
                    .iter()
                    .any(|t| &t.id == tid && t.os == OS_LINUX)
            });
            let has_macos = targets.iter().any(|tid| {
                inventory
                    .target
                    .iter()
                    .any(|t| &t.id == tid && t.os == OS_MACOS)
            });
            if has_linux && !identities.iter().any(|i| i.os.as_deref() == Some(OS_LINUX)) {
                return Err(InventoryError::new(
                    "missing required linux identity for onnx/pdfium",
                ));
            }
            if has_macos && !identities.iter().any(|i| i.os.as_deref() == Some(OS_MACOS)) {
                return Err(InventoryError::new(
                    "missing required macos identity for onnx/pdfium",
                ));
            }
        }
        let (dests, targets, lane) = entry_fields(entry);
        for target in targets {
            if !target_ids.contains(target) {
                missing_targets.insert(target.to_owned());
            }
            for dest in &dests {
                if !dests_by_target.entry(target.clone()).or_default().insert(
                    if target == "windows-x86_64" {
                        dest.to_ascii_lowercase()
                    } else {
                        (*dest).to_owned()
                    },
                ) {
                    return Err(InventoryError::new(format!(
                        "duplicate dest {dest} for target {target} in {}",
                        path.display()
                    )));
                }
                if let Some(declared) = inventory.target.iter().find(|item| item.id == *target)
                    && let Err(error) = crate::layout::admit_dest(&declared.os, dest)
                {
                    unexpected_dests.insert(format!("{target} {dest}: {error}"));
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
    if !unexpected_dests.is_empty() {
        return Err(InventoryError::new(format_named_list(
            "unexpected windows dest",
            &unexpected_dests,
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

fn require_fields(id: &str, fields: &[(&str, &str)], missing: &mut BTreeSet<String>) {
    for (name, value) in fields {
        if value.is_empty() {
            missing.insert(format!("{id} {name}"));
        }
    }
}

fn forbid_fields(id: &str, fields: &[(&str, &str)], unexpected: &mut BTreeSet<String>) {
    for (name, value) in fields {
        if !value.is_empty() {
            unexpected.insert(format!("{id} {name}"));
        }
    }
}

/// Every target declares exactly the field set its own os builds through, and
/// none of the other os's. A macOS target carrying `deb_arch`, or a Linux
/// target carrying `triple_apple`, is refused rather than silently ignored —
/// an ignored field is how one os's contract drifts into the other's.
fn validate_targets(targets: &[Target]) -> Result<(), InventoryError> {
    let mut missing = BTreeSet::new();
    let mut unexpected = BTreeSet::new();
    for target in targets {
        let linux_only = [
            ("deb_arch", target.deb_arch.as_str()),
            ("rpm_arch", target.rpm_arch.as_str()),
            ("triple_musl", target.triple_musl.as_str()),
            ("triple_gnu", target.triple_gnu.as_str()),
            ("zig_gnu", target.zig_gnu.as_str()),
        ];
        let macos_only = [
            ("triple_apple", target.triple_apple.as_str()),
            ("min_macos", target.min_macos.as_str()),
        ];
        let windows_only = [("triple_windows", target.triple_windows.as_str())];
        let lane = target.lane.as_str();
        match target.os.as_str() {
            OS_LINUX => {
                require_fields(&target.id, &linux_only, &mut missing);
                forbid_fields(&target.id, &macos_only, &mut unexpected);
                forbid_fields(&target.id, &windows_only, &mut unexpected);
                if !lane.is_empty() {
                    unexpected.insert(format!("{} lane", target.id));
                }
            }
            OS_MACOS => {
                require_fields(&target.id, &macos_only, &mut missing);
                forbid_fields(&target.id, &linux_only, &mut unexpected);
                forbid_fields(&target.id, &windows_only, &mut unexpected);
                if lane.is_empty() {
                    missing.insert(format!("{} lane", target.id));
                } else if lane != "apple-native" {
                    unexpected.insert(format!("{} lane {lane}", target.id));
                }
                if !target.min_macos.is_empty() && parse_min_macos(&target.min_macos).is_none() {
                    unexpected.insert(format!("{} min_macos {}", target.id, target.min_macos));
                }
            }
            OS_WINDOWS => {
                require_fields(&target.id, &windows_only, &mut missing);
                forbid_fields(&target.id, &linux_only, &mut unexpected);
                forbid_fields(&target.id, &macos_only, &mut unexpected);
                if lane.is_empty() {
                    missing.insert(format!("{} lane", target.id));
                } else if lane != "msvc-native" {
                    unexpected.insert(format!("{} lane {lane}", target.id));
                }
            }
            other => {
                unexpected.insert(format!("{} os {other}", target.id));
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
        | Entry::Copy { dest, targets, .. }
        | Entry::WindowsBuildEvidence { dest, targets, .. }
        | Entry::WindowsNative { dest, targets, .. }
        | Entry::PinnedNative { dest, targets, .. } => (vec![dest.as_str()], targets, None),
        Entry::PinnedMembers {
            staged, targets, ..
        } => (
            staged.iter().map(|s| s.dest.as_str()).collect(),
            targets,
            None,
        ),
        Entry::LicenceTree { targets, .. } => (Vec::new(), targets, None),
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
    #[test]
    fn windows_native_entries_cannot_target_unix_or_reuse_an_input() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let path = repo.join("core/distribution/inventory.toml");
        let original = super::load_inventory(&path).unwrap();
        let native = original
            .entry
            .iter()
            .find(|entry| matches!(entry, super::Entry::WindowsNative { .. }))
            .unwrap()
            .clone();
        let mut duplicate = original.clone();
        duplicate.entry.push(native.clone());
        assert!(
            super::validate_inventory(&path, &duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate Windows native input")
        );
        let mut wrong = original.clone();
        let mut native = native;
        if let super::Entry::WindowsNative { targets, .. } = &mut native {
            *targets = vec!["linux-x86_64".into()];
        }
        wrong.entry.push(native);
        assert!(
            super::validate_inventory(&path, &wrong)
                .unwrap_err()
                .to_string()
                .contains("invalid Windows native entry")
        );
        let mut collision = original;
        let mut bin = collision.entry.iter().find(|entry| matches!(entry, super::Entry::Bin { dest, .. } if dest == "bin/journal.exe")).unwrap().clone();
        if let super::Entry::Bin { dest, .. } = &mut bin {
            *dest = "bin/JOURNAL.exe".into();
        }
        collision.entry.push(bin);
        assert!(
            super::validate_inventory(&path, &collision)
                .unwrap_err()
                .to_string()
                .contains("duplicate dest")
        );
    }

    use super::*;

    use std::fs;

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
        assert_eq!(parse_min_macos(&target.min_macos), Some((15, 0)));
        assert_eq!(target.deb_arch, "");
        assert_eq!(target.rpm_arch, "");
        assert_eq!(target.triple_musl, "");
        assert_eq!(target.triple_gnu, "");
        assert_eq!(target.zig_gnu, "");
        assert_eq!(target.triples(), vec!["aarch64-apple-darwin"]);
    }

    #[test]
    fn the_committed_windows_target_declares_native_commands_and_shared_models() {
        let inventory = committed();
        let target = inventory
            .target
            .iter()
            .find(|target| target.id == "windows-x86_64")
            .expect("windows target");
        assert!(target.is_windows());
        assert_eq!(target.lane, "msvc-native");
        assert_eq!(target.triple_windows, "x86_64-pc-windows-msvc");
        assert_eq!(target.deb_arch, "");
        assert_eq!(target.rpm_arch, "");
        assert_eq!(target.triple_musl, "");
        assert_eq!(target.triple_gnu, "");
        assert_eq!(target.zig_gnu, "");
        assert_eq!(target.triple_apple, "");
        assert_eq!(target.min_macos, "");
        assert_eq!(target.triples(), vec!["x86_64-pc-windows-msvc"]);
        let entries = inventory
            .entry
            .iter()
            .filter(|entry| {
                entry_fields(entry)
                    .1
                    .iter()
                    .any(|id| id == "windows-x86_64")
            })
            .collect::<Vec<_>>();
        for (public, binary) in [
            ("bin/journal.exe", "solstone-core-journal"),
            ("bin/solstone.exe", "solstone-core-sol"),
        ] {
            assert!(entries.iter().any(|entry| matches!(entry,
                Entry::Bin { dest, bin, .. } if dest == public && bin == binary)));
        }
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry, Entry::Launcher { .. }))
        );
        assert!(
            entries
                .iter()
                .any(|entry| matches!(entry, Entry::ModelAsset { dest, .. }
            if dest == crate::windows_payload::WINDOWS_RFDETR_MODEL))
        );
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry, Entry::ModelAsset { dest, .. }
            if dest.contains("ced")))
        );
        assert!(
            entries.iter().any(|entry| matches!(entry,
                Entry::PinnedMembers { staged, .. }
                    if staged.iter().any(|s| s.dest == crate::windows_payload::WINDOWS_CED_MODEL)))
        );
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
    fn a_windows_target_resolves_every_entry_lane_to_its_own_lane() {
        let inventory = committed();
        let windows = inventory
            .target
            .iter()
            .find(|target| target.id == "windows-x86_64")
            .unwrap();
        for entry in &inventory.entry {
            let Entry::Bin { lane, .. } = entry else {
                continue;
            };
            assert_eq!(windows.lane_for(lane), "msvc-native");
        }
        assert_eq!(
            windows.triple_for_lane("msvc-native"),
            "x86_64-pc-windows-msvc"
        );
    }

    #[test]
    fn the_admitted_binary_count_is_twelve_and_names_the_pdf_vad_and_ced_helpers() {
        let inventory = committed();
        let bins = inventory.required_bins();
        // Moved 10 -> 11 when sound tagging gained `solstone-core-ced-analyze`.
        // CED was previously `dlopen`ed in-process by musl-static binaries,
        // which have no dynamic loader, so it could never load in a shipped
        // build; it now ships as a zig-gnu-2.27 helper like its siblings.
        // Moved 11 -> 12 when Windows gained the journal app, `journal-app`.
        assert_eq!(
            bins.len(),
            12,
            "admitted-binary count must move with the inventory, not widen"
        );
        assert!(bins.contains("solstone-core-pdf"));
        assert!(bins.contains("solstone-core-vad-analyze"));
        assert!(bins.contains("solstone-core-ced-analyze"));
        assert!(
            !inventory
                .forbidden_bins()
                .contains("solstone-core-ced-analyze")
        );
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
        // `libced.so` / `libced.dylib` are the same engine with a platform
        // library filename, same reason as the RF-DETR archives.
        // `linux-aarch64` shares `lib/solstone-ced/libced.so` with
        // `linux-x86_64`. The shared GGUF dest stays in the compared set.
        // `libnvat.so.1` / `libnvat.1.dylib` are the same shipped verifier
        // library with a platform filename. `linux-aarch64` shares
        // `lib/solstone-nvattest/lib/libnvat.so.1` with `linux-x86_64`.
        // The executable and CA bundle stay in the compared set.
        // share/README.md is a Linux-only exception too: it is the
        // agent-facing install/crossover README for the Linux v1-to-v2
        // crossover arc, and macOS distribution is out of scope for that arc.
        const MACOS_ONLY: &[&str] = &[
            "bin/parakeet-helper",
            "lib/solstone-ced/libced.dylib",
            "lib/solstone-nvattest/lib/libnvat.1.dylib",
            "lib/solstone_journal_models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz",
        ];
        const LINUX_ONLY: &[&str] = &[
            "lib/solstone-ced/libced.so",
            "lib/solstone-nvattest/lib/libnvat.so.1",
            "lib/solstone_journal_models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz",
            "share/README.md",
        ];
        let inventory = committed();
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let dests_for = |id: &str| {
            let mut set = BTreeSet::new();
            for entry in &inventory.entry {
                let (dests, targets, _) = entry_fields(entry);
                if !targets.iter().any(|target| target == id) {
                    continue;
                }
                if let Entry::LicenceTree {
                    source, component, ..
                } = entry
                {
                    let comp = component.as_deref().expect("licence-tree component");
                    let source_dir = repo.join(source);
                    let rel_files =
                        collect_licence_relative_paths(&source_dir, source).expect("licence files");
                    let prefix = format!("share/solstone-journal/licenses/{comp}/");
                    for rel in rel_files {
                        set.insert(format!("{prefix}{rel}"));
                    }
                } else {
                    for dest in dests {
                        set.insert(dest.to_owned());
                    }
                }
            }
            set
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
    fn each_platform_promotes_only_its_own_containers() {
        let base = "solstone-journal-1.0.22-linux-x86_64";
        let linux = artifact_set_for_os(OS_LINUX, base).expect("linux");
        assert_eq!(linux.len(), 7);
        assert!(linux.iter().any(|name| name.ends_with(".deb")));
        assert!(linux.iter().any(|name| name.ends_with(".rpm")));
        assert!(
            linux
                .iter()
                .any(|name| name == "solstone-journal-1.0.22-install.sh")
        );
        assert!(!linux.iter().any(|name| name.ends_with(".pkg")));
        assert!(!linux.iter().any(|name| name.ends_with(".signing.json")));

        let base = "solstone-journal-1.0.22-macos-arm64";
        let macos = artifact_set_for_os(OS_MACOS, base).expect("macos");
        assert_eq!(macos.len(), 6);
        assert!(macos.iter().any(|name| name.ends_with(".tar.gz")));
        assert!(!macos.iter().any(|name| name.ends_with(".pkg")));
        assert!(macos.iter().any(|name| name.ends_with(".signing.json")));
        assert!(
            macos
                .iter()
                .any(|name| name == "solstone-journal-1.0.22-install.sh")
        );
        assert!(!macos.iter().any(|name| name.ends_with(".deb")));
        assert!(!macos.iter().any(|name| name.ends_with(".rpm")));
    }

    #[test]
    fn windows_artifact_helpers_refuse() {
        let base = "solstone-journal-1.0.22-windows-x86_64";
        let refusal = "windows archive/signing is not implemented on this platform";
        assert_eq!(
            artifact_archives_for_os(OS_WINDOWS, base).unwrap_err(),
            refusal
        );
        assert_eq!(
            checksum_members_for_os(OS_WINDOWS, base).unwrap_err(),
            refusal
        );
        assert_eq!(
            artifact_sidecars_for_os(OS_WINDOWS, base).unwrap_err(),
            refusal
        );
        assert_eq!(
            manifest_members_for_os(OS_WINDOWS, base).unwrap_err(),
            refusal
        );
        assert_eq!(artifact_set_for_os(OS_WINDOWS, base).unwrap_err(), refusal);
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
min_macos = "15.0"
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
min_macos = "15.0"
"#;
        let error = parse(unknown_lane).unwrap_err().to_string();
        assert!(error.contains("macos-arm64 lane xcodebuild"), "{error}");

        let windows_with_deb = r#"
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
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
lane = "msvc-native"
triple_windows = "x86_64-pc-windows-msvc"
deb_arch = "amd64"
"#;
        let error = parse(windows_with_deb).unwrap_err().to_string();
        assert!(error.contains("unexpected target field"), "{error}");
        assert!(error.contains("windows-x86_64 deb_arch"), "{error}");

        let windows_without_triple = r#"
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
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
lane = "msvc-native"
"#;
        let error = parse(windows_without_triple).unwrap_err().to_string();
        assert!(error.contains("missing required target field"), "{error}");
        assert!(error.contains("windows-x86_64 triple_windows"), "{error}");

        let macos_with_msvc_lane = r#"
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
lane = "msvc-native"
triple_apple = "aarch64-apple-darwin"
min_macos = "15.0"
"#;
        let error = parse(macos_with_msvc_lane).unwrap_err().to_string();
        assert!(error.contains("unexpected target field"), "{error}");
        assert!(error.contains("macos-arm64 lane msvc-native"), "{error}");

        let windows_with_apple_lane = r#"
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
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
lane = "apple-native"
triple_windows = "x86_64-pc-windows-msvc"
"#;
        let error = parse(windows_with_apple_lane).unwrap_err().to_string();
        assert!(error.contains("unexpected target field"), "{error}");
        assert!(
            error.contains("windows-x86_64 lane apple-native"),
            "{error}"
        );

        let windows_unknown_lane = r#"
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
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
lane = "xcodebuild"
triple_windows = "x86_64-pc-windows-msvc"
"#;
        let error = parse(windows_unknown_lane).unwrap_err().to_string();
        assert!(error.contains("unexpected target field"), "{error}");
        assert!(error.contains("windows-x86_64 lane xcodebuild"), "{error}");

        let windows_empty_lane = r#"
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
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
triple_windows = "x86_64-pc-windows-msvc"
"#;
        let error = parse(windows_empty_lane).unwrap_err().to_string();
        assert!(error.contains("missing required target field"), "{error}");
        assert!(error.contains("windows-x86_64 lane"), "{error}");

        let windows_with_apple_fields = r#"
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
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
lane = "msvc-native"
triple_windows = "x86_64-pc-windows-msvc"
triple_apple = "aarch64-apple-darwin"
min_macos = "15.0"
"#;
        let error = parse(windows_with_apple_fields).unwrap_err().to_string();
        assert!(error.contains("unexpected target field"), "{error}");
        assert!(error.contains("windows-x86_64 triple_apple"), "{error}");
        assert!(error.contains("windows-x86_64 min_macos"), "{error}");

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
min_macos = "15.0"
"#;
        let inventory: Inventory = toml_edit::de::from_str(text).unwrap();
        assert!(!inventory.apple.is_declared());
        assert!(inventory.target.iter().any(Target::is_macos));
        // And the committed one does declare it.
        assert!(committed().apple.is_declared());
    }

    #[test]
    fn windows_entry_dests_outside_the_layout_are_refused() {
        let root = tempfile::Builder::new()
            .prefix("solstone-distribution-windows-dest-")
            .tempdir()
            .expect("temporary inventory");
        let distribution = root.path().join("core/distribution");
        fs::create_dir_all(&distribution).unwrap();
        fs::write(distribution.join("payload.txt"), "").unwrap();
        fs::write(
            distribution.join("inventory.toml"),
            r#"
version = 1
product = "p"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "core/payload"
deny = []
[artifact]
basename = "p-{version}-{os}-{arch}"
[[target]]
id = "windows-x86_64"
os = "windows"
arch = "x86_64"
lane = "msvc-native"
triple_windows = "x86_64-pc-windows-msvc"
[[entry]]
kind = "copy"
source = "LICENSE"
dest = "runtime/solstone-core.exe"
mode = 0o644
targets = ["windows-x86_64"]
"#,
        )
        .unwrap();
        let error = load_inventory(&distribution.join("inventory.toml"))
            .expect_err("retired synthetic dest on windows")
            .to_string();
        assert!(error.contains("unexpected windows dest"), "{error}");
        assert!(error.contains("runtime/solstone-core.exe"), "{error}");
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

    #[test]
    fn pinned_native_dest_and_target_validation() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let path = repo.join("core/distribution/inventory.toml");
        let original = super::load_inventory(&path).unwrap();

        // No pinned-native entry in committed inventory.toml
        assert!(
            !original
                .entry
                .iter()
                .any(|entry| matches!(entry, super::Entry::PinnedNative { .. })),
            "no pinned-native entry should be in inventory.toml"
        );

        let make_pinned = |dest: &str, target: &str| super::Entry::PinnedNative {
            component: None,
            input: super::PinnedInput::Inline {
                source: "fixture.so".to_owned(),
                digest: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_owned(),
            },
            dest: dest.to_owned(),
            mode: 0o755,
            identity: super::NativeIdentity {
                os: None,
                name: Some("libx.so".to_owned()),
                aliases: vec![],
            },
            targets: vec![target.to_owned()],
        };

        for bad_dest in [
            "bin/foo",
            "lib/foo.so",
            "share/foo.so",
            "lib/solstone_journal_models/foo.so",
        ] {
            let mut inv = original.clone();
            inv.entry.push(make_pinned(bad_dest, "linux-x86_64"));
            assert!(
                super::validate_inventory(&path, &inv).is_err(),
                "should refuse bad dest: {bad_dest}"
            );
        }

        // Windows target is refused
        let mut inv_win = original.clone();
        inv_win
            .entry
            .push(make_pinned("lib/solstone-foo/libx.so", "windows-x86_64"));
        assert!(
            super::validate_inventory(&path, &inv_win).is_err(),
            "should refuse windows target for pinned-native"
        );

        // A dest `lib/solstone-foo/libx.so` with a non-windows target passes validation
        let mut inv_ok = original.clone();
        inv_ok
            .entry
            .push(make_pinned("lib/solstone-foo/libx.so", "linux-x86_64"));
        super::validate_inventory(&path, &inv_ok).expect("valid pinned-native entry passes");
    }

    #[test]
    fn pinned_input_rejects_unknown_fields() {
        let toml_str = r#"
version = 1
product = "solstone"
payload = "core/distribution/payload.txt"
payload_dest_prefix = "share/solstone-journal"
payload_src_root = "core/payload"
entry = [
    { kind = "pinned-members", targets = ["linux-x86_64"], input = { kind = "catalog-acquired", unit = "u", filename = "f", digest = "d" }, staged = [] }
]
deny = []
[artifact]
basename = "solstone-{version}-{os}-{arch}"
[[target]]
id = "linux-x86_64"
os = "linux"
arch = "x86_64"
lane = "glibc-native"
deb_arch = "amd64"
rpm_arch = "x86_64"
triple_musl = "x86_64-unknown-linux-musl"
triple_gnu = "x86_64-unknown-linux-gnu"
zig_gnu = "x86_64-linux-gnu.2.28"
"#;
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), toml_str).unwrap();
        let err = super::load_inventory(temp.path()).unwrap_err().to_string();
        assert!(
            err.contains("digest"),
            "expected error to contain 'digest', got: {err}"
        );

        let toml_inline = r#"
version = 1
product = "solstone"
payload = "core/distribution/payload.txt"
payload_dest_prefix = "share/solstone-journal"
payload_src_root = "core/payload"
entry = [
    { kind = "pinned-native", targets = ["linux-x86_64"], dest = "lib/solstone-foo/libfoo.so.1", mode = 493, identity = { name = "libfoo.so.1" }, input = { kind = "inline", source = "foo", digest = "d" } }
]
deny = []
[artifact]
basename = "solstone-{version}-{os}-{arch}"
[[target]]
id = "linux-x86_64"
os = "linux"
arch = "x86_64"
deb_arch = "amd64"
rpm_arch = "x86_64"
triple_musl = "x86_64-unknown-linux-musl"
triple_gnu = "x86_64-unknown-linux-gnu"
zig_gnu = "x86_64-linux-gnu.2.28"
"#;
        assert!(toml_edit::de::from_str::<super::Inventory>(toml_inline).is_ok());
    }

    #[test]
    fn component_bad_id_refusals() {
        let make_inv = |entry_str: &str| {
            format!(
                r#"
version = 1
product = "solstone"
payload = "core/distribution/payload.txt"
payload_dest_prefix = "share/solstone-journal"
payload_src_root = "core/payload"
entry = [
    {entry_str}
]
deny = []
[artifact]
basename = "solstone-{{version}}-{{os}}-{{arch}}"
[[target]]
id = "linux-x86_64"
os = "linux"
arch = "x86_64"
deb_arch = "amd64"
rpm_arch = "x86_64"
triple_musl = "x86_64-unknown-linux-musl"
triple_gnu = "x86_64-unknown-linux-gnu"
zig_gnu = "x86_64-linux-gnu.2.28"
"#
            )
        };

        // PinnedNative with component = "Bad Id"
        let t1 = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            t1.path(),
            make_inv(
                r#"{ kind = "pinned-native", component = "Bad Id", dest = "lib/solstone-foo/libfoo.so.1", mode = 493, targets = ["linux-x86_64"], identity = { name = "libfoo.so.1" }, input = { kind = "inline", source = "foo", digest = "d" } }"#,
            ),
        )
        .unwrap();
        let err1 = super::load_inventory(t1.path()).unwrap_err().to_string();
        assert!(
            err1.contains("invalid-component")
                && err1.contains("Bad Id")
                && !err1.starts_with("Bad Id:"),
            "{err1}"
        );

        // PinnedMembers with component = "Bad Id"
        let t2 = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            t2.path(),
            make_inv(
                r#"{ kind = "pinned-members", component = "Bad Id", targets = ["linux-x86_64"], input = { kind = "inline", source = "foo", digest = "d" }, staged = [] }"#,
            ),
        )
        .unwrap();
        let err2 = super::load_inventory(t2.path()).unwrap_err().to_string();
        assert!(
            err2.contains("invalid-component")
                && err2.contains("Bad Id")
                && !err2.starts_with("Bad Id:"),
            "{err2}"
        );

        // LicenceTree with component = "Bad Id"
        let t3 = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            t3.path(),
            make_inv(
                r#"{ kind = "licence-tree", component = "Bad Id", source = "licenses/foo", targets = ["linux-x86_64"] }"#,
            ),
        )
        .unwrap();
        let err3 = super::load_inventory(t3.path()).unwrap_err().to_string();
        assert!(
            err3.contains("invalid-component")
                && err3.contains("Bad Id")
                && !err3.starts_with("Bad Id:"),
            "{err3}"
        );
    }
}
