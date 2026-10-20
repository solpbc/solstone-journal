// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::model::ElementKind;
use crate::target::model::TargetBundle;
use crate::vocab::FormatIntent;
use std::fs;
use std::path::Path;

pub fn generate_gettext_target(bundle: &TargetBundle, dest_dir: &Path) -> Result<(), Vec<Fault>> {
    let bundle_dir = dest_dir.join("gettext").join(&bundle.id);
    fs::create_dir_all(&bundle_dir).map_err(|_| vec![Fault::catalog("io-error")])?;

    for msg in &bundle.messages {
        for arg in &msg.meta.arguments {
            if arg.numeric_style == Some(crate::vocab::NumericStyle::LocaleFormatted) {
                return Err(vec![Fault::new("locale-formatted", "*", &msg.id)]);
            }
        }
    }

    let has_cardinal_messages = bundle
        .messages
        .iter()
        .any(|m| m.meta.format_intent == FormatIntent::Cardinal);

    // 1. Generate <locale>.po for each locale
    for loc in &bundle.locales {
        if has_cardinal_messages && loc.cardinal.is_none() {
            return Err(vec![Fault::new("plural-rule", &loc.tag, "*")]);
        }

        let po_content = generate_po_content(bundle, loc)?;
        let po_path = bundle_dir.join(format!("{}.po", loc.tag));
        fs::write(&po_path, po_content.as_bytes()).map_err(|_| vec![Fault::catalog("io-error")])?;
    }

    // 2. Generate select.h
    let header_code = generate_select_header(bundle)?;
    let header_path = bundle_dir.join("select.h");
    fs::write(&header_path, header_code.as_bytes())
        .map_err(|_| vec![Fault::catalog("io-error")])?;

    Ok(())
}

fn generate_po_content(
    bundle: &TargetBundle,
    loc: &crate::locale::LocaleEntry,
) -> Result<String, Vec<Fault>> {
    let mut s = String::new();

    let has_cardinal_messages = bundle
        .messages
        .iter()
        .any(|m| m.meta.format_intent == FormatIntent::Cardinal);

    let plural_forms = match loc.cardinal.as_deref() {
        Some("en") => Some("nplurals=2; plural=(n != 1);"),
        Some("ar") => Some(
            "nplurals=6; plural=(n==0 ? 0 : n==1 ? 1 : n==2 ? 2 : n%100>=3 && n%100<=10 ? 3 : n%100>=11 && n%100<=99 ? 4 : 5);",
        ),
        Some(_) => {
            if has_cardinal_messages {
                return Err(vec![Fault::new("plural-rule", &loc.tag, "*")]);
            }
            None
        }
        None => {
            if has_cardinal_messages {
                return Err(vec![Fault::new("plural-rule", &loc.tag, "*")]);
            }
            None
        }
    };

    s.push_str("msgid \"\"\n");
    s.push_str("msgstr \"\"\n");
    s.push_str(&format!("\"Language: {}\\n\"\n", loc.tag));
    s.push_str("\"MIME-Version: 1.0\\n\"\n");
    s.push_str("\"Content-Type: text/plain; charset=UTF-8\\n\"\n");
    s.push_str("\"Content-Transfer-Encoding: 8bit\\n\"\n");
    if let Some(pf) = plural_forms {
        s.push_str(&format!("\"Plural-Forms: {}\\n\"\n", pf));
    }
    s.push('\n');

    for msg in &bundle.messages {
        let parsed = match msg.translations.get(&loc.tag) {
            Some(p) => p,
            None => return Err(vec![Fault::new("missing-translation", &loc.tag, &msg.id)]),
        };

        match msg.meta.format_intent {
            FormatIntent::Text => {
                let tmpl = render_gettext_template(&parsed.branches[0].pattern, msg)?;
                s.push_str(&format!("msgctxt \"{}\"\n", msg.id));
                s.push_str(&format!("msgid \"{}\"\n", msg.resource_key));
                s.push_str(&format!("msgstr \"{}\"\n\n", escape_po_string(&tmpl)));
            }
            FormatIntent::Cardinal => {
                s.push_str(&format!("msgctxt \"{}\"\n", msg.id));
                s.push_str(&format!("msgid \"{}\"\n", msg.resource_key));
                s.push_str(&format!("msgid_plural \"{}_plural\"\n", msg.resource_key));

                let other_branch = parsed
                    .branches
                    .iter()
                    .find(|b| b.key.as_deref() == Some("other"))
                    .ok_or_else(|| vec![Fault::new("cardinal-default", &loc.tag, &msg.id)])?;
                let other_tmpl = render_gettext_template(&other_branch.pattern, msg)?;

                match loc.cardinal.as_deref() {
                    Some("en") => {
                        let one_tmpl = match parsed
                            .branches
                            .iter()
                            .find(|b| b.key.as_deref() == Some("one"))
                        {
                            Some(b) => render_gettext_template(&b.pattern, msg)?,
                            None => other_tmpl.clone(),
                        };
                        s.push_str(&format!("msgstr[0] \"{}\"\n", escape_po_string(&one_tmpl)));
                        s.push_str(&format!(
                            "msgstr[1] \"{}\"\n\n",
                            escape_po_string(&other_tmpl)
                        ));
                    }
                    Some("ar") => {
                        let get_cat_tmpl = |cat: &str| -> Result<String, Vec<Fault>> {
                            if let Some(b) = parsed
                                .branches
                                .iter()
                                .find(|b| b.key.as_deref() == Some(cat))
                            {
                                render_gettext_template(&b.pattern, msg)
                            } else {
                                Ok(other_tmpl.clone())
                            }
                        };
                        let zero_tmpl = get_cat_tmpl("zero")?;
                        let one_tmpl = get_cat_tmpl("one")?;
                        let two_tmpl = get_cat_tmpl("two")?;
                        let few_tmpl = get_cat_tmpl("few")?;
                        let many_tmpl = get_cat_tmpl("many")?;

                        s.push_str(&format!("msgstr[0] \"{}\"\n", escape_po_string(&zero_tmpl)));
                        s.push_str(&format!("msgstr[1] \"{}\"\n", escape_po_string(&one_tmpl)));
                        s.push_str(&format!("msgstr[2] \"{}\"\n", escape_po_string(&two_tmpl)));
                        s.push_str(&format!("msgstr[3] \"{}\"\n", escape_po_string(&few_tmpl)));
                        s.push_str(&format!("msgstr[4] \"{}\"\n", escape_po_string(&many_tmpl)));
                        s.push_str(&format!(
                            "msgstr[5] \"{}\"\n\n",
                            escape_po_string(&other_tmpl)
                        ));
                    }
                    _ => {
                        s.push_str(&format!(
                            "msgstr[0] \"{}\"\n\n",
                            escape_po_string(&other_tmpl)
                        ));
                    }
                }
            }
            FormatIntent::Enum => {
                let sel = msg.meta.selector.as_ref().unwrap();
                for val in &sel.enum_values {
                    if let Some(b) = parsed
                        .branches
                        .iter()
                        .find(|b| b.key.as_deref() == Some(val.as_str()))
                    {
                        let tmpl = render_gettext_template(&b.pattern, msg)?;
                        let var_key =
                            crate::target::model::enum_variant_resource_key(&msg.resource_key, val);
                        s.push_str(&format!("msgctxt \"{}.{}\"\n", msg.id, val));
                        s.push_str(&format!("msgid \"{}\"\n", var_key));
                        s.push_str(&format!("msgstr \"{}\"\n\n", escape_po_string(&tmpl)));
                    }
                }
            }
        }
    }

    Ok(s)
}

pub fn render_gettext_template(
    elements: &[crate::model::PatternElement],
    msg: &crate::target::model::TargetMessage,
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
                let (pos, _) = msg
                    .meta
                    .arguments
                    .iter()
                    .enumerate()
                    .find(|(_, a)| a.name == el.value)
                    .ok_or_else(|| vec![Fault::catalog("io-error")])?;
                let idx = pos + 1;
                out.push('\u{2068}');
                out.push_str(&format!("%{}$s", idx));
                out.push('\u{2069}');
            }
        }
    }

    Ok(out)
}

fn escape_po_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn generate_select_header(bundle: &TargetBundle) -> Result<String, Vec<Fault>> {
    let mut s = String::new();
    s.push_str("// SPDX-License-Identifier: AGPL-3.0-only\n");
    s.push_str("// Copyright (c) 2026 sol pbc\n\n");
    s.push_str("#ifndef SELECT_H\n");
    s.push_str("#define SELECT_H\n\n");
    s.push_str("#include <stdint.h>\n");
    s.push_str("#include <string.h>\n\n");

    s.push_str("static inline const char* resolve_enum_key(const char* message_id, const char* enum_arg) {\n");
    s.push_str("    if (!message_id) return NULL;\n");
    for msg in &bundle.messages {
        if msg.meta.format_intent == FormatIntent::Enum {
            let sel = msg.meta.selector.as_ref().unwrap();
            let def = sel
                .enum_default
                .as_deref()
                .ok_or_else(|| vec![Fault::new("enum-variant", "*", &msg.id)])?;
            let def_key = crate::target::model::enum_variant_resource_key(&msg.resource_key, def);
            s.push_str(&format!(
                "    if (strcmp(message_id, \"{}\") == 0) {{\n",
                msg.id
            ));
            s.push_str("        if (!enum_arg) return NULL;\n");
            for v in &sel.enum_values {
                let v_key = crate::target::model::enum_variant_resource_key(&msg.resource_key, v);
                s.push_str(&format!(
                    "        if (strcmp(enum_arg, \"{}\") == 0) return \"{}\";\n",
                    v, v_key
                ));
            }
            s.push_str(&format!("        return \"{}\";\n", def_key));
            s.push_str("    }\n");
        } else {
            s.push_str(&format!(
                "    if (strcmp(message_id, \"{}\") == 0) return \"{}\";\n",
                msg.id, msg.resource_key
            ));
        }
    }
    s.push_str("    return NULL;\n");
    s.push_str("}\n\n");

    s.push_str("#endif // SELECT_H\n");
    Ok(s)
}
