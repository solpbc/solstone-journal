// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use solstone_core_sol_client_cli::resolve_surface_leaf;

fn main() -> Result<(), String> {
    let paths = std::env::args()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Err("usage: resolve-parity-leaves <parity.jsonl>...".to_string());
    }
    for path in paths {
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let vector: Value = serde_json::from_str(line)
                .map_err(|error| format!("{}:{}: {error}", path.display(), index + 1))?;
            let id = vector["id"]
                .as_str()
                .ok_or_else(|| format!("{}:{}: missing id", path.display(), index + 1))?;
            let surface = vector
                .get("surface")
                .and_then(Value::as_str)
                .unwrap_or("sol-call");
            let argv = vector["argv"]
                .as_array()
                .ok_or_else(|| format!("{}:{}: missing argv", path.display(), index + 1))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("{}:{}: non-string argv", path.display(), index + 1))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let lookup_args = match surface {
                "sol-chat" => vec!["chat".to_string()],
                "sol-import" => vec!["import".to_string()],
                "sol-status" => vec!["status".to_string()],
                "sol-link" => link_lookup_args(&argv),
                "sol-notify" => vec!["notify".to_string()],
                _ => argv,
            };
            let entry = resolve_surface_leaf(surface, &lookup_args);
            println!(
                "{}",
                json!({
                    "id": id,
                    "surface": surface,
                    "operation_id": entry.map(|entry| entry.operation_id),
                    "entry_type": entry.map(|entry| entry.entry_type),
                })
            );
        }
    }
    Ok(())
}

fn link_lookup_args(argv: &[String]) -> Vec<String> {
    match argv {
        [command, verb, ..] if command == "link" => vec![String::from("link"), verb.clone()],
        [verb, ..] => vec![String::from("link"), verb.clone()],
        [] => vec![String::from("link")],
    }
}
