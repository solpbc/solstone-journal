// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::{ParsedMessage, parse_ftl_resource, validate_admitted_message};
use crate::fault::{Fault, sort_faults};
use crate::freshness::{
    SourceLock, compute_contract_hash, compute_protected_hash, compute_source_hash,
};
use crate::history::{HistoryRegistry, validate_message_id};
use crate::locale::{LocaleRegistry, validate_locale_tag};
use crate::metadata::MessageMetadata;
use crate::vocab::Channel;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogConfig {
    pub catalog_version: u32,
    pub label: String,
    pub history: bool,
    pub source_locale: String,
}

#[derive(Debug)]
pub struct LoadedCatalog {
    pub config: CatalogConfig,
    pub source_messages: HashMap<String, ParsedMessage>,
    pub metadata: HashMap<String, MessageMetadata>,
    pub translations: HashMap<String, HashMap<String, ParsedMessage>>,
    pub history: Option<HistoryRegistry>,
    pub lock: Option<SourceLock>,
    pub registry: LocaleRegistry,
}

impl LoadedCatalog {
    pub fn load_and_validate(
        catalog_dir: &Path,
        channel: Channel,
        requested_locale: Option<&str>,
    ) -> Result<Self, Vec<Fault>> {
        let mut faults = Vec::new();

        if !catalog_dir.is_dir() {
            return Err(vec![Fault::catalog("missing-catalog")]);
        }

        let config_path = catalog_dir.join("catalog.json");
        let config_str = match fs::read_to_string(&config_path) {
            Ok(s) => s,
            Err(_) => return Err(vec![Fault::catalog("missing-catalog")]),
        };
        let config: CatalogConfig = match serde_json::from_str(&config_str) {
            Ok(c) => c,
            Err(_) => return Err(vec![Fault::catalog("missing-catalog")]),
        };
        if config.catalog_version != 1 {
            return Err(vec![Fault::catalog("unsupported-version")]);
        }

        if !validate_locale_tag(&config.source_locale) {
            return Err(vec![Fault::new("locale-tag", &config.source_locale, "*")]);
        }

        if let Some(req) = requested_locale
            && !validate_locale_tag(req)
        {
            return Err(vec![Fault::new("locale-tag", req, "*")]);
        }

        let registry = match LocaleRegistry::embedded() {
            Ok(r) => r,
            Err(f) => return Err(vec![f]),
        };

        let namespace_owners = registry.get_namespace_owners();

        // Load source/<source_locale>.ftl
        let source_ftl_path = catalog_dir
            .join("source")
            .join(format!("{}.ftl", config.source_locale));
        let source_ftl_content = match fs::read_to_string(&source_ftl_path) {
            Ok(s) => s,
            Err(_) => return Err(vec![Fault::catalog("source-missing")]),
        };

        let mut source_file = match parse_ftl_resource(&source_ftl_content, &config.source_locale) {
            Ok(f) => f,
            Err(f) => {
                faults.extend(f);
                sort_faults(&mut faults);
                return Err(faults);
            }
        };

        // Load meta/<id>.json
        let meta_dir = catalog_dir.join("meta");
        let mut metadata_map = HashMap::new();

        for id in source_file.messages.keys() {
            let meta_path = meta_dir.join(format!("{}.json", id));
            let meta_content = match fs::read_to_string(&meta_path) {
                Ok(s) => s,
                Err(_) => {
                    faults.push(Fault::new("metadata-missing", "*", id));
                    continue;
                }
            };
            match MessageMetadata::parse(id, &meta_content) {
                Ok(m) => {
                    metadata_map.insert(id.clone(), m);
                }
                Err(f) => {
                    faults.push(f);
                }
            }
        }

        // Validate message IDs and namespaces
        for id in source_file.messages.keys() {
            let ns = match id.rsplit_once('.') {
                Some((prefix, _)) => prefix,
                None => {
                    faults.push(Fault::new("invalid-id-syntax", "*", id));
                    continue;
                }
            };
            if let Err(rule) = validate_message_id(id, ns) {
                faults.push(Fault::new(rule, "*", id));
            }
        }

        // Validate admitted messages in source
        for (id, msg) in &mut source_file.messages {
            if let Some(meta) = metadata_map.get(id)
                && let Err(mut f) =
                    validate_admitted_message(msg, meta, &config.source_locale, true, None)
            {
                faults.append(&mut f);
            }
        }

        // Validate source revision against metadata
        for (id, msg) in &source_file.messages {
            if let Some(meta) = metadata_map.get(id) {
                let expected_hash = compute_source_hash(msg);
                if meta.source_revision != expected_hash {
                    faults.push(Fault::new(
                        "source-revision-mismatch",
                        &config.source_locale,
                        id,
                    ));
                }
            }
        }

        // Check history if required
        let history_path = catalog_dir.join("history.json");
        let history = if config.history {
            match HistoryRegistry::load_from_file(&history_path) {
                Ok(h) => {
                    let mut src_ids: Vec<String> = source_file.messages.keys().cloned().collect();
                    src_ids.sort();
                    if let Err(mut f) =
                        h.check_against_source(&src_ids, &metadata_map, &namespace_owners)
                    {
                        faults.append(&mut f);
                    }
                    Some(h)
                }
                Err(f) => {
                    faults.push(f);
                    None
                }
            }
        } else {
            None
        };

        // Check lock if messages exist
        let lock_path = catalog_dir.join("source-lock.json");
        let lock = if !source_file.messages.is_empty() {
            match SourceLock::load_from_file(&lock_path) {
                Ok(l) => Some(l),
                Err(f) => {
                    faults.push(f);
                    None
                }
            }
        } else {
            None
        };

        // Check all bundle names across messages
        let mut all_bundles = HashSet::new();
        for (id, meta) in &metadata_map {
            if meta.bundles.is_empty() {
                faults.push(Fault::new("empty-membership", "*", id));
            }
            for b in &meta.bundles {
                if !registry.bundles.iter().any(|entry| &entry.id == b) {
                    faults.push(Fault::new("undeclared-membership", "*", id));
                }
                all_bundles.insert(b.clone());
            }
        }
        let all_bundle_names: Vec<String> = all_bundles.into_iter().collect();

        // Check channel eligibility of requested locale
        let eligible_locales =
            match registry.get_eligible_locales(channel, &all_bundle_names, requested_locale) {
                Ok(locs) => locs,
                Err(f) => {
                    faults.push(f);
                    Vec::new()
                }
            };

        // Load translations
        let mut translations = HashMap::new();
        let trans_dir = catalog_dir.join("translations");

        for loc in &eligible_locales {
            if !validate_locale_tag(loc) {
                faults.push(Fault::new("locale-tag", loc, "*"));
                continue;
            }
            let loc_path = trans_dir.join(format!("{}.ftl", loc));
            if loc_path.is_file() {
                let content = match fs::read_to_string(&loc_path) {
                    Ok(s) => s,
                    Err(_) => {
                        faults.push(Fault::new("io-error", loc, "*"));
                        continue;
                    }
                };
                match parse_ftl_resource(&content, loc) {
                    Ok(mut parsed_file) => {
                        for (id, msg) in &mut parsed_file.messages {
                            if !source_file.messages.contains_key(id)
                                || !metadata_map.contains_key(id)
                            {
                                faults.push(Fault::new("unknown-message", loc, id));
                            } else if let Some(meta) = metadata_map.get(id) {
                                let src_msg = source_file.messages.get(id);
                                if let Err(mut f) =
                                    validate_admitted_message(msg, meta, loc, false, src_msg)
                                {
                                    faults.append(&mut f);
                                }
                            }
                        }
                        translations.insert(loc.clone(), parsed_file.messages);
                    }
                    Err(mut f) => {
                        faults.append(&mut f);
                    }
                }
            }
        }

        // Check missing translations
        for loc in &eligible_locales {
            for msg_id in source_file.messages.keys() {
                if let Some(meta) = metadata_map.get(msg_id) {
                    let is_required = meta.bundles.iter().any(|b| {
                        registry
                            .bundles
                            .iter()
                            .any(|spec| spec.id == *b && spec.locales.iter().any(|l| l == loc))
                    });
                    if is_required {
                        let has_translation =
                            translations.get(loc).and_then(|m| m.get(msg_id)).is_some();
                        if !has_translation {
                            faults.push(Fault::new("missing-translation", loc, msg_id));
                        }
                    }
                }
            }
        }

        // Check freshness (stale) against lock
        if let Some(l) = &lock {
            for (loc, tr_msgs) in &translations {
                for id in tr_msgs.keys() {
                    if let Some(src_msg) = source_file.messages.get(id)
                        && let Some(meta) = metadata_map.get(id)
                    {
                        let cur_src = compute_source_hash(src_msg);
                        let cur_contract = compute_contract_hash(meta);
                        let cur_prot = compute_protected_hash(meta);
                        if let Err(f) =
                            l.check_translation(id, loc, &cur_src, &cur_contract, &cur_prot)
                        {
                            faults.push(f);
                        }
                    }
                }
            }
        }

        // Check approvals
        for (id, meta) in &metadata_map {
            if let Some(app) = &meta.approval {
                let declared = meta.bundles.iter().any(|b| {
                    registry
                        .bundles
                        .iter()
                        .any(|spec| spec.id == *b && spec.locales.iter().any(|l| l == &app.locale))
                });
                if !declared {
                    faults.push(Fault::new("approval-locale", &app.locale, id));
                }
                if let Some(src_msg) = source_file.messages.get(id) {
                    let src_hash = compute_source_hash(src_msg);
                    if app.source_revision != src_hash {
                        faults.push(Fault::new("approval-revision", &app.locale, id));
                    }
                    let tr_msg = translations.get(&app.locale).and_then(|m| m.get(id));
                    let tr_hash = tr_msg.map(compute_source_hash);
                    if tr_hash.as_deref() != Some(&app.translation_revision) {
                        faults.push(Fault::new("approval-revision", &app.locale, id));
                    }
                }
            }
        }

        if !faults.is_empty() {
            sort_faults(&mut faults);
            Err(faults)
        } else {
            Ok(Self {
                config,
                source_messages: source_file.messages,
                metadata: metadata_map,
                translations,
                history,
                lock,
                registry,
            })
        }
    }
}
