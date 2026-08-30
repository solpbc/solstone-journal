// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_journal_io::{BoundReadPrimitive, JournalRoot, run_with_bound_read_barrier};
use solstone_core_sol_link::ca::{generate_ca, jid_from_spki};
use solstone_core_sol_link::committed::{CommittedIdentityError, load_committed_identity_bound};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(label: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock follows epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-committed-{label}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary identity root creates");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("temporary identity root removes");
    }
}

fn write_identity(root: &Path, fallback_state: bool) {
    let ca = generate_ca().expect("test CA generates");
    let instance_id = jid_from_spki(ca.spki_der()).expect("test JID derives");
    let ca_directory = root.join("link/ca");
    fs::create_dir_all(&ca_directory).expect("CA directory creates");
    fs::write(ca_directory.join("cert.pem"), ca.certificate_pem()).expect("certificate writes");
    fs::write(ca_directory.join("private.pem"), ca.private_key_pem()).expect("private key writes");
    let state = format!(r#"{{"instance_id":"{instance_id}","home_label":"Leaf Test"}}"#);
    let state_path = if fallback_state {
        ca_directory.join("state.json")
    } else {
        root.join("link/state.json")
    };
    fs::write(state_path, state).expect("state writes");
}

fn error_kind(error: &CommittedIdentityError) -> &'static str {
    match error {
        CommittedIdentityError::CertificateRead { .. } => "certificate-read",
        CommittedIdentityError::PrivateKeyRead { .. } => "private-key-read",
        CommittedIdentityError::CertificatePem { .. } => "certificate-pem",
        CommittedIdentityError::Ca { .. } => "ca",
        CommittedIdentityError::StateRead { .. } => "state-read",
        CommittedIdentityError::StateMalformed { .. } => "state-malformed",
        CommittedIdentityError::StateInstanceMismatch { .. } => "state-instance-mismatch",
    }
}

fn assert_socket_substitution_rejected(
    root: &Path,
    leaf: &Path,
    open_ordinal: usize,
    expected: &'static str,
) {
    let admitted = JournalRoot::open(root).expect("journal root admits");
    let leaf = leaf.to_path_buf();
    let (result, fired) = run_with_bound_read_barrier(
        BoundReadPrimitive::Open,
        open_ordinal,
        move || {
            fs::remove_file(&leaf).expect("committed leaf removes");
            let listener = UnixListener::bind(&leaf).expect("socket binds");
            drop(listener);
        },
        || load_committed_identity_bound(&admitted),
    );
    assert!(fired, "bound read barrier fires");
    let error = match result {
        Ok(_) => panic!("socket substitution must reject"),
        Err(error) => error,
    };
    assert_eq!(error_kind(&error), expected);
    assert!(
        !root.join("mcp-endpoint").exists(),
        "link reader cannot create endpoint state"
    );
}

#[test]
fn socket_substitution_is_rejected_for_every_committed_leaf() {
    let certificate = TestRoot::new("certificate");
    write_identity(certificate.path(), false);
    assert_socket_substitution_rejected(
        certificate.path(),
        &certificate.path().join("link/ca/cert.pem"),
        1,
        "certificate-read",
    );

    let private_key = TestRoot::new("private-key");
    write_identity(private_key.path(), false);
    assert_socket_substitution_rejected(
        private_key.path(),
        &private_key.path().join("link/ca/private.pem"),
        2,
        "private-key-read",
    );

    let primary_state = TestRoot::new("primary-state");
    write_identity(primary_state.path(), false);
    assert_socket_substitution_rejected(
        primary_state.path(),
        &primary_state.path().join("link/state.json"),
        3,
        "state-read",
    );

    let fallback_state = TestRoot::new("fallback-state");
    write_identity(fallback_state.path(), true);
    assert_socket_substitution_rejected(
        fallback_state.path(),
        &fallback_state.path().join("link/ca/state.json"),
        3,
        "state-read",
    );
}
