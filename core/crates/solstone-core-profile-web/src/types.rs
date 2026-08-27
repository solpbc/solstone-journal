// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Legacy-compatible profile response values.

use serde::Serialize;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ActivitySourceRef {
    pub facet: String,
    pub day: String,
    pub activity_id: String,
    pub field: String,
    pub created_at: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct LedgerItem {
    pub id: String,
    pub state: String,
    pub owner: String,
    pub owner_entity_id: Option<String>,
    pub counterparty: Option<String>,
    pub counterparty_entity_id: Option<String>,
    pub action: String,
    pub summary: String,
    pub when: Option<String>,
    pub context: String,
    pub opened_at: i64,
    pub closed_at: Option<i64>,
    pub age_days: i64,
    pub sources: Vec<ActivitySourceRef>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Decision {
    pub id: String,
    pub owner: String,
    pub owner_entity_id: Option<String>,
    pub action: String,
    pub context: String,
    pub day: String,
    pub created_at: i64,
    pub source: ActivitySourceRef,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Cadence {
    pub recent_interactions_count_30d: i64,
    pub last_seen: Option<String>,
    pub avg_interval_days: Option<f64>,
    pub gone_quiet_since: Option<i64>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ProfileBrief {
    pub entity_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub description: Option<String>,
    pub last_seen: Option<String>,
    pub open_loop_count: usize,
    pub decisions_count_30d: usize,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Profile {
    pub entity_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub aka: Vec<String>,
    pub is_self: bool,
    pub facets: Vec<String>,
    pub description: Option<String>,
    pub cadence: Cadence,
    pub open_with_them: Vec<LedgerItem>,
    pub closed_with_them_30d: Vec<LedgerItem>,
    pub decisions_involving_them: Vec<Decision>,
    pub sources: Vec<ActivitySourceRef>,
    pub generated_at: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ActiveCollection {
    pub items: Vec<String>,
    pub total: usize,
}
