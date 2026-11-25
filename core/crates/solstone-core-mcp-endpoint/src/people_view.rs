// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deduplicated people range summaries and co-occurrence edges for MCP reads.
//!
//! Every range summary is computed strictly from activities and entities inside
//! the connection's granted facets and categories:
//! - primary presence (`days`, `activities`, `co_occurrence`) counts only
//!   `attendee` and `heard` roles across deduplicated cross-facet moments;
//! - real photo appearances (`visible` on an `import.image` segment carrying
//!   `image_transcript.md` and not `screenshot_transcript.md`) are counted
//!   separately in `in_photo_days` / `in_photo_activities` without inflating
//!   spoken/attended presence or co-occurrence;
//! - `mentioned` and screen-capture / non-photo `on_screen` roles are excluded;
//! - the journal's principal (owner), unnamed voices, blocked entities, and
//!   non-Person entities are excluded.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_indexer_query::AdmittedCategory;

use crate::dispatch::{DispatchPrincipal, entity_name, entity_reference, fold_literal};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::EntityReference;
use crate::tools::ToolError;
use crate::tools::activities::{
    ImageSegmentKind, is_real_photo_record, is_unnamed_voice, sort_activity_key,
};

/// Maximum people names returned per day in `get_week(include=["entities"])`.
pub(crate) const MAX_WEEK_DAY_ENTITIES: usize = 50;
/// Maximum people returned in the page-1 range roster on `get_week(include=["entities"])`.
pub(crate) const MAX_ROSTER_PEOPLE: usize = 200;
/// Maximum co-occurrence pairs returned on page 1 of `get_week(include=["entities"])`.
pub(crate) const MAX_ROSTER_CO_OCCURRENCE: usize = 500;

/// The journal owner and non-person / blocked entity exclusion filter, loaded
/// once per request purely to keep the owner, blocked entities, and non-Person
/// entities out of people lists and co-occurrence counts. Never serialized on wire.
#[derive(Debug, Default)]
pub(crate) struct OwnerIdentity {
    id: Option<String>,
    excluded_ids: BTreeSet<String>,
    excluded_names: BTreeSet<String>,
}

impl OwnerIdentity {
    pub(crate) fn load(journal_root: &Path) -> Self {
        let mut id = None;
        let mut excluded_ids = BTreeSet::new();
        let mut excluded_names = BTreeSet::new();
        if let Ok(Some(principal)) = solstone_core_entity::read_journal_principal(journal_root) {
            if let Some(pid) = principal
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                id = Some(pid.to_owned());
                excluded_ids.insert(pid.to_owned());
            }
            if let Some(name) = principal
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                excluded_names.insert(fold_literal(name));
            }
            if let Some(aka) = principal.get("aka").and_then(Value::as_array) {
                for alias in aka.iter().filter_map(Value::as_str) {
                    let trimmed = alias.trim();
                    if !trimmed.is_empty() {
                        excluded_names.insert(fold_literal(trimmed));
                    }
                }
            }
        }
        if let Ok(map) = solstone_core_entity::read_identity_map(journal_root) {
            let mut live_person_names = BTreeSet::new();
            let mut candidate_excluded_names = BTreeSet::new();
            for (eid, dir) in &map.resolved {
                let Ok(Some(snap)) = solstone_core_entity::read_entity_identity(journal_root, dir)
                else {
                    continue;
                };
                let val = snap.value();
                let blocked = val.get("blocked").and_then(Value::as_bool).unwrap_or(false);
                let is_principal = solstone_core_entity::identity_is_principal(val)
                    || id.as_deref() == Some(eid.as_str());
                let kind = val
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let is_person = kind.is_none_or(|k| k.eq_ignore_ascii_case("person"));
                let name = entity_name(val);
                if blocked || is_principal || !is_person {
                    excluded_ids.insert(eid.clone());
                    if !name.is_empty() {
                        candidate_excluded_names.insert(fold_literal(&name));
                    }
                } else if !name.is_empty() {
                    live_person_names.insert(fold_literal(&name));
                }
            }
            for folded in candidate_excluded_names {
                if !live_person_names.contains(&folded) {
                    excluded_names.insert(folded);
                }
            }
        }
        Self {
            id,
            excluded_ids,
            excluded_names,
        }
    }

    pub(crate) fn matches_id(&self, entity_id: &str) -> bool {
        self.id.as_deref() == Some(entity_id)
    }

    pub(crate) fn matches_name(&self, name: &str) -> bool {
        let trimmed = name.trim();
        !trimmed.is_empty() && self.excluded_names.contains(&fold_literal(trimmed))
    }

    pub(crate) fn excludes(&self, entity_id: Option<&str>, name: &str) -> bool {
        match entity_id {
            Some(id) => self.excluded_ids.contains(id),
            None => self.matches_name(name),
        }
    }
}

pub(crate) struct PeopleRangeSummary {
    pub(crate) people: Vec<Value>,
    pub(crate) people_truncated: bool,
    pub(crate) co_occurrence: Vec<Value>,
    pub(crate) co_occurrence_truncated: bool,
    pub(crate) targets: Vec<String>,
}

struct AllowedPerson {
    entity_id: String,
    entity_dir: String,
    name: String,
    /// Allowed `(facet_id, facet_dir)` pairs where this entity is attached,
    /// sorted by `facet_id`.
    attached_facets: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PersonKey {
    Entity(String),
    Unlinked(String),
}

impl PersonKey {
    fn wire_id(&self) -> &str {
        match self {
            Self::Entity(id) => id.as_str(),
            Self::Unlinked(key) => key.as_str(),
        }
    }
}

#[derive(Default)]
struct PersonStats {
    display_name: String,
    present_days: BTreeSet<String>,
    activities: usize,
    corresponded_days: BTreeSet<String>,
    corresponded_activities: usize,
    in_photo_days: BTreeSet<String>,
    in_photo_activities: usize,
    facet_counts: BTreeMap<String, usize>,
}

/// Build the deduplicated people range summary and who-with-whom co-occurrence
/// edges across `[day_from, day_to]` strictly from this connection's allowed
/// facets and entities.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_people_range_summary(
    journal_root: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    facets: &[(String, String)],
    day_from: &str,
    day_to: &str,
    owner: &OwnerIdentity,
    photo_cache: &mut BTreeMap<(String, String), ImageSegmentKind>,
) -> Result<PeopleRangeSummary, ToolError> {
    if !snapshot.categories.contains(&AdmittedCategory::Entities)
        || !snapshot.categories.contains(&AdmittedCategory::Facets)
        || day_from > day_to
    {
        return Ok(PeopleRangeSummary {
            people: Vec::new(),
            people_truncated: false,
            co_occurrence: Vec::new(),
            co_occurrence_truncated: false,
            targets: Vec::new(),
        });
    }

    let identities = solstone_core_facets::read_identity_map(journal_root)
        .map_err(|_| ToolError::FileUnreadable)?;

    let mut allowed_people: BTreeMap<String, AllowedPerson> = BTreeMap::new();
    let mut excluded_names: BTreeSet<String> = BTreeSet::new();

    let mut sorted_facets: Vec<(String, String)> = facets.to_vec();
    sorted_facets.sort_by(|a, b| a.0.cmp(&b.0));

    for (facet_id, facet_dir) in &sorted_facets {
        let mut scoped = solstone_core_facets::list_scoped_facet_entities_with_resolved_ids(
            journal_root,
            facet_dir,
            false,
            true,
            &identities.resolved,
        )
        .map_err(|_| ToolError::FileUnreadable)?;
        scoped.sort_by(|a, b| {
            a.entity_id
                .cmp(&b.entity_id)
                .then_with(|| {
                    (a.relationship_dir != a.entity_id).cmp(&(b.relationship_dir != b.entity_id))
                })
                .then_with(|| a.relationship_dir.cmp(&b.relationship_dir))
        });
        scoped.dedup_by(|a, b| a.entity_id == b.entity_id);

        for item in scoped {
            let name = entity_name(&item.identity);
            let kind = item
                .identity
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let is_person = kind.is_none_or(|k| k.eq_ignore_ascii_case("person"));
            if item.blocked
                || !is_person
                || solstone_core_entity::identity_is_principal(&item.identity)
                || owner.matches_id(&item.entity_id)
            {
                if !name.is_empty() {
                    excluded_names.insert(fold_literal(&name));
                }
                continue;
            }
            let entry = allowed_people
                .entry(item.entity_id.clone())
                .or_insert_with(|| AllowedPerson {
                    entity_id: item.entity_id.clone(),
                    entity_dir: item.entity_dir.clone(),
                    name,
                    attached_facets: Vec::new(),
                });
            if !entry.attached_facets.iter().any(|(fid, _)| fid == facet_id) {
                entry
                    .attached_facets
                    .push((facet_id.clone(), facet_dir.clone()));
            }
        }
    }

    // Unique display-name -> entity_id lookup across allowed people so records
    // omitting `entity_id` still join onto the entity when unambiguous.
    let mut name_lookup: BTreeMap<String, Option<String>> = BTreeMap::new();
    for person in allowed_people.values() {
        let folded = fold_literal(&person.name);
        name_lookup
            .entry(folded)
            .and_modify(|slot| *slot = None)
            .or_insert_with(|| Some(person.entity_id.clone()));
    }

    struct ScannedActivity {
        day: String,
        sort_key: (String, String, i64, String),
        facet_id: String,
        record: Map<String, Value>,
    }

    let mut by_day: BTreeMap<String, Vec<ScannedActivity>> = BTreeMap::new();
    for (facet_id, facet_dir) in &sorted_facets {
        let stems = solstone_core_facets::list_activity_day_stems(journal_root, facet_dir)
            .map_err(|_| ToolError::FileUnreadable)?;
        for day in stems {
            if day.as_str() < day_from || day.as_str() > day_to {
                continue;
            }
            let rel_path = format!("facets/{facet_dir}/activities/{day}.jsonl");
            let day_file_bytes =
                crate::activity_admit::read_contained_source(journal_root, &rel_path)
                    .unwrap_or_default();
            if day_file_bytes.is_empty() {
                let rows = solstone_core_facets::read_activity_records_strict(
                    journal_root,
                    facet_dir,
                    &day,
                )
                .map_err(|_| ToolError::FileUnreadable)?;
                if !rows.is_empty() {
                    return Err(ToolError::FileUnreadable);
                }
                continue;
            }
            let rows =
                solstone_core_facets::read_activity_records_strict(journal_root, facet_dir, &day)
                    .map_err(|_| ToolError::FileUnreadable)?;
            let judged =
                crate::may_see::ActivityDay::new(journal_root, snapshot, facet_dir, &day, &rows);
            for row in rows {
                if judged.shows(&row) != crate::may_see::Shown::Yes
                    || solstone_core_facets::is_body_record(&row)
                {
                    continue;
                }
                let sort_key = sort_activity_key(&day, facet_id, &row);
                by_day
                    .entry(day.clone())
                    .or_default()
                    .push(ScannedActivity {
                        day: day.clone(),
                        sort_key,
                        facet_id: facet_id.clone(),
                        record: row,
                    });
            }
        }
    }

    let mut stats: BTreeMap<PersonKey, PersonStats> = BTreeMap::new();
    let mut co_acts: BTreeMap<(PersonKey, PersonKey), usize> = BTreeMap::new();
    let mut co_days: BTreeMap<(PersonKey, PersonKey), BTreeSet<String>> = BTreeMap::new();

    for (day, mut day_items) in by_day {
        day_items.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        let merged = solstone_core_facets::merge_moment_copies(
            day_items,
            |item| item.day.as_str(),
            |item| &item.record,
        );
        for moment in merged {
            let mut best_rank: BTreeMap<PersonKey, u8> = BTreeMap::new();
            let mut moment_facets: BTreeMap<PersonKey, BTreeSet<String>> = BTreeMap::new();
            let mut display_names: BTreeMap<PersonKey, String> = BTreeMap::new();

            for copy in std::iter::once(&moment.primary).chain(moment.copies.iter()) {
                let Some(entries) = copy.record.get("participation").and_then(Value::as_array)
                else {
                    continue;
                };
                let photo_rec = is_real_photo_record(journal_root, &day, &copy.record, photo_cache);
                for entry in entries.iter().filter_map(Value::as_object) {
                    let Some(name) = entry.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    let trimmed = name.trim();
                    if trimmed.is_empty() || is_unnamed_voice(trimmed) {
                        continue;
                    }
                    let explicit_id = entry
                        .get("entity_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    if owner.excludes(explicit_id, trimmed) {
                        continue;
                    }
                    let (key, resolved_name) = if let Some(eid) = explicit_id {
                        let Some(allowed) = allowed_people.get(eid) else {
                            // Explicit entity_id not in this connection's allowed
                            // unblocked Person entities (owner, blocked, non-Person,
                            // detached, or outside allowed facets).
                            continue;
                        };
                        (PersonKey::Entity(eid.to_owned()), allowed.name.clone())
                    } else {
                        let folded = fold_literal(trimmed);
                        if excluded_names.contains(&folded) {
                            continue;
                        }
                        if let Some(Some(eid)) = name_lookup.get(&folded)
                            && let Some(allowed) = allowed_people.get(eid)
                        {
                            (PersonKey::Entity(eid.clone()), allowed.name.clone())
                        } else {
                            (
                                PersonKey::Unlinked(format!("name:{folded}")),
                                trimmed.to_owned(),
                            )
                        }
                    };
                    let role = entry
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let rank = match role {
                        "attendee" => 5,
                        "heard" => 4,
                        "corresponded" => 3,
                        "visible" if photo_rec => 2,
                        "visible" => 0,
                        _ => 1,
                    };
                    best_rank
                        .entry(key.clone())
                        .and_modify(|r| *r = (*r).max(rank))
                        .or_insert(rank);
                    if rank >= 2 {
                        moment_facets
                            .entry(key.clone())
                            .or_default()
                            .insert(copy.facet_id.clone());
                        display_names.entry(key).or_insert(resolved_name);
                    }
                }
            }

            let mut present_keys: Vec<PersonKey> = Vec::new();
            for (key, rank) in best_rank {
                if rank < 2 {
                    continue;
                }
                let entry = stats.entry(key.clone()).or_default();
                if entry.display_name.is_empty()
                    && let Some(name) = display_names.get(&key)
                {
                    entry.display_name = name.clone();
                }
                if let Some(fids) = moment_facets.get(&key) {
                    for fid in fids {
                        *entry.facet_counts.entry(fid.clone()).or_default() += 1;
                    }
                }
                if rank >= 4 {
                    entry.present_days.insert(day.clone());
                    entry.activities += 1;
                    present_keys.push(key);
                } else if rank == 3 {
                    entry.corresponded_days.insert(day.clone());
                    entry.corresponded_activities += 1;
                } else {
                    entry.in_photo_days.insert(day.clone());
                    entry.in_photo_activities += 1;
                }
            }

            present_keys.sort();
            for i in 0..present_keys.len() {
                for j in (i + 1)..present_keys.len() {
                    let pair = (present_keys[i].clone(), present_keys[j].clone());
                    *co_acts.entry(pair.clone()).or_default() += 1;
                    co_days.entry(pair).or_default().insert(day.clone());
                }
            }
        }
    }

    let photos_allowed = snapshot.categories.contains(&AdmittedCategory::ProfilePics)
        && solstone_core_entities::photos::enabled(journal_root)
            .map_err(|_| ToolError::FileUnreadable)?;

    let mut ranked: Vec<(PersonKey, PersonStats)> = stats.into_iter().collect();
    ranked.sort_by(|(a_key, a), (b_key, b)| {
        Reverse(a.present_days.len())
            .cmp(&Reverse(b.present_days.len()))
            .then_with(|| Reverse(a.activities).cmp(&Reverse(b.activities)))
            .then_with(|| {
                Reverse(a.corresponded_days.len()).cmp(&Reverse(b.corresponded_days.len()))
            })
            .then_with(|| {
                Reverse(a.corresponded_activities).cmp(&Reverse(b.corresponded_activities))
            })
            .then_with(|| Reverse(a.in_photo_days.len()).cmp(&Reverse(b.in_photo_days.len())))
            .then_with(|| Reverse(a.in_photo_activities).cmp(&Reverse(b.in_photo_activities)))
            .then_with(|| {
                a.display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase())
            })
            .then_with(|| a_key.cmp(b_key))
    });

    let people_truncated = ranked.len() > MAX_ROSTER_PEOPLE;
    let retained: Vec<(PersonKey, PersonStats)> =
        ranked.into_iter().take(MAX_ROSTER_PEOPLE).collect();
    let retained_keys: BTreeSet<PersonKey> = retained.iter().map(|(key, _)| key.clone()).collect();

    let mut people_out = Vec::with_capacity(retained.len());
    let mut targets = Vec::new();
    for (key, st) in retained {
        let mut active_facets: Vec<(String, usize)> = st.facet_counts.into_iter().collect();
        active_facets.sort_by(|a, b| Reverse(a.1).cmp(&Reverse(b.1)).then_with(|| a.0.cmp(&b.0)));

        let mut row = Map::new();
        row.insert("id".to_owned(), json!(key.wire_id()));
        row.insert("name".to_owned(), json!(st.display_name));

        let mut has_photo = false;
        let mut reference_value: Option<String> = None;
        let mut link_entity_id: Option<String> = None;

        if let PersonKey::Entity(ref eid) = key
            && let Some(allowed) = allowed_people.get(eid)
        {
            let mut photo_facets = BTreeSet::new();
            if photos_allowed {
                for (fid, fdir) in &allowed.attached_facets {
                    if solstone_core_entities::photos::has_photo_in_facet(
                        journal_root,
                        &allowed.entity_dir,
                        fdir,
                    )
                    .map_err(|_| ToolError::FileUnreadable)?
                    {
                        photo_facets.insert(fid.clone());
                    }
                }
            }
            has_photo = !photo_facets.is_empty();
            let count_for = |fid: &str| {
                active_facets
                    .iter()
                    .find(|(id, _)| id == fid)
                    .map_or(0, |(_, c)| *c)
            };
            if let Some((primary_fid, primary_fdir)) =
                allowed
                    .attached_facets
                    .iter()
                    .max_by(|(a_fid, _), (b_fid, _)| {
                        photo_facets
                            .contains(a_fid)
                            .cmp(&photo_facets.contains(b_fid))
                            .then_with(|| count_for(a_fid).cmp(&count_for(b_fid)))
                            .then_with(|| b_fid.cmp(a_fid))
                    })
            {
                let reference = entity_reference(
                    principal,
                    snapshot,
                    Some(primary_fdir.as_str()),
                    EntityReference {
                        facet_id: primary_fid.clone(),
                        entity_id: eid.clone(),
                    },
                )
                .map_err(|_| ToolError::ReferenceNotFound)?;
                reference_value = Some(reference);
                let mut audited_facets = BTreeSet::new();
                audited_facets.insert(primary_fid.clone());
                for (fid, _) in &active_facets {
                    audited_facets.insert(fid.clone());
                }
                for fid in audited_facets {
                    targets.push(format!("facet:{fid}/entity:{eid}"));
                }
            }
            link_entity_id = Some(eid.clone());
        }

        if let Some(reference) = reference_value {
            row.insert("reference".to_owned(), json!(reference));
        }
        let facet_ids: Vec<String> = active_facets.into_iter().map(|(fid, _)| fid).collect();
        row.insert("facets".to_owned(), json!(facet_ids));
        row.insert("has_photo".to_owned(), json!(has_photo));
        row.insert("days".to_owned(), json!(st.present_days.len()));
        row.insert("activities".to_owned(), json!(st.activities));
        if st.corresponded_activities > 0 {
            row.insert(
                "corresponded_days".to_owned(),
                json!(st.corresponded_days.len()),
            );
            row.insert(
                "corresponded_activities".to_owned(),
                json!(st.corresponded_activities),
            );
        }
        if st.in_photo_activities > 0 {
            row.insert("in_photo_days".to_owned(), json!(st.in_photo_days.len()));
            row.insert(
                "in_photo_activities".to_owned(),
                json!(st.in_photo_activities),
            );
        }
        let mut active_days = BTreeSet::new();
        active_days.extend(st.present_days.iter().cloned());
        active_days.extend(st.corresponded_days.iter().cloned());
        let span_days = if active_days.is_empty() {
            &st.in_photo_days
        } else {
            &active_days
        };
        if let Some(first) = span_days.first() {
            row.insert("first_day".to_owned(), json!(first));
        }
        if let Some(last) = span_days.last() {
            row.insert("last_day".to_owned(), json!(last));
        }
        let mut value = Value::Object(row);
        if let Some(eid) = link_entity_id {
            crate::journal_links::attach(
                journal_root,
                &mut value,
                solstone_core_journal_link::Place::Person { id: eid },
            );
        }
        people_out.push(value);
    }

    let mut edges: Vec<(PersonKey, PersonKey, usize, usize)> = co_acts
        .into_iter()
        .filter(|((a, b), _)| retained_keys.contains(a) && retained_keys.contains(b))
        .map(|((a, b), acts)| {
            let days = co_days
                .get(&(a.clone(), b.clone()))
                .map_or(0, BTreeSet::len);
            (a, b, acts, days)
        })
        .collect();
    edges.sort_by(|a, b| {
        Reverse(a.2)
            .cmp(&Reverse(b.2))
            .then_with(|| Reverse(a.3).cmp(&Reverse(b.3)))
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });

    let co_occurrence_truncated = edges.len() > MAX_ROSTER_CO_OCCURRENCE;
    let co_occurrence = edges
        .into_iter()
        .take(MAX_ROSTER_CO_OCCURRENCE)
        .map(|(a, b, acts, days)| {
            json!({
                "a": a.wire_id(),
                "b": b.wire_id(),
                "activities": acts,
                "days": days,
            })
        })
        .collect();

    Ok(PeopleRangeSummary {
        people: people_out,
        people_truncated,
        co_occurrence,
        co_occurrence_truncated,
        targets,
    })
}
