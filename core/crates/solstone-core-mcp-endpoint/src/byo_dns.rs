// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! BYO owner-hostname DNS evaluation and RFC 8659 CAA policy checking with DNSSEC.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::join_all;
use futures::io::{AsyncRead, AsyncWrite, IoSlice};
use futures::StreamExt;
use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};

use hickory_net::runtime::{DnsTcpStream, DnsUdpSocket, RuntimeProvider, Time, TokioRuntimeProvider};
use hickory_net::xfer::{DnsHandle, Protocol};
use hickory_net::{DnsError, NetError};
use hickory_proto::dnssec::rdata::{DNSSECRData, RRSIG};
use hickory_proto::dnssec::Proof;
use hickory_proto::op::{
    DnsRequest, DnsRequestOptions, Message, MessageType, OpCode, Query, ResponseCode,
};
use hickory_proto::rr::{Name, RData, Record, RecordType, SerialNumber};
use hickory_proto::serialize::binary::{BinDecodable, BinDecoder, BinEncodable};
use hickory_resolver::config::{NameServerConfig, ProtocolConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::net::dnssec::DnssecDnsHandle;
use hickory_resolver::system_conf::read_system_conf;
use hickory_resolver::{NameServerPool, PoolContext, TlsConfig};

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
                "dnssec_bogus_signature_outside_validity_at_local_time"
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

/// Raw CAA record bytes paired with DNSSEC proof counterpart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCaa {
    pub rdata: Vec<u8>,
    pub proof: Option<Proof>,
}

/// Per-level queried CAA outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaaLevel {
    pub name: String,
    pub outcome: DnssecOutcome,
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
    pub found_at: Option<String>,
    pub raw: Vec<RawCaa>,
    pub levels: Vec<CaaLevel>,
}

impl CaaEvidence {
    #[must_use]
    pub fn failure() -> Self {
        Self {
            records: Vec::new(),
            policy: None,
            outcome: DnssecOutcome::TransportFailure,
            found_at: None,
            raw: Vec::new(),
            levels: Vec::new(),
        }
    }
}

/// Host address evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddressEvidence {
    pub a: Vec<Ipv4Addr>,
    pub aaaa: Vec<Ipv6Addr>,
    pub cname: Vec<String>,
    pub outcome: DnssecOutcome,
    pub a_outcome: DnssecOutcome,
    pub aaaa_outcome: DnssecOutcome,
    pub cname_outcome: DnssecOutcome,
}

impl AddressEvidence {
    #[must_use]
    pub fn failure() -> Self {
        Self {
            a: Vec::new(),
            aaaa: Vec::new(),
            cname: Vec::new(),
            outcome: DnssecOutcome::TransportFailure,
            a_outcome: DnssecOutcome::TransportFailure,
            aaaa_outcome: DnssecOutcome::TransportFailure,
            cname_outcome: DnssecOutcome::TransportFailure,
        }
    }

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
    pub caa_cname: Option<String>,
    pub combined_outcome: DnssecOutcome,
    pub signature_outside_validity_at_local_time: Option<bool>,
}

impl SolstoneMeDns {
    #[must_use]
    pub fn failure() -> Self {
        Self {
            caa: CaaEvidence::failure(),
            address: AddressEvidence::failure(),
            caa_cname: None,
            combined_outcome: DnssecOutcome::TransportFailure,
            signature_outside_validity_at_local_time: None,
        }
    }
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
// Wire Capture Layer
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct WireMessage {
    protocol: Protocol,
    peer: SocketAddr,
    id: u16,
    question_name: Option<Name>,
    question_type: Option<RecordType>,
    bytes: Vec<u8>,
}

#[allow(clippy::type_complexity)]
#[derive(Default, Debug)]
struct CaptureLog {
    messages: Mutex<Vec<WireMessage>>,
    outbound_queries: Mutex<HashMap<(SocketAddr, Protocol, u16), (Name, RecordType)>>,
}

impl CaptureLog {
    pub fn clear(&self) {
        self.messages.lock().unwrap().clear();
        self.outbound_queries.lock().unwrap().clear();
    }

    fn record_outbound(
        &self,
        peer: SocketAddr,
        protocol: Protocol,
        id: u16,
        name: Name,
        rtype: RecordType,
    ) {
        self.outbound_queries
            .lock()
            .unwrap()
            .insert((peer, protocol, id), (name, rtype));
    }

    fn record_inbound(&self, peer: SocketAddr, protocol: Protocol, bytes: Vec<u8>) {
        let (id, qname, qtype) = parse_wire_message_header(&bytes);
        let (question_name, question_type) = if let (Some(n), Some(t)) = (qname, qtype) {
            (Some(n), Some(t))
        } else {
            let guard = self.outbound_queries.lock().unwrap();
            if let Some((n, t)) = guard.get(&(peer, protocol, id)) {
                (Some(n.clone()), Some(*t))
            } else {
                (None, None)
            }
        };

        self.messages.lock().unwrap().push(WireMessage {
            protocol,
            peer,
            id,
            question_name,
            question_type,
            bytes,
        });
    }

    pub fn snapshot(&self) -> Vec<WireMessage> {
        self.messages.lock().unwrap().clone()
    }
}

fn parse_wire_message_header(bytes: &[u8]) -> (u16, Option<Name>, Option<RecordType>) {
    if bytes.len() < 12 {
        let id = if bytes.len() >= 2 {
            u16::from_be_bytes([bytes[0], bytes[1]])
        } else {
            0
        };
        return (id, None, None);
    }

    let id = u16::from_be_bytes([bytes[0], bytes[1]]);
    let qdcount = u16::from_be_bytes([bytes[4], bytes[5]]);
    if qdcount == 0 {
        return (id, None, None);
    }

    let mut decoder = BinDecoder::new(bytes);
    if decoder.read_slice(12).is_err() {
        return (id, None, None);
    }

    let Ok(name) = Name::read(&mut decoder) else {
        return (id, None, None);
    };
    let Ok(rtype_u16) = decoder.read_u16() else {
        return (id, Some(name), None);
    };

    (id, Some(name), Some(RecordType::from(rtype_u16.unverified())))
}

/// A `RuntimeProvider` wrapper that captures raw incoming and outgoing DNS wire traffic.
#[derive(Clone)]
struct CapturingProvider<P> {
    inner: P,
    capture_log: Arc<CaptureLog>,
}

#[allow(dead_code)]
impl<P: RuntimeProvider> CapturingProvider<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            capture_log: Arc::new(CaptureLog::default()),
        }
    }

    pub fn with_log(inner: P, capture_log: Arc<CaptureLog>) -> Self {
        Self { inner, capture_log }
    }

    pub fn capture_log(&self) -> &Arc<CaptureLog> {
        &self.capture_log
    }
}

impl<P: RuntimeProvider> RuntimeProvider for CapturingProvider<P> {
    type Handle = P::Handle;
    type Timer = P::Timer;
    type Udp = CapturingUdpSocket<P::Udp>;
    type Tcp = CapturingTcpStream<P::Tcp>;

    fn create_handle(&self) -> Self::Handle {
        self.inner.create_handle()
    }

    fn connect_tcp(
        &self,
        server_addr: SocketAddr,
        bind_addr: Option<SocketAddr>,
        timeout: Option<Duration>,
    ) -> Pin<Box<dyn Send + std::future::Future<Output = Result<Self::Tcp, io::Error>>>> {
        let fut = self.inner.connect_tcp(server_addr, bind_addr, timeout);
        let log = Arc::clone(&self.capture_log);
        Box::pin(async move {
            let tcp = fut.await?;
            Ok(CapturingTcpStream {
                inner: tcp,
                capture_log: log,
                peer: server_addr,
                read_buf: Vec::new(),
                write_buf: Vec::new(),
            })
        })
    }

    fn bind_udp(
        &self,
        local_addr: SocketAddr,
        server_addr: SocketAddr,
    ) -> Pin<Box<dyn Send + std::future::Future<Output = Result<Self::Udp, io::Error>>>> {
        let fut = self.inner.bind_udp(local_addr, server_addr);
        let log = Arc::clone(&self.capture_log);
        Box::pin(async move {
            let udp = fut.await?;
            Ok(CapturingUdpSocket {
                inner: udp,
                capture_log: log,
            })
        })
    }
}

struct CapturingUdpSocket<S> {
    inner: S,
    capture_log: Arc<CaptureLog>,
}

#[async_trait]
impl<S: DnsUdpSocket> DnsUdpSocket for CapturingUdpSocket<S> {
    type Time = S::Time;

    fn poll_recv_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, SocketAddr)>> {
        let res = self.inner.poll_recv_from(cx, buf);
        match res {
            Poll::Ready(Ok((n, src))) => {
                if n > 0 {
                    self.capture_log
                        .record_inbound(src, Protocol::Udp, buf[..n].to_vec());
                }
                Poll::Ready(Ok((n, src)))
            }
            other => other,
        }
    }

    fn poll_send_to(
        &self,
        cx: &mut Context<'_>,
        buf: &[u8],
        target: SocketAddr,
    ) -> Poll<io::Result<usize>> {
        match self.inner.poll_send_to(cx, buf, target) {
            Poll::Ready(Ok(n)) => {
                if n > 0 {
                    let (id, qname, qtype) = parse_wire_message_header(&buf[..n]);
                    if let (Some(qname), Some(qtype)) = (qname, qtype) {
                        self.capture_log
                            .record_outbound(target, Protocol::Udp, id, qname, qtype);
                    }
                }
                Poll::Ready(Ok(n))
            }
            other => other,
        }
    }
}

struct CapturingTcpStream<S> {
    inner: S,
    capture_log: Arc<CaptureLog>,
    peer: SocketAddr,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
}

impl<S: DnsTcpStream> DnsTcpStream for CapturingTcpStream<S> {
    type Time = S::Time;
}

impl<S: AsyncRead + Unpin> AsyncRead for CapturingTcpStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(n)) = res
            && n > 0
        {
            self.read_buf.extend_from_slice(&buf[..n]);
            while self.read_buf.len() >= 2 {
                let msg_len =
                    u16::from_be_bytes([self.read_buf[0], self.read_buf[1]]) as usize;
                if self.read_buf.len() >= 2 + msg_len {
                    let msg_bytes = self.read_buf[2..2 + msg_len].to_vec();
                    self.read_buf.drain(..2 + msg_len);
                    self.capture_log
                        .record_inbound(self.peer, Protocol::Tcp, msg_bytes);
                } else {
                    break;
                }
            }
        }
        res
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CapturingTcpStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = res
            && n > 0
        {
            self.write_buf.extend_from_slice(&buf[..n]);
            while self.write_buf.len() >= 2 {
                let msg_len =
                    u16::from_be_bytes([self.write_buf[0], self.write_buf[1]]) as usize;
                if self.write_buf.len() >= 2 + msg_len {
                    let msg_bytes = self.write_buf[2..2 + msg_len].to_vec();
                    self.write_buf.drain(..2 + msg_len);
                    let (id, qname, qtype) = parse_wire_message_header(&msg_bytes);
                    if let (Some(qname), Some(qtype)) = (qname, qtype) {
                        self.capture_log.record_outbound(
                            self.peer,
                            Protocol::Tcp,
                            id,
                            qname,
                            qtype,
                        );
                    }
                } else {
                    break;
                }
            }
        }
        res
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(n)) = res
            && n > 0
        {
            let mut remaining = n;
            for b in bufs {
                if remaining == 0 {
                    break;
                }
                let to_take = b.len().min(remaining);
                self.write_buf.extend_from_slice(&b[..to_take]);
                remaining -= to_take;
            }
            while self.write_buf.len() >= 2 {
                let msg_len =
                    u16::from_be_bytes([self.write_buf[0], self.write_buf[1]]) as usize;
                if self.write_buf.len() >= 2 + msg_len {
                    let msg_bytes = self.write_buf[2..2 + msg_len].to_vec();
                    self.write_buf.drain(..2 + msg_len);
                    let (id, qname, qtype) = parse_wire_message_header(&msg_bytes);
                    if let (Some(qname), Some(qtype)) = (qname, qtype) {
                        self.capture_log.record_outbound(
                            self.peer,
                            Protocol::Tcp,
                            id,
                            qname,
                            qtype,
                        );
                    }
                } else {
                    break;
                }
            }
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

// ---------------------------------------------------------------------------
// Raw CAA Wire Parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct WireAnswerSection {
    cname: Option<String>,
    caa_rdata: Vec<Vec<u8>>,
    unreadable: bool,
}

/// Slices answer-section type-257 CAA and CNAME records from one discrete DNS message buffer.
fn parse_answer_caa_and_cname(buf: &[u8], query_name: &Name) -> Option<WireAnswerSection> {
    let mut decoder = BinDecoder::new(buf);
    let _id = decoder.read_u16().ok()?;
    let _flags = decoder.read_u16().ok()?;
    let qdcount = decoder.read_u16().ok()?.unverified() as usize;
    let ancount = decoder.read_u16().ok()?.unverified() as usize;
    let _nscount = decoder.read_u16().ok()?;
    let _arcount = decoder.read_u16().ok()?;

    for _ in 0..qdcount {
        Name::read(&mut decoder).ok()?;
        decoder.read_u16().ok()?;
        decoder.read_u16().ok()?;
    }

    let mut caa_rdata = Vec::new();
    let mut cname = None;
    let mut unreadable = false;

    for _ in 0..ancount {
        let name = Name::read(&mut decoder).ok()?;
        let rtype = decoder.read_u16().ok()?.unverified();
        let _rclass = decoder.read_u16().ok()?;
        let _ttl = decoder.read_u32().ok()?;
        let rdlength = decoder.read_u16().ok()?.unverified() as usize;

        if &name == query_name {
            if rtype == 257 {
                let rdata_bytes = decoder.read_slice(rdlength).ok()?.unverified();
                caa_rdata.push(rdata_bytes.to_vec());
                if parse_raw_caa_rdata(rdata_bytes).is_err() {
                    unreadable = true;
                }
            } else if rtype == 5 {
                let start_idx = decoder.index();
                if let Ok(cname_target) = Name::read(&mut decoder) {
                    cname = Some(cname_target.to_utf8().trim_end_matches('.').to_string());
                }
                let read_bytes = decoder.index().saturating_sub(start_idx);
                if read_bytes < rdlength {
                    let _ = decoder.read_slice(rdlength - read_bytes).ok()?;
                }
            } else {
                let _ = decoder.read_slice(rdlength).ok()?;
            }
        } else {
            let _ = decoder.read_slice(rdlength).ok()?;
        }
    }

    Some(WireAnswerSection {
        cname,
        caa_rdata,
        unreadable,
    })
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
    let tag = String::from_utf8(tag_bytes.to_vec()).unwrap_or_default();
    let value_bytes = &rdata_bytes[2 + tag_len..];
    let value = String::from_utf8(value_bytes.to_vec()).unwrap_or_default();
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

fn check_rrsig_outside_validity(rrsigs: &[&RRSIG], now: u32) -> bool {
    if rrsigs.is_empty() {
        return false;
    }

    let now_sn = SerialNumber::new(now);
    rrsigs.iter().all(|s| {
        let inception = SerialNumber::new(s.input().sig_inception.get());
        let expiration = SerialNumber::new(s.input().sig_expiration.get());
        !(now_sn >= inception && now_sn <= expiration)
    })
}

#[derive(Debug)]
struct QueryDnssecResult {
    records: Vec<Record>,
    outcome: DnssecOutcome,
    cname_target: Option<String>,
    raw_caa: Vec<RawCaa>,
    unreadable: bool,
}

/// Executes a validated query through `DnssecDnsHandle`.
async fn query_dnssec<H>(
    handle: &DnssecDnsHandle<H>,
    capture_log: &CaptureLog,
    name: Name,
    record_type: RecordType,
    probe_failed: bool,
) -> QueryDnssecResult
where
    H: hickory_net::xfer::DnsHandle,
    H::Runtime: RuntimeProvider,
{
    let req = make_query_request(name.clone(), record_type);
    let req_id = req.metadata.id;
    let stream_res = handle.send(req).next().await;

    let now_u32 = <<H::Runtime as RuntimeProvider>::Timer as Time>::current_time() as u32;

    match stream_res {
        Some(Ok(response)) => {
            let answers: Vec<Record> = response
                .answers
                .iter()
                .filter(|r| r.record_type() == record_type || r.record_type() == RecordType::CNAME)
                .cloned()
                .collect();

            let target_records: Vec<&Record> = if !answers.is_empty() {
                answers.iter().filter(|r| r.record_type() == record_type).collect()
            } else if !response.authorities.is_empty() {
                response.authorities.iter().collect()
            } else {
                Vec::new()
            };

            // Extract covering RRSIGs for this queried type
            let mut covering_rrsigs = Vec::new();
            for r in response.answers.iter().chain(response.authorities.iter()) {
                if r.record_type() == RecordType::RRSIG
                    && let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &r.data
                    && sig.input().type_covered == record_type
                {
                    covering_rrsigs.push(sig.clone());
                }
            }
            if covering_rrsigs.is_empty() {
                let captures = capture_log.snapshot();
                if let Some(msg) = captures.into_iter().rev().find(|m| {
                    m.id == response.metadata.id
                        && m.question_name.as_ref() == Some(&name)
                }) && let Ok(decoded) = Message::from_vec(&msg.bytes) {
                    for r in decoded.answers.iter().chain(decoded.authorities.iter()) {
                        if r.record_type() == RecordType::RRSIG
                            && let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &r.data
                            && sig.input().type_covered == record_type
                        {
                            covering_rrsigs.push(sig.clone());
                        }
                    }
                }
            }

            let mut all_secure = !target_records.is_empty();
            let mut any_insecure = false;
            let mut any_bogus = false;

            for record in &target_records {
                match record.proof {
                    Proof::Secure => {}
                    Proof::Insecure => {
                        all_secure = false;
                        any_insecure = true;
                    }
                    Proof::Indeterminate => {
                        all_secure = false;
                    }
                    Proof::Bogus => {
                        all_secure = false;
                        any_bogus = true;
                    }
                }
            }

            let outcome = if any_bogus {
                if covering_rrsigs.is_empty() {
                    if probe_failed {
                        DnssecOutcome::Indeterminate
                    } else {
                        DnssecOutcome::Bogus {
                            outside_validity: false,
                        }
                    }
                } else {
                    let sig_refs: Vec<&RRSIG> = covering_rrsigs.iter().collect();
                    let outside = check_rrsig_outside_validity(&sig_refs, now_u32);
                    DnssecOutcome::Bogus {
                        outside_validity: outside,
                    }
                }
            } else if any_insecure {
                DnssecOutcome::Insecure
            } else if all_secure {
                DnssecOutcome::Secure
            } else {
                DnssecOutcome::Indeterminate
            };

            // Capture correlation for CAA
            let mut raw_caa = Vec::new();
            let mut cname_target = None;
            let mut unreadable = false;

            if record_type == RecordType::CAA {
                let captures = capture_log.snapshot();
                let matching: Vec<_> = captures
                    .into_iter()
                    .filter(|m| {
                        m.id == response.metadata.id
                            && m.question_name.as_ref() == Some(&name)
                            && (m.question_type.is_none()
                                || m.question_type == Some(RecordType::CAA))
                    })
                    .collect();

                let mut chosen_msg = None;
                if matching.len() == 1 {
                    chosen_msg = matching.into_iter().next();
                } else if matching.len() > 1 {
                    let mut validated_encoded = Vec::new();
                    for a in answers.iter().filter(|r| r.record_type() == RecordType::CAA) {
                        let mut enc = Vec::new();
                        let mut encoder =
                            hickory_proto::serialize::binary::BinEncoder::new(&mut enc);
                        if a.data.emit(&mut encoder).is_ok() {
                            validated_encoded.push(enc);
                        }
                    }
                    validated_encoded.sort();

                    for msg in matching.iter().rev() {
                        if let Some(parsed) = parse_answer_caa_and_cname(&msg.bytes, &name) {
                            let mut cap_rdata = parsed.caa_rdata.clone();
                            cap_rdata.sort();
                            if !validated_encoded.is_empty() && cap_rdata == validated_encoded {
                                chosen_msg = Some(msg.clone());
                                break;
                            }
                        }
                    }
                }

                if let Some(msg) = chosen_msg
                    && let Some(parsed) = parse_answer_caa_and_cname(&msg.bytes, &name)
                {
                    cname_target = parsed.cname;
                    unreadable = parsed.unreadable;
                    for raw_bytes in parsed.caa_rdata {
                        let matched_proof = answers.iter().find_map(|a| {
                            if a.record_type() == RecordType::CAA {
                                let mut enc = Vec::new();
                                let mut encoder =
                                    hickory_proto::serialize::binary::BinEncoder::new(&mut enc);
                                if a.data.emit(&mut encoder).is_ok() && enc == raw_bytes {
                                    return Some(a.proof);
                                }
                            }
                            None
                        });
                        raw_caa.push(RawCaa {
                            rdata: raw_bytes,
                            proof: matched_proof,
                        });
                    }
                }
            }

            QueryDnssecResult {
                records: answers,
                outcome,
                cname_target,
                raw_caa,
                unreadable,
            }
        }
        Some(Err(NetError::Dns(DnsError::Nsec { proof, .. }))) => {
            let outcome = match proof {
                Proof::Secure => DnssecOutcome::Secure,
                Proof::Insecure => DnssecOutcome::Insecure,
                Proof::Indeterminate => DnssecOutcome::Indeterminate,
                Proof::Bogus => {
                    let captures = capture_log.snapshot();
                    let matching = captures.into_iter().rev().find(|m| {
                        m.id == req_id || m.question_name.as_ref() == Some(&name)
                    });
                    let mut covering_rrsigs = Vec::new();
                    if let Some(msg) = matching
                        && let Ok(decoded) = Message::from_vec(&msg.bytes)
                    {
                        for r in decoded.answers.iter().chain(decoded.authorities.iter()) {
                            if r.record_type() == RecordType::RRSIG
                                && let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &r.data
                            {
                                covering_rrsigs.push(sig.clone());
                            }
                        }
                    }
                    if covering_rrsigs.is_empty() {
                        if probe_failed {
                            DnssecOutcome::Indeterminate
                        } else {
                            DnssecOutcome::Bogus {
                                outside_validity: false,
                            }
                        }
                    } else {
                        let sig_refs: Vec<&RRSIG> = covering_rrsigs.iter().collect();
                        let outside = check_rrsig_outside_validity(&sig_refs, now_u32);
                        DnssecOutcome::Bogus {
                            outside_validity: outside,
                        }
                    }
                }
            };
            QueryDnssecResult {
                records: Vec::new(),
                outcome,
                cname_target: None,
                raw_caa: Vec::new(),
                unreadable: false,
            }
        }
        _ => {
            // Validator errored (or TCP multiplexer dropped message)
            if record_type == RecordType::CAA {
                let captures = capture_log.snapshot();
                let matching = captures.into_iter().rev().find(|m| {
                    (m.id == req_id || m.question_name.as_ref() == Some(&name))
                        && (m.question_type.is_none() || m.question_type == Some(RecordType::CAA))
                });

                if let Some(msg) = matching
                    && let Some(parsed) = parse_answer_caa_and_cname(&msg.bytes, &name)
                {
                    let raw_caa: Vec<RawCaa> = parsed
                        .caa_rdata
                        .into_iter()
                        .map(|rdata| RawCaa { rdata, proof: None })
                        .collect();

                    let mut covering_rrsigs = Vec::new();
                    if let Ok(decoded) = Message::from_vec(&msg.bytes) {
                        for r in decoded.answers.iter().chain(decoded.authorities.iter()) {
                            if r.record_type() == RecordType::RRSIG
                                && let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &r.data
                                && sig.input().type_covered == RecordType::CAA
                            {
                                covering_rrsigs.push(sig.clone());
                            }
                        }
                    }

                    let outcome = if !covering_rrsigs.is_empty() {
                        let sig_refs: Vec<&RRSIG> = covering_rrsigs.iter().collect();
                        let outside = check_rrsig_outside_validity(&sig_refs, now_u32);
                        DnssecOutcome::Bogus {
                            outside_validity: outside,
                        }
                    } else if probe_failed {
                        DnssecOutcome::Indeterminate
                    } else {
                        DnssecOutcome::Bogus {
                            outside_validity: false,
                        }
                    };

                    if !raw_caa.is_empty() || parsed.unreadable || parsed.cname.is_some() {
                        return QueryDnssecResult {
                            records: Vec::new(),
                            outcome,
                            cname_target: parsed.cname,
                            raw_caa,
                            unreadable: parsed.unreadable,
                        };
                    }
                }
            }

            QueryDnssecResult {
                records: Vec::new(),
                outcome: DnssecOutcome::TransportFailure,
                cname_target: None,
                raw_caa: Vec::new(),
                unreadable: false,
            }
        }
    }
}

/// Stripping probe: sends an unvalidated `. DNSKEY` query to all configured servers concurrently.
async fn probe_dnssec_stripping<P: RuntimeProvider>(
    name_servers: &[NameServerConfig],
    provider: &P,
) -> bool {
    let mut opts = ResolverOpts::default();
    opts.timeout = Duration::from_millis(800);
    opts.attempts = 1;

    let probe_futures: Vec<_> = name_servers
        .iter()
        .map(|ns| {
            let tls_config = match TlsConfig::new() {
                Ok(c) => c,
                Err(_) => {
                    return tokio::spawn(async { true });
                }
            };
            let single_pool = NameServerPool::from_config(
                vec![ns.clone()],
                Arc::new(PoolContext::new(opts.clone(), tls_config)),
                provider.clone(),
            );
            let pool = single_pool;
            tokio::spawn(async move {
                let req = make_query_request(Name::root(), RecordType::DNSKEY);
                let mut stream = pool.send(req);

                let res = match stream.next().await {
                    Some(Ok(response)) => {
                        if response.metadata.response_code != ResponseCode::NoError {
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
                            !(has_dnskey && has_rrsig)
                        }
                    }
                    Some(Err(NetError::Timeout)) | None => {
                        // Retry once on timeout/error
                        let retry_req = make_query_request(Name::root(), RecordType::DNSKEY);
                        let mut retry_stream = pool.send(retry_req);
                        match retry_stream.next().await {
                            Some(Ok(response))
                                if response.metadata.response_code == ResponseCode::NoError =>
                            {
                                let has_dnskey = response
                                    .answers
                                    .iter()
                                    .any(|r| r.record_type() == RecordType::DNSKEY);
                                let has_rrsig = response
                                    .answers
                                    .iter()
                                    .any(|r| r.record_type() == RecordType::RRSIG);
                                !(has_dnskey && has_rrsig)
                            }
                            _ => true,
                        }
                    }
                    Some(Err(_)) => true,
                };
                drop(pool);
                res
            })
        })
        .collect();

    let results = join_all(probe_futures).await;
    results.into_iter().any(|res| res.unwrap_or(true))
}

fn protocol_id(p: &ProtocolConfig) -> u8 {
    match p {
        ProtocolConfig::Udp => 0,
        ProtocolConfig::Tcp => 1,
    }
}

struct ProductionPool {
    nameservers: Vec<(IpAddr, u16, u8)>,
    handle: DnssecDnsHandle<NameServerPool<CapturingProvider<TokioRuntimeProvider>>>,
    capture_log: Arc<CaptureLog>,
}

static PROD_POOL: Mutex<Option<ProductionPool>> = Mutex::new(None);

#[allow(clippy::type_complexity)]
fn get_or_create_prod_pool(
    config: &ResolverConfig,
) -> Result<
    (
        DnssecDnsHandle<NameServerPool<CapturingProvider<TokioRuntimeProvider>>>,
        Arc<CaptureLog>,
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
        return Ok((entry.handle.clone(), Arc::clone(&entry.capture_log)));
    }

    let mut opts = ResolverOpts::default();
    opts.timeout = Duration::from_millis(800);
    opts.attempts = 1;

    let tls_config = TlsConfig::new().map_err(|e| e.to_string())?;
    let cx = Arc::new(PoolContext::new(opts, tls_config));
    let capture_log = Arc::new(CaptureLog::default());
    let capturing_provider =
        CapturingProvider::with_log(TokioRuntimeProvider::default(), Arc::clone(&capture_log));

    let pool = NameServerPool::from_config(
        config.name_servers().iter().cloned(),
        cx,
        capturing_provider,
    );

    let handle = DnssecDnsHandle::new(pool);

    *guard = Some(ProductionPool {
        nameservers: ns_list,
        handle: handle.clone(),
        capture_log: Arc::clone(&capture_log),
    });

    Ok((handle, capture_log))
}

// ---------------------------------------------------------------------------
// Resolution Workflows: solstone.me and Owner
// ---------------------------------------------------------------------------

/// Resolves DNS and CAA evidence for solstone.me.
pub async fn resolve_solstone_me_dns(hostname: &str, account_uri: &str) -> SolstoneMeDns {
    let config_res = tokio::task::spawn_blocking(read_system_conf).await;
    let (config, _) = match config_res {
        Ok(Ok(pair)) => pair,
        _ => return SolstoneMeDns::failure(),
    };

    let (handle, capture_log) = match get_or_create_prod_pool(&config) {
        Ok(pair) => pair,
        Err(_) => return SolstoneMeDns::failure(),
    };
    capture_log.clear();

    resolve_solstone_me_dns_with_handles(
        &handle,
        &capture_log,
        &config,
        &TokioRuntimeProvider::default(),
        hostname,
        account_uri,
    )
    .await
}

async fn resolve_solstone_me_dns_with_handles<P: RuntimeProvider>(
    handle: &DnssecDnsHandle<NameServerPool<CapturingProvider<P>>>,
    capture_log: &CaptureLog,
    config: &ResolverConfig,
    provider: &P,
    hostname: &str,
    account_uri: &str,
) -> SolstoneMeDns {
    match tokio::time::timeout(
        Duration::from_secs(5),
        resolve_solstone_me_dns_inner(handle, capture_log, config, provider, hostname, account_uri),
    )
    .await
    {
        Ok(dns) => dns,
        Err(_) => SolstoneMeDns::failure(),
    }
}

async fn resolve_solstone_me_dns_inner<P: RuntimeProvider>(
    handle: &DnssecDnsHandle<NameServerPool<CapturingProvider<P>>>,
    capture_log: &CaptureLog,
    config: &ResolverConfig,
    provider: &P,
    hostname: &str,
    account_uri: &str,
) -> SolstoneMeDns {
    let probe_failed = probe_dnssec_stripping(config.name_servers(), provider).await;
    let clean_host = hostname.trim_end_matches('.');
    let target_name = match Name::from_utf8(format!("{clean_host}.")) {
        Ok(n) => n,
        Err(_) => return SolstoneMeDns::failure(),
    };

    let a_fut = query_dnssec(handle, capture_log, target_name.clone(), RecordType::A, probe_failed);
    let aaaa_fut = query_dnssec(
        handle,
        capture_log,
        target_name.clone(),
        RecordType::AAAA,
        probe_failed,
    );
    let cname_fut = query_dnssec(
        handle,
        capture_log,
        target_name.clone(),
        RecordType::CNAME,
        probe_failed,
    );
    let caa_fut = climb_caa_solstone(handle, capture_log, clean_host, account_uri, probe_failed);

    let (a_res, aaaa_res, cname_res, (caa_evidence, caa_cname)) =
        tokio::join!(a_fut, aaaa_fut, cname_fut, caa_fut);

    let mut addr_a = Vec::new();
    for r in a_res.records {
        if let RData::A(ip) = &r.data {
            addr_a.push(ip.0);
        }
    }
    let mut addr_aaaa = Vec::new();
    for r in aaaa_res.records {
        if let RData::AAAA(ip) = &r.data {
            addr_aaaa.push(ip.0);
        }
    }
    let mut addr_cname = Vec::new();
    for r in cname_res.records {
        if let RData::CNAME(name) = &r.data {
            addr_cname.push(name.to_utf8().trim_end_matches('.').to_string());
        }
    }

    let addr_outcome = a_res.outcome.merge(aaaa_res.outcome).merge(cname_res.outcome);
    let address = AddressEvidence {
        a: addr_a,
        aaaa: addr_aaaa,
        cname: addr_cname,
        outcome: addr_outcome,
        a_outcome: a_res.outcome,
        aaaa_outcome: aaaa_res.outcome,
        cname_outcome: cname_res.outcome,
    };

    let combined_outcome = addr_outcome.merge(caa_evidence.outcome);
    let signature_outside_validity_at_local_time = match combined_outcome {
        DnssecOutcome::Bogus {
            outside_validity: true,
        } => Some(true),
        _ => None,
    };

    SolstoneMeDns {
        caa: caa_evidence,
        address,
        caa_cname,
        combined_outcome,
        signature_outside_validity_at_local_time,
    }
}

async fn climb_caa_solstone<P: RuntimeProvider>(
    handle: &DnssecDnsHandle<NameServerPool<CapturingProvider<P>>>,
    capture_log: &CaptureLog,
    hostname: &str,
    account_uri: &str,
    probe_failed: bool,
) -> (CaaEvidence, Option<String>) {
    let mut current = hostname;
    let mut accumulated_outcome = DnssecOutcome::Secure;
    let mut all_denials_authenticated = true;
    let mut levels = Vec::new();

    loop {
        let Ok(qname) = Name::from_utf8(format!("{current}.")) else {
            return (
                CaaEvidence {
                    records: Vec::new(),
                    policy: None,
                    outcome: DnssecOutcome::TransportFailure,
                    found_at: None,
                    raw: Vec::new(),
                    levels,
                },
                None,
            );
        };

        let res = query_dnssec(handle, capture_log, qname.clone(), RecordType::CAA, probe_failed).await;
        accumulated_outcome = accumulated_outcome.merge(res.outcome);
        levels.push(CaaLevel {
            name: current.to_string(),
            outcome: res.outcome,
        });

        // Sliced CAA records from captured wire
        let mut sliced_records = Vec::new();
        for raw in &res.raw_caa {
            if let Ok(rec) = parse_raw_caa_rdata(&raw.rdata) {
                sliced_records.push(rec);
            }
        }

        if !sliced_records.is_empty() || res.unreadable {
            let mut policy = evaluate_solstone_caa_policy(&sliced_records, account_uri, res.unreadable);
            // Admission gate: if policy is Admitted but any record has no validated counterpart (proof == None), do not admit
            if policy == Some(CaaPolicyCode::Admitted)
                && res.raw_caa.iter().any(|r| r.proof.is_none())
            {
                policy = None;
            }
            return (
                CaaEvidence {
                    records: sliced_records,
                    policy,
                    outcome: accumulated_outcome,
                    found_at: Some(current.to_string()),
                    raw: res.raw_caa,
                    levels,
                },
                None,
            );
        }

        if let Some(cname_target) = res.cname_target {
            // CNAME at the CAA label: stop walk, do not query parent
            return (
                CaaEvidence {
                    records: Vec::new(),
                    policy: None,
                    outcome: accumulated_outcome,
                    found_at: Some(current.to_string()),
                    raw: res.raw_caa,
                    levels,
                },
                Some(cname_target),
            );
        }

        match res.outcome {
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

    (
        CaaEvidence {
            records: Vec::new(),
            policy,
            outcome: accumulated_outcome,
            found_at: None,
            raw: Vec::new(),
            levels,
        },
        None,
    )
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

    let config_res = tokio::task::spawn_blocking(read_system_conf).await;
    let (config, _) = match config_res {
        Ok(Ok(pair)) => pair,
        _ => return DnsVerdict::new(DnsVerdictCode::LookupError, now),
    };

    let (handle, capture_log) = match get_or_create_prod_pool(&config) {
        Ok(pair) => pair,
        Err(_) => return DnsVerdict::new(DnsVerdictCode::LookupError, now),
    };
    capture_log.clear();

    match tokio::time::timeout(
        Duration::from_secs(5),
        resolve_byo_dns_with_handles(
            &handle,
            &capture_log,
            &config,
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

async fn resolve_byo_dns_with_handles<P: RuntimeProvider>(
    handle: &DnssecDnsHandle<NameServerPool<CapturingProvider<P>>>,
    capture_log: &CaptureLog,
    config: &ResolverConfig,
    provider: &P,
    hostname: &str,
    account_uri: &str,
    now: DateTime<Utc>,
) -> DnsVerdict {
    let probe_failed = probe_dnssec_stripping(config.name_servers(), provider).await;
    let clean_host = hostname.trim_end_matches('.');
    let target_name = match Name::from_utf8(format!("{clean_host}.")) {
        Ok(n) => n,
        Err(_) => return DnsVerdict::new(DnsVerdictCode::LookupError, now),
    };

    let a_fut = query_dnssec(handle, capture_log, target_name.clone(), RecordType::A, probe_failed);
    let aaaa_fut = query_dnssec(
        handle,
        capture_log,
        target_name.clone(),
        RecordType::AAAA,
        probe_failed,
    );
    let cname_fut = query_dnssec(
        handle,
        capture_log,
        target_name.clone(),
        RecordType::CNAME,
        probe_failed,
    );
    let caa_fut = climb_caa_owner(handle, capture_log, clean_host, probe_failed);

    let (a_res, aaaa_res, cname_res, (found_caa, caa_outcome, caa_unverified, caa_cname)) =
        tokio::join!(a_fut, aaaa_fut, cname_fut, caa_fut);

    let mut map = HashMap::new();
    let mut target_host_records = HostDnsRecords::default();

    for r in a_res.records {
        if let RData::A(ip) = &r.data {
            target_host_records.a.push(ip.0);
        }
    }
    for r in aaaa_res.records {
        if let RData::AAAA(ip) = &r.data {
            target_host_records.aaaa.push(ip.0);
        }
    }
    for r in cname_res.records {
        if let RData::CNAME(name) = &r.data {
            target_host_records
                .cname
                .push(name.to_utf8().trim_end_matches('.').to_string());
        }
    }

    if let Some(cname_alias) = caa_cname {
        target_host_records.cname.push(cname_alias);
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

    let combined_outcome = a_res
        .outcome
        .merge(aaaa_res.outcome)
        .merge(cname_res.outcome)
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
            let verdict = evaluate_byo_dns_policy(clean_host, account_uri, &map, now);
            if verdict.code == DnsVerdictCode::Admitted && caa_unverified {
                DnsVerdict::new(DnsVerdictCode::LookupError, now)
            } else {
                verdict
            }
        }
    }
}

async fn climb_caa_owner<P: RuntimeProvider>(
    handle: &DnssecDnsHandle<NameServerPool<CapturingProvider<P>>>,
    capture_log: &CaptureLog,
    hostname: &str,
    probe_failed: bool,
) -> (Option<(String, Vec<CaaRecord>)>, DnssecOutcome, bool, Option<String>) {
    let mut current = hostname;
    let mut accumulated_outcome = DnssecOutcome::Secure;

    loop {
        let Ok(qname) = Name::from_utf8(format!("{current}.")) else {
            return (None, DnssecOutcome::TransportFailure, false, None);
        };

        let res = query_dnssec(handle, capture_log, qname.clone(), RecordType::CAA, probe_failed).await;
        accumulated_outcome = accumulated_outcome.merge(res.outcome);

        let mut parsed_caa = Vec::new();
        for raw in &res.raw_caa {
            if let Ok(rec) = parse_raw_caa_rdata(&raw.rdata) {
                parsed_caa.push(rec);
            }
        }

        let unverified = res.raw_caa.iter().any(|r| r.proof.is_none());

        if !parsed_caa.is_empty() || res.unreadable {
            return (
                Some((current.to_string(), parsed_caa)),
                accumulated_outcome,
                unverified,
                None,
            );
        }

        if let Some(cname_target) = res.cname_target {
            return (None, accumulated_outcome, false, Some(cname_target));
        }

        match res.outcome {
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

    (None, accumulated_outcome, false, None)
}

// ---------------------------------------------------------------------------
// Unit Tests & In-Memory Mock Harness
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet, VecDeque};
    use std::str::FromStr;
    use std::sync::atomic::{AtomicU64, Ordering};

    use hickory_proto::dnssec::crypto::Ed25519SigningKey;
    use hickory_proto::dnssec::rdata::nsec::NSEC;
    use hickory_proto::dnssec::rdata::nsec3::NSEC3;
    use hickory_proto::dnssec::rdata::{DNSKEY, RRSIG};
    use hickory_proto::dnssec::{DnssecSigner, Nsec3HashAlgorithm, SigningKey, TrustAnchors};
    use hickory_proto::op::{Message, ResponseCode};
    use hickory_proto::rr::rdata::a::A;
    use hickory_proto::rr::rdata::caa::{CAA, KeyValue};
    use hickory_proto::rr::rdata::NS;
    use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordSet, RecordType};
    use hickory_proto::serialize::binary::BinEncodable;
    use hickory_resolver::config::NameServerConfig;

    const URI: &str = "https://acme-v02.api.letsencrypt.org/acme/acct/12345678";
    const HOST: &str = "mcp.example.com";
    const PINNED_TIMESTAMP: u64 = 1_700_000_000;

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

    // -----------------------------------------------------------------------
    // Scripted In-Memory Transport & Provider
    // -----------------------------------------------------------------------

    #[derive(Clone, Copy, Debug)]
    pub struct PinnedTime;

    static PINNED_CLOCK: AtomicU64 = AtomicU64::new(PINNED_TIMESTAMP);

    #[async_trait]
    impl Time for PinnedTime {
        async fn delay_for(duration: Duration) {
            tokio::time::sleep(duration).await
        }

        async fn timeout<F: 'static + std::future::Future + Send>(
            duration: Duration,
            future: F,
        ) -> Result<F::Output, io::Error> {
            tokio::time::timeout(duration, future)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "future timed out"))
        }

        fn current_time() -> u64 {
            PINNED_CLOCK.load(Ordering::SeqCst)
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct RecordedQuery {
        pub peer: SocketAddr,
        pub protocol: Protocol,
        pub name: Name,
        pub rtype: RecordType,
    }

    use std::sync::atomic::AtomicUsize;

    #[allow(dead_code)]
    #[derive(Clone)]
    pub enum ScriptReply {
        Bytes(Vec<u8>),
        Truncated(Vec<u8>),
        Error(io::ErrorKind),
        Timeout,
        WithholdUntilQueries(usize, Vec<u8>),
    }

    type ScriptKey = (SocketAddr, Protocol, Name, RecordType);

    type ActiveSocketEntry = (
        SocketAddr,
        Arc<Mutex<VecDeque<Vec<u8>>>>,
        Arc<Mutex<Option<(ScriptKey, u16)>>>,
    );

    static NEXT_SOCKET_ID: AtomicUsize = AtomicUsize::new(1);

    #[derive(Clone, Default)]
    pub struct ScriptedProvider {
        handle: hickory_net::runtime::TokioHandle,
        scripts: Arc<Mutex<HashMap<ScriptKey, ScriptReply>>>,
        latest_query_id: Arc<Mutex<HashMap<ScriptKey, u16>>>,
        recorded_queries: Arc<Mutex<Vec<RecordedQuery>>>,
        drop_first_probe: Arc<Mutex<HashSet<SocketAddr>>>,
        active_udp_sockets: Arc<Mutex<HashMap<usize, ActiveSocketEntry>>>,
        wakers: Arc<Mutex<Vec<std::task::Waker>>>,
    }

    impl ScriptedProvider {
        pub fn script_msg(
            &self,
            peer: SocketAddr,
            proto: Protocol,
            name: Name,
            rtype: RecordType,
            mut msg: Message,
        ) {
            msg.queries.clear();
            msg.queries.push(Query::query(name.clone(), rtype));
            let bytes = msg.to_vec().unwrap();
            self.scripts
                .lock()
                .unwrap()
                .insert((peer, proto, name, rtype), ScriptReply::Bytes(bytes));
        }

        pub fn script_truncated_msg(
            &self,
            peer: SocketAddr,
            proto: Protocol,
            name: Name,
            rtype: RecordType,
            mut msg: Message,
        ) {
            msg.queries.clear();
            msg.queries.push(Query::query(name.clone(), rtype));
            let bytes = msg.to_vec().unwrap();
            self.scripts
                .lock()
                .unwrap()
                .insert((peer, proto, name, rtype), ScriptReply::Truncated(bytes));
        }

        pub fn script_raw(
            &self,
            peer: SocketAddr,
            proto: Protocol,
            name: Name,
            rtype: RecordType,
            bytes: Vec<u8>,
        ) {
            self.scripts
                .lock()
                .unwrap()
                .insert((peer, proto, name, rtype), ScriptReply::Bytes(bytes));
        }

        #[allow(dead_code)]
        pub fn script_err(
            &self,
            peer: SocketAddr,
            proto: Protocol,
            name: Name,
            rtype: RecordType,
            kind: io::ErrorKind,
        ) {
            self.scripts
                .lock()
                .unwrap()
                .insert((peer, proto, name, rtype), ScriptReply::Error(kind));
        }

        #[allow(dead_code)]
        pub fn script_timeout(
            &self,
            peer: SocketAddr,
            proto: Protocol,
            name: Name,
            rtype: RecordType,
        ) {
            self.scripts
                .lock()
                .unwrap()
                .insert((peer, proto, name, rtype), ScriptReply::Timeout);
        }

        pub fn recorded_queries(&self) -> Vec<RecordedQuery> {
            self.recorded_queries.lock().unwrap().clone()
        }
    }

    impl RuntimeProvider for ScriptedProvider {
        type Handle = hickory_net::runtime::TokioHandle;
        type Timer = PinnedTime;
        type Udp = ScriptedUdpSocket;
        type Tcp = ScriptedTcpStream;

        fn create_handle(&self) -> Self::Handle {
            self.handle.clone()
        }

        fn connect_tcp(
            &self,
            server_addr: SocketAddr,
            _bind_addr: Option<SocketAddr>,
            _timeout: Option<Duration>,
        ) -> Pin<Box<dyn Send + std::future::Future<Output = Result<Self::Tcp, io::Error>>>> {
            let provider = self.clone();
            Box::pin(async move {
                Ok(ScriptedTcpStream {
                    peer: server_addr,
                    provider,
                    read_buf: Vec::new(),
                    write_buf: Vec::new(),
                })
            })
        }

        fn bind_udp(
            &self,
            _local_addr: SocketAddr,
            server_addr: SocketAddr,
        ) -> Pin<Box<dyn Send + std::future::Future<Output = Result<Self::Udp, io::Error>>>> {
            let provider = self.clone();
            Box::pin(async move {
                let socket_id = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
                let inbound_queue = Arc::new(Mutex::new(VecDeque::new()));
                let last_query = Arc::new(Mutex::new(None));
                provider.active_udp_sockets.lock().unwrap().insert(
                    socket_id,
                    (server_addr, Arc::clone(&inbound_queue), Arc::clone(&last_query)),
                );
                Ok(ScriptedUdpSocket {
                    peer: server_addr,
                    provider,
                    socket_id,
                    inbound_queue,
                    last_query,
                })
            })
        }
    }

    pub struct ScriptedUdpSocket {
        peer: SocketAddr,
        provider: ScriptedProvider,
        socket_id: usize,
        inbound_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
        last_query: Arc<Mutex<Option<(ScriptKey, u16)>>>,
    }

    impl Drop for ScriptedUdpSocket {
        fn drop(&mut self) {
            self.provider
                .active_udp_sockets
                .lock()
                .unwrap()
                .remove(&self.socket_id);
        }
    }

    #[async_trait]
    impl DnsUdpSocket for ScriptedUdpSocket {
        type Time = PinnedTime;

        fn poll_recv_from(
            &self,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<(usize, SocketAddr)>> {
            let mut guard = self.inbound_queue.lock().unwrap();
            if let Some(resp) = guard.pop_front() {
                let len = resp.len().min(buf.len());
                buf[..len].copy_from_slice(&resp[..len]);
                Poll::Ready(Ok((len, self.peer)))
            } else {
                let mut wakers = self.provider.wakers.lock().unwrap();
                if !wakers.iter().any(|w| w.will_wake(cx.waker())) {
                    wakers.push(cx.waker().clone());
                }
                Poll::Pending
            }
        }

        fn poll_send_to(
            &self,
            _cx: &mut Context<'_>,
            buf: &[u8],
            target: SocketAddr,
        ) -> Poll<io::Result<usize>> {
            let (id, qname, qtype) = parse_wire_message_header(buf);
            let Some(qname) = qname else {
                return Poll::Ready(Ok(buf.len()));
            };
            let Some(qtype) = qtype else {
                return Poll::Ready(Ok(buf.len()));
            };

            let script_key = (target, Protocol::Udp, qname.clone(), qtype);
            *self.last_query.lock().unwrap() = Some((script_key.clone(), id));

            self.provider
                .latest_query_id
                .lock()
                .unwrap()
                .insert(script_key.clone(), id);

            self.provider
                .recorded_queries
                .lock()
                .unwrap()
                .push(RecordedQuery {
                    peer: target,
                    protocol: Protocol::Udp,
                    name: qname.clone(),
                    rtype: qtype,
                });

            // Handle dropped probe datagram test case
            {
                let mut drops = self.provider.drop_first_probe.lock().unwrap();
                if drops.remove(&target) {
                    return Poll::Ready(Ok(buf.len()));
                }
            }

            let guard = self.provider.scripts.lock().unwrap();
            let count = self.provider.recorded_queries.lock().unwrap().len();

            if let Some(reply) = guard.get(&script_key) {
                match reply {
                    ScriptReply::Bytes(bytes) => {
                        let mut resp = bytes.clone();
                        if resp.len() >= 2 {
                            let id_bytes = id.to_be_bytes();
                            resp[0] = id_bytes[0];
                            resp[1] = id_bytes[1];
                        }
                        self.inbound_queue.lock().unwrap().push_back(resp);
                    }
                    ScriptReply::Truncated(bytes) => {
                        let mut resp = bytes.clone();
                        if resp.len() >= 4 {
                            let id_bytes = id.to_be_bytes();
                            resp[0] = id_bytes[0];
                            resp[1] = id_bytes[1];
                            resp[2] |= 0x02; // Set TC bit
                        }
                        self.inbound_queue.lock().unwrap().push_back(resp);
                    }
                    ScriptReply::Error(k) => {
                        return Poll::Ready(Err(io::Error::new(*k, "simulated error")));
                    }
                    ScriptReply::Timeout => {}
                    ScriptReply::WithholdUntilQueries(min_count, bytes) => {
                        if count >= *min_count {
                            let mut resp = bytes.clone();
                            if resp.len() >= 2 {
                                let id_bytes = id.to_be_bytes();
                                resp[0] = id_bytes[0];
                                resp[1] = id_bytes[1];
                            }
                            self.inbound_queue.lock().unwrap().push_back(resp);
                        }
                    }
                }
            }

            // Check if any withheld queries across all active sockets are now satisfied
            let active = self.provider.active_udp_sockets.lock().unwrap().clone();
            for (_sock_id, (_peer, queue, last_q)) in active {
                if let Some((key, query_id)) = &*last_q.lock().unwrap()
                    && let Some(ScriptReply::WithholdUntilQueries(min_count, bytes)) =
                        guard.get(key)
                    && count >= *min_count
                {
                    let mut resp = bytes.clone();
                    if resp.len() >= 2 {
                        let id_bytes = query_id.to_be_bytes();
                        resp[0] = id_bytes[0];
                        resp[1] = id_bytes[1];
                    }
                    let mut q_guard = queue.lock().unwrap();
                    if q_guard.is_empty() {
                        q_guard.push_back(resp);
                    }
                }
            }

            let wakers: Vec<std::task::Waker> =
                self.provider.wakers.lock().unwrap().drain(..).collect();
            for w in wakers {
                w.wake();
            }

            Poll::Ready(Ok(buf.len()))
        }
    }

    pub struct ScriptedTcpStream {
        peer: SocketAddr,
        provider: ScriptedProvider,
        read_buf: Vec<u8>,
        write_buf: Vec<u8>,
    }

    impl DnsTcpStream for ScriptedTcpStream {
        type Time = PinnedTime;
    }

    impl AsyncRead for ScriptedTcpStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            if self.read_buf.is_empty() {
                let mut wakers = self.provider.wakers.lock().unwrap();
                if !wakers.iter().any(|w| w.will_wake(cx.waker())) {
                    wakers.push(cx.waker().clone());
                }
                Poll::Pending
            } else {
                let len = self.read_buf.len().min(buf.len());
                buf[..len].copy_from_slice(&self.read_buf[..len]);
                self.read_buf.drain(..len);
                Poll::Ready(Ok(len))
            }
        }
    }

    impl AsyncWrite for ScriptedTcpStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.write_buf.extend_from_slice(buf);
            while self.write_buf.len() >= 2 {
                let msg_len = u16::from_be_bytes([self.write_buf[0], self.write_buf[1]]) as usize;
                if self.write_buf.len() >= 2 + msg_len {
                    let msg_bytes = self.write_buf[2..2 + msg_len].to_vec();
                    self.write_buf.drain(..2 + msg_len);

                    let (id, qname, qtype) = parse_wire_message_header(&msg_bytes);
                    if let (Some(qname), Some(qtype)) = (qname, qtype) {
                        self.provider
                            .recorded_queries
                            .lock()
                            .unwrap()
                            .push(RecordedQuery {
                                peer: self.peer,
                                protocol: Protocol::Tcp,
                                name: qname.clone(),
                                rtype: qtype,
                            });

                        let reply = {
                            let guard = self.provider.scripts.lock().unwrap();
                            guard
                                .get(&(self.peer, Protocol::Tcp, qname.clone(), qtype))
                                .cloned()
                        };
                        if let Some(reply) = reply {
                            match reply {
                                ScriptReply::Bytes(bytes) | ScriptReply::Truncated(bytes) => {
                                    let mut resp = bytes.clone();
                                    if resp.len() >= 2 {
                                        let id_bytes = id.to_be_bytes();
                                        resp[0] = id_bytes[0];
                                        resp[1] = id_bytes[1];
                                    }
                                    let resp_len = (resp.len() as u16).to_be_bytes();
                                    self.read_buf.extend_from_slice(&resp_len);
                                    self.read_buf.extend_from_slice(&resp);
                                }
                                ScriptReply::Error(k) => {
                                    return Poll::Ready(Err(io::Error::new(k, "simulated error")));
                                }
                                ScriptReply::Timeout => {}
                                ScriptReply::WithholdUntilQueries(min_count, bytes) => {
                                    let count =
                                        self.provider.recorded_queries.lock().unwrap().len();
                                    if count >= min_count {
                                        let mut resp = bytes.clone();
                                        if resp.len() >= 2 {
                                            let id_bytes = id.to_be_bytes();
                                            resp[0] = id_bytes[0];
                                            resp[1] = id_bytes[1];
                                        }
                                        let resp_len = (resp.len() as u16).to_be_bytes();
                                        self.read_buf.extend_from_slice(&resp_len);
                                        self.read_buf.extend_from_slice(&resp);
                                    }
                                }
                            }
                        }
                    }
                } else {
                    break;
                }
            }

            let wakers: Vec<std::task::Waker> =
                self.provider.wakers.lock().unwrap().drain(..).collect();
            for w in wakers {
                w.wake();
            }

            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

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
            let signer =
                DnssecSigner::new(dnskey.clone(), Box::new(key), origin.clone(), duration);
            Self {
                origin,
                dnskey,
                signer,
                trust_anchors: Arc::new(anchors),
            }
        }

        fn sign_rrset(&self, mut rrset: RecordSet, in_window: bool) -> RecordSet {
            let now = PINNED_TIMESTAMP as i64;
            let inception_ts = if in_window {
                now - 86400
            } else {
                now - 86400 * 4
            };

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
            msg.queries.push(Query::query(qname.clone(), RecordType::DNSKEY));
            for r in signed.records(true) {
                msg.answers.push(r.clone());
            }
            msg
        }

        fn make_nodata_response(
            &self,
            qname: &Name,
            qtype: RecordType,
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
            nodata_msg.queries.push(Query::query(qname.clone(), qtype));
            for r in signed_nsec.records(true) {
                nodata_msg.authorities.push(r.clone());
            }
            nodata_msg
        }

        fn script_address_nodata(&self, provider: &ScriptedProvider, ip: SocketAddr, target: &Name) {
            let aaaa_nodata = self.make_nodata_response(target, RecordType::AAAA, vec![RecordType::A], true);
            provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::AAAA, aaaa_nodata.clone());
            provider.script_msg(ip, Protocol::Tcp, target.clone(), RecordType::AAAA, aaaa_nodata);
            let cname_nodata = self.make_nodata_response(target, RecordType::CNAME, vec![RecordType::A], true);
            provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CNAME, cname_nodata.clone());
            provider.script_msg(ip, Protocol::Tcp, target.clone(), RecordType::CNAME, cname_nodata);
        }
    }

    fn script_empty_nodata(provider: &ScriptedProvider, ip: SocketAddr, qname: &Name, qtype: RecordType) {
        let mut msg = Message::new(0, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::NoError;
        provider.script_msg(ip, Protocol::Udp, qname.clone(), qtype, msg.clone());
        provider.script_msg(ip, Protocol::Tcp, qname.clone(), qtype, msg);
    }

    fn script_root_dnskey(provider: &ScriptedProvider, ip: SocketAddr) {
        let ctx = TestDnssecContext::new(Name::root(), Duration::from_secs(86400 * 3));
        let root_dnskey = ctx.make_dnskey_response(&Name::root(), true);
        provider.script_msg(ip, Protocol::Udp, Name::root(), RecordType::DNSKEY, root_dnskey.clone());
        provider.script_msg(ip, Protocol::Tcp, Name::root(), RecordType::DNSKEY, root_dnskey);
    }

    fn setup_test_pool(
        provider: ScriptedProvider,
        anchors: Arc<TrustAnchors>,
        servers: Vec<NameServerConfig>,
    ) -> (
        DnssecDnsHandle<NameServerPool<CapturingProvider<ScriptedProvider>>>,
        Arc<CaptureLog>,
        ResolverConfig,
    ) {
        let mut opts = ResolverOpts::default();
        opts.timeout = Duration::from_millis(800);
        opts.attempts = 1;

        let cx = Arc::new(PoolContext::new(opts, TlsConfig::new().unwrap()));
        let capture_log = Arc::new(CaptureLog::default());
        let capturing =
            CapturingProvider::with_log(provider.clone(), Arc::clone(&capture_log));
        let pool = NameServerPool::from_config(servers.clone(), cx, capturing);
        let handle = DnssecDnsHandle::with_trust_anchor(pool, anchors);

        let mut config = ResolverConfig::default();
        for s in servers {
            config.add_name_server(s);
        }

        (handle, capture_log, config)
    }

    fn make_raw_caa_wire(qname: &Name, records: &[(u8, &str, &[u8])]) -> Vec<u8> {
        let mut msg = Message::new(0, MessageType::Response, OpCode::Query);
        msg.queries.push(Query::query(qname.clone(), RecordType::CAA));
        let mut wire = msg.to_vec().unwrap();
        let ancount = records.len() as u16;
        wire[6] = (ancount >> 8) as u8;
        wire[7] = (ancount & 0xff) as u8;
        for (flags, tag, value) in records {
            let mut name_bytes = Vec::new();
            {
                let mut encoder =
                    hickory_proto::serialize::binary::BinEncoder::new(&mut name_bytes);
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

    // -----------------------------------------------------------------------
    // Acceptance Test Cases 1 - 19
    // -----------------------------------------------------------------------
    #[tokio::test(start_paused = true)]
    async fn test_case_1_2_forged_issue_in_additional_or_authority_ignored() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let origin = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(origin.clone(), Duration::from_secs(86400 * 3));
        let provider = ScriptedProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&origin, true);
        provider.script_msg(ip, Protocol::Udp, origin.clone(), RecordType::DNSKEY, dnskey_msg.clone());
        provider.script_msg(ip, Protocol::Tcp, origin.clone(), RecordType::DNSKEY, dnskey_msg);

        let root_dnskey = ctx.make_dnskey_response(&Name::root(), true);
        provider.script_msg(ip, Protocol::Udp, Name::root(), RecordType::DNSKEY, root_dnskey.clone());
        provider.script_msg(ip, Protocol::Tcp, Name::root(), RecordType::DNSKEY, root_dnskey);

        ctx.script_address_nodata(&provider, ip, &target);

        // Case 1: Signed denial for CAA at target, but DNSKEY reply has forged CAA in additionals
        let nodata_caa = ctx.make_nodata_response(&target, RecordType::CAA, vec![RecordType::A], true);
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, nodata_caa.clone());
        provider.script_msg(ip, Protocol::Udp, origin.clone(), RecordType::CAA, nodata_caa);

        // A records
        let mut a_rrset = RecordSet::new(target.clone(), RecordType::A, 3600);
        a_rrset.insert(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))), 3600);
        let signed_a = ctx.sign_rrset(a_rrset, true);
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_a.records(true) {
            a_msg.answers.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns.clone()]);
        let verdict = resolve_byo_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI, Utc::now()).await;
        assert_ne!(verdict.code, DnsVerdictCode::Admitted);

        // Case 2: iodef-only CAA in answer, forged admitting issue in authority section
        let iodef_raw = make_raw_caa_wire(&target, &[(0, "iodef", b"mailto:security@example.com")]);
        let iodef_msg = Message::from_vec(&iodef_raw).unwrap();
        let iodef_rdata = match &iodef_msg.answers[0].data {
            RData::CAA(caa) => caa.clone(),
            _ => unreachable!(),
        };
        let mut caa_rrset = RecordSet::new(target.clone(), RecordType::CAA, 3600);
        caa_rrset.insert(Record::from_rdata(target.clone(), 3600, RData::CAA(iodef_rdata)), 3600);
        let signed_caa = ctx.sign_rrset(caa_rrset, true);
        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_caa.records(true) {
            caa_msg.answers.push(r.clone());
        }
        // Forged admitting issue in authority
        let forged_caa = CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI)]);
        caa_msg.authorities.push(Record::from_rdata(target.clone(), 3600, RData::CAA(forged_caa)));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, caa_msg);

        let (handle2, log2, cfg2) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns.clone()]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle2, &log2, &cfg2, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa.policy, Some(CaaPolicyCode::OtherCa));
        assert_ne!(solstone.caa.policy, Some(CaaPolicyCode::Admitted));
    }

    // Case 3: 16-character ASCII tag, UDP and TCP
    #[tokio::test(start_paused = true)]
    async fn test_case_3_16char_tag_udp_and_tcp() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let apex = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();

        // 1. UDP Lookup
        {
            let provider = ScriptedProvider::default();
            let wire = make_raw_caa_wire(
                &target,
                &[
                    (0, "sixteencharactert", b"custom value"),
                    (0, "issue", b"sectigo.com; accounturi=https://example.com/acct/12345; validationmethods=tls-alpn-01"),
                ],
            );
            provider.script_raw(ip, Protocol::Udp, target.clone(), RecordType::CAA, wire);

            let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
            a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
            provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
            script_empty_nodata(&provider, ip, &target, RecordType::AAAA);
            script_empty_nodata(&provider, ip, &target, RecordType::CNAME);

            let (handle, log, cfg) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns.clone()]);
            let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
            assert_eq!(solstone.caa.raw.len(), 2);
            assert!(solstone.caa.raw.iter().all(|r| r.proof.is_none()));
            assert_ne!(solstone.caa.outcome, DnssecOutcome::Secure);
            assert_eq!(solstone.caa.policy, Some(CaaPolicyCode::OtherCa));

            let owner = resolve_byo_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI, Utc::now()).await;
            assert_ne!(owner.code, DnsVerdictCode::Admitted);
            assert_eq!(owner.code, DnsVerdictCode::OtherCa);

            let recorded = provider.recorded_queries();
            assert!(!recorded.iter().any(|q| q.name == apex && q.rtype == RecordType::CAA));
        }

        // 2. TCP Lookup
        {
            let provider = ScriptedProvider::default();
            let wire = make_raw_caa_wire(
                &target,
                &[
                    (0, "sixteencharactert", b"custom value"),
                    (0, "issue", b"sectigo.com; accounturi=https://example.com/acct/12345; validationmethods=tls-alpn-01"),
                ],
            );
            // TC bit on UDP to force TCP
            provider.script_truncated_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, Message::new(0, MessageType::Response, OpCode::Query));
            provider.script_raw(ip, Protocol::Tcp, target.clone(), RecordType::CAA, wire);

            let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
            a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
            provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
            script_empty_nodata(&provider, ip, &target, RecordType::AAAA);
            script_empty_nodata(&provider, ip, &target, RecordType::CNAME);

            let (handle, log, cfg) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns.clone()]);
            let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
            assert_eq!(solstone.caa.raw.len(), 2);
            assert!(solstone.caa.raw.iter().all(|r| r.proof.is_none()));
            assert_ne!(solstone.caa.outcome, DnssecOutcome::Secure);
            assert_eq!(solstone.caa.policy, Some(CaaPolicyCode::OtherCa));

            let owner = resolve_byo_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI, Utc::now()).await;
            assert_ne!(owner.code, DnsVerdictCode::Admitted);
            assert_eq!(owner.code, DnsVerdictCode::OtherCa);

            let recorded = provider.recorded_queries();
            assert!(!recorded.iter().any(|q| q.name == apex && q.rtype == RecordType::CAA));
        }
    }

    // Case 4: Unslicable rdata through real lookup -> CaaUnreadable, no parent CAA query
    #[tokio::test(start_paused = true)]
    async fn test_case_4_unslicable_rdata() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let apex = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = ScriptedProvider::default();
        script_root_dnskey(&provider, ip);

        // Wire with tag length 20 on a 5-byte rdata buffer
        let mut msg = Message::new(0, MessageType::Response, OpCode::Query);
        msg.queries.push(Query::query(target.clone(), RecordType::CAA));
        let mut wire = msg.to_vec().unwrap();
        wire[6] = 0;
        wire[7] = 1; // 1 answer
        let mut name_bytes = Vec::new();
        {
            let mut encoder = hickory_proto::serialize::binary::BinEncoder::new(&mut name_bytes);
            target.emit(&mut encoder).unwrap();
        }
        wire.extend_from_slice(&name_bytes);
        wire.extend_from_slice(&257u16.to_be_bytes()); // CAA
        wire.extend_from_slice(&1u16.to_be_bytes()); // IN
        wire.extend_from_slice(&3600u32.to_be_bytes()); // TTL
        wire.extend_from_slice(&6u16.to_be_bytes()); // rdlength = 6
        wire.push(0); // flags
        wire.push(20); // tag_len = 20 (unslicable since rdlength - 2 is 4)
        wire.extend_from_slice(b"test");
        provider.script_raw(ip, Protocol::Udp, target.clone(), RecordType::CAA, wire);

        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
        script_empty_nodata(&provider, ip, &target, RecordType::AAAA);
        script_empty_nodata(&provider, ip, &target, RecordType::CNAME);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa.policy, Some(CaaPolicyCode::CaaUnreadable));

        let recorded = provider.recorded_queries();
        assert!(!recorded.iter().any(|q| q.name == apex && q.rtype == RecordType::CAA));
    }

    // Case 4b: Not a DNS message (short garbage buffer) -> LookupError or TransportFailure, no parent query
    #[tokio::test(start_paused = true)]
    async fn test_case_4b_not_a_dns_message() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let apex = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = ScriptedProvider::default();
        script_root_dnskey(&provider, ip);

        provider.script_raw(ip, Protocol::Udp, target.clone(), RecordType::CAA, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
        script_empty_nodata(&provider, ip, &target, RecordType::AAAA);
        script_empty_nodata(&provider, ip, &target, RecordType::CNAME);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns.clone()]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa.outcome, DnssecOutcome::TransportFailure);

        let owner = resolve_byo_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI, Utc::now()).await;
        assert!(owner.code == DnsVerdictCode::LookupError || owner.code == DnsVerdictCode::LookupTimeout);

        let recorded = provider.recorded_queries();
        assert!(!recorded.iter().any(|q| q.name == apex && q.rtype == RecordType::CAA));
    }

    // Case 5: CNAME at CAA label stops walk without querying parent or alias target
    #[tokio::test(start_paused = true)]
    async fn test_case_5_cname_at_caa_label() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let apex = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let alias = Name::from_utf8("target.other.com.").unwrap();
        let provider = ScriptedProvider::default();

        let mut cname_msg = Message::new(0, MessageType::Response, OpCode::Query);
        cname_msg.metadata.response_code = ResponseCode::NoError;
        cname_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::CNAME(hickory_proto::rr::rdata::CNAME(alias.clone()))));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, cname_msg);

        // A record for target
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
        script_empty_nodata(&provider, ip, &target, RecordType::AAAA);
        script_empty_nodata(&provider, ip, &target, RecordType::CNAME);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns.clone()]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa_cname.as_deref(), Some("target.other.com"));
        assert_eq!(solstone.caa.found_at.as_deref(), Some("mcp.example.com"));
        assert_eq!(solstone.caa.policy, None);

        let owner = resolve_byo_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI, Utc::now()).await;
        assert_eq!(owner.code, DnsVerdictCode::Cname);

        let recorded = provider.recorded_queries();
        assert!(!recorded.iter().any(|q| q.name == apex && q.rtype == RecordType::CAA));
        assert!(!recorded.iter().any(|q| q.name == alias && q.rtype == RecordType::CAA));
    }

    // Case 6 & 7: Stripper server in mixed pool -> Indeterminate, Admitted; Failed RRSIG -> Bogus
    #[tokio::test(start_paused = true)]
    async fn test_case_6_7_mixed_pool_and_bogus_signature() {
        let ip_good = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 53);
        let ip_strip = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 53);
        let ns_good = NameServerConfig::udp_and_tcp(ip_good.ip());
        let ns_strip = NameServerConfig::udp_and_tcp(ip_strip.ip());
        let root = Name::root();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(root.clone(), Duration::from_secs(86400 * 3));
        let provider = ScriptedProvider::default();

        // Good server: root DNSKEY with RRSIG
        let good_root = ctx.make_dnskey_response(&root, true);
        provider.script_msg(ip_good, Protocol::Udp, root.clone(), RecordType::DNSKEY, good_root.clone());
        provider.script_msg(ip_good, Protocol::Tcp, root.clone(), RecordType::DNSKEY, good_root);

        // Stripper server: root DNSKEY without RRSIG (fails probe)
        let mut stripped_root = Message::new(0, MessageType::Response, OpCode::Query);
        stripped_root.answers.push(Record::from_rdata(root.clone(), 3600, RData::DNSSEC(DNSSECRData::DNSKEY(ctx.dnskey.clone()))));
        provider.script_msg(ip_strip, Protocol::Udp, root.clone(), RecordType::DNSKEY, stripped_root.clone());
        provider.script_msg(ip_strip, Protocol::Tcp, root.clone(), RecordType::DNSKEY, stripped_root);

        // Stripper serves unsigned pinning CAA and A
        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        caa_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::CAA(CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI), KeyValue::new("validationmethods", "tls-alpn-01")]))));
        provider.script_msg(ip_strip, Protocol::Udp, target.clone(), RecordType::CAA, caa_msg);

        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
        provider.script_msg(ip_strip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
        script_empty_nodata(&provider, ip_strip, &target, RecordType::AAAA);
        script_empty_nodata(&provider, ip_strip, &target, RecordType::CNAME);

        // Case 6: Target answer comes from stripper, outcome Indeterminate, owner Admitted
        let (handle, log, cfg) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns_good.clone(), ns_strip.clone()]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa.outcome, DnssecOutcome::Indeterminate);

        let verdict = resolve_byo_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI, Utc::now()).await;
        assert_eq!(verdict.code, DnsVerdictCode::Admitted);

        // Case 7: If target carries a tampered/bogus RRSIG -> Bogus (not saved by stripping server)
        let mut caa_rrset = RecordSet::new(target.clone(), RecordType::CAA, 3600);
        caa_rrset.insert(Record::from_rdata(target.clone(), 3600, RData::CAA(CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI), KeyValue::new("validationmethods", "tls-alpn-01")]))), 3600);
        let signed_caa = ctx.sign_rrset(caa_rrset, true);
        let mut tampered_caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        tampered_caa_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::CAA(CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI), KeyValue::new("validationmethods", "tls-alpn-01")]))));
        for r in signed_caa.records(true) {
            if r.record_type() == RecordType::RRSIG
                && let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &r.data
            {
                let mut bytes = sig.sig().to_vec();
                bytes[0] ^= 0xFF;
                let new_sig = RRSIG::from_sig(sig.input().clone(), bytes);
                tampered_caa_msg.answers.push(Record::from_rdata(r.name.clone(), r.ttl, RData::DNSSEC(DNSSECRData::RRSIG(new_sig))));
                continue;
            }
        }
        provider.script_msg(ip_strip, Protocol::Udp, target.clone(), RecordType::CAA, tampered_caa_msg);

        let (handle2, log2, cfg2) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns_good.clone(), ns_strip.clone()]);
        let verdict_bogus = resolve_byo_dns_with_handles(&handle2, &log2, &cfg2, &provider, "mcp.example.com", URI, Utc::now()).await;
        assert_eq!(verdict_bogus.code, DnsVerdictCode::DnssecBogus);
    }

    // Case 8 & 9: Probe concurrency & dropped datagram retry / no-DNSKEY failure
    #[tokio::test(start_paused = true)]
    async fn test_case_8_9_probe_concurrency_and_retry() {
        let ip1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 53);
        let ip2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 53);
        let ns1 = NameServerConfig::udp_and_tcp(ip1.ip());
        let ns2 = NameServerConfig::udp_and_tcp(ip2.ip());
        let root = Name::root();
        let ctx = TestDnssecContext::new(root.clone(), Duration::from_secs(86400 * 3));
        let provider = ScriptedProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&root, true);
        let bytes = dnskey_msg.to_vec().unwrap();
        // Server 1 and Server 2 withhold until 2 queries are recorded (concurrent fanout)
        provider.scripts.lock().unwrap().insert((ip1, Protocol::Udp, root.clone(), RecordType::DNSKEY), ScriptReply::WithholdUntilQueries(2, bytes.clone()));
        provider.scripts.lock().unwrap().insert((ip2, Protocol::Udp, root.clone(), RecordType::DNSKEY), ScriptReply::WithholdUntilQueries(2, bytes));

        let failed = probe_dnssec_stripping(&[ns1.clone(), ns2.clone()], &provider).await;
        assert!(!failed, "concurrent probe should not fail");
        let rec = provider.recorded_queries();
        assert!(rec.iter().any(|q| q.peer == ip1));
        assert!(rec.iter().any(|q| q.peer == ip2));

        // Case 9: Dropped probe datagram on server 1 -> retries once and passes
        let queries_before = provider.recorded_queries().iter().filter(|q| q.peer == ip1).count();
        provider.drop_first_probe.lock().unwrap().insert(ip1);
        provider.script_msg(ip1, Protocol::Udp, root.clone(), RecordType::DNSKEY, dnskey_msg.clone());
        provider.script_msg(ip2, Protocol::Udp, root.clone(), RecordType::DNSKEY, dnskey_msg.clone());
        let failed_after_retry = probe_dnssec_stripping(std::slice::from_ref(&ns1), &provider).await;
        assert!(!failed_after_retry);
        let queries_after = provider.recorded_queries().iter().filter(|q| q.peer == ip1).count();
        assert_eq!(queries_after - queries_before, 2);

        // NoError root reply with NO DNSKEY -> fails immediately, queried once
        let queries_before_no_dnskey = provider.recorded_queries().iter().filter(|q| q.peer == ip1).count();
        let mut no_dnskey_msg = Message::new(0, MessageType::Response, OpCode::Query);
        no_dnskey_msg.metadata.response_code = ResponseCode::NoError;
        provider.script_msg(ip1, Protocol::Udp, root.clone(), RecordType::DNSKEY, no_dnskey_msg);
        let failed_no_dnskey = probe_dnssec_stripping(std::slice::from_ref(&ns1), &provider).await;
        assert!(failed_no_dnskey);
        let queries_after_no_dnskey = provider.recorded_queries().iter().filter(|q| q.peer == ip1).count();
        assert_eq!(queries_after_no_dnskey - queries_before_no_dnskey, 1);
    }

    // Case 10 & 11: Expired target RRSIG vs pinned timer, dual signed RRset
    #[tokio::test(start_paused = true)]
    async fn test_case_10_11_validity_window_and_dual_signatures() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let origin = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(origin.clone(), Duration::from_secs(86400 * 3));
        let provider = ScriptedProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&origin, true);
        provider.script_msg(ip, Protocol::Udp, origin.clone(), RecordType::DNSKEY, dnskey_msg.clone());
        provider.script_msg(ip, Protocol::Tcp, origin.clone(), RecordType::DNSKEY, dnskey_msg);

        let root_dnskey = ctx.make_dnskey_response(&Name::root(), true);
        provider.script_msg(ip, Protocol::Udp, Name::root(), RecordType::DNSKEY, root_dnskey.clone());
        provider.script_msg(ip, Protocol::Tcp, Name::root(), RecordType::DNSKEY, root_dnskey);

        ctx.script_address_nodata(&provider, ip, &target);

        // A record
        let mut a_rrset = RecordSet::new(target.clone(), RecordType::A, 3600);
        a_rrset.insert(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))), 3600);
        let signed_a = ctx.sign_rrset(a_rrset, true);
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_a.records(true) {
            a_msg.answers.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        // Expired CAA signature (outside window)
        let mut caa_rrset = RecordSet::new(target.clone(), RecordType::CAA, 3600);
        caa_rrset.insert(Record::from_rdata(target.clone(), 3600, RData::CAA(CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI), KeyValue::new("validationmethods", "tls-alpn-01")]))), 3600);
        let signed_caa_expired = ctx.sign_rrset(caa_rrset.clone(), false);
        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_caa_expired.records(true) {
            caa_msg.answers.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, caa_msg);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns.clone()]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.combined_outcome, DnssecOutcome::Bogus { outside_validity: true });
        assert_eq!(solstone.signature_outside_validity_at_local_time, Some(true));

        let owner = resolve_byo_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI, Utc::now()).await;
        assert_eq!(owner.code, DnsVerdictCode::SignatureOutsideValidityAtLocalTime);

        // 2. Tampered signature whose timestamps contain the pinned time
        let signed_caa_valid_times = ctx.sign_rrset(caa_rrset.clone(), true);
        let mut tampered_caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_caa_valid_times.records(true) {
            if r.record_type() == RecordType::RRSIG
                && let RData::DNSSEC(DNSSECRData::RRSIG(sig)) = &r.data
            {
                let mut bytes = sig.sig().to_vec();
                bytes[0] ^= 0xFF;
                let new_sig = RRSIG::from_sig(sig.input().clone(), bytes);
                tampered_caa_msg.answers.push(Record::from_rdata(r.name.clone(), r.ttl, RData::DNSSEC(DNSSECRData::RRSIG(new_sig))));
                continue;
            }
            tampered_caa_msg.answers.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, tampered_caa_msg);
        let (handle2, log2, cfg2) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns.clone()]);
        let solstone2 = resolve_solstone_me_dns_with_handles(&handle2, &log2, &cfg2, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone2.combined_outcome, DnssecOutcome::Bogus { outside_validity: false });
        assert_eq!(solstone2.signature_outside_validity_at_local_time, None);

        // 3. Dual signatures on one RRset -> Secure
        let signed_dual = ctx.sign_rrset(caa_rrset.clone(), true);
        let signed_expired = ctx.sign_rrset(caa_rrset, false);
        let mut dual_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_dual.records(true) {
            dual_msg.answers.push(r.clone());
        }
        for r in signed_expired.records(true) {
            if r.record_type() == RecordType::RRSIG {
                dual_msg.answers.push(r.clone());
            }
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, dual_msg);
        let (handle3, log3, cfg3) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns.clone()]);
        let solstone3 = resolve_solstone_me_dns_with_handles(&handle3, &log3, &cfg3, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone3.caa.outcome, DnssecOutcome::Secure);
    }

    // Case 12: NSEC3 opt-out covering interval -> Insecure
    #[tokio::test(start_paused = true)]
    async fn test_case_12_nsec3_opt_out_covering() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let origin = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(origin.clone(), Duration::from_secs(86400 * 3));
        let provider = ScriptedProvider::default();

        script_root_dnskey(&provider, ip);

        let dnskey_msg = ctx.make_dnskey_response(&origin, true);
        provider.script_msg(ip, Protocol::Udp, origin.clone(), RecordType::DNSKEY, dnskey_msg.clone());
        provider.script_msg(ip, Protocol::Tcp, origin.clone(), RecordType::DNSKEY, dnskey_msg);

        let salt = Vec::new();
        let hash = Nsec3HashAlgorithm::SHA1.hash(&salt, &target, 0).unwrap();
        let mut owner_bytes = hash.as_ref().to_vec();
        for b in owner_bytes.iter_mut().rev() {
            if *b > 0 {
                *b -= 1;
                break;
            } else {
                *b = 0xFF;
            }
        }
        let mut next_bytes = hash.as_ref().to_vec();
        for b in next_bytes.iter_mut().rev() {
            if *b < 0xFF {
                *b += 1;
                break;
            } else {
                *b = 0;
            }
        }
        let label = base32_dnssec(&owner_bytes).to_ascii_lowercase();
        let nsec3_owner = Name::from_utf8(format!("{label}.example.com.")).unwrap();

        // NSEC3 strictly covering the target name with opt-out = true
        let nsec3 = NSEC3::new(
            Nsec3HashAlgorithm::SHA1,
            true, // opt-out = true
            0,
            salt,
            next_bytes,
            vec![RecordType::NS, RecordType::RRSIG],
        );
        let mut rrset = RecordSet::new(nsec3_owner.clone(), RecordType::NSEC3, 3600);
        rrset.insert(Record::from_rdata(nsec3_owner.clone(), 3600, RData::DNSSEC(DNSSECRData::NSEC3(nsec3))), 3600);
        let signed_nsec3 = ctx.sign_rrset(rrset, true);

        let mut ds_msg = Message::new(0, MessageType::Response, OpCode::Query);
        ds_msg.metadata.response_code = ResponseCode::NoError;
        for r in signed_nsec3.records(true) {
            ds_msg.authorities.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::DS, ds_msg.clone());
        provider.script_msg(ip, Protocol::Tcp, target.clone(), RecordType::DS, ds_msg);

        let caa_rdata = CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI), KeyValue::new("validationmethods", "tls-alpn-01")]);
        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        caa_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::CAA(caa_rdata)));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, caa_msg);

        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
        script_empty_nodata(&provider, ip, &target, RecordType::AAAA);
        script_empty_nodata(&provider, ip, &target, RecordType::CNAME);

        let mut ns_msg = Message::new(0, MessageType::Response, OpCode::Query);
        ns_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::NS(NS(Name::from_utf8("ns1.example.com.").unwrap()))));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::NS, ns_msg.clone());
        provider.script_msg(ip, Protocol::Tcp, target.clone(), RecordType::NS, ns_msg);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns.clone()]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa.outcome, DnssecOutcome::Insecure);
    }

    // Case 13: Compact denial with RecordType::Unknown(128)
    #[tokio::test(start_paused = true)]
    async fn test_case_13_compact_denial() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let apex = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(apex.clone(), Duration::from_secs(86400 * 3));
        let provider = ScriptedProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&apex, true);
        provider.script_msg(ip, Protocol::Udp, apex.clone(), RecordType::DNSKEY, dnskey_msg.clone());
        provider.script_msg(ip, Protocol::Tcp, apex.clone(), RecordType::DNSKEY, dnskey_msg);

        let root_dnskey = ctx.make_dnskey_response(&Name::root(), true);
        provider.script_msg(ip, Protocol::Udp, Name::root(), RecordType::DNSKEY, root_dnskey.clone());
        provider.script_msg(ip, Protocol::Tcp, Name::root(), RecordType::DNSKEY, root_dnskey);

        // Compact denial: NSEC type bitmap contains Unknown(128)
        let nodata = ctx.make_nodata_response(&target, RecordType::CAA, vec![RecordType::A, RecordType::Unknown(128)], true);
        let nodata_aaaa = ctx.make_nodata_response(&target, RecordType::AAAA, vec![RecordType::A, RecordType::Unknown(128)], true);
        let nodata_cname = ctx.make_nodata_response(&target, RecordType::CNAME, vec![RecordType::A, RecordType::Unknown(128)], true);
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, nodata);
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::AAAA, nodata_aaaa);
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CNAME, nodata_cname);

        // Apex CAA response
        let caa_rdata = CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI), KeyValue::new("validationmethods", "tls-alpn-01")]);
        let mut apex_caa_rrset = RecordSet::new(apex.clone(), RecordType::CAA, 3600);
        apex_caa_rrset.insert(Record::from_rdata(apex.clone(), 3600, RData::CAA(caa_rdata)), 3600);
        let signed_apex_caa = ctx.sign_rrset(apex_caa_rrset, true);
        let mut apex_caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_apex_caa.records(true) {
            apex_caa_msg.answers.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, apex.clone(), RecordType::CAA, apex_caa_msg);

        // A record
        let mut a_rrset = RecordSet::new(target.clone(), RecordType::A, 3600);
        a_rrset.insert(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))), 3600);
        let signed_a = ctx.sign_rrset(a_rrset, true);
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_a.records(true) {
            a_msg.answers.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns.clone()]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa.levels[0].outcome, DnssecOutcome::Secure);
        let recorded = provider.recorded_queries();
        assert!(recorded.iter().any(|q| q.name == apex && q.rtype == RecordType::CAA));
    }

    // Case 14 & 18: Climb rules (owner vs solstone.me) & bogus stops climb
    #[tokio::test(start_paused = true)]
    async fn test_case_14_18_climb_rules_and_bogus_stop() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let apex = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = ScriptedProvider::default();

        // Empty NOERROR response for target CAA
        let mut empty_target = Message::new(0, MessageType::Response, OpCode::Query);
        empty_target.metadata.response_code = ResponseCode::NoError;
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, empty_target);

        // Apex CAA
        let caa_rdata = CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI), KeyValue::new("validationmethods", "tls-alpn-01")]);
        let mut apex_caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        apex_caa_msg.answers.push(Record::from_rdata(apex.clone(), 3600, RData::CAA(caa_rdata)));
        provider.script_msg(ip, Protocol::Udp, apex.clone(), RecordType::CAA, apex_caa_msg);

        // A record
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
        script_empty_nodata(&provider, ip, &target, RecordType::AAAA);
        script_empty_nodata(&provider, ip, &target, RecordType::CNAME);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns.clone()]);
        // Owner resolution climbs on empty indeterminate -> Admitted
        let owner = resolve_byo_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI, Utc::now()).await;
        assert_eq!(owner.code, DnsVerdictCode::Admitted);

        // solstone.me does NOT climb on empty indeterminate -> policy None
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa.policy, None);
    }

    // Case 15: UDP TC bit triggers script-recorded TCP query
    #[tokio::test(start_paused = true)]
    async fn test_case_15_udp_tc_bit_triggers_tcp_query() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let origin = Name::from_utf8("example.com.").unwrap();
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let ctx = TestDnssecContext::new(origin.clone(), Duration::from_secs(86400 * 3));
        let provider = ScriptedProvider::default();

        let dnskey_msg = ctx.make_dnskey_response(&origin, true);
        provider.script_truncated_msg(ip, Protocol::Udp, origin.clone(), RecordType::DNSKEY, dnskey_msg.clone());
        provider.script_msg(ip, Protocol::Tcp, origin.clone(), RecordType::DNSKEY, dnskey_msg);

        let root_dnskey = ctx.make_dnskey_response(&Name::root(), true);
        provider.script_msg(ip, Protocol::Udp, Name::root(), RecordType::DNSKEY, root_dnskey.clone());
        provider.script_msg(ip, Protocol::Tcp, Name::root(), RecordType::DNSKEY, root_dnskey);

        let caa_rdata = CAA::new_issue(false, Some(Name::from_str("letsencrypt.org.").unwrap()), vec![KeyValue::new("accounturi", URI), KeyValue::new("validationmethods", "tls-alpn-01")]);
        let mut caa_rrset = RecordSet::new(target.clone(), RecordType::CAA, 3600);
        caa_rrset.insert(Record::from_rdata(target.clone(), 3600, RData::CAA(caa_rdata)), 3600);
        let signed_caa = ctx.sign_rrset(caa_rrset, true);
        let mut caa_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_caa.records(true) {
            caa_msg.answers.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::CAA, caa_msg);

        let mut a_rrset = RecordSet::new(target.clone(), RecordType::A, 3600);
        a_rrset.insert(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))), 3600);
        let signed_a = ctx.sign_rrset(a_rrset, true);
        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        for r in signed_a.records(true) {
            a_msg.answers.push(r.clone());
        }
        provider.script_msg(ip, Protocol::Udp, target.clone(), RecordType::A, a_msg);
        ctx.script_address_nodata(&provider, ip, &target);

        let (handle, log, cfg) = setup_test_pool(provider.clone(), ctx.trust_anchors.clone(), vec![ns.clone()]);
        let solstone = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "mcp.example.com", URI).await;
        assert_eq!(solstone.caa.outcome, DnssecOutcome::Secure);

        let recorded = provider.recorded_queries();
        assert!(recorded.iter().any(|q| q.protocol == Protocol::Tcp && q.rtype == RecordType::DNSKEY));
    }

    // Case 16: Search domain ignored & distinct config routing
    #[tokio::test(start_paused = true)]
    async fn test_case_16_search_domain_and_distinct_config() {
        let ip1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 53);
        let ip2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 53);
        let ns1 = NameServerConfig::udp_and_tcp(ip1.ip());
        let ns2 = NameServerConfig::udp_and_tcp(ip2.ip());
        let target = Name::from_utf8("mcp.example.com.").unwrap();
        let provider = ScriptedProvider::default();
        script_root_dnskey(&provider, ip1);
        script_root_dnskey(&provider, ip2);

        let mut a_msg = Message::new(0, MessageType::Response, OpCode::Query);
        a_msg.answers.push(Record::from_rdata(target.clone(), 3600, RData::A(A(Ipv4Addr::new(93, 184, 216, 34)))));
        provider.script_msg(ip1, Protocol::Udp, target.clone(), RecordType::A, a_msg.clone());
        provider.script_msg(ip2, Protocol::Udp, target.clone(), RecordType::A, a_msg);

        script_empty_nodata(&provider, ip1, &target, RecordType::AAAA);
        script_empty_nodata(&provider, ip1, &target, RecordType::CNAME);
        script_empty_nodata(&provider, ip1, &target, RecordType::CAA);

        script_empty_nodata(&provider, ip2, &target, RecordType::AAAA);
        script_empty_nodata(&provider, ip2, &target, RecordType::CNAME);
        script_empty_nodata(&provider, ip2, &target, RecordType::CAA);

        let (handle1, log1, mut cfg1) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns1]);
        cfg1.add_search(Name::from_utf8("lab.test.").unwrap());

        let _ = resolve_solstone_me_dns_with_handles(&handle1, &log1, &cfg1, &provider, "mcp.example.com", URI).await;
        let recorded1 = provider.recorded_queries();
        for q in &recorded1 {
            assert!(!q.name.to_utf8().contains("lab.test"));
        }
        let queries_to_s1 = recorded1.iter().filter(|q| q.peer == ip1).count();

        // Call with distinct server list
        let (handle2, log2, cfg2) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns2]);
        let _ = resolve_solstone_me_dns_with_handles(&handle2, &log2, &cfg2, &provider, "mcp.example.com", URI).await;
        let recorded2 = provider.recorded_queries();
        assert!(recorded2.iter().any(|q| q.peer == ip2));
        let queries_to_s1_after = recorded2.iter().filter(|q| q.peer == ip1).count();
        assert_eq!(queries_to_s1_after, queries_to_s1);
    }

    // Case 17: Deep hostname timeout < 6s under paused time
    #[tokio::test(start_paused = true)]
    async fn test_case_17_deep_hostname_timeout() {
        let ip = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53);
        let ns = NameServerConfig::udp_and_tcp(ip.ip());
        let provider = ScriptedProvider::default();
        // Server never answers
        let (handle, log, cfg) = setup_test_pool(provider.clone(), Arc::new(TrustAnchors::empty()), vec![ns]);
        let start = tokio::time::Instant::now();
        let res = resolve_solstone_me_dns_with_handles(&handle, &log, &cfg, &provider, "a.b.c.d.e.f.g.example.com", URI).await;
        let elapsed = start.elapsed();
        assert_eq!(res.caa.outcome, DnssecOutcome::TransportFailure);
        assert_eq!(res.caa.policy, None);
        assert!(elapsed < Duration::from_secs(6), "elapsed was {:?}", elapsed);
    }

    // Case 19: Token string representations assert no spaces
    #[test]
    fn test_case_19_token_representations_no_spaces() {
        let variants = [
            DnsVerdictCode::Unchecked,
            DnsVerdictCode::Admitted,
            DnsVerdictCode::Cname,
            DnsVerdictCode::NoAddress,
            DnsVerdictCode::CaaMissing,
            DnsVerdictCode::Issuewild,
            DnsVerdictCode::ExtraIssue,
            DnsVerdictCode::OtherCa,
            DnsVerdictCode::AccountUri,
            DnsVerdictCode::ValidationMethod,
            DnsVerdictCode::LookupError,
            DnsVerdictCode::LookupTimeout,
            DnsVerdictCode::DnssecBogus,
            DnsVerdictCode::SignatureOutsideValidityAtLocalTime,
        ];
        for v in variants {
            let s = v.as_str();
            assert!(!s.contains(' '), "token '{}' contains space", s);
        }
        assert_eq!(
            DnsVerdictCode::SignatureOutsideValidityAtLocalTime.as_str(),
            "dnssec_bogus_signature_outside_validity_at_local_time"
        );
    }
}
