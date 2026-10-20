// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::target::cases::TestCase;
use crate::target::oracle;
use serde_json::Value as JsonValue;
use std::fs;
use std::path::Path;

pub fn read_xcstrings(apple_dir: &Path, case: &TestCase) -> Result<String, String> {
    let file_path = apple_dir.join(format!("{}.xcstrings", case.bundle));
    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;
    let json: JsonValue = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", file_path.display(), e))?;

    let strings = json
        .get("strings")
        .and_then(|v| v.as_object())
        .ok_or("Missing strings object in xcstrings")?;

    // Check if enum or standard key
    let target_key = if let Some(kind_val) = case.inputs.get("kind").and_then(|v| v.as_str()) {
        let key_variant =
            crate::target::model::enum_variant_resource_key(&case.resource_key, kind_val);
        if strings.contains_key(&key_variant) {
            key_variant
        } else {
            // Check enum default (e.g. box)
            let def_key =
                crate::target::model::enum_variant_resource_key(&case.resource_key, "box");
            if strings.contains_key(&def_key) {
                def_key
            } else {
                case.resource_key.clone()
            }
        }
    } else {
        case.resource_key.clone()
    };

    let entry = strings
        .get(&target_key)
        .ok_or_else(|| format!("Key {} not found in xcstrings", target_key))?;
    let loc_entry = entry
        .get("localizations")
        .and_then(|v| v.get(&case.locale))
        .ok_or_else(|| format!("Locale {} not found for {}", case.locale, target_key))?;

    let (template, is_split) =
        if let Some(subs) = loc_entry.get("substitutions").and_then(|v| v.as_object()) {
            let quantity = subs
                .get("quantity")
                .ok_or("Missing quantity in substitutions")?;
            let spec = quantity
                .get("formatSpecifier")
                .and_then(|v| v.as_str())
                .ok_or("Missing formatSpecifier in quantity")?;
            if spec != "lld" {
                return Err(format!("Expected formatSpecifier 'lld', got '{}'", spec));
            }
            let arg_num = quantity
                .get("argNum")
                .and_then(|v| v.as_i64())
                .ok_or("Missing argNum in quantity")?;
            if arg_num < 1 {
                return Err(format!("Expected argNum >= 1, got {}", arg_num));
            }

            let variations = quantity
                .get("variations")
                .and_then(|v| v.get("plural"))
                .ok_or("Missing plural variations in quantity")?;
            let count_val = case
                .inputs
                .get("count")
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            let cardinal_rule = if case.locale == "ar" || case.locale == "en-x-plural" {
                "ar"
            } else {
                "en"
            };
            let cat = oracle::eval_fluent_category(cardinal_rule, count_val)
                .map_err(|f| format!("oracle fault: {:?}", f))?;

            let val_str = if let Some(val) = variations
                .get(cat)
                .and_then(|u| u.get("stringUnit"))
                .and_then(|u| u.get("value"))
                .and_then(|v| v.as_str())
            {
                val.to_string()
            } else {
                variations
                    .get("other")
                    .and_then(|u| u.get("stringUnit"))
                    .and_then(|u| u.get("value"))
                    .and_then(|v| v.as_str())
                    .ok_or("No matching variation or other")?
                    .to_string()
            };
            (val_str, true)
        } else if let Some(variations) = loc_entry.get("variations").and_then(|v| v.get("plural")) {
            let count_val = case
                .inputs
                .get("count")
                .and_then(|v| v.as_i64())
                .unwrap_or(1);
            let cardinal_rule = if case.locale == "ar" || case.locale == "en-x-plural" {
                "ar"
            } else {
                "en"
            };
            let cat = oracle::eval_fluent_category(cardinal_rule, count_val)
                .map_err(|f| format!("oracle fault: {:?}", f))?;

            let val_str = if let Some(val) = variations
                .get(cat)
                .and_then(|u| u.get("stringUnit"))
                .and_then(|u| u.get("value"))
                .and_then(|v| v.as_str())
            {
                val.to_string()
            } else {
                variations
                    .get("other")
                    .and_then(|u| u.get("stringUnit"))
                    .and_then(|u| u.get("value"))
                    .and_then(|v| v.as_str())
                    .ok_or("No matching variation or other")?
                    .to_string()
            };
            (val_str, false)
        } else if let Some(val) = loc_entry
            .get("stringUnit")
            .and_then(|u| u.get("value"))
            .and_then(|v| v.as_str())
        {
            (val.to_string(), false)
        } else {
            return Err("No stringUnit, substitutions, or variations in xcstrings".to_string());
        };

    let count_val = case
        .inputs
        .get("count")
        .and_then(|v| v.as_i64())
        .or_else(|| case.inputs.values().find_map(|v| v.as_i64()))
        .unwrap_or(1);
    let ungrouped = oracle::format_integer_ungrouped(count_val);
    let display_pos = case.inputs.len() + 1;
    let placeholder = format!("%{}$@", display_pos);

    let replaced = if is_split && template.contains(&placeholder) {
        template.replace(&placeholder, &ungrouped)
    } else {
        template
    };

    render_apple_placeholders(&replaced, &case.inputs, &case.adapter_call)
}

pub fn read_android_xml(android_dir: &Path, case: &TestCase) -> Result<String, String> {
    let qual = case
        .folder
        .clone()
        .or_else(|| {
            let tag = &case.locale;
            let bytes = tag.as_bytes();
            if (bytes.len() == 2 || bytes.len() == 3)
                && bytes.iter().all(|b| b.is_ascii_lowercase())
            {
                Some(tag.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| format!("No android qualifier for locale {}", case.locale))?;
    let file_path = android_dir
        .join(&case.bundle)
        .join(format!("values-{}", qual))
        .join("strings.xml");
    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;

    let target_key = if let Some(kind_val) = case.inputs.get("kind").and_then(|v| v.as_str()) {
        let key_variant =
            crate::target::model::enum_variant_resource_key(&case.resource_key, kind_val);
        if content.contains(&format!("name=\"{}\"", key_variant)) {
            key_variant
        } else {
            crate::target::model::enum_variant_resource_key(&case.resource_key, "box")
        }
    } else {
        case.resource_key.clone()
    };

    let template = if let Some(pos) = content.find(&format!("<string name=\"{}\">", target_key)) {
        let rest = &content[pos + format!("<string name=\"{}\">", target_key).len()..];
        let end = rest
            .find("</string>")
            .ok_or("Malformed XML string closing")?;
        let raw = &rest[..end];
        clean_android_raw(raw)?
    } else if let Some(pos) = content.find(&format!("<plurals name=\"{}\">", target_key)) {
        let rest = &content[pos..];
        let end_plurals = rest
            .find("</plurals>")
            .ok_or("Malformed XML plurals closing")?;
        let plurals_block = &rest[..end_plurals];

        let count_val = case
            .inputs
            .get("count")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let cardinal_rule = if case.locale == "ar" || case.locale == "en-x-plural" {
            "ar"
        } else {
            "en"
        };
        let cat = oracle::eval_fluent_category(cardinal_rule, count_val)
            .map_err(|f| format!("oracle fault: {:?}", f))?;

        let tag = format!("<item quantity=\"{}\">", cat);
        let raw = if let Some(item_pos) = plurals_block.find(&tag) {
            let item_rest = &plurals_block[item_pos + tag.len()..];
            let item_end = item_rest.find("</item>").ok_or("Malformed item closing")?;
            &item_rest[..item_end]
        } else {
            let other_tag = "<item quantity=\"other\">";
            let item_pos = plurals_block.find(other_tag).ok_or("Missing other item")?;
            let item_rest = &plurals_block[item_pos + other_tag.len()..];
            let item_end = item_rest.find("</item>").ok_or("Malformed item closing")?;
            &item_rest[..item_end]
        };
        clean_android_raw(raw)?
    } else {
        return Err(format!("Key {} not found in XML", target_key));
    };

    render_placeholders(&template, &case.inputs)
}

fn clean_android_raw(raw: &str) -> Result<String, String> {
    let unquoted = if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };

    let xml = unquoted
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let mut chars = xml.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next().ok_or("Incomplete Android escape")? {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'u' => {
                let digits: String = chars.by_ref().take(4).collect();
                if digits.len() != 4 {
                    return Err("Incomplete Android Unicode escape".into());
                }
                let value = u32::from_str_radix(&digits, 16)
                    .map_err(|_| "Invalid Android Unicode escape")?;
                out.push(char::from_u32(value).ok_or("Invalid Android scalar")?);
            }
            _ => return Err("Unknown Android escape".into()),
        }
    }
    Ok(out)
}

pub fn read_webextension_json(webextension_dir: &Path, case: &TestCase) -> Result<String, String> {
    let folder = case.folder.as_deref().unwrap_or("en");
    let file_path = webextension_dir
        .join(&case.bundle)
        .join("_locales")
        .join(folder)
        .join("messages.json");
    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;
    let json: JsonValue = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", file_path.display(), e))?;

    let entry = json
        .get(&case.resource_key)
        .ok_or_else(|| format!("Key {} not found in messages.json", case.resource_key))?;
    let raw_msg = entry
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or("Missing message string")?;

    if raw_msg != "$SAFE$" {
        return Err(format!(
            "Expected message == \"$SAFE$\", got \"{}\"",
            raw_msg
        ));
    }

    let safe_content = entry
        .get("placeholders")
        .and_then(|v| v.get("safe"))
        .and_then(|v| v.get("content"))
        .and_then(|v| v.as_str())
        .ok_or("Missing placeholders.safe.content")?;

    Ok(safe_content.to_string())
}

pub fn read_gettext_po(gettext_dir: &Path, case: &TestCase) -> Result<String, String> {
    let file_path = gettext_dir
        .join(&case.bundle)
        .join(format!("{}.po", case.locale));
    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;

    let target_msgctxt = if let Some(kind_val) = case.inputs.get("kind").and_then(|v| v.as_str()) {
        let ctxt = format!("{}.{}", case.message_id, kind_val);
        if content.contains(&format!("msgctxt \"{}\"", ctxt)) {
            ctxt
        } else {
            format!("{}.box", case.message_id)
        }
    } else {
        case.message_id.clone()
    };

    let ctxt_pattern = format!("msgctxt \"{}\"", target_msgctxt);
    let pos = content
        .find(&ctxt_pattern)
        .ok_or_else(|| format!("msgctxt {} not found in PO", target_msgctxt))?;
    let rest = &content[pos..];
    let entry_end = rest[ctxt_pattern.len()..]
        .find("\n\n")
        .map(|i| ctxt_pattern.len() + i)
        .unwrap_or(rest.len());
    let entry_str = &rest[..entry_end];

    let template = if let Some(plural_pos) = entry_str.find("msgid_plural") {
        let after_plural = &entry_str[plural_pos..];
        let count_val = case
            .inputs
            .get("count")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let cardinal_rule = if case.locale == "ar" || case.locale == "en-x-plural" {
            "ar"
        } else {
            "en"
        };
        let cat = oracle::eval_fluent_category(cardinal_rule, count_val)
            .map_err(|f| format!("oracle fault: {:?}", f))?;

        let idx = match (cardinal_rule, cat) {
            ("en", "one") => 0,
            ("en", _) => 1,
            ("ar", "zero") => 0,
            ("ar", "one") => 1,
            ("ar", "two") => 2,
            ("ar", "few") => 3,
            ("ar", "many") => 4,
            ("ar", _) => 5,
            _ => 0,
        };

        let target_str_tag = format!("msgstr[{}] \"", idx);
        let str_pos = after_plural
            .find(&target_str_tag)
            .ok_or_else(|| format!("msgstr[{}] not found", idx))?;
        let after = &after_plural[str_pos + target_str_tag.len()..];
        let end = find_closing_po_quote(after).ok_or("Malformed msgstr[i] closing")?;
        clean_po_string(&after[..end])
    } else if let Some(str_pos) = entry_str.find("msgstr \"") {
        let after = &entry_str[str_pos + 8..];
        let end = find_closing_po_quote(after).ok_or("Malformed msgstr closing")?;
        clean_po_string(&after[..end])
    } else {
        return Err("Neither msgstr nor msgid_plural found".to_string());
    };

    render_placeholders(&template, &case.inputs)
}

fn find_closing_po_quote(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'"' {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

fn clean_po_string(s: &str) -> String {
    s.replace("\\\"", "\"")
        .replace("\\\\", "\\")
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
}

fn render_placeholders(
    template: &str,
    inputs: &serde_json::Map<String, JsonValue>,
) -> Result<String, String> {
    let mut result = template.replace("%%", "\x00");

    let mut sorted_inputs: Vec<_> = inputs.iter().collect();
    sorted_inputs.sort_by(|a, b| a.0.cmp(b.0));

    // Map positional index 1, 2, ...
    for (i, (_name, val)) in sorted_inputs.iter().enumerate() {
        let idx = i + 1;
        let val_str = if let Some(s) = val.as_str() {
            s.to_string()
        } else if let Some(b) = val.as_bool() {
            if b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        } else if let Some(n) = val.as_i64() {
            oracle::format_integer_ungrouped(n)
        } else {
            val.to_string()
        };

        result = result.replace(&format!("%{}$@", idx), &val_str);
        result = result.replace(&format!("%{}$lld", idx), &val_str);
        result = result.replace(&format!("%{}$d", idx), &val_str);
        result = result.replace(&format!("%{}$s", idx), &val_str);
    }

    // Unescape %% -> %
    result = result.replace('\x00', "%");

    Ok(result)
}

fn render_apple_placeholders(
    template: &str,
    inputs: &serde_json::Map<String, JsonValue>,
    adapter_call: &str,
) -> Result<String, String> {
    if let Some(pos) = adapter_call.find("arguments: ") {
        let rest = &adapter_call[pos + "arguments: ".len()..];
        if let Some(first_comma) = rest.find(',') {
            let args_part = &rest[first_comma + 1..];
            let trimmed = args_part.trim();
            let trimmed = if let Some(stripped) = trimmed.strip_suffix(')') {
                stripped.trim()
            } else {
                trimmed
            };
            let mut result = template.replace("%%", "\x00");
            for piece in trimmed.split(", ") {
                let piece = piece.trim();
                if let Some(colon) = piece.find(':')
                    && let Ok(idx) = piece[..colon].trim().parse::<usize>()
                {
                    let val_part = &piece[colon + 1..];
                    if let Some(start_paren) = val_part.find('(')
                        && let Some(end_paren) = val_part.rfind(')')
                    {
                        let inner = &val_part[start_paren + 1..end_paren];
                        let clean_val =
                            if inner.starts_with('"') && inner.ends_with('"') && inner.len() >= 2 {
                                &inner[1..inner.len() - 1]
                            } else {
                                inner
                            };
                        result = result.replace(&format!("%{}$@", idx), clean_val);
                        result = result.replace(&format!("%{}$lld", idx), clean_val);
                        result = result.replace(&format!("%{}$d", idx), clean_val);
                        result = result.replace(&format!("%{}$s", idx), clean_val);
                    }
                }
            }
            result = result.replace('\x00', "%");
            return Ok(result);
        }
    }
    render_placeholders(template, inputs)
}
