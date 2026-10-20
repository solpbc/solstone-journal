// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::target::ftl::generate_bundle_ftl;
use crate::target::model::TargetBundle;
use crate::vocab::{FormatIntent, ScalarType};
use std::fs;
use std::path::Path;

pub fn generate_rust_target(bundle: &TargetBundle, dest_dir: &Path) -> Result<(), Vec<Fault>> {
    for msg in &bundle.messages {
        for arg in &msg.meta.arguments {
            if arg.numeric_style == Some(crate::vocab::NumericStyle::LocaleFormatted) {
                return Err(vec![Fault::new("locale-formatted", "*", &msg.id)]);
            }
        }
    }

    let bundle_dir = dest_dir.join("rust").join(&bundle.id);
    fs::create_dir_all(&bundle_dir).map_err(|_| vec![Fault::catalog("io-error")])?;

    // 1. Generate .ftl for each locale
    for loc in &bundle.locales {
        let ftl_content = generate_bundle_ftl(bundle, &loc.tag).map_err(|f| vec![f])?;
        let ftl_path = bundle_dir.join(format!("{}.ftl", loc.tag));
        fs::write(&ftl_path, ftl_content.as_bytes())
            .map_err(|_| vec![Fault::catalog("io-error")])?;
    }

    // 2. Generate api.rs
    let api_code = generate_rust_api_code(bundle).map_err(|f| vec![f])?;
    let api_path = bundle_dir.join("api.rs");
    fs::write(&api_path, api_code.as_bytes()).map_err(|_| vec![Fault::catalog("io-error")])?;

    Ok(())
}

fn generate_rust_api_code(bundle: &TargetBundle) -> Result<String, Fault> {
    let mut s = String::new();
    s.push_str("// SPDX-License-Identifier: AGPL-3.0-only\n");
    s.push_str("// Copyright (c) 2026 sol pbc\n\n");
    s.push_str("use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};\n");
    s.push_str("use std::collections::HashSet;\n\n");

    s.push_str("#[derive(Clone, Debug, PartialEq)]\n");
    s.push_str("pub enum Value<'a> {\n");
    s.push_str("    String(&'a str),\n");
    s.push_str("    Boolean(bool),\n");
    s.push_str("    Integer(i64),\n");
    s.push_str("    Cardinal(i64),\n");
    s.push_str("}\n\n");

    s.push_str("#[derive(Clone, Debug, Eq, PartialEq)]\n");
    s.push_str("pub enum RenderError {\n");
    s.push_str("    UnknownLocale,\n");
    s.push_str("    UnknownMessage,\n");
    s.push_str("    MissingArgument(String),\n");
    s.push_str("    UnknownArgument(String),\n");
    s.push_str("    DuplicateArgument(String),\n");
    s.push_str("    InvalidArgumentType(String),\n");
    s.push_str("    OutOfRange(String),\n");
    s.push_str("    FormatFailed,\n");
    s.push_str("}\n\n");

    // MessageId enum
    s.push_str("#[allow(non_camel_case_types)]\n");
    s.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]\n");
    s.push_str("pub enum MessageId {\n");
    for msg in &bundle.messages {
        s.push_str(&format!("    {},\n", msg.resource_key));
    }
    s.push_str("}\n\n");

    s.push_str("pub fn render_dotted(locale: &str, dotted_id: &str, args: &[(&str, Value)]) -> Result<String, RenderError> {\n");
    s.push_str("    let id = match dotted_id {\n");
    for msg in &bundle.messages {
        s.push_str(&format!(
            "        \"{}\" => MessageId::{},\n",
            msg.id, msg.resource_key
        ));
    }
    s.push_str("        _ => return Err(RenderError::UnknownMessage),\n");
    s.push_str("    };\n");
    s.push_str("    render(locale, id, args)\n");
    s.push_str("}\n\n");

    s.push_str("pub fn render(locale: &str, id: MessageId, args: &[(&str, Value)]) -> Result<String, RenderError> {\n");
    s.push_str("    let message_id = id;\n\n");

    // Check duplicate args
    s.push_str("    let mut seen = HashSet::new();\n");
    s.push_str("    for (name, _) in args {\n");
    s.push_str("        if !seen.insert(*name) {\n");
    s.push_str("            return Err(RenderError::DuplicateArgument((*name).to_string()));\n");
    s.push_str("        }\n");
    s.push_str("    }\n\n");

    // Match locale to get FTL content and cardinal rule
    s.push_str("    let (ftl_content, cardinal_rule) = match locale {\n");
    for loc in &bundle.locales {
        let card_opt = match &loc.cardinal {
            Some(c) => format!("Some(\"{}\")", c),
            None => "None".to_string(),
        };
        s.push_str(&format!(
            "        \"{}\" => (include_str!(\"{}.ftl\"), {}),\n",
            loc.tag, loc.tag, card_opt
        ));
    }
    s.push_str("        _ => return Err(RenderError::UnknownLocale),\n");
    s.push_str("    };\n\n");

    // Match message_id
    s.push_str("    let (res_key, fluent_args) = match message_id {\n");
    for msg in &bundle.messages {
        s.push_str(&format!("        MessageId::{} => {{\n", msg.resource_key));

        // Check declared args
        s.push_str("            const DECLARED: &[&str] = &[");
        for (i, arg) in msg.meta.arguments.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&format!("\"{}\"", arg.name));
        }
        s.push_str("];\n");
        s.push_str("            for (name, _) in args {\n");
        s.push_str("                if !DECLARED.contains(name) {\n");
        s.push_str(
            "                    return Err(RenderError::UnknownArgument((*name).to_string()));\n",
        );
        s.push_str("                }\n");
        s.push_str("            }\n");

        s.push_str("            #[allow(unused_mut)]\n");
        s.push_str("            let mut f_args = FluentArgs::new();\n");

        for (idx, arg) in msg.meta.arguments.iter().enumerate() {
            let val_name = format!("val_v{}", idx);
            let var_name = format!("v{}", idx);
            s.push_str(&format!(
                "            let {} = args.iter().find(|(n, _)| *n == \"{}\").ok_or_else(|| RenderError::MissingArgument(\"{}\".to_string()))?.1.clone();\n",
                val_name, arg.name, arg.name
            ));

            match arg.arg_type {
                ScalarType::String => {
                    s.push_str(&format!(
                        "            let {} = match {} {{\n",
                        var_name, val_name
                    ));
                    s.push_str("                Value::String(s) => s.to_string(),\n");
                    s.push_str(&format!("                _ => return Err(RenderError::InvalidArgumentType(\"{}\".to_string())),\n", arg.name));
                    s.push_str("            };\n");
                    s.push_str(&format!(
                        "            f_args.set(\"{}\", FluentValue::from({}.clone()));\n",
                        arg.name, var_name
                    ));
                }
                ScalarType::Boolean => {
                    s.push_str(&format!(
                        "            let {} = match {} {{\n",
                        var_name, val_name
                    ));
                    s.push_str("                Value::Boolean(b) => b,\n");
                    s.push_str(&format!("                _ => return Err(RenderError::InvalidArgumentType(\"{}\".to_string())),\n", arg.name));
                    s.push_str("            };\n");
                    s.push_str(&format!("            f_args.set(\"{}\", FluentValue::from(if {} {{ \"true\" }} else {{ \"false\" }}));\n", arg.name, var_name));
                }
                ScalarType::Integer => {
                    s.push_str(&format!(
                        "            let {} = match {} {{\n",
                        var_name, val_name
                    ));
                    s.push_str("                Value::Integer(i) => i,\n");
                    s.push_str(&format!("                _ => return Err(RenderError::InvalidArgumentType(\"{}\".to_string())),\n", arg.name));
                    s.push_str("            };\n");
                    const INT_MIN: i64 = -9007199254740991;
                    const INT_MAX: i64 = 9007199254740991;
                    let eff_min = arg.min.map(|m| m as i64).unwrap_or(INT_MIN).max(INT_MIN);
                    let eff_max = arg.max.map(|m| m as i64).unwrap_or(INT_MAX).min(INT_MAX);
                    s.push_str(&format!("            if {} < {}_i64 || {} > {}_i64 {{ return Err(RenderError::OutOfRange(\"{}\".to_string())); }}\n", var_name, eff_min, var_name, eff_max, arg.name));
                    s.push_str(&format!(
                        "            f_args.set(\"{}\", FluentValue::from({}));\n",
                        arg.name, var_name
                    ));
                }
                ScalarType::Cardinal => {
                    s.push_str(&format!(
                        "            let {} = match {} {{\n",
                        var_name, val_name
                    ));
                    s.push_str("                Value::Cardinal(c) => c,\n");
                    s.push_str(&format!("                _ => return Err(RenderError::InvalidArgumentType(\"{}\".to_string())),\n", arg.name));
                    s.push_str("            };\n");
                    const CARD_MIN: i64 = -2147483648;
                    const CARD_MAX: i64 = 2147483647;
                    let eff_min = arg.min.map(|m| m as i64).unwrap_or(CARD_MIN).max(CARD_MIN);
                    let eff_max = arg.max.map(|m| m as i64).unwrap_or(CARD_MAX).min(CARD_MAX);
                    s.push_str(&format!("            if {} < {}_i64 || {} > {}_i64 {{ return Err(RenderError::OutOfRange(\"{}\".to_string())); }}\n", var_name, eff_min, var_name, eff_max, arg.name));
                    s.push_str(&format!(
                        "            f_args.set(\"{}\", FluentValue::from({}));\n",
                        arg.name, var_name
                    ));
                }
            }
        }

        if msg.meta.format_intent == FormatIntent::Enum {
            let sel = msg.meta.selector.as_ref().unwrap();
            let def_val = sel
                .enum_default
                .as_deref()
                .ok_or_else(|| Fault::new("enum-variant", "*", &msg.id))?;
            let sel_idx = msg
                .meta
                .arguments
                .iter()
                .position(|a| a.name == sel.name)
                .ok_or_else(|| Fault::catalog("invalid-metadata"))?;
            s.push_str(&format!(
                "            let target_key = match v{}.as_str() {{\n",
                sel_idx
            ));
            for val in &sel.enum_values {
                let var_key =
                    crate::target::model::enum_variant_resource_key(&msg.resource_key, val);
                s.push_str(&format!(
                    "                \"{}\" => \"{}\",\n",
                    val, var_key
                ));
            }
            let def_key =
                crate::target::model::enum_variant_resource_key(&msg.resource_key, def_val);
            s.push_str(&format!("                _ => \"{}\",\n", def_key));
            s.push_str("            };\n");
            s.push_str("            (target_key.to_string(), f_args)\n");
        } else {
            s.push_str(&format!(
                "            (\"{}\".to_string(), f_args)\n",
                msg.resource_key
            ));
        }

        s.push_str("        }\n");
    }
    s.push_str("    };\n\n");

    // Bundle creation and formatting (Fluent default isolation is preserved!)
    s.push_str("    let lang_str = cardinal_rule.unwrap_or(\"und\");\n");
    s.push_str("    let lang = lang_str.parse().map_err(|_| RenderError::FormatFailed)?;\n");
    s.push_str("    let mut bundle = FluentBundle::new(vec![lang]);\n");
    s.push_str("    let _ = bundle.add_builtins();\n");
    s.push_str("    let res = FluentResource::try_new(ftl_content.to_string()).map_err(|_| RenderError::FormatFailed)?;\n");
    s.push_str("    bundle.add_resource(res).map_err(|_| RenderError::FormatFailed)?;\n\n");

    s.push_str("    let msg = bundle.get_message(&res_key).ok_or(RenderError::FormatFailed)?;\n");
    s.push_str("    let pattern = msg.value().ok_or(RenderError::FormatFailed)?;\n");
    s.push_str("    let mut errs = vec![];\n");
    s.push_str(
        "    let formatted = bundle.format_pattern(pattern, Some(&fluent_args), &mut errs);\n",
    );
    s.push_str("    if !errs.is_empty() {\n");
    s.push_str("        return Err(RenderError::FormatFailed);\n");
    s.push_str("    }\n");
    s.push_str("    Ok(formatted.into_owned())\n");
    s.push_str("}\n");

    Ok(s)
}
