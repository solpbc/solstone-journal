// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-facing journal-source registration commands.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use chrono::{Local, TimeZone};
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, atomic_replace, write_bytes_exclusive,
};

use crate::cli_render::CliRun;

const USAGE: &str = "usage: journal importer journal-source [-h] [--json] [-v] [-d] {create,list,status,revoke} ...";
const STATE_AREAS: [&str; 5] = ["segments", "entities", "facets", "imports", "config"];

type Record = Map<String, Value>;

enum Command {
    Help,
    Create { name: String, json: bool },
    List { mode: Option<Mode>, json: bool },
    Status { name: Option<String>, json: bool },
    Revoke { name: String, json: bool },
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mode {
    Dl,
    Pl,
}

impl Mode {
    fn token(self) -> &'static str {
        match self {
            Self::Dl => "dl",
            Self::Pl => "pl",
        }
    }
}

/// Run the journal-source grammar against a local journal root.
pub fn run_cli(args: &[String], journal_path: &Path) -> CliRun {
    match parse_arguments(args) {
        Ok(Command::Help) => success(format!("{USAGE}\n")),
        Ok(Command::Create { name, json }) => cmd_create(journal_path, &name, json),
        Ok(Command::List { mode, json }) => cmd_list(journal_path, mode, json),
        Ok(Command::Status { name, json }) => cmd_status(journal_path, name.as_deref(), json),
        Ok(Command::Revoke { name, json }) => cmd_revoke(journal_path, &name, json),
        Err(error) => argparse_error(error),
    }
}

fn parse_arguments(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err(String::new());
    }
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        return Ok(Command::Help);
    }

    let mut index = 0;
    let mut json = false;
    while let Some(argument) = args.get(index) {
        match argument.as_str() {
            "--json" => {
                json = true;
                index += 1;
            }
            "-v" | "--verbose" | "-d" | "--debug" => index += 1,
            value if value.starts_with('-') => {
                return Err(format!("unrecognized arguments: {value}"));
            }
            _ => break,
        }
    }

    let Some(subcommand) = args.get(index).map(String::as_str) else {
        return Err(String::new());
    };
    index += 1;

    match subcommand {
        "create" => {
            let name = required_positional(args, &mut index, "name")?;
            reject_remaining(args, index)?;
            Ok(Command::Create { name, json })
        }
        "list" => {
            let mut mode = None;
            while let Some(argument) = args.get(index) {
                if argument == "--mode" {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| "argument --mode: expected one argument".to_owned())?;
                    mode = Some(parse_mode(value)?);
                    index += 1;
                } else if argument == "-h" || argument == "--help" {
                    return Ok(Command::Help);
                } else {
                    return Err(format!("unrecognized arguments: {argument}"));
                }
            }
            Ok(Command::List { mode, json })
        }
        "status" => {
            if matches!(args.get(index).map(String::as_str), Some("-h" | "--help")) {
                return Ok(Command::Help);
            }
            let name = args.get(index).cloned();
            if let Some(value) = name.as_deref().filter(|value| value.starts_with('-')) {
                return Err(format!("unrecognized arguments: {value}"));
            }
            if name.is_some() {
                index += 1;
            }
            reject_remaining(args, index)?;
            Ok(Command::Status { name, json })
        }
        "revoke" => {
            let name = required_positional(args, &mut index, "name")?;
            reject_remaining(args, index)?;
            Ok(Command::Revoke { name, json })
        }
        value => Err(format!(
            "argument command: invalid choice: '{value}' (choose from 'create', 'list', 'status', 'revoke')"
        )),
    }
}

fn required_positional(args: &[String], index: &mut usize, name: &str) -> Result<String, String> {
    let value = args
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("the following arguments are required: {name}"))?;
    if value.starts_with('-') {
        return Err(format!("the following arguments are required: {name}"));
    }
    *index += 1;
    Ok(value)
}

fn reject_remaining(args: &[String], index: usize) -> Result<(), String> {
    if index == args.len() {
        Ok(())
    } else {
        Err(format!(
            "unrecognized arguments: {}",
            args[index..].join(" ")
        ))
    }
}

fn parse_mode(value: &str) -> Result<Mode, String> {
    match value {
        "dl" => Ok(Mode::Dl),
        "pl" => Ok(Mode::Pl),
        _ => Err(format!(
            "argument --mode: invalid choice: '{value}' (choose from 'dl', 'pl')"
        )),
    }
}

fn cmd_create(journal: &Path, name: &str, json_output: bool) -> CliRun {
    if !is_valid_name(name) {
        return failure(
            "",
            &format!("Error: invalid journal source name '{name}'\n"),
            1,
        );
    }
    let records = scan_records(journal);
    if find_dl_by_name(&records, name).is_some() {
        return duplicate_error(name);
    }

    let key = match generate_key() {
        Ok(key) => key,
        Err(()) => return failure("", "Error: failed to save journal source\n", 1),
    };
    let prefix = &key[..8];
    let record = json!({
        "key": key,
        "name": name,
        "created_at": now_ms(),
        "enabled": true,
        "revoked": false,
        "revoked_at": Value::Null,
        "stats": zero_stats(),
    });
    let bytes = match serde_json::to_vec_pretty(&record) {
        Ok(bytes) => bytes,
        Err(_) => return failure("", "Error: failed to save journal source\n", 1),
    };
    let path = sources_dir(journal).join(format!("{name}.json"));
    match write_bytes_exclusive(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) }) {
        Ok(()) => {}
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists =>
        {
            return duplicate_error(name);
        }
        Err(_) => return failure("", "Error: failed to save journal source\n", 1),
    }
    if create_state_directory(journal, prefix).is_err()
        || append_audit(
            journal,
            "journal_source_create",
            json!({"name": name, "key_prefix": prefix}),
        )
        .is_err()
    {
        return failure("", "Error: failed to save journal source\n", 1);
    }

    if json_output {
        match serde_json::to_string(&json!({"name": name, "key": key, "prefix": prefix})) {
            Ok(output) => success(format!("{output}\n")),
            Err(_) => failure("", "Error: failed to save journal source\n", 1),
        }
    } else {
        success(format!(
            "Journal source created:\n  Name:       {name}\n  Prefix:     {prefix}\n  api key:     {key}\n"
        ))
    }
}

fn cmd_list(journal: &Path, mode: Option<Mode>, json_output: bool) -> CliRun {
    let mut records = scan_records(journal);
    if let Some(mode) = mode {
        records.retain(|record| row_mode(record) == mode);
    }
    if records.is_empty() {
        if json_output {
            return success("[]\n".to_owned());
        }
        return success(match mode {
            None => "No journal sources registered.\n".to_owned(),
            Some(mode) => format!("No journal sources match --mode {}.\n", mode.token()),
        });
    }

    let authorized = load_authorized_clients(journal);
    if json_output {
        let rows: Vec<Value> = records
            .iter()
            .filter_map(|record| json_list_row(record, &authorized))
            .collect();
        return match serde_json::to_string(&rows) {
            Ok(output) => success(format!("{output}\n")),
            Err(_) => failure("", "Error: failed to list journal sources\n", 1),
        };
    }

    let mut output = format!(
        "{:<4} {:<16} {:<16} {:<24} {:<8} {:<20} {:<20} {:<16}\n{}\n",
        "Mode",
        "Identifier",
        "Sender Instance",
        "Name / Label",
        "Status",
        "Paired",
        "Last Seen",
        "Created",
        "-".repeat(131),
    );
    for record in &records {
        if let Some(row) = human_list_row(record, &authorized) {
            output.push_str(&format!(
                "{:<4} {:<16} {:<16} {:<24} {:<8} {:<20} {:<20} {:<16}\n",
                row[0], row[1], row[2], row[3], row[4], row[5], row[6], row[7]
            ));
        }
    }
    success(output)
}

fn cmd_status(journal: &Path, name: Option<&str>, json_output: bool) -> CliRun {
    match name {
        Some(name) => status_single(journal, name, json_output),
        None => status_all(journal, json_output),
    }
}

fn status_single(journal: &Path, name: &str, json_output: bool) -> CliRun {
    let records = scan_records(journal);
    let Some(record) = find_dl_by_name(&records, name) else {
        return not_found_error(name);
    };
    let prefix = dl_prefix(record).expect("validated DL record");
    let status = status(record);
    let created_at = integer_field(record, "created_at");
    let revoked = bool_field(record, "revoked");
    let revoked_at = record.get("revoked_at").cloned().unwrap_or(Value::Null);
    let state_dir = journal.join("imports").join(prefix);
    let stats = stats_value(record);
    if json_output {
        return match serde_json::to_string(&json!({
            "name": string_field(record, "name").unwrap_or(""),
            "prefix": prefix,
            "status": status,
            "created_at": created_at,
            "revoked": revoked,
            "revoked_at": revoked_at,
            "state_dir": state_dir,
            "stats": stats,
        })) {
            Ok(output) => success(format!("{output}\n")),
            Err(_) => failure("", "Error: failed to read journal source\n", 1),
        };
    }
    let mut output = format!(
        "Journal source: {}\n  Prefix:     {prefix}\n  Status:     {status}\n  Created:    {}\n",
        string_field(record, "name").unwrap_or(""),
        fmt_time(record.get("created_at").and_then(Value::as_i64)),
    );
    if revoked {
        output.push_str(&format!(
            "  Revoked at: {}\n",
            fmt_time(revoked_at.as_i64())
        ));
    }
    output.push_str(&format!(
        "  State dir:  {}\n  Stats:\n    segments:   {}\n    entities:   {}\n    facets:     {}\n    imports:    {}\n    config:     {}\n",
        state_dir.display(),
        stat(record, "segments_received"),
        stat(record, "entities_received"),
        stat(record, "facets_received"),
        stat(record, "imports_received"),
        stat(record, "config_received"),
    ));
    success(output)
}

fn status_all(journal: &Path, json_output: bool) -> CliRun {
    let records: Vec<Record> = scan_records(journal)
        .into_iter()
        .filter(|record| row_mode(record) == Mode::Dl)
        .collect();
    if json_output {
        let rows: Vec<Value> = records
            .iter()
            .map(|record| status_overview_row(journal, record))
            .collect();
        return match serde_json::to_string(&rows) {
            Ok(output) => success(format!("{output}\n")),
            Err(_) => failure("", "Error: failed to read journal source\n", 1),
        };
    }
    if records.is_empty() {
        return success("No journal sources registered.\n".to_owned());
    }
    let mut output = format!(
        "{:<20} {:<10} {:<18} {:>5} {:>5} {:>5} {:>5} {:>5}\n{}\n",
        "Name",
        "Status",
        "Created",
        "Seg",
        "Ent",
        "Fac",
        "Imp",
        "Cfg",
        "-".repeat(82),
    );
    for record in &records {
        output.push_str(&format!(
            "{:<20} {:<10} {:<18} {:>5} {:>5} {:>5} {:>5} {:>5}\n",
            string_field(record, "name").unwrap_or(""),
            status(record),
            fmt_time(record.get("created_at").and_then(Value::as_i64)),
            stat(record, "segments_received"),
            stat(record, "entities_received"),
            stat(record, "facets_received"),
            stat(record, "imports_received"),
            stat(record, "config_received"),
        ));
    }
    success(output)
}

fn cmd_revoke(journal: &Path, name: &str, json_output: bool) -> CliRun {
    let records = scan_records(journal);
    let Some(mut record) = find_dl_by_name(&records, name).cloned() else {
        return not_found_error(name);
    };
    if bool_field(&record, "revoked") {
        return failure(
            "",
            &format!("Journal source '{name}' is already revoked.\n"),
            1,
        );
    }
    let prefix = dl_prefix(&record).expect("validated DL record").to_owned();
    record.insert("revoked".to_owned(), Value::Bool(true));
    record.insert("revoked_at".to_owned(), json!(now_ms()));
    let bytes = match serde_json::to_vec_pretty(&Value::Object(record)) {
        Ok(bytes) => bytes,
        Err(_) => return failure("", "Error: failed to save journal source\n", 1),
    };
    let path = sources_dir(journal).join(format!("{name}.json"));
    if atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) }).is_err() {
        return failure("", "Error: failed to save journal source\n", 1);
    }
    if append_audit(
        journal,
        "journal_source_revoke",
        json!({"name": name, "key_prefix": prefix}),
    )
    .is_err()
    {
        return failure("", "Error: failed to save journal source\n", 1);
    }
    if json_output {
        match serde_json::to_string(&json!({"name": name, "prefix": prefix, "revoked": true})) {
            Ok(output) => success(format!("{output}\n")),
            Err(_) => failure("", "Error: failed to save journal source\n", 1),
        }
    } else {
        success(format!("Revoked journal source '{name}' ({prefix})\n"))
    }
}

fn sources_dir(journal: &Path) -> PathBuf {
    journal.join("apps/import/journal_sources")
}

fn scan_records(journal: &Path) -> Vec<Record> {
    let directory = sources_dir(journal);
    if fs::create_dir_all(&directory).is_err() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut records: Vec<Record> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path
                .extension()
                .is_some_and(|extension| extension == "json"))
            .then_some(path)
        })
        .filter_map(|path| load_valid_record(&path))
        .collect();
    records.sort_by_key(|record| std::cmp::Reverse(integer_field(record, "created_at")));
    records
}

fn load_valid_record(path: &Path) -> Option<Record> {
    let contents = fs::read(path).ok()?;
    let mut record = serde_json::from_slice::<Value>(&contents)
        .ok()?
        .as_object()?
        .clone();
    validate_record(&record, path.file_name()?.to_str()?)?;
    record.remove("filename_prefix");
    Some(record)
}

fn validate_record(record: &Record, filename: &str) -> Option<()> {
    let key = string_field(record, "key").filter(|value| !value.is_empty());
    let fingerprint = string_field(record, "fingerprint").filter(|value| !value.is_empty());
    if key.is_some() == fingerprint.is_some() {
        return None;
    }
    let pair_mode = string_field(record, "pair_mode");
    if let Some(peer_instance_id) = record.get("peer_instance_id") {
        let peer_instance_id = peer_instance_id.as_str()?;
        if !valid_peer_instance_id(peer_instance_id) || pair_mode != Some("pl") {
            return None;
        }
    }
    let expected_filename = if let Some(fingerprint) = fingerprint {
        if pair_mode != Some("pl") {
            return None;
        }
        format!("{}.json", fingerprint_prefix(fingerprint)?)
    } else {
        if record
            .get("pair_mode")
            .is_some_and(|value| !value.is_null())
        {
            return None;
        }
        let key = key?;
        key_prefix(key)?;
        let name = string_field(record, "name")?;
        if !is_valid_name(name) {
            return None;
        }
        format!("{name}.json")
    };
    (filename == expected_filename).then_some(())
}

fn find_dl_by_name<'a>(records: &'a [Record], name: &str) -> Option<&'a Record> {
    if !is_valid_name(name) {
        return None;
    }
    records
        .iter()
        .find(|record| row_mode(record) == Mode::Dl && string_field(record, "name") == Some(name))
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

fn valid_peer_instance_id(value: &str) -> bool {
    (1..=256).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn fingerprint_prefix(fingerprint: &str) -> Option<&str> {
    let hex = fingerprint.strip_prefix("sha256:")?;
    if hex.len() == 64
        && hex
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'f' | b'0'..=b'9'))
    {
        Some(&hex[..16])
    } else {
        None
    }
}

fn dl_prefix(record: &Record) -> Option<&str> {
    key_prefix(string_field(record, "key")?)
}

fn key_prefix(key: &str) -> Option<&str> {
    key.get(..8)
}

fn row_mode(record: &Record) -> Mode {
    if string_field(record, "pair_mode") == Some("pl") {
        Mode::Pl
    } else {
        Mode::Dl
    }
}

fn generate_key() -> Result<String, ()> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| ())?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn zero_stats() -> Value {
    json!({
        "segments_received": 0,
        "entities_received": 0,
        "facets_received": 0,
        "imports_received": 0,
        "config_received": 0,
    })
}

fn create_state_directory(journal: &Path, prefix: &str) -> Result<(), ()> {
    let state_dir = journal.join("imports").join(prefix);
    fs::create_dir_all(&state_dir).map_err(|_| ())?;
    write_empty_if_absent(&state_dir.join("source.json"))?;
    for area in STATE_AREAS {
        let directory = state_dir.join(area);
        fs::create_dir_all(&directory).map_err(|_| ())?;
        write_empty_if_absent(&directory.join("state.json"))?;
    }
    Ok(())
}

fn write_empty_if_absent(path: &Path) -> Result<(), ()> {
    if path.exists() {
        return Ok(());
    }
    match write_bytes_exclusive(path, b"{}", AtomicWriteOptions::default()) {
        Ok(()) => Ok(()),
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists =>
        {
            Ok(())
        }
        Err(_) => Err(()),
    }
}

fn append_audit(journal: &Path, action: &str, params: Value) -> Result<(), ()> {
    solstone_core_facets::append_action_log(journal, None, "import", "import", action, params)
        .map_err(|_| ())
}

fn load_authorized_clients(journal: &Path) -> BTreeMap<String, Option<String>> {
    let path = journal.join("link/authorized_clients.json");
    let Ok(contents) = fs::read(path) else {
        return BTreeMap::new();
    };
    let Ok(Value::Array(entries)) = serde_json::from_slice::<Value>(&contents) else {
        return BTreeMap::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let fingerprint = string_field(entry, "fingerprint")?.to_owned();
            if entry.get("kind").is_some_and(|kind| kind != "cert") {
                return None;
            }
            Some((
                fingerprint,
                string_field(entry, "last_seen_at").map(str::to_owned),
            ))
        })
        .collect()
}

fn json_list_row(record: &Record, authorized: &BTreeMap<String, Option<String>>) -> Option<Value> {
    match row_mode(record) {
        Mode::Dl => Some(json!({
            "mode": "dl",
            "prefix": dl_prefix(record)?,
            "name": string_field(record, "name")?,
            "status": status(record),
            "created_at": integer_field(record, "created_at"),
        })),
        Mode::Pl => {
            let fingerprint = string_field(record, "fingerprint")?;
            let prefix = fingerprint_prefix(fingerprint)?;
            let entry = authorized.get(fingerprint);
            let mut row = Map::new();
            row.insert("mode".to_owned(), json!("pl"));
            row.insert("prefix".to_owned(), json!(prefix));
            row.insert("fingerprint".to_owned(), json!(fingerprint));
            row.insert(
                "device_label".to_owned(),
                json!(string_field(record, "device_label").unwrap_or("")),
            );
            row.insert("status".to_owned(), json!(status(record)));
            row.insert(
                "paired_at".to_owned(),
                json!(string_field(record, "paired_at").unwrap_or("")),
            );
            row.insert(
                "last_seen_at".to_owned(),
                entry.cloned().flatten().map_or(Value::Null, Value::String),
            );
            row.insert(
                "auth_status".to_owned(),
                json!(if entry.is_some() {
                    "present"
                } else {
                    "missing"
                }),
            );
            row.insert(
                "created_at".to_owned(),
                json!(integer_field(record, "created_at")),
            );
            if let Some(peer_instance_id) = record.get("peer_instance_id") {
                row.insert("peer_instance_id".to_owned(), peer_instance_id.clone());
            }
            Some(Value::Object(row))
        }
    }
}

fn human_list_row(
    record: &Record,
    authorized: &BTreeMap<String, Option<String>>,
) -> Option<[String; 8]> {
    match row_mode(record) {
        Mode::Dl => Some([
            "dl".to_owned(),
            dl_prefix(record)?.to_owned(),
            "—".to_owned(),
            string_field(record, "name")?.to_owned(),
            status(record).to_owned(),
            "—".to_owned(),
            "—".to_owned(),
            fmt_time(record.get("created_at").and_then(Value::as_i64)),
        ]),
        Mode::Pl => {
            let fingerprint = string_field(record, "fingerprint")?;
            let prefix = fingerprint_prefix(fingerprint)?;
            let last_seen = match authorized.get(fingerprint) {
                None => "(no auth)".to_owned(),
                Some(None) => "—".to_owned(),
                Some(Some(last_seen)) => last_seen.clone(),
            };
            Some([
                "pl".to_owned(),
                prefix.to_owned(),
                string_field(record, "peer_instance_id")
                    .unwrap_or("—")
                    .to_owned(),
                string_field(record, "device_label")
                    .unwrap_or("")
                    .to_owned(),
                status(record).to_owned(),
                string_field(record, "paired_at").unwrap_or("").to_owned(),
                last_seen,
                fmt_time(record.get("created_at").and_then(Value::as_i64)),
            ])
        }
    }
}

fn status_overview_row(journal: &Path, record: &Record) -> Value {
    let prefix = dl_prefix(record).expect("validated DL record");
    json!({
        "name": string_field(record, "name").unwrap_or(""),
        "prefix": prefix,
        "status": status(record),
        "created_at": integer_field(record, "created_at"),
        "stats": stats_value(record),
        "state_dir": journal.join("imports").join(prefix),
    })
}

fn string_field<'a>(record: &'a Record, key: &str) -> Option<&'a str> {
    record.get(key)?.as_str()
}

fn integer_field(record: &Record, key: &str) -> i64 {
    value_i64(record.get(key).unwrap_or(&Value::Null))
}

fn value_i64(value: &Value) -> i64 {
    value.as_i64().unwrap_or(0)
}

fn bool_field(record: &Record, key: &str) -> bool {
    record.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn status(record: &Record) -> &'static str {
    if bool_field(record, "revoked") {
        "revoked"
    } else {
        "active"
    }
}

fn stat(record: &Record, key: &str) -> u64 {
    record
        .get("stats")
        .and_then(Value::as_object)
        .and_then(|stats| stats.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn stats_value(record: &Record) -> Value {
    record.get("stats").cloned().unwrap_or_else(|| json!({}))
}

fn fmt_time(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|timestamp| Local.timestamp_millis_opt(timestamp).single())
        .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "never".to_owned())
}

fn argparse_error(arguments: String) -> CliRun {
    if arguments.is_empty() {
        return CliRun {
            stdout: format!("{USAGE}\n"),
            stderr: String::new(),
            exit_code: 1,
        };
    }
    CliRun {
        stdout: String::new(),
        stderr: format!("{USAGE}\njournal importer journal-source: error: {arguments}\n"),
        exit_code: 2,
    }
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(stdout: &str, stderr: &str, exit_code: i32) -> CliRun {
    CliRun {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        exit_code,
    }
}

fn duplicate_error(name: &str) -> CliRun {
    failure(
        "",
        &format!("Error: journal source '{name}' already exists\n"),
        1,
    )
}

fn not_found_error(name: &str) -> CliRun {
    failure(
        "",
        &format!("Error: journal source '{name}' not found\n"),
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_initialization_preserves_existing_files() {
        let root = tempfile::TempDir::new().unwrap();
        let state = root.path().join("imports/abcdefgh");
        create_state_directory(root.path(), "abcdefgh").unwrap();
        fs::write(state.join("source.json"), br#"{"source":"keep"}"#).unwrap();
        fs::write(state.join("segments/state.json"), br#"{"day":{}}"#).unwrap();

        create_state_directory(root.path(), "abcdefgh").unwrap();

        assert_eq!(
            fs::read(state.join("source.json")).unwrap(),
            br#"{"source":"keep"}"#
        );
        assert_eq!(
            fs::read(state.join("segments/state.json")).unwrap(),
            br#"{"day":{}}"#
        );
    }
}
