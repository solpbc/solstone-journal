// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Speaker chronicle segment catalog on journal-io locators.

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_journal_io::{
    ExactLookupError, PathError, PathOrDay, SegmentIdentityError, SegmentLayout,
    day_dirs as journal_day_dirs, iter_segments, resolve_segment_locator_exact,
};

/// Voice-approved Direct-layout refusal reason code for `err(...)`.
pub const UNSUPPORTED_LAYOUT_REASON: &str = "speaker_segment_layout_unsupported";
/// First line of the Direct-layout refusal (`err` message slot).
pub const UNSUPPORTED_LAYOUT_MESSAGE: &str = "This command can't change that speaker review.";
/// Second line of the Direct-layout refusal (`err` detail slot).
pub const UNSUPPORTED_LAYOUT_DETAIL: &str =
    "This segment uses the direct journal layout, which this command doesn't support.";

/// One cataloged chronicle segment with a resolver-validated path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogedSegment {
    /// Chronicle day (`YYYYMMDD`).
    pub day: String,
    /// Explicit on-disk layout. Never inferred from the stream spelling.
    pub layout: SegmentLayout,
    /// UTF-8 stream directory; Direct still spells `_default`.
    pub stream: String,
    /// Exact UTF-8 directory basename. Path identity for every lookup.
    pub name: String,
    /// Parsed `HHMMSS_LEN` time metadata. Display, sort, and start/end only.
    pub key: String,
    /// Path after a successful `resolve_segment_locator_exact` cross-check.
    pub path: PathBuf,
}

/// Failure while building a catalog snapshot.
#[derive(Debug)]
pub enum CatalogBuildError {
    /// Stream directory or segment basename is not UTF-8.
    NotUtf8 { path: PathBuf },
    /// Lossless locator identity was internally inconsistent.
    Identity(SegmentIdentityError),
    /// journal-io `day_dirs` / `iter_segments` failed.
    Walk(PathError),
    /// Required `resolve_segment_locator_exact` cross-check failed.
    Lookup(ExactLookupError),
    /// Discovered path did not match the exact resolver's result.
    PathMismatch {
        discovered: PathBuf,
        resolved: PathBuf,
    },
}

impl fmt::Display for CatalogBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 { path } => write!(
                formatter,
                "segment path is not UTF-8 representable: {}",
                path.display()
            ),
            Self::Identity(error) => write!(formatter, "invalid segment identity: {error}"),
            Self::Walk(error) => write!(formatter, "chronicle walk failed: {error}"),
            Self::Lookup(error) => write!(formatter, "segment locator cross-check failed: {error}"),
            Self::PathMismatch {
                discovered,
                resolved,
            } => write!(
                formatter,
                "catalog path mismatch: discovered {} resolved {}",
                discovered.display(),
                resolved.display()
            ),
        }
    }
}

impl Error for CatalogBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Walk(error) => Some(error),
            Self::Lookup(error) => Some(error),
            Self::NotUtf8 { .. } | Self::Identity(_) | Self::PathMismatch { .. } => None,
        }
    }
}

/// Rejected `stream_layout` spelling or JSON type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutDecodeError {
    /// Anything other than absent / `"direct"` / `"named"`.
    Malformed,
}

impl fmt::Display for LayoutDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("stream_layout must be \"direct\" or \"named\"")
    }
}

impl Error for LayoutDecodeError {}

/// Whether a Direct hit is admitted on this surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectSupport {
    /// GET-style reads may open a Direct segment.
    Allow,
    /// Mutations refuse Direct with [`UNSUPPORTED_LAYOUT_REASON`].
    Refuse,
}

/// Classified result of resolving one request identity.
#[derive(Debug)]
pub enum SegmentLookup {
    /// Admitted, resolver-validated directory.
    Present(PathBuf),
    /// Resolver returned `Ok(None)`.
    Absent,
    /// Well-formed Direct identity on a Named-only surface.
    UnsupportedLayout,
    /// Layout decode failed, or journal-io rejected the request shape.
    MalformedLayout,
    /// Resolver IO / containment / wrong-kind failure.
    Failed(ExactLookupError),
}

/// One multi-target lookup row, including a decoded (or failed) layout.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SegmentTarget<'a> {
    pub day: &'a str,
    pub stream: &'a str,
    pub name: &'a str,
    pub layout: Result<SegmentLayout, LayoutDecodeError>,
}

/// Catalog every representable segment on `day`.
///
/// Missing chronicle or an empty day is `Ok(vec![])`.
pub fn catalog_day(
    journal_root: &Path,
    day: &str,
) -> Result<Vec<CatalogedSegment>, CatalogBuildError> {
    let discovered =
        iter_segments(journal_root, PathOrDay::Day(day)).map_err(CatalogBuildError::Walk)?;
    let mut cataloged = Vec::with_capacity(discovered.len());
    for segment in discovered {
        let identity = match segment.locator_identity() {
            Ok(identity) => identity,
            Err(SegmentIdentityError::NotUtf8 { path }) => {
                return Err(CatalogBuildError::NotUtf8 { path });
            }
            Err(error @ SegmentIdentityError::AmbiguousNamedDefault { .. })
            | Err(error @ SegmentIdentityError::DuplicateKey { .. }) => {
                return Err(CatalogBuildError::Identity(error));
            }
        };
        let resolved = match resolve_exact(
            journal_root,
            day,
            identity.stream,
            identity.name,
            identity.layout,
        ) {
            Ok(Some(path)) => path,
            Ok(None) => {
                return Err(CatalogBuildError::PathMismatch {
                    discovered: segment.path().to_path_buf(),
                    resolved: PathBuf::new(),
                });
            }
            Err(error) => return Err(CatalogBuildError::Lookup(error)),
        };
        if resolved != *segment.path() {
            return Err(CatalogBuildError::PathMismatch {
                discovered: segment.path().to_path_buf(),
                resolved,
            });
        }
        cataloged.push(CatalogedSegment {
            day: day.to_owned(),
            layout: identity.layout,
            stream: identity.stream.to_owned(),
            name: identity.name.to_owned(),
            key: identity.key.to_owned(),
            path: resolved,
        });
    }
    Ok(cataloged)
}

/// Catalog every representable segment in the journal.
///
/// Missing chronicle is `Ok(vec![])`. One bad day fails the whole build.
pub fn catalog_journal(journal_root: &Path) -> Result<Vec<CatalogedSegment>, CatalogBuildError> {
    let days = catalog_days(journal_root)?;
    let mut cataloged = Vec::new();
    for day in days {
        cataloged.extend(catalog_day(journal_root, &day)?);
    }
    Ok(cataloged)
}

/// Return every valid chronicle day, including days with no segments.
///
/// Missing chronicle is `Ok(vec![])`. This stays separate from
/// [`catalog_journal`] so window/count callers do not accidentally derive the
/// day inventory from segment-bearing rows.
pub fn catalog_days(journal_root: &Path) -> Result<Vec<String>, CatalogBuildError> {
    let mut days: Vec<_> = journal_day_dirs(journal_root)
        .map_err(CatalogBuildError::Walk)?
        .into_keys()
        .collect();
    days.sort();
    Ok(days)
}

/// Decode a request or query `stream_layout` string.
///
/// `None` is Named. Only lowercase `"direct"` / `"named"` are accepted.
pub fn decode_stream_layout(raw: Option<&str>) -> Result<SegmentLayout, LayoutDecodeError> {
    match raw {
        None => Ok(SegmentLayout::Named),
        Some("direct") => Ok(SegmentLayout::Direct),
        Some("named") => Ok(SegmentLayout::Named),
        Some(_) => Err(LayoutDecodeError::Malformed),
    }
}

/// Decode a durable-row or JSON-body `stream_layout` value.
///
/// Missing and `Null` are Named. Non-strings are Malformed.
pub fn decode_stream_layout_value(raw: Option<&Value>) -> Result<SegmentLayout, LayoutDecodeError> {
    match raw {
        None | Some(Value::Null) => Ok(SegmentLayout::Named),
        Some(Value::String(value)) => decode_stream_layout(Some(value.as_str())),
        Some(_) => Err(LayoutDecodeError::Malformed),
    }
}

/// Thin adapter over [`resolve_segment_locator_exact`].
///
/// `segment_name` is the exact basename, never the parsed key.
pub fn resolve_exact(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment_name: &str,
    layout: SegmentLayout,
) -> Result<Option<PathBuf>, ExactLookupError> {
    resolve_segment_locator_exact(journal_root, day, stream, segment_name, layout)
}

/// Resolve one identity and classify the result for Shell callers.
pub fn lookup_segment(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment_name: &str,
    layout: Result<SegmentLayout, LayoutDecodeError>,
    direct_support: DirectSupport,
) -> SegmentLookup {
    let layout = match layout {
        Ok(layout) => layout,
        Err(LayoutDecodeError::Malformed) => return SegmentLookup::MalformedLayout,
    };
    match resolve_exact(journal_root, day, stream, segment_name, layout) {
        Ok(Some(path)) => {
            if layout == SegmentLayout::Direct && direct_support == DirectSupport::Refuse {
                SegmentLookup::UnsupportedLayout
            } else {
                SegmentLookup::Present(path)
            }
        }
        Ok(None) => SegmentLookup::Absent,
        Err(
            ExactLookupError::LayoutMismatch { .. } | ExactLookupError::InvalidComponent { .. },
        ) => SegmentLookup::MalformedLayout,
        Err(error) => SegmentLookup::Failed(error),
    }
}

/// Resolve every target as a Named-required lookup, fail closed.
///
/// The first non-[`SegmentLookup::Present`] outcome is returned and no paths
/// are kept, including targets that already resolved.
#[allow(dead_code)]
pub fn lookup_named_segments(
    journal_root: &Path,
    targets: &[SegmentTarget<'_>],
) -> Result<Vec<PathBuf>, SegmentLookup> {
    let mut paths = Vec::with_capacity(targets.len());
    for target in targets {
        match lookup_segment(
            journal_root,
            target.day,
            target.stream,
            target.name,
            target.layout,
            DirectSupport::Refuse,
        ) {
            SegmentLookup::Present(path) => paths.push(path),
            other => return Err(other),
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use solstone_core_journal_io::DEFAULT_STREAM;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::Path;

    const DAY: &str = "20240103";
    const KEY: &str = "080000_300";

    fn journal_root() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary journal parent");
        let journal = temporary.path().join("journal");
        fs::create_dir(&journal).expect("journal creates");
        (temporary, journal)
    }

    fn create_dir(path: &Path) {
        fs::create_dir_all(path).expect("segment directory creates");
    }

    fn twin_journal() -> (tempfile::TempDir, PathBuf) {
        let (temporary, journal) = journal_root();
        let day = journal.join("chronicle").join(DAY);
        create_dir(&day.join(KEY));
        create_dir(&day.join(DEFAULT_STREAM).join(KEY));
        create_dir(&day.join("main").join(KEY));
        create_dir(&day.join("work").join(KEY));
        (temporary, journal)
    }

    struct RestorePermissions<'a> {
        path: &'a Path,
        mode: u32,
    }

    impl Drop for RestorePermissions<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.path, fs::Permissions::from_mode(self.mode));
        }
    }

    fn deny_access(path: &Path) -> RestorePermissions<'_> {
        let mode = fs::metadata(path).expect("metadata").permissions().mode();
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("deny access");
        RestorePermissions { path, mode }
    }

    fn identity_of<'a>(
        catalog: &'a [CatalogedSegment],
        layout: SegmentLayout,
        stream: &str,
        name: &str,
    ) -> &'a CatalogedSegment {
        catalog
            .iter()
            .find(|segment| {
                segment.layout == layout && segment.stream == stream && segment.name == name
            })
            .unwrap_or_else(|| panic!("missing {layout:?} {stream}/{name}"))
    }

    #[test]
    fn unsupported_layout_copy_is_the_voice_approved_text() {
        assert_eq!(
            UNSUPPORTED_LAYOUT_REASON,
            "speaker_segment_layout_unsupported"
        );
        assert_eq!(
            UNSUPPORTED_LAYOUT_MESSAGE,
            "This command can't change that speaker review."
        );
        assert_eq!(
            UNSUPPORTED_LAYOUT_DETAIL,
            "This segment uses the direct journal layout, which this command doesn't support."
        );
    }

    #[test]
    fn catalog_day_keeps_direct_and_named_twins_distinct() {
        let (_temporary, journal) = twin_journal();
        let catalog = catalog_day(&journal, DAY).expect("catalog builds");
        assert_eq!(catalog.len(), 4);

        let direct = identity_of(&catalog, SegmentLayout::Direct, DEFAULT_STREAM, KEY);
        assert_eq!(direct.day, DAY);
        assert_eq!(direct.key, KEY);
        assert_eq!(direct.path, journal.join("chronicle").join(DAY).join(KEY));

        let named_default = identity_of(&catalog, SegmentLayout::Named, DEFAULT_STREAM, KEY);
        assert_eq!(named_default.key, KEY);
        assert_eq!(
            named_default.path,
            journal
                .join("chronicle")
                .join(DAY)
                .join(DEFAULT_STREAM)
                .join(KEY)
        );

        let main = identity_of(&catalog, SegmentLayout::Named, "main", KEY);
        assert_eq!(
            main.path,
            journal.join("chronicle").join(DAY).join("main").join(KEY)
        );

        let work = identity_of(&catalog, SegmentLayout::Named, "work", KEY);
        assert_eq!(
            work.path,
            journal.join("chronicle").join(DAY).join("work").join(KEY)
        );

        let journal_catalog = catalog_journal(&journal).expect("journal catalog builds");
        assert_eq!(journal_catalog.len(), 4);
    }

    #[test]
    fn catalog_day_splits_suffixed_basename_into_name_and_key() {
        let (_temporary, journal) = journal_root();
        create_dir(
            &journal
                .join("chronicle")
                .join(DAY)
                .join("main")
                .join("093000_300_summary"),
        );
        let catalog = catalog_day(&journal, DAY).expect("catalog builds");
        assert_eq!(catalog.len(), 1);
        let segment = &catalog[0];
        assert_eq!(segment.layout, SegmentLayout::Named);
        assert_eq!(segment.stream, "main");
        assert_eq!(segment.name, "093000_300_summary");
        assert_eq!(segment.key, "093000_300");
        assert_eq!(
            segment.path,
            journal
                .join("chronicle")
                .join(DAY)
                .join("main")
                .join("093000_300_summary")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn catalog_day_refuses_non_utf8_identity() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let (_temporary, journal) = journal_root();
        let day = journal.join("chronicle").join(DAY);
        create_dir(&day.join(OsStr::from_bytes(b"s\xff")).join(KEY));
        match catalog_day(&journal, DAY) {
            Err(CatalogBuildError::NotUtf8 { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn catalog_day_errors_when_day_directory_is_unreadable() {
        let (_temporary, journal) = journal_root();
        let day = journal.join("chronicle").join(DAY);
        create_dir(&day.join("main").join(KEY));
        let _restore = deny_access(&day);
        match catalog_day(&journal, DAY) {
            Err(CatalogBuildError::Walk(PathError::Io { .. })) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn catalog_day_errors_when_named_stream_is_unreadable() {
        let (_temporary, journal) = journal_root();
        let stream = journal.join("chronicle").join(DAY).join("main");
        create_dir(&stream.join(KEY));
        let _restore = deny_access(&stream);
        match catalog_day(&journal, DAY) {
            Err(CatalogBuildError::Walk(PathError::Io { .. })) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn catalog_day_errors_when_exact_resolver_rejects_discovered_symlink() {
        let (_temporary, journal) = journal_root();
        let day = journal.join("chronicle").join(DAY);
        let real = day.join("main").join(KEY);
        create_dir(&real);
        create_dir(&day.join("work"));
        symlink(&real, day.join("work").join(KEY)).expect("symlink creates");
        match catalog_day(&journal, DAY) {
            Err(CatalogBuildError::Lookup(ExactLookupError::WrongKind { .. })) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn catalog_journal_errors_when_chronicle_is_unreadable() {
        let (_temporary, journal) = journal_root();
        let chronicle = journal.join("chronicle");
        create_dir(&chronicle.join(DAY).join("main").join(KEY));
        let _restore = deny_access(&chronicle);
        match catalog_journal(&journal) {
            Err(CatalogBuildError::Walk(PathError::Io { .. })) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn catalog_journal_returns_empty_when_chronicle_is_missing() {
        let (_temporary, journal) = journal_root();
        let catalog = catalog_journal(&journal).expect("missing chronicle is empty");
        assert!(catalog.is_empty());
    }

    #[test]
    fn catalog_day_returns_empty_when_day_is_empty() {
        let (_temporary, journal) = journal_root();
        create_dir(&journal.join("chronicle").join(DAY));
        let catalog = catalog_day(&journal, DAY).expect("empty day is empty");
        assert!(catalog.is_empty());
    }

    #[test]
    fn catalog_day_errors_on_invalid_day_key() {
        let (_temporary, journal) = journal_root();
        match catalog_day(&journal, "not-a-day") {
            Err(CatalogBuildError::Walk(PathError::InvalidRelativePath { .. })) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decode_stream_layout_accepts_absent_direct_and_named() {
        assert_eq!(decode_stream_layout(None), Ok(SegmentLayout::Named));
        assert_eq!(
            decode_stream_layout(Some("direct")),
            Ok(SegmentLayout::Direct)
        );
        assert_eq!(
            decode_stream_layout(Some("named")),
            Ok(SegmentLayout::Named)
        );
    }

    #[test]
    fn decode_stream_layout_rejects_wrong_case_and_empty() {
        assert_eq!(
            decode_stream_layout(Some("Direct")),
            Err(LayoutDecodeError::Malformed)
        );
        assert_eq!(
            decode_stream_layout(Some("")),
            Err(LayoutDecodeError::Malformed)
        );
        assert_eq!(
            decode_stream_layout(Some("DIRECT")),
            Err(LayoutDecodeError::Malformed)
        );
    }

    #[test]
    fn decode_stream_layout_value_rejects_non_string_json() {
        assert_eq!(decode_stream_layout_value(None), Ok(SegmentLayout::Named));
        assert_eq!(
            decode_stream_layout_value(Some(&Value::Null)),
            Ok(SegmentLayout::Named)
        );
        assert_eq!(
            decode_stream_layout_value(Some(&json!("named"))),
            Ok(SegmentLayout::Named)
        );
        assert_eq!(
            decode_stream_layout_value(Some(&json!("direct"))),
            Ok(SegmentLayout::Direct)
        );
        assert_eq!(
            decode_stream_layout_value(Some(&json!("Direct"))),
            Err(LayoutDecodeError::Malformed)
        );
        assert_eq!(
            decode_stream_layout_value(Some(&json!(true))),
            Err(LayoutDecodeError::Malformed)
        );
        assert_eq!(
            decode_stream_layout_value(Some(&json!(1))),
            Err(LayoutDecodeError::Malformed)
        );
        assert_eq!(
            decode_stream_layout_value(Some(&json!([]))),
            Err(LayoutDecodeError::Malformed)
        );
        assert_eq!(
            decode_stream_layout_value(Some(&json!({}))),
            Err(LayoutDecodeError::Malformed)
        );
    }

    #[test]
    fn lookup_segment_covers_all_five_outcomes() {
        let (_temporary, journal) = twin_journal();

        match lookup_segment(
            &journal,
            DAY,
            "main",
            KEY,
            Ok(SegmentLayout::Named),
            DirectSupport::Refuse,
        ) {
            SegmentLookup::Present(path) => {
                assert_eq!(
                    path,
                    journal.join("chronicle").join(DAY).join("main").join(KEY)
                );
            }
            other => panic!("{other:?}"),
        }

        match lookup_segment(
            &journal,
            DAY,
            "missing",
            KEY,
            Ok(SegmentLayout::Named),
            DirectSupport::Allow,
        ) {
            SegmentLookup::Absent => {}
            other => panic!("{other:?}"),
        }

        match lookup_segment(
            &journal,
            DAY,
            DEFAULT_STREAM,
            KEY,
            Ok(SegmentLayout::Direct),
            DirectSupport::Refuse,
        ) {
            SegmentLookup::UnsupportedLayout => {}
            other => panic!("{other:?}"),
        }

        match lookup_segment(
            &journal,
            DAY,
            DEFAULT_STREAM,
            KEY,
            Ok(SegmentLayout::Direct),
            DirectSupport::Allow,
        ) {
            SegmentLookup::Present(path) => {
                assert_eq!(path, journal.join("chronicle").join(DAY).join(KEY));
            }
            other => panic!("{other:?}"),
        }

        match lookup_segment(
            &journal,
            DAY,
            "main",
            KEY,
            Err(LayoutDecodeError::Malformed),
            DirectSupport::Allow,
        ) {
            SegmentLookup::MalformedLayout => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lookup_segment_maps_layout_mismatch_and_invalid_component_to_malformed() {
        let (_temporary, journal) = twin_journal();
        match lookup_segment(
            &journal,
            DAY,
            "main",
            KEY,
            Ok(SegmentLayout::Direct),
            DirectSupport::Allow,
        ) {
            SegmentLookup::MalformedLayout => {}
            other => panic!("{other:?}"),
        }
        match lookup_segment(
            &journal,
            DAY,
            "a/b",
            KEY,
            Ok(SegmentLayout::Named),
            DirectSupport::Allow,
        ) {
            SegmentLookup::MalformedLayout => {}
            other => panic!("{other:?}"),
        }
        match lookup_segment(
            &journal,
            DAY,
            "",
            KEY,
            Ok(SegmentLayout::Named),
            DirectSupport::Allow,
        ) {
            SegmentLookup::MalformedLayout => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lookup_segment_maps_io_to_failed() {
        let (_temporary, journal) = journal_root();
        let day = journal.join("chronicle").join(DAY);
        create_dir(&day);
        let looped = day.join("looped");
        symlink(&looped, &looped).expect("looped symlink");
        match lookup_segment(
            &journal,
            DAY,
            "looped",
            KEY,
            Ok(SegmentLayout::Named),
            DirectSupport::Allow,
        ) {
            SegmentLookup::Failed(ExactLookupError::Io { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    fn named<'a>(day: &'a str, stream: &'a str, name: &'a str) -> SegmentTarget<'a> {
        SegmentTarget {
            day,
            stream,
            name,
            layout: Ok(SegmentLayout::Named),
        }
    }

    #[test]
    fn lookup_named_segments_returns_all_named_paths_in_order() {
        let (_temporary, journal) = twin_journal();
        let paths = lookup_named_segments(
            &journal,
            &[named(DAY, "main", KEY), named(DAY, "work", KEY)],
        )
        .expect("named targets resolve");
        assert_eq!(
            paths,
            vec![
                journal.join("chronicle").join(DAY).join("main").join(KEY),
                journal.join("chronicle").join(DAY).join("work").join(KEY),
            ]
        );
    }

    #[test]
    fn lookup_named_segments_fails_closed_on_direct_absent_or_error() {
        let (_temporary, journal) = twin_journal();

        match lookup_named_segments(
            &journal,
            &[
                named(DAY, "main", KEY),
                SegmentTarget {
                    day: DAY,
                    stream: DEFAULT_STREAM,
                    name: KEY,
                    layout: Ok(SegmentLayout::Direct),
                },
                named(DAY, "work", KEY),
            ],
        ) {
            Err(SegmentLookup::UnsupportedLayout) => {}
            other => panic!("{other:?}"),
        }

        match lookup_named_segments(
            &journal,
            &[named(DAY, "main", KEY), named(DAY, "missing", KEY)],
        ) {
            Err(SegmentLookup::Absent) => {}
            other => panic!("{other:?}"),
        }

        let looped = journal.join("chronicle").join(DAY).join("looped");
        symlink(&looped, &looped).expect("looped symlink");
        match lookup_named_segments(
            &journal,
            &[named(DAY, "main", KEY), named(DAY, "looped", KEY)],
        ) {
            Err(SegmentLookup::Failed(ExactLookupError::Io { .. })) => {}
            other => panic!("{other:?}"),
        }
    }
}
