// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only native implementation of `journal talent` commands.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::Path;
use std::time::SystemTime;

use serde_json::Value;

mod args;
mod compose;
mod facets_context;
mod inventory;
pub mod preview;
mod schema;
mod templates;

pub use compose::{compose_talent, compose_talent_instruction};
pub use preview::{PreviewRequest, PromptPreview, PromptPreviewRefusal, PromptPreviewer};
pub use templates::safe_substitute;

mod emit;
mod last_run;
mod list;
mod log;
mod logs;
mod runs;
mod show;

#[derive(Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionFacts {
    pub talent_type: Option<String>,
    pub declared_cwd: Option<String>,
    pub timeout_seconds: Option<u64>,
}

pub fn resolve_execution_facts(
    name: &str,
    talent_root: &Path,
    apps_root: &Path,
    journal_root: &Path,
    templates_dir: &Path,
    focused_facet: Option<&str>,
) -> Result<Option<ExecutionFacts>, String> {
    let configs = solstone_core_talent_config::discover(talent_root, apps_root)?;
    let Some(config) = configs.iter().find(|config| config.key == name) else {
        return Ok(None);
    };
    let declared_cwd = config
        .metadata
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let composed = compose::compose_talent(config, journal_root, templates_dir, focused_facet)?;
    Ok(Some(ExecutionFacts {
        talent_type: composed
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        declared_cwd,
        timeout_seconds: composed.get("timeout_seconds").and_then(Value::as_u64),
    }))
}

pub fn run_cli(
    args: &[OsString],
    talent_root: &Path,
    apps_root: &Path,
    journal_root: &Path,
    now: SystemTime,
    previewer: &dyn preview::PromptPreviewer,
) -> CliRun {
    match args::parse(args) {
        args::Command::Help(text) => success(text),
        args::Command::Error(text) => CliRun {
            stdout: String::new(),
            stderr: text,
            exit_code: 2,
        },
        args::Command::Show(options) => {
            show::run(talent_root, apps_root, journal_root, &options, previewer)
        }
        args::Command::Log(options) => log::run_log(&journal_root.join("talents"), &options),
        args::Command::Inventory(options) => {
            match inventory::run(talent_root, apps_root, journal_root, &options) {
                Ok(output) => success(output),
                Err(error) => CliRun {
                    stdout: String::new(),
                    stderr: format!("{error}\n"),
                    exit_code: 1,
                },
            }
        }
        args::Command::Logs(options) => {
            let mut config_loader = || load_configs(talent_root, apps_root, journal_root, None);
            logs::run_logs(
                journal_root,
                &options,
                now,
                std::io::stdout().is_terminal(),
                &mut config_loader,
            )
        }
        args::Command::List(options) => match load_configs(
            talent_root,
            apps_root,
            journal_root,
            options.schedule.as_deref(),
        ) {
            Ok(configs) if options.json => success(emit::jsonl(&configs, &options)),
            Ok(configs) => success(list::render(&configs, &options, journal_root, now)),
            Err(error) => CliRun {
                stdout: String::new(),
                stderr: format!("{error}\n"),
                exit_code: 1,
            },
        },
    }
}

fn load_configs(
    talent_root: &Path,
    apps_root: &Path,
    journal_root: &Path,
    schedule: Option<&str>,
) -> Result<Vec<solstone_core_talent_config::TalentConfig>, String> {
    let overrides = solstone_core_talent_config::read_talent_overrides(journal_root)?;
    solstone_core_talent_config::load_talent_configs(
        talent_root,
        apps_root,
        overrides.as_ref(),
        solstone_core_talent_config::TalentFilter {
            r#type: None,
            schedule,
            include_disabled: true,
        },
    )
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn roots() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("talent")).expect("talent root");
        fs::create_dir_all(root.path().join("apps/demo/talent")).expect("app root");
        root
    }

    fn run(root: &tempfile::TempDir, args: &[&str]) -> CliRun {
        run_cli(
            &args.iter().map(OsString::from).collect::<Vec<_>>(),
            &root.path().join("talent"),
            &root.path().join("apps"),
            root.path(),
            UNIX_EPOCH + Duration::from_secs(1_000),
            &preview::UnreachablePreviewer,
        )
    }

    #[test]
    fn resolve_execution_facts_keeps_raw_cwd_distinct_from_composer_default() {
        let root = roots();
        fs::create_dir_all(root.path().join("think/templates")).expect("templates");
        fs::write(
            root.path().join("talent/declared.md"),
            "{\n\"type\": \"cogitate\",\n\"cwd\": \"journal\",\n\"timeout_seconds\": 42\n}\nbody\n",
        )
        .expect("declared talent");
        fs::write(
            root.path().join("talent/defaulted.md"),
            "{\n\"type\": \"cogitate\"\n}\nbody\n",
        )
        .expect("defaulted talent");

        let declared = resolve_execution_facts(
            "declared",
            &root.path().join("talent"),
            &root.path().join("apps"),
            root.path(),
            &root.path().join("think/templates"),
            None,
        )
        .expect("resolve")
        .expect("declared facts");
        assert_eq!(declared.talent_type.as_deref(), Some("cogitate"));
        assert_eq!(declared.declared_cwd.as_deref(), Some("journal"));
        assert_eq!(declared.timeout_seconds, Some(42));

        let defaulted = resolve_execution_facts(
            "defaulted",
            &root.path().join("talent"),
            &root.path().join("apps"),
            root.path(),
            &root.path().join("think/templates"),
            None,
        )
        .expect("resolve")
        .expect("defaulted facts");
        assert_eq!(defaulted.talent_type.as_deref(), Some("cogitate"));
        assert_eq!(defaulted.declared_cwd, None);
        assert_eq!(defaulted.timeout_seconds, None);
    }

    #[test]
    fn resolve_execution_facts_returns_none_for_unknown_name() {
        let root = roots();
        fs::create_dir_all(root.path().join("think/templates")).expect("templates");
        assert_eq!(
            resolve_execution_facts(
                "missing",
                &root.path().join("talent"),
                &root.path().join("apps"),
                root.path(),
                &root.path().join("think/templates"),
                None,
            )
            .expect("resolve"),
            None
        );
    }

    #[test]
    fn plain_prompt_is_discovered_and_json_uses_python_spacing() {
        let root = roots();
        fs::write(root.path().join("talent/plain.md"), "prompt body\n").expect("prompt");
        let output = run(&root, &["list", "--json"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert_eq!(
            output.stdout,
            "{\"file\": \"talent/plain.md\", \"color\": \"#6c757d\", \"source\": \"system\"}\n"
        );
    }

    // §7 criteria 1, 2, and 4: JSONL is the consumer observable; it omits path/mtime.
    #[test]
    fn consumer_conformance_table_and_crlf_equality() {
        const CASES: [(&str, &str, bool); 11] = [
            (
                "lf",
                "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\nbody",
                true,
            ),
            (
                "leading_blank",
                "\n{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\n}\nbody",
                true,
            ),
            ("unclosed", "{\n\"type\":\"generate\"\nbody", false),
            (
                "crlf",
                "{\r\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50\r\n}\r\nbody",
                true,
            ),
            ("opening_space", "{ \n\"type\":\"generate\"\n}\nbody", false),
            (
                "nested_column_zero",
                "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50,\n\"nested\": {\n\"x\":1\n}\n}\nbody",
                false,
            ),
            (
                "nested_indented",
                "{\n\"type\":\"generate\",\"output\":\"md\",\"schedule\":\"daily\",\"priority\":50,\n\"nested\": {\n\"x\":1\n }\n}\nbody",
                true,
            ),
            ("invalid", "{\n\"type\": generate\n}\nbody", false),
            ("none", "body", false),
            ("empty", "", false),
            ("array", "[\"generate\"]\nbody", false),
        ];
        for (name, contents, has_metadata) in CASES {
            let root = roots();
            fs::write(root.path().join("talent/case.md"), contents).unwrap();
            let output = run(&root, &["list", "--json"]);
            if matches!(name, "nested_column_zero" | "invalid") {
                assert_ne!(output.exit_code, 0, "{name}");
            } else {
                assert_eq!(output.exit_code, 0, "{name}: {}", output.stderr);
                assert_eq!(output.stdout.contains("\"type\""), has_metadata, "{name}");
            }
        }
        let lf = roots();
        let crlf = roots();
        fs::write(lf.path().join("talent/case.md"), CASES[0].1).unwrap();
        fs::write(crlf.path().join("talent/case.md"), CASES[3].1).unwrap();
        assert_eq!(
            run(&lf, &["list", "--json"]).stdout,
            run(&crlf, &["list", "--json"]).stdout
        );
    }

    #[test]
    fn list_renders_weekly_without_widening_schedule_grammar() {
        let root = roots();
        fs::write(
            root.path().join("talent/weekly.md"),
            "{\n\"title\": \"Weekly\",\n\"schedule\": \"weekly\",\n\"priority\": 1\n}\n",
        )
        .expect("prompt");
        let output = run(&root, &[]);
        assert!(output.stdout.contains("weekly:\n  weekly"));
        let invalid = run(&root, &["list", "--schedule", "weekly"]);
        assert_eq!(invalid.exit_code, 2);
        assert!(invalid.stderr.contains("invalid choice"));
    }

    #[test]
    fn list_width_uses_the_rendered_filter_set() {
        let root = roots();
        fs::write(
            root.path().join("talent/very_long_segment.md"),
            "{\n\"schedule\": \"segment\",\n\"priority\": 1\n}\n",
        )
        .expect("segment prompt");
        fs::write(
            root.path().join("talent/daily.md"),
            "{\n\"schedule\": \"daily\",\n\"priority\": 1\n}\n",
        )
        .expect("daily prompt");
        let output = run(&root, &["list", "--schedule", "daily"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.starts_with("  NAME        TITLE"));
    }

    #[test]
    fn malformed_metadata_fails_without_partial_output() {
        let root = roots();
        fs::write(
            root.path().join("talent/bad.md"),
            "{\n\"title\": true,\n}\n",
        )
        .expect("prompt");
        let output = run(&root, &["list"]);
        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("bad.md"));
    }

    #[test]
    fn log_dispatches_parser_and_not_found_outcomes() {
        let root = roots();
        let missing_id = run(&root, &["log"]);
        assert_eq!(missing_id.exit_code, 2);
        assert_eq!(
            missing_id.stderr,
            "usage: journal talent log [-h] [--json] [--full] id\njournal talent log: error: the following arguments are required: id\n"
        );
        let missing_run = run(&root, &["log", "synthetic-id"]);
        assert_eq!(
            missing_run,
            CliRun {
                stdout: String::new(),
                stderr: "Talent run not found: synthetic-id\n".to_owned(),
                exit_code: 1,
            }
        );
    }

    #[test]
    fn overrides_preserve_order_and_only_apply_supported_fields() {
        let root = roots();
        fs::write(
            root.path().join("talent/system.md"),
            "{\n\"type\": \"cogitate\",\n\"title\": \"System\",\n\"color\": \"#111111\"\n}\n",
        )
        .expect("system prompt");
        fs::write(
            root.path().join("apps/demo/talent/app.md"),
            "{\n\"type\": \"cogitate\",\n\"title\": \"App\"\n}\n",
        )
        .expect("app prompt");
        fs::create_dir_all(root.path().join("config")).expect("config");
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"talent_overrides":{"talent.system.system":{"disabled":true,"extract":true,"title":"ignored"},"talent.demo.app":{"extract":true},"talent.system.demo:app":{"disabled":true}}}"#,
        )
        .expect("overrides");
        let output = run(&root, &["list", "--json", "--disabled"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stdout.contains(r##"{"file": "talent/system.md", "type": "cogitate", "title": "System", "color": "#111111", "source": "system", "disabled": true, "extract": true, "access_tier": "normal", "cwd": "journal"}"##));
        assert!(output.stdout.contains(r##"{"file": "apps/demo/talent/app.md", "type": "cogitate", "title": "App", "color": "#6c757d", "source": "app", "app": "demo", "extract": true, "access_tier": "normal", "cwd": "journal"}"##));
    }

    #[test]
    fn disabled_override_shows_hint_or_disabled_tag() {
        let root = roots();
        fs::write(
            root.path().join("talent/visible.md"),
            "{\n\"title\": \"Visible\"\n}\n",
        )
        .expect("visible prompt");
        fs::write(
            root.path().join("talent/hidden.md"),
            "{\n\"title\": \"Hidden\"\n}\n",
        )
        .expect("hidden prompt");
        fs::create_dir_all(root.path().join("config")).expect("config");
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"talent_overrides":{"talent.system.hidden":{"disabled":true}}}"#,
        )
        .expect("overrides");

        let hidden = run(&root, &["list"]);
        assert_eq!(hidden.exit_code, 0, "{}", hidden.stderr);
        assert!(
            hidden
                .stdout
                .contains("1 prompts (1 disabled hidden, use --disabled)\n")
        );
        assert!(!hidden.stdout.contains("  hidden"));

        let included = run(&root, &["list", "--disabled"]);
        assert_eq!(included.exit_code, 0, "{}", included.stderr);
        assert!(included.stdout.contains("hidden"));
        assert!(included.stdout.contains("disabled"));
        assert!(!included.stdout.contains("disabled hidden, use --disabled"));
    }

    #[test]
    fn synthetic_records_keep_json_types_and_metadata_key_order() {
        let root = roots();
        fs::write(
            root.path().join("talent/rich.md"),
            "{\n\"type\": \"cogitate\",\n\"title\": \"Mañana — plan\",\n\"color\": \"#123456\",\n\"items\": [\"one\", 2],\n\"nested\": {\"enabled\": true},\n\"max_run_cost_usd\": 5.00\n}\n",
        )
        .expect("rich prompt");
        fs::write(
            root.path().join("talent/first.md"),
            "{\n\"type\": \"cogitate\",\n\"title\": \"First\",\n\"color\": \"#010101\"\n}\n",
        )
        .expect("first prompt");
        fs::write(
            root.path().join("talent/second.md"),
            "{\n\"type\": \"cogitate\",\n\"color\": \"#010101\",\n\"title\": \"First\"\n}\n",
        )
        .expect("second prompt");
        let output = run(&root, &["list", "--json"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        let rows = output.stdout.lines().collect::<Vec<_>>();
        assert!(rows.contains(&r##"{"file": "talent/rich.md", "type": "cogitate", "title": "Ma\u00f1ana \u2014 plan", "color": "#123456", "items": ["one", 2], "nested": {"enabled": true}, "max_run_cost_usd": 5.0, "source": "system", "access_tier": "normal", "cwd": "journal"}"##));
        let first = rows
            .iter()
            .find(|row| row.contains("talent/first.md"))
            .expect("first row");
        let second = rows
            .iter()
            .find(|row| row.contains("talent/second.md"))
            .expect("second row");
        assert_ne!(first, second);
        assert!(first.find("\"title\"").expect("title") < first.find("\"color\"").expect("color"));
        assert!(
            second.find("\"color\"").expect("color") < second.find("\"title\"").expect("title")
        );
    }

    #[test]
    fn validation_families_return_reference_messages() {
        let cases = [
            (
                "priority",
                "{\n\"schedule\": \"daily\"\n}\n",
                "Scheduled prompt 'priority' is missing required 'priority' field. All prompts with 'schedule' must declare an explicit priority.",
            ),
            (
                "type",
                "{\n\"type\": \"invalid\"\n}\n",
                "Prompt 'type' has invalid type 'invalid'. Expected 'generate' or 'cogitate'.",
            ),
            (
                "activity",
                "{\n\"schedule\": \"activity\",\n\"priority\": 1\n}\n",
                "Activity-scheduled prompt 'activity' must have a non-empty 'activities' list (activity types to match, or [\"*\"] for all types).",
            ),
            (
                "write",
                "{\n\"type\": \"cogitate\",\n\"write\": true\n}\n",
                "Prompt 'write' declares unsupported 'write: true' (cogitate runs are read-only)",
            ),
            (
                "access",
                "{\n\"type\": \"generate\",\n\"output\": \"json\",\n\"access_tier\": \"normal\"\n}\n",
                "Prompt 'access' sets 'access_tier' but access_tier is only valid for type: cogitate",
            ),
            (
                "cwd",
                "{\n\"type\": \"generate\",\n\"output\": \"json\",\n\"cwd\": \"somewhere\"\n}\n",
                "Prompt 'cwd' sets 'cwd' but cwd is only valid for type: cogitate",
            ),
        ];
        for (name, content, expected) in cases {
            let root = roots();
            fs::write(root.path().join(format!("talent/{name}.md")), content).expect("prompt");
            let output = run(&root, &["list"]);
            assert_eq!(output.exit_code, 1);
            assert_eq!(output.stderr, format!("{expected}\n"));
        }
    }

    #[test]
    fn inventory_keeps_per_talent_compose_errors_in_successful_output() {
        let root = roots();
        fs::write(
            root.path().join("talent/broken.md"),
            "{\n\"type\": \"cogitate\",\n\"access_tier\": \"invalid\"\n}\nbroken\n",
        )
        .expect("broken prompt");
        fs::write(
            root.path().join("talent/healthy.md"),
            "{\n\"type\": \"cogitate\"\n}\nhealthy\n",
        )
        .expect("healthy prompt");
        let output = run(&root, &["inventory"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert!(output.stderr.is_empty());
        assert!(output.stdout.contains("broken"));
        assert!(output.stdout.contains("ERROR:"));
        assert!(output.stdout.contains("  healthy"));
        assert_eq!(output.stdout.matches("ERROR:").count(), 1);
    }

    #[test]
    fn inventory_help_matches_the_parser_contract_exactly() {
        let root = roots();
        let output = run(&root, &["inventory", "--help"]);
        assert_eq!(output.exit_code, 0, "{}", output.stderr);
        assert_eq!(output.stdout, args::INVENTORY_HELP);
    }

    #[test]
    fn real_corpus_list_fixtures_and_json_records_match() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repository = manifest.ancestors().nth(3).expect("repository root");
        let payload = repository.join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT);
        let journal = tempfile::tempdir().expect("journal");
        let talent_root = payload.join("solstone/talent");
        let apps_root = payload.join("solstone/apps");
        let args = |items: &[&str]| items.iter().map(OsString::from).collect::<Vec<_>>();

        let discovered = solstone_core_talent_config::discover(&talent_root, &apps_root)
            .expect("discover corpus");

        let json = run_cli(
            &args(&["list", "--json"]),
            &talent_root,
            &apps_root,
            journal.path(),
            UNIX_EPOCH,
            &preview::UnreachablePreviewer,
        );
        assert_eq!(json.exit_code, 0, "{}", json.stderr);
        let records = json.stdout.lines().collect::<Vec<_>>();
        assert_eq!(records.len(), discovered.len());
        assert!(
            json.stdout
                .find("apps/entities/talent/detection.md")
                .expect("detection")
                < json.stdout.find("talent/event.md").expect("event")
        );
        for expected in [
            r##"{"file": "talent/conversation.md", "type": "generate", "title": "Conversation Story", "description": "Generates a conversation story, topics, and structured commitments, closures, decisions, and relations to merge onto the activity record.", "color": "#00796b", "schedule": "activity", "activities": ["meeting", "call", "messaging", "email"], "priority": 20, "output": "json", "max_output_tokens": 12288, "schema": "story.schema.json", "hook": {"post": "story"}, "degradation_check": true, "load": {"transcripts": true, "percepts": true, "talents": false}, "source": "system"}"##,
            r##"{"file": "talent/partner.md", "type": "cogitate", "access_tier": "synthesis", "title": "your profile", "description": "a weekly profile updated with evidence from the past 7 days: dated entries, repeated topics, recorded interactions, and decisions. your journal is always private, only yours.", "schedule": "weekly", "priority": 95, "max_turns": 100, "color": "#6c757d", "source": "system", "cwd": "journal"}"##,
            r##"{"file": "talent/weekly_reflection.md", "type": "cogitate", "access_tier": "synthesis", "title": "Weekly Reflection", "description": "Sunday-start weekly reflection synthesized from the journal", "schedule": "weekly", "priority": 90, "output": "md", "degradation_check": true, "read_scope_span": 7, "max_turns": 100, "max_run_cost_usd": 5.0, "color": "#6c757d", "source": "system", "cwd": "journal"}"##,
            r##"{"file": "apps/entities/talent/entity_assist.md", "type": "cogitate", "title": "Entity Assistant", "description": "Quick entity addition with intelligent type detection and automatic description generation", "color": "#00695c", "group": "Entities", "source": "app", "app": "entities", "access_tier": "normal", "cwd": "journal"}"##,
        ] {
            assert!(
                records.contains(&expected),
                "missing JSON record: {expected}"
            );
        }

        let list = run_cli(
            &args(&["list"]),
            &talent_root,
            &apps_root,
            journal.path(),
            UNIX_EPOCH,
            &preview::UnreachablePreviewer,
        );
        assert_eq!(list.exit_code, 0, "{}", list.stderr);
        assert_eq!(list.stdout, LIST_FIXTURE);
        assert_eq!(
            run_cli(
                &args(&["list", "--schedule", "daily"]),
                &talent_root,
                &apps_root,
                journal.path(),
                UNIX_EPOCH,
                &preview::UnreachablePreviewer
            )
            .stdout,
            DAILY_FIXTURE
        );
        assert_eq!(
            run_cli(
                &args(&["list", "--schedule", "activity", "--source", "app"]),
                &talent_root,
                &apps_root,
                journal.path(),
                UNIX_EPOCH,
                &preview::UnreachablePreviewer
            )
            .stdout,
            "No prompts found matching filters.\n"
        );
    }

    const DAILY_FIXTURE: &str = concat!(
        "  NAME                      TITLE                         LAST RUN            TAGS\n\n",
        "  daily_schedule            Maintenance Window            -                   json pre post\n",
        "  entities:entities_review  Entity Reviewer               -                   json pre post [entities]\n",
        "  entities:entity_observer  Entity Observer               -                   json pre post [entities]\n",
        "  facet_newsletter          Facet Newsletter Generator    -                   md pre post\n",
        "  morning_briefing          Morning Briefing              -                   json pre\n",
        "  schedule                  Upcoming Schedule             -                   json post\n",
    );

    const LIST_FIXTURE: &str = concat!(
        "  NAME                      TITLE                         LAST RUN            TAGS\n\n",
        "segment:\n",
        "  documents                 Document Analysis             -                   json pre\n",
        "  entities:detection        Entity Detection              -                   json pre post [entities]\n",
        "  screen                    Screen Record                 -                   json\n",
        "  sense                     Segment Sense                 -                   json\n",
        "  speaker_attribution       Speaker Attribution           -                   json pre post\n",
        "  timeline:segment_summary  Segment Summary               -                   json pre post [timeline]\n\n",
        "daily:\n",
        "  daily_schedule            Maintenance Window            -                   json pre post\n",
        "  entities:entities_review  Entity Reviewer               -                   json pre post [entities]\n",
        "  entities:entity_observer  Entity Observer               -                   json pre post [entities]\n",
        "  facet_newsletter          Facet Newsletter Generator    -                   md pre post\n",
        "  morning_briefing          Morning Briefing              -                   json pre\n",
        "  schedule                  Upcoming Schedule             -                   json post\n\n",
        "weekly:\n",
        "  partner                   your profile                  -\n",
        "  weekly_reflection         Weekly Reflection             -                   md\n\n",
        "activity:\n",
        "  conversation              Conversation Story            -                   json post\n",
        "  event                     Event Story                   -                   json post\n",
        "  participation             Participation                 -                   json post\n",
        "  work                      Work Story                    -                   json post\n\n",
        "unscheduled:\n",
        "  entities:entity_assist    Entity Assistant              -                  [entities]\n",
        "  entities:entity_describe  Entity Description            -                   md pre [entities]\n",
        "  pulse                     Pulse                         -                   json pre post\n",
        "  steward                   Steward                       -                   json pre post\n\n",
    );
}
