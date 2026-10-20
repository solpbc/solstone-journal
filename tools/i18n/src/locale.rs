// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::vocab::{Channel, LocalePurpose, LocaleTier, ReleaseState};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub fn validate_locale_tag(tag: &str) -> bool {
    if tag.is_empty() {
        return false;
    }
    let bytes = tag.as_bytes();
    if !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'-')
}

pub fn validate_webextension_folder(folder: &str) -> bool {
    if let Some((lang, region)) = folder.split_once('_') {
        if region.contains('_') {
            return false;
        }
        let lang_bytes = lang.as_bytes();
        if !(lang_bytes.len() == 2 || lang_bytes.len() == 3)
            || !lang_bytes.iter().all(|b| b.is_ascii_lowercase())
        {
            return false;
        }
        let reg_bytes = region.as_bytes();
        if !(reg_bytes.len() >= 2 && reg_bytes.len() <= 4) {
            return false;
        }
        if !reg_bytes[0].is_ascii_uppercase() {
            return false;
        }
        reg_bytes[1..].iter().all(|b| b.is_ascii_alphabetic())
    } else {
        let bytes = folder.as_bytes();
        (bytes.len() == 2 || bytes.len() == 3) && bytes.iter().all(|b| b.is_ascii_lowercase())
    }
}

pub fn validate_android_qualifier(q: &str) -> bool {
    if let Some((lang, reg)) = q.split_once("-r") {
        let lang_bytes = lang.as_bytes();
        let reg_bytes = reg.as_bytes();
        (lang_bytes.len() == 2 || lang_bytes.len() == 3)
            && lang_bytes.iter().all(|b| b.is_ascii_lowercase())
            && reg_bytes.len() == 2
            && reg_bytes.iter().all(|b| b.is_ascii_uppercase())
    } else {
        let bytes = q.as_bytes();
        (bytes.len() == 2 || bytes.len() == 3) && bytes.iter().all(|b| b.is_ascii_lowercase())
    }
}

pub fn is_standard_formatting_locale(tag: &str) -> bool {
    if let Some((lang, region)) = tag.split_once('-') {
        let lang_bytes = lang.as_bytes();
        let reg_bytes = region.as_bytes();
        (lang_bytes.len() == 2 || lang_bytes.len() == 3)
            && lang_bytes.iter().all(|b| b.is_ascii_lowercase())
            && reg_bytes.len() == 2
            && reg_bytes.iter().all(|b| b.is_ascii_uppercase())
    } else {
        let bytes = tag.as_bytes();
        (bytes.len() == 2 || bytes.len() == 3) && bytes.iter().all(|b| b.is_ascii_lowercase())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocaleEntry {
    pub tag: String,
    pub tier: LocaleTier,
    pub purpose: LocalePurpose,
    pub development: ReleaseState,
    pub production: ReleaseState,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cardinal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webextension_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_qualifier: Option<String>,
}

impl LocaleEntry {
    pub fn is_eligible_for_channel(&self, channel: Channel) -> bool {
        match channel {
            Channel::Production => {
                matches!(self.tier, LocaleTier::Source | LocaleTier::Release)
                    && self.purpose == LocalePurpose::Ordinary
                    && self.production == ReleaseState::Released
            }
            Channel::Development => self.development == ReleaseState::Released,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamespaceEntry {
    pub namespace: String,
    pub owner: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleEntry {
    pub id: String,
    pub locales: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocaleRegistry {
    pub locales: Vec<LocaleEntry>,
    pub namespaces: Vec<NamespaceEntry>,
    pub bundles: Vec<BundleEntry>,
}

impl LocaleRegistry {
    pub fn embedded() -> Result<Self, Fault> {
        let json_str = include_str!("../locales.json");
        Self::load(json_str)
    }

    pub fn load(json_str: &str) -> Result<Self, Fault> {
        let reg: Self =
            serde_json::from_str(json_str).map_err(|_| Fault::catalog("locales-syntax"))?;

        let mut locale_tags = HashSet::new();
        for locale in &reg.locales {
            if !validate_locale_tag(&locale.tag) {
                return Err(Fault::catalog("locale-tag"));
            }
            if !locale_tags.insert(locale.tag.as_str()) {
                return Err(Fault::catalog("duplicate-locale"));
            }
            if let Some(cardinal) = &locale.cardinal
                && cardinal != "en"
                && cardinal != "ar"
            {
                return Err(Fault::catalog("plural-rule"));
            }
            if let Some(folder) = &locale.webextension_folder
                && !validate_webextension_folder(folder)
            {
                return Err(Fault::catalog("webextension-folder"));
            }
            if let Some(qualifier) = &locale.android_qualifier
                && !validate_android_qualifier(qualifier)
            {
                return Err(Fault::catalog("locale-qualifier"));
            }
        }

        // Check for duplicate namespaces
        let mut seen_ns = HashSet::new();
        for ns in &reg.namespaces {
            if ns.owner.trim().is_empty() {
                return Err(Fault::catalog("owner-conflict"));
            }
            if !seen_ns.insert(&ns.namespace) {
                return Err(Fault::catalog("duplicate-namespace"));
            }
        }

        let mut bundle_ids = HashSet::new();
        for bundle in &reg.bundles {
            if bundle.id.is_empty()
                || bundle.id.contains("..")
                || !bundle
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            {
                return Err(Fault::catalog("bundle-id"));
            }
            if !bundle_ids.insert(bundle.id.as_str()) {
                return Err(Fault::catalog("duplicate-bundle"));
            }
            let mut membership = HashSet::new();
            for tag in &bundle.locales {
                if !locale_tags.contains(tag.as_str()) {
                    return Err(Fault::catalog("unknown-locale"));
                }
                if !membership.insert(tag.as_str()) {
                    return Err(Fault::catalog("duplicate-locale-membership"));
                }
            }
        }

        Ok(reg)
    }

    pub fn get_namespace_owners(&self) -> HashMap<String, String> {
        self.namespaces
            .iter()
            .map(|n| (n.namespace.clone(), n.owner.clone()))
            .collect()
    }

    pub fn get_eligible_locales(
        &self,
        channel: Channel,
        bundle_names: &[String],
        requested_locale: Option<&str>,
    ) -> Result<Vec<String>, Fault> {
        if bundle_names.is_empty() {
            return Err(Fault::catalog("empty-membership"));
        }

        let mut bundle_locales = HashSet::new();
        for b_name in bundle_names {
            let bundle = self
                .bundles
                .iter()
                .find(|b| &b.id == b_name)
                .ok_or_else(|| Fault::catalog("undeclared-membership"))?;
            for loc in &bundle.locales {
                bundle_locales.insert(loc.as_str());
            }
        }

        if let Some(req) = requested_locale {
            let entry = self
                .locales
                .iter()
                .find(|l| l.tag == req)
                .ok_or_else(|| Fault::catalog("locale-channel"))?;
            if !entry.is_eligible_for_channel(channel) {
                return Err(Fault::catalog("locale-channel"));
            }
            if !bundle_locales.contains(req) {
                return Err(Fault::catalog("locale-channel"));
            }
            return Ok(vec![req.to_string()]);
        }

        let mut selected = Vec::new();
        for entry in &self.locales {
            if bundle_locales.contains(entry.tag.as_str()) && entry.is_eligible_for_channel(channel)
            {
                selected.push(entry.tag.clone());
            }
        }
        selected.sort();
        Ok(selected)
    }
}
