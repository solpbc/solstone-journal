// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::parse_ftl_resource;
use crate::bundle::{export_bundle, import_bundle};
use crate::catalog::{CatalogConfig, LoadedCatalog};
use crate::coverage::CoverageReport;
use crate::fault::{Fault, sort_faults};
use crate::freshness::{
    LocaleAck, SourceLock, SourceLockMessage, compute_contract_hash, compute_protected_hash,
    compute_source_hash,
};
use crate::history::{
    HistoryRecord, HistoryRegistry, check_registry_integrity, compute_resource_key,
    validate_message_id, verify_history_record_against_metadata,
};
use crate::ir::build_ir;
use crate::locale::LocaleRegistry;
use crate::metadata::MessageMetadata;
use crate::pseudo::write_pseudo_locales;
use crate::publish::atomic_write_file;
use crate::vocab::Channel;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run<I>(args: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();

    if args.len() < 2 {
        print_usage();
        return 1;
    }

    match args[1].as_str() {
        "--help" | "-h" | "help" => {
            print_usage();
            0
        }
        "compile" => run_compile(&args[2..]),
        "emit" => run_emit(&args[2..]),
        "history" => run_history(&args[2..]),
        "retire" => run_retire(&args[2..]),
        "lock" => run_lock(&args[2..]),
        "pseudo" => run_pseudo(&args[2..]),
        "coverage" => run_coverage(&args[2..]),
        "import" => run_import(&args[2..]),
        "export" => run_export(&args[2..]),
        "targets" => run_targets(&args[2..]),
        unknown => {
            eprintln!("Unknown command: {}", unknown);
            print_usage();
            1
        }
    }
}

fn print_usage() {
    println!(
        "i18n-catalog - Compiler, validator, and packager for translation catalogs\n\n\
         USAGE:\n\
         i18n-catalog <COMMAND> [OPTIONS]\n\n\
         COMMANDS:\n\
         compile <catalog> --channel <development|production> [--locale <tag>]\n\
         emit <catalog> --channel <development|production> [--locale <tag>] --out <file>\n\
         history <catalog>\n\
         retire <catalog> --id <id> [--replacement <id>]\n\
         lock <catalog> [--acknowledge <locale>]\n\
         pseudo <catalog> --out <dir>\n\
         coverage <catalog> --channel <development|production>\n\
         import <bundle> --dest <dir>\n\
         export <catalog> --out <dir>\n\
         targets <catalog> --channel <development|production> [--locale <tag>] --out <dir> --target <rust|esm|apple|android|webextension|gettext> [--target ...]\n"
    );
}

fn run_compile(args: &[String]) -> i32 {
    let (catalog_dir, channel, locale) = match parse_channel_and_locale_args(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    match LoadedCatalog::load_and_validate(&catalog_dir, channel, locale.as_deref()) {
        Ok(_) => 0,
        Err(faults) => {
            for f in faults {
                eprintln!("{}", f);
            }
            1
        }
    }
}

fn run_emit(args: &[String]) -> i32 {
    let mut out_file = None;
    let mut rest = Vec::new();

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--out" && i + 1 < args.len() {
            out_file = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }

    let out_path = match out_file {
        Some(p) => p,
        None => {
            eprintln!("Missing required --out argument");
            return 1;
        }
    };

    let (catalog_dir, channel, locale) = match parse_channel_and_locale_args(&rest) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    let catalog = match LoadedCatalog::load_and_validate(&catalog_dir, channel, locale.as_deref()) {
        Ok(c) => c,
        Err(faults) => {
            for f in faults {
                eprintln!("{}", f);
            }
            return 1;
        }
    };

    let mut all_bundles = HashSet::new();
    for meta in catalog.metadata.values() {
        for b in &meta.bundles {
            all_bundles.insert(b.clone());
        }
    }
    let all_bundle_names: Vec<String> = all_bundles.into_iter().collect();

    let eligible_locales =
        match catalog
            .registry
            .get_eligible_locales(channel, &all_bundle_names, locale.as_deref())
        {
            Ok(l) => l,
            Err(f) => {
                eprintln!("{}", f);
                return 1;
            }
        };

    let ir = build_ir(
        channel,
        &catalog.config.source_locale,
        &eligible_locales,
        &catalog.source_messages,
        &catalog.metadata,
        &catalog.translations,
        &catalog.registry.get_namespace_owners(),
    );

    let mut ir_json = match serde_json::to_string(&ir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Serialization error: {}", e);
            return 1;
        }
    };
    ir_json.push('\n');

    if let Err(f) = atomic_write_file(&out_path, ir_json.as_bytes()) {
        eprintln!("{}", f);
        return 1;
    }

    0
}

fn run_history(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: i18n-catalog history <catalog>");
        return 1;
    }
    let catalog_dir = Path::new(&args[0]);
    if let Err(f) = update_history(catalog_dir) {
        for fault in f {
            eprintln!("{}", fault);
        }
        return 1;
    }
    0
}

pub fn update_history(catalog_dir: &Path) -> Result<(), Vec<Fault>> {
    let mut faults = Vec::new();
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

    let registry = match LocaleRegistry::embedded() {
        Ok(r) => r,
        Err(f) => return Err(vec![f]),
    };
    let namespace_owners = registry.get_namespace_owners();

    let source_path = catalog_dir
        .join("source")
        .join(format!("{}.ftl", config.source_locale));
    let source_content = match fs::read_to_string(&source_path) {
        Ok(s) => s,
        Err(_) => return Err(vec![Fault::catalog("source-missing")]),
    };
    let mut source_file = parse_ftl_resource(&source_content, &config.source_locale)?;

    let meta_dir = catalog_dir.join("meta");
    let mut meta_map = HashMap::new();
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
                meta_map.insert(id.clone(), m);
            }
            Err(f) => faults.push(f),
        }
    }

    if !faults.is_empty() {
        sort_faults(&mut faults);
        return Err(faults);
    }

    let history_path = catalog_dir.join("history.json");
    let mut existing_reg = if history_path.is_file() {
        match HistoryRegistry::load_from_file(&history_path) {
            Ok(r) => r,
            Err(f) => return Err(vec![f]),
        }
    } else {
        if config.history {
            return Err(vec![Fault::catalog("history-missing")]);
        }
        HistoryRegistry {
            history_version: 1,
            records: Vec::new(),
        }
    };

    // Check existing records for mutation
    let mut src_ids: Vec<String> = source_file.messages.keys().cloned().collect();
    src_ids.sort();

    // Verify existing registry integrity
    check_registry_integrity(&existing_reg, &mut faults);

    // Verify existing records against their metadata
    for rec in &existing_reg.records {
        if source_file.messages.contains_key(&rec.id) && rec.tombstone {
            faults.push(Fault::new("tombstone-reuse", "*", &rec.id));
        }
        if let Some(meta) = meta_map.get(&rec.id) {
            let ns = rec.id.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
            let owner = namespace_owners.get(ns).cloned().unwrap_or_default();
            verify_history_record_against_metadata(rec, meta, &owner, &mut faults);
        }
    }

    if !faults.is_empty() {
        sort_faults(&mut faults);
        return Err(faults);
    }

    for (id, message) in &mut source_file.messages {
        let ns = id.rsplit_once('.').map(|(prefix, _)| prefix).unwrap_or("");
        if let Err(rule) = validate_message_id(id, ns) {
            faults.push(Fault::new(rule, "*", id));
        }
        if !namespace_owners
            .get(ns)
            .is_some_and(|owner| !owner.trim().is_empty())
        {
            faults.push(Fault::new("owner-conflict", "*", id));
        }
        if let Err(mut admission) = crate::admit::validate_admitted_message(
            message,
            &meta_map[id],
            &config.source_locale,
            true,
            None,
        ) {
            faults.append(&mut admission);
        }
    }
    if !faults.is_empty() {
        sort_faults(&mut faults);
        return Err(faults);
    }

    // Append new IDs
    let existing_ids: HashSet<String> = existing_reg.records.iter().map(|r| r.id.clone()).collect();
    for id in &src_ids {
        if !existing_ids.contains(id) {
            let meta = &meta_map[id];
            let ns = id.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
            let owner = namespace_owners.get(ns).cloned().unwrap_or_default();
            let resource = compute_resource_key(id);
            existing_reg.records.push(HistoryRecord {
                id: id.clone(),
                resource,
                namespace: ns.to_string(),
                owner,
                tombstone: false,
                replacement: None,
                format_intent: format!("{:?}", meta.format_intent).to_lowercase(),
                arguments: meta.arguments.clone(),
                protected_slots: meta.protected_slots.clone(),
                selector: meta.selector.clone(),
            });
        }
    }

    existing_reg.records.sort_by(|a, b| a.id.cmp(&b.id));

    if let Err(f) = existing_reg.save_to_file(&history_path) {
        return Err(vec![f]);
    }

    Ok(())
}

fn run_retire(args: &[String]) -> i32 {
    let mut catalog_dir = None;
    let mut id_to_retire = None;
    let mut replacement_id = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--id" if i + 1 < args.len() => {
                id_to_retire = Some(args[i + 1].clone());
                i += 2;
            }
            "--replacement" if i + 1 < args.len() => {
                replacement_id = Some(args[i + 1].clone());
                i += 2;
            }
            arg if !arg.starts_with("--") && catalog_dir.is_none() => {
                catalog_dir = Some(PathBuf::from(arg));
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    let catalog_path = match catalog_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing catalog directory");
            return 1;
        }
    };
    let target_id = match id_to_retire {
        Some(id) => id,
        None => {
            eprintln!("Missing --id parameter");
            return 1;
        }
    };

    let history_path = catalog_path.join("history.json");
    let mut reg = match HistoryRegistry::load_from_file(&history_path) {
        Ok(r) => r,
        Err(f) => {
            eprintln!("{}", f);
            return 1;
        }
    };

    if let Some(rep) = &replacement_id {
        let ns = rep.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
        if let Err(rule) = validate_message_id(rep, ns) {
            eprintln!("*:*:{}", rule);
            return 1;
        }
        if rep == &target_id {
            eprintln!("*:*:invalid-replacement");
            return 1;
        }
        if let Some(r_rec) = reg.records.iter().find(|r| &r.id == rep)
            && r_rec.tombstone
        {
            eprintln!("*:*:tombstone-reuse");
            return 1;
        }
    }

    let record = match reg.records.iter_mut().find(|r| r.id == target_id) {
        Some(r) => r,
        None => {
            eprintln!("*:*:message-not-found");
            return 1;
        }
    };

    record.tombstone = true;
    record.replacement = replacement_id;

    if let Err(f) = reg.save_to_file(&history_path) {
        eprintln!("{}", f);
        return 1;
    }

    0
}

fn run_lock(args: &[String]) -> i32 {
    let mut catalog_dir = None;
    let mut ack_locale = None;

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--acknowledge" && i + 1 < args.len() {
            ack_locale = Some(args[i + 1].clone());
            i += 2;
        } else if catalog_dir.is_none() {
            catalog_dir = Some(PathBuf::from(&args[i]));
            i += 1;
        } else {
            eprintln!("Usage: i18n-catalog lock <catalog> [--acknowledge <locale>]");
            return 1;
        }
    }

    let catalog_dir = match catalog_dir {
        Some(d) => d,
        None => {
            eprintln!("Usage: i18n-catalog lock <catalog> [--acknowledge <locale>]");
            return 1;
        }
    };

    if let Some(ref loc) = ack_locale
        && !crate::locale::validate_locale_tag(loc)
    {
        eprintln!("{}:*:locale-tag", loc);
        return 1;
    }

    let config_path = catalog_dir.join("catalog.json");
    let config_str = match fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("*:*:missing-catalog");
            return 1;
        }
    };
    let config: CatalogConfig = match serde_json::from_str(&config_str) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("*:*:missing-catalog");
            return 1;
        }
    };
    if config.catalog_version != 1 {
        eprintln!("*:*:unsupported-version");
        return 1;
    }
    if !crate::locale::validate_locale_tag(&config.source_locale) {
        eprintln!("{}:*:locale-tag", config.source_locale);
        return 1;
    }

    let source_path = catalog_dir
        .join("source")
        .join(format!("{}.ftl", config.source_locale));
    let source_content = match fs::read_to_string(&source_path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("*:*:source-missing");
            return 1;
        }
    };
    let mut source_file = match parse_ftl_resource(&source_content, &config.source_locale) {
        Ok(f) => f,
        Err(faults) => {
            for f in faults {
                eprintln!("{}", f);
            }
            return 1;
        }
    };

    let meta_dir = catalog_dir.join("meta");
    let lock_path = catalog_dir.join("source-lock.json");

    let existing_lock = if lock_path.exists() {
        match SourceLock::load_from_file(&lock_path) {
            Ok(lock) => Some(lock),
            Err(fault) => {
                eprintln!("{}", fault);
                return 1;
            }
        }
    } else {
        None
    };

    let mut new_messages = Vec::new();

    let mut sorted_ids: Vec<String> = source_file.messages.keys().cloned().collect();
    sorted_ids.sort();

    for id in &sorted_ids {
        let msg = source_file.messages.get_mut(id).unwrap();
        let meta_path = meta_dir.join(format!("{}.json", id));
        let meta_content = match fs::read_to_string(&meta_path) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("*:*:metadata-missing");
                return 1;
            }
        };
        let meta = match MessageMetadata::parse(id, &meta_content) {
            Ok(m) => m,
            Err(f) => {
                eprintln!("{}", f);
                return 1;
            }
        };

        if let Err(faults) =
            crate::admit::validate_admitted_message(msg, &meta, &config.source_locale, true, None)
        {
            for f in faults {
                eprintln!("{}", f);
            }
            return 1;
        }

        let src_hash = compute_source_hash(msg);
        let contract_hash = compute_contract_hash(&meta);
        let protected_hash = compute_protected_hash(&meta);

        let mut locales = match &existing_lock {
            Some(ex) => match ex.messages.iter().find(|m| &m.id == id) {
                Some(m) => m.locales.clone(),
                None => Vec::new(),
            },
            None => Vec::new(),
        };

        if let Some(ref ack_loc) = ack_locale {
            if let Some(ack) = locales.iter_mut().find(|a| &a.locale == ack_loc) {
                ack.source = src_hash.clone();
                ack.contract = contract_hash.clone();
                ack.protected = protected_hash.clone();
            } else {
                locales.push(LocaleAck {
                    locale: ack_loc.clone(),
                    source: src_hash.clone(),
                    contract: contract_hash.clone(),
                    protected: protected_hash.clone(),
                });
            }
        }
        locales.sort_by(|a, b| a.locale.cmp(&b.locale));

        new_messages.push(SourceLockMessage {
            id: id.clone(),
            source: src_hash,
            contract: contract_hash,
            protected: protected_hash,
            locales,
        });
    }

    new_messages.sort_by(|a, b| a.id.cmp(&b.id));

    let mut lock = SourceLock {
        lock_version: 1,
        messages: new_messages,
    };

    if let Err(f) = lock.save_to_file(&lock_path) {
        eprintln!("{}", f);
        return 1;
    }

    0
}

fn run_pseudo(args: &[String]) -> i32 {
    let mut out_dir = None;
    let mut catalog_dir = None;

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--out" && i + 1 < args.len() {
            out_dir = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else if catalog_dir.is_none() {
            catalog_dir = Some(PathBuf::from(&args[i]));
            i += 1;
        } else {
            i += 1;
        }
    }

    let cat_path = match catalog_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing catalog directory");
            return 1;
        }
    };
    let out_path = match out_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing --out argument");
            return 1;
        }
    };

    let loaded = match LoadedCatalog::load_source(&cat_path) {
        Ok(c) => c,
        Err(faults) => {
            for f in faults {
                eprintln!("{}", f);
            }
            return 1;
        }
    };

    if let Err(f) = write_pseudo_locales(&loaded.source_messages, &out_path) {
        eprintln!("{}", f);
        return 1;
    }

    0
}

fn run_coverage(args: &[String]) -> i32 {
    let (catalog_dir, channel, locale) = match parse_channel_and_locale_args(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };

    let catalog = match LoadedCatalog::load_and_validate(&catalog_dir, channel, locale.as_deref()) {
        Ok(c) => c,
        Err(faults) => {
            for f in faults {
                eprintln!("{}", f);
            }
            return 1;
        }
    };

    let mut all_bundles = HashSet::new();
    for meta in catalog.metadata.values() {
        for b in &meta.bundles {
            all_bundles.insert(b.clone());
        }
    }
    let all_bundle_names: Vec<String> = all_bundles.into_iter().collect();

    let eligible_locales =
        match catalog
            .registry
            .get_eligible_locales(channel, &all_bundle_names, locale.as_deref())
        {
            Ok(l) => l,
            Err(f) => {
                eprintln!("{}", f);
                return 1;
            }
        };

    let mut bundle_locales = HashMap::new();
    for b in &catalog.registry.bundles {
        let mut locs = Vec::new();
        for loc in &b.locales {
            if eligible_locales.contains(loc) {
                locs.push(loc.clone());
            }
        }
        locs.sort();
        bundle_locales.insert(b.id.clone(), locs);
    }

    let mut message_bundles = HashMap::new();
    for (id, meta) in &catalog.metadata {
        message_bundles.insert(id.clone(), meta.bundles.clone());
    }

    let stale_message_ids = HashSet::new(); // Already checked in load_and_validate

    let report = CoverageReport::compute(
        &bundle_locales,
        &message_bundles,
        &catalog.source_messages,
        &catalog.translations,
        &stale_message_ids,
    );

    print!("{}", report.format_stdout());
    0
}

fn run_import(args: &[String]) -> i32 {
    let mut bundle_dir = None;
    let mut dest_dir = None;

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dest" && i + 1 < args.len() {
            dest_dir = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else if bundle_dir.is_none() {
            bundle_dir = Some(PathBuf::from(&args[i]));
            i += 1;
        } else {
            i += 1;
        }
    }

    let bundle_path = match bundle_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing bundle directory");
            return 1;
        }
    };
    let dest_path = match dest_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing --dest directory");
            return 1;
        }
    };

    if let Err(f) = import_bundle(&bundle_path, &dest_path) {
        eprintln!("{}", f);
        return 1;
    }
    0
}

fn run_export(args: &[String]) -> i32 {
    let mut catalog_dir = None;
    let mut out_dir = None;

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--out" && i + 1 < args.len() {
            out_dir = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else if catalog_dir.is_none() {
            catalog_dir = Some(PathBuf::from(&args[i]));
            i += 1;
        } else {
            i += 1;
        }
    }

    let cat_path = match catalog_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing catalog directory");
            return 1;
        }
    };
    let out_path = match out_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing --out directory");
            return 1;
        }
    };

    if let Err(f) = export_bundle(&cat_path, &out_path) {
        eprintln!("{}", f);
        return 1;
    }
    0
}

fn parse_channel_and_locale_args(
    args: &[String],
) -> Result<(PathBuf, Channel, Option<String>), String> {
    if args.is_empty() {
        return Err("Missing catalog directory".to_string());
    }

    let catalog_dir = PathBuf::from(&args[0]);
    let mut channel = None;
    let mut locale = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--channel" if i + 1 < args.len() => {
                match args[i + 1].as_str() {
                    "development" => channel = Some(Channel::Development),
                    "production" => channel = Some(Channel::Production),
                    other => return Err(format!("Unknown channel: {}", other)),
                }
                i += 2;
            }
            "--locale" if i + 1 < args.len() => {
                locale = Some(args[i + 1].clone());
                i += 2;
            }
            other => return Err(format!("Unknown argument: {}", other)),
        }
    }

    let ch = channel.ok_or_else(|| "Missing required --channel argument".to_string())?;
    Ok((catalog_dir, ch, locale))
}

fn run_targets(args: &[String]) -> i32 {
    let mut catalog_dir = None;
    let mut channel = None;
    let mut locale = None;
    let mut out_dir = None;
    let mut targets = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--channel" if i + 1 < args.len() => {
                match args[i + 1].as_str() {
                    "development" => channel = Some(Channel::Development),
                    "production" => channel = Some(Channel::Production),
                    other => {
                        eprintln!("Unknown channel: {}", other);
                        return 1;
                    }
                }
                i += 2;
            }
            "--locale" if i + 1 < args.len() => {
                locale = Some(args[i + 1].clone());
                i += 2;
            }
            "--out" if i + 1 < args.len() => {
                out_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--target" if i + 1 < args.len() => {
                match crate::target::Target::parse(&args[i + 1]) {
                    Some(t) => targets.push(t),
                    None => {
                        eprintln!("Unknown target: {}", args[i + 1]);
                        return 1;
                    }
                }
                i += 2;
            }
            arg if !arg.starts_with("--") && catalog_dir.is_none() => {
                catalog_dir = Some(PathBuf::from(arg));
                i += 1;
            }
            other => {
                eprintln!("Unknown argument: {}", other);
                return 1;
            }
        }
    }

    let cat_path = match catalog_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing catalog directory");
            return 1;
        }
    };
    let ch = match channel {
        Some(c) => c,
        None => {
            eprintln!("Missing required --channel argument");
            return 1;
        }
    };
    let out_path = match out_dir {
        Some(p) => p,
        None => {
            eprintln!("Missing required --out argument");
            return 1;
        }
    };
    if targets.is_empty() {
        eprintln!("At least one --target is required");
        return 1;
    }

    let catalog = match LoadedCatalog::load_and_validate(&cat_path, ch, locale.as_deref()) {
        Ok(c) => c,
        Err(faults) => {
            for f in faults {
                eprintln!("{}", f);
            }
            return 1;
        }
    };

    if let Err(faults) =
        crate::target::publish(&catalog, ch, locale.as_deref(), &targets, &out_path)
    {
        for f in faults {
            eprintln!("{}", f);
        }
        return 1;
    }

    0
}
