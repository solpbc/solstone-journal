// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::model::ElementKind;
use crate::target::model::TargetBundle;
use crate::vocab::{FormatIntent, NumericStyle, ScalarType};
use serde_json::{Map, Value as JsonValue, json};
use std::fs;
use std::path::Path;

pub fn generate_apple_target(
    bundle: &TargetBundle,
    dest_dir: &Path,
    source_locale: &str,
) -> Result<(), Vec<Fault>> {
    let apple_dir = dest_dir.join("apple");
    fs::create_dir_all(&apple_dir).map_err(|_| vec![Fault::catalog("io-error")])?;

    // 1. Generate <bundle>.xcstrings
    let xcstrings_val = generate_xcstrings_json(bundle, source_locale)?;
    let mut xcstrings_str = serde_json::to_string_pretty(&xcstrings_val)
        .map_err(|_| vec![Fault::catalog("io-error")])?;
    xcstrings_str.push('\n');

    let xcstrings_path = apple_dir.join(format!("{}.xcstrings", bundle.id));
    fs::write(&xcstrings_path, xcstrings_str.as_bytes())
        .map_err(|_| vec![Fault::catalog("io-error")])?;

    // 2. Generate <bundle>.swift
    let swift_code = generate_apple_swift(bundle)?;
    let swift_path = apple_dir.join(format!("{}.swift", bundle.id));
    fs::write(&swift_path, swift_code.as_bytes()).map_err(|_| vec![Fault::catalog("io-error")])?;

    Ok(())
}

fn generate_xcstrings_json(
    bundle: &TargetBundle,
    source_locale: &str,
) -> Result<JsonValue, Vec<Fault>> {
    let mut strings_map = Map::new();

    for msg in &bundle.messages {
        match msg.meta.format_intent {
            FormatIntent::Text => {
                let mut loc_map = Map::new();
                for loc in &bundle.locales {
                    if let Some(parsed) = msg.translations.get(&loc.tag) {
                        let tmpl = render_apple_template(&parsed.branches[0].pattern, msg, false)?;
                        loc_map.insert(
                            loc.tag.clone(),
                            json!({
                                "stringUnit": {
                                    "state": "translated",
                                    "value": tmpl
                                }
                            }),
                        );
                    }
                }
                strings_map.insert(
                    msg.resource_key.clone(),
                    json!({
                        "localizations": loc_map
                    }),
                );
            }
            FormatIntent::Cardinal => {
                let sel_name = msg
                    .meta
                    .selector
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or("count");
                let (sel_pos, sel_arg) = msg
                    .meta
                    .arguments
                    .iter()
                    .enumerate()
                    .find(|(_, a)| a.name == sel_name)
                    .ok_or_else(|| vec![Fault::catalog("invalid-metadata")])?;
                let sel_arg_num = sel_pos + 1;
                let is_locale_formatted =
                    sel_arg.numeric_style == Some(NumericStyle::LocaleFormatted);

                let mut loc_map = Map::new();
                for loc in &bundle.locales {
                    if let Some(parsed) = msg.translations.get(&loc.tag) {
                        let mut plural_map = Map::new();
                        for b in &parsed.branches {
                            let cat = b.key.as_deref().unwrap_or("other");
                            let tmpl =
                                render_apple_template(&b.pattern, msg, !is_locale_formatted)?;
                            plural_map.insert(
                                cat.to_string(),
                                json!({
                                    "stringUnit": {
                                        "state": "translated",
                                        "value": tmpl
                                    }
                                }),
                            );
                        }
                        if is_locale_formatted {
                            loc_map.insert(
                                loc.tag.clone(),
                                json!({
                                    "variations": {
                                        "plural": plural_map
                                    }
                                }),
                            );
                        } else {
                            loc_map.insert(
                                loc.tag.clone(),
                                json!({
                                    "stringUnit": {
                                        "state": "translated",
                                        "value": "%#@quantity@"
                                    },
                                    "substitutions": {
                                        "quantity": {
                                            "argNum": sel_arg_num,
                                            "formatSpecifier": "lld",
                                            "variations": {
                                                "plural": plural_map
                                            }
                                        }
                                    }
                                }),
                            );
                        }
                    }
                }
                strings_map.insert(
                    msg.resource_key.clone(),
                    json!({
                        "localizations": loc_map
                    }),
                );
            }
            FormatIntent::Enum => {
                let sel = msg.meta.selector.as_ref().unwrap();
                for val in &sel.enum_values {
                    let key =
                        crate::target::model::enum_variant_resource_key(&msg.resource_key, val);
                    let mut loc_map = Map::new();
                    for loc in &bundle.locales {
                        if let Some(parsed) = msg.translations.get(&loc.tag)
                            && let Some(b) = parsed
                                .branches
                                .iter()
                                .find(|b| b.key.as_deref() == Some(val.as_str()))
                        {
                            let tmpl = render_apple_template(&b.pattern, msg, false)?;
                            loc_map.insert(
                                loc.tag.clone(),
                                json!({
                                    "stringUnit": {
                                        "state": "translated",
                                        "value": tmpl
                                    }
                                }),
                            );
                        }
                    }
                    strings_map.insert(
                        key,
                        json!({
                            "localizations": loc_map
                        }),
                    );
                }
            }
        }
    }

    Ok(json!({
        "sourceLanguage": source_locale,
        "version": "1.0",
        "strings": strings_map
    }))
}

pub fn render_apple_template(
    elements: &[crate::model::PatternElement],
    msg: &crate::target::model::TargetMessage,
    is_ungrouped_cardinal: bool,
) -> Result<String, Vec<Fault>> {
    let mut out = String::new();
    for el in elements {
        match el.kind {
            ElementKind::Text => {
                for c in el.value.chars() {
                    if c == '%' {
                        out.push_str("%%");
                    } else {
                        out.push(c);
                    }
                }
            }
            ElementKind::Slot => {
                let slot = msg
                    .meta
                    .protected_slots
                    .iter()
                    .find(|s| s.name == el.value)
                    .ok_or_else(|| vec![Fault::catalog("io-error")])?;
                for c in slot.value.chars() {
                    if c == '%' {
                        out.push_str("%%");
                    } else {
                        out.push(c);
                    }
                }
            }
            ElementKind::Arg => {
                let (pos, arg_spec) = msg
                    .meta
                    .arguments
                    .iter()
                    .enumerate()
                    .find(|(_, a)| a.name == el.value)
                    .ok_or_else(|| vec![Fault::catalog("io-error")])?;
                let sel_name = msg
                    .meta
                    .selector
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or("count");
                if is_ungrouped_cardinal && arg_spec.name == sel_name {
                    let display_pos = msg.meta.arguments.len() + 1;
                    out.push_str(&format!("%{}$@", display_pos));
                } else {
                    let idx = pos + 1;
                    match arg_spec.arg_type {
                        ScalarType::Integer | ScalarType::Cardinal => {
                            if arg_spec.numeric_style == Some(NumericStyle::LocaleFormatted) {
                                out.push_str(&format!("%{}$lld", idx));
                            } else {
                                out.push_str(&format!("%{}$@", idx));
                            }
                        }
                        ScalarType::String | ScalarType::Boolean => {
                            out.push_str(&format!("%{}$@", idx));
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

fn generate_apple_swift(bundle: &TargetBundle) -> Result<String, Vec<Fault>> {
    let mut s = String::new();
    s.push_str("// SPDX-License-Identifier: AGPL-3.0-only\n");
    s.push_str("// Copyright (c) 2026 sol pbc\n\n");
    s.push_str("import Foundation\n\n");
    s.push_str("public enum CatalogSelect {\n");
    s.push_str(
        "    public static func resolveKey(messageId: String, enumArg: String?) -> String {\n",
    );
    s.push_str("        switch messageId {\n");
    for msg in &bundle.messages {
        if msg.meta.format_intent == FormatIntent::Enum {
            let sel = msg.meta.selector.as_ref().unwrap();
            let def = sel
                .enum_default
                .as_deref()
                .ok_or_else(|| vec![Fault::new("enum-variant", "*", &msg.id)])?;
            let def_key = crate::target::model::enum_variant_resource_key(&msg.resource_key, def);
            s.push_str(&format!("        case \"{}\":\n", msg.id));
            s.push_str("            guard let val = enumArg else { return ");
            s.push_str(&format!("\"{}\"", def_key));
            s.push_str(" }\n");
            s.push_str("            switch val {\n");
            for v in &sel.enum_values {
                let v_key = crate::target::model::enum_variant_resource_key(&msg.resource_key, v);
                s.push_str(&format!(
                    "            case \"{}\": return \"{}\"\n",
                    v, v_key
                ));
            }
            s.push_str(&format!("            default: return \"{}\"\n", def_key));
            s.push_str("            }\n");
        } else {
            s.push_str(&format!(
                "        case \"{}\": return \"{}\"\n",
                msg.id, msg.resource_key
            ));
        }
    }
    s.push_str("        default: return messageId\n");
    s.push_str("        }\n");
    s.push_str("    }\n\n");

    s.push_str("    public static func selectCategoryEn(count: Int64) -> String {\n");
    s.push_str("        if count == 1 || count == -1 {\n");
    s.push_str("            return \"one\"\n");
    s.push_str("        }\n");
    s.push_str("        return \"other\"\n");
    s.push_str("    }\n\n");

    s.push_str("    public static func selectCategoryAr(count: Int64) -> String {\n");
    s.push_str("        let v = count < 0 ? (count == Int64.min ? Int64.max : -count) : count\n");
    s.push_str("        if v == 0 { return \"zero\" }\n");
    s.push_str("        if v == 1 { return \"one\" }\n");
    s.push_str("        if v == 2 { return \"two\" }\n");
    s.push_str("        if v >= 3 && v <= 10 { return \"few\" }\n");
    s.push_str("        if v >= 11 && v <= 99 { return \"many\" }\n");
    s.push_str("        return \"other\"\n");
    s.push_str("    }\n\n");

    s.push_str("    public static func formatIntegerUngrouped(_ val: Int64) -> String {\n");
    s.push_str("        return String(val)\n");
    s.push_str("    }\n\n");

    s.push_str(
        "    public static func formatIntegerLocale(_ val: Int64, locale: String) -> String {\n",
    );
    s.push_str("        let formatter = NumberFormatter()\n");
    s.push_str("        formatter.numberStyle = .decimal\n");
    s.push_str("        formatter.locale = Locale(identifier: locale)\n");
    s.push_str("        return formatter.string(from: NSNumber(value: val)) ?? String(val)\n");
    s.push_str("    }\n");
    s.push_str("}\n");
    Ok(s)
}
