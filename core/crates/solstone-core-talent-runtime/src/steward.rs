// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::contract::{CommitPlan, ParsedOutput, PrePostState};
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};
use nix::fcntl::{Flock, FlockArg};
use serde_json::{Map, Value, json};
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(test)]
thread_local! {
    static TEST_RENDERED_BODY: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

#[derive(Clone, Debug, PartialEq)]
pub struct StewardPreState {
    pub body: String,
    pub previous_summary: String,
    pub default_summary: Map<String, Value>,
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let path = context.journal.join("health/.steward.lock");
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return skip(
            prepared,
            "steward pre-hook failed: cannot create lock directory",
        );
    }
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(false);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options
        .open(&path)
        .map_err(|error| RuntimeOutcome::Skipped {
            stage: "steward".into(),
            talent: prepared.name.clone(),
            reason: format!("steward pre-hook failed: {error}"),
        })?;
    // Same file lock as core/src/identity/steward.rs:169; it spans gather, render,
    // and write, releases before generation, and its file is never unlinked.
    let _lock = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => lock,
        Err(_) => return skip(prepared, "steward already in flight"),
    };
    let day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or("19700101");
    let facts = crate::steward_health::gather_health_facts(&context.journal, day);
    let body = rendered_health_body(&facts);
    // Exactly two stages honor DRY_RUN_KEY (steward, speaker_attribution); not a general per-stage dry-run flag.
    if !prepared
        .config
        .get(crate::DRY_RUN_KEY)
        .is_some_and(|value| value == &Value::Bool(true))
    {
        if let Some(reason) = crate::steward_health::validate_steward_health(&body) {
            if let Err(error) = crate::steward_log::append_event(
                &context.journal,
                "render.failed",
                Map::from_iter([
                    ("outcome".into(), json!("render_failed")),
                    ("target".into(), json!("identity/health.md")),
                    ("detail".into(), json!(reason)),
                ]),
            ) {
                return skip(
                    prepared,
                    &format!("steward pre-hook failed to record render failure: {error}"),
                );
            }
        } else if let Err(error) = solstone_core_identity::write_identity(
            &context.journal,
            "health.md",
            "steward",
            "replace",
            None,
            &body,
            "steward synthesis",
        ) {
            return skip(prepared, &format!("steward pre-hook failed: {error}"));
        }
    }
    let default_summary = crate::steward_health::default_summary_from_body(&body);
    let previous_summary = crate::steward_health::load_previous_summary(&context.journal, day)
        .map(|summary| serde_json::to_string_pretty(&summary).unwrap_or_default())
        .unwrap_or_else(|| "(none — first run)".into());
    Ok(PrePostState::Steward(StewardPreState {
        body,
        previous_summary,
        default_summary,
    }))
}

fn rendered_health_body(facts: &Map<String, Value>) -> String {
    #[cfg(test)]
    if let Some(body) = TEST_RENDERED_BODY.with(|value| value.borrow().clone()) {
        return body;
    }
    crate::steward_health::render_health_body(facts)
}

fn skip(prepared: &PreparedTalent, reason: &str) -> Result<PrePostState, RuntimeOutcome> {
    Err(RuntimeOutcome::Skipped {
        stage: "steward".into(),
        talent: prepared.name.clone(),
        reason: reason.into(),
    })
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::Steward(state) = state else {
        return Err(stage_error(
            "prompt-override",
            "steward",
            prepared,
            "steward state is missing",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([
            ("health_state".into(), Value::String(state.body.clone())),
            (
                "previous_summary".into(),
                Value::String(state.previous_summary.clone()),
            ),
        ]),
    );
    Ok(())
}

pub fn parse(
    output: &str,
    _: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.into()))
}

pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    state: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let PrePostState::Steward(state) = state else {
        return Err(stage_error(
            "commit",
            "steward",
            prepared,
            "steward state is missing",
        ));
    };
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "steward",
            prepared,
            "steward output is not text",
        ));
    };
    let mut record = crate::steward_health::normalize_summary(&output, &state.default_summary);
    record.insert(
        "model".into(),
        prepared.config.get("model").cloned().unwrap_or(Value::Null),
    );
    Ok(CommitPlan::Write(
        crate::writers::WriteIntent::DayAccumulator {
            day: prepared
                .config
                .get("day")
                .and_then(Value::as_str)
                .unwrap_or("19700101")
                .into(),
            agent: "steward".into(),
            record,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use solstone_core_journal_io::{
        JournalRoot, MalformedPolicy,
        operational_log::{OplogFormat, catalog_oplogs},
        read_jsonl_with_report,
    };
    use std::fs;
    use std::os::unix::fs::OpenOptionsExt;

    fn prepared(dry_run: bool) -> PreparedTalent {
        let mut config = Map::from_iter([("day".into(), json!("20260101"))]);
        if dry_run {
            config.insert("dry_run".into(), json!(true));
        }
        PreparedTalent {
            name: "steward".into(),
            config,
        }
    }

    fn context(root: &tempfile::TempDir) -> ExecutionContext {
        ExecutionContext {
            journal: root.path().into(),
        }
    }

    fn set_rendered_body(body: Option<&str>) {
        TEST_RENDERED_BODY.with(|value| *value.borrow_mut() = body.map(str::to_owned));
    }

    #[test]
    fn criterion_12_identity_write_preserves_other_owner_content_and_dry_run() {
        let root = tempfile::tempdir().unwrap();
        let partner = root.path().join("identity/partner.md");
        fs::create_dir_all(partner.parent().unwrap()).unwrap();
        fs::write(&partner, b"owner-authored partner details\n").unwrap();
        let before = fs::read(&partner).unwrap();

        let state = build(&mut prepared(false), &context(&root)).unwrap();
        assert!(matches!(state, PrePostState::Steward(_)));
        assert!(root.path().join("identity/health.md").is_file());
        assert_eq!(fs::read(&partner).unwrap(), before);

        let dry_root = tempfile::tempdir().unwrap();
        build(&mut prepared(true), &context(&dry_root)).unwrap();
        assert!(!dry_root.path().join("identity/health.md").exists());
    }

    #[test]
    fn criterion_13_pre_state_reaches_post_fallback() {
        set_rendered_body(Some(
            "## Status\n<!-- generated_at: 2026-01-01T00:00:00Z -->\ndistinctive steward fallback\n\n## Needs your attention\n\n## Auto-repairs (last 7d)\n",
        ));
        let root = tempfile::tempdir().unwrap();
        let state = build(&mut prepared(false), &context(&root)).unwrap();
        set_rendered_body(None);

        let CommitPlan::Write(crate::writers::WriteIntent::DayAccumulator { record, .. }) = commit(
            ParsedOutput::Text("not json".into()),
            &prepared(false),
            &state,
        )
        .unwrap() else {
            panic!("steward post must create its accumulator write");
        };
        assert_eq!(record["summary_sentence"], "distinctive steward fallback");
    }

    #[test]
    fn criterion_20_rejected_health_is_non_fatal_and_logged_once() {
        let root = tempfile::tempdir().unwrap();
        let health = root.path().join("identity/health.md");
        fs::create_dir_all(health.parent().unwrap()).unwrap();
        fs::write(&health, b"owner health content\n").unwrap();
        let before = fs::read(&health).unwrap();
        set_rendered_body(Some("rejected body"));
        let result = build(&mut prepared(false), &context(&root));
        set_rendered_body(None);

        assert!(result.is_ok());
        assert_eq!(fs::read(&health).unwrap(), before);
        let today = Local::now().date_naive();
        let snapshot = catalog_oplogs(JournalRoot::open(root.path()).unwrap(), &[today]).unwrap();
        let entries = snapshot
            .entries()
            .iter()
            .filter(|entry| {
                entry.name().source().display_slug() == "steward"
                    && entry.name().run().display_slug() == "pre-hook"
                    && entry.name().format() == OplogFormat::Jsonl
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        let entry = entries[0];
        let report = read_jsonl_with_report::<Value>(
            root.path()
                .join("chronicle")
                .join(entry.day())
                .join("health")
                .join(entry.leaf()),
            Vec::new(),
            MalformedPolicy::Raise,
        )
        .unwrap();
        assert_eq!(report.records.len(), 1);
        let row = &report.records[0].value;
        assert_eq!(row["event"], "render.failed");
        assert!(row["ts"].as_i64().is_some_and(|ts| ts > 1_000_000_000_000));
    }

    #[test]
    fn rejected_health_invocations_create_distinct_intact_pre_hook_oplogs() {
        let root = tempfile::tempdir().unwrap();
        let health = root.path().join("identity/health.md");
        fs::create_dir_all(health.parent().unwrap()).unwrap();
        fs::write(&health, b"owner health content\n").unwrap();
        set_rendered_body(Some("rejected body"));
        let first = build(&mut prepared(false), &context(&root));
        let second = build(&mut prepared(false), &context(&root));
        set_rendered_body(None);

        assert!(first.is_ok());
        assert!(second.is_ok());
        let today = Local::now().date_naive();
        let snapshot = catalog_oplogs(JournalRoot::open(root.path()).unwrap(), &[today]).unwrap();
        let entries = snapshot
            .entries()
            .iter()
            .filter(|entry| {
                entry.name().source().display_slug() == "steward"
                    && entry.name().run().display_slug() == "pre-hook"
                    && entry.name().format() == OplogFormat::Jsonl
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0].leaf(), entries[1].leaf());
        for entry in entries {
            let report = read_jsonl_with_report::<Value>(
                root.path()
                    .join("chronicle")
                    .join(entry.day())
                    .join("health")
                    .join(entry.leaf()),
                Vec::new(),
                MalformedPolicy::Raise,
            )
            .unwrap();
            assert_eq!(report.records.len(), 1);
            assert_eq!(report.records[0].value["event"], "render.failed");
        }
    }

    #[test]
    fn rejected_health_oplog_failure_is_a_skip() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("chronicle"), b"not a directory").unwrap();
        set_rendered_body(Some("rejected body"));
        let result = build(&mut prepared(false), &context(&root));
        set_rendered_body(None);

        assert!(matches!(
            result,
            Err(RuntimeOutcome::Skipped { reason, .. })
                if reason.contains("failed to record render failure")
        ));
        assert!(!root.path().join("identity/health.md").exists());
    }

    #[test]
    fn criterion_21_file_lock_skips_and_remains() {
        let root = tempfile::tempdir().unwrap();
        let lock = root.path().join("health/.steward.lock");
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock)
            .unwrap();
        let _guard = Flock::lock(file, FlockArg::LockExclusiveNonblock).unwrap();
        let result = build(&mut prepared(false), &context(&root));
        assert!(
            matches!(result, Err(RuntimeOutcome::Skipped { reason, .. }) if reason == "steward already in flight")
        );
        assert!(!root.path().join("identity/health.md").exists());
        assert!(lock.exists());
    }

    #[test]
    fn criterion_5_build_writes_health_unless_dry_run() {
        let root = tempfile::tempdir().unwrap();
        let written = build(&mut prepared(false), &context(&root)).unwrap();
        assert!(matches!(written, PrePostState::Steward(_)));
        assert!(root.path().join("identity/health.md").is_file());

        let dry_root = tempfile::tempdir().unwrap();
        let previewed = build(&mut prepared(true), &context(&dry_root)).unwrap();
        assert!(matches!(previewed, PrePostState::Steward(_)));
        assert!(!dry_root.path().join("identity/health.md").exists());
    }

    #[test]
    fn criterion_8_steward_pre_failure_is_a_skip() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("health"), b"not a directory").unwrap();
        let result = build(&mut prepared(false), &context(&root));
        assert!(matches!(
            result,
            Err(RuntimeOutcome::Skipped { ref stage, ref talent, ref reason })
                if stage == "steward" && talent == "steward" && reason.contains("cannot create lock directory")
        ));
    }
}
