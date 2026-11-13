// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_journal::resolve_checkout_root_from_executable_dir;

pub trait PackageProbe: Send + Sync {
    fn owns_journal(&self, executable: &Path) -> bool;
    fn journal_version(&self, _executable: &Path) -> Option<String> {
        None
    }
    fn payload_matches(&self, _executable: &Path, _version: &str) -> bool {
        false
    }
    fn package_route(&self, _executable: &Path) -> Option<String> {
        None
    }
    fn installed_package_version(&self, _package_name: &str) -> Option<String> {
        None
    }
    fn has_repo_source(&self, etc_dir: &Path) -> bool {
        has_repo_config_file(etc_dir) || self.installed_package_version("solstone").is_some()
    }
}

pub fn normalize_executable_path(executable: &Path) -> PathBuf {
    if !executable.exists()
        && let Some(raw) = executable.to_str()
        && let Some(stripped) = raw.strip_suffix(" (deleted)")
    {
        let candidate = PathBuf::from(stripped);
        if candidate.exists() {
            return candidate;
        }
    }
    executable.to_path_buf()
}

pub fn normalize_package_semver(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let without_epoch = trimmed.split_once(':').map_or(trimmed, |(_, rest)| rest);
    let core = without_epoch
        .split_once('-')
        .map_or(without_epoch, |(ver, _)| ver);
    crate::compare_semver_3part(core, core).map(|_| core.to_owned())
}

fn is_regular_non_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|meta| !meta.file_type().is_symlink() && meta.file_type().is_file())
}

fn repo_config_paths(etc_dir: &Path) -> [PathBuf; 4] {
    [
        etc_dir.join("apt/sources.list.d/solstone.sources"),
        etc_dir.join("apt/sources.list.d/solstone.list"),
        etc_dir.join("yum.repos.d/solstone.repo"),
        etc_dir.join("zypp/repos.d/solstone.repo"),
    ]
}

fn has_repo_config_file(etc_dir: &Path) -> bool {
    repo_config_paths(etc_dir)
        .iter()
        .any(|path| is_regular_non_symlink(path))
}

fn repo_lane(etc_dir: &Path) -> String {
    for path in repo_config_paths(etc_dir) {
        if is_regular_non_symlink(&path)
            && let Ok(contents) = fs::read_to_string(&path)
        {
            if contents.contains("/apt/staging") || contents.contains("/rpm/staging") {
                return "staging".to_owned();
            }
            if contents.contains("/apt/dev") || contents.contains("/rpm/dev") {
                return "dev".to_owned();
            }
            return "release".to_owned();
        }
    }
    "release".to_owned()
}

pub struct RealPackageProbe;

impl PackageProbe for RealPackageProbe {
    fn journal_version(&self, executable: &Path) -> Option<String> {
        use solstone_core_installed_payload::{code, posix};
        let executable = normalize_executable_path(executable);
        let executable = fs::canonicalize(executable).ok()?;
        let root = posix::bin_parent_root(&executable)?;
        let relative = executable.strip_prefix(&root).ok()?.to_str()?;
        let target = solstone_core_installed_payload::compiled_target();
        let compiled = env!("CARGO_PKG_VERSION");
        match posix::InstalledPackage::admit(&root, compiled, target) {
            Ok(package) => package.member(relative).ok().map(|_| compiled.to_owned()),
            Err(refusal) if refusal.code == code::RESTART_TO_FINISH_UPDATE => {
                let (_, on_disk) = refusal.versions.as_deref()?;
                crate::compare_semver_3part(on_disk, on_disk)?;
                let package = posix::InstalledPackage::admit(&root, on_disk, target).ok()?;
                package.member(relative).ok().map(|_| on_disk.clone())
            }
            Err(_) => None,
        }
    }
    fn payload_matches(&self, executable: &Path, version: &str) -> bool {
        use solstone_core_installed_payload::posix;
        if version != env!("CARGO_PKG_VERSION") {
            return false;
        }
        let executable = normalize_executable_path(executable);
        let Ok(executable) = fs::canonicalize(executable) else {
            return false;
        };
        let Some(root) = posix::bin_parent_root(&executable) else {
            return false;
        };
        // POSIX promotion embeds this manifest in release archives.
        // Ownership checks its identity and executable digest; the installer
        // remains responsible for authenticating release inputs.
        let Ok(package) = posix::InstalledPackage::admit(
            &root,
            version,
            solstone_core_installed_payload::compiled_target(),
        ) else {
            return false;
        };
        let Ok(relative) = executable.strip_prefix(&root) else {
            return false;
        };
        relative
            .to_str()
            .is_some_and(|path| package.member(path).is_ok())
    }

    fn owns_journal(&self, executable: &Path) -> bool {
        self.package_route(executable).is_some()
    }

    fn package_route(&self, executable: &Path) -> Option<String> {
        let executable = normalize_executable_path(executable);
        if let Ok(output) = std::process::Command::new("/usr/bin/dpkg-query")
            .arg("-S")
            .arg(&executable)
            .output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.lines().any(|line| {
                line.split_once(": ").is_some_and(|(package, path)| {
                    (package == "solstone-journal"
                        || package
                            .split_once(':')
                            .is_some_and(|(name, _)| name == "solstone-journal"))
                        && Path::new(path) == executable
                })
            }) {
                return Some("deb".to_owned());
            }
        }
        if let Ok(output) = std::process::Command::new("/usr/bin/rpm")
            .args(["-qf", "--queryformat", "%{NAME}\n"])
            .arg(&executable)
            .output()
            && output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == "solstone-journal"
        {
            return Some("rpm".to_owned());
        }
        None
    }

    fn installed_package_version(&self, package_name: &str) -> Option<String> {
        if let Ok(output) = std::process::Command::new("/usr/bin/dpkg-query")
            .args(["-W", "-f=${Status}\t${Version}\n", package_name])
            .output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if let Some((status, ver)) = line.split_once('\t')
                    && status.trim() == "install ok installed"
                    && let Some(clean) = normalize_package_semver(ver)
                {
                    return Some(clean);
                }
            }
        }
        if let Ok(output) = std::process::Command::new("/usr/bin/rpm")
            .args(["-q", "--queryformat", "%{VERSION}\n", package_name])
            .output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(first) = text.lines().next()
                && let Some(clean) = normalize_package_semver(first)
            {
                return Some(clean);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    PlatformTree,
    PlatformPackage,
    PlatformMixed,
    NativeTree,
}

impl RouteKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PlatformTree => "platform-tree",
            Self::PlatformPackage => "platform-package",
            Self::PlatformMixed => "platform-mixed",
            Self::NativeTree => "native-tree",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServicePolicy {
    Start,
    NoStart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedComponent {
    pub name: String,
    pub route: String,
    pub policy: ServicePolicy,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositiveOwnership {
    pub route_kind: RouteKind,
    /// Where the journal's tree lives, when the journal is a tree install.
    pub prefix: Option<PathBuf>,
    /// The platform installer's own prefix for its tree components. It differs
    /// from `prefix` for a journal the platform took over from the journal's
    /// own installer, which stays where it was installed.
    pub platform_prefix: Option<PathBuf>,
    pub lane: String,
    pub current_journal_version: String,
    pub current_platform_version: Option<String>,
    pub components: Vec<RecordedComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipResult {
    SourceCheckout,
    Positive(PositiveOwnership),
    NotOwned,
    Refusal { code: String, message: String },
}

#[derive(Debug, Default)]
struct IniFile {
    sections: BTreeMap<String, BTreeMap<String, String>>,
}

fn parse_ini_file(path: &Path) -> Result<IniFile, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut sections = BTreeMap::new();
    let mut current_section: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix('[') {
            let section_name = rest
                .strip_suffix(']')
                .ok_or_else(|| "unclosed section header".to_owned())?
                .trim();
            if sections.contains_key(section_name) {
                return Err(format!("duplicate section [{section_name}]"));
            }
            sections.insert(section_name.to_owned(), BTreeMap::new());
            current_section = Some(section_name.to_owned());
        } else if let Some((key, val)) = trimmed.split_once('=') {
            let sec_name = current_section
                .as_ref()
                .ok_or_else(|| "key outside section".to_owned())?;
            let sec_map = sections.get_mut(sec_name).expect("section exists");
            let k = key.trim().to_owned();
            let v = val.trim().to_owned();
            if sec_map.contains_key(&k) {
                return Err(format!("duplicate key {k} in section [{sec_name}]"));
            }
            sec_map.insert(k, v);
        } else {
            return Err(format!("malformed line: {trimmed}"));
        }
    }

    Ok(IniFile { sections })
}

pub fn positive_tree_release(prefix: &Path) -> Option<(String, PathBuf)> {
    let versions_dir = prefix.join("versions");
    let versions_meta = fs::symlink_metadata(&versions_dir).ok()?;
    if !versions_meta.is_dir() || versions_meta.file_type().is_symlink() {
        return None;
    }

    let current_link = prefix.join("current");
    let current_meta = fs::symlink_metadata(&current_link).ok()?;
    if !current_meta.file_type().is_symlink() {
        return None;
    }

    let link_target = fs::read_link(&current_link).ok()?;
    let target_path = if link_target.is_absolute() {
        link_target
    } else {
        prefix.join(link_target)
    };

    let target_path = fs::canonicalize(target_path).ok()?;
    if target_path.parent()? != fs::canonicalize(&versions_dir).ok()? {
        return None;
    }
    let dest_meta = fs::symlink_metadata(&target_path).ok()?;
    if !dest_meta.is_dir() || dest_meta.file_type().is_symlink() {
        return None;
    }

    let entry_name = target_path.file_name()?.to_str()?;
    if entry_name.is_empty() || entry_name == "." || entry_name == ".." {
        return None;
    }

    let release_file = target_path.join(".release");
    let release_meta = fs::symlink_metadata(&release_file).ok()?;
    if !release_meta.file_type().is_file() || release_meta.file_type().is_symlink() {
        return None;
    }

    let sha_file = target_path.join(".archive-sha256");
    let sha_meta = fs::symlink_metadata(&sha_file).ok()?;
    if !sha_meta.file_type().is_file() || sha_meta.file_type().is_symlink() {
        return None;
    }

    let recorded_digest = fs::read_to_string(&sha_file).ok()?.trim().to_owned();
    if recorded_digest.len() != 64 || !recorded_digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let digest12 = &recorded_digest[..12];
    let suffix = format!("-{digest12}");
    let version_part = entry_name.strip_suffix(&suffix)?;

    let release_content = fs::read_to_string(&release_file).ok()?;
    let mut release_version = None;
    for line in release_content.lines() {
        if let Some((k, v)) = line.split_once('=')
            && k.trim() == "version"
        {
            release_version = Some(v.trim().to_owned());
        }
    }

    let version = release_version?;
    if version != version_part {
        return None;
    }

    Some((version, target_path))
}

fn executable_matches_prefix(executable: &Path, prefix: &Path) -> bool {
    let Some((_, dest)) = positive_tree_release(prefix) else {
        return false;
    };
    let Ok(executable) = fs::canonicalize(executable) else {
        return false;
    };
    executable.parent() == Some(dest.join("bin").as_path())
}

pub(crate) fn parse_install_receipt(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut map = BTreeMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_owned();
            let val = v.trim().to_owned();
            if map.contains_key(&key) {
                return Err(format!("duplicate key {key}"));
            }
            map.insert(key, val);
        } else {
            return Err("not key=value".to_owned());
        }
    }
    Ok(map)
}

fn normalize_component_section(sec_name: &str) -> Option<&str> {
    let comp = sec_name.strip_prefix("component:")?;
    if ["journal", "cli", "desktop", "tmux"].contains(&comp) {
        Some(comp)
    } else {
        None
    }
}

pub fn classify_ownership(
    executable: &Path,
    data_dir: &Path,
    etc_dir: &Path,
    probe: &dyn PackageProbe,
) -> OwnershipResult {
    let normalized = normalize_executable_path(executable);
    let executable = normalized.as_path();

    // 1. Ancestry checkout detection
    let exe_dir = executable.parent().unwrap_or(executable);
    if resolve_checkout_root_from_executable_dir(exe_dir).is_some() {
        return OwnershipResult::SourceCheckout;
    }

    let tree_receipt_path = data_dir.join("install.conf");
    let pkg_receipt_path = etc_dir.join("solstone/install.conf");

    // Symlink checks on receipts
    if let Ok(meta) = fs::symlink_metadata(&tree_receipt_path)
        && (meta.file_type().is_symlink() || !meta.file_type().is_file())
    {
        return OwnershipResult::Refusal {
            code: "ownership-unknown".to_owned(),
            message: "user install.conf is not a regular file".to_owned(),
        };
    }

    if let Ok(meta) = fs::symlink_metadata(&pkg_receipt_path)
        && (meta.file_type().is_symlink() || !meta.file_type().is_file())
    {
        return OwnershipResult::Refusal {
            code: "ownership-unknown".to_owned(),
            message: "package install.conf is not a regular file".to_owned(),
        };
    }

    let tree_ini = if tree_receipt_path.is_file() {
        match parse_ini_file(&tree_receipt_path) {
            Ok(ini) => Some(ini),
            Err(e) => {
                return OwnershipResult::Refusal {
                    code: "ownership-unknown".to_owned(),
                    message: format!("malformed user install.conf: {e}"),
                };
            }
        }
    } else {
        None
    };

    let pkg_ini = if pkg_receipt_path.is_file() {
        match parse_ini_file(&pkg_receipt_path) {
            Ok(ini) => Some(ini),
            Err(e) => {
                return OwnershipResult::Refusal {
                    code: "ownership-unknown".to_owned(),
                    message: format!("malformed package install.conf: {e}"),
                };
            }
        }
    } else {
        None
    };

    // Validate allowlisted section names
    let check_sections = |ini: &IniFile| -> Option<OwnershipResult> {
        for sec in ini.sections.keys() {
            if sec == "solstone" {
                continue;
            }
            if normalize_component_section(sec).is_none() {
                return Some(OwnershipResult::Refusal {
                    code: "ownership-unknown".to_owned(),
                    message: format!("unrecognized section in install.conf: {sec}"),
                });
            }
        }
        None
    };

    if let Some(ref ini) = tree_ini
        && let Some(r) = check_sections(ini)
    {
        return r;
    }
    if let Some(ref ini) = pkg_ini
        && let Some(r) = check_sections(ini)
    {
        return r;
    }

    for ini in [&tree_ini, &pkg_ini].into_iter().flatten() {
        let valid = ini.sections.get("solstone").is_some_and(|header| {
            header.get("schema_version").map(String::as_str) == Some("1")
                && header.get("origin").map(String::as_str) == Some(crate::fetch::UPDATE_ORIGIN)
                && header
                    .get("lane")
                    .is_some_and(|lane| ["release", "staging", "dev"].contains(&lane.as_str()))
                && header
                    .get("platform_version")
                    .is_some_and(|v| crate::compare_platform_versions(v, v).is_some())
        });
        if !valid {
            return OwnershipResult::Refusal { code: "ownership-unknown".to_owned(), message: "the installation receipt is incomplete or unsupported. use the original installer to recover it.".to_owned() };
        }
    }

    for ini in [&tree_ini, &pkg_ini].into_iter().flatten() {
        for (name, section) in &ini.sections {
            if name == "solstone" {
                continue;
            }
            let valid = section.get("status").map(String::as_str) == Some("installed")
                && match section.get("route").map(String::as_str) {
                    Some("deb" | "rpm") => {
                        section.get("phase").map(String::as_str) == Some("complete")
                            && matches!(
                                section.get("service_policy").map(String::as_str),
                                Some("start" | "skip-service")
                            )
                    }
                    Some("tree") => true,
                    _ => false,
                };
            if !valid {
                return OwnershipResult::Refusal { code: "ownership-unknown".to_owned(), message: "the installation receipt is incomplete or unsupported. use the original installer to recover it.".to_owned() };
            }
        }
    }

    // Check for ambiguous journal & cli in same receipt or across receipts
    let has_tree_journal = tree_ini
        .as_ref()
        .is_some_and(|i| i.sections.contains_key("component:journal"));
    let has_tree_cli = tree_ini
        .as_ref()
        .is_some_and(|i| i.sections.contains_key("component:cli"));

    let has_pkg_journal = pkg_ini
        .as_ref()
        .is_some_and(|i| i.sections.contains_key("component:journal"));
    let has_pkg_cli = pkg_ini
        .as_ref()
        .is_some_and(|i| i.sections.contains_key("component:cli"));

    let any_journal = has_tree_journal || has_pkg_journal;
    let any_cli = has_tree_cli || has_pkg_cli;

    if any_journal && any_cli {
        return OwnershipResult::Refusal {
            code: "source-ambiguous".to_owned(),
            message: "both journal and cli components are recorded".to_owned(),
        };
    }

    // Double claim of the same component across receipts
    if let (Some(t), Some(p)) = (&tree_ini, &pkg_ini) {
        for comp in ["journal", "cli", "desktop", "tmux"] {
            let t_has = t.sections.contains_key(&format!("component:{comp}"))
                || t.sections.contains_key(comp);
            let p_has = p.sections.contains_key(&format!("component:{comp}"))
                || p.sections.contains_key(comp);
            if t_has && p_has {
                return OwnershipResult::Refusal {
                    code: "ownership-unknown".to_owned(),
                    message: format!("component {comp} claimed by both tree and package receipts"),
                };
            }
        }
    }

    if let (Some(tree), Some(package)) = (&tree_ini, &pkg_ini) {
        let tree = &tree.sections["solstone"];
        let package = &package.sections["solstone"];
        if ["lane", "platform_version"]
            .iter()
            .any(|key| tree.get(*key) != package.get(*key))
        {
            return OwnershipResult::Refusal { code: "ownership-unknown".to_owned(), message: "the installation receipt is incomplete or unsupported. use the original installer to recover it.".to_owned() };
        }
    }

    // Check positive platform package
    let pkg_ownership = classify_platform_package(executable, pkg_ini.as_ref(), probe);

    // Check positive platform tree
    let tree_ownership = classify_platform_tree(executable, tree_ini.as_ref(), probe);

    // If both tree and package components exist -> platform-mixed
    if let (Some(t_res), Some(p_res)) = (tree_ownership.as_ref(), pkg_ownership.as_ref()) {
        let mut mixed_components = t_res.components.clone();
        mixed_components.extend(p_res.components.clone());

        let journal_ver = if !t_res.current_journal_version.is_empty() {
            t_res.current_journal_version.clone()
        } else {
            p_res.current_journal_version.clone()
        };

        let plat_ver = t_res
            .current_platform_version
            .clone()
            .or_else(|| p_res.current_platform_version.clone());

        return OwnershipResult::Positive(PositiveOwnership {
            route_kind: RouteKind::PlatformMixed,
            prefix: t_res.prefix.clone(),
            platform_prefix: t_res.platform_prefix.clone(),
            lane: t_res.lane.clone(),
            current_journal_version: journal_ver,
            current_platform_version: plat_ver,
            components: mixed_components,
        });
    }

    if let Some(t_res) = tree_ownership {
        // Check if there are other components in pkg_ini
        if let Some(ref p) = pkg_ini {
            let mut other_pkg_comps = Vec::new();
            for comp in ["desktop", "tmux"] {
                if let Some(sec) = p.sections.get(&format!("component:{comp}"))
                    && sec.get("status").map(|s| s.as_str()) == Some("installed")
                {
                    let policy = if sec.get("service_policy").map(|s| s.as_str()) == Some("start") {
                        ServicePolicy::Start
                    } else {
                        ServicePolicy::NoStart
                    };
                    other_pkg_comps.push(RecordedComponent {
                        name: comp.to_owned(),
                        route: sec
                            .get("route")
                            .cloned()
                            .unwrap_or_else(|| "deb".to_owned()),
                        policy,
                        version: sec.get("package_version").cloned(),
                    });
                }
            }
            if !other_pkg_comps.is_empty() {
                let mut comps = t_res.components;
                comps.extend(other_pkg_comps);
                return OwnershipResult::Positive(PositiveOwnership {
                    route_kind: RouteKind::PlatformMixed,
                    prefix: t_res.prefix,
                    platform_prefix: t_res.platform_prefix,
                    lane: t_res.lane,
                    current_journal_version: t_res.current_journal_version,
                    current_platform_version: t_res.current_platform_version,
                    components: comps,
                });
            }
        }

        return OwnershipResult::Positive(t_res);
    }

    if let Some(p_res) = pkg_ownership {
        // Check if there are other components in tree_ini
        if let Some(ref t) = tree_ini {
            let mut other_tree_comps = Vec::new();
            let prefix = t
                .sections
                .get("solstone")
                .and_then(|s| s.get("prefix"))
                .map(PathBuf::from);
            for comp in ["desktop", "tmux"] {
                if let Some(sec) = t.sections.get(&format!("component:{comp}"))
                    && sec.get("status").map(|s| s.as_str()) == Some("installed")
                {
                    let policy = if sec.get("no_start").map(|s| s.as_str()) == Some("1") {
                        ServicePolicy::NoStart
                    } else {
                        ServicePolicy::Start
                    };
                    other_tree_comps.push(RecordedComponent {
                        name: comp.to_owned(),
                        route: "tree".to_owned(),
                        policy,
                        version: sec.get("version").cloned(),
                    });
                }
            }
            if !other_tree_comps.is_empty() {
                let mut comps = p_res.components;
                comps.extend(other_tree_comps);
                return OwnershipResult::Positive(PositiveOwnership {
                    route_kind: RouteKind::PlatformMixed,
                    prefix: prefix.clone(),
                    platform_prefix: prefix,
                    lane: p_res.lane,
                    current_journal_version: p_res.current_journal_version,
                    current_platform_version: p_res.current_platform_version,
                    components: comps,
                });
            }
        }

        return OwnershipResult::Positive(p_res);
    }

    // Check positive standalone native-tree
    if let Some(native_res) = classify_native_tree(executable, tree_ini.as_ref(), probe) {
        return native_res;
    }

    if tree_ini.is_none()
        && pkg_ini.is_none()
        && probe.has_repo_source(etc_dir)
        && let Some(repo_res) = classify_repo_package(executable, etc_dir, probe)
    {
        return OwnershipResult::Positive(repo_res);
    }

    // A package installed without the solstone installer has no receipt. The
    // installer checks its files against the signed release and records it.
    if probe.owns_journal(executable) {
        return OwnershipResult::Refusal {
            code: "install-not-owned".to_owned(),
            message: UNRECORDED_PACKAGE_MESSAGE.to_owned(),
        };
    }

    OwnershipResult::NotOwned
}

/// What `solstone update` says for a package the solstone installer has not
/// recorded: the one command that records it.
pub const UNRECORDED_PACKAGE_MESSAGE: &str = "this journal was installed from a package without the solstone installer, so solstone update can't update it yet. run curl -fsSL https://solstone.app/install.sh | sh -s -- --upgrade once. it checks the installed files against the signed release, and solstone update updates it after that.";

fn is_valid_package_executable(executable: &Path) -> bool {
    let canon_exe = fs::canonicalize(executable).ok();
    let valid_path = canon_exe.as_deref() == Some(Path::new("/usr/bin/solstone"))
        || canon_exe.as_deref() == Some(Path::new("/usr/bin/journal"))
        || canon_exe.as_deref() == Some(Path::new("/bin/solstone"))
        || canon_exe.as_deref() == Some(Path::new("/bin/journal"))
        || executable == Path::new("/usr/bin/solstone")
        || executable == Path::new("/usr/bin/journal")
        || executable == Path::new("/bin/solstone")
        || executable == Path::new("/bin/journal");

    valid_path || executable.parent() == Some(Path::new("/usr/bin"))
}

fn classify_repo_package(
    executable: &Path,
    etc_dir: &Path,
    probe: &dyn PackageProbe,
) -> Option<PositiveOwnership> {
    if !is_valid_package_executable(executable) || !probe.owns_journal(executable) {
        return None;
    }
    let journal_ver = probe.journal_version(executable)?;
    let route = probe
        .package_route(executable)
        .unwrap_or_else(|| "deb".to_owned());
    let lane = repo_lane(etc_dir);

    let mut components = vec![RecordedComponent {
        name: "journal".to_owned(),
        route: route.clone(),
        policy: ServicePolicy::Start,
        version: Some(journal_ver.clone()),
    }];

    if let Some(desktop_ver) = probe.installed_package_version("solstone-linux") {
        components.push(RecordedComponent {
            name: "desktop".to_owned(),
            route,
            policy: ServicePolicy::NoStart,
            version: Some(desktop_ver),
        });
    }

    Some(PositiveOwnership {
        route_kind: RouteKind::PlatformPackage,
        prefix: None,
        platform_prefix: None,
        lane,
        current_journal_version: journal_ver,
        current_platform_version: None,
        components,
    })
}

fn classify_platform_package(
    executable: &Path,
    pkg_ini: Option<&IniFile>,
    probe: &dyn PackageProbe,
) -> Option<PositiveOwnership> {
    let ini = pkg_ini?;
    let journal_sec = ini
        .sections
        .get("component:journal")
        .or_else(|| ini.sections.get("component:cli"))?;

    let route = journal_sec.get("route")?;
    if route != "deb" && route != "rpm" {
        return None;
    }
    if journal_sec.get("phase").map(|s| s.as_str()) != Some("complete") {
        return None;
    }
    if journal_sec.get("status").map(|s| s.as_str()) != Some("installed") {
        return None;
    }
    if journal_sec.get("package_name").map(|s| s.as_str()) != Some("solstone-journal") {
        return None;
    }

    if !is_valid_package_executable(executable) {
        return None;
    }

    if !probe.owns_journal(executable) {
        return None;
    }

    let lane = ini
        .sections
        .get("solstone")
        .and_then(|s| s.get("lane"))
        .cloned()
        .unwrap_or_else(|| "release".to_owned());

    let recorded_plat_ver = ini
        .sections
        .get("solstone")
        .and_then(|s| s.get("platform_version"))
        .cloned();

    let journal_ver = probe.journal_version(executable)?;
    let live_desktop = probe.installed_package_version("solstone-linux");

    let recorded_journal_ver = journal_sec
        .get("package_version")
        .map(String::as_str)
        .or_else(|| {
            recorded_plat_ver
                .as_deref()
                .map(|v| v.rsplit_once("-r").map_or(v, |(core, _)| core))
        });
    let journal_advanced = recorded_journal_ver.is_some_and(|rec| rec != journal_ver);
    let desktop_advanced = ini
        .sections
        .get("component:desktop")
        .and_then(|s| s.get("package_version"))
        .zip(live_desktop.as_ref())
        .is_some_and(|(rec, live)| rec != live);
    let plat_ver = if journal_advanced || desktop_advanced {
        None
    } else {
        recorded_plat_ver
    };

    let policy = match journal_sec.get("service_policy").map(|s| s.as_str()) {
        Some("start") => ServicePolicy::Start,
        _ => ServicePolicy::NoStart,
    };

    let comp_name = if ini.sections.contains_key("component:cli") {
        "cli"
    } else {
        "journal"
    };

    let mut components = vec![RecordedComponent {
        name: comp_name.to_owned(),
        route: route.clone(),
        policy,
        version: Some(journal_ver.clone()),
    }];

    for comp in ["desktop", "tmux"] {
        if let Some(sec) = ini.sections.get(&format!("component:{comp}"))
            && sec.get("status").map(|s| s.as_str()) == Some("installed")
        {
            let comp_policy = match sec.get("service_policy").map(|s| s.as_str()) {
                Some("start") => ServicePolicy::Start,
                _ => ServicePolicy::NoStart,
            };
            let version = if comp == "desktop" {
                live_desktop
                    .clone()
                    .or_else(|| sec.get("package_version").cloned())
            } else {
                sec.get("package_version").cloned()
            };
            components.push(RecordedComponent {
                name: comp.to_owned(),
                route: sec.get("route").cloned().unwrap_or_else(|| route.clone()),
                policy: comp_policy,
                version,
            });
        }
    }

    if !ini.sections.contains_key("component:desktop")
        && let Some(desktop_ver) = live_desktop
    {
        components.push(RecordedComponent {
            name: "desktop".to_owned(),
            route: route.clone(),
            policy: ServicePolicy::NoStart,
            version: Some(desktop_ver),
        });
    }

    Some(PositiveOwnership {
        route_kind: RouteKind::PlatformPackage,
        prefix: None,
        platform_prefix: None,
        lane,
        current_journal_version: journal_ver,
        current_platform_version: plat_ver,
        components,
    })
}

fn classify_platform_tree(
    executable: &Path,
    tree_ini: Option<&IniFile>,
    probe: &dyn PackageProbe,
) -> Option<PositiveOwnership> {
    let ini = tree_ini?;
    let sol_sec = ini.sections.get("solstone")?;

    if sol_sec.get("route").map(|s| s.as_str()) != Some("tree") {
        return None;
    }
    if sol_sec.get("phase").map(|s| s.as_str()) != Some("complete") {
        return None;
    }

    let platform_prefix = PathBuf::from(sol_sec.get("prefix")?);
    if !platform_prefix.is_absolute() {
        return None;
    }

    let journal_sec = ini
        .sections
        .get("component:journal")
        .or_else(|| ini.sections.get("component:cli"))?;

    if journal_sec.get("route").map(|s| s.as_str()) != Some("tree") {
        return None;
    }
    if journal_sec.get("status").map(|s| s.as_str()) != Some("installed") {
        return None;
    }
    // A journal the platform took over from the journal's own installer keeps
    // the prefix it was installed in, recorded on its own section.
    let prefix = journal_sec
        .get("prefix")
        .map(PathBuf::from)
        .unwrap_or_else(|| platform_prefix.clone());
    if !prefix.is_absolute() {
        return None;
    }

    let (journal_ver, _dest) = positive_tree_release(&prefix)?;

    if !executable_matches_prefix(executable, &prefix)
        || !probe.payload_matches(executable, &journal_ver)
    {
        return None;
    }

    if probe.owns_journal(executable) {
        return None;
    }

    let lane = sol_sec
        .get("lane")
        .cloned()
        .unwrap_or_else(|| "release".to_owned());

    let plat_ver = sol_sec.get("platform_version").cloned();

    let comp_name = if ini.sections.contains_key("component:cli") {
        "cli"
    } else {
        "journal"
    };

    let native_receipt = parse_install_receipt(&prefix.join("install-receipt")).ok()?;
    if native_receipt
        .get("signature_verification")
        .map(String::as_str)
        != Some("verified")
    {
        return None;
    }
    if native_receipt.get("role").map(String::as_str) != Some(comp_name) {
        return None;
    }
    let policy = native_tree_policy(&native_receipt)?;

    let mut components = vec![RecordedComponent {
        name: comp_name.to_owned(),
        route: "tree".to_owned(),
        policy,
        version: Some(journal_ver.clone()),
    }];

    for comp in ["desktop", "tmux"] {
        if let Some(sec) = ini.sections.get(&format!("component:{comp}"))
            && sec.get("status").map(|s| s.as_str()) == Some("installed")
        {
            let comp_policy = if sec.get("no_start").map(|s| s.as_str()) == Some("1") {
                ServicePolicy::NoStart
            } else {
                ServicePolicy::Start
            };
            components.push(RecordedComponent {
                name: comp.to_owned(),
                route: "tree".to_owned(),
                policy: comp_policy,
                version: sec.get("version").cloned(),
            });
        }
    }

    Some(PositiveOwnership {
        route_kind: RouteKind::PlatformTree,
        prefix: Some(prefix),
        platform_prefix: Some(platform_prefix),
        lane,
        current_journal_version: journal_ver,
        current_platform_version: plat_ver,
        components,
    })
}

// Native tree receipts and platform package receipts have different policy
// vocabularies. Preserve the native role/policy pairs written by install.sh.
fn native_tree_policy(receipt: &BTreeMap<String, String>) -> Option<ServicePolicy> {
    match (
        receipt.get("role").map(String::as_str),
        receipt.get("service_policy").map(String::as_str),
    ) {
        (Some("journal"), Some("start")) => Some(ServicePolicy::Start),
        (Some("journal"), Some("no-start")) | (Some("cli"), Some("none")) => {
            Some(ServicePolicy::NoStart)
        }
        _ => None,
    }
}

fn classify_native_tree(
    executable: &Path,
    tree_ini: Option<&IniFile>,
    probe: &dyn PackageProbe,
) -> Option<OwnershipResult> {
    if let Some(ini) = tree_ini
        && (ini.sections.contains_key("component:journal")
            || ini.sections.contains_key("component:cli"))
    {
        return None;
    }

    let mut prefix_cand = None;
    for anc in executable.ancestors() {
        if anc.file_name().map(|n| n == "bin").unwrap_or(false)
            && let Some(parent) = anc.parent()
        {
            if parent.file_name().map(|n| n == "current").unwrap_or(false) {
                prefix_cand = parent.parent();
                break;
            } else if parent
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n == "versions")
                .unwrap_or(false)
            {
                prefix_cand = parent.parent().and_then(|p| p.parent());
                break;
            }
        }
    }

    let prefix = prefix_cand?;
    let receipt_path = prefix.join("install-receipt");
    if let Ok(meta) = fs::symlink_metadata(&receipt_path) {
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            return Some(OwnershipResult::Refusal {
                code: "ownership-unknown".to_owned(),
                message: "native install-receipt is not a regular file".to_owned(),
            });
        }
    } else {
        return None;
    }

    let receipt = match parse_install_receipt(&receipt_path) {
        Ok(r) => r,
        Err(e) => {
            return Some(OwnershipResult::Refusal {
                code: "ownership-unknown".to_owned(),
                message: format!("malformed native install-receipt: {e}"),
            });
        }
    };

    if receipt.get("schema_version").map(|s| s.as_str()) != Some("1") {
        return Some(OwnershipResult::Refusal {
            code: "ownership-unknown".to_owned(),
            message: "native receipt schema_version must be 1".to_owned(),
        });
    }

    if receipt.get("route").map(|s| s.as_str()) != Some("tree") {
        return Some(OwnershipResult::Refusal {
            code: "ownership-unknown".to_owned(),
            message: "native receipt route must be tree".to_owned(),
        });
    }

    let (tree_ver, _dest) = positive_tree_release(prefix)?;
    if !executable_matches_prefix(executable, prefix)
        || !probe.payload_matches(executable, &tree_ver)
        || receipt.get("signature_verification").map(String::as_str) != Some("verified")
    {
        return None;
    }
    let receipt_ver = receipt.get("journal_version")?;
    if &tree_ver != receipt_ver {
        return Some(OwnershipResult::Refusal {
            code: "ownership-unknown".to_owned(),
            message: "installed version does not match receipt journal_version".to_owned(),
        });
    }

    if receipt.get("origin").map(String::as_str) != Some(crate::fetch::UPDATE_ORIGIN) {
        return None;
    }
    let lane = receipt.get("lane")?.clone();
    if !["release", "staging", "dev"].contains(&lane.as_str()) {
        return None;
    }

    let policy = native_tree_policy(&receipt)?;

    let role = receipt.get("role")?.clone();
    if !["journal", "cli"].contains(&role.as_str()) {
        return None;
    }

    Some(OwnershipResult::Positive(PositiveOwnership {
        route_kind: RouteKind::NativeTree,
        prefix: Some(prefix.to_path_buf()),
        platform_prefix: None,
        lane,
        current_journal_version: tree_ver,
        current_platform_version: None,
        components: vec![RecordedComponent {
            name: role,
            route: "tree".to_owned(),
            policy,
            version: Some(receipt_ver.clone()),
        }],
    }))
}

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod tests {
    use super::*;

    struct MockPackageProbe(bool);
    impl PackageProbe for MockPackageProbe {
        fn journal_version(&self, _executable: &Path) -> Option<String> {
            Some("2.0.48".to_owned())
        }
        fn payload_matches(&self, _executable: &Path, _version: &str) -> bool {
            true
        }
        fn owns_journal(&self, _executable: &Path) -> bool {
            self.0
        }
    }

    fn make_valid_tree(prefix: &Path, ver: &str) {
        let digest64 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let digest12 = &digest64[..12];
        let entry = format!("{ver}-{digest12}");
        let ver_dir = prefix.join("versions").join(&entry);
        fs::create_dir_all(ver_dir.join("bin")).unwrap();

        fs::write(ver_dir.join(".archive-sha256"), digest64).unwrap();
        fs::write(prefix.join("install-receipt"), format!("schema_version=1\norigin=https://updates.solstone.app\nroute=tree\njournal_version={ver}\nlane=release\nrole=journal\nservice_policy=start\njournal_state=present\nsignature_verification=verified\n")).unwrap();
        let release_text = format!(
            "product=solstone-journal\nversion={ver}\ntarget=linux-x86_64\ncommit=abc\nlock_sha256=123\nupgrade_epoch=2026-08-epoch\nretention_window=3\nstate_reader_min=1.0.0\nstate_reader_max={ver}\n"
        );
        fs::write(ver_dir.join(".release"), release_text).unwrap();

        fs::write(ver_dir.join("bin/solstone"), b"#!/bin/sh\n").unwrap();

        #[cfg(unix)]
        {
            let current = prefix.join("current");
            std::os::unix::fs::symlink(format!("versions/{entry}"), &current).unwrap();
        }
    }

    #[test]
    fn native_receipt_role_policy_mismatches_refuse_ownership() {
        for (role, policy) in [
            ("cli", "start"),
            ("journal", "none"),
            ("journal", "skip-service"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let prefix = temp.path().join("local");
            make_valid_tree(&prefix, "2.0.48");
            let receipt = prefix.join("install-receipt");
            let contents = fs::read_to_string(&receipt)
                .unwrap()
                .replace("role=journal\n", &format!("role={role}\n"))
                .replace(
                    "service_policy=start\n",
                    &format!("service_policy={policy}\n"),
                );
            fs::write(receipt, contents).unwrap();
            let res = classify_ownership(
                &prefix.join("current/bin/solstone"),
                &temp.path().join("data"),
                &temp.path().join("etc"),
                &MockPackageProbe(false),
            );
            assert!(
                !matches!(res, OwnershipResult::Positive(_)),
                "{role}/{policy}"
            );
        }
    }

    #[test]
    fn ownership_identifies_source_checkout_ancestry() {
        // Use repo root or current executable dir inside checkout
        let exe =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../core/target/debug/solstone");
        let data = tempfile::tempdir().unwrap();
        let etc = tempfile::tempdir().unwrap();
        let probe = MockPackageProbe(false);

        let res = classify_ownership(&exe, data.path(), etc.path(), &probe);
        assert_eq!(res, OwnershipResult::SourceCheckout);
    }

    #[test]
    fn ownership_positively_classifies_platform_tree() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_valid_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();

        let etc = temp.path().join("etc");
        fs::create_dir_all(&etc).unwrap();

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let res = classify_ownership(&exe, &data, &etc, &probe);
        match res {
            OwnershipResult::Positive(pos) => {
                assert_eq!(pos.route_kind, RouteKind::PlatformTree);
                assert_eq!(pos.current_journal_version, "2.0.48");
                assert_eq!(pos.current_platform_version.as_deref(), Some("2.0.48-r1"));
                assert_eq!(pos.components.len(), 1);
                assert_eq!(pos.components[0].name, "journal");
                assert_eq!(pos.components[0].policy, ServicePolicy::Start);
            }
            other => panic!("expected positive platform tree, got {other:?}"),
        }
    }

    #[test]
    fn ownership_positively_classifies_platform_package() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();

        let etc = temp.path().join("etc");
        let etc_sol = etc.join("solstone");
        fs::create_dir_all(&etc_sol).unwrap();

        let ini = "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=deb\nphase=complete\nstatus=installed\npackage_name=solstone-journal\npackage_version=2.0.48\nservice_policy=start\n";
        fs::write(etc_sol.join("install.conf"), ini).unwrap();

        let exe = PathBuf::from("/usr/bin/solstone");
        let probe = MockPackageProbe(true);

        let res = classify_ownership(&exe, &data, &etc, &probe);
        match res {
            OwnershipResult::Positive(pos) => {
                assert_eq!(pos.route_kind, RouteKind::PlatformPackage);
                assert_eq!(pos.current_journal_version, "2.0.48");
                assert_eq!(pos.current_platform_version.as_deref(), Some("2.0.48-r1"));
                assert_eq!(pos.components[0].route, "deb");
                assert_eq!(pos.components[0].policy, ServicePolicy::Start);
            }
            other => panic!("expected positive platform package, got {other:?}"),
        }
    }

    #[test]
    fn ownership_positively_classifies_platform_mixed() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_valid_tree(&prefix, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let tree_ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix.display()
        );
        fs::write(data.join("install.conf"), tree_ini).unwrap();

        let etc = temp.path().join("etc");
        let etc_sol = etc.join("solstone");
        fs::create_dir_all(&etc_sol).unwrap();
        let pkg_ini = "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nlane=release\nplatform_version=2.0.48-r1\n\n[component:desktop]\nrole=desktop\nroute=deb\nphase=complete\nstatus=installed\npackage_name=solstone-desktop\npackage_version=2.0.48\nservice_policy=skip-service\n";
        fs::write(etc_sol.join("install.conf"), pkg_ini).unwrap();

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let res = classify_ownership(&exe, &data, &etc, &probe);
        match res {
            OwnershipResult::Positive(pos) => {
                assert_eq!(pos.route_kind, RouteKind::PlatformMixed);
                assert_eq!(pos.components.len(), 2);
                assert_eq!(pos.components[0].name, "journal");
                assert_eq!(pos.components[1].name, "desktop");
            }
            other => panic!("expected positive platform mixed, got {other:?}"),
        }
    }

    #[test]
    fn ownership_positively_classifies_standalone_native_tree() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("local");
        make_valid_tree(&prefix, "2.0.48");

        let receipt = "schema_version=1\norigin=https://updates.solstone.app\njournal_version=2.0.48\nlane=release\nroute=tree\nrole=journal\nservice_policy=start\nsignature_verification=verified\n";
        fs::write(prefix.join("install-receipt"), receipt).unwrap();

        let data = temp.path().join("data");
        let etc = temp.path().join("etc");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&etc).unwrap();

        let exe = prefix.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let res = classify_ownership(&exe, &data, &etc, &probe);
        match res {
            OwnershipResult::Positive(pos) => {
                assert_eq!(pos.route_kind, RouteKind::NativeTree);
                assert_eq!(pos.current_journal_version, "2.0.48");
                assert_eq!(pos.current_platform_version, None);
            }
            other => panic!("expected native tree, got {other:?}"),
        }
    }

    #[test]
    fn ownership_refuses_when_receipt_is_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();

        let etc = temp.path().join("etc");
        fs::create_dir_all(&etc).unwrap();

        let target = temp.path().join("other.conf");
        fs::write(&target, b"[solstone]\n").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, data.join("install.conf")).unwrap();
            let exe = PathBuf::from("/usr/bin/solstone");
            let probe = MockPackageProbe(false);
            let res = classify_ownership(&exe, &data, &etc, &probe);
            match res {
                OwnershipResult::Refusal { code, .. } => assert_eq!(code, "ownership-unknown"),
                other => panic!("expected refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn ownership_refuses_when_executable_path_disagrees_with_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let prefix1 = temp.path().join("prefix1");
        let prefix2 = temp.path().join("prefix2");
        make_valid_tree(&prefix1, "2.0.48");
        make_valid_tree(&prefix2, "2.0.48");

        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = format!(
            "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix={}\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\nno_start=0\n",
            prefix1.display()
        );
        fs::write(data.join("install.conf"), ini).unwrap();

        let etc = temp.path().join("etc");
        fs::create_dir_all(&etc).unwrap();

        // Executable is in prefix2, which disagrees with prefix1
        let exe = prefix2.join("current/bin/solstone");
        let probe = MockPackageProbe(false);

        let res = classify_ownership(&exe, &data, &etc, &probe);
        assert_eq!(res, OwnershipResult::NotOwned);
    }

    #[test]
    fn ownership_refuses_ambiguous_journal_and_cli_double_claim() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix=/tmp\nlane=release\nplatform_version=2.0.48-r1\n\n[component:journal]\nrole=journal\nroute=tree\nstatus=installed\n\n[component:cli]\nrole=cli\nroute=tree\nstatus=installed\n";
        fs::write(data.join("install.conf"), ini).unwrap();

        let etc = temp.path().join("etc");
        fs::create_dir_all(&etc).unwrap();

        let exe = PathBuf::from("/usr/bin/solstone");
        let probe = MockPackageProbe(false);

        let res = classify_ownership(&exe, &data, &etc, &probe);
        match res {
            OwnershipResult::Refusal { code, .. } => assert_eq!(code, "source-ambiguous"),
            other => panic!("expected source-ambiguous refusal, got {other:?}"),
        }
    }

    #[test]
    fn ownership_refuses_unallowlisted_component_names() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let ini = "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nroute=tree\nphase=complete\nprefix=/tmp\nlane=release\nplatform_version=2.0.48-r1\n\n[component:unknown_plugin]\nrole=unknown_plugin\nroute=tree\nstatus=installed\n";
        fs::write(data.join("install.conf"), ini).unwrap();

        let etc = temp.path().join("etc");
        fs::create_dir_all(&etc).unwrap();

        let exe = PathBuf::from("/usr/bin/solstone");
        let probe = MockPackageProbe(false);

        let res = classify_ownership(&exe, &data, &etc, &probe);
        match res {
            OwnershipResult::Refusal { code, .. } => assert_eq!(code, "ownership-unknown"),
            other => panic!("expected ownership-unknown refusal, got {other:?}"),
        }
    }

    struct RepoPackageProbe {
        journal_ver: &'static str,
        desktop_ver: Option<&'static str>,
        route: &'static str,
    }

    impl PackageProbe for RepoPackageProbe {
        fn owns_journal(&self, _executable: &Path) -> bool {
            true
        }
        fn journal_version(&self, _executable: &Path) -> Option<String> {
            Some(self.journal_ver.to_owned())
        }
        fn payload_matches(&self, _executable: &Path, _version: &str) -> bool {
            true
        }
        fn package_route(&self, _executable: &Path) -> Option<String> {
            Some(self.route.to_owned())
        }
        fn installed_package_version(&self, package_name: &str) -> Option<String> {
            if package_name == "solstone-linux" {
                self.desktop_ver.map(str::to_owned)
            } else {
                None
            }
        }
    }

    #[test]
    fn ownership_positively_classifies_bare_repo_package_and_discovers_desktop() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let etc = temp.path().join("etc");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(etc.join("apt/sources.list.d")).unwrap();
        fs::write(
            etc.join("apt/sources.list.d/solstone.sources"),
            "Types: deb\nURIs: https://updates.solstone.app/apt/release\nSuites: solstone\nComponents: main\n",
        )
        .unwrap();

        let exe = PathBuf::from("/usr/bin/solstone");
        let probe = RepoPackageProbe {
            journal_ver: "2.0.76",
            desktop_ver: Some("2.0.40"),
            route: "deb",
        };

        let res = classify_ownership(&exe, &data, &etc, &probe);
        match res {
            OwnershipResult::Positive(pos) => {
                assert_eq!(pos.route_kind, RouteKind::PlatformPackage);
                assert_eq!(pos.lane, "release");
                assert_eq!(pos.current_journal_version, "2.0.76");
                assert_eq!(pos.current_platform_version, None);
                assert_eq!(pos.components.len(), 2);
                assert_eq!(pos.components[0].name, "journal");
                assert_eq!(pos.components[0].route, "deb");
                assert_eq!(pos.components[0].version.as_deref(), Some("2.0.76"));
                assert_eq!(pos.components[1].name, "desktop");
                assert_eq!(pos.components[1].route, "deb");
                assert_eq!(pos.components[1].version.as_deref(), Some("2.0.40"));
            }
            other => panic!("expected positive repo package ownership, got {other:?}"),
        }
    }

    #[test]
    fn ownership_clears_stale_platform_version_after_out_of_band_package_upgrade() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let etc = temp.path().join("etc");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(etc.join("solstone")).unwrap();

        let ini = "[solstone]\nschema_version=1\norigin=https://updates.solstone.app\nlane=release\nplatform_version=2.0.76-r1\n\n[component:journal]\nrole=journal\nroute=deb\nphase=complete\nstatus=installed\npackage_name=solstone-journal\npackage_version=2.0.76\nservice_policy=start\n\n[component:desktop]\nrole=desktop\nroute=deb\nphase=complete\nstatus=installed\npackage_name=solstone-linux\npackage_version=2.0.40\nservice_policy=skip-service\n";
        fs::write(etc.join("solstone/install.conf"), ini).unwrap();

        let usr_bin = temp.path().join("usr/bin");
        fs::create_dir_all(&usr_bin).unwrap();
        let real_exe = usr_bin.join("solstone");
        fs::write(&real_exe, b"#!/bin/sh\n").unwrap();
        let deleted_exe = PathBuf::from(format!("{} (deleted)", real_exe.display()));
        assert_eq!(normalize_executable_path(&deleted_exe), real_exe);
        assert_eq!(
            normalize_package_semver("1:2.0.41-1"),
            Some("2.0.41".to_owned())
        );

        let probe = RepoPackageProbe {
            journal_ver: "2.0.76",
            desktop_ver: Some("2.0.41"),
            route: "deb",
        };
        let res = classify_ownership(Path::new("/usr/bin/solstone"), &data, &etc, &probe);
        match res {
            OwnershipResult::Positive(pos) => {
                assert_eq!(pos.current_platform_version, None);
                assert_eq!(pos.components[1].version.as_deref(), Some("2.0.41"));
            }
            other => panic!("expected positive ownership after upgrade, got {other:?}"),
        }
    }
}
