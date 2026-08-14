// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::registry::{MaintBodyContext, MaintBodyResult};
use serde_json::{Map, Value, json};
use solstone_core_system::schedule::{ScheduleMutation, mutate_schedule_entries};
use std::fs;

const JOURNAL_SERVICE_COMMANDS: &[&str] = &[
    "backfill-processing-records",
    "backup",
    "brain",
    "config",
    "convey",
    "cortex",
    "depict",
    "describe",
    "down",
    "engage",
    "export",
    "facet-candidates",
    "grab",
    "health",
    "heartbeat",
    "identity",
    "importer",
    "indexer",
    "install-models",
    "install-provider",
    "journal-stats",
    "maint",
    "maintenance",
    "navigate",
    "observer",
    "reprocess",
    "restart-convey",
    "schedule",
    "segment",
    "sense",
    "service",
    "settings",
    "setup",
    "spl",
    "start",
    "streams",
    "supervisor",
    "talent",
    "think",
    "top",
    "transcribe",
    "transfer",
    "up",
    "warm",
];

#[derive(Default)]
struct ScheduleSummary {
    discovered: usize,
    rewritten: usize,
    preserved: usize,
    removed: usize,
    matched: usize,
    installed: bool,
    preserved_brain: bool,
}

fn schedules_path(c: &MaintBodyContext<'_>) -> std::path::PathBuf {
    c.journal.join("config/schedules.json")
}

fn schedule_result(result: Result<ScheduleSummary, impl std::fmt::Display>) -> MaintBodyResult {
    match result {
        Ok(summary) => MaintBodyResult {
            stdout: vec![
                "Summary".into(),
                format!("  discovered: {}", summary.discovered),
                format!("  rewritten:  {}", summary.rewritten),
                format!("  preserved:  {}", summary.preserved),
                format!("  removed:    {}", summary.removed),
            ],
            exit_code: 0,
        },
        Err(error) => MaintBodyResult {
            stdout: vec![error.to_string()],
            exit_code: 1,
        },
    }
}

pub fn migrate_dream_to_think_schedules(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    let path = schedules_path(c);
    if !path.exists() {
        return success(
            "Summary\n  discovered: 0\n  rewritten:  0\n  preserved:  0\n  errors:     0\n  skipped:    no file",
        );
    }
    schedule_result(mutate_schedule_entries(&path, |raw| {
        let mut summary = ScheduleSummary::default();
        for value in raw.values_mut() {
            let Some(entry) = value.as_object_mut() else {
                summary.preserved += 1;
                continue;
            };
            let Some(cmd) = entry.get_mut("cmd").and_then(Value::as_array_mut) else {
                summary.preserved += 1;
                continue;
            };
            if cmd.len() >= 2 && cmd[0] == "sol" && cmd[1] == "dream" {
                summary.discovered += 1;
                summary.rewritten += 1;
                if !c.dry_run {
                    cmd[0] = json!("journal");
                    cmd[1] = json!("think");
                }
            } else {
                summary.preserved += 1;
            }
        }
        ScheduleMutation {
            changed: !c.dry_run && summary.rewritten > 0,
            value: summary,
        }
    }))
}

pub fn migrate_sol_service_schedules(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    let path = schedules_path(c);
    if !path.exists() {
        return success(
            "Summary\n  discovered: 0\n  rewritten:  0\n  preserved:  0\n  errors:     0\n  skipped:    no file",
        );
    }
    schedule_result(mutate_schedule_entries(&path, |raw| {
        let mut summary = ScheduleSummary::default();
        for value in raw.values_mut() {
            let Some(entry) = value.as_object_mut() else {
                summary.preserved += 1;
                continue;
            };
            let Some(cmd) = entry.get_mut("cmd").and_then(Value::as_array_mut) else {
                summary.preserved += 1;
                continue;
            };
            if cmd.len() < 2 || cmd[0] != "sol" {
                summary.preserved += 1;
                continue;
            }
            let Some(verb) = cmd[1].as_str().map(str::to_owned) else {
                summary.preserved += 1;
                continue;
            };
            let sync_import = verb == "import" && cmd.iter().any(|part| part == "--sync");
            if sync_import || JOURNAL_SERVICE_COMMANDS.contains(&verb.as_str()) {
                summary.discovered += 1;
                summary.rewritten += 1;
                if !c.dry_run {
                    cmd[0] = json!("journal");
                    if sync_import {
                        cmd[1] = json!("importer");
                    }
                }
            } else {
                summary.preserved += 1;
            }
        }
        ScheduleMutation {
            changed: !c.dry_run && summary.rewritten > 0,
            value: summary,
        }
    }))
}

fn string_cmd(entry: Option<&Value>) -> Option<Vec<&str>> {
    entry?
        .as_object()?
        .get("cmd")?
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect()
}

fn is_provider_check(name: &str, value: &Value) -> bool {
    if matches!(name, "providers" | "providers-check") {
        return true;
    }
    let Some(cmd) = string_cmd(Some(value)) else {
        return false;
    };
    cmd.len() >= 3
        && matches!(cmd[0], "journal" | "sol")
        && cmd[1] == "providers"
        && cmd[2] == "check"
}

pub fn migrate_provider_check_schedule(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    let path = schedules_path(c);
    let result = mutate_schedule_entries(&path, |raw| {
        let matches = raw
            .iter()
            .filter(|(name, value)| is_provider_check(name, value))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Map<_, _>>();
        let brain_current =
            string_cmd(raw.get("brain")).as_deref() == Some(&["journal", "brain", "refresh"]);
        let mut summary = ScheduleSummary {
            matched: matches.len(),
            preserved_brain: brain_current,
            ..ScheduleSummary::default()
        };
        let source = matches
            .get("providers")
            .or_else(|| matches.get("providers-check"))
            .or_else(|| matches.iter().next().map(|(_, value)| value));
        let cadence = source
            .and_then(Value::as_object)
            .and_then(|entry| entry.get("every"))
            .and_then(Value::as_str)
            .unwrap_or("daily");
        let enabled = if matches.values().any(|value| {
            value.as_object().and_then(|entry| entry.get("enabled")) == Some(&Value::Bool(false))
        }) {
            false
        } else {
            source
                .and_then(Value::as_object)
                .and_then(|entry| entry.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(true)
        };
        if !brain_current {
            summary.installed = true;
            if !c.dry_run {
                raw.insert(
                    "brain".into(),
                    json!({"cmd":["journal","brain","refresh"],"every":cadence,"enabled":enabled,"max_runtime":"5m"}),
                );
            }
        }
        let names = matches
            .keys()
            .filter(|name| name.as_str() != "brain")
            .cloned()
            .collect::<Vec<_>>();
        summary.removed = names.len();
        if !c.dry_run {
            for name in names {
                raw.remove(&name);
            }
        }
        ScheduleMutation {
            changed: !c.dry_run && (summary.installed || summary.removed > 0),
            value: summary,
        }
    });
    let Ok(mut summary) = result else {
        return schedule_result(result);
    };
    let cleanup = cleanup_provider_health(c, &mut summary);
    if let Err(error) = cleanup {
        return MaintBodyResult {
            stdout: vec![error],
            exit_code: 1,
        };
    }
    MaintBodyResult {
        stdout: vec![
            "Summary".into(),
            format!("  matched:        {}", summary.matched),
            format!("  installed:      {}", summary.installed),
            format!("  preserved:      {}", summary.preserved_brain),
            format!("  removed:        {}", summary.removed),
        ],
        exit_code: 0,
    }
}

fn cleanup_provider_health(
    c: &MaintBodyContext<'_>,
    summary: &mut ScheduleSummary,
) -> Result<(), String> {
    let health = c.journal.join("health");
    let mut candidates = [
        "talents.json",
        "talents.json.lock",
        "recheck.lock",
        "providers.log",
    ]
    .iter()
    .map(|name| health.join(name))
    .collect::<Vec<_>>();
    if let Ok(entries) = fs::read_dir(&health) {
        candidates.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name().is_some_and(|name| {
                        let name = name.to_string_lossy();
                        name.starts_with(".talents.json.") && name.ends_with(".tmp")
                    })
                }),
        );
    }
    for path in candidates {
        if path.symlink_metadata().is_ok() && !c.dry_run {
            fs::remove_file(&path)
                .map_err(|error| format!("delete failed: {}: {error}", path.display()))?;
            summary.removed += 0;
        }
    }
    Ok(())
}

pub fn remove_granola_sync_schedule(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    let path = schedules_path(c);
    if !path.exists() {
        return success(
            "Summary\n  removed:   0\n  preserved: 0\n  errors:    0\n  skipped:   no file",
        );
    }
    let result = mutate_schedule_entries(&path, |raw| {
        let mut summary = ScheduleSummary::default();
        let retired = raw.get("sync:granola").is_some_and(|value| {
            let Some(entry) = value.as_object() else {
                return false;
            };
            let Some(cmd_value) = entry.get("cmd") else {
                return true;
            };
            let Some(cmd) = cmd_value.as_array() else {
                return false;
            };
            let Some(tokens) = cmd.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
                return false;
            };
            tokens.len() >= 2
                && matches!(tokens[0], "journal" | "sol")
                && matches!(tokens[1], "import" | "importer")
                && tokens.iter().enumerate().any(|(index, token)| {
                    *token == "--sync=granola"
                        || (*token == "--sync" && tokens.get(index + 1) == Some(&"granola"))
                })
        });
        if retired {
            summary.removed = 1;
            if !c.dry_run {
                raw.remove("sync:granola");
            }
        } else if raw.contains_key("sync:granola") {
            summary.preserved = 1;
        }
        ScheduleMutation {
            changed: !c.dry_run && retired,
            value: summary,
        }
    });
    match result {
        Ok(summary) => MaintBodyResult {
            stdout: vec![
                "Summary".into(),
                format!("  removed:   {}", summary.removed),
                format!("  preserved: {}", summary.preserved),
                "  errors:    0".into(),
            ],
            exit_code: 0,
        },
        Err(error) => MaintBodyResult {
            stdout: vec![error.to_string()],
            exit_code: 1,
        },
    }
}

fn success(line: &str) -> MaintBodyResult {
    MaintBodyResult {
        stdout: line.lines().map(str::to_owned).collect(),
        exit_code: 0,
    }
}
pub fn migrate_agent_run_logs(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_talents::migrate_agent_run_logs(c.journal, c.dry_run) {
        Ok(r) => MaintBodyResult {
            stdout: vec![format!("Migrated {} agent run log(s).", r.moved)],
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
pub fn rename_agents_to_talents(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_talents::rename_agents_to_talents(c.journal, c.dry_run) {
        Ok(r) if r.collisions == 0 => MaintBodyResult {
            stdout: vec![format!("Renamed {} agents path(s).", r.moved)],
            exit_code: 0,
        },
        Ok(r) => MaintBodyResult {
            stdout: vec![format!(
                "Refused {} destination collision(s).",
                r.collisions
            )],
            exit_code: 2,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
pub fn migrate_agent_layout(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_segment::migrate_agent_layout(c.journal, c.dry_run) {
        Ok(r) => MaintBodyResult {
            stdout: vec![
                "Migration complete".into(),
                format!("  moved:   {}", r.moved),
                format!("  cleaned: {}", r.cleaned),
                format!("  skipped: {}", r.skipped),
                format!("  errors:  {}", r.errors),
            ],
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
pub fn migrate_chronicle(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    let report = match solstone_core_segment::migrate_root_days_to_chronicle(c.journal, c.dry_run) {
        Ok(report) => report,
        Err(e) => {
            return MaintBodyResult {
                stdout: vec![e.to_string()],
                exit_code: 1,
            };
        }
    };
    if !report.requires_index_cleanup() {
        return MaintBodyResult {
            stdout: vec![format!("Migrated {} day directory(ies).", report.moved)],
            exit_code: 0,
        };
    }
    // The index names pre-migration paths now, so it is deleted only after the
    // day directories actually moved — never on a no-op or a dry run.
    match solstone_core_indexer_store::migrations::index_stream::remove_legacy_index_artifacts(
        c.journal,
    ) {
        Ok(removal) => MaintBodyResult {
            stdout: vec![
                "Migration complete".into(),
                format!("  moved:          {}", report.moved),
                format!("  merged:         {}", report.merged),
                format!("  skipped:        {}", report.skipped),
                format!("  sqlite_deleted: {}", removal.deleted()),
            ],
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
pub fn retired_unified_triage_providers(_: &MaintBodyContext<'_>) -> MaintBodyResult {
    MaintBodyResult {
        stdout: vec!["Skipped retired migration.".into()],
        exit_code: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;
    #[test]
    fn retired_body_is_unconditional() {
        let context = MaintBodyContext {
            journal: Path::new("/unused"),
            dry_run: false,
            verbose: false,
            task_name: Some("sol:006_rename_unified_triage_providers"),
        };
        assert_eq!(retired_unified_triage_providers(&context).exit_code, 0);
    }

    #[test]
    fn schedule_migrations_rewrite_install_and_remove_only_matching_entries() {
        let journal = tempdir().unwrap();
        fs::create_dir_all(journal.path().join("config")).unwrap();
        fs::write(
            journal.path().join("config/schedules.json"),
            serde_json::to_vec(&json!({
                "dream":{"cmd":["sol","dream","daily"],"every":"daily","custom":1},
                "service":{"cmd":["sol","health","--json"],"every":"hourly"},
                "providers-check":{"cmd":["sol","providers","check"],"every":"weekly","enabled":false},
                "provider-alias":{"cmd":["journal","providers","check","--force"],"every":"daily"},
                "sync:granola":{"cmd":["sol","import","--sync","granola"],"every":"daily"},
                "owner":{"cmd":["sol","custom"],"every":"daily"}
            })).unwrap(),
        ).unwrap();
        let context = MaintBodyContext {
            journal: journal.path(),
            dry_run: false,
            verbose: false,
            task_name: None,
        };
        assert_eq!(migrate_dream_to_think_schedules(&context).exit_code, 0);
        assert_eq!(migrate_sol_service_schedules(&context).exit_code, 0);
        assert_eq!(migrate_provider_check_schedule(&context).exit_code, 0);
        assert_eq!(remove_granola_sync_schedule(&context).exit_code, 0);
        let stored: Value = serde_json::from_slice(
            &fs::read(journal.path().join("config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["dream"]["cmd"], json!(["journal", "think", "daily"]));
        assert_eq!(
            stored["service"]["cmd"],
            json!(["journal", "health", "--json"])
        );
        assert_eq!(stored["owner"]["cmd"], json!(["sol", "custom"]));
        assert_eq!(
            stored["brain"]["cmd"],
            json!(["journal", "brain", "refresh"])
        );
        assert_eq!(stored["brain"]["every"], "weekly");
        assert_eq!(stored["brain"]["enabled"], false);
        assert!(stored.get("providers-check").is_none());
        assert!(stored.get("provider-alias").is_none());
        assert!(stored.get("sync:granola").is_none());
    }
}
