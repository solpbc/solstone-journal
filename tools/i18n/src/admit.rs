// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::metadata::MessageMetadata;
use crate::model::{Branch, ElementKind, PatternElement};
use crate::vocab::FormatIntent;
use fluent_syntax::ast;
use fluent_syntax::parser;
use std::collections::{HashMap, HashSet};

pub const CLDR_CATEGORIES: &[&str] = &["zero", "one", "two", "few", "many", "other"];

#[derive(Clone, Debug)]
pub struct ParsedCatalogFile {
    pub messages: HashMap<String, ParsedMessage>,
}

#[derive(Clone, Debug)]
pub struct ParsedMessage {
    pub id: String,
    pub branches: Vec<Branch>,
    pub selector_var: Option<String>,
}

pub fn dotted_to_fluent_id(dotted: &str) -> String {
    dotted.replace('.', "__DOT__")
}

pub fn fluent_to_dotted_id(fluent: &str) -> String {
    fluent.replace("__DOT__", ".")
}

fn preprocess_ftl_id_dots(input: &str) -> (String, HashMap<String, String>) {
    let mut out = String::with_capacity(input.len());
    let mut rewritten = HashMap::new();
    for line in input.split_inclusive('\n') {
        let trimmed_start = line.trim_start();
        if trimmed_start.len() == line.len()
            && !line.starts_with('#')
            && !line.starts_with('-')
            && let Some((id_part, rest)) = line.split_once('=')
        {
            let id_clean = id_part.trim();
            if !id_clean.is_empty()
                && id_clean
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic())
                && id_clean
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
                && id_clean.contains('.')
                && !id_clean.contains("__DOT__")
            {
                let transformed_id = dotted_to_fluent_id(id_clean);
                rewritten.insert(transformed_id.clone(), id_clean.to_string());
                out.push_str(&transformed_id);
                let trailing_spaces = id_part.len() - id_clean.len();
                for _ in 0..trailing_spaces {
                    out.push(' ');
                }
                out.push('=');
                out.push_str(rest);
                continue;
            }
        }
        out.push_str(line);
    }
    (out, rewritten)
}

pub fn parse_ftl_resource(
    ftl_content: &str,
    locale: &str,
) -> Result<ParsedCatalogFile, Vec<Fault>> {
    let mut faults = Vec::new();
    let (preprocessed, rewritten) = preprocess_ftl_id_dots(ftl_content);
    let res = match parser::parse(preprocessed.as_str()) {
        Ok(r) => r,
        Err((r, errs)) => {
            for err in &errs {
                match &err.kind {
                    parser::ErrorKind::MissingDefaultVariant
                    | parser::ErrorKind::MultipleDefaultVariants => {
                        faults.push(Fault::new("enum-default", locale, "<unparsed>"));
                    }
                    _ => {}
                }
            }
            if faults.is_empty() {
                for entry in &r.body {
                    if let ast::Entry::Junk { .. } = entry {
                        faults.push(Fault::new("junk", locale, "<unparsed>"));
                    }
                }
            }
            if faults.is_empty() {
                faults.push(Fault::new("junk", locale, "<unparsed>"));
            }
            return Err(faults);
        }
    };

    let mut messages = HashMap::new();
    let mut seen_ids = HashSet::new();

    for entry in res.body {
        match entry {
            ast::Entry::Comment(_)
            | ast::Entry::GroupComment(_)
            | ast::Entry::ResourceComment(_) => {
                // Comments ignored
            }
            ast::Entry::Junk { .. } => {
                faults.push(Fault::new("junk", locale, "<unparsed>"));
            }
            ast::Entry::Term(_) => {
                faults.push(Fault::new("term", locale, "<unparsed>"));
            }
            ast::Entry::Message(msg) => {
                let raw_id = msg.id.name;
                let msg_id = if let Some(orig) = rewritten.get(raw_id) {
                    orig.clone()
                } else if raw_id.contains("__DOT__") {
                    faults.push(Fault::new("encoded-alias", locale, raw_id));
                    continue;
                } else {
                    raw_id.to_string()
                };
                if !seen_ids.insert(msg_id.clone()) {
                    faults.push(Fault::new("duplicate-id", locale, &msg_id));
                    continue;
                }
                if !msg.attributes.is_empty() {
                    faults.push(Fault::new("attribute", locale, &msg_id));
                }

                match parse_message_pattern(&msg, locale, &msg_id) {
                    Ok(parsed) => {
                        messages.insert(msg_id, parsed);
                    }
                    Err(mut f) => {
                        faults.append(&mut f);
                    }
                }
            }
        }
    }

    if !faults.is_empty() {
        Err(faults)
    } else {
        Ok(ParsedCatalogFile { messages })
    }
}

fn parse_message_pattern(
    msg: &ast::Message<&str>,
    locale: &str,
    msg_id: &str,
) -> Result<ParsedMessage, Vec<Fault>> {
    let mut faults = Vec::new();

    let pattern = match &msg.value {
        Some(p) => p,
        None => {
            faults.push(Fault::new("junk", locale, msg_id));
            return Err(faults);
        }
    };

    // Check if there are multiple selectors or mixed selectors
    let select_count = pattern
        .elements
        .iter()
        .filter(|el| {
            matches!(
                el,
                ast::PatternElement::Placeable {
                    expression: ast::Expression::Select { .. }
                }
            )
        })
        .count();

    if select_count > 1 {
        faults.push(Fault::new("multiple-selectors", locale, msg_id));
        return Err(faults);
    }

    if select_count == 1 && pattern.elements.len() > 1 {
        faults.push(Fault::new("selector-shape", locale, msg_id));
        return Err(faults);
    }

    if pattern.elements.len() == 1
        && let ast::PatternElement::Placeable {
            expression: ast::Expression::Select { selector, variants },
        } = &pattern.elements[0]
    {
        let selector_var = match selector {
            ast::InlineExpression::VariableReference { id } => id.name.to_string(),
            _ => {
                faults.push(Fault::new("selector-shape", locale, msg_id));
                return Err(faults);
            }
        };

        let mut branches = Vec::new();
        let mut seen_keys = HashSet::new();

        for variant in variants {
            let key_str = match &variant.key {
                ast::VariantKey::Identifier { name } => name.to_string(),
                ast::VariantKey::NumberLiteral { .. } => {
                    faults.push(Fault::new("exact-number-branch", locale, msg_id));
                    continue;
                }
            };

            if !seen_keys.insert(key_str.clone()) {
                faults.push(Fault::new("duplicate-branch", locale, msg_id));
            }

            let branch_pattern =
                match parse_pattern_elements(&variant.value.elements, locale, msg_id) {
                    Ok(p) => p,
                    Err(mut f) => {
                        faults.append(&mut f);
                        Vec::new()
                    }
                };

            branches.push(Branch {
                key: Some(key_str),
                default: variant.default,
                pattern: branch_pattern,
            });
        }

        if !faults.is_empty() {
            return Err(faults);
        }

        return Ok(ParsedMessage {
            id: msg_id.to_string(),
            branches,
            selector_var: Some(selector_var),
        });
    }

    // Non-selector pattern
    let branch_pattern = parse_pattern_elements(&pattern.elements, locale, msg_id)?;

    Ok(ParsedMessage {
        id: msg_id.to_string(),
        branches: vec![Branch {
            key: None,
            default: true,
            pattern: branch_pattern,
        }],
        selector_var: None,
    })
}

fn parse_pattern_elements(
    elements: &[ast::PatternElement<&str>],
    locale: &str,
    msg_id: &str,
) -> Result<Vec<PatternElement>, Vec<Fault>> {
    let mut faults = Vec::new();
    let mut out = Vec::new();

    for elem in elements {
        match elem {
            ast::PatternElement::TextElement { value } => {
                let bytes = value.as_bytes();
                let mut is_rich = false;
                for i in 0..bytes.len() {
                    if bytes[i] == b'<' && i + 1 < bytes.len() {
                        let next = bytes[i + 1];
                        if next.is_ascii_alphabetic() || next == b'/' {
                            is_rich = true;
                            break;
                        }
                    }
                }
                if is_rich {
                    faults.push(Fault::new("rich-text", locale, msg_id));
                }
                out.push(PatternElement {
                    kind: ElementKind::Text,
                    value: value.to_string(),
                });
            }
            ast::PatternElement::Placeable { expression } => match expression {
                ast::Expression::Select { .. } => {
                    faults.push(Fault::new("nested-selector", locale, msg_id));
                }
                ast::Expression::Inline(inline) => match inline {
                    ast::InlineExpression::VariableReference { id } => {
                        // Will be resolved to arg or slot in admission checking
                        out.push(PatternElement {
                            kind: ElementKind::Arg,
                            value: id.name.to_string(),
                        });
                    }
                    ast::InlineExpression::StringLiteral { .. }
                    | ast::InlineExpression::NumberLiteral { .. } => {
                        faults.push(Fault::new("literal-placeable", locale, msg_id));
                    }
                    ast::InlineExpression::MessageReference { .. } => {
                        faults.push(Fault::new("message-reference", locale, msg_id));
                    }
                    ast::InlineExpression::FunctionReference { .. } => {
                        faults.push(Fault::new("function-call", locale, msg_id));
                    }
                    ast::InlineExpression::TermReference { .. }
                    | ast::InlineExpression::Placeable { .. } => {
                        faults.push(Fault::new("rich-text", locale, msg_id));
                    }
                },
            },
        }
    }

    if !faults.is_empty() {
        Err(faults)
    } else {
        Ok(out)
    }
}

pub fn validate_admitted_message(
    msg: &mut ParsedMessage,
    meta: &MessageMetadata,
    locale: &str,
    is_source: bool,
    source_msg: Option<&ParsedMessage>,
) -> Result<(), Vec<Fault>> {
    let mut faults = Vec::new();
    let msg_id = &msg.id;

    // Check format intent vs shape
    match meta.format_intent {
        FormatIntent::Text => {
            if msg.selector_var.is_some() {
                faults.push(Fault::new("selector-shape", locale, msg_id));
            }
        }
        FormatIntent::Cardinal => {
            if let Some(msg_sel) = &msg.selector_var {
                if let Some(sel) = &meta.selector {
                    if sel.name != *msg_sel || sel.selector_type != "cardinal" {
                        faults.push(Fault::new("selector-identity", locale, msg_id));
                    }
                } else {
                    faults.push(Fault::new("selector-identity", locale, msg_id));
                }
            } else {
                faults.push(Fault::new("selector-shape", locale, msg_id));
            }
        }
        FormatIntent::Enum => {
            if let Some(msg_sel) = &msg.selector_var {
                if let Some(sel) = &meta.selector {
                    if sel.name != *msg_sel || sel.selector_type != "string" {
                        faults.push(Fault::new("selector-identity", locale, msg_id));
                    }
                } else {
                    faults.push(Fault::new("selector-identity", locale, msg_id));
                }
            } else {
                faults.push(Fault::new("selector-shape", locale, msg_id));
            }
        }
    }

    // Validate branches defaults and keys
    if let Some(_sel_var) = &msg.selector_var {
        match meta.format_intent {
            FormatIntent::Cardinal => {
                let default_count = msg.branches.iter().filter(|b| b.default).count();
                let has_other_default = msg
                    .branches
                    .iter()
                    .any(|b| b.key.as_deref() == Some("other") && b.default);

                if !has_other_default || default_count != 1 {
                    faults.push(Fault::new("cardinal-default", locale, msg_id));
                }

                for b in &msg.branches {
                    if let Some(key) = &b.key
                        && !CLDR_CATEGORIES.contains(&key.as_str())
                    {
                        faults.push(Fault::new("cardinal-default", locale, msg_id));
                    }
                }
            }
            FormatIntent::Enum => {
                let default_count = msg.branches.iter().filter(|b| b.default).count();
                if default_count != 1 {
                    faults.push(Fault::new("enum-default", locale, msg_id));
                }

                if let Some(sel) = &meta.selector {
                    if let Some(def_b) = msg.branches.iter().find(|b| b.default)
                        && def_b.key.as_deref() != sel.enum_default.as_deref()
                    {
                        faults.push(Fault::new("enum-default", locale, msg_id));
                    }

                    for b in &msg.branches {
                        if let Some(key) = &b.key
                            && !sel.enum_values.contains(key)
                        {
                            faults.push(Fault::new("enum-branch-set", locale, msg_id));
                        }
                    }
                    let msg_keys: HashSet<&str> = msg
                        .branches
                        .iter()
                        .filter_map(|b| b.key.as_deref())
                        .collect();
                    let expected_keys: HashSet<&str> =
                        sel.enum_values.iter().map(|s| s.as_str()).collect();
                    if msg_keys != expected_keys {
                        faults.push(Fault::new("enum-branch-set", locale, msg_id));
                    }
                }
            }
            FormatIntent::Text => {}
        }
    }

    // Resolve pattern elements to Slot or Arg, check undeclared
    let slot_names: HashSet<&str> = meta
        .protected_slots
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let arg_names: HashSet<&str> = meta.arguments.iter().map(|a| a.name.as_str()).collect();

    let mut message_args_union = HashSet::new();

    for branch in &mut msg.branches {
        for el in &mut branch.pattern {
            if el.kind == ElementKind::Arg {
                let name = el.value.as_str();
                if slot_names.contains(name) {
                    el.kind = ElementKind::Slot;
                } else if arg_names.contains(name) {
                    message_args_union.insert(name.to_string());
                } else {
                    faults.push(Fault::new("argument-parity", locale, msg_id));
                }
            }
        }
    }

    if let Some(sel_var) = &msg.selector_var {
        message_args_union.insert(sel_var.clone());
    }

    // Argument parity: union of argument names across message must equal metadata arguments
    let declared_args: HashSet<String> = meta.arguments.iter().map(|a| a.name.clone()).collect();
    if message_args_union != declared_args {
        faults.push(Fault::new("argument-parity", locale, msg_id));
    }

    // Compare translation with source if not source
    if !is_source && let Some(src) = source_msg {
        match meta.format_intent {
            FormatIntent::Cardinal => {
                // Check source categories are present
                for src_b in &src.branches {
                    if let Some(key) = &src_b.key
                        && !msg.branches.iter().any(|b| b.key.as_deref() == Some(key))
                    {
                        faults.push(Fault::new("cardinal-branch-missing", locale, msg_id));
                    }
                }
                // For added categories, slot multiset must match source other
                let src_other_slots = src
                    .branches
                    .iter()
                    .find(|b| b.key.as_deref() == Some("other"))
                    .map(|b| get_slot_multiset(&b.pattern))
                    .unwrap_or_default();

                for b in &msg.branches {
                    let key = b.key.as_deref().unwrap_or("");
                    let is_in_source = src.branches.iter().any(|sb| sb.key.as_deref() == Some(key));
                    if !is_in_source {
                        let added_slots = get_slot_multiset(&b.pattern);
                        if added_slots != src_other_slots {
                            faults.push(Fault::new("protected-slot-mismatch", locale, msg_id));
                        }
                    }
                }
            }
            FormatIntent::Enum => {
                // Enum branch identities and default must match exactly
                let src_keys: HashSet<_> = src
                    .branches
                    .iter()
                    .map(|b| (b.key.clone(), b.default))
                    .collect();
                let msg_keys: HashSet<_> = msg
                    .branches
                    .iter()
                    .map(|b| (b.key.clone(), b.default))
                    .collect();
                if src_keys != msg_keys {
                    faults.push(Fault::new("enum-branch-set", locale, msg_id));
                }
            }
            FormatIntent::Text => {}
        }

        // Check protected slots multiset matching between corresponding branches
        for src_b in &src.branches {
            if let Some(tr_b) = msg.branches.iter().find(|b| b.key == src_b.key) {
                let src_slots = get_slot_multiset(&src_b.pattern);
                let tr_slots = get_slot_multiset(&tr_b.pattern);
                if src_slots != tr_slots {
                    faults.push(Fault::new("protected-slot-mismatch", locale, msg_id));
                }
            }
        }
    }

    if !faults.is_empty() {
        Err(faults)
    } else {
        Ok(())
    }
}

fn get_slot_multiset(pattern: &[PatternElement]) -> Vec<String> {
    let mut slots: Vec<String> = pattern
        .iter()
        .filter(|el| el.kind == ElementKind::Slot)
        .map(|el| el.value.clone())
        .collect();
    slots.sort();
    slots
}
