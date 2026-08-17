// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::net::{Ipv4Addr, Ipv6Addr};

use solstone_core_convey_http::envelope::probe_router;
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_http::listener::bind_loopback;
use solstone_core_convey_http::serve::{serve_connection, tcp_builder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn loopback_binds_exact_addresses_and_accepts_independently() {
    let listeners = bind_loopback(0).await.unwrap();
    assert_eq!(listeners.ipv4_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
    assert_eq!(listeners.ipv6_addr().unwrap().ip(), Ipv6Addr::LOCALHOST);

    let ipv4 = listeners.ipv4_addr().unwrap();
    let (accepted, mut client) = tokio::join!(listeners.accept(), async {
        TcpStream::connect(ipv4).await.unwrap()
    });
    let (stream, identity) = accepted.unwrap();
    assert_eq!(identity, AccessBasis::Localhost);
    let builder = tcp_builder();
    let serve = serve_connection(stream, probe_router(), identity, &builder);
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
    assert!(body.contains("Localhost"));

    let ipv6 = listeners.ipv6_addr().unwrap();
    let (accepted, mut client) = tokio::join!(listeners.accept(), async {
        TcpStream::connect(ipv6).await.unwrap()
    });
    let (stream, identity) = accepted.unwrap();
    assert_eq!(identity, AccessBasis::Localhost);
    let builder = tcp_builder();
    let serve = serve_connection(stream, probe_router(), identity, &builder);
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
    assert!(body.contains("Localhost"));
}
