// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::model::ElementKind;
use crate::target::model::TargetBundle;
use serde_json::{Map, Value as JsonValue, json};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub fn generate_webextension_target(
    bundle: &TargetBundle,
    dest_dir: &Path,
) -> Result<(), Vec<Fault>> {
    let marked_messages: Vec<_> = bundle
        .messages
        .iter()
        .filter(|m| m.meta.webextension)
        .collect();

    if marked_messages.is_empty() {
        // "Bundle with none writes no _locales directory."
        return Ok(());
    }

    // Check folders and folder collisions
    let mut folder_to_locale: HashMap<String, String> = HashMap::new();
    let mut locale_folders: Vec<(&crate::locale::LocaleEntry, String)> = Vec::new();

    for loc in &bundle.locales {
        let folder = match &loc.webextension_folder {
            Some(f) => f.clone(),
            None => return Err(vec![Fault::new("webextension-folder", &loc.tag, "*")]),
        };

        if let Some(existing_loc) = folder_to_locale.get(&folder) {
            if existing_loc != &loc.tag {
                return Err(vec![Fault::new(
                    "webextension-folder-collision",
                    &loc.tag,
                    "*",
                )]);
            }
        } else {
            folder_to_locale.insert(folder.clone(), loc.tag.clone());
        }

        locale_folders.push((loc, folder));
    }

    let bundle_dir = dest_dir.join("webextension").join(&bundle.id);
    let locales_dir = bundle_dir.join("_locales");
    fs::create_dir_all(&locales_dir).map_err(|_| vec![Fault::catalog("io-error")])?;

    for (loc, folder) in locale_folders {
        let folder_dir = locales_dir.join(&folder);
        fs::create_dir_all(&folder_dir).map_err(|_| vec![Fault::catalog("io-error")])?;

        let messages_json = generate_webextension_messages_json(&marked_messages, &loc.tag)?;
        let mut json_str = serde_json::to_string_pretty(&messages_json)
            .map_err(|_| vec![Fault::catalog("io-error")])?;
        json_str.push('\n');

        let json_path = folder_dir.join("messages.json");
        fs::write(&json_path, json_str.as_bytes()).map_err(|_| vec![Fault::catalog("io-error")])?;
    }

    Ok(())
}

fn generate_webextension_messages_json(
    messages: &[&crate::target::model::TargetMessage],
    locale: &str,
) -> Result<JsonValue, Vec<Fault>> {
    let mut root_map = Map::new();

    for msg in messages {
        let parsed = match msg.translations.get(locale) {
            Some(p) => p,
            None => return Err(vec![Fault::new("missing-translation", locale, &msg.id)]),
        };

        let mut raw_content = String::new();

        for el in &parsed.branches[0].pattern {
            match el.kind {
                ElementKind::Text => {
                    raw_content.push_str(&el.value);
                }
                ElementKind::Slot => {
                    let slot = msg
                        .meta
                        .protected_slots
                        .iter()
                        .find(|s| s.name == el.value)
                        .ok_or_else(|| vec![Fault::catalog("io-error")])?;
                    raw_content.push_str(&slot.value);
                }
                ElementKind::Arg => {
                    // Webextension dynamic messages are prohibited by validator
                    return Err(vec![Fault::new("webextension-dynamic", locale, &msg.id)]);
                }
            }
        }

        let entry = json!({
            "message": "$SAFE$",
            "description": msg.meta.description,
            "placeholders": {
                "safe": {
                    "content": raw_content
                }
            }
        });

        root_map.insert(msg.resource_key.clone(), entry);
    }

    Ok(JsonValue::Object(root_map))
}
