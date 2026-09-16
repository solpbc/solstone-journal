// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::candidates::EdgeDropCounter;
use super::{
    EdgeContext, EdgeError, EdgeRow, EdgeValue, JsonObject, edge_value_for_text, edge_value_for_ts,
    json_type_name, python_str, valid_edge_day,
};

pub(crate) fn extract_observation_edges(
    entries: &[JsonObject],
    context: &EdgeContext,
    drops: &mut EdgeDropCounter,
) -> Result<Vec<EdgeRow>, EdgeError> {
    let normalized_path = context.path.replace('\\', "/");
    let parts: Vec<&str> = normalized_path.split('/').collect();
    if parts.len() < 5 {
        return Err(EdgeError::InvalidObservationPath(context.path.clone()));
    }
    let source_id = parts[3];
    let mut rows = Vec::new();

    for observation in entries {
        if super::json_truthy(observation.get("retired")) {
            continue;
        }
        let Some(Value::Object(relation)) = observation.get("relation") else {
            continue;
        };

        let target = relation.get("target_entity_id");
        if !super::json_truthy(target) {
            drops.record_drop();
            continue;
        }
        // Check order differs from Python when both target and kind are bad; both fail the file before insert.
        let Some(Value::String(target_id)) = target else {
            return Err(EdgeError::InvalidObservationTargetEntityId);
        };
        if target_id == source_id {
            continue;
        }

        let kind = match relation.get("kind") {
            None => return Err(EdgeError::MissingObservationRelationKind),
            Some(Value::String(kind)) => kind.clone(),
            Some(value) => {
                return Err(EdgeError::InvalidEdgeKindType {
                    source: "observation relation",
                    value_type: json_type_name(value),
                });
            }
        };

        let observed_at = observation.get("observed_at");
        let anchor = match observed_at {
            None | Some(Value::Null) => None,
            Some(value) => Some(python_str(value, "observed_at")?),
        };

        rows.push(EdgeRow {
            src: source_id.to_string(),
            dst: target_id.clone(),
            kind,
            src_name: EdgeValue::Null,
            dst_name: edge_value_for_text(relation.get("target_name"), "dst_name")?,
            day: observation_day(observation.get("source_day")),
            facet: Some(context.facet.clone()),
            source: "observation".to_string(),
            path: context.path.clone(),
            anchor,
            label: edge_value_for_text(relation.get("note"), "label")?,
            ts: edge_value_for_ts(observed_at, "ts")?,
            weight: 1,
        });
    }

    Ok(rows)
}

fn observation_day(value: Option<&Value>) -> Option<String> {
    let Some(Value::String(day)) = value else {
        return None;
    };
    valid_edge_day(day).then(|| day.clone())
}
