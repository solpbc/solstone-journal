// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An import the owner filed under a facet: Sense still writes the activity, but
//! only for that facet. The importer refuses a facet that is not enabled, so what
//! a transcript header names here was the owner's explicit choice.

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_facets::observe_declared_facet_inventory;
use solstone_core_transcripts::EntrySource;

const FACET_ENUM: &str = "/properties/facets/items/properties/facet/enum";
const FACET_LEVEL: &str = "/properties/facets/items/properties/level/enum";
const FACET_LIMIT: &str = "/properties/facets/maxItems";

/// The facet this segment's transcripts name, where all of them agree on one that is
/// still declared. A facet deleted or renamed since the import names nothing, so the
/// segment routes like any other instead of failing every reprocess.
pub(crate) fn chosen(sources: &[Option<EntrySource>], journal: &Path) -> Option<String> {
    let mut named = sources.iter().flatten().filter_map(header_facet);
    let facet = named.next()?;
    if named.any(|other| other != facet) {
        return None;
    }
    let inventory = observe_declared_facet_inventory(journal).ok()?;
    inventory
        .enabled
        .iter()
        .chain(&inventory.muted)
        .any(|declared| declared == facet)
        .then(|| facet.to_owned())
}

/// Text imports write `imported.facet`; an audio import's event metadata reaches the
/// transcript header as top-level `import_id` and `facet`.
fn header_facet(source: &EntrySource) -> Option<&str> {
    let header = source.records.first()?;
    let facet = match header.get("imported") {
        Some(imported) => imported.get("facet"),
        None if header.contains_key("import_id") => header.get("facet"),
        None => None,
    };
    facet?.as_str().filter(|facet| !facet.is_empty())
}

/// Leave the model only this facet to answer with, once. The activity is still the
/// model's own sentence, and an answer naming any other facet fails schema validation
/// rather than landing in the wrong one. Its level is `high`, the prompt's level for a
/// dominant facet: `low` means a weak fit, which the owner's choice has settled, and
/// activity state drops a `low` row, so the import would have no activity at all.
pub(crate) fn narrow_schema(composed: &mut Map<String, Value>, facet: &str) {
    let Some(schema) = composed.get_mut("json_schema") else {
        return;
    };
    for (pointer, narrowed) in [
        (FACET_ENUM, json!([facet])),
        (FACET_LEVEL, json!(["high"])),
        (FACET_LIMIT, json!(1)),
    ] {
        if let Some(value) = schema.pointer_mut(pointer) {
            *value = narrowed;
        }
    }
}
