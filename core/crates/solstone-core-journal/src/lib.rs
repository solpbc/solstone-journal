// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[cfg(test)]
mod test_support;

#[cfg(unix)]
use std::ffi::OsString;
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
    site_packages_from_executable_dir(executable_dir, |package_dir| {
        let init = package_dir.join("solstone").join("__init__.py");
        fs::metadata(init).is_ok_and(|metadata| metadata.is_file())
    })
}

fn site_packages_from_executable_dir(
    executable_dir: &Path,
    include: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
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
                if include(&package_dir) {
                    candidates.push(package_dir);
                }
            }
        }
    }
    resolve_canonical_site_packages(&candidates)
}

pub const LAYOUT_BUNDLE_ANCHOR: &str = "solstone/talent/journal/contract/bundle.json";
pub const LAYOUT_LAYOUT_ANCHOR: &str = "solstone/think/contract/layout.json";
pub const LAYOUT_TEMPLATE_ANCHOR: &str = "solstone/think/templates/segment_preamble.md";

/// Where a repository checkout keeps the shipped payload.
///
/// The payload is product data the binary reads at runtime, not part of the
/// Python package tree, so it does not live under `solstone/` in the
/// repository even though its installed layout keeps that name. This directory
/// is the checkout's stand-in for the installed `share/` prefix, which is what
/// lets one set of relative paths describe both.
///
/// `core/distribution/inventory.toml` declares the same root as
/// `payload_src_root` for the producer; `repository_payload_inventory` pins the
/// two together so neither can move without the other.
pub const CHECKOUT_PAYLOAD_ROOT: &str = "core/payload";

/// Whether a directory is a payload root: the three anchors the installed
/// layout is recognised by, checked against any candidate root.
fn has_layout_anchors(root: &Path) -> bool {
    [
        LAYOUT_BUNDLE_ANCHOR,
        LAYOUT_LAYOUT_ANCHOR,
        LAYOUT_TEMPLATE_ANCHOR,
    ]
    .iter()
    .all(|anchor| fs::metadata(root.join(anchor)).is_ok_and(|metadata| metadata.is_file()))
}

/// The payload root inside a repository checkout, when that checkout carries
/// one. `None` for a repository whose payload is absent or incomplete.
pub fn payload_root_in_checkout(root: &Path) -> Option<PathBuf> {
    let payload = root.join(CHECKOUT_PAYLOAD_ROOT);
    has_layout_anchors(&payload).then_some(payload)
}

/// The repository root an executable was built inside, when that checkout also
/// carries the shipped payload.
///
/// Deliberately distinct from `resolve_installation_root_from_executable_dir`,
/// which returns the root the payload is *read* from. In an installed tree the
/// two coincide; in a checkout they do not, and a consumer that wants the
/// repository — the developer journal, `.venv` discovery, the contract's schema
/// sources — must ask for it by name rather than take the payload root and hope.
pub fn resolve_checkout_root_from_executable_dir(executable_dir: &Path) -> Option<PathBuf> {
    executable_dir.ancestors().find_map(|candidate| {
        detect_checkout_root(candidate).filter(|root| payload_root_in_checkout(root).is_some())
    })
}

pub fn resolve_installation_root_from_executable_dir(executable_dir: &Path) -> Option<PathBuf> {
    installed_site_packages_from_executable_dir(executable_dir)
        .or_else(|| {
            executable_dir
                .ancestors()
                .find_map(is_solstone_checkout_root)
        })
        .or_else(|| executable_dir.parent().and_then(is_layout_install_root))
}

/// Resolves the stable on-disk installation identity root for an executable.
///
/// A checkout uses its checkout root. A versioned shipped install deliberately
/// uses the directory that owns `current`, not the version selected by that
/// symlink, so an update or downgrade preserves the installation identity.
/// Other layouts retain the ordinary installation-root meaning.
pub fn resolve_identity_root_from_executable_dir(executable_dir: &Path) -> Option<PathBuf> {
    resolve_checkout_root_from_executable_dir(executable_dir)
        .or_else(|| versioned_prefix_from_executable_dir(executable_dir))
        .or_else(|| resolve_installation_root_from_executable_dir(executable_dir))
}

fn versioned_prefix_from_executable_dir(executable_dir: &Path) -> Option<PathBuf> {
    let current = executable_dir.parent()?;
    if executable_dir.file_name()? != OsStr::new("bin")
        || current.file_name()? != OsStr::new("current")
    {
        return None;
    }
    let prefix = current.parent()?;
    if !fs::symlink_metadata(current).ok()?.file_type().is_symlink() {
        return None;
    }
    let target = fs::read_link(current).ok()?;
    let target = if target.is_absolute() {
        target
    } else {
        prefix.join(target)
    };
    (target.parent()?.file_name()? == OsStr::new("versions") && target.is_dir())
        .then(|| prefix.to_path_buf())
}

/// Why [`resolve_installation_root_from_executable_dir`] returned `None`.
///
/// Reuses the same three candidate predicates as the resolver. The text is for
/// operators; it names the executable directory, a walked ancestor, and the
/// checkout / layout markers that were required.
pub fn describe_installation_root_miss(executable_dir: &Path) -> String {
    let site_packages = installed_site_packages_from_executable_dir(executable_dir);
    let checkout = executable_dir.ancestors().find_map(|candidate| {
        detect_checkout_root(candidate)
            .and_then(|root| is_solstone_checkout_root(&root).map(|payload| (root, payload)))
    });
    let layout = executable_dir.parent().and_then(is_layout_install_root);
    let ancestor = executable_dir
        .parent()
        .or_else(|| executable_dir.ancestors().nth(1))
        .unwrap_or(executable_dir);
    format!(
        "could not locate packaged talent roots from executable directory {}\n\
         walked ancestor {}\n\
         site-packages candidate: {}\n\
         checkout candidate (pyproject.toml + .git): {}\n\
         layout share anchors {}, {}, {}: {}",
        executable_dir.display(),
        ancestor.display(),
        site_packages
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_owned()),
        checkout
            .as_ref()
            .map(|(root, _)| root.display().to_string())
            .unwrap_or_else(|| "none".to_owned()),
        LAYOUT_BUNDLE_ANCHOR,
        LAYOUT_LAYOUT_ANCHOR,
        LAYOUT_TEMPLATE_ANCHOR,
        layout
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_owned()),
    )
}

/// Why packaged `solstone/talent` + `solstone/apps` roots could not be formed.
pub fn describe_package_roots_miss(executable_dir: &Path) -> String {
    match resolve_installation_root_from_executable_dir(executable_dir) {
        None => describe_installation_root_miss(executable_dir),
        Some(root) => {
            let talent = root.join("solstone/talent");
            let apps = root.join("solstone/apps");
            if !talent.is_dir() {
                format!(
                    "installation root {} is missing directory {}",
                    root.display(),
                    talent.display()
                )
            } else if !apps.is_dir() {
                format!(
                    "installation root {} is missing directory {}",
                    root.display(),
                    apps.display()
                )
            } else {
                format!(
                    "could not locate packaged talent roots from executable directory {}",
                    executable_dir.display()
                )
            }
        }
    }
}

fn is_layout_install_root(prefix: &Path) -> Option<PathBuf> {
    let share = prefix.join("share");
    has_layout_anchors(&share).then_some(share)
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

/// The checkout candidate of the installation-root resolver.
///
/// It returns the checkout's *payload* root rather than its repository root,
/// which is what keeps every consumer's `root.join("solstone/...")` correct in
/// all three layouts. The anchors are the same three the installed layout is
/// recognised by, so a repository that still has a `solstone/` directory but no
/// payload no longer matches — the previous `solstone` directory test would
/// have kept matching and handed back a root whose payload reads all fail.
fn is_solstone_checkout_root(candidate: &Path) -> Option<PathBuf> {
    (candidate.join("pyproject.toml").is_file() && candidate.join(".git").exists())
        .then(|| payload_root_in_checkout(candidate))
        .flatten()
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
    use crate::test_support::reserve_temp_path;
    use serde_json::Value;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn unique_temp(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-journal-{name}"))
    }

    #[test]
    fn config_strip_matches_python_control_whitespace() {
        let expected = PathBuf::from("/").join("tmp").join("from-config");
        let resolved = resolve_journal_path(
            None,
            Some(
                "\u{001c}\u{001d}\u{001e}\u{001f}/tmp/from-config\u{001f}\u{001e}\u{001d}\u{001c}",
            ),
            None,
            Path::new("/tmp/home"),
        );
        assert_eq!(resolved.source, Source::Config);
        assert_eq!(resolved.path, expected);
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
        // A checkout with a `solstone` directory but no payload must not match.
        // Before the payload moved out of the package tree these two facts were
        // the same fact; now a survivor directory is not evidence of a payload.
        assert_eq!(
            resolve_installation_root_from_executable_dir(&checkout_bin),
            None,
            "a solstone directory without the payload is not an installation root"
        );
        let payload = checkout.join(CHECKOUT_PAYLOAD_ROOT);
        write_layout_anchors(&payload);
        assert_eq!(
            resolve_installation_root_from_executable_dir(&checkout_bin),
            Some(payload.clone())
        );
        assert_eq!(
            resolve_checkout_root_from_executable_dir(&checkout_bin),
            Some(checkout.clone()),
            "the repository root stays reachable by its own name"
        );
        assert_eq!(payload_root_in_checkout(&checkout), Some(payload));
        fs::remove_dir_all(root).expect("cleanup installation helpers");
    }

    #[cfg(unix)]
    #[test]
    fn identity_root_uses_a_versioned_prefix_across_current_switches() {
        use std::os::unix::fs::symlink;

        let root = unique_temp("identity-versioned-prefix");
        let prefix = root.join("prefix");
        let versions = prefix.join("versions");
        let first = versions.join("1.0.0-aaaaaaaa");
        let second = versions.join("0.9.0-bbbbbbbb");
        fs::create_dir_all(first.join("bin")).expect("create first version");
        fs::create_dir_all(second.join("bin")).expect("create second version");
        symlink("versions/1.0.0-aaaaaaaa", prefix.join("current")).expect("link current first");

        assert_eq!(
            resolve_identity_root_from_executable_dir(&prefix.join("current/bin")),
            Some(prefix.clone())
        );
        fs::remove_file(prefix.join("current")).expect("remove current first");
        symlink("versions/0.9.0-bbbbbbbb", prefix.join("current")).expect("link current second");
        assert_eq!(
            resolve_identity_root_from_executable_dir(&prefix.join("current/bin")),
            Some(prefix.clone()),
            "downgrades retain the stable prefix"
        );
        fs::remove_file(prefix.join("current")).expect("remove current second");
        symlink("versions/1.0.0-aaaaaaaa", prefix.join("current")).expect("link current retry");
        assert_eq!(
            resolve_identity_root_from_executable_dir(&prefix.join("current/bin")),
            Some(prefix.clone()),
            "retries retain the stable prefix"
        );
        fs::remove_dir_all(root).expect("cleanup versioned prefix");
    }

    #[test]
    fn identity_root_uses_the_checkout_root() {
        let root = unique_temp("identity-checkout-root");
        let checkout = root.join("checkout");
        let executable_dir = checkout.join(".venv/bin");
        fs::create_dir_all(&executable_dir).expect("create checkout bin");
        fs::create_dir_all(checkout.join(".git")).expect("create checkout git marker");
        fs::write(checkout.join("pyproject.toml"), "").expect("write checkout marker");
        write_layout_anchors(&checkout.join(CHECKOUT_PAYLOAD_ROOT));
        assert_eq!(
            resolve_identity_root_from_executable_dir(&executable_dir),
            resolve_checkout_root_from_executable_dir(&executable_dir)
        );
        fs::remove_dir_all(root).expect("cleanup checkout root");
    }

    fn write_layout_anchors(share: &Path) {
        for relative in [
            LAYOUT_BUNDLE_ANCHOR,
            LAYOUT_LAYOUT_ANCHOR,
            LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            fs::create_dir_all(path.parent().expect("anchor parent")).expect("create anchor dir");
            fs::write(&path, relative).expect("write layout anchor");
        }
    }

    #[test]
    fn describe_installation_root_miss_names_the_paths_it_walked() {
        let root = unique_temp("install-root-miss");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("create bin");
        assert_eq!(resolve_installation_root_from_executable_dir(&bin), None);
        let text = describe_installation_root_miss(&bin);
        for token in [
            bin.to_str().expect("utf8 bin"),
            root.to_str().expect("utf8 ancestor"),
            "pyproject.toml",
            ".git",
            LAYOUT_BUNDLE_ANCHOR,
            LAYOUT_LAYOUT_ANCHOR,
            LAYOUT_TEMPLATE_ANCHOR,
        ] {
            assert!(
                text.contains(token),
                "diagnostic must contain {token:?}: {text}"
            );
        }
        fs::remove_dir_all(root).expect("cleanup install-root-miss");
    }

    #[test]
    fn layout_install_root_requires_three_regular_file_anchors() {
        let root = unique_temp("layout-anchors");
        let prefix = root.join("tree");
        let bin = prefix.join("bin");
        let share = prefix.join("share");
        fs::create_dir_all(&bin).expect("create bin");
        assert_eq!(resolve_installation_root_from_executable_dir(&bin), None);

        write_layout_anchors(&share);
        assert_eq!(
            resolve_installation_root_from_executable_dir(&bin),
            Some(share.clone())
        );

        for relative in [
            LAYOUT_BUNDLE_ANCHOR,
            LAYOUT_LAYOUT_ANCHOR,
            LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            let bytes = fs::read(&path).expect("read anchor");
            fs::remove_file(&path).expect("remove one anchor");
            assert_eq!(
                resolve_installation_root_from_executable_dir(&bin),
                None,
                "{relative} must be required"
            );
            fs::write(&path, &bytes).expect("restore anchor");
            assert_eq!(
                resolve_installation_root_from_executable_dir(&bin),
                Some(share.clone())
            );
            fs::remove_file(&path).expect("remove for directory stand-in");
            fs::create_dir(&path).expect("directory is not a regular file");
            assert_eq!(
                resolve_installation_root_from_executable_dir(&bin),
                None,
                "{relative} must be a regular file"
            );
            fs::remove_dir(&path).expect("remove directory stand-in");
            fs::write(&path, &bytes).expect("restore file");
        }

        assert_eq!(
            resolve_installation_root_from_executable_dir(&prefix.join("bin")),
            Some(share.clone())
        );
        let neighbor = root.join("neighbor/share");
        write_layout_anchors(&neighbor);
        assert_eq!(
            resolve_installation_root_from_executable_dir(&bin),
            Some(share.clone())
        );
        fs::remove_dir_all(root).expect("cleanup layout anchors");
    }

    #[test]
    fn layout_install_root_uses_immediate_parent_only_and_survives_relocate() {
        let root = unique_temp("layout-relocate");
        let prefix = root.join("arbitrary-name");
        let bin = prefix.join("bin");
        fs::create_dir_all(&bin).expect("create bin");
        write_layout_anchors(&prefix.join("share"));
        assert_eq!(
            resolve_installation_root_from_executable_dir(&bin),
            Some(prefix.join("share"))
        );
        assert_eq!(
            resolve_installation_root_from_executable_dir(&bin.join("nested")),
            None,
            "must not walk ancestors for the layout candidate"
        );

        let moved = root.join("relocated-name");
        fs::rename(&prefix, &moved).expect("relocate tree without rewriting files");
        assert_eq!(
            resolve_installation_root_from_executable_dir(&moved.join("bin")),
            Some(moved.join("share"))
        );
        fs::remove_dir_all(root).expect("cleanup relocated layout");
    }

    #[test]
    fn site_packages_and_checkout_still_precede_layout_install() {
        let root = unique_temp("layout-precedence");
        let prefix = root.join("prefix");
        let bin = prefix.join("bin");
        fs::create_dir_all(&bin).expect("create bin");
        write_layout_anchors(&prefix.join("share"));
        let site_packages = prefix.join("lib/python3.12/site-packages");
        fs::create_dir_all(site_packages.join("solstone")).expect("site package");
        fs::write(site_packages.join("solstone/__init__.py"), "").expect("init");
        assert_eq!(
            resolve_installation_root_from_executable_dir(&bin),
            Some(fs::canonicalize(&site_packages).expect("canonical site-packages"))
        );

        let checkout = root.join("checkout");
        let checkout_bin = checkout.join("bin");
        fs::create_dir_all(&checkout_bin).expect("checkout bin");
        fs::create_dir_all(checkout.join("solstone")).expect("checkout package");
        fs::create_dir_all(checkout.join(".git")).expect("checkout marker");
        fs::write(checkout.join("pyproject.toml"), "").expect("pyproject");
        write_layout_anchors(&checkout.join("share"));
        write_layout_anchors(&checkout.join(CHECKOUT_PAYLOAD_ROOT));
        assert_eq!(
            resolve_installation_root_from_executable_dir(&checkout_bin),
            Some(checkout.join(CHECKOUT_PAYLOAD_ROOT)),
            "the checkout candidate still precedes the layout install candidate"
        );
        fs::remove_dir_all(root).expect("cleanup precedence");
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

    #[cfg(unix)]
    #[test]
    fn ensure_journal_dir_reports_read_only_parent() {
        let root = unique_temp("ensure-read-only");
        let parent = root.join("parent");
        fs::create_dir_all(&parent).expect("create parent");
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

    #[cfg(windows)]
    #[test]
    fn ensure_journal_dir_reports_non_directory_parent() {
        // Windows' read-only bit does not deny directory creation. A file in the
        // parent position is a deterministic Windows create-directory failure.
        let root = unique_temp("ensure-non-directory-parent");
        fs::create_dir_all(&root).expect("create root");
        let parent = root.join("parent-file");
        fs::write(&parent, "not a directory").expect("write parent file");
        let target = parent.join("journal");
        let error = ensure_journal_dir(&target, Source::Source)
            .expect_err("non-directory parent should error");
        assert_eq!(error.source, "source");
        assert_eq!(error.path, target);
        fs::remove_dir_all(root).expect("cleanup root");
    }

    fn expand_token(value: &str, root: &Path) -> String {
        value.replace("$TMP", &root.as_os_str().to_string_lossy())
    }

    fn expand_toml_token(value: &str, root: &Path) -> String {
        let escaped = root
            .as_os_str()
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        value.replace("$TMP", &escaped)
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
                    let value = expand_toml_token(mapping_string(&case["config"], "value"), &root);
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
