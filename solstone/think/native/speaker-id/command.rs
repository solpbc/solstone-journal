// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_speaker_id::corrections::append_correction;
use solstone_core_speaker_id::labels::{patch_labels, write_full_labels, write_stub_labels};

use crate::command::{CommandContext, CommandOutput};

const HELP: &str = "usage: sol speaker-id [-h] <action> <segment-dir>\n\nExercise native speaker-label and speaker-correction read/write against a segment directory.\n\nActions: full, stub, patch, append-correction\n";

#[must_use]
pub fn speaker_id(ctx: CommandContext<'_>) -> CommandOutput {
    if matches!(ctx.args, [flag] if flag == "-h" || flag == "--help") {
        return CommandOutput::success(HELP);
    }
    let [action, segment_dir] = ctx.args else {
        return argparse_error("the following arguments are required: action, segment_dir");
    };
    let payload = match parse_payload(ctx.stdin) {
        Ok(payload) => payload,
        Err(error) => return argparse_error(error),
    };
    let segment_dir = Path::new(segment_dir);

    let result = match action.as_str() {
        "full" => run_full(segment_dir, payload),
        "stub" => run_stub(segment_dir, payload),
        "patch" => run_patch(segment_dir, payload),
        "append-correction" => run_append_correction(segment_dir, payload),
        _ => return argparse_error(format!("unknown action: {action}")),
    };
    match result {
        Ok(()) => CommandOutput::success(format!(
            "{}\n",
            json!({"action": action, "segment_dir": segment_dir.display().to_string()})
        )),
        Err(error) => CommandOutput::failure(format!("sol speaker-id: error: {error}\n"), 1),
    }
}

fn parse_payload(stdin: &str) -> Result<Map<String, Value>, String> {
    serde_json::from_str::<Value>(stdin)
        .map_err(|error| format!("invalid JSON payload: {error}"))?
        .as_object()
        .cloned()
        .ok_or_else(|| "payload must be a JSON object".to_owned())
}

fn run_full(segment_dir: &Path, payload: Map<String, Value>) -> Result<(), String> {
    let labels = payload
        .get("labels")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "full payload requires labels to be an array".to_owned())?;
    let metadata = match payload.get("metadata") {
        None => Map::new(),
        Some(Value::Object(metadata)) => metadata.clone(),
        Some(_) => return Err("full payload metadata must be an object".to_owned()),
    };
    write_full_labels(segment_dir, labels, &metadata).map_err(|error| error.to_string())
}

fn run_stub(segment_dir: &Path, payload: Map<String, Value>) -> Result<(), String> {
    let reason = payload
        .get("reason")
        .and_then(Value::as_str)
        .ok_or_else(|| "stub payload requires reason to be a string".to_owned())?;
    write_stub_labels(segment_dir, reason).map_err(|error| error.to_string())
}

fn run_patch(segment_dir: &Path, payload: Map<String, Value>) -> Result<(), String> {
    let allow_insert = match payload.get("allow_insert") {
        None => false,
        Some(Value::Bool(allow_insert)) => *allow_insert,
        Some(_) => return Err("patch payload allow_insert must be a boolean".to_owned()),
    };
    let patch_values = payload
        .get("patches")
        .and_then(Value::as_array)
        .ok_or_else(|| "patch payload requires patches to be an array".to_owned())?;
    let mut patches = Vec::with_capacity(patch_values.len());
    for patch in patch_values {
        let patch = patch
            .as_object()
            .ok_or_else(|| "each patch must be an object".to_owned())?;
        let sentence_id = patch
            .get("sentence_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| "each patch sentence_id must be an integer".to_owned())?;
        let fields = patch
            .get("fields")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| "each patch fields must be an object".to_owned())?;
        patches.push((sentence_id, fields));
    }
    patch_labels(segment_dir, &patches, allow_insert).map_err(|error| error.to_string())
}

fn run_append_correction(segment_dir: &Path, payload: Map<String, Value>) -> Result<(), String> {
    append_correction(segment_dir, payload).map_err(|error| error.to_string())
}

fn argparse_error(error: impl AsRef<str>) -> CommandOutput {
    CommandOutput::failure(
        format!("{HELP}sol speaker-id: error: {}\n", error.as_ref()),
        2,
    )
}
