// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use serde_json::Value;

use super::{
    EdgeContext, EdgeError, EdgeRow, EdgeValue, JsonObject, edge_int, edge_str, json_type_name,
    python_title,
};

pub(crate) fn extract_activity_edges(
    entries: &[JsonObject],
    context: &EdgeContext,
) -> Result<Vec<EdgeRow>, EdgeError> {
    let mut rows = Vec::new();

    for entry in entries {
        let record_id = edge_str(entry.get("id"), "id")?;
        let title = fallback_activity_title(entry)?;
        let edge_title = edge_str(Some(&Value::String(title)), "title")?;
        let ts = edge_int(entry.get("created_at"), "created_at")?;

        extract_participation_edges(entry, context, &record_id, &edge_title, ts, &mut rows);
        extract_relation_rows(entry.get("relations"), context, &record_id, ts, &mut rows)?;
    }

    Ok(rows)
}

fn fallback_activity_title(record: &JsonObject) -> Result<String, EdgeError> {
    let title = edge_str(record.get("title"), "title")?;
    if !title.is_empty() {
        return Ok(title);
    }

    let description = edge_str(record.get("description"), "description")?;
    if !description.is_empty() {
        return Ok(description);
    }

    let activity_value = if super::json_truthy(record.get("activity")) {
        record.get("activity")
    } else {
        record.get("id")
    };
    let activity = edge_str(activity_value, "activity")?;
    if !activity.is_empty() {
        return Ok(python_title(&activity));
    }

    Ok("untitled activity".to_string())
}

fn extract_participation_edges(
    record: &JsonObject,
    context: &EdgeContext,
    record_id: &str,
    title: &str,
    ts: i64,
    rows: &mut Vec<EdgeRow>,
) {
    let Some(Value::Array(participation)) = record.get("participation") else {
        return;
    };
    let mut attendees = Vec::new();
    let mut seen = BTreeSet::new();
    for part in participation {
        let Value::Object(part) = part else {
            continue;
        };
        let Some(Value::String(entity_id)) = part.get("entity_id") else {
            continue;
        };
        if part.get("role") == Some(&Value::String("attendee".to_string()))
            && !entity_id.is_empty()
            && seen.insert(entity_id.clone())
        {
            attendees.push(entity_id.clone());
        }
    }

    for left_index in 0..attendees.len() {
        for dst in attendees.iter().skip(left_index + 1) {
            rows.push(EdgeRow {
                src: attendees[left_index].clone(),
                dst: dst.clone(),
                kind: "attended-with".to_string(),
                src_name: EdgeValue::Null,
                dst_name: EdgeValue::Null,
                day: Some(context.day.clone()),
                facet: Some(context.facet.clone()),
                source: "participation".to_string(),
                path: context.path.clone(),
                anchor: Some(record_id.to_string()),
                label: EdgeValue::Text(title.to_string()),
                ts: EdgeValue::Int(ts),
                weight: 1,
            });
        }
    }
}

fn extract_relation_rows(
    value: Option<&Value>,
    context: &EdgeContext,
    anchor: &str,
    ts: i64,
    rows: &mut Vec<EdgeRow>,
) -> Result<(), EdgeError> {
    let Some(Value::Array(relations)) = value else {
        return Ok(());
    };
    for relation in relations {
        let Value::Object(relation) = relation else {
            continue;
        };
        let Some(Value::String(src)) = relation.get("from_entity_id") else {
            continue;
        };
        let Some(Value::String(dst)) = relation.get("to_entity_id") else {
            continue;
        };
        if src.is_empty() || dst.is_empty() || src == dst {
            continue;
        }
        let kind = match relation.get("kind") {
            Some(Value::String(kind)) => kind.clone(),
            Some(value) => {
                return Err(EdgeError::InvalidEdgeKindType {
                    source: "activity relation",
                    value_type: json_type_name(value),
                });
            }
            None => {
                return Err(EdgeError::InvalidEdgeKindType {
                    source: "activity relation",
                    value_type: "missing",
                });
            }
        };
        rows.push(EdgeRow {
            src: src.clone(),
            dst: dst.clone(),
            kind,
            src_name: EdgeValue::Text(edge_str(relation.get("from"), "from")?),
            dst_name: EdgeValue::Text(edge_str(relation.get("to"), "to")?),
            day: Some(context.day.clone()),
            facet: Some(context.facet.clone()),
            source: "relation".to_string(),
            path: context.path.clone(),
            anchor: Some(anchor.to_string()),
            label: EdgeValue::Text(relation_label(relation.get("note"), relation.get("quote"))?),
            ts: EdgeValue::Int(ts),
            weight: 1,
        });
    }
    Ok(())
}

fn relation_label(note: Option<&Value>, quote: Option<&Value>) -> Result<String, EdgeError> {
    let note = edge_str(note, "note")?;
    let quote = edge_str(quote, "quote")?;
    let mut parts = Vec::new();
    if !note.is_empty() {
        parts.push(note);
    }
    if !quote.is_empty() {
        parts.push(format!("\"{quote}\""));
    }
    Ok(parts.join(" — "))
}
