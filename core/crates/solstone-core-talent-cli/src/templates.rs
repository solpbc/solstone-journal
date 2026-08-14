// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use solstone_core_journal_config::{plain_defaults, read_journal_config};

use solstone_core_talent_config::read_frontmatter;

pub(crate) fn safe_substitute(text: &str, vars: &BTreeMap<String, String>) -> String {
    let mut output = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            let character = text[index..].chars().next().expect("valid UTF-8");
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        let start = index;
        index += 1;
        if index == bytes.len() {
            output.push('$');
            break;
        }
        if bytes[index] == b'$' {
            output.push('$');
            index += 1;
            continue;
        }
        if bytes[index] == b'{' {
            let name_start = index + 1;
            let Some(offset) = bytes[name_start..].iter().position(|byte| *byte == b'}') else {
                output.push('$');
                continue;
            };
            let end = name_start + offset;
            let name = &text[name_start..end];
            if valid_identifier(name) {
                if let Some(value) = vars.get(name) {
                    output.push_str(value);
                } else {
                    output.push_str(&text[start..=end]);
                }
                index = end + 1;
                continue;
            }
            output.push('$');
            continue;
        }
        if !identifier_start(bytes[index]) {
            output.push('$');
            continue;
        }
        let name_start = index;
        while index < bytes.len() && identifier_continue(bytes[index]) {
            index += 1;
        }
        let name = &text[name_start..index];
        if valid_identifier(name) {
            if let Some(value) = vars.get(name) {
                output.push_str(value);
            } else {
                output.push_str(&text[start..index]);
            }
        } else {
            output.push('$');
        }
    }
    output
}

pub(crate) fn compose_prompt_body(
    body: &str,
    journal_root: &Path,
    templates_dir: &Path,
    context: &BTreeMap<String, String>,
) -> Result<String, String> {
    let vars = match template_vars(journal_root, templates_dir, context) {
        Ok(vars) => vars,
        Err(error) if error.starts_with("failed to read journal config:") => return Err(error),
        Err(_) => return Ok(body.to_owned()),
    };
    let templates = match load_raw_templates(templates_dir) {
        Ok(templates) => templates,
        Err(_) => return Ok(body.to_owned()),
    };
    let rendered_templates = templates
        .into_iter()
        .map(|(name, content)| (name, safe_substitute(&content, &vars)))
        .collect::<BTreeMap<_, _>>();
    let mut vars = vars;
    vars.extend(rendered_templates);
    Ok(safe_substitute(body, &vars))
}

fn template_vars(
    journal_root: &Path,
    _templates_dir: &Path,
    context: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let read = read_journal_config(journal_root)
        .map_err(|error| format!("failed to read journal config: {error}"))?;
    let config = read.config.unwrap_or_else(plain_defaults);
    let identity = config.get("identity").and_then(Value::as_object);
    let mut vars = identity.map_or_else(BTreeMap::new, flatten_identity);
    vars.insert("now".to_owned(), format_current_datetime());
    let agent_name = config
        .get("agent")
        .and_then(Value::as_object)
        .and_then(|agent| agent.get("name"))
        .and_then(python_string)
        .unwrap_or_else(|| "sol".to_owned());
    vars.insert("agent_name".to_owned(), agent_name.clone());
    vars.insert("Agent_name".to_owned(), python_capitalize(&agent_name));
    for (key, value) in context {
        vars.insert(key.clone(), value.clone());
        vars.insert(python_capitalize(key), python_capitalize(value));
    }
    for (key, value) in load_identity_markdown_vars(journal_root) {
        vars.entry(key).or_insert(value);
    }
    Ok(vars)
}

fn flatten_identity(identity: &Map<String, Value>) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for (key, value) in identity {
        if let Some(object) = value.as_object() {
            for (subkey, subvalue) in object {
                let name = format!("{key}_{subkey}");
                let value = python_string_any(subvalue);
                vars.insert(name.clone(), value.clone());
                vars.insert(python_capitalize(&name), python_capitalize(&value));
            }
        } else if let Some(value) = python_string(value) {
            vars.insert(key.clone(), value.clone());
            vars.insert(python_capitalize(key), python_capitalize(&value));
        }
    }
    vars
}

fn load_identity_markdown_vars(journal_root: &Path) -> BTreeMap<String, String> {
    let identity_dir = journal_root.join("identity");
    let Ok(entries) = fs::read_dir(identity_dir) else {
        return BTreeMap::new();
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("md")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?.to_owned();
            let content = fs::read_to_string(&path).ok()?;
            Some((format!("identity_{stem}"), content.trim().to_owned()))
        })
        .collect()
}

pub(crate) fn load_raw_templates(templates_dir: &Path) -> Result<BTreeMap<String, String>, String> {
    if !templates_dir.is_dir() {
        return Ok(BTreeMap::new());
    }
    let mut paths = fs::read_dir(templates_dir)
        .map_err(|error| format!("failed to read {}: {error}", templates_dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read {}: {error}", templates_dir.display()))?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("md")
        })
        .collect::<Vec<_>>();
    paths.sort();
    let mut templates = BTreeMap::new();
    for path in paths {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("template filename has no UTF-8 stem: {}", path.display()))?;
        let parsed = read_frontmatter(&path)?;
        templates.insert(stem.to_owned(), parsed.body);
    }
    Ok(templates)
}

fn format_current_datetime() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_date(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let (hour12, meridiem) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    let weekdays = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ];
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    format!(
        "{}, {} {} , {} at {:02}:{:02} {} UTC",
        weekdays[days.rem_euclid(7) as usize],
        months[(month - 1) as usize],
        day,
        year,
        hour12,
        minute,
        meridiem
    )
    .replace(" ,", ",")
}

fn civil_date(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

fn python_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(if *value { "True" } else { "False" }.to_owned()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

pub(crate) fn python_string_any(value: &Value) -> String {
    python_string(value).unwrap_or_else(|| match value {
        Value::Null => "None".to_owned(),
        _ => value.to_string(),
    })
}

fn python_capitalize(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    first
        .to_uppercase()
        .chain(characters.flat_map(char::to_lowercase))
        .collect()
}

fn valid_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && identifier_start(bytes[0])
        && bytes[1..].iter().all(|byte| identifier_continue(*byte))
}

fn identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn identifier_continue(byte: u8) -> bool {
    identifier_start(byte) || byte.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn safe_substitute_matches_template_escaping_and_unknowns() {
        assert_eq!(
            safe_substitute(
                "$$ $name ${name} $missing ${missing} $ ${broken",
                &vars(&[("name", "Sol")])
            ),
            "$ Sol Sol $missing ${missing} $ ${broken"
        );
    }

    #[test]
    fn safe_substitute_preserves_invalid_bare_identifiers() {
        assert_eq!(
            safe_substitute("$1abc $_ok", &vars(&[("_ok", "resolved")])),
            "$1abc resolved"
        );
    }

    #[test]
    fn prompt_composition_pre_substitutes_templates_and_adds_case_variants() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("identity")).expect("identity");
        fs::create_dir_all(root.path().join("templates")).expect("templates");
        fs::create_dir_all(root.path().join("config")).expect("config");
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"identity":{"name":"JER","pronouns":{"subject":"they"}}}"#,
        )
        .expect("config");
        fs::write(root.path().join("identity/partner.md"), "  Friend  \n")
            .expect("identity markdown");
        fs::write(
            root.path().join("templates/greeting.md"),
            "Hello $Name / $pronouns_subject",
        )
        .expect("template");
        let context = vars(&[("facet", "work"), ("name", "context")]);
        let result = compose_prompt_body(
            "$greeting $identity_partner $facet/$Facet $name/$Name",
            root.path(),
            &root.path().join("templates"),
            &context,
        )
        .expect("compose");
        assert_eq!(
            result,
            "Hello Context / they Friend work/Work context/Context"
        );
    }

    #[test]
    fn templates_render_from_a_shared_base_snapshot_before_body_substitution() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("templates")).expect("templates");
        fs::write(root.path().join("templates/first.md"), "rendered").expect("first template");
        fs::write(root.path().join("templates/second.md"), "$first").expect("second template");
        let result = compose_prompt_body(
            "$second $first",
            root.path(),
            &root.path().join("templates"),
            &BTreeMap::new(),
        )
        .expect("compose");
        assert_eq!(result, "$first rendered");
    }

    #[test]
    fn corrupt_journal_config_propagates() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("config")).expect("config");
        fs::write(root.path().join("config/journal.json"), "{").expect("config");
        let error = compose_prompt_body(
            "prompt",
            root.path(),
            &root.path().join("templates"),
            &BTreeMap::new(),
        )
        .expect_err("corrupt config must propagate");
        assert!(error.starts_with("failed to read journal config:"));
    }
}
