// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut entries: Vec<_> = fs::read_dir(directory)
        .expect("embedded asset directory is readable")
        .map(|entry| entry.expect("embedded asset directory entry is readable"))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path);
        }
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("jpeg") | Some("jpg") => "image/jpeg",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn scan_top_level(source: &str, target: u8, mut on_offset: impl FnMut(usize) -> bool) {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (offset, byte) in bytes.iter().copied().enumerate() {
        if let Some(quote_byte) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote_byte {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ if byte == target && depth == 0 && on_offset(offset) => return,
            _ => {}
        }
    }
}

fn first_top_level_offset(source: &str, target: u8) -> Option<usize> {
    let mut first = None;
    scan_top_level(source, target, |offset| {
        first = Some(offset);
        true
    });
    first
}

fn top_level_offsets(source: &str, target: u8) -> Vec<usize> {
    let mut offsets = Vec::new();
    scan_top_level(source, target, |offset| {
        offsets.push(offset);
        false
    });
    offsets
}

fn assignment_end(source: &str, start: usize) -> usize {
    first_top_level_offset(&source[start..], b'\n').map_or(source.len(), |offset| start + offset)
}

fn python_strings(expression: &str, constant_name: &str) -> Vec<String> {
    let bytes = expression.as_bytes();
    let mut strings = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\'' && bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let delimiter = bytes[index];
        index += 1;
        let mut value = Vec::new();
        while index < bytes.len() && bytes[index] != delimiter {
            if bytes[index] == b'\\' {
                assert!(
                    index + 1 < bytes.len(),
                    "copy constant {constant_name} ends with a backslash; \
                     this parser does not support general Python escape decoding"
                );
                let escaped = bytes[index + 1];
                assert!(
                    matches!(escaped, b'\\' | b'\'' | b'"'),
                    "copy constant {constant_name} uses unsupported Python escape \\{}; \
                     this parser does not support general Python escape decoding; \
                     use different quoting or extend the parser",
                    escaped as char
                );
                index += 1;
            }
            value.push(bytes[index]);
            index += 1;
        }
        assert!(
            index < bytes.len(),
            "copy constant {constant_name} string is terminated"
        );
        index += 1;
        let value = match String::from_utf8(value) {
            Ok(value) => value,
            Err(error) => panic!("copy constant {constant_name} is UTF-8: {error}"),
        };
        strings.push(value);
    }
    strings
}

fn python_dict(expression: &str, constant_name: &str) -> serde_json::Map<String, Value> {
    let expression = expression.trim();
    assert!(
        expression.starts_with('{'),
        "copy constant {constant_name} dictionary is open"
    );
    assert!(
        expression.ends_with('}'),
        "copy constant {constant_name} dictionary is closed"
    );
    let body = &expression[1..expression.len() - 1];
    let mut members = serde_json::Map::new();
    let mut member_start = 0;
    let mut member_ends = top_level_offsets(body, b',');
    member_ends.push(body.len());
    if !body.trim().is_empty() {
        for member_end in member_ends {
            let member = &body[member_start..member_end];
            if member.trim().is_empty() {
                assert!(
                    member_end == body.len() && member_start > 0,
                    "copy constant {constant_name} dictionary has an empty member"
                );
            } else {
                let colon = first_top_level_offset(member, b':').unwrap_or_else(|| {
                    panic!("copy constant {constant_name} dictionary member is missing `:`")
                });
                let key_source = &member[..colon];
                let value_source = &member[colon + 1..];
                assert!(
                    !key_source.trim().is_empty(),
                    "copy constant {constant_name} dictionary member has an empty key"
                );
                assert!(
                    !value_source.trim().is_empty(),
                    "copy constant {constant_name} dictionary member has an empty value"
                );
                let key_strings = python_strings(key_source, constant_name);
                if value_source.trim() == "None" {
                    let key = key_strings.concat();
                    assert!(
                        !key.is_empty(),
                        "copy constant {constant_name} dictionary member has an empty key"
                    );
                    members.insert(key, Value::Null);
                    member_start = member_end + usize::from(member_end < body.len());
                    continue;
                }
                let value_strings = python_strings(value_source, constant_name);
                assert!(
                    !key_strings.is_empty(),
                    "copy constant {constant_name} dictionary member key has no string value"
                );
                assert!(
                    !value_strings.is_empty(),
                    "copy constant {constant_name} dictionary member value has no string value"
                );
                let key = key_strings.concat();
                assert!(
                    !key.is_empty(),
                    "copy constant {constant_name} dictionary member has an empty key"
                );
                members.insert(key, Value::String(value_strings.concat()));
            }
            member_start = member_end + usize::from(member_end < body.len());
        }
    }
    assert!(
        !members.is_empty(),
        "copy constant {constant_name} dictionary has no members"
    );
    members
}

fn python_constants(source: &str, required_prefix: Option<&str>) -> serde_json::Map<String, Value> {
    let mut constants = serde_json::Map::new();
    let mut line_offset = 0;
    for source_line in source.split_inclusive('\n') {
        let offset = line_offset;
        line_offset += source_line.len();
        let line = source_line.strip_suffix('\n').unwrap_or(source_line);
        let Some((name, _)) = line.split_once(" =") else {
            continue;
        };
        let name_bytes = name.as_bytes();
        let name_is_constant = matches!(name_bytes.first(), Some(b'A'..=b'Z'))
            && name_bytes[1..]
                .iter()
                .all(|byte| matches!(byte, b'A'..=b'Z' | b'0'..=b'9' | b'_'));
        if !name_is_constant || !required_prefix.is_none_or(|prefix| name.starts_with(prefix)) {
            continue;
        }
        assert!(
            line.as_bytes()[name.len()..].starts_with(b" = "),
            "copy constant {name} assignment uses expected `NAME = expression` shape"
        );
        let start = offset + name.len() + 3;
        let end = assignment_end(source, start);
        let expression = &source[start..end];
        let trimmed = expression.trim_start();
        let value = match trimmed.as_bytes().first() {
            Some(b'[') => {
                let strings = python_strings(expression, name);
                assert!(
                    !strings.is_empty(),
                    "copy constant {name} list has no string values"
                );
                Value::Array(strings.into_iter().map(Value::String).collect())
            }
            Some(b'{') => Value::Object(python_dict(trimmed, name)),
            _ => {
                let strings = python_strings(expression, name);
                assert!(
                    !strings.is_empty(),
                    "copy constant {name} string has no string value"
                );
                Value::String(strings.concat())
            }
        };
        constants.insert(name.to_owned(), value);
    }
    constants
}

fn python_constant(source: &str, name: &str) -> String {
    let assignment = format!("{name} =");
    let start = source.find(&assignment).expect("copy constant exists") + assignment.len();
    let end = assignment_end(source, start);
    python_strings(&source[start..end], name).concat()
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest
        .join("../../..")
        .canonicalize()
        .expect("repository root resolves");
    let static_root = root.join("solstone/convey/static");
    let speakers_root = manifest.join("assets/speakers");
    let speakers_copy = speakers_root.join("copy.py");
    let network_copy = root.join("solstone/apps/network/copy.py");
    let outcomes = root.join("solstone/think/services/outcomes.py");
    let pairing_config = root.join("solstone/think/pairing/config.py");
    let entities_workspace = manifest.join("assets/entities/workspace.html");
    let body_workspace = manifest.join("assets/body/workspace.html");
    let favicon = root.join("favicon.ico");
    let workspace = speakers_root.join("workspace.html");
    let speakers_static = speakers_root.join("who_is_this.js");
    let devices_workspace = manifest.join("assets/devices/workspace.html");
    let thinking_root = manifest.join("assets/thinking");
    let thinking_workspace = thinking_root.join("workspace.html");
    let thinking_static = thinking_root.join("thinking.js");
    let network_root = manifest.join("assets/network");
    let network_workspace = network_root.join("workspace.html");
    let network_static = network_root.join("network.js");

    let mut files = Vec::new();
    collect_files(&static_root, &mut files);
    for path in [
        &favicon,
        &workspace,
        &speakers_static,
        &speakers_copy,
        &network_copy,
        &outcomes,
        &pairing_config,
        &body_workspace,
        &devices_workspace,
        &entities_workspace,
        &thinking_workspace,
        &thinking_static,
        &network_workspace,
        &network_static,
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let mut assets = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&static_root)
            .expect("static asset is under static root")
            .to_string_lossy()
            .replace('\\', "/");
        assets.push((format!("/static/{relative}"), path));
    }
    assets.push(("/favicon.ico".to_owned(), favicon));
    assets.push(("/app/body/workspace".to_owned(), body_workspace));
    assets.push(("/app/devices/workspace".to_owned(), devices_workspace));
    assets.push(("/app/entities/workspace".to_owned(), entities_workspace));
    assets.push(("/app/speakers/workspace".to_owned(), workspace));
    assets.push((
        "/app/speakers/static/who_is_this.js".to_owned(),
        speakers_static,
    ));
    assets.push(("/app/thinking/workspace".to_owned(), thinking_workspace));
    assets.push((
        "/app/thinking/static/thinking.js".to_owned(),
        thinking_static,
    ));
    assets.push(("/app/network/workspace".to_owned(), network_workspace));
    assets.push(("/app/network/static/network.js".to_owned(), network_static));
    assets.sort_by(|left, right| left.0.cmp(&right.0));

    let copy_source = fs::read_to_string(&speakers_copy).expect("copy source is readable");
    let speaker_copy =
        serde_json::to_string(&Value::Object(python_constants(&copy_source, Some("SPK_"))))
            .expect("copy payload serializes");
    let network_copy_source = match fs::read_to_string(&network_copy) {
        Ok(source) => source,
        Err(error) => panic!(
            "network copy source {} is readable: {error}",
            network_copy.display()
        ),
    };
    let network_copy_json =
        serde_json::to_string(&Value::Object(python_constants(&network_copy_source, None)))
            .expect("network copy serializes");
    let outcomes_source = fs::read_to_string(&outcomes).expect("SPL outcome source is readable");
    let spl_outcome_strings_json = serde_json::to_string(&Value::Object(python_constants(
        &outcomes_source,
        Some("SPL_"),
    )))
    .expect("SPL outcome copy serializes");
    let pairing_config_source =
        fs::read_to_string(&pairing_config).expect("pairing config source is readable");
    let home_address_strings_json = serde_json::to_string(&Value::Object(python_constants(
        &pairing_config_source,
        Some("HOME_ADDRESS_"),
    )))
    .expect("home address copy serializes");
    let not_in_new_voices =
        serde_json::to_string(&python_constant(&copy_source, "TR_NOT_IN_NEW_VOICES"))
            .expect("copy string serializes");

    let mut generated = String::from("pub(super) static GENERATED_ASSETS: &[EmbeddedAsset] = &[\n");
    for (route, path) in assets {
        generated.push_str(&format!(
            "    EmbeddedAsset {{ path: {route:?}, content_type: {:?}, bytes: include_bytes!({:?}) }},\n",
            content_type(&path),
            path,
        ));
    }
    generated.push_str("];\n");
    generated.push_str(&format!(
        "pub(super) const SPEAKER_COPY_JSON: &str = {speaker_copy:?};\n"
    ));
    generated.push_str(&format!(
        "pub(super) const NETWORK_COPY_JSON: &str = {network_copy_json:?};\n"
    ));
    generated.push_str(&format!(
        "#[allow(dead_code)]\npub(super) const SPL_OUTCOME_STRINGS_JSON: &str = {spl_outcome_strings_json:?};\n"
    ));
    generated.push_str(&format!(
        "#[allow(dead_code)]\npub(super) const HOME_ADDRESS_STRINGS_JSON: &str = {home_address_strings_json:?};\n"
    ));
    generated.push_str(&format!(
        "pub(super) const NOT_IN_NEW_VOICES_COPY: &str = {not_in_new_voices};\n"
    ));

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output dir"));
    fs::write(output.join("embedded_assets.rs"), generated)
        .expect("generated embedded asset source writes");
}
