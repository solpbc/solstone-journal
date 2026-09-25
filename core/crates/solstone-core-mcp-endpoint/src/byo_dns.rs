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
    let (ca_domain, params) = parse_caa_issue_value(&single_issue.value);

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
fn parse_caa_issue_value(value: &str) -> (String, HashMap<String, String>) {
    let mut parts = value.split(';');
    let ca_domain = parts.next().unwrap_or("").trim().to_string();
    let mut params = HashMap::new();

    for part in parts {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().trim_matches('"').trim().to_string();
            params.insert(key, val);
        }
    }

    (ca_domain, params)
}

#[cfg(test)]
pub static TEST_VERDICT_OVERRIDE: std::sync::RwLock<Option<DnsVerdict>> =
    std::sync::RwLock::new(None);

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

/// Real DNS resolution with 5s timeout using hickory-resolver.
pub async fn resolve_byo_dns(hostname: &str, account_uri: &str, now: DateTime<Utc>) -> DnsVerdict {
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_VERDICT_OVERRIDE.read()
            && let Some(ref verdict) = *guard
        {
            return verdict.clone();
        }
    }

    let (res_config, res_opts) = match load_system_resolver_config() {
        Ok(pair) => pair,
        Err(_) => return DnsVerdict::new(DnsVerdictCode::LookupError, now),
    };

    let lookup_future = async {
        let resolver = hickory_resolver::TokioAsyncResolver::tokio(res_config, res_opts);

        let mut map = HashMap::new();
        let lower_host = hostname.to_ascii_lowercase();

        // Query target A, AAAA, CNAME, CAA
        let target_records = query_host_records(&resolver, &lower_host).await?;
        map.insert(lower_host.clone(), target_records);

        // Query ancestor CAA
        let mut current = lower_host.as_str();
        while let Some((_, parent)) = current.split_once('.') {
            current = parent;
            let caa_records = query_caa_records(&resolver, current).await?;
            let entry = map.entry(current.to_string()).or_default();
            entry.caa = caa_records;
        }

        Ok::<_, String>(map)
    };

    match tokio::time::timeout(Duration::from_secs(5), lookup_future).await {
        Ok(Ok(records)) => evaluate_byo_dns_policy(hostname, account_uri, &records, now),
        Ok(Err(_)) => DnsVerdict::new(DnsVerdictCode::LookupError, now),
        Err(_) => DnsVerdict::new(DnsVerdictCode::LookupTimeout, now),
    }
}

async fn query_host_records(
    resolver: &hickory_resolver::TokioAsyncResolver,
    host: &str,
) -> Result<HostDnsRecords, String> {
    use hickory_resolver::proto::rr::RData;
    use hickory_resolver::proto::rr::RecordType;

    let mut records = HostDnsRecords::default();

    // Query A
    if let Ok(lookup) = resolver.lookup(host, RecordType::A).await {
        for rdata in lookup.iter() {
            if let RData::A(ip) = rdata {
                records.a.push(ip.0);
            }
        }
    }

    // Query AAAA
    if let Ok(lookup) = resolver.lookup(host, RecordType::AAAA).await {
        for rdata in lookup.iter() {
            if let RData::AAAA(ip) = rdata {
                records.aaaa.push(ip.0);
            }
        }
    }

    // Query CNAME
    if let Ok(lookup) = resolver.lookup(host, RecordType::CNAME).await {
        for rdata in lookup.iter() {
            if let RData::CNAME(name) = rdata {
                records.cname.push(name.to_utf8());
            }
        }
    }

    // Query CAA
    if let Ok(lookup) = resolver.lookup(host, RecordType::CAA).await {
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
    if let Ok(lookup) = resolver.lookup(host, RecordType::CAA).await {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
