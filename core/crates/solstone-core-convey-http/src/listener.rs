// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::net::{TcpListener, TcpStream};

use crate::identity::AccessBasis;

/// The IPv4 and IPv6 loopback listeners for the journal-host service.
pub struct LoopbackListeners {
    ipv4: TcpListener,
    ipv6: TcpListener,
}

/// Bind separate listeners to exactly the IPv4 and IPv6 loopback addresses.
///
/// The port is machine-wide and shared across logins. A second copy, including
/// one started under another login, must fail this bind rather than isolate
/// per user. Do not derive a per-user port here.
pub async fn bind_loopback(port: u16) -> io::Result<LoopbackListeners> {
    let ipv4 = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    let ipv6 = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port))).await?;
    Ok(LoopbackListeners { ipv4, ipv6 })
}

impl LoopbackListeners {
    /// Return the IPv4 loopback address currently bound by this listener set.
    pub fn ipv4_addr(&self) -> io::Result<SocketAddr> {
        self.ipv4.local_addr()
    }

    /// Return the IPv6 loopback address currently bound by this listener set.
    pub fn ipv6_addr(&self) -> io::Result<SocketAddr> {
        self.ipv6.local_addr()
    }

    /// Accept a loopback connection and attach its fixed accept-time identity.
    pub async fn accept(&self) -> io::Result<(TcpStream, AccessBasis)> {
        tokio::select! {
            result = self.ipv4.accept() => result.map(|(stream, _)| (stream, AccessBasis::Localhost)),
            result = self.ipv6.accept() => result.map(|(stream, _)| (stream, AccessBasis::Localhost)),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::envelope::probe_router;
    use crate::identity::{AccessBasis, Carrier, LinkedDeviceCid};
    use crate::serve::{mux_builder, serve_connection};

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    async fn duplex_probe(identity: AccessBasis) -> String {
        let (server, mut client) = tokio::io::duplex(8192);
        let builder = mux_builder();
        let serve = serve_connection(server, probe_router(), identity, &builder);
        let exchange = async {
            client
                .write_all(b"GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).await.unwrap();
            String::from_utf8(bytes).unwrap()
        };
        let (served, body) = tokio::join!(serve, exchange);
        served.unwrap();
        body
    }

    #[tokio::test]
    async fn loopback_and_supplied_identities_round_trip() {
        let direct = duplex_probe(AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
        })
        .await;
        assert!(direct.contains("LinkedDevice { carrier: Direct, cid: LinkedDeviceCid"));

        let via_spl = duplex_probe(AccessBasis::LinkedDevice {
            carrier: Carrier::ViaSpl,
            cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
        })
        .await;
        assert!(via_spl.contains("LinkedDevice { carrier: ViaSpl, cid: LinkedDeviceCid"));
    }
}
