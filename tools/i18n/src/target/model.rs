// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::ParsedMessage;
use crate::locale::LocaleEntry;
use crate::metadata::MessageMetadata;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Target {
    Rust,
    Esm,
    Apple,
    Android,
    Webextension,
    Gettext,
}

impl Target {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rust" => Some(Self::Rust),
            "esm" => Some(Self::Esm),
            "apple" => Some(Self::Apple),
            "android" => Some(Self::Android),
            "webextension" => Some(Self::Webextension),
            "gettext" => Some(Self::Gettext),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Esm => "esm",
            Self::Apple => "apple",
            Self::Android => "android",
            Self::Webextension => "webextension",
            Self::Gettext => "gettext",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TargetMessage {
    pub id: String,
    pub resource_key: String,
    pub meta: MessageMetadata,
    pub translations: HashMap<String, ParsedMessage>,
}

#[derive(Clone, Debug)]
pub struct TargetBundle {
    pub id: String,
    pub locales: Vec<LocaleEntry>,
    pub messages: Vec<TargetMessage>,
}

pub fn enum_variant_resource_key(parent_resource_key: &str, variant: &str) -> String {
    let mut hex = String::with_capacity(variant.len() * 2);
    for b in variant.as_bytes() {
        use std::fmt::Write;
        let _ = write!(hex, "{:02x}", b);
    }
    format!("{}_v{}", parent_resource_key, hex)
}
