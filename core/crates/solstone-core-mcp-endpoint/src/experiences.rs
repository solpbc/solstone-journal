// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! MCP read projection and strict `AdmittedCategory` enforcement for Experiences
//! served through existing `get_week`, `search`, and `fetch` tools.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_entity::hidden_participation_entity_ids;
use solstone_core_facets::{
    ExperienceHydrationContext, ExperienceRecord, HydratedExperience, get_experience,
    hydrate_experience_with, list_experiences_in_range,
};
use solstone_core_indexer_query::AdmittedCategory;
use solstone_core_journal_io::is_day_key;

use crate::dispatch::{DispatchPrincipal, available_facets, codec};
use crate::permissions::ConnectionReadSnapshot;
use crate::references::{ExperienceReference, ReferenceTarget};

/// Parse `facets/<facet>/experiences/<YYYYMMDD>.jsonl` into `(facet_dir, anchor_day)`.
pub(crate) fn parse_experience_path(path: &str) -> Option<(String, String)> {
    let trimmed = path.strip_prefix("facets/")?;
    let mut parts = trimmed.split('/');
    let facet = parts.next()?;
    if facet.is_empty() || parts.next() != Some("experiences") {
        return None;
    }
    let file = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let day = file.strip_suffix(".jsonl")?;
    if !is_day_key(day) {
        return None;
    }
    Some((facet.to_owned(), day.to_owned()))
}

/// Check whether one `media_ref` (`YYYYMMDD/stream/HHMMSS_LEN`) is permitted by
/// the connection's `Photos` / `Screenshots` category grants.
pub(crate) fn media_ref_allowed(media_ref: &str, snapshot: &ConnectionReadSnapshot) -> bool {
    let stream = media_ref.split('/').nth(1).unwrap_or("").to_lowercase();
    if stream.contains("screenshot") {
        snapshot.categories.contains(&AdmittedCategory::Screenshots)
    } else {
        snapshot.categories.contains(&AdmittedCategory::Photos)
    }
}

/// Live-verify an indexed Experience hit at `(path, idx)` against the connection's
/// `Facets` grant, admitted facet scope, and live `hidden: false` state.
pub(crate) fn live_experience_for_indexed_hit(
    journal: &Path,
    snapshot: &ConnectionReadSnapshot,
    path: &str,
    idx: i64,
) -> Option<(ExperienceRecord, String, String, String)> {
    if !snapshot.categories.contains(&AdmittedCategory::Facets) {
        return None;
    }
    let (facet_dir, anchor_day) = parse_experience_path(path)?;
    let facets = available_facets(journal, snapshot).ok()?;
    let (facet_id, _, decl) = facets.into_iter().find(|(_, dir, _)| dir == &facet_dir)?;
    let facet_title = decl
        .as_ref()
        .filter(|d| !d.title.is_empty())
        .map(|d| d.title.clone())
        .unwrap_or_else(|| facet_dir.clone());

    let raw_bytes = crate::activity_admit::read_contained_source(journal, path)?;
    let text = String::from_utf8(raw_bytes).ok()?;
    let target_idx = usize::try_from(idx).ok()?;
    let line = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .nth(target_idx)?;
    let record: ExperienceRecord = serde_json::from_str(line).ok()?;
    if record.hidden || record.anchor_day != anchor_day {
        return None;
    }
    Some((record, facet_id, facet_dir, facet_title))
}

/// Searchable narrative text of an Experience excluding `locations[]`, used to
/// ensure search matches never probe location strings when `Places` is withheld.
pub(crate) fn non_location_search_text(record: &ExperienceRecord) -> String {
    let mut parts = Vec::new();
    if !record.title.trim().is_empty() {
        parts.push(record.title.trim());
    }
    if !record.summary.trim().is_empty() {
        parts.push(record.summary.trim());
    }
    for m in &record.moments {
        if !m.summary.trim().is_empty() {
            parts.push(m.summary.trim());
        }
    }
    parts.join(" ")
}

/// Admitted facet directory set for the current read snapshot.
pub(crate) fn admitted_facet_dirs(
    journal: &Path,
    snapshot: &ConnectionReadSnapshot,
) -> BTreeSet<String> {
    available_facets(journal, snapshot)
        .map(|facets| facets.into_iter().map(|(_, dir, _)| dir).collect())
        .unwrap_or_default()
}

/// Live-verify an opaque `ExperienceReference` against the connection's `Facets`
/// grant, admitted facet scope, and live `hidden: false` state.
pub(crate) fn live_experience_for_ref(
    journal: &Path,
    snapshot: &ConnectionReadSnapshot,
    exp_ref: &ExperienceReference,
) -> Option<(ExperienceRecord, String, BTreeSet<String>)> {
    if !snapshot.categories.contains(&AdmittedCategory::Facets) {
        return None;
    }
    let facets = available_facets(journal, snapshot).ok()?;
    let admitted_dirs: BTreeSet<String> = facets.iter().map(|(_, dir, _)| dir.clone()).collect();
    let (_, matched_dir, decl) = facets
        .iter()
        .find(|(id, dir, _)| id == &exp_ref.facet_id || dir == &exp_ref.facet_dir)?;
    let facet_title = decl
        .as_ref()
        .filter(|d| !d.title.is_empty())
        .map(|d| d.title.clone())
        .unwrap_or_else(|| matched_dir.clone());
    let record = get_experience(
        journal,
        matched_dir,
        &exp_ref.anchor_day,
        &exp_ref.experience_id,
    )
    .ok()??;
    if record.hidden {
        return None;
    }
    Some((record, facet_title, admitted_dirs))
}

/// Filter a hydrated Experience payload strictly by every `AdmittedCategory`
/// boundary (`Facets` + admitted facet scope, `Entities` +
/// `hidden_participation_entity_ids`, `Places` + `places_precision`,
/// `Photos` / `Screenshots`).
pub(crate) fn filter_hydrated_experience(
    journal: &Path,
    snapshot: &ConnectionReadSnapshot,
    admitted_facet_dirs: &BTreeSet<String>,
    hydrated: &HydratedExperience,
    facet_title: &str,
    reference: Option<String>,
) -> Value {
    let hidden_entities = hidden_participation_entity_ids(journal);
    filter_hydrated_experience_with(
        snapshot,
        admitted_facet_dirs,
        hydrated,
        facet_title,
        reference,
        &hidden_entities,
    )
}

/// [`filter_hydrated_experience`] against `hidden_participation_entity_ids`
/// already read, so a list of Experiences reads the journal's entities once.
fn filter_hydrated_experience_with(
    snapshot: &ConnectionReadSnapshot,
    admitted_facet_dirs: &BTreeSet<String>,
    hydrated: &HydratedExperience,
    facet_title: &str,
    reference: Option<String>,
    hidden_entities: &BTreeSet<String>,
) -> Value {
    let exp = &hydrated.experience;
    let allow_entities = snapshot.categories.contains(&AdmittedCategory::Entities);
    let places_precision = snapshot
        .categories
        .contains(&AdmittedCategory::Places)
        .then(|| snapshot.places_precision())
        .flatten();

    // Filter constituent activities so cross-facet owner additions from non-admitted
    // facets never leak through MCP.
    let admitted_activities: Vec<Value> = hydrated
        .hydrated
        .activities
        .iter()
        .filter(|act| {
            let eff = act.ref_item.effective_facet(&exp.primary_facet);
            admitted_facet_dirs.contains(eff)
        })
        .map(|act| {
            json!({
                "ref": act.ref_item,
                "facet": act.facet,
                "activity": act.activity,
                "title": act.title,
                "started_at": act.started_at,
                "ended_at": act.ended_at,
                "duration_min": act.duration_min,
                "seg_count": act.seg_count,
                "description": act.description,
                "has_story": act.has_story,
            })
        })
        .collect();

    let admitted_act_keys: BTreeSet<(String, String)> = hydrated
        .hydrated
        .activities
        .iter()
        .filter(|act| {
            let eff = act.ref_item.effective_facet(&exp.primary_facet);
            admitted_facet_dirs.contains(eff)
        })
        .map(|act| (act.ref_item.day.clone(), act.ref_item.stream.clone()))
        .collect();

    let moments: Vec<Value> = exp
        .moments
        .iter()
        .map(|m| {
            let filtered_arefs: Vec<_> = m
                .activity_refs
                .iter()
                .filter(|ar| admitted_facet_dirs.contains(ar.effective_facet(&exp.primary_facet)))
                .cloned()
                .collect();
            let filtered_srefs: Vec<String> = m
                .segment_refs
                .iter()
                .filter(|sr| {
                    let mut parts = sr.split('/');
                    let Some(d) = parts.next() else {
                        return false;
                    };
                    let Some(s) = parts.next() else {
                        return false;
                    };
                    admitted_act_keys.contains(&(d.to_owned(), s.to_owned()))
                })
                .cloned()
                .collect();
            let filtered_mrefs: Vec<String> = m
                .media_refs
                .iter()
                .filter(|mr| media_ref_allowed(mr, snapshot))
                .cloned()
                .collect();
            let mut obj = Map::new();
            obj.insert("id".to_owned(), json!(m.id));
            obj.insert("started_at".to_owned(), json!(m.started_at));
            obj.insert("ended_at".to_owned(), json!(m.ended_at));
            obj.insert("summary".to_owned(), json!(m.summary));
            obj.insert("activity_refs".to_owned(), json!(filtered_arefs));
            obj.insert("segment_refs".to_owned(), json!(filtered_srefs));
            if !filtered_mrefs.is_empty() {
                obj.insert("media_refs".to_owned(), json!(filtered_mrefs));
            }
            Value::Object(obj)
        })
        .collect();

    let mut out = Map::new();
    out.insert("id".to_owned(), json!(exp.id));
    out.insert("title".to_owned(), json!(exp.title));
    out.insert("summary".to_owned(), json!(exp.summary));
    out.insert("facet".to_owned(), json!(facet_title));
    out.insert("primary_facet".to_owned(), json!(exp.primary_facet));
    out.insert("anchor_day".to_owned(), json!(exp.anchor_day));
    out.insert("started_at".to_owned(), json!(exp.started_at));
    out.insert("ended_at".to_owned(), json!(exp.ended_at));
    out.insert("membership_source".to_owned(), json!(exp.membership_source));
    out.insert(
        "content_kind".to_owned(),
        json!(AdmittedCategory::Facets.as_str()),
    );
    out.insert("moments".to_owned(), Value::Array(moments));
    out.insert("activities".to_owned(), Value::Array(admitted_activities));

    if allow_entities {
        let people: Vec<Value> = hydrated
            .hydrated
            .people
            .iter()
            .filter(|p| !hidden_entities.contains(&p.entity_id))
            .map(|p| {
                json!({
                    "entity_id": p.entity_id,
                    "name": p.name,
                    "type": p.entity_type,
                })
            })
            .collect();
        if !people.is_empty() {
            out.insert("people".to_owned(), Value::Array(people));
        }
    }

    if let Some(precision) = places_precision
        && !exp.locations.is_empty()
    {
        out.insert("locations".to_owned(), json!(exp.locations));
        out.insert("places_precision".to_owned(), json!(precision.wire_value()));
    }

    let allowed_media: Vec<Value> = hydrated
        .hydrated
        .media
        .iter()
        .filter(|m| media_ref_allowed(&m.media_ref, snapshot))
        .map(|m| json!(m))
        .collect();
    if !allowed_media.is_empty() {
        out.insert("media".to_owned(), Value::Array(allowed_media));
    }

    if let Some(reference) = reference {
        out.insert("reference".to_owned(), json!(reference));
    }

    Value::Object(out)
}

/// Collect hydrated, category-filtered non-hidden Experiences for a week or day range.
pub(crate) fn collect_range_experiences(
    journal: &Path,
    principal: DispatchPrincipal<'_>,
    snapshot: &ConnectionReadSnapshot,
    facets: &[(String, String, String)],
    start_day: &str,
    end_day: &str,
) -> Vec<Value> {
    if !snapshot.categories.contains(&AdmittedCategory::Facets) {
        return Vec::new();
    }
    let admitted_dirs: BTreeSet<String> = facets.iter().map(|(_, dir, _)| dir.clone()).collect();
    let mut rows: Vec<(ExperienceRecord, String, String, String)> = Vec::new();
    for (facet_id, facet_dir, facet_title) in facets {
        let Ok(list) = list_experiences_in_range(journal, facet_dir, start_day, end_day, false)
        else {
            continue;
        };
        for rec in list {
            if !rec.hidden {
                rows.push((
                    rec,
                    facet_id.clone(),
                    facet_dir.clone(),
                    facet_title.clone(),
                ));
            }
        }
    }
    rows.sort_by(|a, b| {
        a.0.started_at
            .cmp(&b.0.started_at)
            .then_with(|| a.0.id.cmp(&b.0.id))
    });

    if rows.is_empty() {
        return Vec::new();
    }
    // Read the journal's entities once for the whole list, not per Experience.
    let context = ExperienceHydrationContext::load(journal);
    let mut out = Vec::with_capacity(rows.len());
    for (rec, facet_id, facet_dir, facet_title) in rows {
        let reference = codec()
            .mint(
                principal.connection,
                snapshot.generation,
                ReferenceTarget::Experience(ExperienceReference {
                    facet_id,
                    facet_dir,
                    anchor_day: rec.anchor_day.clone(),
                    experience_id: rec.id.clone(),
                }),
            )
            .ok();
        let hydrated = hydrate_experience_with(journal, rec, None, &context);
        out.push(filter_hydrated_experience_with(
            snapshot,
            &admitted_dirs,
            &hydrated,
            &facet_title,
            reference,
            context.hidden_ids(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_facets::{
        ActivityRef, ComposedExperienceUpdate, ExperienceMoment, append_activity_record,
        apply_composed_experience, create_facet, create_owner_experience, set_experience_hidden,
    };
    use solstone_core_format::content::kinds::PlacesPrecision;
    use solstone_core_indexer_query::ConnectionScope;
    use tempfile::TempDir;

    fn snapshot_with(
        categories: &[AdmittedCategory],
        places_precision: Option<PlacesPrecision>,
        scope: ConnectionScope,
    ) -> ConnectionReadSnapshot {
        ConnectionReadSnapshot {
            generation: 1,
            categories: categories.iter().copied().collect(),
            scope,
            places_precision,
        }
    }

    fn seed_person(root: &Path, id: &str, name: &str, hidden: bool) {
        let dir = root.join("entities").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut obj = Map::new();
        obj.insert("id".to_owned(), json!(id));
        obj.insert("name".to_owned(), json!(name));
        obj.insert("type".to_owned(), json!("Person"));
        if hidden {
            obj.insert("blocked".to_owned(), json!(true));
        }
        std::fs::write(dir.join("entity.json"), Value::Object(obj).to_string()).unwrap();
    }

    #[test]
    fn admitted_category_and_hidden_boundaries_are_enforced_strictly() {
        let dir = TempDir::new().unwrap();
        create_facet(
            dir.path(),
            "personal",
            "Personal",
            "",
            "#6366f1",
            "✨",
            None,
        )
        .unwrap();
        create_facet(dir.path(), "work", "Work", "", "#0ea5e9", "💼", None).unwrap();
        seed_person(dir.path(), "maya", "Maya Lin", false);
        seed_person(dir.path(), "secret_person", "Hidden Guest", true);

        let mut act1 = Map::new();
        act1.insert("id".to_owned(), json!("social_180000_600"));
        act1.insert("stream".to_owned(), json!("watch"));
        act1.insert("activity".to_owned(), json!("social"));
        act1.insert("description".to_owned(), json!("Dinner with Maya"));
        act1.insert("segments".to_owned(), json!(["180000_600"]));
        let _ = append_activity_record(dir.path(), "personal", "20260304", act1).unwrap();

        let mut act_work = Map::new();
        act_work.insert("id".to_owned(), json!("meeting_183000_300"));
        act_work.insert("stream".to_owned(), json!("watch"));
        act_work.insert("activity".to_owned(), json!("meeting"));
        act_work.insert("description".to_owned(), json!("Work call"));
        act_work.insert("segments".to_owned(), json!(["183000_300"]));
        let _ = append_activity_record(dir.path(), "work", "20260304", act_work).unwrap();

        let created = create_owner_experience(
            dir.path(),
            "personal",
            vec![
                ActivityRef {
                    day: "20260304".to_owned(),
                    stream: "watch".to_owned(),
                    id: "social_180000_600".to_owned(),
                    facet: None,
                },
                ActivityRef {
                    day: "20260304".to_owned(),
                    stream: "watch".to_owned(),
                    id: "meeting_183000_300".to_owned(),
                    facet: Some("work".to_owned()),
                },
            ],
            Some("Dinner".to_owned()),
            Some("Alex met Maya.".to_owned()),
            "owner",
            "2026-03-04T20:00:00Z",
        )
        .unwrap();

        let composed = apply_composed_experience(
            dir.path(),
            "personal",
            "20260304",
            &created.id,
            ComposedExperienceUpdate {
                title: "Dinner at Riverside".to_owned(),
                summary: "Alex had dinner with Maya at Riverside Cafe.".to_owned(),
                locations: vec!["Riverside Cafe".to_owned()],
                entity_refs: vec!["maya".to_owned(), "secret_person".to_owned()],
                moments: vec![ExperienceMoment {
                    id: "m1".to_owned(),
                    started_at: "2026-03-04 18:00".to_owned(),
                    ended_at: "2026-03-04 18:10".to_owned(),
                    summary: "Alex arrived and shared photos.".to_owned(),
                    activity_refs: vec![
                        ActivityRef {
                            day: "20260304".to_owned(),
                            stream: "watch".to_owned(),
                            id: "social_180000_600".to_owned(),
                            facet: None,
                        },
                        ActivityRef {
                            day: "20260304".to_owned(),
                            stream: "watch".to_owned(),
                            id: "meeting_183000_300".to_owned(),
                            facet: Some("work".to_owned()),
                        },
                    ],
                    segment_refs: vec!["20260304/watch/180000_600".to_owned()],
                    media_refs: vec![
                        "20260304/import.image/180500_0".to_owned(),
                        "20260304/ios_screenshots/180600_0".to_owned(),
                    ],
                }],
                started_at: None,
                ended_at: None,
            },
            "2026-03-04T20:05:00Z",
        )
        .unwrap();

        let principal = DispatchPrincipal {
            connection: "bearer:test",
            agent_identity: "test-agent",
        };
        let personal_id =
            solstone_core_facets::observe_facet_write_identity(dir.path(), "personal").unwrap();
        let facets_only = vec![(
            personal_id.clone(),
            "personal".to_owned(),
            "Personal".to_owned(),
        )];

        // 1. Without AdmittedCategory::Facets -> zero experiences returned.
        let no_facets = snapshot_with(
            &[AdmittedCategory::Entities],
            None,
            ConnectionScope::WholeJournal,
        );
        assert!(
            collect_range_experiences(
                dir.path(),
                principal,
                &no_facets,
                &facets_only,
                "20260301",
                "20260307"
            )
            .is_empty()
        );

        // 2. With Facets only (no Entities, no Places, no Photos, no Screenshots),
        //    and only `personal` admitted (not `work`).
        let facets_snap = snapshot_with(
            &[AdmittedCategory::Facets],
            None,
            ConnectionScope::WholeJournal,
        );
        let list = collect_range_experiences(
            dir.path(),
            principal,
            &facets_snap,
            &facets_only,
            "20260301",
            "20260307",
        );
        assert_eq!(list.len(), 1);
        let row = &list[0];
        assert!(
            row.get("people").is_none(),
            "Entities withheld must omit people"
        );
        assert!(
            row.get("locations").is_none(),
            "Places withheld must omit locations"
        );
        assert!(
            row["moments"][0].get("media_refs").is_none(),
            "Photos/Screenshots withheld must omit media_refs"
        );
        // Cross-facet `work` activity must be stripped when only `personal` is admitted!
        assert_eq!(row["activities"].as_array().unwrap().len(), 1);
        assert_eq!(row["activities"][0]["ref"]["id"], "social_180000_600");

        // 3. With Entities + Places (NamesOnly) + Photos (NO Screenshots):
        let rich_snap = snapshot_with(
            &[
                AdmittedCategory::Facets,
                AdmittedCategory::Entities,
                AdmittedCategory::Places,
                AdmittedCategory::Photos,
            ],
            Some(PlacesPrecision::NamesOnly),
            ConnectionScope::WholeJournal,
        );
        let rich_list = collect_range_experiences(
            dir.path(),
            principal,
            &rich_snap,
            &facets_only,
            "20260301",
            "20260307",
        );
        let rich = &rich_list[0];
        // Only non-hidden Maya appears; hidden `secret_person` is excluded!
        let people = rich["people"].as_array().unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0]["entity_id"], "maya");
        // Locations appear with places_precision!
        assert_eq!(rich["locations"], json!(["Riverside Cafe"]));
        assert_eq!(rich["places_precision"], json!("names"));
        // Only photo media_ref survives; screenshot media_ref is filtered out!
        assert_eq!(
            rich["moments"][0]["media_refs"],
            json!(["20260304/import.image/180500_0"])
        );

        // 4. Mute (`hidden: true`) immediately removes Experience from range AND live ref lookup!
        set_experience_hidden(
            dir.path(),
            "personal",
            "20260304",
            &composed.id,
            true,
            "owner",
            None,
            "2026-03-04T21:00:00Z",
        )
        .unwrap();
        assert!(
            collect_range_experiences(
                dir.path(),
                principal,
                &rich_snap,
                &facets_only,
                "20260301",
                "20260307"
            )
            .is_empty()
        );
        assert!(
            live_experience_for_ref(
                dir.path(),
                &rich_snap,
                &ExperienceReference {
                    facet_id: personal_id,
                    facet_dir: "personal".to_owned(),
                    anchor_day: "20260304".to_owned(),
                    experience_id: composed.id,
                }
            )
            .is_none()
        );
    }

    fn seed_experience(root: &Path, day: &str, segment: &str, people: &[&str]) {
        let id = format!("social_{segment}");
        let mut act = Map::new();
        act.insert("id".to_owned(), json!(id));
        act.insert("stream".to_owned(), json!("watch"));
        act.insert("activity".to_owned(), json!("social"));
        act.insert("description".to_owned(), json!("Dinner"));
        act.insert("segments".to_owned(), json!([segment]));
        let _ = append_activity_record(root, "personal", day, act).unwrap();
        let aref = ActivityRef {
            day: day.to_owned(),
            stream: "watch".to_owned(),
            id,
            facet: None,
        };
        let created = create_owner_experience(
            root,
            "personal",
            vec![aref.clone()],
            Some("Dinner".to_owned()),
            Some("Alex had dinner.".to_owned()),
            "owner",
            "2026-03-04T20:00:00Z",
        )
        .unwrap();
        apply_composed_experience(
            root,
            "personal",
            day,
            &created.id,
            ComposedExperienceUpdate {
                title: format!("Dinner {segment}"),
                summary: "Alex had dinner.".to_owned(),
                locations: Vec::new(),
                entity_refs: people.iter().map(|p| (*p).to_owned()).collect(),
                moments: vec![ExperienceMoment {
                    id: "m1".to_owned(),
                    started_at: format!("{}-{}-{} 18:00", &day[..4], &day[4..6], &day[6..]),
                    ended_at: format!("{}-{}-{} 18:10", &day[..4], &day[4..6], &day[6..]),
                    summary: "Alex arrived.".to_owned(),
                    activity_refs: vec![aref],
                    segment_refs: Vec::new(),
                    media_refs: Vec::new(),
                }],
                started_at: None,
                ended_at: None,
            },
            "2026-03-04T20:05:00Z",
        )
        .unwrap();
    }

    /// A week's Experiences hydrate from one read of the journal's entities (a
    /// read per Experience made `get_week` take ~50 s on a journal with ~24k
    /// entities) and come out exactly as hydrating and filtering each one on its
    /// own does, merged, hidden and unknown people included.
    #[test]
    fn range_experiences_from_one_entity_read_match_per_experience_hydration() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        create_facet(root, "personal", "Personal", "", "#6366f1", "✨", None).unwrap();
        seed_person(root, "maya", "Maya Lin", false);
        seed_person(root, "secret_person", "Hidden Guest", true);
        std::fs::write(
            root.join("entities").join("retired.json"),
            json!({"ids": {"maya_old": {"state": "merged", "dir": "maya_old", "successor": "maya"}}})
                .to_string(),
        )
        .unwrap();
        seed_experience(root, "20260304", "180000_600", &["maya", "secret_person"]);
        seed_experience(
            root,
            "20260305",
            "190000_600",
            &["maya_old", "unknown_slug"],
        );
        seed_experience(root, "20260306", "200000_600", &[]);

        let principal = DispatchPrincipal {
            connection: "bearer:test",
            agent_identity: "test-agent",
        };
        let personal_id =
            solstone_core_facets::observe_facet_write_identity(root, "personal").unwrap();
        let facets = vec![(personal_id, "personal".to_owned(), "Personal".to_owned())];
        let snapshot = snapshot_with(
            &[AdmittedCategory::Facets, AdmittedCategory::Entities],
            None,
            ConnectionScope::WholeJournal,
        );
        let admitted: BTreeSet<String> = ["personal".to_owned()].into_iter().collect();
        let strip = |mut v: Value| {
            v.as_object_mut().unwrap().remove("reference");
            v
        };

        let list =
            collect_range_experiences(root, principal, &snapshot, &facets, "20260301", "20260307");
        assert_eq!(list.len(), 3);
        let merged: Vec<&str> = list[1]["people"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["entity_id"].as_str().unwrap())
            .collect();
        assert_eq!(merged, ["maya", "unknown_slug"]);
        let mut records =
            list_experiences_in_range(root, "personal", "20260301", "20260307", false).unwrap();
        records.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        assert_eq!(records.len(), 3);
        for (row, rec) in list.into_iter().zip(records) {
            let one_by_one = filter_hydrated_experience(
                root,
                &snapshot,
                &admitted,
                &solstone_core_facets::hydrate_experience(root, rec, None),
                "Personal",
                None,
            );
            assert_eq!(strip(row), strip(one_by_one));
        }
        let records =
            list_experiences_in_range(root, "personal", "20260301", "20260307", false).unwrap();
        let listed = solstone_core_facets::hydrate_experiences(root, records.clone());
        for (from_list, rec) in listed.iter().zip(records) {
            let alone = solstone_core_facets::hydrate_experience(root, rec, None);
            assert_eq!(from_list.hydrated.people, alone.hydrated.people);
        }
        assert!(solstone_core_facets::hydrate_experiences(root, Vec::new()).is_empty());

        // The list hydrates from one read of the entities: a change after that
        // read is not seen, so nothing re-reads them per Experience.
        let context = ExperienceHydrationContext::load(root);
        seed_person(root, "maya", "Maya Lin", true);
        let rec = list_experiences_in_range(root, "personal", "20260304", "20260304", false)
            .unwrap()
            .remove(0);
        let held = hydrate_experience_with(root, rec.clone(), None, &context);
        assert_eq!(held.hydrated.people.len(), 1);
        assert_eq!(held.hydrated.people[0].entity_id, "maya");
        assert!(!context.hidden_ids().contains("maya"));
        let fresh = solstone_core_facets::hydrate_experience(root, rec, None);
        assert!(fresh.hydrated.people.is_empty());
    }
}
