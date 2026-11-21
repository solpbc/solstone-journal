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
    Windows,
}
impl Platform {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Darwin => "darwin",
            Self::Windows => "windows",
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
    #[serde(skip)]
    pub(crate) localized_message: Option<String>,
}

impl ExecutionError {
    pub(crate) fn localized(kind: &str, diagnostic: solstone_core_cli_copy::Diagnostic) -> Self {
        let locale = solstone_core_cli_copy::cli_locale();
        Self {
            kind: kind.into(),
            message: diagnostic.render(solstone_core_cli_copy::ENGLISH),
            localized_message: (locale != solstone_core_cli_copy::ENGLISH)
                .then(|| diagnostic.render(locale)),
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    pub severity: Severity,
    pub status: Status,
    pub detail: String,
    pub fix: Option<String>,
    #[serde(skip)]
    pub(crate) changes_service: bool,
    #[serde(skip)]
    pub(crate) conflict_guidance: Option<String>,
    #[serde(skip_serializing)]
    pub platform: Option<String>,
    pub execution_error: Option<ExecutionError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_delivery: Option<ClientDeliveryFacts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heals: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backlog_days: Option<BacklogDays>,
}
impl CheckResult {
    pub(crate) fn changing_service(mut self) -> Self {
        self.changes_service = true;
        self
    }
}

/// The days behind `journal_caught_up`'s counts, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BacklogDays {
    pub pending: Vec<String>,
    pub stuck: Vec<String>,
    pub unknown: Vec<String>,
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
        changes_service: false,
        conflict_guidance: None,
        platform: None,
        execution_error: None,
        client_delivery: None,
        heals: None,
        backlog_days: None,
    }
}

/// Choose count forms for the human report or its backwards-compatible English wire fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportForm {
    Human,
    Structured,
    CanonicalEnglish,
}

impl ReportForm {
    pub(crate) fn locale(self) -> &'static str {
        if self == Self::CanonicalEnglish {
            solstone_core_window_catalog::ENGLISH
        } else {
            solstone_core_window_catalog::cli_locale()
        }
    }

    pub(crate) fn counted(
        self,
        id: &str,
        counts: &[(&str, u64)],
        values: &[(&str, &dyn std::fmt::Display)],
    ) -> String {
        use solstone_core_cli_copy::{Value, render_for};
        use solstone_core_window_catalog::ENGLISH;
        let legacy = self != Self::Human && self.locale() == ENGLISH;
        let strings: Vec<_> = values
            .iter()
            .map(|(name, value)| (*name, value.to_string()))
            .collect();
        let mut args: Vec<_> = strings
            .iter()
            .map(|(name, value)| (*name, Value::String(value)))
            .collect();
        for &(name, count) in counts {
            // In existing English JSON fields the default branch carries the original wording.
            // Large inputs keep their trailing digits for languages with modulo-based plural rules.
            let quantity = if legacy && count == 1 {
                0
            } else if count > i32::MAX as u64 {
                1_000_000 + count % 1_000_000
            } else {
                count
            };
            args.push((name, Value::Cardinal(quantity as i64)));
        }
        render_for(self.locale(), id, &args)
    }
}
pub fn truncate(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() <= limit {
        text
    } else {
        // ⛔ `limit` is a BYTE index and these strings carry owner-chosen
        // filesystem paths, so the cut lands inside a multi-byte character on
        // any non-ASCII path and slicing there panics. There is no
        // `catch_unwind` in this crate, so that aborts the whole doctor run
        // rather than failing one check. Walk back to a boundary.
        let mut end = limit - 3;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
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
            let diagnostic = error
                .localized_message
                .as_deref()
                .map(|text| truncate(text, 512));
            let diagnostic = diagnostic.as_deref().unwrap_or(&message);
            let detail = if diagnostic.is_empty() {
                solstone_core_cli_copy::display(
                    "journal.cli.doctor.execution.error",
                    [("kind", &error.kind)],
                )
            } else {
                solstone_core_cli_copy::display(
                    "journal.cli.doctor.execution.error.detail",
                    [("kind", &error.kind), ("error", &diagnostic)],
                )
            };
            CheckResult {
                name: check.name,
                severity: check.severity,
                status: Status::Fail,
                detail,
                fix: None,
                changes_service: false,
                conflict_guidance: None,
                platform: None,
                execution_error: Some(ExecutionError {
                    kind: error.kind,
                    message,
                    localized_message: None,
                }),
                client_delivery: None,
                heals: None,
                backlog_days: None,
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
    let id = if result.execution_error.is_some() {
        "journal.cli.doctor.status.error"
    } else {
        match result.status {
            Status::Ok => "journal.cli.doctor.status.ok",
            Status::Fail => "journal.cli.doctor.status.fail",
            Status::Warn => "journal.cli.doctor.status.warn",
            Status::Skip => "journal.cli.doctor.status.skip",
        }
    };
    solstone_core_cli_copy::text(id)
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate;

    /// ⛔ A non-ASCII journal path is enough to abort the whole doctor run.
    /// Eight call sites feed this, and the checks now carry owner-chosen paths.
    #[test]
    fn a_multibyte_path_is_cut_without_panicking() {
        let path = format!("/home/owner/journal/{}", "\u{65e5}".repeat(200));
        assert!(
            !path.is_char_boundary(397),
            "the fixture must straddle the cut"
        );

        let out = truncate(&path, 400);

        assert!(out.ends_with("..."));
        assert!(out.len() <= 400);
        assert!(path.starts_with(out.trim_end_matches('.')));
    }

    #[test]
    fn ascii_text_is_unchanged_below_the_limit() {
        assert_eq!(truncate("a short line", 400), "a short line");
    }
}
