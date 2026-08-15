// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Read};
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Map, Value};
use solstone_core_cli::EngageOptions;
use solstone_core_cortex_client::{
    CortexClientError, CortexRequest, CortexRequestClient, CortexRequestPolicy, DispatchError,
    UseEndState, read_use_events,
};

pub(crate) fn run(journal: &Path, options: EngageOptions) -> ExitCode {
    let mut prompt = String::new();
    if let Err(error) = io::stdin().read_to_string(&mut prompt) {
        eprintln!("Error: failed to read prompt from stdin: {error}");
        return ExitCode::from(1);
    }
    let prompt = trim_python_whitespace(&prompt);
    if prompt.is_empty() {
        eprintln!("Error: no prompt provided on stdin.");
        return ExitCode::from(1);
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("Error: failed to send cortex request.");
            return ExitCode::from(1);
        }
    };
    let client = CortexRequestClient::new(journal, CortexRequestPolicy::interactive());
    let request = engage_request(prompt, &options);
    let use_id = match runtime.block_on(client.dispatch(&request)) {
        Ok(use_id) => use_id,
        Err(DispatchError::Unavailable | DispatchError::NotClaimed { .. }) => {
            eprintln!("Error: failed to send cortex request.");
            return ExitCode::from(1);
        }
    };
    if !options.wait {
        println!("{use_id}");
        return ExitCode::SUCCESS;
    }

    let outcome = match runtime.block_on(client.wait_for_uses(std::slice::from_ref(&use_id))) {
        Ok(outcome) => outcome,
        Err(CortexClientError::ReadUseLog(error)) => {
            eprintln!("Error: failed to read agent result: {error}");
            return ExitCode::from(1);
        }
        Err(CortexClientError::Dispatch(_)) => {
            eprintln!("Error: failed to send cortex request.");
            return ExitCode::from(1);
        }
    };
    if outcome
        .timed_out
        .iter()
        .any(|timed_out| timed_out.use_id() == use_id)
    {
        eprintln!("Error: agent timed out.");
        return ExitCode::from(1);
    }
    let end_state = outcome
        .completed
        .get(&use_id)
        .map(|completion| completion.end_state)
        .unwrap_or(UseEndState::Unknown);
    if end_state != UseEndState::Finish {
        eprintln!("Error: agent ended with state: {}", end_state.as_str());
        return ExitCode::from(1);
    }
    match finish_result(journal, &use_id) {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Error: failed to read agent result: {error}");
            ExitCode::from(1)
        }
    }
}

fn engage_request(prompt: &str, options: &EngageOptions) -> CortexRequest {
    let mut config = Map::new();
    if let Some(facet) = &options.facet {
        config.insert("facet".to_owned(), Value::String(facet.clone()));
    }
    if let Some(day) = &options.day {
        config.insert("day".to_owned(), Value::String(day.clone()));
    }
    CortexRequest::new(prompt, options.name.clone()).with_config(config)
}

fn finish_result(journal: &Path, use_id: &str) -> io::Result<String> {
    Ok(read_use_events(journal, use_id)?
        .into_iter()
        .rev()
        .find(|event| event.get("event").and_then(Value::as_str) == Some("finish"))
        .and_then(|event| {
            event
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default())
}

fn trim_python_whitespace(value: &str) -> &str {
    value.trim_matches(|character: char| {
        character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
    })
}

#[cfg(test)]
mod tests {
    use super::{EngageOptions, engage_request, trim_python_whitespace};

    #[test]
    fn request_envelope_is_flat_and_omits_absent_context() {
        let absent = EngageOptions {
            name: "partner".to_owned(),
            wait: false,
            facet: None,
            day: None,
        };
        let value: serde_json::Value = serde_json::from_str(
            &engage_request("review this", &absent)
                .request_line(42, "42")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["tract"], "cortex");
        assert_eq!(value["event"], "request");
        assert_eq!(value["ts"], 42);
        assert_eq!(value["use_id"], "42");
        assert_eq!(value["prompt"], "review this");
        assert_eq!(value["name"], "partner");
        assert!(value.get("facet").is_none());
        assert!(value.get("day").is_none());

        let present = EngageOptions {
            name: "partner".to_owned(),
            wait: false,
            facet: Some("work".to_owned()),
            day: Some("20260404".to_owned()),
        };
        let value: serde_json::Value = serde_json::from_str(
            &engage_request("review this", &present)
                .request_line(42, "42")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["tract"], "cortex");
        assert_eq!(value["event"], "request");
        assert_eq!(value["ts"], 42);
        assert_eq!(value["use_id"], "42");
        assert_eq!(value["prompt"], "review this");
        assert_eq!(value["name"], "partner");
        assert_eq!(value["facet"], "work");
        assert_eq!(value["day"], "20260404");
    }

    #[test]
    fn prompt_trim_matches_python_c0_information_separators() {
        assert_eq!(
            trim_python_whitespace("\u{1c}\u{1f} prompt \u{1e}"),
            "prompt"
        );
        assert_eq!(trim_python_whitespace("\u{1c}\u{1d}"), "");
    }
}
