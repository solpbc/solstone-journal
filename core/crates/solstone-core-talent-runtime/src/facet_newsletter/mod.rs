// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Facet-newsletter hook stages.

mod packet;

use serde_json::{Map, Value};

use crate::contract::{CommitPlan, GateDecision, ParsedOutput, PrePostState};
use crate::writers::WriteIntent;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct FacetNewsletterState {
    packet: packet::Packet,
}

pub fn gate(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
) -> Result<GateDecision, StageError> {
    if !context.journal.is_dir() {
        return Ok(GateDecision::Skip(format!(
            "journal unavailable: {} is not a directory",
            context.journal.display()
        )));
    }
    let Some(facet_value) = prepared.config.get("facet") else {
        return Ok(GateDecision::Skip("missing facet".to_owned()));
    };
    let Some(day_value) = prepared.config.get("day") else {
        return Ok(GateDecision::Skip("missing day".to_owned()));
    };
    let facet = facet_value.as_str().unwrap_or_default().trim();
    let day = day_value.as_str().unwrap_or_default().trim();
    if day.is_empty() {
        return Ok(GateDecision::Skip("missing day".to_owned()));
    }
    if !packet::valid_day(day) {
        return Ok(GateDecision::Skip(format!("invalid day: {day}")));
    }
    if packet::unsafe_facet(facet) {
        return Ok(GateDecision::Skip(format!("unsafe facet: {facet}")));
    }
    match packet::gather(&context.journal, facet, day) {
        Ok(packet) if packet.substantive_items == 0 => Ok(GateDecision::Skip(
            "no substantive facet/day sources".to_owned(),
        )),
        Ok(_) => Ok(GateDecision::Proceed),
        // Preserve solstone/talent/facet_newsletter.py:68-76: packet failures retain error detail.
        Err(error) => Ok(GateDecision::Skip(format!(
            "facet newsletter pre-hook failed: {error}"
        ))),
    }
}

pub fn build(
    prepared: &mut PreparedTalent,
    context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let facet = prepared
        .config
        .get("facet")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let day = prepared
        .config
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let packet = packet::gather(&context.journal, facet, day).map_err(|detail| {
        RuntimeOutcome::StageFailed(stage_error("build", "facet_newsletter", prepared, detail))
    })?;
    Ok(PrePostState::FacetNewsletter(FacetNewsletterState {
        packet,
    }))
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::FacetNewsletter(state) = state else {
        return Err(stage_error(
            "prompt_override",
            "facet_newsletter",
            prepared,
            "missing facet newsletter state",
        ));
    };
    apply_template_vars(
        &mut prepared.config,
        &Map::from_iter([
            (
                "source_packet".to_owned(),
                Value::String(state.packet.source_packet.clone()),
            ),
            (
                "source_counts".to_owned(),
                Value::String(state.packet.source_counts.clone()),
            ),
            (
                "source_gaps".to_owned(),
                Value::String(
                    serde_json::to_string(&state.packet.gaps).expect("strings serialize"),
                ),
            ),
            (
                "coverage_preamble".to_owned(),
                Value::String(state.packet.coverage_preamble.clone()),
            ),
        ]),
    );
    Ok(())
}

pub fn parse(
    output: &str,
    _: &PreparedTalent,
    _: &PrePostState,
) -> Result<ParsedOutput, StageError> {
    Ok(ParsedOutput::Text(output.to_owned()))
}
pub fn commit(
    parsed: ParsedOutput,
    prepared: &PreparedTalent,
    _: &PrePostState,
) -> Result<CommitPlan, StageError> {
    let ParsedOutput::Text(output) = parsed else {
        return Err(stage_error(
            "commit",
            "facet_newsletter",
            prepared,
            "expected text output",
        ));
    };
    let Some(facet) = prepared.config.get("facet").and_then(Value::as_str) else {
        return Ok(CommitPlan::NoOutput);
    };
    let Some(day) = prepared.config.get("day").and_then(Value::as_str) else {
        return Ok(CommitPlan::NoOutput);
    };
    Ok(CommitPlan::Write(WriteIntent::FacetNewsletter {
        output,
        facet: facet.to_owned(),
        day: day.to_owned(),
    }))
}

pub fn apply_result(
    journal: &std::path::Path,
    output: &str,
    facet: &str,
    day: &str,
) -> Result<(), String> {
    // Preserve solstone/talent/facet_newsletter.py:880-912: blank and sentinel results do not write.
    let content = output.trim();
    if facet.is_empty() || day.is_empty() || content.is_empty() || content == "No activity" {
        return Ok(());
    }
    solstone_core_facets::write_news_file(journal, facet, &format!("{day}.md"), content)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_newsletter_file_not_just_a_commit_result() {
        // Derived from solstone/talent/facet_newsletter.py:880-912.
        let root = tempfile::tempdir().unwrap();
        apply_result(root.path(), "# Daily\n", "work", "20260101").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("facets/work/news/20260101.md")).unwrap(),
            "# Daily"
        );
    }
    #[test]
    fn packet_failure_and_no_sources_have_distinct_gate_reasons() {
        // Derived from solstone/talent/facet_newsletter.py:68-79.
        let prepared = PreparedTalent {
            name: "facet_newsletter".to_owned(),
            config: Map::from_iter([
                ("facet".to_owned(), Value::String("work".to_owned())),
                ("day".to_owned(), Value::String("20260101".to_owned())),
            ]),
        };
        let root = tempfile::tempdir().unwrap();
        let no_sources = gate(
            &prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap();
        std::fs::write(root.path().join("facets"), "not a directory").unwrap();
        let failed = gate(
            &prepared,
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap();
        assert_ne!(no_sources, failed);
    }
}
