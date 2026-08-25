// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::command::{CommandContext, CommandOutput};
use crate::decode::decode_response;
use crate::error::ClientError;
use crate::json_format::sorted_json_compact_ascii;
use crate::transport::{
    ApiRequest, FormField, HttpMethod, MultipartFile, TimeoutPolicy, UploadRequest,
};

const IMPORT_API: &str = "/app/import/api";
const JOURNAL_HOST_HINT: &str = "Run this on the journal host with `journal importer`.";
const HELP: &str = "usage: solstone import [-h] [--timestamp TIMESTAMP] [--setting SETTING] [--source SOURCE] [--force] [--auto [AUTO]] [--deterministic-only] [--dry-run] [--backends] [--sync BACKEND] [--save] [--path PATH] [--list-importers] [--json] [-v] [media]\n\nImport media through the journal\n";

#[must_use]
pub fn import_top_level(ctx: CommandContext<'_>) -> CommandOutput {
    let parsed = match parse_args(ctx.args) {
        Ok(parsed) => parsed,
        Err(error) => return argparse_error(error),
    };
    if parsed.help {
        return CommandOutput::success(HELP);
    }
    if let Some(output) = reject_unsupported_modes(&parsed) {
        return output;
    }
    if !parsed.extra.is_empty() {
        return argparse_error(format!(
            "unexpected argument(s): {}",
            parsed.extra.join(" ")
        ));
    }
    if parsed.media.is_none() {
        return argparse_error("the following arguments are required: media".to_string());
    }
    run_import(ctx, &parsed)
}

#[derive(Debug, Clone, Default)]
struct ParsedArgs {
    media: Option<String>,
    extra: Vec<String>,
    timestamp: Option<String>,
    setting: Option<String>,
    source: Option<String>,
    force: bool,
    auto: AutoArg,
    deterministic_only: bool,
    dry_run: bool,
    json: bool,
    help: bool,
    backends: bool,
    sync: Option<String>,
    save: bool,
    path: Option<String>,
    list_importers: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum AutoArg {
    #[default]
    Absent,
    Bare,
    Guidance,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            for value in &args[index + 1..] {
                push_positional(&mut parsed, value.clone());
            }
            break;
        }
        if token == "-h" || token == "--help" {
            parsed.help = true;
        } else if token == "--force" {
            parsed.force = true;
        } else if token == "--deterministic-only" {
            parsed.deterministic_only = true;
        } else if token == "--dry-run" {
            parsed.dry_run = true;
        } else if token == "--json" {
            parsed.json = true;
        } else if token == "-v" || token == "--verbose" {
        } else if token == "--backends" {
            parsed.backends = true;
        } else if token == "--save" {
            parsed.save = true;
        } else if token == "--list-importers" {
            parsed.list_importers = true;
        } else if let Some(value) = token.strip_prefix("--timestamp=") {
            parsed.timestamp = Some(value.to_string());
        } else if token == "--timestamp" {
            parsed.timestamp = Some(take_value(args, &mut index, "--timestamp")?);
        } else if let Some(value) = token.strip_prefix("--setting=") {
            parsed.setting = Some(value.to_string());
        } else if token == "--setting" {
            parsed.setting = Some(take_value(args, &mut index, "--setting")?);
        } else if let Some(value) = token.strip_prefix("--source=") {
            parsed.source = Some(value.to_string());
        } else if token == "--source" {
            parsed.source = Some(take_value(args, &mut index, "--source")?);
        } else if let Some(value) = token.strip_prefix("--sync=") {
            parsed.sync = Some(value.to_string());
        } else if token == "--sync" {
            parsed.sync = Some(take_value(args, &mut index, "--sync")?);
        } else if let Some(value) = token.strip_prefix("--path=") {
            parsed.path = Some(value.to_string());
        } else if token == "--path" {
            parsed.path = Some(take_value(args, &mut index, "--path")?);
        } else if let Some(value) = token.strip_prefix("--auto=") {
            let _ = value;
            parsed.auto = AutoArg::Guidance;
        } else if token == "--auto" {
            if args
                .get(index + 1)
                .is_some_and(|value| !value.starts_with('-'))
            {
                parsed.auto = AutoArg::Guidance;
                index += 1;
            } else {
                parsed.auto = AutoArg::Bare;
            }
        } else if token.starts_with('-') {
            return Err(format!("unrecognized arguments: {token}"));
        } else {
            push_positional(&mut parsed, token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

fn take_value(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("argument {option}: expected one argument"))
}

fn push_positional(parsed: &mut ParsedArgs, value: String) {
    if parsed.media.is_none() {
        parsed.media = Some(value);
    } else {
        parsed.extra.push(value);
    }
}

fn reject_unsupported_modes(parsed: &ParsedArgs) -> Option<CommandOutput> {
    if parsed.media.as_deref() == Some("journal-source") {
        return Some(rejected(
            "journal-source management moved to `solstone call import <verb>`.",
        ));
    }
    if parsed.dry_run {
        return Some(rejected(format!(
            "`--dry-run` requires the journal host. {JOURNAL_HOST_HINT}"
        )));
    }
    if parsed.backends {
        return Some(rejected(format!(
            "`--backends` requires the journal host. {JOURNAL_HOST_HINT}"
        )));
    }
    if parsed.list_importers {
        return Some(rejected(format!(
            "`--list-importers` requires the journal host. {JOURNAL_HOST_HINT}"
        )));
    }
    if parsed.sync.is_some() {
        return Some(rejected(format!(
            "`--sync` requires the journal host. {JOURNAL_HOST_HINT}"
        )));
    }
    if parsed.save {
        return Some(rejected(format!(
            "`--save` requires the journal host. {JOURNAL_HOST_HINT}"
        )));
    }
    if parsed
        .path
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        return Some(rejected(format!(
            "`--path` requires the journal host. {JOURNAL_HOST_HINT}"
        )));
    }
    if matches!(parsed.auto, AutoArg::Guidance) {
        return Some(rejected(
            "`--auto <guidance>` requires the journal host. Use `--timestamp` here or run `journal importer`.",
        ));
    }
    None
}

fn rejected(message: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(format!("solstone import: {}\n", message.as_ref()), 2)
}

fn argparse_error(error: String) -> CommandOutput {
    CommandOutput::failure(format!("{HELP}solstone import: error: {error}\n"), 2)
}

fn run_import(ctx: CommandContext<'_>, parsed: &ParsedArgs) -> CommandOutput {
    let client_item_id = ctx
        .client_item_ids
        .map(|provider| provider.client_item_id())
        .unwrap_or_else(|| "00000000000000000000000000000000".to_string());
    let save_response = match save_media(ctx, parsed, &client_item_id) {
        Ok(response) => response,
        Err(ImportError::Unreachable) => {
            return CommandOutput::failure(
                "solstone import: couldn't reach the journal. Start it with 'journal up' and retry.\n",
                1,
            );
        }
        Err(error) => return print_client_error("stage import", error),
    };
    if save_response.get("status").and_then(Value::as_str) == Some("duplicate")
        || save_response
            .get("recommended_action")
            .and_then(Value::as_str)
            == Some("do_not_start")
    {
        return print_duplicate(&save_response, parsed.json);
    }
    let staged_path = save_response
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_else(|| parsed.media.as_deref().unwrap_or_default())
        .to_string();
    let start_response = match start_import(ctx, parsed, &save_response) {
        Ok(response) => response,
        Err(ImportError::Unreachable) => {
            return CommandOutput::failure(
                format!(
                    "solstone import: staged {staged_path} but processing was not queued: couldn't reach the journal\n"
                ),
                1,
            );
        }
        Err(error) => return print_partial_error(&staged_path, error),
    };
    print_success(&save_response, &start_response, parsed.json)
}

#[derive(Debug, Clone)]
enum ImportError {
    Unreachable,
    Malformed,
    Client {
        error: String,
        detail: Option<String>,
    },
}

fn save_media(
    ctx: CommandContext<'_>,
    parsed: &ParsedArgs,
    client_item_id: &str,
) -> Result<Map<String, Value>, ImportError> {
    let media = parsed.media.as_deref().unwrap_or_default();
    let media_path = expand_user(media, ctx.env);
    let mut data = save_data(parsed, client_item_id);
    let response = if ctx.files.is_some_and(|files| files.is_file(&media_path)) {
        let files = ctx.files.expect("checked files provider exists");
        let body = files
            .read(&media_path)
            .map_err(|error| ImportError::Client {
                error: error.to_string(),
                detail: None,
            })?;
        let filename = media_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        ctx.transport
            .upload(UploadRequest {
                path: format!("{IMPORT_API}/save"),
                files: vec![MultipartFile {
                    field_name: "file".to_string(),
                    filename,
                    content_type: Some("application/octet-stream".to_string()),
                    body,
                }],
                data: data
                    .into_iter()
                    .map(|(name, value)| FormField { name, value })
                    .collect(),
                headers: vec![],
                boundary: None,
                policy: TimeoutPolicy::Upload,
            })
            .map_err(map_transport_error)?
    } else {
        let mut payload = Map::new();
        for (key, value) in data.drain(..) {
            payload.insert(key, Value::String(value));
        }
        payload.insert("path".to_string(), Value::String(path_string(&media_path)));
        ctx.transport
            .request(ApiRequest {
                method: HttpMethod::Post,
                path: format!("{IMPORT_API}/save-path"),
                params: vec![],
                json: Some(Value::Object(payload)),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            })
            .map_err(map_transport_error)?
    };
    decode_object(response)
}

fn save_data(parsed: &ParsedArgs, client_item_id: &str) -> Vec<(String, String)> {
    let mut data = Vec::new();
    data.push(("client_item_id".to_string(), client_item_id.to_string()));
    push_payload_value(&mut data, "setting", parsed.setting.as_deref());
    push_payload_value(&mut data, "source_hint", parsed.source.as_deref());
    if parsed.deterministic_only {
        data.push(("deterministic_only".to_string(), "true".to_string()));
    }
    data
}

fn push_payload_value(data: &mut Vec<(String, String)>, key: &str, value: Option<&str>) {
    let Some(stripped) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    data.push((key.to_string(), stripped.to_string()));
}

fn start_import(
    ctx: CommandContext<'_>,
    parsed: &ParsedArgs,
    save_response: &Map<String, Value>,
) -> Result<Map<String, Value>, ImportError> {
    let path = save_response
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(ImportError::Malformed)?;
    let timestamp = parsed
        .timestamp
        .as_deref()
        .or_else(|| save_response.get("timestamp").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .ok_or(ImportError::Malformed)?;
    let response = ctx
        .transport
        .request(ApiRequest {
            method: HttpMethod::Post,
            path: format!("{IMPORT_API}/start"),
            params: vec![],
            json: Some(json!({
                "path": path,
                "timestamp": timestamp,
                "force": parsed.force,
            })),
            headers: vec![],
            policy: TimeoutPolicy::Api,
        })
        .map_err(map_transport_error)?;
    let object = decode_object(response)?;
    let task_id = object
        .get("task_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    if task_id.is_none() {
        return Err(ImportError::Malformed);
    }
    Ok(object)
}

fn decode_object(
    response: crate::transport::HttpResponse,
) -> Result<Map<String, Value>, ImportError> {
    let value = decode_response(&response).map_err(map_transport_error)?;
    value.as_object().cloned().ok_or(ImportError::Malformed)
}

fn map_transport_error(error: ClientError) -> ImportError {
    match error {
        ClientError::Unreachable { .. } => ImportError::Unreachable,
        ClientError::MalformedSuccess { .. } => ImportError::Malformed,
        other => ImportError::Client {
            error: other.message().to_string(),
            detail: other.detail().map(str::to_string),
        },
    }
}

fn print_client_error(operation: &str, error: ImportError) -> CommandOutput {
    match error {
        ImportError::Malformed => {
            CommandOutput::failure("solstone import: couldn't read journal response\n", 1)
        }
        ImportError::Client { error, detail } => {
            let mut stderr = format!("solstone import: failed to {operation}: {error}\n");
            if let Some(detail) = detail {
                stderr.push_str(&format!("solstone import: {detail}\n"));
            }
            CommandOutput::failure(stderr, 1)
        }
        ImportError::Unreachable => CommandOutput::failure(
            "solstone import: couldn't reach the journal. Start it with 'journal up' and retry.\n",
            1,
        ),
    }
}

fn print_partial_error(staged_path: &str, error: ImportError) -> CommandOutput {
    match error {
        ImportError::Malformed => CommandOutput::failure(
            format!(
                "solstone import: staged {staged_path} but processing was not queued: couldn't read journal response\n"
            ),
            1,
        ),
        ImportError::Client { error, detail } => {
            let mut stderr = format!(
                "solstone import: staged {staged_path} but processing was not queued: {error}\n"
            );
            if let Some(detail) = detail {
                stderr.push_str(&format!("solstone import: {detail}\n"));
            }
            CommandOutput::failure(stderr, 1)
        }
        ImportError::Unreachable => CommandOutput::failure(
            format!(
                "solstone import: staged {staged_path} but processing was not queued: couldn't reach the journal\n"
            ),
            1,
        ),
    }
}

fn print_success(
    save_response: &Map<String, Value>,
    start_response: &Map<String, Value>,
    json_out: bool,
) -> CommandOutput {
    let timestamp = save_response
        .get("timestamp")
        .cloned()
        .unwrap_or(Value::Null);
    let path = save_response.get("path").cloned().unwrap_or(Value::Null);
    if json_out {
        return CommandOutput::success(format!(
            "{}\n",
            sorted_json_compact_ascii(&json!({
                "status": "queued",
                "path": path,
                "timestamp": timestamp,
                "save": Value::Object(save_response.clone()),
                "start": Value::Object(start_response.clone()),
            }))
        ));
    }
    let mut stdout = String::new();
    stdout.push_str(&format!("staged {}\n", display_value(&path)));
    if let Some(timestamp) = timestamp.as_str().filter(|value| !value.is_empty()) {
        stdout.push_str(&format!("timestamp {timestamp}\n"));
    }
    if let Some(task_id) = start_response.get("task_id").and_then(Value::as_str) {
        stdout.push_str(&format!("queued processing task {task_id}\n"));
    } else {
        stdout.push_str("queued processing\n");
    }
    CommandOutput::success(stdout)
}

fn print_duplicate(save_response: &Map<String, Value>, json_out: bool) -> CommandOutput {
    if json_out {
        return CommandOutput::success(format!(
            "{}\n",
            sorted_json_compact_ascii(&Value::Object(save_response.clone()))
        ));
    }
    let duplicate = save_response.get("duplicate").and_then(Value::as_object);
    let Some(duplicate) = duplicate else {
        return CommandOutput::success("solstone import: duplicate import; skipping\n");
    };
    match duplicate.get("state").and_then(Value::as_str) {
        Some("imported") => {
            let imported_at = duplicate
                .get("imported_at")
                .and_then(Value::as_str)
                .unwrap_or("unknown date");
            let entries = duplicate
                .get("entry_count")
                .filter(|value| !value.is_null())
                .map(|count| format!(" ({} entries)", display_value(count)))
                .unwrap_or_default();
            CommandOutput::success(format!(
                "solstone import: already imported on {imported_at}{entries}; skipping\n"
            ))
        }
        Some("staged") => {
            let import_id = duplicate
                .get("import_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            CommandOutput::success(format!(
                "solstone import: already staged as {import_id}; skipping\n"
            ))
        }
        _ => CommandOutput::success("solstone import: duplicate import; skipping\n"),
    }
}

fn display_value(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn expand_user(value: &str, env: &BTreeMap<String, String>) -> PathBuf {
    if value == "~" {
        return env
            .get("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = env.get("HOME")
    {
        return Path::new(home).join(rest);
    }
    PathBuf::from(value)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{BTreeMap, HashMap};

    use serde_json::json;

    use crate::command::{CommandContext, CommandOutput};
    use crate::error::ClientError;
    use crate::seam::{
        ExpectedHttpCall, FakeClientItemIdProvider, FixtureFileProvider, ScriptedHttpTransport,
    };
    use crate::transport::{
        ApiRequest, FormField, HttpMethod, HttpResponse, MultipartFile, TimeoutPolicy,
        UploadRequest,
    };

    fn string_args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn json_response(value: Value, policy: TimeoutPolicy) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: vec![],
            body: serde_json::to_vec(&value).expect("json response"),
            policy,
        }
    }

    fn run_import_case(
        args: &[&str],
        transport: &ScriptedHttpTransport,
        files: &FixtureFileProvider,
        client_item_ids: &FakeClientItemIdProvider,
    ) -> CommandOutput {
        let args = string_args(args);
        let env = BTreeMap::new();
        import_top_level(CommandContext {
            args: &args,
            env: &env,
            stdin: "",
            today: "20260723",
            transport,
            clock: None,
            files: Some(files),
            build_identity: None,
            client_item_ids: Some(client_item_ids),
            notification_sink: None,
            link_pairing: None,
            link_serve: None,
        })
    }

    #[test]
    fn facet_is_rejected_while_setting_remains_a_supported_option() {
        let parsed = parse_args(&string_args(&["media.txt", "--setting", "office"]))
            .expect("setting parses");
        assert_eq!(parsed.setting.as_deref(), Some("office"));
        assert!(parse_args(&string_args(&["media.txt", "--facet", "work"])).is_err());
    }

    #[test]
    fn local_file_uses_multipart_upload_with_generated_client_item_id() {
        let mut fixtures = HashMap::new();
        fixtures.insert(PathBuf::from("/tmp/sample.txt"), b"hello".to_vec());
        let files = FixtureFileProvider::new(fixtures);
        let client_item_ids = FakeClientItemIdProvider::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let transport = ScriptedHttpTransport::new(vec![
            ExpectedHttpCall::Upload {
                expected: UploadRequest {
                    path: "/app/import/api/save".to_string(),
                    files: vec![MultipartFile {
                        field_name: "file".to_string(),
                        filename: "sample.txt".to_string(),
                        content_type: Some("application/octet-stream".to_string()),
                        body: b"hello".to_vec(),
                    }],
                    data: vec![
                        FormField {
                            name: "client_item_id".to_string(),
                            value: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                        },
                        FormField {
                            name: "setting".to_string(),
                            value: "office".to_string(),
                        },
                        FormField {
                            name: "source_hint".to_string(),
                            value: "ics".to_string(),
                        },
                        FormField {
                            name: "deterministic_only".to_string(),
                            value: "true".to_string(),
                        },
                    ],
                    headers: vec![],
                    boundary: None,
                    policy: TimeoutPolicy::Upload,
                },
                result: Ok(json_response(
                    json!({
                        "path": "/staged/imports/20260101_120000/sample.txt",
                        "timestamp": "20260101_120000"
                    }),
                    TimeoutPolicy::Upload,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/import/api/start".to_string(),
                    params: vec![],
                    json: Some(json!({
                        "path": "/staged/imports/20260101_120000/sample.txt",
                        "timestamp": "20260101_120000",
                        "force": true,
                    })),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({"status": "ok", "task_id": "task-file"}),
                    TimeoutPolicy::Api,
                )),
            },
        ]);

        let output = run_import_case(
            &[
                "/tmp/sample.txt",
                "--setting",
                " office ",
                "--source",
                " ics ",
                "--deterministic-only",
                "--force",
            ],
            &transport,
            &files,
            &client_item_ids,
        );

        assert_eq!(
            output,
            CommandOutput {
                stdout: "staged /staged/imports/20260101_120000/sample.txt\ntimestamp 20260101_120000\nqueued processing task task-file\n".to_string(),
                stderr: String::new(),
                exit: 0,
            }
        );
        transport.assert_done();
    }

    #[test]
    fn host_path_uses_save_path_and_timestamp_override_for_start() {
        let files = FixtureFileProvider::default();
        let client_item_ids = FakeClientItemIdProvider::new("bbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb");
        let transport = ScriptedHttpTransport::new(vec![
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/import/api/save-path".to_string(),
                    params: vec![],
                    json: Some(json!({
                        "client_item_id": "bbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb",
                        "path": "/journal-host/media/source-dir"
                    })),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "path": "/staged/imports/20260101_130000/source-dir",
                        "timestamp": "20260101_130000"
                    }),
                    TimeoutPolicy::Api,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/import/api/start".to_string(),
                    params: vec![],
                    json: Some(json!({
                        "path": "/staged/imports/20260101_130000/source-dir",
                        "timestamp": "20260202_030405",
                        "force": false,
                    })),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({"status": "ok", "task_id": "task-path"}),
                    TimeoutPolicy::Api,
                )),
            },
        ]);

        let output = run_import_case(
            &[
                "/journal-host/media/source-dir",
                "--timestamp",
                "20260202_030405",
            ],
            &transport,
            &files,
            &client_item_ids,
        );

        assert_eq!(
            output.stdout,
            "staged /staged/imports/20260101_130000/source-dir\ntimestamp 20260101_130000\nqueued processing task task-path\n"
        );
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit, 0);
        transport.assert_done();
    }

    #[test]
    fn json_output_is_sorted_and_includes_save_and_start_payloads() {
        let files = FixtureFileProvider::default();
        let client_item_ids = FakeClientItemIdProvider::new("cccccccccccc4ccc8ccccccccccccccc");
        let transport = ScriptedHttpTransport::new(vec![
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/import/api/save-path".to_string(),
                    params: vec![],
                    json: Some(json!({
                        "client_item_id": "cccccccccccc4ccc8ccccccccccccccc",
                        "path": "media.txt"
                    })),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "path": "/staged/imports/20260101_140000/media.txt",
                        "timestamp": "20260101_140000"
                    }),
                    TimeoutPolicy::Api,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/import/api/start".to_string(),
                    params: vec![],
                    json: Some(json!({
                        "path": "/staged/imports/20260101_140000/media.txt",
                        "timestamp": "20260101_140000",
                        "force": false,
                    })),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({"status": "ok", "task_id": "task-json"}),
                    TimeoutPolicy::Api,
                )),
            },
        ]);

        let output = run_import_case(
            &["media.txt", "--json"],
            &transport,
            &files,
            &client_item_ids,
        );

        assert_eq!(
            output.stdout,
            "{\"path\": \"/staged/imports/20260101_140000/media.txt\", \"save\": {\"path\": \"/staged/imports/20260101_140000/media.txt\", \"timestamp\": \"20260101_140000\"}, \"start\": {\"status\": \"ok\", \"task_id\": \"task-json\"}, \"status\": \"queued\", \"timestamp\": \"20260101_140000\"}\n"
        );
        assert_eq!(output.stderr, "");
        assert_eq!(output.exit, 0);
        transport.assert_done();
    }

    #[test]
    fn host_only_modes_reject_with_frozen_messages() {
        let cases: Vec<(Vec<&str>, &str)> = vec![
            (
                vec!["media.txt", "--dry-run"],
                "solstone import: `--dry-run` requires the journal host. Run this on the journal host with `journal importer`.\n",
            ),
            (
                vec!["--backends"],
                "solstone import: `--backends` requires the journal host. Run this on the journal host with `journal importer`.\n",
            ),
            (
                vec!["--list-importers"],
                "solstone import: `--list-importers` requires the journal host. Run this on the journal host with `journal importer`.\n",
            ),
            (
                vec!["--sync", "plaud"],
                "solstone import: `--sync` requires the journal host. Run this on the journal host with `journal importer`.\n",
            ),
            (
                vec!["media.txt", "--save"],
                "solstone import: `--save` requires the journal host. Run this on the journal host with `journal importer`.\n",
            ),
            (
                vec!["media.txt", "--path", "/tmp/source"],
                "solstone import: `--path` requires the journal host. Run this on the journal host with `journal importer`.\n",
            ),
            (
                vec!["media.txt", "--auto", "timestamps are Pacific"],
                "solstone import: `--auto <guidance>` requires the journal host. Use `--timestamp` here or run `journal importer`.\n",
            ),
            (
                vec!["journal-source", "list"],
                "solstone import: journal-source management moved to `solstone call import <verb>`.\n",
            ),
        ];

        for (args, stderr) in cases {
            let files = FixtureFileProvider::default();
            let client_item_ids = FakeClientItemIdProvider::new("11111111111141118111111111111111");
            let transport = ScriptedHttpTransport::new(vec![]);
            let output = run_import_case(&args, &transport, &files, &client_item_ids);
            assert_eq!(
                output,
                CommandOutput {
                    stdout: String::new(),
                    stderr: stderr.to_string(),
                    exit: 2,
                },
                "{args:?}"
            );
            transport.assert_done();
        }
    }

    #[test]
    fn malformed_save_response_uses_frozen_error() {
        let files = FixtureFileProvider::default();
        let client_item_ids = FakeClientItemIdProvider::new("11111111111141118111111111111111");
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/app/import/api/save-path".to_string(),
                params: vec![],
                json: Some(json!({
                    "client_item_id": "11111111111141118111111111111111",
                    "path": "media.txt"
                })),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(json_response(json!([]), TimeoutPolicy::Api)),
        }]);

        let output = run_import_case(&["media.txt"], &transport, &files, &client_item_ids);

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: "solstone import: couldn't read journal response\n".to_string(),
                exit: 1,
            }
        );
        transport.assert_done();
    }

    #[test]
    fn unreachable_save_response_uses_frozen_error() {
        let files = FixtureFileProvider::default();
        let client_item_ids = FakeClientItemIdProvider::new("11111111111141118111111111111111");
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/app/import/api/save-path".to_string(),
                params: vec![],
                json: Some(json!({
                    "client_item_id": "11111111111141118111111111111111",
                    "path": "media.txt"
                })),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Err(ClientError::unreachable(Some(
                "connection refused".to_string(),
            ))),
        }]);

        let output = run_import_case(&["media.txt"], &transport, &files, &client_item_ids);

        assert_eq!(
            output,
            CommandOutput {
                stdout: String::new(),
                stderr: "solstone import: couldn't reach the journal. Start it with 'journal up' and retry.\n".to_string(),
                exit: 1,
            }
        );
        transport.assert_done();
    }

    #[test]
    fn duplicate_save_response_short_circuits_without_start_request() {
        let files = FixtureFileProvider::default();
        let client_item_ids = FakeClientItemIdProvider::new("11111111111141118111111111111111");
        let transport = ScriptedHttpTransport::new(vec![ExpectedHttpCall::Request {
            expected: ApiRequest {
                method: HttpMethod::Post,
                path: "/app/import/api/save-path".to_string(),
                params: vec![],
                json: Some(json!({
                    "client_item_id": "11111111111141118111111111111111",
                    "path": "media.txt"
                })),
                headers: vec![],
                policy: TimeoutPolicy::Api,
            },
            result: Ok(json_response(
                json!({
                    "status": "duplicate",
                    "recommended_action": "do_not_start",
                    "duplicate": {
                        "state": "staged",
                        "import_id": "20260101_150000"
                    }
                }),
                TimeoutPolicy::Api,
            )),
        }]);

        let output = run_import_case(&["media.txt"], &transport, &files, &client_item_ids);

        assert_eq!(
            output,
            CommandOutput {
                stdout: "solstone import: already staged as 20260101_150000; skipping\n"
                    .to_string(),
                stderr: String::new(),
                exit: 0,
            }
        );
        transport.assert_done();
    }

    #[test]
    fn staged_but_not_queued_partial_failure_has_no_success_output() {
        let files = FixtureFileProvider::default();
        let client_item_ids = FakeClientItemIdProvider::new("11111111111141118111111111111111");
        let transport = ScriptedHttpTransport::new(vec![
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/import/api/save-path".to_string(),
                    params: vec![],
                    json: Some(json!({
                        "client_item_id": "11111111111141118111111111111111",
                        "path": "media.txt"
                    })),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Ok(json_response(
                    json!({
                        "path": "/staged/imports/20260101_160000/media.txt",
                        "timestamp": "20260101_160000"
                    }),
                    TimeoutPolicy::Api,
                )),
            },
            ExpectedHttpCall::Request {
                expected: ApiRequest {
                    method: HttpMethod::Post,
                    path: "/app/import/api/start".to_string(),
                    params: vec![],
                    json: Some(json!({
                        "path": "/staged/imports/20260101_160000/media.txt",
                        "timestamp": "20260101_160000",
                        "force": false,
                    })),
                    headers: vec![],
                    policy: TimeoutPolicy::Api,
                },
                result: Err(ClientError::ReasonRejected {
                    status: 500,
                    error: "queue failed".to_string(),
                    reason_code: Some("import_metadata_failed".to_string()),
                    detail: None,
                    payload: Box::new(json!({"error": "queue failed"})),
                }),
            },
        ]);

        let output = run_import_case(&["media.txt"], &transport, &files, &client_item_ids);

        assert_eq!(output.stdout, "");
        assert_eq!(
            output.stderr,
            "solstone import: staged /staged/imports/20260101_160000/media.txt but processing was not queued: queue failed\n"
        );
        assert_eq!(output.exit, 1);
        assert!(!output.stdout.contains("queued processing"));
        transport.assert_done();
    }
}
