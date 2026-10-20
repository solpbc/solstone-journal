// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::hexutil::is_hex64;
use crate::model::{ArgumentSpec, ProtectedSlotSpec, SelectorSpec};
use crate::scalar::validate_numeric_bounds;
use crate::vocab::{FormatIntent, NumericStyle, ProtectedSlotKind, Risk, ScalarType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub locale: String,
    pub source_revision: String,
    pub translation_revision: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessageMetadata {
    pub description: String,
    pub context: String,
    pub surfaces: Vec<String>,
    pub bundles: Vec<String>,
    pub arguments: Vec<ArgumentSpec>,
    pub format_intent: FormatIntent,
    pub max_length: usize,
    pub source_revision: String,
    pub risk: Risk,
    pub auto_merge: bool,
    pub approval: Option<ApprovalRecord>,
    pub protected_slots: Vec<ProtectedSlotSpec>,
    pub selector: Option<SelectorSpec>,
    pub webextension: bool,
}

impl MessageMetadata {
    pub fn parse(message_id: &str, raw_json: &str) -> Result<Self, Fault> {
        let value: Value = serde_json::from_str(raw_json)
            .map_err(|_| Fault::new("metadata-syntax", "*", message_id))?;

        let obj = match value {
            Value::Object(map) => map,
            _ => return Err(Fault::new("metadata-syntax", "*", message_id)),
        };

        // Allowed keys check
        const ALLOWED_KEYS: &[&str] = &[
            "description",
            "context",
            "surfaces",
            "bundles",
            "arguments",
            "format_intent",
            "max_length",
            "source_revision",
            "risk",
            "auto_merge",
            "approval",
            "protected_slots",
            "selector",
            "webextension",
        ];

        for key in obj.keys() {
            if !ALLOWED_KEYS.contains(&key.as_str()) {
                return Err(Fault::new("unknown-field", "*", message_id));
            }
        }

        let description = obj
            .get("description")
            .and_then(Value::as_str)
            .ok_or_else(|| Fault::new("empty-description", "*", message_id))?;
        if description.is_empty() {
            return Err(Fault::new("empty-description", "*", message_id));
        }

        let context = obj
            .get("context")
            .and_then(Value::as_str)
            .ok_or_else(|| Fault::new("empty-context", "*", message_id))?;
        if context.is_empty() {
            return Err(Fault::new("empty-context", "*", message_id));
        }

        let surfaces_val = obj
            .get("surfaces")
            .and_then(Value::as_array)
            .ok_or_else(|| Fault::new("empty-surfaces", "*", message_id))?;
        if surfaces_val.is_empty() {
            return Err(Fault::new("empty-surfaces", "*", message_id));
        }
        let mut surfaces = Vec::new();
        let mut seen_surfaces = HashSet::new();
        for value in surfaces_val {
            let surface = value
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| Fault::new("empty-surfaces", "*", message_id))?;
            if !seen_surfaces.insert(surface.to_string()) {
                return Err(Fault::new("duplicate-surface", "*", message_id));
            }
            surfaces.push(surface.to_string());
        }

        let bundles_val = obj
            .get("bundles")
            .and_then(Value::as_array)
            .ok_or_else(|| Fault::new("empty-membership", "*", message_id))?;
        if bundles_val.is_empty() {
            return Err(Fault::new("empty-membership", "*", message_id));
        }
        let mut bundles = Vec::with_capacity(bundles_val.len());
        for b in bundles_val {
            let s = b
                .as_str()
                .ok_or_else(|| Fault::new("empty-membership", "*", message_id))?;
            if s.is_empty() {
                return Err(Fault::new("empty-membership", "*", message_id));
            }
            bundles.push(s.to_string());
        }

        let max_length_val = obj
            .get("max_length")
            .and_then(Value::as_i64)
            .ok_or_else(|| Fault::new("max-length", "*", message_id))?;
        if max_length_val <= 0 {
            return Err(Fault::new("max-length", "*", message_id));
        }
        let max_length = max_length_val as usize;

        let source_rev = obj
            .get("source_revision")
            .and_then(Value::as_str)
            .ok_or_else(|| Fault::new("hash-syntax", "*", message_id))?;
        if !is_hex64(source_rev) {
            return Err(Fault::new("hash-syntax", "*", message_id));
        }

        let risk_str = obj
            .get("risk")
            .and_then(Value::as_str)
            .ok_or_else(|| Fault::new("unknown-field", "*", message_id))?;
        let risk = match risk_str {
            "routine" => Risk::Routine,
            "account" => Risk::Account,
            "payment" => Risk::Payment,
            "legal" => Risk::Legal,
            "statutory" => Risk::Statutory,
            "protocol" => Risk::Protocol,
            "consent" => Risk::Consent,
            _ => return Err(Fault::new("unknown-field", "*", message_id)),
        };

        let auto_merge = match obj.get("auto_merge") {
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(Fault::new("auto-merge-ineligible", "*", message_id)),
            None => false,
        };

        if risk.is_high_risk() && auto_merge {
            return Err(Fault::new("auto-merge-ineligible", "*", message_id));
        }

        let approval = match obj.get("approval") {
            Some(Value::Null) | None => None,
            Some(Value::Object(app_map)) => {
                const APP_KEYS: &[&str] = &["locale", "source_revision", "translation_revision"];
                for k in app_map.keys() {
                    if !APP_KEYS.contains(&k.as_str()) {
                        return Err(Fault::new("unknown-field", "*", message_id));
                    }
                }
                let loc = app_map
                    .get("locale")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::new("approval-shape", "*", message_id))?;
                if !crate::locale::validate_locale_tag(loc) {
                    return Err(Fault::new("locale-tag", loc, message_id));
                }
                let s_rev = app_map
                    .get("source_revision")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::new("approval-shape", "*", message_id))?;
                let t_rev = app_map
                    .get("translation_revision")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::new("approval-shape", "*", message_id))?;
                if !is_hex64(s_rev) || !is_hex64(t_rev) {
                    return Err(Fault::new("hash-syntax", "*", message_id));
                }
                Some(ApprovalRecord {
                    locale: loc.to_string(),
                    source_revision: s_rev.to_string(),
                    translation_revision: t_rev.to_string(),
                })
            }
            _ => return Err(Fault::new("approval-shape", "*", message_id)),
        };

        let format_intent_str = obj
            .get("format_intent")
            .and_then(Value::as_str)
            .ok_or_else(|| Fault::new("format-intent", "*", message_id))?;
        let format_intent = match format_intent_str {
            "text" => FormatIntent::Text,
            "cardinal" => FormatIntent::Cardinal,
            "enum" => FormatIntent::Enum,
            _ => return Err(Fault::new("format-intent", "*", message_id)),
        };

        let args_val = obj
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| Fault::new("arguments-shape", "*", message_id))?;

        let mut arguments = Vec::with_capacity(args_val.len());
        let mut seen_args = HashSet::new();
        for arg_item in args_val {
            let arg_obj = arg_item
                .as_object()
                .ok_or_else(|| Fault::new("arguments-shape", "*", message_id))?;
            const ARG_KEYS: &[&str] = &["name", "type", "numeric_style", "min", "max"];
            for k in arg_obj.keys() {
                if !ARG_KEYS.contains(&k.as_str()) {
                    return Err(Fault::new("unknown-field", "*", message_id));
                }
            }

            let name = arg_obj
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| Fault::new("arguments-shape", "*", message_id))?;
            if name.is_empty() {
                return Err(Fault::new("arguments-shape", "*", message_id));
            }
            if !seen_args.insert(name.to_string()) {
                return Err(Fault::new("duplicate-argument", "*", message_id));
            }

            let type_str = arg_obj
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| Fault::new("arguments-shape", "*", message_id))?;
            let arg_type = match type_str {
                "string" => ScalarType::String,
                "boolean" => ScalarType::Boolean,
                "integer" => ScalarType::Integer,
                "cardinal" => ScalarType::Cardinal,
                _ => return Err(Fault::new("arguments-shape", "*", message_id)),
            };

            let numeric_style = match arg_obj.get("numeric_style") {
                Some(Value::Null) | None => None,
                Some(Value::String(s)) => match s.as_str() {
                    "ungrouped" => Some(NumericStyle::Ungrouped),
                    "locale-formatted" => Some(NumericStyle::LocaleFormatted),
                    _ => return Err(Fault::new("numeric-style", "*", message_id)),
                },
                _ => return Err(Fault::new("numeric-style", "*", message_id)),
            };

            let min = match arg_obj.get("min") {
                Some(Value::Null) | None => None,
                Some(Value::Bool(_)) => {
                    return Err(Fault::new("not-a-number", "*", message_id));
                }
                Some(v) => {
                    let num = v
                        .as_f64()
                        .ok_or_else(|| Fault::new("not-a-number", "*", message_id))?;
                    Some(num)
                }
            };

            let max = match arg_obj.get("max") {
                Some(Value::Null) | None => None,
                Some(Value::Bool(_)) => {
                    return Err(Fault::new("not-a-number", "*", message_id));
                }
                Some(v) => {
                    let num = v
                        .as_f64()
                        .ok_or_else(|| Fault::new("not-a-number", "*", message_id))?;
                    Some(num)
                }
            };

            if matches!(arg_type, ScalarType::String | ScalarType::Boolean) {
                if numeric_style.is_some() {
                    return Err(Fault::new("numeric-style", "*", message_id));
                }
                if min.is_some() || max.is_some() {
                    return Err(Fault::new("numeric-range", "*", message_id));
                }
            } else {
                if numeric_style.is_none() {
                    return Err(Fault::new("numeric-style", "*", message_id));
                }
                validate_numeric_bounds(arg_type, min, max)
                    .map_err(|rule| Fault::new(rule, "*", message_id))?;
            }

            arguments.push(ArgumentSpec {
                name: name.to_string(),
                arg_type,
                numeric_style,
                min,
                max,
            });
        }

        let mut protected_slots = Vec::new();
        let mut seen_slots = HashSet::new();
        if let Some(ps_val) = obj.get("protected_slots") {
            let ps_arr = ps_val
                .as_array()
                .ok_or_else(|| Fault::new("slots-shape", "*", message_id))?;
            for slot_item in ps_arr {
                let s_obj = slot_item
                    .as_object()
                    .ok_or_else(|| Fault::new("slots-shape", "*", message_id))?;
                const SLOT_KEYS: &[&str] = &["name", "kind", "value"];
                for k in s_obj.keys() {
                    if !SLOT_KEYS.contains(&k.as_str()) {
                        return Err(Fault::new("unknown-field", "*", message_id));
                    }
                }
                let s_name = s_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::new("slots-shape", "*", message_id))?;
                if !seen_slots.insert(s_name.to_string()) {
                    return Err(Fault::new("duplicate-slot", "*", message_id));
                }
                let s_kind_str = s_obj
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::new("slots-shape", "*", message_id))?;
                let s_kind = match s_kind_str {
                    "command" => ProtectedSlotKind::Command,
                    "flag" => ProtectedSlotKind::Flag,
                    "accepted-input" => ProtectedSlotKind::AcceptedInput,
                    "url" => ProtectedSlotKind::Url,
                    "mark-word" => ProtectedSlotKind::MarkWord,
                    "whitespace" => ProtectedSlotKind::Whitespace,
                    _ => return Err(Fault::new("slots-shape", "*", message_id)),
                };
                let s_val = s_obj
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::new("slots-shape", "*", message_id))?;

                protected_slots.push(ProtectedSlotSpec {
                    name: s_name.to_string(),
                    kind: s_kind,
                    value: s_val.to_string(),
                });
            }
        }

        let (selector, sel_ev_present, sel_ed_present) = match obj.get("selector") {
            Some(Value::Null) | None => (None, false, false),
            Some(Value::Object(sel_obj)) => {
                const SEL_KEYS: &[&str] = &["name", "type", "enum_values", "enum_default"];
                for k in sel_obj.keys() {
                    if !SEL_KEYS.contains(&k.as_str()) {
                        return Err(Fault::new("unknown-field", "*", message_id));
                    }
                }
                let s_name = sel_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::new("selector-shape", "*", message_id))?;
                let s_type = sel_obj
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::new("selector-shape", "*", message_id))?;

                let mut enum_values = Vec::new();
                let mut seen_values = HashSet::new();
                let mut ev_present = false;
                if let Some(ev_val) = sel_obj.get("enum_values")
                    && !ev_val.is_null()
                {
                    ev_present = true;
                    let ev_arr = ev_val
                        .as_array()
                        .ok_or_else(|| Fault::new("selector-shape", "*", message_id))?;
                    for ev in ev_arr {
                        let s = ev
                            .as_str()
                            .ok_or_else(|| Fault::new("selector-shape", "*", message_id))?;
                        if !seen_values.insert(s.to_string()) {
                            return Err(Fault::new("duplicate-enum-value", "*", message_id));
                        }
                        enum_values.push(s.to_string());
                    }
                }

                let mut enum_default = None;
                let mut ed_present = false;
                if let Some(ed_val) = sel_obj.get("enum_default")
                    && !ed_val.is_null()
                {
                    ed_present = true;
                    let s = ed_val
                        .as_str()
                        .ok_or_else(|| Fault::new("selector-shape", "*", message_id))?;
                    enum_default = Some(s.to_string());
                }

                (
                    Some(SelectorSpec {
                        name: s_name.to_string(),
                        selector_type: s_type.to_string(),
                        enum_values,
                        enum_default,
                    }),
                    ev_present,
                    ed_present,
                )
            }
            _ => return Err(Fault::new("selector-shape", "*", message_id)),
        };

        match format_intent {
            FormatIntent::Text => {
                if selector.is_some() {
                    return Err(Fault::new("selector-intent", "*", message_id));
                }
            }
            FormatIntent::Cardinal => {
                let sel = selector
                    .as_ref()
                    .ok_or_else(|| Fault::new("selector-intent", "*", message_id))?;
                if sel.selector_type != "cardinal" || sel_ev_present || sel_ed_present {
                    return Err(Fault::new("selector-intent", "*", message_id));
                }
            }
            FormatIntent::Enum => {
                let sel = selector
                    .as_ref()
                    .ok_or_else(|| Fault::new("selector-intent", "*", message_id))?;
                if sel.selector_type != "string" {
                    return Err(Fault::new("selector-intent", "*", message_id));
                }
                if !sel_ev_present || sel.enum_values.is_empty() {
                    return Err(Fault::new("enum-default", "*", message_id));
                }
                match &sel.enum_default {
                    Some(def) if sel.enum_values.contains(def) => {}
                    _ => return Err(Fault::new("enum-default", "*", message_id)),
                }
            }
        }

        // Check slot-arg disjointness
        for s in &protected_slots {
            if arguments.iter().any(|a| a.name == s.name) {
                return Err(Fault::new("slot-arg-disjoint", "*", message_id));
            }
        }

        if let Some(sel) = &selector {
            let expected = match format_intent {
                FormatIntent::Cardinal => ScalarType::Cardinal,
                FormatIntent::Enum => ScalarType::String,
                FormatIntent::Text => return Err(Fault::new("selector-intent", "*", message_id)),
            };
            if !arguments
                .iter()
                .any(|arg| arg.name == sel.name && arg.arg_type == expected)
            {
                return Err(Fault::new("selector-type", "*", message_id));
            }
        }

        let webextension = match obj.get("webextension") {
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(Fault::new("unknown-field", "*", message_id)),
            None => false,
        };

        if webextension && (!arguments.is_empty() || selector.is_some()) {
            return Err(Fault::new("webextension-dynamic", "*", message_id));
        }

        Ok(Self {
            description: description.to_string(),
            context: context.to_string(),
            surfaces,
            bundles,
            arguments,
            format_intent,
            max_length,
            source_revision: source_rev.to_string(),
            risk,
            auto_merge,
            approval,
            protected_slots,
            selector,
            webextension,
        })
    }
}
