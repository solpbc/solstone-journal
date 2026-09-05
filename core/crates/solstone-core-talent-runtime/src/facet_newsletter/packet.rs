// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic source packet for `facet_newsletter`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::NaiveDate;
use serde_json::{Map, Value, json};
use solstone_core_indexer_query::{Order, SearchRequest, search};

const MAX_ACTIVITY_RECORDS: usize = 12;
const MAX_NARRATIVES_PER_ACTIVITY: usize = 4;
const INDEX_RESULTS_PER_AGENT: usize = 10;
const MAX_ATTACHED_ENTITY_RECORDS: usize = 12;
const MAX_DETECTED_ENTITY_RECORDS: usize = 12;
const MAX_ENTITY_RESULTS: usize = 12;
const MAX_TITLE_CHARS: usize = 220;
const MAX_DESCRIPTION_CHARS: usize = 700;
const MAX_DETAILS_CHARS: usize = 1200;
const MAX_STORY_BODY_CHARS: usize = 1800;
const MAX_NARRATIVE_CHARS: usize = 2400;
const MAX_INDEX_TEXT_CHARS: usize = 1800;
const MAX_ENTITY_TEXT_CHARS: usize = 1200;
const MAX_PRIOR_NEWSLETTER_CHARS: usize = 4000;
const MAX_FACET_SUMMARY_CHARS: usize = 3000;
const MAX_PACKET_CHARS: usize = 56000;
const TIER_ONE_INDEX_AGENTS: [&str; 4] = ["flow", "span", "event", "meetings"];
const TIER_TWO_INDEX_AGENTS: [&str; 2] = ["decisions", "followups"];

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Packet {
    pub source_packet: String,
    pub source_counts: String,
    pub coverage_preamble: String,
    pub gaps: Vec<String>,
    pub substantive_items: usize,
}
#[derive(Clone, Debug)]
struct Item {
    source_class: String,
    source_label: Option<String>,
    agent: Option<String>,
    origin: String,
    tier: u8,
    text: String,
    clipped: bool,
    order: Vec<String>,
    path: Option<String>,
    result_id: Option<String>,
}
struct ItemSpec {
    source_class: String,
    source_label: Option<String>,
    agent: Option<String>,
    origin: String,
    tier: u8,
    text: String,
    clipped: bool,
    order: Vec<String>,
    path: Option<String>,
    result_id: Option<String>,
}
struct IndexItemSpec<'a> {
    id: &'a str,
    text: &'a str,
    path: &'a str,
    agent: &'a str,
    tier: u8,
    limit: usize,
}

macro_rules! gather_item {
    ($source_class:expr, $source_label:expr, $agent:expr, $origin:expr, $tier:expr, $text:expr, $clipped:expr, $order:expr, $path:expr, $result_id:expr $(,)?) => {
        gather_item(ItemSpec {
            source_class: $source_class.to_owned(),
            source_label: $source_label,
            agent: $agent,
            origin: $origin,
            tier: $tier,
            text: $text,
            clipped: $clipped,
            order: $order,
            path: $path,
            result_id: $result_id,
        })
    };
}

pub(super) fn valid_day(day: &str) -> bool {
    day.len() == 8
        && day.bytes().all(|b| b.is_ascii_digit())
        && NaiveDate::parse_from_str(day, "%Y%m%d").is_ok()
}
pub(super) fn unsafe_facet(facet: &str) -> bool {
    facet.is_empty()
        || facet.contains(['/', '\\'])
        || facet.contains("..")
        || facet.starts_with('.')
}

pub(super) fn gather(journal: &Path, facet: &str, day: &str) -> Result<Packet, String> {
    if journal.join("facets").is_file() {
        return Err("facet store root is not a directory".to_owned());
    }
    let mut gaps = Vec::new();
    let mut items = gather_activity_records(journal, facet, day, &mut gaps);
    items.extend(gather_activity_narratives(journal, facet, day, &mut gaps));
    for agent in TIER_ONE_INDEX_AGENTS {
        items.extend(search_day_evidence(journal, agent, facet, day, &mut gaps));
    }
    for agent in TIER_TWO_INDEX_AGENTS {
        items.extend(search_day_evidence(journal, agent, facet, day, &mut gaps));
    }
    items.extend(load_facet_metadata(journal, facet, &mut gaps));
    items.extend(load_facet_entity_context(journal, facet, day, &mut gaps));
    items.extend(load_prior_newsletter(journal, facet, day, &mut gaps));
    items.extend(search_facet_entities(journal, facet, day, &mut gaps));
    let (included, dropped) = gather_budgeted_items(items.clone());
    gaps.extend(dropped);
    let mut counts = gather_source_counts(&included);
    add_available_source_counts(&mut counts, &items);
    let substantive_items = included
        .iter()
        .filter(|item| matches!(item.tier, 1 | 2))
        .count();
    counts.insert("substantive_items".to_owned(), substantive_items);
    Ok(Packet {
        source_packet: render_packet(&included),
        source_counts: render_source_counts(&counts),
        coverage_preamble: render_coverage_preamble(&counts, &gaps),
        gaps,
        substantive_items,
    })
}

fn gather_activity_records(
    journal: &Path,
    facet: &str,
    day: &str,
    gaps: &mut Vec<String>,
) -> Vec<Item> {
    let mut rows = match solstone_core_facets::load_activity_records(journal, facet, day, false) {
        Ok(rows) => rows,
        Err(error) => {
            gaps.push(format!(
                "failed: activity_record failed for {facet} {day}: {error}"
            ));
            return Vec::new();
        }
    };
    if rows.is_empty() {
        gaps.push(format!("missing: activity_record absent for {facet} {day}"));
        return Vec::new();
    }
    rows.sort_by_key(render_activity_order);
    if rows.len() > MAX_ACTIVITY_RECORDS {
        gaps.push(format!(
            "capped: activity_record limited to {MAX_ACTIVITY_RECORDS}/{} items",
            rows.len()
        ));
        rows.truncate(MAX_ACTIVITY_RECORDS);
    }
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let origin = row
                .get("id")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("activity-{index}"));
            let (text, clipped) = render_activity_record(row, &origin, gaps);
            gather_item!(
                "activity_record",
                None,
                None,
                origin,
                1,
                text,
                clipped,
                vec![
                    "0".to_owned(),
                    format!(
                        "{:020}",
                        row.get("created_at")
                            .and_then(Value::as_i64)
                            .unwrap_or_default()
                    ),
                ],
                None,
                None,
            )
        })
        .collect()
}
fn gather_activity_narratives(
    journal: &Path,
    facet: &str,
    day: &str,
    gaps: &mut Vec<String>,
) -> Vec<Item> {
    let dir = journal
        .join("facets")
        .join(facet)
        .join("activities")
        .join(day);
    let Ok(entries) = fs::read_dir(&dir) else {
        gaps.push(format!(
            "missing: activity_narrative absent for {facet} {day}"
        ));
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten().filter(|e| e.path().is_dir()) {
        let mut files = fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .collect::<Vec<_>>();
        files.sort_by_key(|e| e.file_name());
        if files.len() > MAX_NARRATIVES_PER_ACTIVITY {
            gaps.push(format!(
                "capped: activity_narrative limited to {MAX_NARRATIVES_PER_ACTIVITY}/{} items",
                files.len()
            ));
            files.truncate(MAX_NARRATIVES_PER_ACTIVITY);
        }
        for (i, file) in files.into_iter().enumerate() {
            let origin = format!(
                "{}/{}",
                entry.file_name().to_string_lossy(),
                file.file_name().to_string_lossy()
            );
            match fs::read_to_string(file.path()) {
                Ok(raw) => {
                    let (text, clipped) = render_clipped_text(
                        raw.trim(),
                        MAX_NARRATIVE_CHARS,
                        gaps,
                        "activity_narrative",
                        &origin,
                        "markdown",
                        None,
                    );
                    out.push(gather_item!(
                        "activity_narrative",
                        None,
                        Some(file.file_name().to_string_lossy().into_owned()),
                        origin,
                        1,
                        text,
                        clipped,
                        vec!["1".to_owned(), format!("{i:04}")],
                        None,
                        None,
                    ));
                }
                Err(error) => gaps.push(format!(
                    "failed: activity_narrative failed for {facet} {day}: {error}"
                )),
            }
        }
    }
    if out.is_empty() {
        gaps.push(format!(
            "missing: activity_narrative absent for {facet} {day}"
        ));
    }
    out
}
fn search_day_evidence(
    journal: &Path,
    agent: &str,
    facet: &str,
    day: &str,
    gaps: &mut Vec<String>,
) -> Vec<Item> {
    let mut request = SearchRequest::new("", Order::Relevance);
    request.limit = INDEX_RESULTS_PER_AGENT;
    request.day = Some(day.to_owned());
    request.facet = Some(facet.to_owned());
    request.agent = Some(agent.to_owned());
    let label = format!("index_result:{agent}");
    let response = match search(
        journal,
        &request,
        NaiveDate::parse_from_str(day, "%Y%m%d").expect("validated day"),
    ) {
        Ok(response) => response,
        Err(error) => {
            gaps.push(format!("failed: {label} failed for {facet} {day}: {error}"));
            return Vec::new();
        }
    };
    if response.results.is_empty() {
        gaps.push(format!("missing: {label} absent for {facet} {day}"));
        return Vec::new();
    }
    if response
        .total
        .is_some_and(|total| total as usize > response.results.len())
    {
        gaps.push(format!(
            "capped: {label} limited to {}/{} items",
            response.results.len(),
            response.total.unwrap()
        ));
    }
    let tier = if TIER_ONE_INDEX_AGENTS.contains(&agent) {
        1
    } else {
        2
    };
    response
        .results
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            gather_index_item(
                IndexItemSpec {
                    id: &hit.id,
                    text: &hit.text,
                    path: &hit.metadata.path,
                    agent,
                    tier,
                    limit: MAX_INDEX_TEXT_CHARS,
                },
                gaps,
                vec![
                    format!("{}", if tier == 1 { 2 + index } else { 6 + index }),
                    format!("{index:04}"),
                ],
                None,
            )
        })
        .collect()
}
fn load_prior_newsletter(
    journal: &Path,
    facet: &str,
    day: &str,
    gaps: &mut Vec<String>,
) -> Vec<Item> {
    let news_dir = journal.join("facets").join(facet).join("news");
    let source_day = match fs::read_dir(&news_dir) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter_map(|name| name.strip_suffix(".md").map(str::to_owned))
            .filter(|name| valid_day(name) && name.as_str() < day)
            .max(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            gaps.push(format!(
                "failed: prior_newsletter failed for {facet}: {error}"
            ));
            return Vec::new();
        }
    };
    let Some(source_day) = source_day else {
        gaps.push(format!("missing: prior_newsletter absent for {facet}"));
        return Vec::new();
    };
    match solstone_core_facets::read_news_file(journal, facet, &format!("{source_day}.md")) {
        Ok(Some(raw)) if !raw.trim().is_empty() => {
            let origin = format!("{facet}:{source_day}");
            let (text, clipped) = render_clipped_text(
                raw.trim(),
                MAX_PRIOR_NEWSLETTER_CHARS,
                gaps,
                "prior_newsletter",
                &origin,
                "markdown",
                None,
            );
            vec![gather_item!(
                "prior_newsletter",
                None,
                None,
                origin,
                3,
                text,
                clipped,
                vec!["1".to_owned(), day.to_owned()],
                None,
                None,
            )]
        }
        Ok(_) => {
            gaps.push(format!("missing: prior_newsletter absent for {facet}"));
            Vec::new()
        }
        Err(error) => {
            gaps.push(format!(
                "failed: prior_newsletter failed for {facet}: {error}"
            ));
            Vec::new()
        }
    }
}
fn load_facet_metadata(journal: &Path, facet: &str, gaps: &mut Vec<String>) -> Vec<Item> {
    match solstone_core_facets::read_facet_declaration(journal, facet) {
        Ok(Some(declaration)) if !declaration.description.trim().is_empty() => {
            let (text, clipped) = render_clipped_text(
                declaration.description.trim(),
                MAX_FACET_SUMMARY_CHARS,
                gaps,
                "facet_metadata",
                facet,
                "summary",
                None,
            );
            vec![gather_item!(
                "facet_metadata",
                None,
                None,
                facet.to_owned(),
                3,
                text,
                clipped,
                vec!["0".to_owned(), facet.to_owned()],
                None,
                None,
            )]
        }
        Ok(_) => {
            gaps.push(format!("missing: facet_metadata absent for {facet}"));
            Vec::new()
        }
        Err(error) => {
            gaps.push(format!(
                "failed: facet_metadata failed for {facet}: {error}"
            ));
            Vec::new()
        }
    }
}
fn load_facet_entity_context(
    journal: &Path,
    facet: &str,
    day: &str,
    gaps: &mut Vec<String>,
) -> Vec<Item> {
    let mut result = Vec::new();
    let attached = match solstone_core_facets::list_scoped_facet_entities(journal, facet, false, false) {
        Ok(items) => items.into_iter().map(|item| json!({"id":item.entity_id,"name":item.identity.get("name").cloned().unwrap_or(Value::Null),"type":item.identity.get("type").cloned().unwrap_or(Value::Null),"description":item.relationship.get("description").cloned().unwrap_or(Value::Null)})).collect(),
        Err(error) => { gaps.push(format!("failed: facet_entities:attached failed for {facet}: {error}")); Vec::new() }
    };
    result.extend(entity_items(
        attached,
        "attached",
        MAX_ATTACHED_ENTITY_RECORDS,
        0,
        facet,
        day,
        gaps,
    ));
    let detected = match solstone_core_facets::read_detected_entities(journal, facet, day) {
        Ok(items) => items,
        Err(error) => {
            gaps.push(format!(
                "failed: facet_entities:detected failed for {facet} {day}: {error}"
            ));
            Vec::new()
        }
    };
    result.extend(entity_items(
        detected,
        "detected",
        MAX_DETECTED_ENTITY_RECORDS,
        1,
        facet,
        day,
        gaps,
    ));
    result
}
fn entity_items(
    mut entities: Vec<Value>,
    kind: &str,
    limit: usize,
    order: usize,
    facet: &str,
    day: &str,
    gaps: &mut Vec<String>,
) -> Vec<Item> {
    let label = format!("facet_entities:{kind}");
    if entities.is_empty() {
        gaps.push(format!("missing: {label} absent for {facet} {day}"));
        return Vec::new();
    }
    if entities.len() > limit {
        gaps.push(format!(
            "capped: {label} limited to {limit}/{} items",
            entities.len()
        ));
        entities.truncate(limit);
    }
    entities
        .into_iter()
        .enumerate()
        .map(|(index, entity)| {
            let origin = render_entity_origin(kind, &entity, &format!("{kind}-{index}"));
            let (text, clipped) = render_entity_text(&entity, &origin, kind, gaps);
            gather_item!(
                "facet_entities",
                Some(label.clone()),
                Some(kind.to_owned()),
                origin,
                3,
                text,
                clipped,
                vec!["2".to_owned(), order.to_string(), format!("{index:04}")],
                None,
                None,
            )
        })
        .collect()
}
fn search_facet_entities(
    journal: &Path,
    facet: &str,
    day: &str,
    gaps: &mut Vec<String>,
) -> Vec<Item> {
    let mut request = SearchRequest::new("", Order::Relevance);
    request.limit = MAX_ENTITY_RESULTS;
    request.day = Some(day.to_owned());
    request.facet = Some(facet.to_owned());
    request.agent = Some("entity".to_owned());
    match search(
        journal,
        &request,
        NaiveDate::parse_from_str(day, "%Y%m%d").expect("validated day"),
    ) {
        Ok(response) if !response.results.is_empty() => response
            .results
            .iter()
            .enumerate()
            .map(|(i, hit)| {
                gather_index_item(
                    IndexItemSpec {
                        id: &hit.id,
                        text: &hit.text,
                        path: &hit.metadata.path,
                        agent: "entity",
                        tier: 3,
                        limit: MAX_ENTITY_TEXT_CHARS,
                    },
                    gaps,
                    vec!["2".to_owned(), "2".to_owned(), format!("{i:04}")],
                    Some("facet_entities:indexed".to_owned()),
                )
            })
            .collect(),
        Ok(_) => {
            gaps.push(format!(
                "missing: facet_entities:indexed absent for {facet} {day}"
            ));
            Vec::new()
        }
        Err(error) => {
            gaps.push(format!(
                "failed: facet_entities:indexed failed for {facet} {day}: {error}"
            ));
            Vec::new()
        }
    }
}
fn gather_index_item(
    spec: IndexItemSpec<'_>,
    gaps: &mut Vec<String>,
    order: Vec<String>,
    label: Option<String>,
) -> Item {
    let origin = format!("{} ({}; agent={})", spec.id, spec.path, spec.agent);
    let (text, clipped) = render_clipped_text(
        spec.text.trim(),
        spec.limit,
        gaps,
        "index_result",
        &origin,
        "text",
        Some(spec.agent),
    );
    gather_item!(
        "index_result",
        label,
        Some(spec.agent.to_owned()),
        origin,
        spec.tier,
        text,
        clipped,
        order,
        Some(spec.path.to_owned()),
        Some(spec.id.to_owned()),
    )
}
fn gather_item(spec: ItemSpec) -> Item {
    Item {
        source_class: spec.source_class,
        source_label: spec.source_label,
        agent: spec.agent,
        origin: spec.origin,
        tier: spec.tier,
        text: spec.text,
        clipped: spec.clipped,
        order: spec.order,
        path: spec.path,
        result_id: spec.result_id,
    }
}

fn gather_budgeted_items(mut items: Vec<Item>) -> (Vec<Item>, Vec<String>) {
    items.sort_by_key(|item| (item.tier, item.order.clone()));
    let mut included = Vec::new();
    let mut gaps = Vec::new();
    for item in items {
        let mut next = included.clone();
        next.push(item.clone());
        if render_packet(&next).chars().count() <= MAX_PACKET_CHARS {
            included.push(item)
        } else {
            gaps.push(format!(
                "dropped: {} {} dropped under total packet budget",
                render_source_label(&item),
                item.origin
            ));
        }
    }
    (included, gaps)
}
fn gather_source_counts(items: &[Item]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for key in [
        "total_included",
        "tier1_included",
        "tier2_included",
        "tier3_included",
        "activity_record",
        "activity_narrative",
        "index_result:event",
        "index_result:meetings",
        "index_result:decisions",
        "index_result:followups",
        "index_result:flow",
        "index_result:span",
        "prior_newsletter",
        "facet_metadata",
        "facet_entities:attached",
        "facet_entities:detected",
        "facet_entities:indexed",
    ] {
        counts.insert(key.to_owned(), 0);
    }
    counts.insert("total_included".to_owned(), items.len());
    for item in items {
        *counts
            .entry(format!("tier{}_included", item.tier))
            .or_default() += 1;
        *counts.entry(render_source_label(item)).or_default() += 1;
    }
    counts
}
fn add_available_source_counts(counts: &mut BTreeMap<String, usize>, items: &[Item]) {
    let available = gather_source_counts(items);
    for key in ["total", "tier1", "tier2", "tier3"] {
        counts.insert(
            format!("{key}_available"),
            *available.get(&format!("{key}_included")).unwrap_or(&0),
        );
    }
    for (key, value) in available {
        if key == "total_included" || key.starts_with("tier") {
            continue;
        }
        counts.insert(format!("{key}_available"), value);
        counts.entry(format!("{key}_included")).or_insert(0);
    }
}
fn render_activity_order(record: &Map<String, Value>) -> (i64, String, String) {
    (
        record
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        record
            .get("start")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        record
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}
fn render_activity_record(
    record: &Map<String, Value>,
    origin: &str,
    gaps: &mut Vec<String>,
) -> (String, bool) {
    let mut clipped = false;
    let mut field = |name: &str, limit| {
        let (value, was) = render_clipped_text(
            record
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim(),
            limit,
            gaps,
            "activity_record",
            origin,
            name,
            None,
        );
        clipped |= was;
        value
    };
    let mut lines = vec![format!("Activity ID: {origin}")];
    for (name, label, limit) in [
        ("title", "Title", MAX_TITLE_CHARS),
        ("description", "Description", MAX_DESCRIPTION_CHARS),
        ("details", "Details", MAX_DETAILS_CHARS),
    ] {
        let value = field(name, limit);
        if !value.is_empty() {
            lines.push(format!("{label}: {value}"));
        }
    }
    for name in ["activity", "source", "start", "end", "target_date"] {
        if let Some(value) = record
            .get(name)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            lines.push(format!("{}: {value}", name.replace('_', " ")));
        }
    }
    if let Some(story) = record.get("story").and_then(Value::as_object) {
        let (value, was) = render_clipped_text(
            story
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            MAX_STORY_BODY_CHARS,
            gaps,
            "activity_record",
            origin,
            "story.body",
            None,
        );
        clipped |= was;
        if !value.is_empty() {
            lines.push(format!("Story: {value}"));
        }
    }
    for (name, label) in [
        ("active_entities", "Active entities"),
        ("segments", "Segments"),
        ("participation", "Participation"),
        ("commitments", "Commitments"),
        ("closures", "Closures"),
        ("decisions", "Decisions"),
    ] {
        lines.extend(render_list_field(record, name, label));
    }
    (lines.join("\n"), clipped)
}
fn render_list_field(record: &Map<String, Value>, field: &str, label: &str) -> Vec<String> {
    record
        .get(field)
        .filter(|value| !value.is_null() && !value.as_array().is_some_and(Vec::is_empty))
        .map(|value| {
            vec![format!(
                "{label}: {}",
                serde_json::to_string(value).expect("value serializes")
            )]
        })
        .unwrap_or_default()
}
fn render_clipped_text(
    text: &str,
    limit: usize,
    gaps: &mut Vec<String>,
    source: &str,
    origin: &str,
    field: &str,
    agent: Option<&str>,
) -> (String, bool) {
    if text.chars().count() <= limit {
        return (text.to_owned(), false);
    }
    let label = agent
        .map(|agent| format!("{source}:{agent}"))
        .unwrap_or_else(|| source.to_owned());
    gaps.push(format!(
        "clipped: {label} {origin} field {field} clipped to {limit} chars"
    ));
    (truncate_chars(text, limit).trim_end().to_owned(), true)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}
fn render_source_label(item: &Item) -> String {
    item.source_label.clone().unwrap_or_else(|| {
        match (item.source_class.as_str(), item.agent.as_deref()) {
            ("index_result", Some(agent)) => format!("index_result:{agent}"),
            ("facet_entities", Some("entity")) => "facet_entities:indexed".to_owned(),
            ("facet_entities", Some(agent)) => format!("facet_entities:{agent}"),
            _ => item.source_class.clone(),
        }
    })
}
fn render_entity_origin(kind: &str, entity: &Value, fallback: &str) -> String {
    let name = entity
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| entity.get("id").and_then(Value::as_str).unwrap_or(fallback));
    let id = entity.get("id").and_then(Value::as_str).unwrap_or_default();
    if !id.is_empty() && id != name {
        format!("{kind}:{name} ({id})")
    } else {
        format!("{kind}:{name}")
    }
}
fn render_entity_text(
    entity: &Value,
    origin: &str,
    kind: &str,
    gaps: &mut Vec<String>,
) -> (String, bool) {
    let mut payload = Map::new();
    for key in [
        "id",
        "type",
        "name",
        "description",
        "aka",
        "relationship",
        "attached_at",
        "updated_at",
        "last_seen",
        "last_active_day",
        "count",
    ] {
        if let Some(value) = entity.get(key).filter(|value| {
            !value.is_null()
                && **value != Value::String(String::new())
                && **value != Value::Array(Vec::new())
                && **value != Value::Object(Map::new())
        }) {
            payload.insert(key.to_owned(), value.clone());
        }
    }
    let rendered = if payload.is_empty() {
        entity.clone()
    } else {
        Value::Object(payload)
    };
    let text = serde_json::to_string(&rendered).expect("value serializes");
    render_clipped_text(
        &text,
        MAX_ENTITY_TEXT_CHARS,
        gaps,
        "facet_entities",
        origin,
        "json",
        Some(kind),
    )
}
fn render_packet(items: &[Item]) -> String {
    if items.is_empty() {
        return "(no included sources)".to_owned();
    }
    let mut items = items.to_vec();
    items.sort_by_key(|item| (item.tier, item.order.clone()));
    let mut lines = Vec::new();
    let mut previous = String::new();
    for item in items {
        let label = render_source_label(&item);
        if label != previous {
            if !lines.is_empty() {
                lines.push(String::new())
            }
            lines.push(format!("## {label}"));
            previous = label;
        }
        lines.extend([String::new(), format!("### {}", item.origin)]);
        let mut provenance = vec![
            format!("source_class={}", item.source_class),
            format!("origin={}", item.origin),
            format!("tier={}", item.tier),
            format!("clipped={}", item.clipped),
        ];
        if let Some(agent) = item.agent {
            provenance.push(format!("agent={agent}"))
        }
        if let Some(path) = item.path {
            provenance.push(format!("path={path}"))
        }
        if let Some(id) = item.result_id {
            provenance.push(format!("result_id={id}"))
        }
        lines.push(format!("Provenance: {}", provenance.join("; ")));
        if !item.text.is_empty() {
            lines.extend([String::new(), item.text]);
        }
    }
    lines.join("\n").trim().to_owned()
}
fn render_source_counts(counts: &BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(key, value)| format!("  {key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}
fn render_coverage_preamble(counts: &BTreeMap<String, usize>, gaps: &[String]) -> String {
    let mut lines = vec![
        "Coverage:".to_owned(),
        format!(
            "- Included sources: {}",
            counts.get("total_included").unwrap_or(&0)
        ),
        format!(
            "- Substantive sources: {}",
            counts.get("substantive_items").unwrap_or(&0)
        ),
        format!(
            "- Tier 1 / 2 / 3: {} / {} / {}",
            counts.get("tier1_included").unwrap_or(&0),
            counts.get("tier2_included").unwrap_or(&0),
            counts.get("tier3_included").unwrap_or(&0)
        ),
    ];
    if gaps.is_empty() {
        lines.push("Gaps: none".to_owned())
    } else {
        lines.push("Gaps:".to_owned());
        lines.extend(gaps.iter().map(|gap| format!("- {gap}")));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clips_and_drops_over_budget() {
        // Derived from solstone/talent/facet_newsletter.py:595-606,741-759.
        let mut gaps = Vec::new();
        let (text, clipped) = render_clipped_text("abcdef", 3, &mut gaps, "x", "o", "f", None);
        assert_eq!(text, "abc");
        assert!(clipped);
        let item = gather_item!(
            "x",
            None,
            None,
            "o".to_owned(),
            1,
            "z".repeat(MAX_PACKET_CHARS),
            false,
            vec!["0".to_owned()],
            None,
            None,
        );
        assert!(gather_budgeted_items(vec![item]).0.is_empty());
    }

    #[test]
    fn clips_unicode_by_characters_not_bytes() {
        // Derived from solstone/talent/facet_newsletter.py:741-759.
        let mut gaps = Vec::new();
        let (text, clipped) = render_clipped_text("ééé", 2, &mut gaps, "x", "o", "f", None);
        assert!(clipped);
        assert_eq!(text, "éé");
    }

    #[test]
    fn prior_newsletter_uses_the_newest_day_before_cursor() {
        // Derived from solstone/talent/facet_newsletter.py:319-359 and solstone/think/facets.py:464-549.
        let root = tempfile::tempdir().unwrap();
        solstone_core_facets::write_news_file(root.path(), "work", "20260101.md", "older").unwrap();
        solstone_core_facets::write_news_file(root.path(), "work", "20260102.md", "newer").unwrap();
        let mut gaps = Vec::new();
        let items = load_prior_newsletter(root.path(), "work", "20260103", &mut gaps);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].origin, "work:20260102");
        assert_eq!(items[0].text, "newer");
    }

    #[test]
    fn coverage_preamble_keeps_source_exception_text() {
        // Derived from solstone/talent/facet_newsletter.py:361-403,405-486,864-878.
        let preamble = render_coverage_preamble(
            &BTreeMap::new(),
            &["failed: facet_metadata failed for work: permission denied".to_owned()],
        );
        assert!(preamble.contains("permission denied"));
    }
}
