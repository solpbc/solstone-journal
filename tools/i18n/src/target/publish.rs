// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::bundle;
use crate::catalog::LoadedCatalog;
use crate::fault::Fault;
use crate::target::android::generate_android_target;
use crate::target::apple::generate_apple_target;
use crate::target::cases::generate_cases;
use crate::target::esm::generate_esm_target;
use crate::target::gettext::generate_gettext_target;
use crate::target::model::{Target, TargetBundle, TargetMessage};
use crate::target::oracle;
use crate::target::rust::generate_rust_target;
use crate::target::webextension::generate_webextension_target;
use crate::vocab::Channel;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TARGET_COUNTER: AtomicU64 = AtomicU64::new(0);

fn canonicalize_or_closest(path: &Path) -> Result<PathBuf, std::io::Error> {
    if path.exists() {
        fs::canonicalize(path)
    } else {
        let mut stack = Vec::new();
        let mut cur = path;
        while !cur.exists() {
            if let Some(name) = cur.file_name() {
                stack.push(name);
                if let Some(p) = cur.parent() {
                    cur = p;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        let cur_to_canon = if cur.as_os_str().is_empty() {
            Path::new(".")
        } else {
            cur
        };
        let mut canon = fs::canonicalize(cur_to_canon)?;
        for component in stack.into_iter().rev() {
            canon.push(component);
        }
        Ok(canon)
    }
}

pub fn publish(
    catalog: &LoadedCatalog,
    channel: Channel,
    requested_locale: Option<&str>,
    targets: &[Target],
    out_dir: &Path,
) -> Result<(), Vec<Fault>> {
    // 1. Verify oracle probes
    oracle::verify_oracle_probes().map_err(|f| vec![f])?;

    // 2. Validate destination safety
    if let Ok(meta) = fs::symlink_metadata(out_dir)
        && meta.file_type().is_symlink()
    {
        return Err(vec![Fault::catalog("destination-link")]);
    }

    let cat_canon =
        fs::canonicalize(&catalog.root).map_err(|_| vec![Fault::catalog("io-error")])?;
    let out_canon =
        canonicalize_or_closest(out_dir).map_err(|_| vec![Fault::catalog("io-error")])?;
    if out_canon == cat_canon
        || out_canon.starts_with(&cat_canon)
        || cat_canon.starts_with(&out_canon)
    {
        return Err(vec![Fault::catalog("destination-overlap")]);
    }

    if out_dir.exists() {
        if !out_dir.is_dir() {
            return Err(vec![Fault::catalog("destination-not-dir")]);
        }
        let mut entries = fs::read_dir(out_dir).map_err(|_| vec![Fault::catalog("io-error")])?;
        if entries.next().is_some() {
            let marker_path = out_dir.join("i18n-catalog-targets");
            let is_valid_marker = match fs::symlink_metadata(&marker_path) {
                Ok(m) if m.file_type().is_file() => match fs::read(&marker_path) {
                    Ok(bytes) => bytes == b"i18n-catalog-targets\n",
                    Err(_) => false,
                },
                _ => false,
            };
            if !is_valid_marker {
                return Err(vec![Fault::catalog("destination-unmarked")]);
            }
        }
    }

    // 3. Build TargetBundles
    let mut used_bundle_ids = HashSet::new();
    for meta in catalog.metadata.values() {
        for b in &meta.bundles {
            used_bundle_ids.insert(b.clone());
        }
    }
    let mut used_bundle_list: Vec<String> = used_bundle_ids.into_iter().collect();
    used_bundle_list.sort();

    let mut bundles = Vec::new();
    for b_id in &used_bundle_list {
        let b_entry = match catalog.registry.bundles.iter().find(|b| &b.id == b_id) {
            Some(b) => b,
            None => continue,
        };

        let mut eligible_locales = Vec::new();
        for loc_tag in &b_entry.locales {
            if let Some(req) = requested_locale
                && loc_tag != req
            {
                continue;
            }
            if let Some(loc_entry) = catalog.registry.locales.iter().find(|l| &l.tag == loc_tag)
                && loc_entry.is_eligible_for_channel(channel)
            {
                eligible_locales.push(loc_entry.clone());
            }
        }

        if eligible_locales.is_empty() {
            continue;
        }

        let mut b_messages = Vec::new();
        let mut msg_ids: Vec<String> = catalog
            .metadata
            .iter()
            .filter(|(_, meta)| meta.bundles.contains(b_id))
            .map(|(id, _)| id.clone())
            .collect();
        msg_ids.sort();

        for id in msg_ids {
            let meta = catalog.metadata[&id].clone();
            let resource_key = crate::history::compute_resource_key(&id);

            let mut translations = HashMap::new();
            for loc in &eligible_locales {
                if loc.tag == catalog.config.source_locale {
                    if let Some(parsed) = catalog.source_messages.get(&id) {
                        translations.insert(loc.tag.clone(), parsed.clone());
                    } else {
                        return Err(vec![Fault::new("missing-translation", &loc.tag, &id)]);
                    }
                } else if let Some(tr_map) = catalog.translations.get(&loc.tag)
                    && let Some(parsed) = tr_map.get(&id)
                {
                    translations.insert(loc.tag.clone(), parsed.clone());
                } else {
                    return Err(vec![Fault::new("missing-translation", &loc.tag, &id)]);
                }
            }

            b_messages.push(TargetMessage {
                id,
                resource_key,
                meta,
                translations,
            });
        }

        if b_messages.is_empty() {
            continue;
        }

        bundles.push(TargetBundle {
            id: b_id.clone(),
            locales: eligible_locales,
            messages: b_messages,
        });
    }

    // 4. Create unique stage directory
    let parent = out_dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|_| vec![Fault::catalog("io-error")])?;

    let dir_name = out_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("out");
    let stage_dir =
        bundle::unique_stage_dir(parent, dir_name, &TARGET_COUNTER).map_err(|f| vec![f])?;

    // 5. Generate requested targets into stage_dir
    let vendor_src_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor")
        .join("fluent-js");

    let generate_result = (|| -> Result<(), Vec<Fault>> {
        for &tgt in targets {
            match tgt {
                Target::Rust => {
                    for bundle in &bundles {
                        generate_rust_target(bundle, &stage_dir)?;
                    }
                }
                Target::Esm => {
                    for bundle in &bundles {
                        generate_esm_target(bundle, &stage_dir, &vendor_src_dir)?;
                    }
                }
                Target::Apple => {
                    for bundle in &bundles {
                        generate_apple_target(bundle, &stage_dir, &catalog.config.source_locale)?;
                    }
                }
                Target::Android => {
                    for bundle in &bundles {
                        generate_android_target(bundle, &stage_dir)?;
                    }
                }
                Target::Webextension => {
                    for bundle in &bundles {
                        generate_webextension_target(bundle, &stage_dir)?;
                    }
                }
                Target::Gettext => {
                    for bundle in &bundles {
                        generate_gettext_target(bundle, &stage_dir)?;
                    }
                }
            }
        }

        // Generate cases.json
        let cases_json = generate_cases(&bundles, targets, channel)?;
        let mut cases_str = serde_json::to_string_pretty(&cases_json)
            .map_err(|_| vec![Fault::catalog("io-error")])?;
        cases_str.push('\n');

        let cases_path = stage_dir.join("cases.json");
        fs::write(&cases_path, cases_str.as_bytes())
            .map_err(|_| vec![Fault::catalog("io-error")])?;

        // Write marker file
        fs::write(
            stage_dir.join("i18n-catalog-targets"),
            b"i18n-catalog-targets\n",
        )
        .map_err(|_| vec![Fault::catalog("io-error")])?;

        Ok(())
    })();

    if let Err(faults) = generate_result {
        let _ = fs::remove_dir_all(&stage_dir);
        return Err(faults);
    }

    // 6. Atomic replacement onto out_dir
    if !out_dir.exists() {
        if fs::rename(&stage_dir, out_dir).is_err() {
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(vec![Fault::catalog("io-error")]);
        }
    } else {
        // Claim backup name exclusively with create_dir
        let backup_path = loop {
            let count = TARGET_COUNTER.fetch_add(1, Ordering::Relaxed);
            let p = bundle::staging_path(parent, dir_name, count);
            match fs::create_dir(&p) {
                Ok(_) => break p,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    let _ = fs::remove_dir_all(&stage_dir);
                    return Err(vec![Fault::catalog("io-error")]);
                }
            }
        };

        if fs::rename(out_dir, &backup_path).is_err() {
            let _ = fs::remove_dir(&backup_path);
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(vec![Fault::catalog("io-error")]);
        }

        if fs::rename(&stage_dir, out_dir).is_err() {
            let _ = fs::rename(&backup_path, out_dir);
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(vec![Fault::catalog("io-error")]);
        }

        let _ = fs::remove_dir_all(&backup_path);
    }

    Ok(())
}
