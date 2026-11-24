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

// One shared WebExtension target uses Chrome's documented locale set. Firefox
// and Safari can load additional locales, but Chrome silently ignores them.
pub fn validate_webextension_folder(folder: &str) -> bool {
    matches!(
        folder,
        "ar" | "am"
            | "bg"
            | "bn"
            | "ca"
            | "cs"
            | "da"
            | "de"
            | "el"
            | "en"
            | "en_AU"
            | "en_GB"
            | "en_US"
            | "es"
            | "es_419"
            | "et"
            | "fa"
            | "fi"
            | "fil"
            | "fr"
            | "gu"
            | "he"
            | "hi"
            | "hr"
            | "hu"
            | "id"
            | "it"
            | "ja"
            | "kn"
            | "ko"
            | "lt"
            | "lv"
            | "ml"
            | "mr"
            | "ms"
            | "nl"
            | "no"
            | "pl"
            | "pt_BR"
            | "pt_PT"
            | "ro"
            | "ru"
            | "sk"
            | "sl"
            | "sr"
            | "sv"
            | "sw"
            | "ta"
            | "te"
            | "th"
            | "tr"
            | "uk"
            | "vi"
            | "zh_CN"
            | "zh_TW"
    )
}

fn is_language(s: &str) -> bool {
    (s.len() == 2 || s.len() == 3) && s.bytes().all(|b| b.is_ascii_lowercase())
}

fn is_region(s: &str) -> bool {
    (s.len() == 2 && s.bytes().all(|b| b.is_ascii_uppercase()))
        || (s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()))
}

// Android's resource locale stores language, script, region and one variant.
// Extensions, private use, extlangs and multiple variants are not representable.
fn validate_android_bcp47(locale: &str) -> bool {
    let mut parts = locale.split('+').peekable();
    if !parts.next().is_some_and(is_language) {
        return false;
    }
    if parts.peek().is_some_and(|s| {
        s.len() == 4
            && s.as_bytes()[0].is_ascii_uppercase()
            && s.as_bytes()[1..].iter().all(|b| b.is_ascii_lowercase())
    }) {
        parts.next();
    }
    if parts.peek().is_some_and(|s| is_region(s)) {
        parts.next();
    }
    if parts.peek().is_some_and(|s| {
        ((5..=8).contains(&s.len()) || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit()))
            && s.bytes().all(|b| b.is_ascii_alphanumeric())
    }) {
        parts.next();
    }
    parts.next().is_none()
}

pub fn validate_android_qualifier(q: &str) -> bool {
    if let Some(locale) = q.strip_prefix("b+") {
        validate_android_bcp47(locale)
    } else if let Some((lang, reg)) = q.split_once("-r") {
        is_language(lang) && lang != "car" && reg.len() == 2 && is_region(reg)
    } else {
        is_language(q) && q != "car"
    }
}

// Keep traditional folders for simple tags. Use b+ when a script, numeric
// region or variant needs it. Private-use tags require an explicit projection.
pub fn android_qualifier_from_tag(tag: &str) -> Option<String> {
    let qualifier = if is_language(tag) && tag != "car" {
        tag.to_string()
    } else if let Some((lang, region)) = tag.split_once('-')
        && is_language(lang)
        && lang != "car"
        && region.len() == 2
        && is_region(region)
    {
        format!("{lang}-r{region}")
    } else {
        format!("b+{}", tag.replace('-', "+"))
    };
    validate_android_qualifier(&qualifier).then_some(qualifier)
}

pub fn is_standard_formatting_locale(tag: &str) -> bool {
    if let Some((lang, region)) = tag.split_once('-') {
        is_language(lang) && is_region(region)
    } else {
        is_language(tag)
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
    /// Returns true if the registry declares `tag` at `tier: "draft"`.
    pub fn is_draft(&self, tag: &str) -> bool {
        self.locales
            .iter()
            .any(|locale| locale.tag == tag && locale.tier == LocaleTier::Draft)
    }

    /// The plural family a locale declares, if it declares one.
    pub fn cardinal_family(&self, tag: &str) -> Option<&'static crate::plural::CardinalFamily> {
        self.locales
            .iter()
            .find(|locale| locale.tag == tag)
            .and_then(|locale| locale.cardinal.as_deref())
            .and_then(crate::plural::family)
    }

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
            if let Some(cardinal) = &locale.cardinal {
                if crate::plural::family(cardinal).is_none() {
                    return Err(Fault::catalog("plural-rule"));
                }
                // A family that compiles can still select the wrong form, so an
                // ordinary tag declares one its language is known to use.
                if locale.purpose == LocalePurpose::Ordinary
                    && !locale.tag.contains("-x-")
                    && !crate::plural::language_families(&locale.tag)
                        .is_some_and(|families| families.contains(&cardinal.as_str()))
                {
                    return Err(Fault::catalog("plural-family"));
                }
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
        let requested: Vec<String> = requested_locale.into_iter().map(str::to_string).collect();
        self.get_eligible_locales_for(channel, bundle_names, &requested)
    }

    pub fn get_eligible_locales_for(
        &self,
        channel: Channel,
        bundle_names: &[String],
        requested_locales: &[String],
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

        if !requested_locales.is_empty() {
            let mut selected = Vec::new();
            let mut seen = HashSet::new();
            for req in requested_locales {
                let entry = self
                    .locales
                    .iter()
                    .find(|l| l.tag == *req)
                    .ok_or_else(|| Fault::catalog("locale-channel"))?;
                if !entry.is_eligible_for_channel(channel) {
                    return Err(Fault::catalog("locale-channel"));
                }
                if !bundle_locales.contains(req.as_str()) {
                    return Err(Fault::catalog("locale-channel"));
                }
                if seen.insert(req.as_str()) {
                    selected.push(req.clone());
                }
            }
            selected.sort();
            return Ok(selected);
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

pub fn resolve_namespace_owner<'a>(
    namespace_owners: &'a HashMap<String, String>,
    namespace: &str,
) -> Option<&'a str> {
    let mut candidate = namespace;
    loop {
        if let Some(owner) = namespace_owners.get(candidate) {
            return (!owner.trim().is_empty()).then_some(owner.as_str());
        }
        match candidate.rsplit_once('.') {
            Some((parent, _)) => candidate = parent,
            None => return None,
        }
    }
}
