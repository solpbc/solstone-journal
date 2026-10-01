// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::{
    GenerateInput, GenerateResult, LocalInferenceAuthority, LoopbackAddr, Platform,
    local_generate_input_schema,
};

pub const LOCAL_MODEL_ID: &str = "local/qwen3.5-4b";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundledError {
    UnsupportedPlatform,
    ValueOutOfRange,
}

pub fn bundled_generate(
    request: &GenerateRequest,
    journal_path: &Path,
) -> Result<GenerateResult, BundledError> {
    bundled_generate_with_authority(request, journal_path, None)
}

pub fn bundled_generate_with_authority(
    request: &GenerateRequest,
    journal_path: &Path,
    authority: Option<&mut LocalInferenceAuthority>,
) -> Result<GenerateResult, BundledError> {
    let input = bundled_input(request, journal_path)?;
    Ok(solstone_core_local::generate_with_authority(
        input, authority,
    ))
}

pub fn bundled_input(
    request: &GenerateRequest,
    journal_path: &Path,
) -> Result<GenerateInput, BundledError> {
    let platform = detect_platform()?;
    Ok(GenerateInput {
        schema: local_generate_input_schema().to_owned(),
        journal_path: journal_path.display().to_string(),
        bind_address: LoopbackAddr::IPV4_LOOPBACK,
        default_model_id: LOCAL_MODEL_ID.to_owned(),
        platform,
        contents: Value::Array(request.contents.iter().map(content_value).collect()),
        system_instruction: request.system_instruction.clone(),
        temperature: request.temperature,
        max_output_tokens: u32::try_from(request.max_output_tokens)
            .map_err(|_| BundledError::ValueOutOfRange)?,
        json_output: request.json_output,
        json_schema: request.json_schema.clone(),
        timeout_s: request.timeout_s,
        exclusive_admission: request.exclusive_admission,
        attempt_index: u32::try_from(request.attempt_index)
            .map_err(|_| BundledError::ValueOutOfRange)?,
    })
}

fn detect_platform() -> Result<Platform, BundledError> {
    match std::env::consts::OS {
        "linux" => Ok(Platform::Linux),
        "macos" => Ok(Platform::Darwin),
        "windows" => Ok(Platform::Windows),
        _ => Err(BundledError::UnsupportedPlatform),
    }
}

fn content_value(content: &ContentPart) -> Value {
    match content {
        ContentPart::Text { text } => Value::String(text.clone()),
        ContentPart::Image { mime_type, data } => {
            json!({"type": "image", "mime_type": mime_type, "data": data})
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use solstone_core_generate::ContentPart;
    use solstone_core_local::connect::ConnectedServer;
    use solstone_core_local::{ConnectOutcome, GenerateTransport, HttpResponse, generate_with};

    use super::*;

    fn request() -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "test.generate".into(),
            contents: vec![
                ContentPart::Text {
                    text: "look".into(),
                },
                ContentPart::Image {
                    mime_type: "image/png".into(),
                    data: "data".into(),
                },
            ],
            system_instruction: Some("system".into()),
            temperature: 0.2,
            max_output_tokens: 512,
            timeout_s: Some(5.0),
            json_output: true,
            json_schema: Some(json!({"type": "object"})),
            enforce_responsiveness: true,
            attempt_index: 2,
            exclusive_admission: true,
            transport_retries: None,
        }
    }

    #[test]
    fn builds_the_local_generate_input() {
        let input = bundled_input(&request(), Path::new("/journal")).unwrap();
        assert_eq!(input.schema, local_generate_input_schema());
        assert_eq!(input.journal_path, "/journal");
        assert_eq!(input.default_model_id, LOCAL_MODEL_ID);
        assert_eq!(
            input.contents,
            json!(["look", {"type": "image", "mime_type": "image/png", "data": "data"}])
        );
        assert_eq!(input.attempt_index, 2);
        assert!(input.exclusive_admission);
    }

    #[test]
    fn rejects_values_outside_the_local_input_range() {
        let mut request = request();
        request.max_output_tokens = u64::from(u32::MAX) + 1;
        assert_eq!(
            bundled_input(&request, Path::new("/journal")),
            Err(BundledError::ValueOutOfRange)
        );
    }

    fn journal_path() -> std::path::PathBuf {
        crate::validation::isolated_journal_dir("bundled")
    }

    const QWEN_B10068_WIRE_ORACLE: &str =
        include_str!("../../../fixtures/qwen35_b10068_wire_oracle_v1.json");

    fn oracle_case(name: &str) -> Value {
        let fixture: Value =
            serde_json::from_str(QWEN_B10068_WIRE_ORACLE).expect("wire oracle parses");
        fixture["cases"]
            .as_array()
            .expect("wire oracle cases")
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("wire oracle lost case {name}"))
            .clone()
    }

    fn oracle_server() -> ConnectedServer {
        ConnectedServer {
            model_id: LOCAL_MODEL_ID.into(),
            served_model_id: LOCAL_MODEL_ID.into(),
            port: 1234,
            base_url: "http://127.0.0.1:1234".into(),
            parallel_slots: 1,
            capacity_source: "wire-oracle-test".into(),
            profile: "floor".into(),
        }
    }

    #[derive(Default)]
    struct WireGenerateTransport {
        posts: Vec<(String, String, Value)>,
    }

    impl GenerateTransport for WireGenerateTransport {
        fn get(
            &mut self,
            base_url: &str,
            path: &str,
            _timeout: Duration,
            _auth_token: Option<&str>,
        ) -> Result<HttpResponse, String> {
            assert_eq!(base_url, "http://127.0.0.1:1234");
            assert_eq!(path, "/props");
            Ok(HttpResponse {
                status: 200,
                body: json!({"n_ctx": 16_384}).to_string(),
            })
        }

        fn post_json(
            &mut self,
            base_url: &str,
            path: &str,
            body: &Value,
            _timeout: Duration,
            _auth_token: Option<&str>,
        ) -> Result<HttpResponse, String> {
            match path {
                "/tokenize" => Ok(HttpResponse {
                    status: 200,
                    body: json!({"tokens": [1]}).to_string(),
                }),
                "/v1/chat/completions/input_tokens" => {
                    let fixture: Value =
                        serde_json::from_str(QWEN_B10068_WIRE_ORACLE).expect("wire oracle parses");
                    let case = fixture["cases"]
                        .as_array()
                        .expect("wire oracle cases")
                        .iter()
                        .find(|case| case["body"] == *body)
                        .expect("count receives one exact oracle body");
                    Ok(HttpResponse {
                        status: 200,
                        body: json!({
                            "object": "response.input_tokens",
                            "input_tokens": case["native_input_tokens"],
                        })
                        .to_string(),
                    })
                }
                "/v1/chat/completions" => {
                    self.posts
                        .push((base_url.to_owned(), path.to_owned(), body.clone()));
                    Ok(HttpResponse {
                        status: 200,
                        body: json!({
                            "choices": [{
                                "message": {"content": "ok"},
                                "finish_reason": "stop",
                            }],
                            "usage": {
                                "prompt_tokens": 1,
                                "completion_tokens": 1,
                                "total_tokens": 2,
                            },
                        })
                        .to_string(),
                    })
                }
                other => panic!("unexpected generate POST {other}"),
            }
        }
    }

    fn oracle_generate_request(
        text: &str,
        system_instruction: Option<&str>,
        json_schema: Option<Value>,
    ) -> GenerateRequest {
        GenerateRequest {
            id: None,
            context: "test.qwen-b10068-wire-oracle".into(),
            contents: vec![ContentPart::Text { text: text.into() }],
            system_instruction: system_instruction.map(str::to_owned),
            temperature: 0.0,
            max_output_tokens: 1,
            timeout_s: Some(5.0),
            json_output: json_schema.is_some(),
            json_schema,
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    #[test]
    fn bundled_generate_bodies_match_the_b10068_wire_oracle() {
        let schema = json!({
            "type": "object",
            "properties": {"summary": {"type": "string"}},
            "required": ["summary"],
            "additionalProperties": false,
        });
        let cases = [
            ("empty-user", oracle_generate_request("", None, None)),
            (
                "plain",
                oracle_generate_request("hello world", Some("Return JSON."), None),
            ),
            (
                "unicode",
                oracle_generate_request("café é 東京 👩🏽‍💻 <|im_start|> not control", None, None),
            ),
            (
                "json-terminal-schema",
                oracle_generate_request(
                    r#"{"pane":"%1","text":"\u001b[31mRED\u001b[0m\n$ git status"}"#,
                    Some("Describe the visible terminal."),
                    Some(schema),
                ),
            ),
        ];

        for (name, request) in cases {
            let journal = journal_path();
            let mut input = bundled_input(&request, &journal).expect("bundled input");
            // This oracle records Linux's exact-token-counting request path;
            // Windows additionally requires a live per-launch authority.
            input.platform = Platform::Linux;
            let mut transport = WireGenerateTransport::default();
            let result = generate_with(input, &mut transport, |_| ConnectOutcome::Ready {
                server: oracle_server(),
            });
            assert!(matches!(result, GenerateResult::Success(_)), "case={name}");
            assert_eq!(transport.posts.len(), 1, "case={name}");
            let (base_url, path, body) = &transport.posts[0];
            assert_eq!(base_url, "http://127.0.0.1:1234", "case={name}");
            assert_eq!(path, "/v1/chat/completions", "case={name}");
            let actual = serde_json::to_string(body).expect("production-built body serializes");
            let expected =
                serde_json::to_string(&oracle_case(name)["body"]).expect("oracle body serializes");
            assert_eq!(actual, expected, "case={name}");
            let _ = std::fs::remove_dir_all(journal);
        }
    }
}
