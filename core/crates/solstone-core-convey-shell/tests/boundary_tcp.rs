// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};
use solstone_core_convey_http::listener::bind_loopback;
use solstone_core_convey_http::serve::{
    REQUEST_BODY_LIMIT, STANDARD_BODY_LIMIT, serve_connection, tcp_builder,
};
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

async fn raw_post_declared(
    root: std::path::PathBuf,
    path: &str,
    content_type: &str,
    content_length: usize,
) -> (u16, String, Vec<u8>) {
    let content_type = content_type.to_owned();
    with_served_connection(root, move |mut client| async move {
        let header = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {content_length}\r\n\r\n"
        );
        client
            .write_all(header.as_bytes())
            .await
            .expect("headers write");
        client.shutdown().await.expect("write half closes");
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

fn file_sha256(path: &Path) -> String {
    let mut file = fs::File::open(path).expect("staged file");
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let read = file.read(&mut buffer).expect("hash read");
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    format!("{:x}", hasher.finalize())
}

#[tokio::test]
async fn save_streams_a_file_part_just_over_the_previous_extractor_cap() {
    let journal = established_journal();
    let boundary = "large-save";
    let prefix = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"client_item_id\"\r\n\r\nwire-large\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"big.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    );
    let suffix = format!("\r\n--{boundary}--\r\n");
    let payload_len = STANDARD_BODY_LIMIT + 1;
    let content_length = prefix.len() + payload_len + suffix.len();
    let root = journal.path().to_path_buf();
    let (status, _, body) = with_served_connection(root, move |mut client| async move {
        let header = format!(
            "POST /app/import/api/save HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {content_length}\r\n\r\n"
        );
        client
            .write_all(header.as_bytes())
            .await
            .expect("headers write");
        client
            .write_all(prefix.as_bytes())
            .await
            .expect("prefix writes");
        let chunk = [b'x'; 65_536];
        let mut remaining = payload_len;
        while remaining > 0 {
            let take = remaining.min(chunk.len());
            client
                .write_all(&chunk[..take])
                .await
                .expect("payload writes");
            remaining -= take;
        }
        client
            .write_all(suffix.as_bytes())
            .await
            .expect("suffix writes");
        client
    })
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json");
    let path = Path::new(parsed["path"].as_str().expect("path"));
    let stored = serde_json::from_slice::<serde_json::Value>(
        &fs::read(path.parent().expect("parent").join("import.json")).expect("metadata"),
    )
    .expect("metadata json");
    assert_eq!(
        stored["source_hash"].as_str().expect("hash"),
        file_sha256(path)
    );
    assert_eq!(
        fs::metadata(path).expect("metadata").len(),
        payload_len as u64
    );
}

#[tokio::test]
async fn save_transport_admits_the_raised_limit_and_rejects_one_byte_over() {
    let journal = established_journal();
    let (status, _, _) = raw_post_declared(
        journal.path().to_path_buf(),
        "/app/import/api/save",
        "multipart/form-data; boundary=limit",
        REQUEST_BODY_LIMIT,
    )
    .await;
    assert_ne!(status, 413, "declared limit must be admitted");

    let journal = established_journal();
    let (status, _, body) = raw_post_declared(
        journal.path().to_path_buf(),
        "/app/import/api/save",
        "multipart/form-data; boundary=limit",
        REQUEST_BODY_LIMIT + 1,
    )
    .await;
    assert_eq!(status, 413, "{}", String::from_utf8_lossy(&body));
}

#[tokio::test]
async fn support_still_rejects_one_byte_over_the_standard_cap() {
    let journal = established_journal();
    let payload_len = STANDARD_BODY_LIMIT + 1;
    let (status, _, body) = with_served_connection(journal.path().to_path_buf(), move |mut client| {
        async move {
            let header = format!(
                "POST /app/support/api/feedback HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {payload_len}\r\n\r\n"
            );
            client
                .write_all(header.as_bytes())
                .await
                .expect("headers write");
            let chunk = [b'x'; 65_536];
            let mut remaining = payload_len;
            while remaining > 0 {
                let take = remaining.min(chunk.len());
                client
                    .write_all(&chunk[..take])
                    .await
                    .expect("payload writes");
                remaining -= take;
            }
            client
        }
    })
    .await;
    assert_eq!(status, 413, "{}", String::from_utf8_lossy(&body));
}

#[tokio::test]
async fn support_draft_rejects_one_byte_over_the_standard_cap() {
    let journal = established_journal();
    let payload_len = STANDARD_BODY_LIMIT + 1;
    let (status, _, body) = with_served_connection(journal.path().to_path_buf(), move |mut client| {
        async move {
            let header = format!(
                "POST /app/support/api/draft HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {payload_len}\r\n\r\n"
            );
            client
                .write_all(header.as_bytes())
                .await
                .expect("headers write");
            let chunk = [b'x'; 65_536];
            let mut remaining = payload_len;
            while remaining > 0 {
                let take = remaining.min(chunk.len());
                client
                    .write_all(&chunk[..take])
                    .await
                    .expect("payload writes");
                remaining -= take;
            }
            client
        }
    })
    .await;
    assert_eq!(status, 413, "{}", String::from_utf8_lossy(&body));
}
