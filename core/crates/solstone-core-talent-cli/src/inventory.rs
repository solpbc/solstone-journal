// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::fmt::Write;
use std::path::Path;

use regex::Regex;
use serde_json::{Map, Value};
use solstone_core_cogitate::{
    COGITATE_ACCESS_TIERS, COGITATE_JOURNAL_COMMANDS, FinalizationConfig, FinalizationValue,
    capabilities_for_access_tier, expects_emit_final,
};
use solstone_core_cogitate_tools::bound_tools;

use crate::args::InventoryOptions;
use crate::compose::compose_talent;
use crate::discovery::{self, TalentConfig};
use crate::templates::python_string_any;
use crate::validation::is_truthy;

struct InventoryRow {
    name: String,
    schedule: String,
    cwd: String,
    write: String,
    output: String,
    access_tier: String,
    finalize: String,
    sol: bool,
    reads: bool,
    submit: bool,
    command_examples: Vec<String>,
    error: Option<String>,
}

struct TierInfo {
    name: &'static str,
    sol: bool,
    reads: bool,
    submit: bool,
    tools: Vec<&'static str>,
}

pub(crate) fn run(
    talent_root: &Path,
    apps_root: &Path,
    journal_root: &Path,
    options: &InventoryOptions,
) -> Result<String, String> {
    let mut configs = discovery::discover(talent_root, apps_root)?
        .into_iter()
        .filter(|config| {
            config.metadata.get("type").and_then(Value::as_str) == Some("cogitate")
                && !config.metadata.get("disabled").is_some_and(is_truthy)
        })
        .collect::<Vec<_>>();
    configs.sort_by(|left, right| left.key.cmp(&right.key));
    let templates_dir = talent_root
        .parent()
        .ok_or_else(|| format!("talent root has no parent: {}", talent_root.display()))?
        .join("think/templates");
    let rows = configs
        .iter()
        .map(|config| build_row(config, journal_root, &templates_dir))
        .collect::<Vec<_>>();
    if options.json {
        Ok(render_json(&rows))
    } else {
        Ok(render_table(&rows))
    }
}

fn build_row(config: &TalentConfig, journal_root: &Path, templates_dir: &Path) -> InventoryRow {
    match compose_talent(config, journal_root, templates_dir, None)
        .and_then(|composed| row_from_composed(config, &composed))
    {
        Ok(row) => row,
        Err(error) => error_row(&config.key, error),
    }
}

fn row_from_composed(
    config: &TalentConfig,
    composed: &Map<String, Value>,
) -> Result<InventoryRow, String> {
    let access_tier = composed
        .get("access_tier")
        .and_then(Value::as_str)
        .unwrap_or("normal");
    let capabilities =
        capabilities_for_access_tier(access_tier).map_err(|error| error.to_string())?;
    let finalization = FinalizationConfig {
        diagnostic: finalization_value(composed.get("diagnostic")),
        output_path: finalization_value(composed.get("output_path")),
        schedule: composed.get("schedule").and_then(Value::as_str),
    };
    Ok(InventoryRow {
        name: config.key.clone(),
        schedule: schedule_value(composed.get("schedule")),
        cwd: composed
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("journal")
            .to_owned(),
        write: "ro".to_owned(),
        output: output_value(composed.get("output")),
        access_tier: access_tier.to_owned(),
        finalize: if expects_emit_final(finalization) {
            "emit_final".to_owned()
        } else {
            "FinishTool".to_owned()
        },
        sol: capabilities.sol,
        reads: capabilities.reads,
        submit: capabilities.submit,
        command_examples: scan_command_examples(
            composed
                .get("user_instruction")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        error: None,
    })
}

fn error_row(name: &str, error: String) -> InventoryRow {
    InventoryRow {
        name: name.to_owned(),
        schedule: "-".to_owned(),
        cwd: "-".to_owned(),
        write: "-".to_owned(),
        output: "-".to_owned(),
        access_tier: "-".to_owned(),
        finalize: "-".to_owned(),
        sol: false,
        reads: false,
        submit: false,
        command_examples: Vec::new(),
        error: Some(error),
    }
}

fn schedule_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_owned(),
        Some(Value::String(value)) if value == "none" => "-".to_owned(),
        Some(value) => python_string_any(value),
    }
}

fn output_value(value: Option<&Value>) -> String {
    match value {
        Some(value) if is_truthy(value) => python_string_any(value),
        _ => "-".to_owned(),
    }
}

fn finalization_value(value: Option<&Value>) -> Option<FinalizationValue<'_>> {
    value.map(|value| match value {
        Value::Null => FinalizationValue::Null,
        Value::Bool(value) => FinalizationValue::Boolean(*value),
        Value::String(value) => FinalizationValue::String(value),
        Value::Number(value)
            if value.as_i64() == Some(0)
                || value.as_u64() == Some(0)
                || value.as_f64() == Some(0.0) =>
        {
            FinalizationValue::Falsey
        }
        Value::Number(_) => FinalizationValue::Truthy,
        Value::Array(value) if value.is_empty() => FinalizationValue::Falsey,
        Value::Array(_) => FinalizationValue::Truthy,
        Value::Object(value) if value.is_empty() => FinalizationValue::Falsey,
        Value::Object(_) => FinalizationValue::Truthy,
    })
}

fn scan_command_examples(body: &str) -> Vec<String> {
    let commands = COGITATE_JOURNAL_COMMANDS
        .iter()
        .map(|command| regex::escape(command))
        .collect::<Vec<_>>()
        .join("|");
    let pattern = Regex::new(&format!(
        r"`(?P<cmd>(?:sol\s+call\s+[^\n`]+|journal\s+(?:{commands})\b[^\n`]*))`"
    ))
    .expect("static command example pattern");
    let mut seen = HashSet::new();
    let mut examples = Vec::new();
    for captures in pattern.captures_iter(body) {
        let Some(command) = captures.name("cmd") else {
            continue;
        };
        let normalized = command
            .as_str()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let normalized = normalized.trim_end_matches([',', '.', ';', ':']).to_owned();
        if seen.insert(normalized.clone()) {
            examples.push(normalized);
            if examples.len() == 6 {
                break;
            }
        }
    }
    examples
}

fn tier_table() -> Vec<TierInfo> {
    COGITATE_ACCESS_TIERS
        .into_iter()
        .map(|name| {
            let capabilities = capabilities_for_access_tier(name).expect("known cogitate tier");
            let bound = bound_tools(name, false).expect("known cogitate tier");
            let (_, tools) = bound
                .split_last()
                .expect("each tier has a trailing finalization tool");
            TierInfo {
                name,
                sol: capabilities.sol,
                reads: capabilities.reads,
                submit: capabilities.submit,
                tools: tools.iter().map(|tool| tool.name).collect(),
            }
        })
        .collect()
}

fn tier_summary(reads: bool, submit: bool) -> String {
    let base = if reads { "sol+reads" } else { "sol" };
    if submit {
        format!("{base}+submit")
    } else {
        format!("{base}, no submit")
    }
}

pub(crate) fn tool_surface_line(composed: &Map<String, Value>) -> Result<String, String> {
    let access_tier = composed
        .get("access_tier")
        .and_then(Value::as_str)
        .unwrap_or("normal");
    let capabilities =
        capabilities_for_access_tier(access_tier).map_err(|error| error.to_string())?;
    let bound = bound_tools(access_tier, false).map_err(|error| error.to_string())?;
    let (_, tools) = bound
        .split_last()
        .ok_or_else(|| format!("access tier {access_tier} has no finalization tool"))?;
    let finalization = FinalizationConfig {
        diagnostic: finalization_value(composed.get("diagnostic")),
        output_path: finalization_value(composed.get("output_path")),
        schedule: composed.get("schedule").and_then(Value::as_str),
    };
    let finalize = if expects_emit_final(finalization) {
        "emit_final"
    } else {
        "FinishTool"
    };
    Ok(format!(
        "tools: {}; finalize: {finalize}; tier: {access_tier} ({})",
        tools
            .iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>()
            .join(", "),
        tier_summary(capabilities.reads, capabilities.submit),
    ))
}

fn render_table(rows: &[InventoryRow]) -> String {
    if rows.is_empty() {
        return "No cogitate talents found.\n".to_owned();
    }
    let name_width = rows
        .iter()
        .map(|row| row.name.chars().count())
        .max()
        .expect("rows are nonempty")
        .max(10);
    let mut output = String::new();
    writeln!(
        output,
        "  {:<name_width$}  {:<8}  {:<12}  {:<7}  {:<5}  {:<6}  {:<10}  EXAMPLES",
        "NAME", "SCHEDULE", "TIER", "CWD", "WRITE", "OUTPUT", "FINALIZE"
    )
    .expect("write string");
    output.push('\n');
    for row in rows {
        if let Some(error) = &row.error {
            writeln!(output, "  {:<name_width$}  ERROR: {error}", row.name).expect("write string");
            continue;
        }
        let examples = if row.command_examples.is_empty() {
            "-".to_owned()
        } else {
            row.command_examples.join("; ")
        };
        let examples = examples.chars().take(36).collect::<String>();
        writeln!(
            output,
            "  {:<name_width$}  {:<8}  {:<12}  {:<7}  {:<5}  {:<6}  {:<10}  {examples}",
            row.name, row.schedule, row.access_tier, row.cwd, row.write, row.output, row.finalize,
        )
        .expect("write string");
    }
    output.push('\n');
    output.push_str("tiers:\n");
    for tier in tier_table() {
        writeln!(
            output,
            "  {}: tools={}; {}",
            tier.name,
            tier.tools.join(", "),
            tier_summary(tier.reads, tier.submit)
        )
        .expect("write string");
    }
    output
}

fn render_json(rows: &[InventoryRow]) -> String {
    let mut tiers = Map::new();
    for tier in tier_table() {
        let mut value = Map::new();
        value.insert("sol".to_owned(), Value::Bool(tier.sol));
        value.insert("reads".to_owned(), Value::Bool(tier.reads));
        value.insert("submit".to_owned(), Value::Bool(tier.submit));
        value.insert(
            "tools".to_owned(),
            Value::Array(
                tier.tools
                    .into_iter()
                    .map(|tool| Value::String(tool.to_owned()))
                    .collect(),
            ),
        );
        tiers.insert(tier.name.to_owned(), Value::Object(value));
    }
    let mut root = Map::new();
    root.insert(
        "talents".to_owned(),
        Value::Array(rows.iter().map(row_json).collect()),
    );
    root.insert("tiers".to_owned(), Value::Object(tiers));
    format!(
        "{}\n",
        solstone_core_format::json_compact_ascii(&Value::Object(root))
    )
}

fn row_json(row: &InventoryRow) -> Value {
    let mut value = Map::new();
    value.insert("name".to_owned(), Value::String(row.name.clone()));
    value.insert("schedule".to_owned(), Value::String(row.schedule.clone()));
    value.insert("cwd".to_owned(), Value::String(row.cwd.clone()));
    value.insert("write".to_owned(), Value::String(row.write.clone()));
    value.insert("output".to_owned(), Value::String(row.output.clone()));
    value.insert(
        "access_tier".to_owned(),
        Value::String(row.access_tier.clone()),
    );
    value.insert("finalize".to_owned(), Value::String(row.finalize.clone()));
    value.insert("sol".to_owned(), Value::Bool(row.sol));
    value.insert("reads".to_owned(), Value::Bool(row.reads));
    value.insert("submit".to_owned(), Value::Bool(row.submit));
    value.insert(
        "command_examples".to_owned(),
        Value::Array(
            row.command_examples
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );
    value.insert(
        "error".to_owned(),
        row.error.clone().map_or(Value::Null, Value::String),
    );
    Value::Object(value)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("root");
        for directory in ["talent", "apps", "config", "think/templates"] {
            fs::create_dir_all(root.path().join(directory)).expect("directory");
        }
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"name":"Sol"}}"#,
        )
        .expect("config");
        root
    }

    fn talent(root: &Path, name: &str, metadata: &str, body: &str) {
        fs::write(
            root.join(format!("talent/{name}.md")),
            format!("{{\n{metadata}\n}}\n{body}\n"),
        )
        .expect("talent");
    }

    fn inventory(root: &tempfile::TempDir, json: bool) -> String {
        run(
            &root.path().join("talent"),
            &root.path().join("apps"),
            root.path(),
            &InventoryOptions { json },
        )
        .expect("inventory")
    }

    #[test]
    fn skips_whole_set_validation_and_keeps_later_error_rows() {
        let root = root();
        talent(
            root.path(),
            "alpha_bad",
            r#""type":"cogitate", "access_tier":"bad""#,
            "broken",
        );
        talent(
            root.path(),
            "middle_scheduled",
            r#""type":"cogitate", "schedule":"daily""#,
            "scheduled without priority",
        );
        talent(root.path(), "zeta_good", r#""type":"cogitate""#, "healthy");
        let output = inventory(&root, false);
        let error = output.find("alpha_bad").expect("bad row");
        let scheduled = output.find("  middle_scheduled").expect("scheduled row");
        let healthy = output.find("  zeta_good").expect("healthy row");
        assert!(error < scheduled && scheduled < healthy);
        assert!(output[error..scheduled].contains("ERROR:"));
        assert!(!output[scheduled..healthy].contains("ERROR:"));
    }

    #[test]
    fn scans_examples_with_python_normalization_rules() {
        let body = concat!(
            "`sol call first one` `sol  call\tsecond two.` `sol call first one` ",
            "`journal frobnicate --now` `journal talent inventory;` ",
            "`sol call fourth x` `sol call fifth x` `sol call sixth x` `sol call seventh x`"
        );
        assert_eq!(
            scan_command_examples(body),
            vec![
                "sol call first one",
                "sol call second two",
                "journal talent inventory",
                "sol call fourth x",
                "sol call fifth x",
                "sol call sixth x",
            ]
        );
    }

    #[test]
    fn finalization_value_treats_all_zero_numbers_as_falsey() {
        assert_eq!(
            finalization_value(Some(&json!(0))),
            Some(FinalizationValue::Falsey)
        );
        assert_eq!(
            finalization_value(Some(&json!(0.0))),
            Some(FinalizationValue::Falsey)
        );
        assert_eq!(
            finalization_value(Some(&json!(0.5))),
            Some(FinalizationValue::Truthy)
        );
    }

    #[test]
    fn table_examples_are_truncated_to_36_characters() {
        let row = InventoryRow {
            name: "example".to_owned(),
            schedule: "-".to_owned(),
            cwd: "journal".to_owned(),
            write: "ro".to_owned(),
            output: "-".to_owned(),
            access_tier: "normal".to_owned(),
            finalize: "FinishTool".to_owned(),
            sol: true,
            reads: true,
            submit: false,
            command_examples: vec!["sol call alpha supercalifragilisticexpialidocious".to_owned()],
            error: None,
        };
        let output = render_table(&[row]);
        assert!(output.contains("sol call alpha supercalifragilistice"));
        assert!(!output.contains("sol call alpha supercalifragilisticexpialidocious"));
    }

    #[test]
    fn tier_table_is_derived_from_native_contract_symbols() {
        let expected = COGITATE_ACCESS_TIERS
            .into_iter()
            .map(|name| {
                let capabilities = capabilities_for_access_tier(name).expect("tier");
                let tools = bound_tools(name, false).expect("tools");
                let (_, tools) = tools.split_last().expect("final tool");
                (
                    name,
                    capabilities.sol,
                    capabilities.reads,
                    capabilities.submit,
                    tools.iter().map(|tool| tool.name).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let actual = tier_table()
            .into_iter()
            .map(|tier| (tier.name, tier.sol, tier.reads, tier.submit, tier.tools))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn json_compact_ascii_preserves_inventory_order_and_full_examples() {
        let root = root();
        fs::create_dir_all(root.path().join("facets/work")).expect("facet directory");
        fs::write(
            root.path().join("facets/work/facet.json"),
            r#"{"title":"Mañana","description":"Résumé"}"#,
        )
        .expect("facet");
        let command = "sol call alpha supercalifragilisticexpialidocious";
        talent(
            root.path(),
            "rich",
            r#""type":"cogitate", "schedule":5.0, "output":"Mañana", "diagnostic":{"nested":true}"#,
            &format!("`{command}`\n$facets"),
        );
        // Provenance: this output is emitted by json_compact_ascii, including ASCII escaping.
        let output = inventory(&root, true);
        assert!(output.contains(r#""output": "Ma\u00f1ana""#));
        assert!(output.contains(", \"tiers\": {"));
        let parsed: Value = serde_json::from_str(&output).expect("json");
        let root = parsed.as_object().expect("root object");
        assert_eq!(root.keys().collect::<Vec<_>>(), vec!["talents", "tiers"]);
        let row = root["talents"][0].as_object().expect("row");
        assert_eq!(
            row.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "name",
                "schedule",
                "cwd",
                "write",
                "output",
                "access_tier",
                "finalize",
                "sol",
                "reads",
                "submit",
                "command_examples",
                "error",
            ]
        );
        assert_eq!(row["write"], json!("ro"));
        assert_eq!(row["command_examples"], json!([command]));
        assert!(row["command_examples"][0].as_str().expect("example").len() > 36);
        assert!(row["sol"].is_boolean() && row["reads"].is_boolean() && row["submit"].is_boolean());
        assert!(root["tiers"]["normal"]["tools"].is_array());
    }

    #[test]
    fn empty_table_has_no_tier_section() {
        let root = root();
        assert_eq!(inventory(&root, false), "No cogitate talents found.\n");
        let json = inventory(&root, true);
        let parsed: Value = serde_json::from_str(&json).expect("json");
        assert_eq!(parsed["talents"], json!([]));
    }
}
