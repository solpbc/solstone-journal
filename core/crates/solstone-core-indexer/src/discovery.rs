// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use glob::{GlobError, Pattern, PatternError, glob};

use solstone_core_format::content::{PatternRoot, patterns_for_root};
use solstone_core_format::paths::{CHRONICLE_DIR, resolve_journal_path};

#[derive(Debug)]
pub enum DiscoveryError {
    NonUtf8Root(PathBuf),
    NonUtf8Relative(PathBuf),
    Pattern(PatternError),
    Glob(GlobError),
    StripPrefix { path: PathBuf, root: PathBuf },
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
}
