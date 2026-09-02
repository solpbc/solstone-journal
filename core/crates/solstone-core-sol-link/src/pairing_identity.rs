// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pairer-supplied pairing identity, distinct from authorization.
//!
//! # Production inventory (pairing identity)
//!
//! Pairer-supplied `client_label` / `platform` never grant device-door access.
//! A certificate is authorized only by a unique fingerprint in
//! `link/authorized_clients.json`. Direct and relay carriers are equivalent:
//! the same `additional_fields` produce the same ledger identity.
//!
//! ## Direct / relay pairing handlers
//!
//! - `solstone-core-convey-shell::network::pair` + `PairingAdmission::Direct`
//!   — HTTP POST `/app/network/pair` (aliased `/app/link/pair`). Injects the
//!   owner journal into `complete_pairing`. Ceremony identity is validated
//!   before nonce consume. Absent/valid fields are accepted; empty, wrong-type,
//!   oversize (`client_label` > 253 UTF-8 bytes), and unknown `platform`
//!   vocabulary are refused `400 pairing_request_invalid`. Exact-name lookalikes
//!   and unrelated opaque keys are ignored as extensions.
//! - `solstone-core-convey-shell::network::pair` + `PairingAdmission::Relay`
//!   — same handler, same body, same identity rules. Relay only adds the
//!   nonce-matches-carrier check; it does not change ledger identity.
//! - `solstone-core-sol-link::pairing::complete_pairing` — carrier-blind
//!   ceremony. Applies validated identity onto `ClientEntry` (`client_label`
//!   only when `Some`; `platform` as `Option`). Peer role is refused after
//!   consume with `peer pairing is not available on this build` and never
//!   writes a row.
//! - Client joiners (`pairing_entry::{direct,relay}`, native `solstone link`)
//!   — send `PairRequest.additional_fields` as supplied. The shipped joiner
//!   currently sends an empty map; they are not owner-side writers.
//!
//! ## Authorization-ledger mutations
//!
//! - `AuthorizationLedger::add` — first-write-wins: on fingerprint match, copies
//!   the existing `client_label` and `platform` onto the incoming row and does
//!   not upsert over them. New rows persist ceremony-accepted values.
//! - `update_label` — mutates `device_label` / `label_ordinal` only; pairing
//!   identity fields are left intact.
//! - `backfill_label_ordinals` — ordinals only.
//! - `remove` — deletes the whole row, including pairing identity.
//! - `touch_last_seen`, `touch_last_seen_at`, `record_accepted_ingest`,
//!   `record_ingest_rejection` — `devices.json` activity only; they do not
//!   rewrite `authorized_clients.json` identity fields.
//!
//! ## Readers / serializers
//!
//! Dispositions below are for **legacy ledger JSON** (absent, present-empty,
//! valid, malformed). Ceremony wire rules are stricter and are listed separately.
//!
//! - `PairingIdentityFields::from_object` (raw object, fail-closed read-back):
//!   - `client_label`: absent → `Absent`; present `""` → `Empty`; string of
//!     1..=253 UTF-8 bytes → `Valid`; longer string → `Unprojectable`;
//!     non-string → `Malformed`.
//!   - `platform`: absent → `Absent`; closed vocab → `Valid`; empty, unknown,
//!     or non-string → `Malformed`. There is no present-empty platform state.
//!   - `projection()`: either member `Malformed` → combined `Unavailable`;
//!     otherwise `Available` (empty and unprojectable labels stay available).
//! - Ledger parse into `ClientEntry` (lossy, for devices/clients/inspection):
//!   - `client_label`: absent / non-string → `""`; present-empty → `""`; any
//!     string (including unprojectable) kept as-is.
//!   - `platform`: absent / empty / unknown / non-string → `None`; closed vocab
//!     → `Some(Platform)`.
//! - `client_to_json`: omits empty `client_label`; emits `platform` iff `Some`.
//!   Present-empty and absent therefore serialize identically (omitted).
//! - `network::network_device_json` / `clients::client_json`: emit
//!   `ClientEntry.client_label` as a string (empty when the parse collapsed
//!   absent/malformed); do **not** emit `platform`.
//! - `ClientInspection` / doctor / home: carry `ClientEntry` and inherit that
//!   lossy parse. They 503 on `DuplicateCid` rather than projecting identity.
//!
//! ## Ceremony wire (validate-before-consume)
//!
//! `validate_ceremony_pairing_identity` on `additional_fields`:
//! absent → accept (`None`); valid `client_label` 1..=253 bytes → accept;
//! valid `platform` vocab → accept; empty / wrong-type / oversize /
//! unknown-vocab → refuse, nonce preserved. Runs for every nonce role,
//! including peer.

use serde_json::{Map, Value};

const MAX_CLIENT_LABEL_BYTES: usize = 253;

/// Closed pairer-supplied platform vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Linux,
    Macos,
    Windows,
    Ios,
    Android,
}

impl Platform {
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "linux" => Some(Self::Linux),
            "macos" => Some(Self::Macos),
            "windows" => Some(Self::Windows),
            "ios" => Some(Self::Ios),
            "android" => Some(Self::Android),
            _ => None,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }
}

/// Read-back state of a ledger `client_label` field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientLabelState {
    Absent,
    Empty,
    Valid(String),
    Unprojectable(String),
    Malformed,
}

/// Read-back state of a ledger `platform` field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformState {
    Absent,
    Valid(Platform),
    Malformed,
}

/// Per-field pairing identity parsed from a raw authorized-client object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingIdentityFields {
    pub client_label: ClientLabelState,
    pub platform: PlatformState,
}

impl PairingIdentityFields {
    pub fn from_object(item: &Map<String, Value>) -> Self {
        Self {
            client_label: client_label_state(item),
            platform: platform_state(item),
        }
    }

    pub fn projection(&self) -> PairingIdentity {
        if matches!(self.client_label, ClientLabelState::Malformed)
            || matches!(self.platform, PlatformState::Malformed)
        {
            PairingIdentity::Unavailable
        } else {
            PairingIdentity::Available {
                client_label: self.client_label.clone(),
                platform: self.platform,
            }
        }
    }
}

/// Combined pairing-identity view. One malformed member makes the tuple unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairingIdentity {
    Available {
        client_label: ClientLabelState,
        platform: PlatformState,
    },
    Unavailable,
}

/// Ceremony-accepted pairing identity: valid-or-absent only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CeremonyPairingIdentity {
    pub client_label: Option<String>,
    pub platform: Option<Platform>,
}

/// Validate pairer-supplied wire fields before nonce consumption.
pub fn validate_ceremony_pairing_identity(
    additional_fields: &Map<String, Value>,
) -> Result<CeremonyPairingIdentity, &'static str> {
    let client_label = match additional_fields.get("client_label") {
        None => None,
        Some(Value::String(value)) if (1..=MAX_CLIENT_LABEL_BYTES).contains(&value.len()) => {
            Some(value.clone())
        }
        Some(_) => return Err("client_label is invalid"),
    };
    let platform = match additional_fields.get("platform") {
        None => None,
        Some(Value::String(value)) => {
            Some(Platform::from_wire(value).ok_or("platform is invalid")?)
        }
        Some(_) => return Err("platform is invalid"),
    };
    Ok(CeremonyPairingIdentity {
        client_label,
        platform,
    })
}

fn client_label_state(item: &Map<String, Value>) -> ClientLabelState {
    match item.get("client_label") {
        None => ClientLabelState::Absent,
        Some(Value::String(value)) if value.is_empty() => ClientLabelState::Empty,
        Some(Value::String(value)) if value.len() <= MAX_CLIENT_LABEL_BYTES => {
            ClientLabelState::Valid(value.clone())
        }
        Some(Value::String(value)) => ClientLabelState::Unprojectable(value.clone()),
        Some(_) => ClientLabelState::Malformed,
    }
}

fn platform_state(item: &Map<String, Value>) -> PlatformState {
    match item.get("platform") {
        None => PlatformState::Absent,
        Some(Value::String(value)) => match Platform::from_wire(value) {
            Some(platform) => PlatformState::Valid(platform),
            None => PlatformState::Malformed,
        },
        Some(_) => PlatformState::Malformed,
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn from_object_distinguishes_client_label_presence() {
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({}))).client_label,
            ClientLabelState::Absent
        );
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({"client_label": ""}))).client_label,
            ClientLabelState::Empty
        );
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({"client_label": "phone"})))
                .client_label,
            ClientLabelState::Valid("phone".to_owned())
        );
        let long = "é".repeat(127);
        assert_eq!(long.len(), 254);
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({"client_label": long.clone()})))
                .client_label,
            ClientLabelState::Unprojectable(long)
        );
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({"client_label": 1}))).client_label,
            ClientLabelState::Malformed
        );
    }

    #[test]
    fn from_object_distinguishes_platform_presence() {
        let fields = PairingIdentityFields::from_object(&object(json!({})));
        assert_eq!(fields.platform, PlatformState::Absent);
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({"platform": "linux"}))).platform,
            PlatformState::Valid(Platform::Linux)
        );
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({"platform": ""}))).platform,
            PlatformState::Malformed
        );
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({"platform": "plan9"}))).platform,
            PlatformState::Malformed
        );
        assert_eq!(
            PairingIdentityFields::from_object(&object(json!({"platform": true}))).platform,
            PlatformState::Malformed
        );
    }

    #[test]
    fn unprojectable_client_label_with_valid_platform_stays_available() {
        let long = "a".repeat(254);
        let fields = PairingIdentityFields::from_object(&object(json!({
            "client_label": long,
            "platform": "ios",
        })));
        assert_eq!(
            fields.client_label,
            ClientLabelState::Unprojectable("a".repeat(254))
        );
        assert_eq!(
            fields.projection(),
            PairingIdentity::Available {
                client_label: ClientLabelState::Unprojectable("a".repeat(254)),
                platform: PlatformState::Valid(Platform::Ios),
            }
        );
    }

    #[test]
    fn present_empty_client_label_with_valid_platform_stays_available() {
        let fields = PairingIdentityFields::from_object(&object(json!({
            "client_label": "",
            "platform": "macos",
        })));
        assert_eq!(fields.client_label, ClientLabelState::Empty);
        assert_eq!(
            fields.projection(),
            PairingIdentity::Available {
                client_label: ClientLabelState::Empty,
                platform: PlatformState::Valid(Platform::Macos),
            }
        );
    }

    #[test]
    fn a_malformed_member_makes_the_tuple_unavailable() {
        let malformed_label = PairingIdentityFields::from_object(&object(json!({
            "client_label": ["x"],
            "platform": "android",
        })));
        assert_eq!(malformed_label.projection(), PairingIdentity::Unavailable);
        let malformed_platform = PairingIdentityFields::from_object(&object(json!({
            "client_label": "ok",
            "platform": "plan9",
        })));
        assert_eq!(
            malformed_platform.projection(),
            PairingIdentity::Unavailable
        );
    }

    #[test]
    fn ceremony_validation_accepts_presence_combinations_and_byte_length_bound() {
        let accepted = "é".repeat(126) + "a";
        assert_eq!(accepted.len(), 253);
        let identity = validate_ceremony_pairing_identity(&object(json!({
            "client_label": accepted,
            "platform": "windows",
        })))
        .expect("valid");
        assert_eq!(identity.client_label.as_deref().map(str::len), Some(253));
        assert_eq!(identity.platform, Some(Platform::Windows));
        assert!(validate_ceremony_pairing_identity(&Map::new()).is_ok());
        assert!(
            validate_ceremony_pairing_identity(&object(json!({"client_label": "lab"}))).is_ok()
        );
        assert!(validate_ceremony_pairing_identity(&object(json!({"platform": "linux"}))).is_ok());
    }

    #[test]
    fn ceremony_validation_refuses_invalid_type_empty_oversize_and_unknown_vocab() {
        let oversize = "é".repeat(127);
        assert_eq!(oversize.len(), 254);
        for (fields, detail) in [
            (json!({"client_label": 1}), "client_label is invalid"),
            (json!({"client_label": ""}), "client_label is invalid"),
            (json!({"client_label": oversize}), "client_label is invalid"),
            (json!({"platform": ""}), "platform is invalid"),
            (json!({"platform": "plan9"}), "platform is invalid"),
            (json!({"platform": false}), "platform is invalid"),
            (
                json!({"client_label": "", "platform": "plan9"}),
                "client_label is invalid",
            ),
        ] {
            assert_eq!(
                validate_ceremony_pairing_identity(&object(fields)),
                Err(detail)
            );
        }
    }
}
