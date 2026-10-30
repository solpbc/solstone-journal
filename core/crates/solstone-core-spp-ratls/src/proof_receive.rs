// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixed observations of one returned establishment failure. No content or history.

use serde::Serialize;
use std::net::SocketAddr;

macro_rules! vocabulary {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        pub enum $name { $(#[serde(rename = $wire)] $variant),+ }
        #[cfg(test)] impl $name { const VALUES: &'static [&'static str] = &[$($wire),+]; }
    };
}

vocabulary!(ReceiveStage {
    BeforeReceive => "before_receive", Headers => "headers", Body => "body", Complete => "complete"
});
vocabulary!(ReceiveCategory {
    BeforeReceive => "before_receive", ResponseEof => "response_eof",
    ResponseBodyEof => "response_body_eof", IoUnexpectedEof => "io_unexpected_eof",
    Timeout => "timeout", Reset => "reset", IoOther => "io_other", Framing => "framing",
    Status => "status", ContentType => "content_type", Trailing => "trailing",
    ChannelState => "channel_state", ProofRejected => "proof_rejected"
});
vocabulary!(ReceiveBranch {
    Unknown => "unknown", Plaintext => "plaintext", PlaintextZero => "plaintext_zero",
    MappedEof => "mapped_unexpected_eof", LatchedEof => "latched_eof",
    Io => "io", TlsRejected => "tls_rejected", BufferedBeforeTlsFailure => "buffered_before_tls_failure",
    TrailingBuffered => "trailing_buffered", TrailingClose => "trailing_close",
    TrailingSetupFailed => "trailing_setup_failed", TrailingRestoreFailed => "trailing_restore_failed",
    TrailingWouldBlock => "trailing_would_block", TrailingTcpEof => "trailing_tcp_eof",
    TrailingBytes => "trailing_bytes", TrailingReset => "trailing_reset",
    TrailingTimeout => "trailing_timeout", TrailingIoOther => "trailing_io_other"
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReceiveProgress {
    pub stage: ReceiveStage,
    pub category: ReceiveCategory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_body_bytes: Option<usize>,
}
impl Default for ReceiveProgress {
    fn default() -> Self {
        Self {
            stage: ReceiveStage::BeforeReceive,
            category: ReceiveCategory::BeforeReceive,
            header_bytes: None,
            body_bytes: None,
            declared_body_bytes: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReceiveTermination {
    pub branch: ReceiveBranch,
    pub saw_eof: bool,
    pub tls_failed: bool,
    /// False means unknown, never proof that close_notify was absent.
    pub close_observed: bool,
}
impl Default for ReceiveTermination {
    fn default() -> Self {
        Self {
            branch: ReceiveBranch::Unknown,
            saw_eof: false,
            tls_failed: false,
            close_observed: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SelectedProofFailure {
    pub destination: SocketAddr,
    pub ordinal: u8,
    pub attempted: u8,
    #[serde(flatten)]
    pub progress: ReceiveProgress,
    #[serde(flatten)]
    pub termination: ReceiveTermination,
}

impl SelectedProofFailure {
    pub(crate) fn new(destination: SocketAddr) -> Self {
        Self {
            destination,
            ordinal: 1,
            attempted: 1,
            progress: ReceiveProgress::default(),
            termination: ReceiveTermination::default(),
        }
    }
}

/// The private projection and its maintained reader consume this generated contract.
#[cfg(test)]
fn private_proof_receive_schema() -> serde_json::Value {
    serde_json::json!({
        "_source":"solstone-core-spp-ratls/src/proof_receive.rs",
        "_regenerate":"cargo test --manifest-path core/Cargo.toml -p solstone-core-spp-ratls schema_matches_fixture -- --nocapture",
        "_rule":"Generated; do not retype vocabulary in consumers.",
        "required":["destination","ordinal","attempted","stage","category","branch","saw_eof","tls_failed","close_observed"],
        "optional":["header_bytes","body_bytes","declared_body_bytes"],
        "enums":{"stage":ReceiveStage::VALUES,"category":ReceiveCategory::VALUES,"branch":ReceiveBranch::VALUES},
        "stage_categories": {
            "before_receive":["before_receive"],
            "headers":["response_eof","io_unexpected_eof","timeout","reset","io_other","framing"],
            "body":["response_body_eof","io_unexpected_eof","timeout","reset","io_other","framing"],
            "complete":["status","content_type","trailing","channel_state","proof_rejected"]
        },
        "branch_flags":{"mapped_unexpected_eof":"saw_eof","latched_eof":"saw_eof","trailing_tcp_eof":"saw_eof","trailing_close":"close_observed",
            "tls_rejected":"tls_failed","buffered_before_tls_failure":"tls_failed"},
        "complete_branches":ReceiveBranch::VALUES.iter().filter(|v|v.starts_with("trailing_")).collect::<Vec<_>>(),
        "category_reasons":{"response_eof": "proof_http_eof", "response_body_eof": "proof_http_eof", "io_unexpected_eof": "proof_http_eof", "timeout": "proof_http_timeout", "reset": "proof_http_transport", "io_other": "proof_http_transport", "framing": "proof_http_framing", "status": "proof_http_status", "content_type": "proof_http_content_type", "trailing": "proof_http_trailing", "channel_state": "proof_http_channel_state"},
        "limits":{"header_bytes":16384,"body_bytes":8388608,"declared_body_bytes":8388608,"attempted":8}
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn schema_matches_fixture() {
        let schema = super::private_proof_receive_schema();
        println!("PROOF_RECEIVE_SCHEMA={schema}");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/proof_receive_schema.json"))
                .unwrap();
        assert_eq!(schema, fixture);
    }
}
