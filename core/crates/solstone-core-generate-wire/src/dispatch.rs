// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared provider dispatch for process and app-owned Generate calls.

use serde_json::{Value, json};
use solstone_core_journal_config::read_journal_config;
use solstone_core_local::LocalInferenceAuthority;
use std::borrow::Cow;
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
        None,
        &GenerateCallContext {
            priority: solstone_core_journal_config::work_priority::WorkPriority::from_environment(
            )?,
            cancellation,
        },
        Turn::Wait { may_forward: true },
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
        None,
        &GenerateCallContext {
            priority: solstone_core_journal_config::work_priority::WorkPriority::from_environment(
            )?,
            cancellation,
        },
        // A caller's own configuration is not the supervisor's to serve.
        Turn::Wait { may_forward: false },
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
    respond_with_mobile_local_timeout(
        request,
        endpoint_runtime,
        journal,
        acquire_local_inference_authority,
        context,
        None,
    )
}

/// Carry a trusted talent allowance to the actual lane selection. The wire
/// request retains its base timeout and exposes no allowance field.
///
/// # Errors
/// Returns invalid-allowance, configuration and provider-dispatch errors.
pub fn respond_with_mobile_local_timeout(
    request: &solstone_core_generate::GenerateRequest,
    endpoint_runtime: &crate::EndpointRuntime,
    journal: &Path,
    acquire_local_inference_authority: impl FnOnce(&Path) -> Option<LocalInferenceAuthority>,
    context: &GenerateCallContext<'_>,
    mobile_local_timeout: Option<std::time::Duration>,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    if mobile_local_timeout.is_some_and(|timeout| {
        timeout.is_zero() || std::time::Duration::try_from_secs_f64(timeout.as_secs_f64()).is_err()
    }) {
        return Err("mobile local timeout is outside the supported range".to_owned());
    }
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
        mobile_local_timeout,
        context,
        Turn::Wait { may_forward: true },
    )
}

/// Whether a call still waits for its turn in the journal's call line, or a
/// caller elsewhere already holds that turn for it.
#[derive(Clone, Copy)]
enum Turn {
    /// Wait in the line. A confidential call on the journal's own
    /// configuration may then hand the call to the journal's supervisor.
    Wait { may_forward: bool },
    /// The supervisor serving a confidential call another process has taken
    /// its turn for: waiting again would wait on that caller.
    #[cfg_attr(not(unix), allow(dead_code))]
    Held,
}

/// Serve a confidential call handed over by a process that already holds its
/// turn in the call line, on this process's one channel.
#[cfg(unix)]
pub(crate) fn respond_in_held_turn(
    request: &solstone_core_generate::GenerateRequest,
    endpoint_runtime: &crate::EndpointRuntime,
    journal: &Path,
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
        |_| None,
        false,
        true,
        None,
        context,
        Turn::Held,
    )
}

/// Whether the supervisor, which reads the journal's saved configuration with
/// no process overrides, would choose the same confidential engine.
fn forwardable(config: &serde_json::Map<String, Value>, process_overrides: bool) -> bool {
    use crate::overrides::{
        API_KEY_OVERRIDE_ENV, BASE_URL_OVERRIDE_ENV, MODEL_OVERRIDE_ENV, PROVIDER_OVERRIDE_ENV,
    };
    !process_overrides
        || ([
            API_KEY_OVERRIDE_ENV,
            BASE_URL_OVERRIDE_ENV,
            MODEL_OVERRIDE_ENV,
            PROVIDER_OVERRIDE_ENV,
        ]
        .iter()
        .all(|name| crate::overrides::non_blank_process_env(name).is_none())
            && matches!(
                crate::lane::resolve_lane_with(config, |_| None).1,
                crate::LaneOutcome::ConfidentialEndpoint(_)
            ))
}

/// The owner's provider test and the health check: short calls that report
/// whether an engine answers, not thinking.
fn connection_check(request: &solstone_core_generate::GenerateRequest) -> bool {
    crate::credential_binding::canned_probe(request)
        || request.context == solstone_core_generate::HEALTH_BRAIN_GENERATE_CONTEXT
}

/// Whether this outcome reaches a model; refusals and missing engines do not.
fn calls_a_model(outcome: &crate::LaneOutcome) -> bool {
    matches!(
        outcome,
        crate::LaneOutcome::BundledLocal
            | crate::LaneOutcome::ByoEndpoint(_)
            | crate::LaneOutcome::ConfidentialEndpoint(_)
            | crate::LaneOutcome::Anthropic
            | crate::LaneOutcome::OpenAi
            | crate::LaneOutcome::Google
            | crate::LaneOutcome::ChatGpt
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
    mobile_local_timeout: Option<std::time::Duration>,
    context: &GenerateCallContext<'_>,
    turn: Turn,
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
    let routed_request = request_for_lane(request, &outcome, mobile_local_timeout);
    let request = routed_request.as_ref();
    // Leaving confidential processing closes its idle connection; a check of
    // another engine is not leaving it.
    if !matches!(outcome, crate::LaneOutcome::ConfidentialEndpoint(_)) && !connection_check(request)
    {
        endpoint_runtime.drain_idle_pool();
    }
    // Every engine's call takes its turn in the journal's one call line, best
    // priority first. Only connection checks go straight through, so a health
    // check or a provider test never waits behind a long call.
    let _turn = if matches!(turn, Turn::Wait { .. })
        && calls_a_model(&outcome)
        && !connection_check(request)
    {
        match solstone_core_local::admission::wait_for_model_call(
            journal,
            context.priority,
            &|| cancellation.is_cancelled(),
        ) {
            Ok(turn) => Some(turn),
            Err(solstone_core_local::admission::CheckedAdmissionError::Cancelled) => {
                return Ok(solstone_core_generate::GenerateResponse::Cancelled);
            }
            // The line orders calls; it is no safety boundary. A journal whose
            // line cannot be opened still makes its call, unordered.
            Err(solstone_core_local::admission::CheckedAdmissionError::Admission(error)) => {
                log::warn!(
                    "model call line is unavailable, so this call runs unordered: {error:?}"
                );
                None
            }
        }
    } else {
        None
    };
    let response = match outcome {
        crate::LaneOutcome::BundledLocal => {
            let mut authority = acquire_local_inference_authority(journal);
            match crate::bundled::bundled_generate_with_context(
                request,
                journal,
                authority.as_mut(),
                context,
            ) {
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
                    if cfg!(target_os = "ios") && cancellation.is_cancelled() {
                        return Ok(solstone_core_generate::GenerateResponse::Cancelled);
                    }
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
            // A Generate process hands its call to the journal's supervisor,
            // which keeps the journal's one confidential connection.
            let forwarded = (matches!(turn, Turn::Wait { may_forward: true })
                && !connection_check(request)
                && forwardable(config, process_overrides))
            .then(|| crate::broker::forward(journal, request, context))
            .flatten();
            if let Some(response) = forwarded {
                response?
            } else {
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

fn request_for_lane<'a>(
    request: &'a solstone_core_generate::GenerateRequest,
    lane: &crate::LaneOutcome,
    mobile_local_timeout: Option<std::time::Duration>,
) -> Cow<'a, solstone_core_generate::GenerateRequest> {
    if let Some(timeout) = mobile_local_timeout
        && matches!(lane, crate::LaneOutcome::BundledLocal)
    {
        let mut selected = request.clone();
        selected.timeout_s = Some(timeout.as_secs_f64());
        Cow::Owned(selected)
    } else {
        Cow::Borrowed(request)
    }
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

    fn request() -> solstone_core_generate::GenerateRequest {
        solstone_core_generate::GenerateRequest {
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
            overflow_text: None,
            transport_retries: None,
        }
    }

    #[test]
    fn every_call_waits_its_turn_in_the_line_but_a_connection_check_does_not() {
        let journal = std::env::temp_dir().join(format!(
            "solstone-dispatch-line-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&journal).unwrap();
        // An endpoint nothing serves: each call fails at once once it runs.
        let config = json!({"providers":{"active":{"provider":"local"},"local":{
            "endpoint_url":"http://127.0.0.1:9","served_model_id":"model"}}});
        let config = config.as_object().unwrap().clone();
        let holder = solstone_core_local::admission::wait_for_model_call(
            &journal,
            solstone_core_journal_config::work_priority::WorkPriority::default(),
            &|| false,
        )
        .unwrap();
        let call = |request: solstone_core_generate::GenerateRequest| {
            let journal = journal.clone();
            let config = config.clone();
            std::thread::spawn(move || {
                respond_with_config(
                    &request,
                    &crate::EndpointRuntime::new(1),
                    &journal,
                    &config,
                    |_| None,
                    &solstone_core_generate::Cancellation::never(),
                )
                .unwrap()
            })
        };
        let mut thinking = request();
        thinking.contents = vec![solstone_core_generate::ContentPart::Text {
            text: "think".into(),
        }];
        // The owner's provider test, exactly as settings sends it.
        let mut probe = request();
        probe.context = "settings.cloud.validate_key".into();
        probe.contents = vec![solstone_core_generate::ContentPart::Text {
            text: crate::credential_binding::CANNED_TEXT.into(),
        }];
        probe.max_output_tokens = 512;
        probe.transport_retries = Some(0);
        assert!(connection_check(&probe));
        let probe = call(probe);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !probe.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "a connection check must not wait in the line"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let checked = probe.join().unwrap();
        let waiting = call(thinking);
        assert!(matches!(
            checked,
            solstone_core_generate::GenerateResponse::Refused(_)
        ));
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            !waiting.is_finished(),
            "a call waits while another holds the line"
        );
        drop(holder);
        assert!(matches!(
            waiting.join().unwrap(),
            solstone_core_generate::GenerateResponse::Refused(_)
        ));
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn actual_lane_selects_the_mobile_allowance_and_keeps_the_base_request() {
        let credential = "test-credential";
        let confidential = json!({
            "providers": {
                "active": {"provider":"local", "model":"selected"},
                "local": {
                    "endpoint_url":"https://processing.example/v1",
                    "served_model_id":"served",
                    "credential":credential
                }
            },
            "services": {"confidential": {
                "endpoint_url":"https://processing.example",
                "served_model_id":"served",
                "credential_fingerprint_sha256":solstone_core_brain::fingerprint_sha256(credential)
            }}
        });
        assert_eq!(
            solstone_core_brain::derive_active_brain_lane(confidential.as_object().unwrap())
                .lane
                .as_deref(),
            Some("spp")
        );
        let configurations = [
            (
                json!({"providers":{"active":{"provider":"local"}}}),
                "bundled",
            ),
            (
                json!({"providers":{"active":{"provider":"google"}}}),
                "cloud",
            ),
            (
                json!({"providers":{"active":{"provider":"local"},"local":{
                    "endpoint_url":"https://endpoint", "served_model_id":"served"
                }}}),
                "endpoint",
            ),
            (confidential, "confidential"),
            (json!({}), "none"),
        ];
        for base in [None, Some(480.5)] {
            let mut request = request();
            request.timeout_s = base;
            for allowance in [None, Some(std::time::Duration::from_secs(1200))] {
                for (config, expected_lane) in &configurations {
                    let (_, actual_lane) =
                        crate::lane::resolve_lane_with(config.as_object().unwrap(), |_| None);
                    let actual_kind = match &actual_lane {
                        crate::LaneOutcome::BundledLocal => "bundled",
                        crate::LaneOutcome::Google => "cloud",
                        crate::LaneOutcome::ByoEndpoint(_) => "endpoint",
                        crate::LaneOutcome::ConfidentialEndpoint(_) => "confidential",
                        crate::LaneOutcome::NoEngine => "none",
                        lane => panic!("unexpected lane: {lane:?}"),
                    };
                    assert_eq!(actual_kind, *expected_lane);
                    let selected = request_for_lane(&request, &actual_lane, allowance);
                    let mut expected = request.clone();
                    if *expected_lane == "bundled" && allowance.is_some() {
                        expected.timeout_s = Some(1200.0);
                    }
                    assert_eq!(selected.as_ref(), &expected);
                    assert_eq!(request.timeout_s, base);
                }
            }
        }
    }

    #[test]
    fn mobile_allowance_keeps_the_no_provider_refusal_unchanged() {
        let mut request = request();
        request.timeout_s = Some(480.5);
        let runtime = crate::EndpointRuntime::default();
        let context = GenerateCallContext {
            priority: solstone_core_journal_config::work_priority::WorkPriority::default(),
            cancellation: &solstone_core_generate::Cancellation::never(),
        };
        let dispatch = |allowance| {
            respond_config(
                &request,
                &runtime,
                Path::new("unused"),
                &serde_json::Map::new(),
                |_| panic!("no provider must not acquire local authority"),
                false,
                false,
                allowance,
                &context,
                Turn::Wait { may_forward: false },
            )
            .unwrap()
        };
        let without_allowance = dispatch(None);
        assert!(matches!(
            &without_allowance,
            solstone_core_generate::GenerateResponse::Refused(refusal)
                if refusal.reason == solstone_core_generate::RefusalReason::NoEngineConfigured
        ));
        assert_eq!(
            dispatch(Some(std::time::Duration::from_secs(1200))),
            without_allowance
        );
        assert_eq!(request.timeout_s, Some(480.5));
    }

    #[test]
    fn trusted_mobile_allowance_refuses_zero_and_unrepresentable_durations() {
        let request = request();
        let context = GenerateCallContext {
            priority: solstone_core_journal_config::work_priority::WorkPriority::default(),
            cancellation: &solstone_core_generate::Cancellation::never(),
        };
        for duration in [std::time::Duration::ZERO, std::time::Duration::MAX] {
            assert!(
                respond_with_mobile_local_timeout(
                    &request,
                    &crate::EndpointRuntime::default(),
                    Path::new("unused"),
                    |_| panic!("invalid allowance must be refused before local admission"),
                    &context,
                    Some(duration),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn anthropic_generated_response_preserves_thinking_blocks() {
        let request = request();
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
            overflow_text: None,
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
