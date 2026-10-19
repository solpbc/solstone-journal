// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod admit;
pub mod bundle;
pub mod catalog;
pub mod cli;
pub mod coverage;
pub mod fault;
pub mod freshness;
pub mod hexutil;
pub mod history;
pub mod ir;
pub mod locale;
pub mod metadata;
pub mod model;
pub mod protected;
pub mod pseudo;
pub mod publish;
pub mod scalar;
pub mod vocab;

#[cfg(test)]
mod testutil;

use catalog::LoadedCatalog;
use fault::Fault;
use ir::build_ir;
use publish::atomic_write_file;
use std::collections::HashSet;
use std::path::Path;
use vocab::Channel;

pub fn compile(
    catalog_dir: &Path,
    channel: Channel,
    locale: Option<&str>,
) -> Result<(), Vec<Fault>> {
    LoadedCatalog::load_and_validate(catalog_dir, channel, locale)?;
    Ok(())
}

pub fn emit(
    catalog_dir: &Path,
    channel: Channel,
    locale: Option<&str>,
    out_file: &Path,
) -> Result<(), Vec<Fault>> {
    let catalog = LoadedCatalog::load_and_validate(catalog_dir, channel, locale)?;

    let mut all_bundles = HashSet::new();
    for meta in catalog.metadata.values() {
        for b in &meta.bundles {
            all_bundles.insert(b.clone());
        }
    }
    let all_bundle_names: Vec<String> = all_bundles.into_iter().collect();

    let eligible_locales = catalog
        .registry
        .get_eligible_locales(channel, &all_bundle_names, locale)
        .map_err(|f| vec![f])?;

    let ir = build_ir(
        channel,
        &catalog.config.source_locale,
        &eligible_locales,
        &catalog.source_messages,
        &catalog.metadata,
        &catalog.translations,
        &catalog.registry.get_namespace_owners(),
    );

    let mut ir_json = serde_json::to_string(&ir).map_err(|_| vec![Fault::catalog("io-error")])?;
    ir_json.push('\n');

    atomic_write_file(out_file, ir_json.as_bytes()).map_err(|f| vec![f])?;

    Ok(())
}
