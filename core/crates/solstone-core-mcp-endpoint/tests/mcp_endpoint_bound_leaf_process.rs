// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use nix::sys::stat::Mode;
use nix::unistd::mkfifo;
use solstone_core_journal_io::{
    BoundReadPrimitive, run_with_bound_read_barrier, run_with_two_bound_read_barriers,
};
use solstone_core_mcp_endpoint::{
    McpEndpointBootstrapError, bootstrap_mcp_endpoint_owner_identity,
};

fn write_enabled_config(root: &Path) {
    fs::create_dir_all(root.join("config")).expect("config directory creates");
    fs::write(
        root.join("config/journal.json"),
        br#"{"mcp_endpoint":{"enabled":true}}"#,
    )
    .expect("enabled config writes");
}

fn write_committed_identity(root: &Path) {
    let ca = solstone_core_sol_link::ca::generate_ca().expect("test CA generates");
    let instance_id =
        solstone_core_sol_link::ca::jid_from_spki(ca.spki_der()).expect("test JID derives");
    let directory = root.join("link/ca");
    fs::create_dir_all(&directory).expect("CA directory creates");
    fs::write(directory.join("cert.pem"), ca.certificate_pem()).expect("certificate writes");
    fs::write(directory.join("private.pem"), ca.private_key_pem()).expect("private key writes");
    fs::write(
        root.join("link/state.json"),
        format!(r#"{{"instance_id":"{instance_id}","home_label":"Leaf Test"}}"#),
    )
    .expect("state writes");
}

fn prepared_root() -> tempfile::TempDir {
    let root = tempfile::TempDir::new().expect("temporary journal creates");
    write_enabled_config(root.path());
    write_committed_identity(root.path());
    root
}

fn assert_no_endpoint(root: &Path) {
    assert!(
        !root.join("mcp-endpoint").exists(),
        "rejected leaf read must not create endpoint state"
    );
}

#[test]
fn bootstrap_rejects_fifo_substitution_for_bound_config_leaf() {
    let root = prepared_root();
    let config = root.path().join("config/journal.json");
    let (result, fired) = run_with_bound_read_barrier(
        BoundReadPrimitive::Open,
        1,
        move || {
            fs::remove_file(&config).expect("config removes");
            mkfifo(&config, Mode::from_bits_truncate(0o600)).expect("FIFO creates");
        },
        || bootstrap_mcp_endpoint_owner_identity(root.path()),
    );
    assert!(fired);
    assert!(matches!(result, Err(McpEndpointBootstrapError::ConfigRead)));
    assert_no_endpoint(root.path());
}

#[test]
fn bootstrap_rejects_regular_certificate_substitution_after_open() {
    let root = prepared_root();
    let certificate = root.path().join("link/ca/cert.pem");
    let aside = root.path().join("link/ca/cert.pem.aside");
    let replacement = root.path().join("link/ca/replacement.pem");
    fs::write(&replacement, b"replacement").expect("replacement writes");
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o644))
        .expect("replacement mode sets");
    let observed_replacement = certificate.clone();
    let (result, fired) = run_with_two_bound_read_barriers(
        BoundReadPrimitive::Read,
        2,
        move || {
            fs::rename(&certificate, &aside).expect("certificate moves aside");
            fs::rename(&replacement, &certificate).expect("replacement installs");
        },
        BoundReadPrimitive::FinalNameObserve,
        2,
        move || {
            assert_eq!(
                fs::read(&observed_replacement).expect("replacement remains named"),
                b"replacement"
            );
        },
        || bootstrap_mcp_endpoint_owner_identity(root.path()),
    );
    assert_eq!(fired, 2);
    assert!(matches!(result, Err(McpEndpointBootstrapError::Endpoint)));
    assert_no_endpoint(root.path());
}
