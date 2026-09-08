// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The public facade's original-argv admission and prelaunch refusal boundary.

use super::*;
use std::os::windows::ffi::OsStringExt;

fn action() -> solstone_core_service_unit::WindowsServiceAction {
    use solstone_core_installation_identity::{
        Generation, GuardFields, InstallationId, NamespaceName, journal_token_from_path,
    };
    let journal = "C:\\Users\\Zoë\\Journal & notes\\";
    solstone_core_service_unit::WindowsServiceAction {
        port: 6123,
        journal: journal.into(),
        guard: GuardFields {
            namespace: NamespaceName::parse(&"1".repeat(64)).unwrap(),
            id: InstallationId::parse(&"2".repeat(32)).unwrap(),
            generation: Generation::new(7).unwrap(),
            journal_token: journal_token_from_path(Path::new(journal)).unwrap(),
        },
    }
}

#[test]
fn installed_request_preserves_exact_public_action() {
    let action = action();
    let original = action.arguments().unwrap();
    let args: Vec<_> = original.iter().map(OsString::from).collect();
    let request = installed_task_request(&args, None).unwrap().unwrap();
    assert_eq!(request.arguments, original);
    assert_eq!(request.arguments[1], "6123");
    assert_eq!(request.journal, PathBuf::from(action.journal));
    assert_eq!(request.guard, action.guard);
    let process = crate::processes::native_process_spec_for("supervisor").unwrap();
    assert_eq!(native_process_args(process, &args[1..]), args);
    assert!(
        installed_task_request(&["supervisor".into()], None)
            .unwrap()
            .is_none()
    );
}

#[test]
fn installed_request_refuses_partial_non_unicode_and_rewritten_action() {
    let original: Vec<_> = action()
        .arguments()
        .unwrap()
        .into_iter()
        .map(OsString::from)
        .collect();
    // Keep the private marker while removing each guard pair independently.
    for index in (5..original.len()).step_by(2) {
        let mut args = original.clone();
        args.drain(index..index + 2);
        assert!(installed_task_request(&args, None).is_err());
    }
    for index in 0..original.len() {
        let mut args = original.clone();
        args[index] = OsString::from_wide(&[0xd800]);
        assert!(
            installed_task_request(&args, None).is_err(),
            "argv[{index}]"
        );
    }
    let mut args = original.clone();
    args.insert(0, "-v".into());
    assert!(
        installed_task_request(&args, None).is_err(),
        "no global-option rewrite"
    );
    let mut args = original.clone();
    args.push("--help".into());
    assert!(installed_task_request(&args, None).is_err());
    let mut args = original;
    args[1] = "06123".into();
    assert!(installed_task_request(&args, None).is_err());
}

#[test]
fn installed_forwarding_refuses_changed_argv_before_launch() {
    let original: Vec<_> = action()
        .arguments()
        .unwrap()
        .into_iter()
        .map(OsString::from)
        .collect();
    let request = installed_task_request(&original, None).unwrap().unwrap();
    for index in 0..original.len() {
        let mut changed = original.clone();
        changed[index].push("x");
        let error = exec_process(
            OsStr::new("must-not-launch.exe"),
            &changed,
            None,
            Some(&request),
        )
        .expect_err("changed action must refuse before launching or exiting");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            error.to_string(),
            "installed task forwarding differs from original arguments"
        );
    }
}
