// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Usage summed over every model call a talent makes.

use serde_json::{Value, json};
use solstone_core_generate::{ClientError, GenerateExecutor, GenerateRequest, GenerateResponse};
use std::sync::Mutex;

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
/// Counts every attempt a talent makes, so usage covers the whole sequence.
pub(crate) struct TrackingExecutor<'a> {
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
impl<'a> TrackingExecutor<'a> {
    pub(crate) fn new(inner: &'a dyn GenerateExecutor) -> Self {
        Self {
            inner,
            usage: Mutex::new(Usage::default()),
        }
    }

    pub(crate) fn usage(&self) -> Option<Box<Value>> {
        self.usage.lock().unwrap().value()
    }
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
