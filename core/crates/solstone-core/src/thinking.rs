// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `solstone-core thinking set-lane` mutation.

use std::path::Path;
use std::process::ExitCode;

use serde_json::{Map, Value};
use solstone_core_cli::ThinkingSetLaneOptions;
use solstone_core_thinking::MutationError;
use solstone_core_thinking::providers::{
    ProviderRequestError, ProviderUpdateError, resolve_provider_update, update_providers,
};

use crate::{
    EXIT_CANTCREAT, EXIT_DATAERR, EXIT_INTERNAL_FAILURE, EXIT_IOERR, EXIT_TEMPFAIL,
    EXIT_UNAVAILABLE, print_journal_error, resolve_journal_config_path,
};

struct SetLaneOutcome {
    exit: u8,
    stdout: String,
    stderr: String,
}

pub fn run(options: ThinkingSetLaneOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(options.journal_override.clone()) {
        Ok(journal) => journal.path,
        Err(error) => return print_journal_error(error),
    };
    let outcome = run_set_lane(&journal, &options);
    if !outcome.stdout.is_empty() {
        println!("{}", outcome.stdout);
    }
    if !outcome.stderr.is_empty() {
        eprintln!("{}", outcome.stderr);
    }
    ExitCode::from(outcome.exit)
}

fn run_set_lane(journal: &Path, options: &ThinkingSetLaneOptions) -> SetLaneOutcome {
    let mut request = Map::new();
    if let Some(provider) = &options.provider {
        request.insert("provider".to_owned(), Value::String(provider.clone()));
    }
    if let Some(model) = &options.model {
        request.insert("model".to_owned(), Value::String(model.clone()));
    }
    let update = match resolve_provider_update(journal, &options.lane, &request) {
        Ok(update) => update,
        Err(error) => {
            return SetLaneOutcome {
                exit: provider_request_error_exit(&error),
                stdout: String::new(),
                stderr: request_error_message(&error),
            };
        }
    };
    match update_providers(journal, update, Value::Null) {
        Ok(value) => SetLaneOutcome {
            exit: 0,
            stdout: value.to_string(),
            stderr: String::new(),
        },
        Err(error) => SetLaneOutcome {
            exit: provider_update_error_exit(&error),
            stdout: String::new(),
            stderr: update_error_message(&error),
        },
    }
}

fn provider_request_error_exit(error: &ProviderRequestError) -> u8 {
    match error {
        ProviderRequestError::InvalidInput(_) => EXIT_DATAERR,
        ProviderRequestError::InvalidState(_) => EXIT_CANTCREAT,
        ProviderRequestError::ConfigUnreadable(_) => EXIT_UNAVAILABLE,
    }
}

fn provider_update_error_exit(error: &ProviderUpdateError) -> u8 {
    match error {
        ProviderUpdateError::Confidential(_) => EXIT_CANTCREAT,
        ProviderUpdateError::Mutation(MutationError::ConfigLock(_)) => EXIT_TEMPFAIL,
        ProviderUpdateError::Mutation(MutationError::ConfigLoad(_) | MutationError::Read(_)) => {
            EXIT_UNAVAILABLE
        }
        ProviderUpdateError::Mutation(MutationError::ConfigWrite(_)) => EXIT_IOERR,
        ProviderUpdateError::Mutation(MutationError::ActionLog(_)) => EXIT_INTERNAL_FAILURE,
    }
}

fn request_error_message(error: &ProviderRequestError) -> String {
    match error {
        ProviderRequestError::InvalidInput(detail)
        | ProviderRequestError::InvalidState(detail)
        | ProviderRequestError::ConfigUnreadable(detail) => detail.clone(),
    }
}

fn update_error_message(error: &ProviderUpdateError) -> String {
    match error {
        ProviderUpdateError::Confidential(detail) => detail.clone(),
        ProviderUpdateError::Mutation(MutationError::ConfigLock(detail))
        | ProviderUpdateError::Mutation(MutationError::ConfigLoad(detail))
        | ProviderUpdateError::Mutation(MutationError::ConfigWrite(detail))
        | ProviderUpdateError::Mutation(MutationError::ActionLog(detail)) => detail.clone(),
        ProviderUpdateError::Mutation(MutationError::Read(error)) => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    use serde_json::{Value, json};
    use solstone_core_journal_config::ConfigLoadError;
    use tempfile::TempDir;

    use super::*;

    fn options(lane: &str, provider: Option<&str>, model: Option<&str>) -> ThinkingSetLaneOptions {
        ThinkingSetLaneOptions {
            lane: lane.to_owned(),
            provider: provider.map(ToOwned::to_owned),
            model: model.map(ToOwned::to_owned),
            journal_override: None,
        }
    }

    fn journal_with(config: Value) -> TempDir {
        let journal = TempDir::new().expect("temp journal");
        fs::create_dir_all(journal.path().join("config")).expect("config directory creates");
        fs::write(
            journal.path().join("config/journal.json"),
            serde_json::to_vec_pretty(&config).expect("config serializes"),
        )
        .expect("config writes");
        journal
    }

    fn config_bytes(journal: &Path) -> Vec<u8> {
        fs::read(journal.join("config/journal.json")).expect("config reads")
    }

    fn spp_active_config() -> Value {
        json!({
            "providers": {
                "active": {"provider": "local", "model": "private"},
                "local": {
                    "endpoint_url": "https://private.example/v1",
                    "served_model_id": "private",
                    "credential": "secret"
                }
            },
            "services": {
                "confidential": {
                    "endpoint_url": "https://private.example",
                    "served_model_id": "private",
                    "credential_fingerprint_sha256": "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"
                }
            }
        })
    }

    #[test]
    fn provider_request_error_exit_maps_all_variants() {
        assert_eq!(
            provider_request_error_exit(&ProviderRequestError::InvalidInput("x".to_owned())),
            EXIT_DATAERR
        );
        assert_eq!(
            provider_request_error_exit(&ProviderRequestError::InvalidState("x".to_owned())),
            EXIT_CANTCREAT
        );
        assert_eq!(
            provider_request_error_exit(&ProviderRequestError::ConfigUnreadable("x".to_owned())),
            EXIT_UNAVAILABLE
        );
    }

    #[test]
    fn provider_update_error_exit_maps_all_variants() {
        assert_eq!(
            provider_update_error_exit(&ProviderUpdateError::Confidential("x".to_owned())),
            EXIT_CANTCREAT
        );
        assert_eq!(
            provider_update_error_exit(&ProviderUpdateError::Mutation(MutationError::ConfigLock(
                "x".to_owned()
            ))),
            EXIT_TEMPFAIL
        );
        assert_eq!(
            provider_update_error_exit(&ProviderUpdateError::Mutation(MutationError::ConfigLoad(
                "x".to_owned()
            ))),
            EXIT_UNAVAILABLE
        );
        assert_eq!(
            provider_update_error_exit(&ProviderUpdateError::Mutation(MutationError::Read(
                ConfigLoadError::Corrupt {
                    path: PathBuf::from("/tmp/journal-config-test"),
                    source: Box::new(io::Error::other("test")),
                }
            ))),
            EXIT_UNAVAILABLE
        );
        assert_eq!(
            provider_update_error_exit(&ProviderUpdateError::Mutation(MutationError::ConfigWrite(
                "x".to_owned()
            ))),
            EXIT_IOERR
        );
        assert_eq!(
            provider_update_error_exit(&ProviderUpdateError::Mutation(MutationError::ActionLog(
                "x".to_owned()
            ))),
            EXIT_INTERNAL_FAILURE
        );
    }

    #[test]
    fn set_lane_local_writes_the_bundled_provider() {
        let journal = journal_with(json!({}));
        let outcome = run_set_lane(journal.path(), &options("local", None, None));
        assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
        let body: Value = serde_json::from_str(&outcome.stdout).expect("stdout JSON");
        assert_eq!(body["active"]["provider"], "local");
        let config = solstone_core_thinking::read_config(journal.path()).expect("config reads");
        assert_eq!(config["providers"]["active"]["provider"], "local");
    }

    #[test]
    fn set_lane_byo_anthropic_writes_active_and_remembered_model() {
        let journal = journal_with(json!({}));
        let outcome = run_set_lane(
            journal.path(),
            &options("byo", Some("anthropic"), Some("claude-sonnet-5")),
        );
        assert_eq!(outcome.exit, 0, "{}", outcome.stderr);
        let config = solstone_core_thinking::read_config(journal.path()).expect("config reads");
        assert_eq!(config["providers"]["active"]["provider"], "anthropic");
        assert_eq!(config["providers"]["active"]["model"], "claude-sonnet-5");
        assert_eq!(
            config["providers"]["byo_models"]["anthropic"],
            "claude-sonnet-5"
        );
    }

    #[test]
    fn set_lane_byo_while_spp_is_active_is_a_state_conflict() {
        let journal = journal_with(spp_active_config());
        let before = config_bytes(journal.path());
        let outcome = run_set_lane(
            journal.path(),
            &options("byo", Some("google"), Some("gemini-3.5-flash")),
        );
        assert_eq!(outcome.exit, EXIT_CANTCREAT);
        assert_eq!(
            outcome.stderr,
            "Turn off confidential thinking first, then switch your thinking provider."
        );
        assert_eq!(config_bytes(journal.path()), before);
    }

    #[test]
    fn set_lane_reports_unreadable_config() {
        let journal = journal_with(json!({}));
        fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup": {"completed_at": 17672256"#,
        )
        .expect("corrupt config writes");
        let outcome = run_set_lane(journal.path(), &options("local", None, None));
        assert_eq!(outcome.exit, EXIT_UNAVAILABLE);
        assert!(
            outcome.stderr.contains("your settings file at "),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn set_lane_rejects_invalid_lane_without_writing() {
        let journal = journal_with(json!({}));
        let before = config_bytes(journal.path());
        let outcome = run_set_lane(journal.path(), &options("nope", None, None));
        assert_eq!(outcome.exit, EXIT_DATAERR);
        assert_eq!(
            outcome.stderr,
            "Invalid lane: nope. Must be one of: byo, confidential, local"
        );
        assert_eq!(config_bytes(journal.path()), before);
    }

    #[test]
    fn set_lane_rejects_byo_without_a_provider_without_writing() {
        let journal = journal_with(json!({}));
        let before = config_bytes(journal.path());
        let outcome = run_set_lane(journal.path(), &options("byo", None, None));
        assert_eq!(outcome.exit, EXIT_DATAERR);
        assert_eq!(
            outcome.stderr,
            "No BYO provider selected. Must be one of: anthropic, google, local, openai"
        );
        assert_eq!(config_bytes(journal.path()), before);
    }

    #[test]
    fn set_lane_rejects_invalid_byo_provider_without_writing() {
        let journal = journal_with(json!({}));
        let before = config_bytes(journal.path());
        let outcome = run_set_lane(journal.path(), &options("byo", Some("nope"), None));
        assert_eq!(outcome.exit, EXIT_DATAERR);
        assert_eq!(
            outcome.stderr,
            "Invalid provider for BYO lane. Must be one of: anthropic, google, local, openai"
        );
        assert_eq!(config_bytes(journal.path()), before);
    }

    #[test]
    fn set_lane_rejects_local_when_an_endpoint_is_configured_without_writing() {
        let journal = journal_with(json!({
            "providers": {
                "local": {
                    "endpoint_url": "http://127.0.0.1:1/v1",
                    "served_model_id": "served-model"
                }
            }
        }));
        let before = config_bytes(journal.path());
        let outcome = run_set_lane(journal.path(), &options("local", None, None));
        assert_eq!(outcome.exit, EXIT_CANTCREAT);
        assert_eq!(
            outcome.stderr,
            "clear your endpoint URL first to run the bundled local model."
        );
        assert_eq!(config_bytes(journal.path()), before);
    }

    #[test]
    fn set_lane_rejects_local_when_confidential_is_provisioned_without_writing() {
        let journal = journal_with(json!({
            "services": {"confidential": {"endpoint_url": "https://private.example"}}
        }));
        let before = config_bytes(journal.path());
        let outcome = run_set_lane(journal.path(), &options("local", None, None));
        assert_eq!(outcome.exit, EXIT_CANTCREAT);
        assert_eq!(
            outcome.stderr,
            "Turn off confidential thinking first, then switch to the bundled local model."
        );
        assert_eq!(config_bytes(journal.path()), before);
    }
}
