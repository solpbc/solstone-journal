// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Device stream continuity and the source/registry boundary for takeover.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use solstone_core_journal_io::lock_is_held;
use solstone_core_journal_io::{FileLock, JsonWriteOptions, LockOptions, hold_lock, write_json};

use crate::stream_record::{StreamBindingRecord, list_stream_bindings};
use crate::{SegmentError, hold_source_mutation, is_safe_stream_component, is_valid_device_cid};

const CONTINUITY_FILE: &str = "continuity.json";
const REGISTRY_FILE: &str = ".registry";
const MAX_SOURCE_SNAPSHOT_RETRIES: usize = 4;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuityDocument {
    pub version: u32,
    pub streams: BTreeMap<String, ContinuityRecord>,
}

impl Default for ContinuityDocument {
    fn default() -> Self {
        Self {
            version: 1,
            streams: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuityRecord {
    pub source: String,
    pub writers: Vec<String>,
}

/// Stored decision plan. Sources without a selected stream are retained in the
/// operation record so recovery does not recalculate the continuity choice.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakeoverPlan {
    pub adopted_cid: String,
    pub retired_cid: String,
    pub sources: Vec<TakeoverSourcePlan>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TakeoverSourcePlan {
    pub source: String,
    pub stream: Option<String>,
    pub writers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContinuationBinding {
    Tail(String),
    Ancestor(String),
    None,
}

pub struct TakeoverGuard {
    journal: PathBuf,
    bindings: Vec<StreamBindingRecord>,
    continuity: ContinuityDocument,
    _source_locks: Vec<FileLock>,
    _registry_lock: FileLock,
}

impl TakeoverGuard {
    pub fn plan(&self, adopted_cid: &str, retired_cid: &str) -> TakeoverPlan {
        let mut sources = BTreeSet::new();
        for binding in &self.bindings {
            if binding.cid == adopted_cid || binding.cid == retired_cid {
                sources.insert(binding.source.clone());
            }
        }
        for record in self.continuity.streams.values() {
            if record
                .writers
                .iter()
                .any(|writer| writer == adopted_cid || writer == retired_cid)
            {
                sources.insert(record.source.clone());
            }
        }

        let planned_sources = sources
            .into_iter()
            .map(|source| {
                let prior = self.continuity.streams.iter().find(|(_, record)| {
                    record.source == source
                        && record
                            .writers
                            .last()
                            .is_some_and(|tail| tail == retired_cid)
                });
                let selected =
                    prior.map(|(stream, record)| (stream.clone(), record.writers.clone()));
                let selected = selected.or_else(|| {
                    self.bindings
                        .iter()
                        .filter(|binding| binding.cid == retired_cid && binding.source == source)
                        .max_by_key(|binding| binding.seq)
                        .map(|binding| (binding.name.clone(), vec![retired_cid.to_owned()]))
                });
                let (stream, writers) = match selected {
                    Some((stream, mut writers)) => {
                        if writers.last().map(String::as_str) != Some(adopted_cid) {
                            writers.push(adopted_cid.to_owned());
                        }
                        (Some(stream), writers)
                    }
                    None => (None, Vec::new()),
                };
                TakeoverSourcePlan {
                    source,
                    stream,
                    writers,
                }
            })
            .collect();
        TakeoverPlan {
            adopted_cid: adopted_cid.to_owned(),
            retired_cid: retired_cid.to_owned(),
            sources: planned_sources,
        }
    }

    /// Publish only stream continuations selected by the stored plan.
    pub fn publish(&self, plan: &TakeoverPlan) -> Result<(), SegmentError> {
        if !is_valid_device_cid(&plan.adopted_cid)
            || !is_valid_device_cid(&plan.retired_cid)
            || plan.adopted_cid == plan.retired_cid
            || plan.sources.iter().any(|source| {
                !valid_source(&source.source)
                    || source.stream.as_deref().is_some_and(|stream| {
                        !is_safe_stream_component(stream)
                            || source.writers.len() < 2
                            || source.writers.last() != Some(&plan.adopted_cid)
                            || !source.writers.contains(&plan.retired_cid)
                            || source
                                .writers
                                .iter()
                                .any(|writer| !is_valid_device_cid(writer))
                            || source.writers.iter().collect::<BTreeSet<_>>().len()
                                != source.writers.len()
                    })
                    || (source.stream.is_none() && !source.writers.is_empty())
            })
        {
            return Err(SegmentError::StreamInput(
                "invalid takeover continuity plan",
            ));
        }
        let path = continuity_path(&self.journal);
        let mut document = read_continuity(&self.journal)?;
        let before = document.clone();
        for source in &plan.sources {
            let (Some(stream), true) = (&source.stream, !source.writers.is_empty()) else {
                continue;
            };
            document.streams.insert(
                stream.clone(),
                ContinuityRecord {
                    source: source.source.clone(),
                    writers: source.writers.clone(),
                },
            );
        }
        if document != before {
            write_json(&path, &document, JsonWriteOptions::default())?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn source_locks_are_held(&self) -> bool {
        self._source_locks
            .iter()
            .all(|lock| lock_is_held(lock.path()).unwrap_or(false))
    }
}

pub fn read_continuity(journal: &Path) -> Result<ContinuityDocument, SegmentError> {
    let path = continuity_path(journal);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ContinuityDocument::default());
        }
        Err(source) => return Err(SegmentError::Io { path, source }),
    };
    let document: ContinuityDocument = serde_json::from_slice(&bytes)
        .map_err(|_| SegmentError::StreamInput("continuity record is malformed or unsupported"))?;
    if document.version != 1 {
        return Err(SegmentError::StreamInput(
            "continuity record is malformed or unsupported",
        ));
    }
    let mut source_writers = BTreeSet::new();
    for (stream, record) in &document.streams {
        if !is_safe_stream_component(stream)
            || !valid_source(&record.source)
            || record.writers.len() < 2
            || record
                .writers
                .iter()
                .any(|writer| !is_valid_device_cid(writer))
            || record.writers.iter().collect::<BTreeSet<_>>().len() != record.writers.len()
            || record
                .writers
                .iter()
                .any(|writer| !source_writers.insert((record.source.as_str(), writer.as_str())))
        {
            return Err(SegmentError::StreamInput(
                "continuity record is malformed or unsupported",
            ));
        }
    }
    Ok(document)
}

pub fn continuation_binding(
    journal: &Path,
    cid: &str,
    source: &str,
) -> Result<ContinuationBinding, SegmentError> {
    let document = read_continuity(journal)?;
    let mut found = None;
    for (stream, record) in document.streams {
        if record.source != source || !record.writers.iter().any(|writer| writer == cid) {
            continue;
        }
        let binding = if record.writers.last().is_some_and(|tail| tail == cid) {
            ContinuationBinding::Tail(stream)
        } else {
            ContinuationBinding::Ancestor(stream)
        };
        if found.is_some() {
            return Err(SegmentError::StreamInput(
                "CID appears in multiple continuity chains for one source",
            ));
        }
        found = Some(binding);
    }
    Ok(found.unwrap_or(ContinuationBinding::None))
}

pub fn visible_stream_names(
    journal: &Path,
    cid: &str,
    source: &str,
) -> Result<Vec<String>, SegmentError> {
    let mut names = list_stream_bindings(journal)?
        .into_iter()
        .filter(|binding| binding.cid == cid && binding.source == source)
        .map(|binding| binding.name)
        .collect::<BTreeSet<_>>();
    if let ContinuationBinding::Tail(stream) = continuation_binding(journal, cid, source)? {
        names.insert(stream);
    }
    Ok(names.into_iter().collect())
}

/// Receipt CIDs allowed for one stream/source as viewed by an authenticated
/// reader. Continuity ancestors are read evidence only; upload binding remains
/// restricted to the tail.
pub fn receipt_cids_for_stream(
    journal: &Path,
    stream: &str,
    cid: &str,
    source: &str,
) -> Result<BTreeSet<String>, SegmentError> {
    let document = read_continuity(journal)?;
    if let Some(record) = document.streams.get(stream) {
        if record.source != source {
            return Ok(BTreeSet::new());
        }
        let Some(position) = record.writers.iter().position(|writer| writer == cid) else {
            return Ok(BTreeSet::new());
        };
        return Ok(record.writers[..=position].iter().cloned().collect());
    }
    let direct = list_stream_bindings(journal)?
        .into_iter()
        .any(|binding| binding.name == stream && binding.cid == cid && binding.source == source);
    Ok(if direct {
        BTreeSet::from([cid.to_owned()])
    } else {
        BTreeSet::new()
    })
}

/// Acquire the source locks first, then the registry lock, and invoke `f` only
/// after a stable source snapshot is held.
pub fn with_takeover_stream_boundary<T>(
    journal: &Path,
    adopted_cid: &str,
    retired_cid: &str,
    f: impl FnOnce(&TakeoverGuard) -> Result<T, SegmentError>,
) -> Result<T, SegmentError> {
    let mut f = Some(f);
    for _ in 0..MAX_SOURCE_SNAPSHOT_RETRIES {
        let before_bindings = list_stream_bindings(journal)?;
        let before_continuity = read_continuity(journal)?;
        let before_sources = takeover_sources(
            &before_bindings,
            &before_continuity,
            adopted_cid,
            retired_cid,
        );
        let mut source_locks = Vec::with_capacity(before_sources.len());
        for source in &before_sources {
            source_locks.push(hold_source_mutation(journal, source)?);
        }
        let registry_target = journal.join("streams").join(REGISTRY_FILE);
        let registry_lock = hold_lock(registry_target, LockOptions::default())?;
        let bindings = list_stream_bindings(journal)?;
        let continuity = read_continuity(journal)?;
        let after_sources = takeover_sources(&bindings, &continuity, adopted_cid, retired_cid);
        if after_sources
            .iter()
            .any(|source| !before_sources.contains(source))
        {
            drop(registry_lock);
            drop(source_locks);
            continue;
        }
        let guard = TakeoverGuard {
            journal: journal.to_path_buf(),
            bindings,
            continuity,
            _source_locks: source_locks,
            _registry_lock: registry_lock,
        };
        return f.take().expect("takeover callback is invoked once")(&guard);
    }
    Err(SegmentError::StreamInput(
        "takeover source snapshot did not stabilize",
    ))
}

fn takeover_sources(
    bindings: &[StreamBindingRecord],
    continuity: &ContinuityDocument,
    adopted_cid: &str,
    retired_cid: &str,
) -> BTreeSet<String> {
    let mut sources = bindings
        .iter()
        .filter(|binding| binding.cid == adopted_cid || binding.cid == retired_cid)
        .map(|binding| binding.source.clone())
        .collect::<BTreeSet<_>>();
    for record in continuity.streams.values() {
        if record
            .writers
            .iter()
            .any(|writer| writer == adopted_cid || writer == retired_cid)
        {
            sources.insert(record.source.clone());
        }
    }
    sources
}

fn continuity_path(journal: &Path) -> PathBuf {
    journal.join("streams").join(CONTINUITY_FILE)
}

fn valid_source(source: &str) -> bool {
    if source.len() > 64 || source.as_bytes().contains(&0) || source.contains(['/', '\\', '.']) {
        return false;
    }
    let mut bytes = source.bytes();
    match bytes.next() {
        None => true,
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        }),
        Some(_) => false,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::Value;

    use super::*;
    use crate::{
        Kind, PairedStreamBase, StreamHints, advance_bound_stream, bind_named_stream,
        bind_paired_stream, list_stream_bindings,
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const DAY: &str = "20261004";

    struct Journal(PathBuf);

    impl Journal {
        fn new() -> Self {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-stream-continuity-{}-{nanos}-{id}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("journal root creates");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn hints() -> StreamHints {
        StreamHints {
            kind: Some(Kind::Observed),
            host: None,
            platform: None,
        }
    }

    fn bind(root: &Path, cid: &str, source: &str) -> String {
        bind_paired_stream(
            root,
            DAY,
            "120000_1",
            &PairedStreamBase {
                origin: crate::StreamAllocationBase::Device,
                input: "phone",
            },
            cid,
            source,
            &hints(),
        )
        .unwrap()
        .stream
    }

    fn publish_takeover(root: &Path, adopted: &str, retired: &str) -> TakeoverPlan {
        with_takeover_stream_boundary(root, adopted, retired, |guard| {
            let plan = guard.plan(adopted, retired);
            guard.publish(&plan).unwrap();
            Ok(plan)
        })
        .unwrap()
    }

    #[test]
    fn takeover_boundary_holds_source_lock_and_keeps_both_prechoice_streams() {
        let journal = Journal::new();
        let retired_stream = bind(journal.path(), A, "audio");
        let adopted_stream = bind(journal.path(), B, "audio");
        let mut observed_locked = false;
        let plan = with_takeover_stream_boundary(journal.path(), B, A, |guard| {
            observed_locked = guard.source_locks_are_held();
            let plan = guard.plan(B, A);
            guard.publish(&plan).unwrap();
            Ok(plan)
        })
        .unwrap();

        assert!(observed_locked);
        assert_eq!(
            plan.sources[0].stream.as_deref(),
            Some(retired_stream.as_str())
        );
        assert_eq!(plan.sources[0].writers, vec![A.to_owned(), B.to_owned()]);
        assert_ne!(retired_stream, adopted_stream);
        let visible = visible_stream_names(journal.path(), B, "audio").unwrap();
        assert!(visible.contains(&retired_stream));
        assert!(visible.contains(&adopted_stream));
        assert_eq!(
            continuation_binding(journal.path(), B, "audio").unwrap(),
            ContinuationBinding::Tail(retired_stream.clone())
        );
        assert_eq!(list_stream_bindings(journal.path()).unwrap().len(), 2);
        assert!(!registry_contains_continuity(journal.path()));

        let bound = bind_named_stream(
            journal.path(),
            DAY,
            "120000_2",
            &adopted_stream,
            B,
            "audio",
            &hints(),
        )
        .unwrap();
        assert_eq!(bound.stream, retired_stream);
        advance_bound_stream(
            &bound.stream,
            DAY,
            "120000_2",
            &bound.segment,
            hints(),
            B,
            "audio",
        )
        .unwrap();
        let tail_write = advance_bound_stream(
            &bound.stream,
            DAY,
            "120000_3",
            &crate::SegmentDir::resolve(journal.path(), DAY, "120000_3", &bound.stream).unwrap(),
            hints(),
            A,
            "audio",
        );
        assert!(matches!(
            tail_write,
            Err(crate::SegmentError::StreamBindingConflict { .. })
        ));
    }

    #[test]
    fn continuity_a_to_b_to_c_appends_tail_without_flattening_stream_records() {
        let journal = Journal::new();
        let origin_stream = bind(journal.path(), A, "audio");
        publish_takeover(journal.path(), B, A);
        let plan = publish_takeover(journal.path(), C, B);

        assert_eq!(
            plan.sources[0].stream.as_deref(),
            Some(origin_stream.as_str())
        );
        assert_eq!(
            plan.sources[0].writers,
            vec![A.to_owned(), B.to_owned(), C.to_owned()]
        );
        let document = read_continuity(journal.path()).unwrap();
        assert_eq!(document.streams[&origin_stream].writers, vec![A, B, C]);
        let records = list_stream_bindings(journal.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].cid, A);
        assert_eq!(
            continuation_binding(journal.path(), C, "audio").unwrap(),
            ContinuationBinding::Tail(origin_stream)
        );
    }

    #[test]
    fn continuity_file_is_not_a_stream_registry_record() {
        let journal = Journal::new();
        bind(journal.path(), A, "audio");
        publish_takeover(journal.path(), B, A);
        let paths = crate::stream_record::registry_json_paths(journal.path()).unwrap();
        assert!(
            paths
                .iter()
                .all(|path| path.file_name().and_then(|name| name.to_str())
                    != Some("continuity.json"))
        );
        let raw: Value = serde_json::from_slice(
            &fs::read(journal.path().join("streams/continuity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(raw["version"], 1);
    }

    fn registry_contains_continuity(journal: &Path) -> bool {
        crate::stream_record::registry_json_paths(journal)
            .unwrap()
            .iter()
            .any(|path| path.file_name().and_then(|name| name.to_str()) == Some("continuity.json"))
    }
}
