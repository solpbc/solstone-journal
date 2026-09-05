// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_sol_client::aggregate::{self, InventoryEntry};
use solstone_core_sol_client::command::CommandOutput;

const ROOT_CONTRACT_JSON: &str = include_str!("../../../fixtures/native-sol/root-contract-v1.json");

#[must_use]
pub fn render_root_help() -> String {
    let contract = root_contract();
    let mut output = String::new();
    push_line(&mut output, contract["header"].as_str().unwrap_or_default());
    output.push('\n');
    push_line(&mut output, contract["usage"].as_str().unwrap_or_default());
    output.push('\n');
    if let Some(groups) = contract["access_groups"].as_array() {
        for group in groups {
            push_line(&mut output, group["heading"].as_str().unwrap_or_default());
            if let Some(commands) = group["commands"].as_array() {
                for command in commands {
                    push_line(
                        &mut output,
                        &format!("  {}", command.as_str().unwrap_or_default()),
                    );
                }
            }
            output.push('\n');
        }
    }
    push_line(&mut output, "Apps (solstone call <app>):");
    for group in root_call_groups() {
        push_line(&mut output, &format!("  call {group}"));
    }
    output.push('\n');
    output
}

#[must_use]
pub fn render_call_root_help() -> String {
    let mut output = String::from("Usage: solstone call <app> <verb> [args...]\n\nCommands:\n");
    for group in root_call_groups() {
        push_line(&mut output, &format!("  {group}"));
    }
    output
}

#[must_use]
pub fn render_sol_call_help(args: &[String]) -> Option<CommandOutput> {
    if args.is_empty() {
        return Some(CommandOutput::success(render_call_root_help()));
    }
    if args.len() == 1 && is_help(args[0].as_str()) {
        return Some(CommandOutput::success(render_call_root_help()));
    }
    if args.iter().any(|arg| arg == "--") {
        return None;
    }
    if args.len() < 2 || !is_help(args.last()?.as_str()) {
        return None;
    }
    let path = &args[..args.len() - 1];
    if let Some(entry) = leaf_for_path("sol-call", path) {
        return Some(CommandOutput::success(render_leaf_help(
            &format!("solstone call {}", path.join(" ")),
            entry,
        )));
    }
    if is_sol_call_group(path) {
        return Some(CommandOutput::success(render_group_help(path)));
    }
    None
}

#[must_use]
pub fn render_link_help(args: &[String]) -> Option<CommandOutput> {
    if !args.is_empty() && !(args.len() == 1 && is_help(args[0].as_str())) {
        return None;
    }
    let path = vec![String::from("link")];
    if !is_surface_group("sol-link", &path) {
        return None;
    }
    Some(CommandOutput::success(render_surface_group_help(
        "sol-link",
        "solstone link",
        &path,
    )))
}

#[must_use]
pub fn render_top_level_help(command: &str, args: &[String]) -> Option<CommandOutput> {
    if args.len() != 1 || !is_help(args[0].as_str()) {
        return None;
    }
    let surface = match command {
        "import" => "sol-import",
        "status" => "sol-status",
        _ => return None,
    };
    let entry = leaf_for_path(surface, &[command.to_string()])?;
    Some(CommandOutput::success(render_leaf_help(
        &format!("solstone {command}"),
        entry,
    )))
}

#[must_use]
pub fn render_leaf_help(invocation: &str, entry: &InventoryEntry) -> String {
    let mut output = String::new();
    push_line(&mut output, &format!("Usage: {invocation} [args...]"));
    output.push('\n');
    push_line(&mut output, entry.help);
    output.push('\n');
    push_line(&mut output, &format!("Authority: {}", entry.authority_path));
    push_line(&mut output, &format!("Operation: {}", entry.operation_id));

    let params = visible_params(entry);
    let arguments = params
        .iter()
        .filter(|param| param["kind"].as_str() == Some("argument"))
        .collect::<Vec<_>>();
    let options = params
        .iter()
        .filter(|param| param["kind"].as_str() == Some("option"))
        .collect::<Vec<_>>();
    if !arguments.is_empty() {
        output.push('\n');
        push_line(&mut output, "Arguments:");
        for param in arguments {
            push_line(&mut output, &format!("  {}", format_param(param)));
        }
    }
    if !options.is_empty() {
        output.push('\n');
        push_line(&mut output, "Options:");
        for param in options {
            push_line(&mut output, &format!("  {}", format_param(param)));
        }
    }
    output
}

#[must_use]
pub fn render_group_help(path: &[String]) -> String {
    render_surface_group_help(
        "sol-call",
        &format!("solstone call {}", path.join(" ")),
        path,
    )
}

#[must_use]
pub fn render_surface_group_help(surface: &str, invocation: &str, path: &[String]) -> String {
    let mut output = String::new();
    push_line(
        &mut output,
        &format!("Usage: {invocation} <command> [args...]"),
    );
    output.push('\n');
    push_line(&mut output, "Commands:");
    for child in immediate_children(surface, path) {
        let mut line = format!("  {}", child.name);
        if let Some(help) = child.help {
            line.push_str(&format!("{:width$}{}", "", help, width = child.padding));
        }
        push_line(&mut output, &line);
    }
    output
}

#[must_use]
pub fn is_sol_call_group(path: &[String]) -> bool {
    is_surface_group("sol-call", path)
}

fn is_surface_group(surface: &str, path: &[String]) -> bool {
    if path.is_empty() || leaf_for_path(surface, path).is_some() {
        return false;
    }
    aggregate::entries()
        .iter()
        .filter(|entry| entry.surface == surface)
        .any(|entry| is_strict_prefix(path, entry.path))
}

fn root_contract() -> Value {
    serde_json::from_str(ROOT_CONTRACT_JSON)
        .expect("native sol root contract fixture is valid JSON")
}

fn root_call_groups() -> Vec<String> {
    root_contract()["call_groups"]
        .as_array()
        .expect("root contract call_groups is an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("root contract call group is a string")
                .to_string()
        })
        .collect()
}

fn leaf_for_path(surface: &str, path: &[String]) -> Option<&'static InventoryEntry> {
    let borrowed = path.iter().map(String::as_str).collect::<Vec<_>>();
    aggregate::entries()
        .iter()
        .find(|entry| entry.surface == surface && entry.path == borrowed.as_slice())
}

fn visible_params(entry: &InventoryEntry) -> Vec<Value> {
    serde_json::from_str::<Vec<Value>>(entry.params_json)
        .expect("generated params_json is valid")
        .into_iter()
        .filter(|param| !param["hidden"].as_bool().unwrap_or(false))
        .collect()
}

fn format_param(param: &Value) -> String {
    let spellings = format_spellings(param);
    let name = param["name"].as_str().unwrap_or("");
    let type_name = param["type"].as_str().unwrap_or("value");
    let mut line = if param["is_flag"].as_bool().unwrap_or(false) {
        spellings
    } else {
        format!("{spellings} <{name}:{type_name}>")
    };
    let mut indicators: Vec<String> = Vec::new();
    if param["required"].as_bool().unwrap_or(false) {
        indicators.push("required".to_string());
    }
    if !param["default"].is_null() {
        indicators.push(format!("default: {}", value_label(&param["default"])));
    }
    if !param["flag_value"].is_null() && string_values(param, "secondary").is_empty() {
        indicators.push(format!("flag value: {}", value_label(&param["flag_value"])));
    }
    if param["multiple"].as_bool().unwrap_or(false) {
        indicators.push("multiple".to_string());
    }
    if param["count"].as_bool().unwrap_or(false) {
        indicators.push("count".to_string());
    }
    if !indicators.is_empty() {
        line.push_str(&format!(" [{}]", indicators.join(", ")));
    }
    line
}

fn format_spellings(param: &Value) -> String {
    let options = string_values(param, "options");
    let secondary = string_values(param, "secondary");
    match (options.is_empty(), secondary.is_empty()) {
        (false, false) => format!("{} / {}", options.join(", "), secondary.join(", ")),
        (false, true) => options.join(", "),
        (true, false) => secondary.join(", "),
        (true, true) => String::new(),
    }
}

#[cfg(test)]
fn all_spellings(param: &Value) -> Vec<&str> {
    let mut spellings = string_values(param, "options");
    spellings.extend(string_values(param, "secondary"));
    spellings
}

fn string_values<'a>(param: &'a Value, key: &str) -> Vec<&'a str> {
    param[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn value_label(value: &Value) -> String {
    match value {
        Value::String(text) => text.to_string(),
        _ => value.to_string(),
    }
}

#[derive(Debug)]
struct Child {
    name: String,
    help: Option<&'static str>,
    padding: usize,
}

fn immediate_children(surface: &str, path: &[String]) -> Vec<Child> {
    let mut names: Vec<String> = Vec::new();
    for entry in aggregate::entries()
        .iter()
        .filter(|entry| entry.surface == surface)
    {
        if entry.path.len() <= path.len() || !path_matches(path, entry.path) {
            continue;
        }
        let child = entry.path[path.len()].to_string();
        if !names.contains(&child) {
            names.push(child);
        }
    }
    let max_len = names.iter().map(String::len).max().unwrap_or(0);
    names
        .into_iter()
        .map(|name| {
            let child_path = path
                .iter()
                .cloned()
                .chain(std::iter::once(name.clone()))
                .collect::<Vec<_>>();
            let help = leaf_for_path(surface, &child_path).map(|entry| entry.help);
            Child {
                padding: max_len.saturating_sub(name.len()) + 2,
                name,
                help,
            }
        })
        .collect()
}

fn path_matches(path: &[String], candidate: &[&str]) -> bool {
    path.len() <= candidate.len()
        && path
            .iter()
            .zip(candidate.iter())
            .all(|(left, right)| left == right)
}

fn is_strict_prefix(path: &[String], candidate: &[&str]) -> bool {
    path.len() < candidate.len() && path_matches(path, candidate)
}

fn is_help(value: &str) -> bool {
    matches!(value, "-h" | "--help" | "help")
}

fn push_line(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn status_top_level_help_is_inventory_driven() {
        let output = render_top_level_help("status", &["--help".to_string()])
            .expect("status top-level help");

        assert_eq!(output.stderr, "");
        assert_eq!(output.exit, 0);
        assert!(output.stdout.contains("Usage: solstone status [args...]"));
        assert!(output.stdout.contains("Show journal network status."));
        assert!(
            output
                .stdout
                .contains("Authority: core/native-sol/think/native/status/authority.toml")
        );
        assert!(output.stdout.contains("Operation: status.top_level"));
    }

    #[test]
    fn root_call_groups_come_from_fixture() {
        assert_eq!(root_call_groups().len(), 18);
        assert!(root_call_groups().contains(&"journal".to_string()));
    }

    #[test]
    fn link_group_help_is_inventory_driven() {
        let output = render_link_help(&[]).expect("bare link help");

        assert_eq!(output.stderr, "");
        assert_eq!(output.exit, 0);
        assert!(
            output
                .stdout
                .contains("Usage: solstone link <command> [args...]")
        );
        assert!(output.stdout.contains("  join"));
        assert!(
            output
                .stdout
                .contains("join a solstone with a short code or pair link")
        );
        assert!(output.stdout.contains("  serve"));
        assert!(
            output
                .stdout
                .contains("serve a paired journal over the local link bridge")
        );

        let flag_output = render_link_help(&["--help".to_string()]).expect("flagged link help");
        assert_eq!(flag_output, output);
    }

    #[test]
    fn every_positive_inventory_leaf_renders_declared_metadata() {
        let mut seen = BTreeSet::new();
        let mut scanned = 0;
        let mut secondary_scanned = 0;
        for entry in aggregate::entries() {
            if !matches!(entry.surface, "sol-call" | "sol-import" | "sol-link") {
                continue;
            }
            scanned += 1;
            let key = (entry.surface, entry.path);
            assert!(seen.insert(key), "duplicate inventory path: {key:?}");
            let invocation = if entry.surface == "sol-call" {
                format!("solstone call {}", entry.path.join(" "))
            } else {
                format!("solstone {}", entry.path.join(" "))
            };
            let output = render_leaf_help(&invocation, entry);
            assert!(output.contains(entry.help), "missing help for {key:?}");
            assert!(
                output.contains(entry.authority_path),
                "missing authority path for {key:?}"
            );
            assert!(
                output.contains(entry.operation_id),
                "missing operation id for {key:?}"
            );
            let params = serde_json::from_str::<Vec<Value>>(entry.params_json).unwrap();
            for param in params {
                let spellings = all_spellings(&param);
                if param["hidden"].as_bool().unwrap_or(false) {
                    for spelling in spellings {
                        assert!(
                            !output.contains(spelling),
                            "hidden metadata leaked for {key:?}: {spelling}"
                        );
                    }
                    continue;
                }
                secondary_scanned += string_values(&param, "secondary").len();
                for spelling in spellings {
                    assert!(
                        output.contains(spelling),
                        "missing option spelling for {key:?}: {spelling}"
                    );
                }
                if param["required"].as_bool().unwrap_or(false) {
                    assert!(
                        output.contains("required"),
                        "missing required indicator for {key:?}"
                    );
                }
                if !param["default"].is_null() {
                    assert!(
                        output.contains("default:"),
                        "missing default indicator for {key:?}"
                    );
                }
            }
        }
        assert!(scanned > 0, "positive inventory scan was empty");
        assert!(secondary_scanned > 0, "secondary alias scan was empty");
    }

    #[test]
    fn every_group_renders_immediate_membership() {
        let mut groups = BTreeSet::new();
        for entry in aggregate::entries()
            .iter()
            .filter(|entry| entry.surface == "sol-call")
        {
            for depth in 1..entry.path.len() {
                let path = entry.path[..depth]
                    .iter()
                    .map(|item| item.to_string())
                    .collect::<Vec<_>>();
                if is_sol_call_group(&path) {
                    groups.insert(path);
                }
            }
        }
        assert!(!groups.is_empty(), "group scan was empty");
        for group in groups {
            let output = render_group_help(&group);
            assert!(
                !output.contains("Authority:"),
                "group help fabricated authority line for {group:?}"
            );
            for child in immediate_children("sol-call", &group) {
                assert!(
                    output.contains(&format!("  {}", child.name)),
                    "missing group child {child:?} in {group:?}"
                );
                if let Some(help) = child.help {
                    assert!(
                        output.contains(help),
                        "missing child help for {child:?} in {group:?}"
                    );
                }
            }
        }
    }
}
