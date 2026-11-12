// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A read-only installed DNS entry. Completion stays private to the child owner.

use crate::archive_admission::ArchiveCancellation;
use crate::bounded_child;
use std::ffi::OsString;
use std::io::{self, Write};
use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs};
use std::time::Instant;

const ENTRY: &str = "__owned-http-dns-v1";
const CORE_MEMBER: &str = "bin/solstone-core";
// The pinned ureq 3.4.0 ResolvedSocketAddrs capacity is 16. Filter before capping.
const MAX_ADDRESSES: usize = 16;
const RECORD_BYTES: usize = 27;
const CAPTURE_BYTES: usize = 512;

#[derive(Clone, Copy, Debug)]
pub enum OwnedDnsFamily {
    Any,
    Ipv4,
    Ipv6,
}
impl OwnedDnsFamily {
    fn keyword(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Ipv4 => "v4",
            Self::Ipv6 => "v6",
        }
    }
    fn accepts(self, address: &SocketAddr) -> bool {
        match self {
            Self::Any => true,
            Self::Ipv4 => address.is_ipv4(),
            Self::Ipv6 => address.is_ipv6(),
        }
    }
    fn parse(value: &str) -> Option<Self> {
        match value {
            "any" => Some(Self::Any),
            "v4" => Some(Self::Ipv4),
            "v6" => Some(Self::Ipv6),
            _ => None,
        }
    }
}

fn check(cancellation: &ArchiveCancellation, deadline: Instant) -> io::Result<()> {
    if cancellation.is_cancelled() {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "HTTP DNS cancelled",
        ))
    } else if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "HTTP DNS deadline reached",
        ))
    } else {
        Ok(())
    }
}
fn valid_host(host: &str) -> bool {
    !host.is_empty() && host.len() <= 255 && !host.contains('\0')
}

pub fn resolve_owned_http_host(
    host: &str,
    port: u16,
    family: OwnedDnsFamily,
    deadline: Instant,
    cancellation: &ArchiveCancellation,
) -> io::Result<Vec<SocketAddr>> {
    check(cancellation, deadline)?;
    if !valid_host(host) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid HTTP DNS host",
        ));
    }
    let member = solstone_core_installed_payload::resolve_current_member(CORE_MEMBER)
        .map_err(|_| io::Error::other("installed HTTP DNS member unavailable"))?;
    check(cancellation, deadline)?;
    // Resolution already admitted the package and validated this member and its
    // native dependencies. No external work has happened since that validation.
    // Repeating it here consumes the request's short handoff budget; retain the
    // distinct revalidation after the owned child has actually run.
    let args = [
        ENTRY.into(),
        host.into(),
        port.to_string().into(),
        family.keyword().into(),
    ];
    let completed = bounded_child::run_owned_dns(
        member.path().as_os_str(),
        &args,
        deadline,
        cancellation,
        CAPTURE_BYTES,
    );
    check(cancellation, deadline)?;
    let completed = completed?;
    if !completed.successful() {
        return Err(io::Error::other("installed HTTP DNS entry refused"));
    }
    member
        .revalidate()
        .map_err(|_| io::Error::other("installed HTTP DNS member changed"))?;
    check(cancellation, deadline)?;
    let addresses = decode(&completed.output().stdout, port)?;
    if addresses.iter().any(|address| !family.accepts(address)) {
        return Err(io::Error::other(
            "HTTP DNS returned an unwanted address family",
        ));
    }
    Ok(addresses)
}

/// Exact early dispatch, before logging or any journal/service/config resolution.
/// An entry that can't end with its parent fails here before DNS is performed.
pub fn dispatch(args: &[OsString]) -> Option<u8> {
    if args.first().is_none_or(|arg| arg != ENTRY) {
        return None;
    }
    if !parent_death_armed() {
        return Some(90);
    }
    Some(run_entry(args))
}

/// A privileged exec which cleared PDEATHSIG is refused.
#[cfg(target_os = "linux")]
fn parent_death_armed() -> bool {
    death_signal_armed()
}

/// macOS has no parent-death signal: the parent's exit is watched from here.
#[cfg(target_os = "macos")]
fn parent_death_armed() -> bool {
    fn exit() -> ! {
        std::process::exit(90)
    }
    crate::parent_watch::armed("owned-dns-parent", exit)
}

#[cfg(target_os = "linux")]
fn death_signal_armed() -> bool {
    let mut signal: nix::libc::c_int = 0;
    // SAFETY: the output is one valid local c_int and PR_GET_PDEATHSIG has no
    // pointer retention or journal effects.
    #[allow(unsafe_code)]
    unsafe {
        nix::libc::prctl(
            nix::libc::PR_GET_PDEATHSIG,
            &mut signal as *mut nix::libc::c_int,
        ) == 0
            && signal == nix::libc::SIGKILL
    }
}

fn run_entry(args: &[OsString]) -> u8 {
    if args.len() != 4 {
        return 91;
    }
    let (Some(host), Some(port), Some(family)) =
        (args[1].to_str(), args[2].to_str(), args[3].to_str())
    else {
        return 91;
    };
    if !valid_host(host) {
        return 91;
    }
    let Ok(port) = port.parse::<u16>() else {
        return 91;
    };
    let Some(family) = OwnedDnsFamily::parse(family) else {
        return 91;
    };
    // Preserve ureq's authority.host():port form, including [IPv6] brackets.
    let Ok(addresses) = format!("{host}:{port}").to_socket_addrs() else {
        return 92;
    };
    let addresses: Vec<_> = addresses
        .filter(|address| family.accepts(address))
        .take(MAX_ADDRESSES)
        .collect();
    if addresses.is_empty() {
        return 92;
    }
    let mut output = io::stdout().lock();
    if output
        .write_all(&encode(&addresses))
        .and_then(|_| output.flush())
        .is_err()
    {
        return 93;
    }
    0
}

fn encode(addresses: &[SocketAddr]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(1 + addresses.len() * RECORD_BYTES);
    bytes.push(addresses.len() as u8);
    for address in addresses {
        match address {
            SocketAddr::V4(address) => {
                bytes.push(4);
                bytes.extend_from_slice(&address.ip().octets());
                bytes.extend_from_slice(&[0; 12]);
                bytes.extend_from_slice(&address.port().to_be_bytes());
                bytes.extend_from_slice(&[0; 8]);
            }
            SocketAddr::V6(address) => {
                bytes.push(6);
                bytes.extend_from_slice(&address.ip().octets());
                bytes.extend_from_slice(&address.port().to_be_bytes());
                bytes.extend_from_slice(&address.flowinfo().to_be_bytes());
                bytes.extend_from_slice(&address.scope_id().to_be_bytes());
            }
        }
    }
    bytes
}

fn decode(bytes: &[u8], port: u16) -> io::Result<Vec<SocketAddr>> {
    let count = usize::from(
        *bytes
            .first()
            .ok_or_else(|| io::Error::other("empty HTTP DNS output"))?,
    );
    if count == 0 || count > MAX_ADDRESSES || bytes.len() != 1 + count * RECORD_BYTES {
        return Err(io::Error::other("invalid HTTP DNS output length"));
    }
    let mut addresses = Vec::with_capacity(count);
    for record in bytes[1..].chunks_exact(RECORD_BYTES) {
        let returned_port = u16::from_be_bytes(record[17..19].try_into().unwrap());
        if returned_port != port {
            return Err(io::Error::other("invalid HTTP DNS output port"));
        }
        let flow = u32::from_be_bytes(record[19..23].try_into().unwrap());
        let scope = u32::from_be_bytes(record[23..27].try_into().unwrap());
        let address = match record[0] {
            4 if record[5..17].iter().all(|byte| *byte == 0) && flow == 0 && scope == 0 => {
                SocketAddr::V4(SocketAddrV4::new(
                    <[u8; 4]>::try_from(&record[1..5]).unwrap().into(),
                    port,
                ))
            }
            6 => SocketAddr::V6(SocketAddrV6::new(
                <[u8; 16]>::try_from(&record[1..17]).unwrap().into(),
                port,
                flow,
                scope,
            )),
            _ => return Err(io::Error::other("invalid HTTP DNS output family")),
        };
        addresses.push(address);
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dns_wire_preserves_order_and_ipv6_scope_flow() {
        let addresses = vec![
            "127.0.0.1:443".parse().unwrap(),
            SocketAddr::V6(SocketAddrV6::new("fe80::1".parse().unwrap(), 443, 7, 9)),
        ];
        assert_eq!(decode(&encode(&addresses), 443).unwrap(), addresses);
    }
    #[test]
    fn untrusted_dns_wire_rejects_shape_family_padding_port_and_trailing_bytes() {
        let valid = encode(&["127.0.0.1:443".parse().unwrap()]);
        for bytes in [vec![], vec![0], vec![17], valid[..valid.len() - 1].to_vec()] {
            assert!(decode(&bytes, 443).is_err());
        }
        assert!(decode(&valid, 80).is_err());
        for index in [1, 6, 20] {
            let mut malformed = valid.clone();
            malformed[index] = 99;
            assert!(decode(&malformed, 443).is_err());
        }
        let mut trailing = valid;
        trailing.push(0);
        assert!(decode(&trailing, 443).is_err());
    }
}
