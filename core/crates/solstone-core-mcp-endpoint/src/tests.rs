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

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg, OFlag};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat};
use nix::unistd::{geteuid, mkfifo};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use tempfile::TempDir;

use crate::test_seam::{
    OwnerBootstrapPrimitive, run_with_owner_barrier, run_with_owner_barrier_and_fault,
    run_with_owner_fault, run_with_two_owner_faults,
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

    let unreadable = TempDir::new().expect("test root creates");
    fs::create_dir_all(unreadable.path().join("config/journal.json"))
        .expect("test unreadable config directory creates");
    assert!(matches!(
        bootstrap_mcp_endpoint_owner_identity(unreadable.path()),
        Err(McpEndpointBootstrapError::ConfigRead)
    ));
    assert_no_endpoint(unreadable.path());

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
    let planted_path_for_barrier = planted_path.clone();
    let planted_key_for_barrier = planted_key.clone();
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::KeyGenerate,
        1,
        move || write_private_file(&planted_path_for_barrier, &planted_key_for_barrier),
        || bootstrap_mcp_endpoint_owner_identity(planted.path()),
    );
    assert!(fired);
    assert_endpoint_error(result);
    assert_eq!(
        fs::read(&planted_path).expect("valid plant remains readable"),
        planted_key
    );
    let context = bootstrap_context(planted.path());
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

#[test]
fn acquired_lock_name_must_still_bind_before_any_key_work() {
    let root = prepared_enabled_root();
    let endpoint = prepare_endpoint_directory(root.path());
    let lock_path = endpoint.join(".create.lock");
    let aside = endpoint.join(".create.lock.aside");
    write_private_file(&lock_path, b"original lock");
    let callback_lock = lock_path.clone();
    let callback_aside = aside.clone();
    let (result, fired, later_key_checkpoint_reached) = run_with_owner_barrier_and_fault(
        OwnerBootstrapPrimitive::LockBindingAfterAcquire,
        1,
        move || {
            fs::rename(&callback_lock, &callback_aside).expect("locked inode moves aside");
            write_private_file(&callback_lock, b"replacement lock");
        },
        OwnerBootstrapPrimitive::KeyPrecheckStat,
        1,
        nix::libc::EIO,
        || bootstrap_mcp_endpoint_owner_identity(root.path()),
    );
    assert!(fired, "post-acquire lock-binding checkpoint fires");
    assert!(
        !later_key_checkpoint_reached,
        "lock-name replacement must fail before key precheck"
    );
    assert_endpoint_error(result);
    assert_eq!(
        fs::read(&aside).expect("original lock remains"),
        b"original lock"
    );
    assert_eq!(
        fs::read(&lock_path).expect("replacement lock remains"),
        b"replacement lock"
    );
    assert!(!endpoint.join("pop.ed25519.pk8").exists());
}

#[test]
fn lock_guard_remains_held_until_after_the_last_content_read() {
    let root = prepared_enabled_root();
    let _context = bootstrap_context(root.path());
    let lock_path = endpoint_path(root.path()).join(".create.lock");
    let callback_lock = lock_path.clone();
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::FinalKeyRead,
        1,
        move || {
            let probe = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&callback_lock)
                .expect("canonical lock opens for nonblocking probe");
            match Flock::lock(probe, FlockArg::LockExclusiveNonblock) {
                Ok(_unexpected_lock) => panic!("bootstrap lock was released before final read"),
                Err((_probe, errno)) => assert_eq!(
                    errno,
                    Errno::EAGAIN,
                    "held lock refuses with EAGAIN/EWOULDBLOCK"
                ),
            }
        },
        || bootstrap_mcp_endpoint_owner_identity(root.path()),
    );
    assert!(fired);
    assert!(matches!(result, Ok(Some(_))));

    let probe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("canonical lock reopens after bootstrap");
    let acquired = Flock::lock(probe, FlockArg::LockExclusiveNonblock)
        .expect("lock is available after bootstrap returns");
    drop(acquired);
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
        OwnerBootstrapPrimitive::FinalKeyFsync,
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
    for primitive in [
        OwnerBootstrapPrimitive::FinalKeyContentCompare,
        OwnerBootstrapPrimitive::FinalKeyFsync,
        OwnerBootstrapPrimitive::FinalDirectoryFsync,
        OwnerBootstrapPrimitive::FinalKeyRead,
    ] {
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
            primitive,
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
        assert!(fired, "{primitive:?} overwrite barrier fires");
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
        let recovered = bootstrap_context(root.path());
        assert_eq!(
            recovered.test_verifying_key_bytes(),
            verifying_key(&replacement)
        );
    }

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
fn final_authority_checks_reject_metadata_and_lock_name_changes() {
    for target in ["lock", "directory", "key"] {
        let root = prepared_enabled_root();
        let _context = bootstrap_context(root.path());
        let endpoint = endpoint_path(root.path());
        let changed = match target {
            "lock" => endpoint.join(".create.lock"),
            "directory" => endpoint,
            "key" => endpoint.join("pop.ed25519.pk8"),
            _ => unreachable!("known metadata target"),
        };
        let changed_mode = if target == "directory" { 0o755 } else { 0o644 };
        let (result, fired) = run_with_owner_barrier(
            OwnerBootstrapPrimitive::FinalDirectoryFsync,
            1,
            move || set_mode(&changed, changed_mode),
            || bootstrap_mcp_endpoint_owner_identity(root.path()),
        );
        assert!(fired, "{target} metadata barrier fires");
        assert_endpoint_error(result);
    }

    let root = prepared_enabled_root();
    let _context = bootstrap_context(root.path());
    let endpoint = endpoint_path(root.path());
    let lock_path = endpoint.join(".create.lock");
    let aside = endpoint.join(".create.lock.final-aside");
    let callback_lock = lock_path.clone();
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::FinalLockBinding,
        1,
        move || {
            fs::rename(&callback_lock, &aside).expect("locked inode moves aside");
            write_private_file(&callback_lock, b"replacement lock");
        },
        || bootstrap_mcp_endpoint_owner_identity(root.path()),
    );
    assert!(fired, "final lock-name binding checkpoint fires");
    assert_endpoint_error(result);
    assert_eq!(
        fs::read(&lock_path).expect("replacement lock remains"),
        b"replacement lock"
    );
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
    OwnerBootstrapPrimitive::LockBindingAfterAcquire,
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
    OwnerBootstrapPrimitive::FinalKeyInitialNameBinding,
    OwnerBootstrapPrimitive::FinalKeyContentCompare,
    OwnerBootstrapPrimitive::FinalKeyFsync,
    OwnerBootstrapPrimitive::FinalDirectoryFsync,
    OwnerBootstrapPrimitive::FinalLockBinding,
    OwnerBootstrapPrimitive::FinalDirectoryRestat,
    OwnerBootstrapPrimitive::FinalKeyAuthorityRestat,
    OwnerBootstrapPrimitive::FinalKeyNameBinding,
    OwnerBootstrapPrimitive::FinalRootRevalidate,
    OwnerBootstrapPrimitive::DirectoryBindingCheckBeforeSuccess,
    OwnerBootstrapPrimitive::FinalKeySeek,
    OwnerBootstrapPrimitive::FinalKeyRead,
    OwnerBootstrapPrimitive::FinalKeyDecode,
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
    let mut exactly_512 = valid_pkcs8();
    exactly_512.resize(512, 0);
    let exact = root_with_preexisting_key(&exactly_512);
    assert_endpoint_error(bootstrap_mcp_endpoint_owner_identity(exact.path()));
    let (result, consumed) = run_with_owner_fault(
        OwnerBootstrapPrimitive::KeyDecode,
        1,
        nix::libc::EIO,
        || bootstrap_mcp_endpoint_owner_identity(exact.path()),
    );
    assert_endpoint_error(result);
    assert!(consumed, "exact-limit input reaches the decode checkpoint");

    let large = root_with_preexisting_key(&vec![b'x'; 513]);
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

    let grown = root_with_preexisting_key(&valid_pkcs8());
    let grown_path = endpoint_path(grown.path()).join("pop.ed25519.pk8");
    let (result, fired) = run_with_owner_barrier(
        OwnerBootstrapPrimitive::KeyRead,
        1,
        move || {
            OpenOptions::new()
                .write(true)
                .open(&grown_path)
                .expect("race-growth key opens")
                .set_len(1024 * 1024 * 1024)
                .expect("race-growth key becomes sparse");
        },
        || bootstrap_mcp_endpoint_owner_identity(grown.path()),
    );
    assert!(fired, "bounded read race checkpoint fires");
    assert_endpoint_error(result);
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

    let success_sandbox = TempDir::new().expect("test root creates");
    let success = success_sandbox.path().join(PATH_COMPONENT);
    fs::create_dir(&success).expect("canary success journal directory creates");
    write_enabled_config(&success);
    let success_spki = write_identity(&success, StateLayout::Primary);
    let success_instance_id =
        solstone_core_sol_link::ca::jid_from_spki(&success_spki).expect("test JID derives");
    fs::write(
        success.join("link/state.json"),
        format!(r#"{{"instance_id":"{success_instance_id}","home_label":"{HOME_LABEL}"}}"#),
    )
    .expect("canary success state writes");
    let context = bootstrap_context(&success);
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

fn collect_rust_source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("source directory reads") {
        let path = entry.expect("source entry reads").path();
        if path.is_dir() {
            collect_rust_source_files(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn rust_source_files(source: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_source_files(source, &mut files);
    files.sort();
    files
}

fn push_unique(violations: &mut Vec<&'static str>, violation: &'static str) {
    if !violations.contains(&violation) {
        violations.push(violation);
    }
}

fn normalized_ident(ident: &proc_macro2::Ident) -> String {
    let raw = ident.to_string();
    raw.strip_prefix("r#").unwrap_or(&raw).to_owned()
}

fn collect_macro_token_idents(stream: proc_macro2::TokenStream, idents: &mut Vec<String>) {
    for token in stream {
        match token {
            proc_macro2::TokenTree::Group(group) => {
                collect_macro_token_idents(group.stream(), idents);
            }
            proc_macro2::TokenTree::Ident(ident) => idents.push(normalized_ident(&ident)),
            proc_macro2::TokenTree::Punct(_) | proc_macro2::TokenTree::Literal(_) => {}
        }
    }
}

fn macro_tokens_have_invocation(stream: proc_macro2::TokenStream, prohibited: &[&str]) -> bool {
    let tokens: Vec<_> = stream.into_iter().collect();
    for (index, token) in tokens.iter().enumerate() {
        if let proc_macro2::TokenTree::Group(group) = token
            && macro_tokens_have_invocation(group.stream(), prohibited)
        {
            return true;
        }
        let proc_macro2::TokenTree::Ident(ident) = token else {
            continue;
        };
        let Some(proc_macro2::TokenTree::Punct(bang)) = tokens.get(index + 1) else {
            continue;
        };
        if bang.as_char() == '!'
            && prohibited
                .iter()
                .any(|name| normalized_ident(ident) == *name)
        {
            return true;
        }
    }
    false
}

fn macro_tokens_have_path_root(stream: proc_macro2::TokenStream, prohibited: &[&str]) -> bool {
    let tokens: Vec<_> = stream.into_iter().collect();
    for (index, token) in tokens.iter().enumerate() {
        if let proc_macro2::TokenTree::Group(group) = token
            && macro_tokens_have_path_root(group.stream(), prohibited)
        {
            return true;
        }
        let proc_macro2::TokenTree::Ident(ident) = token else {
            continue;
        };
        let Some(proc_macro2::TokenTree::Punct(first_colon)) = tokens.get(index + 1) else {
            continue;
        };
        let Some(proc_macro2::TokenTree::Punct(second_colon)) = tokens.get(index + 2) else {
            continue;
        };
        if first_colon.as_char() == ':'
            && second_colon.as_char() == ':'
            && prohibited
                .iter()
                .any(|name| normalized_ident(ident) == *name)
        {
            return true;
        }
    }
    false
}

fn collect_use_idents(tree: &syn::UseTree, idents: &mut Vec<String>) {
    match tree {
        syn::UseTree::Path(path) => {
            idents.push(normalized_ident(&path.ident));
            collect_use_idents(&path.tree, idents);
        }
        syn::UseTree::Name(name) => idents.push(normalized_ident(&name.ident)),
        syn::UseTree::Rename(rename) => {
            idents.push(normalized_ident(&rename.ident));
            idents.push(normalized_ident(&rename.rename));
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_use_idents(item, idents);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

fn collect_use_roots(tree: &syn::UseTree, roots: &mut Vec<String>) {
    match tree {
        syn::UseTree::Path(path) => roots.push(normalized_ident(&path.ident)),
        syn::UseTree::Name(name) => roots.push(normalized_ident(&name.ident)),
        syn::UseTree::Rename(rename) => roots.push(normalized_ident(&rename.ident)),
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_use_roots(item, roots);
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

#[derive(Clone)]
struct UseBinding {
    path: Vec<String>,
    local: String,
}

fn collect_use_bindings(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    bindings: &mut Vec<UseBinding>,
    glob_paths: &mut Vec<Vec<String>>,
) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(normalized_ident(&path.ident));
            collect_use_bindings(&path.tree, prefix, bindings, glob_paths);
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            let local = normalized_ident(&name.ident);
            let mut path = prefix.clone();
            path.push(local.clone());
            bindings.push(UseBinding { path, local });
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix.clone();
            path.push(normalized_ident(&rename.ident));
            bindings.push(UseBinding {
                path,
                local: normalized_ident(&rename.rename),
            });
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_use_bindings(item, prefix, bindings, glob_paths);
            }
        }
        syn::UseTree::Glob(_) => glob_paths.push(prefix.clone()),
    }
}

#[derive(Default)]
struct FilesystemModuleAliasCollector {
    bindings: Vec<UseBinding>,
    glob_paths: Vec<Vec<String>>,
}

impl<'ast> syn::visit::Visit<'ast> for FilesystemModuleAliasCollector {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        collect_use_bindings(
            &node.tree,
            &mut Vec::new(),
            &mut self.bindings,
            &mut self.glob_paths,
        );
        syn::visit::visit_item_use(self, node);
    }
}

impl FilesystemModuleAliasCollector {
    fn resolve(self) -> FilesystemAliases {
        let mut module_aliases = std::collections::BTreeSet::new();
        let mut write_aliases = std::collections::BTreeSet::new();
        let mut io_module_aliases = std::collections::BTreeSet::new();
        let mut io_write_trait_aliases = std::collections::BTreeSet::new();
        loop {
            let before = (
                module_aliases.len(),
                write_aliases.len(),
                io_module_aliases.len(),
                io_write_trait_aliases.len(),
            );
            for binding in &self.bindings {
                let is_module_alias = binding.path == ["std", "fs"]
                    || binding.path == ["std", "fs", "self"]
                    || (binding.path.len() == 1 && module_aliases.contains(&binding.path[0]))
                    || (binding.path.len() == 2
                        && module_aliases.contains(&binding.path[0])
                        && binding.path[1] == "self");
                if is_module_alias {
                    module_aliases.insert(binding.local.clone());
                }
                let is_io_module_alias = binding.path == ["std", "io"]
                    || binding.path == ["std", "io", "self"]
                    || (binding.path.len() == 1 && io_module_aliases.contains(&binding.path[0]))
                    || (binding.path.len() == 2
                        && io_module_aliases.contains(&binding.path[0])
                        && binding.path[1] == "self");
                if is_io_module_alias {
                    io_module_aliases.insert(binding.local.clone());
                }
                if binding.path.last().is_some_and(|last| last == "write")
                    && ((binding.path.len() == 3 && binding.path[..2] == ["std", "fs"])
                        || (binding.path.len() == 2 && module_aliases.contains(&binding.path[0])))
                {
                    write_aliases.insert(binding.local.clone());
                }
                if binding.path.len() == 1 && write_aliases.contains(&binding.path[0]) {
                    write_aliases.insert(binding.local.clone());
                }
                if binding.path.last().is_some_and(|last| last == "Write")
                    && ((binding.path.len() == 3 && binding.path[..2] == ["std", "io"])
                        || (binding.path.len() == 2
                            && io_module_aliases.contains(&binding.path[0])))
                {
                    io_write_trait_aliases.insert(binding.local.clone());
                }
                if binding.path.len() == 1 && io_write_trait_aliases.contains(&binding.path[0]) {
                    io_write_trait_aliases.insert(binding.local.clone());
                }
            }
            if (
                module_aliases.len(),
                write_aliases.len(),
                io_module_aliases.len(),
                io_write_trait_aliases.len(),
            ) == before
            {
                break;
            }
        }
        let has_forbidden_glob_import = self.glob_paths.iter().any(|path| {
            path == &["std", "fs"]
                || path == &["std", "io"]
                || (path.len() == 1 && module_aliases.contains(&path[0]))
                || (path.len() == 1 && io_module_aliases.contains(&path[0]))
        });
        FilesystemAliases {
            module_aliases,
            write_aliases,
            io_module_aliases,
            io_write_trait_aliases,
            has_forbidden_glob_import,
        }
    }
}

struct FilesystemAliases {
    module_aliases: std::collections::BTreeSet<String>,
    write_aliases: std::collections::BTreeSet<String>,
    io_module_aliases: std::collections::BTreeSet<String>,
    io_write_trait_aliases: std::collections::BTreeSet<String>,
    has_forbidden_glob_import: bool,
}

struct PatternBindingVisitor<'target> {
    target: &'target str,
    found: bool,
}

impl<'ast> syn::visit::Visit<'ast> for PatternBindingVisitor<'_> {
    fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
        if normalized_ident(&node.ident) == self.target {
            self.found = true;
        }
        syn::visit::visit_pat_ident(self, node);
    }
}

fn pattern_binds_ident(pattern: &syn::Pat, target: &str) -> bool {
    let mut visitor = PatternBindingVisitor {
        target,
        found: false,
    };
    syn::visit::Visit::visit_pat(&mut visitor, pattern);
    visitor.found
}

struct DiagnosticSyntaxVisitor {
    allow_fixture_file_writes: bool,
    allow_account_protocol_writes: bool,
    allowed_protocol_writer: Option<&'static str>,
    in_account_protocol_write: bool,
    account_protocol_writer_shadowed: bool,
    in_test_scope: bool,
    filesystem_module_aliases: std::collections::BTreeSet<String>,
    filesystem_write_aliases: std::collections::BTreeSet<String>,
    io_module_aliases: std::collections::BTreeSet<String>,
    io_write_trait_aliases: std::collections::BTreeSet<String>,
    violations: Vec<&'static str>,
}

impl DiagnosticSyntaxVisitor {
    fn new(
        allow_fixture_file_writes: bool,
        allow_account_protocol_writes: bool,
        allowed_protocol_writer: Option<&'static str>,
        aliases: FilesystemAliases,
    ) -> Self {
        Self {
            allow_fixture_file_writes,
            allow_account_protocol_writes,
            allowed_protocol_writer,
            in_account_protocol_write: false,
            account_protocol_writer_shadowed: false,
            in_test_scope: false,
            filesystem_module_aliases: aliases.module_aliases,
            filesystem_write_aliases: aliases.write_aliases,
            io_module_aliases: aliases.io_module_aliases,
            io_write_trait_aliases: aliases.io_write_trait_aliases,
            violations: aliases
                .has_forbidden_glob_import
                .then_some("filesystem or I/O glob import")
                .into_iter()
                .collect(),
        }
    }

    fn writes_allowed(&self) -> bool {
        self.allow_fixture_file_writes || self.in_test_scope
    }

    fn account_protocol_write_allowed(&self, node: &syn::ExprMethodCall) -> bool {
        self.allow_account_protocol_writes
            && self.in_account_protocol_write
            && !self.account_protocol_writer_shadowed
            && matches!(
                normalized_ident(&node.method).as_str(),
                "write" | "write_all"
            )
            && matches!(node.receiver.as_ref(), syn::Expr::Path(path) if path.path.is_ident("writer"))
    }

    fn reject_account_protocol_writer_shadow(&mut self, pattern: &syn::Pat) {
        if self.in_account_protocol_write && pattern_binds_ident(pattern, "writer") {
            self.account_protocol_writer_shadowed = true;
            push_unique(&mut self.violations, "account protocol writer is shadowed");
        }
    }
}

impl<'ast> syn::visit::Visit<'ast> for DiagnosticSyntaxVisitor {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let prior = self.in_test_scope;
        self.in_test_scope |= is_cfg_test(&node.attrs);
        syn::visit::visit_item_mod(self, node);
        self.in_test_scope = prior;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let prior = self.in_test_scope;
        let prior_account_protocol_write = self.in_account_protocol_write;
        let prior_account_protocol_writer_shadowed = self.account_protocol_writer_shadowed;
        self.in_test_scope |= is_cfg_test(&node.attrs);
        self.in_account_protocol_write = self.allow_account_protocol_writes
            && self
                .allowed_protocol_writer
                .is_some_and(|name| normalized_ident(&node.sig.ident) == name);
        self.account_protocol_writer_shadowed = false;
        syn::visit::visit_item_fn(self, node);
        self.in_test_scope = prior;
        self.in_account_protocol_write = prior_account_protocol_write;
        self.account_protocol_writer_shadowed = prior_account_protocol_writer_shadowed;
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let prior = self.in_test_scope;
        self.in_test_scope |= is_cfg_test(&node.attrs);
        syn::visit::visit_item_impl(self, node);
        self.in_test_scope = prior;
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let name = node
            .path
            .segments
            .last()
            .map(|segment| normalized_ident(&segment.ident));
        let mut token_idents = Vec::new();
        collect_macro_token_idents(node.tokens.clone(), &mut token_idents);
        if macro_tokens_have_invocation(
            node.tokens.clone(),
            &[
                "print", "println", "eprint", "eprintln", "dbg", "write", "writeln",
            ],
        ) || macro_tokens_have_path_root(node.tokens.clone(), &["log", "tracing"])
            || token_idents
                .iter()
                .any(|ident| matches!(ident.as_str(), "stdout" | "stderr"))
        {
            push_unique(
                &mut self.violations,
                "diagnostic egress inside macro tokens",
            );
        }
        if !self.writes_allowed()
            && token_idents
                .iter()
                .any(|ident| matches!(ident.as_str(), "write_all" | "write_fmt"))
        {
            push_unique(&mut self.violations, "write egress inside macro tokens");
        }
        match name.as_deref() {
            Some("print") => push_unique(&mut self.violations, "print macro"),
            Some("println") => push_unique(&mut self.violations, "println macro"),
            Some("eprint") => push_unique(&mut self.violations, "eprint macro"),
            Some("eprintln") => push_unique(&mut self.violations, "eprintln macro"),
            Some("dbg") => push_unique(&mut self.violations, "dbg macro"),
            Some("writeln") => push_unique(&mut self.violations, "writeln macro"),
            Some("write") => push_unique(&mut self.violations, "write macro"),
            _ => {}
        }
        syn::visit::visit_macro(self, node);
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        if node.segments.len() > 1 {
            match node
                .segments
                .first()
                .map(|segment| normalized_ident(&segment.ident))
            {
                Some(root) if root == "log" => push_unique(&mut self.violations, "log path"),
                Some(root) if root == "tracing" => {
                    push_unique(&mut self.violations, "tracing path")
                }
                _ => {}
            }
        }
        let path_idents: Vec<_> = node
            .segments
            .iter()
            .map(|segment| normalized_ident(&segment.ident))
            .collect();
        if path_idents.windows(3).any(|window| {
            window[0] == "std"
                && window[1] == "io"
                && matches!(window[2].as_str(), "stdout" | "stderr")
        }) || path_idents.windows(2).any(|window| {
            (window[0] == "io" || self.io_module_aliases.contains(&window[0]))
                && matches!(window[1].as_str(), "stdout" | "stderr")
        }) {
            push_unique(&mut self.violations, "stdio output path");
        }
        if !self.writes_allowed()
            && (path_idents
                .windows(3)
                .any(|window| window == ["std", "io", "Write"])
                || path_idents
                    .windows(2)
                    .any(|window| window == ["io", "Write"])
                || path_idents.windows(2).any(|window| {
                    window[1] == "Write" && self.io_module_aliases.contains(&window[0])
                })
                || (path_idents.len() == 1
                    && self.io_write_trait_aliases.contains(&path_idents[0]))
                || (path_idents.len() > 1
                    && self.io_write_trait_aliases.contains(&path_idents[0])
                    && path_idents[1..].iter().any(|ident| {
                        matches!(ident.as_str(), "write" | "write_all" | "write_fmt")
                    }))
                || path_idents
                    .iter()
                    .any(|ident| matches!(ident.as_str(), "write_all" | "write_fmt")))
        {
            push_unique(&mut self.violations, "I/O Write trait");
        }
        if !self.writes_allowed()
            && (path_idents
                .windows(3)
                .any(|window| window == ["std", "fs", "write"])
                || path_idents
                    .windows(2)
                    .any(|window| window == ["fs", "write"])
                || path_idents.windows(2).any(|window| {
                    window[1] == "write" && self.filesystem_module_aliases.contains(&window[0])
                })
                || (path_idents.len() == 1
                    && self.filesystem_write_aliases.contains(&path_idents[0])))
        {
            push_unique(&mut self.violations, "filesystem write function");
        }
        syn::visit::visit_path(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if !self.writes_allowed()
            && !self.account_protocol_write_allowed(node)
            && matches!(
                normalized_ident(&node.method).as_str(),
                "write" | "write_all" | "write_fmt"
            )
        {
            push_unique(&mut self.violations, "direct write method");
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        self.reject_account_protocol_writer_shadow(&node.pat);
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        for input in &node.inputs {
            self.reject_account_protocol_writer_shadow(input);
        }
        syn::visit::visit_expr_closure(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.reject_account_protocol_writer_shadow(&node.pat);
        syn::visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_let(&mut self, node: &'ast syn::ExprLet) {
        self.reject_account_protocol_writer_shadow(&node.pat);
        syn::visit::visit_expr_let(self, node);
    }

    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        self.reject_account_protocol_writer_shadow(&node.pat);
        syn::visit::visit_arm(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        let mut roots = Vec::new();
        collect_use_roots(&node.tree, &mut roots);
        if roots.iter().any(|root| root == "log") {
            push_unique(&mut self.violations, "log import");
        }
        if roots.iter().any(|root| root == "tracing") {
            push_unique(&mut self.violations, "tracing import");
        }
        let mut idents = Vec::new();
        collect_use_idents(&node.tree, &mut idents);
        if idents.iter().any(|ident| ident == "stdout") {
            push_unique(&mut self.violations, "stdout import");
        }
        if idents.iter().any(|ident| ident == "stderr") {
            push_unique(&mut self.violations, "stderr import");
        }
        if idents.iter().any(|ident| {
            matches!(
                ident.as_str(),
                "print" | "println" | "eprint" | "eprintln" | "dbg" | "write" | "writeln"
            )
        }) {
            push_unique(&mut self.violations, "diagnostic macro import");
        }
        if !self.writes_allowed()
            && ["std", "io", "Write"]
                .iter()
                .all(|required| idents.iter().any(|ident| ident == required))
        {
            push_unique(&mut self.violations, "I/O Write import");
        }
        if !self.writes_allowed()
            && ["std", "fs", "write"]
                .iter()
                .all(|required| idents.iter().any(|ident| ident == required))
        {
            push_unique(&mut self.violations, "filesystem write import");
        }
        syn::visit::visit_item_use(self, node);
    }

    fn visit_item_extern_crate(&mut self, node: &'ast syn::ItemExternCrate) {
        match normalized_ident(&node.ident).as_str() {
            "log" => push_unique(&mut self.violations, "log extern crate"),
            "tracing" => push_unique(&mut self.violations, "tracing extern crate"),
            _ => {}
        }
        syn::visit::visit_item_extern_crate(self, node);
    }
}

struct TypeNameVisitor<'a> {
    names: &'a std::collections::BTreeSet<String>,
    found: bool,
}

impl<'ast> syn::visit::Visit<'ast> for TypeNameVisitor<'_> {
    fn visit_ident(&mut self, node: &'ast syn::Ident) {
        if self.names.contains(&normalized_ident(node)) {
            self.found = true;
        }
    }
}

fn type_mentions_names(ty: &syn::Type, names: &std::collections::BTreeSet<String>) -> bool {
    let mut visitor = TypeNameVisitor {
        names,
        found: false,
    };
    syn::visit::Visit::visit_type(&mut visitor, ty);
    visitor.found
}

#[derive(Default)]
struct AuthoritySyntaxVisitor {
    is_privacy_unit: bool,
    wire_aliases: std::collections::BTreeSet<String>,
    violations: Vec<&'static str>,
    saw_parser: bool,
    saw_wire: bool,
    saw_error: bool,
}

impl AuthoritySyntaxVisitor {
    fn new(is_privacy_unit: bool, wire_aliases: std::collections::BTreeSet<String>) -> Self {
        Self {
            is_privacy_unit,
            wire_aliases,
            ..Self::default()
        }
    }

    fn wire_names(&self) -> std::collections::BTreeSet<String> {
        let mut names = self.wire_aliases.clone();
        names.insert("McpAccountResponseWire".to_owned());
        names
    }

    fn type_mentions_wire(&self, ty: &syn::Type) -> bool {
        type_mentions_names(ty, &self.wire_names())
    }
}

impl<'ast> syn::visit::Visit<'ast> for AuthoritySyntaxVisitor {
    fn visit_ident(&mut self, node: &'ast syn::Ident) {
        if !self.is_privacy_unit {
            match normalized_ident(node).as_str() {
                "parse_account_registration_response" => {
                    push_unique(&mut self.violations, "parser outside privacy unit")
                }
                "McpAccountResponseWire" => {
                    push_unique(&mut self.violations, "wire outside privacy unit")
                }
                "McpAccountResponseWireError" => {
                    push_unique(&mut self.violations, "wire error outside privacy unit")
                }
                _ => {}
            }
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if normalized_ident(&node.sig.ident) == "parse_account_registration_response" {
            self.saw_parser = true;
            if !matches!(node.vis, syn::Visibility::Inherited) {
                push_unique(&mut self.violations, "parser visibility");
            }
        }
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        let mut idents = Vec::new();
        collect_use_idents(&node.tree, &mut idents);
        if idents.iter().any(|ident| {
            matches!(
                ident.as_str(),
                "parse_account_registration_response"
                    | "McpAccountResponseWire"
                    | "McpAccountResponseWireError"
            )
        }) {
            push_unique(&mut self.violations, "response authority item alias");
        }
        syn::visit::visit_item_use(self, node);
    }

    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        let mut response_use = ResponseUseVisitor::default();
        syn::visit::Visit::visit_type(&mut response_use, &node.ty);
        syn::visit::Visit::visit_expr(&mut response_use, &node.expr);
        if response_use.direct_response_use {
            push_unique(&mut self.violations, "response authority const alias");
        }
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        let mut response_use = ResponseUseVisitor::default();
        syn::visit::Visit::visit_type(&mut response_use, &node.ty);
        syn::visit::Visit::visit_expr(&mut response_use, &node.expr);
        if response_use.direct_response_use {
            push_unique(&mut self.violations, "response authority static alias");
        }
        syn::visit::visit_item_static(self, node);
    }

    fn visit_impl_item_const(&mut self, node: &'ast syn::ImplItemConst) {
        let mut response_use = ResponseUseVisitor::default();
        syn::visit::Visit::visit_type(&mut response_use, &node.ty);
        syn::visit::Visit::visit_expr(&mut response_use, &node.expr);
        if response_use.direct_response_use {
            push_unique(
                &mut self.violations,
                "response authority associated const alias",
            );
        }
        syn::visit::visit_impl_item_const(self, node);
    }

    fn visit_trait_item_const(&mut self, node: &'ast syn::TraitItemConst) {
        let mut response_use = ResponseUseVisitor::default();
        syn::visit::Visit::visit_type(&mut response_use, &node.ty);
        if let Some((_, expression)) = &node.default {
            syn::visit::Visit::visit_expr(&mut response_use, expression);
        }
        if response_use.direct_response_use {
            push_unique(&mut self.violations, "response authority trait const alias");
        }
        syn::visit::visit_trait_item_const(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if normalized_ident(&node.ident) == "McpAccountResponseWire" {
            self.saw_wire = true;
            if !matches!(node.vis, syn::Visibility::Inherited) {
                push_unique(&mut self.violations, "wire visibility");
            }
            if node
                .fields
                .iter()
                .any(|field| !matches!(field.vis, syn::Visibility::Inherited))
            {
                push_unique(&mut self.violations, "wire field visibility");
            }
            if !node.attrs.is_empty() {
                push_unique(&mut self.violations, "wire has an attribute");
            }
        } else {
            if node
                .fields
                .iter()
                .any(|field| self.type_mentions_wire(&field.ty))
            {
                push_unique(&mut self.violations, "wire stored in another struct");
            }
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if normalized_ident(&node.ident) == "McpAccountResponseWireError" {
            self.saw_error = true;
            if !matches!(node.vis, syn::Visibility::Inherited) {
                push_unique(&mut self.violations, "wire error visibility");
            }
        } else if node
            .variants
            .iter()
            .flat_map(|variant| variant.fields.iter())
            .any(|field| self.type_mentions_wire(&field.ty))
        {
            push_unique(&mut self.violations, "wire stored in another enum");
        }
        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_union(&mut self, node: &'ast syn::ItemUnion) {
        if node
            .fields
            .named
            .iter()
            .any(|field| self.type_mentions_wire(&field.ty))
        {
            push_unique(&mut self.violations, "wire stored in a union");
        }
        syn::visit::visit_item_union(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if self.type_mentions_wire(&node.self_ty) {
            if node.trait_.is_some() {
                push_unique(&mut self.violations, "wire implements a trait");
            } else if node.items.iter().any(|item| {
                matches!(item, syn::ImplItem::Fn(function) if !matches!(function.vis, syn::Visibility::Inherited))
            }) {
                push_unique(&mut self.violations, "wire accessor visibility");
            }
        }
        syn::visit::visit_item_impl(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        if self.type_mentions_wire(&node.ty) {
            push_unique(&mut self.violations, "wire type alias");
        }
        syn::visit::visit_item_type(self, node);
    }

    fn visit_impl_item_type(&mut self, node: &'ast syn::ImplItemType) {
        if self.type_mentions_wire(&node.ty) {
            push_unique(&mut self.violations, "wire associated type alias");
        }
        syn::visit::visit_impl_item_type(self, node);
    }

    fn visit_trait_item_type(&mut self, node: &'ast syn::TraitItemType) {
        if let Some((_, ty)) = &node.default
            && self.type_mentions_wire(ty)
        {
            push_unique(&mut self.violations, "wire trait associated type alias");
        }
        syn::visit::visit_trait_item_type(self, node);
    }

    fn visit_item_foreign_mod(&mut self, node: &'ast syn::ItemForeignMod) {
        for item in &node.items {
            let mut response_use = ResponseUseVisitor::default();
            match item {
                syn::ForeignItem::Fn(function) => {
                    syn::visit::Visit::visit_signature(&mut response_use, &function.sig)
                }
                syn::ForeignItem::Static(item) => {
                    syn::visit::Visit::visit_type(&mut response_use, &item.ty)
                }
                syn::ForeignItem::Type(_)
                | syn::ForeignItem::Macro(_)
                | syn::ForeignItem::Verbatim(_) => {}
                _ => {}
            }
            if response_use.direct_response_use {
                push_unique(&mut self.violations, "response authority foreign module");
            }
        }
        syn::visit::visit_item_foreign_mod(self, node);
    }

    fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
        let mut idents = Vec::new();
        collect_macro_token_idents(node.mac.tokens.clone(), &mut idents);
        if idents.iter().any(|ident| {
            matches!(
                ident.as_str(),
                "parse_account_registration_response"
                    | "McpAccountResponseWire"
                    | "McpAccountResponseWireError"
            )
        }) {
            push_unique(
                &mut self.violations,
                "response authority inside an item macro",
            );
        }
        syn::visit::visit_item_macro(self, node);
    }
}

#[derive(Default)]
struct ResponseUseVisitor {
    direct_response_use: bool,
    calls: Vec<String>,
}

impl<'ast> syn::visit::Visit<'ast> for ResponseUseVisitor {
    fn visit_ident(&mut self, node: &'ast syn::Ident) {
        if matches!(
            normalized_ident(node).as_str(),
            "parse_account_registration_response" | "McpAccountResponseWire"
        ) {
            self.direct_response_use = true;
        }
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = node.func.as_ref()
            && let Some(segment) = path.path.segments.last()
        {
            self.calls.push(normalized_ident(&segment.ident));
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.calls.push(normalized_ident(&node.method));
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if let Some(segment) = node.path.segments.last() {
            self.calls.push(normalized_ident(&segment.ident));
        }
        syn::visit::visit_expr_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let mut idents = Vec::new();
        collect_macro_token_idents(node.tokens.clone(), &mut idents);
        if idents.iter().any(|ident| {
            matches!(
                ident.as_str(),
                "parse_account_registration_response" | "McpAccountResponseWire"
            )
        }) {
            self.direct_response_use = true;
        }
        self.calls.extend(idents);
        syn::visit::visit_macro(self, node);
    }
}

struct ResponseFunction {
    name: String,
    visible: bool,
    direct_response_use: bool,
    is_allowed_direct_response_consumer: bool,
    calls: Vec<String>,
}

fn is_cfg_test(attributes: &[syn::Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && match &attribute.meta {
                syn::Meta::List(list) => list.tokens.to_string().trim() == "test",
                _ => false,
            }
    })
}

fn inspect_response_function(
    signature: &syn::Signature,
    visible: bool,
    is_allowed_direct_response_consumer: bool,
    block: &syn::Block,
    functions: &mut Vec<ResponseFunction>,
) {
    let mut visitor = ResponseUseVisitor::default();
    syn::visit::Visit::visit_signature(&mut visitor, signature);
    syn::visit::Visit::visit_block(&mut visitor, block);
    functions.push(ResponseFunction {
        name: normalized_ident(&signature.ident),
        visible,
        direct_response_use: visitor.direct_response_use,
        is_allowed_direct_response_consumer,
        calls: visitor.calls,
    });
}

fn collect_response_functions(
    items: &[syn::Item],
    under_test: bool,
    at_root: bool,
    local_traits: &std::collections::BTreeSet<String>,
    visible_traits: &std::collections::BTreeSet<String>,
    functions: &mut Vec<ResponseFunction>,
) {
    for item in items {
        let item_under_test = under_test
            || match item {
                syn::Item::Fn(item) => is_cfg_test(&item.attrs),
                syn::Item::Impl(item) => is_cfg_test(&item.attrs),
                syn::Item::Mod(item) => is_cfg_test(&item.attrs),
                _ => false,
            };
        if item_under_test {
            continue;
        }
        match item {
            syn::Item::Fn(function) => {
                let is_private = matches!(function.vis, syn::Visibility::Inherited);
                let is_allowed_direct_response_consumer = at_root
                    && is_private
                    && matches!(
                        normalized_ident(&function.sig.ident).as_str(),
                        "parse_account_registration_response"
                            | "validate_account_registration"
                            | "request_account_registration"
                            | "run_fixed_account_attempt"
                            | "establish_mcp_bridge_carrier"
                    );
                inspect_response_function(
                    &function.sig,
                    !is_private,
                    is_allowed_direct_response_consumer,
                    &function.block,
                    functions,
                );
            }
            syn::Item::Impl(implementation) => {
                let trait_is_visible =
                    implementation.trait_.as_ref().is_some_and(|(_, path, _)| {
                        path.segments.len() > 1
                            || path.segments.last().is_some_and(|segment| {
                                let name = normalized_ident(&segment.ident);
                                !local_traits.contains(&name) || visible_traits.contains(&name)
                            })
                    });
                for item in &implementation.items {
                    if let syn::ImplItem::Fn(function) = item
                        && !is_cfg_test(&function.attrs)
                    {
                        inspect_response_function(
                            &function.sig,
                            trait_is_visible || !matches!(function.vis, syn::Visibility::Inherited),
                            false,
                            &function.block,
                            functions,
                        );
                    }
                }
            }
            syn::Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    collect_response_functions(
                        items,
                        false,
                        false,
                        local_traits,
                        visible_traits,
                        functions,
                    );
                }
            }
            syn::Item::Trait(trait_item) => {
                let trait_visible = !matches!(trait_item.vis, syn::Visibility::Inherited);
                for item in &trait_item.items {
                    if let syn::TraitItem::Fn(function) = item
                        && let Some(block) = &function.default
                    {
                        inspect_response_function(
                            &function.sig,
                            trait_visible,
                            false,
                            block,
                            functions,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_trait_visibility(
    items: &[syn::Item],
    under_test: bool,
    local_traits: &mut std::collections::BTreeSet<String>,
    visible_traits: &mut std::collections::BTreeSet<String>,
) {
    for item in items {
        let item_under_test = under_test
            || match item {
                syn::Item::Mod(item) => is_cfg_test(&item.attrs),
                syn::Item::Trait(item) => is_cfg_test(&item.attrs),
                _ => false,
            };
        if item_under_test {
            continue;
        }
        match item {
            syn::Item::Trait(item) => {
                let name = normalized_ident(&item.ident);
                local_traits.insert(name.clone());
                if !matches!(item.vis, syn::Visibility::Inherited) {
                    visible_traits.insert(name);
                }
            }
            syn::Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    collect_trait_visibility(items, false, local_traits, visible_traits);
                }
            }
            _ => {}
        }
    }
}

fn response_function_violations(file: &syn::File) -> Vec<&'static str> {
    let mut functions = Vec::new();
    let mut local_traits = std::collections::BTreeSet::new();
    let mut visible_traits = std::collections::BTreeSet::new();
    collect_trait_visibility(&file.items, false, &mut local_traits, &mut visible_traits);
    collect_response_functions(
        &file.items,
        false,
        true,
        &local_traits,
        &visible_traits,
        &mut functions,
    );
    let mut tainted: std::collections::BTreeSet<String> = functions
        .iter()
        .filter(|function| function.direct_response_use)
        .map(|function| function.name.clone())
        .collect();
    loop {
        let before = tainted.len();
        for function in &functions {
            if function.calls.iter().any(|called| tainted.contains(called)) {
                tainted.insert(function.name.clone());
            }
        }
        if tainted.len() == before {
            break;
        }
    }

    let mut violations = Vec::new();
    for function in &functions {
        if function.direct_response_use && !function.is_allowed_direct_response_consumer {
            push_unique(
                &mut violations,
                "response use outside parser or named validation transition",
            );
        }
        if function.visible
            && tainted.contains(&function.name)
            && !matches!(
                function.name.as_str(),
                "establish_mcp_bridge_carrier" | "refresh_mcp_bridge_authority"
            )
        {
            push_unique(
                &mut violations,
                "response authority reaches a visible function",
            );
        }
    }
    violations
}

#[derive(Default)]
struct TypeAliasCollector {
    aliases: Vec<(String, syn::Type)>,
}

impl<'ast> syn::visit::Visit<'ast> for TypeAliasCollector {
    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        self.aliases
            .push((normalized_ident(&node.ident), (*node.ty).clone()));
        syn::visit::visit_item_type(self, node);
    }
}

fn wire_type_aliases(file: &syn::File) -> std::collections::BTreeSet<String> {
    let mut collector = TypeAliasCollector::default();
    syn::visit::Visit::visit_file(&mut collector, file);
    let mut aliases = std::collections::BTreeSet::new();
    loop {
        let before = aliases.len();
        let mut names = aliases.clone();
        names.insert("McpAccountResponseWire".to_owned());
        for (alias, target) in &collector.aliases {
            if type_mentions_names(target, &names) {
                aliases.insert(alias.clone());
            }
        }
        if aliases.len() == before {
            break;
        }
    }
    aliases
}

fn syntax_detector_violations(
    source: &Path,
    detector: fn(&Path, &Path, &syn::File) -> Vec<&'static str>,
) -> Vec<(PathBuf, &'static str)> {
    rust_source_files(source)
        .into_iter()
        .flat_map(|path| {
            let text = fs::read_to_string(&path).expect("source reads");
            let file = syn::parse_file(&text).expect("Rust source parses");
            detector(source, &path, &file)
                .into_iter()
                .map(move |violation| (path.clone(), violation))
        })
        .collect()
}

fn diagnostic_syntax_detector(root: &Path, path: &Path, file: &syn::File) -> Vec<&'static str> {
    let mut aliases = FilesystemModuleAliasCollector::default();
    syn::visit::Visit::visit_file(&mut aliases, file);
    let mut visitor = DiagnosticSyntaxVisitor::new(
        path == root.join("tests.rs"),
        path == root.join("account_wire.rs")
            || path == root.join("bridge_carrier.rs")
            || path == root.join("bridge_session.rs"),
        if path == root.join("account_wire.rs") {
            Some("write_account_request")
        } else if path == root.join("bridge_carrier.rs") {
            Some("write_bridge_control")
        } else if path == root.join("bridge_session.rs") {
            Some("write_mux_frames")
        } else {
            None
        },
        aliases.resolve(),
    );
    syn::visit::Visit::visit_file(&mut visitor, file);
    visitor.violations
}

fn authority_syntax_detector(root: &Path, path: &Path, file: &syn::File) -> Vec<&'static str> {
    let is_privacy_unit = path == root.join("account_wire.rs");
    let mut visitor = AuthoritySyntaxVisitor::new(is_privacy_unit, wire_type_aliases(file));
    syn::visit::Visit::visit_file(&mut visitor, file);
    if is_privacy_unit {
        for violation in response_function_violations(file) {
            push_unique(&mut visitor.violations, violation);
        }
    }
    visitor.violations
}

fn assert_closed_corpus_files(source: &Path) {
    let files = rust_source_files(source);
    assert!(!files.is_empty(), "source corpus is nonempty");
    let account_wire = source.join("account_wire.rs");
    let test_seam = source.join("test_seam.rs");
    assert!(
        files.contains(&account_wire),
        "corpus includes exact account-wire path"
    );
    assert!(
        files.contains(&test_seam),
        "corpus includes exact test-seam path"
    );

    let account_file =
        syn::parse_file(&fs::read_to_string(&account_wire).expect("account-wire source reads"))
            .expect("account-wire source parses");
    assert!(account_file.items.iter().any(|item| {
        matches!(item, syn::Item::Fn(function) if function.sig.ident == "parse_account_registration_response")
    }));
    let seam_file =
        syn::parse_file(&fs::read_to_string(&test_seam).expect("test-seam source reads"))
            .expect("test-seam source parses");
    assert!(seam_file.items.iter().any(|item| {
        matches!(item, syn::Item::Enum(enumeration) if enumeration.ident == "OwnerBootstrapPrimitive")
    }));
}

#[test]
fn production_source_has_no_logging_or_printing_surface() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert_closed_corpus_files(&source);
    let violations = syntax_detector_violations(&source, diagnostic_syntax_detector);
    assert!(violations.is_empty(), "diagnostic egress: {violations:?}");
}

#[test]
fn production_source_has_no_response_wire_visibility_or_authority_leaks() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert_closed_corpus_files(&source);
    let violations = syntax_detector_violations(&source, authority_syntax_detector);
    assert!(
        violations.is_empty(),
        "response-wire visibility: {violations:?}"
    );
    let privacy_unit = fs::read_to_string(source.join("account_wire.rs"))
        .expect("account-wire privacy unit reads");
    let file = syn::parse_file(&privacy_unit).expect("account-wire Rust parses");
    let mut visitor = AuthoritySyntaxVisitor::new(true, wire_type_aliases(&file));
    syn::visit::Visit::visit_file(&mut visitor, &file);
    assert!(visitor.saw_parser, "privacy unit defines the parser");
    assert!(visitor.saw_wire, "privacy unit defines the response wire");
    assert!(visitor.saw_error, "privacy unit defines the closed error");
}

#[test]
fn diagnostic_egress_detector_rejects_each_planted_family() {
    let root = TempDir::new().expect("synthetic source root creates");
    let nested = root.path().join("nested");
    fs::create_dir(&nested).expect("synthetic nested source creates");
    let planted = [
        ["print", "!(\"x\");"].concat(),
        ["print", "ln!(\"x\");"].concat(),
        ["r#print", "ln!(\"x\");"].concat(),
        ["eprint", "!(\"x\");"].concat(),
        ["eprint", "ln!(\"x\");"].concat(),
        ["d", "bg!(1);"].concat(),
        ["std::io::std", "out();"].concat(),
        ["std::io::std", "err();"].concat(),
        [
            "use std::io::{self, Wri",
            "te}; io::stderr().write_all(b\"x\");",
        ]
        .concat(),
        ["use std::io::stderr as leak; le", "ak();"].concat(),
        ["write", "ln!(sink, \"x\");"].concat(),
        ["wri", "te!(sink, \"{}\", token);"].concat(),
        ["wri", "te!(std::io::stderr(), \"x\");"].concat(),
        [
            "fn leak<W: std::io::Wri",
            "te>(mut sink: W, token: &[u8]) { sink.write_all(token).unwrap(); }",
        ]
        .concat(),
        [
            "macro_rules! leak { () => { fn emit<W: std::io::Wri",
            "te>(mut sink: W, token: &[u8]) { sink.write_all(token).unwrap(); } } }",
        ]
        .concat(),
        [
            "macro_rules! leak { () => { eprint",
            "ln!(\"{}\", token); } }",
        ]
        .concat(),
        [
            "macro_rules! leak { () => { trac",
            "ing::info!(\"{}\", token); } }",
        ]
        .concat(),
        [
            "use std::io; io::Wri",
            "te::write_all(&mut sink, token).unwrap();",
        ]
        .concat(),
        ["use lo", "g::info;"].concat(),
        ["lo", "g::info!(\"x\");"].concat(),
        ["use trac", "ing::info;"].concat(),
        ["trac", "ing::info!(\"x\");"].concat(),
        ["r#trac", "ing::info!(\"x\");"].concat(),
        ["extern crate lo", "g as audit; audit::info!(\"x\");"].concat(),
        ["extern crate trac", "ing as audit; audit::info!(\"x\");"].concat(),
        ["use std::eprint", "ln as emit; emit!(\"{}\", token);"].concat(),
        [
            "use std::fs::wri",
            "te as persist; persist(\"token.txt\", token).unwrap();",
        ]
        .concat(),
        [
            "use std::fs as disk; disk::wri",
            "te(\"token.txt\", token).unwrap();",
        ]
        .concat(),
        [
            "use std::fs::{self as disk}; disk::wri",
            "te(\"token.txt\", token).unwrap();",
        ]
        .concat(),
        [
            "use std::fs as filesystem; use filesystem as disk; disk::wri",
            "te(\"token.txt\", token).unwrap();",
        ]
        .concat(),
        [
            "use std::fs::*; wri",
            "te(\"token.txt\", token).unwrap();",
        ]
        .concat(),
        [
            "use std::io::{self as input}; fn leak<W: input::Wri",
            "te>(mut sink: W, token: &[u8]) { <W as input::Write>::write(&mut sink, token).unwrap(); }",
        ]
        .concat(),
        [
            "use std::io::*; fn leak<W: Wri",
            "te>(mut sink: W, token: &[u8]) { <W as Write>::write(&mut sink, token).unwrap(); }",
        ]
        .concat(),
        [
            "use std::io::Wri",
            "te as Writer; fn leak(mut sink: std::io::Cursor<Vec<u8>>, token: &[u8]) { Writer::write(&mut sink, token).unwrap(); }",
        ]
        .concat(),
        [
            "use std::io::Wri",
            "te as Writer; use Writer as W; fn leak(mut sink: std::io::Cursor<Vec<u8>>, token: &[u8]) { W::write(&mut sink, token).unwrap(); }",
        ]
        .concat(),
        [
            "use std::fs as filesystem; use filesystem::wri",
            "te as persist; persist(\"token.txt\", token).unwrap();",
        ]
        .concat(),
        ["std::fs::wri", "te(\"token.txt\", token).unwrap();"].concat(),
    ];
    for (index, control) in planted.into_iter().enumerate() {
        let path = nested.join(format!("control-{index}.rs"));
        fs::write(&path, format!("fn control() {{ {control} }}")).expect("synthetic source writes");
        assert!(
            !syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty(),
            "diagnostic family {index} is rejected"
        );
        fs::remove_file(path).expect("synthetic source control removes");
    }

    let account_wire = root.path().join("account_wire.rs");
    fs::write(
        &account_wire,
        "fn persistence(writer: Writer) { writer.write_all(b\"x\"); }",
    )
    .expect("synthetic account-wire source writes");
    assert!(
        !syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty(),
        "only the exact account protocol writer function may write a request"
    );
    fs::remove_file(&account_wire).expect("synthetic account-wire control removes");

    fs::write(
        &account_wire,
        "async fn write_account_request(writer: Writer) { fn persistence(writer: Writer) { writer.write_all(b\"x\"); } let _ = writer; }",
    )
    .expect("synthetic nested account-wire source writes");
    assert!(
        !syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty(),
        "the account protocol writer exemption may not flow into nested functions"
    );
    fs::remove_file(&account_wire).expect("synthetic nested account-wire control removes");

    let carrier = root.path().join("bridge_carrier.rs");
    fs::write(
        &carrier,
        "async fn write_bridge_control(writer: Writer) { writer.write_all(b\"x\"); }",
    )
    .expect("synthetic bridge control writer writes");
    assert!(
        syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty(),
        "only the exact bridge protocol writer may write control bytes"
    );
    fs::write(
        &carrier,
        "async fn persistence(writer: Writer) { writer.write_all(b\"x\"); }",
    )
    .expect("synthetic bridge control writer mutation writes");
    assert!(
        !syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty(),
        "a bridge source file may not use another writer name as an egress exemption"
    );
    fs::remove_file(&carrier).expect("synthetic bridge control writer removes");

    let session = root.path().join("bridge_session.rs");
    fs::write(
        &session,
        "async fn write_mux_frames(writer: Writer) { writer.write_all(b\"x\"); }",
    )
    .expect("synthetic bridge mux writer writes");
    assert!(
        syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty(),
        "only the exact bridge mux writer may write framed bytes"
    );
    fs::write(
        &session,
        "async fn persistence(writer: Writer) { writer.write_all(b\"x\"); }",
    )
    .expect("synthetic bridge mux writer mutation writes");
    assert!(
        !syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty(),
        "a bridge mux source file may not use another writer name as an egress exemption"
    );
    fs::remove_file(&session).expect("synthetic bridge mux writer removes");

    fs::write(
        &account_wire,
        "async fn write_account_request(writer: Writer) { let mut writer = storage(); writer.write_all(b\"x\"); }",
    )
    .expect("synthetic shadowed account-wire source writes");
    assert!(
        !syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty(),
        "the account protocol writer parameter may not be shadowed"
    );
    fs::remove_file(account_wire).expect("synthetic shadowed account-wire control removes");

    fs::write(
        nested.join("clean.rs"),
        "use logistics::catalog; use std::fs as filesystem; use filesystem as disk; enum Permission { Write } fn clean(permission: Permission, log: &str, tracing: &str) { let stdout = \"stdout\"; let stderr = \"stderr\"; let _ = catalog; let _ = matches!(permission, Permission::Write); let _ = (log, tracing, stdout, stderr); let _ = disk::read(\"fixture\"); }",
    )
    .expect("clean synthetic source writes");
    assert!(syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty());
}

#[test]
fn response_wire_visibility_detector_rejects_each_planted_leak() {
    let root = TempDir::new().expect("synthetic source root creates");
    let nested = root.path().join("nested");
    fs::create_dir(&nested).expect("synthetic nested source creates");
    let wire = ["McpAccount", "ResponseWire"].concat();
    let parser = ["parse_account_registration", "_response"].concat();
    for (index, control) in [
        format!("pub(crate) struct {wire};"),
        format!("pub(super) struct {wire};"),
        format!("pub(in crate) struct {wire};"),
        format!("fn sibling(value: {wire}) {{ let {wire} {{ .. }} = value; }}"),
        format!("fn sibling() -> {wire} {{ todo!() }}"),
        format!("use crate::account_wire::{wire};"),
        format!("fn sibling() {{ let _ = {parser}(200, &[], &[]); }}"),
    ]
    .into_iter()
    .enumerate()
    {
        let path = nested.join(format!("leak-{index}.rs"));
        fs::write(&path, control).expect("synthetic source writes");
        assert!(
            !syntax_detector_violations(root.path(), authority_syntax_detector).is_empty(),
            "authority leak {index} is rejected"
        );
        fs::remove_file(path).expect("synthetic source control removes");
    }

    let nested_basename = nested.join("account_wire.rs");
    fs::write(&nested_basename, format!("struct {wire};")).expect("nested basename leak writes");
    assert!(
        !syntax_detector_violations(root.path(), authority_syntax_detector).is_empty(),
        "only the exact root privacy-unit path is allowlisted"
    );
    fs::remove_file(nested_basename).expect("nested basename leak removes");

    let privacy_unit = root.path().join("account_wire.rs");
    fs::write(
        &privacy_unit,
        format!(
            "struct {wire}; fn {parser}() -> {wire} {{ {wire} }} fn validate_account_registration(value: {wire}) {{ let _ = value; }} #[cfg(test)] pub(super) fn test_only() {{ let _ = {parser}(); }}"
        ),
    )
    .expect("synthetic privacy unit writes");
    assert!(
        syntax_detector_violations(root.path(), authority_syntax_detector).is_empty(),
        "private parser and named same-unit consumer remain allowed"
    );

    for (index, control) in [
        format!("#[derive(Clone)] struct {wire};"),
        format!("#[derive(Debug, serde::Serialize)] struct {wire};"),
        format!("use core::clone::Clone as C; #[derive(C)] struct {wire};"),
        format!("#[cfg_attr(not(test), derive(Debug))] struct {wire};"),
        format!("struct {wire} {{ pub(crate) token: String }}"),
        format!("struct {wire}; enum Holder {{ Wire({wire}) }}"),
        format!("struct {wire}; union Holder {{ wire: std::mem::ManuallyDrop<{wire}> }}"),
        format!("struct {wire}; trait Carrier {{ type Output; }} struct Holder; impl Carrier for Holder {{ type Output = {wire}; }}"),
        format!("struct {wire}; trait Carrier {{ type Output = {wire}; }}"),
        format!("struct {wire}; impl {wire} {{ pub(super) fn token(&self) {{}} }}"),
        format!("struct {wire}; impl {wire} {{ pub(crate) fn one(&self) {{}} }} impl {wire} {{ pub(in crate) fn two(&self) {{}} }}"),
        format!("struct {wire}; pub(super) async fn {parser}() -> {wire} {{ {wire} }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} pub(super) fn leak() -> String {{ {parser}().token }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} fn helper() -> String {{ {parser}().token }} pub(super) fn leak() -> String {{ helper() }}"),
        format!("struct {wire} {{ token: String }} fn validate_account_registration(value: {wire}) -> String {{ value.token }} pub(super) fn leak(value: {wire}) -> String {{ validate_account_registration(value) }}"),
        format!("struct {wire} {{ token: String }} struct Holder; impl Holder {{ fn validate_account_registration(&self, value: {wire}) -> String {{ value.token }} pub(super) fn leak(&self, value: {wire}) -> String {{ self.validate_account_registration(value) }} }}"),
        format!("struct {wire} {{ token: String }} fn validate_account_registration(value: {wire}) -> String {{ value.token }} pub(super) fn leak(value: {wire}) -> String {{ let transition = validate_account_registration; transition(value) }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} use self::{parser} as parse; pub(super) fn leak() -> String {{ parse().token }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} const PARSE: fn() -> {wire} = {parser}; pub(super) fn leak() -> String {{ PARSE().token }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} static PARSE: fn() -> {wire} = {parser}; pub(super) fn leak() -> String {{ PARSE().token }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} struct Holder; impl Holder {{ const PARSE: fn() -> {wire} = {parser}; }} pub(super) fn leak() -> String {{ (Holder::PARSE)().token }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} pub trait Reveal {{ const PARSE: fn() -> {wire} = {parser}; }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} fn validate_account_registration() -> String {{ {parser}().token }} pub trait Reveal {{ fn leak() -> String {{ validate_account_registration() }} }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} fn validate_account_registration() -> String {{ {parser}().token }} struct Holder; pub trait Reveal {{ fn leak() -> String; }} impl Reveal for Holder {{ fn leak() -> String {{ validate_account_registration() }} }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} mod child {{ use super::{wire}; fn validate_account_registration(value: {wire}) -> String {{ value.token }} }}"),
        format!("struct {wire}; extern \"C\" {{ fn hand_off(value: {wire}); }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} fn validate_account_registration() -> String {{ {parser}().token }} trait Display {{}} struct Holder; impl std::fmt::Display for Holder {{ fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ let _ = validate_account_registration(); Ok(()) }} }}"),
        format!("struct {wire} {{ token: String }} fn validate_account_registration(value: {wire}) -> String {{ value.token }} pub(super) fn leak(value: {wire}) -> String {{ invoke!(validate_account_registration(value)) }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} #[cfg(not(test))] pub(super) fn leak() -> String {{ {parser}().token }}"),
        format!("struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} #[cfg(any(test, feature = \"test-hooks\"))] pub(super) fn leak() -> String {{ {parser}().token }}"),
        format!("struct {wire} {{ token: String }} fn r#{parser}() -> {wire} {{ todo!() }} pub(super) fn leak() -> String {{ r#{parser}().r#token }}"),
        format!("struct {wire} {{ token: String }} macro_rules! expose {{ () => {{ pub(crate) fn leak() -> String {{ {parser}().token }} }} }}"),
        format!("struct {wire}; impl Clone for {wire} {{ fn clone(&self) -> Self {{ {wire} }} }}"),
        format!("use std::fmt::Debug as Reveal; struct {wire}; impl Reveal for {wire} {{ fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ Ok(()) }} }}"),
        format!("struct {wire}; type Alias = {wire}; impl std::fmt::Debug for Alias {{ fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ Ok(()) }} }}"),
        format!("struct {wire}; impl std::fmt::Debug for &{wire} {{ fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ Ok(()) }} }}"),
        format!("struct {wire}; type Alias<'a> = &'a {wire}; impl std::fmt::Debug for Alias<'_> {{ fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ Ok(()) }} }}"),
        format!("struct {wire}; impl std::fmt::Debug for {wire} {{ fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ Ok(()) }} }}"),
        format!("struct {wire}; impl serde::Serialize for {wire} {{ fn serialize<S>(&self, _: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {{ todo!() }} }}"),
        format!("struct {wire}; impl std::fmt::Display for {wire} {{ fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ Ok(()) }} }}"),
    ]
    .into_iter()
    .enumerate()
    {
        fs::write(&privacy_unit, control).expect("synthetic privacy violation writes");
        assert!(
            !syntax_detector_violations(root.path(), authority_syntax_detector).is_empty(),
            "privacy-unit leak {index} is rejected"
        );
    }
    fs::write(
        &privacy_unit,
        format!(
            "struct {wire}; fn {parser}() -> {wire} {{ {wire} }} fn validate_account_registration(value: {wire}) {{ let _ = value; }} #[cfg(test)] pub(super) fn test_only() {{ let _ = {parser}(); }}"
        ),
    )
    .expect("synthetic privacy unit restores");
    fs::write(
        &privacy_unit,
        format!(
            "struct {wire} {{ token: String }} fn {parser}() -> {wire} {{ todo!() }} fn validate_account_registration() -> String {{ {parser}().token }} trait PrivateDefault {{ fn reveal() -> String {{ validate_account_registration() }} }} trait PrivateImplemented {{ fn reveal() -> String; }} struct Holder; impl PrivateImplemented for Holder {{ fn reveal() -> String {{ validate_account_registration() }} }}"
        ),
    )
    .expect("private trait clean control writes");
    assert!(
        syntax_detector_violations(root.path(), authority_syntax_detector).is_empty(),
        "private traits do not make their methods externally visible"
    );
    fs::write(
        &privacy_unit,
        format!(
            "struct {wire}; fn {parser}() -> {wire} {{ {wire} }} fn validate_account_registration(value: {wire}) {{ let _ = value; }}"
        ),
    )
    .expect("synthetic privacy unit restores after clean trait control");

    fs::write(
        nested.join("combined.rs"),
        format!("fn leak(value: {wire}) {{ std::io::stderr(); let _ = value; }}"),
    )
    .expect("combined synthetic source writes");
    assert!(!syntax_detector_violations(root.path(), authority_syntax_detector).is_empty());
    assert!(!syntax_detector_violations(root.path(), diagnostic_syntax_detector).is_empty());
}
