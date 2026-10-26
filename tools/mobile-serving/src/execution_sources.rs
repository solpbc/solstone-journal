// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed source recurrence over the maintained legacy and owner aliases.

use super::*;
use solstone_core_system::schedule::{enabled_schedule_entry, validate_schedule_entry};
use solstone_core_system::{partition::journal_alias_form, request::TaskArgv};

pub(super) struct SourceOperation {
    pub source: String,
    pub action: SourceAction,
}
pub(super) enum SourceAction {
    Status,
    Put {
        every: String,
        enabled: bool,
        max_runtime: Option<String>,
    },
    Remove,
    SetEnabled(bool),
}

fn names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    if matches!(source, "plaud" | "obsidian") {
        names.push(format!("sync:{source}"));
    }
    names.push(format!("owner:source:{source}"));
    names
}

fn is_source(entry: &Value, source: &str) -> bool {
    let Some(command) = entry.get("cmd") else {
        return false;
    };
    let Ok(command) = serde_json::from_value::<Vec<String>>(command.clone()) else {
        return false;
    };
    if !matches!(
        TaskArgv::from_wire(command.clone()),
        Ok(TaskArgv::Importer(_))
    ) {
        return false;
    }
    let alias = journal_alias_form(&command);
    solstone_core_import_host::call::classify_arguments(&alias[2..])
        .is_ok_and(|class| class.sync_source.as_deref() == Some(source))
}

fn status(raw: &serde_json::Map<String, Value>, source: &str) -> Value {
    let mut enabled = false;
    let mut entries = Vec::new();
    let mut blocked = Vec::new();
    for name in names(source) {
        if let Some(entry) = raw.get(&name) {
            if is_source(entry, source) {
                let active = enabled_schedule_entry(entry).is_some();
                enabled |= active;
                entries.push(json!({"name":name,"entry":entry,"active":active}));
            } else {
                blocked.push(name);
            }
        }
    }
    json!({"source":source,"enabled":enabled,"entries":entries,"blocked_names":blocked})
}

pub(super) fn execute(
    root: &Arc<JournalRoot>,
    request: SourceOperation,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> (StatusCode, Value) {
    let source = request.source;
    let Some(command) = schedules::source_command(&source) else {
        return (
            StatusCode::BAD_REQUEST,
            json!({"error":"recurrence_invalid"}),
        );
    };
    schedules::mutate(root, cancelled, move |raw| {
        apply(raw, &source, &request.action, &command)
    })
}

fn apply(
    raw: &mut serde_json::Map<String, Value>,
    source: &str,
    action: &SourceAction,
    command: &[String],
) -> (StatusCode, Value) {
    match action {
        SourceAction::Status => (StatusCode::OK, status(raw, source)),
        SourceAction::Remove => {
            let mut removed = Vec::new();
            for name in names(source) {
                if raw.get(&name).is_some_and(|entry| is_source(entry, source)) {
                    raw.remove(&name);
                    removed.push(name);
                }
            }
            let mut result = status(raw, source);
            result["removed"] = json!(!removed.is_empty());
            result["removed_names"] = json!(removed);
            (StatusCode::OK, result)
        }
        SourceAction::Put {
            every,
            enabled,
            max_runtime,
        } => {
            let aliases = names(source);
            let name = aliases
                .iter()
                .find(|name| raw.get(*name).is_some_and(|entry| is_source(entry, source)))
                .or_else(|| aliases.iter().find(|name| !raw.contains_key(*name)));
            let Some(name) = name else {
                return (StatusCode::CONFLICT, json!({"error":"recurrence_conflict"}));
            };
            let mut entry = raw
                .get(name)
                .cloned()
                .unwrap_or_else(|| json!({"cmd":command}));
            entry["every"] = json!(every);
            entry["enabled"] = json!(enabled);
            // A cadence card does not implicitly discard an existing bound.
            if let Some(max) = max_runtime {
                entry["max_runtime"] = json!(max);
            }
            if validate_schedule_entry(&entry).is_err() {
                return (
                    StatusCode::BAD_REQUEST,
                    json!({"error":"recurrence_invalid"}),
                );
            }
            for alias in &aliases {
                if alias != name
                    && let Some(duplicate) = raw.get_mut(alias)
                    && is_source(duplicate, source)
                {
                    duplicate["enabled"] = json!(false);
                }
            }
            raw.insert(name.clone(), entry);
            (StatusCode::OK, status(raw, source))
        }
        SourceAction::SetEnabled(false) => {
            for name in names(source) {
                if let Some(entry) = raw.get_mut(&name)
                    && is_source(entry, source)
                {
                    entry["enabled"] = json!(false);
                }
            }
            (StatusCode::OK, status(raw, source))
        }
        SourceAction::SetEnabled(true) => {
            let every = names(source)
                .iter()
                .find_map(|name| raw.get(name).filter(|entry| is_source(entry, source)))
                .map(|entry| {
                    entry
                        .get("every")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let every = match every {
                Some(Some(every)) => every,
                Some(None) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        json!({"error":"recurrence_invalid"}),
                    );
                }
                None => "hourly".into(),
            };
            apply(
                raw,
                source,
                &SourceAction::Put {
                    every,
                    enabled: true,
                    max_runtime: None,
                },
                command,
            )
        }
    }
}

pub(super) fn settings(
    root: &Arc<JournalRoot>,
    updates: Option<Vec<(&'static str, bool)>>,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> (StatusCode, Value) {
    let config = match solstone_core_journal_config::read_journal_config_bound(root) {
        Ok(config) => config.config.unwrap_or_default(),
        Err(error) => {
            log::warn!("owner sync settings: {error}");
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error":"schedules_unavailable"}),
            );
        }
    };
    let available = solstone_core_settings_web::source_sync_plaud_available(&config);
    let mut changed_fields = serde_json::Map::new();
    let result = schedules::mutate(root, cancelled, |raw| {
        let mut working = raw.clone();
        if let Some(updates) = &updates {
            for (source, enabled) in updates {
                let before = working.clone();
                let (code, result) = apply(
                    &mut working,
                    source,
                    &SourceAction::SetEnabled(*enabled),
                    &schedules::source_command(source).expect("parsed Settings source"),
                );
                if !code.is_success() {
                    return (code, result);
                }
                if working != before {
                    changed_fields.insert(format!("{source}.enabled"), json!(enabled));
                }
            }
        }
        *raw = working;
        let mut result = serde_json::Map::new();
        for source in ["plaud", "obsidian"] {
            let source_status = status(raw, source);
            result.insert(
                source.into(),
                json!({
                    "available":source != "plaud" || available,
                    "enabled":source_status["enabled"],
                    "configured":!source_status["entries"].as_array().unwrap().is_empty(),
                    "blocked_names":source_status["blocked_names"],
                }),
            );
        }
        (StatusCode::OK, Value::Object(result))
    });
    if result.0.is_success() && !changed_fields.is_empty() {
        let checkpoint = || {
            if cancelled() {
                Err("execution stopped".to_owned())
            } else {
                Ok(())
            }
        };
        if let Err(error) = solstone_core_facets::append_journal_action_log_at_root_checked(
            root,
            "app",
            "settings",
            "sync_update",
            json!({"changed_fields":changed_fields}),
            &checkpoint,
        ) {
            log::warn!("owner sync settings audit: {error}");
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error":"schedules_unavailable"}),
            );
        }
    }
    result
}
