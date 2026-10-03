// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]
#![allow(unsafe_code)]

use std::fs;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_journal_io::{
    PrivateLockWait, PrivateReadCeilingExceeded, PrivateStateError, PrivateStatePhase,
    acquire_private_lock, create_or_open_private_child_directory, create_or_open_private_directory,
    publish_private_file, read_private_file, run_with_private_cleanup_fault,
    run_with_private_descriptor_barrier, run_with_private_destination_barrier,
    run_with_private_lock_barrier, run_with_private_post_commit_flush_fault,
    run_with_private_pre_commit_flush_fault, run_with_private_read_barrier,
    run_with_private_write_fault,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, GetAce, GetKernelObjectSecurity,
    GetLengthSid, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    GetSecurityDescriptorOwner, GetTokenInformation, InitializeSecurityDescriptor,
    OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED, SECURITY_DESCRIPTOR,
    SetKernelObjectSecurity, SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

fn make_temp_journal() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary journal directory")
}

fn current_process_user_sid() -> Vec<u8> {
    let mut token: HANDLE = INVALID_HANDLE_VALUE;
    unsafe {
        let ok = windows_sys::Win32::System::Threading::OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY,
            &mut token,
        );
        assert_ne!(ok, 0, "OpenProcessToken failed");

        let mut needed = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        assert_ne!(needed, 0, "GetTokenInformation query length failed");

        let mut buffer = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        );
        CloseHandle(token);
        assert_ne!(ok, 0, "GetTokenInformation query user failed");

        let token_user = buffer.as_ptr().cast::<TOKEN_USER>();
        let psid = (*token_user).User.Sid;
        assert!(!psid.is_null(), "TokenUser SID is null");
        let sid_len = GetLengthSid(psid) as usize;
        std::slice::from_raw_parts(psid.cast::<u8>(), sid_len).to_vec()
    }
}

#[test]
fn private_lock_child_helper() {
    let Some(dir_path) = std::env::var_os("SOLSTONE_PRIVATE_LOCK_HELPER_DIR") else {
        return;
    };
    let lock_name = std::env::var("SOLSTONE_PRIVATE_LOCK_HELPER_NAME")
        .unwrap_or_else(|_| "test.lock".to_string());
    let journal_path = PathBuf::from(dir_path);
    let root = JournalRoot::open(&journal_path).expect("open root in helper");
    let dir = create_or_open_private_directory(&root, "private_locks").expect("open private dir");
    let lock = acquire_private_lock(&dir, &lock_name, PrivateLockWait::Immediate)
        .expect("acquire lock in helper");

    println!("\nLOCK_ACQUIRED");
    io::stdout().flush().unwrap();

    let mut buf = [0u8; 1];
    let _ = io::stdin().read_exact(&mut buf);
    drop(lock);
}

#[test]
fn publish_and_read_exact_bytes_and_query_descriptor() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir = create_or_open_private_directory(&root, "private_data").expect("create private dir");

    let payload = b"solstone confidential payload bytes";
    let published =
        publish_private_file(&dir, "secret.bin", payload).expect("publish private file");

    let read_back = read_private_file(&dir, "secret.bin", 1024)
        .expect("read private file")
        .expect("file must exist");
    assert_eq!(read_back, payload);

    let published_path = temp.path().join("private_data").join("secret.bin");
    let wide: Vec<u16> = published_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);

    let mut needed = 0u32;
    unsafe {
        GetKernelObjectSecurity(
            handle,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        let mut buffer = vec![0u8; needed as usize];
        let ok = GetKernelObjectSecurity(
            handle,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        );
        CloseHandle(handle);
        assert_ne!(ok, 0, "GetKernelObjectSecurity failed");

        let sd_ptr = buffer.as_ptr() as *const SECURITY_DESCRIPTOR;
        let mut control: u16 = 0;
        let mut revision: u32 = 0;
        assert_ne!(
            GetSecurityDescriptorControl(sd_ptr.cast_mut().cast(), &mut control, &mut revision),
            0
        );
        assert_ne!(
            control & SE_DACL_PROTECTED,
            0,
            "descriptor DACL must be protected"
        );

        let mut owner_psid: PSID = std::ptr::null_mut();
        let mut owner_defaulted = 0i32;
        assert_ne!(
            GetSecurityDescriptorOwner(
                sd_ptr.cast_mut().cast(),
                &mut owner_psid,
                &mut owner_defaulted,
            ),
            0
        );
        assert!(!owner_psid.is_null());
        let owner_len = GetLengthSid(owner_psid) as usize;
        let owner_bytes = std::slice::from_raw_parts(owner_psid.cast::<u8>(), owner_len);
        let user_sid = current_process_user_sid();
        assert_eq!(owner_bytes, &user_sid[..], "owner must match TokenUser SID");

        let mut dacl_present = 0i32;
        let mut dacl_ptr: *mut ACL = std::ptr::null_mut();
        let mut dacl_defaulted = 0i32;
        assert_ne!(
            GetSecurityDescriptorDacl(
                sd_ptr.cast_mut().cast(),
                &mut dacl_present,
                &mut dacl_ptr,
                &mut dacl_defaulted,
            ),
            0
        );
        assert_ne!(dacl_present, 0, "DACL must be present");
        assert!(!dacl_ptr.is_null(), "DACL must not be null");
        assert_eq!((*dacl_ptr).AceCount, 1, "exactly one ACE required");

        let mut ace_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        assert_ne!(GetAce(dacl_ptr, 0, &mut ace_ptr), 0);
        let ace = ace_ptr.cast::<ACCESS_ALLOWED_ACE>();
        assert_eq!((*ace).Header.AceType, 0, "ACE type must be ACCESS_ALLOWED");
        assert_eq!((*ace).Header.AceFlags, 0, "ACE flags must be 0");
        assert_eq!((*ace).Mask, 0x0013019F, "ACE mask must be FILE_ACCESS_MASK");

        let ace_sid_ptr = (&(*ace).SidStart as *const u32).cast::<std::ffi::c_void>();
        let ace_sid_len = GetLengthSid(ace_sid_ptr as PSID) as usize;
        let ace_sid_bytes = std::slice::from_raw_parts(ace_sid_ptr.cast::<u8>(), ace_sid_len);
        assert_eq!(
            ace_sid_bytes,
            &user_sid[..],
            "ACE SID must match TokenUser SID"
        );
    }

    let child_dir =
        create_or_open_private_child_directory(&dir, "child_sub").expect("create child dir");
    let child_published = publish_private_file(&child_dir, "child_secret.bin", b"child payload")
        .expect("publish child private file");
    assert_ne!(published.identity(), child_published.identity());

    let child_read = read_private_file(&child_dir, "child_secret.bin", 1024)
        .expect("read child private file")
        .expect("child file must exist");
    assert_eq!(child_read, b"child payload");
}

#[test]
fn poisoned_descriptor_rejected_with_security_phase() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir =
        create_or_open_private_directory(&root, "private_poison").expect("create private dir");

    publish_private_file(&dir, "target.bin", b"initial data").expect("publish target");

    let file_path = temp.path().join("private_poison").join("target.bin");
    let wide: Vec<u16> = file_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            WRITE_DAC | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);

    let mut sd = SECURITY_DESCRIPTOR {
        Revision: 0,
        Sbz1: 0,
        Control: 0,
        Owner: std::ptr::null_mut(),
        Group: std::ptr::null_mut(),
        Sacl: std::ptr::null_mut(),
        Dacl: std::ptr::null_mut(),
    };
    unsafe {
        assert_ne!(
            InitializeSecurityDescriptor((&mut sd as *mut SECURITY_DESCRIPTOR).cast(), 1),
            0
        );
        assert_ne!(
            SetSecurityDescriptorDacl(
                (&mut sd as *mut SECURITY_DESCRIPTOR).cast(),
                1,
                std::ptr::null_mut(),
                0,
            ),
            0
        );
        assert_ne!(
            SetKernelObjectSecurity(
                handle,
                DACL_SECURITY_INFORMATION,
                (&mut sd as *mut SECURITY_DESCRIPTOR).cast(),
            ),
            0
        );
        CloseHandle(handle);
    }

    let err = read_private_file(&dir, "target.bin", 1024).expect_err("poisoned read must fail");
    match err {
        PrivateStateError::Failed { phase, .. } => {
            assert_eq!(phase, PrivateStatePhase::Security);
        }
        other => panic!("expected Security failure, got: {other:?}"),
    }

    // Verify the null DACL remains unchanged (no repair on read).
    let query_handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(query_handle, INVALID_HANDLE_VALUE);
    let mut needed = 0u32;
    unsafe {
        GetKernelObjectSecurity(
            query_handle,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        let mut buffer = vec![0u8; needed as usize];
        assert_ne!(
            GetKernelObjectSecurity(
                query_handle,
                DACL_SECURITY_INFORMATION,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            ),
            0
        );
        CloseHandle(query_handle);

        let sd_ptr = buffer.as_ptr() as *const SECURITY_DESCRIPTOR;
        let mut dacl_present = 0i32;
        let mut dacl_ptr: *mut ACL = std::ptr::null_mut();
        let mut dacl_defaulted = 0i32;
        assert_ne!(
            GetSecurityDescriptorDacl(
                sd_ptr.cast_mut().cast(),
                &mut dacl_present,
                &mut dacl_ptr,
                &mut dacl_defaulted,
            ),
            0
        );
        assert_eq!(dacl_present, 1, "DACL must remain present");
        assert!(dacl_ptr.is_null(), "DACL must remain null");
    }
}

#[test]
fn read_barrier_substitution_rejected() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir = create_or_open_private_directory(&root, "private_read_barrier")
        .expect("create private dir");

    let dir_path = temp.path().join("private_read_barrier");
    let orig_path = dir_path.join("target.bin");
    let side_path = dir_path.join("target.side");

    publish_private_file(&dir, "target.bin", b"initial original data").unwrap();

    let err = run_with_private_read_barrier(
        move || {
            fs::rename(&orig_path, &side_path).unwrap();
            fs::write(&orig_path, b"substituted foreign data").unwrap();
        },
        || read_private_file(&dir, "target.bin", 1024),
    )
    .expect_err("substituted read must fail");

    match err {
        PrivateStateError::Failed { phase, .. } => {
            assert_eq!(phase, PrivateStatePhase::NameBinding);
        }
        other => panic!("expected NameBinding failure, got: {other:?}"),
    }

    assert_eq!(
        fs::read(dir_path.join("target.bin")).unwrap(),
        b"substituted foreign data"
    );
    assert_eq!(
        fs::read(dir_path.join("target.side")).unwrap(),
        b"initial original data"
    );
}

#[test]
fn read_barrier_denies_in_place_writer() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).unwrap();
    let dir = create_or_open_private_directory(&root, "private_read_writer").unwrap();
    publish_private_file(&dir, "target.bin", b"stable original bytes").unwrap();
    let path = temp.path().join("private_read_writer/target.bin");
    // The control proves that the ACL permits this owner to open a writer.
    drop(fs::OpenOptions::new().write(true).open(&path).unwrap());
    let barrier_path = path.clone();
    let bytes = run_with_private_read_barrier(
        move || {
            let error = fs::OpenOptions::new()
                .write(true)
                .open(&barrier_path)
                .expect_err("retained read must deny a concurrent writer");
            assert_eq!(error.raw_os_error(), Some(32), "expected sharing violation");
        },
        || read_private_file(&dir, "target.bin", 1024),
    )
    .unwrap()
    .unwrap();
    assert_eq!(bytes, b"stable original bytes");
    drop(fs::OpenOptions::new().write(true).open(&path).unwrap());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

fn set_null_dacl(path: &Path) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let raw = unsafe {
        CreateFileW(
            wide.as_ptr(),
            WRITE_DAC | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE, "open descriptor mutation handle");
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut descriptor: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
    unsafe {
        assert_ne!(
            InitializeSecurityDescriptor((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(), 1),
            0
        );
        assert_ne!(
            SetSecurityDescriptorDacl(
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
                1,
                std::ptr::null_mut(),
                0
            ),
            0
        );
        assert_ne!(
            SetKernelObjectSecurity(
                handle.as_raw_handle(),
                DACL_SECURITY_INFORMATION,
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast()
            ),
            0
        );
    }
}

#[test]
fn changed_parent_descriptor_refuses_all_child_operations() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).unwrap();
    let parent = create_or_open_private_directory(&root, "private_parent_acl").unwrap();
    let child = create_or_open_private_child_directory(&parent, "child").unwrap();
    publish_private_file(&child, "target.bin", b"preserved synthetic bytes").unwrap();
    let parent_path = temp.path().join("private_parent_acl");
    set_null_dacl(&parent_path);
    let errors = [
        read_private_file(&child, "target.bin", 1024).unwrap_err(),
        publish_private_file(&child, "new.bin", b"must not publish").unwrap_err(),
        acquire_private_lock(&child, "new.lock", PrivateLockWait::Immediate).unwrap_err(),
        create_or_open_private_child_directory(&child, "new_child").unwrap_err(),
    ];
    for error in errors {
        assert!(matches!(
            error,
            PrivateStateError::Failed {
                phase: PrivateStatePhase::Security,
                ..
            }
        ));
    }
    assert_eq!(
        fs::read(parent_path.join("child/target.bin")).unwrap(),
        b"preserved synthetic bytes"
    );
    assert_eq!(fs::read_dir(parent_path.join("child")).unwrap().count(), 1);
}

#[test]
fn lock_barrier_substitution_rejected() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir = create_or_open_private_directory(&root, "private_lock_barrier")
        .expect("create private dir");
    let path = temp.path().join("private_lock_barrier/barrier.lock");
    let side = temp.path().join("private_lock_barrier/barrier.side");
    let initial = acquire_private_lock(&dir, "barrier.lock", PrivateLockWait::Immediate).unwrap();
    let identity = initial.identity();
    drop(initial);
    let barrier_path = path.clone();
    let barrier_side = side.clone();
    let lock = run_with_private_lock_barrier(
        move || {
            assert!(
                fs::rename(&barrier_path, &barrier_side).is_err(),
                "open lock must prevent substitution"
            );
        },
        || acquire_private_lock(&dir, "barrier.lock", PrivateLockWait::Immediate),
    )
    .expect("unchanged persistent lock must remain acquirable");
    assert_eq!(lock.identity(), identity);
    assert!(
        fs::rename(&path, &side).is_err(),
        "held lock must prevent substitution"
    );
    assert!(!side.exists());
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    drop(lock);
    assert_eq!(fs::read(&path).unwrap(), b"");
    assert_eq!(
        acquire_private_lock(&dir, "barrier.lock", PrivateLockWait::Immediate)
            .unwrap()
            .identity(),
        identity
    );
}

#[test]
fn missing_oversized_collision_and_ancestor_rename() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir =
        create_or_open_private_directory(&root, "private_matrix").expect("create private dir");

    let missing = read_private_file(&dir, "missing.bin", 1024).expect("missing is Ok(None)");
    assert_eq!(missing, None);

    publish_private_file(&dir, "sized.bin", b"0123456789").expect("publish sized");

    let zero_ceiling_err =
        read_private_file(&dir, "sized.bin", 0).expect_err("ceiling 0 must fail");
    match zero_ceiling_err {
        PrivateStateError::Failed { phase, source, .. } => {
            assert_eq!(phase, PrivateStatePhase::Admission);
            let detail = source
                .get_ref()
                .and_then(|e| e.downcast_ref::<PrivateReadCeilingExceeded>());
            assert_eq!(
                detail,
                Some(&PrivateReadCeilingExceeded {
                    observed_size: 10,
                    ceiling: 0,
                })
            );
        }
        other => panic!("expected Admission failure, got: {other:?}"),
    }

    let small_ceiling_err =
        read_private_file(&dir, "sized.bin", 5).expect_err("ceiling 5 must fail");
    match small_ceiling_err {
        PrivateStateError::Failed { phase, source, .. } => {
            assert_eq!(phase, PrivateStatePhase::Admission);
            let detail = source
                .get_ref()
                .and_then(|e| e.downcast_ref::<PrivateReadCeilingExceeded>());
            assert_eq!(
                detail,
                Some(&PrivateReadCeilingExceeded {
                    observed_size: 10,
                    ceiling: 5,
                })
            );
        }
        other => panic!("expected Admission failure, got: {other:?}"),
    }

    let collision_err =
        publish_private_file(&dir, "sized.bin", b"new content").expect_err("collision must fail");
    match collision_err {
        PrivateStateError::Failed { phase, .. } => {
            assert_eq!(phase, PrivateStatePhase::Create);
        }
        other => panic!("expected Create failure, got: {other:?}"),
    }
    let after_collision = read_private_file(&dir, "sized.bin", 1024).unwrap().unwrap();
    assert_eq!(after_collision, b"0123456789");

    let old_ancestor = temp.path().join("ancestor_old");
    fs::create_dir(&old_ancestor).unwrap();
    let anc_root = JournalRoot::open(&old_ancestor).expect("open old ancestor root");
    let anc_dir =
        create_or_open_private_directory(&anc_root, "private_anc").expect("create anc dir");
    let root_id_before = anc_root.identity();
    let anc_id_before = anc_dir.identity();
    publish_private_file(&anc_dir, "doc.txt", b"doc data").expect("publish anc doc");

    let new_ancestor = temp.path().join("ancestor_new");
    let rename_error = fs::rename(&old_ancestor, &new_ancestor)
        .expect_err("ancestor rename must be refused while private descendant authority is held");
    assert_eq!(rename_error.raw_os_error(), Some(5));
    assert!(!new_ancestor.exists());

    let read_after_refusal = read_private_file(&anc_dir, "doc.txt", 1024)
        .expect("unchanged retained authority must still admit the exact owner bytes");
    assert_eq!(read_after_refusal, Some(b"doc data".to_vec()));
    assert_eq!(anc_root.identity(), root_id_before);
    assert_eq!(anc_dir.identity(), anc_id_before);

    drop(anc_root);
    let retained_parent_error = fs::rename(&old_ancestor, &new_ancestor)
        .expect_err("the private child must retain its ancestor authority");
    assert_eq!(retained_parent_error.raw_os_error(), Some(5));
    assert!(!new_ancestor.exists());
    assert_eq!(
        read_private_file(&anc_dir, "doc.txt", 1024).unwrap(),
        Some(b"doc data".to_vec())
    );

    drop(anc_dir);
    fs::rename(&old_ancestor, &new_ancestor)
        .expect("positive control: released ancestor can actually be renamed");
    assert!(!old_ancestor.exists());
    let reopened_root = JournalRoot::open(&new_ancestor).expect("open renamed root");
    assert_eq!(reopened_root.identity(), root_id_before);
    let reopened_dir = create_or_open_private_directory(&reopened_root, "private_anc")
        .expect("unchanged descriptor must still admit the renamed private directory");
    assert_eq!(reopened_dir.identity(), anc_id_before);
    assert_eq!(
        read_private_file(&reopened_dir, "doc.txt", 1024).unwrap(),
        Some(b"doc data".to_vec())
    );
}

#[test]
fn phase_fault_injections_and_barriers() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir =
        create_or_open_private_directory(&root, "private_faults").expect("create private dir");

    let write_err =
        run_with_private_write_fault(|| publish_private_file(&dir, "write_fail.bin", b"bytes"))
            .expect_err("write fault must fail");
    match write_err {
        PrivateStateError::Failed { phase, .. } => assert_eq!(phase, PrivateStatePhase::Write),
        other => panic!("expected Write failure, got: {other:?}"),
    }

    let pre_flush_err = run_with_private_pre_commit_flush_fault(|| {
        publish_private_file(&dir, "flush_fail.bin", b"bytes")
    })
    .expect_err("pre-commit flush fault must fail");
    match pre_flush_err {
        PrivateStateError::Failed { phase, .. } => assert_eq!(phase, PrivateStatePhase::Flush),
        other => panic!("expected Flush failure, got: {other:?}"),
    }

    let plant_dir = temp.path().join("private_faults");
    let commit_err = run_with_private_destination_barrier(
        move || {
            let dest = plant_dir.join("substituted.bin");
            fs::write(dest, b"planted foreign bytes").unwrap();
        },
        || publish_private_file(&dir, "substituted.bin", b"original bytes"),
    )
    .expect_err("late destination substitution must fail");
    match commit_err {
        PrivateStateError::Failed { phase, .. } => assert_eq!(phase, PrivateStatePhase::Commit),
        other => panic!("expected Commit failure, got: {other:?}"),
    }
    let foreign = fs::read(temp.path().join("private_faults").join("substituted.bin")).unwrap();
    assert_eq!(foreign, b"planted foreign bytes");

    let desc_err = run_with_private_descriptor_barrier(
        |stage_path: &Path| {
            set_null_dacl(stage_path);
        },
        || publish_private_file(&dir, "desc_tampered.bin", b"bytes"),
    )
    .expect_err("descriptor tampering must fail");
    match desc_err {
        PrivateStateError::Failed { phase, .. } => {
            assert_eq!(phase, PrivateStatePhase::Security);
        }
        other => panic!("expected Security failure, got: {other:?}"),
    }
    assert!(
        !temp
            .path()
            .join("private_faults")
            .join("desc_tampered.bin")
            .exists(),
        "destination name must remain absent"
    );

    let post_flush_err = run_with_private_post_commit_flush_fault(|| {
        publish_private_file(&dir, "post_flush.bin", b"post flush bytes")
    })
    .expect_err("post commit flush fault must fail");
    match post_flush_err {
        PrivateStateError::Failed { phase, .. } => {
            assert_eq!(phase, PrivateStatePhase::PostCommitDurability);
        }
        other => panic!("expected PostCommitDurability failure, got: {other:?}"),
    }
    let post_flush_bytes = fs::read(temp.path().join("private_faults").join("post_flush.bin"))
        .expect("published file must still exist after post-commit flush failure");
    assert_eq!(post_flush_bytes, b"post flush bytes");

    let cleanup_err = run_with_private_write_fault(|| {
        run_with_private_cleanup_fault(|| publish_private_file(&dir, "cleanup_fail.bin", b"bytes"))
    })
    .expect_err("cleanup fault must fail with Cleanup phase");
    match cleanup_err {
        PrivateStateError::Failed { phase, .. } => assert_eq!(phase, PrivateStatePhase::Cleanup),
        other => panic!("expected Cleanup failure, got: {other:?}"),
    }
    assert!(
        !temp
            .path()
            .join("private_faults")
            .join("cleanup_fail.bin")
            .exists(),
        "cleanup_fail.bin must remain absent after failed publish"
    );
    let stage_entries: Vec<_> = fs::read_dir(temp.path().join("private_faults"))
        .expect("read_dir private_faults")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str != "cleanup_fail.bin" {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    assert!(
        !stage_entries.is_empty(),
        "private directory must still contain the un-cleaned owned stage"
    );
}

#[test]
fn extra_hard_link_rejected_with_admission() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir =
        create_or_open_private_directory(&root, "private_hardlink").expect("create private dir");

    let dir_path = temp.path().join("private_hardlink");
    let orig_file = dir_path.join("link_target.bin");
    let hard_link = dir_path.join("extra_link.bin");

    publish_private_file(&dir, "link_target.bin", b"hard link payload bytes").unwrap();
    fs::hard_link(&orig_file, &hard_link).expect("create hard link");

    let err = read_private_file(&dir, "link_target.bin", 1024).expect_err("hard link must fail");
    match err {
        PrivateStateError::Failed { phase, .. } => {
            assert_eq!(phase, PrivateStatePhase::Admission);
        }
        other => panic!("expected Admission failure, got: {other:?}"),
    }

    assert_eq!(fs::read(&orig_file).unwrap(), b"hard link payload bytes");
    assert_eq!(fs::read(&hard_link).unwrap(), b"hard link payload bytes");
}

#[test]
fn reparse_point_rejected_with_admission() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir =
        create_or_open_private_directory(&root, "private_reparse").expect("create private dir");

    let dir_path = temp.path().join("private_reparse");
    let target = temp.path().join("reparse_target.bin");
    fs::write(&target, b"external target bytes").unwrap();

    let symlink = dir_path.join("symlink.bin");
    std::os::windows::fs::symlink_file(&target, &symlink).expect("create file symlink");

    let err = read_private_file(&dir, "symlink.bin", 1024).expect_err("symlink must fail");
    match err {
        PrivateStateError::Failed { phase, .. } => {
            assert_eq!(phase, PrivateStatePhase::Admission);
        }
        other => panic!("expected Admission failure, got: {other:?}"),
    }

    assert!(
        fs::symlink_metadata(&symlink).is_ok(),
        "symlink must remain"
    );
}

#[test]
fn directory_name_replacement() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let parent = create_or_open_private_directory(&root, "parent_dir").expect("create parent dir");
    let child =
        create_or_open_private_child_directory(&parent, "child_dir").expect("create child dir");
    let parent_id = parent.identity();
    let child_id = child.identity();
    publish_private_file(&child, "doc.txt", b"child doc bytes").unwrap();
    let parent_path = temp.path().join("parent_dir");
    let side_path = temp.path().join("parent_side");
    let rename_error = fs::rename(&parent_path, &side_path)
        .expect_err("replacement must be refused while the private child authority is retained");
    assert_eq!(rename_error.raw_os_error(), Some(5));
    assert!(!side_path.exists());
    assert_eq!(parent.identity(), parent_id);
    assert_eq!(child.identity(), child_id);
    assert_eq!(
        read_private_file(&child, "doc.txt", 1024).unwrap(),
        Some(b"child doc bytes".to_vec())
    );
    assert_eq!(
        fs::read(parent_path.join("child_dir/doc.txt")).unwrap(),
        b"child doc bytes"
    );

    drop(parent);
    let retained_parent_error = fs::rename(&parent_path, &side_path)
        .expect_err("dropping the caller's parent must not release the child's parent authority");
    assert_eq!(retained_parent_error.raw_os_error(), Some(5));
    assert!(!side_path.exists());
    assert_eq!(child.identity(), child_id);
    assert_eq!(
        read_private_file(&child, "doc.txt", 1024).unwrap(),
        Some(b"child doc bytes".to_vec())
    );
}

struct LockChild {
    child: std::process::Child,
}
impl LockChild {
    fn spawn(root: &Path) -> Self {
        let child = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", "private_lock_child_helper", "--nocapture"])
            .env("SOLSTONE_PRIVATE_LOCK_HELPER_DIR", root)
            .env("SOLSTONE_PRIVATE_LOCK_HELPER_NAME", "exclusive.lock")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn lock child");
        Self { child }
    }
    fn wait_for_marker(&mut self) {
        use std::io::BufRead;
        let output = self.child.stdout.take().expect("child stdout");
        let (sender, receiver) = std::sync::mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(output.take(65536));
            for line in reader.by_ref().lines() {
                let read_failed = line.is_err();
                let _ = sender.send(line);
                if read_failed {
                    break;
                }
            }
            let mut output = reader.into_inner().into_inner();
            let _ = io::copy(&mut output, &mut io::sink());
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = receiver
                .recv_timeout(remaining)
                .expect("bounded lock child marker wait")
                .expect("read lock child output");
            if line.trim() == "LOCK_ACQUIRED" {
                break;
            }
        }
    }
    fn wait_bounded(&mut self) -> io::Result<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "lock child exit deadline",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for LockChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            if let Err(error) = self.wait_bounded() {
                eprintln!("lock child cleanup failed: {error}");
            }
        }
    }
}

#[test]
fn cross_process_lock_contention_and_persistence() {
    let temp = make_temp_journal();
    let root = JournalRoot::open(temp.path()).expect("open root");
    let dir = create_or_open_private_directory(&root, "private_locks").expect("create private dir");

    let lock = acquire_private_lock(&dir, "exclusive.lock", PrivateLockWait::Immediate)
        .expect("acquire initial lock");
    let identity = lock.identity();
    drop(lock);

    let mut child = LockChild::spawn(temp.path());
    child.wait_for_marker();

    let immediate_err = acquire_private_lock(&dir, "exclusive.lock", PrivateLockWait::Immediate)
        .expect_err("lock should contend immediately");
    match immediate_err {
        PrivateStateError::Failed { phase, .. } => assert_eq!(phase, PrivateStatePhase::Lock),
        other => panic!("expected Lock failure, got: {other:?}"),
    }

    let bounded_err = acquire_private_lock(
        &dir,
        "exclusive.lock",
        PrivateLockWait::Bounded(Duration::from_millis(50)),
    )
    .expect_err("lock should contend under bounded wait");
    match bounded_err {
        PrivateStateError::Failed { phase, .. } => assert_eq!(phase, PrivateStatePhase::Lock),
        other => panic!("expected Lock failure, got: {other:?}"),
    }

    drop(child.child.stdin.take());
    assert!(child.wait_bounded().expect("wait child").success());

    let lock_after = acquire_private_lock(&dir, "exclusive.lock", PrivateLockWait::Immediate)
        .expect("acquire lock after child release");
    assert_eq!(lock_after.identity(), identity);
    drop(lock_after);

    let mut child2 = LockChild::spawn(temp.path());
    child2.wait_for_marker();
    child2.child.kill().expect("kill child2");
    child2.wait_bounded().expect("reap killed child");

    let lock_after_kill = acquire_private_lock(&dir, "exclusive.lock", PrivateLockWait::Immediate)
        .expect("acquire lock after child kill");
    assert_eq!(lock_after_kill.identity(), identity);
    drop(lock_after_kill);
}
