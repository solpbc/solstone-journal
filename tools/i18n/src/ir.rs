// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::{CLDR_CATEGORIES, ParsedMessage};
use crate::history::compute_resource_key;
use crate::metadata::{ApprovalRecord, MessageMetadata};
use crate::model::{Branch, ElementKind};
use crate::vocab::{Channel, FormatIntent, NumericStyle, ScalarType};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct CompiledCatalogIr {
    pub ir_version: u32,
    pub compiler: String,
    pub channel: String,
    pub source_locale: String,
    pub locales: Vec<String>,
    pub messages: Vec<CompiledMessage>,
}

#[derive(Serialize)]
pub struct CompiledMessage {
    pub id: String,
    pub resource: String,
    pub namespace: String,
    pub owner: String,
    pub risk: String,
    pub auto_merge: bool,
    pub format_intent: String,
    pub max_length: usize,
    pub description: String,
    pub context: String,
    pub bundles: Vec<String>,
    pub source_revision: String,
    pub approval: Option<ApprovalRecord>,
    pub arguments: Vec<CompiledArgument>,
    pub protected_slots: Vec<CompiledSlot>,
    pub selector: Option<CompiledSelector>,
    pub source: CompiledPatternContainer,
    pub translations: Vec<CompiledTranslation>,
}

#[derive(Serialize)]
pub struct CompiledArgument {
    pub name: String,
    #[serde(rename = "type")]
    pub arg_type: String,
    pub numeric_style: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Serialize)]
pub struct CompiledSlot {
    pub name: String,
    pub kind: String,
    pub value: String,
}

#[derive(Serialize)]
pub struct CompiledSelector {
    pub name: String,
    #[serde(rename = "type")]
    pub selector_type: String,
    pub enum_values: Vec<String>,
    pub enum_default: Option<String>,
}

#[derive(Serialize)]
pub struct CompiledPatternContainer {
    pub branches: Vec<CompiledBranch>,
}

#[derive(Serialize)]
pub struct CompiledTranslation {
    pub locale: String,
    pub branches: Vec<CompiledBranch>,
}

#[derive(Serialize)]
pub struct CompiledBranch {
    pub key: Option<String>,
    pub default: bool,
    pub pattern: Vec<CompiledElement>,
}

#[derive(Serialize)]
pub struct CompiledElement {
    pub kind: String,
    pub value: String,
}

pub fn build_ir(
    channel: Channel,
    source_locale: &str,
    selected_locales: &[String],
    source_messages: &HashMap<String, ParsedMessage>,
    metadata_map: &HashMap<String, MessageMetadata>,
    translations: &HashMap<String, HashMap<String, ParsedMessage>>,
    namespace_owners: &HashMap<String, String>,
) -> CompiledCatalogIr {
    let mut locales_sorted = selected_locales.to_vec();
    locales_sorted.sort();

    let mut message_ids: Vec<&String> = source_messages.keys().collect();
    message_ids.sort();

    let mut compiled_messages = Vec::new();

    for id in message_ids {
        let src_msg = &source_messages[id];
        let meta = &metadata_map[id];

        let ns = id.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
        let owner = namespace_owners.get(ns).cloned().unwrap_or_default();
        let resource = compute_resource_key(id);

        let mut args: Vec<CompiledArgument> = meta
            .arguments
            .iter()
            .map(|a| CompiledArgument {
                name: a.name.clone(),
                arg_type: match a.arg_type {
                    ScalarType::String => "string".to_string(),
                    ScalarType::Boolean => "boolean".to_string(),
                    ScalarType::Integer => "integer".to_string(),
                    ScalarType::Cardinal => "cardinal".to_string(),
                },
                numeric_style: a.numeric_style.map(|s| match s {
                    NumericStyle::Ungrouped => "ungrouped".to_string(),
                    NumericStyle::LocaleFormatted => "locale-formatted".to_string(),
                }),
                min: a.min,
                max: a.max,
            })
            .collect();
        args.sort_by(|a, b| a.name.cmp(&b.name));

        let mut slots: Vec<CompiledSlot> = meta
            .protected_slots
            .iter()
            .map(|s| CompiledSlot {
                name: s.name.clone(),
                kind: s.kind.as_str().to_string(),
                value: s.value.clone(),
            })
            .collect();
        slots.sort_by(|a, b| a.name.cmp(&b.name));

        let selector = match meta.format_intent {
            FormatIntent::Text => None,
            FormatIntent::Cardinal => {
                let name = src_msg.selector_var.clone().unwrap_or_default();
                Some(CompiledSelector {
                    name,
                    selector_type: "cardinal".to_string(),
                    enum_values: Vec::new(),
                    enum_default: None,
                })
            }
            FormatIntent::Enum => {
                let name = src_msg.selector_var.clone().unwrap_or_default();
                let mut enum_values = meta
                    .selector
                    .as_ref()
                    .map(|s| s.enum_values.clone())
                    .unwrap_or_default();
                enum_values.sort();
                let enum_default = meta.selector.as_ref().and_then(|s| s.enum_default.clone());
                Some(CompiledSelector {
                    name,
                    selector_type: "string".to_string(),
                    enum_values,
                    enum_default,
                })
            }
        };

        let source_branches = compile_branches(&src_msg.branches, meta.format_intent);

        let mut compiled_translations = Vec::new();
        for loc in &locales_sorted {
            if let Some(loc_map) = translations.get(loc)
                && let Some(tr_msg) = loc_map.get(id)
            {
                let tr_branches = compile_branches(&tr_msg.branches, meta.format_intent);
                compiled_translations.push(CompiledTranslation {
                    locale: loc.clone(),
                    branches: tr_branches,
                });
            }
        }
        compiled_translations.sort_by(|a, b| a.locale.cmp(&b.locale));

        let mut bundles_sorted = meta.bundles.clone();
        bundles_sorted.sort();

        compiled_messages.push(CompiledMessage {
            id: id.clone(),
            resource,
            namespace: ns.to_string(),
            owner,
            risk: meta.risk.as_str().to_string(),
            auto_merge: meta.auto_merge,
            format_intent: match meta.format_intent {
                FormatIntent::Text => "text".to_string(),
                FormatIntent::Cardinal => "cardinal".to_string(),
                FormatIntent::Enum => "enum".to_string(),
            },
            max_length: meta.max_length,
            description: meta.description.clone(),
            context: meta.context.clone(),
            bundles: bundles_sorted,
            source_revision: meta.source_revision.clone(),
            approval: meta.approval.clone(),
            arguments: args,
            protected_slots: slots,
            selector,
            source: CompiledPatternContainer {
                branches: source_branches,
            },
            translations: compiled_translations,
        });
    }

    CompiledCatalogIr {
        ir_version: 1,
        compiler: "0.1.0".to_string(),
        channel: match channel {
            Channel::Development => "development".to_string(),
            Channel::Production => "production".to_string(),
        },
        source_locale: source_locale.to_string(),
        locales: locales_sorted,
        messages: compiled_messages,
    }
}

fn compile_branches(branches: &[Branch], intent: FormatIntent) -> Vec<CompiledBranch> {
    let mut compiled: Vec<CompiledBranch> = branches
        .iter()
        .map(|b| CompiledBranch {
            key: b.key.clone(),
            default: b.default,
            pattern: b
                .pattern
                .iter()
                .map(|el| CompiledElement {
                    kind: match el.kind {
                        ElementKind::Text => "text".to_string(),
                        ElementKind::Arg => "arg".to_string(),
                        ElementKind::Slot => "slot".to_string(),
                    },
                    value: el.value.clone(),
                })
                .collect(),
        })
        .collect();

    match intent {
        FormatIntent::Cardinal => {
            compiled.sort_by_key(|b| {
                let k = b.key.as_deref().unwrap_or("");
                CLDR_CATEGORIES
                    .iter()
                    .position(|&c| c == k)
                    .unwrap_or(usize::MAX)
            });
        }
        FormatIntent::Enum => {
            compiled.sort_by(|a, b| a.key.cmp(&b.key));
        }
        FormatIntent::Text => {}
    }

    compiled
}
