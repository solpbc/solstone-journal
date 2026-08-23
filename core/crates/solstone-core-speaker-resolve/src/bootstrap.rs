// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bootstrap Person voiceprints from single-speaker segment evidence.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_entity::{
    EncoderIdentity, EntityResolutionEntity, EntityResolutionError, EntityResolutionOutcome,
    JournalEntity, VoiceprintItem, load_all_journal_entities, load_entity_voiceprints_file,
    read_journal_principal, record_entity_resolution_from_name_evidence, save_voiceprints_batch,
};
use solstone_core_journal_io::{PathOrDay, SegmentIdentityError, day_dirs, iter_segments};
use solstone_core_speaker_id::embeddings::load_embeddings_file;
use thiserror::Error;

use crate::admission::{
    admissible_person_pool, admissible_resolution_entities, saved_choice_excluded_by_admission,
};
use crate::evidence::load_segment_speakers_with_gaps;
use crate::owner_centroid::load_owner_centroid;
use crate::voiceprint_metadata::VoiceprintMetadata;

/// Kept verbatim from `solstone/apps/speakers/bootstrap.py`; consumed by the
/// name-variant scan when identifying merge candidates.
pub const NAME_MERGE_THRESHOLD: f64 = 0.90;
const RESOLUTION_FUZZY_THRESHOLD: f64 = 90.0;
const AI_CHAT_STREAMS: [&str; 3] = ["import.chatgpt", "import.claude", "import.gemini"];

#[derive(Debug, Clone, PartialEq)]
pub struct BootstrapRequest {
    pub journal_root: PathBuf,
    pub encoder: EncoderIdentity,
    pub added_at: i64,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootstrapStats {
    pub segments_scanned: usize,
    pub single_speaker_segments: usize,
    pub speakers_found: BTreeMap<String, usize>,
    pub entities_created: usize,
    pub embeddings_saved: usize,
    pub embeddings_skipped_owner: usize,
    pub embeddings_skipped_duplicate: usize,
    pub speakers_unmatched: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapOutcome {
    NoOwnerCentroid,
    Completed(BootstrapStats),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedFromImportsStats {
    pub segments_scanned: usize,
    pub segments_with_speakers: usize,
    pub speakers_found: BTreeMap<String, usize>,
    pub embeddings_saved: usize,
    pub embeddings_skipped_owner: usize,
    pub embeddings_skipped_duplicate: usize,
    pub speakers_unmatched: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedFromImportsOutcome {
    NoOwnerCentroid,
    Completed(SeedFromImportsStats),
}

#[derive(Debug, Error)]
pub enum BootstrapError {
    #[error("entity lookup failed: {0}")]
    Entity(#[from] solstone_core_entity::EntityStoreError),
    #[error("entity resolution failed: {0}")]
    Resolution(#[from] EntityResolutionError),
    #[error("entity lifecycle failed: {0}")]
    Lifecycle(#[from] solstone_core_entity::EntityLifecycleError),
    #[error("owner lookup failed: {0}")]
    Owner(#[from] crate::owner_centroid::OwnerCentroidError),
    #[error("journal path failed: {0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    #[error("journal scan failed at {path}: {source}")]
    Scan {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("segment path is not UTF-8 representable: {}", path.display())]
    NotUtf8 { path: PathBuf },
    #[error(
        "named stream directory \"_default\" cannot be spelled as a record identity: {}",
        path.display()
    )]
    AmbiguousNamedDefault { path: PathBuf },
    #[error("multiple segments share day {day:?} key {key:?}")]
    DuplicateDayKey { day: String, key: String },
    #[error(transparent)]
    Identity(SegmentIdentityError),
}

/// Outcome of the validation portion of a requested name merge.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeNamesOutcome {
    Ambiguous {
        field: &'static str,
        ambiguity_id: Option<String>,
        candidates: Vec<solstone_core_entity::ResolutionCandidate>,
    },
    AliasNotFound,
    CanonicalNotFound,
    SameEntity {
        entity_id: String,
    },
    PrincipalEntity {
        entity_id: String,
    },
    Ready {
        alias_entity_id: String,
        canonical_entity_id: String,
    },
}

/// Read the journal's one-speaker evidence and append safe Person voiceprints.
pub fn bootstrap_voiceprints(
    request: &BootstrapRequest,
) -> Result<BootstrapOutcome, BootstrapError> {
    let Some(principal) = read_journal_principal(&request.journal_root)? else {
        return Ok(BootstrapOutcome::NoOwnerCentroid);
    };
    let Some(owner_id) = principal.get("id").and_then(Value::as_str) else {
        return Ok(BootstrapOutcome::NoOwnerCentroid);
    };
    let Some(owner) = load_owner_centroid(&request.journal_root, owner_id)? else {
        return Ok(BootstrapOutcome::NoOwnerCentroid);
    };

    let entities = load_all_journal_entities(&request.journal_root)?;
    let all_entities = entities.iter().collect::<Vec<_>>();
    let unblocked = entities
        .iter()
        .filter(|entity| !entity.is_blocked())
        .collect::<Vec<_>>();
    let pool = admissible_person_pool(&unblocked);
    let resolution_entities = admissible_resolution_entities(&pool);
    let scope = json!({"kind": "journal"});
    let mut existing_keys = HashMap::<String, HashSet<String>>::new();
    let mut batches = BTreeMap::<String, Vec<VoiceprintItem>>::new();
    let mut entity_names = HashMap::<String, String>::new();
    let mut stats = BootstrapStats::default();

    for segment in scan_segments(&request.journal_root)? {
        stats.segments_scanned += 1;
        if segment.speakers.len() != 1 {
            continue;
        }
        stats.single_speaker_segments += 1;
        let speaker = &segment.speakers[0];
        if saved_choice_excluded_by_admission(
            &request.journal_root,
            &scope,
            speaker,
            &all_entities,
        )? {
            if !stats.speakers_unmatched.contains(speaker) {
                stats.speakers_unmatched.push(speaker.clone());
            }
            continue;
        }
        let resolution = record_entity_resolution_from_name_evidence(
            &request.journal_root,
            speaker,
            &resolution_entities,
            scope.clone(),
            json!({"lane": "speaker_resolve.bootstrap", "day": segment.day, "segment_id": segment.key}),
            RESOLUTION_FUZZY_THRESHOLD,
            false,
        )?;
        let (entity_id, entity_name) = match resolution.outcome {
            EntityResolutionOutcome::Resolved => resolution
                .entity_index
                .and_then(|index| pool.get(index).copied())
                .map(|entity| {
                    (
                        entity.id.clone(),
                        entity
                            .value
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(speaker)
                            .to_owned(),
                    )
                })
                .expect("resolved entity index belongs to admitted Person pool"),
            EntityResolutionOutcome::Ambiguous => {
                if !stats.speakers_unmatched.contains(speaker) {
                    stats.speakers_unmatched.push(speaker.clone());
                }
                continue;
            }
            EntityResolutionOutcome::NoMatch => {
                if !stats.speakers_unmatched.contains(speaker) {
                    stats.speakers_unmatched.push(speaker.clone());
                }
                continue;
            }
        };
        entity_names.insert(entity_id.clone(), entity_name.clone());
        stats.speakers_found.entry(entity_name.clone()).or_default();
        let keys = existing_keys.entry(entity_id.clone()).or_insert_with(|| {
            metadata_keys(load_entity_voiceprints_file(
                &request.journal_root,
                &entity_id,
            ))
        });
        for source in &segment.sources {
            let Ok(Some(embeddings)) =
                load_embeddings_file(&segment.path.join(format!("{source}.npz")))
            else {
                continue;
            };
            for (sentence_id, values) in embeddings.statements {
                let key = provenance_key(&segment.day, &segment.key, source, sentence_id);
                if keys.contains(&key) {
                    stats.embeddings_skipped_duplicate += 1;
                    continue;
                }
                let Some(embedding) = solstone_core_entity::normalize_embedding(&values) else {
                    continue;
                };
                if dot(&embedding, &owner.centroid) >= owner.threshold {
                    stats.embeddings_skipped_owner += 1;
                    continue;
                }
                keys.insert(key);
                *stats.speakers_found.entry(entity_name.clone()).or_default() += 1;
                batches
                    .entry(entity_id.clone())
                    .or_default()
                    .push(VoiceprintItem {
                        embedding,
                        metadata: VoiceprintMetadata::new(
                            &segment.day,
                            &segment.key,
                            source,
                            &segment.stream,
                            sentence_id,
                            request.added_at,
                            request.added_at,
                        )
                        .to_json(),
                    });
            }
        }
    }

    for (entity_id, items) in batches {
        if request.dry_run {
            stats.embeddings_saved += items.len();
            continue;
        }
        match save_voiceprints_batch(&request.journal_root, &entity_id, &items, &request.encoder) {
            Ok(saved) => stats.embeddings_saved += saved,
            Err(error) => stats.errors.push(format!(
                "Failed to save for {}: {error}",
                entity_names.get(&entity_id).unwrap_or(&entity_id),
            )),
        }
    }
    Ok(BootstrapOutcome::Completed(stats))
}

/// Seed existing entities from speaker-attributed non-AI import transcripts.
pub fn seed_from_imports(
    request: &BootstrapRequest,
) -> Result<SeedFromImportsOutcome, BootstrapError> {
    let Some(principal) = read_journal_principal(&request.journal_root)? else {
        return Ok(SeedFromImportsOutcome::NoOwnerCentroid);
    };
    let Some(owner_id) = principal.get("id").and_then(Value::as_str) else {
        return Ok(SeedFromImportsOutcome::NoOwnerCentroid);
    };
    let Some(owner) = load_owner_centroid(&request.journal_root, owner_id)? else {
        return Ok(SeedFromImportsOutcome::NoOwnerCentroid);
    };

    let entities = load_all_journal_entities(&request.journal_root)?;
    let all_entities = entities.iter().collect::<Vec<_>>();
    let unblocked = entities
        .iter()
        .filter(|entity| !entity.is_blocked())
        .collect::<Vec<_>>();
    let pool = admissible_person_pool(&unblocked);
    let resolution_entities = admissible_resolution_entities(&pool);
    let scope = json!({"kind": "journal"});
    let mut speaker_entity_cache = HashMap::<String, Option<(String, String)>>::new();
    let mut existing_keys = HashMap::<String, HashSet<String>>::new();
    let mut batches = BTreeMap::<String, Vec<VoiceprintItem>>::new();
    let mut stats = SeedFromImportsStats::default();

    for segment in scan_segments(&request.journal_root)? {
        if !segment.stream.starts_with("import.")
            || AI_CHAT_STREAMS.contains(&segment.stream.as_str())
        {
            continue;
        }
        stats.segments_scanned += 1;
        let speaker_entries = parse_conversation_speakers(&segment.path);
        if speaker_entries.is_empty() {
            continue;
        }
        stats.segments_with_speakers += 1;

        for source in &segment.sources {
            let source_path = segment.path.join(format!("{source}.jsonl"));
            let statement_times = if !source_path.exists() {
                HashMap::new()
            } else {
                match read_statement_times(&source_path) {
                    Ok(times) => times,
                    Err(error) => {
                        stats.errors.push(format!(
                            "Failed to read source transcript {}/{}/{}: {error}",
                            segment.day, segment.key, source
                        ));
                        continue;
                    }
                }
            };
            let Ok(Some(embeddings)) =
                load_embeddings_file(&segment.path.join(format!("{source}.npz")))
            else {
                continue;
            };
            for (sentence_id, values) in embeddings.statements {
                let Some(target_time) = statement_times.get(&sentence_id).copied() else {
                    continue;
                };
                let Some(speaker_name) = find_speaker_at_time(&speaker_entries, target_time) else {
                    continue;
                };
                if !speaker_entity_cache.contains_key(speaker_name) {
                    let entity = if saved_choice_excluded_by_admission(
                        &request.journal_root,
                        &scope,
                        speaker_name,
                        &all_entities,
                    )? {
                        None
                    } else {
                        let resolution = record_entity_resolution_from_name_evidence(
                            &request.journal_root,
                            speaker_name,
                            &resolution_entities,
                            scope.clone(),
                            json!({"lane": "apps.speakers.seed_from_imports", "day": segment.day, "segment_id": segment.key, "field": "speaker"}),
                            RESOLUTION_FUZZY_THRESHOLD,
                            false,
                        )?;
                        (resolution.outcome == EntityResolutionOutcome::Resolved)
                            .then(|| {
                                resolution
                                    .entity_index
                                    .and_then(|index| pool.get(index).copied())
                                    .map(|entity| {
                                        (
                                            entity.id.clone(),
                                            entity
                                                .value
                                                .get("name")
                                                .and_then(Value::as_str)
                                                .unwrap_or(speaker_name)
                                                .to_owned(),
                                        )
                                    })
                            })
                            .flatten()
                    };
                    speaker_entity_cache.insert(speaker_name.to_owned(), entity);
                }
                let entity = speaker_entity_cache
                    .get(speaker_name)
                    .expect("speaker cache entry was just inserted")
                    .clone();
                let Some((entity_id, entity_name)) = entity else {
                    if !stats
                        .speakers_unmatched
                        .iter()
                        .any(|name| name == speaker_name)
                    {
                        stats.speakers_unmatched.push(speaker_name.to_owned());
                    }
                    continue;
                };
                stats.speakers_found.entry(entity_name.clone()).or_default();
                let keys = existing_keys.entry(entity_id.clone()).or_insert_with(|| {
                    metadata_keys(load_entity_voiceprints_file(
                        &request.journal_root,
                        &entity_id,
                    ))
                });
                let key = provenance_key(&segment.day, &segment.key, source, sentence_id);
                if keys.contains(&key) {
                    stats.embeddings_skipped_duplicate += 1;
                    continue;
                }
                let Some(embedding) = solstone_core_entity::normalize_embedding(&values) else {
                    continue;
                };
                if dot(&embedding, &owner.centroid) >= owner.threshold {
                    stats.embeddings_skipped_owner += 1;
                    continue;
                }
                keys.insert(key);
                *stats.speakers_found.entry(entity_name).or_default() += 1;
                batches.entry(entity_id).or_default().push(VoiceprintItem {
                    embedding,
                    metadata: VoiceprintMetadata::new(
                        &segment.day,
                        &segment.key,
                        source,
                        &segment.stream,
                        sentence_id,
                        request.added_at,
                        request.added_at,
                    )
                    .to_json(),
                });
            }
        }
    }

    for (entity_id, items) in batches {
        if request.dry_run {
            stats.embeddings_saved += items.len();
            continue;
        }
        match save_voiceprints_batch(&request.journal_root, &entity_id, &items, &request.encoder) {
            Ok(saved) => stats.embeddings_saved += saved,
            Err(error) => stats
                .errors
                .push(format!("Failed to save for {entity_id}: {error}")),
        }
    }
    Ok(SeedFromImportsOutcome::Completed(stats))
}

/// Resolve both names and perform every merge guard before the write.
pub fn merge_names(
    journal_root: &Path,
    alias_name: &str,
    canonical_name: &str,
) -> Result<MergeNamesOutcome, BootstrapError> {
    let entities = load_all_journal_entities(journal_root)?;
    let resolution_entities = entities
        .iter()
        .map(JournalEntity::resolution_entity)
        .collect::<Vec<_>>();
    let alias = resolve_merge_name(journal_root, alias_name, &resolution_entities, "alias")?;
    let alias_id = match alias.outcome {
        EntityResolutionOutcome::Ambiguous => return Ok(ambiguous("alias", alias)),
        EntityResolutionOutcome::NoMatch => return Ok(MergeNamesOutcome::AliasNotFound),
        EntityResolutionOutcome::Resolved => entity_id(&entities, alias.entity_index),
    };
    let canonical = resolve_merge_name(
        journal_root,
        canonical_name,
        &resolution_entities,
        "canonical",
    )?;
    let canonical_id = match canonical.outcome {
        EntityResolutionOutcome::Ambiguous => return Ok(ambiguous("canonical", canonical)),
        EntityResolutionOutcome::NoMatch => return Ok(MergeNamesOutcome::CanonicalNotFound),
        EntityResolutionOutcome::Resolved => entity_id(&entities, canonical.entity_index),
    };
    if alias_id == canonical_id {
        return Ok(MergeNamesOutcome::SameEntity {
            entity_id: alias_id,
        });
    }
    if entities
        .iter()
        .any(|entity| entity.is_principal() && (entity.id == alias_id || entity.id == canonical_id))
    {
        return Ok(MergeNamesOutcome::PrincipalEntity {
            entity_id: entities
                .iter()
                .find(|entity| {
                    entity.is_principal() && (entity.id == alias_id || entity.id == canonical_id)
                })
                .map(|entity| entity.id.clone())
                .expect("principal entity was found"),
        });
    }
    Ok(MergeNamesOutcome::Ready {
        alias_entity_id: alias_id,
        canonical_entity_id: canonical_id,
    })
}

fn resolve_merge_name(
    journal_root: &Path,
    name: &str,
    entities: &[EntityResolutionEntity],
    field: &str,
) -> Result<solstone_core_entity::EntityResolution, BootstrapError> {
    Ok(record_entity_resolution_from_name_evidence(
        journal_root,
        name,
        entities,
        json!({"kind": "journal"}),
        json!({"lane": "speaker_resolve.bootstrap.merge_names", "field": field}),
        RESOLUTION_FUZZY_THRESHOLD,
        false,
    )?)
}

fn ambiguous(
    field: &'static str,
    resolution: solstone_core_entity::EntityResolution,
) -> MergeNamesOutcome {
    MergeNamesOutcome::Ambiguous {
        field,
        ambiguity_id: resolution.ambiguity_id,
        candidates: resolution.candidates,
    }
}

fn entity_id(entities: &[JournalEntity], index: Option<usize>) -> String {
    entities
        .get(index.expect("resolved outcome has an entity index"))
        .map(|entity| entity.id.clone())
        .expect("resolved entity index belongs to supplied entity list")
}

#[derive(Debug)]
pub(crate) struct ScannedSegment {
    pub(crate) day: String,
    pub(crate) stream: String,
    pub(crate) key: String,
    pub(crate) path: PathBuf,
    pub(crate) speakers: Vec<String>,
    pub(crate) sources: Vec<String>,
}

pub(crate) fn scan_segments(journal_root: &Path) -> Result<Vec<ScannedSegment>, BootstrapError> {
    let mut days = day_dirs(journal_root)?.into_iter().collect::<Vec<_>>();
    days.sort_by(|left, right| left.0.cmp(&right.0));
    let mut scanned = Vec::new();
    let mut seen = HashMap::<(String, String), ()>::new();
    for (day, path) in days {
        for segment in iter_segments(journal_root, PathOrDay::Directory(&path))? {
            let identity = match segment.record_identity() {
                Ok(identity) => identity,
                Err(SegmentIdentityError::NotUtf8 { path }) => {
                    return Err(BootstrapError::NotUtf8 { path });
                }
                Err(SegmentIdentityError::AmbiguousNamedDefault { path }) => {
                    return Err(BootstrapError::AmbiguousNamedDefault { path });
                }
                Err(error) => return Err(BootstrapError::Identity(error)),
            };
            if seen
                .insert((day.clone(), identity.key.to_owned()), ())
                .is_some()
            {
                return Err(BootstrapError::DuplicateDayKey {
                    day: day.clone(),
                    key: identity.key.to_owned(),
                });
            }
            let (speakers, _) = load_segment_speakers_with_gaps(segment.path());
            let mut sources = fs::read_dir(segment.path())
                .map_err(|source| BootstrapError::Scan {
                    path: segment.path().to_path_buf(),
                    source,
                })?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    (path.is_file() && path.extension().is_some_and(|extension| extension == "npz"))
                        .then(|| path.file_stem()?.to_str().map(str::to_owned))
                        .flatten()
                })
                .collect::<Vec<_>>();
            sources.sort();
            scanned.push(ScannedSegment {
                day: day.clone(),
                stream: identity.stream.to_owned(),
                key: identity.key.to_owned(),
                path: segment.path().to_path_buf(),
                speakers,
                sources,
            });
        }
    }
    Ok(scanned)
}

fn parse_conversation_speakers(segment_dir: &Path) -> Vec<(i64, String)> {
    let Ok(contents) = fs::read_to_string(segment_dir.join("conversation_transcript.jsonl")) else {
        return Vec::new();
    };
    let mut entries = contents
        .lines()
        .skip(1)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|entry| {
            let speaker = entry.get("speaker")?.as_str()?.trim();
            let start = entry.get("start")?.as_str()?;
            (!speaker.is_empty())
                .then(|| time_str_to_seconds(start).map(|seconds| (seconds, speaker.to_owned())))
                .flatten()
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|(seconds, _)| *seconds);
    entries
}

fn read_statement_times(path: &Path) -> Result<HashMap<i64, i64>, std::io::Error> {
    let contents = fs::read_to_string(path)?;
    Ok(contents
        .lines()
        .skip(1)
        .enumerate()
        .filter_map(|(index, line)| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|entry| entry.get("start")?.as_str().and_then(time_str_to_seconds))
                .map(|seconds| {
                    (
                        i64::try_from(index + 1).expect("line index fits i64"),
                        seconds,
                    )
                })
        })
        .collect())
}

fn time_str_to_seconds(value: &str) -> Option<i64> {
    let mut parts = value.split(':');
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next()?.parse::<i64>().ok()?;
    let second = parts.next()?.parse::<i64>().ok()?;
    (!value.is_empty()).then_some(hour * 3600 + minute * 60 + second)
}

fn find_speaker_at_time(entries: &[(i64, String)], target_seconds: i64) -> Option<&str> {
    let index = entries.partition_point(|(seconds, _)| *seconds <= target_seconds);
    index.checked_sub(1).map(|index| entries[index].1.as_str())
}

fn metadata_keys(archive: Option<solstone_core_entity::VoiceprintArchive>) -> HashSet<String> {
    archive
        .into_iter()
        .flat_map(|archive| archive.metadata)
        .filter_map(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter_map(|value| {
            Some(provenance_key(
                value.get("day")?.as_str()?,
                value.get("segment_key")?.as_str()?,
                value.get("source")?.as_str()?,
                value.get("sentence_id")?.as_i64()?,
            ))
        })
        .collect()
}

fn provenance_key(day: &str, segment_key: &str, source: &str, sentence_id: i64) -> String {
    format!("{day}|{segment_key}|{source}|{sentence_id}")
}

pub(crate) fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_segments_refuses_duplicate_day_keys() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260101/alpha/080000_300")).unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260101/beta/080000_300")).unwrap();
        let error = scan_segments(root.path()).unwrap_err();
        assert!(
            matches!(
                error,
                BootstrapError::DuplicateDayKey { ref day, ref key }
                    if day == "20260101" && key == "080000_300"
            ),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn scan_segments_refuses_non_utf8_names() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(
            root.path()
                .join("chronicle/20260101")
                .join(OsStr::from_bytes(b"s\xff"))
                .join("080000_300"),
        )
        .unwrap();
        let error = scan_segments(root.path()).unwrap_err();
        assert!(matches!(error, BootstrapError::NotUtf8 { .. }), "{error:?}");
    }
}
