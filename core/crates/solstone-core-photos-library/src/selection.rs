// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic Photos Library selection engine (Rules v2).
//!
//! All state that must outlive the SQLite query cache (decisions, left-out
//! list, moments, learned routine places, owner corrections) is produced as
//! plain JSON structures with canonical SHA-256 digests so deleting and
//! rebuilding the SQLite cache produces identical decisions.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::geo::{
    GeoLookup, GeoResolver, classify_location, learn_routine_places, parse_utc_ts, to_local_time,
};
use crate::types::{
    ALBUM_CLOUD_SHARED, ALBUM_REGULAR, AssetDecision, CollapsePickStrategy, DecisionKind,
    LeftOutBreakdown, LeftOutRecord, MomentRecord, OwnerCorrections, SMART_CAPTURED_BY_ME,
    SMART_HIDDEN, SMART_RECENTLY_SAVED, SMART_RECENTS, SMART_SCREENSHOTS, ScanAssetRow,
    SelectionConfig, SelectionOutput, SelectionSummary, SmartAlbumCatalogEntry,
};

/// Raised loudly when Apple's smart-album subtypes drift or disappear.
/// Never silently drops photos on subtype drift.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartAlbumContractError(pub String);

impl std::fmt::Display for SmartAlbumContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SmartAlbumContractError {}

/// Check whether an asset belongs to a specific PhotoKit smart album subtype.
#[must_use]
pub fn has_smart_album(row: &ScanAssetRow, subtype: i64) -> bool {
    row.albums
        .iter()
        .any(|a| a.kind == "smart" && a.subtype == subtype)
}

/// Return sorted unique user album IDs (`subtype == 2` when `regular_only`).
#[must_use]
pub fn user_album_ids(row: &ScanAssetRow, regular_only: bool) -> Vec<String> {
    let mut set = BTreeSet::new();
    for a in &row.albums {
        if a.kind != "user" {
            continue;
        }
        if regular_only && a.subtype != ALBUM_REGULAR {
            continue;
        }
        set.insert(a.id.clone());
    }
    set.into_iter().collect()
}

/// Whether the asset belongs to an iCloud Shared Album (`subtype == 101`).
#[must_use]
pub fn is_cloud_shared(row: &ScanAssetRow) -> bool {
    row.albums
        .iter()
        .any(|a| a.kind == "user" && a.subtype == ALBUM_CLOUD_SHARED)
}

/// Return deterministic list of owner-mark signals present on this asset.
#[must_use]
pub fn extract_owner_marks(row: &ScanAssetRow, owner_override: bool) -> Vec<String> {
    let mut marks = Vec::new();
    if row.favorite {
        marks.push("favorite".to_owned());
    }
    if row.edited {
        marks.push("edited".to_owned());
    }
    let rating = row.rating.filter(|r| *r > 0.0).or_else(|| {
        row.extended
            .as_ref()
            .and_then(|e| e.rating)
            .filter(|r| *r > 0.0)
    });
    if rating.is_some() {
        marks.push("rating".to_owned());
    }
    if row
        .extended
        .as_ref()
        .and_then(|e| e.caption.as_deref())
        .is_some_and(|c| !c.trim().is_empty())
    {
        marks.push("caption".to_owned());
    }
    if row
        .extended
        .as_ref()
        .is_some_and(|e| e.keywords.iter().any(|k| !k.trim().is_empty()))
    {
        marks.push("keywords".to_owned());
    }
    if !user_album_ids(row, true).is_empty() {
        marks.push("user_album".to_owned());
    }
    if owner_override {
        marks.push("owner_rule".to_owned());
    }
    marks
}

/// Whether `marks` contains at least one strong owner mark beyond `edited`
/// alone (`favorite`, `user_album`, `rating`, `caption`, `keywords`, `owner_rule`).
fn has_strong_owner_mark(marks: &[String]) -> bool {
    marks.iter().any(|m| m != "edited")
}

fn mark_priority_score(row: &ScanAssetRow, marks: &[String]) -> i32 {
    let mut score = 0;
    if row.represents_burst || row.burst_selection.unwrap_or(0) != 0 {
        score += 90;
    }
    for m in marks {
        score += match m.as_str() {
            "favorite" => 100,
            "owner_rule" => 80,
            "edited" => 60,
            "rating" => 50,
            "caption" => 40,
            "keywords" => 30,
            "user_album" => 20,
            _ => 0,
        };
    }
    score
}

/// Fail loudly if Apple renumbers or drops `'Captured by Me'` (`1000000220`)
/// or `'Recently Saved'` (`1000000218`). Never silently drops photos.
pub fn validate_smart_album_contract(
    rows: &[ScanAssetRow],
    smart_catalog: Option<&[SmartAlbumCatalogEntry]>,
) -> Result<(), SmartAlbumContractError> {
    if let Some(catalog) = smart_catalog {
        let subtypes: BTreeSet<i64> = catalog.iter().map(|c| c.subtype).collect();
        if !subtypes.contains(&SMART_CAPTURED_BY_ME) {
            return Err(SmartAlbumContractError(format!(
                "Expected smart album subtype {SMART_CAPTURED_BY_ME} ('Captured by Me') missing from smart album catalog."
            )));
        }
        if !subtypes.contains(&SMART_RECENTLY_SAVED) {
            return Err(SmartAlbumContractError(format!(
                "Expected smart album subtype {SMART_RECENTLY_SAVED} ('Recently Saved') missing from smart album catalog."
            )));
        }
    }

    let mut seen_smart_subtypes: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    let mut cbm_count = 0usize;
    let mut unexplained_recents_stills = 0usize;

    for row in rows {
        let mut is_cbm = false;
        let mut is_saved = false;
        let mut is_recents = false;
        let mut is_ss_album = false;
        for a in &row.albums {
            if a.kind == "smart" {
                let title = a.title.as_deref().unwrap_or("").trim().to_ascii_lowercase();
                seen_smart_subtypes
                    .entry(a.subtype)
                    .or_default()
                    .insert(title);
                if a.subtype == SMART_CAPTURED_BY_ME {
                    is_cbm = true;
                } else if a.subtype == SMART_RECENTLY_SAVED {
                    is_saved = true;
                } else if a.subtype == SMART_RECENTS {
                    is_recents = true;
                } else if a.subtype == SMART_SCREENSHOTS {
                    is_ss_album = true;
                }
            }
        }
        if is_cbm {
            cbm_count += 1;
        } else if is_recents
            && !is_saved
            && !row.hidden
            && row.media_type == "photo"
            && !row.screenshot
            && !is_ss_album
            && !is_cloud_shared(row)
        {
            unexplained_recents_stills += 1;
        }
    }

    for (st, titles) in &seen_smart_subtypes {
        if *st != SMART_CAPTURED_BY_ME && titles.contains("captured by me") {
            return Err(SmartAlbumContractError(format!(
                "Smart album 'Captured by Me' appeared under unexpected subtype {st} (expected {SMART_CAPTURED_BY_ME})."
            )));
        }
        if *st != SMART_RECENTLY_SAVED && titles.contains("recently saved") {
            return Err(SmartAlbumContractError(format!(
                "Smart album 'Recently Saved' appeared under unexpected subtype {st} (expected {SMART_RECENTLY_SAVED})."
            )));
        }
    }

    if !rows.is_empty() && cbm_count == 0 && unexplained_recents_stills > 0 {
        return Err(SmartAlbumContractError(format!(
            "Smart album subtype {SMART_CAPTURED_BY_ME} matched 0 assets while Recents ({SMART_RECENTS}) \
             contains {unexplained_recents_stills} non-screenshot, non-saved still photos. \
             Refusing to silently drop photos."
        )));
    }

    Ok(())
}

/// Deterministic evenly-spaced indices from `0..n` preserving endpoints when `k >= 2`.
fn select_indices_temporal_spread(n: usize, k: usize) -> Vec<usize> {
    if k == 0 || n == 0 {
        return Vec::new();
    }
    if n <= k {
        return (0..n).collect();
    }
    if k == 1 {
        return vec![0];
    }
    let mut chosen = BTreeSet::new();
    for i in 0..k {
        let idx = ((i * (n - 1)) as f64 / ((k - 1) as f64)).round() as usize;
        chosen.insert(idx.min(n - 1));
    }
    chosen.into_iter().collect()
}

/// Map an internal left-out reason code to VPX's 6 owner-facing bucket names.
#[must_use]
pub fn left_out_bucket(reason: &str) -> Option<&'static str> {
    match reason {
        "burst_non_pick" | "close_shot_collapsed" => Some("near_duplicates"),
        "video" | "non_photo" => Some("videos"),
        "recently_saved" | "cloud_shared_album" | "not_captured_by_me" | "others_no_face_match" => {
            Some("saved_and_shared")
        }
        "moment_capped" => Some("busy_moments"),
        "screenshot" => Some("screenshots"),
        "read_as_noise" | "noise" => Some("noise"),
        // Hidden assets are never exposed in counts or left-out breakdowns.
        "hidden" | "owner_rule_drop" | "owner_rule_never_album" => None,
        _ => Some("saved_and_shared"),
    }
}

/// Aggregate left-out counts into VPX's bucket map (excluding zero buckets).
#[must_use]
pub fn aggregate_left_out_buckets<I, S>(reasons: I) -> LeftOutBreakdown
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut map: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for r in reasons {
        if let Some(bucket) = left_out_bucket(r.as_ref()) {
            *map.entry(bucket.to_owned()).or_insert(0) += 1;
            total += 1;
        }
    }
    LeftOutBreakdown {
        total,
        reasons: map,
    }
}

#[derive(Clone)]
struct RowContext {
    captured_ts: f64,
    timezone: Option<String>,
    tz_source: Option<String>,
    utc_offset_seconds: Option<i32>,
    local_iso: Option<String>,
    local_date: Option<String>,
    local_hm: Option<String>,
    coarse_place: Option<String>,
    place_cluster_id: Option<String>,
    away_from_routine: Option<bool>,
}

struct CloseGroup<'a> {
    start_ts: f64,
    end_ts: f64,
    members: Vec<&'a ScanAssetRow>,
    kept: Vec<&'a ScanAssetRow>,
    primary: &'a ScanAssetRow,
    owner_marked: bool,
}

fn canonical_sha256<T: serde::Serialize>(value: &T) -> String {
    let raw = serde_json::to_vec(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(raw))
}

/// Run deterministic Rules v2 selection over scanned metadata rows.
pub fn run_selection(
    rows: &[ScanAssetRow],
    config: &SelectionConfig,
    corrections: &OwnerCorrections,
    smart_catalog: Option<&[SmartAlbumCatalogEntry]>,
    resolver: &GeoResolver,
) -> Result<SelectionOutput, SmartAlbumContractError> {
    validate_smart_album_contract(rows, smart_catalog)?;

    // Canonical deterministic ordering by (capturedAt_ts, id)
    let mut ordered: Vec<&ScanAssetRow> = rows.iter().collect();
    ordered.sort_by(|a, b| {
        parse_utc_ts(a.captured_at.as_deref())
            .total_cmp(&parse_utc_ts(b.captured_at.as_deref()))
            .then_with(|| a.id.cmp(&b.id))
    });

    let always_albums: BTreeSet<&str> = corrections
        .always_albums
        .iter()
        .map(String::as_str)
        .collect();
    let never_albums: BTreeSet<&str> = corrections
        .never_albums
        .iter()
        .map(String::as_str)
        .collect();
    let keep_assets: BTreeSet<&str> = corrections.keep_assets.iter().map(String::as_str).collect();
    let drop_assets: BTreeSet<&str> = corrections.drop_assets.iter().map(String::as_str).collect();

    // Track bursts whose representative is in Captured by Me so non-rep frames attribute cleanly.
    let mut cbm_burst_rep: BTreeMap<&str, &str> = BTreeMap::new();
    for r in &ordered {
        if let Some(bid) = r.burst_identifier.as_deref()
            && has_smart_album(r, SMART_CAPTURED_BY_ME)
            && r.represents_burst
        {
            cbm_burst_rep.insert(bid, r.id.as_str());
        }
    }

    // Pass 1: Rule 1 gate + Rule 2 owner marks.
    let mut eligible: Vec<&ScanAssetRow> = Vec::new();
    let mut rule1_out: BTreeMap<&str, (&'static str, String, Option<String>)> = BTreeMap::new();
    let mut owner_override_ids: BTreeSet<&str> = BTreeSet::new();
    let mut admitted_via_mark_source: BTreeMap<&str, &'static str> = BTreeMap::new();
    let mut marks_by_id: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    // Unmarked non-hidden non-screenshot stills that failed Rule 1 (candidates for ±2h others rung).
    let mut others_candidates: Vec<(&ScanAssetRow, &'static str)> = Vec::new();

    for r in &ordered {
        let aid = r.id.as_str();
        let in_cbm = has_smart_album(r, SMART_CAPTURED_BY_ME);
        let in_saved = has_smart_album(r, SMART_RECENTLY_SAVED);
        let is_ss = r.screenshot || has_smart_album(r, SMART_SCREENSHOTS);
        let is_hidden = r.hidden || has_smart_album(r, SMART_HIDDEN);
        let all_user_albums = user_album_ids(r, false);

        if is_hidden {
            rule1_out.insert(aid, ("hidden", "Asset is marked hidden".to_owned(), None));
            continue;
        }
        if r.media_type != "photo" {
            let reason = if r.media_type == "video" {
                "video"
            } else {
                "non_photo"
            };
            rule1_out.insert(
                aid,
                (reason, format!("Media type is {}", r.media_type), None),
            );
            continue;
        }
        if is_ss {
            rule1_out.insert(
                aid,
                (
                    "screenshot",
                    "Screenshots belong to the screenshots source".to_owned(),
                    None,
                ),
            );
            continue;
        }
        if drop_assets.contains(aid) {
            rule1_out.insert(
                aid,
                (
                    "owner_rule_drop",
                    "Excluded by owner asset rule".to_owned(),
                    None,
                ),
            );
            continue;
        }
        if all_user_albums
            .iter()
            .any(|u| never_albums.contains(u.as_str()))
        {
            rule1_out.insert(
                aid,
                (
                    "owner_rule_never_album",
                    "Excluded by owner never-album rule".to_owned(),
                    None,
                ),
            );
            continue;
        }

        let is_override = keep_assets.contains(aid)
            || all_user_albums
                .iter()
                .any(|u| always_albums.contains(u.as_str()));
        if is_override {
            owner_override_ids.insert(aid);
            marks_by_id.insert(aid, extract_owner_marks(r, true));
            eligible.push(r);
            continue;
        }

        let marks = extract_owner_marks(r, false);
        if config.owner_marks_beat_rule1 && !marks.is_empty() && (in_saved || !in_cbm) {
            let source = if in_saved {
                "recently_saved"
            } else if is_cloud_shared(r) {
                "cloud_shared_album"
            } else {
                "not_captured_by_me"
            };
            admitted_via_mark_source.insert(aid, source);
            marks_by_id.insert(aid, marks);
            eligible.push(r);
            continue;
        }

        if in_saved {
            rule1_out.insert(
                aid,
                (
                    "recently_saved",
                    "In 'Recently Saved' smart album (1000000218)".to_owned(),
                    None,
                ),
            );
            others_candidates.push((r, "recently_saved"));
            continue;
        }
        if !in_cbm {
            if let Some(bid) = r.burst_identifier.as_deref()
                && let Some(rep_id) = cbm_burst_rep.get(bid)
                && !r.represents_burst
            {
                rule1_out.insert(
                    aid,
                    (
                        "burst_non_pick",
                        "Non-representative frame of an own burst".to_owned(),
                        Some((*rep_id).to_owned()),
                    ),
                );
            } else if is_cloud_shared(r) {
                rule1_out.insert(
                    aid,
                    (
                        "cloud_shared_album",
                        "In iCloud Shared Album (subtype 101), not Captured by Me".to_owned(),
                        None,
                    ),
                );
                others_candidates.push((r, "cloud_shared_album"));
            } else {
                rule1_out.insert(
                    aid,
                    (
                        "not_captured_by_me",
                        "Not in 'Captured by Me' smart album (1000000220)".to_owned(),
                        None,
                    ),
                );
                others_candidates.push((r, "not_captured_by_me"));
            }
            continue;
        }

        marks_by_id.insert(aid, marks);
        eligible.push(r);
    }

    // Rule 5 context: learn routine places from located own (Captured by Me) eligible stills.
    let located_cbm: Vec<&ScanAssetRow> = eligible
        .iter()
        .copied()
        .filter(|r| r.location.is_some() && has_smart_album(r, SMART_CAPTURED_BY_ME))
        .collect();
    let located_eligible: Vec<&ScanAssetRow> = if located_cbm.is_empty() {
        eligible
            .iter()
            .copied()
            .filter(|r| r.location.is_some())
            .collect()
    } else {
        located_cbm
    };
    let routine_model = learn_routine_places(
        &located_eligible,
        config.routine_cluster_radius_km,
        config.routine_window_days,
        config.routine_min_days,
        config.routine_min_months,
        config.routine_min_span_days,
        resolver,
    );
    let home_tz = routine_model
        .home_timezone
        .clone()
        .unwrap_or_else(|| "UTC".to_owned());

    // Resolve location -> (coarse_place, tz, routine_info) for all located rows.
    let mut loc_geo_cache: BTreeMap<(i64, i64), GeoLookup> = BTreeMap::new();
    let mut row_context: BTreeMap<&str, RowContext> = BTreeMap::new();
    let mut located_timeline: Vec<(f64, String)> = Vec::new();

    for r in &ordered {
        let aid = r.id.as_str();
        let ts = parse_utc_ts(r.captured_at.as_deref());
        if let Some(loc) = &r.location {
            let ckey = (
                (loc.latitude * 10_000.0).round() as i64,
                (loc.longitude * 10_000.0).round() as i64,
            );
            let geo = loc_geo_cache
                .entry(ckey)
                .or_insert_with(|| resolver.lookup(loc.latitude, loc.longitude))
                .clone();
            let rcls = classify_location(
                loc.latitude,
                loc.longitude,
                &routine_model,
                &corrections.place_routine_overrides,
            );
            let tz = if geo.timezone.is_empty() {
                home_tz.clone()
            } else {
                geo.timezone
            };
            located_timeline.push((ts, tz.clone()));
            row_context.insert(
                aid,
                RowContext {
                    captured_ts: ts,
                    timezone: Some(tz),
                    tz_source: Some("location".to_owned()),
                    utc_offset_seconds: None,
                    local_iso: None,
                    local_date: None,
                    local_hm: None,
                    coarse_place: geo.coarse_place,
                    place_cluster_id: rcls.cluster_id,
                    away_from_routine: Some(rcls.away_from_routine),
                },
            );
        } else {
            row_context.insert(
                aid,
                RowContext {
                    captured_ts: ts,
                    timezone: None,
                    tz_source: None,
                    utc_offset_seconds: None,
                    local_iso: None,
                    local_date: None,
                    local_hm: None,
                    coarse_place: None,
                    place_cluster_id: None,
                    away_from_routine: None,
                },
            );
        }
    }

    // Fill missing timezones via binary search on located_timeline within nearest_tz_max_hours.
    let max_tz_gap = config.nearest_tz_max_hours * 3600.0;
    for r in &ordered {
        let aid = r.id.as_str();
        let Some(ctx) = row_context.get_mut(aid) else {
            continue;
        };
        if ctx.timezone.is_none() {
            let ts = ctx.captured_ts;
            if !located_timeline.is_empty() {
                let pos = located_timeline.partition_point(|(t, _)| *t < ts);
                let mut best_gap = f64::INFINITY;
                let mut best_tz: Option<&str> = None;
                for cand in [pos.checked_sub(1), Some(pos)].into_iter().flatten() {
                    if let Some((t, tz)) = located_timeline.get(cand) {
                        let g = (t - ts).abs();
                        if g < best_gap {
                            best_gap = g;
                            best_tz = Some(tz.as_str());
                        }
                    }
                }
                if best_gap <= max_tz_gap
                    && let Some(tz) = best_tz
                {
                    ctx.timezone = Some(tz.to_owned());
                    ctx.tz_source = Some("nearest_photo".to_owned());
                }
            }
            if ctx.timezone.is_none() {
                ctx.timezone = Some(home_tz.clone());
                ctx.tz_source = Some(if routine_model.home_timezone.is_some() {
                    "home_fallback".to_owned()
                } else {
                    "utc_fallback".to_owned()
                });
            }
        }
        if let Some(cap) = r.captured_at.as_deref()
            && let Some(tz) = ctx.timezone.as_deref()
            && let Some(lt) = to_local_time(cap, tz)
        {
            ctx.local_iso = Some(lt.local_iso);
            ctx.local_date = Some(lt.local_date);
            ctx.local_hm = Some(lt.local_hm);
            ctx.utc_offset_seconds = Some(lt.utc_offset_seconds);
        }
    }

    // Rule 3: Close-shot collapse over eligible still photos.
    let mut close_groups_raw: Vec<Vec<&ScanAssetRow>> = Vec::new();
    for r in &eligible {
        let ts = row_context[r.id.as_str()].captured_ts;
        if let Some(prev_group) = close_groups_raw.last_mut()
            && let (Some(first), Some(prev)) = (prev_group.first(), prev_group.last())
        {
            let first_ts = row_context[first.id.as_str()].captured_ts;
            let prev_ts = row_context[prev.id.as_str()].captured_ts;
            let same_burst =
                r.burst_identifier.is_some() && r.burst_identifier == prev.burst_identifier;
            let within_gap = (ts - prev_ts) <= config.collapse_gap_seconds;
            let within_span = config
                .collapse_max_span_seconds
                .is_none_or(|span| (ts - first_ts) <= span);
            if same_burst || (within_gap && within_span) {
                prev_group.push(r);
                continue;
            }
        }
        close_groups_raw.push(vec![r]);
    }

    let mut group_records: Vec<CloseGroup<'_>> = Vec::with_capacity(close_groups_raw.len());
    let mut collapse_dropped: BTreeMap<&str, (&'static str, String, String)> = BTreeMap::new();

    for grp in close_groups_raw {
        // Under v2, strong marks (favorite, user_album, rating, caption, keywords, owner_rule)
        // never collapse; `edited`-only collapses inside `<= collapse_gap_seconds` when enabled.
        let uncollapsible_marked: Vec<&ScanAssetRow> = grp
            .iter()
            .copied()
            .filter(|r| {
                marks_by_id.get(r.id.as_str()).is_some_and(|m| {
                    if config.edited_only_collapses_in_close_group {
                        has_strong_owner_mark(m)
                    } else {
                        !m.is_empty()
                    }
                })
            })
            .collect();

        let (kept_in_grp, primary, owner_marked_group): (Vec<&ScanAssetRow>, &ScanAssetRow, bool) =
            if !uncollapsible_marked.is_empty() {
                let mut sorted = uncollapsible_marked;
                sorted.sort_by(|a, b| {
                    let sa = mark_priority_score(a, &marks_by_id[a.id.as_str()]);
                    let sb = mark_priority_score(b, &marks_by_id[b.id.as_str()]);
                    sb.cmp(&sa)
                        .then_with(|| {
                            row_context[a.id.as_str()]
                                .captured_ts
                                .total_cmp(&row_context[b.id.as_str()].captured_ts)
                        })
                        .then_with(|| a.id.cmp(&b.id))
                });
                let primary = sorted[0];
                (sorted, primary, true)
            } else {
                // Prefer any edited member if edited_only_collapses_in_close_group is active,
                // else burst pick, else deterministic strategy pick (Middle in v2).
                let burst_pick = grp
                    .iter()
                    .copied()
                    .find(|r| r.represents_burst || r.burst_selection.unwrap_or(0) != 0);
                let edited_members: Vec<&ScanAssetRow> = grp
                    .iter()
                    .copied()
                    .filter(|r| {
                        marks_by_id
                            .get(r.id.as_str())
                            .is_some_and(|m| !m.is_empty())
                    })
                    .collect();
                let primary = if let Some(bp) = burst_pick {
                    bp
                } else if !edited_members.is_empty() {
                    edited_members[edited_members.len() / 2]
                } else {
                    match config.collapse_pick_strategy {
                        CollapsePickStrategy::First => grp[0],
                        CollapsePickStrategy::Middle => grp[grp.len() / 2],
                        CollapsePickStrategy::Last => grp[grp.len() - 1],
                    }
                };
                let any_marked = !edited_members.is_empty();
                (vec![primary], primary, any_marked)
            };

        let kept_set: BTreeSet<&str> = kept_in_grp.iter().map(|r| r.id.as_str()).collect();
        let is_burst_grp = grp.len() > 1
            && grp[0].burst_identifier.is_some()
            && grp
                .iter()
                .all(|r| r.burst_identifier == grp[0].burst_identifier);
        let primary_ts = row_context[primary.id.as_str()].captured_ts;

        for r in &grp {
            if !kept_set.contains(r.id.as_str()) {
                let reason = if is_burst_grp {
                    "burst_non_pick"
                } else {
                    "close_shot_collapsed"
                };
                let gap = ((row_context[r.id.as_str()].captured_ts - primary_ts).abs() * 10.0)
                    .round()
                    / 10.0;
                let detail = if is_burst_grp {
                    "Non-selected burst frame".to_owned()
                } else {
                    format!(
                        "Collapsed into close shot ({gap:.1}s from kept shot; group of {})",
                        grp.len()
                    )
                };
                collapse_dropped.insert(r.id.as_str(), (reason, detail, primary.id.clone()));
            }
        }

        let start_ts = row_context[grp[0].id.as_str()].captured_ts;
        let end_ts = row_context[grp[grp.len() - 1].id.as_str()].captured_ts;
        group_records.push(CloseGroup {
            start_ts,
            end_ts,
            members: grp,
            kept: kept_in_grp,
            primary,
            owner_marked: owner_marked_group,
        });
    }

    // Rule 4: Group close-groups into 5-minute moments.
    let mut raw_moments: Vec<Vec<usize>> = Vec::new();
    for (idx, gr) in group_records.iter().enumerate() {
        if let Some(last_m) = raw_moments.last_mut()
            && let Some(&last_idx) = last_m.last()
            && (gr.start_ts - group_records[last_idx].end_ts) <= config.moment_gap_seconds
        {
            last_m.push(idx);
            continue;
        }
        raw_moments.push(vec![idx]);
    }

    let mut moments_out: Vec<MomentRecord> = Vec::with_capacity(raw_moments.len());
    let mut moment_of_asset: BTreeMap<&str, String> = BTreeMap::new();
    let mut kept_assets_final: BTreeMap<&str, String> = BTreeMap::new();
    let mut moment_dropped: BTreeMap<&str, (&'static str, String, String)> = BTreeMap::new();
    // Track own (Captured by Me) moment windows for the ±2 h others'-photos rung.
    let mut own_moment_windows: Vec<(f64, f64, String)> = Vec::new();

    for (m_idx, m_group_indices) in raw_moments.iter().enumerate() {
        let first_primary = group_records[m_group_indices[0]].primary;
        let first_ctx = &row_context[first_primary.id.as_str()];
        let day_tag = first_ctx
            .local_date
            .as_deref()
            .unwrap_or("undated")
            .replace('-', "");
        let moment_id = format!("m-{day_tag}-{:05}", m_idx + 1);

        let mut all_members: Vec<&ScanAssetRow> = Vec::new();
        let mut has_own_photo = false;
        for &gi in m_group_indices {
            for r in &group_records[gi].members {
                moment_of_asset.insert(r.id.as_str(), moment_id.clone());
                if has_smart_album(r, SMART_CAPTURED_BY_ME) {
                    has_own_photo = true;
                }
                all_members.push(r);
            }
        }

        let moment_away = all_members
            .iter()
            .filter_map(|r| row_context[r.id.as_str()].away_from_routine)
            .any(|v| v);
        let mut place_counts: BTreeMap<&str, usize> = BTreeMap::new();
        for r in &all_members {
            if let Some(p) = row_context[r.id.as_str()].coarse_place.as_deref() {
                *place_counts.entry(p).or_insert(0) += 1;
            }
        }
        let moment_place = place_counts
            .into_iter()
            .max_by(|(pa, ca), (pb, cb)| ca.cmp(cb).then_with(|| pb.cmp(pa)))
            .map(|(p, _)| p.to_owned());

        let cap = if moment_away {
            config.moment_cap_away
        } else {
            config.moment_cap_routine
        };
        let marked_local_indices: BTreeSet<usize> = m_group_indices
            .iter()
            .enumerate()
            .filter(|&(_, &gi)| group_records[gi].owner_marked)
            .map(|(local_i, _)| local_i)
            .collect();

        let surviving_local: Vec<usize> = match cap {
            None => (0..m_group_indices.len()).collect(),
            Some(c) if m_group_indices.len() <= c => (0..m_group_indices.len()).collect(),
            Some(c) => {
                let mut s = marked_local_indices.clone();
                for idx in select_indices_temporal_spread(m_group_indices.len(), c) {
                    if s.len() >= c {
                        break;
                    }
                    s.insert(idx);
                }
                s.into_iter().collect()
            }
        };
        let surviving_set: BTreeSet<usize> = surviving_local.iter().copied().collect();

        let mut kept_ids_in_moment: Vec<String> = Vec::new();
        for &local_i in &surviving_local {
            let gr = &group_records[m_group_indices[local_i]];
            for r in &gr.kept {
                kept_ids_in_moment.push(r.id.clone());
                let keep_reason = if let Some(marks) = marks_by_id.get(r.id.as_str())
                    && !marks.is_empty()
                {
                    format!("owner_marked:{}", marks.join(","))
                } else if m_group_indices.len() == 1 && gr.members.len() == 1 {
                    "own_photo_single".to_owned()
                } else if gr.members.len() > 1 {
                    "close_group_pick".to_owned()
                } else {
                    "moment_pick".to_owned()
                };
                kept_assets_final.insert(r.id.as_str(), keep_reason);
            }
        }

        let kept_ts_pairs: Vec<(f64, &str)> = kept_ids_in_moment
            .iter()
            .map(|kid| (row_context[kid.as_str()].captured_ts, kid.as_str()))
            .collect();
        let mut dropped_ids_in_moment: Vec<String> = Vec::new();
        for (local_i, &gi) in m_group_indices.iter().enumerate() {
            let gr = &group_records[gi];
            if !surviving_set.contains(&local_i) {
                for r in &gr.kept {
                    let rts = row_context[r.id.as_str()].captured_ts;
                    let nearest = kept_ts_pairs
                        .iter()
                        .min_by(|a, b| {
                            (a.0 - rts)
                                .abs()
                                .total_cmp(&(b.0 - rts).abs())
                                .then_with(|| a.1.cmp(b.1))
                        })
                        .map(|p| p.1.to_owned())
                        .unwrap_or_default();
                    let cap_val = cap.unwrap_or(0);
                    moment_dropped.insert(
                        r.id.as_str(),
                        (
                            "moment_capped",
                            format!(
                                "Exceeded moment cap ({cap_val}) in {}-group moment ({})",
                                m_group_indices.len(),
                                if moment_away { "away" } else { "routine" }
                            ),
                            nearest,
                        ),
                    );
                    dropped_ids_in_moment.push(r.id.clone());
                }
            }
            for r in &gr.members {
                if collapse_dropped.contains_key(r.id.as_str()) {
                    dropped_ids_in_moment.push(r.id.clone());
                }
            }
        }

        let start_ts = group_records[m_group_indices[0]].start_ts;
        let last_gr = &group_records[m_group_indices[m_group_indices.len() - 1]];
        let end_ts = last_gr.end_ts;
        if has_own_photo {
            own_moment_windows.push((start_ts, end_ts, moment_id.clone()));
        }

        moments_out.push(MomentRecord {
            moment_id,
            local_date: first_ctx.local_date.clone(),
            timezone: first_ctx.timezone.clone(),
            start_local_iso: first_ctx.local_iso.clone(),
            end_local_iso: row_context[last_gr.primary.id.as_str()].local_iso.clone(),
            start_ts,
            end_ts,
            duration_seconds: ((end_ts - start_ts) * 10.0).round() / 10.0,
            coarse_place: moment_place,
            away_from_routine: moment_away,
            has_owner_mark: !marked_local_indices.is_empty(),
            raw_count: all_members.len(),
            close_group_count: m_group_indices.len(),
            cap_applied: cap,
            kept_ids: kept_ids_in_moment,
            dropped_ids: dropped_ids_in_moment,
        });
    }

    // Others'-photos rung: unmarked non-hidden/non-screenshot stills within ±2 h
    // of any own moment are staged for side-effect-free face matching before keeping.
    let mut staged_others: BTreeMap<&str, (&'static str, String)> = BTreeMap::new();
    if config.others_window_seconds > 0.0 && !own_moment_windows.is_empty() {
        for (r, source_reason) in &others_candidates {
            let ts = row_context[r.id.as_str()].captured_ts;
            if ts <= 0.0 {
                continue;
            }
            // Binary search own_moment_windows sorted by start_ts.
            let pos = own_moment_windows
                .partition_point(|(s, _, _)| *s <= ts + config.others_window_seconds);
            let mut nearest_moment: Option<(f64, &str)> = None;
            for (s, e, mid) in own_moment_windows[..pos].iter().rev() {
                if *e + config.others_window_seconds < ts {
                    // Since windows are ordered by start_ts, check a small suffix before breaking.
                    if ts - *s > config.others_window_seconds + 86_400.0 {
                        break;
                    }
                    continue;
                }
                let gap = if ts < *s {
                    *s - ts
                } else if ts > *e {
                    ts - *e
                } else {
                    0.0
                };
                if gap <= config.others_window_seconds
                    && nearest_moment.is_none_or(|(bg, _)| gap < bg)
                {
                    nearest_moment = Some((gap, mid.as_str()));
                }
            }
            if let Some((_, mid)) = nearest_moment {
                staged_others.insert(r.id.as_str(), (*source_reason, mid.to_owned()));
            }
        }
    }

    // Assemble final per-asset decisions & durable left-out list (ZERO image bytes).
    let mut decisions: Vec<AssetDecision> = Vec::with_capacity(ordered.len());
    let mut left_out: Vec<LeftOutRecord> = Vec::new();
    let mut keep_by_reason: BTreeMap<String, usize> = BTreeMap::new();
    let mut left_out_by_reason: BTreeMap<String, usize> = BTreeMap::new();
    let mut kept_count = 0usize;
    let mut staged_count = 0usize;

    for r in &ordered {
        let aid = r.id.as_str();
        let ctx = &row_context[aid];
        let in_cbm = has_smart_album(r, SMART_CAPTURED_BY_ME);
        let in_saved = has_smart_album(r, SMART_RECENTLY_SAVED);
        let is_ss = r.screenshot || has_smart_album(r, SMART_SCREENSHOTS);
        let marks = marks_by_id
            .get(aid)
            .cloned()
            .unwrap_or_else(|| extract_owner_marks(r, owner_override_ids.contains(aid)));

        let (decision, reason, detail, collapsed_into, near_moment_id) =
            if let Some(keep_reason) = kept_assets_final.get(aid) {
                kept_count += 1;
                let base = keep_reason
                    .split(':')
                    .next()
                    .unwrap_or(keep_reason)
                    .to_owned();
                *keep_by_reason.entry(base).or_insert(0) += 1;
                (
                    DecisionKind::Keep,
                    keep_reason.clone(),
                    keep_reason.clone(),
                    None,
                    None,
                )
            } else if let Some((source_reason, near_mid)) = staged_others.get(aid) {
                staged_count += 1;
                (
                    DecisionKind::StageOthersFaceCheck,
                    (*source_reason).to_owned(),
                    format!("Within ±2 h of own moment {near_mid}; awaits face match"),
                    None,
                    Some(near_mid.clone()),
                )
            } else if let Some((r_code, det, col)) = rule1_out.get(aid) {
                *left_out_by_reason.entry((*r_code).to_owned()).or_insert(0) += 1;
                (
                    DecisionKind::LeftOut,
                    (*r_code).to_owned(),
                    det.clone(),
                    col.clone(),
                    None,
                )
            } else if let Some((r_code, det, col)) = collapse_dropped.get(aid) {
                *left_out_by_reason.entry((*r_code).to_owned()).or_insert(0) += 1;
                (
                    DecisionKind::LeftOut,
                    (*r_code).to_owned(),
                    det.clone(),
                    Some(col.clone()),
                    None,
                )
            } else if let Some((r_code, det, col)) = moment_dropped.get(aid) {
                *left_out_by_reason.entry((*r_code).to_owned()).or_insert(0) += 1;
                (
                    DecisionKind::LeftOut,
                    (*r_code).to_owned(),
                    det.clone(),
                    Some(col.clone()),
                    None,
                )
            } else {
                *left_out_by_reason
                    .entry("not_captured_by_me".to_owned())
                    .or_insert(0) += 1;
                (
                    DecisionKind::LeftOut,
                    "not_captured_by_me".to_owned(),
                    "Asset excluded by selection rules".to_owned(),
                    None,
                    None,
                )
            };

        let mid = moment_of_asset.get(aid).cloned();
        if decision == DecisionKind::LeftOut && reason != "hidden" {
            left_out.push(LeftOutRecord {
                id: r.id.clone(),
                reason: reason.clone(),
                detail: detail.clone(),
                moment_id: mid.clone(),
                collapsed_into: collapsed_into.clone(),
                media_type: r.media_type.clone(),
                local_date: ctx.local_date.clone(),
                local_iso: ctx.local_iso.clone(),
                coarse_place: ctx.coarse_place.clone(),
            });
        }

        decisions.push(AssetDecision {
            id: r.id.clone(),
            cloud_id: r.cloud_id.clone(),
            media_type: r.media_type.clone(),
            captured_at: r.captured_at.clone(),
            captured_ts: ctx.captured_ts,
            local_date: ctx.local_date.clone(),
            local_iso: ctx.local_iso.clone(),
            local_hm: ctx.local_hm.clone(),
            timezone: ctx.timezone.clone(),
            tz_source: ctx.tz_source.clone(),
            utc_offset_seconds: ctx.utc_offset_seconds,
            coarse_place: ctx.coarse_place.clone(),
            place_cluster_id: ctx.place_cluster_id.clone(),
            away_from_routine: ctx.away_from_routine,
            in_cbm,
            in_saved,
            in_cloud_shared: is_cloud_shared(r),
            admitted_via_owner_mark: admitted_via_mark_source.contains_key(aid),
            admitted_via_mark_source: admitted_via_mark_source.get(aid).map(|s| (*s).to_owned()),
            screenshot: is_ss,
            owner_marked: !marks.is_empty(),
            owner_marks: marks,
            decision,
            reason,
            detail,
            moment_id: mid,
            collapsed_into,
            near_moment_id,
        });
    }

    let mut mark_promoted_by_source: BTreeMap<String, usize> = BTreeMap::new();
    for src in admitted_via_mark_source.values() {
        *mark_promoted_by_source
            .entry((*src).to_owned())
            .or_insert(0) += 1;
    }

    let summary = SelectionSummary {
        config: config.clone(),
        total_assets: decisions.len(),
        eligible_own_stills: eligible.len(),
        mark_promoted_total: admitted_via_mark_source.len(),
        mark_promoted_by_source,
        close_groups: group_records.len(),
        moments: moments_out.len(),
        kept: kept_count,
        staged_others_face_check: staged_count,
        left_out: left_out.len(),
        keep_by_reason,
        left_out_by_reason,
        routine_places_learned: routine_model.routine_count,
        home_timezone: routine_model.home_timezone.clone(),
        decisions_sha256: canonical_sha256(&decisions),
        left_out_sha256: canonical_sha256(&left_out),
        moments_sha256: canonical_sha256(&moments_out),
    };

    Ok(SelectionOutput {
        summary,
        decisions,
        left_out,
        moments: moments_out,
        routine_places: routine_model,
    })
}
