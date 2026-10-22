// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::router::{assemble_journal_entity_records, journal_bucket_rank, journal_type_bucket};

#[derive(Debug, Serialize)]
pub struct IdTidyUpRow {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub facets: Vec<String>,
    pub kept_notes_count: usize,
    pub last_active_day: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IdGroupResponse {
    pub actionable: bool,
    pub count: usize,
    pub items: Option<Vec<IdTidyUpRow>>,
    pub members: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct DuplicateItem {
    pub pair: [String; 2],
    pub facets: Vec<String>,
    pub keys: Vec<String>,
}

/// One suggested merge pair and the review keys that formed it.
///
/// The member list can contain both directions (A→B and B→A) and several
/// targets from one source (A→B and A→C), and merging every pair depends
/// on order.
#[derive(Debug, Serialize)]
pub struct DuplicateMember {
    pub pair: [String; 2],
    pub keys: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DuplicatesGroupResponse {
    pub error: Option<String>,
    pub count: Option<usize>,
    pub items: Option<Vec<DuplicateItem>>,
    pub members: Option<Vec<DuplicateMember>>,
}

#[derive(Debug, Serialize)]
pub struct DamageReport {
    pub count: usize,
    pub paths: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TidyUpResponse {
    pub offset: usize,
    pub limit: usize,
    pub damage: DamageReport,
    pub id_shaped: IdGroupResponse,
    pub duplicates: DuplicatesGroupResponse,
}

#[derive(Debug)]
pub enum TidyUpError {
    JournalReadFailed,
}

struct RawDuplicateRow {
    source_slug: String,
    target_slug: String,
    facet: String,
    key: String,
    composite: f64,
}

pub fn build_tidy_up_response(
    journal_root: &Path,
    offset: usize,
    limit: usize,
) -> Result<TidyUpResponse, TidyUpError> {
    let plan = solstone_core_facets::plan_facet_entity_relationships(journal_root)
        .map_err(|_| TidyUpError::JournalReadFailed)?;

    let assembled_records = assemble_journal_entity_records(journal_root, None)
        .map_err(|_| TidyUpError::JournalReadFailed)?;

    let mut last_active_day_by_folder: HashMap<String, Option<String>> = HashMap::new();
    for record in &assembled_records {
        if let Some(folder) = record.get("id").and_then(Value::as_str) {
            let day = record
                .get("last_active_day")
                .and_then(Value::as_str)
                .map(str::to_owned);
            last_active_day_by_folder.insert(folder.to_owned(), day);
        }
    }

    let identity_map = solstone_core_entity::read_identity_map(journal_root)
        .map_err(|_| TidyUpError::JournalReadFailed)?;

    let mut relationships_by_id: HashMap<
        String,
        Vec<&solstone_core_facets::PlannedFacetRelationship>,
    > = HashMap::new();
    for rel in &plan.relationships {
        relationships_by_id
            .entry(rel.entity_id.clone())
            .or_default()
            .push(rel);
    }

    let mut sorted_resolved: Vec<(&String, &String)> = identity_map.resolved.iter().collect();
    sorted_resolved.sort_by_key(|(id, _)| *id);

    let mut id_members = Vec::new();
    for (effective_id, winner_dir) in sorted_resolved {
        let Ok(Some(identity)) =
            solstone_core_entity::read_entity_identity(journal_root, winner_dir)
        else {
            continue;
        };

        let name = identity
            .value()
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !crate::id_shape::looks_like_generated_id(name) {
            continue;
        }

        if solstone_core_entity::entity_is_hidden(identity.value()) {
            continue;
        }

        if solstone_core_entity::entity_is_hide_protected(&identity) {
            continue;
        }

        let attributed = relationships_by_id
            .get(effective_id)
            .cloned()
            .unwrap_or_default();
        let mut total_notes = 0usize;
        let mut notes_failed = false;
        let mut unique_facets = BTreeSet::new();
        for rel in attributed {
            unique_facets.insert(rel.facet_dir.clone());
            match solstone_core_entity::count_observations(
                journal_root,
                &rel.facet_dir,
                &rel.relationship_dir,
            ) {
                Ok(count) => total_notes += count,
                Err(_) => {
                    notes_failed = true;
                    break;
                }
            }
        }

        if notes_failed || total_notes > 0 {
            continue;
        }

        let last_active_day = last_active_day_by_folder.get(winner_dir).cloned().flatten();
        let entity_type = identity
            .value()
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();

        id_members.push(IdTidyUpRow {
            id: effective_id.clone(),
            name: name.to_owned(),
            entity_type,
            facets: unique_facets.into_iter().collect(),
            kept_notes_count: 0,
            last_active_day,
        });
    }

    id_members.sort_by(|left, right| {
        let left_rank = journal_bucket_rank(journal_type_bucket(&left.entity_type));
        let right_rank = journal_bucket_rank(journal_type_bucket(&right.entity_type));
        left_rank
            .cmp(&right_rank)
            .then_with(|| {
                let left_day_key = (
                    !left.last_active_day.is_some(),
                    std::cmp::Reverse(left.last_active_day.as_deref()),
                );
                let right_day_key = (
                    !right.last_active_day.is_some(),
                    std::cmp::Reverse(right.last_active_day.as_deref()),
                );
                left_day_key.cmp(&right_day_key)
            })
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });

    let damage_count = plan.damaged_paths.len();
    let has_damage = damage_count > 0;
    let id_member_ids: Vec<String> = id_members.iter().map(|row| row.id.clone()).collect();
    let id_count = id_members.len();
    let id_items: Vec<IdTidyUpRow> = id_members.into_iter().skip(offset).take(limit).collect();

    let id_shaped = if has_damage {
        IdGroupResponse {
            actionable: false,
            count: id_count,
            items: None,
            members: None,
        }
    } else {
        IdGroupResponse {
            actionable: true,
            count: id_count,
            items: Some(id_items),
            members: Some(id_member_ids),
        }
    };

    let damage = DamageReport {
        count: damage_count,
        paths: plan.damaged_paths,
    };

    let duplicates = match solstone_core_entity::load_merge_candidates(journal_root, None, None) {
        Err(err) => DuplicatesGroupResponse {
            error: Some(err.to_string()),
            count: None,
            items: None,
            members: None,
        },
        Ok(raw_rows) => {
            let mut open_rows = Vec::new();
            for row in raw_rows {
                if matches!(
                    solstone_core_entity::entity_merge_open_class(&row),
                    solstone_core_entity::EntityMergeOpenClass::Open
                ) {
                    let evidence = row.get("evidence").and_then(Value::as_object);
                    let detection_count = evidence.and_then(|e| e.get("detection_count"));
                    let strength = solstone_core_entity::json_i64(detection_count);
                    let composite = strength as f64;
                    let facet = row
                        .get("facet")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let source_slug = row
                        .get("source_slug")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let target_slug = row
                        .get("target_slug")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let key = solstone_core_entity::entity_merge_row_key(
                        &facet,
                        &source_slug,
                        &target_slug,
                    );
                    open_rows.push(RawDuplicateRow {
                        source_slug,
                        target_slug,
                        facet,
                        key,
                        composite,
                    });
                }
            }

            open_rows.sort_by(|left, right| {
                solstone_core_entity::cmp_composite_then_key(
                    left.composite,
                    &left.key,
                    right.composite,
                    &right.key,
                )
            });

            let mut pair_order = Vec::new();
            let mut pair_data: HashMap<(String, String), (Vec<String>, Vec<String>)> =
                HashMap::new();
            for row in open_rows {
                let pair = (row.source_slug, row.target_slug);
                let entry = pair_data.entry(pair.clone()).or_insert_with(|| {
                    pair_order.push(pair);
                    (Vec::new(), Vec::new())
                });
                if !entry.0.contains(&row.facet) {
                    entry.0.push(row.facet);
                }
                entry.1.push(row.key);
            }

            let is_slug_hidden = |slug: &str| -> bool {
                let Some(winner_dir) = identity_map.resolved.get(slug) else {
                    return false;
                };
                let Ok(Some(identity)) =
                    solstone_core_entity::read_entity_identity(journal_root, winner_dir)
                else {
                    return false;
                };
                solstone_core_entity::entity_is_hidden(identity.value())
            };

            pair_order.retain(|pair| !is_slug_hidden(&pair.0) && !is_slug_hidden(&pair.1));

            let dup_count = pair_order.len();
            let dup_members: Vec<DuplicateMember> = pair_order
                .iter()
                .map(|pair| {
                    let (_, keys) = &pair_data[pair];
                    DuplicateMember {
                        pair: [pair.0.clone(), pair.1.clone()],
                        keys: keys.clone(),
                    }
                })
                .collect();

            let dup_items: Vec<DuplicateItem> = pair_order
                .iter()
                .skip(offset)
                .take(limit)
                .map(|pair| {
                    let (facets, keys) = &pair_data[pair];
                    DuplicateItem {
                        pair: [pair.0.clone(), pair.1.clone()],
                        facets: facets.clone(),
                        keys: keys.clone(),
                    }
                })
                .collect();

            DuplicatesGroupResponse {
                error: None,
                count: Some(dup_count),
                items: Some(dup_items),
                members: Some(dup_members),
            }
        }
    };

    Ok(TidyUpResponse {
        offset,
        limit,
        damage,
        id_shaped,
        duplicates,
    })
}
