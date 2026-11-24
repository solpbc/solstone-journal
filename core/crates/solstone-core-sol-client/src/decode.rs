// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use crate::error::{ClientError, SERVER_ERROR_MESSAGE};
use crate::transport::HttpResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub type DecodeResult<T> = Result<T, ClientError>;

pub type JsonValue = Value;

pub fn decode_response(response: &HttpResponse) -> DecodeResult<JsonValue> {
    let text = String::from_utf8_lossy(&response.body);
    let stripped = text.trim();
    let parsed = if stripped.is_empty() {
        None
    } else {
        serde_json::from_str::<JsonValue>(stripped).ok()
    };

    if (200..300).contains(&response.status) {
        return parsed.ok_or(ClientError::MalformedSuccess {
            status: Some(response.status),
        });
    }

    if let Some(JsonValue::Object(object)) = parsed.as_ref()
        && (object.contains_key("error") || object.contains_key("reason_code"))
    {
        let error = object
            .get("error")
            .or_else(|| object.get("reason_code"))
            .map(json_value_to_string)
            .unwrap_or_else(|| SERVER_ERROR_MESSAGE.to_string());
        let reason_code = object.get("reason_code").map(json_value_to_string);
        let detail = object.get("detail").map(json_value_to_string);
        return Err(ClientError::ReasonRejected {
            status: response.status,
            error,
            reason_code,
            detail,
            payload: Box::new(parsed.expect("parsed object exists")),
        });
    }

    // Before first-run setup the journal answers every call by sending it to
    // its setup page.
    if (300..400).contains(&response.status)
        && response.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("location") && value.trim_end_matches('/').ends_with("/init")
        })
    {
        return Err(ClientError::NotSetUp {
            status: response.status,
        });
    }

    Err(ClientError::UnreadableServerError {
        status: Some(response.status),
    })
}

fn json_value_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(value) => value.clone(),
        JsonValue::Null => "null".to_string(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::{
        LOCAL_CONVEY_TIMEOUT_REASON, MALFORMED_RESPONSE_MESSAGE, TIMEOUT_MESSAGE,
        UNREACHABLE_MESSAGE,
    };
    use crate::transport::TimeoutPolicy;

    fn response(status: u16, body: &[u8]) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![],
            body: body.to_vec(),
            policy: TimeoutPolicy::Api,
        }
    }

    #[test]
    fn decodes_success_json() {
        assert_eq!(
            decode_response(&response(200, br#"{"a":1}"#)),
            Ok(json!({"a": 1}))
        );
    }

    #[test]
    fn decodes_success_json_preserving_object_order() {
        let value = decode_response(&response(200, br#"{"b":1,"a":2}"#)).expect("decode");
        let keys = value
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["b", "a"]);
    }

    #[test]
    fn success_empty_is_malformed() {
        let error = decode_response(&response(204, b"")).expect_err("empty 2xx fails");
        assert_eq!(error.message(), MALFORMED_RESPONSE_MESSAGE);
        assert_eq!(error.status(), Some(204));
    }

    #[test]
    fn success_non_json_is_malformed() {
        let error = decode_response(&response(200, b"not-json")).expect_err("non-json 2xx fails");
        assert!(matches!(
            error,
            ClientError::MalformedSuccess { status: Some(200) }
        ));
    }

    #[test]
    fn reason_coded_error_uses_error_field() {
        let error = decode_response(&response(
            404,
            br#"{"error":"not found","reason_code":"THING_MISSING","detail":"gone"}"#,
        ))
        .expect_err("non-2xx reason fails");
        assert_eq!(error.message(), "not found");
        assert_eq!(error.reason_code(), Some("THING_MISSING"));
        assert_eq!(error.detail(), Some("gone"));
        assert_eq!(error.status(), Some(404));
    }

    #[test]
    fn reason_coded_error_falls_back_to_reason_code() {
        let error = decode_response(&response(409, br#"{"reason_code":"ALREADY_EXISTS"}"#))
            .expect_err("non-2xx reason fails");
        assert_eq!(error.message(), "ALREADY_EXISTS");
        assert_eq!(error.reason_code(), Some("ALREADY_EXISTS"));
    }

    #[test]
    fn non_2xx_non_json_is_unreadable_server_error() {
        let error = decode_response(&response(500, b"<html>")).expect_err("500 html fails");
        assert_eq!(error.message(), SERVER_ERROR_MESSAGE);
        assert_eq!(error.status(), Some(500));
    }

    #[test]
    fn a_call_sent_to_first_run_setup_says_the_journal_is_not_set_up() {
        let mut redirect = response(302, b"<html>");
        redirect.headers = vec![("Location".to_owned(), "/init".to_owned())];
        let error = decode_response(&redirect).expect_err("setup redirect fails");
        assert_eq!(error, ClientError::NotSetUp { status: 302 });
        assert_eq!(error.message_for("en-US"), error.message());

        redirect.headers = vec![("location".to_owned(), "/elsewhere".to_owned())];
        let error = decode_response(&redirect).expect_err("other redirect fails");
        assert_eq!(error.message(), SERVER_ERROR_MESSAGE);
    }

    #[test]
    fn transport_error_messages_are_exact() {
        let unreachable = ClientError::unreachable(Some("connection refused".to_string()));
        assert_eq!(unreachable.message(), UNREACHABLE_MESSAGE);
        let timeout = ClientError::timeout(Some("GET /x exceeded".to_string()));
        assert_eq!(timeout.message(), TIMEOUT_MESSAGE);
        assert_eq!(timeout.reason_code(), Some(LOCAL_CONVEY_TIMEOUT_REASON));
    }
}
