// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
//! Reconcile one committed meeting, using only private, screen-bound evidence.
use super::*;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    LockOptions, PathOrDay, StreamLocation, contained_path, iter_segments,
};
use solstone_core_segment::output_attempt::OutputClaims;
use std::{collections::BTreeMap, path::Path};

pub(super) fn claim_options() -> LockOptions {
    // Derived work under entity trust never waits on a producer that could be
    // waiting for a segment being removed. The maintenance pass retries it.
    LockOptions {
        timeout: std::time::Duration::ZERO,
        ..LockOptions::default()
    }
}
pub fn source_prefix(source: &str) -> String {
    format!(
        ".faces-v1-{}",
        &format!("{:x}", Sha256::digest(source.as_bytes()))[..16]
    )
}
fn text<'a>(v: &'a Value, field: &str) -> &'a str {
    v.get(field).and_then(Value::as_str).unwrap_or("")
}
fn normalize(s: &str) -> String {
    let mut depth = 0;
    let mut out = String::new();
    for c in s.split('|').next().unwrap_or("").to_lowercase().chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            _ if depth == 0 && (c.is_alphanumeric() || c.is_whitespace()) => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn aliases(entity: &crate::JournalEntity) -> Vec<String> {
    let mut names = vec![normalize(text(&entity.value, "name"))];
    names.extend(
        entity
            .value
            .get("aka")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(normalize),
    );
    names.retain(|s| !s.is_empty());
    names
}
/// A unique exact normalized name/alias among the record's resolved people.
/// The principal is deliberately admitted independently: participation omits it.
pub fn participant_entity(
    name: &str,
    participation: &[Value],
    entities: &[crate::JournalEntity],
) -> Option<String> {
    let name = normalize(name);
    if name.is_empty() {
        return None;
    }
    let mut matches = BTreeSet::new();
    for entity in entities {
        if !crate::is_admissible_person(entity) {
            continue;
        }
        let entries: Vec<_> = participation
            .iter()
            .filter(|p| text(p, "entity_id") == entity.id)
            .collect();
        if (entity.is_principal() || !entries.is_empty())
            && (aliases(entity).contains(&name)
                || entries.iter().any(|p| normalize(text(p, "name")) == name))
        {
            matches.insert(entity.id.clone());
        }
    }
    (matches.len() == 1)
        .then(|| matches.into_iter().next())
        .flatten()
}

/// Restrict filesystem work to current participants, the principal and people
/// who already hold face material. The authoritative identity census still
/// resolves collisions and aliases; unrelated identities cannot join a meeting.
pub fn relevant_entities(
    record: &Map<String, Value>,
    directories: &[(String, crate::JournalEntity)],
    holders: &BTreeSet<String>,
) -> Vec<(String, crate::JournalEntity)> {
    let ids: BTreeSet<_> = record
        .get("participation")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("entity_id").and_then(Value::as_str))
        .collect();
    directories
        .iter()
        .filter(|(folder, e)| {
            e.is_principal() || ids.contains(e.id.as_str()) || holders.contains(folder)
        })
        .cloned()
        .collect()
}

pub fn face_holders(
    journal: &Path,
    directories: &[(String, crate::JournalEntity)],
) -> Result<BTreeSet<String>> {
    let mut holders = BTreeSet::new();
    for (folder, _) in directories {
        let dir = solstone_core_journal_io::contained_path(journal, &format!("entities/{folder}"))
            .map_err(|e| e.to_string())?;
        for name in [ARCHIVE, STATE] {
            if solstone_core_journal_io::path_lexists(&dir.join(name)).map_err(|e| e.to_string())? {
                holders.insert(folder.clone());
                break;
            }
        }
    }
    Ok(holders)
}

pub fn reconcile_activity(
    journal: &Path,
    facet: &str,
    day: &str,
    record: &Map<String, Value>,
) -> Result<usize> {
    if !enabled(journal)? {
        return Ok(0);
    }
    let _trust = crate::hold_entity_trust_lock(journal).map_err(|e| e.to_string())?;
    let directories = crate::live_journal_entities(journal).map_err(|e| e.to_string())?;
    let holders = face_holders(journal, &directories)?;
    let relevant = relevant_entities(record, &directories, &holders);
    reconcile_activity_with_entities(journal, facet, day, record, &relevant)
}

/// A batch resolves identities once while holding entity trust for the batch.
pub fn reconcile_activity_with_entities(
    journal: &Path,
    facet: &str,
    day: &str,
    record: &Map<String, Value>,
    directories: &[(String, crate::JournalEntity)],
) -> Result<usize> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(|e| e.to_string())?;
    if !enabled(journal)? {
        return Ok(0);
    }
    let activity = record
        .get("id")
        .and_then(Value::as_str)
        .ok_or("activity id missing")?;
    let coordinate = serde_json::to_string(&[facet, day, activity]).map_err(|e| e.to_string())?;
    let participation = record
        .get("participation")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let ids: BTreeSet<_> = participation
        .iter()
        .filter_map(|p| p.get("entity_id").and_then(Value::as_str))
        .collect();
    let entities: Vec<_> = directories
        .iter()
        .filter(|(_, e)| e.is_principal() || ids.contains(e.id.as_str()))
        .map(|(_, e)| e.clone())
        .collect();
    let mut desired: BTreeMap<String, Vec<Face>> = BTreeMap::new();
    let mut encoder = None;
    let mut claims = vec![];
    if record.get("activity").and_then(Value::as_str) == Some("meeting") {
        let segments: BTreeSet<_> = record
            .get("segments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let stream = record
            .get("stream")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let located =
            if solstone_core_journal_io::path_lexists(&journal.join("chronicle").join(day))
                .map_err(|e| e.to_string())?
            {
                iter_segments(journal, PathOrDay::Day(day)).map_err(|e| e.to_string())?
            } else {
                vec![]
            };
        for segment in located {
            if !segments.contains(segment.name().to_str().unwrap_or(""))
                || solstone_core_segment::is_body_only_segment(&segment)
            {
                continue;
            }
            let (stream_name, layout) = match segment.stream() {
                StreamLocation::Direct => ("_default".to_owned(), "direct"),
                StreamLocation::Named(s) => (s.to_string_lossy().into_owned(), "named"),
            };
            if stream.is_some_and(|s| s != stream_name || (s == "_default" && layout != "direct")) {
                continue;
            }
            if !matches!(
                solstone_core_segment::owner_deletion_state(segment.path()),
                Ok(solstone_core_segment::OwnerDeletionState::Live)
            ) {
                continue;
            }
            let mut manifests = solstone_core_journal_io::list_dir_entries(segment.path())
                .map_err(|e| e.to_string())?;
            manifests.sort_by_key(|e| e.name.clone());
            let claim = OutputClaims::discover(segment.path(), claim_options())
                .map_err(|e| e.to_string())?;
            for manifest in manifests {
                let filename = manifest.name.to_string_lossy().into_owned();
                if !filename.starts_with(".faces-v1-") || !filename.ends_with(".jsonl") {
                    continue;
                }
                // Claim discovery covers the screen even when its raw is retained away.
                let bytes = solstone_core_journal_io::read_bytes(&manifest.path, vec![])
                    .map_err(|e| e.to_string())?;
                let mut lines = bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty());
                let header: Value =
                    serde_json::from_slice(lines.next().ok_or("empty face manifest")?)
                        .map_err(|e| e.to_string())?;
                let source = text(&header, "source_name");
                if source.contains(['/', '\\'])
                    || filename != format!("{}.jsonl", source_prefix(source))
                    || header["schema_version"] != 1
                {
                    return Err("invalid face manifest identity".into());
                }
                let screen = segment.path().join(source).with_extension("jsonl");
                if header["screen_body_sha256"].as_str()
                    != Some(
                        solstone_core_segment::screen_body_digest(&screen)
                            .map_err(|e| e.to_string())?
                            .as_str(),
                    )
                {
                    return Err("face evidence no longer matches the screen reading".into());
                }
                let e: Encoder = serde_json::from_value(header["embedder"].clone())
                    .map_err(|e| e.to_string())?;
                if e.width != WIDTH || encoder.as_ref().is_some_and(|old| old != &e) {
                    return Err("face encoder mismatch".into());
                }
                encoder = Some(e);
                let npy = text(&header, "embeddings");
                if npy.contains(['/', '\\'])
                    || !npy.starts_with(&format!("{}-", source_prefix(source)))
                    || !npy.ends_with(".npy")
                {
                    return Err("invalid face embeddings reference".into());
                }
                let embeddings = codec::embeddings(
                    &solstone_core_journal_io::read_bytes(segment.path().join(npy), vec![])
                        .map_err(|e| e.to_string())?,
                )?;
                let faces: Vec<Value> = lines
                    .map(|l| serde_json::from_slice(l).map_err(|e| e.to_string()))
                    .collect::<Result<_>>()?;
                if faces.len() != embeddings.len() {
                    return Err("face sidecar rows differ".into());
                }
                let screen_rows: Vec<Value> = solstone_core_journal_io::read_optional_text(&screen)
                    .map_err(|e| e.to_string())?
                    .ok_or("screen reading disappeared")?
                    .lines()
                    .skip(1)
                    .map(|l| serde_json::from_str(l).map_err(|e| e.to_string()))
                    .collect::<Result<_>>()?;
                for f in &faces {
                    let frame_id = f["frame_id"].as_u64().ok_or("face frame id missing")?;
                    let Some(frame) = screen_rows
                        .iter()
                        .find(|r| r["frame_id"].as_u64() == Some(frame_id))
                    else {
                        return Err("face frame missing from screen".into());
                    };
                    let parts = frame
                        .pointer("/content/meeting/participants")
                        .and_then(Value::as_array)
                        .ok_or("face frame has no participants")?;
                    let mut associations = vec![];
                    if let Some(label) = f.get("label").filter(|v| v.is_object())
                        && let Some(entity) = participant_entity(
                            text(label, "participant_name"),
                            participation,
                            &entities,
                        )
                    {
                        associations.push((
                            entity,
                            Source::Anchored,
                            label["text"].as_str().map(str::to_owned),
                        ));
                    }
                    if let Some(index) = f["in_box"].as_u64()
                        && let Some(p) = parts.get(index as usize)
                        && let Some(b) = p["box_2d"].as_array().filter(|a| a.len() == 4)
                    {
                        let coords: Vec<_> = b
                            .iter()
                            .filter_map(Value::as_f64)
                            .map(|v| v.clamp(0., 1000.))
                            .collect();
                        let sole = faces
                            .iter()
                            .filter(|g| {
                                g["frame_id"] == f["frame_id"] && g["in_box"] == f["in_box"]
                            })
                            .count()
                            == 1;
                        if sole
                            && f["det_score"].as_f64().unwrap_or(0.) >= 0.85
                            && f["box"][2].as_f64().unwrap_or(0.) >= 40.
                            && coords.len() == 4
                            && (coords[2] - coords[0]) * (coords[3] - coords[1]) < 350000.
                            && let Some(entity) =
                                participant_entity(text(p, "name"), participation, &entities)
                            && !associations.iter().any(|(e, _, _)| e == &entity)
                        {
                            associations.push((entity, Source::Box, None));
                        }
                    }
                    for (entity, source_kind, label_text) in associations {
                        let key = FaceKey {
                            day: day.into(),
                            stream: stream_name.clone(),
                            stream_layout: layout.into(),
                            segment_key: segment.name().to_string_lossy().into_owned(),
                            source_name: source.into(),
                            frame_id,
                            face_idx: f["face_idx"].as_u64().ok_or("face index missing")? as usize,
                        };
                        let crop = f["crop"].as_str().map(str::to_owned);
                        if let Some(c) = &crop {
                            super::crop_path(journal, &key, c)?;
                        }
                        let embedding = embeddings
                            .get(
                                f["emb_row"]
                                    .as_u64()
                                    .ok_or("face embedding index missing")?
                                    as usize,
                            )
                            .ok_or("face embedding index out of range")?
                            .clone();
                        let metadata = Metadata {
                            schema_version: 1,
                            key,
                            activities: BTreeSet::from([coordinate.clone()]),
                            source: source_kind,
                            label_text,
                            quality: f["quality"].as_f64().ok_or("face quality missing")? as f32,
                            det_score: f["det_score"].as_f64().ok_or("face score missing")? as f32,
                            face_w: f["box"][2].as_f64().ok_or("face width missing")? as f32,
                            category: text(f, "category").into(),
                            crop,
                            added_at: chrono::Utc::now().to_rfc3339(),
                            rejected: false,
                        };
                        let pool = desired.entry(entity).or_default();
                        if !pool.iter().any(|old| old.metadata.key == metadata.key) {
                            pool.push(Face {
                                embedding,
                                metadata,
                            });
                        }
                    }
                }
                claim
                    .segment_identity(segment.path())
                    .map_err(|e| e.to_string())?;
            }
            claims.push((claim, segment.path().to_owned()));
        }
    }
    // No current sidecar can mean participation or the meeting itself was removed.
    if encoder.is_none() {
        for (folder, _) in directories {
            if let Some(a) = load_in_dir(journal, folder)?.0 {
                encoder = Some(a.envelope.encoder);
                break;
            }
        }
    }
    let Some(encoder) = encoder else {
        return Ok(0);
    };
    for (claim, segment) in &claims {
        claim.segment_identity(segment).map_err(|e| e.to_string())?;
    }
    super::storage::reconcile_in_dirs(journal, &coordinate, &encoder, &desired, directories)
}

/// Clear all face material explicitly, independent of the collection switch.
/// Callers turn the switch off first, so a subsequent describe cannot refill it.
pub fn clear(journal: &Path, surface: solstone_core_removals::Surface) -> Result<usize> {
    let _trust = crate::hold_entity_trust_lock(journal).map_err(|e| e.to_string())?;
    if enabled(journal)? {
        return Err("turn off face suggestions before clearing face data".into());
    }
    crate::recover_interrupted_entity_merge(journal)?;
    // Acquired before the first deletion; it never waits behind a trash move
    // that could be waiting for entity trust.
    let trash = solstone_core_trash::hold_payload_scrub(journal).map_err(|e| e.to_string())?;
    let removal = solstone_core_removals::Removal::begin(
        journal,
        solstone_core_removals::Action::FacesWipe,
        surface,
    );
    removal.commit(solstone_core_removals::Unit::Faces, |io| -> Result<usize> {
        let mut count = 0;
        let derived = super::cross::path(journal)?;
        if solstone_core_journal_io::path_lexists(&derived).map_err(|e| e.to_string())? {
            remove_path(journal, io, &derived)?;
            count += 1;
        }
        super::cross::clear_candidates(journal)?;
        for (_, day) in solstone_core_journal_io::day_dirs(journal).map_err(|e| e.to_string())? {
            for segment in
                iter_segments(journal, PathOrDay::Directory(&day)).map_err(|e| e.to_string())?
            {
                // Fence every source, including a first publication with no
                // installed sidecar yet. Retain only one source's handles.
                let claims = OutputClaims::discover(segment.path(), claim_options())
                    .map_err(|e| e.to_string())?;
                claims
                    .segment_identity(segment.path())
                    .map_err(|e| e.to_string())?;
                let paths: Vec<_> = solstone_core_journal_io::list_dir_entries(segment.path())
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .filter(|e| face_sidecar_name(&e.name.to_string_lossy()))
                    .map(|e| e.path)
                    .collect();
                for p in paths {
                    remove_path(journal, io, &p)?;
                    count += 1;
                }
            }
        }
        for entry in solstone_core_journal_io::list_dir_entries(&journal.join("entities"))
            .map_err(|e| e.to_string())?
        {
            if entry.kind == solstone_core_journal_io::DirEntryKind::Directory {
                for child in solstone_core_journal_io::list_dir_entries(&entry.path)
                    .map_err(|e| e.to_string())?
                {
                    if entity_face_or_stage(&child.name.to_string_lossy()) {
                        remove_path(journal, io, &child.path)?;
                        count += 1;
                    }
                }
            }
        }
        count += trash
            .remove_files(|relative| {
                let components: Vec<_> = relative.split('/').collect();
                match components.as_slice() {
                    ["entities", name] => *name == super::cross::FILE,
                    ["entities", _, name] => entity_face_or_stage(name),
                    ["chronicle", ..] => components.iter().any(|s| face_sidecar_name(s)),
                    _ => false,
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(count)
    })
}

fn face_sidecar_name(name: &str) -> bool {
    (name.starts_with(".faces-v1-") && (name.ends_with(".jsonl") || name.ends_with(".npy") || name.ends_with("-crops")))
        // Source claims fence all describe publication; these unpublished
        // candidates can contain embeddings or manifests after a crash.
        || (name.starts_with(".describe-stage-") && name.ends_with(".tmp"))
}
fn entity_face_or_stage(name: &str) -> bool {
    [ARCHIVE, STATE, PHOTO].contains(&name)
        // Entity trust excludes writers. Abandoned candidates have never
        // become authoritative identity or owner data.
        || solstone_core_journal_io::is_publication_candidate_name(std::ffi::OsStr::new(name))
}
fn remove_path(journal: &Path, io: &solstone_core_removals::RemovalIo<'_>, p: &Path) -> Result<()> {
    let rel = p
        .strip_prefix(journal)
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let p = contained_path(journal, &rel).map_err(|e| e.to_string())?;
    if p.is_dir() {
        io.remove_tree(&rel).map_err(|e| e.to_string())
    } else {
        io.remove_file(&rel).map(|_| ()).map_err(|e| e.to_string())
    }
}
