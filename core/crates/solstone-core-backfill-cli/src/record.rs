// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};
use solstone_core_processing_record::vocab::{SCHEMA, STATE_EMPTY};

pub fn processing_record(
    handler: &str,
    reason: &str,
    input_size: u64,
    instant: DateTime<Utc>,
) -> Value {
    let mut record = Map::new();
    record.insert("schema".to_owned(), Value::String(SCHEMA.to_owned()));
    record.insert("state".to_owned(), Value::String(STATE_EMPTY.to_owned()));
    record.insert("reason_code".to_owned(), Value::String(reason.to_owned()));
    record.insert("handler".to_owned(), Value::String(handler.to_owned()));
    record.insert(
        "attempted_at".to_owned(),
        Value::String(instant.to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    record.insert("input_size".to_owned(), Value::from(input_size));
    record.insert("source".to_owned(), Value::String("backfill".to_owned()));
    Value::Object(record)
}
