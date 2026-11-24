// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::{
    ParsedCatalogFile, ParsedMessage, parse_ftl_resource, validate_admitted_message,
};
use crate::fault::{Fault, sort_faults};
use crate::freshness::{
    SourceLock, compute_contract_hash, compute_protected_hash, compute_source_hash,
};
use crate::history::{HistoryRegistry, validate_message_id};
use crate::locale::{LocaleRegistry, resolve_namespace_owner, validate_locale_tag};
use crate::metadata::MessageMetadata;
use crate::vocab::{Channel, Risk};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub(crate) fn is_restricted_draft_message(
    meta: &MessageMetadata,
    loc: &str,
    loc_is_draft: bool,
) -> bool {
    loc_is_draft
        && matches!(meta.risk, Risk::Legal | Risk::Statutory)
        && meta.approval.as_ref().map(|a| a.locale.as_str()) != Some(loc)
}

pub(crate) fn load_locale_translations(
    trans_dir: &Path,
    loc: &str,
) -> Result<Option<ParsedCatalogFile>, Vec<Fault>> {
    let loc_file = trans_dir.join(format!("{}.ftl", loc));
    let loc_dir = trans_dir.join(loc);
    let has_file = loc_file.is_file();
    let has_dir = loc_dir.is_dir();

    if has_file && has_dir {
        return Err(vec![Fault::new("duplicate-locale-source", loc, "*")]);
    }
    if has_file {
        let content =
            fs::read_to_string(&loc_file).map_err(|_| vec![Fault::new("io-error", loc, "*")])?;
        return parse_ftl_resource(&content, loc).map(Some);
    }
    if has_dir {
        let mut shard_paths = Vec::new();
        let entries = fs::read_dir(&loc_dir).map_err(|_| vec![Fault::new("io-error", loc, "*")])?;
        for entry in entries {
            let entry = entry.map_err(|_| vec![Fault::new("io-error", loc, "*")])?;
            let path = entry.path();
            if path.is_dir() {
                return Err(vec![Fault::new("invalid-translation-shard", loc, "*")]);
            }
            if path.extension().and_then(|s| s.to_str()) == Some("ftl") {
                shard_paths.push(path);
            }
        }
        shard_paths.sort();

        let mut combined = HashMap::new();
        let mut faults = Vec::new();
        for path in shard_paths {
            let content = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => {
                    faults.push(Fault::new("io-error", loc, "*"));
                    continue;
                }
            };
            match parse_ftl_resource(&content, loc) {
                Ok(parsed) => {
                    for (id, msg) in parsed.messages {
                        match combined.entry(id) {
                            Entry::Occupied(occ) => {
                                faults.push(Fault::new("duplicate-id", loc, occ.key()));
                            }
                            Entry::Vacant(vac) => {
                                vac.insert(msg);
                            }
                        }
                    }
                }
                Err(mut f) => faults.append(&mut f),
            }
        }
        if !faults.is_empty() {
            return Err(faults);
        }
        return Ok(Some(ParsedCatalogFile { messages: combined }));
    }
    Ok(None)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogConfig {
    pub catalog_version: u32,
    pub label: String,
    pub history: bool,
    pub source_locale: String,
}

impl CatalogConfig {
    pub fn validate(&self) -> Result<(), Fault> {
        if self.catalog_version != 1 {
            return Err(Fault::catalog("unsupported-version"));
        }
        if !validate_locale_tag(&self.source_locale) {
            return Err(Fault::new("locale-tag", &self.source_locale, "*"));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct LoadedCatalog {
    pub root: std::path::PathBuf,
    pub config: CatalogConfig,
    pub source_messages: HashMap<String, ParsedMessage>,
    pub metadata: HashMap<String, MessageMetadata>,
    pub translations: HashMap<String, HashMap<String, ParsedMessage>>,
    pub stale_translations: HashMap<String, HashSet<String>>,
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
        Self::load_and_validate_with_options(catalog_dir, channel, requested_locale, false)
    }

    pub fn load_and_validate_with_options(
        catalog_dir: &Path,
        channel: Channel,
        requested_locale: Option<&str>,
        fallback_stale: bool,
    ) -> Result<Self, Vec<Fault>> {
        let requested: Vec<String> = requested_locale.into_iter().map(str::to_string).collect();
        Self::load_and_validate_for_locales(catalog_dir, channel, &requested, fallback_stale)
    }

    pub fn load_and_validate_for_locales(
        catalog_dir: &Path,
        channel: Channel,
        requested_locales: &[String],
        fallback_stale: bool,
    ) -> Result<Self, Vec<Fault>> {
        let registry = LocaleRegistry::embedded().map_err(|fault| vec![fault])?;
        Self::load_with_registry_for_locales(
            catalog_dir,
            channel,
            requested_locales,
            registry,
            fallback_stale,
        )
    }

    #[cfg(test)]
    pub(crate) fn load_with_registry(
        catalog_dir: &Path,
        channel: Channel,
        requested_locale: Option<&str>,
        registry: LocaleRegistry,
    ) -> Result<Self, Vec<Fault>> {
        Self::load_with_registry_options(catalog_dir, channel, requested_locale, registry, false)
    }

    #[cfg(test)]
    pub(crate) fn load_with_registry_options(
        catalog_dir: &Path,
        channel: Channel,
        requested_locale: Option<&str>,
        registry: LocaleRegistry,
        fallback_stale: bool,
    ) -> Result<Self, Vec<Fault>> {
        let requested: Vec<String> = requested_locale.into_iter().map(str::to_string).collect();
        Self::load_with_registry_for_locales(
            catalog_dir,
            channel,
            &requested,
            registry,
            fallback_stale,
        )
    }

    pub(crate) fn load_with_registry_for_locales(
        catalog_dir: &Path,
        channel: Channel,
        requested_locales: &[String],
        registry: LocaleRegistry,
        fallback_stale: bool,
    ) -> Result<Self, Vec<Fault>> {
        Self::load_internal(
            catalog_dir,
            channel,
            requested_locales,
            registry,
            true,
            fallback_stale,
        )
    }

    pub(crate) fn load_source(catalog_dir: &Path) -> Result<Self, Vec<Fault>> {
        let registry = LocaleRegistry::embedded().map_err(|fault| vec![fault])?;
        Self::load_internal(
            catalog_dir,
            Channel::Production,
            &[],
            registry,
            false,
            false,
        )
    }

    fn load_internal(
        catalog_dir: &Path,
        channel: Channel,
        requested_locales: &[String],
        registry: LocaleRegistry,
        include_translations: bool,
        fallback_stale: bool,
    ) -> Result<Self, Vec<Fault>> {
        let mut faults = Vec::new();

        if fallback_stale && channel == Channel::Production {
            return Err(vec![Fault::catalog("fallback-stale-production")]);
        }

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
        config.validate().map_err(|fault| vec![fault])?;

        for req in requested_locales {
            if !validate_locale_tag(req) {
                return Err(vec![Fault::new("locale-tag", req, "*")]);
            }
        }

        let namespace_owners = registry.get_namespace_owners();
        if !registry
            .locales
            .iter()
            .any(|locale| locale.tag == config.source_locale)
        {
            return Err(vec![Fault::new(
                "unknown-locale",
                &config.source_locale,
                "*",
            )]);
        }

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
            if resolve_namespace_owner(&namespace_owners, ns).is_none() {
                faults.push(Fault::new("owner-conflict", "*", id));
            }
        }

        // Validate admitted messages in source
        for (id, msg) in &mut source_file.messages {
            if let Some(meta) = metadata_map.get(id)
                && let Err(mut f) = validate_admitted_message(
                    msg,
                    meta,
                    &config.source_locale,
                    true,
                    None,
                    registry.cardinal_family(&config.source_locale),
                )
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
        if include_translations && !config.history {
            faults.push(Fault::catalog("history-required"));
        }
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
        let eligible_locales = if include_translations {
            match registry.get_eligible_locales_for(channel, &all_bundle_names, requested_locales) {
                Ok(locs) => locs,
                Err(f) => {
                    faults.push(f);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        // Load translations
        let mut translations = HashMap::new();
        let trans_dir = catalog_dir.join("translations");

        for loc in &eligible_locales {
            if *loc == config.source_locale {
                translations.insert(loc.clone(), source_file.messages.clone());
                continue;
            }
            if !validate_locale_tag(loc) {
                faults.push(Fault::new("locale-tag", loc, "*"));
                continue;
            }
            let loc_is_draft = registry.is_draft(loc);
            match load_locale_translations(&trans_dir, loc) {
                Ok(Some(mut parsed_file)) => {
                    for (id, msg) in &mut parsed_file.messages {
                        if !source_file.messages.contains_key(id) || !metadata_map.contains_key(id)
                        {
                            faults.push(Fault::new("unknown-message", loc, id));
                        } else if let Some(meta) = metadata_map.get(id) {
                            if is_restricted_draft_message(meta, loc, loc_is_draft) {
                                faults.push(Fault::new("restricted-draft", loc, id));
                            }
                            let src_msg = source_file.messages.get(id);
                            if let Err(mut f) = validate_admitted_message(
                                msg,
                                meta,
                                loc,
                                false,
                                src_msg,
                                registry.cardinal_family(loc),
                            ) {
                                faults.append(&mut f);
                            }
                        }
                    }
                    translations.insert(loc.clone(), parsed_file.messages);
                }
                Ok(None) => {}
                Err(mut f) => {
                    faults.append(&mut f);
                }
            }
        }

        // Check missing translations (draft locales fall back to source English on development)
        for loc in &eligible_locales {
            if registry.is_draft(loc) {
                continue;
            }
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
        let mut stale_translations: HashMap<String, HashSet<String>> = HashMap::new();
        if let Some(l) = &lock {
            let mut drop_stale: Vec<(String, String)> = Vec::new();
            for (loc, tr_msgs) in &translations {
                if *loc == config.source_locale {
                    continue;
                }
                let allow_stale_fallback = fallback_stale && registry.is_draft(loc);
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
                            if allow_stale_fallback {
                                drop_stale.push((loc.clone(), id.clone()));
                            } else {
                                faults.push(f);
                            }
                        }
                    }
                }
            }
            for (loc, id) in drop_stale {
                if let Some(tr_map) = translations.get_mut(&loc) {
                    tr_map.remove(&id);
                }
                stale_translations.entry(loc).or_default().insert(id);
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
                // Packaging a channel must not load or validate excluded locale content.
                if !eligible_locales.contains(&app.locale) {
                    continue;
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
                root: catalog_dir.to_path_buf(),
                config,
                source_messages: source_file.messages,
                metadata: metadata_map,
                translations,
                stale_translations,
                history,
                lock,
                registry,
            })
        }
    }
}
