// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic inventory survey for repairing non-Person speaker contamination.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_entity::{
    JournalEntity, is_admissible_person, try_load_entity_voiceprints_in_dir,
};
use solstone_core_journal_io::durability::{ArtifactId, DurableObservation, observe_json_durable};
use solstone_core_journal_io::{DirEntryKind, SegmentLayout, contained_path, list_dir_entries};
use solstone_core_speaker_id::labels::{compute_file_sha256, corrections_path, labels_path};

use crate::segment_catalog::catalog_journal;

/// Entity classification vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityClassification {
    AdmissibleActivePerson,
    ProtectedBlockedPerson,
    ProtectedInvalidPrincipal,
    RepairableNonPerson,
    Gap,
}

/// Classification of a speaker reference in a label or correction row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerRefClassification {
    AdmissibleActivePerson,
    ProtectedBlockedPerson,
    ProtectedInvalidPrincipal,
    RepairableNonPerson,
    MissingEntity,
    Unattributed,
    MalformedRow,
}

/// A specific gap encountered during inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairGap {
    pub path: PathBuf,
    pub reason: String,
}

/// Collision group where multiple directories share one effective ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollisionGroup {
    pub effective_id: String,
    pub directories: Vec<String>,
    pub is_relevant: bool,
}

/// Inventory status of a single entity directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityInventoryItem {
    pub entity_id: String,
    pub directory: String,
    pub classification: EntityClassification,
    pub entity_type: Option<String>,
    pub is_principal: bool,
    pub is_blocked: bool,
    pub voiceprint_count: usize,
    pub voiceprint_keys: Vec<Value>,
}

/// Voiceprint removal plan for an entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityVoiceprintRemovalPlan {
    pub entity_id: String,
    pub entity_dir: String,
    pub voiceprint_keys: Vec<Value>,
    pub voiceprint_count: usize,
}

/// Segment label repair plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentRepairPlan {
    pub day: String,
    pub stream_layout: SegmentLayout,
    pub stream: String,
    pub segment_name: String,
    pub segment_dir: PathBuf,
    pub current_label_sha256: String,
    pub current_corrections_sha256: String,
    pub contaminated_speakers: Vec<String>,
}

/// Counts and summaries for the inventory report.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepairInventorySummary {
    pub total_entities: usize,
    pub admissible_active_persons: usize,
    pub protected_blocked_persons: usize,
    pub protected_invalid_principals: usize,
    pub repairable_non_persons: usize,
    pub gap_entities: usize,
    pub planned_voiceprint_removals_count: usize,
    pub planned_segments_count: usize,
    pub user_authored_non_person_refs_count: usize,
    pub protected_entity_voiceprints_count: usize,
    pub labels_by_method: BTreeMap<String, usize>,
    pub labels_by_confidence: BTreeMap<String, usize>,
}

/// Full inventory report for non-Person contamination repair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairInventory {
    pub complete: bool,
    pub clean: bool,
    pub entities: Vec<EntityInventoryItem>,
    pub planned_removals: Vec<EntityVoiceprintRemovalPlan>,
    pub planned_segments: Vec<SegmentRepairPlan>,
    pub gaps: Vec<RepairGap>,
    pub collision_groups: Vec<CollisionGroup>,
    pub summary: RepairInventorySummary,
}

struct ScannedEntityDir {
    dir_name: String,
    effective_id: String,
    value: Option<Value>,
    classification: EntityClassification,
    voiceprint_count: usize,
    voiceprint_keys: Vec<Value>,
    has_voiceprints: bool,
}

/// Survey the entire journal to produce a deterministic repair inventory.
pub fn survey_repair_inventory(journal_root: &Path) -> Result<RepairInventory, String> {
    let entities_dir = contained_path(journal_root, "entities").map_err(|e| e.to_string())?;
    let mut scanned_dirs: Vec<ScannedEntityDir> = Vec::new();
    let mut gaps: Vec<RepairGap> = Vec::new();

    if entities_dir.is_dir() {
        let entries = list_dir_entries(&entities_dir).map_err(|e| e.to_string())?;
        for entry in entries {
            if entry.kind != DirEntryKind::Directory {
                continue;
            }
            let dir_name = entry.name.to_string_lossy().into_owned();
            let entity_json_path = entities_dir.join(&dir_name).join("entity.json");

            // 1. Observe entity.json
            let observation = observe_json_durable::<Value>(ArtifactId::Entity, &entity_json_path);

            // 2. Load voiceprints
            let (has_vp, vp_count, vp_keys) = match try_load_entity_voiceprints_in_dir(journal_root, &dir_name) {
                Ok(Some(archive)) => {
                    let mut parse_ok = true;
                    let mut parsed_keys = Vec::new();
                    for meta_str in &archive.metadata {
                        match serde_json::from_str::<Value>(meta_str) {
                            Ok(val) => parsed_keys.push(val),
                            Err(err) => {
                                parse_ok = false;
                                gaps.push(RepairGap {
                                    path: entities_dir.join(&dir_name).join("voiceprints.npz"),
                                    reason: format!("malformed metadata row: {err}"),
                                });
                            }
                        }
                    }
                    if parse_ok {
                        (true, archive.rows, parsed_keys)
                    } else {
                        (true, 0, Vec::new())
                    }
                }
                Ok(None) => (false, 0, Vec::new()),
                Err(err) => {
                    gaps.push(RepairGap {
                        path: entities_dir.join(&dir_name).join("voiceprints.npz"),
                        reason: format!("unreadable voiceprint archive: {err}"),
                    });
                    (true, 0, Vec::new())
                }
            };

            // 3. Classify entity based on observation
            match &observation {
                DurableObservation::Present(value) => {
                    let effective_id = value
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&dir_name)
                        .to_owned();

                    let journal_entity = JournalEntity {
                        id: effective_id.clone(),
                        value: value.clone(),
                    };

                    let classification = if is_admissible_person(&journal_entity) {
                        EntityClassification::AdmissibleActivePerson
                    } else if journal_entity.is_blocked() && journal_entity.entity_type() == Some("Person") {
                        EntityClassification::ProtectedBlockedPerson
                    } else if journal_entity.is_principal() {
                        EntityClassification::ProtectedInvalidPrincipal
                    } else {
                        EntityClassification::RepairableNonPerson
                    };

                    scanned_dirs.push(ScannedEntityDir {
                        dir_name,
                        effective_id,
                        value: Some(value.clone()),
                        classification,
                        voiceprint_count: vp_count,
                        voiceprint_keys: vp_keys,
                        has_voiceprints: has_vp,
                    });
                }
                DurableObservation::Absent => {
                    if has_vp {
                        // Orphan voiceprint archive without entity.json
                        gaps.push(RepairGap {
                            path: entity_json_path,
                            reason: "orphan voiceprint archive with absent entity.json".to_owned(),
                        });
                        scanned_dirs.push(ScannedEntityDir {
                            dir_name: dir_name.clone(),
                            effective_id: dir_name,
                            value: None,
                            classification: EntityClassification::Gap,
                            voiceprint_count: vp_count,
                            voiceprint_keys: vp_keys,
                            has_voiceprints: has_vp,
                        });
                    }
                }
                DurableObservation::Malformed { path, source } => {
                    gaps.push(RepairGap {
                        path: path.clone(),
                        reason: format!("malformed entity.json: {source}"),
                    });
                    scanned_dirs.push(ScannedEntityDir {
                        dir_name: dir_name.clone(),
                        effective_id: dir_name,
                        value: None,
                        classification: EntityClassification::Gap,
                        voiceprint_count: vp_count,
                        voiceprint_keys: vp_keys,
                        has_voiceprints: has_vp,
                    });
                }
                DurableObservation::Unreadable { path, source } => {
                    gaps.push(RepairGap {
                        path: path.clone(),
                        reason: format!("unreadable entity.json: {source}"),
                    });
                    scanned_dirs.push(ScannedEntityDir {
                        dir_name: dir_name.clone(),
                        effective_id: dir_name,
                        value: None,
                        classification: EntityClassification::Gap,
                        voiceprint_count: vp_count,
                        voiceprint_keys: vp_keys,
                        has_voiceprints: has_vp,
                    });
                }
            }
        }
    }

    // Build map of effective_id -> scanned dirs
    let mut effective_id_map: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, dir) in scanned_dirs.iter().enumerate() {
        if dir.classification != EntityClassification::Gap {
            effective_id_map
                .entry(dir.effective_id.clone())
                .or_default()
                .push(idx);
        }
    }

    // Catalog journal segments
    let cataloged_segments = match catalog_journal(journal_root) {
        Ok(segs) => segs,
        Err(err) => {
            gaps.push(RepairGap {
                path: journal_root.to_path_buf(),
                reason: format!("catalog journal failed: {err}"),
            });
            Vec::new()
        }
    };

    let mut planned_segments: Vec<SegmentRepairPlan> = Vec::new();
    let mut referenced_effective_ids: HashSet<String> = HashSet::new();
    let mut user_authored_non_person_refs_count = 0;
    let mut labels_by_method: BTreeMap<String, usize> = BTreeMap::new();
    let mut labels_by_confidence: BTreeMap<String, usize> = BTreeMap::new();

    for segment in &cataloged_segments {
        let label_p = labels_path(&segment.path);
        let corr_p = corrections_path(&segment.path);

        let label_sha256 = compute_file_sha256(&label_p).map_err(|e| e.to_string())?;
        let corr_sha256 = compute_file_sha256(&corr_p).map_err(|e| e.to_string())?;

        let mut segment_contaminated_speakers: Vec<String> = Vec::new();

        // 1. Scan labels
        if label_p.is_file() {
            match fs::read(&label_p) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(Value::Object(map)) => {
                        match map.get("labels") {
                            Some(Value::Array(rows)) => {
                                for row in rows {
                                    let method = row.get("method").and_then(Value::as_str).unwrap_or("unknown");
                                    *labels_by_method.entry(method.to_owned()).or_default() += 1;

                                    if let Some(conf) = row.get("confidence").and_then(Value::as_str) {
                                        *labels_by_confidence.entry(conf.to_owned()).or_default() += 1;
                                    }

                                    if let Some(speaker) = row.get("speaker").and_then(Value::as_str) {
                                        referenced_effective_ids.insert(speaker.to_owned());

                                        // Look up entity classification
                                        match effective_id_map.get(speaker) {
                                            Some(indices) => {
                                                let entity_dir = &scanned_dirs[indices[0]];
                                                if entity_dir.classification == EntityClassification::RepairableNonPerson {
                                                    if method.starts_with("user_") {
                                                        user_authored_non_person_refs_count += 1;
                                                    } else {
                                                        // System label to repairable non-person -> contamination!
                                                        if !segment_contaminated_speakers.contains(&speaker.to_owned()) {
                                                            segment_contaminated_speakers.push(speaker.to_owned());
                                                        }
                                                    }
                                                }
                                            }
                                            None => {
                                                // Missing entity reference
                                                gaps.push(RepairGap {
                                                    path: label_p.clone(),
                                                    reason: format!("label references missing entity {speaker}"),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                gaps.push(RepairGap {
                                    path: label_p.clone(),
                                    reason: "labels field is not an array".to_owned(),
                                });
                            }
                        }
                    }
                    Ok(_) => {
                        gaps.push(RepairGap {
                            path: label_p.clone(),
                            reason: "speaker_labels.json is not an object".to_owned(),
                        });
                    }
                    Err(err) => {
                        gaps.push(RepairGap {
                            path: label_p.clone(),
                            reason: format!("malformed speaker_labels.json: {err}"),
                        });
                    }
                },
                Err(err) => {
                    gaps.push(RepairGap {
                        path: label_p.clone(),
                        reason: format!("unreadable speaker_labels.json: {err}"),
                    });
                }
            }
        }

        // 2. Scan corrections (operative fields: corrected_speaker, original_speaker)
        if corr_p.is_file() {
            match fs::read(&corr_p) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(Value::Object(map)) => {
                        if let Some(Value::Array(rows)) = map.get("corrections") {
                            for row in rows {
                                for field in &["corrected_speaker", "original_speaker"] {
                                    if let Some(speaker) = row.get(*field).and_then(Value::as_str) {
                                        referenced_effective_ids.insert(speaker.to_owned());
                                        if !effective_id_map.contains_key(speaker) {
                                            gaps.push(RepairGap {
                                                path: corr_p.clone(),
                                                reason: format!(
                                                    "correction field {field} references missing entity {speaker}"
                                                ),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(_) => {
                        gaps.push(RepairGap {
                            path: corr_p.clone(),
                            reason: "speaker_corrections.json is not an object".to_owned(),
                        });
                    }
                    Err(err) => {
                        gaps.push(RepairGap {
                            path: corr_p.clone(),
                            reason: format!("malformed speaker_corrections.json: {err}"),
                        });
                    }
                },
                Err(err) => {
                    gaps.push(RepairGap {
                        path: corr_p.clone(),
                        reason: format!("unreadable speaker_corrections.json: {err}"),
                    });
                }
            }
        }

        if !segment_contaminated_speakers.is_empty() {
            planned_segments.push(SegmentRepairPlan {
                day: segment.day.clone(),
                stream_layout: segment.layout,
                stream: segment.stream.clone(),
                segment_name: segment.name.clone(),
                segment_dir: segment.path.clone(),
                current_label_sha256: label_sha256,
                current_corrections_sha256: corr_sha256,
                contaminated_speakers: segment_contaminated_speakers,
            });
        }
    }

    // 4. Analyze collisions
    let mut collision_groups: Vec<CollisionGroup> = Vec::new();
    let mut colliding_effective_ids: HashSet<String> = HashSet::new();

    for (effective_id, indices) in &effective_id_map {
        if indices.len() >= 2 {
            let dirs = indices
                .iter()
                .map(|i| scanned_dirs[*i].dir_name.clone())
                .collect::<Vec<_>>();

            let has_any_vp = indices.iter().any(|i| scanned_dirs[*i].has_voiceprints);
            let is_referenced = referenced_effective_ids.contains(effective_id);
            let is_relevant = has_any_vp || is_referenced;

            if is_relevant {
                colliding_effective_ids.insert(effective_id.clone());
                gaps.push(RepairGap {
                    path: entities_dir.join(&dirs[0]),
                    reason: format!(
                        "relevant collision on effective_id '{effective_id}' across directories: {:?}",
                        dirs
                    ),
                });
            }

            collision_groups.push(CollisionGroup {
                effective_id: effective_id.clone(),
                directories: dirs,
                is_relevant,
            });
        }
    }

    // 5. Build planned voiceprint removals and entity inventory items
    let mut planned_removals: Vec<EntityVoiceprintRemovalPlan> = Vec::new();
    let mut entity_items: Vec<EntityInventoryItem> = Vec::new();
    let mut summary = RepairInventorySummary {
        labels_by_method,
        labels_by_confidence,
        user_authored_non_person_refs_count,
        ..Default::default()
    };

    for dir in &scanned_dirs {
        summary.total_entities += 1;
        match dir.classification {
            EntityClassification::AdmissibleActivePerson => {
                summary.admissible_active_persons += 1;
            }
            EntityClassification::ProtectedBlockedPerson => {
                summary.protected_blocked_persons += 1;
                if dir.voiceprint_count > 0 {
                    summary.protected_entity_voiceprints_count += dir.voiceprint_count;
                }
            }
            EntityClassification::ProtectedInvalidPrincipal => {
                summary.protected_invalid_principals += 1;
                if dir.voiceprint_count > 0 {
                    summary.protected_entity_voiceprints_count += dir.voiceprint_count;
                }
            }
            EntityClassification::RepairableNonPerson => {
                summary.repairable_non_persons += 1;
                if dir.voiceprint_count > 0 && !colliding_effective_ids.contains(&dir.effective_id) {
                    planned_removals.push(EntityVoiceprintRemovalPlan {
                        entity_id: dir.effective_id.clone(),
                        entity_dir: dir.dir_name.clone(),
                        voiceprint_keys: dir.voiceprint_keys.clone(),
                        voiceprint_count: dir.voiceprint_count,
                    });
                }
            }
            EntityClassification::Gap => {
                summary.gap_entities += 1;
            }
        }

        let entity_type = dir
            .value
            .as_ref()
            .and_then(|v| v.get("type").and_then(Value::as_str))
            .map(str::to_owned);
        let is_principal = dir
            .value
            .as_ref()
            .and_then(|v| v.get("is_principal").and_then(Value::as_bool))
            .unwrap_or(false);
        let is_blocked = dir
            .value
            .as_ref()
            .and_then(|v| v.get("blocked").and_then(Value::as_bool))
            .unwrap_or(false);

        entity_items.push(EntityInventoryItem {
            entity_id: dir.effective_id.clone(),
            directory: dir.dir_name.clone(),
            classification: dir.classification,
            entity_type,
            is_principal,
            is_blocked,
            voiceprint_count: dir.voiceprint_count,
            voiceprint_keys: dir.voiceprint_keys.clone(),
        });
    }

    summary.planned_voiceprint_removals_count = planned_removals.len();
    summary.planned_segments_count = planned_segments.len();

    let complete = gaps.is_empty();
    let clean = complete
        && planned_removals.is_empty()
        && planned_segments.is_empty()
        && summary.user_authored_non_person_refs_count == 0
        && summary.protected_entity_voiceprints_count == 0;

    Ok(RepairInventory {
        complete,
        clean,
        entities: entity_items,
        planned_removals,
        planned_segments,
        gaps,
        collision_groups,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_entity(
        root: &Path,
        dir_name: &str,
        id_field: &str,
        entity_type: &str,
        is_blocked: bool,
        is_principal: bool,
        has_vp: bool,
    ) {
        let dir = root.join("entities").join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        let entity_json = serde_json::json!({
            "id": id_field,
            "type": entity_type,
            "blocked": is_blocked,
            "is_principal": is_principal,
            "names": [id_field],
            "display_name": id_field
        });
        fs::write(dir.join("entity.json"), serde_json::to_vec_pretty(&entity_json).unwrap()).unwrap();

        if has_vp {
            let items = vec![solstone_core_entity::VoiceprintItem {
                embedding: vec![0.0; 256],
                metadata: serde_json::json!({
                    "day": "20260101",
                    "segment_key": "20260101_001",
                    "source": "audio",
                    "sentence_id": 1,
                }),
            }];
            let encoder = solstone_core_entity::EncoderIdentity {
                id: "test".to_string(),
                sha256: "0".repeat(64),
                width: 256,
            };
            solstone_core_entity::save_voiceprints_batch(root, dir_name, &items, &encoder).unwrap();
        }
    }

    #[test]
    fn test_inventory_classifications_and_residuals() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        // 1. Active unblocked Person
        make_entity(root, "alice", "alice", "Person", false, false, true);
        // 2. Blocked Person
        make_entity(root, "bob", "bob", "Person", true, false, true);
        // 3. Principal entity failing unblocked Person (e.g. Place)
        make_entity(root, "hq", "hq", "Place", false, true, true);
        // 4. Valid repairable non-Person with voiceprints
        make_entity(root, "acme_corp", "acme_corp", "Organization", false, false, true);
        // 5. Valid repairable non-Person clean
        make_entity(root, "widget_proj", "widget_proj", "Project", false, false, false);

        let inv = survey_repair_inventory(root).unwrap();

        assert!(inv.complete);
        assert!(!inv.clean); // Has residuals and planned removals

        let alice = inv.entities.iter().find(|e| e.entity_id == "alice").unwrap();
        assert_eq!(alice.classification, EntityClassification::AdmissibleActivePerson);

        let bob = inv.entities.iter().find(|e| e.entity_id == "bob").unwrap();
        assert_eq!(bob.classification, EntityClassification::ProtectedBlockedPerson);

        let hq = inv.entities.iter().find(|e| e.entity_id == "hq").unwrap();
        assert_eq!(hq.classification, EntityClassification::ProtectedInvalidPrincipal);

        let acme = inv.entities.iter().find(|e| e.entity_id == "acme_corp").unwrap();
        assert_eq!(acme.classification, EntityClassification::RepairableNonPerson);

        let widget = inv.entities.iter().find(|e| e.entity_id == "widget_proj").unwrap();
        assert_eq!(widget.classification, EntityClassification::RepairableNonPerson);

        // Only acme_corp is planned for removal
        assert_eq!(inv.planned_removals.len(), 1);
        assert_eq!(inv.planned_removals[0].entity_id, "acme_corp");
    }

    #[test]
    fn test_orphan_archive_and_collisions() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        // Orphan archive: create valid entity, save voiceprints, then delete entity.json
        make_entity(root, "orphan_entity", "orphan_entity", "Person", false, false, true);
        fs::remove_file(root.join("entities/orphan_entity/entity.json")).unwrap();

        let inv = survey_repair_inventory(root).unwrap();
        assert!(!inv.complete);
        assert!(inv.gaps.iter().any(|g| g.reason.contains("orphan voiceprint archive")));
    }

    #[test]
    fn test_malformed_entity_with_voiceprints_is_named_gap_and_incomplete() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_entity(root, "bad_ent", "bad_ent", "Person", false, false, true);
        fs::write(root.join("entities/bad_ent/entity.json"), b"invalid json {").unwrap();

        let inv = survey_repair_inventory(root).unwrap();
        assert!(!inv.complete);
        assert!(inv.gaps.iter().any(|g| g.path.to_string_lossy().contains("bad_ent")));
    }

    #[test]
    fn test_relevant_collision_is_gap_without_planned_removals() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        // Two directories claiming the same id, one has voiceprints
        make_entity(root, "dir_a", "dir_a", "Organization", false, false, true);
        let entity_json = serde_json::json!({
            "id": "shared_id",
            "type": "Organization",
            "blocked": false,
            "is_principal": false,
            "names": ["dir_a"],
            "display_name": "dir_a"
        });
        fs::write(root.join("entities/dir_a/entity.json"), serde_json::to_vec_pretty(&entity_json).unwrap()).unwrap();
        make_entity(root, "dir_b", "shared_id", "Organization", false, false, false);

        let inv = survey_repair_inventory(root).unwrap();
        assert!(!inv.complete);
        assert!(inv.gaps.iter().any(|g| g.reason.contains("collision")));
        assert!(inv.planned_removals.is_empty());
    }

    #[test]
    fn test_irrelevant_collision_is_reported_group_without_incomplete() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        // Two directories claiming same id, neither has voiceprints, no labels reference them
        make_entity(root, "dir_a", "shared_id", "Person", false, false, false);
        make_entity(root, "dir_b", "shared_id", "Person", false, false, false);

        let inv = survey_repair_inventory(root).unwrap();
        assert!(inv.complete);
        assert_eq!(inv.collision_groups.len(), 1);
        assert_eq!(inv.collision_groups[0].effective_id, "shared_id");
    }

    #[test]
    fn test_missing_entity_label_reference_is_gap() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        let seg_dir = root.join("chronicle/20260808/120000_300/talents");
        fs::create_dir_all(&seg_dir).unwrap();
        let labels_json = serde_json::json!({
            "schema_version": 1,
            "labels": [{
                "speaker": "ghost_entity",
                "method": "attributed",
                "segment_id": 1,
            }]
        });
        fs::write(seg_dir.join("speaker_labels.json"), serde_json::to_vec_pretty(&labels_json).unwrap()).unwrap();

        let inv = survey_repair_inventory(root).unwrap();
        assert!(!inv.complete);
        assert!(inv.gaps.iter().any(|g| g.reason.contains("missing entity") && g.reason.contains("ghost_entity")));
    }

    #[test]
    fn test_unattributed_label_does_not_block_complete() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        let seg_dir = root.join("chronicle/20260808/120000_300/talents");
        fs::create_dir_all(&seg_dir).unwrap();
        let labels_json = serde_json::json!({
            "schema_version": 1,
            "labels": [{
                "speaker": null,
                "speaker_name": "Speaker 1",
                "method": "unattributed",
                "segment_id": 1,
            }]
        });
        fs::write(seg_dir.join("speaker_labels.json"), serde_json::to_vec_pretty(&labels_json).unwrap()).unwrap();

        let inv = survey_repair_inventory(root).unwrap();
        assert!(inv.complete);
    }

    #[test]
    fn test_user_authored_non_person_reference_is_not_planned() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        make_entity(root, "org_entity", "org_entity", "Organization", false, false, false);

        let seg_dir = root.join("chronicle/20260808/120000_300/talents");
        fs::create_dir_all(&seg_dir).unwrap();
        let labels_json = serde_json::json!({
            "schema_version": 1,
            "labels": [{
                "speaker": "org_entity",
                "method": "user_assigned",
                "segment_id": 1,
            }]
        });
        fs::write(seg_dir.join("speaker_labels.json"), serde_json::to_vec_pretty(&labels_json).unwrap()).unwrap();

        let inv = survey_repair_inventory(root).unwrap();
        assert!(inv.complete);
        assert!(!inv.clean); // Has non-Person residual
        assert!(inv.planned_segments.is_empty()); // But segment is NOT planned because user-authored!
    }

    #[test]
    fn test_labels_key_not_array_is_gap() {
        let temp = tempdir().unwrap();
        let root = temp.path();

        let seg_dir = root.join("chronicle/20260808/120000_300/talents");
        fs::create_dir_all(&seg_dir).unwrap();
        let labels_json = serde_json::json!({
            "schema_version": 1,
            "labels": "not-an-array"
        });
        fs::write(seg_dir.join("speaker_labels.json"), serde_json::to_vec_pretty(&labels_json).unwrap()).unwrap();

        let inv = survey_repair_inventory(root).unwrap();
        assert!(!inv.complete);
        assert!(inv.gaps.iter().any(|g| g.reason.contains("labels field is not an array")));
    }
}
