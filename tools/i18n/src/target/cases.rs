// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::model::ElementKind;
use crate::target::model::{Target, TargetBundle, TargetMessage};
use crate::target::oracle;
use crate::vocab::{Channel, FormatIntent, NumericStyle, ScalarType};
use serde_json::{Map, Value as JsonValue, json};

#[derive(Clone, Debug)]
pub struct TestCase {
    pub target: String,
    pub bundle: String,
    pub locale: String,
    pub folder: Option<String>,
    pub message_id: String,
    pub resource_key: String,
    pub adapter_call: String,
    pub inputs: Map<String, JsonValue>,
    pub expected: String,
    pub own_reader: String,
    pub platform_reader: String,
}

impl TestCase {
    pub fn to_json_value(&self) -> JsonValue {
        let mut map = Map::new();
        map.insert("target".to_string(), json!(self.target));
        map.insert("bundle".to_string(), json!(self.bundle));
        map.insert("locale".to_string(), json!(self.locale));
        map.insert(
            "folder".to_string(),
            match &self.folder {
                Some(f) => json!(f),
                None => JsonValue::Null,
            },
        );
        map.insert("message_id".to_string(), json!(self.message_id));
        map.insert("resource_key".to_string(), json!(self.resource_key));
        map.insert("adapter_call".to_string(), json!(self.adapter_call));

        // Sort inputs keys
        let mut sorted_inputs = Map::new();
        let mut keys: Vec<String> = self.inputs.keys().cloned().collect();
        keys.sort();
        for k in keys {
            sorted_inputs.insert(k.clone(), self.inputs[&k].clone());
        }
        map.insert("inputs".to_string(), JsonValue::Object(sorted_inputs));
        map.insert("expected".to_string(), json!(self.expected));
        map.insert("own_reader".to_string(), json!(self.own_reader));
        map.insert("platform_reader".to_string(), json!(self.platform_reader));

        JsonValue::Object(map)
    }
}

pub fn generate_cases(
    bundles: &[TargetBundle],
    targets: &[Target],
    channel: Channel,
) -> Result<JsonValue, Vec<Fault>> {
    let mut cases = Vec::new();

    for &tgt in targets {
        for bundle in bundles {
            let own_reader = match tgt {
                Target::Rust => "rust-api",
                Target::Esm => "fluent-js",
                Target::Apple => "xcstrings",
                Target::Android => "android-xml",
                Target::Webextension => "webextension-json",
                Target::Gettext => "gettext-po",
            };

            for msg in &bundle.messages {
                if tgt == Target::Webextension && !msg.meta.webextension {
                    continue;
                }

                for loc in &bundle.locales {
                    if !msg.translations.contains_key(&loc.tag) {
                        continue;
                    }

                    let folder = if tgt == Target::Webextension {
                        loc.webextension_folder.clone()
                    } else if tgt == Target::Android {
                        crate::target::android::resolve_android_qualifier(loc).ok()
                    } else {
                        None
                    };

                    let test_inputs_list = get_test_inputs(bundle, msg, &loc.tag)?;
                    for (inputs, expected) in test_inputs_list {
                        let adapter_call =
                            format_adapter_call(tgt, &bundle.id, &loc.tag, msg, &inputs);

                        cases.push(TestCase {
                            target: tgt.as_str().to_string(),
                            bundle: bundle.id.clone(),
                            locale: loc.tag.clone(),
                            folder: folder.clone(),
                            message_id: msg.id.clone(),
                            resource_key: msg.resource_key.clone(),
                            adapter_call,
                            inputs,
                            expected,
                            own_reader: own_reader.to_string(),
                            platform_reader: "pending".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Sort cases by (target, bundle, locale, message_id, adapter_call)
    cases.sort_by(|a, b| {
        a.target
            .cmp(&b.target)
            .then_with(|| a.bundle.cmp(&b.bundle))
            .then_with(|| a.locale.cmp(&b.locale))
            .then_with(|| a.message_id.cmp(&b.message_id))
            .then_with(|| a.adapter_call.cmp(&b.adapter_call))
    });

    let cases_json: Vec<JsonValue> = cases.iter().map(|c| c.to_json_value()).collect();

    let channel_str = match channel {
        Channel::Development => "development",
        Channel::Production => "production",
    };

    Ok(json!({
        "catalog_version": 1,
        "channel": channel_str,
        "cases": cases_json
    }))
}

type TestInputPair = (Map<String, JsonValue>, String);

fn get_test_inputs(
    bundle: &TargetBundle,
    msg: &TargetMessage,
    locale: &str,
) -> Result<Vec<TestInputPair>, Vec<Fault>> {
    let mut res = Vec::new();

    match msg.id.as_str() {
        "synth.notes.anchor" | "synth.notes.percent" | "synth.notes.chrome" => {
            let inputs = Map::new();
            let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
            res.push((inputs, exp));
        }
        "synth.notes.label" => {
            let mut inputs = Map::new();
            inputs.insert("name".to_string(), json!("Alice"));
            let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
            res.push((inputs, exp));
        }
        "synth.notes.gate" => {
            let mut inputs = Map::new();
            inputs.insert("open".to_string(), json!(true));
            let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
            res.push((inputs, exp));
        }
        "synth.notes.span" => {
            for val in [1_i64, -1_i64, i32::MIN as i64, 10000_i64] {
                let mut inputs = Map::new();
                inputs.insert("count".to_string(), json!(val));
                let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
                res.push((inputs, exp));
            }
        }
        "synth.notes.forms" => {
            for val in [0_i64, 1_i64, 2_i64, 3_i64, 11_i64, 100_i64] {
                let mut inputs = Map::new();
                inputs.insert("count".to_string(), json!(val));
                let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
                res.push((inputs, exp));
            }
        }
        "synth.notes.choice" => {
            for kind in ["box", "bag", "unknown"] {
                let mut inputs = Map::new();
                inputs.insert("kind".to_string(), json!(kind));
                let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
                res.push((inputs, exp));
            }
        }
        "synth.notes.group" => {
            let mut inputs = Map::new();
            inputs.insert("n".to_string(), json!(10000));
            let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
            res.push((inputs, exp));
        }
        "synth.notes.wide" => {
            let mut inputs = Map::new();
            inputs.insert("n".to_string(), json!(9007199254740991_i64));
            let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
            res.push((inputs, exp));
        }
        "synth.notes.shell" => {
            let mut inputs = Map::new();
            inputs.insert("name".to_string(), json!("build"));
            let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
            res.push((inputs, exp));
        }
        _ => {
            // Default input generation for arbitrary messages
            let mut inputs = Map::new();
            for arg in &msg.meta.arguments {
                match arg.arg_type {
                    ScalarType::String => {
                        inputs.insert(arg.name.clone(), json!("sample"));
                    }
                    ScalarType::Boolean => {
                        inputs.insert(arg.name.clone(), json!(true));
                    }
                    ScalarType::Integer => {
                        let val = arg.min.map(|m| m as i64).unwrap_or(1);
                        inputs.insert(arg.name.clone(), json!(val));
                    }
                    ScalarType::Cardinal => {
                        let val = arg.min.map(|m| m as i64).unwrap_or(1);
                        inputs.insert(arg.name.clone(), json!(val));
                    }
                }
            }
            let exp = compute_expected_string(bundle, msg, locale, &inputs)?;
            res.push((inputs, exp));
        }
    }

    Ok(res)
}

pub fn compute_expected_string(
    bundle: &TargetBundle,
    msg: &TargetMessage,
    locale: &str,
    inputs: &Map<String, JsonValue>,
) -> Result<String, Vec<Fault>> {
    let parsed = match msg.translations.get(locale) {
        Some(p) => p,
        None => return Err(vec![Fault::new("missing-translation", locale, &msg.id)]),
    };

    let loc_entry = bundle.locales.iter().find(|l| l.tag == locale);
    let cardinal_rule = loc_entry
        .and_then(|l| l.cardinal.as_deref())
        .unwrap_or("und");

    // Determine which branch to evaluate
    let branch = match msg.meta.format_intent {
        FormatIntent::Text => &parsed.branches[0],
        FormatIntent::Enum => {
            let sel = msg.meta.selector.as_ref().unwrap();
            let def = sel
                .enum_default
                .as_deref()
                .ok_or_else(|| vec![Fault::new("enum-variant", locale, &msg.id)])?;
            let selected_val = inputs
                .get(&sel.name)
                .and_then(|v| v.as_str())
                .unwrap_or(def);
            parsed
                .branches
                .iter()
                .find(|b| b.key.as_deref() == Some(selected_val))
                .or_else(|| {
                    parsed
                        .branches
                        .iter()
                        .find(|b| b.key.as_deref() == Some(def))
                })
                .unwrap_or(&parsed.branches[0])
        }
        FormatIntent::Cardinal => {
            let sel = msg.meta.selector.as_ref().unwrap();
            let count_val = inputs.get(&sel.name).and_then(|v| v.as_i64()).unwrap_or(1);
            let cat =
                oracle::eval_fluent_category(cardinal_rule, count_val).map_err(|f| vec![f])?;
            parsed
                .branches
                .iter()
                .find(|b| b.key.as_deref() == Some(cat))
                .or_else(|| {
                    parsed
                        .branches
                        .iter()
                        .find(|b| b.key.as_deref() == Some("other"))
                })
                .unwrap_or(&parsed.branches[0])
        }
    };

    let mut out = String::new();
    for el in &branch.pattern {
        match el.kind {
            ElementKind::Text => {
                out.push_str(&el.value);
            }
            ElementKind::Slot => {
                let slot = msg
                    .meta
                    .protected_slots
                    .iter()
                    .find(|s| s.name == el.value)
                    .ok_or_else(|| vec![Fault::catalog("io-error")])?;
                out.push_str(&slot.value);
            }
            ElementKind::Arg => {
                let arg_spec = msg.meta.arguments.iter().find(|a| a.name == el.value);
                let val = inputs.get(&el.value);
                if let Some(v) = val {
                    if let Some(s) = v.as_str() {
                        out.push_str(s);
                    } else if let Some(b) = v.as_bool() {
                        out.push_str(if b { "true" } else { "false" });
                    } else if let Some(n) = v.as_i64() {
                        if arg_spec.and_then(|a| a.numeric_style)
                            == Some(NumericStyle::LocaleFormatted)
                        {
                            let formatted = oracle::format_fluent_number(cardinal_rule, n)
                                .map_err(|f| vec![f])?;
                            out.push_str(&formatted);
                        } else {
                            out.push_str(&oracle::format_integer_ungrouped(n));
                        }
                    }
                }
            }
        }
    }

    Ok(out)
}

pub(crate) fn format_adapter_call(
    target: Target,
    bundle_id: &str,
    locale: &str,
    msg: &TargetMessage,
    inputs: &Map<String, JsonValue>,
) -> String {
    match target {
        Target::Rust => {
            let mut args_str = String::new();
            for (k, v) in inputs {
                if !args_str.is_empty() {
                    args_str.push_str(", ");
                }
                if let Some(s) = v.as_str() {
                    args_str.push_str(&format!("(\"{}\", Value::String(\"{}\"))", k, s));
                } else if let Some(b) = v.as_bool() {
                    args_str.push_str(&format!("(\"{}\", Value::Boolean({}))", k, b));
                } else if let Some(n) = v.as_i64() {
                    let arg_spec = msg.meta.arguments.iter().find(|a| a.name == *k);
                    if arg_spec.map(|a| a.arg_type) == Some(ScalarType::Cardinal) {
                        args_str.push_str(&format!("(\"{}\", Value::Cardinal({}))", k, n));
                    } else {
                        args_str.push_str(&format!("(\"{}\", Value::Integer({}))", k, n));
                    }
                }
            }
            format!("render(\"{}\", \"{}\", &[{}])", locale, msg.id, args_str)
        }
        Target::Esm => {
            let mut args_str = String::new();
            for (k, v) in inputs {
                if !args_str.is_empty() {
                    args_str.push_str(", ");
                }
                args_str.push_str(&format!("[\"{}\", {}]", k, v));
            }
            format!("render(\"{}\", \"{}\", [{}])", locale, msg.id, args_str)
        }
        Target::Apple => {
            if msg.meta.format_intent == FormatIntent::Cardinal {
                let sel_name = msg
                    .meta
                    .selector
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or("count");
                let sel_arg = msg.meta.arguments.iter().find(|a| a.name == sel_name);
                let is_locale_formatted = sel_arg
                    .map(|a| a.numeric_style == Some(NumericStyle::LocaleFormatted))
                    .unwrap_or(false);

                let mut args_list = Vec::new();
                for arg in &msg.meta.arguments {
                    if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("\"{}\"", s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!("{}", b));
                        } else if let Some(n) = v.as_i64() {
                            if arg.name == sel_name
                                || arg.numeric_style == Some(NumericStyle::LocaleFormatted)
                            {
                                args_list.push(format!("{}", n));
                            } else {
                                args_list
                                    .push(format!("\"{}\"", oracle::format_integer_ungrouped(n)));
                            }
                        }
                    }
                }
                if !is_locale_formatted {
                    let count_val = inputs.get(sel_name).and_then(|v| v.as_i64()).unwrap_or(0);
                    let ungrouped = oracle::format_integer_ungrouped(count_val);
                    args_list.push(format!("\"{}\"", ungrouped));
                }
                if args_list.is_empty() {
                    format!("String(format:locale:arguments: \"{}\")", msg.resource_key)
                } else {
                    format!(
                        "String(format:locale:arguments: \"{}\", {})",
                        msg.resource_key,
                        args_list.join(", ")
                    )
                }
            } else if msg.meta.format_intent == FormatIntent::Enum {
                let sel = msg.meta.selector.as_ref().unwrap();
                let def = sel.enum_default.as_deref().unwrap_or("");
                let val = inputs
                    .get(&sel.name)
                    .and_then(|v| v.as_str())
                    .unwrap_or(def);
                let res_key =
                    crate::target::model::enum_variant_resource_key(&msg.resource_key, val);
                let mut args_list = Vec::new();
                for arg in &msg.meta.arguments {
                    if arg.name == sel.name {
                        continue;
                    }
                    if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("\"{}\"", s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!("{}", b));
                        } else if let Some(n) = v.as_i64() {
                            if arg.numeric_style == Some(NumericStyle::LocaleFormatted) {
                                args_list.push(format!("{}", n));
                            } else {
                                args_list
                                    .push(format!("\"{}\"", oracle::format_integer_ungrouped(n)));
                            }
                        }
                    }
                }
                if args_list.is_empty() {
                    format!("String(format:locale:arguments: \"{}\")", res_key)
                } else {
                    format!(
                        "String(format:locale:arguments: \"{}\", {})",
                        res_key,
                        args_list.join(", ")
                    )
                }
            } else {
                let mut args_list = Vec::new();
                for arg in &msg.meta.arguments {
                    if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("\"{}\"", s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!("{}", b));
                        } else if let Some(n) = v.as_i64() {
                            if arg.numeric_style == Some(NumericStyle::LocaleFormatted) {
                                args_list.push(format!("{}", n));
                            } else {
                                args_list
                                    .push(format!("\"{}\"", oracle::format_integer_ungrouped(n)));
                            }
                        }
                    }
                }
                if args_list.is_empty() {
                    format!("String(format:locale:arguments: \"{}\")", msg.resource_key)
                } else {
                    format!(
                        "String(format:locale:arguments: \"{}\", {})",
                        msg.resource_key,
                        args_list.join(", ")
                    )
                }
            }
        }
        Target::Android => {
            format!("Select.resolveResourceKey(\"{}\")", msg.id)
        }
        Target::Webextension => {
            format!("chrome.i18n.getMessage(\"{}\")", msg.resource_key)
        }
        Target::Gettext => {
            format!("dgettext(\"{}\", \"{}\")", bundle_id, msg.resource_key)
        }
    }
}
