// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    Config,
    Source,
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Source::Env => "env",
            Source::Config => "config",
            Source::Source => "source",
            Source::Default => "default",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedJournal {
    pub path: PathBuf,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Decode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeError {
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledDistribution {
    pub version: String,
    pub requires_python: Option<String>,
}

#[derive(Debug)]
pub enum DistributionMetadataError {
    ReadSitePackages {
        path: PathBuf,
        error: io::Error,
    },
    ReadEntry {
        path: PathBuf,
        error: io::Error,
    },
    InvalidMetadata {
        path: PathBuf,
        reason: String,
    },
    ConflictingVersions {
        target: String,
        existing: String,
        found: String,
    },
}

impl fmt::Display for DistributionMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSitePackages { path, error } | Self::ReadEntry { path, error } => {
                write!(formatter, "could not read {}: {error}", path.display())
            }
            Self::InvalidMetadata { path, reason } => write!(
                formatter,
                "invalid metadata at {}: {reason}",
                path.join("METADATA").display()
            ),
            Self::ConflictingVersions {
                target,
                existing,
                found,
            } => write!(
                formatter,
                "conflicting installed versions for {target}: {existing} and {found}"
            ),
        }
    }
}

impl std::error::Error for DistributionMetadataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadSitePackages { error, .. } | Self::ReadEntry { error, .. } => Some(error),
            Self::InvalidMetadata { .. } | Self::ConflictingVersions { .. } => None,
        }
    }
}

#[derive(Debug)]
pub struct EnsureJournalDirError {
    pub source: String,
    pub path: PathBuf,
    pub error: io::Error,
}

impl fmt::Display for EnsureJournalDirError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not create journal directory ({}): {}: {}",
            self.source,
            self.path.display(),
            self.error
        )
    }
}

impl std::error::Error for EnsureJournalDirError {}

pub fn resolve_journal_path(
    env_journal: Option<&OsStr>,
    config_journal: Option<&str>,
    checkout_root: Option<&Path>,
    home: &Path,
) -> ResolvedJournal {
    if let Some(path) = env_journal.filter(|value| *value != OsStr::new("")) {
        return ResolvedJournal {
            path: PathBuf::from(path),
            source: Source::Env,
        };
    }

    if let Some(path) = config_journal
        .map(python_strip)
        .filter(|value| !value.is_empty())
    {
        return ResolvedJournal {
            path: PathBuf::from(path),
            source: Source::Config,
        };
    }

    if let Some(root) = checkout_root {
        return ResolvedJournal {
            path: root.join("journal"),
            source: Source::Source,
        };
    }

    ResolvedJournal {
        path: join_journal_like_pathlib(home),
        source: Source::Default,
    }
}

pub fn read_config_journal(home: &Path) -> Result<Option<String>, ConfigError> {
    let path = home.join(".config").join("solstone").join("config.toml");
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let text = String::from_utf8(bytes).map_err(|_error| ConfigError::Decode)?;
    let document = match toml_edit::DocumentMut::from_str(&text) {
        Ok(document) => document,
        Err(_) => return Ok(None),
    };
    Ok(document
        .get("journal")
        .and_then(|item| item.as_value())
        .and_then(|value| value.as_str())
        .map(str::to_owned))
}

pub fn discover_home(
    home_env: Option<&OsStr>,
    passwd_home: Option<&Path>,
) -> Result<PathBuf, HomeError> {
    match home_env {
        Some(value) => normalize_home(value),
        None => match passwd_home {
            Some(path) => normalize_home(path.as_os_str()),
            None => Err(HomeError::Unavailable),
        },
    }
}

pub fn detect_checkout_root(root: &Path) -> Option<PathBuf> {
    if root.join("pyproject.toml").exists() && root.join(".git").exists() {
        Some(root.to_path_buf())
    } else {
        None
    }
}

pub fn installed_site_packages_from_executable_dir(executable_dir: &Path) -> Option<PathBuf> {
    let prefix = executable_dir.parent()?;
    let entries = fs::read_dir(prefix).ok()?;
    let mut candidates = Vec::new();
    for lib_entry in entries.flatten() {
        let lib_path = lib_entry.path();
        if !lib_path.is_dir() {
            continue;
        }
        let Some(lib_name) = lib_path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if !lib_name.starts_with("lib") {
            continue;
        }
        let Ok(python_entries) = fs::read_dir(&lib_path) else {
            continue;
        };
        for python_entry in python_entries.flatten() {
            let python_path = python_entry.path();
            if !python_path.is_dir() {
                continue;
            }
            let Some(python_name) = python_path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if !python_name.starts_with("python") {
                continue;
            }
            for package_dir_name in ["site-packages", "dist-packages"] {
                let package_dir = python_path.join(package_dir_name);
                let init = package_dir.join("solstone").join("__init__.py");
                if fs::metadata(&init).is_ok_and(|metadata| metadata.is_file()) {
                    candidates.push(package_dir);
                }
            }
        }
    }
    resolve_canonical_site_packages(&candidates)
}

pub fn resolve_installation_root_from_executable_dir(executable_dir: &Path) -> Option<PathBuf> {
    installed_site_packages_from_executable_dir(executable_dir).or_else(|| {
        executable_dir
            .ancestors()
            .find_map(is_solstone_checkout_root)
    })
}

pub fn installed_distributions(
    site_packages: &Path,
    target_names: &[&str],
) -> Result<BTreeMap<String, InstalledDistribution>, DistributionMetadataError> {
    let targets = target_names
        .iter()
        .map(|name| normalize_distribution_name(name))
        .collect::<Vec<_>>();
    let entries = fs::read_dir(site_packages).map_err(|error| {
        DistributionMetadataError::ReadSitePackages {
            path: site_packages.to_path_buf(),
            error,
        }
    })?;
    let mut distributions: BTreeMap<String, InstalledDistribution> = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|error| DistributionMetadataError::ReadEntry {
            path: site_packages.to_path_buf(),
            error,
        })?;
        let path = entry.path();
        if !path.is_dir()
            || !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".dist-info"))
        {
            continue;
        }
        let directory_target = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| target_from_dist_info_directory(name, &targets));
        let metadata = match fs::read_to_string(path.join("METADATA")) {
            Ok(metadata) => metadata,
            Err(error) => {
                if directory_target.is_some() {
                    return Err(metadata_error(&path, error.to_string()));
                }
                continue;
            }
        };
        let (name, version, requires_python) = metadata_headers(&metadata);
        let header_target = name
            .as_deref()
            .map(normalize_distribution_name)
            .filter(|name| targets.contains(name));
        let target = match (header_target, directory_target) {
            (Some(header_target), Some(directory_target)) if header_target != directory_target => {
                return Err(metadata_error(
                    &path,
                    "Name header does not match target package",
                ));
            }
            (Some(header_target), _) => Some(header_target),
            // A generic target list may contain a prefix of another package
            // name (for example `solstone` and `solstone-journal`). A present
            // non-target Name header makes that entry unrelated, not malformed.
            (None, Some(_)) if name.is_some() => None,
            (None, directory_target) => directory_target,
        };
        let Some(target) = target else {
            continue;
        };
        let Some(name) = name else {
            return Err(metadata_error(&path, "missing Name header"));
        };
        let Some(version) = version else {
            return Err(metadata_error(&path, "missing Version header"));
        };
        if normalize_distribution_name(&name) != target {
            return Err(metadata_error(
                &path,
                "Name header does not match target package",
            ));
        }
        let distribution = InstalledDistribution {
            version,
            requires_python,
        };
        match distributions.get(&target) {
            Some(existing) if existing.version == distribution.version => {}
            Some(existing) => {
                return Err(DistributionMetadataError::ConflictingVersions {
                    target,
                    existing: existing.version.clone(),
                    found: distribution.version,
                });
            }
            None => {
                distributions.insert(target, distribution);
            }
        }
    }
    Ok(distributions)
}

fn resolve_canonical_site_packages(candidates: &[PathBuf]) -> Option<PathBuf> {
    let mut canonical = candidates
        .iter()
        .filter_map(|candidate| fs::canonicalize(candidate).ok())
        .collect::<Vec<_>>();
    canonical.sort();
    canonical.dedup();
    canonical.into_iter().next()
}

fn is_solstone_checkout_root(candidate: &Path) -> Option<PathBuf> {
    (candidate.join("pyproject.toml").is_file()
        && candidate.join(".git").exists()
        && candidate.join("solstone").is_dir())
    .then(|| candidate.to_path_buf())
}

fn target_from_dist_info_directory(name: &str, targets: &[String]) -> Option<String> {
    let stem = name.strip_suffix(".dist-info")?;
    let normalized = normalize_distribution_name(stem);
    targets
        .iter()
        .filter(|target| {
            normalized
                .strip_prefix(target.as_str())
                .is_some_and(|suffix| suffix.starts_with('-'))
        })
        .max_by_key(|target| target.len())
        .cloned()
}

fn metadata_headers(metadata: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut name = None;
    let mut version = None;
    let mut requires_python = None;
    for line in metadata.lines() {
        if line.is_empty() {
            break;
        }
        if name.is_none() {
            name = line.strip_prefix("Name:").map(str::trim).map(str::to_owned);
        }
        if version.is_none() {
            version = line
                .strip_prefix("Version:")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
        }
        if requires_python.is_none() {
            requires_python = line
                .strip_prefix("Requires-Python:")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
        }
    }
    (
        name.filter(|value| !value.is_empty()),
        version,
        requires_python,
    )
}

fn normalize_distribution_name(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut separator = false;
    for character in value.chars() {
        if matches!(character, '-' | '_' | '.') {
            if !separator {
                normalized.push('-');
                separator = true;
            }
        } else {
            normalized.extend(character.to_lowercase());
            separator = false;
        }
    }
    normalized
}

fn metadata_error(path: &Path, reason: impl fmt::Display) -> DistributionMetadataError {
    DistributionMetadataError::InvalidMetadata {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

pub fn ensure_journal_dir(path: &Path, source: Source) -> Result<(), EnsureJournalDirError> {
    ensure_journal_dir_with_label(path, &source.to_string())
}

pub fn ensure_journal_dir_with_label(
    path: &Path,
    source: &str,
) -> Result<(), EnsureJournalDirError> {
    if path.as_os_str() == OsStr::new("") {
        return Err(EnsureJournalDirError {
            source: source.to_string(),
            path: path.to_path_buf(),
            error: io::Error::new(io::ErrorKind::NotFound, "empty path"),
        });
    }
    fs::create_dir_all(path).map_err(|error| EnsureJournalDirError {
        source: source.to_string(),
        path: path.to_path_buf(),
        error,
    })
}

pub fn python_strip(value: &str) -> &str {
    let start = value
        .char_indices()
        .find_map(|(index, ch)| (!is_python_whitespace(ch)).then_some(index))
        .unwrap_or(value.len());
    let end = value
        .char_indices()
        .rev()
        .find_map(|(index, ch)| (!is_python_whitespace(ch)).then_some(index + ch.len_utf8()))
        .unwrap_or(start);
    &value[start..end]
}

fn is_python_whitespace(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{001c}'..='\u{001f}')
}

#[cfg(unix)]
fn normalize_home(value: &OsStr) -> Result<PathBuf, HomeError> {
    let stripped = strip_trailing_slashes_or_root(value.as_bytes());
    if stripped.first() == Some(&b'~') {
        return Err(HomeError::Unavailable);
    }
    Ok(PathBuf::from(OsString::from_vec(normalize_path_bytes(
        &stripped,
    ))))
}

#[cfg(not(unix))]
fn normalize_home(value: &OsStr) -> Result<PathBuf, HomeError> {
    let string = value.to_string_lossy();
    let stripped = string.trim_end_matches('/');
    let normalized = if stripped.is_empty() {
        "/".to_string()
    } else {
        if stripped.starts_with('~') {
            return Err(HomeError::Unavailable);
        }
        normalize_path_string(stripped)
    };
    Ok(PathBuf::from(normalized))
}

#[cfg(unix)]
fn join_journal_like_pathlib(home: &Path) -> PathBuf {
    let bytes = home.as_os_str().as_bytes();
    if bytes.is_empty() || bytes == b"." {
        return PathBuf::from("journal");
    }
    let mut output = bytes.to_vec();
    if !output.ends_with(b"/") {
        output.push(b'/');
    }
    output.extend_from_slice(b"journal");
    PathBuf::from(OsString::from_vec(output))
}

#[cfg(not(unix))]
fn join_journal_like_pathlib(home: &Path) -> PathBuf {
    if home.as_os_str() == OsStr::new("") || home.as_os_str() == OsStr::new(".") {
        return PathBuf::from("journal");
    }
    home.join("journal")
}

#[cfg(unix)]
fn strip_trailing_slashes_or_root(bytes: &[u8]) -> Vec<u8> {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        b"/".to_vec()
    } else {
        bytes[..end].to_vec()
    }
}

#[cfg(unix)]
fn normalize_path_bytes(bytes: &[u8]) -> Vec<u8> {
    let (prefix, tail) = if bytes.starts_with(b"//") && !bytes.starts_with(b"///") {
        (&b"//"[..], &bytes[2..])
    } else if bytes.starts_with(b"/") {
        (&b"/"[..], &bytes[1..])
    } else {
        (&b""[..], bytes)
    };

    let parts: Vec<&[u8]> = tail
        .split(|byte| *byte == b'/')
        .filter(|part| !part.is_empty() && *part != b".")
        .collect();

    if parts.is_empty() {
        if prefix.is_empty() {
            return b".".to_vec();
        }
        return prefix.to_vec();
    }

    let mut output = Vec::new();
    output.extend_from_slice(prefix);
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            output.push(b'/');
        }
        output.extend_from_slice(part);
    }
    output
}

#[cfg(not(unix))]
fn normalize_path_string(value: &str) -> String {
    let prefix = if value.starts_with("//") && !value.starts_with("///") {
        "//"
    } else if value.starts_with('/') {
        "/"
    } else {
        ""
    };
    let tail = &value[prefix.len()..];
    let parts: Vec<&str> = tail
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        if prefix.is_empty() {
            ".".to_string()
        } else {
            prefix.to_string()
        }
    } else {
        format!("{prefix}{}", parts.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn unique_temp(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic enough for tests")
            .as_nanos();
        PathBuf::from("/tmp").join(format!("solstone-core-journal-{name}-{stamp}"))
    }

    #[test]
    fn config_strip_matches_python_control_whitespace() {
        let resolved = resolve_journal_path(
            None,
            Some("\u{001c}/tmp/from-config\u{001f}"),
            None,
            Path::new("/tmp/home"),
        );
        assert_eq!(resolved.source, Source::Config);
        assert_eq!(resolved.path, PathBuf::from("/tmp/from-config"));
    }

    #[test]
    fn empty_env_falls_through_to_config() {
        let resolved = resolve_journal_path(
            Some(OsStr::new("")),
            Some("/tmp/from-config"),
            None,
            Path::new("/tmp/home"),
        );
        assert_eq!(resolved.source, Source::Config);
        assert_eq!(resolved.path, PathBuf::from("/tmp/from-config"));
    }

    #[test]
    fn env_spaces_win_unstripped() {
        let resolved =
            resolve_journal_path(Some(OsStr::new("   ")), None, None, Path::new("/tmp/home"));
        assert_eq!(resolved.source, Source::Env);
        assert_eq!(resolved.path, PathBuf::from("   "));
    }

    #[test]
    fn discover_home_normalizes_pathlib_layers() {
        let cases = [
            ("", "/"),
            ("///", "/"),
            ("/tmp//y", "/tmp/y"),
            ("/tmp/./z", "/tmp/z"),
            ("//server//share", "//server/share"),
            ("rel//home", "rel/home"),
            ("./relative", "relative"),
            (".", "."),
        ];
        for (input, expected) in cases {
            let home = discover_home(Some(OsStr::new(input)), None).expect("home should resolve");
            assert_eq!(home, PathBuf::from(expected), "{input:?}");
        }
    }

    #[test]
    fn discover_home_rejects_tilde_leading_expanded_home() {
        for input in ["~", "~/", "~/x", "~~"] {
            assert_eq!(
                discover_home(Some(OsStr::new(input)), None),
                Err(HomeError::Unavailable)
            );
        }
        assert_eq!(
            discover_home(None, Some(Path::new("~/passwd"))),
            Err(HomeError::Unavailable)
        );
        assert_eq!(
            discover_home(Some(OsStr::new("x~")), None).expect("x~ is not tilde-leading"),
            PathBuf::from("x~")
        );
        assert_eq!(
            discover_home(Some(OsStr::new("./~")), None).expect("./~ passes the pathlib guard"),
            PathBuf::from("~")
        );
    }

    #[test]
    fn absent_home_uses_injected_passwd_fallback() {
        let home =
            discover_home(None, Some(Path::new("/tmp//passwd/"))).expect("home should resolve");
        assert_eq!(home, PathBuf::from("/tmp/passwd"));
    }

    #[test]
    fn absent_home_without_passwd_fallback_errors() {
        assert_eq!(discover_home(None, None), Err(HomeError::Unavailable));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_env_path_is_preserved() {
        let raw = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]);
        let resolved = resolve_journal_path(Some(raw.as_os_str()), None, None, Path::new("/tmp"));
        assert_eq!(resolved.source, Source::Env);
        assert_eq!(resolved.path.as_os_str().as_bytes(), raw.as_bytes());
    }

    #[test]
    fn detect_checkout_root_requires_pyproject_and_git() {
        let root = unique_temp("checkout");
        fs::create_dir_all(&root).expect("create root");
        assert_eq!(detect_checkout_root(&root), None);
        fs::write(root.join("pyproject.toml"), "").expect("write pyproject");
        assert_eq!(detect_checkout_root(&root), None);
        fs::create_dir(root.join(".git")).expect("create git marker");
        assert_eq!(detect_checkout_root(&root), Some(root.clone()));
        fs::remove_dir_all(root).expect("cleanup checkout root");
    }

    #[test]
    fn installation_helpers_resolve_staged_site_packages_and_checkout() {
        let root = unique_temp("installation-helpers");
        let bin = root.join("prefix/bin");
        let site_packages = root.join("prefix/lib/python3.12/site-packages");
        fs::create_dir_all(site_packages.join("solstone")).expect("create staged package");
        fs::write(site_packages.join("solstone/__init__.py"), "").expect("write package marker");

        assert_eq!(
            installed_site_packages_from_executable_dir(&bin),
            Some(fs::canonicalize(&site_packages).expect("canonical staged site-packages"))
        );
        assert_eq!(
            resolve_installation_root_from_executable_dir(&bin),
            Some(fs::canonicalize(&site_packages).expect("canonical staged site-packages"))
        );

        let checkout = root.join("checkout");
        let checkout_bin = checkout.join(".venv/bin");
        fs::create_dir_all(checkout.join("solstone")).expect("create checkout package");
        fs::create_dir_all(checkout.join(".git")).expect("create checkout git marker");
        fs::write(checkout.join("pyproject.toml"), "").expect("write checkout marker");
        assert_eq!(
            resolve_installation_root_from_executable_dir(&checkout_bin),
            Some(checkout.clone())
        );
        fs::remove_dir_all(root).expect("cleanup installation helpers");
    }

    #[test]
    fn installed_distributions_reads_requires_python_from_staged_metadata() {
        let root = unique_temp("distribution-metadata");
        let site_packages = root.join("site-packages");
        let dist_info = site_packages.join("solstone-1.2.3.dist-info");
        fs::create_dir_all(&dist_info).expect("create dist-info");
        fs::write(
            dist_info.join("METADATA"),
            "Name: solstone\nVersion: 1.2.3\nRequires-Python: >=3.11\n\nignored: body\n",
        )
        .expect("write metadata");
        let journal_dist_info = site_packages.join("solstone_journal-1.2.3.dist-info");
        fs::create_dir_all(&journal_dist_info).expect("create neighboring dist-info");
        fs::write(
            journal_dist_info.join("METADATA"),
            "Name: solstone-journal\nVersion: 1.2.3\n\n",
        )
        .expect("write neighboring metadata");

        let distributions = installed_distributions(&site_packages, &["solstone"])
            .expect("read staged distribution metadata");
        assert_eq!(
            distributions.get("solstone"),
            Some(&InstalledDistribution {
                version: "1.2.3".into(),
                requires_python: Some(">=3.11".into()),
            })
        );
        assert_eq!(distributions.len(), 1);
        fs::remove_dir_all(root).expect("cleanup distribution metadata");
    }

    #[test]
    fn read_config_distinguishes_invalid_utf8() {
        let home = unique_temp("invalid-utf8");
        let config_dir = home.join(".config").join("solstone");
        fs::create_dir_all(&config_dir).expect("create config dir");
        fs::write(config_dir.join("config.toml"), [0xff]).expect("write invalid bytes");
        assert_eq!(read_config_journal(&home), Err(ConfigError::Decode));
        fs::remove_dir_all(home).expect("cleanup home");
    }

    #[test]
    fn read_config_ignores_invalid_toml_and_non_string_values() {
        let home = unique_temp("config-values");
        let config_dir = home.join(".config").join("solstone");
        fs::create_dir_all(&config_dir).expect("create config dir");
        let config_path = config_dir.join("config.toml");
        fs::write(&config_path, "journal = \"\n").expect("write invalid toml");
        assert_eq!(read_config_journal(&home), Ok(None));
        fs::write(&config_path, "journal = 123\n").expect("write non-string toml");
        assert_eq!(read_config_journal(&home), Ok(None));
        fs::write(&config_path, "journal = \"/tmp/x\"\n").expect("write string toml");
        assert_eq!(read_config_journal(&home), Ok(Some("/tmp/x".to_string())));
        fs::remove_dir_all(home).expect("cleanup home");
    }

    #[test]
    fn ensure_journal_dir_creates_fresh_and_nested_dirs() {
        let root = unique_temp("ensure-create");
        let target = root.join("a").join("b").join("journal");
        ensure_journal_dir(&target, Source::Default).expect("create nested journal");
        assert!(target.is_dir());
        fs::remove_dir_all(root).expect("cleanup created journal");
    }

    #[test]
    fn ensure_journal_dir_accepts_existing_dir() {
        let target = unique_temp("ensure-existing");
        fs::create_dir_all(&target).expect("create existing dir");
        ensure_journal_dir(&target, Source::Config).expect("existing dir should be ok");
        fs::remove_dir_all(target).expect("cleanup existing dir");
    }

    #[test]
    fn ensure_journal_dir_errors_when_path_is_file() {
        let root = unique_temp("ensure-file");
        fs::create_dir_all(&root).expect("create root");
        let target = root.join("journal");
        fs::write(&target, "not a dir").expect("write file");
        let error = ensure_journal_dir(&target, Source::Env).expect_err("file should error");
        assert_eq!(error.source, "env");
        assert_eq!(error.path, target);
        assert_eq!(error.error.kind(), io::ErrorKind::AlreadyExists);
        fs::remove_dir_all(root).expect("cleanup file root");
    }

    #[test]
    fn ensure_journal_dir_rejects_empty_path() {
        let error = ensure_journal_dir(Path::new(""), Source::Config).expect_err("empty path");
        assert_eq!(error.source, "config");
        assert_eq!(error.path, PathBuf::from(""));
        assert_eq!(error.error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn ensure_journal_dir_reports_read_only_parent() {
        let root = unique_temp("ensure-read-only");
        let parent = root.join("parent");
        fs::create_dir_all(&parent).expect("create parent");
        #[cfg(unix)]
        {
            let original_permissions = fs::metadata(&parent).expect("metadata").permissions();
            let mut read_only_permissions = original_permissions.clone();
            read_only_permissions.set_mode(0o500);
            fs::set_permissions(&parent, read_only_permissions).expect("set readonly");
            let target = parent.join("journal");
            let result = ensure_journal_dir(&target, Source::Source);
            fs::set_permissions(&parent, original_permissions).expect("restore permissions");
            fs::remove_dir_all(root).expect("cleanup readonly root");
            let error = result.expect_err("read-only parent should error");
            assert_eq!(error.source, "source");
            assert_eq!(error.path, target);
        }
        #[cfg(not(unix))]
        {
            let mut permissions = fs::metadata(&parent).expect("metadata").permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&parent, permissions.clone()).expect("set readonly");
            let target = parent.join("journal");
            let result = ensure_journal_dir(&target, Source::Source);
            permissions.set_readonly(false);
            fs::set_permissions(&parent, permissions).expect("restore permissions");
            fs::remove_dir_all(root).expect("cleanup readonly root");
            let error = result.expect_err("read-only parent should error");
            assert_eq!(error.source, "source");
            assert_eq!(error.path, target);
        }
    }

    fn expand_token(value: &str, root: &Path) -> String {
        value.replace("$TMP", &root.as_os_str().to_string_lossy())
    }

    fn mapping_string<'a>(mapping: &'a Value, key: &str) -> &'a str {
        mapping
            .get(key)
            .and_then(Value::as_str)
            .expect("vector string field should exist")
    }

    fn mapping_state<'a>(mapping: &'a Value, id: &str, field: &str, allowed: &[&str]) -> &'a str {
        let state = mapping_string(mapping, "state");
        if !allowed.contains(&state) {
            panic!("unknown {field} state for {id}: {state}");
        }
        state
    }

    fn outcome_type<'a>(outcome: &'a Value, id: &str) -> &'a str {
        match outcome.get("type").and_then(Value::as_str) {
            Some("ok") => "ok",
            Some("error") => "error",
            Some(other) => panic!("unknown outcome type for {id}: {other}"),
            None => panic!("vector outcome type should exist for {id}"),
        }
    }

    fn assert_error_class(outcome: &Value, id: &str, expected: &str) {
        assert_eq!(mapping_string(outcome, "python_class"), expected, "{id}");
    }

    #[test]
    fn vectors_match_rust_resolver() {
        let text = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../tests/fixtures/journal_path_resolution_vectors.json"),
        )
        .expect("read journal path vectors");
        let vectors: Value = serde_json::from_str(&text).expect("parse vectors");
        let root = unique_temp("vectors");
        let cases = vectors["cases"].as_array().expect("cases should be array");
        assert!(!cases.is_empty(), "journal path vectors must not be empty");
        let mut asserted_cases = 0;

        for case in cases {
            let id = case["id"].as_str().expect("case id");
            let outcome = &case["outcome"];
            let expected_outcome_type = outcome_type(outcome, id);
            let env_journal_string = match mapping_state(
                &case["solstone_journal"],
                id,
                "solstone_journal",
                &["set", "absent"],
            ) {
                "set" => Some(expand_token(
                    mapping_string(&case["solstone_journal"], "value"),
                    &root,
                )),
                "absent" => None,
                _ => unreachable!("solstone_journal state was validated"),
            };
            if env_journal_string
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            {
                assert_eq!(expected_outcome_type, "ok", "{id}");
                let resolved = resolve_journal_path(
                    env_journal_string.as_deref().map(OsStr::new),
                    None,
                    None,
                    Path::new("unused"),
                );
                assert_eq!(
                    resolved.source.to_string(),
                    outcome["source"].as_str().expect("expected source"),
                    "{id}",
                );
                assert_eq!(
                    resolved.path,
                    PathBuf::from(expand_token(
                        outcome["path"].as_str().expect("expected path"),
                        &root,
                    )),
                    "{id}",
                );
                asserted_cases += 1;
                continue;
            }

            let home = match mapping_state(&case["home"], id, "home", &["set", "absent"]) {
                "set" => {
                    let home_value = mapping_string(&case["home"], "value");
                    let home_string = expand_token(home_value, &root);
                    discover_home(Some(OsStr::new(&home_string)), None)
                }
                "absent" => discover_home(None, None),
                _ => unreachable!("home state was validated"),
            };
            let home = match home {
                Ok(home) => home,
                Err(HomeError::Unavailable) => {
                    assert_eq!(expected_outcome_type, "error", "{id}");
                    assert_error_class(outcome, id, "RuntimeError");
                    asserted_cases += 1;
                    continue;
                }
            };

            let config_state =
                mapping_state(&case["config"], id, "config", &["absent", "text", "hex"]);
            let config_journal = match config_state {
                "absent" => Ok(None),
                "text" => {
                    let value = expand_token(mapping_string(&case["config"], "value"), &root);
                    let document = toml_edit::DocumentMut::from_str(&value);
                    Ok(document.ok().and_then(|doc| {
                        doc.get("journal")
                            .and_then(|item| item.as_value())
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    }))
                }
                "hex" => Err(ConfigError::Decode),
                _ => unreachable!("config state was validated"),
            };

            if matches!(&config_journal, Err(ConfigError::Decode)) {
                assert_eq!(expected_outcome_type, "error", "{id}");
                assert_error_class(outcome, id, "UnicodeDecodeError");
                asserted_cases += 1;
                continue;
            }
            if expected_outcome_type == "error" {
                panic!("expected error outcome was not produced for {id}");
            }

            let checkout_state = mapping_state(
                &case["checkout_root"],
                id,
                "checkout_root",
                &["present", "absent"],
            );
            let checkout_root = if checkout_state == "present" {
                Some(PathBuf::from(expand_token(
                    mapping_string(&case["checkout_root"], "path"),
                    &root,
                )))
            } else {
                None
            };
            let resolved = resolve_journal_path(
                env_journal_string.as_deref().map(OsStr::new),
                config_journal
                    .expect("config should not error after decode handling")
                    .as_deref(),
                checkout_root.as_deref(),
                &home,
            );

            assert_eq!(
                resolved.source.to_string(),
                outcome["source"].as_str().expect("expected source"),
                "{id}",
            );
            assert_eq!(
                resolved.path,
                PathBuf::from(expand_token(
                    outcome["path"].as_str().expect("expected path"),
                    &root,
                )),
                "{id}",
            );
            asserted_cases += 1;
        }
        assert_eq!(asserted_cases, cases.len());
    }
}
