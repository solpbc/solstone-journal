// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! BYO owner-hostname DNS evaluation and RFC 8659 CAA policy checking with DNSSEC.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use futures::StreamExt;
use futures::stream::Stream;
use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};

use hickory_net::runtime::TokioRuntimeProvider;
use hickory_net::xfer::DnsHandle;
use hickory_net::{DnsError, NetError};
use hickory_proto::dnssec::Proof;
use hickory_proto::dnssec::rdata::DNSSECRData;
use hickory_proto::op::{
    DnsRequest, DnsRequestOptions, DnsResponse, Message, MessageType, OpCode, Query, ResponseCode,
};
use hickory_proto::rr::{Name, RData, Record, RecordType, SerialNumber};
use hickory_proto::serialize::binary::{BinDecodable, BinDecoder};
use hickory_resolver::config::{NameServerConfig, ProtocolConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::net::dnssec::DnssecDnsHandle;
use hickory_resolver::system_conf::read_system_conf;
use hickory_resolver::{ConnectionProvider, NameServerPool, PoolContext, TlsConfig};

/// DNS lookup outcome codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsVerdictCode {
    Unchecked,
    Admitted,
    Cname,
    NoAddress,
    CaaMissing,
    Issuewild,
    ExtraIssue,
    OtherCa,
    AccountUri,
    ValidationMethod,
    LookupError,
    LookupTimeout,
    DnssecBogus,
    SignatureOutsideValidityAtLocalTime,
}

impl DnsVerdictCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchecked => "unchecked",
            Self::Admitted => "admitted",
            Self::Cname => "cname",
            Self::NoAddress => "no_address",
            Self::CaaMissing => "caa_missing",
            Self::Issuewild => "issuewild",
            Self::ExtraIssue => "extra_issue",
            Self::OtherCa => "other_ca",
            Self::AccountUri => "account_uri",
            Self::ValidationMethod => "validation_method",
            Self::LookupError => "lookup_error",
            Self::LookupTimeout => "lookup_timeout",
            Self::DnssecBogus => "dnssec_bogus",
            Self::SignatureOutsideValidityAtLocalTime => {
                "dnssec_bogus signature_outside_validity_at_local_time"
            }
        }
    }
}

/// Parsed CAA record entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaaRecord {
    pub flags: u8,
    pub tag: String,
    pub value: String,
}

/// Collection of DNS records for a hostname.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostDnsRecords {
    pub a: Vec<Ipv4Addr>,
    pub aaaa: Vec<Ipv6Addr>,
    pub cname: Vec<String>,
    pub caa: Vec<CaaRecord>,
}

/// The evaluated verdict of DNS and CAA policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsVerdict {
    pub code: DnsVerdictCode,
    pub observed_at: DateTime<Utc>,
}

impl DnsVerdict {
    #[must_use]
    pub fn new(code: DnsVerdictCode, observed_at: DateTime<Utc>) -> Self {
        Self { code, observed_at }
    }

    #[must_use]
    pub fn is_admitted(&self) -> bool {
        self.code == DnsVerdictCode::Admitted
    }
}

/// DNSSEC outcome classification (weakest-wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DnssecOutcome {
    #[default]
    Secure,
    Insecure,
    Indeterminate,
    Bogus {
        outside_validity: bool,
    },
    TransportFailure,
}

impl DnssecOutcome {
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::TransportFailure, _) | (_, Self::TransportFailure) => Self::TransportFailure,
            (
                Self::Bogus {
                    outside_validity: a,
                },
                Self::Bogus {
                    outside_validity: b,
                },
            ) => Self::Bogus {
                outside_validity: a && b,
            },
            (Self::Bogus { outside_validity }, _) | (_, Self::Bogus { outside_validity }) => {
                Self::Bogus { outside_validity }
            }
            (Self::Indeterminate, _) | (_, Self::Indeterminate) => Self::Indeterminate,
            (Self::Insecure, _) | (_, Self::Insecure) => Self::Insecure,
            (Self::Secure, Self::Secure) => Self::Secure,
        }
    }
}

/// solstone.me CAA policy evaluation codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaaPolicyCode {
    Admitted,
    OtherCa,
    AccountUri,
    ValidationMethod,
    Issuewild,
    ExtraIssue,
    CaaUnreadable,
    Missing,
}

impl CaaPolicyCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::OtherCa => "other_ca",
            Self::AccountUri => "account_uri",
            Self::ValidationMethod => "validation_method",
            Self::Issuewild => "issuewild",
            Self::ExtraIssue => "extra_issue",
            Self::CaaUnreadable => "caa_unreadable",
            Self::Missing => "missing",
        }
    }
}

/// CAA evidence collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaaEvidence {
    pub records: Vec<CaaRecord>,
    pub policy: Option<CaaPolicyCode>,
    pub outcome: DnssecOutcome,
}

/// Host address evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddressEvidence {
    pub a: Vec<Ipv4Addr>,
    pub aaaa: Vec<Ipv6Addr>,
    pub cname: Vec<String>,
    pub outcome: DnssecOutcome,
}

impl AddressEvidence {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.a.is_empty() && self.aaaa.is_empty() && self.cname.is_empty()
    }
}

/// solstone.me aggregate DNS evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolstoneMeDns {
    pub caa: CaaEvidence,
    pub address: AddressEvidence,
}

// ---------------------------------------------------------------------------
// Policy Evaluation
// ---------------------------------------------------------------------------

/// Evaluate DNS records according to RFC 8659 CAA and BYO ingress policy.
pub fn evaluate_byo_dns_policy(
    hostname: &str,
    account_uri: &str,
    records_by_domain: &HashMap<String, HostDnsRecords>,
    now: DateTime<Utc>,
) -> DnsVerdict {
    let lower_host = hostname.to_ascii_lowercase();

    let target_records = records_by_domain.get(&lower_host);
    if let Some(target) = target_records {
        if !target.cname.is_empty() {
            return DnsVerdict::new(DnsVerdictCode::Cname, now);
        }
        if target.a.is_empty() && target.aaaa.is_empty() {
            return DnsVerdict::new(DnsVerdictCode::NoAddress, now);
        }
    } else {
        return DnsVerdict::new(DnsVerdictCode::NoAddress, now);
    }

    // RFC 8659: Find closest ancestor CAA RRset (including target domain)
    let closest_caa = find_closest_caa(&lower_host, records_by_domain);
    let Some(caa_set) = closest_caa else {
        return DnsVerdict::new(DnsVerdictCode::CaaMissing, now);
    };

    if caa_set.is_empty() {
        return DnsVerdict::new(DnsVerdictCode::CaaMissing, now);
    }

    // 1. Any issuewild tag is disallowed
    if caa_set
        .iter()
        .any(|r| r.tag.eq_ignore_ascii_case("issuewild"))
    {
        return DnsVerdict::new(DnsVerdictCode::Issuewild, now);
    }

    // 2. Filter for issue tags
    let issue_records: Vec<&CaaRecord> = caa_set
        .iter()
        .filter(|r| r.tag.eq_ignore_ascii_case("issue"))
        .collect();

    if issue_records.is_empty() {
        return DnsVerdict::new(DnsVerdictCode::CaaMissing, now);
    }
    if issue_records.len() > 1 {
        return DnsVerdict::new(DnsVerdictCode::ExtraIssue, now);
    }

    let single_issue = issue_records[0];
    let Some((ca_domain, params)) = parse_caa_issue_value(&single_issue.value) else {
        return DnsVerdict::new(DnsVerdictCode::OtherCa, now);
    };

    if !ca_domain.eq_ignore_ascii_case("letsencrypt.org") {
        return DnsVerdict::new(DnsVerdictCode::OtherCa, now);
    }

    let Some(parsed_account_uri) = params.get("accounturi") else {
        return DnsVerdict::new(DnsVerdictCode::AccountUri, now);
    };
    if parsed_account_uri != account_uri {
        return DnsVerdict::new(DnsVerdictCode::AccountUri, now);
    }

    let Some(parsed_val_methods) = params.get("validationmethods") else {
        return DnsVerdict::new(DnsVerdictCode::ValidationMethod, now);
    };
    if parsed_val_methods != "tls-alpn-01" {
        return DnsVerdict::new(DnsVerdictCode::ValidationMethod, now);
    }

    DnsVerdict::new(DnsVerdictCode::Admitted, now)
}

fn find_closest_caa<'a>(
    hostname: &str,
    records_by_domain: &'a HashMap<String, HostDnsRecords>,
) -> Option<&'a [CaaRecord]> {
    let mut current = hostname;
    loop {
        if let Some(records) = records_by_domain.get(current)
            && !records.caa.is_empty()
        {
            return Some(&records.caa);
        }
        if let Some((_, parent)) = current.split_once('.') {
            current = parent;
        } else {
            break;
        }
    }
    None
}

/// Parse CAA issue value: `letsencrypt.org; accounturi=...; validationmethods=tls-alpn-01`
pub fn parse_caa_issue_value(value: &str) -> Option<(String, HashMap<String, String>)> {
    let mut parts = value.split(';');
    let issuer = parts.next().unwrap_or("").trim();
    let ca_domain = issuer.strip_suffix('.').unwrap_or(issuer).to_string();
    let mut params = HashMap::new();

    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (k, v) = trimmed.split_once('=')?;
        let key = k.trim().to_ascii_lowercase();
        if key != "accounturi" && key != "validationmethods" {
            return None;
        }
        let val = v.trim().trim_matches('"').trim().to_string();
        if val.is_empty() || params.insert(key, val).is_some() {
            return None;
        }
    }

    Some((ca_domain, params))
}

/// Evaluate solstone.me CAA policy from a non-empty CAA set and unreadable flag.
fn evaluate_solstone_caa_policy(
    records: &[CaaRecord],
    account_uri: &str,
    unreadable: bool,
) -> Option<CaaPolicyCode> {
    if unreadable {
        return Some(CaaPolicyCode::CaaUnreadable);
    }
    if records.is_empty() {
        return None;
    }
    if records
        .iter()
        .any(|r| r.tag.eq_ignore_ascii_case("issuewild"))
    {
        return Some(CaaPolicyCode::Issuewild);
    }
    let issue_records: Vec<&CaaRecord> = records
        .iter()
        .filter(|r| r.tag.eq_ignore_ascii_case("issue"))
        .collect();
    if issue_records.is_empty() {
        // Non-empty CAA set with no issue and no issuewild fails closed as other_ca
        return Some(CaaPolicyCode::OtherCa);
    }
    if issue_records.len() > 1 {
        return Some(CaaPolicyCode::ExtraIssue);
    }
    let single_issue = issue_records[0];
    let Some((ca_domain, params)) = parse_caa_issue_value(&single_issue.value) else {
        return Some(CaaPolicyCode::OtherCa);
    };
    if !ca_domain.eq_ignore_ascii_case("letsencrypt.org") {
        return Some(CaaPolicyCode::OtherCa);
    }
    let Some(parsed_account_uri) = params.get("accounturi") else {
        return Some(CaaPolicyCode::AccountUri);
    };
    if parsed_account_uri != account_uri {
        return Some(CaaPolicyCode::AccountUri);
    }
    let Some(parsed_val_methods) = params.get("validationmethods") else {
        return Some(CaaPolicyCode::ValidationMethod);
    };
    if parsed_val_methods != "tls-alpn-01" {
        return Some(CaaPolicyCode::ValidationMethod);
    }
    Some(CaaPolicyCode::Admitted)
}

// ---------------------------------------------------------------------------
// Transport Memory & Watching DnsHandle
// ---------------------------------------------------------------------------

#[derive(Default)]
struct LookupTracker {
    has_transport_error: AtomicBool,
    rrsig_snapshots: Mutex<Vec<RrsigSnapshot>>,
    raw_answer_sections: Mutex<Vec<Vec<u8>>>,
}

#[derive(Clone, Debug)]
struct RrsigSnapshot {
    #[allow(dead_code)]
    type_covered: RecordType,
    sig_inception: u32,
    sig_expiration: u32,
}

tokio::task_local! {
    static LOOKUP_TRACKER: Arc<LookupTracker>;
}

impl LookupTracker {
    fn record_response(&self, response: &DnsResponse) {
        let mut rrsigs = Vec::new();
        for record in response.answers.iter().chain(response.authorities.iter()) {
            if record.record_type() == RecordType::RRSIG
                && let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &record.data
            {
                rrsigs.push(RrsigSnapshot {
                    type_covered: sig.input().type_covered,
                    sig_inception: sig.input().sig_inception.get(),
                    sig_expiration: sig.input().sig_expiration.get(),
                });
            }
        }
        if !rrsigs.is_empty() {
            let mut guard = self.rrsig_snapshots.lock().unwrap();
            guard.extend(rrsigs);
        }

        // Snapshot raw buffer for CAA parsing
        let buf = response.as_buffer();
        if !buf.is_empty() {
            let mut guard = self.raw_answer_sections.lock().unwrap();
            guard.push(buf.to_vec());
        }
    }
}

/// A watching wrapper around an inner `DnsHandle` to record transport failures and wire responses.
#[derive(Clone)]
struct WatchingHandle<H> {
    inner: H,
}

impl<H: DnsHandle> DnsHandle for WatchingHandle<H> {
    type Response = WatchingStream<H::Response>;
    type Runtime = H::Runtime;

    fn is_verifying_dnssec(&self) -> bool {
        self.inner.is_verifying_dnssec()
    }

    fn is_using_edns(&self) -> bool {
        self.inner.is_using_edns()
    }

    fn send(&self, request: DnsRequest) -> Self::Response {
        let stream = self.inner.send(request);
        WatchingStream { inner: stream }
    }
}

struct WatchingStream<S> {
    inner: S,
}

fn is_transport_error(err: &NetError) -> bool {
    matches!(
        err,
        NetError::Io(_)
            | NetError::Timeout
            | NetError::NoConnections
            | NetError::Busy
            | NetError::Dns(DnsError::ResponseCode(ResponseCode::ServFail))
    )
}

impl<S: Stream<Item = Result<DnsResponse, NetError>> + Send + Unpin + 'static> Stream
    for WatchingStream<S>
{
    type Item = Result<DnsResponse, NetError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<DnsResponse, NetError>>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(response))) => {
                let _ = LOOKUP_TRACKER.try_with(|tracker| {
                    if response.metadata.response_code == ResponseCode::ServFail {
                        tracker.has_transport_error.store(true, Ordering::SeqCst);
                    }
                    tracker.record_response(&response);
                });
                Poll::Ready(Some(Ok(response)))
            }
            Poll::Ready(Some(Err(err))) => {
                let is_trans = is_transport_error(&err);
                let _ = LOOKUP_TRACKER.try_with(|tracker| {
                    if is_trans {
                        tracker.has_transport_error.store(true, Ordering::SeqCst);
                    }
                });
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: Send + Unpin + 'static> Unpin for WatchingStream<S> {}

// ---------------------------------------------------------------------------
// Raw CAA Wire Parsing
// ---------------------------------------------------------------------------

/// Slice CAA records directly from raw DNS wire message buffers without library tag validation.
fn parse_caa_from_wire_buffers(buffers: &[Vec<u8>], query_name: &Name) -> (Vec<CaaRecord>, bool) {
    let mut records = Vec::new();
    let mut unreadable = false;

    for buf in buffers {
        let mut decoder = BinDecoder::new(buf);
        let Ok(_id) = decoder.read_u16() else {
            continue;
        };
        let Ok(_flags) = decoder.read_u16() else {
            continue;
        };
        let Ok(qdcount) = decoder.read_u16() else {
            continue;
        };
        let Ok(ancount) = decoder.read_u16() else {
            continue;
        };
        let Ok(nscount) = decoder.read_u16() else {
            continue;
        };
        let Ok(arcount) = decoder.read_u16() else {
            continue;
        };

        let qdcount = qdcount.unverified() as usize;
        let ancount = ancount.unverified() as usize;
        let nscount = nscount.unverified() as usize;
        let arcount = arcount.unverified() as usize;

        let mut q_ok = true;
        for _ in 0..qdcount {
            if Name::read(&mut decoder).is_err()
                || decoder.read_u16().is_err()
                || decoder.read_u16().is_err()
            {
                q_ok = false;
                break;
            }
        }
        if !q_ok {
            continue;
        }

        let total_records = ancount + nscount + arcount;
        for _ in 0..total_records {
            let Ok(name) = Name::read(&mut decoder) else {
                break;
            };
            let Ok(rtype) = decoder.read_u16() else { break };
            let Ok(_rclass) = decoder.read_u16() else {
                break;
            };
            let Ok(_ttl) = decoder.read_u32() else { break };
            let Ok(rdlength) = decoder.read_u16() else {
                break;
            };
            let rdlength = rdlength.unverified() as usize;
            let Ok(rdata_bytes) = decoder.read_slice(rdlength) else {
                break;
            };
            let rdata_bytes = rdata_bytes.unverified();

            if rtype.unverified() == 257 && &name == query_name {
                match parse_raw_caa_rdata(rdata_bytes) {
                    Ok(rec) => records.push(rec),
                    Err(()) => unreadable = true,
                }
            }
        }
    }

    (records, unreadable)
}

/// Direct slice of raw wire RDATA bytes into CAA flags, tag, and value.
#[allow(clippy::result_unit_err)]
pub fn parse_raw_caa_rdata(rdata_bytes: &[u8]) -> Result<CaaRecord, ()> {
    if rdata_bytes.len() < 2 {
        return Err(());
    }
    let flags = rdata_bytes[0];
    let tag_len = rdata_bytes[1] as usize;
    if rdata_bytes.len() < 2 + tag_len {
        return Err(());
    }
    let tag_bytes = &rdata_bytes[2..2 + tag_len];
    let tag = String::from_utf8_lossy(tag_bytes).to_string();
    let value_bytes = &rdata_bytes[2 + tag_len..];
    let value = String::from_utf8_lossy(value_bytes).to_string();
    Ok(CaaRecord { flags, tag, value })
}

// ---------------------------------------------------------------------------
// DNS Query Execution & Stripping Probe
// ---------------------------------------------------------------------------

fn random_u16() -> u16 {
    let rng = ring::rand::SystemRandom::new();
    let mut buf = [0u8; 2];
    let _ = rng.fill(&mut buf);
    u16::from_ne_bytes(buf)
}

fn make_query_request(name: Name, record_type: RecordType) -> DnsRequest {
    let mut message = Message::new(random_u16(), MessageType::Query, OpCode::Query);
    message.metadata.recursion_desired = true;
    message.queries.push(Query::query(name, record_type));
    let mut options = DnsRequestOptions::default();
    options.edns_set_dnssec_ok = true;
    DnsRequest::new(message, options)
}

/// Evaluates outside-validity for bogus RRSIGs relative to validator clock (`now`).
fn check_rrsig_outside_validity(snapshots: &[RrsigSnapshot], now: u32) -> bool {
    if snapshots.is_empty() {
        return false;
    }

    let now_sn = SerialNumber::new(now);
    snapshots.iter().all(|s| {
        let inception = SerialNumber::new(s.sig_inception);
        let expiration = SerialNumber::new(s.sig_expiration);
        !(now_sn >= inception && now_sn <= expiration)
    })
}

/// Executes a validated query through `DnssecDnsHandle` with task-local transport tracking.
async fn query_dnssec<H: DnsHandle>(
    handle: &DnssecDnsHandle<WatchingHandle<H>>,
    name: Name,
    record_type: RecordType,
    probe_failed: bool,
) -> (Vec<Record>, DnssecOutcome, Vec<Vec<u8>>) {
    let tracker = Arc::new(LookupTracker::default());
    let req = make_query_request(name.clone(), record_type);

    let result = LOOKUP_TRACKER
        .scope(tracker.clone(), async {
            let mut stream = handle.send(req);
            stream.next().await
        })
        .await;

    let now_u32 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);

    let had_inner_transport_err = tracker.has_transport_error.load(Ordering::SeqCst);
    let rrsig_snaps = tracker.rrsig_snapshots.lock().unwrap().clone();
    let raw_buffers = tracker.raw_answer_sections.lock().unwrap().clone();

    match result {
        Some(Ok(response)) => {
            let answers: Vec<Record> = response
                .answers
                .iter()
                .filter(|r| r.record_type() == record_type || r.record_type() == RecordType::CNAME)
                .cloned()
                .collect();

            let records_to_check: Vec<&Record> = if !answers.is_empty() {
                answers.iter().collect()
            } else {
                response
                    .authorities
                    .iter()
                    .chain(response.additionals.iter())
                    .collect()
            };

            let mut all_secure = !records_to_check.is_empty();
            let mut any_insecure = false;
            let mut any_indeterminate = false;
            let mut any_bogus = false;

            for record in &records_to_check {
                match record.proof {
                    Proof::Secure => {}
                    Proof::Insecure => {
                        all_secure = false;
                        any_insecure = true;
                    }
                    Proof::Indeterminate => {
                        all_secure = false;
                        any_indeterminate = true;
                    }
                    Proof::Bogus => {
                        all_secure = false;
                        any_bogus = true;
                    }
                }
            }

            let outcome = if any_bogus {
                if had_inner_transport_err {
                    DnssecOutcome::TransportFailure
                } else if rrsig_snaps.is_empty() && probe_failed {
                    DnssecOutcome::Indeterminate
                } else {
                    let outside = check_rrsig_outside_validity(&rrsig_snaps, now_u32);
                    DnssecOutcome::Bogus {
                        outside_validity: outside,
                    }
                }
            } else if any_indeterminate {
                DnssecOutcome::Indeterminate
            } else if any_insecure {
                DnssecOutcome::Insecure
            } else if all_secure {
                DnssecOutcome::Secure
            } else if had_inner_transport_err {
                DnssecOutcome::TransportFailure
            } else if probe_failed {
                DnssecOutcome::Indeterminate
            } else {
                DnssecOutcome::Insecure
            };

            (answers, outcome, raw_buffers)
        }
        Some(Err(NetError::Dns(DnsError::Nsec { proof, .. }))) => match proof {
            Proof::Secure => (Vec::new(), DnssecOutcome::Secure, raw_buffers),
            Proof::Insecure => (Vec::new(), DnssecOutcome::Insecure, raw_buffers),
            Proof::Indeterminate => (Vec::new(), DnssecOutcome::Indeterminate, raw_buffers),
            Proof::Bogus => {
                if had_inner_transport_err {
                    (Vec::new(), DnssecOutcome::TransportFailure, raw_buffers)
                } else if rrsig_snaps.is_empty() && probe_failed {
                    (Vec::new(), DnssecOutcome::Indeterminate, raw_buffers)
                } else {
                    let outside = check_rrsig_outside_validity(&rrsig_snaps, now_u32);
                    (
                        Vec::new(),
                        DnssecOutcome::Bogus {
                            outside_validity: outside,
                        },
                        raw_buffers,
                    )
                }
            }
        },
        Some(Err(ref err)) if !is_transport_error(err) => {
            (Vec::new(), DnssecOutcome::Indeterminate, raw_buffers)
        }
        _ => (Vec::new(), DnssecOutcome::TransportFailure, raw_buffers),
    }
}

/// Stripping probe: sends an unvalidated `. DNSKEY` query to all configured servers.
async fn probe_dnssec_stripping<P: ConnectionProvider + Clone>(
    name_servers: &[NameServerConfig],
    provider: &P,
) -> bool {
    let mut opts = ResolverOpts::default();
    opts.timeout = Duration::from_millis(800);
    opts.attempts = 1;

    let Ok(tls_config) = TlsConfig::new() else {
        return true;
    };
    let cx = Arc::new(PoolContext::new(opts, tls_config));

    for ns in name_servers {
        let single_pool =
            NameServerPool::from_config(vec![ns.clone()], cx.clone(), provider.clone());
        let req = make_query_request(Name::root(), RecordType::DNSKEY);
        let mut stream = single_pool.send(req);

        let failed = match stream.next().await {
            Some(Ok(response)) => {
                if response.metadata.response_code == ResponseCode::ServFail {
                    true
                } else {
                    let has_dnskey = response
                        .answers
                        .iter()
                        .any(|r| r.record_type() == RecordType::DNSKEY);
                    let has_rrsig = response
                        .answers
                        .iter()
                        .any(|r| r.record_type() == RecordType::RRSIG);
                    has_dnskey && !has_rrsig
                }
            }
            _ => true,
        };

        if failed {
            return true;
        }
    }

    false
}

fn protocol_id(p: &ProtocolConfig) -> u8 {
    match p {
        ProtocolConfig::Udp => 0,
        ProtocolConfig::Tcp => 1,
    }
}

struct ProductionPool {
    nameservers: Vec<(IpAddr, u16, u8)>,
    handle: DnssecDnsHandle<WatchingHandle<NameServerPool<TokioRuntimeProvider>>>,
    pool: NameServerPool<TokioRuntimeProvider>,
}

static PROD_POOL: Mutex<Option<ProductionPool>> = Mutex::new(None);

#[allow(clippy::type_complexity)]
fn get_or_create_prod_pool(
    config: &ResolverConfig,
) -> Result<
    (
        DnssecDnsHandle<WatchingHandle<NameServerPool<TokioRuntimeProvider>>>,
        NameServerPool<TokioRuntimeProvider>,
    ),
    String,
> {
    let mut ns_list = Vec::new();
    for ns in config.name_servers() {
        for conn in &ns.connections {
            ns_list.push((ns.ip, conn.port, protocol_id(&conn.protocol)));
        }
    }
    ns_list.sort();

    let mut guard = PROD_POOL.lock().unwrap();
    if let Some(ref entry) = *guard
        && entry.nameservers == ns_list
    {
        return Ok((entry.handle.clone(), entry.pool.clone()));
    }

    let mut opts = ResolverOpts::default();
    opts.timeout = Duration::from_millis(800);
    opts.attempts = 1;

    let tls_config = TlsConfig::new().map_err(|e| e.to_string())?;
    let cx = Arc::new(PoolContext::new(opts, tls_config));
    let pool = NameServerPool::from_config(
        config.name_servers().iter().cloned(),
        cx,
        TokioRuntimeProvider::default(),
    );

    let watching = WatchingHandle {
        inner: pool.clone(),
    };
    let handle = DnssecDnsHandle::new(watching);

    *guard = Some(ProductionPool {
        nameservers: ns_list,
        handle: handle.clone(),
        pool: pool.clone(),
    });

    Ok((handle, pool))
}

// ---------------------------------------------------------------------------
// Resolution Workflows: solstone.me and Owner
// ---------------------------------------------------------------------------

/// Resolves DNS and CAA evidence for solstone.me.
pub async fn resolve_solstone_me_dns(hostname: &str, account_uri: &str) -> SolstoneMeDns {
    let (config, _) = match read_system_conf() {
        Ok(pair) => pair,
        Err(_) => {
            return SolstoneMeDns {
                caa: CaaEvidence {
                    records: Vec::new(),
                    policy: None,
                    outcome: DnssecOutcome::TransportFailure,
                },
                address: AddressEvidence {
                    a: Vec::new(),
                    aaaa: Vec::new(),
                    cname: Vec::new(),
                    outcome: DnssecOutcome::TransportFailure,
                },
            };
        }
    };

    let (handle, _) = match get_or_create_prod_pool(&config) {
        Ok(pair) => pair,
        Err(_) => {
            return SolstoneMeDns {
                caa: CaaEvidence {
                    records: Vec::new(),
                    policy: None,
                    outcome: DnssecOutcome::TransportFailure,
                },
                address: AddressEvidence {
                    a: Vec::new(),
                    aaaa: Vec::new(),
                    cname: Vec::new(),
                    outcome: DnssecOutcome::TransportFailure,
                },
            };
        }
    };

    resolve_solstone_me_dns_with_handles(
        &handle,
        config.name_servers(),
        &TokioRuntimeProvider::default(),
        hostname,
        account_uri,
    )
    .await
}

async fn resolve_solstone_me_dns_with_handles<H: DnsHandle, P: ConnectionProvider + Clone>(
    handle: &DnssecDnsHandle<WatchingHandle<H>>,
    name_servers: &[NameServerConfig],
    provider: &P,
    hostname: &str,
    account_uri: &str,
) -> SolstoneMeDns {
    let probe_failed = probe_dnssec_stripping(name_servers, provider).await;
    let clean_host = hostname.trim_end_matches('.');
    let target_name = match Name::from_utf8(format!("{clean_host}.")) {
        Ok(n) => n,
        Err(_) => {
            return SolstoneMeDns {
                caa: CaaEvidence {
                    records: Vec::new(),
                    policy: None,
                    outcome: DnssecOutcome::TransportFailure,
                },
                address: AddressEvidence {
                    a: Vec::new(),
                    aaaa: Vec::new(),
                    cname: Vec::new(),
                    outcome: DnssecOutcome::TransportFailure,
                },
            };
        }
    };

    let a_fut = query_dnssec(handle, target_name.clone(), RecordType::A, probe_failed);
    let aaaa_fut = query_dnssec(handle, target_name.clone(), RecordType::AAAA, probe_failed);
    let cname_fut = query_dnssec(handle, target_name.clone(), RecordType::CNAME, probe_failed);
    let caa_fut = climb_caa_solstone(handle, clean_host, account_uri, probe_failed);

    let (
        (a_records, a_outcome, _),
        (aaaa_records, aaaa_outcome, _),
        (cname_records, cname_outcome, _),
        (caa_records, caa_policy, caa_outcome),
    ) = tokio::join!(a_fut, aaaa_fut, cname_fut, caa_fut);

    let mut addr_a = Vec::new();
    for r in a_records {
        if let RData::A(ip) = &r.data {
            addr_a.push(ip.0);
        }
    }
    let mut addr_aaaa = Vec::new();
    for r in aaaa_records {
        if let RData::AAAA(ip) = &r.data {
            addr_aaaa.push(ip.0);
        }
    }
    let mut addr_cname = Vec::new();
    for r in cname_records {
        if let RData::CNAME(name) = &r.data {
            addr_cname.push(name.to_utf8().trim_end_matches('.').to_string());
        }
    }

    let addr_outcome = a_outcome.merge(aaaa_outcome).merge(cname_outcome);
    let address = AddressEvidence {
        a: addr_a,
        aaaa: addr_aaaa,
        cname: addr_cname,
        outcome: addr_outcome,
    };

    SolstoneMeDns {
        caa: CaaEvidence {
            records: caa_records,
            policy: caa_policy,
            outcome: caa_outcome,
        },
        address,
    }
}

async fn climb_caa_solstone<H: DnsHandle>(
    handle: &DnssecDnsHandle<WatchingHandle<H>>,
    hostname: &str,
    account_uri: &str,
    probe_failed: bool,
) -> (Vec<CaaRecord>, Option<CaaPolicyCode>, DnssecOutcome) {
    let mut current = hostname;
    let mut accumulated_outcome = DnssecOutcome::Secure;
    let mut all_denials_authenticated = true;

    loop {
        let Ok(qname) = Name::from_utf8(format!("{current}.")) else {
            return (Vec::new(), None, DnssecOutcome::TransportFailure);
        };

        let (_records, outcome, raw_buffers) =
            query_dnssec(handle, qname.clone(), RecordType::CAA, probe_failed).await;
        accumulated_outcome = accumulated_outcome.merge(outcome);

        let (parsed_caa, unreadable) = parse_caa_from_wire_buffers(&raw_buffers, &qname);

        if !parsed_caa.is_empty() || unreadable {
            let policy = evaluate_solstone_caa_policy(&parsed_caa, account_uri, unreadable);
            return (parsed_caa, policy, accumulated_outcome);
        }

        match outcome {
            DnssecOutcome::Secure | DnssecOutcome::Insecure => {
                if let Some((_, parent)) = current.split_once('.') {
                    current = parent;
                } else {
                    break;
                }
            }
            DnssecOutcome::Indeterminate
            | DnssecOutcome::Bogus { .. }
            | DnssecOutcome::TransportFailure => {
                all_denials_authenticated = false;
                break;
            }
        }
    }

    let policy = if all_denials_authenticated {
        Some(CaaPolicyCode::Missing)
    } else {
        None
    };

    (Vec::new(), policy, accumulated_outcome)
}

#[cfg(test)]
pub static TEST_VERDICT_OVERRIDE: std::sync::RwLock<Option<DnsVerdict>> =
    std::sync::RwLock::new(None);

/// Main owner-hostname resolution entry point with 5-second outer timeout.
pub async fn resolve_byo_dns(hostname: &str, account_uri: &str, now: DateTime<Utc>) -> DnsVerdict {
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_VERDICT_OVERRIDE.read()
            && let Some(ref verdict) = *guard
        {
            return verdict.clone();
        }
    }

    let (config, _) = match read_system_conf() {
        Ok(pair) => pair,
        Err(_) => return DnsVerdict::new(DnsVerdictCode::LookupError, now),
    };

    let (handle, _) = match get_or_create_prod_pool(&config) {
        Ok(pair) => pair,
        Err(_) => return DnsVerdict::new(DnsVerdictCode::LookupError, now),
    };

    match tokio::time::timeout(
        Duration::from_secs(5),
        resolve_byo_dns_with_handles(
            &handle,
            config.name_servers(),
            &TokioRuntimeProvider::default(),
            hostname,
            account_uri,
            now,
        ),
    )
    .await
    {
        Ok(verdict) => verdict,
        Err(_) => DnsVerdict::new(DnsVerdictCode::LookupTimeout, now),
    }
}

async fn resolve_byo_dns_with_handles<H: DnsHandle, P: ConnectionProvider + Clone>(
    handle: &DnssecDnsHandle<WatchingHandle<H>>,
    name_servers: &[NameServerConfig],
    provider: &P,
    hostname: &str,
    account_uri: &str,
    now: DateTime<Utc>,
) -> DnsVerdict {
    let probe_failed = probe_dnssec_stripping(name_servers, provider).await;
    let clean_host = hostname.trim_end_matches('.');
    let target_name = match Name::from_utf8(format!("{clean_host}.")) {
        Ok(n) => n,
        Err(_) => return DnsVerdict::new(DnsVerdictCode::LookupError, now),
    };

    let a_fut = query_dnssec(handle, target_name.clone(), RecordType::A, probe_failed);
    let aaaa_fut = query_dnssec(handle, target_name.clone(), RecordType::AAAA, probe_failed);
    let cname_fut = query_dnssec(handle, target_name.clone(), RecordType::CNAME, probe_failed);
    let caa_fut = climb_caa_owner(handle, clean_host, probe_failed);

    let (
        (a_records, a_outcome, _),
        (aaaa_records, aaaa_outcome, _),
        (cname_records, cname_outcome, _),
        (found_caa, caa_outcome),
    ) = tokio::join!(a_fut, aaaa_fut, cname_fut, caa_fut);

    let mut map = HashMap::new();
    let mut target_host_records = HostDnsRecords::default();

    for r in a_records {
        if let RData::A(ip) = &r.data {
            target_host_records.a.push(ip.0);
        }
    }
    for r in aaaa_records {
        if let RData::AAAA(ip) = &r.data {
            target_host_records.aaaa.push(ip.0);
        }
    }
    for r in cname_records {
        if let RData::CNAME(name) = &r.data {
            target_host_records
                .cname
                .push(name.to_utf8().trim_end_matches('.').to_string());
        }
    }

    if let Some((caa_domain, caa_records)) = found_caa {
        let entry = map
            .entry(caa_domain.to_ascii_lowercase())
            .or_insert_with(HostDnsRecords::default);
        entry.caa = caa_records;
    }

    let target_entry = map
        .entry(clean_host.to_ascii_lowercase())
        .or_insert_with(HostDnsRecords::default);
    target_entry.a = target_host_records.a;
    target_entry.aaaa = target_host_records.aaaa;
    target_entry.cname = target_host_records.cname;

    let combined_outcome = a_outcome
        .merge(aaaa_outcome)
        .merge(cname_outcome)
        .merge(caa_outcome);

    match combined_outcome {
        DnssecOutcome::TransportFailure => DnsVerdict::new(DnsVerdictCode::LookupError, now),
        DnssecOutcome::Bogus {
            outside_validity: true,
        } => DnsVerdict::new(DnsVerdictCode::SignatureOutsideValidityAtLocalTime, now),
        DnssecOutcome::Bogus {
            outside_validity: false,
        } => DnsVerdict::new(DnsVerdictCode::DnssecBogus, now),
        DnssecOutcome::Secure | DnssecOutcome::Insecure | DnssecOutcome::Indeterminate => {
            evaluate_byo_dns_policy(clean_host, account_uri, &map, now)
        }
    }
}

async fn climb_caa_owner<H: DnsHandle>(
    handle: &DnssecDnsHandle<WatchingHandle<H>>,
    hostname: &str,
    probe_failed: bool,
) -> (Option<(String, Vec<CaaRecord>)>, DnssecOutcome) {
    let mut current = hostname;
    let mut accumulated_outcome = DnssecOutcome::Secure;

    loop {
        let Ok(qname) = Name::from_utf8(format!("{current}.")) else {
            return (None, DnssecOutcome::TransportFailure);
        };

        let (_records, outcome, raw_buffers) =
            query_dnssec(handle, qname.clone(), RecordType::CAA, probe_failed).await;
        accumulated_outcome = accumulated_outcome.merge(outcome);

        let (parsed_caa, unreadable) = parse_caa_from_wire_buffers(&raw_buffers, &qname);

        if !parsed_caa.is_empty() || unreadable {
            return (Some((current.to_string(), parsed_caa)), accumulated_outcome);
        }

        match outcome {
            DnssecOutcome::Secure | DnssecOutcome::Insecure | DnssecOutcome::Indeterminate => {
                if let Some((_, parent)) = current.split_once('.') {
                    current = parent;
                } else {
                    break;
                }
            }
            DnssecOutcome::Bogus { .. } | DnssecOutcome::TransportFailure => {
                break;
            }
        }
    }

    (None, accumulated_outcome)
}

// ---------------------------------------------------------------------------
// Unit Tests & In-Memory Mock Harness
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use std::sync::RwLock;

    use hickory_net::xfer::{DnsResponseStream, Protocol};
    use hickory_proto::dnssec::DnssecSigner;
    use hickory_proto::dnssec::Nsec3HashAlgorithm;
    use hickory_proto::dnssec::SigningKey;
    use hickory_proto::dnssec::TrustAnchors;
    use hickory_proto::dnssec::crypto::Ed25519SigningKey;
    use hickory_proto::dnssec::rdata::DNSKEY;
    use hickory_proto::dnssec::rdata::RRSIG;
    use hickory_proto::dnssec::rdata::nsec::NSEC;
    use hickory_proto::dnssec::rdata::nsec3::NSEC3;
    use hickory_proto::serialize::binary::{BinEncodable, BinEncoder};

    fn base32_dnssec(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuv";
        let mut out = String::new();
        let mut buffer = 0u64;
        let mut bits = 0;
        for &b in bytes {
            buffer = (buffer << 8) | (b as u64);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                let idx = ((buffer >> bits) & 0x1F) as usize;
                out.push(ALPHABET[idx] as char);
            }
        }
        if bits > 0 {
            let idx = ((buffer << (5 - bits)) & 0x1F) as usize;
            out.push(ALPHABET[idx] as char);
        }
        out
    }
    use hickory_proto::op::{Message, ResponseCode};
    use hickory_proto::rr::rdata::NS;
    use hickory_proto::rr::rdata::a::A;
    use hickory_proto::rr::rdata::caa::{CAA, KeyValue};
    use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordSet, RecordType};
    use hickory_resolver::ConnectionProvider;
    use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ProtocolConfig};

    const URI: &str = "https://acme-v02.api.letsencrypt.org/acme/acct/12345678";
    const HOST: &str = "mcp.example.com";

    fn base_records() -> HashMap<String, HostDnsRecords> {
        let mut map = HashMap::new();
        map.insert(
            HOST.to_string(),
            HostDnsRecords {
                a: vec![Ipv4Addr::new(93, 184, 216, 34)],
                aaaa: vec![],
                cname: vec![],
                caa: vec![CaaRecord {
                    flags: 0,
                    tag: "issue".to_string(),
                    value: format!(
                        "letsencrypt.org; accounturi={URI}; validationmethods=tls-alpn-01"
                    ),
                }],
            },
        );
        map
    }

    #[test]
    fn policy_admitted_case() {
        let map = base_records();
        let now = Utc::now();
        let verdict = evaluate_byo_dns_policy(HOST, URI, &map, now);
        assert_eq!(verdict.code, DnsVerdictCode::Admitted);
        assert!(verdict.is_admitted());
    }

    #[test]
    fn policy_accepts_fully_qualified_issuer_name() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa[0].value =
            format!("letsencrypt.org.; accounturi={URI}; validationmethods=tls-alpn-01");
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, Utc::now()).code,
            DnsVerdictCode::Admitted
        );
    }

    #[test]
    fn policy_rejects_duplicate_account_uri_even_when_last_matches() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa[0].value = format!(
            "letsencrypt.org; accounturi=https://acme.example/acct/other; accounturi={URI}; validationmethods=tls-alpn-01"
        );
        assert_ne!(
            evaluate_byo_dns_policy(HOST, URI, &map, Utc::now()).code,
            DnsVerdictCode::Admitted
        );
    }

    #[test]
    fn policy_rejects_unknown_parameter() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa[0].value =
            format!("letsencrypt.org; accounturi={URI}; validationmethods=tls-alpn-01; unknown=1");
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, Utc::now()).code,
            DnsVerdictCode::OtherCa
        );
    }

    #[test]
    fn policy_rejects_issuewild() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa = vec![CaaRecord {
            flags: 0,
            tag: "issuewild".to_string(),
            value: "letsencrypt.org".to_string(),
        }];
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, Utc::now()).code,
            DnsVerdictCode::Issuewild
        );
    }

    #[test]
    fn policy_rejects_extra_issue() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa.push(CaaRecord {
            flags: 0,
            tag: "issue".to_string(),
            value: "otherca.com".to_string(),
        });
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, Utc::now()).code,
            DnsVerdictCode::ExtraIssue
        );
    }

    #[test]
    fn policy_rejects_cname() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().cname = vec!["other.example.com".to_string()];
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, Utc::now()).code,
            DnsVerdictCode::Cname
        );
    }

    #[test]
    fn policy_rejects_no_address() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().a.clear();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, Utc::now()).code,
            DnsVerdictCode::NoAddress
        );
    }

    #[derive(Clone)]
    enum MockScript {
        Message(Result<Message, NetError>, bool),
        Wire(Vec<u8>),
    }

    #[derive(Default, Clone)]
    struct MockProvider {
        scripts: Arc<RwLock<HashMap<(IpAddr, Protocol, Name, RecordType), MockScript>>>,
        queries_recorded: Arc<RwLock<Vec<(Name, RecordType)>>>,
        runtime_provider: TokioRuntimeProvider,
    }

    impl MockProvider {
        fn script(&self, ip: IpAddr, proto: Protocol, name: Name, rtype: RecordType, msg: Message) {
            let mut guard = self.scripts.write().unwrap();
            guard.insert(
                (ip, proto, name, rtype),
                MockScript::Message(Ok(msg), false),
            );
        }

        fn script_truncated(
            &self,
            ip: IpAddr,
            proto: Protocol,
            name: Name,
            rtype: RecordType,
            msg: Message,
        ) {
            let mut guard = self.scripts.write().unwrap();
            guard.insert((ip, proto, name, rtype), MockScript::Message(Ok(msg), true));
        }

        fn script_err(
            &self,
            ip: IpAddr,
            proto: Protocol,
            name: Name,
            rtype: RecordType,
            err: NetError,
        ) {
            let mut guard = self.scripts.write().unwrap();
            guard.insert(
                (ip, proto, name, rtype),
                MockScript::Message(Err(err), false),
            );
        }

        fn script_wire(
            &self,
            ip: IpAddr,
            proto: Protocol,
            name: Name,
            rtype: RecordType,
            wire: Vec<u8>,
        ) {
            let mut guard = self.scripts.write().unwrap();
            guard.insert((ip, proto, name, rtype), MockScript::Wire(wire));
        }
    }

    #[derive(Clone)]
    struct MockConnection {
        provider: MockProvider,
        ip: IpAddr,
        proto: Protocol,
    }

    impl ConnectionProvider for MockProvider {
        type Conn = MockConnection;
        type FutureConn = std::future::Ready<Result<MockConnection, NetError>>;
        type RuntimeProvider = TokioRuntimeProvider;

        fn new_connection(
            &self,
            ip: IpAddr,
            config: &ConnectionConfig,
            _cx: &PoolContext,
        ) -> Result<Self::FutureConn, NetError> {
            let proto = match config.protocol {
                ProtocolConfig::Tcp => Protocol::Tcp,
                _ => Protocol::Udp,
            };
            let conn = MockConnection {
                provider: self.clone(),
                ip,
                proto,
            };
            Ok(std::future::ready(Ok(conn)))
        }

        fn runtime_provider(&self) -> &Self::RuntimeProvider {
            &self.runtime_provider
        }
    }

    impl DnsHandle for MockConnection {
        type Response = DnsResponseStream;
        type Runtime = TokioRuntimeProvider;

        fn send(&self, request: DnsRequest) -> Self::Response {
            let query = request.queries.first().cloned();
            let Some(q) = query else {
                return DnsResponseStream::from(Box::pin(std::future::ready(Err(NetError::from(
                    DnsError::ResponseCode(ResponseCode::FormErr),
                )))));
            };

            self.provider
                .queries_recorded
                .write()
                .unwrap()
                .push((q.name().clone(), q.query_type()));

            let key = (self.ip, self.proto, q.name().clone(), q.query_type());
            let guard = self.provider.scripts.read().unwrap();
            if let Some(script) = guard.get(&key) {
                match script {
                    MockScript::Message(Ok(msg), truncated) => {
                        let mut m = msg.clone();
                        m.metadata.id = request.metadata.id;
                        m.metadata.message_type = MessageType::Response;
                        m.metadata.authoritative = true;
                        m.queries = request.queries.clone();
                        if *truncated {
                            m.metadata.truncation = true;
                        }
                        let dns_resp = DnsResponse::from_message(m).unwrap();
                        DnsResponseStream::from(Box::pin(std::future::ready(Ok(dns_resp))))
                    }
                    MockScript::Message(Err(e), _) => {
                        DnsResponseStream::from(Box::pin(std::future::ready(Err(e.clone()))))
                    }
                    MockScript::Wire(wire) => {
                        let mut wire = wire.clone();
                        if wire.len() >= 2 {
                            let req_id = request.metadata.id.to_be_bytes();
                            wire[0] = req_id[0];
                            wire[1] = req_id[1];
                        }
                        let _ = LOOKUP_TRACKER.try_with(|tracker| {
                            let mut guard = tracker.raw_answer_sections.lock().unwrap();
                            guard.push(wire.clone());
                        });
                        let mut empty_msg =
                            Message::new(request.metadata.id, MessageType::Response, OpCode::Query);
                        empty_msg.metadata.response_code = ResponseCode::NoError;
                        empty_msg.metadata.authoritative = true;
                        empty_msg.queries = request.queries.clone();
                        let dns_resp = DnsResponse::from_message(empty_msg).unwrap();
                        DnsResponseStream::from(Box::pin(std::future::ready(Ok(dns_resp))))
                    }
                }
            } else {
                // Return default NOERROR empty response
                let mut m = Message::new(request.metadata.id, MessageType::Response, OpCode::Query);
                m.metadata.response_code = ResponseCode::NoError;
                m.metadata.authoritative = true;
                m.queries = request.queries.clone();
                let dns_resp = DnsResponse::from_message(m).unwrap();
                DnsResponseStream::from(Box::pin(std::future::ready(Ok(dns_resp))))
            }
        }
    }

    // Signer helper generating keys and signing RRsets at test runtime
    struct TestDnssecContext {
        origin: Name,
        dnskey: DNSKEY,
        signer: DnssecSigner,
        trust_anchors: Arc<TrustAnchors>,
    }

    impl TestDnssecContext {
        fn new(origin: Name, duration: Duration) -> Self {
            let pkcs8 = Ed25519SigningKey::generate_pkcs8().unwrap();
            let key = Ed25519SigningKey::from_pkcs8(&pkcs8).unwrap();
            let pub_key = key.to_public_key().unwrap();
            let dnskey = DNSKEY::from_key(&pub_key);
            let mut anchors = TrustAnchors::empty();
            anchors.insert_with_name(&pub_key, origin.clone().into());
            let signer = DnssecSigner::new(dnskey.clone(), Box::new(key), origin.clone(), duration);
            Self {
                origin,
                dnskey,
                signer,
                trust_anchors: Arc::new(anchors),
            }
        }

        fn sign_rrset(&self, mut rrset: RecordSet, in_window: bool) -> RecordSet {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            let inception_ts = if in_window { now - 86400 } else { now - 172800 };

            let inception = time::OffsetDateTime::from_unix_timestamp(inception_ts).unwrap();
            let rrsig = RRSIG::from_rrset(&rrset, DNSClass::IN, inception, &self.signer).unwrap();

            rrset.insert_rrsig(Record::from_rdata(
                rrset.name().clone(),
                rrset.ttl(),
                RData::DNSSEC(DNSSECRData::RRSIG(rrsig)),
            ));
            rrset
        }

        fn make_dnskey_response(&self, qname: &Name, in_window: bool) -> Message {
            let mut rrset = RecordSet::new(qname.clone(), RecordType::DNSKEY, 3600);
            let rec = Record::from_rdata(
                qname.clone(),
                3600,
                RData::DNSSEC(DNSSECRData::DNSKEY(self.dnskey.clone())),
            );
            rrset.insert(rec, 3600);
            let signed = self.sign_rrset(rrset, in_window);

            let mut msg = Message::new(0, MessageType::Response, OpCode::Query);
            msg.metadata.response_code = ResponseCode::NoError;
            for r in signed.records(true) {
                msg.answers.push(r.clone());
            }
            msg
        }

        fn make_nodata_response(
            &self,
            qname: &Name,
            existing_types: Vec<RecordType>,
            in_window: bool,
        ) -> Message {
            let mut types = existing_types;
            types.push(RecordType::NSEC);
            types.push(RecordType::RRSIG);
            let nsec_data = NSEC::new(self.origin.clone(), types);
            let mut nsec_rrset = RecordSet::new(qname.clone(), RecordType::NSEC, 3600);
            nsec_rrset.insert(
                Record::from_rdata(
                    qname.clone(),
                    3600,
                    RData::DNSSEC(DNSSECRData::NSEC(nsec_data)),
                ),
                3600,
            );
            let signed_nsec = self.sign_rrset(nsec_rrset, in_window);

            let mut nodata_msg = Message::new(0, MessageType::Response, OpCode::Query);
            nodata_msg.metadata.response_code = ResponseCode::NoError;
            for r in signed_nsec.records(true) {
                nodata_msg.authorities.push(r.clone());
            }
            nodata_msg
        }
    }

    fn setup_test_pool(
        provider: MockProvider,
        anchors: Arc<TrustAnchors>,
    ) -> (
        DnssecDnsHandle<WatchingHandle<NameServerPool<MockProvider>>>,
        Vec<NameServerConfig>,
        MockProvider,
    ) {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let ns = NameServerConfig::udp_and_tcp(ip);
        let mut opts = ResolverOpts::default();
        opts.timeout = Duration::from_millis(800);
        opts.attempts = 1;

        let cx = Arc::new(PoolContext::new(opts, TlsConfig::new().unwrap()));
        let pool = NameServerPool::from_config(vec![ns.clone()], cx, provider.clone());
        let watching = WatchingHandle { inner: pool };
        let handle = DnssecDnsHandle::with_trust_anchor(watching, anchors);
        (handle, vec![ns], provider)
    }

    // Acceptance 1: Positive in-window, tampered in-window (dnssec_bogus), expired outside-window (SignatureOutsideValidityAtLocalTime)
    #[tokio::test]
    async fn test_acceptance_1_positive_and_validity_window() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let origin = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(origin.clone(), Duration::from_secs(86400 * 3));
        let provider = MockProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&origin, true);
        provider.script(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::DNSKEY,
            dnskey_msg.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            origin.clone(),
            RecordType::DNSKEY,
            dnskey_msg,
        );

        let caa_rdata = CAA::new_issue(
            false,
            Some(Name::from_str("letsencrypt.org.").unwrap()),
            vec![
                KeyValue::new("accounturi", URI),
                KeyValue::new("validationmethods", "tls-alpn-01"),
            ],
        );
        let mut rrset = RecordSet::new(target.clone(), RecordType::CAA, 3600);
        rrset.insert(
            Record::from_rdata(target.clone(), 3600, RData::CAA(caa_rdata)),
            3600,
        );
        let signed_in_window = ctx.sign_rrset(rrset.clone(), true);

        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_in_window.records(true) {
            caa_msg.answers.push(r.clone());
        }
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::CAA, caa_msg);

        let mut a_rrset = RecordSet::new(target.clone(), RecordType::A, 3600);
        a_rrset.insert(
            Record::from_rdata(
                target.clone(),
                3600,
                RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
            ),
            3600,
        );
        let signed_a = ctx.sign_rrset(a_rrset.clone(), true);
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_a.records(true) {
            a_msg.answers.push(r.clone());
        }
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        let nodata = ctx.make_nodata_response(&target, vec![RecordType::A, RecordType::CAA], true);
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::AAAA,
            nodata.clone(),
        );
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CNAME,
            nodata.clone(),
        );
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::NS, nodata);

        let mut ns_rrset = RecordSet::new(origin.clone(), RecordType::NS, 3600);
        ns_rrset.insert(
            Record::from_rdata(
                origin.clone(),
                3600,
                RData::NS(hickory_proto::rr::rdata::NS(
                    Name::from_utf8("ns1.example.com.").unwrap(),
                )),
            ),
            3600,
        );
        let signed_ns = ctx.sign_rrset(ns_rrset.clone(), true);
        let mut ns_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_ns.records(true) {
            ns_msg.answers.push(r.clone());
        }
        provider.script(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::NS,
            ns_msg.clone(),
        );
        provider.script(ip, Protocol::Tcp, origin.clone(), RecordType::NS, ns_msg);

        let (handle, ns_list, p) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone());

        // 1. Secure in-window
        let verdict =
            resolve_byo_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI, Utc::now())
                .await;
        assert_eq!(verdict.code, DnsVerdictCode::Admitted);

        // 2. Tamper one signature byte inside validity window -> dnssec_bogus (outside_validity=false)
        let mut tampered_caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_in_window.records(true) {
            if r.record_type() == RecordType::RRSIG {
                if let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &r.data {
                    let mut bytes = sig.sig().to_vec();
                    bytes[0] ^= 0xFF;
                    let new_sig = RRSIG::from_sig(sig.input().clone(), bytes);
                    tampered_caa_msg.answers.push(Record::from_rdata(
                        r.name.clone(),
                        r.ttl,
                        RData::DNSSEC(DNSSECRData::RRSIG(new_sig)),
                    ));
                    continue;
                }
            }
            tampered_caa_msg.answers.push(r.clone());
        }
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CAA,
            tampered_caa_msg,
        );

        let (handle_tampered, ns_list, p) =
            setup_test_pool(provider.clone(), ctx.trust_anchors.clone());
        let verdict_tampered = resolve_byo_dns_with_handles(
            &handle_tampered,
            &ns_list,
            &p,
            "mcp.example.com",
            URI,
            Utc::now(),
        )
        .await;
        assert_eq!(verdict_tampered.code, DnsVerdictCode::DnssecBogus);

        let solstone_tampered = resolve_solstone_me_dns_with_handles(
            &handle_tampered,
            &ns_list,
            &p,
            "mcp.example.com",
            URI,
        )
        .await;
        assert_eq!(solstone_tampered.caa.policy, Some(CaaPolicyCode::Admitted));
        assert_eq!(solstone_tampered.caa.records.len(), 1);

        // 3. Outside validity window (signer duration 1 hour, signed in past) -> SignatureOutsideValidityAtLocalTime
        let expired_ctx = TestDnssecContext::new(origin.clone(), Duration::from_secs(3600));
        let dnskey_exp = expired_ctx.make_dnskey_response(&origin, false);
        provider.script(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::DNSKEY,
            dnskey_exp.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            origin.clone(),
            RecordType::DNSKEY,
            dnskey_exp,
        );

        let signed_a_outside = expired_ctx.sign_rrset(a_rrset.clone(), false);
        let mut a_outside_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_a_outside.records(true) {
            a_outside_msg.answers.push(r.clone());
        }
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::A,
            a_outside_msg,
        );

        let signed_outside = expired_ctx.sign_rrset(rrset.clone(), false);
        let mut caa_outside_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_outside.records(true) {
            caa_outside_msg.answers.push(r.clone());
        }
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CAA,
            caa_outside_msg,
        );

        let nodata_outside =
            expired_ctx.make_nodata_response(&target, vec![RecordType::A, RecordType::CAA], false);
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::AAAA,
            nodata_outside.clone(),
        );
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CNAME,
            nodata_outside.clone(),
        );
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::NS,
            nodata_outside,
        );

        let signed_ns_outside = expired_ctx.sign_rrset(ns_rrset.clone(), false);
        let mut ns_outside_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_ns_outside.records(true) {
            ns_outside_msg.answers.push(r.clone());
        }
        provider.script(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::NS,
            ns_outside_msg.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            origin.clone(),
            RecordType::NS,
            ns_outside_msg,
        );

        let (handle_outside, ns_list, p) =
            setup_test_pool(provider.clone(), expired_ctx.trust_anchors.clone());
        let verdict_outside = resolve_byo_dns_with_handles(
            &handle_outside,
            &ns_list,
            &p,
            "mcp.example.com",
            URI,
            Utc::now(),
        )
        .await;
        assert_eq!(
            verdict_outside.code,
            DnsVerdictCode::SignatureOutsideValidityAtLocalTime
        );
    }

    // Acceptance 2: Stripping probe and indeterminate remapping
    #[tokio::test]
    async fn test_acceptance_2_stripping_probe() {
        let ip1 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
        let root = Name::root();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = MockProvider::default();

        // 1. All servers strip root DNSKEY -> indeterminate, Admitted
        let mut root_dnskey_stripped = Message::new(0, MessageType::Response, OpCode::Query);
        root_dnskey_stripped.metadata.response_code = ResponseCode::NoError;
        let pkcs8 = Ed25519SigningKey::generate_pkcs8().unwrap();
        let key = Ed25519SigningKey::from_pkcs8(&pkcs8).unwrap();
        let pub_key = key.to_public_key().unwrap();
        let dnskey = DNSKEY::from_key(&pub_key);
        root_dnskey_stripped.answers.push(Record::from_rdata(
            root.clone(),
            3600,
            RData::DNSSEC(DNSSECRData::DNSKEY(dnskey.clone())),
        ));

        provider.script(
            ip1,
            Protocol::Udp,
            root.clone(),
            RecordType::DNSKEY,
            root_dnskey_stripped.clone(),
        );
        provider.script(
            ip1,
            Protocol::Tcp,
            root.clone(),
            RecordType::DNSKEY,
            root_dnskey_stripped.clone(),
        );

        let caa_rdata = CAA::new_issue(
            false,
            Some(Name::from_str("letsencrypt.org.").unwrap()),
            vec![
                KeyValue::new("accounturi", URI),
                KeyValue::new("validationmethods", "tls-alpn-01"),
            ],
        );
        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        caa_msg.answers.push(Record::from_rdata(
            target.clone(),
            3600,
            RData::CAA(caa_rdata),
        ));
        provider.script(
            ip1,
            Protocol::Udp,
            target.clone(),
            RecordType::CAA,
            caa_msg.clone(),
        );

        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(
            target.clone(),
            3600,
            RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
        ));
        provider.script(
            ip1,
            Protocol::Udp,
            target.clone(),
            RecordType::A,
            a_msg.clone(),
        );

        let (handle, ns_list, p) =
            setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()));
        let verdict =
            resolve_byo_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI, Utc::now())
                .await;
        assert_eq!(verdict.code, DnsVerdictCode::Admitted);

        // 2. Signed root DNSKEY, but unsigned target in signed zone -> dnssec_bogus
        let ctx = TestDnssecContext::new(root.clone(), Duration::from_secs(86400 * 3));
        let signed_root_dnskey = ctx.make_dnskey_response(&root, true);
        provider.script(
            ip1,
            Protocol::Udp,
            root.clone(),
            RecordType::DNSKEY,
            signed_root_dnskey.clone(),
        );
        provider.script(
            ip1,
            Protocol::Tcp,
            root.clone(),
            RecordType::DNSKEY,
            signed_root_dnskey,
        );

        let (handle_signed_root, ns_list, p) =
            setup_test_pool(provider.clone(), ctx.trust_anchors.clone());
        let verdict_bogus = resolve_byo_dns_with_handles(
            &handle_signed_root,
            &ns_list,
            &p,
            "mcp.example.com",
            URI,
            Utc::now(),
        )
        .await;
        assert_eq!(verdict_bogus.code, DnsVerdictCode::DnssecBogus);

        // 3. One server passes probe, one fails probe -> probe_failed is true -> indeterminate
        let ns1 = NameServerConfig::udp_and_tcp(ip1);
        let ns2 = NameServerConfig::udp_and_tcp(ip2);
        // Server 1 returns signed root DNSKEY (passes probe)
        // Server 2 returns ServFail (fails probe)
        provider.script_err(
            ip2,
            Protocol::Udp,
            root.clone(),
            RecordType::DNSKEY,
            NetError::from(DnsError::ResponseCode(ResponseCode::ServFail)),
        );
        let servers = vec![ns1, ns2];
        let verdict_mixed = resolve_byo_dns_with_handles(
            &handle,
            &servers,
            &provider,
            "mcp.example.com",
            URI,
            Utc::now(),
        )
        .await;
        assert_eq!(verdict_mixed.code, DnsVerdictCode::Admitted);
    }

    // Acceptance 3: DS/DNSKEY transport failure preserves records & yields LookupError on owner, TransportFailure on solstone.me
    #[tokio::test]
    async fn test_acceptance_3_transport_failure() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let root = Name::root();
        let origin = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(root.clone(), Duration::from_secs(86400 * 3));
        let provider = MockProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&root, true);
        provider.script(
            ip,
            Protocol::Udp,
            root.clone(),
            RecordType::DNSKEY,
            dnskey_msg.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            root.clone(),
            RecordType::DNSKEY,
            dnskey_msg,
        );

        // NS record for delegation at example.com.
        let mut ns_msg = Message::new(0, MessageType::Response, OpCode::Query);
        ns_msg.answers.push(Record::from_rdata(
            origin.clone(),
            3600,
            RData::NS(NS(Name::from_utf8("ns1.example.com.").unwrap())),
        ));
        provider.script(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::NS,
            ns_msg.clone(),
        );
        provider.script(ip, Protocol::Tcp, origin.clone(), RecordType::NS, ns_msg);

        provider.script_err(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::DS,
            NetError::from(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "refused",
            )),
        );
        provider.script_err(
            ip,
            Protocol::Tcp,
            origin.clone(),
            RecordType::DS,
            NetError::from(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "refused",
            )),
        );
        provider.script_err(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::DNSKEY,
            NetError::from(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "refused",
            )),
        );
        provider.script_err(
            ip,
            Protocol::Tcp,
            origin.clone(),
            RecordType::DNSKEY,
            NetError::from(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "refused",
            )),
        );

        let caa_rdata =
            CAA::new_issue(false, Some(Name::from_str("otherca.com.").unwrap()), vec![]);
        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        caa_msg.answers.push(Record::from_rdata(
            target.clone(),
            3600,
            RData::CAA(caa_rdata),
        ));
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::CAA, caa_msg);

        let (handle, ns_list, p) = setup_test_pool(provider, ctx.trust_anchors.clone());
        let verdict =
            resolve_byo_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI, Utc::now())
                .await;
        assert_eq!(verdict.code, DnsVerdictCode::LookupError);

        let solstone_res =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_eq!(solstone_res.caa.outcome, DnssecOutcome::TransportFailure);
        assert_eq!(solstone_res.caa.policy, Some(CaaPolicyCode::OtherCa));
        assert_eq!(solstone_res.caa.records.len(), 1);
    }

    // Acceptance 4: NSEC3 insecure delegation -> Insecure -> Admitted
    #[tokio::test]
    async fn test_acceptance_4_nsec3_insecure_delegation() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let root = Name::root();
        let origin = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(root.clone(), Duration::from_secs(86400 * 3));
        let provider = MockProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&root, true);
        provider.script(
            ip,
            Protocol::Udp,
            root.clone(),
            RecordType::DNSKEY,
            dnskey_msg.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            root.clone(),
            RecordType::DNSKEY,
            dnskey_msg,
        );

        // NS record for delegation at example.com.
        let mut ns_msg = Message::new(0, MessageType::Response, OpCode::Query);
        ns_msg.answers.push(Record::from_rdata(
            origin.clone(),
            3600,
            RData::NS(NS(Name::from_utf8("ns1.example.com.").unwrap())),
        ));
        provider.script(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::NS,
            ns_msg.clone(),
        );
        provider.script(ip, Protocol::Tcp, origin.clone(), RecordType::NS, ns_msg);

        // NSEC3 record proving insecure delegation at example.com.
        let salt = Vec::new();
        let hash = Nsec3HashAlgorithm::SHA1.hash(&salt, &origin, 0).unwrap();
        let label = base32_dnssec(hash.as_ref()).to_ascii_lowercase();
        let nsec3_owner = Name::from_utf8(format!("{label}.")).unwrap();

        let nsec3 = NSEC3::new(
            Nsec3HashAlgorithm::SHA1,
            false,
            0,
            salt,
            hash.as_ref().to_vec(),
            vec![RecordType::NS, RecordType::RRSIG],
        );
        let mut rrset = RecordSet::new(nsec3_owner.clone(), RecordType::NSEC3, 3600);
        rrset.insert(
            Record::from_rdata(
                nsec3_owner.clone(),
                3600,
                RData::DNSSEC(DNSSECRData::NSEC3(nsec3)),
            ),
            3600,
        );
        let signed_nsec3 = ctx.sign_rrset(rrset, true);

        let mut ds_msg = Message::new(0, MessageType::Response, OpCode::Query);
        ds_msg.metadata.response_code = ResponseCode::NoError;
        for r in signed_nsec3.records(true) {
            ds_msg.authorities.push(r.clone());
        }
        provider.script(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::DS,
            ds_msg.clone(),
        );
        provider.script(ip, Protocol::Tcp, origin.clone(), RecordType::DS, ds_msg);

        let caa_rdata = CAA::new_issue(
            false,
            Some(Name::from_str("letsencrypt.org.").unwrap()),
            vec![
                KeyValue::new("accounturi", URI),
                KeyValue::new("validationmethods", "tls-alpn-01"),
            ],
        );
        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        caa_msg.answers.push(Record::from_rdata(
            target.clone(),
            3600,
            RData::CAA(caa_rdata),
        ));
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::CAA, caa_msg);

        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(
            target.clone(),
            3600,
            RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
        ));
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        let (handle, ns_list, p) = setup_test_pool(provider, ctx.trust_anchors.clone());
        let verdict =
            resolve_byo_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI, Utc::now())
                .await;
        assert_eq!(verdict.code, DnsVerdictCode::Admitted);
    }

    // Acceptance 5: NSEC NODATA and NXNAME secure denials climb to apex CAA
    #[tokio::test]
    async fn test_acceptance_5_nsec_nodata_and_nxname() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let apex = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(apex.clone(), Duration::from_secs(86400 * 3));
        let provider = MockProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&apex, true);
        provider.script(
            ip,
            Protocol::Udp,
            apex.clone(),
            RecordType::DNSKEY,
            dnskey_msg.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            apex.clone(),
            RecordType::DNSKEY,
            dnskey_msg,
        );

        let nodata = ctx.make_nodata_response(&target, vec![RecordType::A], true);
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CAA,
            nodata.clone(),
        );
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::AAAA,
            nodata.clone(),
        );
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::CNAME, nodata);

        let mut a_rrset = RecordSet::new(target.clone(), RecordType::A, 3600);
        a_rrset.insert(
            Record::from_rdata(
                target.clone(),
                3600,
                RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
            ),
            3600,
        );
        let signed_a = ctx.sign_rrset(a_rrset, true);
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_a.records(true) {
            a_msg.answers.push(r.clone());
        }
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        let caa_rdata = CAA::new_issue(
            false,
            Some(Name::from_str("letsencrypt.org.").unwrap()),
            vec![
                KeyValue::new("accounturi", URI),
                KeyValue::new("validationmethods", "tls-alpn-01"),
            ],
        );
        let mut apex_caa_rrset = RecordSet::new(apex.clone(), RecordType::CAA, 3600);
        apex_caa_rrset.insert(
            Record::from_rdata(apex.clone(), 3600, RData::CAA(caa_rdata)),
            3600,
        );
        let signed_apex_caa = ctx.sign_rrset(apex_caa_rrset, true);
        let mut apex_caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_apex_caa.records(true) {
            apex_caa_msg.answers.push(r.clone());
        }
        provider.script(
            ip,
            Protocol::Udp,
            apex.clone(),
            RecordType::CAA,
            apex_caa_msg,
        );

        let (handle, ns_list, p) = setup_test_pool(provider, ctx.trust_anchors.clone());
        let res =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_eq!(res.caa.policy, Some(CaaPolicyCode::Admitted));
        assert_eq!(res.caa.records.len(), 1);
    }

    // Acceptance 6 & 7: Ancestor climbing logic differences between owner and solstone.me
    #[tokio::test]
    async fn test_acceptance_6_7_climbing_rules() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let root = Name::root();
        let apex = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = MockProvider::default();

        // 1. Stripping resolver -> Indeterminate empty target, admitting apex CAA
        let mut root_dnskey = Message::new(0, MessageType::Response, OpCode::Query);
        let pkcs8 = Ed25519SigningKey::generate_pkcs8().unwrap();
        let key = Ed25519SigningKey::from_pkcs8(&pkcs8).unwrap();
        let pub_key = key.to_public_key().unwrap();
        root_dnskey.answers.push(Record::from_rdata(
            root.clone(),
            3600,
            RData::DNSSEC(DNSSECRData::DNSKEY(DNSKEY::from_key(&pub_key))),
        ));
        provider.script(
            ip,
            Protocol::Udp,
            root.clone(),
            RecordType::DNSKEY,
            root_dnskey.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            root.clone(),
            RecordType::DNSKEY,
            root_dnskey,
        );

        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(
            target.clone(),
            3600,
            RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
        ));
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        let caa_rdata = CAA::new_issue(
            false,
            Some(Name::from_str("letsencrypt.org.").unwrap()),
            vec![
                KeyValue::new("accounturi", URI),
                KeyValue::new("validationmethods", "tls-alpn-01"),
            ],
        );
        let mut apex_caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        apex_caa_msg.answers.push(Record::from_rdata(
            apex.clone(),
            3600,
            RData::CAA(caa_rdata),
        ));
        provider.script(
            ip,
            Protocol::Udp,
            apex.clone(),
            RecordType::CAA,
            apex_caa_msg,
        );

        let (handle, ns_list, p) =
            setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()));

        // Owner resolution climbs on Indeterminate empty target and finds apex CAA -> Admitted
        let verdict =
            resolve_byo_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI, Utc::now())
                .await;
        assert_eq!(verdict.code, DnsVerdictCode::Admitted);
        let recorded = provider.queries_recorded.read().unwrap().clone();
        assert!(recorded.contains(&(apex.clone(), RecordType::CAA)));

        // solstone.me does NOT climb on Indeterminate empty target -> policy None
        provider.queries_recorded.write().unwrap().clear();
        let solstone_res =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_eq!(solstone_res.caa.policy, None);
        let solstone_recorded = provider.queries_recorded.read().unwrap().clone();
        assert!(!solstone_recorded.contains(&(apex.clone(), RecordType::CAA)));
    }

    // Acceptance 8: Secure denials through root -> Missing; Indeterminate/Bogus empty -> None
    #[tokio::test]
    async fn test_acceptance_8_secure_denials_to_root() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let root = Name::root();
        let apex = Name::from_utf8("example.com.").unwrap();
        let com = Name::from_utf8("com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(root.clone(), Duration::from_secs(86400 * 3));
        let provider = MockProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&root, true);
        provider.script(
            ip,
            Protocol::Udp,
            root.clone(),
            RecordType::DNSKEY,
            dnskey_msg.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            root.clone(),
            RecordType::DNSKEY,
            dnskey_msg,
        );

        // Secure denial for target CAA, apex CAA, com CAA, and root CAA
        let nodata_target = ctx.make_nodata_response(&target, vec![RecordType::A], true);
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CAA,
            nodata_target,
        );

        let nodata_apex = ctx.make_nodata_response(&apex, vec![], true);
        provider.script(
            ip,
            Protocol::Udp,
            apex.clone(),
            RecordType::CAA,
            nodata_apex,
        );

        let nodata_com = ctx.make_nodata_response(&com, vec![], true);
        provider.script(ip, Protocol::Udp, com.clone(), RecordType::CAA, nodata_com);

        let nodata_root = ctx.make_nodata_response(&root, vec![], true);
        provider.script(
            ip,
            Protocol::Udp,
            root.clone(),
            RecordType::CAA,
            nodata_root,
        );

        let (handle, ns_list, p) = setup_test_pool(provider, ctx.trust_anchors.clone());
        let res =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_eq!(res.caa.policy, Some(CaaPolicyCode::Missing));
        assert!(res.caa.records.is_empty());
    }

    fn make_raw_caa_wire(qname: &Name, records: &[(u8, &str, &[u8])]) -> Vec<u8> {
        let mut wire = Message::new(0, MessageType::Response, OpCode::Query)
            .to_vec()
            .unwrap();
        let ancount = records.len() as u16;
        wire[6] = (ancount >> 8) as u8;
        wire[7] = (ancount & 0xff) as u8;
        for (flags, tag, value) in records {
            let mut name_bytes = Vec::new();
            {
                let mut encoder = BinEncoder::new(&mut name_bytes);
                qname.emit(&mut encoder).unwrap();
            }
            wire.extend_from_slice(&name_bytes);
            wire.extend_from_slice(&257u16.to_be_bytes()); // type CAA
            wire.extend_from_slice(&1u16.to_be_bytes()); // class IN
            wire.extend_from_slice(&3600u32.to_be_bytes()); // TTL
            let rdlength = 2 + tag.len() + value.len();
            wire.extend_from_slice(&(rdlength as u16).to_be_bytes());
            wire.push(*flags);
            wire.push(tag.len() as u8);
            wire.extend_from_slice(tag.as_bytes());
            wire.extend_from_slice(value);
        }
        wire
    }

    // Acceptance 9: 16-character alphanumeric tag & unreadable CAA through the pool
    #[tokio::test]
    async fn test_acceptance_9_caa_wire_tags() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = MockProvider::default();

        // 1. 16-char tag beside non-admitting issue
        let raw_wire = make_raw_caa_wire(
            &target,
            &[
                (0, "customalphanume1", b"customvalue"),
                (0, "issue", b"otherca.com"),
            ],
        );
        provider.script_wire(ip, Protocol::Udp, target.clone(), RecordType::CAA, raw_wire);

        let (handle, ns_list, p) =
            setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()));
        let res =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_ne!(res.caa.policy, Some(CaaPolicyCode::Admitted));
        assert_ne!(res.caa.outcome, DnssecOutcome::TransportFailure);
        assert_eq!(res.caa.records.len(), 2);

        // 2. Unslicable CAA raw data (< 2 bytes) -> CaaUnreadable
        let mut unreadable_wire = Message::new(0, MessageType::Response, OpCode::Query)
            .to_vec()
            .unwrap();
        // Append a raw CAA record with 1 byte RDATA
        let mut caa_bytes = Vec::new();
        {
            let mut encoder = BinEncoder::new(&mut caa_bytes);
            target.emit(&mut encoder).unwrap();
        }
        caa_bytes.extend_from_slice(&257u16.to_be_bytes()); // type CAA
        caa_bytes.extend_from_slice(&1u16.to_be_bytes()); // class IN
        caa_bytes.extend_from_slice(&3600u32.to_be_bytes()); // TTL
        caa_bytes.extend_from_slice(&1u16.to_be_bytes()); // rdlength 1
        caa_bytes.push(0u8); // 1 byte RDATA (invalid)
        unreadable_wire[6] = 0;
        unreadable_wire[7] = 1; // ancount = 1
        unreadable_wire.extend_from_slice(&caa_bytes);

        provider.script_wire(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CAA,
            unreadable_wire,
        );
        let res_unreadable =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_eq!(
            res_unreadable.caa.policy,
            Some(CaaPolicyCode::CaaUnreadable)
        );
    }

    // Acceptance 10: Merge table combinations
    #[test]
    fn test_acceptance_10_merge_table() {
        assert_eq!(
            DnssecOutcome::Secure.merge(DnssecOutcome::Indeterminate),
            DnssecOutcome::Indeterminate
        );
        assert_eq!(
            DnssecOutcome::Insecure.merge(DnssecOutcome::Indeterminate),
            DnssecOutcome::Indeterminate
        );
        assert_eq!(
            DnssecOutcome::Bogus {
                outside_validity: false
            }
            .merge(DnssecOutcome::Indeterminate),
            DnssecOutcome::Bogus {
                outside_validity: false
            }
        );
        assert_eq!(
            DnssecOutcome::TransportFailure.merge(DnssecOutcome::Secure),
            DnssecOutcome::TransportFailure
        );
    }

    // Acceptance 11: Exact wire parameters validation
    #[tokio::test]
    async fn test_acceptance_11_wire_parameters() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = MockProvider::default();

        let val_admitting =
            format!("letsencrypt.org; accounturi={URI}; validationmethods=tls-alpn-01");
        let wire_ok = make_raw_caa_wire(&target, &[(0, "issue", val_admitting.as_bytes())]);
        provider.script_wire(ip, Protocol::Udp, target.clone(), RecordType::CAA, wire_ok);

        let (handle, ns_list, p) =
            setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()));
        let res_ok =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_eq!(res_ok.caa.policy, Some(CaaPolicyCode::Admitted));

        let val_extra =
            format!("letsencrypt.org; accounturi={URI}; validationmethods=tls-alpn-01; extra=1");
        let wire_extra = make_raw_caa_wire(&target, &[(0, "issue", val_extra.as_bytes())]);
        provider.script_wire(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CAA,
            wire_extra,
        );
        let res_extra =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_ne!(res_extra.caa.policy, Some(CaaPolicyCode::Admitted));
    }

    // Acceptance 12: Address separation and AAAA transport error independence
    #[tokio::test]
    async fn test_acceptance_12_address_separation_and_transport_error() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = MockProvider::default();

        let caa_wire = make_raw_caa_wire(&target, &[(0, "issue", b"otherca.com")]);
        provider.script_wire(ip, Protocol::Udp, target.clone(), RecordType::CAA, caa_wire);

        provider.script_err(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::AAAA,
            NetError::from(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout")),
        );

        let (handle, ns_list, p) = setup_test_pool(provider, Arc::new(TrustAnchors::empty()));
        let res =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;

        assert_eq!(res.address.outcome, DnssecOutcome::TransportFailure);
        assert_eq!(res.caa.outcome, DnssecOutcome::Indeterminate);
        assert_eq!(res.caa.policy, Some(CaaPolicyCode::OtherCa));
    }

    // Acceptance 13: UDP Truncation (TC bit) triggers TCP retry
    #[tokio::test]
    async fn test_acceptance_13_udp_truncation_tcp_retry() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let origin = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(origin.clone(), Duration::from_secs(86400 * 3));
        let provider = MockProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&origin, true);
        provider.script_truncated(
            ip,
            Protocol::Udp,
            origin.clone(),
            RecordType::DNSKEY,
            dnskey_msg.clone(),
        );
        provider.script(
            ip,
            Protocol::Tcp,
            origin.clone(),
            RecordType::DNSKEY,
            dnskey_msg,
        );

        let mut a_rrset = RecordSet::new(target.clone(), RecordType::A, 3600);
        a_rrset.insert(
            Record::from_rdata(
                target.clone(),
                3600,
                RData::A(A(Ipv4Addr::new(93, 184, 216, 34))),
            ),
            3600,
        );
        let signed_a = ctx.sign_rrset(a_rrset, true);
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_a.records(true) {
            a_msg.answers.push(r.clone());
        }
        provider.script(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        let nodata = ctx.make_nodata_response(&target, vec![RecordType::A], true);
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::AAAA,
            nodata.clone(),
        );
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CNAME,
            nodata.clone(),
        );
        provider.script(
            ip,
            Protocol::Udp,
            target.clone(),
            RecordType::CAA,
            nodata.clone(),
        );
        provider.script(ip, Protocol::Udp, origin.clone(), RecordType::CAA, nodata);

        let (handle, ns_list, p) = setup_test_pool(provider, ctx.trust_anchors.clone());
        let res =
            resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
                .await;
        assert_eq!(res.address.a, vec![Ipv4Addr::new(93, 184, 216, 34)]);
    }

    // Acceptance 14 & 15: Pool reuse key and search domain discarded
    #[tokio::test]
    async fn test_acceptance_14_15_pool_reuse_and_search_domain() {
        let ip1 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));

        let mut cfg1 = ResolverConfig::default();
        cfg1.add_name_server(NameServerConfig::udp_and_tcp(ip1));
        let (h1, p1) = get_or_create_prod_pool(&cfg1).unwrap();

        let (h1_again, p1_again) = get_or_create_prod_pool(&cfg1).unwrap();
        assert!(Arc::ptr_eq(p1.context(), p1_again.context()));
        drop(h1);
        drop(h1_again);

        let mut cfg2 = ResolverConfig::default();
        cfg2.add_name_server(NameServerConfig::udp_and_tcp(ip2));
        let (h2, p2) = get_or_create_prod_pool(&cfg2).unwrap();
        assert!(!Arc::ptr_eq(p1.context(), p2.context()));
        drop(h2);

        // Search domain: configured in ResolverConfig, but absolute query names ignore it
        let mut cfg_search = ResolverConfig::default();
        cfg_search.add_name_server(NameServerConfig::udp_and_tcp(ip1));
        cfg_search.add_search(Name::from_utf8("lab.test.").unwrap());

        let provider = MockProvider::default();
        let (handle, ns_list, p) =
            setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()));
        let _ = resolve_solstone_me_dns_with_handles(&handle, &ns_list, &p, "mcp.example.com", URI)
            .await;

        let queries = provider.queries_recorded.read().unwrap().clone();
        for (q, _) in queries {
            assert!(!q.to_utf8().contains("lab.test"));
        }
    }
}
