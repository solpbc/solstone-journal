// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

pub const SOURCE_APPLE_HEALTH: &str = "apple_health";
pub const SOURCE_OURA: &str = "oura";
pub const SOURCE_OURA_API: &str = "oura_api";
pub const SOURCE_DEXCOM_CLARITY: &str = "dexcom_clarity";
pub const SOURCE_STRAVA: &str = "strava";

struct HealthCardFamilyEntry {
    family: &'static str,
    stream: Option<&'static str>,
    is_wire: bool,
}

const HEALTH_CARD_FAMILY_COUNT: usize = 5;

const HEALTH_CARD_FAMILIES: [HealthCardFamilyEntry; HEALTH_CARD_FAMILY_COUNT] = [
    HealthCardFamilyEntry {
        family: SOURCE_APPLE_HEALTH,
        stream: Some("import.apple_health"),
        is_wire: true,
    },
    HealthCardFamilyEntry {
        family: SOURCE_OURA_API,
        stream: Some("import.oura"),
        is_wire: true,
    },
    HealthCardFamilyEntry {
        family: SOURCE_OURA,
        stream: None,
        is_wire: false,
    },
    HealthCardFamilyEntry {
        family: SOURCE_DEXCOM_CLARITY,
        stream: None,
        is_wire: false,
    },
    HealthCardFamilyEntry {
        family: SOURCE_STRAVA,
        stream: Some("import.strava"),
        is_wire: false,
    },
];

pub const HEALTH_CARD_STREAM_BY_FAMILY: [(&str, Option<&str>); HEALTH_CARD_FAMILY_COUNT] = {
    let mut pairs = [("", None); HEALTH_CARD_FAMILY_COUNT];
    let mut index = 0;
    while index < HEALTH_CARD_FAMILY_COUNT {
        pairs[index] = (
            HEALTH_CARD_FAMILIES[index].family,
            HEALTH_CARD_FAMILIES[index].stream,
        );
        index += 1;
    }
    pairs
};

#[derive(Debug, PartialEq, Eq)]
pub enum HealthCardStreamError {
    UnknownFamily { family: String },
    NoCardStream { family: String },
}

impl fmt::Display for HealthCardStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFamily { family } => {
                write!(formatter, "Unknown health source family: {family:?}")
            }
            Self::NoCardStream { family } => write!(
                formatter,
                "Health source family {family:?} does not declare a chronicle card stream"
            ),
        }
    }
}

impl std::error::Error for HealthCardStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnknownFamily { .. } | Self::NoCardStream { .. } => None,
        }
    }
}

pub fn health_card_stream(family: &str) -> Result<&'static str, HealthCardStreamError> {
    match HEALTH_CARD_FAMILIES
        .iter()
        .find(|entry| entry.family == family)
    {
        None => Err(HealthCardStreamError::UnknownFamily {
            family: family.to_owned(),
        }),
        Some(entry) => match entry.stream {
            Some(stream) => Ok(stream),
            None => Err(HealthCardStreamError::NoCardStream {
                family: family.to_owned(),
            }),
        },
    }
}

/// The body streams. Their content is read like any other journal content; a
/// body import thinks only about its recent days (see the import publisher).
pub fn health_card_streams() -> impl Iterator<Item = &'static str> {
    HEALTH_CARD_FAMILIES.iter().filter_map(|entry| entry.stream)
}

pub(crate) fn is_manifest_wire_family(family: &str) -> bool {
    HEALTH_CARD_FAMILIES
        .iter()
        .any(|entry| entry.family == family && entry.is_wire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BodySourceFamily, BodySourcePolicyError};

    #[test]
    fn health_card_stream_lookups_and_manifest_wire_acceptance() {
        assert_eq!(
            health_card_stream(SOURCE_APPLE_HEALTH).unwrap(),
            "import.apple_health"
        );
        assert_eq!(health_card_stream(SOURCE_OURA_API).unwrap(), "import.oura");
        assert_eq!(health_card_stream("strava").unwrap(), "import.strava");
        assert!(matches!(
            health_card_stream(SOURCE_OURA).unwrap_err(),
            HealthCardStreamError::NoCardStream { .. }
        ));
        assert!(matches!(
            health_card_stream(SOURCE_DEXCOM_CLARITY).unwrap_err(),
            HealthCardStreamError::NoCardStream { .. }
        ));
        assert!(matches!(
            health_card_stream("unregistered").unwrap_err(),
            HealthCardStreamError::UnknownFamily { .. }
        ));

        let streams = health_card_streams().collect::<Vec<_>>();
        assert_eq!(
            streams,
            ["import.apple_health", "import.oura", "import.strava"]
        );

        assert_eq!(
            BodySourceFamily::from_bytes(b"apple_health").unwrap(),
            BodySourceFamily::AppleHealth
        );
        assert_eq!(
            BodySourceFamily::from_bytes(b"oura_api").unwrap(),
            BodySourceFamily::OuraApi
        );
        assert!(matches!(
            BodySourceFamily::from_bytes(b"strava").unwrap_err(),
            BodySourcePolicyError::InvalidFormat(_)
        ));
        assert!(matches!(
            BodySourceFamily::from_bytes(b"oura").unwrap_err(),
            BodySourcePolicyError::InvalidFormat(_)
        ));
        assert!(matches!(
            BodySourceFamily::from_bytes(b"dexcom_clarity").unwrap_err(),
            BodySourcePolicyError::InvalidFormat(_)
        ));
        assert!(is_manifest_wire_family(SOURCE_APPLE_HEALTH));
        assert!(is_manifest_wire_family(SOURCE_OURA_API));
        assert!(!is_manifest_wire_family(SOURCE_OURA));
        assert!(!is_manifest_wire_family(SOURCE_DEXCOM_CLARITY));
        assert!(!is_manifest_wire_family(SOURCE_STRAVA));
    }
}
