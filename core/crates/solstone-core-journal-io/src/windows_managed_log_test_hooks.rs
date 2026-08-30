// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native-only receipt facade for the inactive Windows managed-log substrate.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, OPEN_EXISTING,
};

use crate::atomic::{
    ATOMIC_CANDIDATE_MARKER, BoundAtomicPublishError, DetailedAtomicOutcome,
    StrictManagedLogPublication, atomic_replace_detailed_bound, publication_candidate_name,
    require_strict_managed_log_publication, run_with_windows_detailed_atomic_backoffs,
    run_with_windows_detailed_atomic_barrier, run_with_windows_detailed_atomic_faults,
};
use crate::locking::{LockOptions, open_windows_path};
use crate::managed_log_names::{
    ManagedLogAliasRole, alias_lock_name, canonical_payload_name, day_alias_name, root_alias_name,
};
use crate::name_admission::check_portable_component;
use crate::windows_managed_log_lock::acquire_managed_log_alias_lock;
use crate::windows_managed_log_open::{
    create_canonical_for_append, open_canonical_for_append, open_canonical_for_read,
};
use crate::windows_managed_log_record::ManagedLogRecord;
use crate::windows_managed_log_resolve::resolve_managed_log_record;
use crate::windows_sync_dir::{
    WindowsFlatDirectory, create_or_open_windows_flat_directory_bound,
    open_windows_flat_directory_bound,
};

const DAY: &str = "20260829";
const REFERENCE: &str = "writer";
const NAME: &str = "stream";
const ORIGINAL: &[u8] = b"managed-log-original\n";
const APPENDED: &[u8] = b"managed-log-appended\n";
const REPLACEMENT: &[u8] = b"managed-log-replacement\n";

fn root_handle(path: &Path) -> File {
    open_windows_path(
        path,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
    )
    .unwrap_or_else(|error| panic!("bind receipt root {}: {error}", path.display()))
}

fn child(parent: &impl AsHandle, parent_path: &Path, name: &str) -> WindowsFlatDirectory {
    create_or_open_windows_flat_directory_bound(parent, OsStr::new(name), parent_path)
        .unwrap_or_else(|error| panic!("bind receipt directory {name}: {error}"))
}

fn lock_options(timeout: Duration) -> LockOptions {
    LockOptions {
        timeout,
        poll_interval: Duration::from_millis(10),
        mode: None,
    }
}

fn assert_reserved_names() {
    let hostile = [
        "maintenance:<task>",
        "Name",
        "name",
        "a/b",
        "a\\b",
        "stream:zone",
        ".",
        "..",
        "CON",
        "NUL",
        "COM1",
        "trailing. ",
        "",
        "e\u{301}",
        "é",
    ];
    let mut root_names = BTreeSet::new();
    let mut day_names = BTreeSet::new();
    for (index, value) in hostile.iter().enumerate() {
        let root_alias = root_alias_name(value);
        let day_alias = day_alias_name(value);
        let root_lock = alias_lock_name(ManagedLogAliasRole::Root, value);
        let day_lock = alias_lock_name(ManagedLogAliasRole::Day, value);
        let payload = canonical_payload_name(REFERENCE, value);
        let stage =
            publication_candidate_name(&payload, ATOMIC_CANDIDATE_MARKER, &[index as u128, 1]);
        for name in [
            &root_alias,
            &day_alias,
            &root_lock,
            &day_lock,
            &payload,
            &stage,
        ] {
            check_portable_component(&name.to_string_lossy())
                .unwrap_or_else(|error| panic!("unsafe managed-log name {name:?}: {error}"));
        }
        for name in [
            root_alias,
            root_lock,
            OsString::from(format!("ordinary-{index}.log")),
        ] {
            assert!(
                root_names.insert(name.to_string_lossy().to_ascii_lowercase()),
                "root managed-log role collision for {value:?}"
            );
        }
        for name in [day_alias, day_lock, payload, stage] {
            assert!(
                day_names.insert(name.to_string_lossy().to_ascii_lowercase()),
                "day managed-log role collision for {value:?}"
            );
        }
    }
}

fn create_junction(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("launch cmd.exe for native junction fixture");
    assert!(
        output.status.success(),
        "create junction fixture {} -> {}: status={} stdout={} stderr={}",
        link.display(),
        target.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn open_without_delete_share(path: &Path) -> OwnedHandle {
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: `wide` is NUL-terminated and remains live for the synchronous call.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        raw,
        INVALID_HANDLE_VALUE,
        "open sharing-violation fixture {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
    // SAFETY: the invalid-handle sentinel was rejected and ownership transfers once.
    #[allow(unsafe_code)]
    unsafe {
        OwnedHandle::from_raw_handle(raw)
    }
}

fn exercise_open_publish_resolve(root: &Path) {
    let root_file = root_handle(root);
    let aliases = child(&root_file, root, "aliases");
    let days = child(&root_file, root, "days");
    let day = child(&days, &root.join("days"), DAY);
    let health = child(&day, &root.join("days").join(DAY), "health");
    let payload_name = canonical_payload_name(REFERENCE, NAME);
    let payload_path = root
        .join("days")
        .join(DAY)
        .join("health")
        .join(&payload_name);

    let mut created = create_canonical_for_append(&health, &payload_name)
        .expect("create retained canonical append file");
    created.file_mut().write_all(ORIGINAL).unwrap();
    created.file_mut().sync_all().unwrap();
    let identity = created.identity();
    drop(created);

    let record = ManagedLogRecord::new(
        1,
        DAY.to_owned(),
        REFERENCE.to_owned(),
        NAME.to_owned(),
        identity,
    )
    .unwrap();
    let alias_name = root_alias_name(NAME);
    let lock = acquire_managed_log_alias_lock(
        &aliases,
        ManagedLogAliasRole::Root,
        NAME,
        lock_options(Duration::from_secs(2)),
    )
    .unwrap();
    let published = atomic_replace_detailed_bound(
        &aliases,
        lock.bound_parent_lock(),
        &alias_name,
        &record.to_bytes().unwrap(),
        0o600,
    );
    assert!(matches!(
        require_strict_managed_log_publication(published),
        Ok(StrictManagedLogPublication::Published)
    ));
    drop(lock);

    let mut resolved = resolve_managed_log_record(&aliases, &alias_name, |_| {
        open_windows_flat_directory_bound(&day, OsStr::new("health"), &root.join("days").join(DAY))?
            .ok_or_else(|| crate::errors::FlatDirectoryError::EnumerationChanged {
                path: root.join("days").join(DAY).join("health"),
            })
    })
    .unwrap();
    assert_eq!(resolved.identity, identity);
    let mut bytes = Vec::new();
    resolved.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, ORIGINAL);

    let mut appender = open_canonical_for_append(&health, &payload_name, identity).unwrap();
    let mut reader = open_canonical_for_read(&health, &payload_name, identity).unwrap();
    let retired_path = payload_path.with_extension("retired");
    fs::rename(&payload_path, &retired_path).unwrap();
    fs::write(&payload_path, REPLACEMENT).unwrap();
    appender.file_mut().write_all(APPENDED).unwrap();
    appender.file_mut().sync_all().unwrap();
    reader.file_mut().seek(SeekFrom::Start(0)).unwrap();
    let mut retained = Vec::new();
    reader.file_mut().read_to_end(&mut retained).unwrap();
    assert_eq!(retained, [ORIGINAL, APPENDED].concat());
    assert_eq!(
        fs::read(&retired_path).unwrap(),
        [ORIGINAL, APPENDED].concat()
    );
    assert_eq!(fs::read(&payload_path).unwrap(), REPLACEMENT);
    assert!(
        resolve_managed_log_record(&aliases, &alias_name, |_| {
            open_windows_flat_directory_bound(
                &day,
                OsStr::new("health"),
                &root.join("days").join(DAY),
            )?
            .ok_or_else(|| crate::errors::FlatDirectoryError::EnumerationChanged {
                path: root.join("days").join(DAY).join("health"),
            })
        })
        .is_err(),
        "resolver accepted a path replacement with a different full file identity"
    );

    let wrong_type = child(&day, &root.join("days").join(DAY), "wrong-type");
    let wrong_type_path = root.join("days").join(DAY).join("wrong-type");
    fs::create_dir(wrong_type_path.join(&payload_name)).unwrap();
    assert!(create_canonical_for_append(&wrong_type, &payload_name).is_err());

    let reparse = child(&day, &root.join("days").join(DAY), "reparse");
    let reparse_path = root.join("days").join(DAY).join("reparse");
    let junction_target = root.join("junction-target");
    fs::create_dir(&junction_target).unwrap();
    create_junction(&reparse_path.join(&payload_name), &junction_target);
    assert!(create_canonical_for_append(&reparse, &payload_name).is_err());
}

fn exercise_publication_outcomes(root: &Path) {
    let root_file = root_handle(root);

    let unverified = child(&root_file, root, "unverified");
    let unverified_name = root_alias_name("unverified");
    let unverified_lock = acquire_managed_log_alias_lock(
        &unverified,
        ManagedLogAliasRole::Root,
        "unverified",
        lock_options(Duration::from_secs(2)),
    )
    .unwrap();
    let (result, attempted, real_moves) = run_with_windows_detailed_atomic_faults(
        [(
            "post-publication-observation",
            1,
            ERROR_ACCESS_DENIED as i32,
        )],
        || {
            atomic_replace_detailed_bound(
                &unverified,
                unverified_lock.bound_parent_lock(),
                &unverified_name,
                b"unverified-landed",
                0o600,
            )
        },
    );
    assert!(matches!(
        result,
        Ok(DetailedAtomicOutcome::PublishedParentPathUnverified { .. })
    ));
    assert_eq!(real_moves, 1);
    assert!(attempted.contains(&"post-publication-observation"));
    assert_eq!(
        fs::read(root.join("unverified").join(&unverified_name)).unwrap(),
        b"unverified-landed"
    );

    let raced = child(&root_file, root, "raced");
    let raced_name = root_alias_name("raced");
    let raced_lock = acquire_managed_log_alias_lock(
        &raced,
        ManagedLogAliasRole::Root,
        "raced",
        lock_options(Duration::from_secs(2)),
    )
    .unwrap();
    let raced_path = root.join("raced");
    let retired_path = root.join("raced-retired");
    let raced_for_barrier = raced_path.clone();
    let retired_for_barrier = retired_path.clone();
    let (result, fired) = run_with_windows_detailed_atomic_barrier(
        "post-publication-reread",
        1,
        move || {
            fs::rename(&raced_for_barrier, &retired_for_barrier).unwrap();
            fs::create_dir(&raced_for_barrier).unwrap();
        },
        || {
            atomic_replace_detailed_bound(
                &raced,
                raced_lock.bound_parent_lock(),
                &raced_name,
                b"raced-landed",
                0o600,
            )
        },
    );
    assert!(fired);
    assert!(matches!(
        result,
        Ok(DetailedAtomicOutcome::PublishedParentPathRaced { .. })
    ));
    assert_eq!(
        fs::read(retired_path.join(&raced_name)).unwrap(),
        b"raced-landed"
    );

    let sharing = child(&root_file, root, "sharing");
    let sharing_name = root_alias_name("sharing");
    let sharing_path = root.join("sharing").join(&sharing_name);
    fs::write(&sharing_path, b"sharing-old").unwrap();
    let sharing_lock = acquire_managed_log_alias_lock(
        &sharing,
        ManagedLogAliasRole::Root,
        "sharing",
        lock_options(Duration::from_secs(2)),
    )
    .unwrap();
    let blocker = open_without_delete_share(&sharing_path);
    let (blocked, backoffs) = run_with_windows_detailed_atomic_backoffs(|| {
        atomic_replace_detailed_bound(
            &sharing,
            sharing_lock.bound_parent_lock(),
            &sharing_name,
            b"sharing-new",
            0o600,
        )
    });
    assert!(matches!(blocked, Err(BoundAtomicPublishError::Atomic(_))));
    assert!(
        !backoffs.is_empty(),
        "sharing violation did not reach retry policy"
    );
    assert_eq!(fs::read(&sharing_path).unwrap(), b"sharing-old");
    drop(blocker);
    assert!(matches!(
        atomic_replace_detailed_bound(
            &sharing,
            sharing_lock.bound_parent_lock(),
            &sharing_name,
            b"sharing-new",
            0o600,
        ),
        Ok(DetailedAtomicOutcome::Published)
    ));
    assert_eq!(fs::read(&sharing_path).unwrap(), b"sharing-new");

    assert!(matches!(
        require_strict_managed_log_publication(Ok(
            DetailedAtomicOutcome::PublishedDurabilityUncertain {
                source: std::io::Error::other("native policy control"),
            },
        )),
        Ok(StrictManagedLogPublication::Outcome(
            DetailedAtomicOutcome::PublishedDurabilityUncertain { .. }
        ))
    ));
}

/// Exercise the inactive substrate against one native Windows filesystem root.
pub fn exercise_windows_managed_log_reference_substrate(root: &Path) {
    assert_reserved_names();
    let open_resolve = root.join("open-resolve");
    let outcomes = root.join("outcomes");
    fs::create_dir(&open_resolve).unwrap();
    fs::create_dir(&outcomes).unwrap();
    exercise_open_publish_resolve(&open_resolve);
    exercise_publication_outcomes(&outcomes);
}

/// Return the deterministic root alias component used by process receipts.
pub fn root_test_managed_log_alias_name(logical_name: &str) -> OsString {
    root_alias_name(logical_name)
}

/// Try one process-visible root alias lock under `<root>/aliases`.
pub fn try_test_managed_log_alias_lock(root: &Path, logical_name: &str, timeout: Duration) -> bool {
    let root_file = root_handle(root);
    let aliases = child(&root_file, root, "aliases");
    acquire_managed_log_alias_lock(
        &aliases,
        ManagedLogAliasRole::Root,
        logical_name,
        lock_options(timeout),
    )
    .is_ok()
}

/// Publish one labelled alias through the current `<root>/aliases` identity.
pub fn publish_test_managed_log_alias(root: &Path, logical_name: &str, bytes: &[u8]) {
    let root_file = root_handle(root);
    let aliases = child(&root_file, root, "aliases");
    let lock = acquire_managed_log_alias_lock(
        &aliases,
        ManagedLogAliasRole::Root,
        logical_name,
        lock_options(Duration::from_secs(2)),
    )
    .unwrap();
    assert!(matches!(
        atomic_replace_detailed_bound(
            &aliases,
            lock.bound_parent_lock(),
            &root_alias_name(logical_name),
            bytes,
            0o600,
        ),
        Ok(DetailedAtomicOutcome::Published)
    ));
}

/// Hold the old root alias lock, then prove publication refuses after a root rebind.
pub fn hold_old_managed_log_alias_then_publish(
    root: &Path,
    logical_name: &str,
    ready: &Path,
    release: &Path,
    outcome: &Path,
) {
    let root_file = root_handle(root);
    let aliases = child(&root_file, root, "aliases");
    let lock = acquire_managed_log_alias_lock(
        &aliases,
        ManagedLogAliasRole::Root,
        logical_name,
        lock_options(Duration::from_secs(2)),
    )
    .unwrap();
    fs::write(ready, b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !release.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(release.exists(), "parent did not release split-lock child");
    let result = atomic_replace_detailed_bound(
        &aliases,
        lock.bound_parent_lock(),
        &root_alias_name(logical_name),
        b"old-parent-must-not-publish",
        0o600,
    );
    assert!(matches!(
        result,
        Err(BoundAtomicPublishError::NamespaceChanged)
    ));
    fs::write(outcome, b"namespace-changed").unwrap();
}
