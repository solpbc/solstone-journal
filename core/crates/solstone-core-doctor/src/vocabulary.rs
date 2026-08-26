// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use serde::Serialize;

/// Capture-delivery facts attached to capture checks for machine consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientDeliveryFacts {
    pub registry: ClientRegistryState,
    pub assessed: Vec<AssessedClientFact>,
    pub unassessed: Vec<UnassessedClientFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientRegistryState {
    RegistryUnknown,
    RegistryEmpty,
    RegistryComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssessedClientFact {
    pub name: String,
    pub state: String,
    pub reach: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnassessedClientFact {
    pub name: String,
    pub reason: String,
    pub reach: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Blocker,
    Advisory,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Fail,
    Warn,
    Skip,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Darwin,
}
impl Platform {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Darwin => "darwin",
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Check {
    pub name: &'static str,
    pub severity: Severity,
    pub platforms: &'static [Platform],
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionError {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    pub severity: Severity,
    pub status: Status,
    pub detail: String,
    pub fix: Option<String>,
    #[serde(skip_serializing)]
    pub platform: Option<String>,
    pub execution_error: Option<ExecutionError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_delivery: Option<ClientDeliveryFacts>,
}
pub fn make_result(
    check: Check,
    status: Status,
    detail: impl Into<String>,
    fix: Option<impl Into<String>>,
) -> CheckResult {
    CheckResult {
        name: check.name,
        severity: check.severity,
        status,
        detail: detail.into(),
        fix: fix.map(Into::into),
        platform: None,
        execution_error: None,
        client_delivery: None,
    }
}
pub fn truncate(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() <= limit {
        text
    } else {
        format!("{}...", &text[..limit - 3])
    }
}
pub type RunnerResult = Result<CheckResult, ExecutionError>;
pub fn run_check(
    check: Check,
    runner: fn(&crate::context::CheckContext) -> RunnerResult,
    context: &crate::context::CheckContext,
) -> CheckResult {
    match runner(context) {
        Ok(result) => result,
        Err(error) => {
            let message = truncate(&error.message, 512);
            let detail = if message.is_empty() {
                format!("check execution failed: {}", error.kind)
            } else {
                format!("check execution failed: {}: {message}", error.kind)
            };
            CheckResult {
                name: check.name,
                severity: check.severity,
                status: Status::Fail,
                detail,
                fix: None,
                platform: None,
                execution_error: Some(ExecutionError {
                    kind: error.kind,
                    message,
                }),
                client_delivery: None,
            }
        }
    }
}
pub fn results_failed(results: &[CheckResult]) -> bool {
    results.iter().any(|r| {
        r.execution_error.is_some() || (r.severity == Severity::Blocker && r.status == Status::Fail)
    })
}
pub fn summary_counts(results: &[CheckResult]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    counts.insert("total", results.len());
    counts.insert(
        "failed",
        results.iter().filter(|r| r.status == Status::Fail).count(),
    );
    counts.insert(
        "warnings",
        results.iter().filter(|r| r.status == Status::Warn).count(),
    );
    counts.insert(
        "skipped",
        results.iter().filter(|r| r.status == Status::Skip).count(),
    );
    counts.insert(
        "errors",
        results
            .iter()
            .filter(|r| r.execution_error.is_some())
            .count(),
    );
    counts
}
pub fn status_label(result: &CheckResult) -> String {
    if result.execution_error.is_some() {
        "ERROR".into()
    } else {
        format!("{:?}", result.status).to_uppercase()
    }
}
