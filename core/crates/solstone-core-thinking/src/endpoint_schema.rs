// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Probing whether a configured local endpoint answers in structured output.
//!
//! Almost every talent asks for a JSON schema. An endpoint that answers text and
//! reads images can still refuse `response_format` outright, and then every
//! talent fails, so saving an endpoint checks this too.

use std::path::Path;

use solstone_core_generate::{ClientError, ContentPart, GenerateRequest, GenerateResponse};

use solstone_core_generate::OneShotClient;

use crate::endpoint_image::{
    CheckReason, ImageCheck, ImageProbe, check_with_hue, hue_from_byte, refusal_reason,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaCheck {
    Followed,
    NotFollowed,
    /// The endpoint answered the structured request with an error. That is the
    /// structured request's fault only if the endpoint answers an ordinary
    /// request, so [`SchemaCheck::settle`] decides it against the image check.
    Errored,
    Unchecked {
        reason: CheckReason,
    },
}

impl SchemaCheck {
    /// An error reply counts against structured output when the same endpoint
    /// then answered the image request; otherwise the endpoint was failing and
    /// nothing is known about structured output.
    pub fn settle(self, image: &ImageCheck) -> Self {
        match (self, image) {
            (Self::Errored, ImageCheck::Read | ImageCheck::NotRead) => Self::NotFollowed,
            (Self::Errored, _) => Self::Unchecked {
                reason: CheckReason::Unusable,
            },
            (other, _) => other,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Followed => serde_json::json!({"status": "followed"}),
            Self::NotFollowed => serde_json::json!({"status": "not_followed"}),
            Self::Errored => serde_json::json!({"status": "unchecked", "reason": "unusable"}),
            Self::Unchecked { reason } => serde_json::json!({
                "status": "unchecked",
                "reason": reason.as_wire(),
            }),
        }
    }
}

/// Both save checks, structured output first: it is text-only and quick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointCheck {
    pub schema: SchemaCheck,
    pub image: ImageCheck,
}

impl EndpointCheck {
    pub fn to_json(&self) -> serde_json::Value {
        let mut value = self.image.to_json();
        value["schema_check"] = self.schema.to_json();
        value
    }
}

fn schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {"ok": {"type": "boolean"}},
        "required": ["ok"],
        "additionalProperties": false,
    })
}

pub fn schema_request() -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "settings.local.endpoint_schema".to_owned(),
        contents: vec![ContentPart::Text {
            text: "Answer with the JSON object {\"ok\": true}.".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.0,
        max_output_tokens: 512,
        timeout_s: Some(20.0),
        json_output: true,
        json_schema: Some(schema()),
        attempt_index: 0,
        exclusive_admission: false,
        overflow_text: None,
        transport_retries: Some(0),
    }
}

/// Run both save checks against the journal's own endpoint.
#[allow(clippy::result_unit_err)]
pub fn check_endpoint(journal: &Path) -> Result<EndpointCheck, ()> {
    let mut client = OneShotClient::sibling().map_err(|_| ())?;
    let schema = check_schema(&mut client, journal);
    let mut byte = [0u8; 1];
    let image = if getrandom::fill(&mut byte).is_err() {
        ImageCheck::Unchecked {
            reason: CheckReason::Unavailable,
        }
    } else {
        check_with_hue(&mut client, journal, hue_from_byte(byte[0]))
    };
    Ok(EndpointCheck {
        schema: schema.settle(&image),
        image,
    })
}

pub fn check_schema<P: ImageProbe>(probe: &mut P, journal: &Path) -> SchemaCheck {
    // As for the image check: a root the child cannot be told is not checked.
    if journal.to_str().is_none() {
        return SchemaCheck::Unchecked {
            reason: CheckReason::Unavailable,
        };
    }
    classify_schema_reply(probe.probe(journal, &schema_request()))
}

pub fn classify_schema_reply(response: Result<GenerateResponse, ClientError>) -> SchemaCheck {
    match response {
        Err(_) => SchemaCheck::Unchecked {
            reason: CheckReason::Unavailable,
        },
        Ok(GenerateResponse::Generated(generated)) => {
            if generated.finish_reason == "max_tokens" {
                return SchemaCheck::Unchecked {
                    reason: CheckReason::Unusable,
                };
            }
            let text = match generated.text.rfind("</think>") {
                Some(index) => &generated.text[index + "</think>".len()..],
                None => generated.text.as_str(),
            };
            let followed = serde_json::from_str::<serde_json::Value>(text.trim())
                .ok()
                .and_then(|value| value.get("ok").map(serde_json::Value::is_boolean))
                .unwrap_or(false);
            if followed {
                SchemaCheck::Followed
            } else {
                SchemaCheck::NotFollowed
            }
        }
        Ok(GenerateResponse::Refused(refusal)) => {
            let code = refusal.reason_code.as_ref().map(|code| code.as_wire());
            match code {
                // The endpoint turned the structured request down.
                Some("provider_request_rejected") => SchemaCheck::NotFollowed,
                // It answered with an error, such as Ollama's MLX runner's 501, or
                // a passing 500; the image check tells the two apart.
                Some("provider_response_invalid") => SchemaCheck::Errored,
                _ => SchemaCheck::Unchecked {
                    reason: refusal_reason(code),
                },
            }
        }
        Ok(GenerateResponse::Cancelled) => SchemaCheck::Unchecked {
            reason: CheckReason::Other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_generate::{
        GeneratedResponse, ReasonCode, ReasonCodeValue, RefusalReason, RefusedResponse,
    };

    fn generated(text: &str, finish_reason: &str) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test-model".to_owned(),
            usage: serde_json::Value::Null,
            finish_reason: finish_reason.to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: vec![],
        })))
    }

    fn refused(code: &str) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason: RefusalReason::Unknown,
            reason_code: Some(ReasonCodeValue::Known(
                ReasonCode::new(code).expect("contract code"),
            )),
            retryable: false,
            blocking: false,
            reset_at_ms: None,
            provider: None,
            detail: String::new(),
        }))
    }

    #[test]
    fn a_schema_answer_is_followed() {
        assert_eq!(
            classify_schema_reply(generated("{\"ok\": true}", "stop")),
            SchemaCheck::Followed
        );
        assert_eq!(
            classify_schema_reply(generated("<think>hm</think>\n{\"ok\":false}", "stop")),
            SchemaCheck::Followed
        );
    }

    #[test]
    fn prose_or_a_refused_schema_is_not_followed() {
        assert_eq!(
            classify_schema_reply(generated("Sure! ok is true.", "stop")),
            SchemaCheck::NotFollowed
        );
        assert_eq!(
            classify_schema_reply(refused("provider_request_rejected")),
            SchemaCheck::NotFollowed
        );
    }

    #[test]
    fn an_error_reply_counts_only_when_the_endpoint_answers_otherwise() {
        // Ollama's MLX runner answers every `response_format` with HTTP 501,
        // which reaches here as an invalid response, and still reads images.
        let errored = classify_schema_reply(refused("provider_response_invalid"));
        assert_eq!(errored, SchemaCheck::Errored);
        assert_eq!(
            errored.clone().settle(&ImageCheck::Read),
            SchemaCheck::NotFollowed
        );
        assert_eq!(
            errored.clone().settle(&ImageCheck::NotRead),
            SchemaCheck::NotFollowed
        );
        // A server failing every request says nothing about structured output.
        assert_eq!(
            errored.settle(&ImageCheck::Unchecked {
                reason: CheckReason::Unusable
            }),
            SchemaCheck::Unchecked {
                reason: CheckReason::Unusable
            }
        );
        assert_eq!(
            SchemaCheck::Followed.settle(&ImageCheck::Unchecked {
                reason: CheckReason::Timeout
            }),
            SchemaCheck::Followed
        );
    }

    #[test]
    fn an_endpoint_that_cannot_be_asked_is_unchecked() {
        assert_eq!(
            classify_schema_reply(refused("local_endpoint_unreachable")),
            SchemaCheck::Unchecked {
                reason: CheckReason::Unreachable
            }
        );
        assert_eq!(
            classify_schema_reply(generated("{\"ok\"", "max_tokens")),
            SchemaCheck::Unchecked {
                reason: CheckReason::Unusable
            }
        );
    }

    #[test]
    fn the_wire_shape_keeps_image_check_and_adds_schema_check() {
        let value = EndpointCheck {
            schema: SchemaCheck::NotFollowed,
            image: ImageCheck::Read,
        }
        .to_json();
        assert_eq!(value["image_check"]["status"], "read");
        assert_eq!(value["schema_check"]["status"], "not_followed");
    }
}
