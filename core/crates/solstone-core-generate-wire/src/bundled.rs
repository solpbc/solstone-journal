// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::Instant;

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_local::{
    ByoEndpoint, ConnectInput, ConnectOutcome, GenerateInput, GenerateResult, LoopbackAddr,
    Platform, connect, generate, local_generate_input_schema,
};

use crate::endpoint::{
    EndpointConverseCall, EndpointConverseResult, EndpointRuntime, EndpointTransport,
    UreqEndpointTransport, configured_served_context_window, converse_failure,
    endpoint_converse_with,
};
use crate::{ConverseMessage, ConverseToolSpec};

pub const LOCAL_MODEL_ID: &str = "local/qwen3.5-4b";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundledError {
    UnsupportedPlatform,
    ValueOutOfRange,
}

pub(crate) struct BundledConverseCall<'a> {
    pub request: &'a GenerateRequest,
    pub messages: &'a [ConverseMessage],
    pub tools: &'a [ConverseToolSpec],
    pub journal_path: &'a Path,
    pub config: &'a Map<String, Value>,
    pub runtime: &'a EndpointRuntime,
}

pub fn bundled_generate(
    request: &GenerateRequest,
    journal_path: &Path,
) -> Result<GenerateResult, BundledError> {
    Ok(generate(bundled_input(request, journal_path)?))
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

pub fn bundled_converse(
    request: &GenerateRequest,
    messages: &[ConverseMessage],
    tools: &[ConverseToolSpec],
    journal_path: &Path,
    config: &Map<String, Value>,
    runtime: &EndpointRuntime,
) -> EndpointConverseResult {
    let mut transport = UreqEndpointTransport;
    bundled_converse_with(
        BundledConverseCall {
            request,
            messages,
            tools,
            journal_path,
            config,
            runtime,
        },
        &mut transport,
        connect,
        Instant::now(),
    )
}

pub(crate) fn bundled_converse_with<T: EndpointTransport>(
    call: BundledConverseCall<'_>,
    transport: &mut T,
    connector: impl FnOnce(ConnectInput) -> ConnectOutcome,
    now: Instant,
) -> EndpointConverseResult {
    bundled_converse_with_observer(call, transport, connector, now, |_| {})
}

fn bundled_converse_with_observer<T: EndpointTransport>(
    call: BundledConverseCall<'_>,
    transport: &mut T,
    connector: impl FnOnce(ConnectInput) -> ConnectOutcome,
    now: Instant,
    observe_endpoint: impl FnOnce(&ByoEndpoint),
) -> EndpointConverseResult {
    let BundledConverseCall {
        request,
        messages,
        tools,
        journal_path,
        config,
        runtime,
    } = call;
    let platform = match detect_platform() {
        Ok(platform) => platform,
        Err(BundledError::UnsupportedPlatform) => return converse_failure("unsupported_platform"),
        Err(BundledError::ValueOutOfRange) => unreachable!("platform detection has no range value"),
    };
    let connect_input = ConnectInput {
        schema: "solstone-local-connect-input-v1".into(),
        journal_path: journal_path.display().to_string(),
        bind_address: LoopbackAddr::IPV4_LOOPBACK,
        default_model_id: LOCAL_MODEL_ID.to_owned(),
        platform,
    };
    let server = match connector(connect_input) {
        ConnectOutcome::Ready { server } => server,
        ConnectOutcome::Loading { .. } => return converse_failure("local_model_loading"),
        ConnectOutcome::NotReady { .. } | ConnectOutcome::Failed { .. } => {
            return converse_failure("local_model_not_ready");
        }
    };
    let endpoint = ByoEndpoint {
        base_url: server.base_url,
        served_model_id: server.served_model_id,
        credential: None,
        parallel_slots: Some(server.parallel_slots),
        is_confidential: false,
        is_bundled: true,
    };
    observe_endpoint(&endpoint);
    let mut config = config.clone();
    ensure_served_context_window(&mut config);
    endpoint_converse_with(
        EndpointConverseCall {
            request,
            messages,
            tools,
            journal_path,
            endpoint: &endpoint,
            config: &config,
            runtime,
        },
        transport,
        now,
    )
}

fn detect_platform() -> Result<Platform, BundledError> {
    match std::env::consts::OS {
        "linux" => Ok(Platform::Linux),
        "macos" => Ok(Platform::Darwin),
        _ => Err(BundledError::UnsupportedPlatform),
    }
}

fn ensure_served_context_window(config: &mut Map<String, Value>) {
    if configured_served_context_window(config).is_some() {
        return;
    }
    let providers = object_at(config, "providers");
    let local = object_at(providers, "local");
    local
        .entry("served_context_window".to_owned())
        .and_modify(|value| *value = solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS.into())
        .or_insert_with(|| solstone_core_local::plan::LOCAL_MIN_CONTEXT_TOKENS.into());
}

fn object_at<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    if !object.get(key).is_some_and(Value::is_object) {
        object.insert(key.to_owned(), Value::Object(Map::new()));
    }
    object
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object inserted above")
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
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;
    use std::time::Duration;

    use serde_json::json;
    use solstone_core_generate::ContentPart;
    use solstone_core_local::connect::ConnectedServer;
    use solstone_core_local::{
        GenerateTransport, HttpResponse, admission::acquire_local_slot, admission::admission_dir,
        generate_with,
    };

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
            thinking_budget: None,
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

    #[derive(Clone)]
    struct StubTransport {
        response: HttpResponse,
        posts: Arc<Mutex<Vec<(String, String, Value)>>>,
        gets: Arc<Mutex<usize>>,
    }

    impl StubTransport {
        fn success() -> Self {
            Self {
                response: tool_response(),
                posts: Arc::new(Mutex::new(Vec::new())),
                gets: Arc::new(Mutex::new(0)),
            }
        }
    }

    impl EndpointTransport for StubTransport {
        fn get(
            &mut self,
            _base_url: &str,
            _path: &str,
            _credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, crate::EndpointTransportError> {
            *self.gets.lock().expect("get count") += 1;
            Err(crate::EndpointTransportError::Other)
        }

        fn post_json(
            &mut self,
            base_url: &str,
            path: &str,
            body: &Value,
            _credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, crate::EndpointTransportError> {
            self.posts.lock().expect("posts").push((
                base_url.to_owned(),
                path.to_owned(),
                body.clone(),
            ));
            Ok(self.response.clone())
        }
    }

    #[derive(Clone)]
    struct HoldingTransport {
        state: Arc<(Mutex<ConcurrencyState>, Condvar)>,
        response: HttpResponse,
    }

    #[derive(Default)]
    struct ConcurrencyState {
        current: u32,
        peak: u32,
        posts: u32,
        started: u32,
        released: bool,
    }

    impl EndpointTransport for HoldingTransport {
        fn get(
            &mut self,
            _base_url: &str,
            _path: &str,
            _credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, crate::EndpointTransportError> {
            Err(crate::EndpointTransportError::Other)
        }

        fn post_json(
            &mut self,
            _base_url: &str,
            _path: &str,
            _body: &Value,
            _credential: Option<&str>,
            _timeout: Duration,
        ) -> Result<HttpResponse, crate::EndpointTransportError> {
            let (lock, entered) = &*self.state;
            let mut guard = lock.lock().expect("concurrency state");
            guard.current += 1;
            guard.peak = guard.peak.max(guard.current);
            guard.posts += 1;
            entered.notify_all();
            while !guard.released {
                let (next, wait) = entered
                    .wait_timeout(guard, Duration::from_secs(5))
                    .expect("concurrency wait");
                guard = next;
                if wait.timed_out() {
                    panic!("holding transport was not released");
                }
            }
            guard.current -= 1;
            Ok(self.response.clone())
        }
    }

    fn journal_path() -> std::path::PathBuf {
        crate::validation::isolated_journal_dir("bundled")
    }

    fn server(parallel_slots: u32) -> ConnectedServer {
        ConnectedServer {
            model_id: LOCAL_MODEL_ID.into(),
            served_model_id: "served-local".into(),
            port: 1234,
            base_url: "http://127.0.0.1:1234".into(),
            parallel_slots,
            capacity_source: "test".into(),
            profile: "floor".into(),
        }
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
        ) -> Result<HttpResponse, String> {
            match path {
                "/tokenize" => Ok(HttpResponse {
                    status: 200,
                    body: json!({"tokens": [1]}).to_string(),
                }),
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
            thinking_budget: None,
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
            let input = bundled_input(&request, &journal).expect("bundled input");
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

    #[test]
    fn bundled_converse_body_matches_the_b10068_wire_oracle() {
        let journal = journal_path();
        let mut request = converse_request();
        request.system_instruction = Some("Use tools when needed.".into());
        request.temperature = 0.0;
        request.max_output_tokens = 1;
        let messages = vec![
            ConverseMessage::User {
                text: "What is the temperature in Denver?".into(),
            },
            ConverseMessage::Assistant {
                text: String::new(),
                tool_calls: vec![crate::ConverseToolCall {
                    id: "call-1".into(),
                    name: "weather".into(),
                    arguments: json!({"city": "Denver"}),
                    not_offered: false,
                    thought_signature: None,
                }],
            },
            ConverseMessage::ToolResult {
                tool_call_id: "call-1".into(),
                tool_name: "weather".into(),
                output: r#"{"temperature_c":20}"#.into(),
            },
            ConverseMessage::Assistant {
                text: "It is 20°C.".into(),
                tool_calls: Vec::new(),
            },
            ConverseMessage::User {
                text: "Return that as JSON.".into(),
            },
        ];
        let tools = vec![ConverseToolSpec {
            name: "weather".into(),
            description: "Read current weather".into(),
            parameters: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            }),
        }];
        let mut transport = StubTransport::success();
        bundled_converse_with(
            BundledConverseCall {
                request: &request,
                messages: &messages,
                tools: &tools,
                journal_path: &journal,
                config: &Map::new(),
                runtime: &EndpointRuntime::default(),
            },
            &mut transport,
            |_| ConnectOutcome::Ready {
                server: oracle_server(),
            },
            Instant::now(),
        )
        .expect("bundled Converse succeeds");

        let posts = transport.posts.lock().expect("posts");
        assert_eq!(posts.len(), 1);
        let (base_url, path, body) = &posts[0];
        assert_eq!(base_url, "http://127.0.0.1:1234");
        assert_eq!(path, "/v1/chat/completions");
        let expected_body = oracle_case("tool-roundtrip-wire")["body"].clone();
        assert_eq!(
            serde_json::to_string(body).expect("production-built body serializes"),
            serde_json::to_string(&expected_body).expect("oracle body serializes")
        );

        let mut wrong_empty = body.clone();
        wrong_empty["messages"][2]["content"] = Value::String(String::new());
        assert_ne!(wrong_empty, expected_body);
        let mut wrong_arguments = body.clone();
        wrong_arguments["messages"][2]["tool_calls"][0]["function"]["arguments"] =
            json!({"city": "Denver"});
        assert_ne!(wrong_arguments, expected_body);
        drop(posts);
        let _ = std::fs::remove_dir_all(journal);
    }

    fn converse_request() -> GenerateRequest {
        GenerateRequest {
            id: Some("bundled-converse".into()),
            context: "test.bundled-converse".into(),
            contents: Vec::new(),
            system_instruction: None,
            temperature: 0.2,
            max_output_tokens: 64,
            thinking_budget: None,
            timeout_s: Some(5.0),
            json_output: false,
            json_schema: None,
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        }
    }

    fn messages() -> Vec<ConverseMessage> {
        vec![ConverseMessage::User { text: "ask".into() }]
    }

    fn tools() -> Vec<ConverseToolSpec> {
        vec![ConverseToolSpec {
            name: "weather".into(),
            description: "weather".into(),
            parameters: json!({"type": "object"}),
        }]
    }

    fn tool_response() -> HttpResponse {
        HttpResponse {
            status: 200,
            body: json!({
                "choices": [{
                    "message": {"content": "", "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "weather", "arguments": "{}"},
                    }]},
                    "finish_reason": "tool_calls",
                }],
            })
            .to_string(),
        }
    }

    fn bundled_with<T: EndpointTransport>(
        transport: &mut T,
        connector: impl FnOnce(ConnectInput) -> ConnectOutcome,
    ) -> EndpointConverseResult {
        let journal = journal_path();
        let result = bundled_converse_with(
            BundledConverseCall {
                request: &converse_request(),
                messages: &messages(),
                tools: &tools(),
                journal_path: &journal,
                config: &Map::new(),
                runtime: &EndpointRuntime::default(),
            },
            transport,
            connector,
            Instant::now(),
        );
        let _ = std::fs::remove_dir_all(journal);
        result
    }

    #[test]
    fn bundled_converse_uses_discovered_server_native_tools_and_qwen_controls() {
        let mut transport = StubTransport::success();
        let posts = Arc::clone(&transport.posts);
        let gets = Arc::clone(&transport.gets);

        let turn = bundled_with(&mut transport, |_| ConnectOutcome::Ready {
            server: server(2),
        })
        .expect("bundled turn");

        assert_eq!(turn.model, "served-local");
        assert_eq!(turn.tool_calls[0].name, "weather");
        assert_eq!(*gets.lock().expect("get count"), 0);
        let (base_url, path, body) = posts.lock().expect("posts")[0].clone();
        assert_eq!(base_url, server(2).base_url);
        assert_eq!(path, "/v1/chat/completions");
        assert_eq!(body["model"], "served-local");
        assert_eq!(
            body["messages"],
            json!([{"role": "user", "content": "ask"}])
        );
        assert_eq!(body["tools"][0]["function"]["name"], "weather");
        for field in [
            "chat_template_kwargs",
            "top_p",
            "top_k",
            "min_p",
            "presence_penalty",
        ] {
            assert!(body.get(field).is_some(), "{field}");
        }
    }

    #[test]
    fn bundled_converse_loading_has_a_named_failure() {
        let mut transport = StubTransport::success();
        let error = bundled_with(&mut transport, |_| ConnectOutcome::Loading {
            reason: "loading".into(),
        })
        .expect_err("loading fails");
        assert_eq!(error.reason_code, "local_model_loading");
    }

    #[test]
    fn bundled_converse_not_ready_has_a_named_failure() {
        let mut transport = StubTransport::success();
        let error = bundled_with(&mut transport, |_| ConnectOutcome::NotReady {
            reason: "no port".into(),
        })
        .expect_err("not ready fails");
        assert_eq!(error.reason_code, "local_model_not_ready");
    }

    #[test]
    fn bundled_converse_failed_has_a_named_failure() {
        let mut transport = StubTransport::success();
        let error = bundled_with(&mut transport, |_| ConnectOutcome::Failed {
            reason: "connection refused".into(),
        })
        .expect_err("failed connect fails");
        assert_eq!(error.reason_code, "local_model_not_ready");
    }

    #[test]
    fn bundled_converse_rejects_missing_structured_tool_calls() {
        let mut transport = StubTransport::success();
        transport.response = HttpResponse {
            status: 200,
            body: json!({
                "choices": [{"message": {"tool_calls": []}, "finish_reason": "tool_calls"}],
            })
            .to_string(),
        };
        let error = bundled_with(&mut transport, |_| ConnectOutcome::Ready {
            server: server(1),
        })
        .expect_err("missing calls fail");
        assert_eq!(error.reason_code, "tool_calls_missing");
    }

    #[test]
    fn bundled_converse_rejects_synthesized_tool_call_prose() {
        let mut transport = StubTransport::success();
        transport.response = HttpResponse {
            status: 200,
            body: json!({
                "choices": [{
                    "message": {"content": "<tool_call>{}</tool_call>"},
                    "finish_reason": "stop",
                }],
            })
            .to_string(),
        };
        let error = bundled_with(&mut transport, |_| ConnectOutcome::Ready {
            server: server(1),
        })
        .expect_err("synthesized prose fails");
        assert_eq!(error.reason_code, "tool_call_synthesized_as_prose");
    }

    #[test]
    fn bundled_converse_rejects_tool_request_contract_failures_and_releases_admission() {
        let journal = journal_path();
        let mut transport = StubTransport::success();
        transport.response = HttpResponse {
            status: 400,
            body: "tools are not supported".into(),
        };
        let error = bundled_converse_with(
            BundledConverseCall {
                request: &converse_request(),
                messages: &messages(),
                tools: &tools(),
                journal_path: &journal,
                config: &Map::new(),
                runtime: &EndpointRuntime::default(),
            },
            &mut transport,
            |_| ConnectOutcome::Ready { server: server(1) },
            Instant::now(),
        )
        .expect_err("tool contract failure");
        assert_eq!(error.reason_code, "local_endpoint_contract_failed");
        let permit = acquire_local_slot(&admission_dir(&journal), 1, Some(Duration::ZERO), false)
            .expect("failure released admission");
        drop(permit);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn bundled_converse_applies_the_context_floor_and_rejects_unfittable_history() {
        let journal = journal_path();
        let mut transport = StubTransport::success();
        let messages = vec![ConverseMessage::User {
            text: "x".repeat(60_000),
        }];
        let error = bundled_converse_with(
            BundledConverseCall {
                request: &converse_request(),
                messages: &messages,
                tools: &tools(),
                journal_path: &journal,
                config: &Map::new(),
                runtime: &EndpointRuntime::default(),
            },
            &mut transport,
            |_| ConnectOutcome::Ready { server: server(1) },
            Instant::now(),
        )
        .expect_err("unfittable history");
        assert_eq!(error.reason_code, "context_budget_exceeded");
        assert!(transport.posts.lock().expect("posts").is_empty());
        assert_eq!(*transport.gets.lock().expect("get count"), 0);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn bundled_converse_preserves_valid_context_window_overrides() {
        let journal = journal_path();
        let mut transport = StubTransport::success();
        let messages = vec![ConverseMessage::User {
            text: "x".repeat(6_000),
        }];
        let config = json!({"providers": {"local": {"served_context_window": 2048}}})
            .as_object()
            .expect("config object")
            .clone();
        let error = bundled_converse_with(
            BundledConverseCall {
                request: &converse_request(),
                messages: &messages,
                tools: &tools(),
                journal_path: &journal,
                config: &config,
                runtime: &EndpointRuntime::default(),
            },
            &mut transport,
            |_| ConnectOutcome::Ready { server: server(1) },
            Instant::now(),
        )
        .expect_err("configured window applies");
        assert_eq!(error.reason_code, "context_budget_exceeded");
        assert_eq!(*transport.gets.lock().expect("get count"), 0);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn bundled_converse_replaces_invalid_context_window_overrides_with_the_floor() {
        let journal = journal_path();
        let mut transport = StubTransport::success();
        let config = json!({"providers": {"local": {"served_context_window": 1}}})
            .as_object()
            .expect("config object")
            .clone();
        bundled_converse_with(
            BundledConverseCall {
                request: &converse_request(),
                messages: &messages(),
                tools: &tools(),
                journal_path: &journal,
                config: &config,
                runtime: &EndpointRuntime::default(),
            },
            &mut transport,
            |_| ConnectOutcome::Ready { server: server(1) },
            Instant::now(),
        )
        .expect("invalid override uses floor");
        assert_eq!(*transport.gets.lock().expect("get count"), 0);
        let _ = std::fs::remove_dir_all(journal);
    }

    #[test]
    fn bundled_converse_shares_and_releases_the_local_admission_pool() {
        let journal = journal_path();
        let state = Arc::new((Mutex::new(ConcurrencyState::default()), Condvar::new()));
        let transport = HoldingTransport {
            state: Arc::clone(&state),
            response: tool_response(),
        };
        let spawn_worker = |transport: HoldingTransport, journal: std::path::PathBuf| {
            thread::spawn(move || {
                {
                    let (lock, entered) = &*transport.state;
                    let mut guard = lock.lock().expect("concurrency state");
                    guard.started += 1;
                    entered.notify_all();
                }
                let mut transport = transport;
                bundled_converse_with_observer(
                    BundledConverseCall {
                        request: &converse_request(),
                        messages: &messages(),
                        tools: &tools(),
                        journal_path: &journal,
                        config: &Map::new(),
                        runtime: &EndpointRuntime::default(),
                    },
                    &mut transport,
                    |_| ConnectOutcome::Ready { server: server(1) },
                    Instant::now(),
                    |endpoint| assert_eq!(endpoint.parallel_slots, Some(1)),
                )
            })
        };
        let first = spawn_worker(transport.clone(), journal.clone());
        let (state_lock, entered) = &*state;
        let mut state_guard = state_lock.lock().expect("concurrency state");
        while state_guard.current != 1 {
            let (next, wait) = entered
                .wait_timeout(state_guard, Duration::from_secs(5))
                .expect("concurrency wait");
            state_guard = next;
            if wait.timed_out() {
                panic!("worker 1 never entered transport");
            }
        }
        drop(state_guard);
        let second = spawn_worker(transport, journal.clone());
        let mut state_guard = state_lock.lock().expect("concurrency state");
        while state_guard.started != 2 {
            let (next, wait) = entered
                .wait_timeout(state_guard, Duration::from_secs(5))
                .expect("concurrency wait");
            state_guard = next;
            if wait.timed_out() {
                panic!("worker 2 never started");
            }
        }
        assert_eq!(state_guard.current, 1);
        assert_eq!(state_guard.peak, 1);
        state_guard.released = true;
        entered.notify_all();
        drop(state_guard);
        for worker in [first, second] {
            worker
                .join()
                .expect("join bundled worker")
                .expect("bundled worker succeeds");
        }
        let state = state.0.lock().expect("concurrency state");
        assert_eq!(state.posts, 2);
        assert_eq!(state.peak, 1);
        drop(state);
        let permit = acquire_local_slot(&admission_dir(&journal), 1, Some(Duration::ZERO), false)
            .expect("success released admission");
        drop(permit);
        let _ = std::fs::remove_dir_all(journal);
    }
}
