// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical occurrence resolution and member identity.

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_time::{LocalResult, from_local, parse_rfc3339};

use super::plan::LeftOutReason;
use crate::timestamp::epoch_millis;

/// Starting time evidence provided for an audio archive member.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberStartEvidence {
    /// RFC3339 timestamp with explicit offset ('Z' or '±HH:MM').
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instant: Option<String>,
    /// Civil local time without offset (e.g. 'YYYY-MM-DDTHH:MM:SS' or 'YYYY-MM-DDTHH:MM:SS.mmm').
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_civil: Option<String>,
    /// IANA time zone identifier for `local_civil` (e.g. 'America/New_York').
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_zone: Option<String>,
    /// Origin of the owner's stated time, retained even when it cannot be settled.
    #[serde(default)]
    pub authority: MemberTimeAuthority,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberTimeAuthority {
    #[default]
    Manifest,
    Filename,
}

/// Resolved occurrence of a member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedOccurrence {
    /// Occurrence settled to a single UTC instant.
    UniqueInstant {
        instant: Timestamp,
        canonical: String,
    },
    /// Occurrence settled across an ambiguous fold to a stable civil time.
    SettledFold {
        candidate_instants: Vec<Timestamp>,
        canonical: String,
    },
}

impl ResolvedOccurrence {
    pub fn canonical(&self) -> &str {
        match self {
            Self::UniqueInstant { canonical, .. } => canonical,
            Self::SettledFold { canonical, .. } => canonical,
        }
    }

    pub fn original_time_ms(&self) -> Option<i64> {
        match self {
            Self::UniqueInstant { instant, .. } => Some(epoch_millis(*instant)),
            Self::SettledFold { .. } => None,
        }
    }
}

/// Member identity used for claim-index lookup and deduplication.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MemberIdentity {
    pub byte_len: u64,
    pub sha256: String,
    pub occurrence: String,
}

impl MemberIdentity {
    /// Raw targets bind the complete identity, including the stated occurrence.
    pub fn storage_digest(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(self.byte_len.to_be_bytes());
        digest.update(self.sha256.as_bytes());
        digest.update([0]);
        digest.update(self.occurrence.as_bytes());
        format!("{:x}", digest.finalize())
    }

    pub fn new(byte_len: u64, sha256: impl Into<String>, occurrence: impl Into<String>) -> Self {
        Self {
            byte_len,
            sha256: sha256.into().to_ascii_lowercase(),
            occurrence: occurrence.into(),
        }
    }
}

/// Attempt to resolve member start evidence into a unique instant or candidate instants.
pub fn resolve_start_evidence(
    start: &MemberStartEvidence,
    owner_zone: &TimeZone,
    duration_ms: i64,
) -> Result<ResolvedOccurrence, LeftOutReason> {
    let parsed_instant = match &start.instant {
        Some(s) => Some(parse_rfc3339_offset_instant(s)?),
        None => None,
    };

    let (parsed_civil, parsed_zone) = match (&start.local_civil, &start.source_zone) {
        (Some(civil_str), Some(zone_str)) => {
            let zone = solstone_core_time::zone(zone_str).ok_or(LeftOutReason::UnsettledTime)?;
            let civil = parse_civil_datetime(civil_str, &zone)?;
            (Some(civil), Some(zone))
        }
        (Some(_), None) => return Err(LeftOutReason::UnsettledTime),
        (None, Some(_)) => return Err(LeftOutReason::UnsettledTime),
        (None, None) => (None, None),
    };

    match (parsed_instant, parsed_civil, parsed_zone) {
        (None, None, None) => Err(LeftOutReason::UnsettledTime),
        (Some(instant), None, None) => {
            let canonical = format!("utc|{}", epoch_millis(instant));
            Ok(ResolvedOccurrence::UniqueInstant { instant, canonical })
        }
        (None, Some(civil), Some(zone)) => {
            resolve_civil_in_zone(civil, &zone, owner_zone, duration_ms)
        }
        (Some(instant), Some(civil), Some(zone)) => {
            let local_in_source = instant.to_zoned_datetime(zone.clone());
            let matches_civil = local_in_source.year() == civil.year()
                && local_in_source.month() == civil.month()
                && local_in_source.day() == civil.day()
                && local_in_source.hour() == civil.hour()
                && local_in_source.minute() == civil.minute()
                && local_in_source.second() == civil.second()
                && local_in_source.subsec_nanosecond() / 1_000_000
                    == civil.subsec_nanosecond() / 1_000_000;

            if !matches_civil {
                return Err(LeftOutReason::ConflictingEvidence);
            }

            match from_local(&zone, civil) {
                LocalResult::None => Err(LeftOutReason::NonexistentLocalTime),
                LocalResult::Single(_) => {
                    let canonical = format!("utc|{}", epoch_millis(instant));
                    Ok(ResolvedOccurrence::UniqueInstant { instant, canonical })
                }
                LocalResult::Ambiguous(inst1, inst2) => {
                    let c1 = inst1.timestamp();
                    let c2 = inst2.timestamp();
                    if instant == c1 || instant == c2 {
                        let canonical = format!("utc|{}", epoch_millis(instant));
                        Ok(ResolvedOccurrence::UniqueInstant { instant, canonical })
                    } else {
                        Err(LeftOutReason::ConflictingEvidence)
                    }
                }
            }
        }
        _ => Err(LeftOutReason::UnsettledTime),
    }
}

fn parse_rfc3339_offset_instant(s: &str) -> Result<Timestamp, LeftOutReason> {
    let trimmed = s.trim();
    let Some((_date, time_part)) = trimmed.split_once(['T', 't', ' ']) else {
        return Err(LeftOutReason::UnsettledTime);
    };
    if !time_part.contains('Z')
        && !time_part.contains('z')
        && !time_part.contains('+')
        && !time_part.contains('-')
    {
        return Err(LeftOutReason::UnsettledTime);
    }
    parse_rfc3339(trimmed)
        .map(|dt| dt.timestamp())
        .map_err(|_| LeftOutReason::UnsettledTime)
}

fn parse_civil_datetime(s: &str, zone: &TimeZone) -> Result<DateTime, LeftOutReason> {
    let trimmed = s.trim();
    // If the string carries its own offset, it must not disagree with the source zone
    if let Ok(dt) = parse_rfc3339(trimmed) {
        let expected_offset = zone.to_offset(dt.timestamp());
        if dt.offset().seconds() != expected_offset.seconds() {
            return Err(LeftOutReason::ConflictingEvidence);
        }
        return Ok(dt.datetime());
    }

    if let Ok(dt) = DateTime::strptime("%Y-%m-%dT%H:%M:%S%.f", trimmed) {
        return Ok(dt);
    }
    if let Ok(dt) = DateTime::strptime("%Y-%m-%d %H:%M:%S%.f", trimmed) {
        return Ok(dt);
    }
    if let Ok(dt) = DateTime::strptime("%Y-%m-%dT%H:%M:%S", trimmed) {
        return Ok(dt);
    }
    if let Ok(dt) = DateTime::strptime("%Y-%m-%d %H:%M:%S", trimmed) {
        return Ok(dt);
    }
    Err(LeftOutReason::UnsettledTime)
}

fn resolve_civil_in_zone(
    civil: DateTime,
    zone: &TimeZone,
    owner_zone: &TimeZone,
    duration_ms: i64,
) -> Result<ResolvedOccurrence, LeftOutReason> {
    match from_local(zone, civil) {
        LocalResult::None => Err(LeftOutReason::NonexistentLocalTime),
        LocalResult::Single(inst) => {
            let instant = inst.timestamp();
            let canonical = format!("utc|{}", epoch_millis(instant));
            Ok(ResolvedOccurrence::UniqueInstant { instant, canonical })
        }
        LocalResult::Ambiguous(inst1, inst2) => {
            if duration_ms > super::plan::AUDIO_ARCHIVE_WINDOW_MS {
                return Err(LeftOutReason::UnsettledTime);
            }
            let cand1 = inst1.timestamp();
            let cand2 = inst2.timestamp();

            let parts1 = super::plan::simulate_parts(cand1, owner_zone, duration_ms);
            let parts2 = super::plan::simulate_parts(cand2, owner_zone, duration_ms);

            match (parts1, parts2) {
                (Ok(p1), Ok(p2)) if p1 == p2 => {
                    let sub_ms = civil.subsec_nanosecond() / 1_000_000;
                    let formatted = if sub_ms == 0 {
                        civil.strftime("%Y-%m-%dT%H:%M:%S.000").to_string()
                    } else {
                        format!("{}.{:03}", civil.strftime("%Y-%m-%dT%H:%M:%S"), sub_ms)
                    };
                    let name = zone.iana_name().unwrap_or_default();
                    let canonical = format!("local|{name}|{formatted}");
                    Ok(ResolvedOccurrence::SettledFold {
                        candidate_instants: vec![cand1, cand2],
                        canonical,
                    })
                }
                _ => Err(LeftOutReason::UnsettledTime),
            }
        }
    }
}
