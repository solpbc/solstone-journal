// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(windows)]
mod windows_home_tests {
    use solstone_core_doctor::context::CheckContext;
    use std::{env, path::PathBuf};
    #[test]
    fn production_context_uses_the_same_home_as_journal_entry() {
        const CASE: &str = "SOLSTONE_DOCTOR_HOME_TEST_CASE";
        if let Ok(case) = env::var(CASE) {
            let shared = solstone_core_journal::discover_current_home();
            let doctor = CheckContext::production(5015);
            if case == "invalid" {
                assert!(shared.is_err());
                assert!(doctor.is_err());
            } else {
                let expected = PathBuf::from(env::var_os("SOLSTONE_DOCTOR_EXPECTED_HOME").unwrap());
                assert_eq!(shared.unwrap(), expected);
                assert_eq!(doctor.unwrap().home_dir, expected);
            }
            return;
        }
        let profile = env::temp_dir().join("doctor-native-profile-😀");
        let explicit = env::temp_dir().join("doctor-explicit-home-😀");
        for case in ["profile", "explicit", "invalid"] {
            let mut child = std::process::Command::new(env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "windows_home_tests::production_context_uses_the_same_home_as_journal_entry",
                    "--nocapture",
                ])
                .env(CASE, case)
                .env("USERPROFILE", &profile)
                .env_remove("HOME")
                .env_remove("SOLSTONE_JOURNAL")
                .env(
                    "SOLSTONE_DOCTOR_EXPECTED_HOME",
                    if case == "explicit" {
                        &explicit
                    } else {
                        &profile
                    },
                );
            if case == "explicit" {
                child.env("HOME", &explicit);
            }
            if case == "invalid" {
                child.env("HOME", "~invalid");
            }
            let output = child.output().unwrap();
            assert!(output.status.success(), "{case}: {:?}", output);
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
    }
}
