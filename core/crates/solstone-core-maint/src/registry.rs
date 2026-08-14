// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_system_health::MaintTaskDefinition;

/// One static one-time journal migration.
#[derive(Clone, Copy)]
pub struct MaintTask {
    pub app: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub retry_on_next_start: bool,
    pub blocks_supervisor_start: bool,
    pub body: MaintBody,
}

impl MaintTask {
    pub fn qualified_name(self) -> String {
        format!("{}:{}", self.app, self.name)
    }

    pub const fn definition(self) -> MaintTaskDefinition<'static> {
        MaintTaskDefinition {
            app: self.app,
            task: self.name,
            description: self.description,
            retry_on_next_start: self.retry_on_next_start,
            blocks_supervisor_start: self.blocks_supervisor_start,
        }
    }
}

pub type MaintBody = for<'a> fn(&MaintBodyContext<'a>) -> MaintBodyResult;

/// Context supplied to one native migration body.
pub struct MaintBodyContext<'a> {
    pub journal: &'a Path,
    pub dry_run: bool,
    pub verbose: bool,
    pub task_name: Option<&'a str>,
}

/// Captured task-body output and process-style exit code.
pub struct MaintBodyResult {
    pub stdout: Vec<String>,
    pub exit_code: i32,
}

const EXPECTED_QUALIFIED_NAMES: [&str; 27] = [
    "activities:000_migrate_activity_icon_to_emoji",
    "sol:000_migrate_agent_layout",
    "observer:000_migrate_remote_to_observer",
    "thinking:000_unify_provider_config",
    "settings:001_backfill_streams",
    "sol:001_migrate_agent_run_logs",
    "thinking:001_migrate_provider_install_state",
    "entities:001_migrate_to_journal_entities",
    "sol:002_migrate_chronicle",
    "timeline:002_migrate_rollup_schedules",
    "thinking:002_pin_google_model_aliases",
    "timeline:002_register_segment_summary_model",
    "settings:002_restructure_stream_dirs",
    "search:003_migrate_index_stream",
    "settings:003_seed_default_app_navigation",
    "settings:004_backfill_import_manifests",
    "search:004_migrate_topic_to_agent",
    "sol:004_rename_agents_to_talents",
    "sol:005_migrate_dream_to_think_schedules",
    "settings:005_pin_curation_nav",
    "settings:006_drop_services_nav",
    "sol:006_rename_unified_triage_providers",
    "settings:007_migrate_pdf_extractions",
    "sol:007_migrate_sol_service_schedules",
    "settings:008_migrate_pairing_home_address",
    "sol:008_migrate_provider_check_schedule",
    "sol:009_remove_granola_sync_schedule",
];

const TASKS: [MaintTask; 27] = [
    task(
        "activities",
        "000_migrate_activity_icon_to_emoji",
        "Migrate legacy custom activity glyphs from icon to emoji.",
        false,
        false,
    ),
    task(
        "sol",
        "000_migrate_agent_layout",
        "Migrate agent output files to the new agents/ directory layout.",
        false,
        false,
    ),
    task(
        "observer",
        "000_migrate_remote_to_observer",
        "Migrate remote observer data and config to observer naming.",
        false,
        false,
    ),
    task(
        "thinking",
        "000_unify_provider_config",
        "Collapse legacy thinking provider routing into one active brain profile.",
        false,
        false,
    ),
    task(
        "settings",
        "001_backfill_streams",
        "Backfill stream.json markers into all journal segments.",
        false,
        false,
    ),
    task(
        "sol",
        "001_migrate_agent_run_logs",
        "Migrate agent run logs from flat to per-agent subdirectory layout.",
        false,
        false,
    ),
    task(
        "thinking",
        "001_migrate_provider_install_state",
        "Move provider install truth to provider-owned status and manifest records.",
        true,
        true,
    ),
    task(
        "entities",
        "001_migrate_to_journal_entities",
        "Migrate entities from legacy facet-scoped format to journal-wide structure.",
        false,
        false,
    ),
    task(
        "sol",
        "002_migrate_chronicle",
        "Migrate root day directories into chronicle/.",
        false,
        false,
    ),
    task(
        "timeline",
        "002_migrate_rollup_schedules",
        "Remove legacy timeline rollup schedule entries superseded by journal maintenance routines.",
        false,
        false,
    ),
    task(
        "thinking",
        "002_pin_google_model_aliases",
        "Pin byte-exact Google model aliases in thinking provider config.",
        true,
        true,
    ),
    task(
        "timeline",
        "002_register_segment_summary_model",
        "Retired provider-context registration for timeline segment summary.",
        false,
        false,
    ),
    task(
        "settings",
        "002_restructure_stream_dirs",
        "Restructure journal to day/stream/segment/ directory layout.",
        false,
        false,
    ),
    task(
        "search",
        "003_migrate_index_stream",
        "Add stream column to journal search index.",
        false,
        false,
    ),
    task(
        "settings",
        "003_seed_default_app_navigation",
        "Seed default Convey app navigation config.",
        false,
        false,
    ),
    task(
        "settings",
        "004_backfill_import_manifests",
        "Backfill byte-hash dedup manifests for pre-dedup audio/text imports.",
        false,
        false,
    ),
    task(
        "search",
        "004_migrate_topic_to_agent",
        "Migrate journal event/stats keys from topic naming to agent naming.",
        false,
        false,
    ),
    task(
        "sol",
        "004_rename_agents_to_talents",
        "Rename live journal agents paths to talents.",
        false,
        false,
    ),
    task(
        "sol",
        "005_migrate_dream_to_think_schedules",
        "Rewrite stale `sol dream` schedule commands to `journal think`.",
        false,
        false,
    ),
    task(
        "settings",
        "005_pin_curation_nav",
        "Pin the curation app into existing Convey nav configs.",
        false,
        false,
    ),
    task(
        "settings",
        "006_drop_services_nav",
        "Remove the dissolved services app from existing Convey nav configs.",
        false,
        false,
    ),
    task(
        "sol",
        "006_rename_unified_triage_providers",
        "Retired provider-context rename for the chat refactor.",
        false,
        false,
    ),
    task(
        "settings",
        "007_migrate_pdf_extractions",
        "Migrate legacy segment PDF extraction JSONL into document transcripts.",
        false,
        false,
    ),
    task(
        "sol",
        "007_migrate_sol_service_schedules",
        "Heal stale sol-surface schedule commands to the journal service surface.",
        false,
        false,
    ),
    task(
        "settings",
        "008_migrate_pairing_home_address",
        "Migrate legacy pairing host URLs to bare home addresses.",
        false,
        false,
    ),
    task(
        "sol",
        "008_migrate_provider_check_schedule",
        "Migrate legacy provider-check schedules to active-brain refresh.",
        true,
        false,
    ),
    task(
        "sol",
        "009_remove_granola_sync_schedule",
        "Remove the retired sync:granola schedule entry.",
        true,
        false,
    ),
];

const fn task(
    app: &'static str,
    name: &'static str,
    description: &'static str,
    retry_on_next_start: bool,
    blocks_supervisor_start: bool,
) -> MaintTask {
    MaintTask {
        app,
        name,
        description,
        retry_on_next_start,
        blocks_supervisor_start,
        body: body_dispatch,
    }
}

fn body_dispatch(context: &MaintBodyContext<'_>) -> MaintBodyResult {
    match context.task_name {
        Some("activities:000_migrate_activity_icon_to_emoji") => {
            crate::bodies::activities::migrate_activity_icon_to_emoji(context)
        }
        Some("observer:000_migrate_remote_to_observer") => {
            crate::bodies::observer::migrate_remote_to_observer(context)
        }
        Some("entities:001_migrate_to_journal_entities") => {
            crate::bodies::entities::migrate_to_journal_entities(context)
        }
        Some("thinking:000_unify_provider_config") => {
            crate::bodies::thinking::unify_provider_config(context)
        }
        Some("thinking:001_migrate_provider_install_state") => {
            crate::bodies::thinking::migrate_provider_install_state(context)
        }
        Some("thinking:002_pin_google_model_aliases") => {
            crate::bodies::thinking::pin_google_model_aliases(context)
        }
        Some("sol:000_migrate_agent_layout") => crate::bodies::sol::migrate_agent_layout(context),
        Some("sol:001_migrate_agent_run_logs") => {
            crate::bodies::sol::migrate_agent_run_logs(context)
        }
        Some("sol:002_migrate_chronicle") => crate::bodies::sol::migrate_chronicle(context),
        Some("sol:004_rename_agents_to_talents") => {
            crate::bodies::sol::rename_agents_to_talents(context)
        }
        Some("sol:006_rename_unified_triage_providers") => {
            crate::bodies::sol::retired_unified_triage_providers(context)
        }
        Some("sol:005_migrate_dream_to_think_schedules") => {
            crate::bodies::sol::migrate_dream_to_think_schedules(context)
        }
        Some("sol:007_migrate_sol_service_schedules") => {
            crate::bodies::sol::migrate_sol_service_schedules(context)
        }
        Some("sol:008_migrate_provider_check_schedule") => {
            crate::bodies::sol::migrate_provider_check_schedule(context)
        }
        Some("sol:009_remove_granola_sync_schedule") => {
            crate::bodies::sol::remove_granola_sync_schedule(context)
        }
        Some("timeline:002_migrate_rollup_schedules") => {
            crate::bodies::timeline::migrate_rollup_schedules(context)
        }
        Some("timeline:002_register_segment_summary_model") => {
            crate::bodies::timeline::retired_segment_summary_model(context)
        }
        Some("settings:001_backfill_streams") => crate::bodies::settings::backfill_streams(context),
        Some("settings:002_restructure_stream_dirs") => {
            crate::bodies::settings::restructure_stream_dirs(context)
        }
        Some("settings:003_seed_default_app_navigation") => {
            crate::bodies::settings::seed_default_app_navigation(context)
        }
        Some("settings:007_migrate_pdf_extractions") => {
            crate::bodies::settings::migrate_pdf_extractions(context)
        }
        Some("settings:004_backfill_import_manifests") => {
            crate::bodies::settings::backfill_import_manifests(context)
        }
        Some("search:004_migrate_topic_to_agent") => {
            crate::bodies::search::migrate_topic_to_agent(context)
        }
        Some("search:003_migrate_index_stream") => {
            crate::bodies::search::migrate_index_stream(context)
        }
        Some("settings:005_pin_curation_nav") => {
            crate::bodies::settings::pin_curation_navigation(context)
        }
        Some("settings:006_drop_services_nav") => {
            crate::bodies::settings::drop_services_navigation(context)
        }
        Some("settings:008_migrate_pairing_home_address") => {
            crate::bodies::settings::migrate_pairing_home_address(context)
        }
        _ => MaintBodyResult {
            stdout: vec!["unknown native maintenance task".to_owned()],
            exit_code: 2,
        },
    }
}

pub fn tasks() -> &'static [MaintTask] {
    &TASKS
}

pub fn task_definitions() -> Vec<MaintTaskDefinition<'static>> {
    tasks().iter().copied().map(MaintTask::definition).collect()
}

pub fn get_task_by_name(name: &str) -> Option<MaintTask> {
    if name.contains(':') {
        return tasks()
            .iter()
            .copied()
            .find(|task| task.qualified_name() == name);
    }
    let mut matches = tasks().iter().copied().filter(|task| task.name == name);
    let task = matches.next()?;
    matches.next().is_none().then_some(task)
}

/// Validate the fixed census so tests prove additions, removals, and metadata
/// drift cannot silently pass.
pub fn validate_task_census(tasks: &[MaintTask]) -> Result<(), &'static str> {
    if tasks.len() != 27 {
        return Err("maint task census must contain 27 tasks");
    }
    let names = tasks
        .iter()
        .map(|task| task.qualified_name())
        .collect::<Vec<_>>();
    if names
        .iter()
        .map(String::as_str)
        .ne(EXPECTED_QUALIFIED_NAMES)
    {
        return Err("maint task qualified names differ from the fixed census");
    }
    if tasks
        .windows(2)
        .any(|pair| (pair[0].name, pair[0].app) > (pair[1].name, pair[1].app))
    {
        return Err("maint task census must be sorted by task name then app");
    }
    let retry = tasks
        .iter()
        .filter(|task| task.retry_on_next_start)
        .map(|task| task.qualified_name())
        .collect::<Vec<_>>();
    if retry
        != [
            "thinking:001_migrate_provider_install_state",
            "thinking:002_pin_google_model_aliases",
            "sol:008_migrate_provider_check_schedule",
            "sol:009_remove_granola_sync_schedule",
        ]
    {
        return Err("maint retry-on-next-start flags differ from the fixed census");
    }
    let blocks = tasks
        .iter()
        .filter(|task| task.blocks_supervisor_start)
        .map(|task| task.qualified_name())
        .collect::<Vec<_>>();
    if blocks
        != [
            "thinking:001_migrate_provider_install_state",
            "thinking:002_pin_google_model_aliases",
        ]
    {
        return Err("maint blocks-supervisor-start flags differ from the fixed census");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_registry_has_every_qualified_name_in_python_sort_order() {
        validate_task_census(tasks()).expect("fixed registry validates");
        assert_eq!(tasks().len(), 27);
        assert_eq!(
            tasks()[0].qualified_name(),
            "activities:000_migrate_activity_icon_to_emoji"
        );
        assert_eq!(
            tasks()[26].qualified_name(),
            "sol:009_remove_granola_sync_schedule"
        );
        assert!(get_task_by_name("002_migrate_rollup_schedules").is_some());
        assert!(get_task_by_name("002_register_segment_summary_model").is_some());
        assert!(get_task_by_name("002").is_none());
    }

    #[test]
    fn census_validation_falsifies_removal_and_misflagging() {
        let mut missing = tasks().to_vec();
        missing.pop();
        assert!(validate_task_census(&missing).is_err());

        let mut misflagged = tasks().to_vec();
        misflagged[0].retry_on_next_start = true;
        assert!(validate_task_census(&misflagged).is_err());

        let mut renamed = tasks().to_vec();
        renamed[0].name = "000_replaced";
        assert!(validate_task_census(&renamed).is_err());
    }

    #[test]
    fn every_registered_body_dispatches_without_a_placeholder() {
        let temporary = tempfile::tempdir().expect("temporary journal");
        for task in tasks() {
            let qualified = task.qualified_name();
            let result = (task.body)(&MaintBodyContext {
                journal: temporary.path(),
                dry_run: true,
                verbose: false,
                task_name: Some(&qualified),
            });
            assert_ne!(result.stdout, ["not yet implemented"], "{qualified}");
            assert_ne!(
                result.stdout,
                ["unknown native maintenance task"],
                "{qualified}"
            );
        }
    }
}
