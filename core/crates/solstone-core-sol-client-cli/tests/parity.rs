// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde_json::{Value, json};
use solstone_core_sol_client::command::CommandContext;
use solstone_core_sol_client::error::ClientError;
use solstone_core_sol_client::seam::{
    ExpectedHttpCall, FakeBuildIdentityProvider, FakeClientItemIdProvider, FakeClock,
    FixtureFileProvider, RecordedHttpCall, ScriptedHttpTransport, ScriptedLinkJoinPairingSeam,
    ScriptedLinkServeRunner,
};
use solstone_core_sol_client::transport::{
    ApiRequest, FormField, HttpMethod, HttpResponse, MultipartFile, QueryParam, SseRequest,
    TimeoutPolicy, UploadRequest,
};
use solstone_core_sol_client_cli::{
    DispatchSeams, LinkDispatch, LinkDispatchSeams, dispatch_sol_call_with_seams,
    dispatch_sol_import_with_seams, dispatch_sol_link_with_seams, dispatch_sol_status_with_seams,
};

const ACTIVITIES_VECTORS: &str =
    include_str!("../../../fixtures/native-sol/parity/activities.jsonl");
const ACTIVITIES_COVERAGE_VECTORS: &str =
    include_str!("../../../fixtures/native-sol/parity/activities_coverage.jsonl");
const AWARENESS_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/awareness.jsonl");
const BODY_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/body.jsonl");
const ENTITIES_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/entities.jsonl");
const FACETS_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/facets.jsonl");
const HEALTH_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/health.jsonl");
const HEALTH_COVERAGE_VECTORS: &str =
    include_str!("../../../fixtures/native-sol/parity/health_coverage.jsonl");
const JOURNAL_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/journal.jsonl");
const IMPORT_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/import.jsonl");
const LINK_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/link.jsonl");
const LINK_JOIN_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/link_join.jsonl");
const LINK_SERVE_VECTORS: &str =
    include_str!("../../../fixtures/native-sol/parity/link_serve.jsonl");
const MOVED_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/moved.jsonl");
const PROFILE_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/profile.jsonl");
const SETTINGS_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/settings.jsonl");
const SOL_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/sol.jsonl");
const STATUS_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/status.jsonl");
const SPEAKERS_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/speakers.jsonl");
const SUPPORT_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/support.jsonl");
const SUPPORT_COVERAGE_VECTORS: &str =
    include_str!("../../../fixtures/native-sol/parity/support_coverage.jsonl");
const THINKING_VECTORS: &str = include_str!("../../../fixtures/native-sol/parity/thinking.jsonl");
const TRANSCRIPTS_VECTORS: &str =
    include_str!("../../../fixtures/native-sol/parity/transcripts.jsonl");
const FILE_ROOT: &str = "/native-sol-parity-files";

#[test]
fn native_matches_sol_call_parity_vectors() {
    for vector in load_vectors(ACTIVITIES_VECTORS)
        .into_iter()
        .chain(load_vectors(ACTIVITIES_COVERAGE_VECTORS))
        .chain(load_vectors(AWARENESS_VECTORS))
        .chain(load_vectors(BODY_VECTORS))
        .chain(load_vectors(ENTITIES_VECTORS))
        .chain(load_vectors(FACETS_VECTORS))
        .chain(load_vectors(HEALTH_VECTORS))
        .chain(load_vectors(HEALTH_COVERAGE_VECTORS))
        .chain(load_vectors(JOURNAL_VECTORS))
        .chain(load_vectors(IMPORT_VECTORS))
        .chain(load_vectors(LINK_VECTORS))
        .chain(load_vectors(LINK_JOIN_VECTORS))
        .chain(load_vectors(LINK_SERVE_VECTORS))
        .chain(load_vectors(MOVED_VECTORS))
        .chain(load_vectors(PROFILE_VECTORS))
        .chain(load_vectors(SETTINGS_VECTORS))
        .chain(load_vectors(SOL_VECTORS))
        .chain(load_vectors(STATUS_VECTORS))
        .chain(load_vectors(SPEAKERS_VECTORS))
        .chain(load_vectors(SUPPORT_VECTORS))
        .chain(load_vectors(SUPPORT_COVERAGE_VECTORS))
        .chain(load_vectors(THINKING_VECTORS))
        .chain(load_vectors(TRANSCRIPTS_VECTORS))
    {
        run_vector(&vector);
    }
}

#[test]
fn retired_commitment_ledger_commands_are_unsupported_without_http() {
    let transport = ScriptedHttpTransport::new(vec![]);
    let output = dispatch_sol_call_with_seams(
        &["ledger".to_string(), "list".to_string()],
        &BTreeMap::new(),
        "",
        "20260723",
        DispatchSeams {
            transport: &transport,
            clock: None,
            files: None,
            build_identity: None,
            client_item_ids: None,
            notification_sink: None,
        },
    );

    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "unsupported command.\n");
    assert_eq!(output.exit, 64);
    transport.assert_done();
}

fn run_vector(vector: &Value) {
    let argv = expand_file_args(string_array(&vector["argv"]));
    let env = object_to_string_map(&vector["env"]);
    let stdin = vector["stdin"].as_str().unwrap_or_default();
    let today = vector["clock"]["today"].as_str().unwrap_or("20260723");
    let transport = ScriptedHttpTransport::new(scripted_calls(vector));
    let clock = FakeClock::at_unix(clock_unix_seconds(vector));
    let files = fixture_files(vector);
    let build_identity = FakeBuildIdentityProvider::new(Some(json!({
        "version": "9.9.9",
        "revision": "abc123",
        "platform": {
            "system": "TestOS",
            "release": "1.0",
            "machine": "test64",
            "python": "3.test"
        }
    })));
    let client_item_ids = FakeClientItemIdProvider::new(
        vector
            .get("client_item_id")
            .and_then(Value::as_str)
            .unwrap_or("11111111111141118111111111111111"),
    );
    let link_pairing = ScriptedLinkJoinPairingSeam::new(vec![]);
    let link_serve = ScriptedLinkServeRunner::new(vec![]);

    let output = if vector["surface"].as_str() == Some("sol-import") {
        let import_args = argv.iter().skip(1).cloned().collect::<Vec<_>>();
        dispatch_sol_import_with_seams(
            &import_args,
            &env,
            stdin,
            today,
            DispatchSeams {
                transport: &transport,
                clock: Some(&clock),
                files: Some(&files),
                build_identity: Some(&build_identity),
                client_item_ids: Some(&client_item_ids),
                notification_sink: None,
            },
        )
    } else if vector["surface"].as_str() == Some("sol-status") {
        dispatch_sol_status_with_seams(
            &argv,
            &env,
            stdin,
            today,
            DispatchSeams {
                transport: &transport,
                clock: Some(&clock),
                files: Some(&files),
                build_identity: Some(&build_identity),
                client_item_ids: Some(&client_item_ids),
                notification_sink: None,
            },
        )
    } else if vector["surface"].as_str() == Some("sol-link") {
        let dispatch = dispatch_sol_link_with_seams(
            &argv,
            &env,
            stdin,
            today,
            LinkDispatchSeams {
                transport: &transport,
                clock: Some(&clock),
                files: Some(&files),
                link_pairing: Some(&link_pairing),
                link_serve: Some(&link_serve),
                link_status_probe: None,
            },
        );
        match dispatch {
            LinkDispatch::Buffered(output) => output,
            LinkDispatch::Resident { handler, args } => {
                let output = handler(CommandContext {
                    args: &args,
                    env: &env,
                    stdin,
                    today,
                    transport: &transport,
                    clock: Some(&clock),
                    files: Some(&files),
                    build_identity: None,
                    client_item_ids: None,
                    notification_sink: None,
                    link_pairing: Some(&link_pairing),
                    link_serve: Some(&link_serve),
                    link_status_probe: None,
                });
                match output {
                    Err(output) => output,
                    Ok(_) => panic!("resident parity vector entered the serve loop"),
                }
            }
        }
    } else {
        dispatch_sol_call_with_seams(
            &argv,
            &env,
            stdin,
            today,
            DispatchSeams {
                transport: &transport,
                clock: Some(&clock),
                files: Some(&files),
                build_identity: Some(&build_identity),
                client_item_ids: Some(&client_item_ids),
                notification_sink: None,
            },
        )
    };
    transport.assert_done();
    link_pairing.assert_done();
    link_serve.assert_done();
    let actual = json!({
        "stdout": output.stdout,
        "stderr": output.stderr,
        "exit": output.exit,
        "requests": recorded_calls_to_json(transport.recorded()),
    });
    let normalizations = normalization_array(vector);
    assert_eq!(
        normalize_result(actual, &normalizations),
        normalize_result(vector["expected"].clone(), &normalizations),
        "{}",
        vector["id"]
    );
}

fn load_vectors(text: &str) -> Vec<Value> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid parity vector"))
        .collect()
}

fn clock_unix_seconds(vector: &Value) -> u64 {
    vector["clock"]
        .get("unix_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn scripted_calls(vector: &Value) -> Vec<ExpectedHttpCall> {
    let vector_id = vector["id"].as_str().unwrap_or("<missing-id>");
    vector["transport"]["requests"]
        .as_array()
        .expect("transport requests")
        .iter()
        .map(|request| scripted_call(vector_id, request))
        .collect()
}

fn scripted_call(vector_id: &str, request: &Value) -> ExpectedHttpCall {
    let policy = timeout_policy(request["timeout_policy"].as_str().unwrap_or("api"));
    if request["method"].as_str() == Some("SSE") {
        return ExpectedHttpCall::Sse {
            expected: SseRequest {
                path: request["path"].as_str().expect("path").to_string(),
                policy,
            },
            chunks: request["chunks"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .map(|chunk| chunk.as_str().expect("SSE chunk").as_bytes().to_vec())
                .collect(),
        };
    }
    if request["method"].as_str() == Some("UPLOAD") {
        let form_values = request["multipart"]["data"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        return ExpectedHttpCall::Upload {
            expected: UploadRequest {
                path: request["path"].as_str().expect("path").to_string(),
                files: request["multipart"]["files"]
                    .as_array()
                    .expect("multipart files")
                    .iter()
                    .map(|file| MultipartFile {
                        field_name: file["field_name"].as_str().expect("field").to_string(),
                        filename: file["filename"].as_str().expect("filename").to_string(),
                        content_type: file
                            .get("content_type")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        body: vec![b'x'; file["length"].as_u64().expect("length") as usize],
                    })
                    .collect(),
                data: form_values
                    .iter()
                    .map(|pair| {
                        let pair = pair.as_array().expect("form pair");
                        FormField {
                            name: pair[0].as_str().expect("form key").to_string(),
                            value: pair[1].as_str().expect("form value").to_string(),
                        }
                    })
                    .collect(),
                headers: header_pairs(request.get("headers")),
                boundary: None,
                policy,
            },
            result: scripted_result(vector_id, request, policy),
        };
    }
    let query_values = request["query"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let api_request = ApiRequest {
        method: http_method(request["method"].as_str().expect("method")),
        path: request["path"].as_str().expect("path").to_string(),
        params: query_values
            .iter()
            .map(|pair| {
                let pair = pair.as_array().expect("query pair");
                QueryParam::single(
                    pair[0].as_str().expect("query key"),
                    pair[1].as_str().expect("query value"),
                )
            })
            .collect(),
        json: request
            .get("json")
            .filter(|value| !value.is_null())
            .cloned(),
        headers: header_pairs(request.get("headers")),
        policy,
    };
    let result = scripted_result(vector_id, request, policy);
    ExpectedHttpCall::Request {
        expected: api_request,
        result,
    }
}

fn scripted_result(
    vector_id: &str,
    request: &Value,
    policy: TimeoutPolicy,
) -> Result<HttpResponse, ClientError> {
    if let Some(response) = request.get("response") {
        Ok(HttpResponse {
            status: response
                .get("status")
                .and_then(Value::as_u64)
                .unwrap_or(200) as u16,
            headers: header_pairs(response.get("headers")),
            body: scripted_response_body(vector_id, response),
            policy,
        })
    } else {
        let fault = &request["fault"];
        // `service_down` models the pre-client require_solstone() exit (solstone
        // not running). Native has no preflight — a refused connection surfaces as
        // Unreachable, which the require_service=True apps render as the shared
        // service-down message. `unreachable` stays distinct for client-level
        // semantics (support portal fallback, chat service-down).
        if matches!(
            fault.get("kind").and_then(Value::as_str),
            Some("unreachable") | Some("service_down")
        ) {
            return Err(ClientError::unreachable(
                fault
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ));
        }
        if fault.get("kind").and_then(Value::as_str) == Some("timeout") {
            return Err(ClientError::timeout(
                fault
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ));
        }
        Err(ClientError::ReasonRejected {
            status: fault.get("status").and_then(Value::as_u64).unwrap_or(500) as u16,
            error: fault
                .get("error")
                .or_else(|| fault.get("reason_code"))
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_string(),
            reason_code: fault
                .get("reason_code")
                .and_then(Value::as_str)
                .map(str::to_string),
            detail: fault
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_string),
            payload: Box::new(fault.get("payload").cloned().unwrap_or(Value::Null)),
        })
    }
}

fn scripted_response_body(vector_id: &str, response: &Value) -> Vec<u8> {
    match (response.get("json"), response.get("raw_body")) {
        (Some(json), None) => serde_json::to_vec(json).expect("response JSON"),
        (None, Some(raw_body)) => raw_body
            .as_str()
            .unwrap_or_else(|| {
                panic!("parity vector {vector_id}: response.raw_body must be a string")
            })
            .as_bytes()
            .to_vec(),
        (Some(_), Some(_)) => panic!(
            "parity vector {vector_id}: response must set exactly one of response.json or response.raw_body"
        ),
        (None, None) => panic!(
            "parity vector {vector_id}: response must set exactly one of response.json or response.raw_body"
        ),
    }
}

fn recorded_calls_to_json(calls: Vec<RecordedHttpCall>) -> Value {
    Value::Array(
        calls
            .into_iter()
            .map(|call| match call {
                RecordedHttpCall::Request {
                    method,
                    path,
                    query,
                    json,
                    headers,
                    timeout_policy,
                } => json!({
                    "method": method,
                    "path": path,
                    "query": query_pairs_to_json(query),
                    "json": json,
                    "headers": header_pairs_to_json(headers),
                    "timeout_policy": timeout_policy,
                }),
                RecordedHttpCall::Upload {
                    path,
                    files,
                    data,
                    headers,
                    timeout_policy,
                } => json!({
                    "method": "UPLOAD",
                    "path": path,
                    "multipart": {
                        "files": files.into_iter().map(|file| json!({
                            "field_name": file.field_name,
                            "filename": file.filename,
                            "content_type": file.content_type,
                            "length": file.length,
                        })).collect::<Vec<_>>(),
                        "data": query_pairs_to_json(data),
                    },
                    "headers": header_pairs_to_json(headers),
                    "timeout_policy": timeout_policy,
                }),
                RecordedHttpCall::Sse {
                    path,
                    timeout_policy,
                } => json!({
                    "method": "SSE",
                    "path": path,
                    "headers": [],
                    "timeout_policy": timeout_policy,
                }),
            })
            .collect(),
    )
}

fn fixture_files(vector: &Value) -> FixtureFileProvider {
    let files = vector["files"]
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(relative, body)| {
                    (
                        PathBuf::from(format!("{FILE_ROOT}/{relative}")),
                        body.as_str().unwrap_or_default().as_bytes().to_vec(),
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut provider = FixtureFileProvider::new(files);
    if let Some(paths) = vector.get("unreadable_files").and_then(Value::as_array) {
        for relative in paths {
            provider.mark_unreadable(PathBuf::from(format!(
                "{FILE_ROOT}/{}",
                relative.as_str().expect("unreadable path")
            )));
        }
    }
    provider
}

fn expand_file_args(args: Vec<String>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.replace("{files}", FILE_ROOT))
        .collect()
}

fn object_to_string_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .expect("env object")
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

fn string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("string array")
        .iter()
        .map(|value| value.as_str().expect("string").to_string())
        .collect()
}

fn normalization_array(vector: &Value) -> Vec<String> {
    vector
        .get("normalizations")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(|value| value.as_str().expect("normalization").to_string())
        .collect()
}

fn http_method(value: &str) -> HttpMethod {
    match value {
        "DELETE" => HttpMethod::Delete,
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        other => panic!("unsupported method {other}"),
    }
}

fn timeout_policy(value: &str) -> TimeoutPolicy {
    match value {
        "api" => TimeoutPolicy::Api,
        "upload" => TimeoutPolicy::Upload,
        "sse-open" => TimeoutPolicy::SseOpen,
        other => panic!("unsupported timeout policy {other}"),
    }
}

fn header_pairs(value: Option<&Value>) -> Vec<(String, String)> {
    let pairs = value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    pairs
        .iter()
        .map(|pair| {
            let pair = pair.as_array().expect("header pair");
            (
                pair[0].as_str().expect("header name").to_string(),
                pair[1].as_str().expect("header value").to_string(),
            )
        })
        .collect()
}

fn query_pairs_to_json(pairs: Vec<(String, String)>) -> Value {
    Value::Array(
        pairs
            .into_iter()
            .map(|(key, value)| json!([key, value]))
            .collect(),
    )
}

fn header_pairs_to_json(pairs: Vec<(String, String)>) -> Value {
    Value::Array(
        pairs
            .into_iter()
            .map(|(key, value)| json!([key, value]))
            .collect(),
    )
}

fn normalize_result(mut value: Value, normalizations: &[String]) -> Value {
    for normalization in normalizations {
        match normalization.as_str() {
            "invalid-json-error-tail" => normalize_invalid_json_stderr(&mut value),
            "unreadable-file-error" => normalize_unreadable_file_stderr(&mut value),
            other => panic!("unsupported normalization {other}"),
        }
    }
    value
}

fn normalize_unreadable_file_stderr(value: &mut Value) {
    let stderr = value["stderr"].as_str().expect("stderr string");
    if stderr.contains("not readable")
        || stderr.contains("file is not readable")
        || (stderr.contains("Invalid value for 'FILES...'") && stderr.contains("readable"))
    {
        value["stderr"] = Value::String("Error: unreadable fixture file\n".to_string());
    }
}

fn normalize_invalid_json_stderr(value: &mut Value) {
    const PREFIX: &str = "Error: invalid JSON on stdin:";
    let stderr = value["stderr"].as_str().expect("stderr string");
    if stderr.starts_with(PREFIX) {
        value["stderr"] = Value::String(format!("{PREFIX} <parser error>\n"));
    }
}
