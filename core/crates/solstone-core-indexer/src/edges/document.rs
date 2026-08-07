// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde_json::Value;

use super::candidates::EdgeResolver;
use super::{EdgeContext, EdgeError, EdgeRow, EdgeValue, JsonObject, segment_start_ts_ms};
use solstone_core_journal::python_strip;

pub(crate) fn extract_document_edges(
    payload: &JsonObject,
    context: &EdgeContext,
    resolver: &mut EdgeResolver,
) -> Result<Vec<EdgeRow>, EdgeError> {
    let Some(Value::Array(parties)) = payload.get("parties") else {
        return Ok(Vec::new());
    };

    let (anchor, segment) = segment_ref(&context.path)?;
    let ts = segment_start_ts_ms(&context.day, &segment, resolver.owner_timezone()?)?;
    let mut resolved = BTreeMap::new();
    for party in parties {
        let Value::Object(party) = party else {
            continue;
        };
        let name = match party.get("name") {
            Some(Value::String(name)) => name.as_str(),
            _ => "",
        };
        let Some(entity_id) = resolver.resolve(context, name)? else {
            continue;
        };
        resolved
            .entry(entity_id)
            .or_insert_with(|| python_strip(name).to_string());
    }

    let mut rows = Vec::new();
    let resolved_ids: Vec<String> = resolved.keys().cloned().collect();
    for left_index in 0..resolved_ids.len() {
        for right_id in resolved_ids.iter().skip(left_index + 1) {
            let left_id = &resolved_ids[left_index];
            rows.push(EdgeRow {
                src: left_id.clone(),
                dst: right_id.clone(),
                kind: "party-of".to_string(),
                src_name: EdgeValue::Text(resolved[left_id].clone()),
                dst_name: EdgeValue::Text(resolved[right_id].clone()),
                day: Some(context.day.clone()),
                facet: Some(context.facet.clone()),
                source: "document".to_string(),
                path: context.path.clone(),
                anchor: Some(anchor.clone()),
                label: EdgeValue::Text(String::new()),
                ts: EdgeValue::Int(ts),
                weight: 1,
            });
        }
    }

    Ok(rows)
}

fn segment_ref(path: &str) -> Result<(String, String), EdgeError> {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() < 3 {
        return Err(EdgeError::InvalidSegmentKey(path.to_string()));
    }
    Ok((parts[..3].join("/"), parts[2].to_string()))
}
