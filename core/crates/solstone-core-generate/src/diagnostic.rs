// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

/// A diagnostic produced by the one-shot transport itself. External errors
/// and captured bytes are passed through by the caller.
#[derive(Debug)]
pub enum TransportDiagnostic {
    Object,
    StringField { field: &'static str },
    OptionalString { field: &'static str },
    StringItems { field: &'static str },
    OptionalArray { field: &'static str },
    Schema,
    Usage,
    Boolean { field: &'static str },
    Outcome,
    Truncated { limit: usize },
    WireUnavailable { pipe: &'static str },
    StartWorker { pipe: &'static str, detail: String },
    Terminate { detail: String },
    Inspect { detail: String },
    Reap { detail: String },
    RetryReap { detail: String },
    Completion { detail: String },
    JoinPanic { pipe: &'static str },
    Writer { detail: String },
    Collector { pipe: &'static str, detail: String },
    WriterPanic,
    CollectorPanic { pipe: &'static str },
    EmptyContents,
    OverflowRequiresText,
}

impl fmt::Display for TransportDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Object => f.write_str("record must be a JSON object"),
            Self::StringField { field } => write!(f, "{field} must be a string"),
            Self::OptionalString { field } => write!(f, "{field} must be a string or null"),
            Self::StringItems { field } => write!(f, "{field} must contain only strings"),
            Self::OptionalArray { field } => write!(f, "{field} must be an array or null"),
            Self::Schema => f.write_str("record schema is not supported"),
            Self::Usage => f.write_str("usage is required"),
            Self::Boolean { field } => write!(f, "{field} must be a boolean"),
            Self::Outcome => f.write_str("response outcome is not supported"),
            Self::Truncated { limit } => write!(f, "stdout truncated at {limit} bytes"),
            Self::WireUnavailable { pipe } => write!(f, "wire {pipe} is unavailable"),
            Self::StartWorker { pipe, detail } => write!(f, "start {pipe} worker: {detail}"),
            Self::Terminate { detail } => write!(f, "terminate child: {detail}"),
            Self::Inspect { detail } => write!(f, "inspect child before termination: {detail}"),
            Self::Reap { detail } => write!(f, "reap child: {detail}"),
            Self::RetryReap { detail } => write!(f, "retry reap child: {detail}"),
            Self::Completion { detail } => write!(f, "pipe worker completion channel: {detail}"),
            Self::JoinPanic { pipe } => write!(f, "{pipe} worker join: thread panicked"),
            Self::Writer { detail } => write!(f, "stdin writer: {detail}"),
            Self::Collector { pipe, detail } => write!(f, "{pipe} collector: {detail}"),
            Self::WriterPanic => f.write_str("stdin writer: thread panicked"),
            Self::CollectorPanic { pipe } => write!(f, "{pipe} collector: thread panicked"),
            Self::EmptyContents => f.write_str("contents must be non-empty"),
            Self::OverflowRequiresText => {
                f.write_str("overflow_text requires the first content part to be text")
            }
        }
    }
}

impl From<TransportDiagnostic> for String {
    fn from(diagnostic: TransportDiagnostic) -> Self {
        diagnostic.to_string()
    }
}
