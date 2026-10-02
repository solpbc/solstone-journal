// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `solstone-core thinking set-lane` mutation.

use std::path::Path;
use std::process::ExitCode;

use serde_json::{Map, Value};
use solstone_core_cli::{
    ChatGptModelsOptions, ChatGptSignInOptions, ChatGptSignOutOptions, ChatGptStatusOptions,
    ThinkingSetLaneOptions,
};
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

pub trait IsTerminal {
    fn is_terminal(&self) -> bool;
}

impl<T: std::io::IsTerminal> IsTerminal for T {
    fn is_terminal(&self) -> bool {
        std::io::IsTerminal::is_terminal(self)
    }
}

pub fn read_pasted_line_if_terminal<R: std::io::BufRead + IsTerminal>(
    reader: &mut R,
) -> Option<String> {
    if !reader.is_terminal() {
        return None;
    }
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(n) if n > 0 => {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                Some(trimmed.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn run_chatgpt_sign_in(options: ChatGptSignInOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(options.journal_override) {
        Ok(journal) => journal.path,
        Err(error) => return print_journal_error(error),
    };

    let attempt = match solstone_core_thinking::chatgpt::begin_sign_in(&journal) {
        Ok(attempt) => attempt,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(EXIT_DATAERR);
        }
    };

    println!("{}", attempt.authorize_url);

    if !options.no_browser {
        let _ = solstone_core_thinking::chatgpt::open_browser_for_url(&attempt.authorize_url);
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let tx_clone = tx.clone();
    let j_clone = journal.clone();
    let attempt_clone = attempt.clone();

    // Loopback listener thread (waits up to 600 seconds)
    std::thread::spawn(move || {
        let transport = solstone_core_thinking::chatgpt::UreqTransport;
        let res = solstone_core_thinking::chatgpt::finish_sign_in(
            &j_clone,
            &transport,
            &attempt_clone,
            None,
            std::time::Duration::from_secs(600),
        );
        let _ = tx_clone.send(res);
    });

    // Terminal stdin thread
    if std::io::stdin().is_terminal() {
        let j_clone2 = journal.clone();
        let attempt_clone2 = attempt.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            if let Some(pasted) = read_pasted_line_if_terminal(&mut stdin) {
                let transport = solstone_core_thinking::chatgpt::UreqTransport;
                let res = solstone_core_thinking::chatgpt::finish_sign_in(
                    &j_clone2,
                    &transport,
                    &attempt_clone2,
                    Some(&pasted),
                    std::time::Duration::from_secs(30),
                );
                let _ = tx.send(res);
            }
        });
    }

    let finish_result = rx
        .recv()
        .unwrap_or(Err(solstone_core_thinking::chatgpt::ClosedOutcome::Expired));

    match finish_result {
        Ok(_res) => {
            println!("signed in to ChatGPT successfully.");
            ExitCode::SUCCESS
        }
        Err(solstone_core_thinking::chatgpt::ClosedOutcome::RegistrationRefused)
        | Err(solstone_core_thinking::chatgpt::ClosedOutcome::PlanUsageNotGranted) => {
            eprintln!("ChatGPT sign-in required: run 'journal thinking chatgpt sign-in'");
            ExitCode::from(EXIT_DATAERR)
        }
        Err(solstone_core_thinking::chatgpt::ClosedOutcome::AccountMismatch) => {
            eprintln!("account mismatch: run 'journal thinking chatgpt sign-out --forget'");
            ExitCode::from(EXIT_DATAERR)
        }
        Err(outcome) => {
            eprintln!("ChatGPT sign-in failed: {outcome}");
            ExitCode::from(EXIT_DATAERR)
        }
    }
}

pub fn run_chatgpt_sign_out(options: ChatGptSignOutOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(options.journal_override) {
        Ok(journal) => journal.path,
        Err(error) => return print_journal_error(error),
    };

    let transport = solstone_core_thinking::chatgpt::UreqTransport;
    match solstone_core_thinking::chatgpt::sign_out(&journal, &transport, options.forget) {
        Ok(res) => {
            if !res.revoked {
                println!(
                    "signed out of ChatGPT.\nTo complete sign-out, disconnect solstone in your ChatGPT account settings."
                );
            } else {
                println!("signed out of ChatGPT.");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_DATAERR)
        }
    }
}

pub fn run_chatgpt_status(options: ChatGptStatusOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(options.journal_override) {
        Ok(journal) => journal.path,
        Err(error) => return print_journal_error(error),
    };

    match solstone_core_thinking::chatgpt::get_status(&journal) {
        Ok(status) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status).unwrap_or_default()
                );
            } else if status.signed_in {
                println!("status: signed in");
                if status.plan_usage_declined {
                    println!("plan usage: declined");
                }
            } else {
                println!("status: not signed in");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_DATAERR)
        }
    }
}

pub fn run_chatgpt_models(options: ChatGptModelsOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(options.journal_override) {
        Ok(journal) => journal.path,
        Err(error) => return print_journal_error(error),
    };

    let transport = std::sync::Arc::new(solstone_core_thinking::chatgpt::UreqTransport);
    match solstone_core_thinking::chatgpt::list_models(&journal, transport) {
        Ok(models) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({ "models": models }))
                        .unwrap_or_default()
                );
            } else {
                for m in models {
                    println!("{:<24} {}", m.slug, m.display_name);
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
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
        ProviderRequestError::InvalidInput(_)
        | ProviderRequestError::ModelMissing(_)
        | ProviderRequestError::Reason { .. } => EXIT_DATAERR,
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
        | ProviderRequestError::ModelMissing(detail)
        | ProviderRequestError::InvalidState(detail)
        | ProviderRequestError::ConfigUnreadable(detail) => detail.clone(),
        ProviderRequestError::Reason {
            reason_code,
            detail,
        } => format!("{reason_code}\n{detail}"),
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
        assert_eq!(
            provider_request_error_exit(&ProviderRequestError::Reason {
                reason_code: "model_not_found",
                detail: "x".to_owned(),
            }),
            EXIT_DATAERR
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
            "turn off confidential processing first, then switch your thinking provider."
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
            "No BYO provider selected. Must be one of: anthropic, chatgpt, google, local, or openai"
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
            "Invalid provider for BYO lane. Must be one of: anthropic, chatgpt, google, local, or openai"
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
            "clear your own endpoint first to run the bundled local model."
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
            "turn off confidential processing first, then switch to the bundled local model."
        );
        assert_eq!(config_bytes(journal.path()), before);
    }

    struct MockReader<R> {
        inner: R,
        is_term: bool,
    }

    impl<R: std::io::Read> std::io::Read for MockReader<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }
    }

    impl<R: std::io::BufRead> std::io::BufRead for MockReader<R> {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            self.inner.fill_buf()
        }

        fn consume(&mut self, amt: usize) {
            self.inner.consume(amt);
        }
    }

    impl<R> IsTerminal for MockReader<R> {
        fn is_terminal(&self) -> bool {
            self.is_term
        }
    }

    #[test]
    fn read_pasted_line_tests() {
        use std::io::Cursor;

        // 1. A non-terminal reader that contains a line returns None
        let mut non_terminal = MockReader {
            inner: Cursor::new(b"https://example.com/callback?code=abc\n".to_vec()),
            is_term: false,
        };
        assert_eq!(read_pasted_line_if_terminal(&mut non_terminal), None);

        // 2. A terminal reader at EOF returns None
        let mut eof_terminal = MockReader {
            inner: Cursor::new(Vec::<u8>::new()),
            is_term: true,
        };
        assert_eq!(read_pasted_line_if_terminal(&mut eof_terminal), None);

        // 3. A terminal reader with one line returns that trimmed line
        let mut line_terminal = MockReader {
            inner: Cursor::new(b"   https://example.com/callback?code=xyz \r\n".to_vec()),
            is_term: true,
        };
        assert_eq!(
            read_pasted_line_if_terminal(&mut line_terminal),
            Some("https://example.com/callback?code=xyz".to_string())
        );
    }
}
