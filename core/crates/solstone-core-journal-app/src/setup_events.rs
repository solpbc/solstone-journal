// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The progress the window shows while `journal setup --jsonl` runs.

use serde::Serialize;
use serde_json::Value;

/// One line of setup progress, reduced to what the window shows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SetupProgress {
    StepStarted { step: String },
    StepFinished { step: String, outcome: String },
    StepFailed { step: String, message: String },
    Warning { step: String, message: String },
    Completed { ok: bool },
}

/// Read one line of setup output. Lines that are not setup events (the
/// doctor's own forwarded checks, blank lines) are not progress.
#[must_use]
pub fn parse_line(line: &str) -> Option<SetupProgress> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default();
    let step = text("step").to_owned();
    match text("event") {
        "step.started" => Some(SetupProgress::StepStarted { step }),
        "step.completed" => Some(SetupProgress::StepFinished {
            step,
            outcome: text("outcome").to_owned(),
        }),
        "step.failed" => Some(SetupProgress::StepFailed {
            step,
            message: error_message(value.get("error")),
        }),
        "step.warning" => Some(SetupProgress::Warning {
            step,
            message: text("text").to_owned(),
        }),
        "setup.completed" => Some(SetupProgress::Completed {
            ok: !matches!(text("status"), "failed"),
        }),
        _ => None,
    }
}

/// The owner-facing sentence a failed step carries, wherever its payload
/// keeps it.
fn error_message(error: Option<&Value>) -> String {
    let Some(error) = error else {
        return String::new();
    };
    for key in ["message", "detail", "reason"] {
        if let Some(text) = error.get(key).and_then(Value::as_str) {
            return text.to_owned();
        }
    }
    error.as_str().map(str::to_owned).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_step_events_and_ignores_other_lines() {
        assert_eq!(
            parse_line(r#"{"event":"step.started","ts":"t","step":"journal"}"#),
            Some(SetupProgress::StepStarted {
                step: "journal".to_owned()
            })
        );
        assert_eq!(
            parse_line(
                r#"{"event":"step.failed","step":"service","error":{"code":"service_up_failed","message":"the service did not start"}}"#
            ),
            Some(SetupProgress::StepFailed {
                step: "service".to_owned(),
                message: "the service did not start".to_owned()
            })
        );
        assert_eq!(
            parse_line(r#"{"event":"check.completed","name":"x"}"#),
            None
        );
        assert_eq!(parse_line("not json"), None);
    }
}
