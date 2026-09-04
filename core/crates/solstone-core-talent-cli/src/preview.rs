// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRequest {
    pub name: String,
    pub day: Option<String>,
    pub segment: Option<String>,
    pub facet: Option<String>,
    pub activity: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptPreviewRefusal {
    pub code: String,
    pub segment: Option<String>,
    pub recovery: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptPreview {
    Assembled {
        access_tier: Option<String>,
        loads_sources: bool,
        parts: Vec<String>,
    },
    WouldNotRun {
        reason: String,
    },
    Refused(PromptPreviewRefusal),
    UnavailablePreStep,
    Failed {
        error: String,
    },
}

pub trait PromptPreviewer {
    fn preview(&self, journal_root: &Path, request: &PreviewRequest) -> PromptPreview;
}

pub struct UnreachablePreviewer;

impl PromptPreviewer for UnreachablePreviewer {
    fn preview(&self, _: &Path, _: &PreviewRequest) -> PromptPreview {
        unreachable!("generate prompt previewer invoked unexpectedly")
    }
}
