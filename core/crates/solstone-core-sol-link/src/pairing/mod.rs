// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The native owner of the durable pairing domain.

pub mod addresses;
pub mod attestation;
pub mod nonces;

use std::fmt;
use std::fs;
use std::net::Ipv4Addr;
use std::path::Path;

use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::ca::sign_csr;
use crate::committed::{CommittedIdentityError, load_committed_identity};
use crate::ledger::{AuthorizationLedger, AuthorizedClientsMutationError, ClientEntry, ClientRole};

use self::addresses::{
    AddressError, PairLinkEncodeError, PairingSnapshot, RawInterfaceSource, RouteIpv4Source,
    SystemInterfaceSource, SystemRouteIpv4Source, encode_configured_home_pair_link,
    encode_pair_link, resolve_pair_link_candidates, snapshot_from_sources,
};
use self::attestation::{AttestationError, mint_home_attestation};
use self::nonces::{NONCE_TTL_SECONDS, NonceStore, NonceStoreError};

/// Input accepted by the owner-only pair-start handler.
#[derive(Clone, Debug)]
pub struct MintRequest {
    pub device_label: String,
    pub role: String,
    /// `None` represents a non-boolean wire value and is rejected before any
    /// persistent read that could write later in this operation.
    pub same_machine: Option<bool>,
    pub hardened_loopback: bool,
    pub configured_home: Option<Ipv4Addr>,
}

/// Pair-start's exact success body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MintResponse {
    pub nonce: String,
    pub pair_link: String,
    pub expires_in: i64,
    pub device_label: String,
    pub ca_fingerprint: String,
}

/// Pairing-domain failures retain wire-facing reason and status for the route
/// adapter without coupling this domain to HTTP.
#[derive(Debug)]
pub enum PairingError {
    PairingRequestInvalid(&'static str),
    InvalidOperationForState(&'static str),
    OperationNoLongerAvailable,
    CommittedIdentityUnavailable(CommittedIdentityError),
    NonceStore(NonceStoreError),
    Address(AddressError),
    PairLink(PairLinkEncodeError),
    Certificate(crate::ca::CaError),
    Ledger(AuthorizedClientsMutationError),
    Attestation(AttestationError),
    Serialization(serde_json::Error),
    Clock,
    RelayPairingRegistrationRefused,
    RelayPairingUnavailable,
    RelayPairingRegistrationTimedOut,
    RelayPairingNonceCommit,
    RelayPairingTunnelInstanceMismatch,
}

impl PairingError {
    pub fn status(&self) -> u16 {
        match self {
            Self::OperationNoLongerAvailable => 410,
            Self::PairingRequestInvalid(_) | Self::InvalidOperationForState(_) => 400,
            Self::RelayPairingRegistrationRefused | Self::RelayPairingUnavailable => 503,
            Self::RelayPairingRegistrationTimedOut => 504,
            Self::RelayPairingTunnelInstanceMismatch => 502,
            Self::CommittedIdentityUnavailable(_)
            | Self::NonceStore(_)
            | Self::Address(_)
            | Self::PairLink(_)
            | Self::Certificate(_)
            | Self::Ledger(_)
            | Self::Attestation(_)
            | Self::Serialization(_)
            | Self::Clock
            | Self::RelayPairingNonceCommit => 500,
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::PairingRequestInvalid(_) => "pairing_request_invalid",
            Self::InvalidOperationForState(_) => "invalid_operation_for_state",
            Self::OperationNoLongerAvailable => "operation_no_longer_available",
            Self::CommittedIdentityUnavailable(_) => "committed_identity_unavailable",
            Self::RelayPairingRegistrationRefused => "relay_pairing_registration_refused",
            Self::RelayPairingUnavailable => "relay_pairing_unavailable",
            Self::RelayPairingRegistrationTimedOut => "relay_pairing_registration_timed_out",
            Self::RelayPairingTunnelInstanceMismatch => "relay_pairing_tunnel_instance_mismatch",
            Self::NonceStore(_)
            | Self::Address(_)
            | Self::PairLink(_)
            | Self::Certificate(_)
            | Self::Ledger(_)
            | Self::Attestation(_)
            | Self::Serialization(_)
            | Self::Clock
            | Self::RelayPairingNonceCommit => "internal_error",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::PairingRequestInvalid(detail) | Self::InvalidOperationForState(detail) => {
                Some(detail)
            }
            Self::OperationNoLongerAvailable
            | Self::CommittedIdentityUnavailable(_)
            | Self::NonceStore(_)
            | Self::Address(_)
            | Self::PairLink(_)
            | Self::Certificate(_)
            | Self::Ledger(_)
            | Self::Attestation(_)
            | Self::Serialization(_)
            | Self::Clock
            | Self::RelayPairingRegistrationRefused
            | Self::RelayPairingUnavailable
            | Self::RelayPairingRegistrationTimedOut
            | Self::RelayPairingNonceCommit
            | Self::RelayPairingTunnelInstanceMismatch => None,
        }
    }
}

impl fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PairingRequestInvalid(detail) | Self::InvalidOperationForState(detail) => {
                formatter.write_str(detail)
            }
            Self::OperationNoLongerAvailable => {
                formatter.write_str("pairing operation is no longer available")
            }
            Self::CommittedIdentityUnavailable(error) => {
                write!(formatter, "committed identity unavailable: {error}")
            }
            Self::NonceStore(error) => error.fmt(formatter),
            Self::Address(error) => error.fmt(formatter),
            Self::PairLink(error) => error.fmt(formatter),
            Self::Certificate(error) => error.fmt(formatter),
            Self::Ledger(error) => error.fmt(formatter),
            Self::Attestation(error) => error.fmt(formatter),
            Self::Serialization(error) => error.fmt(formatter),
            Self::Clock => formatter.write_str("pairing clock is outside the supported range"),
            Self::RelayPairingRegistrationRefused => {
                formatter.write_str("relay pairing registration refused")
            }
            Self::RelayPairingUnavailable => formatter.write_str("relay pairing unavailable"),
            Self::RelayPairingRegistrationTimedOut => {
                formatter.write_str("relay pairing registration timed out")
            }
            Self::RelayPairingNonceCommit => {
                formatter.write_str("relay pairing nonce commit failed")
            }
            Self::RelayPairingTunnelInstanceMismatch => {
                formatter.write_str("relay pairing tunnel instance mismatch")
            }
        }
    }
}

impl std::error::Error for PairingError {}

/// Mint using the production raw-interface and route-probe construction path.
pub fn mint_pairing(
    journal_root: &Path,
    request: &MintRequest,
    now: i64,
) -> Result<MintResponse, PairingError> {
    if request.same_machine == Some(true) || request.configured_home.is_some() {
        return mint_pairing_from_snapshot(journal_root, request, now, &PairingSnapshot::default());
    }
    mint_pairing_from_sources(
        journal_root,
        request,
        now,
        &SystemInterfaceSource,
        &SystemRouteIpv4Source,
    )
}

/// Mint through level-B producers. Tests inject raw interface data and route
/// results so the production classifier and resolver are both exercised.
pub fn mint_pairing_from_sources(
    journal_root: &Path,
    request: &MintRequest,
    now: i64,
    interfaces: &impl RawInterfaceSource,
    route: &impl RouteIpv4Source,
) -> Result<MintResponse, PairingError> {
    if request.same_machine == Some(true) {
        return mint_pairing_from_snapshot(journal_root, request, now, &PairingSnapshot::default());
    }
    let snapshot = snapshot_from_sources(interfaces, route).map_err(PairingError::Address)?;
    mint_pairing_from_snapshot(journal_root, request, now, &snapshot)
}

/// Mint through level-A's explicit snapshot seam.
pub fn mint_pairing_from_snapshot(
    journal_root: &Path,
    request: &MintRequest,
    now: i64,
    snapshot: &PairingSnapshot,
) -> Result<MintResponse, PairingError> {
    validate_mint_request(request)?;
    let same_machine = request.same_machine.expect("validated above");
    if same_machine && !request.hardened_loopback {
        return Err(PairingError::PairingRequestInvalid(
            "same-machine pairing requires a hardened loopback request",
        ));
    }
    if is_spl_posture(journal_root) && !same_machine {
        return Err(PairingError::InvalidOperationForState(
            "This journal cannot pair this device directly, and relay pairing is not available on this build.",
        ));
    }

    // Read only: loading this identity never creates or rewrites CA material.
    let identity = load_committed_identity(journal_root)
        .map_err(PairingError::CommittedIdentityUnavailable)?;
    let ca_fingerprint = spl_core::ca::sha256_hex(identity.certificate_der());
    let digest = sha256_bytes(identity.certificate_der());
    let mut ca_fp_prefix = [0_u8; 16];
    ca_fp_prefix.copy_from_slice(&digest[..16]);
    let nonce = random_nonce()?;
    let nonce_bytes = nonce_bytes(&nonce)?;
    let pair_link = if same_machine {
        // Same-host pairing bypasses configured-home and all discovery, exactly
        // as the reference's loopback branch does.
        encode_configured_home_pair_link(Ipv4Addr::LOCALHOST, nonce_bytes, ca_fp_prefix)
    } else {
        match request.configured_home {
            Some(home) => encode_configured_home_pair_link(home, nonce_bytes, ca_fp_prefix),
            None => {
                let candidates =
                    resolve_pair_link_candidates(&snapshot.endpoints, snapshot.route_ipv4);
                encode_pair_link(&candidates, nonce_bytes, ca_fp_prefix).map_err(|error| {
                    match error {
                        PairLinkEncodeError::CandidateCount(_)
                        | PairLinkEncodeError::DisallowedAddress => {
                            PairingError::PairingRequestInvalid(
                                "no usable local address is available for pairing",
                            )
                        }
                        error @ PairLinkEncodeError::RelayOriginLength(_) => {
                            PairingError::PairLink(error)
                        }
                    }
                })?
            }
        }
    };
    // The nonce is persisted last: every refusal above writes nothing.
    NonceStore::new(journal_root)
        .add(
            nonce.clone(),
            request.device_label.clone(),
            request.role.clone(),
            same_machine,
            now,
        )
        .map_err(PairingError::NonceStore)?;
    Ok(MintResponse {
        nonce,
        pair_link,
        expires_in: NONCE_TTL_SECONDS,
        device_label: request.device_label.clone(),
        ca_fingerprint,
    })
}

/// Ceremony input keeps the canonical SPL request type intact.
pub struct CeremonyRequest<'a> {
    pub request: &'a spl_core::PairRequest,
    pub nonce: &'a str,
    pub sender_instance_id: Option<&'a str>,
    pub local_endpoints: Option<Value>,
}

/// Complete a one-shot pairing ceremony and return SPL's canonical response.
pub fn complete_pairing(
    journal_root: &Path,
    ceremony: CeremonyRequest<'_>,
    now: i64,
) -> Result<spl_core::PairResponse, PairingError> {
    if ceremony
        .sender_instance_id
        .is_some_and(|value| !valid_sender_instance_id(value))
    {
        return Err(PairingError::PairingRequestInvalid(
            "sender_instance_id is invalid",
        ));
    }
    // Read only, and deliberately before consume: unavailable identity cannot burn a nonce.
    let identity = load_committed_identity(journal_root)
        .map_err(PairingError::CommittedIdentityUnavailable)?;
    let entry = NonceStore::new(journal_root)
        .consume(ceremony.nonce, now)
        .map_err(PairingError::NonceStore)?
        .ok_or(PairingError::OperationNoLongerAvailable)?;
    if entry.role == "peer" {
        // Deliberate divergence from the Python peer branch: native direct peer pairing is unsupported.
        return Err(PairingError::PairingRequestInvalid(
            "peer pairing is not available on this build",
        ));
    }
    let issued = sign_csr(
        identity.ca(),
        &ceremony.request.csr,
        &ceremony.request.device_label,
    )
    .map_err(PairingError::Certificate)?;
    let paired_at = OffsetDateTime::from_unix_timestamp(now)
        .map_err(|_| PairingError::Clock)?
        .format(&Rfc3339)
        .map_err(|_| PairingError::Clock)?;
    let mut ledger = AuthorizationLedger::new(journal_root);
    let mut ledger_entry = ClientEntry::new(
        issued.did(),
        &ceremony.request.device_label,
        paired_at,
        identity.instance_id(),
        ClientRole::from_wire(Some(&entry.role)),
    );
    if entry.same_machine {
        ledger_entry.network = Some("home".to_owned());
    }
    ledger.add(ledger_entry).map_err(PairingError::Ledger)?;
    let attestation =
        mint_home_attestation(identity.ca(), identity.instance_id(), issued.did(), now)
            .map_err(PairingError::Attestation)?;
    Ok(spl_core::PairResponse {
        client_cert: issued.pem().to_owned(),
        ca_chain: vec![
            String::from_utf8(identity.certificate_pem().to_vec())
                .expect("committed identity PEM is UTF-8"),
        ],
        instance_id: identity.instance_id().to_owned(),
        home_label: identity.home_label().to_owned(),
        fingerprint: issued.did().to_owned(),
        home_attestation: Some(attestation),
        local_endpoints: ceremony.local_endpoints,
    })
}

/// Serialize the canonical response while applying the protocol's absent,
/// rather than null, empty-endpoint rule. `PairResponse` cannot express it
/// because its canonical optional field has no `skip_serializing_if`.
pub fn pair_response_json(response: &spl_core::PairResponse) -> Result<Value, PairingError> {
    let mut value = serde_json::to_value(response).map_err(PairingError::Serialization)?;
    if response.local_endpoints.is_none() {
        value
            .as_object_mut()
            .expect("PairResponse serializes as an object")
            .remove("local_endpoints");
    }
    Ok(value)
}

fn validate_mint_request(request: &MintRequest) -> Result<(), PairingError> {
    if !matches!(request.role.as_str(), "" | "phone" | "observer" | "peer") {
        return Err(PairingError::PairingRequestInvalid("role is invalid"));
    }
    if request.same_machine.is_none() {
        return Err(PairingError::PairingRequestInvalid(
            "same_machine must be a boolean",
        ));
    }
    Ok(())
}

fn is_spl_posture(journal_root: &Path) -> bool {
    fs::read(journal_root.join("config").join("journal.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("link")?
                .get("posture")?
                .as_str()
                .map(str::to_owned)
        })
        .as_deref()
        == Some("spl")
}

fn random_nonce() -> Result<String, PairingError> {
    let mut bytes = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| PairingError::PairingRequestInvalid("system randomness unavailable"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn nonce_bytes(nonce: &str) -> Result<[u8; 16], PairingError> {
    let mut bytes = [0_u8; 16];
    if nonce.len() != 32 {
        return Err(PairingError::PairingRequestInvalid(
            "nonce encoding is invalid",
        ));
    }
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&nonce[index * 2..index * 2 + 2], 16)
            .map_err(|_| PairingError::PairingRequestInvalid("nonce encoding is invalid"))?;
    }
    Ok(bytes)
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

fn valid_sender_instance_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;
    use x509_parser::pem::parse_x509_pem;

    use super::*;
    use crate::ca::{generate_ca, jid_from_spki};
    use crate::pairing::addresses::{EndpointScope, LocalEndpoint, RawInterfaceAddress};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("solstone-pairing-{nanos}-{sequence}"));
            fs::create_dir(&path).expect("temporary journal creates");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn link_tree(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        fn walk(root: &Path, directory: &Path, output: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
            let Ok(entries) = fs::read_dir(directory) else {
                return;
            };
            for entry in entries {
                let path = entry.expect("link entry").path();
                if path.is_dir() {
                    walk(root, &path, output);
                } else {
                    output.push((
                        path.strip_prefix(root)
                            .expect("relative link path")
                            .to_path_buf(),
                        fs::read(&path).expect("link bytes"),
                    ));
                }
            }
        }
        let mut output = Vec::new();
        walk(root, &root.join("link"), &mut output);
        output.sort_by(|left, right| left.0.cmp(&right.0));
        output
    }

    fn request() -> MintRequest {
        MintRequest {
            device_label: "phone".into(),
            role: "phone".into(),
            same_machine: Some(false),
            hardened_loopback: false,
            configured_home: None,
        }
    }

    fn identity(root: &Path) -> String {
        let ca = generate_ca().expect("CA");
        fs::create_dir_all(root.join("link/ca")).expect("ca directory");
        fs::write(root.join("link/ca/cert.pem"), ca.certificate_pem()).expect("certificate");
        fs::write(root.join("link/ca/private.pem"), ca.private_key_pem()).expect("key");
        let instance_id = jid_from_spki(ca.spki_der()).expect("jid");
        fs::write(
            root.join("link/state.json"),
            json!({"instance_id": instance_id, "home_label":"Home"}).to_string(),
        )
        .expect("state");
        instance_id
    }

    fn pair_request() -> spl_core::PairRequest {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("key");
        let params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("params");
        spl_core::PairRequest {
            csr: params
                .serialize_request(&key)
                .expect("csr")
                .pem()
                .expect("csr PEM"),
            device_label: "phone".into(),
            additional_fields: serde_json::Map::new(),
        }
    }

    #[test]
    fn relay_pairing_errors_are_stable_and_secret_free() {
        let cases = [
            (
                PairingError::RelayPairingRegistrationRefused,
                503,
                "relay_pairing_registration_refused",
            ),
            (
                PairingError::RelayPairingUnavailable,
                503,
                "relay_pairing_unavailable",
            ),
            (
                PairingError::RelayPairingRegistrationTimedOut,
                504,
                "relay_pairing_registration_timed_out",
            ),
            (PairingError::RelayPairingNonceCommit, 500, "internal_error"),
            (
                PairingError::RelayPairingTunnelInstanceMismatch,
                502,
                "relay_pairing_tunnel_instance_mismatch",
            ),
        ];
        let placeholder_secret = "S=0123456789abcdef RK=e34481a4cde647ba9c9fb29a59e18271 https://relay.example/?token=secret";

        for (error, status, reason) in cases {
            assert_eq!(error.status(), status);
            assert_eq!(error.reason(), reason);
            assert_eq!(error.detail(), None);
            assert!(!format!("{error}").contains(placeholder_secret));
            assert!(!format!("{error:?}").contains(placeholder_secret));
        }
    }

    #[test]
    fn mint_validates_before_persistence_and_preserves_ca_bytes() {
        let temporary = TempDir::new();
        identity(temporary.path());
        let ca_before = fs::read(temporary.path().join("link/ca/cert.pem")).expect("ca");
        let mut invalid = request();
        invalid.role = "bad".into();
        assert!(matches!(
            mint_pairing_from_snapshot(temporary.path(), &invalid, 1, &PairingSnapshot::default()),
            Err(PairingError::PairingRequestInvalid(_))
        ));
        assert!(!temporary.path().join("link/nonces.json").exists());
        assert!(
            !fs::read_dir(temporary.path().join("link"))
                .expect("link directory")
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with(".tmp_")),
            "failed mint leaves no temporary artifact"
        );
        let snapshot = PairingSnapshot {
            endpoints: vec![LocalEndpoint {
                ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                scope: EndpointScope::Lan,
            }],
            route_ipv4: None,
        };
        let minted =
            mint_pairing_from_snapshot(temporary.path(), &request(), 1, &snapshot).expect("mint");
        assert_eq!(minted.expires_in, 300);
        assert_eq!(
            ca_before,
            fs::read(temporary.path().join("link/ca/cert.pem")).expect("same ca")
        );
        let (_, pem) = parse_x509_pem(&ca_before).expect("ca PEM");
        assert_eq!(
            minted.ca_fingerprint,
            spl_core::ca::sha256_hex(&pem.contents)
        );
        assert_eq!(minted.device_label, "phone");
    }

    #[test]
    fn committed_identity_refusals_preserve_link_bytes_and_present_identity_does_too() {
        let absent = TempDir::new();
        let absent_before = link_tree(absent.path());
        assert!(matches!(
            mint_pairing_from_snapshot(
                absent.path(),
                &request(),
                1,
                &PairingSnapshot {
                    endpoints: vec![LocalEndpoint {
                        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                        scope: EndpointScope::Lan
                    }],
                    route_ipv4: None
                },
            ),
            Err(PairingError::CommittedIdentityUnavailable(_))
        ));
        assert_eq!(
            link_tree(absent.path()),
            absent_before,
            "absent identity writes nothing"
        );

        let unreadable = TempDir::new();
        identity(unreadable.path());
        fs::remove_file(unreadable.path().join("link/ca/private.pem")).expect("remove key");
        let unreadable_before = link_tree(unreadable.path());
        assert!(matches!(
            mint_pairing_from_snapshot(
                unreadable.path(),
                &request(),
                1,
                &PairingSnapshot {
                    endpoints: vec![LocalEndpoint {
                        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                        scope: EndpointScope::Lan
                    }],
                    route_ipv4: None
                },
            ),
            Err(PairingError::CommittedIdentityUnavailable(_))
        ));
        assert_eq!(
            link_tree(unreadable.path()),
            unreadable_before,
            "unreadable identity writes nothing"
        );

        let present = TempDir::new();
        identity(present.path());
        let present_before = link_tree(present.path())
            .into_iter()
            .filter(|(path, _)| path.starts_with("link/ca"))
            .collect::<Vec<_>>();
        assert!(
            mint_pairing_from_snapshot(
                present.path(),
                &request(),
                1,
                &PairingSnapshot {
                    endpoints: vec![LocalEndpoint {
                        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                        scope: EndpointScope::Lan
                    }],
                    route_ipv4: None
                },
            )
            .is_ok()
        );
        let present_after = link_tree(present.path())
            .into_iter()
            .filter(|(path, _)| path.starts_with("link/ca"))
            .collect::<Vec<_>>();
        assert_eq!(
            present_after, present_before,
            "present identity's CA is byte-identical"
        );
    }

    #[test]
    fn same_machine_mint_bypasses_configured_home_with_a_loopback_v04_link() {
        let temporary = TempDir::new();
        identity(temporary.path());
        let request = MintRequest {
            same_machine: Some(true),
            hardened_loopback: true,
            configured_home: Some(Ipv4Addr::new(192, 168, 1, 7)),
            ..request()
        };

        let minted =
            mint_pairing_from_snapshot(temporary.path(), &request, 1, &PairingSnapshot::default())
                .expect("same-machine mint");
        let blob = spl_core::crockford::decode(
            minted
                .pair_link
                .split('#')
                .nth(1)
                .expect("pair-link fragment"),
        )
        .expect("pair-link bytes");
        assert_eq!(blob[0], 0x04);
        let spl_core::pairlink::ParsedPairLink::Direct(link) =
            spl_core::pairlink::parse(&minted.pair_link).expect("pair-link parses")
        else {
            panic!("same-machine mint emits a direct link");
        };
        assert_eq!(link.candidates.len(), 1);
        assert_eq!(link.candidates[0].host, Ipv4Addr::LOCALHOST.to_string());
        assert_eq!(link.candidates[0].port, spl_core::DEFAULT_DIRECT_PORT);
    }

    #[test]
    fn direct_posture_refusal_and_empty_route_positive_twin() {
        let temporary = TempDir::new();
        identity(temporary.path());
        fs::create_dir_all(temporary.path().join("config")).expect("config");
        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"link":{"posture":"spl"}}"#,
        )
        .expect("posture");
        let refusal = mint_pairing_from_snapshot(
            temporary.path(),
            &request(),
            1,
            &PairingSnapshot::default(),
        )
        .expect_err("SPL posture refuses direct pairing");
        assert!(matches!(refusal, PairingError::InvalidOperationForState(_)));
        assert_eq!(refusal.status(), 400);
        assert_eq!(refusal.reason(), "invalid_operation_for_state");
        fs::write(
            temporary.path().join("config/journal.json"),
            r#"{"link":{"posture":"direct"}}"#,
        )
        .expect("posture");
        let snapshot = PairingSnapshot {
            endpoints: vec![],
            route_ipv4: Some(Ipv4Addr::new(10, 0, 0, 2)),
        };
        assert!(mint_pairing_from_snapshot(temporary.path(), &request(), 1, &snapshot).is_ok());
    }

    struct Raw(Vec<RawInterfaceAddress>);
    impl RawInterfaceSource for Raw {
        fn enumerate(&self) -> Result<Vec<RawInterfaceAddress>, AddressError> {
            Ok(self.0.clone())
        }
    }
    struct Route(Option<Ipv4Addr>);
    impl RouteIpv4Source for Route {
        fn route_ipv4(&self) -> Option<Ipv4Addr> {
            self.0
        }
    }

    #[test]
    fn production_source_path_uses_the_real_classifier_and_route_probe_seam() {
        let temporary = TempDir::new();
        identity(temporary.path());
        let raw = Raw(vec![]);
        let response = mint_pairing_from_sources(
            temporary.path(),
            &request(),
            1,
            &raw,
            &Route(Some(Ipv4Addr::new(10, 0, 0, 2))),
        )
        .expect("mint");
        let link = spl_core::pairlink::parse(&response.pair_link).expect("link parses");
        let spl_core::pairlink::ParsedPairLink::Direct(link) = link else {
            panic!("production path emits direct link");
        };
        let blob =
            spl_core::crockford::decode(response.pair_link.split('#').nth(1).expect("fragment"))
                .expect("blob");
        assert_eq!(blob[0], 0x04);
        assert_eq!(link.candidates[0].host, "10.0.0.2");
    }

    #[test]
    fn snapshot_mint_filters_invalid_addresses_and_keeps_only_allowed_ipv4_candidates() {
        let temporary = TempDir::new();
        identity(temporary.path());
        let response = mint_pairing_from_snapshot(
            temporary.path(),
            &request(),
            1,
            &PairingSnapshot {
                endpoints: vec![
                    LocalEndpoint {
                        ip: IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
                        scope: EndpointScope::Lan,
                    },
                    LocalEndpoint {
                        ip: IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1)),
                        scope: EndpointScope::Lan,
                    },
                    LocalEndpoint {
                        ip: "fd00::2".parse().expect("ULA"),
                        scope: EndpointScope::Ula,
                    },
                    LocalEndpoint {
                        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                        scope: EndpointScope::Lan,
                    },
                ],
                route_ipv4: Some(Ipv4Addr::new(192, 168, 1, 9)),
            },
        )
        .expect("allowed candidate mints");
        let spl_core::pairlink::ParsedPairLink::Direct(link) =
            spl_core::pairlink::parse(&response.pair_link).expect("public parser")
        else {
            panic!("direct link")
        };
        assert_eq!(
            link.candidates
                .iter()
                .map(|candidate| candidate.host.as_str())
                .collect::<Vec<_>>(),
            ["10.0.0.2"]
        );
    }

    #[test]
    fn response_json_omits_empty_endpoints_but_preserves_present_endpoints() {
        let response = spl_core::PairResponse {
            client_cert: "cert".into(),
            ca_chain: vec!["ca".into()],
            instance_id: "id".into(),
            home_label: "home".into(),
            fingerprint: "sha256:fp".into(),
            home_attestation: None,
            local_endpoints: None,
        };
        assert!(
            pair_response_json(&response)
                .expect("json")
                .get("local_endpoints")
                .is_none()
        );
        let response = spl_core::PairResponse {
            local_endpoints: Some(json!([])),
            ..response
        };
        assert_eq!(
            pair_response_json(&response).expect("json")["local_endpoints"],
            json!([])
        );
    }

    #[test]
    fn ceremony_sender_instance_id_is_optional_and_malformed_sender_preserves_nonce() {
        let temporary = TempDir::new();
        identity(temporary.path());
        let store = NonceStore::new(temporary.path());
        store
            .add("absent".into(), "phone".into(), "phone".into(), false, 1)
            .expect("absent nonce");
        store
            .add("valid".into(), "phone".into(), "phone".into(), false, 1)
            .expect("valid nonce");
        let request = pair_request();
        assert!(
            complete_pairing(
                temporary.path(),
                CeremonyRequest {
                    request: &request,
                    nonce: "absent",
                    sender_instance_id: None,
                    local_endpoints: None,
                },
                2,
            )
            .is_ok()
        );
        let invalid = complete_pairing(
            temporary.path(),
            CeremonyRequest {
                request: &request,
                nonce: "valid",
                sender_instance_id: Some("!bad"),
                local_endpoints: None,
            },
            2,
        );
        let malformed_detail = invalid
            .expect_err("malformed sender refuses")
            .detail()
            .expect("malformed sender detail")
            .to_owned();
        assert_eq!(malformed_detail, "sender_instance_id is invalid");
        assert!(!store.peek("valid").expect("preserved nonce").used);
        let ca_bytes = fs::read(temporary.path().join("link/ca/cert.pem")).expect("CA bytes");
        let response = complete_pairing(
            temporary.path(),
            CeremonyRequest {
                request: &request,
                nonce: "valid",
                sender_instance_id: Some("sender-1"),
                local_endpoints: None,
            },
            2,
        )
        .expect("complete pairing");
        assert_eq!(
            response.ca_chain,
            vec![String::from_utf8(ca_bytes).expect("CA PEM")]
        );
        assert!(response.fingerprint.starts_with("sha256:"));
        assert!(store.peek("valid").expect("burned nonce").used);
        assert_eq!(
            complete_pairing(
                temporary.path(),
                CeremonyRequest {
                    request: &request,
                    nonce: "valid",
                    sender_instance_id: Some("sender-1"),
                    local_endpoints: None,
                },
                2,
            )
            .expect_err("reused nonce refused")
            .status(),
            410
        );
        store
            .add("peer".into(), "phone".into(), "peer".into(), false, 1)
            .expect("peer nonce");
        let peer = complete_pairing(
            temporary.path(),
            CeremonyRequest {
                request: &request,
                nonce: "peer",
                sender_instance_id: Some("sender-2"),
                local_endpoints: None,
            },
            2,
        );
        let peer_detail = peer
            .expect_err("peer refuses")
            .detail()
            .expect("peer detail")
            .to_owned();
        assert_ne!(peer_detail, malformed_detail);
        assert!(store.peek("peer").expect("burned peer nonce").used);
    }

    #[test]
    fn ceremony_ledger_entry_uses_the_home_instance_not_sender_instance_id() {
        let temporary = TempDir::new();
        let expected_instance_id = identity(temporary.path());
        let store = NonceStore::new(temporary.path());
        store
            .add("ledger".into(), "phone".into(), "phone".into(), false, 1)
            .expect("nonce");
        let request = pair_request();
        let response = complete_pairing(
            temporary.path(),
            CeremonyRequest {
                request: &request,
                nonce: "ledger",
                sender_instance_id: Some("a-different-valid-sender"),
                local_endpoints: None,
            },
            2,
        )
        .expect("ceremony");
        let entry = AuthorizationLedger::new(temporary.path())
            .get(&response.fingerprint)
            .expect("ledger entry");
        assert_eq!(entry.instance_id, expected_instance_id);
        assert_ne!(entry.instance_id, "a-different-valid-sender");
    }
}
