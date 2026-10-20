// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::model::ElementKind;
use crate::target::model::{TargetBundle, TargetMessage};
use crate::vocab::{FormatIntent, NumericStyle, ScalarType};

pub fn generate_bundle_ftl(bundle: &TargetBundle, locale: &str) -> Result<String, Fault> {
    let mut out = String::new();

    for msg in &bundle.messages {
        let parsed = match msg.translations.get(locale) {
            Some(p) => p,
            None => return Err(Fault::new("missing-translation", locale, &msg.id)),
        };

        match msg.meta.format_intent {
            FormatIntent::Text => {
                let pattern_str = render_ftl_pattern(&parsed.branches[0].pattern, msg, true)?;
                out.push_str(&format!("{} = {}\n\n", msg.resource_key, pattern_str));
            }
            FormatIntent::Cardinal => {
                let sel_var = msg
                    .meta
                    .selector
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or("count");

                out.push_str(&format!("{} = {{ ${} ->\n", msg.resource_key, sel_var));
                for b in &parsed.branches {
                    let key = b.key.as_deref().unwrap_or("other");
                    let prefix = if b.default { "   *" } else { "    " };
                    let pattern_str = render_ftl_pattern(&b.pattern, msg, false)?;
                    out.push_str(&format!("{}[{}] {}\n", prefix, key, pattern_str));
                }
                out.push_str("}\n\n");
            }
            FormatIntent::Enum => {
                for b in &parsed.branches {
                    let variant = b.key.as_deref().unwrap_or("default");
                    let var_key =
                        crate::target::model::enum_variant_resource_key(&msg.resource_key, variant);
                    let pattern_str = render_ftl_pattern(&b.pattern, msg, true)?;
                    out.push_str(&format!("{} = {}\n\n", var_key, pattern_str));
                }
            }
        }
    }

    Ok(out)
}

fn render_ftl_pattern(
    elements: &[crate::model::PatternElement],
    msg: &TargetMessage,
    is_start: bool,
) -> Result<String, Fault> {
    let mut out = String::new();

    for (idx, el) in elements.iter().enumerate() {
        match el.kind {
            ElementKind::Text => {
                let text_at_start = is_start && idx == 0;
                out.push_str(&escape_ftl_text(&el.value, text_at_start));
            }
            ElementKind::Slot => {
                let slot = msg
                    .meta
                    .protected_slots
                    .iter()
                    .find(|s| s.name == el.value)
                    .ok_or_else(|| Fault::catalog("io-error"))?;
                let escaped = escape_ftl_string_literal(&slot.value);
                out.push_str(&format!("{{ \"{}\" }}", escaped));
            }
            ElementKind::Arg => {
                let arg_spec = msg.meta.arguments.iter().find(|a| a.name == el.value);
                if arg_spec.and_then(|a| a.numeric_style) == Some(NumericStyle::LocaleFormatted) {
                    out.push_str(&format!("{{ NUMBER(${}) }}", el.value));
                } else if arg_spec
                    .map(|a| {
                        a.arg_type == ScalarType::Integer || a.arg_type == ScalarType::Cardinal
                    })
                    .unwrap_or(false)
                {
                    out.push_str(&format!("{{ NUMBER(${}, useGrouping: 0) }}", el.value));
                } else {
                    out.push_str(&format!("{{ ${} }}", el.value));
                }
            }
        }
    }

    Ok(out)
}

fn escape_ftl_text(text: &str, is_at_start: bool) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();

    if is_at_start {
        let mut leading_ws = String::new();
        while let Some(&c) = chars.peek() {
            if c == ' ' || c == '\t' {
                leading_ws.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if !leading_ws.is_empty() {
            out.push_str(&format!("{{ \"{}\" }}", leading_ws));
        }
    }

    for c in chars {
        match c {
            '{' => out.push_str("{ \"{\" }"),
            '}' => out.push_str("{ \"}\" }"),
            '\n' => out.push_str("{ \"\\u000a\" }"),
            '\r' => out.push_str("{ \"\\u000d\" }"),
            _ => out.push(c),
        }
    }

    out
}

fn escape_ftl_string_literal(val: &str) -> String {
    let mut out = String::with_capacity(val.len() + 8);
    for c in val.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\u000a"),
            '\r' => out.push_str("\\u000d"),
            '\t' => out.push_str("\\u0009"),
            _ => out.push(c),
        }
    }
    out
}
