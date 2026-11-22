// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Passages for one activity, from the source its talent would read.

use serde_json::{Map, Value, json};
use solstone_core_generate::GenerateExecutor;
use solstone_core_talent_cli::preview::{PreviewRequest, PromptPreview, PromptPreviewRefusal};

use super::{Extraction, SourceUnit, document, extract, source_units};
use crate::ExecutionContext;
use crate::assemble::prepare_preview;
use crate::prepare::{RuntimePaths, SPARSE_INPUT_NOTE};

/// One activity's source units and the passages extracted from them.
#[derive(Debug)]
pub struct ActivityPassages {
    pub talent: String,
    pub day: String,
    pub facet: String,
    pub activity: String,
    pub units: Vec<SourceUnit>,
    pub extraction: Extraction,
}

impl ActivityPassages {
    pub fn document(&self) -> Value {
        let header = Map::from_iter([
            ("talent".to_owned(), json!(self.talent)),
            ("day".to_owned(), json!(self.day)),
            ("facet".to_owned(), json!(self.facet)),
            ("activity".to_owned(), json!(self.activity)),
        ]);
        document(&self.units, &self.extraction, header)
    }

    pub fn readable(&self) -> String {
        self.readable_in(solstone_core_cli_copy::ENGLISH)
    }

    pub fn readable_in(&self, locale: &str) -> String {
        super::readable_in(locale, &self.units, &self.extraction)
    }
}

fn refused(code: &str, recovery: &str) -> PromptPreview {
    PromptPreview::Refused(PromptPreviewRefusal {
        code: code.to_owned(),
        segment: None,
        recovery: recovery.to_owned(),
    })
}

/// Prepare the activity exactly as its talent's run would, then extract
/// passages from the source that run would read. Nothing in the journal is
/// written; each nomination completion is recorded by the generate boundary
/// like any other.
pub fn activity_passages(
    request: &PreviewRequest,
    paths: &RuntimePaths,
    context: &ExecutionContext,
    generate: &dyn GenerateExecutor,
) -> Result<ActivityPassages, PromptPreview> {
    activity_passages_in(None, request, paths, context, generate)
}

pub fn activity_passages_in(
    display_locale: Option<&str>,
    request: &PreviewRequest,
    paths: &RuntimePaths,
    context: &ExecutionContext,
    generate: &dyn GenerateExecutor,
) -> Result<ActivityPassages, PromptPreview> {
    let locale = display_locale.unwrap_or(solstone_core_cli_copy::ENGLISH);
    let (Some(activity), Some(day), Some(facet)) = (
        request.activity.as_deref(),
        request.day.as_deref(),
        request.facet.as_deref(),
    ) else {
        return Err(refused(
            "passages_require_activity",
            &solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.talent.preview.recovery.pass.day.yyyymmdd.facet.name.and.activity.id",
                &[],
            ),
        ));
    };
    let prepared = prepare_preview(display_locale, request, paths, context)?;
    if prepared
        .config
        .get("hook")
        .and_then(|hook| hook.get("pre"))
        .is_some()
    {
        // A pre-step can rewrite what the talent reads; passages come only
        // from the source as loaded.
        return Err(refused(
            "activity_prestep_unsupported",
            &solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.talent.preview.recovery.choose.a.talent.that.reads.its.activity.source.without.a.pre",
                &[],
            ),
        ));
    }
    let Some(transcript) = prepared.config.get("transcript").and_then(Value::as_str) else {
        return Err(refused(
            "activity_sources_unavailable",
            &solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.talent.preview.recovery.choose.a.talent.that.loads.its.activity.s.sources",
                &[],
            ),
        ));
    };
    let transcript = transcript
        .strip_prefix(SPARSE_INPUT_NOTE)
        .unwrap_or(transcript);
    let kind = prepared
        .config
        .get("activity")
        .and_then(|record| record.get("activity"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let units = match prepared.passage_layout.as_ref() {
        Some((digest, layout)) => {
            use sha2::Digest;
            let current: [u8; 32] = sha2::Sha256::digest(transcript.as_bytes()).into();
            if &current == digest {
                super::units::source_units_from_layout(transcript, kind, layout)
            } else {
                // Fitting changed the text. The original offsets can no longer
                // substantiate a source channel in this version.
                super::units::source_units_from_channels(
                    kind,
                    None,
                    &[super::units::SourceChannel {
                        channel: "unknown",
                        text: transcript,
                    }],
                )
                .into_iter()
                .map(|(_, unit)| unit)
                .collect()
            }
        }
        None => source_units(transcript, kind),
    };
    let extraction = extract(&units, &prepared.name, generate);
    Ok(ActivityPassages {
        talent: prepared.name,
        day: day.to_owned(),
        facet: facet.to_owned(),
        activity: activity.to_owned(),
        units,
        extraction,
    })
}
