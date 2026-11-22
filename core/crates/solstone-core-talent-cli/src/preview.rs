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

pub fn path_detail(locale: &str, error: &solstone_core_journal_io::PathError) -> String {
    use solstone_core_journal_io::{PathError, paths::RelativePathProblem};
    match error {
        PathError::InvalidRelativePath { rel, .. } => match RelativePathProblem::of(rel) {
            Some(RelativePathProblem::Empty) => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.empty", &[])
            }
            Some(RelativePathProblem::Absolute) => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.absolute", &[])
            }
            Some(RelativePathProblem::Backslash) => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.backslash", &[])
            }
            Some(RelativePathProblem::Component) => {
                solstone_core_cli_copy::render_for(locale, "journal.cli.talent.path.component", &[])
            }
            Some(RelativePathProblem::Unreadable) => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.talent.path.unreadable",
                &[],
            ),
            None => error.to_string(),
        },
        PathError::Escape(error) => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.talent.path.escape",
            [
                ("relative", &(format!("{:?}", error.rel))),
                ("path", &(error.path.display())),
            ],
        ),
        PathError::Io { .. } => error.to_string(),
    }
}

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

pub fn read_detail(locale: &str, error: &solstone_core_journal_io::ReadError) -> String {
    use solstone_core_journal_io::ReadError;
    match error {
        ReadError::Malformed(error) => match error.line {
            Some(line) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.facet.malformed.line",
                [("path", &(error.path.display())), ("line", &(line))],
            ),
            None => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.talent.facet.malformed",
                [("path", &(error.path.display()))],
            ),
        },
        ReadError::Io { .. } => error.to_string(),
    }
}

pub fn settings_detail(
    locale: &str,
    error: &solstone_core_journal_config::ConfigLoadError,
) -> String {
    let solstone_core_journal_config::ConfigLoadError::Corrupt { path, .. } = error;
    solstone_core_window_catalog::strings(
        locale,
        "journal.window.facets.awareness.response.detail.unreadable.settings",
        [("settings_path", path.display().to_string().as_str())],
    )
}
