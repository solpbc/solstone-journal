// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Desktop CLI adapter for `journal brain` commands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jiff::Timestamp;
use serde_json::{Map, Value};
use solstone_core_brain_owner::{
    BundledRuntimePrerequisiteAssessment, OwnerProbe, OwnerRefreshOptions, OwnerRefreshOutcome,
    OwnerView, ProductionSppPrerequisite, human_text, json_text, owner_exit_code, owner_view,
};
use solstone_core_cli::JournalBrainOwnerCommand;
use solstone_core_generate::{ClientError, GenerateRequest, GenerateResponse, OneShotClient};

use crate::{EXIT_UNAVAILABLE, resolve_journal_config_path};

struct DesktopProbe;

impl OwnerProbe for DesktopProbe {
    fn bundled_runtime_fingerprint(
        &self,
        journal: &Path,
        config: &Map<String, Value>,
    ) -> Option<String> {
        solstone_core_brain_owner::bundled_runtime_fingerprint(journal, config)
    }

    fn assess_bundled_runtime(
        &self,
        journal: &Path,
        expected: Option<&str>,
    ) -> BundledRuntimePrerequisiteAssessment {
        solstone_core_brain_owner::assess_bundled_runtime(journal, expected)
    }

    fn execute_generate(
        &self,
        request: &GenerateRequest,
        _journal: &Path,
        _config: &Map<String, Value>,
        cancellation: &solstone_core_generate::Cancellation,
    ) -> Result<GenerateResponse, ClientError> {
        let _ = cancellation;
        let _ = solstone_core_generate::Cancellation::never();
        OneShotClient::sibling().and_then(|client| client.execute(request))
    }
}

pub(crate) fn run(command: JournalBrainOwnerCommand) -> ExitCode {
    match command {
        JournalBrainOwnerCommand::Status { json } => run_status(json),
        JournalBrainOwnerCommand::Refresh(options) => {
            let Ok((journal, _)) = journal() else {
                return ExitCode::from(EXIT_UNAVAILABLE);
            };
            let outcome = solstone_core_brain_owner::refresh(
                &journal,
                &OwnerRefreshOptions {
                    expected_fingerprint: options.expected_fingerprint.clone(),
                    expected_active_fingerprint: options.expected_active_fingerprint,
                    expect_active_fingerprint_absent: options.expect_active_fingerprint_absent,
                },
                &DesktopProbe,
                &ProductionSppPrerequisite,
                &|| false,
            );
            handle_outcome(outcome, options.json)
        }
        JournalBrainOwnerCommand::RenewPrerequisites {
            json,
            expected_fingerprint,
        } => {
            let Ok((journal, _)) = journal() else {
                return ExitCode::from(EXIT_UNAVAILABLE);
            };
            let outcome = solstone_core_brain_owner::renew_prerequisites(
                &journal,
                expected_fingerprint.as_deref(),
                &DesktopProbe,
                &ProductionSppPrerequisite,
                &|| false,
            );
            handle_outcome(outcome, json)
        }
        _ => ExitCode::from(2),
    }
}

fn journal() -> Result<(PathBuf, Map<String, Value>), ExitCode> {
    let line = resolve_journal_config_path(None).map_err(|error| {
        crate::eprint_journal_path_error(error);
        ExitCode::from(EXIT_UNAVAILABLE)
    })?;
    let config = solstone_core_brain::read_journal_config(&line.path)
        .map_err(|error| {
            eprintln!("thinking check failed: could not read journal config: {error}");
            ExitCode::from(EXIT_UNAVAILABLE)
        })?
        .config
        .unwrap_or_default();
    Ok((line.path, config))
}

fn run_status(json_output: bool) -> ExitCode {
    let Ok((journal, config)) = journal() else {
        return ExitCode::from(EXIT_UNAVAILABLE);
    };
    let view = owner_view(&journal, &config, Timestamp::now());
    render(&view, json_output);
    ExitCode::from(owner_exit_code(&view))
}

fn handle_outcome(outcome: OwnerRefreshOutcome, json_output: bool) -> ExitCode {
    match outcome {
        OwnerRefreshOutcome::OperationalFailure { message } => {
            eprintln!("{message}");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
        OwnerRefreshOutcome::PersistedReady { view }
        | OwnerRefreshOutcome::PersistedNonReady { view, .. }
        | OwnerRefreshOutcome::Busy { view }
        | OwnerRefreshOutcome::StaleExpectedFingerprint { view }
        | OwnerRefreshOutcome::LostFence { view } => {
            render(&view, json_output);
            ExitCode::from(owner_exit_code(&view))
        }
        OwnerRefreshOutcome::Cancelled => ExitCode::from(EXIT_UNAVAILABLE),
    }
}

fn render(view: &OwnerView, json_output: bool) {
    if json_output {
        println!("{}", json_text(view));
    } else {
        println!("{}", human_text(view));
    }
}
