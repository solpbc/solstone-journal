// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use solstone_core_distribution::cleanroom::bind_loopback;

const CONNECT_DEADLINE: Duration = Duration::from_secs(2);

#[test]
fn returned_loopback_port_is_owned_and_reachable_without_rebinding() {
    let (listener, reported_port) = bind_loopback().expect("bind cleanroom loopback listener");
    let owned = listener
        .local_addr()
        .expect("inspect owned listener address");
    assert_eq!(owned.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(owned.port(), 0);
    assert_eq!(reported_port, owned.port());

    let reported = SocketAddr::from((Ipv4Addr::LOCALHOST, reported_port));
    let client = TcpStream::connect_timeout(&reported, CONNECT_DEADLINE)
        .expect("connect to the exact reported address");
    assert_eq!(client.peer_addr().expect("client peer address"), owned);

    listener
        .set_nonblocking(true)
        .expect("make the already-ready accept bounded");
    let (accepted, peer) = listener.accept().expect("accept exact client");
    assert_eq!(
        accepted.local_addr().expect("accepted local address"),
        owned
    );
    assert_eq!(accepted.peer_addr().expect("accepted peer address"), peer);
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}
