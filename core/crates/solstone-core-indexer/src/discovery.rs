// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glob::{GlobError, Pattern, PatternError, glob};

use solstone_core_format::content::{PatternRoot, patterns_for_root};
use solstone_core_format::paths::{CHRONICLE_DIR, resolve_journal_path};

#[derive(Debug)]
pub enum DiscoveryError {
    NonUtf8Root(PathBuf),
    NonUtf8Relative(PathBuf),
    Pattern(PatternError),
    Glob(GlobError),
    Listing {
        path: PathBuf,
        error: std::io::Error,
    },
    StripPrefix {
        path: PathBuf,
        root: PathBuf,
    },
    JournalPath(solstone_core_format::paths::JournalPathError),
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiscoveryError::NonUtf8Root(path) => {
                write!(
                    formatter,
                    "glob root is not valid UTF-8: {}",
                    path.display()
                )
            }
            DiscoveryError::NonUtf8Relative(path) => {
                write!(
                    formatter,
                    "discovered path is not valid UTF-8: {}",
                    path.display()
                )
            }
            DiscoveryError::Pattern(error) => write!(formatter, "invalid glob pattern: {error}"),
            DiscoveryError::Glob(error) => write!(formatter, "glob traversal failed: {error}"),
            DiscoveryError::Listing { path, error } => write!(
                formatter,
                "glob traversal failed: attempting to read `{}` resulted in an error: {error}",
                path.display()
            ),
            DiscoveryError::StripPrefix { path, root } => write!(
                formatter,
                "discovered path {} is not under root {}",
                path.display(),
                root.display()
            ),
            DiscoveryError::JournalPath(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<solstone_core_format::paths::JournalPathError> for DiscoveryError {
    fn from(error: solstone_core_format::paths::JournalPathError) -> Self {
        Self::JournalPath(error)
    }
}

impl std::error::Error for DiscoveryError {}

impl From<PatternError> for DiscoveryError {
    fn from(error: PatternError) -> Self {
        DiscoveryError::Pattern(error)
    }
}

impl From<GlobError> for DiscoveryError {
    fn from(error: GlobError) -> Self {
        DiscoveryError::Glob(error)
    }
}

pub fn discover_indexable_files(
    journal: &Path,
) -> Result<BTreeMap<String, PathBuf>, DiscoveryError> {
    let mut files = BTreeMap::new();
    for spec in patterns_for_root(PatternRoot::Structural) {
        discover_from_root(journal, journal, spec.pattern, &mut files)?;
    }

    let chronicle = journal.join(CHRONICLE_DIR);
    let day_root = if chronicle.is_dir() {
        chronicle.as_path()
    } else {
        journal
    };
    for spec in patterns_for_root(PatternRoot::DayRooted) {
        discover_from_root(day_root, day_root, spec.pattern, &mut files)?;
    }
    Ok(files)
}

struct CompiledStep<'a> {
    is_list: bool,
    source: &'a str,
    pattern: Pattern,
}

#[derive(Clone, Debug)]
struct Listed {
    path: PathBuf,
    name: OsString,
    is_directory: bool,
}

#[cfg(test)]
thread_local! {
    static RECORDED_LISTINGS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn reset_recorded_listings() {
    RECORDED_LISTINGS.with(|r| r.borrow_mut().clear());
}

#[cfg(test)]
pub(crate) fn take_recorded_listings() -> Vec<PathBuf> {
    RECORDED_LISTINGS.with(|r| r.borrow_mut().clone())
}

fn list_dir(
    path: &Path,
    cache: &mut HashMap<PathBuf, Arc<[Listed]>>,
) -> Result<Arc<[Listed]>, DiscoveryError> {
    if let Some(cached) = cache.get(path) {
        return Ok(Arc::clone(cached));
    }

    #[cfg(test)]
    RECORDED_LISTINGS.with(|recorded| {
        recorded.borrow_mut().push(path.to_path_buf());
    });

    let read_dir = fs::read_dir(path).map_err(|error| DiscoveryError::Listing {
        path: path.to_path_buf(),
        error,
    })?;

    let mut entries = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|error| DiscoveryError::Listing {
            path: path.to_path_buf(),
            error,
        })?;
        let filename = entry.file_name();
        let child_path = if path == Path::new(".") {
            PathBuf::from(&filename)
        } else {
            path.join(&filename)
        };
        let is_directory = entry
            .file_type()
            .ok()
            .and_then(|ft| {
                if ft.is_symlink() {
                    None
                } else {
                    Some(ft.is_dir())
                }
            })
            .or_else(|| fs::metadata(&child_path).map(|m| m.is_dir()).ok())
            .unwrap_or(false);
        entries.push(Listed {
            path: child_path,
            name: filename,
            is_directory,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let snapshot: Arc<[Listed]> = Arc::from(entries);
    cache.insert(path.to_path_buf(), Arc::clone(&snapshot));
    Ok(snapshot)
}

fn strip_relative<'a>(path: &'a Path, rel_root: &Path) -> Result<&'a Path, ()> {
    let rel_root_clean = glob_root_path(rel_root);
    if let Ok(rel) = path.strip_prefix(&rel_root_clean) {
        return Ok(rel);
    }
    if rel_root_clean == Path::new(".") {
        return Ok(path);
    }
    if let Ok(rel) = rel_root_clean
        .strip_prefix(".")
        .and_then(|cleaned| path.strip_prefix(cleaned))
    {
        return Ok(rel);
    }
    Err(())
}

fn walk_pattern(
    current_path: &Path,
    current_is_dir: bool,
    comp_idx: usize,
    steps: &[CompiledStep<'_>],
    rel_root: &Path,
    cache: &mut HashMap<PathBuf, Arc<[Listed]>>,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), DiscoveryError> {
    let step = &steps[comp_idx];
    let is_last = comp_idx + 1 == steps.len();

    if step.is_list {
        if !current_is_dir {
            return Ok(());
        }
        let entries = list_dir(current_path, cache)?;
        for entry in entries.iter() {
            let Some(name_str) = entry.name.to_str() else {
                continue;
            };
            if !step.pattern.matches(name_str) {
                continue;
            }
            if is_last {
                if entry.path.is_file() {
                    let rel_root_clean = glob_root_path(rel_root);
                    let rel_path = strip_relative(&entry.path, rel_root).map_err(|()| {
                        DiscoveryError::StripPrefix {
                            path: entry.path.clone(),
                            root: rel_root_clean.clone(),
                        }
                    })?;
                    let rel = path_to_posix(rel_path)
                        .ok_or_else(|| DiscoveryError::NonUtf8Relative(entry.path.clone()))?;
                    files.insert(rel, entry.path.clone());
                }
            } else if entry.is_directory {
                walk_pattern(
                    &entry.path,
                    true,
                    comp_idx + 1,
                    steps,
                    rel_root,
                    cache,
                    files,
                )?;
            }
        }
    } else {
        let child_path = if current_path == Path::new(".") {
            PathBuf::from(step.source)
        } else {
            current_path.join(step.source)
        };
        let meta = fs::metadata(&child_path);
        let symlink_meta = fs::symlink_metadata(&child_path);
        if meta.is_err() && symlink_meta.is_err() {
            return Ok(());
        }
        let is_dir = meta.map(|m| m.is_dir()).unwrap_or(false);
        if is_last {
            if child_path.is_file() {
                let rel_root_clean = glob_root_path(rel_root);
                let rel_path = strip_relative(&child_path, rel_root).map_err(|()| {
                    DiscoveryError::StripPrefix {
                        path: child_path.clone(),
                        root: rel_root_clean.clone(),
                    }
                })?;
                let rel = path_to_posix(rel_path)
                    .ok_or_else(|| DiscoveryError::NonUtf8Relative(child_path.clone()))?;
                files.insert(rel, child_path);
            }
        } else if is_dir {
            walk_pattern(
                &child_path,
                true,
                comp_idx + 1,
                steps,
                rel_root,
                cache,
                files,
            )?;
        }
    }
    Ok(())
}

pub fn discover_index_status_files(
    journal: &Path,
) -> Result<BTreeMap<String, PathBuf>, DiscoveryError> {
    if journal.to_str().is_none() {
        return Err(DiscoveryError::NonUtf8Root(journal.to_path_buf()));
    }

    let journal_clean = glob_root_path(journal);
    let normalized = journal_clean.to_str().expect("UTF-8 root checked above");
    // Escaped or relative prefixes retain the original glob's traversal and
    // error behavior. Only absolute literal prefixes use shared listings.
    if !journal_clean.is_absolute() || Pattern::escape(normalized) != normalized {
        return discover_indexable_files(journal);
    }

    let mut files = BTreeMap::new();
    let mut listing_cache = HashMap::new();
    let journal_scope = if journal == Path::new(".") {
        PathBuf::from(".")
    } else {
        journal_clean.clone()
    };

    // Pattern order plus ascending DFS is what selects the first listing error.
    if journal_clean.is_dir() {
        for spec in patterns_for_root(PatternRoot::Structural) {
            let components: Vec<&str> = spec.pattern.split('/').collect();
            let mut steps = Vec::with_capacity(components.len());
            for comp in components {
                let is_list = comp.contains(['*', '?', '[']);
                let pat = Pattern::new(comp)?;
                steps.push(CompiledStep {
                    is_list,
                    source: comp,
                    pattern: pat,
                });
            }
            walk_pattern(
                &journal_scope,
                true,
                0,
                &steps,
                journal,
                &mut listing_cache,
                &mut files,
            )?;
        }
    }

    let chronicle = if journal == Path::new(".") {
        PathBuf::from(CHRONICLE_DIR)
    } else {
        journal_clean.join(CHRONICLE_DIR)
    };
    let (day_root_scope, rel_root) = if chronicle.is_dir() {
        (chronicle.clone(), chronicle.as_path())
    } else {
        (journal_scope, journal)
    };

    if day_root_scope.is_dir() {
        for spec in patterns_for_root(PatternRoot::DayRooted) {
            let components: Vec<&str> = spec.pattern.split('/').collect();
            let mut steps = Vec::with_capacity(components.len());
            for comp in components {
                let is_list = comp.contains(['*', '?', '[']);
                let pat = Pattern::new(comp)?;
                steps.push(CompiledStep {
                    is_list,
                    source: comp,
                    pattern: pat,
                });
            }
            walk_pattern(
                &day_root_scope,
                true,
                0,
                &steps,
                rel_root,
                &mut listing_cache,
                &mut files,
            )?;
        }
    }

    Ok(files)
}

/// Discover exactly the talent Markdown source set formerly folded into one
/// segment aggregate. Keep these patterns aligned with the DayRooted Markdown
/// registry: `*/*/*/talents/*.md` and `*/*/*/talents/*/*.md`.
pub fn discover_segment_talent_markdown_files(
    journal: &Path,
    rel_segment: &str,
) -> Result<Vec<(String, PathBuf)>, DiscoveryError> {
    let segment_dir = resolve_journal_path(journal, rel_segment)?;
    let mut files = Vec::new();
    for suffix in ["talents/*.md", "talents/*/*.md"] {
        for entry in glob(&rooted_pattern(&segment_dir, suffix)?)? {
            let path = entry?;
            if !path.is_file() {
                continue;
            }
            let segment_root = glob_root_path(&segment_dir);
            let suffix =
                path.strip_prefix(&segment_root)
                    .map_err(|_error| DiscoveryError::StripPrefix {
                        path: path.clone(),
                        root: segment_root.clone(),
                    })?;
            let suffix = path_to_posix(suffix)
                .ok_or_else(|| DiscoveryError::NonUtf8Relative(path.clone()))?;
            files.push((format!("{rel_segment}/{suffix}"), path));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files.dedup_by(|left, right| left.0 == right.0);
    Ok(files)
}

pub(crate) fn discover_from_root(
    root: &Path,
    rel_root: &Path,
    pattern: &str,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), DiscoveryError> {
    let full_pattern = rooted_pattern(root, pattern)?;
    for entry in glob(&full_pattern)? {
        let path = entry?;
        if !path.is_file() {
            continue;
        }
        let rel_root = glob_root_path(rel_root);
        let rel_path =
            path.strip_prefix(&rel_root)
                .map_err(|_error| DiscoveryError::StripPrefix {
                    path: path.clone(),
                    root: rel_root.clone(),
                })?;
        let rel =
            path_to_posix(rel_path).ok_or_else(|| DiscoveryError::NonUtf8Relative(path.clone()))?;
        files.insert(rel, path);
    }
    Ok(())
}

/// The spelling of a journal root that `glob` can match through.
///
/// 🔴 On Windows the journal root arrives canonicalized in the verbatim form
/// `\\?\C:\...`. `Pattern::escape` spells that prefix `\\[?]\`, which matches
/// nothing on disk (measured on the Windows builder 2026-09-23), so discovery
/// found no files at all and a full rescan emptied the index ("Indexed 0 file(s),
/// removed 4", measured on the Windows checkpoint guest 2026-09-23). A verbatim
/// LOCAL DRIVE path is the same path without the prefix; `\\?\UNC\` and every
/// other form are left alone.
pub(crate) fn glob_root(root: &str) -> &str {
    match root.strip_prefix(r"\\?\") {
        Some(rest)
            if rest.len() >= 3
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':'
                && rest.as_bytes()[2] == b'\\' =>
        {
            rest
        }
        _ => root,
    }
}

fn glob_root_path(root: &Path) -> PathBuf {
    match root.to_str() {
        Some(text) => PathBuf::from(glob_root(text)),
        None => root.to_path_buf(),
    }
}

pub(crate) fn rooted_pattern(root: &Path, pattern: &str) -> Result<String, DiscoveryError> {
    let root = root
        .to_str()
        .ok_or_else(|| DiscoveryError::NonUtf8Root(root.to_path_buf()))?;
    let escaped = Pattern::escape(glob_root(root));
    let separator = if escaped.ends_with('/') { "" } else { "/" };
    Ok(format!("{escaped}{separator}{pattern}"))
}

pub(crate) fn path_to_posix(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.components() {
        parts.push(part.as_os_str().to_str()?);
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use std::fs;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-{name}"))
    }

    fn write(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create parent");
        fs::write(path, "# Title\n\nbody\n").expect("write test file");
    }

    #[test]
    fn a_verbatim_local_drive_root_is_globbed_without_its_prefix() {
        assert_eq!(
            glob_root(r"\\?\C:\Users\Owner\journal-Zoë-日誌"),
            r"C:\Users\Owner\journal-Zoë-日誌"
        );
        assert_eq!(glob_root(r"\\?\d:\j"), r"d:\j");
        assert_eq!(
            glob_root(r"C:\Users\Owner\journal"),
            r"C:\Users\Owner\journal"
        );
        assert_eq!(glob_root("/home/owner/journal"), "/home/owner/journal");
        assert_eq!(
            glob_root(r"\\?\UNC\server\share\journal"),
            r"\\?\UNC\server\share\journal"
        );
        assert_eq!(
            glob_root(r"\\?\Volume{0}\journal"),
            r"\\?\Volume{0}\journal"
        );
        assert_eq!(glob_root(r"\\?\C:"), r"\\?\C:");
    }

    #[cfg(windows)]
    #[test]
    fn a_canonicalized_windows_journal_root_still_discovers_its_files() {
        let root = temp_root("verbatim-root");
        write(&root, "chronicle/20260101/import.ics/imported.jsonl");
        let canonical = root.canonicalize().expect("canonical root");
        assert!(canonical.to_str().unwrap().starts_with(r"\\?\"));
        let found = discover_indexable_files(&canonical).expect("discovery");
        assert!(
            found.contains_key("20260101/import.ics/imported.jsonl"),
            "a verbatim root discovered {found:?}"
        );
    }

    #[test]
    fn discovers_chronicle_free_indexable_rels() {
        let root = temp_root("discover");
        write(&root, "chronicle/20240101/talents/flow.md");
        write(&root, "chronicle/.hidden/talents/secret.md");
        write(
            &root,
            "chronicle/20240101/default/123456_300/talents/audio.md",
        );
        write(
            &root,
            "chronicle/20240101/default/123456_300/talents/documents.json",
        );
        write(
            &root,
            "chronicle/20240101/default/123456_300/talents/screen.json",
        );
        write(
            &root,
            "chronicle/20240101/default/123456_300/talents/sense.json",
        );
        write(
            &root,
            "chronicle/20240101/default/123456_300/talents/morning_briefing.json",
        );
        write(&root, "chronicle/20240101/talents/morning_briefing.json");
        write(&root, "chronicle/20240101/location.md");
        write(&root, "chronicle/20240101/phone/123456_300/location.jsonl");
        write(
            &root,
            "chronicle/20260101/import.ics/090000_300/event_transcript.md",
        );
        write(&root, "facets/work/news/20240101.md");
        write(&root, "imports/20260101_120000/summary.md");
        write(&root, "config/actions/20240101.jsonl");
        write(&root, "facets/work/events/20240101.jsonl");
        write(&root, "facets/work/entities/20260304.jsonl");
        write(
            &root,
            "facets/work/entities/alice_johnson/observations.jsonl",
        );
        write(&root, "facets/work/activities/20240101.jsonl");
        write(&root, "facets/work/logs/20240101.jsonl");
        write(&root, "chronicle/20240101/default/123456_300/audio.jsonl");
        write(
            &root,
            "chronicle/20240101/default/123456_300/left_audio.jsonl",
        );
        write(
            &root,
            "chronicle/20240101/default/123456_300/left_transcript.jsonl",
        );
        write(&root, "chronicle/20240101/default/123456_300/screen.jsonl");
        write(
            &root,
            "chronicle/20240101/default/123456_300/left_screen.jsonl",
        );
        write(&root, "entities/alice/entity.json");

        let files = discover_indexable_files(&root).expect("discover files");
        let rels: Vec<_> = files.keys().cloned().collect();
        assert_eq!(
            rels,
            vec![
                ".hidden/talents/secret.md",
                "20240101/default/123456_300/talents/audio.md",
                "20240101/default/123456_300/talents/documents.json",
                "20240101/default/123456_300/talents/screen.json",
                "20240101/default/123456_300/talents/sense.json",
                "20240101/location.md",
                "20240101/talents/flow.md",
                "20240101/talents/morning_briefing.json",
                "20260101/import.ics/090000_300/event_transcript.md",
                "config/actions/20240101.jsonl",
                "facets/work/activities/20240101.jsonl",
                "facets/work/entities/20260304.jsonl",
                "facets/work/entities/alice_johnson/observations.jsonl",
                "facets/work/events/20240101.jsonl",
                "facets/work/logs/20240101.jsonl",
                "facets/work/news/20240101.md",
                "imports/20260101_120000/summary.md",
            ]
        );
        assert!(!files.contains_key("20240101/default/123456_300/talents/morning_briefing.json"));
        for unindexed in [
            "20240101/default/123456_300/audio.jsonl",
            "20240101/default/123456_300/left_audio.jsonl",
            "20240101/default/123456_300/left_transcript.jsonl",
            "20240101/default/123456_300/screen.jsonl",
            "20240101/default/123456_300/left_screen.jsonl",
            "20240101/phone/123456_300/location.jsonl",
            "entities/alice/entity.json",
        ] {
            assert!(
                !files.contains_key(unindexed),
                "known-unindexed pattern leaked into discovery: {unindexed}"
            );
        }
        fs::remove_dir_all(root).expect("cleanup discover root");
    }

    #[test]
    fn discovers_talent_json_from_chronicle_less_root() {
        let root = temp_root("discover-talent-json-rootless");
        write(&root, "20240101/default/123456_300/talents/documents.json");
        write(&root, "20240101/default/123456_300/talents/screen.json");
        write(&root, "20240101/default/123456_300/talents/sense.json");
        write(
            &root,
            "20240101/default/123456_300/talents/morning_briefing.json",
        );
        write(&root, "20240101/talents/morning_briefing.json");

        let files = discover_indexable_files(&root).expect("discover files");
        assert!(files.contains_key("20240101/default/123456_300/talents/documents.json"));
        assert!(files.contains_key("20240101/default/123456_300/talents/screen.json"));
        assert!(files.contains_key("20240101/default/123456_300/talents/sense.json"));
        assert!(files.contains_key("20240101/talents/morning_briefing.json"));
        assert!(!files.contains_key("20240101/default/123456_300/talents/morning_briefing.json"));
        fs::remove_dir_all(root).expect("cleanup discover rootless root");
    }

    #[test]
    fn discovers_import_content_families() {
        let root = temp_root("discover-imports");
        write(&root, "chronicle/20260101/import.ics/imported.jsonl");
        write(
            &root,
            "chronicle/20260101/import.claude/thread_a/conversation_transcript.jsonl",
        );
        write(
            &root,
            "chronicle/20260101/import.chatgpt/conv_b/imported_audio.jsonl",
        );

        let files = discover_indexable_files(&root).expect("discover files");
        assert!(files.contains_key("20260101/import.ics/imported.jsonl"));
        assert!(
            files.contains_key("20260101/import.claude/thread_a/conversation_transcript.jsonl")
        );
        assert!(files.contains_key("20260101/import.chatgpt/conv_b/imported_audio.jsonl"));
        fs::remove_dir_all(root).expect("cleanup discover imports root");
    }

    #[test]
    fn facet_entity_pattern_does_not_capture_observation_paths() {
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        let observation_path = Path::new("facets/work/entities/alice/observations.jsonl");
        assert!(
            Pattern::new("facets/*/entities/*/observations.jsonl")
                .expect("valid observation pattern")
                .matches_path_with(observation_path, options)
        );
        assert!(
            !Pattern::new("facets/*/entities/*.jsonl")
                .expect("valid entity pattern")
                .matches_path_with(observation_path, options)
        );
    }

    #[test]
    fn status_walk_equals_oracle_with_chronicle() {
        let root = temp_root("status-with-chronicle");
        write(&root, "chronicle/20240101/talents/flow.md");
        write(
            &root,
            "chronicle/20240101/default/123456_300/talents/sense.json",
        );
        write(&root, "chronicle/20240101/default/123456_300/audio.jsonl");
        write(&root, "facets/work/news/20240101.md");
        write(&root, "entities/alice/entity.json");

        let oracle = discover_indexable_files(&root).expect("oracle");
        let status = discover_index_status_files(&root).expect("status");
        assert_eq!(status, oracle);
        assert!(status.contains_key("20240101/default/123456_300/talents/sense.json"));
        assert!(!status.contains_key("20240101/default/123456_300/audio.jsonl"));
        assert!(!status.contains_key("entities/alice/entity.json"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn status_walk_equals_oracle_without_chronicle() {
        let root = temp_root("status-without-chronicle");
        write(&root, "20240101/talents/flow.md");
        write(&root, "20240101/default/123456_300/talents/sense.json");
        write(&root, "facets/work/news/20240101.md");

        let oracle = discover_indexable_files(&root).expect("oracle");
        let status = discover_index_status_files(&root).expect("status");
        assert_eq!(status, oracle);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn status_walk_equals_oracle_with_metacharacter_root() {
        let parent = temp_root("status-meta-root");
        let root = parent.join(if cfg!(windows) {
            "journal-[meta]"
        } else {
            "journal-[meta]?*"
        });
        fs::create_dir_all(&root).expect("create meta root");
        write(&root, "chronicle/20240101/talents/flow.md");
        write(&root, "facets/work/news/20240101.md");

        let oracle = discover_indexable_files(&root).expect("oracle");
        let status = discover_index_status_files(&root).expect("status");
        assert_eq!(status, oracle);
        fs::remove_dir_all(parent).expect("cleanup");
    }

    #[test]
    fn status_walk_equals_oracle_with_hidden_day_component() {
        let root = temp_root("status-hidden-day");
        write(&root, "chronicle/.hidden/talents/secret.md");

        let oracle = discover_indexable_files(&root).expect("oracle");
        let status = discover_index_status_files(&root).expect("status");
        assert_eq!(status, oracle);
        assert!(status.contains_key(".hidden/talents/secret.md"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    fn generate_match_and_near_miss(pattern: &str, idx: usize) -> (String, String) {
        let components: Vec<&str> = pattern.split('/').collect();
        let mut match_parts = Vec::new();
        let mut near_miss_parts = Vec::new();

        for (c_idx, comp) in components.iter().enumerate() {
            let is_last = c_idx + 1 == components.len();
            let match_comp = if *comp == "*" {
                format!("mm{idx}")
            } else if *comp == "import.*" {
                format!("import.mm{idx}")
            } else if comp.starts_with("agent-memory-") {
                format!("agent-memory-{}", "0".repeat(64))
            } else if *comp == "*.md" {
                format!("file_{idx}.md")
            } else if *comp == "*.jsonl" {
                format!("file_{idx}.jsonl")
            } else if comp.ends_with("_transcript.md") {
                format!("clip_{idx}_transcript.md")
            } else if comp.starts_with("browser_") && comp.ends_with(".jsonl") {
                format!("browser_{idx}.jsonl")
            } else if comp.ends_with("_protected_video.jsonl") {
                format!("vid_{idx}_protected_video.jsonl")
            } else if comp.ends_with("_desktop_activity.jsonl") {
                format!("note_{idx}_desktop_activity.jsonl")
            } else {
                comp.to_string()
            };

            let near_miss_comp = if is_last {
                format!("near-miss-{idx}")
            } else {
                match_comp.clone()
            };

            match_parts.push(match_comp);
            near_miss_parts.push(near_miss_comp);
        }

        (match_parts.join("/"), near_miss_parts.join("/"))
    }

    #[test]
    fn status_walk_equals_oracle_for_every_registry_pattern_and_near_miss() {
        let root = temp_root("every-pattern-near-miss");
        let mut match_keys = Vec::new();
        let mut near_miss_keys = Vec::new();

        for (idx, spec) in patterns_for_root(PatternRoot::Structural).enumerate() {
            let (rel_match, rel_near_miss) = generate_match_and_near_miss(spec.pattern, idx);
            write(&root, &rel_match);
            write(&root, &rel_near_miss);
            match_keys.push(rel_match);
            near_miss_keys.push(rel_near_miss);
        }

        for (idx, spec) in patterns_for_root(PatternRoot::DayRooted).enumerate() {
            let offset = 100 + idx;
            let (rel_match, rel_near_miss) = generate_match_and_near_miss(spec.pattern, offset);
            write(&root, &format!("chronicle/{rel_match}"));
            write(&root, &format!("chronicle/{rel_near_miss}"));
            match_keys.push(rel_match);
            near_miss_keys.push(rel_near_miss);
        }

        let oracle = discover_indexable_files(&root).expect("oracle");
        let status = discover_index_status_files(&root).expect("status");

        for key in &match_keys {
            assert!(oracle.contains_key(key), "oracle missing match key: {key}");
        }
        for key in &near_miss_keys {
            assert!(
                !oracle.contains_key(key),
                "oracle contains near-miss key: {key}"
            );
        }
        assert_eq!(status, oracle);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn status_walk_equals_oracle_overlapping_browser_protected_video() {
        let root = temp_root("overlapping-browser-protected");
        write(
            &root,
            "chronicle/20240101/stream/segment/browser_clip_protected_video.jsonl",
        );
        let oracle = discover_indexable_files(&root).expect("oracle");
        let status = discover_index_status_files(&root).expect("status");
        assert!(oracle.contains_key("20240101/stream/segment/browser_clip_protected_video.jsonl"));
        assert_eq!(status, oracle);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn status_walk_caches_directory_listings_and_never_lists_unmatched_or_journal_root() {
        let root = temp_root("status-cache-record");
        write(&root, "chronicle/20240101/aaa/seg/talents/sense.json");
        write(&root, "chronicle/20240101/aaa/seg/browser_x.jsonl");
        write(&root, "chronicle/20240101/bbb/seg/talents/sense.json");
        write(&root, "chronicle/20240101/bbb/seg/browser_x.jsonl");
        write(&root, "chronicle/20240102/aaa/seg/talents/sense.json");
        write(&root, "chronicle/20240102/aaa/seg/browser_x.jsonl");
        write(&root, "chronicle/20240102/bbb/seg/talents/sense.json");
        write(&root, "chronicle/20240102/bbb/seg/browser_x.jsonl");
        write(&root, "facets/work/news/20240101.md");
        write(&root, "unmatched/nested/file.txt");

        reset_recorded_listings();
        let status = discover_index_status_files(&root).expect("status walk");
        assert!(status.contains_key("20240101/aaa/seg/browser_x.jsonl"));
        let recorded = take_recorded_listings();

        let chronicle_dir = root.join("chronicle");
        let day_1 = chronicle_dir.join("20240101");
        let day_2 = chronicle_dir.join("20240102");

        assert_eq!(
            recorded.iter().filter(|p| **p == chronicle_dir).count(),
            1,
            "chronicle listed once"
        );
        assert_eq!(
            recorded.iter().filter(|p| **p == day_1).count(),
            1,
            "day 1 listed once"
        );
        assert_eq!(
            recorded.iter().filter(|p| **p == day_2).count(),
            1,
            "day 2 listed once"
        );
        assert!(!recorded.contains(&root), "journal root must not be listed");
        let unmatched_dir = root.join("unmatched");
        assert!(
            !recorded.iter().any(|p| p.starts_with(&unmatched_dir)),
            "unmatched directory must never be listed"
        );
        let facet_work = root.join("facets/work");
        assert!(
            !recorded.contains(&facet_work),
            "facets/work must not be listed"
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn status_walk_reflects_filesystem_mutations_across_calls() {
        let root = temp_root("status-mutation");
        write(&root, "chronicle/20240101/talents/flow.md");
        write(&root, "chronicle/20240101/location.md");

        let first = discover_index_status_files(&root).expect("first call");
        assert!(first.contains_key("20240101/talents/flow.md"));
        assert!(first.contains_key("20240101/location.md"));

        write(&root, "facets/work/news/20240101.md");
        fs::remove_file(root.join("chronicle/20240101/location.md")).expect("remove");

        let second = discover_index_status_files(&root).expect("second call");
        let fresh_oracle = discover_indexable_files(&root).expect("fresh oracle");
        assert_eq!(second, fresh_oracle);
        assert!(second.contains_key("facets/work/news/20240101.md"));
        assert!(!second.contains_key("20240101/location.md"));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn status_walk_semantics_missing_directory_is_empty_absence() {
        let root = temp_root("status-missing-dir");
        assert_eq!(
            discover_index_status_files(&root).expect("missing root"),
            discover_indexable_files(&root).expect("missing root oracle")
        );
        fs::create_dir_all(&root).expect("create root");
        assert_eq!(
            discover_index_status_files(&root).expect("empty root"),
            discover_indexable_files(&root).expect("empty root oracle")
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn status_walk_semantics_file_where_dir_expected_is_absence() {
        let root = temp_root("file-where-dir-expected");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("config"), "not a dir\n").expect("write file");

        let status = discover_index_status_files(&root).expect("status");
        let oracle = discover_indexable_files(&root).expect("oracle");
        assert_eq!(status, oracle);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn status_walk_semantics_file_and_directory_symlinks_match_oracle() {
        let root = temp_root("status-symlinks");
        let external = temp_root("status-symlinks-external");
        fs::create_dir_all(&external).expect("external dir");

        let real_file = external.join("real_news.md");
        fs::write(&real_file, "# Real News\n").expect("write real file");

        let real_day = external.join("real_day");
        fs::create_dir_all(real_day.join("talents")).expect("real day talents");
        fs::write(real_day.join("talents/flow.md"), "# Flow\n").expect("write flow");

        fs::create_dir_all(root.join("facets/work/news")).expect("facets news");
        std::os::unix::fs::symlink(&real_file, root.join("facets/work/news/20240101.md"))
            .expect("symlink file");

        fs::create_dir_all(root.join("chronicle")).expect("chronicle");
        std::os::unix::fs::symlink(&real_day, root.join("chronicle/20240101"))
            .expect("symlink day dir");

        let status = discover_index_status_files(&root).expect("status");
        let oracle = discover_indexable_files(&root).expect("oracle");
        assert_eq!(status, oracle);
        assert!(status.contains_key("facets/work/news/20240101.md"));
        assert!(status.contains_key("20240101/talents/flow.md"));

        fs::remove_dir_all(root).expect("cleanup root");
        fs::remove_dir_all(external).expect("cleanup external");
    }

    #[cfg(all(unix, feature = "full-tests"))]
    struct TempRoot {
        path: PathBuf,
        restore: Vec<(PathBuf, u32)>,
    }

    #[cfg(all(unix, feature = "full-tests"))]
    impl TempRoot {
        fn new(name: &str) -> Self {
            Self {
                path: temp_root(name),
                restore: Vec::new(),
            }
        }

        fn restore_mode(&mut self, path: PathBuf, mode: u32) {
            self.restore.push((path, mode));
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    impl std::ops::Deref for TempRoot {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            &self.path
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    impl AsRef<Path> for TempRoot {
        fn as_ref(&self) -> &Path {
            &self.path
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    impl Drop for TempRoot {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            for (path, mode) in &self.restore {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(*mode));
            }
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn status_walk_semantics_execute_only_ancestor_matches_oracle() {
        use std::os::unix::fs::PermissionsExt;
        let mut root = TempRoot::new("execute-only-ancestor");
        write(&root, "config/actions/a.jsonl");
        fs::create_dir_all(root.join("chronicle")).expect("create chronicle dir");

        let config_dir = root.join("config");
        root.restore_mode(config_dir.clone(), 0o755);
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o111))
            .expect("chmod config 0o111");

        let status_a = discover_index_status_files(&root).expect("status case a");
        let oracle_a = discover_indexable_files(&root).expect("oracle case a");
        assert_eq!(status_a, oracle_a);
        assert!(status_a.contains_key("config/actions/a.jsonl"));

        let actions_dir = root.join("config/actions");
        root.restore_mode(actions_dir.clone(), 0o755);
        fs::set_permissions(&actions_dir, fs::Permissions::from_mode(0o111))
            .expect("chmod actions 0o111");

        if fs::read_dir(&actions_dir).is_ok() {
            eprintln!("skipping case (b): chmod 0o111 did not make directory unreadable");
            return;
        }

        let err_status = discover_index_status_files(&root).expect_err("status case b");
        let err_oracle = discover_indexable_files(&root).expect_err("oracle case b");

        let (status_path, status_kind, status_raw) = match &err_status {
            DiscoveryError::Listing { path, error } => (path, error.kind(), error.raw_os_error()),
            other => panic!("expected Listing, got {other:?}"),
        };
        let (oracle_path, oracle_kind, oracle_raw) = match &err_oracle {
            DiscoveryError::Glob(glob_err) => (
                glob_err.path(),
                glob_err.error().kind(),
                glob_err.error().raw_os_error(),
            ),
            other => panic!("expected Glob, got {other:?}"),
        };
        assert_eq!(status_path, oracle_path);
        assert_eq!(status_kind, oracle_kind);
        assert_eq!(status_raw, oracle_raw);
        assert_eq!(err_status.to_string(), err_oracle.to_string());
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn status_walk_semantics_first_unreadable_directory_error_matches_oracle() {
        use std::os::unix::fs::PermissionsExt;
        let mut root = TempRoot::new("first-unreadable-error");
        write(&root, "facets/a/entities/mm/observations.jsonl");
        write(&root, "facets/z/entities/mm/observations.jsonl");
        write(&root, "chronicle/20240101/location.md");

        let dir_a = root.join("facets/a/entities");
        let dir_z = root.join("facets/z/entities");
        let dir_day = root.join("chronicle/20240101");

        root.restore_mode(dir_a.clone(), 0o755);
        root.restore_mode(dir_z.clone(), 0o755);
        root.restore_mode(dir_day.clone(), 0o755);

        fs::set_permissions(&dir_a, fs::Permissions::from_mode(0o000)).expect("chmod a");
        fs::set_permissions(&dir_z, fs::Permissions::from_mode(0o000)).expect("chmod z");
        fs::set_permissions(&dir_day, fs::Permissions::from_mode(0o000)).expect("chmod day");

        if fs::read_dir(&dir_a).is_ok() {
            eprintln!("skipping: chmod 0o000 did not make facets/a/entities unreadable");
            return;
        }

        let err_status = discover_index_status_files(&root).expect_err("status must error");
        let err_oracle = discover_indexable_files(&root).expect_err("oracle must error");

        let (status_path, status_kind, status_raw) = match &err_status {
            DiscoveryError::Listing { path, error } => (path, error.kind(), error.raw_os_error()),
            other => panic!("expected Listing, got {other:?}"),
        };
        let (oracle_path, oracle_kind, oracle_raw) = match &err_oracle {
            DiscoveryError::Glob(glob_err) => (
                glob_err.path(),
                glob_err.error().kind(),
                glob_err.error().raw_os_error(),
            ),
            other => panic!("expected Glob, got {other:?}"),
        };
        assert_eq!(status_path, &dir_a);
        assert_eq!(status_path, oracle_path);
        assert_eq!(status_kind, oracle_kind);
        assert_eq!(status_raw, oracle_raw);
        assert_eq!(err_status.to_string(), err_oracle.to_string());
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn status_walk_semantics_unreadable_unmatched_directory_does_not_error() {
        use std::os::unix::fs::PermissionsExt;
        let mut root = TempRoot::new("unreadable-unmatched");
        write(&root, "facets/work/news/20240101.md");
        fs::create_dir_all(root.join("chronicle")).expect("create chronicle dir");
        let unmatched = root.join("unmatched");
        fs::create_dir_all(&unmatched).expect("create unmatched");
        fs::write(unmatched.join("ignored.txt"), "hello").expect("write file");

        root.restore_mode(unmatched.clone(), 0o755);
        let _ = fs::set_permissions(&unmatched, fs::Permissions::from_mode(0o000));

        let status =
            discover_index_status_files(&root).expect("status must not error on unmatched dir");
        let oracle =
            discover_indexable_files(&root).expect("oracle must not error on unmatched dir");
        assert_eq!(status, oracle);
        assert!(status.contains_key("facets/work/news/20240101.md"));
    }

    #[cfg(unix)]
    #[test]
    fn status_walk_semantics_non_utf8_filename_is_skipped() {
        use std::os::unix::ffi::OsStrExt;
        let root = temp_root("non-utf8-filename");
        write(&root, "config/actions/valid.jsonl");

        let bad_name = std::ffi::OsStr::from_bytes(&[0xff, 0xfe]);
        let bad_path = root.join("config/actions").join(bad_name);
        let _ = fs::write(&bad_path, "{}\n");

        let status = discover_index_status_files(&root).expect("status with non-utf8 filename");
        let oracle = discover_indexable_files(&root).expect("oracle with non-utf8 filename");
        assert_eq!(status, oracle);
        assert!(status.contains_key("config/actions/valid.jsonl"));
        let _ = fs::remove_file(&bad_path);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn status_walk_semantics_non_utf8_root_returns_non_utf8_root_error() {
        use std::os::unix::ffi::OsStrExt;
        let bad_root = PathBuf::from(std::ffi::OsStr::from_bytes(&[0x2f, 0xff, 0xfe]));
        let err_status = discover_index_status_files(&bad_root).expect_err("status non-utf8 root");
        let err_oracle = discover_indexable_files(&bad_root).expect_err("oracle non-utf8 root");

        assert!(matches!(err_status, DiscoveryError::NonUtf8Root(_)));
        assert!(matches!(err_oracle, DiscoveryError::NonUtf8Root(_)));
        assert_eq!(err_status.to_string(), err_oracle.to_string());
    }
}
