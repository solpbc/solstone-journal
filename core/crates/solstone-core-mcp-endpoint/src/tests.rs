// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use nix::fcntl::OFlag;
use nix::sys::stat::{FileStat, Mode, SFlag, fstat};
use nix::unistd::{geteuid, mkfifo};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use tempfile::TempDir;

use crate::test_seam::{
    OwnerBootstrapPrimitive, run_with_owner_barrier, run_with_owner_fault,
    run_with_two_owner_faults,
};
use crate::unix::{
    FILE_OPEN_FLAGS, LOCK_OPEN_FLAGS, identity, is_directory, is_exact_directory, is_exact_regular,
    is_regular, same_key_metadata,
};
use crate::{McpEndpointBootstrapError, bootstrap_mcp_endpoint_owner_identity};

#[derive(Clone, Copy)]
enum StateLayout {
    Primary,
    Fallback,
    Both,
}

#[derive(Debug, Eq, PartialEq)]
enum LinkEntry {
    Directory {
        mode: u32,
        device: u64,
        inode: u64,
        mtime: i64,
        mtime_nsec: i64,
    },
    File {
        bytes: Vec<u8>,
        mode: u32,
        device: u64,
        inode: u64,
        mtime: i64,
        mtime_nsec: i64,
    },
    Symlink(Vec<u8>),
    Other,
}

fn write_config(root: &Path, config: &[u8]) {
    fs::create_dir_all(root.join("config")).expect("test config directory creates");
    fs::write(root.join("config/journal.json"), config).expect("test config writes");
}

fn write_enabled_config(root: &Path) {
    write_config(root, br#"{"mcp_endpoint":{"enabled":true}}"#);
}

fn write_identity(root: &Path, layout: StateLayout) -> Vec<u8> {
    let ca = solstone_core_sol_link::ca::generate_ca().expect("test CA generates");
    let instance_id =
        solstone_core_sol_link::ca::jid_from_spki(ca.spki_der()).expect("test JID derives");
    let ca_directory = root.join("link/ca");
    fs::create_dir_all(&ca_directory).expect("test CA directory creates");
    fs::write(ca_directory.join("cert.pem"), ca.certificate_pem())
        .expect("test certificate writes");
    fs::write(ca_directory.join("private.pem"), ca.private_key_pem())
        .expect("test private key writes");
    let primary = format!(r#"{{"instance_id":"{instance_id}","home_label":"Primary"}}"#);
    let fallback = format!(r#"{{"instance_id":"{instance_id}","home_label":"Fallback"}}"#);
    match layout {
        StateLayout::Primary => {
            fs::write(root.join("link/state.json"), primary).expect("test primary state writes");
        }
        StateLayout::Fallback => {
            fs::write(ca_directory.join("state.json"), fallback)
                .expect("test fallback state writes");
        }
        StateLayout::Both => {
            fs::write(root.join("link/state.json"), primary).expect("test primary state writes");
            fs::write(ca_directory.join("state.json"), b"{")
                .expect("test malformed fallback writes");
        }
    }
    ca.spki_der().to_vec()
}

fn endpoint_path(root: &Path) -> PathBuf {
    root.join("mcp-endpoint")
}

fn assert_no_endpoint(root: &Path) {
    assert!(
        !endpoint_path(root).exists(),
        "disabled or refused bootstrap must not create endpoint state"
    );
}

fn assert_disabled_without_late_witness(root: &Path) {
    for primitive in [
        OwnerBootstrapPrimitive::CommittedIdentityLoad,
        OwnerBootstrapPrimitive::EffectiveUid,
        OwnerBootstrapPrimitive::DirectoryNoFollowProbe,
        OwnerBootstrapPrimitive::KeyGenerate,
    ] {
        let (result, consumed) = run_with_owner_fault(primitive, 1, nix::libc::EIO, || {
            bootstrap_mcp_endpoint_owner_identity(root)
        });
        assert!(matches!(result, Ok(None)));
        assert!(!consumed, "disabled gate reached {primitive:?}");
        assert_no_endpoint(root);
    }
}

#[test]
fn disabled_configurations_return_none_without_late_witnesses() {
    let missing = TempDir::new().expect("test root creates");
    assert_disabled_without_late_witness(missing.path());

    let absent_endpoint = TempDir::new().expect("test root creates");
    write_config(absent_endpoint.path(), br#"{}"#);
    assert_disabled_without_late_witness(absent_endpoint.path());

    let absent_enabled = TempDir::new().expect("test root creates");
    write_config(absent_enabled.path(), br#"{"mcp_endpoint":{}}"#);
    assert_disabled_without_late_witness(absent_enabled.path());

    let disabled = TempDir::new().expect("test root creates");
    write_config(disabled.path(), br#"{"mcp_endpoint":{"enabled":false}}"#);
    assert_disabled_without_late_witness(disabled.path());
}

#[test]
fn malformed_and_invalid_capability_configs_stop_before_endpoint_state() {
    let malformed = TempDir::new().expect("test root creates");
    write_config(malformed.path(), b"{");
    assert!(matches!(
        bootstrap_mcp_endpoint_owner_identity(malformed.path()),
        Err(McpEndpointBootstrapError::ConfigRead)
    ));
    assert_no_endpoint(malformed.path());

    let capability = TempDir::new().expect("test root creates");
    write_config(capability.path(), br#"{"mcp_endpoint":{"enabled":"yes"}}"#);
    assert!(matches!(
        bootstrap_mcp_endpoint_owner_identity(capability.path()),
        Err(McpEndpointBootstrapError::Capability)
    ));
    assert_no_endpoint(capability.path());
}

#[test]
fn disabled_root_is_independent_of_an_enabled_root() {
    let enabled = TempDir::new().expect("test enabled root creates");
    write_enabled_config(enabled.path());
    write_identity(enabled.path(), StateLayout::Primary);
    assert!(matches!(
        bootstrap_mcp_endpoint_owner_identity(enabled.path()),
        Ok(Some(_))
    ));

    let disabled = TempDir::new().expect("test disabled root creates");
    assert!(matches!(
        bootstrap_mcp_endpoint_owner_identity(disabled.path()),
        Ok(None)
    ));
    assert_no_endpoint(disabled.path());
}

#[test]
fn invalid_committed_identity_never_reaches_endpoint_state_or_key_generation() {
    let missing = TempDir::new().expect("test root creates");
    write_enabled_config(missing.path());
    assert_identity_failure_is_early(missing.path());

    let incomplete = TempDir::new().expect("test root creates");
    write_enabled_config(incomplete.path());
    fs::create_dir_all(incomplete.path().join("link/ca")).expect("test partial CA creates");
    assert_identity_failure_is_early(incomplete.path());

    let malformed = TempDir::new().expect("test root creates");
    write_enabled_config(malformed.path());
    write_identity(malformed.path(), StateLayout::Primary);
    fs::write(malformed.path().join("link/ca/cert.pem"), b"not PEM")
        .expect("test malformed certificate writes");
    assert_identity_failure_is_early(malformed.path());

    let mismatched = TempDir::new().expect("test root creates");
    write_enabled_config(mismatched.path());
    write_identity(mismatched.path(), StateLayout::Primary);
    fs::write(
        mismatched.path().join("link/state.json"),
        br#"{"instance_id":"wrong","home_label":"Mismatch"}"#,
    )
    .expect("test mismatched state writes");
    assert_identity_failure_is_early(mismatched.path());
}

fn assert_identity_failure_is_early(root: &Path) {
    for primitive in [
        OwnerBootstrapPrimitive::EffectiveUid,
        OwnerBootstrapPrimitive::KeyGenerate,
    ] {
        let (result, consumed) = run_with_owner_fault(primitive, 1, nix::libc::EIO, || {
            bootstrap_mcp_endpoint_owner_identity(root)
        });
        assert!(matches!(result, Err(McpEndpointBootstrapError::Endpoint)));
        assert!(
            !consumed,
            "invalid committed identity reached {primitive:?}"
        );
    }
    assert_no_endpoint(root);
}

#[test]
fn accepted_committed_identity_layouts_bootstrap_a_verifying_key() {
    for layout in [
        StateLayout::Primary,
        StateLayout::Fallback,
        StateLayout::Both,
    ] {
        let root = TempDir::new().expect("test root creates");
        write_enabled_config(root.path());
        write_identity(root.path(), layout);
        let context = bootstrap_mcp_endpoint_owner_identity(root.path())
            .expect("bootstrap succeeds")
            .expect("enabled endpoint returns a context");
        assert_eq!(context.test_verifying_key_bytes().len(), 32);
        assert_eq!(
            fs::metadata(endpoint_path(root.path()))
                .expect("endpoint directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(endpoint_path(root.path()).join(".create.lock"))
                .expect("lock metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(endpoint_path(root.path()).join("pop.ed25519.pk8"))
                .expect("key metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn reload_reuses_the_existing_pop_key_without_replacing_it() {
    let root = TempDir::new().expect("test root creates");
    write_enabled_config(root.path());
    write_identity(root.path(), StateLayout::Primary);

    let first = bootstrap_mcp_endpoint_owner_identity(root.path())
        .expect("first bootstrap succeeds")
        .expect("enabled endpoint returns a context");
    let key_path = endpoint_path(root.path()).join("pop.ed25519.pk8");
    let before = fs::metadata(&key_path).expect("key metadata");
    let second = bootstrap_mcp_endpoint_owner_identity(root.path())
        .expect("second bootstrap succeeds")
        .expect("enabled endpoint returns a context");
    let after = fs::metadata(&key_path).expect("key metadata");

    assert_eq!(
        first.test_verifying_key_bytes(),
        second.test_verifying_key_bytes()
    );
    assert_eq!(before.ino(), after.ino());
    assert_eq!(before.mtime(), after.mtime());
    assert_eq!(before.mtime_nsec(), after.mtime_nsec());
    assert_eq!(
        fs::read_dir(endpoint_path(root.path()))
            .expect("endpoint listing")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() == "pop.ed25519.pk8")
            .count(),
        1
    );
}

#[test]
fn bootstrap_keeps_link_material_unchanged_and_uses_a_distinct_key_type() {
    let root = TempDir::new().expect("test root creates");
    write_enabled_config(root.path());
    let ca_spki = write_identity(root.path(), StateLayout::Primary);
    let before = snapshot_link(root.path());

    let context = bootstrap_mcp_endpoint_owner_identity(root.path())
        .expect("bootstrap succeeds")
        .expect("enabled endpoint returns a context");

    assert_ne!(context.test_verifying_key_bytes(), ca_spki);
    assert_eq!(snapshot_link(root.path()), before);
}

fn snapshot_link(root: &Path) -> BTreeMap<PathBuf, LinkEntry> {
    let link = root.join("link");
    let mut entries = BTreeMap::new();
    snapshot_directory(&link, Path::new(""), &mut entries);
    entries
}

fn snapshot_directory(path: &Path, relative: &Path, entries: &mut BTreeMap<PathBuf, LinkEntry>) {
    let metadata = fs::symlink_metadata(path).expect("test link entry metadata");
    entries.insert(relative.to_path_buf(), snapshot_entry(path, &metadata));
    if metadata.is_dir() {
        let mut children = fs::read_dir(path)
            .expect("test link directory listing")
            .map(|entry| entry.expect("test link entry"))
            .collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            snapshot_directory(&child.path(), &relative.join(child.file_name()), entries);
        }
    }
}

fn snapshot_entry(path: &Path, metadata: &fs::Metadata) -> LinkEntry {
    if metadata.is_dir() {
        LinkEntry::Directory {
            mode: metadata.permissions().mode(),
            device: metadata.dev(),
            inode: metadata.ino(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
        }
    } else if metadata.is_file() {
        LinkEntry::File {
            bytes: fs::read(path).expect("test link file reads"),
            mode: metadata.permissions().mode(),
            device: metadata.dev(),
            inode: metadata.ino(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
        }
    } else if metadata.file_type().is_symlink() {
        LinkEntry::Symlink(
            fs::read_link(path)
                .expect("test link symlink reads")
                .into_os_string()
                .into_encoded_bytes(),
        )
    } else {
        LinkEntry::Other
    }
}

fn bootstrap_context(root: &Path) -> crate::McpEndpointOwnerContext {
    bootstrap_mcp_endpoint_owner_identity(root)
        .expect("bootstrap succeeds")
        .expect("enabled endpoint returns a context")
}

fn assert_endpoint_error(
    result: Result<Option<crate::McpEndpointOwnerContext>, McpEndpointBootstrapError>,
) {
    assert!(matches!(result, Err(McpEndpointBootstrapError::Endpoint)));
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("test mode sets");
}

fn prepare_endpoint_directory(root: &Path) -> PathBuf {
    let endpoint = endpoint_path(root);
    fs::create_dir(&endpoint).expect("test endpoint directory creates");
    set_mode(&endpoint, 0o700);
    endpoint
}

fn write_private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("test private file writes");
    set_mode(path, 0o600);
}

fn valid_pkcs8() -> Vec<u8> {
    Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .expect("test Ed25519 key generates")
        .as_ref()
        .to_vec()
}

fn verifying_key(bytes: &[u8]) -> Vec<u8> {
    Ed25519KeyPair::from_pkcs8(bytes)
        .expect("test Ed25519 key parses")
        .public_key()
        .as_ref()
        .to_vec()
}

fn assert_consistent_endpoint_artifacts(root: &Path) {
    let endpoint = endpoint_path(root);
    if !endpoint.exists() {
        return;
    }
    let directory = fs::metadata(&endpoint).expect("endpoint metadata");
    assert!(directory.is_dir());
    assert_eq!(directory.permissions().mode() & 0o777, 0o700);
    let lock = endpoint.join(".create.lock");
    if lock.exists() {
        let lock_metadata = fs::metadata(&lock).expect("lock metadata");
        assert!(lock_metadata.is_file());
        assert_eq!(lock_metadata.permissions().mode() & 0o777, 0o600);
    }
    let key = endpoint.join("pop.ed25519.pk8");
    if key.exists() {
        let key_metadata = fs::metadata(&key).expect("key metadata");
        assert!(key_metadata.is_file());
        assert_eq!(key_metadata.permissions().mode() & 0o777, 0o600);
        let bytes = fs::read(key).expect("key reads");
        assert!(Ed25519KeyPair::from_pkcs8(&bytes).is_ok());
    }
}

fn prepared_enabled_root() -> TempDir {
    let root = TempDir::new().expect("test root creates");
    write_enabled_config(root.path());
    write_identity(root.path(), StateLayout::Primary);
    root
}

#[test]
fn root_path_swaps_are_rejected_before_endpoint_work() {
    for primitive in [
        OwnerBootstrapPrimitive::CommittedIdentityLoad,
        OwnerBootstrapPrimitive::RootRevalidateBeforeEndpoint,
    ] {
        let sandbox = TempDir::new().expect("test sandbox creates");
        let root_a = sandbox.path().join("root-a");
        let root_b = sandbox.path().join("root-b");
        let detached_a = sandbox.path().join("root-a-detached");
        fs::create_dir(&root_a).expect("root A creates");
        fs::create_dir(&root_b).expect("root B creates");
        write_enabled_config(&root_a);
        write_identity(&root_a, StateLayout::Primary);
        write_enabled_config(&root_b);
        write_identity(&root_b, StateLayout::Primary);

        let bootstrap_path = root_a.clone();
        let swap_a = root_a.clone();
        let swap_b = root_b.clone();
        let swap_detached = detached_a.clone();
        let (result, fired) = run_with_owner_barrier(
            primitive,
            1,
            move || {
                fs::rename(&swap_a, &swap_detached).expect("root A moves aside");
                fs::rename(&swap_b, &swap_a).expect("root B takes root A name");
            },
            || bootstrap_mcp_endpoint_owner_identity(&bootstrap_path),
        );

        assert!(fired);
        assert!(
            matches!(result, Err(McpEndpointBootstrapError::Endpoint)),
            "{primitive:?} fault must refuse bootstrap"
        );
        assert_no_endpoint(&detached_a);
        assert_no_endpoint(&root_a);
    }
}

#[test]
fn rename_free_root_control_bootstraps_only_its_own_journal() {
    let sandbox = TempDir::new().expect("test sandbox creates");
    let root_a = sandbox.path().join("root-a");
    let root_b = sandbox.path().join("root-b");
    fs::create_dir(&root_a).expect("root A creates");
    fs::create_dir(&root_b).expect("root B creates");
    write_enabled_config(&root_a);
    write_identity(&root_a, StateLayout::Primary);
    write_enabled_config(&root_b);
    write_identity(&root_b, StateLayout::Primary);

    let _context = bootstrap_context(&root_a);
    assert!(endpoint_path(&root_a).is_dir());
    assert_no_endpoint(&root_b);
}

fn synthetic_stat(kind: SFlag, permissions: u32, owner: u32) -> FileStat {
    let temporary = TempDir::new().expect("test metadata directory creates");
    let file = File::open(temporary.path()).expect("test metadata directory opens");
    let mut stat = fstat(&file).expect("test metadata stats");
    stat.st_mode = kind.bits() | permissions as nix::libc::mode_t;
    stat.st_uid = owner;
    stat.st_dev = 41;
    stat.st_ino = 99;
    stat.st_size = 48;
    stat
}

#[test]
fn metadata_validators_require_exact_type_owner_and_mode() {
    let owner = geteuid().as_raw();
    let wrong_low = if owner == 0 { 1 } else { owner - 1 };
    let wrong_high = owner.saturating_add(1);
    let directory = synthetic_stat(SFlag::S_IFDIR, 0o700, owner);
    let regular = synthetic_stat(SFlag::S_IFREG, 0o600, owner);

    assert!(is_directory(&directory));
    assert!(is_exact_directory(&directory, owner, 0o700));
    assert!(is_regular(&regular));
    assert!(is_exact_regular(&regular, owner, 0o600));
    assert!(same_key_metadata(&regular, &regular, owner));
    assert_eq!(identity(&directory), identity(&directory));

    for kind in [SFlag::S_IFLNK, SFlag::S_IFSOCK, SFlag::S_IFREG] {
        let candidate = synthetic_stat(kind, 0o700, owner);
        assert!(!is_exact_directory(&candidate, owner, 0o700));
    }
    for kind in [SFlag::S_IFLNK, SFlag::S_IFSOCK, SFlag::S_IFDIR] {
        let candidate = synthetic_stat(kind, 0o600, owner);
        assert!(!is_exact_regular(&candidate, owner, 0o600));
    }
    for mode in [0o701, 0o600, 0o750] {
        assert!(!is_exact_directory(
            &synthetic_stat(SFlag::S_IFDIR, mode, owner),
            owner,
            0o700
        ));
    }
    for mode in [0o601, 0o400, 0o640] {
        assert!(!is_exact_regular(
            &synthetic_stat(SFlag::S_IFREG, mode, owner),
            owner,
            0o600
        ));
    }
    for wrong_owner in [wrong_low, wrong_high] {
        assert!(!is_exact_directory(&directory, wrong_owner, 0o700));
        assert!(!is_exact_regular(&regular, wrong_owner, 0o600));
    }

    let mut different_inode = synthetic_stat(SFlag::S_IFREG, 0o600, owner);
    different_inode.st_ino = 100;
    assert!(!same_key_metadata(&regular, &different_inode, owner));
}

#[test]
fn untrusted_file_opens_are_nonblocking_and_no_follow() {
    for flags in [LOCK_OPEN_FLAGS, FILE_OPEN_FLAGS] {
        assert!(flags.contains(OFlag::O_NOFOLLOW));
        assert!(flags.contains(OFlag::O_NONBLOCK));
    }
}

#[test]
fn bootstrap_rejects_symlink_entries_at_directory_lock_and_key_names() {
    let directory = prepared_enabled_root();
    symlink("unrelated", endpoint_path(directory.path())).expect("directory symlink creates");
    assert_endpoint_error(bootstrap_mcp_endpoint_owner_identity(directory.path()));

    let lock = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(lock.path());
    symlink("unrelated", endpoint.join(".create.lock")).expect("lock symlink creates");
    assert_endpoint_error(bootstrap_mcp_endpoint_owner_identity(lock.path()));

    let key = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(key.path());
    symlink("unrelated", endpoint.join("pop.ed25519.pk8")).expect("key symlink creates");
    assert_endpoint_error(bootstrap_mcp_endpoint_owner_identity(key.path()));
}

#[test]
fn bootstrap_rejects_wrong_type_entries_at_directory_lock_and_key_names() {
    let directory = prepared_enabled_root();
    fs::write(endpoint_path(directory.path()), b"not a directory").expect("wrong directory writes");
    assert_endpoint_error(bootstrap_mcp_endpoint_owner_identity(directory.path()));

    let lock = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(lock.path());
    fs::create_dir(endpoint.join(".create.lock")).expect("wrong lock directory creates");
    assert_endpoint_error(bootstrap_mcp_endpoint_owner_identity(lock.path()));

    let key = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(key.path());
    fs::create_dir(endpoint.join("pop.ed25519.pk8")).expect("wrong key directory creates");
    assert_endpoint_error(bootstrap_mcp_endpoint_owner_identity(key.path()));
}

#[test]
fn post_precheck_hostile_key_plants_are_never_overwritten_or_trusted_when_invalid() {
    let planted = prepared_enabled_root();
    let planted_key = valid_pkcs8();
    let expected = verifying_key(&planted_key);
    let planted_path = endpoint_path(planted.path()).join("pop.ed25519.pk8");
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::KeyGenerate,
        1,
        move || write_private_file(&planted_path, &planted_key),
        || bootstrap_mcp_endpoint_owner_identity(planted.path()),
    );
    assert!(fired);
    let context = result
        .expect("planted valid key bootstrap succeeds")
        .expect("enabled endpoint returns a context");
    assert_eq!(context.test_verifying_key_bytes(), expected);

    for plant in ["corrupt", "symlink", "fifo"] {
        let root = prepared_enabled_root();
        let key_path = endpoint_path(root.path()).join("pop.ed25519.pk8");
        let (result, fired) = run_with_owner_barrier(
            OwnerBootstrapPrimitive::KeyGenerate,
            1,
            move || match plant {
                "corrupt" => write_private_file(&key_path, b"not PKCS8"),
                "symlink" => symlink("unrelated", &key_path).expect("key symlink creates"),
                "fifo" => {
                    mkfifo(&key_path, Mode::from_bits_truncate(0o600)).expect("key FIFO creates")
                }
                _ => unreachable!("known hostile key plant"),
            },
            || bootstrap_mcp_endpoint_owner_identity(root.path()),
        );
        assert!(fired, "{plant} plant barrier fires");
        assert_endpoint_error(result);
    }
}

fn assert_fifo_replacement_is_bounded(
    root: &Path,
    primitive: OwnerBootstrapPrimitive,
    replacement: PathBuf,
) {
    let bootstrap_path = root.to_path_buf();
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (result, fired) = run_with_owner_barrier(
            primitive,
            1,
            move || {
                fs::remove_file(&replacement).expect("regular entry removes");
                mkfifo(&replacement, Mode::from_bits_truncate(0o600))
                    .expect("FIFO replacement creates");
            },
            || bootstrap_mcp_endpoint_owner_identity(&bootstrap_path),
        );
        sender
            .send((
                matches!(result, Err(McpEndpointBootstrapError::Endpoint)),
                fired,
            ))
            .expect("bounded worker reports");
    });
    let (refused, fired) = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("nonblocking FIFO open completes within the bounded window");
    worker.join().expect("bounded worker joins");
    assert!(fired);
    assert!(refused);
}

#[test]
fn fifo_replacements_between_probe_and_open_fail_without_blocking() {
    let lock = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(lock.path());
    let lock_path = endpoint.join(".create.lock");
    write_private_file(&lock_path, b"");
    assert_fifo_replacement_is_bounded(lock.path(), OwnerBootstrapPrimitive::LockOpen, lock_path);

    let key = prepared_enabled_root();
    let _context = bootstrap_context(key.path());
    let key_path = endpoint_path(key.path()).join("pop.ed25519.pk8");
    assert_fifo_replacement_is_bounded(key.path(), OwnerBootstrapPrimitive::KeyOpen, key_path);
}

fn assert_final_key_name_replacement_is_rejected(root: &Path) {
    let key_path = endpoint_path(root).join("pop.ed25519.pk8");
    let aside = endpoint_path(root).join("pop.ed25519.pk8.aside");
    let replacement = valid_pkcs8();
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::FinalKeyOpen,
        1,
        move || {
            fs::rename(&key_path, &aside).expect("current key moves aside");
            write_private_file(&key_path, &replacement);
        },
        || bootstrap_mcp_endpoint_owner_identity(root),
    );
    assert!(fired);
    assert_endpoint_error(result);
}

#[test]
fn final_key_name_replacements_are_rejected_for_existing_and_fresh_keys() {
    let existing = prepared_enabled_root();
    let _context = bootstrap_context(existing.path());
    assert_final_key_name_replacement_is_rejected(existing.path());

    let fresh = prepared_enabled_root();
    assert_final_key_name_replacement_is_rejected(fresh.path());
}

#[test]
fn final_key_content_compare_rejects_same_inode_overwrites_and_accepts_unchanged_bytes() {
    let root = prepared_enabled_root();
    let _context = bootstrap_context(root.path());
    let key_path = endpoint_path(root.path()).join("pop.ed25519.pk8");
    let before = fs::metadata(&key_path).expect("key metadata");
    let replacement = valid_pkcs8();
    assert_eq!(
        replacement.len(),
        fs::read(&key_path).expect("key reads").len()
    );
    let overwrite_path = key_path.clone();
    let replacement_for_barrier = replacement.clone();
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::FinalKeyContentCompare,
        1,
        move || {
            let mut file = OpenOptions::new()
                .write(true)
                .open(&overwrite_path)
                .expect("same inode opens for overwrite");
            file.seek(SeekFrom::Start(0)).expect("overwrite seeks");
            file.write_all(&replacement_for_barrier)
                .expect("same-length overwrite writes");
            file.sync_all().expect("overwrite syncs");
        },
        || bootstrap_mcp_endpoint_owner_identity(root.path()),
    );
    assert!(fired);
    assert_endpoint_error(result);
    let after = fs::metadata(&key_path).expect("key metadata");
    assert_eq!(before.ino(), after.ino());
    assert_eq!(
        before.permissions().mode() & 0o777,
        after.permissions().mode() & 0o777
    );
    assert_eq!(
        fs::read(&key_path).expect("overwritten key reads"),
        replacement
    );

    let clean = prepared_enabled_root();
    let _context = bootstrap_context(clean.path());
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::FinalKeyContentCompare,
        1,
        || {},
        || bootstrap_mcp_endpoint_owner_identity(clean.path()),
    );
    assert!(fired);
    assert!(matches!(result, Ok(Some(_))));
}

#[test]
fn directory_binding_checks_reject_swaps_before_lock_and_before_success() {
    let before_lock = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(before_lock.path());
    let sentinel = endpoint.join("sentinel");
    fs::write(&sentinel, b"original").expect("sentinel writes");
    let aside = endpoint_path(before_lock.path()).with_extension("aside");
    let replacement = endpoint_path(before_lock.path());
    let callback_aside = aside.clone();
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::DirectoryBindingCheckBeforeLock,
        1,
        move || {
            fs::rename(&replacement, &callback_aside).expect("original endpoint moves aside");
            fs::create_dir(&replacement).expect("replacement endpoint creates");
            set_mode(&replacement, 0o700);
        },
        || bootstrap_mcp_endpoint_owner_identity(before_lock.path()),
    );
    assert!(fired);
    assert_endpoint_error(result);
    assert_eq!(
        fs::read(aside.join("sentinel")).expect("sentinel reads"),
        b"original"
    );
    assert!(
        fs::read_dir(endpoint_path(before_lock.path()))
            .expect("replacement listing")
            .next()
            .is_none()
    );

    let before_success = prepared_enabled_root();
    let endpoint = endpoint_path(before_success.path());
    let aside = endpoint.with_extension("aside");
    let replacement = endpoint.clone();
    let callback_aside = aside.clone();
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::DirectoryBindingCheckBeforeSuccess,
        1,
        move || {
            fs::rename(&replacement, &callback_aside).expect("published endpoint moves aside");
            fs::create_dir(&replacement).expect("replacement endpoint creates");
            set_mode(&replacement, 0o700);
        },
        || bootstrap_mcp_endpoint_owner_identity(before_success.path()),
    );
    assert!(fired);
    assert_endpoint_error(result);
    assert!(aside.join("pop.ed25519.pk8").is_file());
    assert!(
        fs::read_dir(endpoint_path(before_success.path()))
            .expect("replacement listing")
            .next()
            .is_none()
    );

    let context = bootstrap_context(before_success.path());
    let current = fs::read(endpoint_path(before_success.path()).join("pop.ed25519.pk8"))
        .expect("replacement key reads");
    assert_eq!(context.test_verifying_key_bytes(), verifying_key(&current));
}

#[test]
fn cooperating_bootstraps_share_one_key_without_touching_link_state() {
    let root = prepared_enabled_root();
    let ca_spki = solstone_core_sol_link::committed::load_committed_identity(root.path())
        .expect("committed identity loads")
        .ca()
        .spki_der()
        .to_vec();
    let link_before = snapshot_link(root.path());
    let start = Arc::new(Barrier::new(3));
    let journal_one = root.path().to_path_buf();
    let journal_two = root.path().to_path_buf();
    let start_one = Arc::clone(&start);
    let start_two = Arc::clone(&start);
    let first = thread::spawn(move || {
        start_one.wait();
        bootstrap_mcp_endpoint_owner_identity(&journal_one)
            .expect("first bootstrap succeeds")
            .expect("first context returns")
            .test_verifying_key_bytes()
    });
    let second = thread::spawn(move || {
        start_two.wait();
        bootstrap_mcp_endpoint_owner_identity(&journal_two)
            .expect("second bootstrap succeeds")
            .expect("second context returns")
            .test_verifying_key_bytes()
    });
    start.wait();
    let first = first.join().expect("first bootstrap thread joins");
    let second = second.join().expect("second bootstrap thread joins");
    let third = bootstrap_context(root.path()).test_verifying_key_bytes();

    assert_eq!(first, second);
    assert_eq!(first, third);
    assert_ne!(first, ca_spki);
    assert_eq!(snapshot_link(root.path()), link_before);
    assert_eq!(
        fs::read_dir(endpoint_path(root.path()))
            .expect("endpoint listing")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() == "pop.ed25519.pk8")
            .count(),
        1
    );
}

const OWNER_FAULT_PRIMITIVES: &[OwnerBootstrapPrimitive] = &[
    OwnerBootstrapPrimitive::CommittedIdentityLoad,
    OwnerBootstrapPrimitive::RootRevalidateBeforeEndpoint,
    OwnerBootstrapPrimitive::EffectiveUid,
    OwnerBootstrapPrimitive::DirectoryNoFollowProbe,
    OwnerBootstrapPrimitive::DirectoryCreate,
    OwnerBootstrapPrimitive::DirectoryOpen,
    OwnerBootstrapPrimitive::DirectoryFchmod,
    OwnerBootstrapPrimitive::DirectoryFstat,
    OwnerBootstrapPrimitive::RootRevalidateAndFsync,
    OwnerBootstrapPrimitive::DirectoryBindingCheckBeforeLock,
    OwnerBootstrapPrimitive::LockNoFollowProbe,
    OwnerBootstrapPrimitive::LockCreate,
    OwnerBootstrapPrimitive::LockOpen,
    OwnerBootstrapPrimitive::LockFchmod,
    OwnerBootstrapPrimitive::LockFstat,
    OwnerBootstrapPrimitive::LockAcquire,
    OwnerBootstrapPrimitive::KeyPrecheckStat,
    OwnerBootstrapPrimitive::KeyOpen,
    OwnerBootstrapPrimitive::KeyFstat,
    OwnerBootstrapPrimitive::KeyRead,
    OwnerBootstrapPrimitive::KeyFinalRestat,
    OwnerBootstrapPrimitive::KeyDecode,
    OwnerBootstrapPrimitive::KeyGenerate,
    OwnerBootstrapPrimitive::KeyPublish,
    OwnerBootstrapPrimitive::FinalKeyOpen,
    OwnerBootstrapPrimitive::FinalKeyRestat,
    OwnerBootstrapPrimitive::FinalKeyContentCompare,
    OwnerBootstrapPrimitive::FinalKeyFsync,
    OwnerBootstrapPrimitive::FinalDirectoryFsync,
    OwnerBootstrapPrimitive::DirectoryBindingCheckBeforeSuccess,
];

#[test]
fn every_owner_checkpoint_fault_refuses_without_partial_key_material() {
    for primitive in OWNER_FAULT_PRIMITIVES {
        let root = prepared_enabled_root();
        if *primitive == OwnerBootstrapPrimitive::LockOpen {
            let endpoint = prepare_endpoint_directory(root.path());
            write_private_file(&endpoint.join(".create.lock"), b"");
        }
        let (result, consumed) = run_with_owner_fault(*primitive, 1, nix::libc::EIO, || {
            bootstrap_mcp_endpoint_owner_identity(root.path())
        });
        assert!(
            matches!(result, Err(McpEndpointBootstrapError::Endpoint)),
            "{primitive:?} fault must refuse bootstrap"
        );
        assert!(consumed, "{primitive:?} fault checkpoint was reached");
        assert_consistent_endpoint_artifacts(root.path());
    }
}

#[test]
fn root_revalidate_and_fsync_failure_stops_later_witnesses_and_a_clean_call_recovers() {
    for blocked in [
        OwnerBootstrapPrimitive::LockAcquire,
        OwnerBootstrapPrimitive::KeyGenerate,
    ] {
        let root = prepared_enabled_root();
        let (result, consumed) = run_with_two_owner_faults(
            OwnerBootstrapPrimitive::RootRevalidateAndFsync,
            1,
            nix::libc::EIO,
            blocked,
            1,
            nix::libc::EIO,
            || bootstrap_mcp_endpoint_owner_identity(root.path()),
        );
        assert_endpoint_error(result);
        assert_eq!(consumed, 1, "root failure prevents {blocked:?} witness");
        assert!(endpoint_path(root.path()).is_dir());
        assert!(!endpoint_path(root.path()).join(".create.lock").exists());
        assert!(!endpoint_path(root.path()).join("pop.ed25519.pk8").exists());
    }

    let root = prepared_enabled_root();
    let (result, consumed) = run_with_owner_fault(
        OwnerBootstrapPrimitive::RootRevalidateAndFsync,
        1,
        nix::libc::EIO,
        || bootstrap_mcp_endpoint_owner_identity(root.path()),
    );
    assert_endpoint_error(result);
    assert!(consumed);
    let context = bootstrap_context(root.path());
    assert_eq!(context.test_verifying_key_bytes().len(), 32);
    assert!(endpoint_path(root.path()).join("pop.ed25519.pk8").is_file());
}

#[test]
fn successful_bootstrap_reopens_the_same_durable_key_bytes() {
    let root = prepared_enabled_root();
    let first = bootstrap_context(root.path()).test_verifying_key_bytes();
    let endpoint = endpoint_path(root.path());
    let directory_before = fs::metadata(&endpoint).expect("endpoint metadata");
    let key_path = endpoint.join("pop.ed25519.pk8");
    let bytes_before = fs::read(&key_path).expect("key reads");

    let second = bootstrap_context(root.path()).test_verifying_key_bytes();
    let directory_after = fs::metadata(&endpoint).expect("endpoint metadata");
    assert_eq!(first, second);
    assert_eq!(
        bytes_before,
        fs::read(&key_path).expect("reopened key reads")
    );
    assert_eq!(directory_before.dev(), directory_after.dev());
    assert_eq!(directory_before.ino(), directory_after.ino());
}

fn root_with_preexisting_key(bytes: &[u8]) -> TempDir {
    let root = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(root.path());
    write_private_file(&endpoint.join("pop.ed25519.pk8"), bytes);
    root
}

#[test]
fn oversized_key_prechecks_avoid_reads_and_exact_limit_reaches_decode() {
    let mut exactly_513 = valid_pkcs8();
    exactly_513.resize(513, 0);
    let exact = root_with_preexisting_key(&exactly_513);
    assert_endpoint_error(bootstrap_mcp_endpoint_owner_identity(exact.path()));
    let (result, consumed) = run_with_owner_fault(
        OwnerBootstrapPrimitive::KeyDecode,
        1,
        nix::libc::EIO,
        || bootstrap_mcp_endpoint_owner_identity(exact.path()),
    );
    assert_endpoint_error(result);
    assert!(consumed, "exact-limit input reaches the decode checkpoint");

    let large = root_with_preexisting_key(&vec![b'x'; 514]);
    let (result, consumed) =
        run_with_owner_fault(OwnerBootstrapPrimitive::KeyRead, 1, nix::libc::EIO, || {
            bootstrap_mcp_endpoint_owner_identity(large.path())
        });
    assert_endpoint_error(result);
    assert!(
        !consumed,
        "ordinary oversized key is refused before reading"
    );

    let sparse = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(sparse.path());
    let key_path = endpoint.join("pop.ed25519.pk8");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&key_path)
        .expect("sparse key creates");
    file.set_len(1024 * 1024 * 1024)
        .expect("sparse key length sets");
    drop(file);
    set_mode(&key_path, 0o600);
    assert_eq!(
        fs::metadata(&key_path).expect("sparse key metadata").len(),
        1024 * 1024 * 1024
    );
    let (result, consumed) =
        run_with_owner_fault(OwnerBootstrapPrimitive::KeyRead, 1, nix::libc::EIO, || {
            bootstrap_mcp_endpoint_owner_identity(sparse.path())
        });
    assert_endpoint_error(result);
    assert!(!consumed, "sparse oversized key is refused before reading");
}

#[test]
fn stale_stage_files_are_ignored_and_remain_non_authoritative() {
    let root = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(root.path());
    let stage = endpoint.join(".tmp_pop.ed25519.pk8_stale");
    write_private_file(&stage, b"unrelated stage bytes");
    let before = fs::read(&stage).expect("stage reads");

    let context = bootstrap_context(root.path());
    assert_eq!(context.test_verifying_key_bytes().len(), 32);
    assert_eq!(fs::read(&stage).expect("stage rereads"), before);
    assert!(endpoint.join("pop.ed25519.pk8").is_file());
}

fn captured_diagnostic(value: &(impl std::fmt::Display + std::fmt::Debug)) -> String {
    format!("display={value}; debug={value:?}")
}

fn assert_canaries_are_redacted(
    result: Result<Option<crate::McpEndpointOwnerContext>, McpEndpointBootstrapError>,
    canaries: &[&str],
) {
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("fixture must refuse bootstrap"),
    };
    let rendered = captured_diagnostic(&error);
    for canary in canaries {
        assert!(
            !rendered.contains(canary),
            "bootstrap diagnostics must redact {canary:?}: {rendered:?}"
        );
    }
}

#[test]
fn sensitive_inputs_never_reach_mcp_endpoint_diagnostics_or_public_context() {
    const HOME_LABEL: &str = "CANARY-HOME-LABEL-9f4540";
    const INSTANCE_ID: &str = "CANARY-INSTANCE-ID-588d5a";
    const KEY_BYTES: &str = "CANARY-KEY-BYTES-0c1824";
    const PATH_COMPONENT: &str = "CANARY-PATH-COMPONENT-40d8ce";
    let canaries = [HOME_LABEL, INSTANCE_ID, KEY_BYTES, PATH_COMPONENT];

    let sandbox = TempDir::new().expect("test sandbox creates");
    let malformed_identity = sandbox.path().join(PATH_COMPONENT);
    fs::create_dir(&malformed_identity).expect("canary journal directory creates");
    write_enabled_config(&malformed_identity);
    write_identity(&malformed_identity, StateLayout::Primary);
    fs::write(
        malformed_identity.join("link/state.json"),
        format!(r#"{{"instance_id":"{INSTANCE_ID}","home_label":"{HOME_LABEL}"}}"#),
    )
    .expect("canary state writes");
    assert_canaries_are_redacted(
        bootstrap_mcp_endpoint_owner_identity(&malformed_identity),
        &canaries,
    );

    let corrupt_key = TempDir::new().expect("test root creates");
    write_enabled_config(corrupt_key.path());
    write_identity(corrupt_key.path(), StateLayout::Primary);
    let endpoint = prepare_endpoint_directory(corrupt_key.path());
    write_private_file(&endpoint.join("pop.ed25519.pk8"), KEY_BYTES.as_bytes());
    assert_canaries_are_redacted(
        bootstrap_mcp_endpoint_owner_identity(corrupt_key.path()),
        &canaries,
    );

    let toctou = TempDir::new().expect("test root creates");
    write_enabled_config(toctou.path());
    write_identity(toctou.path(), StateLayout::Primary);
    let endpoint = prepare_endpoint_directory(toctou.path());
    let aside = endpoint.with_extension("aside");
    let callback_endpoint = endpoint.clone();
    let callback_aside = aside.clone();
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::DirectoryBindingCheckBeforeLock,
        1,
        move || {
            fs::rename(&callback_endpoint, &callback_aside).expect("endpoint moves aside");
            fs::create_dir(&callback_endpoint).expect("replacement endpoint creates");
            set_mode(&callback_endpoint, 0o700);
        },
        || bootstrap_mcp_endpoint_owner_identity(toctou.path()),
    );
    assert!(fired);
    assert_canaries_are_redacted(result, &canaries);

    let success = TempDir::new().expect("test root creates");
    write_enabled_config(success.path());
    write_identity(success.path(), StateLayout::Primary);
    let context = bootstrap_context(success.path());
    assert_eq!(context.test_verifying_key_bytes().len(), 32);
    // The public type has no Display, Debug, or accessor implementation. The
    // compile-fail doctests on McpEndpointOwnerContext pin that caller boundary.

    struct SyntheticDiagnostic(&'static str);
    impl std::fmt::Display for SyntheticDiagnostic {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.0)
        }
    }
    impl std::fmt::Debug for SyntheticDiagnostic {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.0)
        }
    }
    let control = "synthetic-diagnostic-capture-control";
    assert!(captured_diagnostic(&SyntheticDiagnostic(control)).contains(control));
}

#[test]
fn production_source_has_no_logging_or_printing_surface() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let production_files = [
        source.join("lib.rs"),
        source.join("unix.rs"),
        source.join("test_seam.rs"),
    ];
    for path in production_files {
        let text = fs::read_to_string(&path).expect("production source reads");
        for prohibited in ["println!", "eprintln!", "log::", "tracing::"] {
            assert!(
                !text.contains(prohibited),
                "{} must not expose {prohibited}",
                path.display()
            );
        }
    }
}
