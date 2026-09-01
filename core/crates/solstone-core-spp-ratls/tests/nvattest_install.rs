// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Loopback download/extract/swap coverage. CI topology forbids TcpListener in
//! the routine lib harness, so this lives as an integration target.

#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use sha2::{Digest, Sha256};
use solstone_core_artifact_download::DownloadHostPolicy;
use solstone_core_spp_attest::locate_nvattest;
use solstone_core_spp_ratls::{NvattestEnsureStatus, ensure_nvattest_installed_with};

const PLATFORM: &str = "linux-x86_64";
const ARTIFACT: &str = "payload.tar.xz";

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-spp-ratls-install-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn xz_tar(tar_bytes: Vec<u8>) -> Vec<u8> {
    let mut xz = Vec::new();
    lzma_rs::xz_compress(&mut tar_bytes.as_slice(), &mut xz).unwrap();
    xz
}

fn pack_tar_xz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(*name).unwrap();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.finish().unwrap();
    }
    xz_tar(tar_bytes)
}

fn pack_absolute_symlink_tar_xz() -> Vec<u8> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_link_name("/etc/passwd").unwrap();
        header.set_path("bin/nvattest").unwrap();
        header.set_size(0);
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
        builder.finish().unwrap();
    }
    xz_tar(tar_bytes)
}

fn valid_archive() -> Vec<u8> {
    pack_tar_xz(&[
        ("bin/nvattest", b"placeholder"),
        ("lib/.keep", b""),
        ("share/ca/ca-bundle.pem", b"CA"),
    ])
}

fn authority(sha256: &str, size: u64) -> String {
    format!(
        r#"{{"targets":{{"{PLATFORM}":{{"artifact":{{"name":"{ARTIFACT}","sha256":"{sha256}","size_bytes":{size},"url":"https://updates.solstone.app/providers/nvattest/{ARTIFACT}"}},"inventory":[{{"executable":true,"kind":"regular","relpath":"bin/nvattest","symlink_target":null}},{{"executable":false,"kind":"regular","relpath":"share/ca/ca-bundle.pem","symlink_target":null}}],"source":{{"version":"test"}}}}}}}}"#
    )
}

fn policy<'a>(origin: &'a str) -> DownloadHostPolicy<'a> {
    DownloadHostPolicy {
        allowed_hosts: &["127.0.0.1"],
        allow_http: true,
        origin_base_url: origin,
    }
}

fn serve(body: Vec<u8>, status: u16) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let _ = stream.read(&mut buf);
        let header = format!(
            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&body);
    });
    (format!("http://{addr}"), handle)
}

fn serve_once_with_delay(
    body: Vec<u8>,
    delay: Duration,
) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_for_server = hits.clone();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        hits_for_server.fetch_add(1, Ordering::SeqCst);
        thread::sleep(delay);
        let mut buf = [0; 4096];
        let _ = stream.read(&mut buf);
        let header = format!(
            "HTTP/1.1 200 X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&body);
    });
    (format!("http://{addr}"), hits, handle)
}

fn attempt_dirs(parent: &Path) -> Vec<String> {
    fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name();
            let name = name.to_str()?.to_owned();
            name.starts_with(".nvattest-install-").then_some(name)
        })
        .collect()
}

fn parent_dir(nvattest_dir: &Path) -> PathBuf {
    nvattest_dir.parent().unwrap().to_path_buf()
}

#[test]
fn loopback_install_publishes_symlink_and_sidecar() {
    let root = TempDir::new("install");
    let dest = root.path().join("nvattest");
    let archive = valid_archive();
    let json = authority(&digest(&archive), archive.len() as u64);
    let (origin, handle) = serve(archive, 200);
    assert_eq!(
        ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy(&origin)),
        NvattestEnsureStatus::Installed
    );
    handle.join().unwrap();
    assert!(dest.symlink_metadata().unwrap().file_type().is_symlink());
    locate_nvattest(&dest).expect("locator follows the published symlink");
    assert!(dest.join(".nvattest-install.json").is_file());
    assert_eq!(
        ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy("http://127.0.0.1:1")),
        NvattestEnsureStatus::AlreadyInstalled
    );
}

#[test]
fn failed_fetch_removes_the_attempt_directory() {
    let root = TempDir::new("fail");
    let dest = root.path().join("nvattest");
    let archive = valid_archive();
    let json = authority(&digest(&archive), archive.len() as u64);
    let (origin, handle) = serve(Vec::new(), 404);
    assert_eq!(
        ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy(&origin)),
        NvattestEnsureStatus::InstallFailed
    );
    handle.join().unwrap();
    assert!(!dest.exists());
    assert!(attempt_dirs(&parent_dir(&dest)).is_empty());
}

#[test]
fn digest_mismatch_is_install_failed() {
    let root = TempDir::new("digest");
    let dest = root.path().join("nvattest");
    let archive = valid_archive();
    let json = authority(&"ab".repeat(32), archive.len() as u64);
    let (origin, handle) = serve(archive, 200);
    assert_eq!(
        ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy(&origin)),
        NvattestEnsureStatus::InstallFailed
    );
    handle.join().unwrap();
    assert!(!dest.exists());
    assert!(attempt_dirs(&parent_dir(&dest)).is_empty());
}

#[test]
fn path_escape_refuses_and_leaves_no_tree() {
    let root = TempDir::new("escape");
    let dest = root.path().join("nvattest");
    let archive = pack_absolute_symlink_tar_xz();
    let json = authority(&digest(&archive), archive.len() as u64);
    let (origin, handle) = serve(archive, 200);
    assert_eq!(
        ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy(&origin)),
        NvattestEnsureStatus::InstallFailed
    );
    handle.join().unwrap();
    assert!(!dest.exists());
    assert!(attempt_dirs(&parent_dir(&dest)).is_empty());
}

#[test]
fn second_install_retargets_and_reclaims_previous() {
    let root = TempDir::new("swap");
    let dest = root.path().join("nvattest");
    let first = valid_archive();
    let json = authority(&digest(&first), first.len() as u64);
    let (origin, handle) = serve(first, 200);
    assert_eq!(
        ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy(&origin)),
        NvattestEnsureStatus::Installed
    );
    handle.join().unwrap();
    let first_target = fs::read_link(&dest).unwrap();
    fs::write(
        dest.join(".nvattest-install.json"),
        r#"{"schema_version":1,"platform":"linux-x86_64","version":"old","artifact_name":"payload.tar.xz","artifact_sha256":"00","artifact_size_bytes":1,"attempt_id":"old","installed_at_unix":0}"#,
    )
    .unwrap();
    let second = valid_archive();
    let json = authority(&digest(&second), second.len() as u64);
    let (origin, handle) = serve(second, 200);
    assert_eq!(
        ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy(&origin)),
        NvattestEnsureStatus::Installed
    );
    handle.join().unwrap();
    let second_target = fs::read_link(&dest).unwrap();
    assert_ne!(first_target, second_target);
    assert!(!parent_dir(&dest).join(&first_target).exists());
    locate_nvattest(&dest).unwrap();
}

#[test]
fn preexisting_directory_is_displaced() {
    let root = TempDir::new("plain");
    let dest = root.path().join("nvattest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("stale"), "old").unwrap();
    let archive = valid_archive();
    let json = authority(&digest(&archive), archive.len() as u64);
    let (origin, handle) = serve(archive, 200);
    assert_eq!(
        ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy(&origin)),
        NvattestEnsureStatus::Installed
    );
    handle.join().unwrap();
    assert!(dest.symlink_metadata().unwrap().file_type().is_symlink());
    locate_nvattest(&dest).unwrap();
    assert!(!dest.join("stale").exists());
    assert!(!fs::read_dir(parent_dir(&dest)).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("displaced")
    }));
}

#[test]
fn concurrent_calls_perform_exactly_one_download() {
    let root = TempDir::new("concurrent");
    let dest = root.path().join("nvattest");
    let archive = valid_archive();
    let json = authority(&digest(&archive), archive.len() as u64);
    let (origin, hits, server) = serve_once_with_delay(archive, Duration::from_millis(400));

    let dest_first = dest.clone();
    let json_first = json.clone();
    let origin_first = origin.clone();
    let first = thread::spawn(move || {
        ensure_nvattest_installed_with(&dest_first, PLATFORM, &json_first, &policy(&origin_first))
    });
    thread::sleep(Duration::from_millis(50));
    let second = ensure_nvattest_installed_with(&dest, PLATFORM, &json, &policy(&origin));
    let first = first.join().unwrap();
    server.join().unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let results = [first, second];
    assert!(results.contains(&NvattestEnsureStatus::Installed));
    assert!(results.contains(&NvattestEnsureStatus::InstallInFlight));
    locate_nvattest(&dest).expect("the tree is fully installed, not half-done");
}
