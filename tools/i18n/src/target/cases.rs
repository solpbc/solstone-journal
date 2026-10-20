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
    pub expected_exact: bool,
    pub call_contract: JsonValue,
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
        map.insert("own_reader_call".to_string(), json!(self.adapter_call));

        // Sort inputs keys
        let mut sorted_inputs = Map::new();
        let mut keys: Vec<String> = self.inputs.keys().cloned().collect();
        keys.sort();
        for k in keys {
            sorted_inputs.insert(k.clone(), self.inputs[&k].clone());
        }
        map.insert("inputs".to_string(), JsonValue::Object(sorted_inputs));
        map.insert("logical_expected".to_string(), json!(self.expected));
        map.insert(
            "expected".to_string(),
            if self.expected_exact {
                json!(self.expected)
            } else {
                JsonValue::Null
            },
        );
        map.insert(
            "expectation_kind".to_string(),
            json!(if self.expected_exact {
                "resource-golden"
            } else {
                "native-reader-required"
            }),
        );
        map.insert("call_contract".to_string(), self.call_contract.clone());
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
                        let expected_exact =
                            tgt != Target::Esm
                                && !msg.meta.arguments.iter().any(|a| {
                                    a.numeric_style == Some(NumericStyle::LocaleFormatted)
                                });
                        let expected = if tgt == Target::Rust {
                            rust_resource_expected(bundle, msg, &loc.tag, &inputs)?
                        } else {
                            expected
                        };
                        let call_contract =
                            native_call_contract(tgt, &bundle.id, &loc.tag, msg, &inputs);

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
                            expected_exact,
                            call_contract,
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
                        let sample = msg
                            .meta
                            .selector
                            .as_ref()
                            .filter(|s| s.name == arg.name)
                            .and_then(|s| s.enum_default.as_deref())
                            .unwrap_or("sample");
                        inputs.insert(arg.name.clone(), json!(sample));
                    }
                    ScalarType::Boolean => {
                        inputs.insert(arg.name.clone(), json!(true));
                    }
                    ScalarType::Integer => {
                        let val = 0_i64.clamp(
                            arg.min.map(|m| m as i64).unwrap_or(-9007199254740991),
                            arg.max.map(|m| m as i64).unwrap_or(9007199254740991),
                        );
                        inputs.insert(arg.name.clone(), json!(val));
                    }
                    ScalarType::Cardinal => {
                        let val = 0_i64.clamp(
                            arg.min.map(|m| m as i64).unwrap_or(i32::MIN as i64),
                            arg.max.map(|m| m as i64).unwrap_or(i32::MAX as i64),
                        );
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
                    out.push('\u{2068}');
                    if let Some(s) = v.as_str() {
                        out.push_str(s);
                    } else if let Some(b) = v.as_bool() {
                        out.push_str(if b { "true" } else { "false" });
                    } else if let Some(n) = v.as_i64() {
                        // This is the logical scalar value, not a native formatting oracle.
                        let _ = arg_spec;
                        out.push_str(&oracle::format_integer_ungrouped(n));
                    }
                    out.push('\u{2069}');
                }
            }
        }
    }

    Ok(out)
}

fn resolved_identity(msg: &TargetMessage, inputs: &Map<String, JsonValue>) -> (String, String) {
    if msg.meta.format_intent != FormatIntent::Enum {
        return (msg.resource_key.clone(), msg.id.clone());
    }
    let selector = msg.meta.selector.as_ref().expect("validated selector");
    let supplied = inputs.get(&selector.name).and_then(JsonValue::as_str);
    let selected = supplied
        .filter(|v| selector.enum_values.iter().any(|e| e == v))
        .unwrap_or_else(|| selector.enum_default.as_deref().expect("validated default"));
    (
        crate::target::model::enum_variant_resource_key(&msg.resource_key, selected),
        format!("{}.{}", msg.id, selected),
    )
}

fn rust_resource_expected(
    bundle: &TargetBundle,
    msg: &TargetMessage,
    locale: &str,
    inputs: &Map<String, JsonValue>,
) -> Result<String, Vec<Fault>> {
    use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};
    let entry = bundle
        .locales
        .iter()
        .find(|l| l.tag == locale)
        .expect("selected locale");
    let lang = entry
        .cardinal
        .as_deref()
        .unwrap_or("und")
        .parse()
        .map_err(|_| vec![Fault::catalog("plural-rule")])?;
    let mut reader = FluentBundle::new(vec![lang]);
    reader
        .add_builtins()
        .map_err(|_| vec![Fault::catalog("plural-rule")])?;
    let ftl = crate::target::ftl::generate_bundle_ftl(bundle, locale).map_err(|f| vec![f])?;
    let resource =
        FluentResource::try_new(ftl).map_err(|_| vec![Fault::catalog("resource-format")])?;
    reader
        .add_resource(resource)
        .map_err(|_| vec![Fault::catalog("resource-format")])?;
    let mut args = FluentArgs::new();
    for (name, value) in inputs {
        if let Some(v) = value.as_str() {
            args.set(name, v.to_string());
        } else if let Some(v) = value.as_bool() {
            args.set(name, if v { "true" } else { "false" });
        } else if let Some(v) = value.as_i64() {
            args.set(name, v);
        }
    }
    let (key, _) = resolved_identity(msg, inputs);
    let pattern = reader
        .get_message(&key)
        .and_then(|m| m.value())
        .ok_or_else(|| vec![Fault::catalog("resource-format")])?;
    let mut errors = Vec::new();
    let rendered = reader
        .format_pattern(pattern, Some(&args), &mut errors)
        .into_owned();
    if !errors.is_empty() {
        return Err(vec![Fault::catalog("resource-format")]);
    }
    Ok(rendered)
}

pub(crate) fn native_call_contract(
    target: Target,
    bundle: &str,
    locale: &str,
    msg: &TargetMessage,
    inputs: &Map<String, JsonValue>,
) -> JsonValue {
    let (key, context) = resolved_identity(msg, inputs);
    let selector = msg.meta.selector.as_ref();
    let quantity = selector
        .and_then(|s| inputs.get(&s.name))
        .and_then(JsonValue::as_i64);
    let mut arguments = Vec::new();
    for arg in &msg.meta.arguments {
        let value = &inputs[&arg.name];
        let numeric = value.as_i64();
        let display = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        let is_quantity = msg.meta.format_intent == FormatIntent::Cardinal
            && selector.is_some_and(|s| s.name == arg.name);
        let item = match target {
            Target::Apple
                if numeric.is_some()
                    && (is_quantity
                        || arg.numeric_style == Some(NumericStyle::LocaleFormatted)) =>
            {
                json!({"type":"integer","integer":numeric.unwrap()})
            }
            Target::Apple | Target::Gettext => json!({"type":"string","string":display}),
            Target::Android if arg.numeric_style == Some(NumericStyle::LocaleFormatted) => {
                json!({"type":"string","format":"Select.formatIntegerLocale","value":value,"locale":locale})
            }
            _ => json!({"name":arg.name,"type":arg.arg_type.as_str(),"value":value}),
        };
        arguments.push(item);
    }
    if target == Target::Apple && msg.meta.format_intent == FormatIntent::Cardinal {
        let sel = selector.expect("validated selector");
        if msg
            .meta
            .arguments
            .iter()
            .find(|a| a.name == sel.name)
            .and_then(|a| a.numeric_style)
            == Some(NumericStyle::Ungrouped)
        {
            arguments.push(
                json!({"type":"string","string":quantity.expect("validated quantity").to_string()}),
            );
        }
    }
    let entry = match target {
        Target::Rust => "render(locale, MessageId, named_args)",
        Target::Esm => "render(locale, dotted_id, named_args)",
        Target::Apple => "Bundle.localizedString then String(format:locale:arguments:)",
        Target::Android if msg.meta.format_intent == FormatIntent::Cardinal => {
            "Resources.getQuantityString"
        }
        Target::Android => "Resources.getString",
        Target::Gettext if msg.meta.format_intent == FormatIntent::Cardinal => {
            "dngettext then snprintf"
        }
        Target::Gettext => "dgettext then snprintf",
        Target::Webextension => "chrome.runtime.getManifest via __MSG_resource_key__",
    };
    json!({"entry":entry,"bundle":bundle,"locale":locale,"resource_key":key,"message_id":msg.id,
           "enum_argument":selector.and_then(|s| inputs.get(&s.name)).filter(|v|v.is_string()),
           "arguments":arguments,"signed_quantity":quantity,
           "gettext_lookup_key":format!("{}\u{4}{}",context,key),
           "gettext_plural_key":format!("{}_plural",key),"gettext_unsigned_quantity":quantity.map(i64::unsigned_abs),
           "format_passes":1})
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
                for (pos, arg) in msg.meta.arguments.iter().enumerate() {
                    let idx = pos + 1;
                    if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("{}:String(\"{}\")", idx, s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!(
                                "{}:String(\"{}\")",
                                idx,
                                if b { "true" } else { "false" }
                            ));
                        } else if let Some(n) = v.as_i64() {
                            if arg.name == sel_name
                                || arg.numeric_style == Some(NumericStyle::LocaleFormatted)
                            {
                                args_list.push(format!("{}:Int64({})", idx, n));
                            } else {
                                args_list.push(format!(
                                    "{}:String(\"{}\")",
                                    idx,
                                    oracle::format_integer_ungrouped(n)
                                ));
                            }
                        }
                    }
                }
                if !is_locale_formatted {
                    let count_val = inputs.get(sel_name).and_then(|v| v.as_i64()).unwrap_or(0);
                    let ungrouped = oracle::format_integer_ungrouped(count_val);
                    let display_pos = msg.meta.arguments.len() + 1;
                    args_list.push(format!("{}:String(\"{}\")", display_pos, ungrouped));
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
                let def = sel.enum_default.as_deref().expect("enum must have default");
                let input_val = inputs.get(&sel.name).and_then(|v| v.as_str());
                let resolved = match input_val {
                    Some(v) if sel.enum_values.iter().any(|ev| ev == v) => v,
                    _ => def,
                };
                let res_key =
                    crate::target::model::enum_variant_resource_key(&msg.resource_key, resolved);

                let parsed = msg
                    .translations
                    .get(locale)
                    .or_else(|| msg.translations.values().next());
                let selected_branch = parsed.and_then(|p| {
                    p.branches
                        .iter()
                        .find(|b| b.key.as_deref() == Some(resolved))
                        .or_else(|| p.branches.iter().find(|b| b.key.as_deref() == Some(def)))
                        .or_else(|| p.branches.iter().find(|b| b.default))
                });
                let branch_has_arg = |name: &str| -> bool {
                    selected_branch
                        .map(|b| {
                            b.pattern
                                .iter()
                                .any(|el| el.kind == ElementKind::Arg && el.value == name)
                        })
                        .unwrap_or(false)
                };

                let mut args_list = Vec::new();
                for (pos, arg) in msg.meta.arguments.iter().enumerate() {
                    let idx = pos + 1;
                    if arg.name == sel.name {
                        if branch_has_arg(&arg.name) {
                            let val_str = input_val.unwrap_or(resolved);
                            args_list.push(format!("{}:String(\"{}\")", idx, val_str));
                        }
                    } else if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("{}:String(\"{}\")", idx, s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!(
                                "{}:String(\"{}\")",
                                idx,
                                if b { "true" } else { "false" }
                            ));
                        } else if let Some(n) = v.as_i64() {
                            if arg.numeric_style == Some(NumericStyle::LocaleFormatted) {
                                args_list.push(format!("{}:Int64({})", idx, n));
                            } else {
                                args_list.push(format!(
                                    "{}:String(\"{}\")",
                                    idx,
                                    oracle::format_integer_ungrouped(n)
                                ));
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
                for (pos, arg) in msg.meta.arguments.iter().enumerate() {
                    let idx = pos + 1;
                    if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("{}:String(\"{}\")", idx, s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!(
                                "{}:String(\"{}\")",
                                idx,
                                if b { "true" } else { "false" }
                            ));
                        } else if let Some(n) = v.as_i64() {
                            if arg.numeric_style == Some(NumericStyle::LocaleFormatted) {
                                args_list.push(format!("{}:Int64({})", idx, n));
                            } else {
                                args_list.push(format!(
                                    "{}:String(\"{}\")",
                                    idx,
                                    oracle::format_integer_ungrouped(n)
                                ));
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
            if msg.meta.format_intent == FormatIntent::Cardinal {
                let sel_name = msg
                    .meta
                    .selector
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or("count");
                let count_val = inputs.get(sel_name).and_then(|v| v.as_i64()).unwrap_or(0);

                let mut args_list = Vec::new();
                for arg in &msg.meta.arguments {
                    if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("\"{}\"", s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!("\"{}\"", if b { "true" } else { "false" }));
                        } else if let Some(n) = v.as_i64() {
                            if arg.numeric_style == Some(NumericStyle::LocaleFormatted) {
                                args_list
                                    .push(format!("formatIntegerLocale({}, \"{}\")", n, locale));
                            } else {
                                args_list.push(format!("{}", n));
                            }
                        }
                    }
                }
                if args_list.is_empty() {
                    format!(
                        "getQuantityString(R.plurals.{}, {})",
                        msg.resource_key, count_val
                    )
                } else {
                    format!(
                        "getQuantityString(R.plurals.{}, {}, {})",
                        msg.resource_key,
                        count_val,
                        args_list.join(", ")
                    )
                }
            } else if msg.meta.format_intent == FormatIntent::Enum {
                let sel = msg.meta.selector.as_ref().unwrap();
                let def = sel.enum_default.as_deref().expect("enum must have default");
                let input_val = inputs.get(&sel.name).and_then(|v| v.as_str());
                let resolved = match input_val {
                    Some(v) if sel.enum_values.iter().any(|ev| ev == v) => v,
                    _ => def,
                };
                let res_key =
                    crate::target::model::enum_variant_resource_key(&msg.resource_key, resolved);

                let parsed = msg
                    .translations
                    .get(locale)
                    .or_else(|| msg.translations.values().next());
                let selected_branch = parsed.and_then(|p| {
                    p.branches
                        .iter()
                        .find(|b| b.key.as_deref() == Some(resolved))
                        .or_else(|| p.branches.iter().find(|b| b.key.as_deref() == Some(def)))
                        .or_else(|| p.branches.iter().find(|b| b.default))
                });
                let branch_has_arg = |name: &str| -> bool {
                    selected_branch
                        .map(|b| {
                            b.pattern
                                .iter()
                                .any(|el| el.kind == ElementKind::Arg && el.value == name)
                        })
                        .unwrap_or(false)
                };

                let mut args_list = Vec::new();
                for arg in &msg.meta.arguments {
                    if arg.name == sel.name {
                        if branch_has_arg(&arg.name) {
                            let val_str = input_val.unwrap_or(resolved);
                            args_list.push(format!("\"{}\"", val_str));
                        }
                    } else if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("\"{}\"", s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!("\"{}\"", if b { "true" } else { "false" }));
                        } else if let Some(n) = v.as_i64() {
                            if arg.numeric_style == Some(NumericStyle::LocaleFormatted) {
                                args_list
                                    .push(format!("formatIntegerLocale({}, \"{}\")", n, locale));
                            } else {
                                args_list.push(format!("{}", n));
                            }
                        }
                    }
                }
                if args_list.is_empty() {
                    format!("getString(R.string.{})", res_key)
                } else {
                    format!("getString(R.string.{}, {})", res_key, args_list.join(", "))
                }
            } else {
                let mut args_list = Vec::new();
                for arg in &msg.meta.arguments {
                    if let Some(v) = inputs.get(&arg.name) {
                        if let Some(s) = v.as_str() {
                            args_list.push(format!("\"{}\"", s));
                        } else if let Some(b) = v.as_bool() {
                            args_list.push(format!("\"{}\"", if b { "true" } else { "false" }));
                        } else if let Some(n) = v.as_i64() {
                            if arg.numeric_style == Some(NumericStyle::LocaleFormatted) {
                                args_list
                                    .push(format!("formatIntegerLocale({}, \"{}\")", n, locale));
                            } else {
                                args_list.push(format!("{}", n));
                            }
                        }
                    }
                }
                if args_list.is_empty() {
                    format!("getString(R.string.{})", msg.resource_key)
                } else {
                    format!(
                        "getString(R.string.{}, {})",
                        msg.resource_key,
                        args_list.join(", ")
                    )
                }
            }
        }
        Target::Webextension => {
            format!("chrome.runtime.getManifest() __MSG_{}__", msg.resource_key)
        }
        Target::Gettext => {
            format!("dgettext(\"{}\", \"{}\")", bundle_id, msg.resource_key)
        }
    }
}
