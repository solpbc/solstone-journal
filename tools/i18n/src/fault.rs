// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Fault {
    pub message_id: String,
    pub locale: String,
    pub rule: &'static str,
}

impl Fault {
    pub fn new(
        rule: &'static str,
        locale: impl Into<String>,
        message_id: impl Into<String>,
    ) -> Self {
        Self {
            rule,
            locale: locale.into(),
            message_id: message_id.into(),
        }
    }

    pub fn catalog(rule: &'static str) -> Self {
        Self {
            rule,
            locale: "*".to_string(),
            message_id: "*".to_string(),
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.message_id, self.locale, self.rule)
    }
}

pub fn sort_faults(faults: &mut [Fault]) {
    faults.sort_by(|a, b| {
        a.message_id
            .cmp(&b.message_id)
            .then_with(|| a.locale.cmp(&b.locale))
            .then_with(|| a.rule.cmp(b.rule))
    });
}
