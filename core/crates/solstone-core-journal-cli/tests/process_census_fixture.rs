// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[allow(dead_code)]
#[path = "../src/processes.rs"]
mod production_processes;

const PROCESS_CENSUS_JSON: &str =
    include_str!("../../../fixtures/native-journal/process-census-v1.json");
const EXPECTED_PREDECESSOR_COMMIT: &str = "d8200fdf34e4af31f106c7f28fb73cd439d0081b";
const EXPECTED_PREDECESSOR_BLOB: &str = "ea62371d5c320329724d051032efbe20f165b25f";
const EXPECTED_CENSUS_SHA256: &str =
    "915756eb24e7ee0ea891067a0d2ea01275b3a9fe26caf085f89adc18a1723db6";

fn entries<'a>(fixture: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    fixture[key]
        .as_array()
        .ok_or_else(|| format!("{key} must be an array"))
}

fn text<'a>(entry: &'a Value, key: &str) -> Result<&'a str, String> {
    entry[key]
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))
}

fn token_set(items: &[Value]) -> Result<BTreeSet<&str>, String> {
    items.iter().map(|entry| text(entry, "token")).collect()
}

fn presets(entry: &Value) -> Result<Vec<&str>, String> {
    entry["preset_argv"]
        .as_array()
        .ok_or_else(|| "preset_argv must be an array".to_owned())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "preset_argv values must be strings".to_owned())
        })
        .collect()
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn census_digest(commands: &[Value], aliases: &[Value]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for (kind, items) in [("command", commands), ("alias", aliases)] {
        for entry in items {
            hash_field(&mut hasher, kind);
            hash_field(&mut hasher, text(entry, "token")?);
            hash_field(&mut hasher, text(entry, "module")?);
            hash_field(&mut hasher, text(entry, "surface")?);
            let preset_argv = presets(entry)?;
            hasher.update((preset_argv.len() as u64).to_be_bytes());
            for arg in preset_argv {
                hash_field(&mut hasher, arg);
            }
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn production_digest() -> String {
    let mut hasher = Sha256::new();
    for spec in production_processes::PROCESS_SPECS {
        hash_field(&mut hasher, spec.kind.census_kind());
        hash_field(&mut hasher, spec.token);
        hash_field(&mut hasher, spec.module);
        hash_field(&mut hasher, spec.kind.surface());
        hasher.update((spec.preset_argv.len() as u64).to_be_bytes());
        for arg in spec.preset_argv {
            hash_field(&mut hasher, arg);
        }
    }
    format!("{:x}", hasher.finalize())
}

fn validate_fixture(fixture: &Value) -> Result<(), String> {
    if fixture.as_object().map(serde_json::Map::len) != Some(4) {
        return Err("fixture must have exactly four fields".to_owned());
    }
    if fixture["schema"] != "native-journal-process-census-v1" {
        return Err("unexpected schema".to_owned());
    }
    if fixture["predecessor"].as_object().map(serde_json::Map::len) != Some(3)
        || fixture["predecessor"]["path"] != "solstone/think/sol_cli.py"
        || fixture["predecessor"]["commit"] != EXPECTED_PREDECESSOR_COMMIT
        || fixture["predecessor"]["git_blob"] != EXPECTED_PREDECESSOR_BLOB
    {
        return Err("unexpected predecessor provenance".to_owned());
    }

    let commands = entries(fixture, "commands")?;
    let aliases = entries(fixture, "aliases")?;
    if commands.len() != 41 || aliases.len() != 2 {
        return Err("expected 41 process commands and two aliases".to_owned());
    }
    if commands
        .iter()
        .chain(aliases)
        .any(|entry| entry.as_object().map(serde_json::Map::len) != Some(4))
    {
        return Err("every census entry must have exactly four fields".to_owned());
    }

    let service_commands: BTreeSet<_> = commands
        .iter()
        .filter(|entry| entry["surface"] == "service")
        .map(|entry| text(entry, "token"))
        .collect::<Result<_, _>>()?;
    let universal_commands: BTreeSet<_> = commands
        .iter()
        .filter(|entry| entry["surface"] == "universal")
        .map(|entry| text(entry, "token"))
        .collect::<Result<_, _>>()?;
    if service_commands.len() != 38
        || universal_commands != BTreeSet::from(["check", "contract", "doctor"])
        || commands
            .iter()
            .any(|entry| !matches!(entry["surface"].as_str(), Some("service" | "universal")))
    {
        return Err("unexpected command surfaces".to_owned());
    }

    let alias_tokens = token_set(aliases)?;
    if alias_tokens != BTreeSet::from(["down", "up"])
        || aliases.iter().any(|entry| entry["surface"] != "service")
    {
        return Err("unexpected alias tokens or surfaces".to_owned());
    }
    let service_tokens: BTreeSet<_> = service_commands
        .iter()
        .copied()
        .chain(alias_tokens.iter().copied())
        .collect();
    if service_tokens
        != solstone_core_cli_boundary::JOURNAL_HOST_COMMANDS
            .iter()
            .copied()
            .collect()
    {
        return Err("service tokens differ from the boundary manifest".to_owned());
    }

    let command_tokens = token_set(commands)?;
    if !command_tokens.is_disjoint(&alias_tokens) || command_tokens.len() + alias_tokens.len() != 43
    {
        return Err("process tokens must be unique".to_owned());
    }

    for entry in commands.iter().chain(aliases) {
        let module = text(entry, "module")?;
        if !module.starts_with("solstone.") || module.chars().any(char::is_whitespace) {
            return Err(format!("invalid module: {module}"));
        }
    }
    if commands.iter().any(|entry| match presets(entry) {
        Ok(args) => !args.is_empty(),
        Err(_) => true,
    }) {
        return Err("named commands must not have preset argv".to_owned());
    }

    let alias_presets: BTreeMap<_, Vec<_>> = aliases
        .iter()
        .map(|entry| Ok((text(entry, "token")?, presets(entry)?)))
        .collect::<Result<_, String>>()?;
    if alias_presets != BTreeMap::from([("down", vec!["down"]), ("up", vec!["up"])]) {
        return Err("unexpected alias preset argv".to_owned());
    }
    if census_digest(commands, aliases)? != EXPECTED_CENSUS_SHA256 {
        return Err("ordered process census digest mismatch".to_owned());
    }
    Ok(())
}

#[test]
fn process_census_is_hash_bound_and_complete() {
    let fixture: Value = serde_json::from_str(PROCESS_CENSUS_JSON).expect("parse process census");
    validate_fixture(&fixture).expect("validate process census");
}

#[test]
fn production_process_table_matches_the_hash_bound_census() {
    assert_eq!(production_processes::PROCESS_SPECS.len(), 43);
    assert_eq!(production_processes::process_tokens().count(), 43);
    for spec in production_processes::PROCESS_SPECS {
        assert_eq!(
            production_processes::process_spec_for(spec.token),
            Some(spec)
        );
    }
    assert_eq!(production_digest(), EXPECTED_CENSUS_SHA256);
}

#[test]
fn validation_rejects_mapping_and_alias_corruption() {
    let fixture: Value = serde_json::from_str(PROCESS_CENSUS_JSON).expect("parse process census");

    let mut swapped_module = fixture.clone();
    let first = swapped_module["commands"][0]["module"].clone();
    let second = swapped_module["commands"][1]["module"].clone();
    swapped_module["commands"][0]["module"] = second;
    swapped_module["commands"][1]["module"] = first;
    assert!(validate_fixture(&swapped_module).is_err());

    let mut altered_surface = fixture.clone();
    altered_surface["aliases"][0]["surface"] = Value::String("universal".to_owned());
    assert!(validate_fixture(&altered_surface).is_err());

    let mut extra_preset = fixture.clone();
    extra_preset["aliases"][0]["preset_argv"]
        .as_array_mut()
        .expect("preset array")
        .push(Value::String("extra".to_owned()));
    assert!(validate_fixture(&extra_preset).is_err());

    let mut duplicate_alias = fixture;
    duplicate_alias["aliases"][1]["token"] = duplicate_alias["aliases"][0]["token"].clone();
    assert!(validate_fixture(&duplicate_alias).is_err());
}
