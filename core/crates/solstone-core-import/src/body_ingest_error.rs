// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a body import's source reading, Oura connection and token store report.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyIngestErrorKind {
    Gate,
    Source,
    Publication,
}

#[derive(Clone, PartialEq, Eq)]
pub struct BodyIngestError {
    kind: BodyIngestErrorKind,
    stage: &'static str,
}

impl BodyIngestError {
    pub const fn new(kind: BodyIngestErrorKind, stage: &'static str) -> Self {
        Self { kind, stage }
    }

    pub(crate) const fn gate(stage: &'static str) -> Self {
        Self::new(BodyIngestErrorKind::Gate, stage)
    }

    pub fn kind(&self) -> BodyIngestErrorKind {
        self.kind
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }
}

impl fmt::Display for BodyIngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            BodyIngestErrorKind::Gate => "gate",
            BodyIngestErrorKind::Source => "source",
            BodyIngestErrorKind::Publication => "publication",
        };
        write!(formatter, "body-ingest {kind}: {}", self.stage)
    }
}

impl fmt::Debug for BodyIngestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodyIngestError {}
