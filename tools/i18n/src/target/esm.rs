// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::target::ftl::generate_bundle_ftl;
use crate::target::model::TargetBundle;
use std::fs;
use std::path::Path;

pub fn generate_esm_target(
    bundle: &TargetBundle,
    dest_dir: &Path,
    vendor_src_dir: &Path,
) -> Result<(), Vec<Fault>> {
    let mut faults = Vec::new();
    for msg in &bundle.messages {
        let has_locale_formatted = msg
            .meta
            .arguments
            .iter()
            .any(|a| a.numeric_style == Some(crate::vocab::NumericStyle::LocaleFormatted));
        if has_locale_formatted {
            for loc in &bundle.locales {
                if !crate::locale::is_standard_formatting_locale(&loc.tag) {
                    faults.push(Fault::new("locale-formatted", &loc.tag, &msg.id));
                }
            }
        }
    }
    if !faults.is_empty() {
        return Err(faults);
    }

    let esm_root = dest_dir.join("esm");
    let bundle_dir = esm_root.join(&bundle.id);
    fs::create_dir_all(&bundle_dir).map_err(|_| vec![Fault::catalog("io-error")])?;

    // 1. Copy vendored fluent-js tree to esm/vendor/fluent-bundle/
    let vendor_dest = esm_root.join("vendor").join("fluent-bundle");
    if !vendor_dest.exists() {
        fs::create_dir_all(&vendor_dest).map_err(|_| vec![Fault::catalog("io-error")])?;
        copy_dir_recursive(vendor_src_dir, &vendor_dest)
            .map_err(|_| vec![Fault::catalog("io-error")])?;
    }

    // 2. Generate .ftl for each locale and catalogs.mjs
    let mut catalogs_mjs = String::new();
    catalogs_mjs.push_str("// SPDX-License-Identifier: AGPL-3.0-only\n");
    catalogs_mjs.push_str("// Copyright (c) 2026 sol pbc\n\n");
    catalogs_mjs.push_str("export const CATALOGS = {\n");
    for loc in &bundle.locales {
        let ftl_content = generate_bundle_ftl(bundle, &loc.tag).map_err(|f| vec![f])?;
        let ftl_path = bundle_dir.join(format!("{}.ftl", loc.tag));
        fs::write(&ftl_path, ftl_content.as_bytes())
            .map_err(|_| vec![Fault::catalog("io-error")])?;
        catalogs_mjs.push_str(&format!(
            "  {}: {},\n",
            serde_json::to_string(&loc.tag).unwrap(),
            serde_json::to_string(&ftl_content).unwrap()
        ));
    }
    catalogs_mjs.push_str("};\n");
    let catalogs_path = bundle_dir.join("catalogs.mjs");
    fs::write(&catalogs_path, catalogs_mjs.as_bytes())
        .map_err(|_| vec![Fault::catalog("io-error")])?;

    // 3. Generate api.mjs
    let api_code = generate_esm_api_code(bundle)?;
    let api_path = bundle_dir.join("api.mjs");
    fs::write(&api_path, api_code.as_bytes()).map_err(|_| vec![Fault::catalog("io-error")])?;

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let dest_child = dst.join(&file_name);
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_child)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), dest_child)?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported vendored file kind",
            ));
        }
    }
    Ok(())
}

fn generate_esm_api_code(bundle: &TargetBundle) -> Result<String, Vec<Fault>> {
    let mut s = String::new();
    s.push_str("// SPDX-License-Identifier: AGPL-3.0-only\n");
    s.push_str("// Copyright (c) 2026 sol pbc\n\n");
    s.push_str(
        "import { FluentBundle, FluentResource } from \"../vendor/fluent-bundle/index.js\";\n",
    );
    s.push_str("import { CATALOGS } from \"./catalogs.mjs\";\n\n");

    s.push_str("export function cardinal(val) { return { type: \"cardinal\", value: val }; }\n");
    s.push_str("export function integer(val) { return { type: \"integer\", value: val }; }\n\n");

    // Metadata table
    s.push_str("const LOCALES = {\n");
    for loc in &bundle.locales {
        let card = loc.cardinal.as_deref().unwrap_or("und");
        let bundle_locale = if crate::locale::is_standard_formatting_locale(&loc.tag) {
            loc.tag.as_str()
        } else {
            card
        };
        s.push_str(&format!(
            "  \"{}\": {{ bundleLocale: \"{}\", cardinal: \"{}\" }},\n",
            loc.tag, bundle_locale, card
        ));
    }
    s.push_str("};\n\n");

    s.push_str("const MESSAGES = {\n");
    for msg in &bundle.messages {
        s.push_str(&format!("  \"{}\": {{\n", msg.id));
        s.push_str(&format!("    resourceKey: \"{}\",\n", msg.resource_key));
        s.push_str(&format!(
            "    formatIntent: \"{}\",\n",
            msg.meta.format_intent.as_str()
        ));
        if msg.meta.format_intent == crate::vocab::FormatIntent::Enum
            && let Some(sel) = &msg.meta.selector
        {
            s.push_str(&format!("    selectorVar: \"{}\",\n", sel.name));
            s.push_str("    enumValues: [");
            for (i, v) in sel.enum_values.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&format!("\"{}\"", v));
            }
            s.push_str("],\n");
            let def = sel
                .enum_default
                .as_deref()
                .ok_or_else(|| vec![Fault::new("enum-variant", "*", &msg.id)])?;
            s.push_str(&format!("    enumDefault: \"{}\",\n", def));
            s.push_str("    variantKeys: {\n");
            for val in &sel.enum_values {
                let var_key =
                    crate::target::model::enum_variant_resource_key(&msg.resource_key, val);
                s.push_str(&format!("      \"{}\": \"{}\",\n", val, var_key));
            }
            s.push_str("    },\n");
            let def_key = crate::target::model::enum_variant_resource_key(&msg.resource_key, def);
            s.push_str(&format!("    defaultKey: \"{}\",\n", def_key));
        }
        s.push_str("    arguments: [\n");
        for arg in &msg.meta.arguments {
            let min_str = arg
                .min
                .map(|m| format!("{}", m))
                .unwrap_or_else(|| "null".to_string());
            let max_str = arg
                .max
                .map(|m| format!("{}", m))
                .unwrap_or_else(|| "null".to_string());
            let num_style = match arg.numeric_style {
                Some(crate::vocab::NumericStyle::LocaleFormatted) => "\"locale-formatted\"",
                _ => "\"ungrouped\"",
            };
            s.push_str(&format!(
                "      {{ name: \"{}\", type: \"{}\", min: {}, max: {}, numericStyle: {} }},\n",
                arg.name,
                arg.arg_type.as_str(),
                min_str,
                max_str,
                num_style
            ));
        }
        s.push_str("    ],\n");
        s.push_str("  },\n");
    }
    s.push_str("};\n\n");

    s.push_str("const BUNDLES = new Map();\n\n");
    s.push_str("function getBundle(locale) {\n");
    s.push_str("  let bundle = BUNDLES.get(locale);\n");
    s.push_str("  if (!bundle) {\n");
    s.push_str("    const locInfo = LOCALES[locale];\n");
    s.push_str("    if (!locInfo) throw new Error(`Unknown locale: ${locale}`);\n");
    s.push_str("    const ftlSource = CATALOGS[locale];\n");
    s.push_str("    if (ftlSource === undefined) throw new Error(`Unknown catalog: ${locale}`);\n");
    s.push_str("    const resource = new FluentResource(ftlSource);\n");
    s.push_str("    bundle = new FluentBundle(locInfo.bundleLocale);\n");
    s.push_str("    const errors = bundle.addResource(resource);\n");
    s.push_str(
        "    if (errors.length > 0) throw new Error(`FTL errors: ${errors.join(\", \")}`);\n",
    );
    s.push_str("    BUNDLES.set(locale, bundle);\n");
    s.push_str("  }\n");
    s.push_str("  return bundle;\n");
    s.push_str("}\n\n");

    s.push_str("export function render(locale, messageId, args = []) {\n");
    s.push_str("  const locInfo = LOCALES[locale];\n");
    s.push_str("  if (!locInfo) throw new Error(`Unknown locale: ${locale}`);\n\n");

    s.push_str("  const msgInfo = MESSAGES[messageId];\n");
    s.push_str("  if (!msgInfo) throw new Error(`Unknown message: ${messageId}`);\n\n");

    // Check duplicate args
    s.push_str("  const seen = new Set();\n");
    s.push_str("  for (const [k] of args) {\n");
    s.push_str("    if (seen.has(k)) throw new Error(`Duplicate argument: ${k}`);\n");
    s.push_str("    seen.add(k);\n");
    s.push_str("  }\n\n");

    // Check declared args
    s.push_str("  const declaredMap = new Map(msgInfo.arguments.map(a => [a.name, a]));\n");
    s.push_str("  for (const [k] of args) {\n");
    s.push_str("    if (!declaredMap.has(k)) throw new Error(`Unknown argument: ${k}`);\n");
    s.push_str("  }\n");
    s.push_str("  for (const k of declaredMap.keys()) {\n");
    s.push_str("    if (!seen.has(k)) throw new Error(`Missing argument: ${k}`);\n");
    s.push_str("  }\n\n");

    // Type and range check
    s.push_str("  const fArgs = {};\n");
    s.push_str("  let selectedVariant = msgInfo.enumDefault || null;\n\n");
    s.push_str("  for (const [k, v] of args) {\n");
    s.push_str("    const spec = declaredMap.get(k);\n");
    s.push_str("    if (spec.type === \"string\") {\n");
    s.push_str("      if (typeof v !== \"string\") throw new Error(`Expected string for ${k}`);\n");
    s.push_str("      if (msgInfo.formatIntent === \"enum\" && msgInfo.selectorVar === k) {\n");
    s.push_str("        if (msgInfo.enumValues.includes(v)) {\n");
    s.push_str("          selectedVariant = v;\n");
    s.push_str("        } else {\n");
    s.push_str("          selectedVariant = msgInfo.enumDefault;\n");
    s.push_str("        }\n");
    s.push_str("      }\n");
    s.push_str("      fArgs[k] = v;\n");
    s.push_str("    } else if (spec.type === \"boolean\") {\n");
    s.push_str(
        "      if (typeof v !== \"boolean\") throw new Error(`Expected boolean for ${k}`);\n",
    );
    s.push_str("      fArgs[k] = v ? \"true\" : \"false\";\n");
    s.push_str("    } else if (spec.type === \"integer\") {\n");
    s.push_str("      if (v && typeof v === \"object\" && v.type === \"cardinal\") throw new Error(`Expected integer, got cardinal for ${k}`);\n");
    s.push_str("      const num = (v && typeof v === \"object\" && v.type === \"integer\") ? v.value : v;\n");
    s.push_str("      if (typeof num !== \"number\" && typeof num !== \"bigint\") throw new Error(`Expected number for ${k}`);\n");
    s.push_str("      if (typeof num === \"number\" && (!Number.isFinite(num) || !Number.isInteger(num))) throw new Error(`Expected integer number for ${k}`);\n");
    s.push_str("      const nVal = BigInt(num);\n");
    s.push_str("      const minVal = spec.min !== null ? BigInt(spec.min) : -9007199254740991n;\n");
    s.push_str("      const maxVal = spec.max !== null ? BigInt(spec.max) : 9007199254740991n;\n");
    s.push_str("      const effMin = minVal < -9007199254740991n ? -9007199254740991n : minVal;\n");
    s.push_str("      const effMax = maxVal > 9007199254740991n ? 9007199254740991n : maxVal;\n");
    s.push_str(
        "      if (nVal < effMin || nVal > effMax) throw new Error(`Out of range for ${k}`);\n",
    );
    s.push_str("      fArgs[k] = Number(nVal);\n");
    s.push_str("    } else if (spec.type === \"cardinal\") {\n");
    s.push_str("      if (v && typeof v === \"object\" && v.type === \"integer\") throw new Error(`Expected cardinal, got integer for ${k}`);\n");
    s.push_str("      const num = (v && typeof v === \"object\" && v.type === \"cardinal\") ? v.value : v;\n");
    s.push_str("      if (typeof num !== \"number\" && typeof num !== \"bigint\") throw new Error(`Expected number for ${k}`);\n");
    s.push_str("      if (typeof num === \"number\" && (!Number.isFinite(num) || !Number.isInteger(num))) throw new Error(`Expected integer number for ${k}`);\n");
    s.push_str("      const nVal = BigInt(num);\n");
    s.push_str("      const minVal = spec.min !== null ? BigInt(spec.min) : -2147483648n;\n");
    s.push_str("      const maxVal = spec.max !== null ? BigInt(spec.max) : 2147483647n;\n");
    s.push_str("      const effMin = minVal < -2147483648n ? -2147483648n : minVal;\n");
    s.push_str("      const effMax = maxVal > 2147483647n ? 2147483647n : maxVal;\n");
    s.push_str(
        "      if (nVal < effMin || nVal > effMax) throw new Error(`Out of range for ${k}`);\n",
    );
    s.push_str("      fArgs[k] = Number(nVal);\n");
    s.push_str("    }\n");
    s.push_str("  }\n\n");

    s.push_str("  const bundle = getBundle(locale);\n\n");
    s.push_str("  const targetKey = msgInfo.formatIntent === \"enum\" ? (msgInfo.variantKeys[selectedVariant] || msgInfo.defaultKey) : msgInfo.resourceKey;\n");
    s.push_str("  const message = bundle.getMessage(targetKey);\n");
    s.push_str(
        "  if (!message || !message.value) throw new Error(`Message not found: ${targetKey}`);\n\n",
    );

    s.push_str("  const formatErrors = [];\n");
    s.push_str("  const output = bundle.formatPattern(message.value, fArgs, formatErrors);\n");
    s.push_str("  if (formatErrors.length > 0) throw new Error(`Format error: ${formatErrors.join(\", \")}`);\n");
    s.push_str("  return output;\n");
    s.push_str("}\n");

    Ok(s)
}
