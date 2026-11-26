// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Offline coarse place naming, IANA timezone derivation, and routine-place
//! learning for the Photos Library holding area.
//!
//! Uses `solstone-core-location`'s bundled area tier (`area-tier.bin`) and an
//! embedded public-domain `zone1970.tab` alongside `jiff`'s bundled IANA tzdb.
//! Zero network lookups; deterministic across OSes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use solstone_core_location::AreaTier;

use crate::types::{RoutineCluster, RoutinePlacesModel, ScanAssetRow};

const ZONE1970_TAB: &str = include_str!("../data/zone1970.tab");
const EARTH_RADIUS_M: f64 = 6_371_008.8;
const REGION_NAME_COUNTRIES: &[&str] = &["US", "CA", "AU", "GB"];

/// Single-timezone US states / Canadian provinces / Australian states so points
/// near a zone border never pick a closer principal city in the wrong zone or
/// DST regime. Split states fall through to country-filtered nearest principal
/// city in `zone1970.tab`.
fn admin1_tz(cc: &str, admin1: &str) -> Option<&'static str> {
    match (cc, admin1) {
        // US single-zone states
        ("US", "Alabama") => Some("America/Chicago"),
        ("US", "Arizona") => Some("America/Phoenix"),
        ("US", "Arkansas") => Some("America/Chicago"),
        ("US", "California") => Some("America/Los_Angeles"),
        ("US", "Colorado") => Some("America/Denver"),
        ("US", "Connecticut") => Some("America/New_York"),
        ("US", "Delaware") => Some("America/New_York"),
        ("US", "District of Columbia") => Some("America/New_York"),
        ("US", "Georgia") => Some("America/New_York"),
        ("US", "Hawaii") => Some("Pacific/Honolulu"),
        ("US", "Illinois") => Some("America/Chicago"),
        ("US", "Iowa") => Some("America/Chicago"),
        ("US", "Louisiana") => Some("America/Chicago"),
        ("US", "Maine") => Some("America/New_York"),
        ("US", "Maryland") => Some("America/New_York"),
        ("US", "Massachusetts") => Some("America/New_York"),
        ("US", "Minnesota") => Some("America/Chicago"),
        ("US", "Mississippi") => Some("America/Chicago"),
        ("US", "Missouri") => Some("America/Chicago"),
        ("US", "Montana") => Some("America/Denver"),
        ("US", "Nevada") => Some("America/Los_Angeles"),
        ("US", "New Hampshire") => Some("America/New_York"),
        ("US", "New Jersey") => Some("America/New_York"),
        ("US", "New Mexico") => Some("America/Denver"),
        ("US", "New York") => Some("America/New_York"),
        ("US", "North Carolina") => Some("America/New_York"),
        ("US", "Ohio") => Some("America/New_York"),
        ("US", "Oklahoma") => Some("America/Chicago"),
        ("US", "Pennsylvania") => Some("America/New_York"),
        ("US", "Rhode Island") => Some("America/New_York"),
        ("US", "South Carolina") => Some("America/New_York"),
        ("US", "Utah") => Some("America/Denver"),
        ("US", "Vermont") => Some("America/New_York"),
        ("US", "Virginia") => Some("America/New_York"),
        ("US", "Washington") => Some("America/Los_Angeles"),
        ("US", "West Virginia") => Some("America/New_York"),
        ("US", "Wisconsin") => Some("America/Chicago"),
        ("US", "Wyoming") => Some("America/Denver"),
        // Canada mostly single-zone provinces
        ("CA", "Alberta") => Some("America/Edmonton"),
        ("CA", "Manitoba") => Some("America/Winnipeg"),
        ("CA", "New Brunswick") => Some("America/Moncton"),
        ("CA", "Nova Scotia") => Some("America/Halifax"),
        ("CA", "Prince Edward Island") => Some("America/Halifax"),
        ("CA", "Saskatchewan") => Some("America/Regina"),
        // Australia single-zone states
        ("AU", "New South Wales") => Some("Australia/Sydney"),
        ("AU", "Victoria") => Some("Australia/Melbourne"),
        ("AU", "Queensland") => Some("Australia/Brisbane"),
        ("AU", "South Australia") => Some("Australia/Adelaide"),
        ("AU", "Western Australia") => Some("Australia/Perth"),
        ("AU", "Tasmania") => Some("Australia/Hobart"),
        ("AU", "Northern Territory") => Some("Australia/Darwin"),
        ("AU", "Australian Capital Territory") => Some("Australia/Sydney"),
        _ => None,
    }
}

/// Great-circle distance in metres.
#[must_use]
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let p1 = lat1.to_radians();
    let p2 = lat2.to_radians();
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().min(1.0).asin()
}

/// Great-circle distance in kilometres.
#[must_use]
pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    haversine_m(lat1, lon1, lat2, lon2) / 1000.0
}

#[derive(Clone, Debug)]
struct ZoneTabRow {
    countries: Vec<String>,
    lat: f64,
    lon: f64,
    tz: String,
}

fn parse_iso6709(coord: &str) -> Option<(f64, f64)> {
    let bytes = coord.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let split = coord[1..].find(['+', '-']).map(|idx| idx + 1)?;
    let lat_part = &coord[..split];
    let lon_part = &coord[split..];
    let lat = parse_dms(lat_part, 2)?;
    let lon = parse_dms(lon_part, 3)?;
    Some((lat, lon))
}

fn parse_dms(part: &str, deg_digits: usize) -> Option<f64> {
    let sign = match part.as_bytes().first()? {
        b'+' => 1.0,
        b'-' => -1.0,
        _ => return None,
    };
    let rest = &part[1..];
    if rest.len() != deg_digits + 2 && rest.len() != deg_digits + 4 {
        return None;
    }
    let deg: f64 = rest[..deg_digits].parse().ok()?;
    let min: f64 = rest[deg_digits..deg_digits + 2].parse().ok()?;
    let sec: f64 = if rest.len() == deg_digits + 4 {
        rest[deg_digits + 2..].parse().ok()?
    } else {
        0.0
    };
    Some(sign * (deg + min / 60.0 + sec / 3600.0))
}

fn parse_zone_tab(text: &str) -> Vec<ZoneTabRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let Some((lat, lon)) = parse_iso6709(cols[1]) else {
            continue;
        };
        let countries = cols[0].split(',').map(str::to_owned).collect();
        rows.push(ZoneTabRow {
            countries,
            lat,
            lon,
            tz: cols[2].to_owned(),
        });
    }
    rows
}

/// Offline coarse place + IANA timezone lookup result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeoLookup {
    pub coarse_place: Option<String>,
    pub country_code: Option<String>,
    pub region: Option<String>,
    pub timezone: String,
}

/// Offline coarse place name and IANA timezone resolver.
pub struct GeoResolver {
    area_tier: Option<&'static AreaTier>,
    zones: Vec<ZoneTabRow>,
}

static SHARED_RESOLVER: OnceLock<GeoResolver> = OnceLock::new();

impl GeoResolver {
    /// Return the process-wide cached resolver backed by the bundled area tier
    /// (when present) and embedded `zone1970.tab`.
    #[must_use]
    pub fn shared() -> &'static Self {
        SHARED_RESOLVER.get_or_init(|| Self {
            area_tier: solstone_core_location::area_tier().ok(),
            zones: parse_zone_tab(ZONE1970_TAB),
        })
    }

    /// Resolve `(lat, lon)` to a coarse non-street place name and IANA timezone.
    #[must_use]
    pub fn lookup(&self, lat: f64, lon: f64) -> GeoLookup {
        let mut cc: Option<String> = None;
        let mut region: Option<String> = None;
        let mut coarse_place: Option<String> = None;

        if let Some(tier) = self.area_tier {
            let area = tier.name_point(lat, lon);
            if let Some(town) = &area.town {
                cc = Some(town.country_code.clone());
                region = town.region.clone();
                coarse_place = if REGION_NAME_COUNTRIES.contains(&town.country_code.as_str())
                    && let Some(r) = town.region.as_deref()
                {
                    Some(format!("near {}, {r}", town.name))
                } else if !town.country.is_empty() {
                    Some(format!("near {}, {}", town.name, town.country))
                } else {
                    Some(format!("near {}", town.name))
                };
            } else {
                region = area.region.clone();
                coarse_place = match (&area.region, &area.country) {
                    (Some(r), Some(c)) => Some(format!("{r}, {c}")),
                    (_, Some(c)) => Some(c.clone()),
                    (Some(r), None) => Some(r.clone()),
                    (None, None) => None,
                };
            }
        }

        let timezone = self.lookup_tz(lat, lon, cc.as_deref(), region.as_deref());
        GeoLookup {
            coarse_place,
            country_code: cc,
            region,
            timezone,
        }
    }

    fn lookup_tz(&self, lat: f64, lon: f64, cc: Option<&str>, region: Option<&str>) -> String {
        if let (Some(cc), Some(r)) = (cc, region)
            && let Some(tz) = admin1_tz(cc, r)
        {
            return tz.to_owned();
        }
        let country_matches: Vec<&ZoneTabRow> = cc
            .map(|code| {
                self.zones
                    .iter()
                    .filter(|z| z.countries.iter().any(|c| c == code))
                    .collect()
            })
            .unwrap_or_default();
        let candidates: Vec<&ZoneTabRow> = if country_matches.is_empty() {
            self.zones.iter().collect()
        } else {
            country_matches
        };
        candidates
            .into_iter()
            .min_by(|a, b| {
                haversine_m(lat, lon, a.lat, a.lon).total_cmp(&haversine_m(lat, lon, b.lat, b.lon))
            })
            .map(|z| z.tz.clone())
            .unwrap_or_else(|| "UTC".to_owned())
    }
}

/// Local capture-time projection from a UTC RFC 3339 string and an IANA timezone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTimeParts {
    pub local_iso: String,
    pub local_date: String,
    pub local_hm: String,
    pub utc_offset_seconds: i32,
}

/// Parse an ISO-8601 / RFC 3339 timestamp string into UTC epoch seconds.
#[must_use]
pub fn parse_utc_ts(iso: Option<&str>) -> f64 {
    let Some(raw) = iso.map(str::trim).filter(|s| !s.is_empty()) else {
        return 0.0;
    };
    if let Ok(ts) = raw.parse::<Timestamp>() {
        return ts.as_second() as f64 + f64::from(ts.subsec_nanosecond()) / 1e9;
    }
    if let Ok(zoned) = solstone_core_time::parse_rfc3339(raw) {
        let ts = zoned.timestamp();
        return ts.as_second() as f64 + f64::from(ts.subsec_nanosecond()) / 1e9;
    }
    if let Ok(dt) = raw.parse::<jiff::civil::DateTime>()
        && let Ok(ts) = jiff::tz::Offset::UTC.to_timestamp(dt)
    {
        return ts.as_second() as f64;
    }
    0.0
}

/// Convert a UTC RFC 3339 instant to local ISO-8601 with exact DST offset.
#[must_use]
pub fn to_local_time(utc_iso: &str, tz_name: &str) -> Option<LocalTimeParts> {
    let ts: Timestamp = utc_iso
        .trim()
        .parse::<Timestamp>()
        .or_else(|_| solstone_core_time::parse_rfc3339(utc_iso.trim()).map(|z| z.timestamp()))
        .ok()?;
    let tz = TimeZone::get(tz_name).unwrap_or(TimeZone::UTC);
    let offset = tz.to_offset(ts);
    let offset_secs = offset.seconds();
    let wall = offset.to_datetime(ts);
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let abs = offset_secs.unsigned_abs();
    let oh = abs / 3600;
    let om = (abs % 3600) / 60;
    let d = wall.date();
    let t = wall.time();
    let local_date = format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day());
    let local_hm = format!("{:02}:{:02}", t.hour(), t.minute());
    let local_iso = format!(
        "{local_date}T{:02}:{:02}:{:02}{sign}{oh:02}:{om:02}",
        t.hour(),
        t.minute(),
        t.second()
    );
    Some(LocalTimeParts {
        local_iso,
        local_date,
        local_hm,
        utc_offset_seconds: offset_secs,
    })
}

/// Classification of one coordinate against learned routine places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocationClassification {
    pub cluster_id: Option<String>,
    pub is_routine: bool,
    pub is_home: bool,
    pub away_from_routine: bool,
}

#[derive(Default)]
struct GridCell {
    days: BTreeSet<jiff::civil::Date>,
    lats: Vec<f64>,
    lons: Vec<f64>,
}

struct WorkCluster {
    lat: f64,
    lon: f64,
    days: BTreeSet<jiff::civil::Date>,
    photos: usize,
}

/// Deterministically learn routine places from located own-still rows.
#[must_use]
pub fn learn_routine_places(
    located_rows: &[&ScanAssetRow],
    cluster_radius_km: f64,
    window_days: i64,
    min_days: usize,
    min_months: usize,
    min_span_days: i64,
    resolver: &GeoResolver,
) -> RoutinePlacesModel {
    let mut empty = RoutinePlacesModel {
        cluster_radius_km,
        window_days,
        ..RoutinePlacesModel::default()
    };
    if located_rows.is_empty() {
        return empty;
    }

    let mut parsed: Vec<(Timestamp, jiff::civil::Date, f64, f64)> = Vec::new();
    for r in located_rows {
        let Some(loc) = &r.location else {
            continue;
        };
        let Some(ts_str) = r.captured_at.as_deref() else {
            continue;
        };
        let Ok(ts) = ts_str
            .trim()
            .parse::<Timestamp>()
            .or_else(|_| solstone_core_time::parse_rfc3339(ts_str.trim()).map(|z| z.timestamp()))
        else {
            continue;
        };
        let date = jiff::tz::Offset::UTC.to_datetime(ts).date();
        parsed.push((ts, date, loc.latitude, loc.longitude));
    }
    if parsed.is_empty() {
        return empty;
    }

    let max_ts = parsed
        .iter()
        .map(|p| p.0)
        .max()
        .unwrap_or(Timestamp::UNIX_EPOCH);
    let max_date = jiff::tz::Offset::UTC.to_datetime(max_ts).date();
    let min_date_all = parsed.iter().map(|p| p.1).min().unwrap_or(max_date);
    let window_start_ts = max_ts
        .checked_sub(jiff::SignedDuration::from_hours(
            window_days.saturating_mul(24),
        ))
        .unwrap_or(Timestamp::UNIX_EPOCH);
    let window_start_date = jiff::tz::Offset::UTC.to_datetime(window_start_ts).date();

    let in_window: Vec<_> = parsed
        .iter()
        .copied()
        .filter(|p| p.0 >= window_start_ts)
        .collect();
    let active = if in_window.is_empty() {
        parsed
    } else {
        in_window
    };

    // Quantize to ~550m fine cells (lat*200, lon*200 rounded to integer) for speed
    // and input-order independence.
    let mut cells: BTreeMap<(i64, i64), GridCell> = BTreeMap::new();
    for (_, date, lat, lon) in active {
        let key = ((lat * 200.0).round() as i64, (lon * 200.0).round() as i64);
        let cell = cells.entry(key).or_default();
        cell.days.insert(date);
        cell.lats.push(lat);
        cell.lons.push(lon);
    }

    let mut ordered: Vec<((i64, i64), GridCell)> = cells.into_iter().collect();
    ordered.sort_by(|(ka, va), (kb, vb)| {
        vb.days
            .len()
            .cmp(&va.days.len())
            .then_with(|| vb.lats.len().cmp(&va.lats.len()))
            .then_with(|| ka.0.cmp(&kb.0))
            .then_with(|| ka.1.cmp(&kb.1))
    });

    let mut clusters: Vec<WorkCluster> = Vec::new();
    for (_, info) in ordered {
        let n = info.lats.len();
        let clat = info.lats.iter().sum::<f64>() / (n as f64);
        let clon = info.lons.iter().sum::<f64>() / (n as f64);
        let mut merged = false;
        for c in &mut clusters {
            if haversine_km(clat, clon, c.lat, c.lon) <= cluster_radius_km {
                c.days.extend(info.days.iter().copied());
                let total_n = c.photos + n;
                c.lat = (c.lat * (c.photos as f64) + clat * (n as f64)) / (total_n as f64);
                c.lon = (c.lon * (c.photos as f64) + clon * (n as f64)) / (total_n as f64);
                c.photos = total_n;
                merged = true;
                break;
            }
        }
        if !merged {
            clusters.push(WorkCluster {
                lat: clat,
                lon: clon,
                days: info.days,
                photos: n,
            });
        }
    }

    clusters.sort_by(|a, b| {
        b.days
            .len()
            .cmp(&a.days.len())
            .then_with(|| b.photos.cmp(&a.photos))
            .then_with(|| {
                ((a.lat * 10_000.0).round() as i64).cmp(&((b.lat * 10_000.0).round() as i64))
            })
            .then_with(|| {
                ((a.lon * 10_000.0).round() as i64).cmp(&((b.lon * 10_000.0).round() as i64))
            })
    });

    let data_span_days = max_date
        .since(min_date_all)
        .map(|s| i64::from(s.get_days()))
        .unwrap_or(0);
    let home_lat = clusters[0].lat;
    let home_lon = clusters[0].lon;

    let mut out_clusters = Vec::with_capacity(clusters.len());
    for (idx, c) in clusters.iter().enumerate() {
        let n_days = c.days.len();
        let months: BTreeSet<(i16, i8)> = c.days.iter().map(|d| (d.year(), d.month())).collect();
        let n_months = months.len();
        let first_d = *c.days.first().unwrap_or(&max_date);
        let last_d = *c.days.last().unwrap_or(&max_date);
        let span = last_d
            .since(first_d)
            .map(|s| i64::from(s.get_days()))
            .unwrap_or(0);
        let dist_home = (haversine_km(home_lat, home_lon, c.lat, c.lon) * 100.0).round() / 100.0;
        let is_routine = n_days >= min_days && n_months >= min_months && span >= min_span_days;
        let geo = resolver.lookup(c.lat, c.lon);
        let cid = format!("place-{:04}", idx + 1);
        out_clusters.push(RoutineCluster {
            cluster_id: cid,
            lat: (c.lat * 1_000_000.0).round() / 1_000_000.0,
            lon: (c.lon * 1_000_000.0).round() / 1_000_000.0,
            days: n_days,
            months: n_months,
            span_days: span,
            photos: c.photos,
            dist_home_km: dist_home,
            is_routine,
            is_home: idx == 0 && is_routine,
            coarse_place: geo.coarse_place,
            timezone: Some(geo.timezone),
        });
    }

    let routine_count = out_clusters.iter().filter(|c| c.is_routine).count();
    let home_cluster = out_clusters
        .iter()
        .find(|c| c.is_routine)
        .or_else(|| out_clusters.first());

    empty.data_span_days = data_span_days;
    empty.window_start = Some(format!(
        "{:04}-{:02}-{:02}",
        window_start_date.year(),
        window_start_date.month(),
        window_start_date.day()
    ));
    empty.window_end = Some(format!(
        "{:04}-{:02}-{:02}",
        max_date.year(),
        max_date.month(),
        max_date.day()
    ));
    empty.home_cluster_id = home_cluster.map(|c| c.cluster_id.clone());
    empty.home_timezone = home_cluster.and_then(|c| c.timezone.clone());
    empty.routine_count = routine_count;
    empty.total_clusters = out_clusters.len();
    empty.clusters = out_clusters;
    empty
}

/// Match one coordinate against learned routine clusters.
#[must_use]
pub fn classify_location(
    lat: f64,
    lon: f64,
    model: &RoutinePlacesModel,
    overrides: &BTreeMap<String, bool>,
) -> LocationClassification {
    let radius = if model.cluster_radius_km > 0.0 {
        model.cluster_radius_km
    } else {
        1.5
    };
    let mut best: Option<(&RoutineCluster, f64)> = None;
    for c in &model.clusters {
        let d = haversine_km(lat, lon, c.lat, c.lon);
        if d <= radius && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((c, d));
        }
    }
    let Some((cluster, _)) = best else {
        return LocationClassification {
            cluster_id: None,
            is_routine: false,
            is_home: false,
            away_from_routine: true,
        };
    };
    let is_routine = overrides
        .get(&cluster.cluster_id)
        .copied()
        .unwrap_or(cluster.is_routine);
    LocationClassification {
        cluster_id: Some(cluster.cluster_id.clone()),
        is_routine,
        is_home: cluster.is_home && is_routine,
        away_from_routine: !is_routine,
    }
}
