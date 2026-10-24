// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared provider dispatch for process and app-owned Generate calls.

use serde_json::{Value, json};
use solstone_core_journal_config::read_journal_config;
use solstone_core_local::LocalInferenceAuthority;
use std::path::Path;

/// Admission and cancellation belonging to one hosted call, never process globals.
pub struct GenerateCallContext<'a> {
    pub priority: solstone_core_journal_config::work_priority::WorkPriority,
    pub cancellation: &'a solstone_core_generate::Cancellation<'a>,
}

/// Resolve the journal's configured lane and run the production Generate policy.
/// The local authority factory is called only for bundled local inference.
///
/// # Errors
/// Returns configuration, response encoding or local-runtime errors. Provider
/// and attestation refusals remain typed Generate responses. A definitive
/// rejection whose journal record could not be published is returned as Err.
pub fn respond(
    request: &solstone_core_generate::GenerateRequest,
    endpoint_runtime: &crate::EndpointRuntime,
    journal: &Path,
    acquire_local_inference_authority: impl FnOnce(&Path) -> Option<LocalInferenceAuthority>,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    let config = match read_journal_config(journal) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => return Err(format!("could not read journal config: {error}")),
    };
    respond_config(
        request,
        endpoint_runtime,
        journal,
        &config,
        acquire_local_inference_authority,
        true,
        true,
        &GenerateCallContext {
            priority: solstone_core_journal_config::work_priority::WorkPriority::from_environment(
            )?,
            cancellation,
        },
    )
}

/// Run the same dispatch against an explicit owner-selected configuration.
/// Process overrides are ignored. Arbitrary content is admitted and retained
/// for the caller-supplied configuration. The existing exact canned probe is
/// the only exemption.
///
/// # Errors
/// Returns response encoding or local-runtime errors; provider refusals stay typed.
/// A definitive rejection whose journal record could not be published is returned as Err.
pub fn respond_with_config(
    request: &solstone_core_generate::GenerateRequest,
    endpoint_runtime: &crate::EndpointRuntime,
    journal: &Path,
    config: &serde_json::Map<String, Value>,
    acquire_local_inference_authority: impl FnOnce(&Path) -> Option<LocalInferenceAuthority>,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    respond_config(
        request,
        endpoint_runtime,
        journal,
        config,
        acquire_local_inference_authority,
        false,
        true,
        &GenerateCallContext {
            priority: solstone_core_journal_config::work_priority::WorkPriority::from_environment(
            )?,
            cancellation,
        },
    )
}

/// Dispatch an app-owned call with its admitted task priority and owner configuration.
///
/// # Errors
/// Returns configuration, response encoding or local-runtime errors.
pub fn respond_with_context(
    request: &solstone_core_generate::GenerateRequest,
    endpoint_runtime: &crate::EndpointRuntime,
    journal: &Path,
    acquire_local_inference_authority: impl FnOnce(&Path) -> Option<LocalInferenceAuthority>,
    context: &GenerateCallContext<'_>,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    let config = read_journal_config(journal)
        .map_err(|error| format!("could not read journal config: {error}"))?
        .config
        .unwrap_or_default();
    respond_config(
        request,
        endpoint_runtime,
        journal,
        &config,
        acquire_local_inference_authority,
        false,
        true,
        context,
    )
}

#[allow(clippy::too_many_arguments)]
fn respond_config(
    request: &solstone_core_generate::GenerateRequest,
    endpoint_runtime: &crate::EndpointRuntime,
    journal: &Path,
    config: &serde_json::Map<String, Value>,
    acquire_local_inference_authority: impl FnOnce(&Path) -> Option<LocalInferenceAuthority>,
    process_overrides: bool,
    admit_content: bool,
    context: &GenerateCallContext<'_>,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    let cancellation = context.cancellation;
    if cancellation.is_cancelled() {
        return Ok(solstone_core_generate::GenerateResponse::Cancelled);
    }
    let gate = crate::credential_binding::ContentGate {
        journal,
        admit_content,
        process_overrides,
        cancellation,
    };
    let request_id = request.id.clone();
    let (provider, outcome) = if process_overrides {
        crate::resolve_lane(config)
    } else {
        crate::lane::resolve_lane_with(config, |_| None)
    };
    if !matches!(outcome, crate::LaneOutcome::ConfidentialEndpoint(_)) {
        endpoint_runtime.drain_idle_pool();
    }
    let response = match outcome {
        crate::LaneOutcome::BundledLocal => {
            let mut authority = acquire_local_inference_authority(journal);
            match crate::bundled_generate_with_authority(request, journal, authority.as_mut()) {
                Ok(solstone_core_local::GenerateResult::Success(mut success)) => {
                    if cancellation.is_cancelled() {
                        return Ok(solstone_core_generate::GenerateResponse::Cancelled);
                    }
                    let usage = success
                        .usage
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(|error| error.to_string())?
                        .unwrap_or_else(|| json!({}));
                    let assessment = crate::assess_provider_result(crate::ProviderResultView {
                        journal_path: journal,
                        context: &request.context,
                        model: &success.model,
                        text: &success.text,
                        finish_reason: &success.finish_reason,
                        usage: &usage,
                        json_output: request.json_output,
                        raw_response_snippet: None,
                        thinking_seen: false,
                    });
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                            &crate::LaneOutcome::ValidationFailure(failure),
                            &provider,
                            request_id.clone(),
                        ))
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response = generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                Ok(solstone_core_local::GenerateResult::Failure(failure)) => {
                    solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                        &crate::LaneOutcome::BundledFailure(Box::new(failure)),
                        &provider,
                        request_id.clone(),
                    ))
                }
                Err(error) => {
                    return Err(format!("bundled local generate is unavailable: {error:?}"));
                }
            }
        }
        crate::LaneOutcome::ByoEndpoint(endpoint) => endpoint_result_response(
            crate::endpoint::endpoint_generate_with_context(
                request,
                journal,
                &endpoint,
                config,
                endpoint_runtime,
                context,
                Some(&gate),
            )?,
            request,
            journal,
            &provider,
            request_id.clone(),
            cancellation,
        )?,
        crate::LaneOutcome::ConfidentialEndpoint(endpoint) => {
            match crate::confidential::confidential_generate_with_context(
                request,
                journal,
                &endpoint,
                config,
                endpoint_runtime,
                context,
            ) {
                crate::ConfidentialResult::Generated(success) => endpoint_result_response(
                    crate::EndpointResult::Generated(success),
                    request,
                    journal,
                    &provider,
                    request_id.clone(),
                    cancellation,
                )?,
                crate::ConfidentialResult::Failed(failure) => endpoint_result_response(
                    crate::EndpointResult::Failed(failure),
                    request,
                    journal,
                    &provider,
                    request_id.clone(),
                    cancellation,
                )?,
                crate::ConfidentialResult::AttestationNotVerified(detail) => {
                    solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                        &crate::LaneOutcome::AttestationNotVerified(detail),
                        &provider,
                        request_id.clone(),
                    ))
                }
                crate::ConfidentialResult::AttestationFailed(detail) => {
                    solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                        &crate::LaneOutcome::AttestationFailed(detail),
                        &provider,
                        request_id.clone(),
                    ))
                }
            }
        }
        crate::LaneOutcome::Anthropic => match crate::anthropic::anthropic_generate_admitted(
            request,
            config,
            process_overrides,
            &gate,
        )? {
            crate::AnthropicResult::Generated(mut success) => {
                if cancellation.is_cancelled() {
                    return Ok(solstone_core_generate::GenerateResponse::Cancelled);
                }
                let assessment = crate::assess_provider_result(crate::ProviderResultView {
                    journal_path: journal,
                    context: &request.context,
                    model: &success.model,
                    text: &success.text,
                    finish_reason: &success.finish_reason,
                    usage: &success.usage,
                    json_output: request.json_output,
                    raw_response_snippet: success.raw_response_snippet.as_deref(),
                    thinking_seen: success.thinking.is_some(),
                });
                if let Some(error) = assessment.token_log_error {
                    eprintln!("generate token usage log failed: {error}");
                }
                if let Some(failure) = assessment.failure {
                    solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                        &crate::LaneOutcome::ValidationFailure(failure),
                        &provider,
                        request_id.clone(),
                    ))
                } else {
                    let schema_validation =
                        apply_schema_validation(&mut success.text, request.json_schema.as_ref());
                    let response =
                        anthropic_generated_response(request, success, schema_validation)?;
                    solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                }
            }
            crate::AnthropicResult::Failed(failure) => {
                solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                    &crate::LaneOutcome::AnthropicFailure(failure),
                    &provider,
                    request_id.clone(),
                ))
            }
        },
        crate::LaneOutcome::OpenAi => {
            match crate::openai::openai_generate_admitted(
                request,
                config,
                process_overrides,
                &gate,
            )? {
                crate::OpenAiResult::Generated(mut success) => {
                    if cancellation.is_cancelled() {
                        return Ok(solstone_core_generate::GenerateResponse::Cancelled);
                    }
                    let assessment = crate::assess_provider_result(crate::ProviderResultView {
                        journal_path: journal,
                        context: &request.context,
                        model: &success.model,
                        text: &success.text,
                        finish_reason: &success.finish_reason,
                        usage: &success.usage,
                        json_output: request.json_output,
                        raw_response_snippet: success.raw_response_snippet.as_deref(),
                        thinking_seen: false,
                    });
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                            &crate::LaneOutcome::ValidationFailure(failure),
                            &provider,
                            request_id.clone(),
                        ))
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response =
                            openai_generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                crate::OpenAiResult::Failed(failure) => {
                    solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                        &crate::LaneOutcome::OpenAiFailure(failure),
                        &provider,
                        request_id.clone(),
                    ))
                }
            }
        }
        crate::LaneOutcome::Google => {
            match crate::google::google_generate_admitted(
                request,
                config,
                process_overrides,
                &gate,
            )? {
                crate::GoogleResult::Generated(mut success) => {
                    if cancellation.is_cancelled() {
                        return Ok(solstone_core_generate::GenerateResponse::Cancelled);
                    }
                    let assessment = crate::assess_provider_result(crate::ProviderResultView {
                        journal_path: journal,
                        context: &request.context,
                        model: &success.model,
                        text: &success.text,
                        finish_reason: &success.finish_reason,
                        usage: &success.usage,
                        json_output: request.json_output,
                        raw_response_snippet: success.raw_response_snippet.as_deref(),
                        thinking_seen: false,
                    });
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                            &crate::LaneOutcome::ValidationFailure(failure),
                            &provider,
                            request_id.clone(),
                        ))
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response =
                            google_generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                crate::GoogleResult::Failed(failure) => {
                    solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                        &crate::LaneOutcome::GoogleFailure(failure),
                        &provider,
                        request_id.clone(),
                    ))
                }
            }
        }
        crate::LaneOutcome::ChatGpt => match if process_overrides {
            crate::chatgpt_generate(request, config, journal)
        } else {
            crate::chatgpt::chatgpt_generate_from_config(request, config, journal)
        } {
            crate::ChatGptResult::Generated(mut success) => {
                if cancellation.is_cancelled() {
                    return Ok(solstone_core_generate::GenerateResponse::Cancelled);
                }
                let assessment = crate::assess_provider_result(crate::ProviderResultView {
                    journal_path: journal,
                    context: &request.context,
                    model: &success.model,
                    text: &success.text,
                    finish_reason: &success.finish_reason,
                    usage: &success.usage,
                    json_output: request.json_output,
                    raw_response_snippet: success.raw_response_snippet.as_deref(),
                    thinking_seen: false,
                });
                if let Some(error) = assessment.token_log_error {
                    eprintln!("generate token usage log failed: {error}");
                }
                if let Some(failure) = assessment.failure {
                    solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                        &crate::LaneOutcome::ValidationFailure(failure),
                        &provider,
                        request_id.clone(),
                    ))
                } else {
                    let schema_validation =
                        apply_schema_validation(&mut success.text, request.json_schema.as_ref());
                    let response = openai_generated_response(request, success, schema_validation)?;
                    solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                }
            }
            crate::ChatGptResult::Failed(failure) => {
                solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                    &crate::LaneOutcome::ChatGptFailure(failure),
                    &provider,
                    request_id.clone(),
                ))
            }
        },
        crate::LaneOutcome::NoEngine
        | crate::LaneOutcome::AttestationNotVerified(_)
        | crate::LaneOutcome::AttestationFailed(_)
        | crate::LaneOutcome::AttestationStale
        | crate::LaneOutcome::UnimplementedLane => {
            solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                &outcome,
                &provider,
                request_id.clone(),
            ))
        }
        crate::LaneOutcome::BundledFailure(_)
        | crate::LaneOutcome::EndpointFailure(_)
        | crate::LaneOutcome::AnthropicFailure(_)
        | crate::LaneOutcome::OpenAiFailure(_)
        | crate::LaneOutcome::GoogleFailure(_)
        | crate::LaneOutcome::ChatGptFailure(_)
        | crate::LaneOutcome::ValidationFailure(_) => {
            unreachable!("lane resolution cannot return an arm failure")
        }
    };
    if matches!(
        response,
        solstone_core_generate::GenerateResponse::Generated(_)
    ) && cancellation.is_cancelled()
    {
        return Ok(solstone_core_generate::GenerateResponse::Cancelled);
    }
    Ok(response)
}

fn endpoint_result_response(
    result: crate::EndpointResult,
    request: &solstone_core_generate::GenerateRequest,
    journal: &std::path::Path,
    provider: &str,
    request_id: Option<String>,
    cancellation: &solstone_core_generate::Cancellation,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    if cancellation.is_cancelled() {
        return Ok(solstone_core_generate::GenerateResponse::Cancelled);
    }
    match result {
        crate::EndpointResult::Generated(mut success) => {
            let usage = success
                .usage
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|error| error.to_string())?
                .unwrap_or_else(|| json!({}));
            let assessment = crate::assess_provider_result(crate::ProviderResultView {
                journal_path: journal,
                context: &request.context,
                model: &success.model,
                text: &success.text,
                finish_reason: &success.finish_reason,
                usage: &usage,
                json_output: request.json_output,
                raw_response_snippet: None,
                thinking_seen: false,
            });
            if let Some(error) = assessment.token_log_error {
                eprintln!("generate token usage log failed: {error}");
            }
            if let Some(failure) = assessment.failure {
                Ok(solstone_core_generate::GenerateResponse::Refused(
                    crate::refusal_for(
                        &crate::LaneOutcome::ValidationFailure(failure),
                        provider,
                        request_id,
                    ),
                ))
            } else {
                let schema_validation =
                    apply_schema_validation(&mut success.text, request.json_schema.as_ref());
                let response = endpoint_generated_response(request, success, schema_validation)?;
                Ok(solstone_core_generate::GenerateResponse::Generated(
                    Box::new(response),
                ))
            }
        }
        crate::EndpointResult::Failed(failure) => Ok(
            solstone_core_generate::GenerateResponse::Refused(crate::refusal_for(
                &crate::LaneOutcome::EndpointFailure(failure),
                provider,
                request_id,
            )),
        ),
    }
}

fn generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_local::GenerateSuccess,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let usage = success
        .usage
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| json!({}));
    let input_budget = success
        .input_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let request_budget = serde_json::to_value(&success.request_budget)
        .map(Some)
        .map_err(|error| error.to_string())?;
    let inference = serde_json::to_value(&success.inference)
        .map(Some)
        .map_err(|error| error.to_string())?;
    let hints_applied = applied_hints(request, true, true);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage,
        finish_reason: success.finish_reason,
        thinking: None,
        schema_validation,
        input_budget,
        request_budget,
        inference,
        hints_applied,
    })
}

/// Which request hints the boundary actually applied.
///
/// 🔴 A hint is reported only by a lane that HONOURED it, and the lanes differ:
/// measured, `bundled` reads `exclusive_admission` and the attempt index,
/// `endpoint` reads `exclusive_admission` to size its admission slot, and the
/// four cloud lanes read neither — their only mention of it is a test fixture.
///
/// ⚠ Two wrong rules were tried before this one, and both passed a real test
/// suite. The original reported `exclusive_admission` from every lane, so the
/// cloud lanes claimed an effect they never had. The correction after that keyed
/// off the response carrying an inference block, which is how the Python
/// reference decided it — but that gates on the *shape of the result*, not on
/// what was honoured, and it silenced the endpoint lane, which genuinely does
/// acquire the slot. ⛔ Do not re-derive this from the reference; it
/// under-reported.
fn applied_hints(
    request: &solstone_core_generate::GenerateRequest,
    honours_attempt_index: bool,
    honours_exclusive_admission: bool,
) -> Vec<String> {
    let mut hints = Vec::new();
    if honours_attempt_index && request.attempt_index != 0 {
        hints.push("attempt_index".to_owned());
    }
    if honours_exclusive_admission && request.exclusive_admission {
        hints.push("exclusive_admission".to_owned());
    }
    hints
}

fn endpoint_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: crate::EndpointGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let usage = success
        .usage
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| json!({}));
    let input_budget = success
        .input_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let request_budget = success
        .request_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let hints_applied = applied_hints(request, false, true);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage,
        finish_reason: success.finish_reason,
        thinking: None,
        schema_validation,
        input_budget,
        request_budget,
        inference: None,
        hints_applied,
    })
}

fn anthropic_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: crate::AnthropicGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn openai_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: crate::OpenAiGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn google_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: crate::GoogleGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn apply_schema_validation(text: &mut String, schema: Option<&Value>) -> Option<Value> {
    schema.map(|schema| {
        let result = crate::validate_schema_with_annotations(text, schema);
        *text = result.text;
        log_schema_validation(&result.validation);
        result.validation
    })
}

/// Schema validation is advisory, so a failure never changes the outcome and is
/// visible only in the response record — which reaches the caller and nothing
/// else.
///
/// ⚠ The reference emitted one operator-visible line per error and the port
/// dropped it. Nothing failed: the record still carried the block, so the
/// contract-level assertions passed and only the diagnostic disappeared. ⛔ On
/// stderr, never stdout — a line on stdout would corrupt the response stream,
/// which is what the test guarding this is named for.
fn log_schema_validation(validation: &Value) {
    let Some(errors) = validation.get("errors").and_then(Value::as_array) else {
        return;
    };
    for error in errors {
        let path = error.get("path").and_then(Value::as_str).unwrap_or("");
        let constraint = error
            .get("constraint")
            .and_then(Value::as_str)
            .unwrap_or("");
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        eprintln!("schema_validation: {path}: {constraint}: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn anthropic_generated_response_preserves_thinking_blocks() {
        let request = solstone_core_generate::GenerateRequest {
            id: Some("request".into()),
            context: "context".into(),
            contents: Vec::new(),
            system_instruction: None,
            temperature: 0.0,
            max_output_tokens: 1,
            timeout_s: None,
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        };
        let response = anthropic_generated_response(
            &request,
            crate::AnthropicGenerated {
                text: "text".into(),
                model: "model".into(),
                usage: json!({"input_tokens": 1}),
                finish_reason: "stop".into(),
                thinking: Some(json!({"type": "thinking", "thinking": "work"})),
                raw_response_snippet: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            response.thinking,
            Some(json!({"type": "thinking", "thinking": "work"}))
        );
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn direct_generate_cancellation_discards_provider_success() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Condvar, Mutex};
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("local addr");
        listener.set_nonblocking(true).unwrap();

        struct SyncBarrier {
            entered: Mutex<bool>,
            entered_cvar: Condvar,
            released: Mutex<bool>,
            released_cvar: Condvar,
        }
        let sync_barrier = Arc::new(SyncBarrier {
            entered: Mutex::new(false),
            entered_cvar: Condvar::new(),
            released: Mutex::new(false),
            released_cvar: Condvar::new(),
        });
        let sync_barrier_clone = Arc::clone(&sync_barrier);

        let server_thread = thread::spawn(move || {
            fn accept_request(listener: &TcpListener) -> std::net::TcpStream {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => return stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "provider request did not arrive"
                            );
                            thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(error) => panic!("provider accept failed: {error}"),
                    }
                }
            }
            fn read_request(stream: &mut std::net::TcpStream) {
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    assert!(header.len() < 8192, "bounded fixture request headers");
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = std::str::from_utf8(&header).unwrap();
                assert!(header.starts_with("POST "));
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .expect("fixture request content length");
                assert!(length < 16384, "bounded fixture request body");
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                serde_json::from_slice::<serde_json::Value>(&body).unwrap();
            }
            // First run: provider blocks until released
            let mut stream = accept_request(&listener);
            read_request(&mut stream);
            {
                let mut entered = sync_barrier_clone.entered.lock().unwrap();
                *entered = true;
                sync_barrier_clone.entered_cvar.notify_all();
            }
            {
                let released = sync_barrier_clone.released.lock().unwrap();
                let (released, _) = sync_barrier_clone
                    .released_cvar
                    .wait_timeout_while(released, std::time::Duration::from_secs(10), |released| {
                        !*released
                    })
                    .unwrap();
                assert!(*released, "provider release did not arrive");
            }
            let body = json!({
                "choices": [{"message": {"content": "provider output"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 5, "total_tokens": 10},
            })
            .to_string();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
            drop(stream);

            // Second run: provider returns success directly
            let mut stream2 = accept_request(&listener);
            read_request(&mut stream2);
            let _ = stream2.write_all(header.as_bytes());
            let _ = stream2.write_all(body.as_bytes());
            let _ = stream2.flush();
            drop(stream2);
        });

        let temp_journal = std::path::PathBuf::from(format!(
            "/var/tmp/solstone-generate-wire-dispatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&temp_journal);
        std::fs::create_dir_all(&temp_journal).unwrap();
        let journal_path = temp_journal.as_path();
        let config = json!({
            "providers": {
                "active": {"provider": "local"},
                "local": {
                    "endpoint_url": format!("http://{addr}"),
                    "served_model_id": "served",
                    "served_context_window": 4096
                }
            }
        })
        .as_object()
        .unwrap()
        .clone();
        std::fs::create_dir_all(temp_journal.join("config")).unwrap();
        std::fs::write(
            temp_journal.join("config/journal.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();

        let request = solstone_core_generate::GenerateRequest {
            id: Some("req-1".into()),
            context: "test".into(),
            contents: vec![solstone_core_generate::ContentPart::Text {
                text: "hello".into(),
            }],
            system_instruction: None,
            temperature: 0.0,
            max_output_tokens: 16,
            timeout_s: Some(10.0),
            json_output: false,
            json_schema: None,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        };

        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = Arc::clone(&cancelled);

        let req_clone = request.clone();
        let config_clone = config.clone();
        let runtime = crate::EndpointRuntime::default();
        let jpath = journal_path.to_path_buf();
        let client_thread = thread::spawn(move || {
            let check_fn = move || cancelled_clone.load(Ordering::SeqCst);
            let cancellation = solstone_core_generate::Cancellation::from_fn(&check_fn);
            respond_with_config(
                &req_clone,
                &runtime,
                &jpath,
                &config_clone,
                |_| None,
                &cancellation,
            )
        });

        // Wait until provider blocks
        {
            let entered = sync_barrier.entered.lock().unwrap();
            let (entered, _) = sync_barrier
                .entered_cvar
                .wait_timeout_while(entered, std::time::Duration::from_secs(10), |entered| {
                    !*entered
                })
                .unwrap();
            assert!(
                *entered,
                "provider did not receive the cancellation control request"
            );
        }

        // Test cancels
        cancelled.store(true, Ordering::SeqCst);

        // Test releases
        {
            let mut released = sync_barrier.released.lock().unwrap();
            *released = true;
            sync_barrier.released_cvar.notify_all();
        }

        let result = client_thread
            .join()
            .unwrap()
            .expect("respond_with_config ok");
        assert_eq!(result, solstone_core_generate::GenerateResponse::Cancelled);

        // The same fixture without cancel returns the provider success
        let runtime2 = crate::EndpointRuntime::default();
        let success_result = respond_with_config(
            &request,
            &runtime2,
            journal_path,
            &config,
            |_| None,
            &solstone_core_generate::Cancellation::never(),
        )
        .expect("respond_with_config ok");

        let solstone_core_generate::GenerateResponse::Generated(generated) = success_result else {
            panic!("expected Generated response, got {success_result:?}");
        };
        assert_eq!(generated.text, "provider output");

        server_thread.join().unwrap();
        let _ = std::fs::remove_dir_all(&temp_journal);
    }
}
