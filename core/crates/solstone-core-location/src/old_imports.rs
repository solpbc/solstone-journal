// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Old body bundles under `imports/`, and whether the new path has reached their days.
//!
//! An old bundle stays on disk and is never read as data. A reader lists what it
//! holds from its manifest and says when one covers a day the new path hasn't
//! touched. Nothing here opens a window: a day's windows are read as a directory
//! listing, and an import's own files as far as they say what it covered.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chrono::NaiveDate;
use serde_json::Value;
use solstone_core_format::content::body_records::BODY_RECORD_STREAMS;

const NORMALIZED_DIR: &str = "normalized";
const MANIFEST_FILE: &str = "manifest.json";
const IMPORT_RECORD: &str = "import.json";

/// What an import of the new path lists of what it touched.
pub const TOUCHED_FILE: &str = "body-touched.jsonl";
/// How far an import of the new path has got.
pub const PROGRESS_FILE: &str = "body-progress.json";

/// Whether a directory under `imports/` is an old body bundle: it holds
/// `normalized/` and neither of the files the new path writes. This is the one
/// definition; a new-path import names itself with those files, so it is never
/// taken for one.
#[must_use]
pub fn is_old_bundle(import_dir: &Path) -> bool {
    import_dir.join(NORMALIZED_DIR).is_dir()
        && !import_dir.join(TOUCHED_FILE).exists()
        && !import_dir.join(PROGRESS_FILE).exists()
}

/// A body source an old bundle can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OldSource {
    AppleHealth,
    Oura,
    DexcomClarity,
}

impl OldSource {
    /// A manifest's source family. `None` for a family that isn't a body import.
    #[must_use]
    pub fn from_family(family: &str) -> Option<Self> {
        match family {
            "apple_health" => Some(Self::AppleHealth),
            "oura" | "oura_api" => Some(Self::Oura),
            "dexcom_clarity" => Some(Self::DexcomClarity),
            _ => None,
        }
    }

    /// The stream the new path writes this source to. `None` when it has no
    /// importer now.
    #[must_use]
    pub fn stream(self) -> Option<&'static str> {
        match self {
            Self::AppleHealth => Some(BODY_RECORD_STREAMS[0]),
            Self::Oura => Some(BODY_RECORD_STREAMS[1]),
            Self::DexcomClarity => None,
        }
    }

    /// The source in the owner's words.
    #[must_use]
    pub fn words(self) -> &'static str {
        match self {
            Self::AppleHealth => "Apple Health",
            Self::Oura => "Oura",
            Self::DexcomClarity => "Dexcom Clarity",
        }
    }
}

/// How much of the calendar an old bundle says it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OldSpan {
    /// A native bundle: `YYYYMMDD` days from its manifest.
    Days(BTreeSet<String>),
    /// A Python-era bundle: `YYYY-MM` months from its `normalized/` file names.
    Months(BTreeSet<String>),
}

/// One old bundle, as far as a reader says of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OldBundle {
    /// Its directory name.
    pub id: String,
    pub source: OldSource,
    pub span: OldSpan,
}

fn valid_day(text: &str) -> bool {
    text.len() == 8
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && NaiveDate::parse_from_str(text, "%Y%m%d").is_ok()
}

fn read_manifest(path: &Path) -> Option<serde_json::Map<String, Value>> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text).ok()? {
        Value::Object(manifest) => Some(manifest),
        _ => None,
    }
}

fn read_old_span(bundle: &Path, manifest: &serde_json::Map<String, Value>) -> OldSpan {
    let days = manifest
        .get("days_affected")
        .and_then(Value::as_array)
        .map(|days| {
            days.iter()
                .filter_map(Value::as_str)
                .filter(|day| valid_day(day))
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if !days.is_empty() {
        return OldSpan::Days(days);
    }
    // Month file names only: a bundle's rows are never opened here.
    let months = fs::read_dir(bundle.join(NORMALIZED_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let month = name.strip_suffix(".jsonl")?;
            is_month_name(month).then(|| month.to_owned())
        })
        .collect();
    OldSpan::Months(months)
}

fn is_month_name(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

/// Every old bundle under `imports/`, sorted by directory name. Reads each
/// bundle's manifest and the names of its month files, and nothing else. A
/// bundle whose manifest can't be read or names a family that isn't a body
/// import isn't listed.
#[must_use]
pub fn read_old_bundles(journal: &Path) -> Vec<OldBundle> {
    let imports = journal.join("imports");
    let Ok(entries) = fs::read_dir(&imports) else {
        return Vec::new();
    };
    let mut bundles = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        if !is_old_bundle(&path) {
            continue;
        }
        let Some(manifest) = read_manifest(&path.join(MANIFEST_FILE)) else {
            continue;
        };
        let Some(source) = manifest
            .get("source_type")
            .and_then(Value::as_str)
            .and_then(OldSource::from_family)
        else {
            continue;
        };
        bundles.push(OldBundle {
            id: name,
            source,
            span: read_old_span(&path, &manifest),
        });
    }
    bundles.sort_by(|left, right| left.id.cmp(&right.id));
    bundles
}

/// One import of the new path: what it names itself as and the calendar it says
/// it covered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewImport {
    pub id: String,
    pub source: OldSource,
    /// Inclusive `YYYYMMDD` ranges from its manifest's `span`, when written.
    pub span: Vec<(String, String)>,
}

/// Every import of the new path under `imports/`, sorted by directory name: a
/// directory holding [`TOUCHED_FILE`] or [`PROGRESS_FILE`], with a source of
/// Apple Health or Oura.
#[must_use]
pub fn read_new_imports(journal: &Path) -> Vec<NewImport> {
    let imports = journal.join("imports");
    let Ok(entries) = fs::read_dir(&imports) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        if !path.join(TOUCHED_FILE).exists() && !path.join(PROGRESS_FILE).exists() {
            continue;
        }
        let manifest = read_manifest(&path.join(MANIFEST_FILE));
        // The manifest is written at a run's end; before that its source is on
        // the import's own record.
        let family = manifest
            .as_ref()
            .and_then(|manifest| manifest.get("source_type"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                read_manifest(&path.join(IMPORT_RECORD))
                    .and_then(|record| record.get("source_hint").cloned())
                    .and_then(|hint| hint.as_str().map(str::to_owned))
            });
        let Some(source) = family.as_deref().and_then(OldSource::from_family) else {
            continue;
        };
        let span = manifest.as_ref().map_or_else(Vec::new, read_new_span);
        found.push(NewImport {
            id: name,
            source,
            span,
        });
    }
    found.sort_by(|left, right| left.id.cmp(&right.id));
    found
}

fn read_new_span(manifest: &serde_json::Map<String, Value>) -> Vec<(String, String)> {
    manifest
        .get("span")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|range| {
            let [from, to] = range.as_array()?.as_slice() else {
                return None;
            };
            let (from, to) = (from.as_str()?, to.as_str()?);
            (valid_day(from) && valid_day(to) && from <= to)
                .then(|| (from.to_owned(), to.to_owned()))
        })
        .collect()
}

/// Whether a day's folder for this stream holds a window or a tombstone: a
/// directory entry only, no window opened.
fn has_window_or_tombstone(journal: &Path, day: &str, stream: &str) -> bool {
    let dir = journal.join("chronicle").join(day).join(stream);
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            !entry.file_name().to_string_lossy().starts_with('.')
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
        })
}

/// Whether an import of the new path's listing names `day`.
fn listing_names_day(journal: &Path, id: &str, day: &str) -> bool {
    let path = journal.join("imports").join(id).join(TOUCHED_FILE);
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    text.lines().any(|line| {
        serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|line| line.get("day").and_then(Value::as_str).map(str::to_owned))
            .as_deref()
            == Some(day)
    })
}

/// What's needed to say whether the new path has touched a day, for one
/// reader's few days.
struct Touched<'a> {
    journal: &'a Path,
    imports: std::cell::OnceCell<Vec<NewImport>>,
}

impl<'a> Touched<'a> {
    fn new(journal: &'a Path) -> Self {
        Self {
            journal,
            imports: std::cell::OnceCell::new(),
        }
    }

    fn imports(&self) -> &[NewImport] {
        self.imports.get_or_init(|| read_new_imports(self.journal))
    }

    /// Whether the new path has reached this source's day: a window or
    /// tombstone of that source on the day, the day in an import of that
    /// source's listing, or the day inside what an import of that source
    /// covered, which counts even when the day held nothing.
    fn source_touched(&self, source: OldSource, day: &str) -> bool {
        let Some(stream) = source.stream() else {
            // No importer, so nothing can have reached it.
            return false;
        };
        if has_window_or_tombstone(self.journal, day, stream) {
            return true;
        }
        self.imports()
            .iter()
            .filter(|import| import.source == source)
            .any(|import| {
                import
                    .span
                    .iter()
                    .any(|(from, to)| from.as_str() <= day && day <= to.as_str())
                    || listing_names_day(self.journal, &import.id, day)
            })
    }
}

/// One old bundle's say on one day: which source, and whether it names the day
/// or only its month.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OldCover {
    pub source: OldSource,
    pub scope: CoverScope,
}

/// What a line about an old bundle may say it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverScope {
    /// Its manifest names this day.
    Day,
    /// Its month files include this `YYYY-MM`; a line never calls the day itself covered.
    Month(String),
}

fn covers(bundle: &OldBundle, day: &str) -> Option<CoverScope> {
    match &bundle.span {
        OldSpan::Days(days) if days.contains(day) => Some(CoverScope::Day),
        OldSpan::Days(_) => None,
        OldSpan::Months(months) => {
            let month = format!("{}-{}", day.get(..4)?, day.get(4..6)?);
            months.contains(&month).then_some(CoverScope::Month(month))
        }
    }
}

/// One line per source for what old bundles say of `day`: a source whose bundle
/// covers the day (or its month) and which the new path hasn't touched that
/// day. A day-level bundle wins over a month-level one for its source.
#[must_use]
pub fn covers_on_day(journal: &Path, day: &str, only: Option<OldSource>) -> Vec<OldCover> {
    let mut by_source = BTreeMap::<OldSource, CoverScope>::new();
    for bundle in read_old_bundles(journal) {
        if only.is_some_and(|only| only != bundle.source) {
            continue;
        }
        let Some(scope) = covers(&bundle, day) else {
            continue;
        };
        let slot = by_source
            .entry(bundle.source)
            .or_insert_with(|| scope.clone());
        if scope == CoverScope::Day {
            *slot = CoverScope::Day;
        }
    }
    if by_source.is_empty() {
        return Vec::new();
    }
    let touched = Touched::new(journal);
    by_source
        .into_iter()
        .filter(|(source, _)| !touched.source_touched(*source, day))
        .map(|(source, scope)| OldCover { source, scope })
        .collect()
}

/// A `YYYY-MM` month as a person says it: "March 2026".
#[must_use]
pub fn month_words(month: &str) -> Option<String> {
    let date = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").ok()?;
    Some(date.format("%B %Y").to_string())
}

#[cfg(test)]
#[path = "old_imports_tests.rs"]
mod tests;
