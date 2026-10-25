// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::Candidate;
use crate::usage::TrackingExecutor;
use crate::{GeneratedTalentResponse, PreparedTalent, RuntimeOutcome};
use serde_json::json;
use solstone_core_generate::{
    Cancellation, GenerateExecutor, GenerateResponse, ReasonCodeValue, RefusedResponse,
};
use std::io::Write;

pub(crate) fn execute(
    candidate: Candidate,
    prepared: &mut PreparedTalent,
    generate: &dyn GenerateExecutor,
    writer: &mut impl Write,
    cancellation: &Cancellation,
) -> Result<GeneratedTalentResponse, RuntimeOutcome> {
    let tracking = TrackingExecutor::new(generate);
    let result = run(&candidate, prepared, &tracking, writer, cancellation);
    let usage = tracking.usage();
    if let Some(value) = &usage {
        crate::emit(
            writer,
            json!({"event":"terminal_sense_usage","usage":value}),
        );
    }
    match result {
        Ok((text, _, degraded)) => Ok((text, usage, degraded)),
        Err(mut outcome) => {
            match &mut outcome {
                RuntimeOutcome::StageFailed(e)
                | RuntimeOutcome::GenerateRefused { error: e, .. } => e.usage = usage,
                _ => {}
            }
            Err(outcome)
        }
    }
}
fn run(
    candidate: &Candidate,
    prepared: &mut PreparedTalent,
    generate: &dyn GenerateExecutor,
    writer: &mut impl Write,
    cancellation: &Cancellation,
) -> Result<GeneratedTalentResponse, RuntimeOutcome> {
    if cancellation.is_cancelled() {
        return Err(RuntimeOutcome::Cancelled);
    }
    crate::emit(
        writer,
        json!({"event":"terminal_sense","branch":"complete_timeline","anchored":candidate.anchored(),"binding":candidate.binding_fingerprint()}),
    );
    crate::emit_generate_input(writer, &candidate.request);
    let response = crate::execute_bounded_attempts(
        &candidate.request,
        None,
        |_, request| {
            if cancellation.is_cancelled() {
                return Ok(GenerateResponse::Cancelled);
            }
            generate
                .execute(request)
                .map_err(|e| crate::stage_error("generate", "runtime", prepared, e.to_string()))
        },
        |event| crate::emit(writer, event),
    )?;
    if cancellation.is_cancelled() {
        return Err(RuntimeOutcome::Cancelled);
    }
    let cause = match response {
        GenerateResponse::Cancelled => return Err(RuntimeOutcome::Cancelled),
        GenerateResponse::Generated(response) => match candidate.render(&response.text) {
            Ok(text) => {
                // An unanchored account is ordinary Sense output, checked like any.
                let text = if candidate.anchored() {
                    text
                } else {
                    crate::verified_product_quotes(prepared, &text, writer)?
                };
                crate::record_input_budget(prepared, response.input_budget.as_ref());
                return Ok((text, None, None));
            }
            Err(_) if candidate.anchored() => "invalid_source_binding",
            Err(_) => "invalid_output",
        },
        GenerateResponse::Refused(response) if exceeds_context_window(&response) => {
            "input_exceeds_context_window"
        }
        GenerateResponse::Refused(response) => {
            return Err(RuntimeOutcome::GenerateRefused {
                error: crate::stage_error("generate", "runtime", prepared, response.detail.clone()),
                response: Box::new(response),
            });
        }
    };
    if cancellation.is_cancelled() {
        return Err(RuntimeOutcome::Cancelled);
    }
    crate::emit(
        writer,
        json!({"event":"terminal_sense","branch":"native_fallback","cause":cause}),
    );
    crate::native_generate_response(prepared, generate, writer)
}

/// Fitting trims the timeline like any input. These refusals mean even the
/// preserved instructions, cards and passages do not fit the context window.
fn exceeds_context_window(response: &RefusedResponse) -> bool {
    matches!(
        response.reason_code.as_ref().map(ReasonCodeValue::as_wire),
        Some(
            "context_budget_exceeded"
                | "context_fitted_overflow"
                | "context_preserved_overflow"
                | "context_window_exceeded"
        )
    )
}
