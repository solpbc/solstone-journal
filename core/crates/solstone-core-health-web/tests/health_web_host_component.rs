// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[test]
fn ac16_poisoned_python_is_observable_only_under_ci() {
    assert_eq!(
        std::env::var("SOLSTONE_CI_POISONED").as_deref(),
        Ok("1"),
        "set SOLSTONE_CI_POISONED=1 to run this host-tool check"
    );
    let output = std::process::Command::new("python3")
        .output()
        .expect("poison shim starts");
    assert_eq!(output.status.code(), Some(97));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Rust gate invoked a forbidden interpreter:")
    );
}
