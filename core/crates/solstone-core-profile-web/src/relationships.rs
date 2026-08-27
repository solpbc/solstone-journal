// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Declared-facet relationship descriptions for profile reads.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use solstone_core_facets::{
    enrich_relationship_with_journal, list_declared_facet_names, read_facet_entity_link,
};

use crate::error::{ProfileError, ProfileResult};
use crate::resolution::ResolvedEntity;

pub(crate) fn load_facet_descriptions(
    journal_root: &Path,
    target: &ResolvedEntity,
) -> ProfileResult<BTreeMap<String, String>> {
    let facets = list_declared_facet_names(journal_root).map_err(ProfileError::internal)?;
    let mut descriptions = BTreeMap::new();
    for facet in facets {
        let relationship = read_facet_entity_link(journal_root, &facet, &target.entity_id)
            .map_err(ProfileError::internal)?;
        let Some(relationship) = relationship else {
            continue;
        };
        let enriched =
            enrich_relationship_with_journal(relationship.value(), Some(&target.entity.value));
        descriptions.insert(facet, string_field(&enriched, "description"));
    }
    Ok(descriptions)
}

pub(crate) fn selected_facets(
    descriptions: &BTreeMap<String, String>,
    requested: Option<&[String]>,
) -> Vec<String> {
    match requested {
        None => descriptions.keys().cloned().collect(),
        Some(requested) => requested
            .iter()
            .filter(|facet| descriptions.contains_key(facet.as_str()))
            .cloned()
            .collect(),
    }
}

pub(crate) fn description_for(
    descriptions: &BTreeMap<String, String>,
    requested: Option<&[String]>,
) -> Option<String> {
    let selected = selected_facets(descriptions, requested);
    let descriptions = selected
        .iter()
        .filter_map(|facet| descriptions.get(facet))
        .filter(|description| !description.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    (!descriptions.is_empty()).then(|| descriptions.join(" | "))
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{description_for, load_facet_descriptions, selected_facets};
    use crate::resolution::resolve_target;
    use crate::test_support::{journal, write_json};

    #[test]
    fn preserves_requested_csv_order_duplicates_and_blank_descriptions() {
        let temporary = journal();
        write_json(
            temporary.path(),
            "entities/pat/entity.json",
            json!({"id":"pat","name":"Pat"}),
        );
        for (facet, muted, description) in [
            ("work", false, "Work colleague"),
            ("quiet", true, ""),
            ("personal", false, "Neighbor"),
        ] {
            write_json(
                temporary.path(),
                &format!("facets/{facet}/facet.json"),
                json!({"name":facet,"muted":muted}),
            );
            write_json(
                temporary.path(),
                &format!("facets/{facet}/entities/pat/entity.json"),
                json!({"entity_id":"pat","description":description}),
            );
        }
        let target = resolve_target(temporary.path(), "pat")
            .expect("resolution")
            .expect("target");
        let descriptions =
            load_facet_descriptions(temporary.path(), &target).expect("descriptions");
        let requested = vec![
            "personal".to_owned(),
            "missing".to_owned(),
            "work".to_owned(),
            "personal".to_owned(),
            "quiet".to_owned(),
        ];

        assert_eq!(
            selected_facets(&descriptions, Some(&requested)),
            vec!["personal", "work", "personal", "quiet"]
        );
        assert_eq!(
            description_for(&descriptions, Some(&requested)),
            Some("Neighbor | Work colleague | Neighbor".to_owned())
        );
    }
}
