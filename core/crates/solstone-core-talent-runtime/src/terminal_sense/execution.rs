// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::Candidate;
use crate::{ExecutionContext, GeneratedTalentResponse, PreparedTalent, RuntimeOutcome};
use serde_json::{Value, json};
use solstone_core_generate::{
    Cancellation, ClientError, GenerateExecutor, GenerateRequest, GenerateResponse,
};
use std::{io::Write, sync::Mutex};

#[derive(Default)]
struct Usage {
    attempts: u64,
    known: u64,
    input: u64,
    output: u64,
    total: u64,
}
impl Usage {
    fn add(&mut self, response: &GenerateResponse) {
        self.attempts += 1;
        if let GenerateResponse::Generated(r) = response
            && let (Some(i), Some(o), Some(t)) = (
                r.usage["input_tokens"].as_u64(),
                r.usage["output_tokens"].as_u64(),
                r.usage["total_tokens"].as_u64(),
            )
        {
            self.known += 1;
            self.input += i;
            self.output += o;
            self.total += t;
        }
    }
    fn value(&self) -> Option<Box<Value>> {
        if self.attempts == 0 {
            return None;
        }
        let complete = self.known == self.attempts;
        let mut v = json!({"usage_complete":complete,"attempts":self.attempts,"unknown_usage_attempts":self.attempts-self.known});
        if self.known > 0 {
            v["known_input_tokens"] = json!(self.input);
            v["known_output_tokens"] = json!(self.output);
            v["known_total_tokens"] = json!(self.total);
            if complete {
                v["input_tokens"] = json!(self.input);
                v["output_tokens"] = json!(self.output);
                v["total_tokens"] = json!(self.total);
            }
        }
        Some(Box::new(v))
    }
}
/// Counts every candidate and fallback attempt, so usage covers the whole sequence.
struct TrackingExecutor<'a> {
    inner: &'a dyn GenerateExecutor,
    usage: Mutex<Usage>,
}
impl GenerateExecutor for TrackingExecutor<'_> {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        let result = self.inner.execute(request);
        if let Ok(response) = &result {
            self.usage.lock().unwrap().add(response);
        } else {
            self.usage.lock().unwrap().attempts += 1;
        }
        result
    }
}

pub(crate) fn execute(
    candidate: Candidate,
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
    generate: &dyn GenerateExecutor,
    writer: &mut impl Write,
    cancellation: &Cancellation,
) -> Result<GeneratedTalentResponse, RuntimeOutcome> {
    let tracking = TrackingExecutor {
        inner: generate,
        usage: Mutex::new(Usage::default()),
    };
    let result = run(
        &candidate,
        prepared,
        context,
        &tracking,
        writer,
        cancellation,
    );
    let usage = tracking.usage.lock().unwrap().value();
    if let Some(value) = &usage {
        crate::emit(
            writer,
            json!({"event":"terminal_sense_usage","usage":value}),
        );
    }
    match result {
        Ok((text, _, degraded)) => Ok((text, usage, degraded)),
        Err(mut outcome) => {
            match &mut outcome {
                RuntimeOutcome::StageFailed(e)
                | RuntimeOutcome::GenerateRefused { error: e, .. } => e.usage = usage,
                _ => {}
            }
            Err(outcome)
        }
    }
}
fn run(
    candidate: &Candidate,
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
    generate: &dyn GenerateExecutor,
    writer: &mut impl Write,
    cancellation: &Cancellation,
) -> Result<GeneratedTalentResponse, RuntimeOutcome> {
    if cancellation.is_cancelled() {
        return Err(RuntimeOutcome::Cancelled);
    }
    crate::emit(
        writer,
        json!({"event":"terminal_sense","branch":"complete_timeline","binding":candidate.binding_fingerprint()}),
    );
    crate::emit_generate_input(writer, &candidate.request);
    let response = crate::execute_bounded_attempts(
        &candidate.request,
        None,
        |_, request| {
            if cancellation.is_cancelled() {
                return Ok(GenerateResponse::Cancelled);
            }
            generate
                .execute(request)
                .map_err(|e| crate::stage_error("generate", "runtime", prepared, e.to_string()))
        },
        |event| crate::emit(writer, event),
    )?;
    if cancellation.is_cancelled() {
        return Err(RuntimeOutcome::Cancelled);
    }
    let cause = match response {
        GenerateResponse::Cancelled => return Err(RuntimeOutcome::Cancelled),
        GenerateResponse::Generated(response) => {
            if response
                .input_budget
                .as_ref()
                .is_some_and(|b| b["clipped"] == true)
            {
                "protected_input_clipped"
            } else if let Ok(text) = candidate.render(&response.text) {
                prepared
                    .config
                    .insert(crate::story::INPUT_WHOLE_KEY.into(), Value::Bool(true));
                return Ok((text, None, None));
            } else {
                "invalid_source_binding"
            }
        }
        GenerateResponse::Refused(response) => {
            let code = response
                .reason_code
                .as_ref()
                .map(solstone_core_generate::ReasonCodeValue::as_wire)
                .unwrap_or("");
            if matches!(
                code,
                "context_preserved_overflow" | "context_budget_exceeded"
            ) {
                "protected_input_refused"
            } else {
                return Err(RuntimeOutcome::GenerateRefused {
                    error: crate::stage_error(
                        "generate",
                        "runtime",
                        prepared,
                        response.detail.clone(),
                    ),
                    response: Box::new(response),
                });
            }
        }
    };
    if cancellation.is_cancelled() {
        return Err(RuntimeOutcome::Cancelled);
    }
    crate::emit(
        writer,
        json!({"event":"terminal_sense","branch":"native_fallback","cause":cause}),
    );
    crate::native_generate_response(prepared, context, generate, writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sums_every_returned_attempt_and_keeps_unknown_distinct() {
        let mut usage = Usage::default();
        for (i, o) in [(100, 10), (200, 20), (300, 30), (400, 40)] {
            let response =
                GenerateResponse::Generated(Box::new(solstone_core_generate::GeneratedResponse {
                    id: None,
                    text: String::new(),
                    model: String::new(),
                    usage: json!({"input_tokens":i,"output_tokens":o,"total_tokens":i+o}),
                    finish_reason: String::new(),
                    thinking: None,
                    schema_validation: None,
                    input_budget: None,
                    request_budget: None,
                    inference: None,
                    hints_applied: vec![],
                }));
            usage.add(&response);
        }
        let value = usage.value().unwrap();
        assert_eq!(value["input_tokens"], 1000);
        assert_eq!(value["output_tokens"], 100);
        assert_eq!(value["total_tokens"], 1100);
        usage.attempts += 1;
        let value = usage.value().unwrap();
        assert!(value.get("total_tokens").is_none());
        assert_eq!(value["known_total_tokens"], 1100);
        assert_eq!(value["unknown_usage_attempts"], 1);
    }
}
