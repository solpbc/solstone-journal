// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! BYO owner-hostname DNS evaluation and RFC 8659 CAA policy checking.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
        }
    }
}

/// Parsed CAA record entry.
#[derive(Debug, Clone, PartialEq, Eq)]
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
fn parse_caa_issue_value(value: &str) -> Option<(String, HashMap<String, String>)> {
    let mut parts = value.split(';');
    // Hickory prints an absolute issuer Name with its terminal DNS root dot.
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
            // Ambiguous duplicate parameters may be interpreted differently by
            // the CA, so never admit them as a constrained issuance policy.
            return None;
        }
    }

    Some((ca_domain, params))
}

// All tests sharing the process-wide DNS and relay seams hold this guard.
#[cfg(test)]
pub(crate) static BYO_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
pub static TEST_VERDICT_OVERRIDE: std::sync::RwLock<Option<(DnsVerdict, Vec<String>)>> =
    std::sync::RwLock::new(None);

#[cfg(all(test, feature = "test-hooks"))]
pub struct ByoTestSeam {
    pub tick_hold: std::sync::atomic::AtomicBool,
    pub tick_notify: tokio::sync::Notify,
    pub outside_check_addr_override: std::sync::RwLock<Option<std::net::SocketAddr>>,
    pub relay_dial_addr_override: std::sync::RwLock<Option<std::net::SocketAddr>>,
    pub acme_spawn_observer: std::sync::RwLock<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
    pub dns_lookup_stall: std::sync::atomic::AtomicBool,
    pub dns_stall_notify: tokio::sync::Notify,
    pub check_hold: std::sync::atomic::AtomicBool,
    pub check_release: tokio::sync::Notify,
    pub relay_forward_ready: tokio::sync::Notify,
    pub client_handle_exit: std::sync::atomic::AtomicU8,
    pub ingress_dial_hold: std::sync::atomic::AtomicBool,
    pub ingress_dial_release: tokio::sync::Notify,
    pub ingress_dial_entered: std::sync::atomic::AtomicUsize,
    pub ingress_dial_connected: std::sync::atomic::AtomicUsize,
}

#[cfg(all(test, feature = "test-hooks"))]
pub static BYO_TEST_SEAM: ByoTestSeam = ByoTestSeam {
    tick_hold: std::sync::atomic::AtomicBool::new(false),
    tick_notify: tokio::sync::Notify::const_new(),
    outside_check_addr_override: std::sync::RwLock::new(None),
    relay_dial_addr_override: std::sync::RwLock::new(None),
    acme_spawn_observer: std::sync::RwLock::new(None),
    dns_lookup_stall: std::sync::atomic::AtomicBool::new(false),
    dns_stall_notify: tokio::sync::Notify::const_new(),
    check_hold: std::sync::atomic::AtomicBool::new(false),
    check_release: tokio::sync::Notify::const_new(),
    relay_forward_ready: tokio::sync::Notify::const_new(),
    client_handle_exit: std::sync::atomic::AtomicU8::new(0),
    ingress_dial_hold: std::sync::atomic::AtomicBool::new(false),
    ingress_dial_release: tokio::sync::Notify::const_new(),
    ingress_dial_entered: std::sync::atomic::AtomicUsize::new(0),
    ingress_dial_connected: std::sync::atomic::AtomicUsize::new(0),
};

#[cfg(not(windows))]
fn load_system_resolver_config() -> Result<
    (
        hickory_resolver::config::ResolverConfig,
        hickory_resolver::config::ResolverOpts,
    ),
    String,
> {
    let content = std::fs::read_to_string("/etc/resolv.conf").map_err(|e| e.to_string())?;
    let mut config = hickory_resolver::config::ResolverConfig::new();
    let mut count = 0;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2
            && parts[0] == "nameserver"
            && let Ok(ip) = parts[1].parse::<std::net::IpAddr>()
        {
            config.add_name_server(hickory_resolver::config::NameServerConfig {
                socket_addr: std::net::SocketAddr::new(ip, 53),
                protocol: hickory_resolver::config::Protocol::Udp,
                tls_dns_name: None,
                trust_negative_responses: true,
                bind_addr: None,
            });
            count += 1;
        }
    }
    if count == 0 {
        return Err("no nameservers found in /etc/resolv.conf".to_string());
    }
    Ok((config, hickory_resolver::config::ResolverOpts::default()))
}

#[cfg(windows)]
fn load_system_resolver_config() -> Result<
    (
        hickory_resolver::config::ResolverConfig,
        hickory_resolver::config::ResolverOpts,
    ),
    String,
> {
    let adapters = ipconfig::get_adapters().map_err(|e| e.to_string())?;
    resolver_config_from_nameservers(
        adapters
            .iter()
            .flat_map(|adapter| adapter.dns_servers().iter().copied()),
    )
}

#[cfg(any(windows, test))]
fn resolver_config_from_nameservers(
    nameservers: impl IntoIterator<Item = std::net::IpAddr>,
) -> Result<
    (
        hickory_resolver::config::ResolverConfig,
        hickory_resolver::config::ResolverOpts,
    ),
    String,
> {
    let mut config = hickory_resolver::config::ResolverConfig::new();
    for ip in nameservers {
        config.add_name_server(hickory_resolver::config::NameServerConfig {
            socket_addr: std::net::SocketAddr::new(ip, 53),
            protocol: hickory_resolver::config::Protocol::Udp,
            tls_dns_name: None,
            trust_negative_responses: true,
            bind_addr: None,
        });
    }
    if config.name_servers().is_empty() {
        return Err("no nameservers found in Windows adapter configuration".to_string());
    }
    Ok((config, hickory_resolver::config::ResolverOpts::default()))
}

/// Real DNS resolution with 5s timeout using hickory-resolver.
pub async fn resolve_byo_dns(
    hostname: &str,
    account_uri: &str,
    now: DateTime<Utc>,
) -> (DnsVerdict, Vec<String>) {
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_VERDICT_OVERRIDE.read()
            && let Some((ref verdict, ref addrs)) = *guard
        {
            return (verdict.clone(), addrs.clone());
        }
    }

    #[cfg(all(test, feature = "test-hooks"))]
    {
        if BYO_TEST_SEAM
            .dns_lookup_stall
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            BYO_TEST_SEAM.dns_stall_notify.notified().await;
        }
    }

    let (res_config, res_opts) = match load_system_resolver_config() {
        Ok(pair) => pair,
        Err(_) => {
            return (
                DnsVerdict::new(DnsVerdictCode::LookupError, now),
                Vec::new(),
            );
        }
    };

    let lookup_future = async {
        let resolver = hickory_resolver::TokioAsyncResolver::tokio(res_config, res_opts);

        let mut map = HashMap::new();
        let lower_host = hostname.to_ascii_lowercase();

        // Query target A, AAAA, CNAME, CAA
        let target_records = query_host_records(&resolver, &lower_host).await?;
        let mut addresses = Vec::new();
        for a in &target_records.a {
            addresses.push(a.to_string());
        }
        for aaaa in &target_records.aaaa {
            addresses.push(aaaa.to_string());
        }
        map.insert(lower_host.clone(), target_records);

        // Query ancestor CAA
        let mut current = lower_host.as_str();
        while let Some((_, parent)) = current.split_once('.') {
            current = parent;
            let caa_records = query_caa_records(&resolver, current).await?;
            let entry = map.entry(current.to_string()).or_default();
            entry.caa = caa_records;
        }

        Ok::<_, String>((map, addresses))
    };

    match tokio::time::timeout(Duration::from_secs(5), lookup_future).await {
        Ok(Ok((records, addresses))) => {
            let verdict = evaluate_byo_dns_policy(hostname, account_uri, &records, now);
            (verdict, addresses)
        }
        Ok(Err(_)) => (
            DnsVerdict::new(DnsVerdictCode::LookupError, now),
            Vec::new(),
        ),
        Err(_) => (
            DnsVerdict::new(DnsVerdictCode::LookupTimeout, now),
            Vec::new(),
        ),
    }
}

/// Pure diagnostic evaluator for BYO relay and DNS address alignment.
pub fn compute_byo_diagnostics(
    relay_address: Option<Ipv4Addr>,
    dns_addresses: &[String],
) -> Vec<&'static str> {
    let mut diagnostics = Vec::new();
    let mut has_non_public = false;
    let mut a_records = Vec::new();
    let mut aaaa_count = 0usize;

    for s in dns_addresses {
        if let Ok(ip) = s.parse::<std::net::IpAddr>() {
            match ip {
                std::net::IpAddr::V4(v4) => {
                    a_records.push(v4);
                    let oct = v4.octets();
                    let is_cgnat = oct[0] == 100 && (64..=127).contains(&oct[1]);
                    if v4.is_loopback()
                        || v4.is_private()
                        || v4.is_link_local()
                        || v4.is_broadcast()
                        || oct == [255, 255, 255, 255]
                        || is_cgnat
                    {
                        has_non_public = true;
                    }
                }
                std::net::IpAddr::V6(v6) => {
                    aaaa_count += 1;
                    let seg = v6.segments();
                    let is_unique_local = (seg[0] & 0xfe00) == 0xfc00;
                    let is_link_local = (seg[0] & 0xffc0) == 0xfe80;
                    if v6.is_loopback() || is_unique_local || is_link_local {
                        has_non_public = true;
                    }
                }
            }
        }
    }

    if let Some(relay_ip) = relay_address {
        let is_exact_match = a_records.len() == 1 && a_records[0] == relay_ip && aaaa_count == 0;
        if !is_exact_match {
            diagnostics.push("address_mismatch");
        }
    }

    if has_non_public {
        diagnostics.push("non_public_address");
    }

    diagnostics
}

async fn query_host_records(
    resolver: &hickory_resolver::TokioAsyncResolver,
    host: &str,
) -> Result<HostDnsRecords, String> {
    use hickory_resolver::proto::rr::RData;
    use hickory_resolver::proto::rr::RecordType;

    let mut records = HostDnsRecords::default();

    // Query A
    if let Some(lookup) = lookup_optional(resolver, host, RecordType::A).await? {
        for rdata in lookup.iter() {
            if let RData::A(ip) = rdata {
                records.a.push(ip.0);
            }
        }
    }

    // Query AAAA
    if let Some(lookup) = lookup_optional(resolver, host, RecordType::AAAA).await? {
        for rdata in lookup.iter() {
            if let RData::AAAA(ip) = rdata {
                records.aaaa.push(ip.0);
            }
        }
    }

    // Query CNAME
    if let Some(lookup) = lookup_optional(resolver, host, RecordType::CNAME).await? {
        for rdata in lookup.iter() {
            if let RData::CNAME(name) = rdata {
                records.cname.push(name.to_utf8());
            }
        }
    }

    // Query CAA
    if let Some(lookup) = lookup_optional(resolver, host, RecordType::CAA).await? {
        for rdata in lookup.iter() {
            if let RData::CAA(caa) = rdata {
                let flags = if caa.issuer_critical() { 128 } else { 0 };
                let tag = caa.tag().to_string();
                let value = match caa.value() {
                    hickory_resolver::proto::rr::rdata::caa::Value::Issuer(name, key_values) => {
                        let mut v = name.as_ref().map(|n| n.to_utf8()).unwrap_or_default();
                        for kv in key_values {
                            if !v.is_empty() {
                                v.push_str("; ");
                            }
                            v.push_str(kv.key());
                            v.push('=');
                            v.push_str(kv.value());
                        }
                        v
                    }
                    hickory_resolver::proto::rr::rdata::caa::Value::Url(url) => url.to_string(),
                    hickory_resolver::proto::rr::rdata::caa::Value::Unknown(bytes) => {
                        String::from_utf8_lossy(bytes).to_string()
                    }
                };
                records.caa.push(CaaRecord { flags, tag, value });
            }
        }
    }

    Ok(records)
}

async fn query_caa_records(
    resolver: &hickory_resolver::TokioAsyncResolver,
    host: &str,
) -> Result<Vec<CaaRecord>, String> {
    use hickory_resolver::proto::rr::RData;
    use hickory_resolver::proto::rr::RecordType;

    let mut caa_list = Vec::new();
    if let Some(lookup) = lookup_optional(resolver, host, RecordType::CAA).await? {
        for rdata in lookup.iter() {
            if let RData::CAA(caa) = rdata {
                let flags = if caa.issuer_critical() { 128 } else { 0 };
                let tag = caa.tag().to_string();
                let value = match caa.value() {
                    hickory_resolver::proto::rr::rdata::caa::Value::Issuer(name, key_values) => {
                        let mut v = name.as_ref().map(|n| n.to_utf8()).unwrap_or_default();
                        for kv in key_values {
                            if !v.is_empty() {
                                v.push_str("; ");
                            }
                            v.push_str(kv.key());
                            v.push('=');
                            v.push_str(kv.value());
                        }
                        v
                    }
                    hickory_resolver::proto::rr::rdata::caa::Value::Url(url) => url.to_string(),
                    hickory_resolver::proto::rr::rdata::caa::Value::Unknown(bytes) => {
                        String::from_utf8_lossy(bytes).to_string()
                    }
                };
                caa_list.push(CaaRecord { flags, tag, value });
            }
        }
    }
    Ok(caa_list)
}

async fn lookup_optional(
    resolver: &hickory_resolver::TokioAsyncResolver,
    host: &str,
    record_type: hickory_resolver::proto::rr::RecordType,
) -> Result<Option<hickory_resolver::lookup::Lookup>, String> {
    use hickory_resolver::error::ResolveErrorKind;
    use hickory_resolver::proto::op::ResponseCode;

    match resolver.lookup(host, record_type).await {
        Ok(lookup) => Ok(Some(lookup)),
        Err(error)
            if matches!(
                error.kind(),
                ResolveErrorKind::NoRecordsFound {
                    response_code: ResponseCode::NoError,
                    ..
                }
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URI: &str = "https://acme-v02.api.letsencrypt.org/acme/acct/12345678";
    const HOST: &str = "mcp.example.com";

    #[test]
    fn configured_nameservers_only_no_public_fallback_or_search_suffix() {
        let ips = [
            "192.0.2.53".parse().unwrap(),
            "2001:db8::53".parse().unwrap(),
        ];
        let (config, _) = resolver_config_from_nameservers(ips).unwrap();
        let servers = config.name_servers();
        assert_eq!(servers.len(), ips.len());
        for (server, ip) in servers.iter().zip(ips) {
            assert_eq!(server.socket_addr, std::net::SocketAddr::new(ip, 53));
            assert_eq!(server.protocol, hickory_resolver::config::Protocol::Udp);
        }
        assert!(config.domain().is_none());
        assert!(config.search().is_empty());
        assert!(resolver_config_from_nameservers([]).is_err());
    }

    #[cfg(all(windows, feature = "full-tests"))]
    #[test]
    fn windows_system_resolver_uses_actual_adapter_dns_servers() {
        // Exercise the production loader, never TEST_VERDICT_OVERRIDE. This
        // catches a Unix-file read and a substituted public resolver on Windows.
        let expected: Vec<_> = ipconfig::get_adapters()
            .unwrap()
            .iter()
            .flat_map(|adapter| adapter.dns_servers().iter().copied())
            .collect();
        assert!(
            !expected.is_empty(),
            "native host needs configured DNS servers"
        );
        let (config, _) = load_system_resolver_config().unwrap();
        let actual: Vec<_> = config
            .name_servers()
            .iter()
            .map(|server| server.socket_addr.ip())
            .collect();
        assert_eq!(actual, expected);
        assert!(config.domain().is_none());
        assert!(config.search().is_empty());
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
    fn policy_cname_rejected() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().cname = vec!["alias.example.com".to_string()];
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::Cname
        );
    }

    #[test]
    fn policy_no_address_rejected() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().a = vec![];
        map.get_mut(HOST).unwrap().aaaa = vec![];
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::NoAddress
        );
    }

    #[test]
    fn policy_caa_missing() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa = vec![];
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::CaaMissing
        );
    }

    #[test]
    fn policy_ancestor_caa_inherited() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa = vec![];
        map.insert(
            "example.com".to_string(),
            HostDnsRecords {
                a: vec![],
                aaaa: vec![],
                cname: vec![],
                caa: vec![CaaRecord {
                    flags: 0,
                    tag: "issue".to_string(),
                    value: format!(
                        "letsencrypt.org; accounturi=\"{URI}\"; validationmethods=\"tls-alpn-01\""
                    ),
                }],
            },
        );
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::Admitted
        );
    }

    #[test]
    fn policy_issuewild_rejected() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa.push(CaaRecord {
            flags: 0,
            tag: "issuewild".to_string(),
            value: ";".to_string(),
        });
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::Issuewild
        );
    }

    #[test]
    fn policy_extra_issue_rejected() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa.push(CaaRecord {
            flags: 0,
            tag: "issue".to_string(),
            value: "otherca.com".to_string(),
        });
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::ExtraIssue
        );
    }

    #[test]
    fn policy_other_ca_rejected() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa[0].value =
            format!("digicert.com; accounturi={URI}; validationmethods=tls-alpn-01");
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::OtherCa
        );
    }

    #[test]
    fn policy_account_uri_mismatch_rejected() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa[0].value = "letsencrypt.org; accounturi=https://acme.org/acct/wrong; validationmethods=tls-alpn-01".to_string();
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::AccountUri
        );
    }

    #[test]
    fn policy_validation_method_mismatch_rejected() {
        let mut map = base_records();
        map.get_mut(HOST).unwrap().caa[0].value =
            format!("letsencrypt.org; accounturi={URI}; validationmethods=http-01");
        let now = Utc::now();
        assert_eq!(
            evaluate_byo_dns_policy(HOST, URI, &map, now).code,
            DnsVerdictCode::ValidationMethod
        );
    }

    #[test]
    fn diagnostics_address_mismatch_positive_cases() {
        let relay = Some(Ipv4Addr::new(198, 51, 100, 1));

        // 1. Missing relay A
        let d1 = compute_byo_diagnostics(relay, &["93.184.216.34".to_string()]);
        assert_eq!(d1, vec!["address_mismatch"]);

        // 2. Extra A
        let d2 = compute_byo_diagnostics(
            relay,
            &["198.51.100.1".to_string(), "198.51.100.2".to_string()],
        );
        assert_eq!(d2, vec!["address_mismatch"]);

        // 3. AAAA-only
        let d3 = compute_byo_diagnostics(relay, &["2001:db8::1".to_string()]);
        assert_eq!(d3, vec!["address_mismatch"]);

        // 4. Public AAAA beside a matching A
        let d4 = compute_byo_diagnostics(
            relay,
            &["198.51.100.1".to_string(), "2001:db8::1".to_string()],
        );
        assert_eq!(d4, vec!["address_mismatch"]);

        // 5. Configured relay and empty address list (e.g. no A)
        let d5 = compute_byo_diagnostics(relay, &[]);
        assert_eq!(d5, vec!["address_mismatch"]);
    }

    #[test]
    fn diagnostics_non_fires() {
        let relay = Some(Ipv4Addr::new(198, 51, 100, 1));

        // Exact match
        let d1 = compute_byo_diagnostics(relay, &["198.51.100.1".to_string()]);
        assert!(d1.is_empty());

        // No relay configured with public addresses
        let d2 = compute_byo_diagnostics(
            None,
            &["93.184.216.34".to_string(), "2001:db8::1".to_string()],
        );
        assert!(d2.is_empty());

        // No relay configured with empty addresses
        let d3 = compute_byo_diagnostics(None, &[]);
        assert!(d3.is_empty());
    }

    #[test]
    fn diagnostics_non_public_address_cases() {
        // Private IPv4
        let d1 = compute_byo_diagnostics(None, &["192.168.1.1".to_string()]);
        assert_eq!(d1, vec!["non_public_address"]);

        // CGNAT IPv4
        let d2 = compute_byo_diagnostics(None, &["100.64.0.1".to_string()]);
        assert_eq!(d2, vec!["non_public_address"]);

        // Loopback IPv6
        let d3 = compute_byo_diagnostics(None, &["::1".to_string()]);
        assert_eq!(d3, vec!["non_public_address"]);

        // Unique local IPv6
        let d4 = compute_byo_diagnostics(None, &["fd00::1".to_string()]);
        assert_eq!(d4, vec!["non_public_address"]);

        // Public IPv4 and public IPv6 do not fire
        let d5 = compute_byo_diagnostics(
            None,
            &["93.184.216.34".to_string(), "2001:db8::1".to_string()],
        );
        assert!(d5.is_empty());
    }

    #[test]
    fn diagnostics_both_codes_together() {
        let relay = Some(Ipv4Addr::new(198, 51, 100, 1));

        // Private IP that doesn't match relay
        let d1 = compute_byo_diagnostics(relay, &["192.168.1.1".to_string()]);
        assert_eq!(d1, vec!["address_mismatch", "non_public_address"]);

        // CGNAT IP beside matching relay
        let d2 = compute_byo_diagnostics(
            relay,
            &["198.51.100.1".to_string(), "100.64.0.1".to_string()],
        );
        assert_eq!(d2, vec!["address_mismatch", "non_public_address"]);
    }
}
