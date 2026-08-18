// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use solstone_core_convey_http::listener::bind_loopback;
use solstone_core_convey_http::serve::{serve_connection, tcp_builder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn with_served_connection<F, Fut>(
    root: std::path::PathBuf,
    write: F,
) -> (u16, String, Vec<u8>)
where
    F: FnOnce(TcpStream) -> Fut,
    Fut: std::future::Future<Output = TcpStream>,
{
    let listeners = bind_loopback(0).await.expect("loopback binds");
    let address = listeners.ipv4_addr().expect("IPv4 address");
    let task = tokio::spawn(async move {
        let (stream, identity) = listeners.accept().await.expect("connection accepts");
        let builder = tcp_builder();
        serve_connection(
            stream,
            solstone_core_convey_shell::router(root),
            identity,
            &builder,
        )
        .await
        .expect("connection serves");
    });
    let client = TcpStream::connect(address).await.expect("client connects");
    let mut client = write(client).await;
    let mut bytes = Vec::new();
    client
        .read_to_end(&mut bytes)
        .await
        .expect("response reads");
    task.await.expect("server task joins");

    let marker = b"\r\n\r\n";
    let header_end = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("response has headers");
    let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("headers are text");
    let status = headers
        .split_whitespace()
        .nth(1)
        .expect("status exists")
        .parse()
        .expect("status parses");
    (status, headers, bytes[header_end + marker.len()..].to_vec())
}

async fn raw_get(root: std::path::PathBuf, path: &str) -> (u16, String, Vec<u8>) {
    with_served_connection(root, |mut client| async move {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        );
        client
            .write_all(request.as_bytes())
            .await
            .expect("request writes");
        client
    })
    .await
}

fn established_journal() -> tempfile::TempDir {
    let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal");
    fs::create_dir(journal.path().join("config")).expect("config");
    fs::write(
        journal.path().join("config/journal.json"),
        br#"{"setup":{"completed_at":1767225600}}"#,
    )
    .expect("config");
    journal
}

#[tokio::test]
async fn entities_workspace_round_trips_over_loopback() {
    let journal = established_journal();
    let (status, headers, body) =
        raw_get(journal.path().to_path_buf(), "/app/entities/workspace").await;
    assert_eq!(status, 200);
    assert!(
        headers.lines().any(|line| {
            let lower = line.to_ascii_lowercase();
            lower.starts_with("content-type:") && lower.contains("text/html")
        }),
        "{headers}"
    );
    let expected =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/entities/workspace.html"))
            .expect("crate asset reads");
    assert_eq!(body, expected);
}

#[tokio::test]
async fn entities_unknown_route_is_html_404_on_the_wire() {
    let journal = established_journal();
    let (status, headers, body) =
        raw_get(journal.path().to_path_buf(), "/definitely-not-a-route").await;
    assert_eq!(status, 404);
    assert!(
        headers.to_ascii_lowercase().contains("text/html"),
        "{headers}"
    );
    assert!(
        String::from_utf8(body)
            .expect("body text")
            .contains("<title>404 Not Found</title>")
    );
}
