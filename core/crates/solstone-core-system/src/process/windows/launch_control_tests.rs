// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Component controls for the production entry/offer validation boundaries.
//! These do not substitute for loaded-binding or authenticated pipe execution.

use super::*;
use std::os::windows::ffi::OsStringExt;

use solstone_core_installation_identity::{
    Generation, GuardFields, InstallationId, NamespaceName, service_guard_environment,
};

fn request() -> InstalledTaskLaunchRequest {
    let journal = PathBuf::from("C:\\Users\\Zoë\\Journal & notes\\");
    let mut request = InstalledTaskLaunchRequest {
        guard: GuardFields {
            namespace: NamespaceName::parse(&"1".repeat(64)).unwrap(),
            id: InstallationId::parse(&"2".repeat(32)).unwrap(),
            generation: Generation::new(7).unwrap(),
            journal_token: solstone_core_installation_identity::journal_token_from_path(&journal)
                .unwrap(),
        },
        arguments: vec![
            "supervisor".into(),
            "6123".into(),
            "--journal".into(),
            journal.to_str().unwrap().into(),
            "--windows-service".into(),
        ],
        journal,
        acknowledgement_timeout: Duration::from_secs(3),
    };
    let values = service_guard_environment(&request.guard);
    for (flag, key) in [
        ("--installation-namespace", GUARDS[0]),
        ("--installation-id", GUARDS[1]),
        ("--installation-generation", GUARDS[2]),
        ("--installation-journal-token", GUARDS[3]),
    ] {
        request
            .arguments
            .extend([flag.to_owned(), values[key].clone()]);
    }
    request
}

fn installed(request: &InstalledTaskLaunchRequest) -> LaunchTuple {
    LaunchTuple::InstalledTask(InstalledTaskTuple {
        parent: current_windows_process_instance().unwrap(),
        journal: request.journal.clone(),
        launch_id: "3".repeat(48),
        guards: service_guard_environment(&request.guard),
        arguments: request.arguments.clone(),
    })
}

#[test]
fn installed_entry_refuses_cross_variant_and_every_changed_action_field() {
    let request = request();
    let root = installed(&request);
    let hosted = LaunchTuple::Hosted(HostedTuple {
        parent: root.parent(),
        journal: request.journal.clone(),
        generation: 7,
        launch_id: "4".repeat(48),
        parent_launch_id: None,
        service: None,
        guards: service_guard_environment(&request.guard),
    });
    validate_entry(&root, Some(&request)).unwrap();
    validate_entry(&hosted, None).unwrap();
    assert!(validate_entry(&root, None).is_err());
    assert!(validate_entry(&hosted, Some(&request)).is_err());

    for index in 0..request.arguments.len() {
        let mut changed = request.clone();
        changed.arguments[index].push('x');
        assert!(
            validate_entry(&root, Some(&changed)).is_err(),
            "argv[{index}]"
        );
    }
    for length in 0..request.arguments.len() {
        let mut changed = request.clone();
        changed.arguments.truncate(length);
        assert!(validate_entry(&root, Some(&changed)).is_err());
    }
    let mut changed = request.clone();
    changed.arguments.push("-v".into());
    assert!(validate_entry(&root, Some(&changed)).is_err());
    changed = request.clone();
    changed.journal.push("other");
    assert!(validate_entry(&root, Some(&changed)).is_err());

    let mut guards = Vec::new();
    let mut changed = request.guard.clone();
    changed.namespace = NamespaceName::parse(&"5".repeat(64)).unwrap();
    guards.push(changed);
    let mut changed = request.guard.clone();
    changed.id = InstallationId::parse(&"6".repeat(32)).unwrap();
    guards.push(changed);
    let mut changed = request.guard.clone();
    changed.generation = Generation::new(8).unwrap();
    guards.push(changed);
    let mut changed = request.guard.clone();
    changed.journal_token =
        solstone_core_installation_identity::journal_token_from_path(&PathBuf::from("C:\\other"))
            .unwrap();
    guards.push(changed);
    for guard in guards {
        let mut changed = request.clone();
        changed.guard = guard;
        assert!(validate_entry(&root, Some(&changed)).is_err());
    }
    validate_entry(&root, Some(&request)).unwrap();
}

#[test]
fn installed_wire_refuses_hosted_fields_unknown_variant_and_file_grants() {
    let tuple = installed(&request());
    let value = serde_json::to_value(&tuple).unwrap();
    assert_eq!(
        serde_json::from_value::<LaunchTuple>(value.clone()).unwrap(),
        tuple
    );
    for field in ["generation", "service", "parent_launch_id", "grants"] {
        let mut changed = value.clone();
        changed["launch"][field] = serde_json::json!(1);
        assert!(
            serde_json::from_value::<LaunchTuple>(changed).is_err(),
            "{field}"
        );
    }
    let mut changed = value;
    changed["kind"] = serde_json::json!("FutureRoot");
    assert!(serde_json::from_value::<LaunchTuple>(changed).is_err());
    validate_offered_grants(&tuple, &[]).unwrap();
    let forbidden = [WireGrant {
        kind: ReadFileGrantKind::SpeakersAnalyzeGeneration,
        handle: 0, // scalar is refused before any handle adoption, never dereferenced
    }];
    assert!(validate_offered_grants(&tuple, &forbidden).is_err());
    assert!(
        serde_json::from_value::<WireGrant>(serde_json::json!({
            "kind": "FutureGeneration", "handle": 0
        }))
        .is_err()
    );
}

#[test]
fn guard_extraction_refuses_every_partial_set_and_non_unicode_value() {
    let complete = service_guard_environment(&request().guard);
    assert_eq!(
        guard_environment(|name| complete.get(name).map(OsString::from)).unwrap(),
        complete
    );
    assert!(guard_environment(|_| None).unwrap().is_empty());
    for mask in 1u8..15 {
        assert!(
            guard_environment(|name| {
                let index = GUARDS.iter().position(|key| *key == name).unwrap();
                (mask & (1 << index) != 0).then(|| OsString::from(&complete[name]))
            })
            .is_err(),
            "partial mask {mask}"
        );
    }
    for rejected in GUARDS {
        for invalid in [OsString::new(), OsString::from_wide(&[0xd800])] {
            assert!(
                guard_environment(|name| {
                    Some(if name == rejected {
                        invalid.clone()
                    } else {
                        OsString::from(&complete[name])
                    })
                })
                .is_err(),
                "{rejected}"
            );
        }
    }
}
