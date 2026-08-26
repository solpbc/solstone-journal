// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::http::{HeaderMap, StatusCode};
use solstone_core_convey_http::identity::AccessBasis;

use crate::model::ReasonCode;

pub const PROTOCOL_HEADER: &str = "X-Solstone-Protocol-Version";

pub fn validate_access(basis: &AccessBasis) -> Result<String, (ReasonCode, StatusCode, String)> {
    match basis {
        AccessBasis::LinkedDevice { cid, .. } => Ok(cid.as_str().to_owned()),
        AccessBasis::Localhost => Err((
            ReasonCode::LinkedDeviceRequired,
            StatusCode::FORBIDDEN,
            "a linked device identity is required".to_owned(),
        )),
        AccessBasis::PairingPeer { .. } => Err((
            ReasonCode::LinkedDeviceRequired,
            StatusCode::FORBIDDEN,
            "a linked device identity is required".to_owned(),
        )),
    }
}

pub fn validate_protocol(headers: &HeaderMap) -> Result<(), (ReasonCode, StatusCode, String)> {
    let Some(raw) = headers.get(PROTOCOL_HEADER) else {
        return Err((
            ReasonCode::ProtocolVersionRequired,
            StatusCode::BAD_REQUEST,
            "missing X-Solstone-Protocol-Version".to_owned(),
        ));
    };
    let Ok(raw) = raw.to_str() else {
        return Err((
            ReasonCode::ProtocolVersionMalformed,
            StatusCode::BAD_REQUEST,
            "protocol version is not text".to_owned(),
        ));
    };
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err((
            ReasonCode::ProtocolVersionMalformed,
            StatusCode::BAD_REQUEST,
            "protocol version must be decimal".to_owned(),
        ));
    }
    let Ok(version) = raw.parse::<u64>() else {
        return Err((
            ReasonCode::ProtocolVersionMalformed,
            StatusCode::BAD_REQUEST,
            "protocol version overflows".to_owned(),
        ));
    };
    match version.cmp(&3) {
        std::cmp::Ordering::Less => Err((
            ReasonCode::ProtocolVersionLegacy,
            StatusCode::UPGRADE_REQUIRED,
            "protocol version 3 is required".to_owned(),
        )),
        std::cmp::Ordering::Greater => Err((
            ReasonCode::ProtocolVersionFuture,
            StatusCode::UPGRADE_REQUIRED,
            "protocol version is newer than this server".to_owned(),
        )),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

pub(crate) fn validate_source(raw: &[u8]) -> Result<String, ReasonCode> {
    let source = std::str::from_utf8(raw).map_err(|_| ReasonCode::SourceNotUtf8)?;
    if source.len() > 64 {
        return Err(ReasonCode::SourceTooLong);
    }
    if source.as_bytes().contains(&0) {
        return Err(ReasonCode::SourceContainsNul);
    }
    if source.contains('/') || source.contains('\\') {
        return Err(ReasonCode::SourceContainsPathSeparator);
    }
    if source.contains('.') {
        return Err(ReasonCode::SourceContainsDot);
    }
    let mut bytes = source.bytes();
    let Some(first) = bytes.next() else {
        return Ok(String::new());
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(ReasonCode::SourceInvalidCharacter);
    }
    if !bytes.all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
    }) {
        return Err(ReasonCode::SourceInvalidCharacter);
    }
    Ok(source.to_owned())
}

pub fn validate_day(day: &str) -> Result<(), ReasonCode> {
    if day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(ReasonCode::DayInvalid)
    }
}

/// Validate only the `HHMMSS_LEN` shape; LEN intentionally has no numeric ceiling.
pub fn validate_segment(segment: &str) -> Result<(), ReasonCode> {
    let Some((clock, len)) = segment.split_once('_') else {
        return Err(ReasonCode::SegmentInvalid);
    };
    if clock.len() == 6
        && clock.bytes().all(|byte| byte.is_ascii_digit())
        && !len.is_empty()
        && len.bytes().all(|byte| byte.is_ascii_digit())
    {
        Ok(())
    } else {
        Err(ReasonCode::SegmentInvalid)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode};

    use crate::model::ReasonCode;

    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};

    use super::{
        PROTOCOL_HEADER, validate_access, validate_protocol, validate_segment, validate_source,
    };

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn access_validation_accepts_linked_devices_and_refuses_pairing_peers() {
        let linked = AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
        };
        assert_eq!(validate_access(&linked), Ok(VALID_CID.to_owned()));

        let refusal = validate_access(&AccessBasis::PairingPeer {
            carrier: Carrier::Direct,
        })
        .unwrap_err();
        assert_eq!(refusal.0, ReasonCode::LinkedDeviceRequired);
        assert_eq!(refusal.1, StatusCode::FORBIDDEN);
    }

    #[test]
    fn segment_length_has_no_numeric_ceiling() {
        assert!(validate_segment("120000_1").is_ok());
        assert!(validate_segment("120000_86400").is_ok());
    }

    #[test]
    fn source_validation_reports_each_refusal() {
        assert_eq!(validate_source(b""), Ok(String::new()));
        assert_eq!(validate_source(&[0xff]), Err(ReasonCode::SourceNotUtf8));
        assert_eq!(validate_source(&[b'a'; 65]), Err(ReasonCode::SourceTooLong));
        assert_eq!(validate_source(b"a\0b"), Err(ReasonCode::SourceContainsNul));
        assert_eq!(
            validate_source(b"a/b"),
            Err(ReasonCode::SourceContainsPathSeparator)
        );
        assert_eq!(
            validate_source(b"a\\b"),
            Err(ReasonCode::SourceContainsPathSeparator)
        );
        assert_eq!(validate_source(b"a.b"), Err(ReasonCode::SourceContainsDot));
        assert_eq!(
            validate_source(b"Upper"),
            Err(ReasonCode::SourceInvalidCharacter)
        );
    }

    #[test]
    fn protocol_validation_distinguishes_every_version_refusal() {
        let missing = HeaderMap::new();
        assert_eq!(
            validate_protocol(&missing).unwrap_err().0,
            ReasonCode::ProtocolVersionRequired
        );
        for (value, code, status) in [
            (
                "three",
                ReasonCode::ProtocolVersionMalformed,
                StatusCode::BAD_REQUEST,
            ),
            (
                "2",
                ReasonCode::ProtocolVersionLegacy,
                StatusCode::UPGRADE_REQUIRED,
            ),
            (
                "4",
                ReasonCode::ProtocolVersionFuture,
                StatusCode::UPGRADE_REQUIRED,
            ),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(PROTOCOL_HEADER, HeaderValue::from_static(value));
            let refusal = validate_protocol(&headers).unwrap_err();
            assert_eq!(refusal.0, code);
            assert_eq!(refusal.1, status);
        }
    }
}
