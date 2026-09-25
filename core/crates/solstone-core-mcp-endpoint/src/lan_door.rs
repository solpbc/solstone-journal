// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct LAN MCP door service.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use futures::FutureExt;
use nix::libc;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_config::{
    LocalDoorConfig, MCP_LAN_DOOR_PORT, lan_door_config, read_journal_config,
};
use solstone_core_journal_io::{AtomicWriteOptions, JsonWriteOptions, atomic_replace, write_json};
use solstone_core_sol_link::pairing::addresses::{
    EndpointScope, LocalEndpoint, RawInterfaceSource, SystemInterfaceSource,
    classify_interface_addresses,
};
use solstone_core_system::lifecycle::{
    HostedServiceParentRuntime, HostedServiceShutdownEvidence, ParentLossReason,
};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use x509_parser::prelude::parse_x509_certificate;

use crate::McpServiceError;
use crate::oauth::OAuthRuntime;
use crate::permits::try_acquire_connection_permit;
use crate::server::{RequestGuard, serve_stream};
use crate::session::SessionTable;

pub(crate) const LAN_DOOR_STATE_PATH: &str = "mcp-endpoint/lan-door-state.json";
pub(crate) const LAN_TLS_PEM_PATH: &str = "mcp-endpoint/lan-tls.pem";
pub(crate) const LAN_CA_KEY_PATH: &str = "mcp-endpoint/lan-ca.key";
pub(crate) const LAN_CA_PEM_PATH: &str = "mcp-endpoint/lan-ca.pem";
pub(crate) const LAN_LEAF_PEM_PATH: &str = "mcp-endpoint/lan-leaf.pem";

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LanPublishStep {
    None = 0,
    BeforeCaKey = 1,
    BeforeCaCert = 2,
    BeforeLeaf = 3,
    BeforeRetireLegacy = 4,
}

#[cfg(test)]
pub static LAN_PUBLISH_STOP: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
pub struct LanPublishStopGuard;

#[cfg(test)]
impl Drop for LanPublishStopGuard {
    fn drop(&mut self) {
        LAN_PUBLISH_STOP.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
pub fn set_lan_publish_stop(step: LanPublishStep) -> LanPublishStopGuard {
    LAN_PUBLISH_STOP.store(step as u8, std::sync::atomic::Ordering::SeqCst);
    LanPublishStopGuard
}

#[cfg(test)]
pub static LAN_BIND_ATTEMPTS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

const DEFAULT_CONNECTION_PERMITS: usize = 64;
const DEFAULT_PER_SOURCE_LIMIT: usize = 16;
const DEFAULT_HANDSHAKE_DEADLINE: Duration = Duration::from_secs(5);
const DEFAULT_ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Execution options for running the LAN door loop.
#[derive(Debug, Clone)]
pub struct LanDoorRun {
    pub port: u16,
    pub config_interval: Duration,
    pub rewrite_interval: Duration,
    pub bind_retry_interval: Duration,
    pub enumeration_interval: Duration,
    pub accept_backoff: Duration,
    pub handshake_deadline: Duration,
    pub connection_permits: usize,
    pub per_source_limit: usize,
    pub peer_admitted: fn(IpAddr) -> bool,
    pub bind_admitted: fn(&LocalEndpoint) -> bool,
    #[cfg(test)]
    pub fail_accept_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(test)]
    pub one_shot_panic: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl LanDoorRun {
    pub fn production() -> Self {
        Self {
            port: MCP_LAN_DOOR_PORT,
            config_interval: Duration::from_secs(1),
            rewrite_interval: Duration::from_secs(10),
            bind_retry_interval: Duration::from_secs(2),
            enumeration_interval: Duration::from_secs(5),
            accept_backoff: DEFAULT_ACCEPT_BACKOFF,
            handshake_deadline: DEFAULT_HANDSHAKE_DEADLINE,
            connection_permits: DEFAULT_CONNECTION_PERMITS,
            per_source_limit: DEFAULT_PER_SOURCE_LIMIT,
            peer_admitted: is_admitted_lan_peer,
            bind_admitted: is_admitted_lan_bind_endpoint,
            #[cfg(test)]
            fail_accept_flag: None,
            #[cfg(test)]
            one_shot_panic: None,
        }
    }
}

/// Address-specific status in the LAN door state record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanAddressState {
    pub address: String,
    pub listening: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Non-secret status for the direct LAN door.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanDoorState {
    pub listening: bool,
    pub observed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<LanAddressState>>,
}

/// Read the LAN door state file.
pub fn read_lan_door_state(journal_root: &Path) -> Option<LanDoorState> {
    let bytes = std::fs::read(journal_root.join(LAN_DOOR_STATE_PATH)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write the LAN door state file atomically with mode 0o600.
pub fn write_lan_door_state(
    journal_root: &Path,
    listening: bool,
    reason: Option<&str>,
    fingerprint: Option<&str>,
    addresses: Option<Vec<LanAddressState>>,
) {
    // a crash leaves the last record; until that record ages out of the reader window a turn-off can reach its wait bound, and the agents page can keep showing the door open until the owner reloads.
    let state = LanDoorState {
        listening,
        observed_at: Utc::now(),
        reason: reason.map(str::to_owned),
        fingerprint: fingerprint.map(str::to_owned),
        addresses,
    };
    let path = journal_root.join(LAN_DOOR_STATE_PATH);
    if let Err(_error) = write_json(
        path,
        &state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    ) {
        log::error!("failed to write LAN door state");
    }
}

/// Admission predicate for interface addresses to bind.
pub fn is_admitted_lan_bind_endpoint(endpoint: &LocalEndpoint) -> bool {
    match endpoint.ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            if octets[0] == 10
                || (octets[0] == 172 && (16..=31).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 168)
            {
                true
            } else if octets[0] == 100 && (64..=127).contains(&octets[1]) {
                endpoint.scope == EndpointScope::Vpn
            } else {
                false
            }
        }
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            (octets[0] & 0xfe) == 0xfc
        }
    }
}

/// Fold IPv4-mapped IPv6 address to canonical IPv4 address.
pub fn fold_ipv4_mapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0xffff {
                IpAddr::V4(Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                    (s[7] >> 8) as u8,
                    s[7] as u8,
                ))
            } else {
                IpAddr::V6(v6)
            }
        }
        IpAddr::V4(v4) => IpAddr::V4(v4),
    }
}

/// Admission predicate for connecting peers.
pub fn is_admitted_lan_peer(ip: IpAddr) -> bool {
    let ip = fold_ipv4_mapped(ip);
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
            {
                return false;
            }
            let octets = v4.octets();
            octets[0] == 10
                || (octets[0] == 172 && (16..=31).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 168)
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            let octets = v6.octets();
            if (octets[0] == 0xfe) && ((octets[1] & 0xc0) == 0x80) {
                return false;
            }
            (octets[0] & 0xfe) == 0xfc
        }
    }
}

/// Calculate the source bucket IP for rate limiting (IPv4 /32, IPv6 /64).
pub fn source_bucket_ip(ip: IpAddr) -> IpAddr {
    let ip = fold_ipv4_mapped(ip);
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            IpAddr::V6(Ipv6Addr::new(seg[0], seg[1], seg[2], seg[3], 0, 0, 0, 0))
        }
    }
}

/// Accept error classification disposition.
#[derive(Debug, PartialEq, Eq)]
pub enum AcceptDisposition {
    Skip,
    Backoff,
    Release,
}

/// Classify an accept error into its disposition.
pub fn classify_accept_error(err: &io::Error) -> AcceptDisposition {
    let Some(raw) = err.raw_os_error() else {
        return AcceptDisposition::Release;
    };
    match raw {
        libc::ECONNABORTED
        | libc::ECONNRESET
        | libc::EPROTO
        | libc::ENETDOWN
        | libc::ENETUNREACH
        | libc::EHOSTUNREACH
        | libc::EHOSTDOWN
        | libc::ENOPROTOOPT
        | libc::EOPNOTSUPP
        | libc::EAGAIN
        | libc::EINTR => AcceptDisposition::Skip,
        #[cfg(target_os = "linux")]
        libc::ENONET => AcceptDisposition::Skip,
        libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM => AcceptDisposition::Backoff,
        _ => AcceptDisposition::Release,
    }
}

/// Bind an individual admitted address with non-blocking, IPV6_V6ONLY, and no SO_REUSEADDR/SO_REUSEPORT.
pub fn bind_admitted_address(ip: IpAddr, port: u16) -> io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let domain = match ip {
        IpAddr::V4(_) => Domain::IPV4,
        IpAddr::V6(_) => Domain::IPV6,
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    if ip.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.set_nonblocking(true)?;
    let sock_addr: SocketAddr = (ip, port).into();
    socket.bind(&sock_addr.into())?;
    socket.listen(128)?;
    let std_listener: std::net::TcpListener = socket.into();
    TcpListener::from_std(std_listener)
}

pub(crate) fn compute_cert_fingerprint(cert_der: &[u8]) -> String {
    let digest = Sha256::digest(cert_der);
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub(crate) fn parse_cert_san_ips(
    x509: &x509_parser::certificate::X509Certificate,
) -> BTreeSet<IpAddr> {
    let mut ips = BTreeSet::new();
    for ext in x509.extensions() {
        if let x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) =
            ext.parsed_extension()
        {
            for name in &san.general_names {
                if let x509_parser::extensions::GeneralName::IPAddress(raw_bytes) = name {
                    if raw_bytes.len() == 4 {
                        let octets: [u8; 4] = (*raw_bytes).try_into().unwrap();
                        ips.insert(fold_ipv4_mapped(IpAddr::V4(Ipv4Addr::from(octets))));
                    } else if raw_bytes.len() == 16 {
                        let octets: [u8; 16] = (*raw_bytes).try_into().unwrap();
                        ips.insert(fold_ipv4_mapped(IpAddr::V6(Ipv6Addr::from(octets))));
                    }
                }
            }
        }
    }
    ips
}

#[derive(Debug, Clone)]
pub struct SelectedLanIdentity {
    pub config: Arc<rustls::ServerConfig>,
    pub sans: BTreeSet<IpAddr>,
    pub leaf_fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct LanIdentity {
    pub ca_fingerprint: Option<String>,
    pub selected: Option<Arc<SelectedLanIdentity>>,
}

#[derive(Debug)]
struct ValidatedCa {
    ca_cert_der: Vec<u8>,
    ca_key_pem: String,
    ca_pem: String,
    ca_fingerprint: String,
}

#[derive(Debug)]
enum CaValidation {
    Absent,
    Invalid { ca_fingerprint: Option<String> },
    Valid(ValidatedCa),
}

fn validate_existing_ca(journal_root: &Path) -> CaValidation {
    let ca_pem_path = journal_root.join(LAN_CA_PEM_PATH);
    let ca_key_path = journal_root.join(LAN_CA_KEY_PATH);

    if !ca_pem_path.exists() {
        return CaValidation::Absent;
    }

    let ca_pem_content = match std::fs::read_to_string(&ca_pem_path) {
        Ok(c) => c,
        Err(_) => {
            return CaValidation::Invalid {
                ca_fingerprint: None,
            };
        }
    };

    let ca_entries = match pem::parse_many(&ca_pem_content) {
        Ok(e) => e,
        Err(_) => {
            return CaValidation::Invalid {
                ca_fingerprint: None,
            };
        }
    };

    if ca_entries.len() != 1 || ca_entries[0].tag() != "CERTIFICATE" {
        return CaValidation::Invalid {
            ca_fingerprint: None,
        };
    }

    let ca_cert_der = ca_entries[0].contents().to_vec();
    let (_, ca_x509) = match parse_x509_certificate(&ca_cert_der) {
        Ok(res) => res,
        Err(_) => {
            return CaValidation::Invalid {
                ca_fingerprint: None,
            };
        }
    };

    let ca_fingerprint = compute_cert_fingerprint(&ca_cert_der);

    let ca_key_content = match std::fs::read_to_string(&ca_key_path) {
        Ok(c) => c,
        Err(_) => {
            return CaValidation::Invalid {
                ca_fingerprint: Some(ca_fingerprint),
            };
        }
    };

    let key_entries = match pem::parse_many(&ca_key_content) {
        Ok(e) => e,
        Err(_) => {
            return CaValidation::Invalid {
                ca_fingerprint: Some(ca_fingerprint),
            };
        }
    };

    if key_entries.len() != 1
        || !(key_entries[0].tag() == "PRIVATE KEY" || key_entries[0].tag() == "EC PRIVATE KEY")
    {
        return CaValidation::Invalid {
            ca_fingerprint: Some(ca_fingerprint),
        };
    }

    let ca_key = match KeyPair::from_pem_and_sign_algo(&ca_key_content, &PKCS_ECDSA_P256_SHA256) {
        Ok(k) => k,
        Err(_) => {
            return CaValidation::Invalid {
                ca_fingerprint: Some(ca_fingerprint),
            };
        }
    };

    if ca_key.public_key_der() != ca_x509.tbs_certificate.subject_pki.raw {
        return CaValidation::Invalid {
            ca_fingerprint: Some(ca_fingerprint),
        };
    }

    CaValidation::Valid(ValidatedCa {
        ca_cert_der,
        ca_key_pem: ca_key_content,
        ca_pem: ca_pem_content,
        ca_fingerprint,
    })
}

enum LeafValidation {
    Absent,
    CorruptOrFailClosed,
    ValidMatching(Arc<SelectedLanIdentity>),
    NeedsReissue {
        previous_usable: Option<Arc<SelectedLanIdentity>>,
    },
}

fn classify_existing_leaf(
    journal_root: &Path,
    ca_x509: &x509_parser::certificate::X509Certificate,
    expected_sans: &BTreeSet<IpAddr>,
) -> LeafValidation {
    let leaf_pem_path = journal_root.join(LAN_LEAF_PEM_PATH);
    if !leaf_pem_path.exists() {
        return LeafValidation::Absent;
    }

    let leaf_content = match std::fs::read_to_string(&leaf_pem_path) {
        Ok(c) => c,
        Err(_) => return LeafValidation::CorruptOrFailClosed,
    };

    let entries = match pem::parse_many(&leaf_content) {
        Ok(e) => e,
        Err(_) => return LeafValidation::CorruptOrFailClosed,
    };

    if entries.len() != 2 {
        return LeafValidation::CorruptOrFailClosed;
    }

    let cert_entry = entries.iter().find(|e| e.tag() == "CERTIFICATE");
    let key_entry = entries
        .iter()
        .find(|e| e.tag() == "PRIVATE KEY" || e.tag() == "EC PRIVATE KEY");

    let (cert_der, key_der) = match (cert_entry, key_entry) {
        (Some(c), Some(k)) => (c.contents().to_vec(), k.contents().to_vec()),
        _ => return LeafValidation::CorruptOrFailClosed,
    };

    let (_, leaf_x509) = match parse_x509_certificate(&cert_der) {
        Ok(res) => res,
        Err(_) => return LeafValidation::CorruptOrFailClosed,
    };

    if leaf_x509
        .verify_signature(Some(&ca_x509.tbs_certificate.subject_pki))
        .is_err()
    {
        return LeafValidation::CorruptOrFailClosed;
    }

    let cert_typed = CertificateDer::from(cert_der.clone());
    let key_typed = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));
    let mut config =
        match rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(vec![cert_typed], key_typed)
        {
            Ok(cfg) => cfg,
            Err(_) => return LeafValidation::CorruptOrFailClosed,
        };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let leaf_fingerprint = compute_cert_fingerprint(&cert_der);
    let leaf_sans = parse_cert_san_ips(&leaf_x509);

    let now = Utc::now().timestamp();
    let not_before = leaf_x509.validity().not_before.timestamp();
    let not_after = leaf_x509.validity().not_after.timestamp();

    let is_time_valid = now >= not_before && now < not_after;
    let is_near_expiry = not_after - now < 7 * 86400;

    let usable_identity = if is_time_valid {
        Some(Arc::new(SelectedLanIdentity {
            config: Arc::new(config),
            sans: leaf_sans.clone(),
            leaf_fingerprint,
        }))
    } else {
        None
    };

    if is_time_valid && !is_near_expiry && leaf_sans == *expected_sans {
        LeafValidation::ValidMatching(usable_identity.unwrap())
    } else {
        LeafValidation::NeedsReissue {
            previous_usable: usable_identity,
        }
    }
}

fn generate_and_save_leaf(
    journal_root: &Path,
    ca: &ValidatedCa,
    expected_sans: &BTreeSet<IpAddr>,
) -> Result<Arc<SelectedLanIdentity>, McpServiceError> {
    let ca_params =
        CertificateParams::from_ca_cert_pem(&ca.ca_pem).map_err(|_| McpServiceError::Runtime)?;
    let ca_key = KeyPair::from_pem_and_sign_algo(&ca.ca_key_pem, &PKCS_ECDSA_P256_SHA256)
        .map_err(|_| McpServiceError::Runtime)?;
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|_| McpServiceError::Runtime)?;

    let leaf_key =
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|_| McpServiceError::Runtime)?;
    let mut leaf_params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "solstone lan leaf");
    leaf_params.distinguished_name = dn;
    leaf_params.is_ca = IsCa::ExplicitNoCa;
    leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf_params.subject_alt_names = expected_sans
        .iter()
        .map(|ip| SanType::IpAddress(*ip))
        .collect();
    let now = time::OffsetDateTime::now_utc();
    leaf_params.not_before = now;
    leaf_params.not_after = now + time::Duration::days(825);

    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .map_err(|_| McpServiceError::Runtime)?;
    let leaf_cert_pem = leaf_cert.pem();
    let leaf_key_pem = leaf_key.serialize_pem();
    let combined_leaf_pem = format!("{leaf_cert_pem}\n{leaf_key_pem}");

    let leaf_pem_path = journal_root.join(LAN_LEAF_PEM_PATH);
    atomic_replace(
        &leaf_pem_path,
        combined_leaf_pem.as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|_| McpServiceError::Runtime)?;

    let leaf_cert_der = leaf_cert.der().to_vec();
    let leaf_key_der = leaf_key.serialize_der();

    let (_, leaf_x509) =
        parse_x509_certificate(&leaf_cert_der).map_err(|_| McpServiceError::Runtime)?;
    let sans = parse_cert_san_ips(&leaf_x509);

    let cert_typed = CertificateDer::from(leaf_cert_der.clone());
    let key_typed = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key_der));
    let mut config =
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(vec![cert_typed], key_typed)
            .map_err(|_| McpServiceError::Runtime)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let leaf_fingerprint = compute_cert_fingerprint(&leaf_cert_der);

    Ok(Arc::new(SelectedLanIdentity {
        config: Arc::new(config),
        sans,
        leaf_fingerprint,
    }))
}

fn generate_and_save_ca(journal_root: &Path) -> Result<ValidatedCa, McpServiceError> {
    #[cfg(test)]
    if LAN_PUBLISH_STOP.load(std::sync::atomic::Ordering::SeqCst)
        == LanPublishStep::BeforeCaKey as u8
    {
        return Err(McpServiceError::Runtime);
    }

    let ca_key =
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|_| McpServiceError::Runtime)?;
    let mut ca_params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "solstone lan ca");
    ca_params.distinguished_name = dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let now = time::OffsetDateTime::now_utc();
    ca_params.not_before = now;
    ca_params.not_after = now + time::Duration::days(3650);

    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|_| McpServiceError::Runtime)?;
    let ca_pem = ca_cert.pem();
    let ca_key_pem = ca_key.serialize_pem();

    let ca_key_path = journal_root.join(LAN_CA_KEY_PATH);
    atomic_replace(
        &ca_key_path,
        ca_key_pem.as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|_| McpServiceError::Runtime)?;

    #[cfg(test)]
    if LAN_PUBLISH_STOP.load(std::sync::atomic::Ordering::SeqCst)
        == LanPublishStep::BeforeCaCert as u8
    {
        return Err(McpServiceError::Runtime);
    }

    let ca_pem_path = journal_root.join(LAN_CA_PEM_PATH);
    atomic_replace(
        &ca_pem_path,
        ca_pem.as_bytes(),
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|_| McpServiceError::Runtime)?;

    let ca_cert_der = ca_cert.der().to_vec();
    let ca_fingerprint = compute_cert_fingerprint(&ca_cert_der);

    Ok(ValidatedCa {
        ca_cert_der,
        ca_key_pem,
        ca_pem,
        ca_fingerprint,
    })
}

fn write_lan_door_state_tls_unavailable(journal_root: &Path, admitted_ips: &[IpAddr]) {
    let addr_states: Vec<LanAddressState> = admitted_ips
        .iter()
        .map(|ip| LanAddressState {
            address: ip.to_string(),
            listening: false,
            reason: Some("tls_unavailable".to_string()),
        })
        .collect();
    write_lan_door_state(
        journal_root,
        false,
        Some("tls_unavailable"),
        None,
        Some(addr_states),
    );
}

pub fn reconcile_lan_identity(journal_root: &Path, admitted_ips: &[IpAddr]) -> LanIdentity {
    let ca = match validate_existing_ca(journal_root) {
        CaValidation::Valid(ca) => ca,
        CaValidation::Absent => match generate_and_save_ca(journal_root) {
            Ok(ca) => ca,
            Err(_) => {
                let identity = LanIdentity {
                    ca_fingerprint: None,
                    selected: None,
                };
                if !admitted_ips.is_empty() {
                    write_lan_door_state_tls_unavailable(journal_root, admitted_ips);
                }
                return identity;
            }
        },
        CaValidation::Invalid { ca_fingerprint } => {
            let identity = LanIdentity {
                ca_fingerprint,
                selected: None,
            };
            if !admitted_ips.is_empty() {
                write_lan_door_state_tls_unavailable(journal_root, admitted_ips);
            }
            return identity;
        }
    };

    let ca_fingerprint = ca.ca_fingerprint.clone();

    if admitted_ips.is_empty() {
        return LanIdentity {
            ca_fingerprint: Some(ca_fingerprint),
            selected: None,
        };
    }

    let expected_sans: BTreeSet<IpAddr> = admitted_ips
        .iter()
        .map(|ip| fold_ipv4_mapped(*ip))
        .collect();

    let (_, ca_x509) = match parse_x509_certificate(&ca.ca_cert_der) {
        Ok(parsed) => parsed,
        Err(_) => {
            let identity = LanIdentity {
                ca_fingerprint: Some(ca_fingerprint),
                selected: None,
            };
            write_lan_door_state_tls_unavailable(journal_root, admitted_ips);
            return identity;
        }
    };

    let leaf_classification = classify_existing_leaf(journal_root, &ca_x509, &expected_sans);

    let selected = match leaf_classification {
        LeafValidation::ValidMatching(selected) => selected,
        LeafValidation::CorruptOrFailClosed => {
            let identity = LanIdentity {
                ca_fingerprint: Some(ca_fingerprint),
                selected: None,
            };
            write_lan_door_state_tls_unavailable(journal_root, admitted_ips);
            return identity;
        }
        LeafValidation::Absent | LeafValidation::NeedsReissue { .. } => {
            let previous_usable = match leaf_classification {
                LeafValidation::NeedsReissue { previous_usable } => previous_usable,
                _ => None,
            };

            #[cfg(test)]
            if LAN_PUBLISH_STOP.load(std::sync::atomic::Ordering::SeqCst)
                == LanPublishStep::BeforeLeaf as u8
            {
                let identity = LanIdentity {
                    ca_fingerprint: Some(ca_fingerprint),
                    selected: previous_usable,
                };
                if identity.selected.is_none() {
                    write_lan_door_state_tls_unavailable(journal_root, admitted_ips);
                }
                return identity;
            }

            match generate_and_save_leaf(journal_root, &ca, &expected_sans) {
                Ok(newly_generated) => newly_generated,
                Err(_) => {
                    let identity = LanIdentity {
                        ca_fingerprint: Some(ca_fingerprint),
                        selected: previous_usable,
                    };
                    if identity.selected.is_none() {
                        write_lan_door_state_tls_unavailable(journal_root, admitted_ips);
                    }
                    return identity;
                }
            }
        }
    };

    #[cfg(test)]
    let skip_legacy_retire = LAN_PUBLISH_STOP.load(std::sync::atomic::Ordering::SeqCst)
        == LanPublishStep::BeforeRetireLegacy as u8;
    #[cfg(not(test))]
    let skip_legacy_retire = false;

    if !skip_legacy_retire {
        let legacy_path = journal_root.join(LAN_TLS_PEM_PATH);
        if legacy_path.exists() {
            let _ = std::fs::remove_file(legacy_path);
        }
    }

    LanIdentity {
        ca_fingerprint: Some(ca_fingerprint),
        selected: Some(selected),
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn test_loopback_server_config() -> Arc<rustls::ServerConfig> {
    test_loopback_selected_identity(&[
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ])
    .config
    .clone()
}

#[cfg(test)]
pub(crate) fn test_loopback_selected_identity(sans: &[IpAddr]) -> Arc<SelectedLanIdentity> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "solstone test loopback");
    params.distinguished_name = dn;
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.subject_alt_names = sans.iter().map(|ip| SanType::IpAddress(*ip)).collect();
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(1);
    let cert = params.self_signed(&key).unwrap();
    let cert_der = cert.der().to_vec();
    let key_der = key.serialize_der();

    let cert_typed = CertificateDer::from(cert_der.clone());
    let key_typed = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der));
    let mut config =
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(vec![cert_typed], key_typed)
            .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let leaf_fingerprint = compute_cert_fingerprint(&cert_der);
    Arc::new(SelectedLanIdentity {
        config: Arc::new(config),
        sans: sans.iter().copied().collect(),
        leaf_fingerprint,
    })
}

struct SourceSlotGuard {
    bucket: IpAddr,
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl Drop for SourceSlotGuard {
    fn drop(&mut self) {
        let mut map = self.counts.lock().unwrap();
        if let Some(count) = map.get_mut(&self.bucket) {
            if *count <= 1 {
                map.remove(&self.bucket);
            } else {
                *count -= 1;
            }
        }
    }
}

/// Run the native LAN door service with an optional hosted parent runtime.
pub fn run_lan_door_with_hosted_parent(
    journal_root: PathBuf,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), McpServiceError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("mcp-lan-door")
        .build()
        .map_err(|_| McpServiceError::Runtime)?;
    runtime.block_on(run_lan_door_hosted_async(
        journal_root,
        LanDoorRun::production(),
        hosted_parent,
    ))
}

pub async fn run_lan_door_hosted_async(
    journal_root: PathBuf,
    run: LanDoorRun,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
) -> Result<(), McpServiceError> {
    let (shutdown_send, shutdown_receive) = watch::channel(false);
    let signal_task = tokio::spawn(wait_for_shutdown_signal(shutdown_send.clone()));
    let (parent_loss_send, mut parent_loss_receive) = tokio::sync::oneshot::channel();
    let parent_task = hosted_parent.as_ref().map(|parent| {
        tokio::spawn(wait_for_hosted_parent(
            Arc::clone(parent),
            shutdown_send.clone(),
            parent_loss_send,
        ))
    });

    let oauth = Arc::new(OAuthRuntime::new_lan_door(&journal_root));
    let iface_source = SystemInterfaceSource;

    let result =
        run_lan_door_loop_with_sources(&journal_root, run, shutdown_receive, oauth, &iface_source)
            .await;

    let service_stopped = result.is_ok();
    signal_task.abort();
    let _ = signal_task.await;
    if let Some(parent_task) = parent_task {
        parent_task.abort();
        let _ = parent_task.await;
    }
    write_lan_door_state(&journal_root, false, Some("not_running"), None, None);
    if let Some(parent) = hosted_parent {
        let reason = parent_loss_receive.try_recv().ok().or_else(|| {
            parent
                .retire_expected_requested()
                .then_some(ParentLossReason::ExitedOrReused)
        });
        if reason.is_some() {
            parent
                .finish_parent_loss(HostedServiceShutdownEvidence {
                    listener_stopped: service_stopped,
                    service_runner_stopped: service_stopped,
                    operational_artifacts_cleaned: true,
                })
                .map_err(|_| McpServiceError::ParentLoss)?;
        }
    }
    result
}

async fn wait_for_shutdown_signal(shutdown_send: watch::Sender<bool>) {
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
    let mut sigint =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).expect("signal");
    tokio::select! {
        _ = sigterm.recv() => {}
        _ = sigint.recv() => {}
    }
    let _ = shutdown_send.send(true);
}

async fn wait_for_hosted_parent(
    parent: Arc<HostedServiceParentRuntime>,
    shutdown_send: watch::Sender<bool>,
    parent_loss_send: tokio::sync::oneshot::Sender<ParentLossReason>,
) {
    let reason = parent.await_parent_loss().await;
    let _ = parent_loss_send.send(reason);
    let _ = shutdown_send.send(true);
}

/// Run the LAN door asynchronously with production defaults.
pub(crate) async fn run_lan_door_async(
    journal_root: PathBuf,
    oauth: Arc<OAuthRuntime>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), McpServiceError> {
    run_lan_door_loop_with_sources(
        &journal_root,
        LanDoorRun::production(),
        shutdown_rx,
        oauth,
        &SystemInterfaceSource,
    )
    .await
}

/// Run the main LAN door loop.
pub async fn run_lan_door_loop(
    journal_root: &Path,
    run: LanDoorRun,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), McpServiceError> {
    let oauth = Arc::new(OAuthRuntime::new_lan_door(journal_root));
    let iface_source = SystemInterfaceSource;

    run_lan_door_loop_with_sources(journal_root, run, shutdown_rx, oauth, &iface_source).await
}

pub(crate) async fn run_lan_door_loop_with_sources(
    journal_root: &Path,
    run: LanDoorRun,
    mut shutdown_rx: watch::Receiver<bool>,
    oauth: Arc<OAuthRuntime>,
    iface_source: &(dyn RawInterfaceSource + Send + Sync),
) -> Result<(), McpServiceError> {
    let journal_root_arc = Arc::new(journal_root.to_path_buf());
    let sessions = Arc::new(SessionTable::new());
    let pool_semaphore = Arc::new(Semaphore::new(run.connection_permits));
    let source_counts: Arc<Mutex<HashMap<IpAddr, usize>>> = Arc::new(Mutex::new(HashMap::new()));
    let server_config_cell: Arc<ArcSwap<Option<Arc<SelectedLanIdentity>>>> =
        Arc::new(ArcSwap::from_pointee(None));

    let mut active_tasks: HashMap<IpAddr, (watch::Sender<bool>, JoinHandle<()>)> = HashMap::new();
    let mut address_states: HashMap<IpAddr, LanAddressState> = HashMap::new();
    let mut current_fingerprint: Option<String> = None;
    let mut first_observation = true;

    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        let pass_result = std::panic::AssertUnwindSafe(async {
            #[cfg(test)]
            if let Some(panic_flag) = &run.one_shot_panic {
                #[allow(clippy::collapsible_if)]
                if panic_flag.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    panic!("simulated panic in control pass");
                }
            }

            // 1. Read config
            let config_res = read_journal_config(journal_root);
            let config_enum = match config_res {
                Ok(cfg) => {
                    first_observation = false;
                    lan_door_config(&cfg)
                }
                Err(_err) => {
                    if first_observation {
                        write_lan_door_state(
                            journal_root,
                            false,
                            Some("config_unreadable"),
                            None,
                            None,
                        );
                        return Ok(());
                    }
                    LocalDoorConfig::On
                }
            };

            match config_enum {
                LocalDoorConfig::Invalid => {
                    stop_all_listeners(&mut active_tasks).await;
                    address_states.clear();
                    server_config_cell.store(Arc::new(None));
                    current_fingerprint = None;
                    write_lan_door_state(journal_root, false, Some("config_invalid"), None, None);
                    return Ok(());
                }
                LocalDoorConfig::Off => {
                    stop_all_listeners(&mut active_tasks).await;
                    address_states.clear();
                    server_config_cell.store(Arc::new(None));
                    current_fingerprint = None;
                    write_lan_door_state(journal_root, false, Some("disabled"), None, None);
                    return Ok(());
                }
                LocalDoorConfig::On => {}
            }

            #[cfg(test)]
            if LAN_PUBLISH_STOP.load(std::sync::atomic::Ordering::SeqCst) != 0 {
                stop_all_listeners(&mut active_tasks).await;
                address_states.clear();
                server_config_cell.store(Arc::new(None));
                current_fingerprint = None;
                write_lan_door_state(journal_root, false, Some("tls_unavailable"), None, None);
                return Ok(());
            }

            // 2. Enumerate interfaces
            let raw_interfaces = match iface_source.enumerate() {
                Ok(raw) => raw,
                Err(_) => {
                    log::error!("interface enumeration failed");
                    if active_tasks.is_empty() {
                        write_lan_door_state(
                            journal_root,
                            false,
                            Some("enumeration_failed"),
                            None,
                            None,
                        );
                    }
                    return Ok(());
                }
            };

            let endpoints = classify_interface_addresses(&raw_interfaces);
            let mut admitted_ips: Vec<IpAddr> = endpoints
                .into_iter()
                .filter(run.bind_admitted)
                .map(|ep| ep.ip)
                .collect();
            admitted_ips.sort();
            admitted_ips.dedup();

            if admitted_ips.is_empty() {
                stop_all_listeners(&mut active_tasks).await;
                address_states.clear();
                server_config_cell.store(Arc::new(None));
                current_fingerprint = None;
                write_lan_door_state(journal_root, false, Some("no_address"), None, None);
                return Ok(());
            }

            // 3. Reconcile TLS identity
            let identity = reconcile_lan_identity(journal_root, &admitted_ips);
            let selected = match identity.selected {
                Some(s) => s,
                None => {
                    stop_all_listeners(&mut active_tasks).await;
                    address_states.clear();
                    server_config_cell.store(Arc::new(None));
                    current_fingerprint = None;
                    write_lan_door_state_tls_unavailable(journal_root, &admitted_ips);
                    return Ok(());
                }
            };

            current_fingerprint = Some(selected.leaf_fingerprint.clone());
            server_config_cell.store(Arc::new(Some(Arc::clone(&selected))));

            // Stop listeners whose IP is not in admitted_ips or not in the parsed SAN set
            let mut to_remove = Vec::new();
            for ip in active_tasks.keys() {
                if !admitted_ips.contains(ip) || !selected.sans.contains(&fold_ipv4_mapped(*ip)) {
                    to_remove.push(*ip);
                }
            }
            for ip in to_remove {
                if let Some((stop_tx, handle)) = active_tasks.remove(&ip) {
                    let _ = stop_tx.send(true);
                    handle.abort();
                    let _ = handle.await;
                }
                address_states.remove(&ip);
            }

            // Clean up finished tasks
            let finished_ips: Vec<IpAddr> = active_tasks
                .iter()
                .filter(|(_, (_, handle))| handle.is_finished())
                .map(|(ip, _)| *ip)
                .collect();
            for ip in finished_ips {
                active_tasks.remove(&ip);
                if let Some(state) = address_states.get_mut(&ip) {
                    state.listening = false;
                    state.reason = Some("retrying".to_string());
                }
            }

            // Bind new or retrying IPs
            for ip in &admitted_ips {
                let bound_ip = *ip;
                if !selected.sans.contains(&fold_ipv4_mapped(bound_ip)) {
                    address_states.insert(
                        bound_ip,
                        LanAddressState {
                            address: bound_ip.to_string(),
                            listening: false,
                            reason: Some("tls_unavailable".to_string()),
                        },
                    );
                    continue;
                }

                if active_tasks.contains_key(&bound_ip) {
                    continue;
                }

                #[cfg(test)]
                LAN_BIND_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                match bind_admitted_address(bound_ip, run.port) {
                    Ok(listener) => {
                        let (stop_tx, stop_rx) = watch::channel(false);
                        let root_clone = Arc::clone(&journal_root_arc);
                        let oauth_clone = Arc::clone(&oauth);
                        let sessions_clone = Arc::clone(&sessions);
                        let config_cell_clone = Arc::clone(&server_config_cell);
                        let pool_clone = Arc::clone(&pool_semaphore);
                        let counts_clone = Arc::clone(&source_counts);
                        let run_clone = run.clone();

                        let handle = tokio::spawn(async move {
                            accept_loop_for_address(
                                listener,
                                bound_ip,
                                root_clone,
                                oauth_clone,
                                sessions_clone,
                                config_cell_clone,
                                pool_clone,
                                counts_clone,
                                run_clone,
                                stop_rx,
                            )
                            .await;
                        });

                        active_tasks.insert(bound_ip, (stop_tx, handle));
                        address_states.insert(
                            bound_ip,
                            LanAddressState {
                                address: bound_ip.to_string(),
                                listening: true,
                                reason: None,
                            },
                        );
                    }
                    Err(err) => {
                        let reason = if err.kind() == io::ErrorKind::AddrInUse {
                            "port_in_use"
                        } else {
                            "bind_failed"
                        };
                        address_states.insert(
                            bound_ip,
                            LanAddressState {
                                address: bound_ip.to_string(),
                                listening: false,
                                reason: Some(reason.to_string()),
                            },
                        );
                    }
                }
            }

            // Write state record
            let sorted_states: Vec<LanAddressState> = admitted_ips
                .iter()
                .filter_map(|ip| address_states.get(ip).cloned())
                .collect();
            let listening = sorted_states.iter().any(|s| s.listening);
            let top_reason: Option<String> = if listening {
                None
            } else {
                sorted_states
                    .iter()
                    .find_map(|s| s.reason.clone())
                    .or_else(|| Some("retrying".to_string()))
            };
            let fp = if listening {
                current_fingerprint.as_deref()
            } else {
                None
            };

            write_lan_door_state(
                journal_root,
                listening,
                top_reason.as_deref(),
                fp,
                Some(sorted_states),
            );

            Ok::<(), McpServiceError>(())
        })
        .catch_unwind()
        .await;

        if let Err(_panic_err) = pass_result {
            log::error!("recovered from panic in LAN door control loop");
            stop_all_listeners(&mut active_tasks).await;
            address_states.clear();
        }

        tokio::select! {
            _ = tokio::time::sleep(run.config_interval) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    stop_all_listeners(&mut active_tasks).await;
    Ok(())
}

async fn stop_all_listeners(tasks: &mut HashMap<IpAddr, (watch::Sender<bool>, JoinHandle<()>)>) {
    for (_, (stop_tx, handle)) in tasks.drain() {
        let _ = stop_tx.send(true);
        handle.abort();
        let _ = handle.await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn accept_loop_for_address(
    listener: TcpListener,
    bound_ip: IpAddr,
    journal_root: Arc<PathBuf>,
    oauth: Arc<OAuthRuntime>,
    sessions: Arc<SessionTable>,
    server_config_cell: Arc<ArcSwap<Option<Arc<SelectedLanIdentity>>>>,
    pool_semaphore: Arc<Semaphore>,
    source_counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
    run: LanDoorRun,
    mut stop_rx: watch::Receiver<bool>,
) {
    loop {
        if *stop_rx.borrow() {
            break;
        }

        #[cfg(test)]
        if let Some(fail_flag) = &run.fail_accept_flag {
            #[allow(clippy::collapsible_if)]
            if fail_flag.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
        }

        tokio::select! {
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    break;
                }
            }
            res = listener.accept() => {
                match res {
                    Ok((stream, peer_addr)) => {
                        let selected_opt = server_config_cell.load_full();
                        let selected = match selected_opt.as_ref() {
                            Some(s) if s.sans.contains(&fold_ipv4_mapped(bound_ip)) => Arc::clone(s),
                            _ => {
                                drop(stream);
                                continue;
                            }
                        };

                        // 1. Peer admission check
                        if !(run.peer_admitted)(peer_addr.ip()) {
                            drop(stream);
                            continue;
                        }

                        // 2. Global connection pool permit
                        let permit = match try_acquire_connection_permit(&pool_semaphore) {
                            Some(p) => p,
                            None => {
                                drop(stream);
                                continue;
                            }
                        };

                        // 3. Per-source slot limit
                        let bucket = source_bucket_ip(peer_addr.ip());
                        {
                            let mut counts = source_counts.lock().unwrap();
                            let count = counts.entry(bucket).or_insert(0);
                            if *count >= run.per_source_limit {
                                drop(stream);
                                drop(permit);
                                continue;
                            }
                            *count += 1;
                        }
                        let slot_guard = SourceSlotGuard {
                            bucket,
                            counts: Arc::clone(&source_counts),
                        };

                        // 4. Spawn handshake & connection handler
                        let tls_acceptor = TlsAcceptor::from(Arc::clone(&selected.config));
                        let root_clone = Arc::clone(&journal_root);
                        let oauth_clone = Arc::clone(&oauth);
                        let sessions_clone = Arc::clone(&sessions);
                        let port = run.port;
                        let handshake_deadline = run.handshake_deadline;
                        let mut stop_rx_task = stop_rx.clone();

                        tokio::spawn(async move {
                            let _permit = permit;
                            let _guard = slot_guard;

                            let handshake_future = tls_acceptor.accept(stream);
                            let tls_res = tokio::select! {
                                res = tokio::time::timeout(handshake_deadline, handshake_future) => {
                                    match res {
                                        Ok(Ok(tls_stream)) => Ok(tls_stream),
                                        Ok(Err(e)) => Err(io::Error::other(e)),
                                        Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out")),
                                    }
                                }
                                _ = stop_rx_task.changed() => {
                                    Err(io::Error::new(io::ErrorKind::Interrupted, "shutdown"))
                                }
                            };

                            let tls_stream = match tls_res {
                                Ok(s) => s,
                                Err(_) => return,
                            };

                            let _ = serve_stream(
                                tls_stream,
                                root_clone,
                                oauth_clone,
                                sessions_clone,
                                bucket,
                                stop_rx_task,
                                RequestGuard::IpLiteral { port },
                            )
                            .await;
                        });
                    }
                    Err(err) => {
                        match classify_accept_error(&err) {
                            AcceptDisposition::Skip => continue,
                            AcceptDisposition::Backoff => {
                                tokio::select! {
                                    _ = tokio::time::sleep(run.accept_backoff) => continue,
                                    _ = stop_rx.changed() => break,
                                }
                            }
                            AcceptDisposition::Release => {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::client::danger::ServerCertVerifier;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn bind_admission_predicate_cases() {
        // RFC 1918 in any scope
        assert!(is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 1, 2)),
            scope: EndpointScope::Lan,
        }));
        assert!(is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(172, 16, 0, 5)),
            scope: EndpointScope::Lan,
        }));
        assert!(is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
            scope: EndpointScope::Lan,
        }));

        // 100.64/10 only when scope is Vpn
        assert!(is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            scope: EndpointScope::Vpn,
        }));
        assert!(!is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            scope: EndpointScope::Lan,
        }));

        // fc00::/7 ULA in any scope
        assert!(is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
            scope: EndpointScope::Ula,
        }));
        assert!(is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)),
            scope: EndpointScope::Lan,
        }));

        // Excluded: loopback, link-local, public, multicast
        assert!(!is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            scope: EndpointScope::Lan,
        }));
        assert!(!is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
            scope: EndpointScope::Lan,
        }));
        assert!(!is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
            scope: EndpointScope::Lan,
        }));
        assert!(!is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
            scope: EndpointScope::Lan,
        }));
        assert!(!is_admitted_lan_bind_endpoint(&LocalEndpoint {
            ip: IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            scope: EndpointScope::Lan,
        }));
    }

    #[test]
    fn peer_admission_predicate_cases() {
        // Folded IPv4-mapped
        assert!(is_admitted_lan_peer(IpAddr::V6(Ipv6Addr::new(
            0, 0, 0, 0, 0, 0xffff, 0x0a00, 0x0102
        )))); // 10.0.1.2
        assert!(is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 2))));
        assert!(is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::new(
            172, 20, 1, 2
        ))));
        assert!(is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::new(
            192, 168, 1, 2
        ))));
        assert!(is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::new(
            100, 64, 1, 2
        ))));
        assert!(is_admitted_lan_peer(IpAddr::V6(Ipv6Addr::new(
            0xfd12, 0x3456, 0, 0, 0, 0, 0, 1
        ))));

        // Refused: loopback, linklocal, public, multicast, broadcast
        assert!(!is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::new(
            127, 0, 0, 1
        ))));
        assert!(!is_admitted_lan_peer(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::new(
            169, 254, 1, 1
        ))));
        assert!(!is_admitted_lan_peer(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(!is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::BROADCAST)));
        assert!(!is_admitted_lan_peer(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert!(!is_admitted_lan_peer(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn source_bucket_ip_cases() {
        assert_eq!(
            source_bucket_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50))),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50))
        );
        assert_eq!(
            source_bucket_ip(IpAddr::V6(Ipv6Addr::new(0xfd00, 1, 2, 3, 4, 5, 6, 7))),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 1, 2, 3, 0, 0, 0, 0))
        );
        assert_eq!(
            source_bucket_ip(IpAddr::V6(Ipv6Addr::new(
                0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0101
            ))),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))
        );
    }

    #[test]
    fn accept_error_classification_cases() {
        assert_eq!(
            classify_accept_error(&io::Error::from_raw_os_error(libc::ECONNABORTED)),
            AcceptDisposition::Skip
        );
        assert_eq!(
            classify_accept_error(&io::Error::from_raw_os_error(libc::ECONNRESET)),
            AcceptDisposition::Skip
        );
        assert_eq!(
            classify_accept_error(&io::Error::from_raw_os_error(libc::EAGAIN)),
            AcceptDisposition::Skip
        );
        assert_eq!(
            classify_accept_error(&io::Error::from_raw_os_error(libc::EMFILE)),
            AcceptDisposition::Backoff
        );
        assert_eq!(
            classify_accept_error(&io::Error::from_raw_os_error(libc::ENFILE)),
            AcceptDisposition::Backoff
        );
        assert_eq!(
            classify_accept_error(&io::Error::from_raw_os_error(libc::ENOBUFS)),
            AcceptDisposition::Backoff
        );
        assert_eq!(
            classify_accept_error(&io::Error::from_raw_os_error(libc::ETIMEDOUT)),
            AcceptDisposition::Release
        );
        assert_eq!(
            classify_accept_error(&io::Error::other("custom")),
            AcceptDisposition::Release
        );
    }

    #[test]
    fn state_file_serialization_and_read() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-state-test-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();

        let addrs = vec![
            LanAddressState {
                address: "192.168.1.10".to_string(),
                listening: true,
                reason: None,
            },
            LanAddressState {
                address: "fd00::1".to_string(),
                listening: false,
                reason: Some("bind_failed".to_string()),
            },
        ];

        write_lan_door_state(
            journal_root,
            true,
            None,
            Some("AA:BB:CC:DD"),
            Some(addrs.clone()),
        );

        let read = read_lan_door_state(journal_root).unwrap();
        assert!(read.listening);
        assert_eq!(read.reason, None);
        assert_eq!(read.fingerprint, Some("AA:BB:CC:DD".to_string()));
        assert_eq!(read.addresses, Some(addrs));
    }

    #[test]
    fn lan_ca_and_leaf_atomic_generation_permissions_and_extensions() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-ca-leaf-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();

        // Put a legacy file to prove it gets removed
        std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
        std::fs::write(journal_root.join(LAN_TLS_PEM_PATH), "legacy").unwrap();

        let admitted = vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
        ];
        let identity = reconcile_lan_identity(journal_root, &admitted);

        let ca_fp = identity.ca_fingerprint.expect("ca_fingerprint present");
        assert_eq!(ca_fp.len(), 32 * 3 - 1);
        let selected = identity.selected.expect("selected present");
        assert_eq!(selected.leaf_fingerprint.len(), 32 * 3 - 1);

        // Check files exist
        let ca_key_path = journal_root.join(LAN_CA_KEY_PATH);
        let ca_pem_path = journal_root.join(LAN_CA_PEM_PATH);
        let leaf_pem_path = journal_root.join(LAN_LEAF_PEM_PATH);
        let legacy_path = journal_root.join(LAN_TLS_PEM_PATH);

        assert!(ca_key_path.exists());
        assert!(ca_pem_path.exists());
        assert!(leaf_pem_path.exists());
        assert!(!legacy_path.exists());

        // File permissions 0o600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !nix::unistd::geteuid().is_root() {
                assert_eq!(
                    std::fs::metadata(&ca_key_path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
                assert_eq!(
                    std::fs::metadata(&ca_pem_path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
                assert_eq!(
                    std::fs::metadata(&leaf_pem_path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }

        // Validate lan-ca.pem has exactly one CERTIFICATE and no private keys
        let ca_pem_str = std::fs::read_to_string(&ca_pem_path).unwrap();
        let ca_entries = pem::parse_many(&ca_pem_str).unwrap();
        assert_eq!(ca_entries.len(), 1);
        assert_eq!(ca_entries[0].tag(), "CERTIFICATE");

        // Validate lan-ca.key has exactly one PRIVATE KEY
        let ca_key_str = std::fs::read_to_string(&ca_key_path).unwrap();
        let key_entries = pem::parse_many(&ca_key_str).unwrap();
        assert_eq!(key_entries.len(), 1);
        assert!(key_entries[0].tag() == "PRIVATE KEY" || key_entries[0].tag() == "EC PRIVATE KEY");

        // Validate lan-leaf.pem has exactly one CERTIFICATE and one PRIVATE KEY
        let leaf_pem_str = std::fs::read_to_string(&leaf_pem_path).unwrap();
        let leaf_entries = pem::parse_many(&leaf_pem_str).unwrap();
        assert_eq!(leaf_entries.len(), 2);
        assert!(leaf_entries.iter().any(|e| e.tag() == "CERTIFICATE"));
        assert!(
            leaf_entries
                .iter()
                .any(|e| e.tag() == "PRIVATE KEY" || e.tag() == "EC PRIVATE KEY")
        );

        // Validate CA cert x509
        let (_, ca_x509) = parse_x509_certificate(ca_entries[0].contents()).unwrap();
        let ca_cn = ca_x509
            .subject()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(ca_cn, "solstone lan ca");
        assert_eq!(
            ca_x509.signature_algorithm.algorithm.to_string(),
            "1.2.840.10045.4.3.2"
        ); // ECDSA P-256
        let ca_val = ca_x509.validity();
        assert!(
            ca_val.not_after.timestamp() - ca_val.not_before.timestamp() >= 3000 * 86_400,
            "CA lifetime should be at least 3000 days"
        );

        // Validate Leaf cert x509
        let leaf_cert_entry = leaf_entries
            .iter()
            .find(|e| e.tag() == "CERTIFICATE")
            .unwrap();
        let (_, leaf_x509) = parse_x509_certificate(leaf_cert_entry.contents()).unwrap();
        let leaf_cn = leaf_x509
            .subject()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(leaf_cn, "solstone lan leaf");
        let leaf_val = leaf_x509.validity();
        assert_eq!(
            leaf_val.not_after.timestamp() - leaf_val.not_before.timestamp(),
            825 * 86_400
        );
        let leaf_sans = parse_cert_san_ips(&leaf_x509);
        let expected_sans: BTreeSet<IpAddr> = admitted.into_iter().collect();
        assert_eq!(leaf_sans, expected_sans);
    }

    #[test]
    fn lan_leaf_reissued_on_san_change() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-san-change-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();

        let addrs1 = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))];
        let id1 = reconcile_lan_identity(journal_root, &addrs1);
        let ca_pem_1 = std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        let fp_ca_1 = id1.ca_fingerprint.unwrap();
        let fp_leaf_1 = id1.selected.unwrap().leaf_fingerprint.clone();

        let addrs2 = vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
        ];
        let id2 = reconcile_lan_identity(journal_root, &addrs2);
        let ca_pem_2 = std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        let fp_ca_2 = id2.ca_fingerprint.unwrap();
        let fp_leaf_2 = id2.selected.unwrap().leaf_fingerprint.clone();

        assert_eq!(fp_ca_1, fp_ca_2);
        assert_eq!(ca_pem_1, ca_pem_2);
        assert_ne!(fp_leaf_1, fp_leaf_2);

        // Check new leaf has both SANs
        let leaf_pem_str = std::fs::read_to_string(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap();
        let entries = pem::parse_many(&leaf_pem_str).unwrap();
        let cert_entry = entries.iter().find(|e| e.tag() == "CERTIFICATE").unwrap();
        let (_, leaf_x509) = parse_x509_certificate(cert_entry.contents()).unwrap();
        let leaf_sans = parse_cert_san_ips(&leaf_x509);
        let expected_sans: BTreeSet<IpAddr> = addrs2.into_iter().collect();
        assert_eq!(leaf_sans, expected_sans);
    }

    #[test]
    fn lan_leaf_reissued_on_removed_address() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-san-remove-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();

        let addrs1 = vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
        ];
        let id1 = reconcile_lan_identity(journal_root, &addrs1);
        let ca_pem_1 = std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        let fp_leaf_1 = id1.selected.unwrap().leaf_fingerprint.clone();

        let addrs2 = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))];
        let id2 = reconcile_lan_identity(journal_root, &addrs2);
        let ca_pem_2 = std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        let fp_leaf_2 = id2.selected.unwrap().leaf_fingerprint.clone();

        assert_eq!(ca_pem_1, ca_pem_2);
        assert_ne!(fp_leaf_1, fp_leaf_2);

        let leaf_pem_str = std::fs::read_to_string(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap();
        let entries = pem::parse_many(&leaf_pem_str).unwrap();
        let cert_entry = entries.iter().find(|e| e.tag() == "CERTIFICATE").unwrap();
        let (_, leaf_x509) = parse_x509_certificate(cert_entry.contents()).unwrap();
        let leaf_sans = parse_cert_san_ips(&leaf_x509);
        let expected_sans: BTreeSet<IpAddr> = addrs2.into_iter().collect();
        assert_eq!(leaf_sans, expected_sans);
    }

    #[test]
    fn lan_leaf_reissued_when_near_expiry_or_future() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-reissue-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();

        let addrs = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))];
        let id1 = reconcile_lan_identity(journal_root, &addrs);
        let ca_pem_1 = std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        let fp_leaf_1 = id1.selected.unwrap().leaf_fingerprint.clone();

        // 1. Near-expiry leaf (remaining < 7 days)
        let ca_key_str = std::fs::read_to_string(journal_root.join(LAN_CA_KEY_PATH)).unwrap();
        let ca_params = CertificateParams::from_ca_cert_pem(&ca_pem_1).unwrap();
        let ca_key = KeyPair::from_pem_and_sign_algo(&ca_key_str, &PKCS_ECDSA_P256_SHA256).unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();

        let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut leaf_params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "solstone lan leaf");
        leaf_params.distinguished_name = dn;
        leaf_params.is_ca = IsCa::ExplicitNoCa;
        leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        leaf_params.subject_alt_names = vec![SanType::IpAddress(addrs[0])];
        let now = time::OffsetDateTime::now_utc();
        leaf_params.not_before = now - time::Duration::days(100);
        leaf_params.not_after = now + time::Duration::days(3); // 3 days left < 7 days!
        let near_cert = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();
        let combined = format!("{}\n{}", near_cert.pem(), leaf_key.serialize_pem());
        std::fs::write(journal_root.join(LAN_LEAF_PEM_PATH), combined).unwrap();

        let id_near = reconcile_lan_identity(journal_root, &addrs);
        let ca_pem_near = std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        assert_eq!(ca_pem_1, ca_pem_near);
        let fp_near = id_near.selected.unwrap().leaf_fingerprint.clone();
        assert_ne!(fp_leaf_1, fp_near);

        // 2. Future not_before leaf (not yet valid)
        let leaf_key2 = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut leaf_params2 = CertificateParams::default();
        let mut dn2 = DistinguishedName::new();
        dn2.push(DnType::CommonName, "solstone lan leaf");
        leaf_params2.distinguished_name = dn2;
        leaf_params2.is_ca = IsCa::ExplicitNoCa;
        leaf_params2.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params2.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        leaf_params2.subject_alt_names = vec![SanType::IpAddress(addrs[0])];
        leaf_params2.not_before = now + time::Duration::hours(2); // in future!
        leaf_params2.not_after = now + time::Duration::days(825);
        let future_cert = leaf_params2
            .signed_by(&leaf_key2, &ca_cert, &ca_key)
            .unwrap();
        let combined2 = format!("{}\n{}", future_cert.pem(), leaf_key2.serialize_pem());
        std::fs::write(journal_root.join(LAN_LEAF_PEM_PATH), combined2).unwrap();

        let id_future = reconcile_lan_identity(journal_root, &addrs);
        let ca_pem_future = std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        assert_eq!(ca_pem_1, ca_pem_future);
        let fp_future = id_future.selected.unwrap().leaf_fingerprint.clone();
        assert_ne!(fp_near, fp_future);
    }

    #[test]
    fn lan_leaf_fail_closed_on_corrupted_or_mismatched_inputs() {
        let addrs = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))];

        // 1. Corrupted leaf file
        {
            let temp = tempfile::Builder::new()
                .prefix("lan-door-fail-closed-1-")
                .tempdir_in("/var/tmp")
                .unwrap();
            let journal_root = temp.path();
            let id_good = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_good.selected.is_some());
            let good_ca_bytes = std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap();

            std::fs::write(journal_root.join(LAN_LEAF_PEM_PATH), "corrupted leaf").unwrap();
            let id_corrupt = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_corrupt.selected.is_none());
            assert_eq!(
                std::fs::read_to_string(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap(),
                "corrupted leaf"
            );
            assert_eq!(
                std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap(),
                good_ca_bytes
            );
            let state = read_lan_door_state(journal_root).unwrap();
            assert!(!state.listening);
            assert_eq!(state.reason.as_deref(), Some("tls_unavailable"));
        }

        // 2. lan-ca.pem replaced with non-PEM bytes
        {
            let temp = tempfile::Builder::new()
                .prefix("lan-door-fail-closed-2-")
                .tempdir_in("/var/tmp")
                .unwrap();
            let journal_root = temp.path();
            let id_good = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_good.selected.is_some());

            std::fs::write(journal_root.join(LAN_CA_PEM_PATH), "not a valid pem").unwrap();
            let id_bad_ca = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_bad_ca.selected.is_none());
            assert_eq!(
                std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap(),
                "not a valid pem"
            );
            let state = read_lan_door_state(journal_root).unwrap();
            assert!(!state.listening);
            assert_eq!(state.reason.as_deref(), Some("tls_unavailable"));
        }

        // 3. lan-ca.pem contains private key
        {
            let temp = tempfile::Builder::new()
                .prefix("lan-door-fail-closed-3-")
                .tempdir_in("/var/tmp")
                .unwrap();
            let journal_root = temp.path();
            let id_good = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_good.selected.is_some());
            let good_ca_bytes = std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
            let ca_key_str = std::fs::read_to_string(journal_root.join(LAN_CA_KEY_PATH)).unwrap();

            let ca_with_key = format!(
                "{}\n{}",
                String::from_utf8(good_ca_bytes).unwrap(),
                ca_key_str
            );
            std::fs::write(journal_root.join(LAN_CA_PEM_PATH), &ca_with_key).unwrap();
            let id_ca_with_key = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_ca_with_key.selected.is_none());
            assert_eq!(
                std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap(),
                ca_with_key
            );
        }

        // 4. Leaf replaced by cert signed by unrelated CA
        {
            let temp = tempfile::Builder::new()
                .prefix("lan-door-fail-closed-4-")
                .tempdir_in("/var/tmp")
                .unwrap();
            let journal_root = temp.path();
            let id_good = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_good.selected.is_some());
            let good_ca_bytes = std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap();

            let unrelated_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let mut unrelated_params = CertificateParams::default();
            unrelated_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            let unrelated_ca = unrelated_params.self_signed(&unrelated_key).unwrap();

            let foreign_leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let mut foreign_leaf_params = CertificateParams::default();
            foreign_leaf_params.is_ca = IsCa::ExplicitNoCa;
            foreign_leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            foreign_leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
            foreign_leaf_params.subject_alt_names = vec![SanType::IpAddress(addrs[0])];
            let foreign_cert = foreign_leaf_params
                .signed_by(&foreign_leaf_key, &unrelated_ca, &unrelated_key)
                .unwrap();
            let foreign_leaf_pem = format!(
                "{}\n{}",
                foreign_cert.pem(),
                foreign_leaf_key.serialize_pem()
            );
            std::fs::write(journal_root.join(LAN_LEAF_PEM_PATH), &foreign_leaf_pem).unwrap();

            let id_foreign = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_foreign.selected.is_none());
            assert_eq!(
                std::fs::read_to_string(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap(),
                foreign_leaf_pem
            );
            assert_eq!(
                std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap(),
                good_ca_bytes
            );
        }

        // 5. Leaf key swapped for a different key
        {
            let temp = tempfile::Builder::new()
                .prefix("lan-door-fail-closed-5-")
                .tempdir_in("/var/tmp")
                .unwrap();
            let journal_root = temp.path();
            let id_good = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_good.selected.is_some());
            let good_ca_bytes = std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap();

            let current_leaf_str =
                std::fs::read_to_string(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap();
            let entries = pem::parse_many(&current_leaf_str).unwrap();
            let cert_entry = entries.iter().find(|e| e.tag() == "CERTIFICATE").unwrap();
            let swapped_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let swapped_leaf_pem = format!(
                "{}\n{}",
                pem::encode(cert_entry),
                swapped_key.serialize_pem()
            );
            std::fs::write(journal_root.join(LAN_LEAF_PEM_PATH), &swapped_leaf_pem).unwrap();

            let id_swapped = reconcile_lan_identity(journal_root, &addrs);
            assert!(id_swapped.selected.is_none());
            assert_eq!(
                std::fs::read_to_string(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap(),
                swapped_leaf_pem
            );
            assert_eq!(
                std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap(),
                good_ca_bytes
            );
        }
    }

    #[test]
    fn lan_ca_durability_and_issuance_failure_cuts() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-cuts-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();
        let addrs_a = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))];

        let id_a = reconcile_lan_identity(journal_root, &addrs_a);
        assert!(id_a.selected.is_some());
        let ca_bytes_initial = std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        let leaf_bytes_initial = std::fs::read(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap();

        // 1. Arm BeforeLeaf cut and reconcile {A, B}
        let guard = set_lan_publish_stop(LanPublishStep::BeforeLeaf);
        let addrs_ab = vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
        ];
        let id_cut = reconcile_lan_identity(journal_root, &addrs_ab);

        // On-disk leaf bytes unchanged
        let leaf_bytes_after_cut = std::fs::read(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap();
        assert_eq!(leaf_bytes_initial, leaf_bytes_after_cut);

        // Returned selected is previous usable leaf whose sans do not contain .20
        if let Some(sel) = id_cut.selected {
            assert!(
                !sel.sans
                    .contains(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)))
            );
            assert_eq!(sel.sans, addrs_a.iter().copied().collect());
        }

        // CA bytes identical
        assert_eq!(
            std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap(),
            ca_bytes_initial
        );

        // Drop guard and reconcile {A, B} cleanly
        drop(guard);
        let id_clean = reconcile_lan_identity(journal_root, &addrs_ab);
        assert!(id_clean.selected.is_some());
        let clean_sel = id_clean.selected.unwrap();
        assert_eq!(clean_sel.sans, addrs_ab.iter().copied().collect());
        assert_eq!(
            std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap(),
            ca_bytes_initial
        );
    }

    #[test]
    fn lan_migration_from_legacy_cuts() {
        let cuts = [
            LanPublishStep::BeforeCaKey,
            LanPublishStep::BeforeCaCert,
            LanPublishStep::BeforeLeaf,
            LanPublishStep::BeforeRetireLegacy,
        ];

        let admitted = vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
        ];

        for cut in cuts {
            let temp = tempfile::Builder::new()
                .prefix("lan-migration-cut-")
                .tempdir_in("/var/tmp")
                .unwrap();
            let journal_root = temp.path();

            // Setup legacy self-signed certificate
            let leg_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
            let mut leg_params = CertificateParams::default();
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "solstone journal");
            leg_params.distinguished_name = dn;
            leg_params.is_ca = IsCa::ExplicitNoCa;
            let leg_cert = leg_params.self_signed(&leg_key).unwrap();
            let leg_combined = format!("{}\n{}", leg_cert.pem(), leg_key.serialize_pem());
            std::fs::create_dir_all(journal_root.join("mcp-endpoint")).unwrap();
            std::fs::write(journal_root.join(LAN_TLS_PEM_PATH), &leg_combined).unwrap();
            let leg_fp = compute_cert_fingerprint(leg_cert.der());

            let guard = set_lan_publish_stop(cut);
            let id = reconcile_lan_identity(journal_root, &admitted);

            if let Some(sel) = &id.selected {
                let ca_pem_bytes = std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
                let ca_entries = pem::parse_many(String::from_utf8(ca_pem_bytes).unwrap()).unwrap();
                let mut root_store = rustls::RootCertStore::empty();
                root_store
                    .add(CertificateDer::from(ca_entries[0].contents().to_vec()))
                    .unwrap();
                let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(root_store))
                    .build()
                    .unwrap();

                let leaf_bytes = std::fs::read(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap();
                let leaf_entries = pem::parse_many(String::from_utf8(leaf_bytes).unwrap()).unwrap();
                let leaf_cert = leaf_entries
                    .iter()
                    .find(|e| e.tag() == "CERTIFICATE")
                    .unwrap();
                let end_entity = CertificateDer::from(leaf_cert.contents().to_vec());

                for ip in &admitted {
                    let server_name =
                        rustls::pki_types::ServerName::try_from(ip.to_string()).unwrap();
                    assert!(
                        verifier
                            .verify_server_cert(
                                &end_entity,
                                &[],
                                &server_name,
                                &[],
                                rustls::pki_types::UnixTime::now(),
                            )
                            .is_ok()
                    );
                }
                assert_ne!(sel.leaf_fingerprint, leg_fp);
            } else {
                let state = read_lan_door_state(journal_root).unwrap();
                assert!(!state.listening);
                assert_eq!(state.reason.as_deref(), Some("tls_unavailable"));
            }

            let ca_bytes_before = if journal_root.join(LAN_CA_PEM_PATH).exists() {
                Some(std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap())
            } else {
                None
            };

            drop(guard);
            let id_complete = reconcile_lan_identity(journal_root, &admitted);
            assert!(id_complete.selected.is_some());
            let complete_sel = id_complete.selected.unwrap();

            if let Some(before) = ca_bytes_before {
                assert_eq!(
                    std::fs::read(journal_root.join(LAN_CA_PEM_PATH)).unwrap(),
                    before
                );
            }

            assert!(!journal_root.join(LAN_TLS_PEM_PATH).exists());
            assert_eq!(complete_sel.sans, admitted.iter().copied().collect());
            assert_ne!(complete_sel.leaf_fingerprint, leg_fp);
        }
    }

    #[test]
    fn lan_leaf_webpki_ip_san_and_chain_verification() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-webpki-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();
        let ip_v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
        let ip_v6 = IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1));
        let admitted = vec![ip_v4, ip_v6];

        let id = reconcile_lan_identity(journal_root, &admitted);
        assert!(id.selected.is_some());

        let ca_pem_str = std::fs::read_to_string(journal_root.join(LAN_CA_PEM_PATH)).unwrap();
        let leaf_pem_str = std::fs::read_to_string(journal_root.join(LAN_LEAF_PEM_PATH)).unwrap();

        let ca_entries = pem::parse_many(&ca_pem_str).unwrap();
        let (_, ca_x509) = parse_x509_certificate(ca_entries[0].contents()).unwrap();

        let leaf_entries = pem::parse_many(&leaf_pem_str).unwrap();
        let leaf_cert_entry = leaf_entries
            .iter()
            .find(|e| e.tag() == "CERTIFICATE")
            .unwrap();
        let (_, leaf_x509) = parse_x509_certificate(leaf_cert_entry.contents()).unwrap();

        // 1. x509-parser structural checks
        assert_eq!(
            parse_cert_san_ips(&leaf_x509),
            admitted.iter().copied().collect::<BTreeSet<_>>()
        );
        assert!(ca_x509.tbs_certificate.is_ca()); // CA is_ca true
        assert!(!leaf_x509.tbs_certificate.is_ca()); // leaf is_ca false

        // 2. WebPkiServerVerifier accepts admitted IPs
        let mut root_store = rustls::RootCertStore::empty();
        root_store
            .add(CertificateDer::from(ca_entries[0].contents().to_vec()))
            .unwrap();
        let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(root_store))
            .build()
            .unwrap();

        let end_entity = CertificateDer::from(leaf_cert_entry.contents().to_vec());

        for ip in &admitted {
            let server_name = rustls::pki_types::ServerName::try_from(ip.to_string()).unwrap();
            let verify_res = verifier.verify_server_cert(
                &end_entity,
                &[],
                &server_name,
                &[],
                rustls::pki_types::UnixTime::now(),
            );
            assert!(
                verify_res.is_ok(),
                "WebPkiServerVerifier failed for {ip}: {:?}",
                verify_res
            );
        }

        // 3. Unlisted IP yields InvalidCertificate (NotValidForName)
        let unlisted_name = rustls::pki_types::ServerName::try_from("192.0.2.1").unwrap();
        let unlisted_res = verifier.verify_server_cert(
            &end_entity,
            &[],
            &unlisted_name,
            &[],
            rustls::pki_types::UnixTime::now(),
        );
        assert!(
            matches!(
                unlisted_res,
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::NotValidForName
                )) | Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::NotValidForNameContext { .. }
                ))
            ),
            "Expected NotValidForName, got {:?}",
            unlisted_res
        );

        // 4. Unrelated CA fails chain building
        let unrelated_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut unrelated_params = CertificateParams::default();
        unrelated_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let unrelated_ca = unrelated_params.self_signed(&unrelated_key).unwrap();

        let mut unrelated_root_store = rustls::RootCertStore::empty();
        unrelated_root_store
            .add(CertificateDer::from(unrelated_ca.der().to_vec()))
            .unwrap();
        let unrelated_verifier =
            rustls::client::WebPkiServerVerifier::builder(Arc::new(unrelated_root_store))
                .build()
                .unwrap();

        let server_name_v4 = rustls::pki_types::ServerName::try_from(ip_v4.to_string()).unwrap();
        let unrelated_res = unrelated_verifier.verify_server_cert(
            &end_entity,
            &[],
            &server_name_v4,
            &[],
            rustls::pki_types::UnixTime::now(),
        );
        assert!(
            matches!(
                unrelated_res,
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::UnknownIssuer
                ))
            ),
            "Expected UnknownIssuer, got {:?}",
            unrelated_res
        );
    }

    #[tokio::test]
    async fn lan_door_control_loop_drop_uncovered_listener() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-uncovered-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();

        // Bind on 127.0.0.1
        let test_port = {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            l.local_addr().unwrap().port()
        };
        let listener = bind_admitted_address(IpAddr::V4(Ipv4Addr::LOCALHOST), test_port).unwrap();

        // Create identity with 192.168.1.50 only (NOT 127.0.0.1)
        let identity =
            reconcile_lan_identity(journal_root, &[IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50))]);
        let selected = identity.selected.unwrap();
        let server_config_cell = Arc::new(ArcSwap::from_pointee(Some(selected)));

        let oauth = Arc::new(OAuthRuntime::new_lan_door(journal_root));
        let sessions = Arc::new(SessionTable::new());
        let pool_semaphore = Arc::new(Semaphore::new(64));
        let source_counts = Arc::new(Mutex::new(HashMap::new()));
        let (stop_tx, stop_rx) = watch::channel(false);

        let mut run = LanDoorRun::production();
        run.port = test_port;
        run.bind_admitted = |_| true;
        run.peer_admitted = |_| true;

        let root_arc = Arc::new(journal_root.to_path_buf());
        let accept_task = tokio::spawn(accept_loop_for_address(
            listener,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            root_arc,
            oauth,
            sessions,
            server_config_cell,
            pool_semaphore,
            source_counts,
            run,
            stop_rx,
        ));

        // Connect client to 127.0.0.1:test_port
        use tokio::io::AsyncReadExt;
        let mut client_stream = tokio::net::TcpStream::connect(("127.0.0.1", test_port))
            .await
            .unwrap();
        let mut buf = [0u8; 10];
        let n = client_stream.read(&mut buf).await.unwrap();
        // Server immediately drops uncovered stream -> EOF (0 bytes read)
        assert_eq!(n, 0);

        let _ = stop_tx.send(true);
        let _ = accept_task.await;
    }

    #[tokio::test]
    async fn local_door_isolation_with_lan_door() {
        let temp = tempfile::Builder::new()
            .prefix("local-door-isolation-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal_root = temp.path();

        let config_dir = journal_root.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_json = serde_json::json!({
            "mcp_endpoint": {
                "lan_door": true,
                "local_door": true
            }
        });
        std::fs::write(
            config_dir.join("journal.json"),
            serde_json::to_vec_pretty(&config_json).unwrap(),
        )
        .unwrap();

        let guard = set_lan_publish_stop(LanPublishStep::BeforeCaKey);

        let test_port = {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let p = l.local_addr().unwrap().port();
            drop(l);
            p
        };

        let attempts_before = LAN_BIND_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst);

        let mut run = crate::local_door::LocalDoorRun::production();
        run.port = test_port;
        run.connection_permits = 256;
        run.config_interval = Duration::from_millis(50);
        run.rewrite_interval = Duration::from_millis(50);
        run.bind_retry_interval = Duration::from_millis(50);

        let root_buf = journal_root.to_path_buf();
        let local_handle = tokio::spawn(async move {
            let _ = crate::local_door::run_local_door_async(root_buf, run, None).await;
        });

        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(5) {
                panic!("timed out waiting for local door and lan door state");
            }
            tokio::time::sleep(Duration::from_millis(30)).await;

            let local_state = crate::local_door::read_local_door_state(journal_root);
            let lan_state = read_lan_door_state(journal_root);

            if let (Some(loc), Some(lan)) = (local_state, lan_state)
                && loc.listening
                && !lan.listening
                && lan.reason.as_deref() == Some("tls_unavailable")
            {
                break;
            }
        }

        let attempts_after = LAN_BIND_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(attempts_after - attempts_before, 0);

        drop(guard);
        crate::local_door::TEST_FORCE_LOCAL_SHUTDOWN
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = local_handle.await;
        crate::local_door::TEST_FORCE_LOCAL_SHUTDOWN
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod full_tests {
    use super::*;
    use std::fs;
    use std::net::Ipv4Addr;
    use std::sync::atomic::AtomicBool;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::rustls::pki_types::ServerName;

    use crate::tokens::TokenStore;
    use solstone_core_sol_link::pairing::addresses::RawInterfaceAddress;

    #[derive(Clone)]
    struct MockInterfaceSource {
        raw: Arc<Mutex<Result<Vec<RawInterfaceAddress>, String>>>,
    }

    impl MockInterfaceSource {
        fn new(addresses: Vec<RawInterfaceAddress>) -> Self {
            Self {
                raw: Arc::new(Mutex::new(Ok(addresses))),
            }
        }

        fn set_addresses(&self, addresses: Vec<RawInterfaceAddress>) {
            *self.raw.lock().unwrap() = Ok(addresses);
        }

        fn set_error(&self, error: &str) {
            *self.raw.lock().unwrap() = Err(error.to_string());
        }
    }

    impl RawInterfaceSource for MockInterfaceSource {
        fn enumerate(
            &self,
        ) -> Result<
            Vec<RawInterfaceAddress>,
            solstone_core_sol_link::pairing::addresses::AddressError,
        > {
            match self.raw.lock().unwrap().clone() {
                Ok(addrs) => Ok(addrs),
                Err(err) => Err(
                    solstone_core_sol_link::pairing::addresses::AddressError::Enumeration(
                        io::Error::other(err),
                    ),
                ),
            }
        }
    }

    fn write_config(journal: &Path, lan_door: Option<bool>) {
        let config_dir = journal.join("config");
        fs::create_dir_all(&config_dir).unwrap();
        let value = match lan_door {
            Some(true) => serde_json::json!({"mcp_endpoint": {"lan_door": true}}),
            Some(false) => serde_json::json!({"mcp_endpoint": {"lan_door": false}}),
            None => serde_json::json!({"mcp_endpoint": {"lan_door": "invalid"}}),
        };
        fs::write(
            config_dir.join("journal.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    #[derive(Debug)]
    struct NoVerify;

    impl rustls::client::danger::ServerCertVerifier for NoVerify {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            vec![
                rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
                rustls::SignatureScheme::ED25519,
                rustls::SignatureScheme::RSA_PSS_SHA256,
            ]
        }
    }

    #[tokio::test]
    async fn lan_door_control_loop_config_and_enumeration_transitions() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-ctrl-test-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal = temp.path();

        // 1. Initial config is off
        write_config(journal, Some(false));
        let mock_ifaces = MockInterfaceSource::new(vec![RawInterfaceAddress {
            interface: "eth0".to_string(),
            address: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
        }]);

        let mut run = LanDoorRun::production();
        run.config_interval = Duration::from_millis(50);
        run.enumeration_interval = Duration::from_millis(50);
        run.bind_retry_interval = Duration::from_millis(50);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let oauth = Arc::new(OAuthRuntime::new_lan_door(journal));
        let root_buf = journal.to_path_buf();
        let mock_clone = mock_ifaces.clone();

        let loop_handle = tokio::spawn(async move {
            let _ = run_lan_door_loop_with_sources(&root_buf, run, shutdown_rx, oauth, &mock_clone)
                .await;
        });

        // Wait for disabled state
        tokio::time::sleep(Duration::from_millis(150)).await;
        let state = read_lan_door_state(journal).unwrap();
        assert!(!state.listening);
        assert_eq!(state.reason.as_deref(), Some("disabled"));

        // 2. Switch to invalid config
        write_config(journal, None);
        tokio::time::sleep(Duration::from_millis(150)).await;
        let state = read_lan_door_state(journal).unwrap();
        assert!(!state.listening);
        assert_eq!(state.reason.as_deref(), Some("config_invalid"));

        // 3. Switch to no address
        write_config(journal, Some(true));
        mock_ifaces.set_addresses(vec![]);
        tokio::time::sleep(Duration::from_millis(150)).await;
        let state = read_lan_door_state(journal).unwrap();
        assert!(!state.listening);
        assert_eq!(state.reason.as_deref(), Some("no_address"));

        // 4. Enumeration failure with 0 listeners
        mock_ifaces.set_error("simulated interface failure");
        tokio::time::sleep(Duration::from_millis(150)).await;
        let state = read_lan_door_state(journal).unwrap();
        assert!(!state.listening);
        assert_eq!(state.reason.as_deref(), Some("enumeration_failed"));

        let _ = shutdown_tx.send(true);
        let _ = loop_handle.await;
    }

    #[tokio::test]
    async fn lan_door_control_loop_panic_recovery() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-panic-test-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal = temp.path();
        write_config(journal, Some(false));

        let mock_ifaces = MockInterfaceSource::new(vec![]);
        let panic_flag = Arc::new(AtomicBool::new(true));

        let mut run = LanDoorRun::production();
        run.config_interval = Duration::from_millis(50);
        run.bind_retry_interval = Duration::from_millis(50);
        run.one_shot_panic = Some(Arc::clone(&panic_flag));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let oauth = Arc::new(OAuthRuntime::new_lan_door(journal));
        let root_buf = journal.to_path_buf();
        let mock_clone = mock_ifaces.clone();

        let loop_handle = tokio::spawn(async move {
            let _ = run_lan_door_loop_with_sources(&root_buf, run, shutdown_rx, oauth, &mock_clone)
                .await;
        });

        // Wait for loop to panic, recover, and process subsequent pass
        tokio::time::sleep(Duration::from_millis(250)).await;
        let state = read_lan_door_state(journal).unwrap();
        assert!(!state.listening);
        assert_eq!(state.reason.as_deref(), Some("disabled"));

        let _ = shutdown_tx.send(true);
        let _ = loop_handle.await;
    }

    #[tokio::test]
    async fn lan_door_duplex_tls_handshake_and_ip_literal_guard() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-tls-duplex-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal = temp.path();

        let server_config = test_loopback_server_config();
        let tls_acceptor = TlsAcceptor::from(server_config);

        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify))
                .with_no_client_auth(),
        );
        let tls_connector = tokio_rustls::TlsConnector::from(client_config);

        let journal_root_arc = Arc::new(journal.to_path_buf());
        let oauth = Arc::new(OAuthRuntime::new_lan_door(journal));
        let sessions = Arc::new(SessionTable::new());

        // 1. Send request with valid IP literal host: 192.168.1.1:7660
        {
            let (client_io, server_io) = tokio::io::duplex(65536);
            let (_stop_tx, stop_rx) = watch::channel(false);
            let root_clone = Arc::clone(&journal_root_arc);
            let oauth_clone = Arc::clone(&oauth);
            let sessions_clone = Arc::clone(&sessions);
            let acceptor = tls_acceptor.clone();

            let server_task = tokio::spawn(async move {
                let tls_stream = acceptor.accept(server_io).await.unwrap();
                let _ = serve_stream(
                    tls_stream,
                    root_clone,
                    oauth_clone,
                    sessions_clone,
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
                    stop_rx,
                    RequestGuard::IpLiteral { port: 7660 },
                )
                .await;
            });

            let server_name = ServerName::try_from("192.168.1.1")
                .unwrap_or_else(|_| ServerName::try_from("localhost").unwrap());
            let mut client_tls = tls_connector.connect(server_name, client_io).await.unwrap();

            let req = b"POST /mcp HTTP/1.1\r\nHost: 192.168.1.1:7660\r\nContent-Length: 0\r\n\r\n";
            client_tls.write_all(req).await.unwrap();

            let mut buf = vec![0_u8; 1024];
            let n = client_tls.read(&mut buf).await.unwrap();
            let resp = String::from_utf8_lossy(&buf[..n]);
            assert!(resp.starts_with("HTTP/1.1 401 Unauthorized"), "got {resp}");
            assert!(
                resp.contains("https://192.168.1.1:7660/.well-known/oauth-protected-resource"),
                "got {resp}"
            );

            drop(client_tls);
            let _ = server_task.await;
        }

        // 2. Send request with invalid Host (domain name rejected by IP-literal guard)
        {
            let (client_io, server_io) = tokio::io::duplex(65536);
            let (_stop_tx, stop_rx) = watch::channel(false);
            let root_clone = Arc::clone(&journal_root_arc);
            let oauth_clone = Arc::clone(&oauth);
            let sessions_clone = Arc::clone(&sessions);
            let acceptor = tls_acceptor.clone();

            let server_task = tokio::spawn(async move {
                let tls_stream = acceptor.accept(server_io).await.unwrap();
                let _ = serve_stream(
                    tls_stream,
                    root_clone,
                    oauth_clone,
                    sessions_clone,
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
                    stop_rx,
                    RequestGuard::IpLiteral { port: 7660 },
                )
                .await;
            });

            let server_name = ServerName::try_from("journal.local")
                .unwrap_or_else(|_| ServerName::try_from("localhost").unwrap());
            let mut client_tls = tls_connector.connect(server_name, client_io).await.unwrap();

            let req2 =
                b"POST /mcp HTTP/1.1\r\nHost: journal.local:7660\r\nContent-Length: 0\r\n\r\n";
            client_tls.write_all(req2).await.unwrap();

            let mut buf = vec![0_u8; 1024];
            let n2 = client_tls.read(&mut buf).await.unwrap();
            let resp2 = String::from_utf8_lossy(&buf[..n2]);
            assert!(resp2.starts_with("HTTP/1.1 403 Forbidden"), "got {resp2}");

            drop(client_tls);
            let _ = server_task.await;
        }

        // 3. Static bearer token rejected on LAN door with 401 without burning token
        {
            let token_store = TokenStore::open(journal);
            let minted = token_store.create("test-bearer-agent").unwrap();

            let (client_io, server_io) = tokio::io::duplex(65536);
            let (_stop_tx, stop_rx) = watch::channel(false);
            let root_clone = Arc::clone(&journal_root_arc);
            let oauth_clone = Arc::clone(&oauth);
            let sessions_clone = Arc::clone(&sessions);
            let acceptor = tls_acceptor.clone();

            let server_task = tokio::spawn(async move {
                let tls_stream = acceptor.accept(server_io).await.unwrap();
                let _ = serve_stream(
                    tls_stream,
                    root_clone,
                    oauth_clone,
                    sessions_clone,
                    IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
                    stop_rx,
                    RequestGuard::IpLiteral { port: 7660 },
                )
                .await;
            });

            let server_name = ServerName::try_from("192.168.1.1")
                .unwrap_or_else(|_| ServerName::try_from("localhost").unwrap());
            let mut client_tls = tls_connector.connect(server_name, client_io).await.unwrap();

            let req = format!(
                "POST /mcp HTTP/1.1\r\nHost: 192.168.1.1:7660\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\n\r\n",
                minted.token
            );
            client_tls.write_all(req.as_bytes()).await.unwrap();

            let mut buf = vec![0_u8; 1024];
            let n = client_tls.read(&mut buf).await.unwrap();
            let resp = String::from_utf8_lossy(&buf[..n]);
            assert!(resp.starts_with("HTTP/1.1 401 Unauthorized"), "got {resp}");

            // Token is NOT burned or revoked
            let verified = token_store.verify(&minted.token);
            assert!(verified.is_ok(), "token should remain valid and unburned");

            drop(client_tls);
            let _ = server_task.await;
        }
    }

    use crate::oauth::store::OAuthStore;
    use crate::oauth::urlparse::query_value_encode;
    use base64::Engine as _;
    use rusqlite::params;
    use serde_json::{Value, json};
    use solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE;
    use std::net::Ipv6Addr;

    const OAUTH_VERIFIER: &str =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

    fn pkce_challenge() -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(OAUTH_VERIFIER.as_bytes()))
    }

    fn hidden_transaction_id(body: &str) -> String {
        let marker = "name=\"transaction_id\" value=\"";
        let start = body.find(marker).expect("hidden transaction_id") + marker.len();
        let end = start + body[start..].find('"').expect("value terminator");
        body[start..end].to_owned()
    }

    fn location_param(location: &str, name: &str) -> String {
        let query = location.split_once('?').expect("redirect has query").1;
        let prefix = format!("{name}=");
        query
            .split('&')
            .find_map(|piece| piece.strip_prefix(&prefix))
            .expect("parameter present")
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    }

    fn seed_indexed_note(journal: &Path) {
        let path = "20260831/default/123456_1/talents/brief.md";
        let source = journal.join("chronicle/20260831/default/123456_1");
        fs::create_dir_all(source.join("talents")).expect("fixture segment directory");
        fs::write(source.join("talents/brief.md"), "disk fixture source")
            .expect("fixture source content");
        fs::write(source.join("talents/facets.json"), "[]").expect("fixture assignments");
        let connection =
            solstone_core_indexer_store::db::open_index(journal).expect("fixture index opens");
        connection
            .execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?1, ?2, '20260831', '', 'fixture', 'default', 0, '')",
                params!["MCP fixture search needle", path],
            )
            .expect("fixture chunk inserts");
        connection
            .execute(
                "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', 1, 1)",
                [],
            )
            .expect("fixture index completes");
        connection
            .execute(
                "INSERT INTO chunk_classification(path, category, basis, eligible, unclassified) VALUES (?1, 'transcripts', 'segment_assigned', 1, 0)",
                params![path],
            )
            .expect("fixture classification inserts");
        connection
            .execute(
                "REPLACE INTO chunk_classification_backfill(id, cursor, completed, stalled, stalled_path, resume_count) VALUES (1, '', 1, 0, NULL, 0)",
                [],
            )
            .expect("fixture classification completes");
    }

    async fn exchange_tls_http(port: u16, raw_request: &str) -> (u16, Vec<u8>, String) {
        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify))
                .with_no_client_auth(),
        );
        let tls_connector = tokio_rustls::TlsConnector::from(client_config);
        let tcp_stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let server_name = ServerName::try_from("127.0.0.1")
            .unwrap_or_else(|_| ServerName::try_from("localhost").unwrap());
        let mut client_tls = tls_connector
            .connect(server_name, tcp_stream)
            .await
            .unwrap();

        client_tls.write_all(raw_request.as_bytes()).await.unwrap();

        let mut head = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            client_tls.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            if head.len() > 65536 {
                panic!("response headers too large");
            }
        }
        let head_str = String::from_utf8_lossy(&head).to_string();
        let status = head_str
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let content_length = head_str
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("content-length:") {
                    Some(
                        line.split_once(':')
                            .unwrap()
                            .1
                            .trim()
                            .parse::<usize>()
                            .unwrap(),
                    )
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let mut body = vec![0_u8; content_length];
        if content_length > 0 {
            client_tls.read_exact(&mut body).await.unwrap();
        }
        (status, body, head_str)
    }

    async fn exchange_tls_http_v6(port: u16, raw_request: &str) -> (u16, Vec<u8>, String) {
        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify))
                .with_no_client_auth(),
        );
        let tls_connector = tokio_rustls::TlsConnector::from(client_config);
        let tcp_stream = tokio::net::TcpStream::connect(("::1", port)).await.unwrap();
        let server_name = ServerName::try_from("localhost").unwrap();
        let mut client_tls = tls_connector
            .connect(server_name, tcp_stream)
            .await
            .unwrap();

        client_tls.write_all(raw_request.as_bytes()).await.unwrap();

        let mut head = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            client_tls.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            if head.len() > 65536 {
                panic!("response headers too large");
            }
        }
        let head_str = String::from_utf8_lossy(&head).to_string();
        let status = head_str
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let content_length = head_str
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("content-length:") {
                    Some(
                        line.split_once(':')
                            .unwrap()
                            .1
                            .trim()
                            .parse::<usize>()
                            .unwrap(),
                    )
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let mut body = vec![0_u8; content_length];
        if content_length > 0 {
            client_tls.read_exact(&mut body).await.unwrap();
        }
        (status, body, head_str)
    }

    fn test_admit_loopback_bind(endpoint: &LocalEndpoint) -> bool {
        endpoint.ip == IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
            || endpoint.ip == IpAddr::V6(Ipv6Addr::LOCALHOST)
            || is_admitted_lan_bind_endpoint(endpoint)
    }

    fn test_admit_loopback_peer(ip: IpAddr) -> bool {
        ip == IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
            || ip == IpAddr::V6(Ipv6Addr::LOCALHOST)
            || is_admitted_lan_peer(ip)
    }

    #[tokio::test]
    async fn lan_door_loopback_injected_live_listener_lifecycle() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-live-test-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal = temp.path();

        // Find an ephemeral port
        let test_port = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };

        let listener = bind_admitted_address(IpAddr::V4(Ipv4Addr::LOCALHOST), test_port).unwrap();
        let selected = test_loopback_selected_identity(&[IpAddr::V4(Ipv4Addr::LOCALHOST)]);
        let server_config_cell = Arc::new(ArcSwap::from_pointee(Some(selected)));
        let oauth = Arc::new(OAuthRuntime::new_lan_door(journal));
        let sessions = Arc::new(SessionTable::new());
        let pool_semaphore = Arc::new(Semaphore::new(64));
        let source_counts = Arc::new(Mutex::new(HashMap::new()));
        let (stop_tx, stop_rx) = watch::channel(false);

        let mut run = LanDoorRun::production();
        run.port = test_port;
        run.bind_admitted = test_admit_loopback_bind;
        run.peer_admitted = test_admit_loopback_peer;

        let root_arc = Arc::new(journal.to_path_buf());
        let accept_task = tokio::spawn(accept_loop_for_address(
            listener,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            root_arc,
            oauth,
            sessions,
            server_config_cell,
            pool_semaphore,
            source_counts,
            run,
            stop_rx,
        ));

        // Connect over TLS
        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify))
                .with_no_client_auth(),
        );
        let tls_connector = tokio_rustls::TlsConnector::from(client_config);
        let tcp_stream = tokio::net::TcpStream::connect(("127.0.0.1", test_port))
            .await
            .unwrap();

        let server_name = ServerName::try_from("127.0.0.1")
            .unwrap_or_else(|_| ServerName::try_from("localhost").unwrap());
        let mut client_tls = tls_connector
            .connect(server_name, tcp_stream)
            .await
            .unwrap();

        let req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{test_port}\r\nContent-Length: 0\r\n\r\n"
        );
        client_tls.write_all(req.as_bytes()).await.unwrap();

        let mut buf = vec![0_u8; 1024];
        let n = client_tls.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 401 Unauthorized"), "got {resp}");
        assert!(
            resp.contains(&format!(
                "https://127.0.0.1:{test_port}/.well-known/oauth-protected-resource"
            )),
            "got {resp}"
        );

        drop(client_tls);

        // Shutdown and verify listener task abort and termination
        let _ = stop_tx.send(true);
        accept_task.abort();
        let res = accept_task.await;
        assert!(res.is_err() && res.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn lan_door_oauth_end_to_end_flow() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-oauth-e2e-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal = temp.path();
        write_config(journal, Some(true));
        seed_indexed_note(journal);

        let port = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let p = listener.local_addr().unwrap().port();
            drop(listener);
            p
        };

        let listener = bind_admitted_address(IpAddr::V4(Ipv4Addr::LOCALHOST), port).unwrap();
        let selected = test_loopback_selected_identity(&[IpAddr::V4(Ipv4Addr::LOCALHOST)]);
        let server_config_cell = Arc::new(ArcSwap::from_pointee(Some(selected)));
        let oauth = Arc::new(OAuthRuntime::new_lan_door(journal));
        let sessions = Arc::new(SessionTable::new());
        let pool_semaphore = Arc::new(Semaphore::new(64));
        let source_counts = Arc::new(Mutex::new(HashMap::new()));
        let (stop_tx, stop_rx) = watch::channel(false);

        let mut run = LanDoorRun::production();
        run.port = port;
        run.bind_admitted = test_admit_loopback_bind;
        run.peer_admitted = test_admit_loopback_peer;

        let root_arc = Arc::new(journal.to_path_buf());
        let accept_task = tokio::spawn(accept_loop_for_address(
            listener,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            root_arc,
            oauth,
            sessions,
            server_config_cell,
            pool_semaphore,
            source_counts,
            run,
            stop_rx,
        ));

        let origin = format!("https://127.0.0.1:{port}");

        // 1. Unauthenticated POST /mcp -> 401 with resource_metadata
        let (status, _, head) = exchange_tls_http(
            port,
            &format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"),
        )
        .await;
        assert_eq!(status, 401);
        let auth_hdr = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("www-authenticate:"))
            .unwrap();
        assert!(auth_hdr.contains(&format!(
            "resource_metadata=\"{origin}/.well-known/oauth-protected-resource\""
        )));

        // 2. GET /.well-known/oauth-protected-resource
        let (status, body, _) = exchange_tls_http(
            port,
            &format!("GET /.well-known/oauth-protected-resource HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let pr: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(pr["resource"], format!("{origin}/mcp"));
        assert_eq!(pr["authorization_servers"], json!([origin]));

        // 3. GET /.well-known/oauth-authorization-server
        let (status, body, _) = exchange_tls_http(
            port,
            &format!("GET /.well-known/oauth-authorization-server HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let as_meta: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(as_meta["issuer"], origin);

        // 4. DCR: POST /register
        let reg_payload = serde_json::to_vec(&json!({
            "redirect_uris": ["http://localhost:12345/callback"],
            "token_endpoint_auth_method": "none",
            "client_name": "LAN Test",
        }))
        .unwrap();
        let (status, body, _) = exchange_tls_http(
            port,
            &format!(
                "POST /register HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                reg_payload.len(),
                std::str::from_utf8(&reg_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 201);
        let reg_res: Value = serde_json::from_slice(&body).unwrap();
        let client_id = reg_res["client_id"].as_str().unwrap().to_owned();
        assert!(client_id.starts_with("oauth:dcr:"));

        // 5. GET /authorize
        let challenge = pkce_challenge();
        let query = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&resource={}/mcp&state=st_lan",
            query_value_encode(&client_id),
            query_value_encode("http://localhost:12345/callback"),
            query_value_encode(&challenge),
            query_value_encode(&origin),
        );
        let (status, body, _) = exchange_tls_http(
            port,
            &format!("GET /authorize?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let html = String::from_utf8(body).unwrap();
        let tx_id = hidden_transaction_id(&html);

        // 6. POST /authorize with pairing code
        let store = OAuthStore::open(journal);
        let pairing = store
            .generate_pairing_code_with_door(Some(MCP_LAN_DOOR_RESOURCE))
            .unwrap();
        let auth_form = format!(
            "transaction_id={}&pairing_code={}&scope=whole_journal&category=transcripts",
            query_value_encode(&tx_id),
            query_value_encode(&pairing.code),
        );
        let (status, _, head) = exchange_tls_http(
            port,
            &format!(
                "POST /authorize HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{auth_form}",
                auth_form.len()
            ),
        )
        .await;
        assert_eq!(status, 302);
        let location = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("location:"))
            .unwrap();
        assert!(location.contains(&format!("iss={}", query_value_encode(&origin))));
        let code = location_param(location, "code");

        // 7. POST /token exchange
        let token_form = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}&resource={}/mcp",
            query_value_encode(&code),
            query_value_encode("http://localhost:12345/callback"),
            query_value_encode(&client_id),
            query_value_encode(OAUTH_VERIFIER),
            query_value_encode(&origin),
        );
        let (status, body, _) = exchange_tls_http(
            port,
            &format!(
                "POST /token HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{token_form}",
                token_form.len()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let token_res: Value = serde_json::from_slice(&body).unwrap();
        let _active_access_token = token_res["access_token"].as_str().unwrap().to_owned();
        let active_refresh_token = token_res["refresh_token"].as_str().unwrap().to_owned();

        // 8. POST /token refresh
        let refresh_form = format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}&resource={}/mcp",
            query_value_encode(&active_refresh_token),
            query_value_encode(&client_id),
            query_value_encode(&origin),
        );
        let (status, body, _) = exchange_tls_http(
            port,
            &format!(
                "POST /token HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{refresh_form}",
                refresh_form.len()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let refreshed: Value = serde_json::from_slice(&body).unwrap();
        let refreshed_access_token = refreshed["access_token"].as_str().unwrap().to_owned();

        // 9. POST /mcp tools/list carries readOnlyHint
        let list_payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .unwrap();
        let (status, body, _) = exchange_tls_http(
            port,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {refreshed_access_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                list_payload.len(),
                std::str::from_utf8(&list_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let list_res: Value = serde_json::from_slice(&body).unwrap();
        let tools = list_res["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        for tool in tools {
            let read_only = tool.get("readOnlyHint") == Some(&Value::Bool(true))
                || tool.get("annotations").and_then(|a| a.get("readOnlyHint"))
                    == Some(&Value::Bool(true));
            assert!(read_only, "tool {} must declare readOnlyHint", tool["name"]);
        }

        // 10. POST /mcp tools/call search -> writes admission and outcome
        let call_payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "search",
                "arguments": {"query": "needle", "limit": 1}
            }
        }))
        .unwrap();
        let (status, body, _) = exchange_tls_http(
            port,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {refreshed_access_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                call_payload.len(),
                std::str::from_utf8(&call_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let call_res: Value = serde_json::from_slice(&body).unwrap();
        let content = call_res["result"]["content"].as_array().unwrap();
        assert!(!content.is_empty());

        let audit_count = fs::read_dir(journal.join("chronicle"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .flat_map(|day| {
                fs::read_dir(day.path())
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
            })
            .flat_map(|agent| {
                fs::read_dir(agent.path())
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
            })
            .filter(|segment| {
                segment
                    .path()
                    .join("interaction/interaction.json")
                    .is_file()
                    || segment.path().join("interaction.json").is_file()
            })
            .count();
        assert!(audit_count >= 1);

        // 11. Verify stored grant resource is MCP_LAN_DOOR_RESOURCE
        let raw_store = fs::read_to_string(journal.join("mcp-endpoint/oauth.json")).unwrap();
        let store_val: Value = serde_json::from_str(&raw_store).unwrap();
        let grant = &store_val["grants"][0];
        assert_eq!(
            grant["resource"].as_str(),
            Some(MCP_LAN_DOOR_RESOURCE),
            "LAN door grant must be stored with canonical MCP_LAN_DOOR_RESOURCE"
        );

        let _ = stop_tx.send(true);
        accept_task.abort();
        let _ = accept_task.await;
    }

    #[tokio::test]
    async fn lan_door_dual_host_authorization_and_host_matching() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-dual-host-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal = temp.path();
        write_config(journal, Some(true));
        seed_indexed_note(journal);

        let port_v4 = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let p = listener.local_addr().unwrap().port();
            drop(listener);
            p
        };
        let port_v6 = {
            let listener = std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
                .expect("fail the test if [::1] cannot be bound");
            let p = listener.local_addr().unwrap().port();
            drop(listener);
            p
        };

        let listener_v4 = bind_admitted_address(IpAddr::V4(Ipv4Addr::LOCALHOST), port_v4).unwrap();
        let listener_v6 = bind_admitted_address(IpAddr::V6(Ipv6Addr::LOCALHOST), port_v6)
            .expect("fail the test if [::1] cannot be bound");

        let selected = test_loopback_selected_identity(&[
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ]);
        let server_config_cell = Arc::new(ArcSwap::from_pointee(Some(selected)));
        let oauth = Arc::new(OAuthRuntime::new_lan_door(journal));
        let sessions = Arc::new(SessionTable::new());
        let pool_semaphore = Arc::new(Semaphore::new(64));
        let source_counts = Arc::new(Mutex::new(HashMap::new()));
        let (stop_tx, stop_rx) = watch::channel(false);

        let mut run_v4 = LanDoorRun::production();
        run_v4.port = port_v4;
        run_v4.bind_admitted = test_admit_loopback_bind;
        run_v4.peer_admitted = test_admit_loopback_peer;

        let mut run_v6 = LanDoorRun::production();
        run_v6.port = port_v6;
        run_v6.bind_admitted = test_admit_loopback_bind;
        run_v6.peer_admitted = test_admit_loopback_peer;

        let root_arc = Arc::new(journal.to_path_buf());
        let accept_task_v4 = tokio::spawn(accept_loop_for_address(
            listener_v4,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            Arc::clone(&root_arc),
            Arc::clone(&oauth),
            Arc::clone(&sessions),
            Arc::clone(&server_config_cell),
            Arc::clone(&pool_semaphore),
            Arc::clone(&source_counts),
            run_v4,
            stop_rx.clone(),
        ));
        let accept_task_v6 = tokio::spawn(accept_loop_for_address(
            listener_v6,
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            Arc::clone(&root_arc),
            Arc::clone(&oauth),
            Arc::clone(&sessions),
            Arc::clone(&server_config_cell),
            Arc::clone(&pool_semaphore),
            Arc::clone(&source_counts),
            run_v6,
            stop_rx,
        ));

        let origin_v4 = format!("https://127.0.0.1:{port_v4}");
        let origin_v6 = format!("https://[::1]:{port_v6}");

        // Register client on v6
        let reg_payload = serde_json::to_vec(&json!({
            "redirect_uris": ["http://localhost:12345/callback"],
            "token_endpoint_auth_method": "none",
            "client_name": "Dual Host Test",
        }))
        .unwrap();
        let (status, body, _) = exchange_tls_http_v6(
            port_v6,
            &format!(
                "POST /register HTTP/1.1\r\nHost: [::1]:{port_v6}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                reg_payload.len(),
                std::str::from_utf8(&reg_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 201);
        let client_id = serde_json::from_slice::<Value>(&body).unwrap()["client_id"]
            .as_str()
            .unwrap()
            .to_owned();

        // Authorize with Host: [::1]:port_v6
        let challenge = pkce_challenge();
        let query = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&resource={}/mcp&state=st_dual",
            query_value_encode(&client_id),
            query_value_encode("http://localhost:12345/callback"),
            query_value_encode(&challenge),
            query_value_encode(&origin_v6),
        );
        let (status, body, _) = exchange_tls_http_v6(
            port_v6,
            &format!("GET /authorize?{query} HTTP/1.1\r\nHost: [::1]:{port_v6}\r\n\r\n"),
        )
        .await;
        assert_eq!(status, 200);
        let tx_id = hidden_transaction_id(&String::from_utf8(body).unwrap());

        let store = OAuthStore::open(journal);
        let pairing = store
            .generate_pairing_code_with_door(Some(MCP_LAN_DOOR_RESOURCE))
            .unwrap();
        let auth_form = format!(
            "transaction_id={}&pairing_code={}&scope=whole_journal&category=transcripts",
            query_value_encode(&tx_id),
            query_value_encode(&pairing.code),
        );
        let (status, _, head) = exchange_tls_http_v6(
            port_v6,
            &format!(
                "POST /authorize HTTP/1.1\r\nHost: [::1]:{port_v6}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{auth_form}",
                auth_form.len()
            ),
        )
        .await;
        assert_eq!(status, 302);
        let location = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("location:"))
            .unwrap();
        let code = location_param(location, "code");

        // Exchange code at /token with Host: 127.0.0.1:port_v4 and matching resource -> succeeds!
        let token_form = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}&resource={}/mcp",
            query_value_encode(&code),
            query_value_encode("http://localhost:12345/callback"),
            query_value_encode(&client_id),
            query_value_encode(OAUTH_VERIFIER),
            query_value_encode(&origin_v4),
        );
        let (status, body, _) = exchange_tls_http(
            port_v4,
            &format!(
                "POST /token HTTP/1.1\r\nHost: 127.0.0.1:{port_v4}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{token_form}",
                token_form.len()
            ),
        )
        .await;
        assert_eq!(status, 200);
        let access_token = serde_json::from_slice::<Value>(&body).unwrap()["access_token"]
            .as_str()
            .unwrap()
            .to_owned();

        // Access token answers tools/call on 127.0.0.1
        let call_payload = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "search", "arguments": {"query": "needle", "limit": 1}}
        }))
        .unwrap();
        let (status, _, _) = exchange_tls_http(
            port_v4,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port_v4}\r\nAuthorization: Bearer {access_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                call_payload.len(),
                std::str::from_utf8(&call_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200);

        // Access token answers tools/call on [::1]
        let (status, _, _) = exchange_tls_http_v6(
            port_v6,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: [::1]:{port_v6}\r\nAuthorization: Bearer {access_token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                call_payload.len(),
                std::str::from_utf8(&call_payload).unwrap()
            ),
        )
        .await;
        assert_eq!(status, 200);

        // A /token request with resource origin differing from request's Host is invalid_request
        let mismatched_token_form = format!(
            "grant_type=authorization_code&code=dummy&redirect_uri={}&client_id={}&code_verifier={}&resource=https://192.168.1.1:7660/mcp",
            query_value_encode("http://localhost:12345/callback"),
            query_value_encode(&client_id),
            query_value_encode(OAUTH_VERIFIER),
        );
        let (status, body, _) = exchange_tls_http(
            port_v4,
            &format!(
                "POST /token HTTP/1.1\r\nHost: 127.0.0.1:{port_v4}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{mismatched_token_form}",
                mismatched_token_form.len()
            ),
        )
        .await;
        assert_eq!(status, 400);
        let err_json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(err_json["error"], "invalid_request");

        let _ = stop_tx.send(true);
        accept_task_v4.abort();
        accept_task_v6.abort();
        let _ = accept_task_v4.await;
        let _ = accept_task_v6.await;
    }

    #[tokio::test]
    async fn lan_door_token_isolation_and_static_bearer_refusal() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-isolation-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal = temp.path();

        let store = OAuthStore::open(journal);
        let client = store
            .register_client(
                "https://client.example/app.json",
                vec!["http://localhost/cb".to_string()],
                None,
                "127.0.0.1",
            )
            .unwrap();

        let lan_runtime = OAuthRuntime::new_lan_door(journal);
        let local_runtime = OAuthRuntime::new_bound(journal, "http://127.0.0.1:7659".to_string());
        let unbound_runtime = OAuthRuntime::new(journal, "https://solstone.me".to_string());

        let challenge = pkce_challenge();

        // 1. LAN grant with resource MCP_LAN_DOOR_RESOURCE
        let tx_lan = store
            .create_transaction(
                &client.id,
                "http://localhost/cb",
                MCP_LAN_DOOR_RESOURCE,
                "https://127.0.0.1:7660",
                &challenge,
                "S256",
                Some("state-lan"),
                "127.0.0.1",
            )
            .unwrap();
        let p_lan = store
            .generate_pairing_code_with_door(Some(MCP_LAN_DOOR_RESOURCE))
            .unwrap();
        let auth_lan = store
            .complete_pairing_with_permission(&tx_lan, &p_lan.code, None, &lan_runtime.binding())
            .unwrap();
        let tokens_lan = store
            .redeem_authorization_code(
                &auth_lan.code,
                &client.client_id,
                "http://localhost/cb",
                MCP_LAN_DOOR_RESOURCE,
                OAUTH_VERIFIER,
                &lan_runtime.binding(),
            )
            .unwrap();

        // 2. Local-door grant with resource bound to 127.0.0.1:7659
        let tx_local = store
            .create_transaction(
                &client.id,
                "http://localhost/cb",
                "http://127.0.0.1:7659/mcp",
                "http://127.0.0.1:7659",
                &challenge,
                "S256",
                Some("state-local"),
                "127.0.0.1",
            )
            .unwrap();
        let p_local = store.generate_pairing_code_with_door(None).unwrap();
        let auth_local = store
            .complete_pairing_with_permission(
                &tx_local,
                &p_local.code,
                None,
                &local_runtime.binding(),
            )
            .unwrap();
        let tokens_local = store
            .redeem_authorization_code(
                &auth_local.code,
                &client.client_id,
                "http://localhost/cb",
                "http://127.0.0.1:7659/mcp",
                OAUTH_VERIFIER,
                &local_runtime.binding(),
            )
            .unwrap();

        // 3. Unbound solstone.me grant
        let tx_unbound = store
            .create_transaction(
                &client.id,
                "http://localhost/cb",
                "https://solstone.me/mcp",
                "https://solstone.me",
                &challenge,
                "S256",
                Some("state-unbound"),
                "127.0.0.1",
            )
            .unwrap();
        let p_unbound = store.generate_pairing_code_with_door(None).unwrap();
        let auth_unbound = store
            .complete_pairing_with_permission(
                &tx_unbound,
                &p_unbound.code,
                None,
                &unbound_runtime.binding(),
            )
            .unwrap();
        let tokens_unbound = store
            .redeem_authorization_code(
                &auth_unbound.code,
                &client.client_id,
                "http://localhost/cb",
                "https://solstone.me/mcp",
                OAUTH_VERIFIER,
                &unbound_runtime.binding(),
            )
            .unwrap();

        // LAN token is valid on LAN runtime, but 401 at local-door runtime and at unbound runtime
        assert!(
            store
                .verify_access_token(&tokens_lan.access_token, &lan_runtime.binding())
                .is_ok()
        );
        assert!(
            store
                .verify_access_token(&tokens_lan.access_token, &local_runtime.binding())
                .is_err()
        );
        assert!(
            store
                .verify_access_token(&tokens_lan.access_token, &unbound_runtime.binding())
                .is_err()
        );

        // Local-door token is valid on local runtime, but 401 at LAN door runtime and at unbound runtime
        assert!(
            store
                .verify_access_token(&tokens_local.access_token, &local_runtime.binding())
                .is_ok()
        );
        assert!(
            store
                .verify_access_token(&tokens_local.access_token, &lan_runtime.binding())
                .is_err()
        );
        assert!(
            store
                .verify_access_token(&tokens_local.access_token, &unbound_runtime.binding())
                .is_err()
        );

        // Unbound token is valid on unbound runtime, but 401 at LAN door runtime and at local door runtime
        assert!(
            store
                .verify_access_token(&tokens_unbound.access_token, &unbound_runtime.binding())
                .is_ok()
        );
        assert!(
            store
                .verify_access_token(&tokens_unbound.access_token, &lan_runtime.binding())
                .is_err()
        );
        assert!(
            store
                .verify_access_token(&tokens_unbound.access_token, &local_runtime.binding())
                .is_err()
        );

        // Static bearer token
        let token_store = TokenStore::open(journal);
        let minted = token_store.create("test-static-isolation-agent").unwrap();

        let port = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let p = listener.local_addr().unwrap().port();
            drop(listener);
            p
        };
        let listener = bind_admitted_address(IpAddr::V4(Ipv4Addr::LOCALHOST), port).unwrap();
        let selected = test_loopback_selected_identity(&[IpAddr::V4(Ipv4Addr::LOCALHOST)]);
        let server_config_cell = Arc::new(ArcSwap::from_pointee(Some(selected)));
        let oauth_arc = Arc::new(lan_runtime);
        let sessions = Arc::new(SessionTable::new());
        let pool_semaphore = Arc::new(Semaphore::new(64));
        let source_counts = Arc::new(Mutex::new(HashMap::new()));
        let (stop_tx, stop_rx) = watch::channel(false);

        let mut run = LanDoorRun::production();
        run.port = port;
        run.bind_admitted = test_admit_loopback_bind;
        run.peer_admitted = test_admit_loopback_peer;

        let root_arc = Arc::new(journal.to_path_buf());
        let accept_task = tokio::spawn(accept_loop_for_address(
            listener,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            root_arc,
            oauth_arc,
            sessions,
            server_config_cell,
            pool_semaphore,
            source_counts,
            run,
            stop_rx,
        ));

        // Call LAN door with static bearer -> 401
        let (status, _, _) = exchange_tls_http(
            port,
            &format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\n\r\n", minted.token),
        )
        .await;
        assert_eq!(status, 401);

        // Still verifies in TokenStore (unburned)
        assert!(token_store.verify(&minted.token).is_ok());

        // Writes no activity row
        let chronicle_dir = journal.join("chronicle");
        assert!(!chronicle_dir.exists() || fs::read_dir(&chronicle_dir).unwrap().next().is_none());

        let _ = stop_tx.send(true);
        accept_task.abort();
        let _ = accept_task.await;
    }

    #[tokio::test]
    async fn lan_door_pairing_code_door_isolation() {
        let temp = tempfile::Builder::new()
            .prefix("lan-door-pairing-isolation-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let journal = temp.path();

        let store = OAuthStore::open(journal);
        let client = store
            .register_client(
                "https://client.example/app.json",
                vec!["http://localhost/cb".to_string()],
                None,
                "127.0.0.1",
            )
            .unwrap();

        let lan_runtime = OAuthRuntime::new_lan_door(journal);
        let local_runtime = OAuthRuntime::new_bound(journal, "http://127.0.0.1:7659".to_string());
        let unbound_runtime = OAuthRuntime::new(journal, "https://solstone.me".to_string());

        let challenge = pkce_challenge();

        // 1. Pairing code stored with door MCP_LAN_DOOR_RESOURCE
        let pairing_lan = store
            .generate_pairing_code_with_door(Some(MCP_LAN_DOOR_RESOURCE))
            .unwrap();

        let tx1 = store
            .create_transaction(
                &client.id,
                "http://localhost/cb",
                "http://127.0.0.1:7659/mcp",
                "http://127.0.0.1:7659",
                &challenge,
                "S256",
                None,
                "127.0.0.1",
            )
            .unwrap();
        // Fails on local door
        assert!(matches!(
            store.complete_pairing_with_permission(
                &tx1,
                &pairing_lan.code,
                None,
                &local_runtime.binding(),
            ),
            Err(crate::oauth::store::OAuthStoreError::PairingMismatch)
        ));

        let tx2 = store
            .create_transaction(
                &client.id,
                "http://localhost/cb",
                "https://solstone.me/mcp",
                "https://solstone.me",
                &challenge,
                "S256",
                None,
                "127.0.0.1",
            )
            .unwrap();
        // Fails on unbound solstone.me runtime
        assert!(matches!(
            store.complete_pairing_with_permission(
                &tx2,
                &pairing_lan.code,
                None,
                &unbound_runtime.binding(),
            ),
            Err(crate::oauth::store::OAuthStoreError::PairingMismatch)
        ));

        let tx3 = store
            .create_transaction(
                &client.id,
                "http://localhost/cb",
                MCP_LAN_DOOR_RESOURCE,
                "https://127.0.0.1:7660",
                &challenge,
                "S256",
                None,
                "127.0.0.1",
            )
            .unwrap();
        // Completes on LAN runtime
        assert!(
            store
                .complete_pairing_with_permission(
                    &tx3,
                    &pairing_lan.code,
                    None,
                    &lan_runtime.binding(),
                )
                .is_ok()
        );

        // 2. Unbound pairing code
        let pairing_unbound = store.generate_pairing_code_with_door(None).unwrap();

        // Unbound oauth.json has no "door" key (null in JSON)
        let store_json: Value = serde_json::from_str(
            &fs::read_to_string(journal.join("mcp-endpoint/oauth.json")).unwrap(),
        )
        .unwrap();
        assert!(store_json["pairing"]["door"].is_null());

        let tx4 = store
            .create_transaction(
                &client.id,
                "http://localhost/cb",
                MCP_LAN_DOOR_RESOURCE,
                "https://127.0.0.1:7660",
                &challenge,
                "S256",
                None,
                "127.0.0.1",
            )
            .unwrap();
        // Unbound pairing fails on LAN runtime
        assert!(matches!(
            store.complete_pairing_with_permission(
                &tx4,
                &pairing_unbound.code,
                None,
                &lan_runtime.binding(),
            ),
            Err(crate::oauth::store::OAuthStoreError::PairingMismatch)
        ));

        let tx5 = store
            .create_transaction(
                &client.id,
                "http://localhost/cb",
                "http://127.0.0.1:7659/mcp",
                "http://127.0.0.1:7659",
                &challenge,
                "S256",
                None,
                "127.0.0.1",
            )
            .unwrap();
        // Unbound pairing completes on local door
        assert!(
            store
                .complete_pairing_with_permission(
                    &tx5,
                    &pairing_unbound.code,
                    None,
                    &local_runtime.binding(),
                )
                .is_ok()
        );
    }
}
