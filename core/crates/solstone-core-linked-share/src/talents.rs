// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The three linked-share talents, called in process through the talent
//! runtime. Requests carry no day or activity, so the runtime composes no
//! output path and writes nothing to the journal.

use std::path::PathBuf;

use serde_json::{Map, Value};
use solstone_core_generate::GenerateExecutor;
use solstone_core_talent_runtime::prepare::RuntimePaths;
use solstone_core_talent_runtime::{ExecutionContext, RuntimeOutcome};

pub const PRESENCE_TALENT: &str = "linked_presence";
pub const SEGMENT_TALENT: &str = "linked_share";
pub const ACTIVITY_TALENT: &str = "linked_share_activity";

/// Runs one talent over prepared text and returns its JSON output.
pub trait TalentCaller {
    fn call(&self, talent: &str, transcript: &str) -> Result<Value, String>;
}

impl<F> TalentCaller for F
where
    F: Fn(&str, &str) -> Result<Value, String>,
{
    fn call(&self, talent: &str, transcript: &str) -> Result<Value, String> {
        self(talent, transcript)
    }
}

/// The talent runtime with a model client, as `think` runs talents.
pub struct RuntimeTalents<'a> {
    paths: RuntimePaths,
    context: ExecutionContext,
    generate: &'a dyn GenerateExecutor,
    cancelled: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
}

impl<'a> RuntimeTalents<'a> {
    pub fn new(
        journal: impl Into<PathBuf>,
        paths: RuntimePaths,
        generate: &'a dyn GenerateExecutor,
    ) -> Self {
        Self {
            paths,
            context: ExecutionContext {
                journal: journal.into(),
            },
            generate,
            cancelled: None,
        }
    }

    /// Attach read-only preview work to its owner, including every talent
    /// preparation and model call in a multi-segment package.
    pub fn with_cancellation(mut self, cancelled: &'a (dyn Fn() -> bool + Send + Sync)) -> Self {
        self.cancelled = Some(cancelled);
        self
    }
}

impl TalentCaller for RuntimeTalents<'_> {
    fn call(&self, talent: &str, transcript: &str) -> Result<Value, String> {
        let never = || false;
        let cancellation =
            solstone_core_generate::Cancellation::from_fn(self.cancelled.unwrap_or(&never));
        if cancellation.is_cancelled() {
            return Err("cancelled".to_owned());
        }
        let request = Map::from_iter([
            ("name".to_owned(), Value::String(talent.to_owned())),
            (
                "transcript".to_owned(),
                Value::String(transcript.to_owned()),
            ),
        ]);
        let outcome = solstone_core_talent_runtime::execute_request(
            request,
            &self.paths,
            &self.context,
            self.generate,
            &mut std::io::sink(),
            &cancellation,
        );
        match outcome {
            RuntimeOutcome::Finished { output, .. } => serde_json::from_str(&output)
                .map_err(|error| format!("talent '{talent}' returned invalid JSON: {error}")),
            RuntimeOutcome::StageFailed(error) => Err(error.to_string()),
            RuntimeOutcome::PrepareFailed(error) => Err(format!("talent '{talent}': {error}")),
            other => Err(format!("talent '{talent}' did not finish: {other:?}")),
        }
    }
}

/// Aggregate the accepted calls contributing to this read-only result.
pub(crate) fn combine_partial_inputs<'a>(
    markers: impl Iterator<Item = &'a Value>,
) -> Option<Value> {
    let markers: Vec<_> = markers.filter(|v| v.is_object()).collect();
    if markers.is_empty() {
        return None;
    }
    let mut result = Map::new();
    for key in ["dropped_entries", "dropped_chars"] {
        let sum = markers.iter().try_fold(0u64, |sum, marker| {
            marker.get(key)?.as_u64()?.checked_add(sum)
        });
        result.insert(key.into(), sum.map(Value::from).unwrap_or(Value::Null));
    }
    Some(Value::Object(result))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;
    use solstone_core_generate::{GenerateRequest, GenerateResponse, GeneratedResponse};

    use super::*;

    fn payload_talents() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../payload/solstone/talent")
    }

    fn generated(text: &str) -> GenerateResponse {
        GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test-model".to_owned(),
            usage: json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: Some(json!({"valid": true, "errors": []})),
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))
    }

    fn files(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).expect("read dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    }

    /// The shipped talents compose without a day, reach the model with the
    /// prepared text and their schema, and leave the journal untouched.
    #[test]
    fn shipped_talents_run_in_process_and_write_nothing() {
        let journal = tempfile::tempdir().expect("journal");
        fs::create_dir_all(journal.path().join("config")).expect("config");
        fs::write(
            journal.path().join("config/journal.json"),
            json!({"providers":{"active":{"provider":"google","model":"test-model"}}}).to_string(),
        )
        .expect("config");
        let before = files(journal.path());
        let scratch = tempfile::tempdir().expect("scratch");
        let paths = RuntimePaths {
            talent_root: payload_talents(),
            apps_root: scratch.path().join("apps"),
            templates_dir: scratch.path().join("templates"),
        };
        let replies = [
            (PRESENCE_TALENT, json!({"role": "with"})),
            (
                SEGMENT_TALENT,
                json!({"points": [{"ids": ["u1"], "point": "proposed moving the launch to May"}]}),
            ),
            (ACTIVITY_TALENT, json!({"summary": ""})),
        ];
        let generate = |request: &GenerateRequest| {
            let (_, reply) = replies
                .iter()
                .find(|(talent, _)| *talent == request.context)
                .expect("known talent");
            assert_eq!(request.temperature, 0.0);
            assert!(request.json_schema.is_some(), "{}", request.context);
            let text = request
                .contents
                .iter()
                .map(|part| format!("{part:?}"))
                .collect::<String>();
            assert!(text.contains("PREPARED TEXT"), "{text}");
            Ok(generated(&reply.to_string()))
        };
        let talents = RuntimeTalents::new(journal.path(), paths, &generate);
        for (talent, reply) in &replies {
            assert_eq!(talents.call(talent, "PREPARED TEXT").expect(talent), *reply);
        }
        assert_eq!(files(journal.path()), before);
    }

    #[test]
    fn a_refused_or_unknown_talent_is_an_error() {
        let journal = tempfile::tempdir().expect("journal");
        let paths = RuntimePaths {
            talent_root: payload_talents(),
            apps_root: journal.path().join("apps"),
            templates_dir: journal.path().join("templates"),
        };
        let generate = |_: &GenerateRequest| Ok(generated("{}"));
        let talents = RuntimeTalents::new(journal.path(), paths, &generate);
        // No model is configured for this journal.
        assert!(talents.call(SEGMENT_TALENT, "text").is_err());
        assert!(talents.call("no_such_talent", "text").is_err());
    }
}
