// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use crate::args::ListOptions;
use crate::discovery::TalentConfig;
use crate::validation::is_truthy;

pub(crate) fn jsonl(configs: &[TalentConfig], options: &ListOptions) -> String {
    let mut output = String::new();
    for config in filtered(configs, options) {
        let mut record = Map::new();
        record.insert("file".to_owned(), Value::String(config.file.clone()));
        for (key, value) in &config.metadata {
            if key != "path" && key != "mtime" {
                record.insert(key.clone(), value.clone());
            }
        }
        output.push_str(&solstone_core_format::json_compact_ascii(&Value::Object(
            record,
        )));
        output.push('\n');
    }
    output
}

pub(crate) fn filtered<'a>(
    configs: &'a [TalentConfig],
    options: &ListOptions,
) -> Vec<&'a TalentConfig> {
    configs
        .iter()
        .filter(|config| {
            options.schedule.as_deref().is_none_or(|schedule| {
                config.metadata.get("schedule").and_then(Value::as_str) == Some(schedule)
            }) && options.source.as_deref().is_none_or(|source| {
                config.metadata.get("source").and_then(Value::as_str) == Some(source)
            }) && (options.disabled || !config.metadata.get("disabled").is_some_and(is_truthy))
        })
        .collect()
}
