// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pair-link address discovery and wire encoding.

use std::ffi::CStr;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

use serde::Serialize;

/// A classified local endpoint retained for direct pairing.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct LocalEndpoint {
    pub ip: IpAddr,
    pub scope: EndpointScope,
}

/// The source category used by the reference candidate ordering.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointScope {
    Lan,
    Ula,
    Vpn,
}

/// An address returned from the platform enumeration seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawInterfaceAddress {
    pub interface: String,
    pub address: IpAddr,
}

/// The level-A injected discovery snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PairingSnapshot {
    pub endpoints: Vec<LocalEndpoint>,
    pub route_ipv4: Option<Ipv4Addr>,
}

#[derive(Debug)]
pub enum AddressError {
    Enumeration(io::Error),
}

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enumeration(error) => {
                write!(formatter, "could not enumerate local interfaces: {error}")
            }
        }
    }
}

impl std::error::Error for AddressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Enumeration(error) => Some(error),
        }
    }
}

/// Raw enumeration seam. Tests supply synthetic records and never inspect host
/// interfaces.
pub trait RawInterfaceSource {
    fn enumerate(&self) -> Result<Vec<RawInterfaceAddress>, AddressError>;
}

/// Route-probe seam. The production probe calls UDP `connect`, which sends no
/// packets; tests supply a fixed route and never open a socket.
pub trait RouteIpv4Source {
    fn route_ipv4(&self) -> Option<Ipv4Addr>;
}

/// Production raw-interface source.
pub struct SystemInterfaceSource;

impl RawInterfaceSource for SystemInterfaceSource {
    fn enumerate(&self) -> Result<Vec<RawInterfaceAddress>, AddressError> {
        enumerate_system_interfaces()
    }
}

/// Production route source. The route address comes from this probe, never the
/// interface enumeration, so a VPN's selected egress remains observable.
pub struct SystemRouteIpv4Source;

impl RouteIpv4Source for SystemRouteIpv4Source {
    fn route_ipv4(&self) -> Option<Ipv4Addr> {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
        socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).ok()?;
        match socket.local_addr().ok()?.ip() {
            IpAddr::V4(address) => Some(address),
            IpAddr::V6(_) => None,
        }
    }
}

/// The level-B discovery result retaining raw enumeration records alongside
/// the classified snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryResult {
    pub snapshot: PairingSnapshot,
    pub raw_interfaces: Vec<RawInterfaceAddress>,
    pub route: Option<Ipv4Addr>,
}

/// Sibling context passed to snapshot minting to support address diagnostics
/// without re-enumeration or re-probing.
#[derive(Clone, Copy, Debug)]
pub struct DiscoveryContext<'a> {
    pub raw_interfaces: &'a [RawInterfaceAddress],
    pub route: Option<Ipv4Addr>,
}

/// Construct a snapshot through the level-B production seams.
pub fn snapshot_from_sources(
    interfaces: &impl RawInterfaceSource,
    route: &impl RouteIpv4Source,
) -> Result<PairingSnapshot, AddressError> {
    discover_sources(interfaces, route).map(|result| result.snapshot)
}

/// Perform discovery through the level-B production seams, retaining the raw
/// interface records for diagnostics without re-enumeration.
pub fn discover_sources(
    interfaces: &impl RawInterfaceSource,
    route: &impl RouteIpv4Source,
) -> Result<DiscoveryResult, AddressError> {
    let raw_interfaces = interfaces.enumerate()?;
    let endpoints = classify_interface_addresses(&raw_interfaces);
    let route = route.route_ipv4();
    Ok(DiscoveryResult {
        snapshot: PairingSnapshot {
            endpoints,
            route_ipv4: route,
        },
        raw_interfaces,
        route,
    })
}

/// Pure classifier for platform records. This is deliberately separate from
/// `getifaddrs` so tests drive the real classification without host I/O.
pub fn classify_interface_addresses(raw: &[RawInterfaceAddress]) -> Vec<LocalEndpoint> {
    let mut endpoints = raw.iter().filter_map(classify_one).collect::<Vec<_>>();
    endpoints.sort_by_key(|endpoint| (endpoint.scope, endpoint.ip));
    endpoints.dedup();
    endpoints
}

/// Port of Python's `is_usable_ipv4`.
pub fn is_usable_ipv4(address: Ipv4Addr) -> bool {
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_link_local()
        || address.is_multicast())
}

/// Parse the retired HTTP pairing URL into the canonical direct home address.
pub fn parse_legacy_pairing_home_address(raw: &str) -> Result<String, LegacyPairingAddressError> {
    let raw = raw.trim();
    let Some(rest) = raw.strip_prefix("http://") else {
        return Err(LegacyPairingAddressError);
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains(['/', '?', '#', '@']) {
        return Err(LegacyPairingAddressError);
    }
    let (host, port) = rest.rsplit_once(':').ok_or(LegacyPairingAddressError)?;
    let host = host
        .parse::<Ipv4Addr>()
        .map_err(|_| LegacyPairingAddressError)?;
    let port = port.parse::<u16>().map_err(|_| LegacyPairingAddressError)?;
    if port != 7657 || !is_usable_ipv4(host) {
        return Err(LegacyPairingAddressError);
    }
    Ok(format!("{host}:{port}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyPairingAddressError;
impl fmt::Display for LegacyPairingAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid legacy pairing address")
    }
}
impl std::error::Error for LegacyPairingAddressError {}

/// Port of `resolve_pair_link_candidates`: filter first, put a matching route
/// first within its class, de-duplicate before capping, then cap to four.
pub fn resolve_pair_link_candidates(
    endpoints: &[LocalEndpoint],
    route_ipv4: Option<Ipv4Addr>,
) -> Vec<Ipv4Addr> {
    let usable_route = route_ipv4.filter(|address| is_usable_ipv4(*address));
    let filtered = endpoints
        .iter()
        .filter_map(|endpoint| match endpoint.ip {
            IpAddr::V4(address) if is_usable_ipv4(address) && is_allowed_direct_ipv4(address) => {
                Some((address, endpoint.scope))
            }
            IpAddr::V4(_) | IpAddr::V6(_) => None,
        })
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        return usable_route
            .filter(|address| is_allowed_direct_ipv4(*address))
            .into_iter()
            .collect();
    }
    let mut non_vpn = Vec::new();
    let mut vpn = Vec::new();
    for (address, scope) in filtered {
        if scope == EndpointScope::Vpn {
            vpn.push(address);
        } else {
            non_vpn.push(address);
        }
    }
    if let Some(route) = usable_route {
        for group in [&mut non_vpn, &mut vpn] {
            if let Some(index) = group.iter().position(|address| *address == route) {
                group.remove(index);
                group.insert(0, route);
                break;
            }
        }
    }
    let mut deduplicated = Vec::new();
    for address in non_vpn.into_iter().chain(vpn) {
        if !deduplicated.contains(&address) {
            deduplicated.push(address);
        }
    }
    deduplicated.truncate(4);
    deduplicated
}

/// Encode direct candidates into the pinned SPL pair-link format.
pub fn encode_pair_link(
    candidates: &[Ipv4Addr],
    nonce: [u8; 16],
    ca_fp_prefix: [u8; 16],
    port: u16,
) -> Result<String, PairLinkEncodeError> {
    if candidates.is_empty() || candidates.len() > 4 {
        return Err(PairLinkEncodeError::CandidateCount(candidates.len()));
    }
    if candidates
        .iter()
        .any(|address| !is_allowed_direct_ipv4(*address))
    {
        return Err(PairLinkEncodeError::DisallowedAddress);
    }
    Ok(encode_unchecked_pair_link(
        candidates,
        nonce,
        ca_fp_prefix,
        port,
    ))
}

/// The configured-home branch intentionally bypasses resolver and parser-range
/// eligibility, matching the reference's explicit single-host behavior.
pub fn encode_configured_home_pair_link(
    home: Ipv4Addr,
    nonce: [u8; 16],
    ca_fp_prefix: [u8; 16],
    port: u16,
) -> String {
    encode_unchecked_pair_link(&[home], nonce, ca_fp_prefix, port)
}

/// Encode a v06 relay pair link.
///
/// The caller supplies the first 16 bytes of the SHA-256 digest of the
/// committed CA's SPKI. This deliberately differs from the direct-link
/// callers, which currently supply a certificate-DER digest prefix.
pub fn encode_relay_pair_link(
    secret: [u8; 8],
    ca_fp_spki_prefix: [u8; 16],
    relay_origin: &str,
) -> Result<String, PairLinkEncodeError> {
    let custom_origin = if relay_origin == spl_core::pairlink::DEFAULT_RELAY_ORIGIN {
        None
    } else {
        let length = relay_origin.len();
        if length == 0 || length > u8::MAX as usize {
            return Err(PairLinkEncodeError::RelayOriginLength(length));
        }
        Some(relay_origin.as_bytes())
    };

    let mut blob = Vec::with_capacity(27 + custom_origin.map_or(0, <[u8]>::len));
    blob.push(0x06);
    blob.extend(secret);
    blob.push(0x01);
    blob.extend(ca_fp_spki_prefix);
    match custom_origin {
        Some(origin) => {
            blob.push(origin.len() as u8);
            blob.extend(origin);
        }
        None => blob.push(0),
    }
    Ok(format!(
        "https://go.solstone.app/p#{}",
        spl_core::crockford::encode(&blob)
    ))
}

#[derive(Debug, Eq, PartialEq)]
pub enum PairLinkEncodeError {
    CandidateCount(usize),
    DisallowedAddress,
    RelayOriginLength(usize),
}

impl fmt::Display for PairLinkEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CandidateCount(count) => write!(
                formatter,
                "pair-link candidate count must be 1 through 4, got {count}"
            ),
            Self::DisallowedAddress => formatter
                .write_str("pair-link candidate is outside SPL's allowed direct IPv4 ranges"),
            Self::RelayOriginLength(length) => write!(
                formatter,
                "relay pair-link origin must contain 1 through 255 bytes, got {length}"
            ),
        }
    }
}

impl std::error::Error for PairLinkEncodeError {}

fn classify_one(entry: &RawInterfaceAddress) -> Option<LocalEndpoint> {
    // Compared case-insensitively because the same classes of interface are
    // spelled differently per platform: `lo` and `vmnet1` on Unix,
    // `Loopback Pseudo-Interface 1` and `VMware Network Adapter VMnet1` on
    // Windows. A host-only or container bridge address is reachable from
    // nothing an owner would pair with, so offering one costs a candidate slot
    // out of the four the pair link can carry.
    let interface = entry.interface.to_ascii_lowercase();
    if [
        "lo",
        "docker",
        "br-",
        "vbox",
        "virtualbox",
        "vmnet",
        "vmware",
        "vethernet",
        "tap",
    ]
    .iter()
    .any(|prefix| interface.starts_with(prefix))
    {
        return None;
    }
    let overlay = ["utun", "tun", "tailscale"]
        .iter()
        .any(|prefix| interface.starts_with(prefix));
    match entry.address {
        IpAddr::V4(address) if is_rfc1918(address) && !overlay => Some(LocalEndpoint {
            ip: IpAddr::V4(address),
            scope: EndpointScope::Lan,
        }),
        IpAddr::V4(address) if is_cgnat(address) && overlay => Some(LocalEndpoint {
            ip: IpAddr::V4(address),
            scope: EndpointScope::Vpn,
        }),
        IpAddr::V6(address) if is_ula(address) => Some(LocalEndpoint {
            ip: IpAddr::V6(address),
            scope: EndpointScope::Ula,
        }),
        IpAddr::V4(_) | IpAddr::V6(_) => None,
    }
}

fn is_rfc1918(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    (0x0a00_0000..=0x0aff_ffff).contains(&value)
        || (0xac10_0000..=0xac1f_ffff).contains(&value)
        || (0xc0a8_0000..=0xc0a8_ffff).contains(&value)
}

fn is_cgnat(address: Ipv4Addr) -> bool {
    (0x6440_0000..=0x647f_ffff).contains(&u32::from(address))
}

fn is_ula(address: Ipv6Addr) -> bool {
    (address.octets()[0] & 0xfe) == 0xfc
}

/// Whether an IPv4 address is in the allow-list of direct pairing candidates.
pub fn is_allowed_direct_ipv4(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    [
        (0x0a00_0000, 0x0aff_ffff),
        (0xac10_0000, 0xac1f_ffff),
        (0xc0a8_0000, 0xc0a8_ffff),
        (0xa9fe_0000, 0xa9fe_ffff),
        (0x6440_0000, 0x647f_ffff),
        (0x7f00_0000, 0x7fff_ffff),
    ]
    .iter()
    .any(|(low, high)| (*low..=*high).contains(&value))
}

/// The failure kind category for a pair-start address diagnostic line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    NoCandidates,
    DisallowedAddress,
    EnumerationError,
}

impl DiagnosticKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoCandidates => "no_candidates",
            Self::DisallowedAddress => "disallowed_address",
            Self::EnumerationError => "enumeration_error",
        }
    }
}

/// Determine the single outcome tag for a raw interface record without altering
/// the classifier's admit/drop decision.
pub fn classify_outcome(raw: &RawInterfaceAddress) -> &'static str {
    match classify_one(raw) {
        Some(endpoint) => match endpoint.scope {
            EndpointScope::Lan => "lan",
            EndpointScope::Ula => "ula",
            EndpointScope::Vpn => "vpn",
        },
        None => "dropped",
    }
}

/// Escape control characters and cap the interface name at 32 UTF-8 bytes.
pub fn escape_interface_name(name: &str) -> String {
    let mut end = name.len().min(32);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = &name[..end];
    let mut escaped = String::with_capacity(truncated.len());
    for c in truncated.chars() {
        match c {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\\' => escaped.push_str("\\\\"),
            c if c.is_ascii_control() => {
                escaped.push_str(&format!("\\x{:02x}", c as u8));
            }
            c => escaped.push(c),
        }
    }
    escaped
}

/// Build the structured pair-start address failure diagnostic line.
pub fn build_pair_start_diagnostic(
    kind: DiagnosticKind,
    saved_home: &str,
    raw_interfaces: &[RawInterfaceAddress],
    route: Option<Ipv4Addr>,
    candidates: &[Ipv4Addr],
    enumeration_error: Option<&AddressError>,
    route_not_probed: bool,
) -> String {
    let mut out = String::new();
    out.push_str("pair-start address failed: kind=");
    out.push_str(kind.as_str());
    out.push_str(" saved_home=");
    out.push_str(saved_home);
    out.push_str(" interfaces=[");
    let cap = raw_interfaces.len().min(32);
    for (i, entry) in raw_interfaces[..cap].iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&escape_interface_name(&entry.interface));
        out.push(':');
        out.push_str(&entry.address.to_string());
        out.push(':');
        out.push_str(classify_outcome(entry));
    }
    out.push(']');
    if raw_interfaces.len() > 32 {
        out.push_str(&format!(" remainder={}", raw_interfaces.len() - 32));
    }
    out.push_str(" route=");
    if route_not_probed {
        out.push_str("not_probed");
    } else if let Some(ip) = route {
        out.push_str(&ip.to_string());
    } else {
        out.push_str("none");
    }
    out.push_str(" candidates=[");
    for (i, cand) in candidates.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&cand.to_string());
    }
    out.push(']');
    if let Some(err) = enumeration_error {
        out.push_str(&format!(" error=\"{err}\""));
    }
    out
}

fn encode_unchecked_pair_link(
    candidates: &[Ipv4Addr],
    nonce: [u8; 16],
    ca_fp_prefix: [u8; 16],
    port: u16,
) -> String {
    let mut blob = Vec::new();
    if candidates.len() == 1 {
        blob.extend([0x04, 0x01]);
        blob.extend(candidates[0].octets());
        blob.extend(port.to_be_bytes());
    } else {
        blob.extend([0x05, 0x01, candidates.len() as u8]);
        blob.extend(port.to_be_bytes());
        for address in candidates {
            blob.extend(address.octets());
        }
    }
    blob.extend(nonce);
    blob.extend(ca_fp_prefix);
    format!(
        "https://go.solstone.app/p#{}",
        spl_core::crockford::encode(&blob)
    )
}

// This narrow FFI boundary is the only unsafe code needed to enumerate
// interfaces through libc's `getifaddrs` API.
#[cfg(unix)]
#[allow(unsafe_code)]
fn enumerate_system_interfaces() -> Result<Vec<RawInterfaceAddress>, AddressError> {
    let mut head = std::ptr::null_mut::<libc::ifaddrs>();
    // SAFETY: libc initializes `head` on success; the guard below frees exactly
    // that list before return and no list node escapes this function.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(AddressError::Enumeration(io::Error::last_os_error()));
    }
    struct IfAddrs(*mut libc::ifaddrs);
    impl Drop for IfAddrs {
        fn drop(&mut self) {
            // SAFETY: this guard owns the list returned by `getifaddrs`.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
    let _guard = IfAddrs(head);
    let mut entries = Vec::new();
    let mut current = head;
    while !current.is_null() {
        // SAFETY: `current` walks the valid linked list owned by `_guard`.
        let item = unsafe { &*current };
        if !item.ifa_addr.is_null() && !item.ifa_name.is_null() {
            // SAFETY: both pointers are valid within the owned getifaddrs list.
            let name = unsafe { CStr::from_ptr(item.ifa_name) }
                .to_string_lossy()
                .into_owned();
            // SAFETY: `ifa_addr` points at a sockaddr whose family selects the cast.
            let address = unsafe {
                match (*item.ifa_addr).sa_family as i32 {
                    libc::AF_INET => {
                        let ipv4 = &*(item.ifa_addr.cast::<libc::sockaddr_in>());
                        Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                            ipv4.sin_addr.s_addr,
                        ))))
                    }
                    libc::AF_INET6 => {
                        let ipv6 = &*(item.ifa_addr.cast::<libc::sockaddr_in6>());
                        Some(IpAddr::V6(Ipv6Addr::from(ipv6.sin6_addr.s6_addr)))
                    }
                    _ => None,
                }
            };
            if let Some(address) = address {
                entries.push(RawInterfaceAddress {
                    interface: name,
                    address,
                });
            }
        }
        current = item.ifa_next;
    }
    Ok(entries)
}

// This narrow FFI boundary is the only unsafe code needed to enumerate
// interfaces through the IP Helper `GetAdaptersAddresses` API, which is what
// Windows offers in place of `getifaddrs`. The records it produces go through
// the same `classify_one` filtering as every other platform's.
#[cfg(windows)]
#[allow(unsafe_code)]
fn enumerate_system_interfaces() -> Result<Vec<RawInterfaceAddress>, AddressError> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
    };

    const FLAGS: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    // The API documents 15 KB as the working starting size; the loop below
    // still honours whatever size it asks for rather than trusting that.
    const INITIAL_WORDS: usize = 2048;
    const ATTEMPTS: usize = 4;

    // `IP_ADAPTER_ADDRESSES_LH` is pointer-aligned and `Vec<u8>` is not, so the
    // buffer is allocated as 64-bit words and measured in whole words.
    let mut words = vec![0_u64; INITIAL_WORDS];
    for attempt in 0..ATTEMPTS {
        let mut size = u32::try_from(words.len() * size_of::<u64>()).map_err(|_| {
            AddressError::Enumeration(io::Error::other("interface buffer too large"))
        })?;
        // SAFETY: the buffer holds `size` writable bytes at the alignment the
        // struct requires, and the API retains no caller memory past the call.
        let status = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                FLAGS,
                std::ptr::null(),
                words.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &raw mut size,
            )
        };
        if status == ERROR_BUFFER_OVERFLOW && attempt + 1 < ATTEMPTS {
            let requested = (size as usize).div_ceil(size_of::<u64>());
            words = vec![0_u64; requested.max(words.len() + 1)];
            continue;
        }
        // `ERROR_NO_DATA` means the call found no addresses for the requested
        // family, which is an empty list rather than a failure -- the same
        // answer `getifaddrs` gives by returning a list with nothing in it.
        // Reporting it as an error turned "this host has no usable address"
        // into "pairing could not be completed".
        if status == ERROR_NO_DATA {
            return Ok(Vec::new());
        }
        if status != NO_ERROR {
            return Err(AddressError::Enumeration(io::Error::from_raw_os_error(
                status as i32,
            )));
        }
        let mut entries = Vec::new();
        let mut adapter = words.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !adapter.is_null() {
            // SAFETY: `adapter` walks the list the API just wrote into `words`.
            let record = unsafe { &*adapter };
            let usable =
                record.OperStatus == IfOperStatusUp && record.IfType != IF_TYPE_SOFTWARE_LOOPBACK;
            if usable {
                let name = if record.FriendlyName.is_null() {
                    String::new()
                } else {
                    // SAFETY: `FriendlyName` is a NUL-terminated wide string
                    // owned by the buffer this call filled.
                    let mut end = record.FriendlyName;
                    while unsafe { *end } != 0 {
                        end = unsafe { end.add(1) };
                    }
                    let length = unsafe { end.offset_from(record.FriendlyName) } as usize;
                    let wide = unsafe { std::slice::from_raw_parts(record.FriendlyName, length) };
                    OsString::from_wide(wide).to_string_lossy().into_owned()
                };
                let mut unicast = record.FirstUnicastAddress;
                while !unicast.is_null() {
                    // SAFETY: the unicast list belongs to the same buffer.
                    let entry = unsafe { &*unicast };
                    let sockaddr = entry.Address.lpSockaddr;
                    if !sockaddr.is_null() {
                        // SAFETY: the family field selects the sockaddr cast,
                        // and `iSockaddrLength` covers the wider struct.
                        let address = unsafe {
                            match (*sockaddr).sa_family {
                                AF_INET => {
                                    let ipv4 = &*(sockaddr.cast::<SOCKADDR_IN>());
                                    Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                                        ipv4.sin_addr.S_un.S_addr,
                                    ))))
                                }
                                AF_INET6 => {
                                    let ipv6 = &*(sockaddr.cast::<SOCKADDR_IN6>());
                                    Some(IpAddr::V6(Ipv6Addr::from(ipv6.sin6_addr.u.Byte)))
                                }
                                _ => None,
                            }
                        };
                        if let Some(address) = address {
                            entries.push(RawInterfaceAddress {
                                interface: name.clone(),
                                address,
                            });
                        }
                    }
                    unicast = entry.Next;
                }
            }
            adapter = record.Next;
        }
        return Ok(entries);
    }
    Err(AddressError::Enumeration(io::Error::other(
        "interface enumeration did not settle on a buffer size",
    )))
}

#[cfg(not(any(unix, windows)))]
fn enumerate_system_interfaces() -> Result<Vec<RawInterfaceAddress>, AddressError> {
    // The gap is ours, not the machine's: every system this could run on can
    // list its own addresses. And `AddressError`'s own Display already says
    // the listing failed ("could not enumerate local interfaces: {error}"),
    // so this half has to carry the reason rather than restate the failure.
    //
    // No "yet". A trailing "yet" promises a port that nothing in the tree
    // commits to, and this arm exists precisely because no such commitment has
    // been made for any platform that is neither unix nor windows.
    Err(AddressError::Enumeration(io::Error::other(
        "solstone isn't built for this platform",
    )))
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    fn endpoint(address: &str, scope: EndpointScope) -> LocalEndpoint {
        LocalEndpoint {
            ip: address.parse().expect("address"),
            scope,
        }
    }

    #[test]
    fn classifier_keeps_only_supported_interface_classes() {
        let raw = vec![
            RawInterfaceAddress {
                interface: "eth0".into(),
                address: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 3)),
            },
            RawInterfaceAddress {
                interface: "tailscale0".into(),
                address: IpAddr::V4(Ipv4Addr::new(100, 64, 0, 2)),
            },
            RawInterfaceAddress {
                interface: "eth0".into(),
                address: IpAddr::V6(Ipv6Addr::LOCALHOST),
            },
            RawInterfaceAddress {
                interface: "eth0".into(),
                address: "fd00::2".parse().expect("ula"),
            },
            RawInterfaceAddress {
                interface: "docker0".into(),
                address: IpAddr::V4(Ipv4Addr::new(172, 17, 0, 2)),
            },
        ];
        assert_eq!(
            classify_interface_addresses(&raw),
            vec![
                endpoint("192.168.1.3", EndpointScope::Lan),
                endpoint("fd00::2", EndpointScope::Ula),
                endpoint("100.64.0.2", EndpointScope::Vpn),
            ]
        );
    }

    #[test]
    fn classifier_drops_windows_virtual_switches_and_keeps_the_real_adapter() {
        // Windows spells the same interface classes in friendly names, so the
        // classifier has to recognise them there too: before this, a host with
        // WSL or Hyper-V offered its virtual-switch address as a direct pairing
        // candidate, which nothing on the owner's network can reach.
        let raw = vec![
            RawInterfaceAddress {
                interface: "Ethernet".into(),
                address: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 40)),
            },
            RawInterfaceAddress {
                interface: "vEthernet (WSL (Hyper-V firewall))".into(),
                address: IpAddr::V4(Ipv4Addr::new(172, 24, 96, 1)),
            },
            RawInterfaceAddress {
                interface: "vEthernet (Default Switch)".into(),
                address: IpAddr::V4(Ipv4Addr::new(172, 21, 128, 1)),
            },
            RawInterfaceAddress {
                interface: "VirtualBox Host-Only Network".into(),
                address: IpAddr::V4(Ipv4Addr::new(192, 168, 56, 1)),
            },
            RawInterfaceAddress {
                interface: "VMware Network Adapter VMnet8".into(),
                address: IpAddr::V4(Ipv4Addr::new(192, 168, 179, 1)),
            },
            RawInterfaceAddress {
                interface: "Loopback Pseudo-Interface 1".into(),
                address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            },
            RawInterfaceAddress {
                interface: "Tailscale".into(),
                address: IpAddr::V4(Ipv4Addr::new(100, 90, 1, 7)),
            },
        ];
        assert_eq!(
            classify_interface_addresses(&raw),
            vec![
                endpoint("192.168.1.40", EndpointScope::Lan),
                endpoint("100.90.1.7", EndpointScope::Vpn),
            ]
        );
    }

    #[test]
    fn classifier_still_admits_a_windows_adapter_named_like_nothing_excluded() {
        // The exclusion list must not swallow an ordinary adapter: the Windows
        // guest this port is proven on presents exactly one.
        let raw = vec![RawInterfaceAddress {
            interface: "Ethernet Instance 0".into(),
            address: IpAddr::V4(Ipv4Addr::new(10, 0, 2, 15)),
        }];
        assert_eq!(
            classify_interface_addresses(&raw),
            vec![endpoint("10.0.2.15", EndpointScope::Lan)]
        );
        assert_eq!(
            resolve_pair_link_candidates(&classify_interface_addresses(&raw), None),
            vec![Ipv4Addr::new(10, 0, 2, 15)]
        );
    }

    #[test]
    fn resolver_orders_deduplicates_before_cap_and_keeps_vpn_first_when_alone() {
        let endpoints = vec![
            endpoint("192.168.1.2", EndpointScope::Lan),
            endpoint("192.168.1.3", EndpointScope::Lan),
            endpoint("192.168.1.2", EndpointScope::Lan),
            endpoint("10.0.0.2", EndpointScope::Lan),
            endpoint("10.0.0.3", EndpointScope::Lan),
            endpoint("100.64.0.2", EndpointScope::Vpn),
        ];
        assert_eq!(
            resolve_pair_link_candidates(&endpoints, Some(Ipv4Addr::new(10, 0, 0, 2))),
            vec![
                Ipv4Addr::new(10, 0, 0, 2),
                Ipv4Addr::new(192, 168, 1, 2),
                Ipv4Addr::new(192, 168, 1, 3),
                Ipv4Addr::new(10, 0, 0, 3)
            ]
        );
        assert_eq!(
            resolve_pair_link_candidates(
                &[endpoint("100.64.0.2", EndpointScope::Vpn)],
                Some(Ipv4Addr::new(100, 64, 0, 2))
            ),
            vec![Ipv4Addr::new(100, 64, 0, 2)]
        );
        assert_eq!(
            resolve_pair_link_candidates(&endpoints, Some(Ipv4Addr::new(10, 9, 0, 1))),
            vec![
                Ipv4Addr::new(192, 168, 1, 2),
                Ipv4Addr::new(192, 168, 1, 3),
                Ipv4Addr::new(10, 0, 0, 2),
                Ipv4Addr::new(10, 0, 0, 3),
            ],
            "a route absent from a non-empty snapshot is never injected"
        );
    }

    #[test]
    fn usable_filter_and_empty_filtered_route_twin_match_reference() {
        assert!(is_usable_ipv4(Ipv4Addr::new(10, 0, 0, 2)));
        assert!(!is_usable_ipv4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            resolve_pair_link_candidates(&[], None),
            Vec::<Ipv4Addr>::new()
        );
        assert_eq!(
            resolve_pair_link_candidates(&[], Some(Ipv4Addr::new(10, 0, 0, 2))),
            vec![Ipv4Addr::new(10, 0, 0, 2)]
        );
        assert_eq!(
            resolve_pair_link_candidates(&[], Some(Ipv4Addr::new(203, 0, 113, 1))),
            Vec::<Ipv4Addr>::new()
        );
        assert_eq!(
            resolve_pair_link_candidates(&[], Some(Ipv4Addr::new(8, 8, 8, 8))),
            Vec::<Ipv4Addr>::new()
        );
    }

    #[test]
    fn encoded_links_round_trip_at_the_spl_boundary() {
        let nonce = [7; 16];
        let pin = [9; 16];
        for port in [spl_core::DEFAULT_DIRECT_PORT, 9000] {
            for candidates in [
                vec![Ipv4Addr::new(10, 0, 0, 2)],
                vec![Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(192, 168, 1, 2)],
            ] {
                let link = encode_pair_link(&candidates, nonce, pin, port).expect("link");
                let blob = spl_core::crockford::decode(link.split('#').nth(1).expect("fragment"))
                    .expect("decode");
                assert_eq!(blob[0], if candidates.len() == 1 { 0x04 } else { 0x05 });
                let spl_core::pairlink::ParsedPairLink::Direct(parsed) =
                    spl_core::pairlink::parse(&link).expect("direct pair link parses")
                else {
                    panic!("direct encoder must emit a direct pair link");
                };
                assert!(
                    parsed
                        .candidates
                        .iter()
                        .all(|candidate| candidate.port == port)
                );
            }
        }
        assert_eq!(
            encode_pair_link(
                &[Ipv4Addr::new(8, 8, 8, 8)],
                nonce,
                pin,
                spl_core::DEFAULT_DIRECT_PORT,
            ),
            Err(PairLinkEncodeError::DisallowedAddress)
        );
    }

    #[test]
    fn configured_home_is_single_host_v04_without_resolution() {
        let link =
            encode_configured_home_pair_link(Ipv4Addr::new(192, 168, 1, 7), [1; 16], [2; 16], 9000);
        let blob =
            spl_core::crockford::decode(link.split('#').nth(1).expect("fragment")).expect("decode");
        assert_eq!(blob[0], 0x04);
        let spl_core::pairlink::ParsedPairLink::Direct(parsed) =
            spl_core::pairlink::parse(&link).expect("configured-home link parses")
        else {
            panic!("configured-home encoder must emit a direct link");
        };
        assert_eq!(parsed.candidates[0].port, 9000);
    }

    #[test]
    fn relay_links_match_v06_conformance_vectors_and_parse() {
        let secret = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let ca_fp_spki_prefix = [
            0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ];
        let vectors = [
            (
                spl_core::pairlink::DEFAULT_RELAY_ORIGIN,
                "060123456789abcdef01deadbeefcafebabe0123456789abcdef00",
                "0R0J6HB7H6NWVVR1VTPVXVYAZTXBW0938NKRKAYDXW00",
            ),
            (
                "https://relay.example",
                "060123456789abcdef01deadbeefcafebabe0123456789abcdef1568747470733a2f2f72656c61792e6578616d706c65",
                "0R0J6HB7H6NWVVR1VTPVXVYAZTXBW0938NKRKAYDXWAPGX3ME1SKMBSFE9JPRRBS5SJQGRBDE1P6A",
            ),
        ];

        for (relay_origin, expected_hex, expected_fragment) in vectors {
            let link = encode_relay_pair_link(secret, ca_fp_spki_prefix, relay_origin)
                .expect("relay link encodes");
            let fragment = link.split('#').nth(1).expect("fragment");
            assert_eq!(fragment, expected_fragment);
            let blob = spl_core::crockford::decode(fragment).expect("fragment decodes");
            assert_eq!(hex(&blob), expected_hex);

            let spl_core::pairlink::ParsedPairLink::Relay(parsed) =
                spl_core::pairlink::parse(&link).expect("relay link parses")
            else {
                panic!("v06 link parses as relay");
            };
            assert_eq!(parsed.s, secret);
            assert_eq!(parsed.ca_fp_spki, ca_fp_spki_prefix);
            assert_eq!(parsed.relay_origin, relay_origin);
        }
    }

    #[test]
    fn relay_secret_uses_the_pinned_relay_key_derivation() {
        let secret = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        assert_eq!(
            spl_core::relay_window::derive_rk(&secret),
            [
                0xe3, 0x44, 0x81, 0xa4, 0xcd, 0xe6, 0x47, 0xba, 0x9c, 0x9f, 0xb2, 0x9a, 0x59, 0xe1,
                0x82, 0x71,
            ]
        );
    }

    #[test]
    fn relay_encoder_rejects_empty_and_oversized_custom_origins() {
        assert_eq!(
            encode_relay_pair_link([0; 8], [0; 16], ""),
            Err(PairLinkEncodeError::RelayOriginLength(0))
        );
        let oversized = "x".repeat(256);
        assert_eq!(
            encode_relay_pair_link([0; 8], [0; 16], &oversized),
            Err(PairLinkEncodeError::RelayOriginLength(256))
        );
    }

    #[test]
    fn diagnostic_builder_formats_tokens_and_handles_escapes_and_remainders() {
        let raw = vec![
            RawInterfaceAddress {
                interface: "docker0".into(),
                address: IpAddr::V4(Ipv4Addr::new(172, 17, 0, 1)),
            },
            RawInterfaceAddress {
                interface: "eth0".into(),
                address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)),
            },
            RawInterfaceAddress {
                interface: "en0".into(),
                address: "fd00::1".parse().expect("ula"),
            },
        ];
        let line = build_pair_start_diagnostic(
            DiagnosticKind::NoCandidates,
            "none",
            &raw,
            None,
            &[],
            None,
            false,
        );
        assert_eq!(
            line,
            "pair-start address failed: kind=no_candidates saved_home=none interfaces=[docker0:172.17.0.1:dropped,eth0:203.0.113.9:dropped,en0:fd00::1:ula] route=none candidates=[]"
        );

        let disallowed_line = build_pair_start_diagnostic(
            DiagnosticKind::DisallowedAddress,
            "none",
            &[],
            Some(Ipv4Addr::new(203, 0, 113, 9)),
            &[Ipv4Addr::new(203, 0, 113, 9)],
            None,
            false,
        );
        assert_eq!(
            disallowed_line,
            "pair-start address failed: kind=disallowed_address saved_home=none interfaces=[] route=203.0.113.9 candidates=[203.0.113.9]"
        );

        let err = AddressError::Enumeration(io::Error::other("permission denied"));
        let err_line = build_pair_start_diagnostic(
            DiagnosticKind::EnumerationError,
            "config_unreadable",
            &[],
            None,
            &[],
            Some(&err),
            true,
        );
        assert!(err_line.contains("kind=enumeration_error"));
        assert!(err_line.contains("saved_home=config_unreadable"));
        assert!(err_line.contains("route=not_probed"));
        assert!(
            err_line.contains("error=\"could not enumerate local interfaces: permission denied\"")
        );

        // 40 dropped records -> exactly 32 address entries + remainder 8
        let forty_dropped: Vec<RawInterfaceAddress> = (0..40)
            .map(|i| RawInterfaceAddress {
                interface: format!("docker{i}"),
                address: IpAddr::V4(Ipv4Addr::new(172, 17, 0, 1)),
            })
            .collect();
        let forty_line = build_pair_start_diagnostic(
            DiagnosticKind::NoCandidates,
            "none",
            &forty_dropped,
            None,
            &[],
            None,
            false,
        );
        assert!(forty_line.contains("remainder=8"));
        assert!(forty_line.contains("interfaces=[docker0:172.17.0.1:dropped,"));
        assert!(forty_line.contains("docker31:172.17.0.1:dropped]"));
        assert!(!forty_line.contains("docker32:"));

        // Interface name escaping and 32-byte limit
        assert_eq!(
            escape_interface_name("eth0\nweird\r\t\\"),
            "eth0\\nweird\\r\\t\\\\"
        );
        assert_eq!(escape_interface_name("eth\x00zero"), "eth\\x00zero");
        let long_name = "a".repeat(40);
        assert_eq!(escape_interface_name(&long_name), "a".repeat(32));

        // Builder newline via builder: line contains escaped \n sequence and no raw newline
        let newline_records = vec![RawInterfaceAddress {
            interface: "eth0\nweird".into(),
            address: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)),
        }];
        let newline_line = build_pair_start_diagnostic(
            DiagnosticKind::NoCandidates,
            "none",
            &newline_records,
            None,
            &[],
            None,
            false,
        );
        assert!(newline_line.contains("eth0\\nweird:203.0.113.9:dropped"));
        assert!(!newline_line.contains('\n'));
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
