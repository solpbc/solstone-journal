// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read recently active, speech-friendly entity names across facets.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;
use solstone_core_entity::entity_last_active_ts;
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};

use super::error::{FacetEntityWriteError, FacetStoreError};
use super::facet_entities::list_scoped_facet_entities;
use super::paths::facets_dir;
use super::relationship_scans::enrich_relationship_with_journal;

/// Load attached entities from every facet, keeping the first occurrence of each id.
pub fn load_all_attached_entities(
    journal_root: &Path,
    sort_by_last_seen: bool,
    limit: Option<usize>,
) -> Result<Vec<Value>, FacetEntityWriteError> {
    let mut entities = Vec::new();
    let mut seen_ids = HashSet::new();
    for entry in list_dir_entries(&facets_dir(journal_root)?).map_err(FacetStoreError::from)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let facet_dir = entry.name.to_string_lossy().into_owned();
        for scoped in list_scoped_facet_entities(journal_root, &facet_dir, false, false)? {
            let enriched =
                enrich_relationship_with_journal(&scoped.relationship, Some(&scoped.identity));
            let Some(entity_id) = enriched.get("id").and_then(Value::as_str) else {
                continue;
            };
            if entity_id.is_empty() || !seen_ids.insert(entity_id.to_owned()) {
                continue;
            }
            entities.push(enriched);
        }
    }
    if sort_by_last_seen {
        // `sort_by_key` is stable, preserving sorted-facet first-occurrence order for equal activity.
        entities.sort_by_key(|entity| Reverse(entity_last_active_ts(entity)));
    }
    if let Some(limit) = limit.filter(|limit| *limit > 0) {
        entities.truncate(limit);
    }
    Ok(entities)
}

/// Whether a name is suitable for a speech-recognition vocabulary.
///
/// This intentionally accepts ASCII whitespace only. Python accepts all Unicode `\s` characters,
/// but persisted entity names in this vocabulary path are expected to use ordinary ASCII spacing.
pub fn is_speakable(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character.is_ascii_whitespace()
                || matches!(character, '.' | '-' | '\'')
        })
        && name
            .chars()
            .any(|character| character.is_ascii_alphabetic())
}

/// Extract first-word and first-parenthetical spoken variants, preserving insertion order.
pub fn extract_spoken_names(entities: &[Value]) -> Vec<String> {
    let mut spoken_names = Vec::new();
    for entity in entities {
        if let Some(name) = entity.get("name").and_then(Value::as_str) {
            add_name_variants(name, &mut spoken_names);
        }
        if let Some(aka) = entity.get("aka").and_then(Value::as_array) {
            for name in aka.iter().filter_map(Value::as_str) {
                add_name_variants(name, &mut spoken_names);
            }
        }
    }
    spoken_names
}

/// Load speech-friendly names from the most recently active attached entities.
pub fn load_recent_entity_names(
    journal_root: &Path,
    limit: usize,
) -> Result<Option<Vec<String>>, FacetEntityWriteError> {
    let entities = load_all_attached_entities(journal_root, true, Some(limit))?;
    if entities.is_empty() {
        return Ok(None);
    }
    let names = extract_spoken_names(&entities);
    Ok((!names.is_empty()).then_some(names))
}

fn add_name_variants(name: &str, spoken_names: &mut Vec<String>) {
    if name.is_empty() {
        return;
    }
    if let Some(first_word) = strip_parentheticals(name).split_whitespace().next() {
        add_if_speakable(first_word, spoken_names);
    }
    if let Some(contents) = first_parenthetical_contents(name) {
        for item in contents.split(',').map(str::trim) {
            add_if_speakable(item, spoken_names);
        }
    }
}

fn add_if_speakable(name: &str, spoken_names: &mut Vec<String>) {
    if !name.is_empty()
        && !spoken_names.iter().any(|existing| existing == name)
        && is_speakable(name)
    {
        spoken_names.push(name.to_owned());
    }
}

/// Mirror `re.sub(r"\s*\([^)]+\)", "", name)`: strip every non-empty pair.
fn strip_parentheticals(name: &str) -> String {
    let characters: Vec<char> = name.chars().collect();
    let mut result = String::new();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] == '(' {
            let mut end = index + 1;
            while end < characters.len() && characters[end] != ')' {
                end += 1;
            }
            if end < characters.len() && end > index + 1 {
                while result.chars().last().is_some_and(char::is_whitespace) {
                    result.pop();
                }
                index = end + 1;
                continue;
            }
        }
        result.push(characters[index]);
        index += 1;
    }
    result.trim().to_owned()
}

/// Mirror `re.search(r"\(([^)]+)\)", name)`: retain items from only the first pair.
fn first_parenthetical_contents(name: &str) -> Option<&str> {
    let mut search_start = 0;
    while let Some(relative_start) = name[search_start..].find('(') {
        let start = search_start + relative_start;
        let after_open = &name[start + '('.len_utf8()..];
        let end = after_open.find(')')?;
        let contents = &after_open[..end];
        if !contents.is_empty() {
            return Some(contents);
        }
        search_start = start + '('.len_utf8();
    }
    None
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use serde_json::{Map, json};
    use solstone_core_entity::save_entity_identity;

    use crate::store_tests::TempDir;
    use crate::{create_facet, save_facet_entity_link};

    fn attach_entity(
        root: &Path,
        facet: &str,
        entity_id: &str,
        name: &str,
        aka: &[&str],
        relationship_fields: Map<String, Value>,
    ) {
        create_facet(root, facet, facet, "", "blue", "💼", None).unwrap();
        let identity = json!({
            "id": entity_id,
            "name": name,
            "type": "Person",
            "aka": aka,
        });
        save_entity_identity(root, entity_id, &identity, None).unwrap();
        save_facet_entity_link(
            root,
            facet,
            &format!("{entity_id}-{facet}"),
            entity_id,
            &relationship_fields,
        )
        .unwrap();
    }

    fn entity(name: &str) -> Value {
        json!({"name": name})
    }

    #[test]
    fn spoken_names_expand_after_the_entity_limit() {
        let temporary = TempDir::new();
        attach_entity(
            temporary.path(),
            "work",
            "federal",
            "Federal Aviation Administration (FAA)",
            &[],
            Map::new(),
        );
        attach_entity(
            temporary.path(),
            "work",
            "ryan",
            "Ryan Reed (R2)",
            &[],
            Map::new(),
        );

        let names = load_recent_entity_names(temporary.path(), 2)
            .unwrap()
            .unwrap();

        assert_eq!(names, ["Federal", "FAA", "Ryan", "R2"]);
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn attached_entity_sort_keeps_equal_timestamp_facet_order() {
        let temporary = TempDir::new();
        attach_entity(temporary.path(), "alpha", "zeta", "Zeta", &[], Map::new());
        attach_entity(temporary.path(), "bravo", "alpha", "Alpha", &[], Map::new());
        attach_entity(
            temporary.path(),
            "charlie",
            "middle",
            "Middle",
            &[],
            Map::new(),
        );

        let entities = load_all_attached_entities(temporary.path(), true, None).unwrap();
        let ordered_ids: Vec<_> = entities
            .iter()
            .filter_map(|entity| entity.get("id").and_then(Value::as_str))
            .collect();
        assert_eq!(ordered_ids, ["zeta", "alpha", "middle"]);

        // The former ascending-sort-then-reverse approach reverses equal-key ties.
        let mut ascending_then_reversed = entities.clone();
        ascending_then_reversed.sort_by_key(entity_last_active_ts);
        ascending_then_reversed.reverse();
        let reversed_ids: Vec<_> = ascending_then_reversed
            .iter()
            .filter_map(|entity| entity.get("id").and_then(Value::as_str))
            .collect();
        assert_eq!(reversed_ids, ["middle", "alpha", "zeta"]);
    }

    #[test]
    fn attached_entities_deduplicate_across_facets_by_first_occurrence() {
        let temporary = TempDir::new();
        let mut alpha = Map::new();
        alpha.insert("description".to_owned(), Value::String("first".to_owned()));
        attach_entity(
            temporary.path(),
            "alpha",
            "shared",
            "Shared Entity",
            &[],
            alpha,
        );
        let mut bravo = Map::new();
        bravo.insert("description".to_owned(), Value::String("second".to_owned()));
        attach_entity(
            temporary.path(),
            "bravo",
            "shared",
            "Shared Entity",
            &[],
            bravo,
        );

        let entities = load_all_attached_entities(temporary.path(), false, None).unwrap();

        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["description"], "first");
    }

    #[test]
    fn spoken_name_examples_and_parenthetical_asymmetry_match_python() {
        assert_eq!(
            extract_spoken_names(&[entity("Ryan Reed (R2)")]),
            ["Ryan", "R2"]
        );
        assert_eq!(
            extract_spoken_names(&[entity("Federal Aviation Administration (FAA)")]),
            ["Federal", "FAA"]
        );
        assert_eq!(extract_spoken_names(&[entity("Acme Corp")]), ["Acme"]);
        assert_eq!(
            extract_spoken_names(&[entity("send2trash")]),
            ["send2trash"]
        );
        assert!(extract_spoken_names(&[entity("entity_registry")]).is_empty());
        assert_eq!(
            extract_spoken_names(&[entity("Ryan Reed (R2) (backup)")]),
            ["Ryan", "R2"]
        );
        assert_eq!(
            extract_spoken_names(&[json!({"name": "Alice Chen", "aka": ["Ally One (A1)"]})]),
            ["Alice", "Ally", "A1"]
        );
    }

    #[test]
    fn spoken_names_skip_empty_parenthetical_groups_when_finding_variants() {
        assert_eq!(extract_spoken_names(&[entity("A () (B)")]), ["A", "B"]);
    }

    #[test]
    fn spoken_names_stop_at_whitespace_only_parenthetical_groups() {
        assert_eq!(extract_spoken_names(&[entity("A ( ) (Bob)")]), ["A"]);
    }

    #[test]
    fn spoken_names_keep_cross_entity_first_occurrence_order() {
        let entities = [entity("Ryan Reed"), entity("Ryan Stone (RS)")];

        assert_eq!(extract_spoken_names(&entities), ["Ryan", "RS"]);
    }

    #[test]
    fn speakable_names_require_ascii_letters_and_allowed_characters() {
        assert!(is_speakable("O'Connor-2.0"));
        assert!(!is_speakable("entity_registry"));
        assert!(!is_speakable("123"));
        assert!(!is_speakable(""));
    }

    #[test]
    fn recent_names_loads_the_most_active_attached_entity() {
        let temporary = TempDir::new();
        let mut older = Map::new();
        older.insert("attached_at".to_owned(), Value::Number(1.into()));
        attach_entity(
            temporary.path(),
            "work",
            "alice",
            "Alice Adams (Al)",
            &[],
            older,
        );
        let mut newer = Map::new();
        newer.insert("attached_at".to_owned(), Value::Number(2.into()));
        attach_entity(temporary.path(), "work", "bob", "Bob Builder", &[], newer);

        assert_eq!(
            load_recent_entity_names(temporary.path(), 1).unwrap(),
            Some(vec!["Bob".to_owned()])
        );
    }
}
