// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Notification payload sealing and opening for linked push devices.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{Month, OffsetDateTime, UtcOffset};

const PROTOCOL_VERSION: u8 = 0x01;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const PADDED_PAYLOAD_LEN: usize = 1024;
const ENVELOPE_LEN: usize = 1 + NONCE_LEN + PADDED_PAYLOAD_LEN + TAG_LEN; // 1053 bytes

/// A 32-byte secret key used to seal and open push notification envelopes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PushKey {
    bytes: [u8; 32],
}

impl PushKey {
    /// Construct a `PushKey` from validated raw 32 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    /// Construct a `PushKey` from a URL-safe unpadded base64 string.
    pub(crate) fn from_base64url(value: &str) -> Result<Self, ()> {
        let decoded = URL_SAFE_NO_PAD.decode(value).map_err(|_| ())?;
        let bytes: [u8; 32] = decoded.try_into().map_err(|_| ())?;
        Ok(Self { bytes })
    }

    /// Encode this key as a URL-safe unpadded base64 string.
    pub(crate) fn to_base64url(self) -> String {
        URL_SAFE_NO_PAD.encode(self.bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

impl fmt::Debug for PushKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PushKey([redacted])")
    }
}

impl Serialize for PushKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_base64url())
    }
}

impl<'de> Deserialize<'de> for PushKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_base64url(&s).map_err(|_| serde::de::Error::custom("invalid push_key"))
    }
}

/// A plaintext push notification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    pub at: OffsetDateTime,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub open: Option<String>,
}

#[derive(Serialize)]
struct CanonicalNotificationJson<'a> {
    v: u32,
    at: String,
    kind: &'a str,
    title: &'a str,
    body: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    open: Option<&'a str>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParsedNotificationJson {
    v: u32,
    at: String,
    kind: String,
    title: String,
    body: String,
    #[serde(default)]
    open: Option<String>,
}

/// A sealed notification envelope ready for delivery.
///
/// ```rust
/// use solstone_core_push::envelope::{Notification, PushKey, seal};
/// use time::OffsetDateTime;
///
/// let key = PushKey::from_bytes([0u8; 32]);
/// let notification = Notification {
///     at: OffsetDateTime::UNIX_EPOCH,
///     kind: "test".to_owned(),
///     title: "Test".to_owned(),
///     body: "Notification".to_owned(),
///     open: None,
/// };
/// let sealed = seal(&key, &notification).expect("seal succeeds");
/// assert_eq!(sealed.as_base64url().len(), 1404);
/// ```
///
/// ```compile_fail,E0451
/// use solstone_core_push::envelope::SealedEnvelope;
///
/// let _ = SealedEnvelope { bytes: Vec::new() };
/// ```
///
/// ```compile_fail,E0277
/// use solstone_core_push::envelope::SealedEnvelope;
///
/// let _: SealedEnvelope = Vec::<u8>::new().into();
/// ```
///
/// ```compile_fail,E0277
/// use solstone_core_push::envelope::SealedEnvelope;
///
/// let _ = serde_json::from_str::<SealedEnvelope>("[]");
/// ```
///
/// ```compile_fail,E0599
/// use solstone_core_push::envelope::SealedEnvelope;
///
/// let _ = SealedEnvelope::default();
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedEnvelope {
    bytes: Vec<u8>,
}

impl SealedEnvelope {
    /// Encode this sealed envelope as unpadded URL-safe base64 (1404 chars).
    #[must_use]
    pub fn as_base64url(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.bytes)
    }
}

/// Errors returned when sealing or opening a push envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SealError {
    TooLarge { len: usize },
    InvalidOpen,
    BadVersion,
    Auth,
    Random,
}

impl fmt::Display for SealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { len } => {
                write!(formatter, "notification payload too large: {len} bytes")
            }
            Self::InvalidOpen => formatter.write_str("invalid open path"),
            Self::BadVersion => formatter.write_str("bad envelope protocol version"),
            Self::Auth => formatter.write_str("envelope authentication failed"),
            Self::Random => formatter.write_str("system randomness failure"),
        }
    }
}

impl std::error::Error for SealError {}

/// Seal a notification with a random nonce.
pub fn seal(key: &PushKey, notification: &Notification) -> Result<SealedEnvelope, SealError> {
    let mut nonce = [0u8; NONCE_LEN];
    let rng = SystemRandom::new();
    rng.fill(&mut nonce).map_err(|_| SealError::Random)?;
    seal_inner(key, notification, &nonce)
}

fn seal_inner(
    key: &PushKey,
    notification: &Notification,
    nonce: &[u8; NONCE_LEN],
) -> Result<SealedEnvelope, SealError> {
    if let Some(open) = &notification.open
        && !validate_open_path(open)
    {
        return Err(SealError::InvalidOpen);
    }

    let at_utc = notification.at.to_offset(UtcOffset::UTC);
    let at_str = format_notification_at(at_utc);

    let canonical = CanonicalNotificationJson {
        v: 1,
        at: at_str,
        kind: &notification.kind,
        title: &notification.title,
        body: &notification.body,
        open: notification.open.as_deref(),
    };

    let json_bytes = serde_json::to_vec(&canonical).map_err(|_| SealError::Auth)?;
    if json_bytes.len() >= PADDED_PAYLOAD_LEN {
        return Err(SealError::TooLarge {
            len: json_bytes.len(),
        });
    }

    let mut padded = Vec::with_capacity(PADDED_PAYLOAD_LEN);
    padded.extend_from_slice(&json_bytes);
    padded.push(0x80);
    padded.resize(PADDED_PAYLOAD_LEN, 0x00);

    let unbound_key =
        UnboundKey::new(&aead::AES_256_GCM, key.as_bytes()).map_err(|_| SealError::Auth)?;
    let less_safe_key = LessSafeKey::new(unbound_key);
    let aead_nonce = Nonce::assume_unique_for_key(*nonce);

    let mut ciphertext = padded;
    less_safe_key
        .seal_in_place_append_tag(aead_nonce, Aad::from([PROTOCOL_VERSION]), &mut ciphertext)
        .map_err(|_| SealError::Auth)?;

    let mut envelope = Vec::with_capacity(ENVELOPE_LEN);
    envelope.push(PROTOCOL_VERSION);
    envelope.extend_from_slice(nonce);
    envelope.extend_from_slice(&ciphertext);

    Ok(SealedEnvelope { bytes: envelope })
}

#[cfg(test)]
fn seal_with_nonce(
    key: &PushKey,
    notification: &Notification,
    nonce: &[u8; NONCE_LEN],
) -> Result<SealedEnvelope, SealError> {
    seal_inner(key, notification, nonce)
}

#[allow(dead_code)]
pub(crate) fn open_envelope(
    key: &PushKey,
    envelope_bytes: &[u8],
) -> Result<Notification, SealError> {
    if envelope_bytes.is_empty() {
        return Err(SealError::Auth);
    }
    if envelope_bytes[0] != PROTOCOL_VERSION {
        return Err(SealError::BadVersion);
    }
    if envelope_bytes.len() != ENVELOPE_LEN {
        return Err(SealError::Auth);
    }

    let nonce_bytes: [u8; NONCE_LEN] = envelope_bytes[1..1 + NONCE_LEN]
        .try_into()
        .map_err(|_| SealError::Auth)?;
    let mut ciphertext_and_tag = envelope_bytes[1 + NONCE_LEN..].to_vec();

    let unbound_key =
        UnboundKey::new(&aead::AES_256_GCM, key.as_bytes()).map_err(|_| SealError::Auth)?;
    let less_safe_key = LessSafeKey::new(unbound_key);
    let aead_nonce = Nonce::assume_unique_for_key(nonce_bytes);

    let plaintext = less_safe_key
        .open_in_place(
            aead_nonce,
            Aad::from([PROTOCOL_VERSION]),
            &mut ciphertext_and_tag,
        )
        .map_err(|_| SealError::Auth)?;

    if plaintext.len() != PADDED_PAYLOAD_LEN {
        return Err(SealError::Auth);
    }

    let mut end = plaintext.len();
    while end > 0 && plaintext[end - 1] == 0x00 {
        end -= 1;
    }
    if end == 0 || plaintext[end - 1] != 0x80 {
        return Err(SealError::Auth);
    }
    let json_slice = &plaintext[..end - 1];

    let parsed: ParsedNotificationJson =
        serde_json::from_slice(json_slice).map_err(|_| SealError::Auth)?;
    if parsed.v != 1 {
        return Err(SealError::Auth);
    }

    let at = parse_notification_at(&parsed.at).ok_or(SealError::Auth)?;

    if let Some(open) = &parsed.open
        && !validate_open_path(open)
    {
        return Err(SealError::Auth);
    }

    Ok(Notification {
        at,
        kind: parsed.kind,
        title: parsed.title,
        body: parsed.body,
        open: parsed.open,
    })
}

fn validate_open_path(path: &str) -> bool {
    if !path.starts_with("/app/") {
        return false;
    }
    if path.contains("//") {
        return false;
    }
    for segment in path.split('/') {
        if segment == "." || segment == ".." {
            return false;
        }
    }
    for b in path.bytes() {
        let valid = b.is_ascii_alphanumeric()
            || b == b'.'
            || b == b'_'
            || b == b'~'
            || b == b'/'
            || b == b'-';
        if !valid {
            return false;
        }
    }
    true
}

fn format_notification_at(at: OffsetDateTime) -> String {
    let year = at.year();
    let month = u8::from(at.month());
    let day = at.day();
    let hour = at.hour();
    let minute = at.minute();
    let second = at.second();
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[allow(dead_code)]
fn parse_notification_at(value: &str) -> Option<OffsetDateTime> {
    if value.len() != 20 || !value.ends_with('Z') {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year: i32 = value[0..4].parse().ok()?;
    let month_num: u8 = value[5..7].parse().ok()?;
    let month = Month::try_from(month_num).ok()?;
    let day: u8 = value[8..10].parse().ok()?;
    let hour: u8 = value[11..13].parse().ok()?;
    let minute: u8 = value[14..16].parse().ok()?;
    let second: u8 = value[17..19].parse().ok()?;

    let date = time::Date::from_calendar_date(year, month, day).ok()?;
    let time = time::Time::from_hms(hour, minute, second).ok()?;
    Some(date.with_time(time).assume_utc())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use ring::digest::{Context, SHA256};
    use serde::{Deserialize, Serialize};
    use time::{Duration, OffsetDateTime, UtcOffset};

    use super::*;

    const VECTOR_A_B64URL: &str = "AQABAgMEBQYHCAkKCzwgoDn_1O457DW1sZPbSF-1-7cN3UlrKAhX37UtUzCCWzKC3sSofPxWnl2Z7fRcGsJ7FOQuusb4BbVZdnSQgYGeWeRW8bNJBWV2kEyb6n2cGOSmFh8FOgucifejFtDf_6SVeZ_LNLo69HNLwCYUTVGYGuKr48Ulmk3_V82C5m4Ou1IAxrTXy1MI4UihPdpMwjb04zU1EZNKjF_od_rS6e6Tl54V6f_Mfqdj3frfgIa7nc7KIourc7xl38DHccW38xNMivoN3dbJN1bT5rpB2YThfvh2PQC68XGRmsdBSI_6QyPUr-YgAyVSR7gItRF8UaiTuXWdMImlF2tnc4ucQoEU0fKQndpIPnffcge3LToK2rTKzYCLga9xL21RCvS6SwED3ki6xc4Xkjw6WgqVoNOYylteSZTPF5gxHRaXPUXpyZV_KotgeOmj4HsgXdMdYpMa14ScFIUISaN4Zo2xSP9ZL_nZhVRyyyCvpCOXtWIohP0fee7xAMGf5TTc3FdZMqWDwNmHB9PcoXD-fz5AEofiC2y7nyBS1D0Q3kZN5rtWGXpXa8PRUXKguNxS__iwpV1rRBIcgyDl57nj7YarH3h6eq7drle_fesa4FfxlNVRaYprUsOaHjxDxbqwEf-GtBnvKQGXk9_OUaoEjeis7YCQrNGaPa7hILcCkV3H6FMcoWzq-L8x3FdBu-F9OhJhuIeyOzpue9K0j2VAN7dUHrBsDzaH7EewCzDUV6bKb1vtRq-XP63FdGUK5yrAiiCKUXPtyg40ADdM0KC2g4SYTYcHlp60FYnx77YPuqUj2Cvdt0CXKtp3tJY1m9yDEGbWVXmTRC00vLnfGxIJEt3Wpyol0Y1AtsMY0RfObSbyPw2Ubn2JsRopkOxA2r_k0OWYzYOVRy0YS35ctyXIuxlBMoRvQcW9d8fK98XgNyvDcBARWKfeUwZgzTlmA0e4yvYlh3lLPlWoStjiyv-QjrmA0MauUj3L7MctzhfOhwbx0B2sQxFwWOEBr_e24LmBx2eaj-vrAV3OANH3nIRC5mtemuKM21EjvYD1hnfz5FOAvZaYsNNbB3fAHd8fob2apzYjjfYKRqh3moHQNnfErzZNoaA38ll4vL7Di8D7_4LWCLugF9N1VUQF8n4qIPHppVD4ogdz7lvoxJ1Oj6e0X8N-VqzY0L1c4nLbkIGe5nbE3le2AcSgz8e3Ljt_OoDOTz2HJ2rXXsN3o8a0GOLKLcPDWNeu9iw4Zr-TrVNvZSTBRSom5vUYjUgvsbDZZ_l_DVVgqjIYmBqmBfHJLNG2HcsJV4JQKTHG-wpHQFKx1iyssa5N7PKiXkO5dCXRrkf1SfQbTITj9acKMSs4iO9dkptWTq_Xjj59SFOxPrU7hIt1FJEz";
    const VECTOR_A_SHA256: &str =
        "824015e69eb95a3edb150c4e9e1b8c3e0df60a8d2064bc5681b565a543639070";

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct VectorFile {
        _about: String,
        vectors: Vec<VectorCase>,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct VectorCase {
        id: String,
        key: String,
        nonce: String,
        plaintext_json: String,
        envelope: Option<String>,
        expect: String,
    }

    fn hex_encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    fn test_notification_a() -> Notification {
        Notification {
            at: OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap(), // 2026-09-24T00:00:00Z
            kind: "test".to_owned(),
            title: "solstone".to_owned(),
            body: "test notification from your journal.".to_owned(),
            open: None,
        }
    }

    fn test_notification_a2() -> Notification {
        Notification {
            at: OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap(),
            kind: "test".to_owned(),
            title: "solstone".to_owned(),
            body: "test notification from your journal.".to_owned(),
            open: Some("/app/health".to_owned()),
        }
    }

    #[test]
    fn open_path_validation() {
        assert!(validate_open_path("/app/x"));
        assert!(validate_open_path("/app/health"));

        for invalid in [
            "/api/push/register",
            "/apps/x",
            "/app",
            "app/x",
            "//evil",
            "/a\\b",
            "/a b",
            "/app/../b",
            "/app/./b",
            "/app/a?x=1",
            "/app/a#b",
            "/app/a%20b",
        ] {
            assert!(
                !validate_open_path(invalid),
                "expected {invalid} to be invalid"
            );
        }
    }

    #[test]
    fn envelope_vectors() {
        let vectors_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/design/push-envelope-vectors.json");

        let key_a_bytes: [u8; 32] = (0x00..=0x1f).collect::<Vec<u8>>().try_into().unwrap();
        let nonce_a_bytes: [u8; 12] = (0x00..=0x0b).collect::<Vec<u8>>().try_into().unwrap();
        let key_a = PushKey::from_bytes(key_a_bytes);

        // Case a
        let notif_a = test_notification_a();
        let sealed_a = seal_with_nonce(&key_a, &notif_a, &nonce_a_bytes).expect("seal a");
        let b64url_a = sealed_a.as_base64url();
        assert_eq!(b64url_a, VECTOR_A_B64URL);
        assert_eq!(sealed_a.bytes.len(), ENVELOPE_LEN);
        assert_eq!(sealed_a.bytes[0], 0x01);
        assert_eq!(&sealed_a.bytes[1..13], &nonce_a_bytes);

        let mut ctx = Context::new(&SHA256);
        ctx.update(&sealed_a.bytes);
        let digest_a = ctx.finish();
        let sha256_a = hex_encode(digest_a.as_ref());
        assert_eq!(sha256_a, VECTOR_A_SHA256);

        let opened_a = open_envelope(&key_a, &sealed_a.bytes).expect("open a");
        assert_eq!(opened_a, notif_a);

        // Case a2
        let notif_a2 = test_notification_a2();
        let json_a2 = serde_json::to_string(&CanonicalNotificationJson {
            v: 1,
            at: "2026-09-24T00:00:00Z".to_owned(),
            kind: &notif_a2.kind,
            title: &notif_a2.title,
            body: &notif_a2.body,
            open: notif_a2.open.as_deref(),
        })
        .unwrap();
        assert_eq!(
            json_a2,
            r#"{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":"test notification from your journal.","open":"/app/health"}"#
        );
        let sealed_a2 = seal_with_nonce(&key_a, &notif_a2, &nonce_a_bytes).expect("seal a2");
        let opened_a2 = open_envelope(&key_a, &sealed_a2.bytes).expect("open a2");
        assert_eq!(opened_a2, notif_a2);

        // Case b: Contains UTF-8 0x80 byte (U+0400 'Ѐ', UTF-8: D0 80)
        let notif_b = Notification {
            at: OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap(),
            kind: "test".to_owned(),
            title: "solstone".to_owned(),
            body: "test with 0x80: Ѐ.".to_owned(),
            open: None,
        };
        let sealed_b = seal_with_nonce(&key_a, &notif_b, &nonce_a_bytes).expect("seal b");
        let opened_b = open_envelope(&key_a, &sealed_b.bytes).expect("open b");
        assert_eq!(opened_b, notif_b);

        // Prove forward unpad fails on case b
        {
            let unbound_key = UnboundKey::new(&aead::AES_256_GCM, key_a.as_bytes()).unwrap();
            let less_safe_key = LessSafeKey::new(unbound_key);
            let mut c = sealed_b.bytes[13..].to_vec();
            let pt = less_safe_key
                .open_in_place(
                    Nonce::assume_unique_for_key(nonce_a_bytes),
                    Aad::from([0x01]),
                    &mut c,
                )
                .unwrap();
            let first_80_idx = pt.iter().position(|&b| b == 0x80).unwrap();
            let forward_unpadded = &pt[..first_80_idx];
            // Forward unpad truncated the JSON inside the UTF-8 character, so parsing fails
            assert!(serde_json::from_slice::<ParsedNotificationJson>(forward_unpadded).is_err());
        }

        // Case c: JSON exactly 1023 bytes
        let base_prefix =
            r#"{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":""#;
        let base_suffix = r#""}"#;
        let needed_fill = 1023 - base_prefix.len() - base_suffix.len();
        let body_c = "a".repeat(needed_fill);
        let notif_c = Notification {
            at: OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap(),
            kind: "test".to_owned(),
            title: "solstone".to_owned(),
            body: body_c,
            open: None,
        };
        let sealed_c = seal_with_nonce(&key_a, &notif_c, &nonce_a_bytes).expect("seal c");
        let opened_c = open_envelope(&key_a, &sealed_c.bytes).expect("open c");
        assert_eq!(opened_c, notif_c);

        // Case d: JSON exactly 1024 bytes -> TooLarge
        let body_d = "a".repeat(needed_fill + 1);
        let notif_d = Notification {
            at: OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap(),
            kind: "test".to_owned(),
            title: "solstone".to_owned(),
            body: body_d,
            open: None,
        };
        let err_d = seal_with_nonce(&key_a, &notif_d, &nonce_a_bytes).unwrap_err();
        assert_eq!(err_d, SealError::TooLarge { len: 1024 });

        // Case e: Corrupted tag (last byte flipped)
        let mut corrupted_e = sealed_a.bytes.clone();
        let last_idx = corrupted_e.len() - 1;
        corrupted_e[last_idx] ^= 0x01;
        let err_e = open_envelope(&key_a, &corrupted_e).unwrap_err();
        assert_eq!(err_e, SealError::Auth);

        // Case f: Bad version byte
        let mut bad_version_f = sealed_a.bytes.clone();
        bad_version_f[0] = 0x02;
        let err_f = open_envelope(&key_a, &bad_version_f).unwrap_err();
        assert_eq!(err_f, SealError::BadVersion);

        // Subsecond timestamp serialization
        let notif_subsec = Notification {
            at: OffsetDateTime::from_unix_timestamp(1_790_208_000).unwrap()
                + Duration::nanoseconds(500_000_000),
            kind: "test".to_owned(),
            title: "solstone".to_owned(),
            body: "test".to_owned(),
            open: None,
        };
        let canonical_subsec = CanonicalNotificationJson {
            v: 1,
            at: format_notification_at(notif_subsec.at.to_offset(UtcOffset::UTC)),
            kind: &notif_subsec.kind,
            title: &notif_subsec.title,
            body: &notif_subsec.body,
            open: None,
        };
        assert_eq!(canonical_subsec.at, "2026-09-24T00:00:00Z");

        // Non-UTC timezone offset conversion
        let notif_offset = Notification {
            at: OffsetDateTime::from_unix_timestamp(1_790_208_000)
                .unwrap()
                .to_offset(UtcOffset::from_hms(2, 0, 0).unwrap()),
            kind: "test".to_owned(),
            title: "solstone".to_owned(),
            body: "test".to_owned(),
            open: None,
        };
        let canonical_offset = CanonicalNotificationJson {
            v: 1,
            at: format_notification_at(notif_offset.at.to_offset(UtcOffset::UTC)),
            kind: &notif_offset.kind,
            title: &notif_offset.title,
            body: &notif_offset.body,
            open: None,
        };
        assert_eq!(canonical_offset.at, "2026-09-24T00:00:00Z");

        let vector_cases = vec![
            VectorCase {
                id: "a".to_owned(),
                key: hex_encode(&key_a_bytes),
                nonce: hex_encode(&nonce_a_bytes),
                plaintext_json: r#"{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":"test notification from your journal."}"#.to_owned(),
                envelope: Some(b64url_a),
                expect: "ok".to_owned(),
            },
            VectorCase {
                id: "a2".to_owned(),
                key: hex_encode(&key_a_bytes),
                nonce: hex_encode(&nonce_a_bytes),
                plaintext_json: json_a2,
                envelope: Some(sealed_a2.as_base64url()),
                expect: "ok".to_owned(),
            },
            VectorCase {
                id: "b".to_owned(),
                key: hex_encode(&key_a_bytes),
                nonce: hex_encode(&nonce_a_bytes),
                plaintext_json: r#"{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":"test with 0x80: Ѐ."}"#.to_owned(),
                envelope: Some(sealed_b.as_base64url()),
                expect: "ok".to_owned(),
            },
            VectorCase {
                id: "c".to_owned(),
                key: hex_encode(&key_a_bytes),
                nonce: hex_encode(&nonce_a_bytes),
                plaintext_json: format!(r#"{{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":"{}"}}"#, notif_c.body),
                envelope: Some(sealed_c.as_base64url()),
                expect: "ok".to_owned(),
            },
            VectorCase {
                id: "d".to_owned(),
                key: hex_encode(&key_a_bytes),
                nonce: hex_encode(&nonce_a_bytes),
                plaintext_json: format!(r#"{{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":"{}"}}"#, notif_d.body),
                envelope: None,
                expect: "too_large".to_owned(),
            },
            VectorCase {
                id: "e".to_owned(),
                key: hex_encode(&key_a_bytes),
                nonce: hex_encode(&nonce_a_bytes),
                plaintext_json: r#"{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":"test notification from your journal."}"#.to_owned(),
                envelope: Some(URL_SAFE_NO_PAD.encode(&corrupted_e)),
                expect: "auth_fail".to_owned(),
            },
            VectorCase {
                id: "f".to_owned(),
                key: hex_encode(&key_a_bytes),
                nonce: hex_encode(&nonce_a_bytes),
                plaintext_json: r#"{"v":1,"at":"2026-09-24T00:00:00Z","kind":"test","title":"solstone","body":"test notification from your journal."}"#.to_owned(),
                envelope: Some(URL_SAFE_NO_PAD.encode(&bad_version_f)),
                expect: "bad_version".to_owned(),
            },
        ];

        let vector_file = VectorFile {
            _about: "Notification envelope test vectors. Generated by the envelope vector test in solstone-core-push; values are never hand-edited. Values for key and nonce are lowercase hex. Regenerate with: PUSH_ENVELOPE_VECTORS_WRITE=1 cargo test --manifest-path core/Cargo.toml -p solstone-core-push --lib --locked envelope_vectors -- --test-threads=1 --exact".to_owned(),
            vectors: vector_cases,
        };

        if std::env::var("PUSH_ENVELOPE_VECTORS_WRITE").as_deref() == Ok("1") {
            let json_str = serde_json::to_string_pretty(&vector_file).unwrap() + "\n";
            fs::write(&vectors_path, json_str).expect("write vectors file");
        } else {
            let existing_bytes = fs::read(&vectors_path).expect("read existing vectors file");
            let existing_file: VectorFile =
                serde_json::from_slice(&existing_bytes).expect("parse existing vectors file");
            assert_eq!(existing_file, vector_file, "vectors file drift detected");
        }
    }
}
