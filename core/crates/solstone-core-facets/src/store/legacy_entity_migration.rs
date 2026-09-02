// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-time migration of legacy facet-scoped `entities.jsonl` records into the
//! journal-wide entity structure.
//!
//! Legacy files are read and never modified. Canonical identities are built by
//! exact case-insensitive name match first, then token-sorted fuzzy match at the
//! caller's threshold compared with `>=`, mirroring the Python migration's
//! per-canonical two-check loop rather than two separate passes.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_entity::{EntityWriteError, is_valid_entity_type, save_entity_identity};
use solstone_core_entity_matching::{char_len, entity_slug, token_sort_ratio};
use solstone_core_journal_io::read_text;

use super::error::{FacetStoreError, FacetWriteError};
use super::map::list_facet_directories;
use super::write::save_facet_entity_link;

/// Journal-level identity fields that never belong to a facet relationship.
const JOURNAL_ONLY_FIELDS: [&str; 6] = ["id", "name", "type", "aka", "is_principal", "created_at"];

/// What one legacy facet-entity migration run loaded, merged, and wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyFacetEntityMigrationReport {
    /// Non-detached legacy records loaded from every facet.
    pub loaded: usize,
    /// Canonical journal identities written (or planned, when `dry_run`).
    pub canonicals: usize,
    /// Legacy records folded into an already-built canonical.
    pub merges: usize,
    /// Facet relationships written (or planned, when `dry_run`).
    pub relationships: usize,
    /// Detached legacy records skipped before any matching happened.
    pub skipped_detached: usize,
    /// Canonicals whose sources disagreed about entity type.
    pub type_conflicts: usize,
    /// Whether the run planned only.
    pub dry_run: bool,
}

/// Failure while migrating legacy facet entities.
#[derive(Debug)]
pub enum FacetEntityMigrationError {
    /// A facet directory or legacy file could not be enumerated.
    Read(FacetStoreError),
    /// A journal identity could not be written.
    Identity(EntityWriteError),
    /// A facet relationship could not be written.
    Relationship(FacetWriteError),
}

impl fmt::Display for FacetEntityMigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Identity(error) => error.fmt(formatter),
            Self::Relationship(error) => error.fmt(formatter),
        }
    }
}

impl Error for FacetEntityMigrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Identity(error) => Some(error),
            Self::Relationship(error) => Some(error),
        }
    }
}

impl From<FacetStoreError> for FacetEntityMigrationError {
    fn from(error: FacetStoreError) -> Self {
        Self::Read(error)
    }
}

impl From<EntityWriteError> for FacetEntityMigrationError {
    fn from(error: EntityWriteError) -> Self {
        Self::Identity(error)
    }
}

impl From<FacetWriteError> for FacetEntityMigrationError {
    fn from(error: FacetWriteError) -> Self {
        Self::Relationship(error)
    }
}

/// One legacy record as it exists inside a single facet.
struct FacetEntity {
    facet: String,
    name: String,
    entity_type: String,
    aka: Vec<String>,
    is_principal: bool,
    raw: Map<String, Value>,
}

/// One merged identity spanning every facet that contributed to it.
struct CanonicalEntity {
    name: String,
    entity_type: String,
    aka: BTreeSet<String>,
    is_principal: bool,
    merged_from: Vec<(String, String)>,
}

impl CanonicalEntity {
    fn id(&self) -> String {
        entity_slug(&self.name)
    }
}

/// Migrate every facet's legacy `entities.jsonl` into journal-wide entities.
///
/// The legacy files are never rewritten or removed. `fuzzy_threshold` is
/// compared with `>=` on the 0..=100 scale.
pub fn migrate_legacy_facet_entities(
    journal_root: &Path,
    fuzzy_threshold: u8,
    dry_run: bool,
) -> Result<LegacyFacetEntityMigrationReport, FacetEntityMigrationError> {
    let mut report = LegacyFacetEntityMigrationReport {
        dry_run,
        ..LegacyFacetEntityMigrationReport::default()
    };
    let (facet_entities, skipped_detached) = load_legacy_entities(journal_root)?;
    report.skipped_detached = skipped_detached;
    report.loaded = facet_entities.len();
    if facet_entities.is_empty() {
        return Ok(report);
    }

    let mut canonicals: Vec<CanonicalEntity> = Vec::new();
    for entity in &facet_entities {
        if entity.name.is_empty() {
            continue;
        }
        match find_matching_canonical(entity, &canonicals, f64::from(fuzzy_threshold)) {
            Some(index) => {
                merge_into_canonical(&mut canonicals[index], entity);
                report.merges += 1;
            }
            None => canonicals.push(create_canonical(entity)),
        }
    }
    report.type_conflicts = count_type_conflicts(&canonicals, &facet_entities);

    let named = facet_entities
        .iter()
        .filter(|entity| !entity.name.is_empty())
        .count();
    if dry_run {
        report.canonicals = canonicals.len();
        report.relationships = named;
        return Ok(report);
    }

    for canonical in &canonicals {
        let identity = journal_identity(canonical);
        save_entity_identity(journal_root, &canonical.id(), &identity, None)?;
        report.canonicals += 1;
    }

    // Later canonicals win a duplicate `(facet, name)` key exactly as Python's
    // dict assignment does.
    let mut lookup: BTreeMap<(&str, &str), String> = BTreeMap::new();
    for canonical in &canonicals {
        let id = canonical.id();
        for (facet, name) in &canonical.merged_from {
            lookup.insert((facet.as_str(), name.as_str()), id.clone());
        }
    }

    for entity in &facet_entities {
        if entity.name.is_empty() {
            continue;
        }
        let Some(entity_id) = lookup.get(&(entity.facet.as_str(), entity.name.as_str())) else {
            log::warn!(
                "no canonical entity found for {}/{}",
                entity.facet,
                entity.name
            );
            continue;
        };
        let relationship = facet_relationship_fields(entity);
        save_facet_entity_link(
            journal_root,
            &entity.facet,
            entity_id,
            entity_id,
            &relationship,
        )?;
        report.relationships += 1;
    }

    Ok(report)
}

/// Read every facet's legacy records, skipping detached ones.
fn load_legacy_entities(
    journal_root: &Path,
) -> Result<(Vec<FacetEntity>, usize), FacetEntityMigrationError> {
    let mut facets = list_facet_directories(journal_root)?;
    facets.sort();
    let mut loaded = Vec::new();
    let mut skipped_detached = 0;
    for facet in facets {
        let path = journal_root
            .join("facets")
            .join(&facet)
            .join("entities.jsonl");
        let text = read_text(&path, String::new())
            .map_err(|error| FacetEntityMigrationError::Read(FacetStoreError::Read(error)))?;
        for record in parse_entity_file(&text) {
            // Type validation already dropped invalid records, so a detached
            // record with an invalid type is never counted here.
            if is_truthy(record.get("detached")) {
                skipped_detached += 1;
                continue;
            }
            loaded.push(FacetEntity {
                facet: facet.clone(),
                name: string_field(&record, "name"),
                entity_type: string_field(&record, "type"),
                aka: string_list_field(&record, "aka"),
                is_principal: is_truthy(record.get("is_principal")),
                raw: record,
            });
        }
    }
    Ok((loaded, skipped_detached))
}

/// Parse legacy JSONL exactly as Python `parse_entity_file` does with type
/// validation enabled: blank and malformed lines are skipped, non-objects are
/// skipped, invalid entity types are dropped, and the four core fields are
/// materialized ahead of every other retained field.
fn parse_entity_file(text: &str) -> Vec<Map<String, Value>> {
    let mut records = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(Value::Object(data)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let entity_type = string_field(&data, "type");
        let name = string_field(&data, "name");
        if !is_valid_entity_type(&entity_type) {
            continue;
        }
        let id = match data.get("id") {
            Some(Value::String(id)) if !id.is_empty() => id.clone(),
            _ => entity_slug(&name),
        };
        let mut record = Map::new();
        record.insert("id".to_owned(), Value::String(id));
        record.insert("type".to_owned(), Value::String(entity_type));
        record.insert("name".to_owned(), Value::String(name));
        record.insert(
            "description".to_owned(),
            Value::String(string_field(&data, "description")),
        );
        for (key, value) in data {
            record.entry(key).or_insert(value);
        }
        records.push(record);
    }
    records
}

/// Find the first canonical matching by exact-lowercase name, then by fuzzy
/// score, checking both against each canonical before moving to the next.
fn find_matching_canonical(
    entity: &FacetEntity,
    canonicals: &[CanonicalEntity],
    threshold: f64,
) -> Option<usize> {
    if entity.name.is_empty() {
        return None;
    }
    let lowered = entity.name.to_lowercase();
    canonicals.iter().position(|canonical| {
        lowered == canonical.name.to_lowercase()
            || token_sort_ratio(&entity.name, &canonical.name) >= threshold
    })
}

/// Fold one legacy record into an existing canonical: longest name wins, akas
/// union, principal is a logical OR.
fn merge_into_canonical(canonical: &mut CanonicalEntity, entity: &FacetEntity) {
    if char_len(&entity.name) > char_len(&canonical.name) {
        let previous = std::mem::replace(&mut canonical.name, entity.name.clone());
        canonical.aka.insert(previous);
    } else if entity.name.to_lowercase() != canonical.name.to_lowercase() {
        canonical.aka.insert(entity.name.clone());
    }
    for aka in &entity.aka {
        if !aka.is_empty() && aka.to_lowercase() != canonical.name.to_lowercase() {
            canonical.aka.insert(aka.clone());
        }
    }
    let canonical_lower = canonical.name.to_lowercase();
    canonical
        .aka
        .retain(|aka| aka.to_lowercase() != canonical_lower);
    if entity.is_principal {
        canonical.is_principal = true;
    }
    canonical
        .merged_from
        .push((entity.facet.clone(), entity.name.clone()));
}

/// Seed a canonical from one legacy record.
fn create_canonical(entity: &FacetEntity) -> CanonicalEntity {
    let lowered = entity.name.to_lowercase();
    let aka = entity
        .aka
        .iter()
        .filter(|aka| aka.to_lowercase() != lowered)
        .cloned()
        .collect();
    CanonicalEntity {
        name: entity.name.clone(),
        entity_type: entity.entity_type.clone(),
        aka,
        is_principal: entity.is_principal,
        merged_from: vec![(entity.facet.clone(), entity.name.clone())],
    }
}

/// Build the journal identity payload Python's `to_journal_entity` produces.
fn journal_identity(canonical: &CanonicalEntity) -> Value {
    let mut identity = Map::new();
    identity.insert("id".to_owned(), Value::String(canonical.id()));
    identity.insert("name".to_owned(), Value::String(canonical.name.clone()));
    identity.insert(
        "type".to_owned(),
        Value::String(canonical.entity_type.clone()),
    );
    identity.insert(
        "created_at".to_owned(),
        Value::Number(Utc::now().timestamp_millis().into()),
    );
    if !canonical.aka.is_empty() {
        identity.insert(
            "aka".to_owned(),
            Value::Array(canonical.aka.iter().cloned().map(Value::String).collect()),
        );
    }
    if canonical.is_principal {
        identity.insert("is_principal".to_owned(), Value::Bool(true));
    }
    Value::Object(identity)
}

/// Strip journal-level fields, leaving only relationship-scoped ones.
fn facet_relationship_fields(entity: &FacetEntity) -> Map<String, Value> {
    entity
        .raw
        .iter()
        .filter(|(key, _)| !JOURNAL_ONLY_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Count canonicals whose contributing records disagreed about entity type.
fn count_type_conflicts(canonicals: &[CanonicalEntity], entities: &[FacetEntity]) -> usize {
    canonicals
        .iter()
        .filter(|canonical| {
            let mut types = BTreeSet::new();
            for (facet, name) in &canonical.merged_from {
                for entity in entities {
                    if &entity.facet == facet && &entity.name == name {
                        types.insert(entity.entity_type.as_str());
                    }
                }
            }
            types.len() > 1
        })
        .count()
}

fn string_field(record: &Map<String, Value>, key: &str) -> String {
    record
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn string_list_field(record: &Map<String, Value>, key: &str) -> Vec<String> {
    match record.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Python truthiness for the fields this migration branches on.
fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(fields)) => !fields.is_empty(),
        Some(Value::Null) | None => false,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;

    use tempfile::{TempDir, tempdir};

    use super::*;

    /// Threshold the production adapter passes, pinned as a literal here so the
    /// boundary tests below cannot drift with a caller default.
    const FUZZY_THRESHOLD: u8 = 90;

    fn write_facet(journal: &Path, facet: &str, lines: &[Value]) {
        let dir = journal.join("facets").join(facet);
        fs::create_dir_all(&dir).unwrap();
        let text = lines
            .iter()
            .map(|line| serde_json::to_string(line).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.join("entities.jsonl"), format!("{text}\n")).unwrap();
    }

    fn journal_entity(journal: &Path, id: &str) -> Value {
        let text = fs::read_to_string(journal.join("entities").join(id).join("entity.json"))
            .unwrap_or_else(|_| panic!("journal entity {id} exists"));
        serde_json::from_str(&text).unwrap()
    }

    fn relationship(journal: &Path, facet: &str, id: &str) -> Value {
        let path = journal
            .join("facets")
            .join(facet)
            .join("entities")
            .join(id)
            .join("entity.json");
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn fresh() -> TempDir {
        tempdir().unwrap()
    }

    #[test]
    fn a_journal_without_facets_migrates_nothing() {
        let temp = fresh();
        let report =
            migrate_legacy_facet_entities(temp.path(), FUZZY_THRESHOLD, false).expect("migrates");
        assert_eq!(report, LegacyFacetEntityMigrationReport::default());
    }

    #[test]
    fn detached_records_are_skipped_before_matching_and_legacy_bytes_survive() {
        let temp = fresh();
        write_facet(
            temp.path(),
            "work",
            &[
                serde_json::json!({
                    "name": "Acme Corp", "type": "organization",
                    "description": "A company.", "aka": ["Acme"],
                    "is_principal": false, "detached": false
                }),
                serde_json::json!({
                    "name": "Ghost Entity", "type": "person",
                    "description": "Soft-deleted.", "aka": [],
                    "is_principal": false, "detached": true
                }),
            ],
        );
        let legacy = temp.path().join("facets/work/entities.jsonl");
        let before = fs::read(&legacy).unwrap();

        let report =
            migrate_legacy_facet_entities(temp.path(), FUZZY_THRESHOLD, false).expect("migrates");

        assert_eq!(report.loaded, 1);
        assert_eq!(report.skipped_detached, 1);
        assert_eq!(report.canonicals, 1);
        assert_eq!(report.relationships, 1);
        assert_eq!(
            fs::read(&legacy).unwrap(),
            before,
            "legacy file is retained"
        );
        let entity = journal_entity(temp.path(), "acme_corp");
        assert_eq!(entity["name"], "Acme Corp");
        assert_eq!(entity["aka"], serde_json::json!(["Acme"]));
        assert!(entity["created_at"].is_number());
        let link = relationship(temp.path(), "work", "acme_corp");
        assert_eq!(link["entity_id"], "acme_corp");
        assert_eq!(link["description"], "A company.");
        assert_eq!(link["detached"], false);
        for journal_only in ["name", "type", "aka", "is_principal", "created_at"] {
            assert!(
                link.get(journal_only).is_none(),
                "{journal_only} leaked into the relationship"
            );
        }
    }

    #[test]
    fn an_exact_case_insensitive_name_merges_across_facets() {
        let temp = fresh();
        write_facet(
            temp.path(),
            "work",
            &[serde_json::json!({"name": "Acme Corp", "type": "organization"})],
        );
        write_facet(
            temp.path(),
            "personal",
            &[serde_json::json!({"name": "acme corp", "type": "organization"})],
        );

        let report =
            migrate_legacy_facet_entities(temp.path(), FUZZY_THRESHOLD, false).expect("migrates");

        assert_eq!(report.loaded, 2);
        assert_eq!(report.canonicals, 1);
        assert_eq!(report.merges, 1);
        assert_eq!(report.relationships, 2);
        // "personal" sorts before "work", so its record seeds the canonical.
        let entity = journal_entity(temp.path(), "acme_corp");
        assert_eq!(entity["name"], "acme corp");
        assert!(
            entity.get("aka").is_none(),
            "a case-only variant is not an aka"
        );
    }

    #[test]
    fn fuzzy_matching_is_inclusive_at_exactly_ninety_and_rejects_eighty_nine() {
        // These two pairs sit hard against the literal `FUZZY_THRESHOLD = 90`
        // compared with `>=`. The matching pair scores exactly 90.0, so a `>`
        // comparison would fail this test; the rejected pair scores 89.47, so a
        // threshold of 89 would also fail it. Both scores are Python
        // `rapidfuzz.fuzz.token_sort_ratio` values.
        let rejected = token_sort_ratio("Evangeline Kowalski", "Evangelina Kowalsky");
        let matched = token_sort_ratio("Anastasia Vandenberg", "Anastasio Vandenbarg");
        assert!(
            (89.0..90.0).contains(&rejected),
            "expected the 89 bucket, scored {rejected}"
        );
        assert_eq!(matched, 90.0, "expected exactly the inclusive boundary");

        let temp = fresh();
        write_facet(
            temp.path(),
            "aaa",
            &[serde_json::json!({"name": "Anastasia Vandenberg", "type": "person"})],
        );
        write_facet(
            temp.path(),
            "bbb",
            &[serde_json::json!({"name": "Anastasio Vandenbarg", "type": "person"})],
        );
        write_facet(
            temp.path(),
            "ccc",
            &[serde_json::json!({"name": "Evangeline Kowalski", "type": "person"})],
        );
        write_facet(
            temp.path(),
            "ddd",
            &[serde_json::json!({"name": "Evangelina Kowalsky", "type": "person"})],
        );

        let report =
            migrate_legacy_facet_entities(temp.path(), FUZZY_THRESHOLD, false).expect("migrates");

        assert_eq!(report.loaded, 4);
        assert_eq!(
            report.merges, 1,
            "the 90.0 pair merges and the 89.47 pair does not"
        );
        assert_eq!(report.canonicals, 3);
        assert_eq!(
            journal_entity(temp.path(), "anastasia_vandenberg")["aka"],
            serde_json::json!(["Anastasio Vandenbarg"]),
            "the boundary match folds its partner in as an aka"
        );
        // The rejected pair kept two separate identities.
        assert_eq!(
            journal_entity(temp.path(), "evangeline_kowalski")["name"],
            "Evangeline Kowalski"
        );
        assert_eq!(
            journal_entity(temp.path(), "evangelina_kowalsky")["name"],
            "Evangelina Kowalsky"
        );
    }

    #[test]
    fn raising_the_threshold_above_a_score_stops_that_merge() {
        let temp = fresh();
        write_facet(
            temp.path(),
            "aaa",
            &[serde_json::json!({"name": "Jonathan Smith", "type": "person"})],
        );
        write_facet(
            temp.path(),
            "ccc",
            &[serde_json::json!({"name": "Johnathan Smith", "type": "person"})],
        );

        let merged =
            migrate_legacy_facet_entities(temp.path(), 90, true).expect("dry run at threshold");
        assert_eq!(merged.merges, 1);

        let split =
            migrate_legacy_facet_entities(temp.path(), 100, true).expect("dry run above score");
        assert_eq!(split.merges, 0);
        assert_eq!(split.canonicals, 2);
    }

    #[test]
    fn merging_takes_the_longest_name_unions_akas_and_ors_principal() {
        let temp = fresh();
        write_facet(
            temp.path(),
            "aaa",
            &[serde_json::json!({
                "name": "Jonathan Smith", "type": "person",
                "aka": ["Jonny"], "is_principal": false
            })],
        );
        write_facet(
            temp.path(),
            "bbb",
            &[serde_json::json!({
                "name": "Johnathan Smithe", "type": "person",
                "aka": ["J. Smith"], "is_principal": true
            })],
        );

        let report =
            migrate_legacy_facet_entities(temp.path(), FUZZY_THRESHOLD, false).expect("migrates");

        assert_eq!(report.merges, 1);
        assert_eq!(report.canonicals, 1);
        let entity = journal_entity(temp.path(), "johnathan_smithe");
        assert_eq!(
            entity["name"], "Johnathan Smithe",
            "the longest name wins the canonical slot"
        );
        assert_eq!(
            entity["aka"],
            serde_json::json!(["J. Smith", "Jonathan Smith", "Jonny"]),
            "the displaced name and both aka lists union, sorted"
        );
        assert_eq!(
            entity["is_principal"], true,
            "principal is a logical OR across sources"
        );
        // Both facets point at the winning canonical's slug.
        assert_eq!(
            relationship(temp.path(), "aaa", "johnathan_smithe")["entity_id"],
            "johnathan_smithe"
        );
        assert_eq!(
            relationship(temp.path(), "bbb", "johnathan_smithe")["entity_id"],
            "johnathan_smithe"
        );
    }

    #[test]
    fn a_dry_run_plans_the_same_counts_and_writes_nothing() {
        let temp = fresh();
        write_facet(
            temp.path(),
            "work",
            &[
                serde_json::json!({"name": "Acme Corp", "type": "organization"}),
                serde_json::json!({"name": "Beta Inc", "type": "organization"}),
            ],
        );

        let report =
            migrate_legacy_facet_entities(temp.path(), FUZZY_THRESHOLD, true).expect("plans");

        assert!(report.dry_run);
        assert_eq!(report.loaded, 2);
        assert_eq!(report.canonicals, 2);
        assert_eq!(report.relationships, 2);
        assert!(!temp.path().join("entities").exists());
        assert!(!temp.path().join("facets/work/entities").exists());
    }

    #[test]
    fn invalid_types_and_malformed_lines_are_dropped_before_the_detached_count() {
        let temp = fresh();
        let dir = temp.path().join("facets/work");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entities.jsonl"),
            concat!(
                "\n",
                "not json\n",
                "[1, 2]\n",
                "{\"name\": \"Bad Type\", \"type\": \"ab\"}\n",
                "{\"name\": \"Detached Bad Type\", \"type\": \"x\", \"detached\": true}\n",
                "{\"name\": \"Good\", \"type\": \"person\"}\n",
            ),
        )
        .unwrap();

        let report =
            migrate_legacy_facet_entities(temp.path(), FUZZY_THRESHOLD, false).expect("migrates");

        assert_eq!(report.loaded, 1);
        assert_eq!(
            report.skipped_detached, 0,
            "a detached record with an invalid type never reaches the detached count"
        );
        assert_eq!(report.canonicals, 1);
    }

    #[test]
    fn type_conflicts_are_counted_without_blocking_the_merge() {
        let temp = fresh();
        write_facet(
            temp.path(),
            "aaa",
            &[serde_json::json!({"name": "Acme Corp", "type": "organization"})],
        );
        write_facet(
            temp.path(),
            "bbb",
            &[serde_json::json!({"name": "Acme Corp", "type": "company"})],
        );

        let report =
            migrate_legacy_facet_entities(temp.path(), FUZZY_THRESHOLD, false).expect("migrates");

        assert_eq!(report.merges, 1);
        assert_eq!(report.type_conflicts, 1);
        assert_eq!(report.canonicals, 1);
    }
}
