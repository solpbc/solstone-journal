// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only facet relationship scans and journal-identity enrichment.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};

use super::error::{FacetEntityWriteError, FacetStoreError};
use super::facet_entities::list_scoped_facet_entities;
use super::identity::read_facet_entity_link;
use super::map::list_facet_entity_directories;
use super::paths::facets_dir;

/// One facet relationship associated with a resolved journal entity.
#[derive(Debug, Clone, PartialEq)]
pub struct FacetRelationshipRecord {
    pub facet_dir: String,
    pub relationship: Value,
}

/// List relationship directory names in one facet.
pub fn scan_facet_relationships(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<Vec<String>, FacetStoreError> {
    list_facet_entity_directories(journal_root, facet_dir)
}

/// Load one facet's relationships keyed by their raw relationship directories.
pub fn load_all_facet_relationships(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<BTreeMap<String, Value>, FacetStoreError> {
    let mut relationships = BTreeMap::new();
    for relationship_dir in scan_facet_relationships(journal_root, facet_dir)? {
        let Some(link) = read_facet_entity_link(journal_root, facet_dir, &relationship_dir)? else {
            continue;
        };
        relationships.insert(relationship_dir, link.value().clone());
    }
    Ok(relationships)
}

/// Load relationships from every physical facet, keyed by resolved journal entity directory.
pub fn load_all_facet_relationships_across_facets(
    journal_root: &Path,
) -> Result<BTreeMap<String, Vec<FacetRelationshipRecord>>, FacetEntityWriteError> {
    let mut relationships = BTreeMap::new();
    for entry in list_dir_entries(&facets_dir(journal_root)?).map_err(FacetStoreError::from)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let facet_dir = entry.name.to_string_lossy().into_owned();
        for entity in list_scoped_facet_entities(journal_root, &facet_dir, true, true)? {
            relationships
                .entry(entity.entity_dir)
                .or_insert_with(Vec::new)
                .push(FacetRelationshipRecord {
                    facet_dir: facet_dir.clone(),
                    relationship: entity.relationship,
                });
        }
    }
    Ok(relationships)
}

/// Merge journal identity fields into a facet relationship without performing a lookup.
pub fn enrich_relationship_with_journal(
    relationship: &Value,
    journal_entity: Option<&Value>,
) -> Value {
    let mut result = relationship.clone();
    let Some(result_object) = result.as_object_mut() else {
        return result;
    };

    if let Some(journal_entity) = journal_entity {
        result_object.insert(
            "id".to_owned(),
            journal_entity
                .get("id")
                .cloned()
                .or_else(|| relationship.get("entity_id").cloned())
                .unwrap_or_else(|| Value::String(String::new())),
        );
        result_object.insert(
            "name".to_owned(),
            journal_entity
                .get("name")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())),
        );
        result_object.insert(
            "type".to_owned(),
            journal_entity
                .get("type")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())),
        );
        if let Some(aka) = journal_entity.get("aka").filter(|aka| is_truthy(aka)) {
            result_object.insert("aka".to_owned(), aka.clone());
        }
        if journal_entity.get("is_principal").is_some_and(is_truthy) {
            result_object.insert("is_principal".to_owned(), Value::Bool(true));
        }
        if journal_entity.get("blocked").is_some_and(is_truthy) {
            result_object.insert("blocked".to_owned(), Value::Bool(true));
        }
    } else {
        result_object.insert(
            "id".to_owned(),
            relationship
                .get("entity_id")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())),
        );
    }
    result_object.remove("entity_id");
    result
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
