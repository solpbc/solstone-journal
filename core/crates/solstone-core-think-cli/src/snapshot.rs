// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_talent_config::TalentConfig;
use std::path::Path;

pub fn compute_daily_evidence_revision(
    journal: &Path,
    day: &str,
    config: &TalentConfig,
    facet: Option<&str>,
    overrides: Option<&Map<String, Value>>,
) -> Result<(String, String), String> {
    solstone_core_indexer::daily_evidence::compute_daily_evidence_revision(
        journal,
        day,
        &config.key,
        &config.metadata,
        &config.body,
        facet,
        overrides,
    )
}

pub(crate) fn prepare_daily_packet(
    context: &crate::context::ThinkContext,
    config: &TalentConfig,
    facet: Option<&str>,
    extra: &Map<String, Value>,
) -> Result<Value, String> {
    use solstone_core_talent_runtime::{
        ExecutionContext,
        prepare::{PrepareMode, RuntimePaths},
    };
    let day = extra
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or(&context.day);
    let before = compute_daily_evidence_revision(&context.journal, day, config, facet, None)?;
    if extra.get("evidence_revision").and_then(Value::as_str) != Some(&before.0)
        || extra.get("contract_digest").and_then(Value::as_str) != Some(&before.1)
    {
        return Err("daily evidence changed before preparation".to_owned());
    }
    let mut request = Map::from_iter([
        ("name".to_owned(), Value::String(config.key.clone())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("schedule".to_owned(), Value::String("daily".to_owned())),
    ]);
    if let Some(facet) = facet {
        request.insert("facet".to_owned(), Value::String(facet.to_owned()));
    }
    request.extend(extra.clone());
    let paths = RuntimePaths {
        talent_root: context.talent_root.clone(),
        apps_root: context.apps_root.clone(),
        templates_dir: context
            .talent_root
            .parent()
            .ok_or("talent root has no parent")?
            .join("think/templates"),
    };
    let execution = ExecutionContext {
        journal: context.journal.clone(),
    };
    let prepared = solstone_core_talent_runtime::prepare::prepare(
        request,
        &paths,
        &execution,
        PrepareMode::Execute,
    )
    .map_err(|e| e.to_string())?;
    let packet = solstone_core_talent_runtime::daily_prepare::freeze(prepared, &execution)
        .map_err(|e| format!("daily input preparation: {e:?}"))?;
    let mut configs =
        solstone_core_talent_config::discover(&context.talent_root, &context.apps_root)?;
    let overrides = solstone_core_journal_config::read_journal_config(&context.journal)
        .map_err(|e| e.to_string())?
        .config
        .and_then(|v| {
            v.get("talent_overrides")
                .and_then(Value::as_object)
                .cloned()
        });
    solstone_core_talent_config::merge(&mut configs, overrides.as_ref());
    let current = configs
        .iter()
        .find(|c| c.key == config.key)
        .ok_or("daily talent removed during preparation")?;
    let after = compute_daily_evidence_revision(&context.journal, day, current, facet, None)?;
    if before != after {
        return Err("daily evidence or contract changed during preparation".to_owned());
    }
    Ok(packet)
}
