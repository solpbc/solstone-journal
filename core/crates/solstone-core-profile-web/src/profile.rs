// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Endpoint-independent profile response composition.

use std::path::Path;

use chrono::{DateTime, FixedOffset};

use crate::cadence::{compute_cadence, list_active_entity_ids};
use crate::error::ProfileResult;
use crate::relationships::{
    description_for, detached_facets, load_facet_descriptions, selected_facets,
};
use crate::resolution::resolve_target;
use crate::types::{Cadence, Profile, ProfileBrief};

pub(crate) fn full(
    journal_root: &Path,
    name: &str,
    facets: Option<&[String]>,
    include_mentions: bool,
    now: DateTime<FixedOffset>,
) -> ProfileResult<Option<Profile>> {
    let Some(target) = resolve_target(journal_root, name)? else {
        return Ok(None);
    };
    let descriptions = load_facet_descriptions(journal_root, &target)?;
    let (cadence, sources) =
        compute_cadence(journal_root, &target.entity_id, include_mentions, now)?;

    Ok(Some(Profile {
        entity_id: target.entity_id,
        name: target.name,
        r#type: target.r#type,
        aka: target.aka,
        is_self: target.is_self,
        blocked: target.blocked,
        facets: selected_facets(&descriptions, facets),
        detached_facets: detached_facets(&descriptions),
        description: description_for(&descriptions, facets),
        cadence,
        sources,
        generated_at: now.timestamp_millis(),
    }))
}

pub(crate) fn brief(
    journal_root: &Path,
    name: &str,
    now: DateTime<FixedOffset>,
) -> ProfileResult<Option<ProfileBrief>> {
    let Some(target) = resolve_target(journal_root, name)? else {
        return Ok(None);
    };
    let descriptions = load_facet_descriptions(journal_root, &target)?;
    let (cadence, _) = compute_cadence(journal_root, &target.entity_id, false, now)?;

    Ok(Some(ProfileBrief {
        entity_id: target.entity_id,
        name: target.name,
        r#type: target.r#type,
        blocked: target.blocked,
        description: description_for(&descriptions, None),
        last_seen: cadence.last_seen,
    }))
}

pub(crate) fn cadence(
    journal_root: &Path,
    name: &str,
    include_mentions: bool,
    now: DateTime<FixedOffset>,
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
    now: DateTime<FixedOffset>,
) -> ProfileResult<Vec<String>> {
    list_active_entity_ids(journal_root, window_days, now)
}
