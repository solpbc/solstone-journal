// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::admit::ParsedMessage;
use crate::fault::Fault;
use crate::hexutil::sha256_hex;
use crate::metadata::MessageMetadata;
use crate::publish::atomic_write_file;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocaleAck {
    pub locale: String,
    pub source: String,
    pub contract: String,
    pub protected: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLockMessage {
    pub id: String,
    pub source: String,
    pub contract: String,
    pub protected: String,
    pub locales: Vec<LocaleAck>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLock {
    pub lock_version: u32,
    pub messages: Vec<SourceLockMessage>,
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    let len = s.len() as u64;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

pub fn compute_source_hash(msg: &ParsedMessage) -> String {
    let mut bytes = Vec::new();
    for b in &msg.branches {
        let key_str = b.key.as_deref().unwrap_or("");
        put_str(&mut bytes, key_str);
        bytes.push(if b.default { 1 } else { 0 });
        let pattern_len = b.pattern.len() as u64;
        bytes.extend_from_slice(&pattern_len.to_be_bytes());
        for el in &b.pattern {
            put_str(&mut bytes, el.kind.as_str());
            put_str(&mut bytes, &el.value);
        }
    }
    sha256_hex(&bytes)
}

pub fn compute_contract_hash(meta: &MessageMetadata) -> String {
    let mut args = meta.arguments.clone();
    args.sort_by(|a, b| a.name.cmp(&b.name));
    let mut bytes = Vec::new();
    put_str(&mut bytes, meta.format_intent.as_str());
    let args_len = args.len() as u64;
    bytes.extend_from_slice(&args_len.to_be_bytes());
    for a in args {
        put_str(&mut bytes, &a.name);
        put_str(&mut bytes, a.arg_type.as_str());
        let num_style_str = a.numeric_style.map(|s| s.as_str()).unwrap_or("");
        put_str(&mut bytes, num_style_str);
        let min_str = a.min.map(|n| (n as i64).to_string()).unwrap_or_default();
        put_str(&mut bytes, &min_str);
        let max_str = a.max.map(|n| (n as i64).to_string()).unwrap_or_default();
        put_str(&mut bytes, &max_str);
    }
    if let Some(sel) = &meta.selector {
        bytes.push(1);
        put_str(&mut bytes, &sel.name);
        put_str(&mut bytes, &sel.selector_type);
        let ev_len = sel.enum_values.len() as u64;
        bytes.extend_from_slice(&ev_len.to_be_bytes());
        for v in &sel.enum_values {
            put_str(&mut bytes, v);
        }
        let def_str = sel.enum_default.as_deref().unwrap_or("");
        put_str(&mut bytes, def_str);
    } else {
        bytes.push(0);
    }
    sha256_hex(&bytes)
}

pub fn compute_protected_hash(meta: &MessageMetadata) -> String {
    let mut slots = meta.protected_slots.clone();
    slots.sort_by(|a, b| a.name.cmp(&b.name));
    let mut bytes = Vec::new();
    let slots_len = slots.len() as u64;
    bytes.extend_from_slice(&slots_len.to_be_bytes());
    for s in slots {
        put_str(&mut bytes, &s.name);
        put_str(&mut bytes, s.kind.as_str());
        put_str(&mut bytes, &s.value);
    }
    sha256_hex(&bytes)
}

impl SourceLock {
    pub fn load_from_file(path: &Path) -> Result<Self, Fault> {
        let content = std::fs::read_to_string(path).map_err(|_| Fault::catalog("lock-missing"))?;
        let lock: Self =
            serde_json::from_str(&content).map_err(|_| Fault::catalog("lock-syntax"))?;
        if lock.lock_version != 1 {
            return Err(Fault::catalog("unsupported-version"));
        }
        Ok(lock)
    }

    pub fn save_to_file(&mut self, path: &Path) -> Result<(), Fault> {
        self.messages.sort_by(|a, b| a.id.cmp(&b.id));
        for m in &mut self.messages {
            m.locales.sort_by(|a, b| a.locale.cmp(&b.locale));
        }
        let mut content =
            serde_json::to_string_pretty(self).map_err(|_| Fault::catalog("lock-syntax"))?;
        content.push('\n');
        atomic_write_file(path, content.as_bytes())
    }

    pub fn check_translation(
        &self,
        id: &str,
        locale: &str,
        current_source: &str,
        current_contract: &str,
        current_protected: &str,
    ) -> Result<(), Fault> {
        let msg = match self.messages.iter().find(|m| m.id == id) {
            Some(m) => m,
            None => return Err(Fault::new("stale", locale, id)),
        };
        let ack = match msg.locales.iter().find(|a| a.locale == locale) {
            Some(a) => a,
            None => return Err(Fault::new("stale", locale, id)),
        };
        if ack.source != current_source
            || ack.contract != current_contract
            || ack.protected != current_protected
        {
            return Err(Fault::new("stale", locale, id));
        }
        Ok(())
    }
}
