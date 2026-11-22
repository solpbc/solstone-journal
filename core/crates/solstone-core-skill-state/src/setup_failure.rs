// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};

pub const DISPLAY_LOCALE_ENV: &str = "SOLSTONE_SETUP_SKILLS_DISPLAY_LOCALE";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupSkillFailure {
    skills_failure_v1: String,
    locale: String,
    display: String,
}

pub fn encode(source: String, locale: String, display: String) -> String {
    serde_json::to_string(&SetupSkillFailure {
        skills_failure_v1: source,
        locale,
        display,
    })
    .expect("skill failure strings are serializable")
}

pub fn display(stdout: &str, stderr: &str, locale: &str) -> Option<String> {
    let row: SetupSkillFailure = serde_json::from_str(stdout).ok()?;
    let remainder = stderr.strip_prefix(&row.skills_failure_v1)?;
    (row.locale == locale && !row.skills_failure_v1.is_empty() && remainder.starts_with('\n'))
        .then_some(row.display)
}
