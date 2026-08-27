// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Endpoint-independent profile response composition.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};

use crate::cadence::{compute_cadence, list_active_entity_ids};
use crate::error::ProfileResult;
use crate::ledger_fold::{DecisionQuery, LedgerListQuery, LedgerState, decisions, list};
use crate::relationships::{description_for, load_facet_descriptions, selected_facets};
use crate::resolution::resolve_target;
use crate::types::{Cadence, Profile, ProfileBrief};

pub(crate) fn full(
    journal_root: &Path,
    name: &str,
    facets: Option<&[String]>,
    include_mentions: bool,
    now: DateTime<Utc>,
) -> ProfileResult<Option<Profile>> {
    let Some(target) = resolve_target(journal_root, name)? else {
        return Ok(None);
    };
    let descriptions = load_facet_descriptions(journal_root, &target)?;
    let (cadence, sources) =
        compute_cadence(journal_root, &target.entity_id, include_mentions, now)?;
    let open_with_them = list(
        journal_root,
        now,
        ledger_query(LedgerState::Open, &target.entity_id, None),
    )?;
    let closed_with_them_30d = list(
        journal_root,
        now,
        ledger_query(
            LedgerState::Closed,
            &target.entity_id,
            Some(day_minus(now, 30)),
        ),
    )?;
    let decisions_involving_them = decisions(
        journal_root,
        DecisionQuery {
            owner: None,
            involving: Some(target.entity_id.clone()),
            since: None,
            top: None,
            facets: None,
        },
    )?;

    Ok(Some(Profile {
        entity_id: target.entity_id,
        name: target.name,
        r#type: target.r#type,
        aka: target.aka,
        is_self: target.is_self,
        facets: selected_facets(&descriptions, facets),
        description: description_for(&descriptions, facets),
        cadence,
        open_with_them,
        closed_with_them_30d,
        decisions_involving_them,
        sources,
        generated_at: now.timestamp_millis(),
    }))
}

pub(crate) fn brief(
    journal_root: &Path,
    name: &str,
    now: DateTime<Utc>,
) -> ProfileResult<Option<ProfileBrief>> {
    let Some(target) = resolve_target(journal_root, name)? else {
        return Ok(None);
    };
    let descriptions = load_facet_descriptions(journal_root, &target)?;
    let (cadence, _) = compute_cadence(journal_root, &target.entity_id, false, now)?;
    let open_loop_count = list(
        journal_root,
        now,
        ledger_query(LedgerState::Open, &target.entity_id, None),
    )?
    .len();
    let decisions_count_30d = decisions(
        journal_root,
        DecisionQuery {
            owner: None,
            involving: Some(target.entity_id.clone()),
            since: Some(day_minus(now, 30)),
            top: None,
            facets: None,
        },
    )?
    .len();

    Ok(Some(ProfileBrief {
        entity_id: target.entity_id,
        name: target.name,
        r#type: target.r#type,
        description: description_for(&descriptions, None),
        last_seen: cadence.last_seen,
        open_loop_count,
        decisions_count_30d,
    }))
}

pub(crate) fn cadence(
    journal_root: &Path,
    name: &str,
    include_mentions: bool,
    now: DateTime<Utc>,
) -> ProfileResult<Option<Cadence>> {
    let Some(target) = resolve_target(journal_root, name)? else {
        return Ok(None);
    };
    compute_cadence(journal_root, &target.entity_id, include_mentions, now)
        .map(|(cadence, _)| Some(cadence))
}

pub(crate) fn list_active(
    journal_root: &Path,
    window_days: i64,
    now: DateTime<Utc>,
) -> ProfileResult<Vec<String>> {
    list_active_entity_ids(journal_root, window_days, now)
}

fn ledger_query(
    state: LedgerState,
    counterparty: &str,
    closed_since: Option<String>,
) -> LedgerListQuery {
    LedgerListQuery {
        state,
        owner: None,
        counterparty: Some(counterparty.to_owned()),
        age_days_gte: None,
        closed_since,
        top: None,
        sort: None,
        facets: None,
    }
}

fn day_minus(now: DateTime<Utc>, days: i64) -> String {
    (now.date_naive() - Duration::days(days))
        .format("%Y%m%d")
        .to_string()
}
