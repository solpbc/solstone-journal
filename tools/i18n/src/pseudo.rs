// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::{ParsedMessage, dotted_to_fluent_id};
use crate::fault::Fault;
use crate::model::ElementKind;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static PSEUDO_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn transform_en_xa(text: &str) -> String {
    let mut out = String::from("\u{27E6}");
    for ch in text.chars() {
        match ch {
            'a' | 'A' => out.push_str("åå"),
            'e' | 'E' => out.push_str("éé"),
            'i' | 'I' => out.push_str("îî"),
            'o' | 'O' => out.push_str("öö"),
            'u' | 'U' => out.push_str("üü"),
            'c' | 'C' => out.push('ç'),
            'n' | 'N' => out.push('ñ'),
            other => out.push(other),
        }
    }
    out.push('\u{27E7}');
    out
}

pub fn transform_ar_xb(text: &str) -> String {
    format!("\u{202B}{}\u{202C}", text)
}

pub fn generate_pseudo_ftl(
    messages: &HashMap<String, ParsedMessage>,
    transform_fn: fn(&str) -> String,
) -> String {
    let mut ids: Vec<&String> = messages.keys().collect();
    ids.sort();

    let mut out = String::new();
    for id in ids {
        let msg = &messages[id];
        let fluent_id = dotted_to_fluent_id(&msg.id);
        if let Some(sel_var) = &msg.selector_var {
            out.push_str(&format!("{} = {{ ${} ->\n", fluent_id, sel_var));
            for b in &msg.branches {
                let default_star = if b.default { "*" } else { " " };
                let key_str = b.key.as_deref().unwrap_or("other");
                out.push_str(&format!("   {}[{}] ", default_star, key_str));
                for el in &b.pattern {
                    match el.kind {
                        ElementKind::Text => out.push_str(&transform_fn(&el.value)),
                        ElementKind::Arg | ElementKind::Slot => {
                            out.push_str(&format!("{{ ${} }}", el.value));
                        }
                    }
                }
                out.push('\n');
            }
            out.push_str("}\n\n");
        } else {
            out.push_str(&format!("{} = ", fluent_id));
            if let Some(b) = msg.branches.first() {
                for el in &b.pattern {
                    match el.kind {
                        ElementKind::Text => out.push_str(&transform_fn(&el.value)),
                        ElementKind::Arg | ElementKind::Slot => {
                            out.push_str(&format!("{{ ${} }}", el.value));
                        }
                    }
                }
            }
            out.push_str("\n\n");
        }
    }
    out
}

pub fn write_pseudo_locales(
    messages: &HashMap<String, ParsedMessage>,
    out_dir: &Path,
) -> Result<(), Fault> {
    if out_dir.exists() {
        return Err(Fault::catalog("destination-exists"));
    }

    let en_xa_content = generate_pseudo_ftl(messages, transform_en_xa);
    let ar_xb_content = generate_pseudo_ftl(messages, transform_ar_xb);

    let messages_den = messages.len();
    let mut branches_den = 0;
    let mut slots_den = 0;
    for msg in messages.values() {
        if msg.selector_var.is_some() {
            branches_den += msg.branches.len();
        }
        for b in &msg.branches {
            for el in &b.pattern {
                if el.kind == ElementKind::Slot {
                    slots_den += 1;
                }
            }
        }
    }

    #[derive(serde::Serialize)]
    struct PseudoDenominators {
        messages_den: usize,
        branches_den: usize,
        slots_den: usize,
    }

    let den = PseudoDenominators {
        messages_den,
        branches_den,
        slots_den,
    };
    let mut den_str = serde_json::to_string(&den).map_err(|_| Fault::catalog("io-error"))?;
    den_str.push('\n');

    let parent = out_dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|_| Fault::catalog("io-error"))?;

    let dir_name = out_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("locales");
    let pid = std::process::id();
    let count = PSEUDO_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_dir_name = format!("{}.tmp.{}.{}", dir_name, pid, count);
    let temp_dir = parent.join(temp_dir_name);

    fs::create_dir_all(&temp_dir).map_err(|_| Fault::catalog("io-error"))?;

    let write_res = (|| -> Result<(), std::io::Error> {
        let mut f1 = fs::File::create(temp_dir.join("en-XA.ftl"))?;
        f1.write_all(en_xa_content.as_bytes())?;
        f1.sync_all()?;
        drop(f1);

        let mut f2 = fs::File::create(temp_dir.join("ar-XB.ftl"))?;
        f2.write_all(ar_xb_content.as_bytes())?;
        f2.sync_all()?;
        drop(f2);

        let mut f3 = fs::File::create(temp_dir.join("denominators.json"))?;
        f3.write_all(den_str.as_bytes())?;
        f3.sync_all()?;
        drop(f3);

        Ok(())
    })();

    if write_res.is_err() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(Fault::catalog("io-error"));
    }

    if fs::rename(&temp_dir, out_dir).is_err() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(Fault::catalog("io-error"));
    }

    Ok(())
}
