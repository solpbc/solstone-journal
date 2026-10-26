// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal `describe` settings shared by the CLI and in-process callers.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_config::read_journal_config;

use crate::WinnowConfig;
use crate::selection::{CategoryOverride, parse_category_overrides};

#[derive(Clone, Debug)]
pub struct DescribeSettings {
    pub winnow: WinnowConfig,
    pub redact_rules: Vec<String>,
    pub max_extractions: u32,
    pub category_overrides: BTreeMap<String, CategoryOverride>,
}

impl Default for DescribeSettings {
    fn default() -> Self {
        Self {
            winnow: WinnowConfig::default(),
            redact_rules: Vec::new(),
            max_extractions: 20,
            category_overrides: BTreeMap::new(),
        }
    }
}

/// Read describe settings. Only scene-cut and stride override the winnow
/// defaults; dhash stays 8. Missing `describe` config is the default.
pub fn read_config(journal_path: &Path) -> Result<DescribeSettings, String> {
    let config = read_journal_config(journal_path)
        .map_err(|error| format!("failed to read journal config: {error}"))?
        .config;
    let Some(describe) = config
        .as_ref()
        .and_then(|config| config.get("describe"))
        .and_then(Value::as_object)
    else {
        return Ok(DescribeSettings::default());
    };

    let mut winnow = WinnowConfig::default();
    if let Some(value) = describe.get("scene_cut_threshold") {
        let Some(value) = value.as_u64().and_then(|value| u32::try_from(value).ok()) else {
            return Err(
                "describe.scene_cut_threshold must be an unsigned 32-bit integer".to_owned(),
            );
        };
        winnow.scene_cut_threshold = value;
    }
    if let Some(value) = describe.get("min_stride_seconds") {
        let Some(value) = value.as_f64().filter(|value| value.is_finite()) else {
            return Err("describe.min_stride_seconds must be a finite number".to_owned());
        };
        winnow.min_stride_seconds = value;
    }
    let redact_rules = match describe.get("redact") {
        None => Vec::new(),
        Some(Value::Array(rules)) => rules
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "describe.redact must be an array of strings".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("describe.redact must be an array of strings".to_owned()),
    };
    let max_extractions = match describe.get("max_extractions") {
        None => 20,
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                "describe.max_extractions must be an unsigned 32-bit integer".to_owned()
            })?,
    };
    let category_overrides = parse_category_overrides(describe.get("categories"))?;
    Ok(DescribeSettings {
        winnow,
        redact_rules,
        max_extractions,
        category_overrides,
    })
}
