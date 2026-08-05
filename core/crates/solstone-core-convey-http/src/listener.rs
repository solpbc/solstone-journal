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

    use super::bind_loopback;
    use crate::envelope::probe_router;
    use crate::identity::{AccessBasis, Carrier, LinkedDeviceDid};

    const VALID_DID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    use crate::serve::{mux_builder, serve_connection, tcp_builder};

    #[tokio::test]
    async fn listeners_bind_only_exact_loopback_addresses() {
        let listeners = bind_loopback(0).await.unwrap();

        assert_eq!(
            listeners.ipv4_addr().unwrap().ip(),
            std::net::Ipv4Addr::LOCALHOST
        );
        assert_eq!(
            listeners.ipv6_addr().unwrap().ip(),
            std::net::Ipv6Addr::LOCALHOST
        );
    }

    async fn duplex_probe(identity: AccessBasis) -> String {
        let (server, mut client) = tokio::io::duplex(8192);
        let task = tokio::spawn(async move {
            let builder = mux_builder();
            serve_connection(server, probe_router(), identity, &builder)
                .await
                .unwrap();
        });

        client
            .write_all(b"GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).await.unwrap();
        task.await.unwrap();

        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn loopback_and_supplied_identities_round_trip() {
        let listeners = bind_loopback(0).await.unwrap();
        let address = listeners.ipv4_addr().unwrap();
        let task = tokio::spawn(async move {
            let (stream, identity) = listeners.accept().await.unwrap();
            let builder = tcp_builder();
            serve_connection(stream, probe_router(), identity, &builder)
                .await
                .unwrap();
        });

        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).await.unwrap();
        task.await.unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("Localhost"));

        let direct = duplex_probe(AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            did: LinkedDeviceDid::try_from(VALID_DID).unwrap(),
        })
        .await;
        assert!(direct.contains("LinkedDevice { carrier: Direct, did: LinkedDeviceDid"));

        let via_spl = duplex_probe(AccessBasis::LinkedDevice {
            carrier: Carrier::ViaSpl,
            did: LinkedDeviceDid::try_from(VALID_DID).unwrap(),
        })
        .await;
        assert!(via_spl.contains("LinkedDevice { carrier: ViaSpl, did: LinkedDeviceDid"));
    }
}
