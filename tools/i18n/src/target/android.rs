// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::model::ElementKind;
use crate::target::model::TargetBundle;
use crate::vocab::FormatIntent;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub fn resolve_android_qualifier(loc: &crate::locale::LocaleEntry) -> Result<String, Fault> {
    if let Some(qual) = &loc.android_qualifier {
        Ok(qual.clone())
    } else {
        let tag = loc.tag.as_str();
        let bytes = tag.as_bytes();
        if (bytes.len() == 2 || bytes.len() == 3) && bytes.iter().all(|b| b.is_ascii_lowercase()) {
            Ok(tag.to_string())
        } else {
            Err(Fault::new("locale-qualifier", &loc.tag, "*"))
        }
    }
}

pub fn generate_android_target(bundle: &TargetBundle, dest_dir: &Path) -> Result<(), Vec<Fault>> {
    // Check qualifier resolution and collisions before creating directories
    let mut seen_quals = HashSet::new();
    let mut resolved_quals = Vec::new();
    for loc in &bundle.locales {
        let qual = resolve_android_qualifier(loc).map_err(|f| vec![f])?;
        if !seen_quals.insert(qual.clone()) {
            return Err(vec![Fault::new("locale-qualifier", &loc.tag, "*")]);
        }
        resolved_quals.push((loc, qual));
    }

    let bundle_dir = dest_dir.join("android").join(&bundle.id);
    fs::create_dir_all(&bundle_dir).map_err(|_| vec![Fault::catalog("io-error")])?;

    // 1. Generate values-<qual>/strings.xml for each locale
    for (loc, qual) in &resolved_quals {
        let values_dir = bundle_dir.join(format!("values-{}", qual));
        fs::create_dir_all(&values_dir).map_err(|_| vec![Fault::catalog("io-error")])?;

        let xml_content = generate_strings_xml(bundle, &loc.tag)?;
        let xml_path = values_dir.join("strings.xml");
        fs::write(&xml_path, xml_content.as_bytes())
            .map_err(|_| vec![Fault::catalog("io-error")])?;
    }

    // Default values/ table:
    // Contract: the default file is a byte-for-byte copy of the channel-selected locale
    // whose tag sorts first. Also write values-<qual>/ for every selected locale, including that one.
    // Use that locale's own cells. Do not substitute source English unless that first tag is the selected source locale.
    // Do not copy an excluded locale. Do not emit b+ folders.
    if let Some((first_loc, _)) = resolved_quals.iter().min_by_key(|(loc, _)| &loc.tag) {
        let default_values_dir = bundle_dir.join("values");
        fs::create_dir_all(&default_values_dir).map_err(|_| vec![Fault::catalog("io-error")])?;
        let default_xml_content = generate_strings_xml(bundle, &first_loc.tag)?;
        let default_xml_path = default_values_dir.join("strings.xml");
        fs::write(&default_xml_path, default_xml_content.as_bytes())
            .map_err(|_| vec![Fault::catalog("io-error")])?;
    }

    // 2. Generate Select.java
    let java_code = generate_select_java(bundle)?;
    let java_path = bundle_dir.join("Select.java");
    fs::write(&java_path, java_code.as_bytes()).map_err(|_| vec![Fault::catalog("io-error")])?;

    Ok(())
}

fn generate_strings_xml(bundle: &TargetBundle, locale: &str) -> Result<String, Vec<Fault>> {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    s.push_str("<resources>\n");

    for msg in &bundle.messages {
        let parsed = match msg.translations.get(locale) {
            Some(p) => p,
            None => return Err(vec![Fault::new("missing-translation", locale, &msg.id)]),
        };

        match msg.meta.format_intent {
            FormatIntent::Text => {
                let tmpl = render_android_template(&parsed.branches[0].pattern, msg)?;
                s.push_str(&format!(
                    "  <string name=\"{}\">{}</string>\n",
                    msg.resource_key, tmpl
                ));
            }
            FormatIntent::Cardinal => {
                s.push_str(&format!("  <plurals name=\"{}\">\n", msg.resource_key));
                for b in &parsed.branches {
                    let cat = b.key.as_deref().unwrap_or("other");
                    let tmpl = render_android_template(&b.pattern, msg)?;
                    s.push_str(&format!("    <item quantity=\"{}\">{}</item>\n", cat, tmpl));
                }
                s.push_str("  </plurals>\n");
            }
            FormatIntent::Enum => {
                let sel = msg.meta.selector.as_ref().unwrap();
                for val in &sel.enum_values {
                    if let Some(b) = parsed
                        .branches
                        .iter()
                        .find(|b| b.key.as_deref() == Some(val.as_str()))
                    {
                        let tmpl = render_android_template(&b.pattern, msg)?;
                        let var_key =
                            crate::target::model::enum_variant_resource_key(&msg.resource_key, val);
                        s.push_str(&format!(
                            "  <string name=\"{}\">{}</string>\n",
                            var_key, tmpl
                        ));
                    }
                }
            }
        }
    }

    s.push_str("</resources>\n");
    Ok(s)
}

pub fn render_android_template(
    elements: &[crate::model::PatternElement],
    msg: &crate::target::model::TargetMessage,
) -> Result<String, Vec<Fault>> {
    let mut raw = String::new();
    for el in elements {
        match el.kind {
            ElementKind::Text => {
                for c in el.value.chars() {
                    if c == '%' {
                        raw.push_str("%%");
                    } else {
                        raw.push(c);
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
                        raw.push_str("%%");
                    } else {
                        raw.push(c);
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
                let idx = pos + 1;
                raw.push('\u{2068}');
                let _ = arg_spec;
                raw.push_str(&format!("%{}$s", idx));
                raw.push('\u{2069}');
            }
        }
    }

    // Android collapses unquoted internal whitespace and interprets backslash escapes.
    let mut out = String::from("\"");
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('\"');

    Ok(out)
}

fn generate_select_java(bundle: &TargetBundle) -> Result<String, Vec<Fault>> {
    let mut s = String::new();
    s.push_str("// SPDX-License-Identifier: AGPL-3.0-only\n");
    s.push_str("// Copyright (c) 2026 sol pbc\n\n");
    s.push_str("package solstone.i18n;\n\n");
    s.push_str("public final class Select {\n");
    s.push_str("    private Select() {}\n\n");
    s.push_str("    public static String resolveResourceKey(String messageId, String enumArg) {\n");
    s.push_str("        switch (messageId) {\n");
    for msg in &bundle.messages {
        if msg.meta.format_intent == FormatIntent::Enum {
            let sel = msg.meta.selector.as_ref().unwrap();
            let def = sel
                .enum_default
                .as_deref()
                .ok_or_else(|| vec![Fault::new("enum-variant", "*", &msg.id)])?;
            let def_key = crate::target::model::enum_variant_resource_key(&msg.resource_key, def);
            s.push_str(&format!("            case \"{}\":\n", msg.id));
            s.push_str("                if (enumArg == null) throw new IllegalArgumentException(\"Missing enum argument\");\n");
            s.push_str("                switch (enumArg) {\n");
            for v in &sel.enum_values {
                let v_key = crate::target::model::enum_variant_resource_key(&msg.resource_key, v);
                s.push_str(&format!(
                    "                    case \"{}\": return \"{}\";\n",
                    v, v_key
                ));
            }
            s.push_str(&format!(
                "                    default: return \"{}\";\n",
                def_key
            ));
            s.push_str("                }\n");
        } else {
            s.push_str(&format!(
                "            case \"{}\": return \"{}\";\n",
                msg.id, msg.resource_key
            ));
        }
    }
    s.push_str(
        "            default: throw new IllegalArgumentException(\"Unknown message identity\");\n",
    );
    s.push_str("        }\n");
    s.push_str("    }\n\n");

    s.push_str("    public static String formatIntegerUngrouped(long val) {\n");
    s.push_str("        return Long.toString(val);\n");
    s.push_str("    }\n\n");

    s.push_str("    public static String formatIntegerLocale(long val, String locale) {\n");
    s.push_str("        android.icu.util.ULocale uloc = new android.icu.util.ULocale(locale);\n");
    s.push_str("        return android.icu.text.NumberFormat.getInstance(uloc).format(val);\n");
    s.push_str("    }\n");
    s.push_str("}\n");
    Ok(s)
}
