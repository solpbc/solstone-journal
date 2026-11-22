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

/// The outcome of extracting an activity's source passages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PassageRun {
    /// The passage document, its readable rendering, and whether every unit
    /// has its passages.
    Extracted {
        document: String,
        readable: String,
        complete: bool,
    },
    /// The activity could not be prepared; says why, as a preview would.
    NotPrepared(PromptPreview),
}

pub trait PromptPreviewer {
    fn preview(&self, journal_root: &Path, request: &PreviewRequest) -> PromptPreview;

    fn preview_in(
        &self,
        _locale: Option<&str>,
        journal_root: &Path,
        request: &PreviewRequest,
    ) -> PromptPreview {
        self.preview(journal_root, request)
    }

    fn passages_in(
        &self,
        _locale: Option<&str>,
        journal_root: &Path,
        request: &PreviewRequest,
    ) -> PassageRun {
        self.passages(journal_root, request)
    }

    fn passages(&self, _journal_root: &Path, _request: &PreviewRequest) -> PassageRun {
        PassageRun::NotPrepared(PromptPreview::Failed {
            error: "source passages are not available here".to_owned(),
        })
    }
}

pub struct UnreachablePreviewer;

impl PromptPreviewer for UnreachablePreviewer {
    fn preview(&self, _: &Path, _: &PreviewRequest) -> PromptPreview {
        unreachable!("generate prompt previewer invoked unexpectedly")
    }
}

pub use solstone_core_journal_io::cli_diagnostics::path_detail;

pub fn facet_detail(locale: &str, error: &solstone_core_facets::FacetStoreError) -> String {
    use solstone_core_facets::FacetStoreError;
    match error {
        FacetStoreError::Path(error) => path_detail(locale, error),
        FacetStoreError::DeclarationNotObject { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.facet.object",
            [("path", &(path.display()))],
        ),
        FacetStoreError::Read(error) => read_detail(locale, error),
        _ => error.to_string(),
    }
}

pub use solstone_core_journal_io::cli_diagnostics::read_detail;

pub fn settings_detail(
    locale: &str,
    error: &solstone_core_journal_config::ConfigLoadError,
) -> String {
    error.render(locale)
}
