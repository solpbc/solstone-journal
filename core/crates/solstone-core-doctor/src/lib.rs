// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
pub mod args;
pub mod checks;
pub mod context;
pub mod output;
pub mod registry;
pub mod vocabulary;
use context::CheckContext;
use registry::{Battery, RegistryEntry};
use vocabulary::*;
pub fn run(args: &args::DoctorArgs, context: &CheckContext) -> Vec<CheckResult> {
    let battery = if args.readiness {
        Battery::JournalReadiness
    } else {
        Battery::Journal
    };
    let mut results = registry::entries(battery)
        .iter()
        .filter_map(|entry| run_entry(entry, context))
        .collect::<Vec<_>>();
    apply_conflict_policy(&mut results);
    results
}

#[cfg(test)]
mod doctor_check_behavior_tests;
fn run_entry(entry: &RegistryEntry, context: &CheckContext) -> Option<CheckResult> {
    if !entry.check.platforms.contains(&context.platform) {
        let mut r = make_result(
            entry.check,
            Status::Skip,
            format!("not supported on {}", context.platform.tag()),
            None::<String>,
        );
        r.platform = Some(context.platform.tag().into());
        return Some(r);
    }
    if let Some(set) = entry.deferred {
        return Some(make_result(
            entry.check,
            Status::Skip,
            format!("deferred check set: {set}"),
            None::<String>,
        ));
    }
    let mut r = run_check(entry.check, entry.runner, context);
    r.name = entry.check.name;
    r.severity = entry.check.severity;
    Some(r)
}
const POINTER: &str = "resolve the macOS supervisor conflict first: ";
const WARN_POINTER: &str =
    "resolve the macOS supervisor topology warning before changing the journal service";
const UNSAFE: &[&str] = &[
    "journal setup",
    "journal service install",
    "journal service start",
    "journal service restart",
];
fn apply_conflict_policy(results: &mut [CheckResult]) {
    let conflict = results
        .iter()
        .find(|r| r.name == "supervisor_conflict")
        .cloned();
    let Some(conflict) = conflict else { return };
    if !matches!(conflict.status, Status::Fail | Status::Warn) {
        return;
    }
    for result in results.iter_mut() {
        if result.name == "supervisor_conflict" || result.fix.is_none() {
            continue;
        }
        if conflict.status == Status::Fail && conflict.execution_error.is_none() {
            result.fix = Some(format!(
                "{POINTER}{}",
                conflict.fix.as_deref().unwrap_or_default()
            ))
        } else if result
            .fix
            .as_ref()
            .is_some_and(|f| UNSAFE.iter().any(|s| f.contains(s)))
        {
            result.fix = Some(WARN_POINTER.into())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(0);

    fn context() -> CheckContext {
        let root = std::env::temp_dir().join(format!(
            "solstone-doctor-test-{}-{}",
            std::process::id(),
            NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::create_dir_all(&root);
        CheckContext {
            home_dir: root.join("home"),
            install_bin_dir: root.join("install/bin"),
            journal_path: root.join("journal"),
            callosum_socket_path: root.join("journal/health/callosum.sock"),
            platform: Platform::Linux,
            now: chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            host_arch: "x86_64".into(),
            hostname: "test-host".into(),
            checkout_root: None,
            payload_root: None,
            port: 5015,
            service_status_timeout: Duration::from_millis(10),
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
            vad_runtime_probe: None,
            free_space_bytes_override: None,
        }
    }
    #[test]
    fn registry_ground_truth() {
        assert_eq!(registry::union_names().len(), 22);
        assert_eq!(
            registry::union_names()
                .iter()
                .filter(|name| registry::lookup(Battery::Journal, name)
                    .or_else(|| registry::lookup(Battery::JournalReadiness, name))
                    .is_some())
                .count(),
            22
        );
        assert_eq!(
            registry::union_names()
                .iter()
                .filter(|name| {
                    registry::lookup(Battery::Journal, name)
                        .or_else(|| registry::lookup(Battery::JournalReadiness, name))
                        .and_then(|entry| entry.deferred)
                        .is_some()
                })
                .count(),
            0
        )
    }
    #[test]
    fn ac2_doctor_usage_parse_error() {
        assert!(args::parse_doctor_args(&["--nonsense".into()]).is_err());
    }
    #[test]
    fn ac3_blocker_and_advisory_exit_matrix() {
        let check = Check {
            name: "x",
            severity: Severity::Advisory,
            platforms: &[Platform::Linux],
        };
        assert!(!results_failed(&[make_result(
            check,
            Status::Fail,
            "x",
            None::<String>
        )]));
    }
    #[test]
    fn ac4_execution_errors_are_failures() {
        let check = Check {
            name: "x",
            severity: Severity::Advisory,
            platforms: &[Platform::Linux],
        };
        let r = run_check(
            check,
            |_| {
                Err(ExecutionError {
                    kind: "Oops".into(),
                    message: "bad".into(),
                })
            },
            &context(),
        );
        assert!(results_failed(std::slice::from_ref(&r)));
        assert_eq!(status_label(&r), "ERROR");
    }
    #[test]
    fn ac5_json_shape_has_reference_keys() {
        let r = make_result(
            Check {
                name: "x",
                severity: Severity::Blocker,
                platforms: &[Platform::Linux],
            },
            Status::Ok,
            "ok",
            None::<String>,
        );
        let value = serde_json::to_value(&r).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 6);
    }
    #[test]
    fn ac6_jsonl_status_translation_and_event_shape() {
        let check = |name| Check {
            name,
            severity: Severity::Advisory,
            platforms: &[Platform::Linux],
        };
        let results = vec![
            make_result(check("ok"), Status::Ok, "ok", None::<String>),
            make_result(check("warn"), Status::Warn, "warn", None::<String>),
            make_result(check("fail"), Status::Fail, "fail", None::<String>),
            make_result(check("skip"), Status::Skip, "skip", None::<String>),
        ];
        let mut bytes = Vec::new();
        output::emit_jsonl_to(&mut bytes, &results, "2026-01-01T00:00:00Z", 7, 5015);
        let lines = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0]["event"], "doctor.started");
        for key in ["event", "ts", "started_at", "version", "port"] {
            assert!(lines[0].get(key).is_some());
        }
        assert_eq!(
            lines
                .iter()
                .skip(1)
                .take(4)
                .map(|line| line["status"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["ok", "warning", "failed", "skipped"]
        );
        assert!(
            lines
                .iter()
                .all(|line| line.get("event").is_some() && line.get("ts").is_some())
        );
        assert_eq!(lines[5]["event"], "doctor.completed");
    }
    #[test]
    fn ac7_advisory_statuses_match_setup_filters() {
        let r = make_result(
            Check {
                name: "x",
                severity: Severity::Advisory,
                platforms: &[Platform::Linux],
            },
            Status::Fail,
            "x",
            None::<String>,
        );
        assert_eq!(output::summary_status(&[r]), "warning");
    }
    #[test]
    fn ac8_conflict_policy_rewrites_fixes_and_execution_errors() {
        let mut r = vec![
            make_result(
                Check {
                    name: "supervisor_conflict",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                Some("foreign fix"),
            ),
            make_result(
                Check {
                    name: "x",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                Some("journal service start"),
            ),
            make_result(
                Check {
                    name: "no-fix",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                None::<String>,
            ),
        ];
        apply_conflict_policy(&mut r);
        assert_eq!(
            r[1].fix.as_deref(),
            Some("resolve the macOS supervisor conflict first: foreign fix")
        );
        assert!(r[2].fix.is_none());
        let mut execution_error = vec![
            make_result(
                Check {
                    name: "supervisor_conflict",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                Some("foreign fix"),
            ),
            make_result(
                Check {
                    name: "x",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                Some("journal service start"),
            ),
        ];
        execution_error[0].execution_error = Some(ExecutionError {
            kind: "Boom".into(),
            message: "x".into(),
        });
        apply_conflict_policy(&mut execution_error);
        assert_eq!(execution_error[1].fix.as_deref(), Some(WARN_POINTER));
    }
    #[test]
    fn ac9_platform_skip_precedes_runner() {
        let mut c = context();
        c.platform = Platform::Linux;
        let r = run_entry(
            &RegistryEntry {
                check: Check {
                    name: "x",
                    severity: Severity::Blocker,
                    platforms: &[Platform::Darwin],
                },
                runner: |_| panic!("runner"),
                deferred: None,
            },
            &c,
        )
        .unwrap();
        assert_eq!(r.status, Status::Skip);
    }

    #[test]
    fn windows_platform_is_an_explicit_skip_before_a_linux_runner() {
        let mut c = context();
        c.platform = Platform::Windows;
        let r = run_entry(
            &RegistryEntry {
                check: Check {
                    name: "x",
                    severity: Severity::Blocker,
                    platforms: &[Platform::Linux],
                },
                runner: |_| panic!("runner"),
                deferred: None,
            },
            &c,
        )
        .expect("result");
        assert_eq!(r.status, Status::Skip);
        assert_eq!(r.detail, "not supported on windows");
        assert_eq!(r.platform.as_deref(), Some("windows"));
    }

    #[test]
    fn ac10_conflict_policy_six_rows_including_real_foreign_fixture() {
        let mut c = context();
        c.platform = Platform::Darwin;
        fs::create_dir_all(c.home_dir.join("Library/LaunchAgents")).unwrap();
        fs::write(c.home_dir.join("Library/LaunchAgents/foreign.plist"), "<?xml version=\"1.0\"?><plist><dict><key>Label</key><string>example.foreign</string><key>KeepAlive</key><true/><key>ProgramArguments</key><array><string>/Applications/solstone.app/Contents/MacOS/solstone</string></array></dict></plist>").unwrap();
        let conflict = checks::supervisor_conflict::run(
            &c,
            Check {
                name: "supervisor_conflict",
                severity: Severity::Blocker,
                platforms: &[Platform::Darwin],
            },
        )
        .unwrap();
        assert!(conflict.fix.as_ref().unwrap().len() > "journal service uninstall".len());
        let mut r = vec![
            conflict,
            make_result(
                Check {
                    name: "x",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                Some("journal service start"),
            ),
            make_result(
                Check {
                    name: "none",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                None::<String>,
            ),
        ];
        apply_conflict_policy(&mut r);
        assert!(
            r[1].fix
                .as_ref()
                .unwrap()
                .contains("remove foreign launchers")
        );
        assert!(r[2].fix.is_none());
        let mut execution = vec![
            r[0].clone(),
            make_result(
                Check {
                    name: "x",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                Some("journal service start"),
            ),
        ];
        execution[0].execution_error = Some(ExecutionError {
            kind: "X".into(),
            message: "x".into(),
        });
        apply_conflict_policy(&mut execution);
        assert_eq!(execution[1].fix.as_deref(), Some(WARN_POINTER));
        let mut r = vec![
            make_result(
                Check {
                    name: "supervisor_conflict",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Warn,
                "x",
                None::<String>,
            ),
            make_result(
                Check {
                    name: "x",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                Some("journal service restart"),
            ),
            make_result(
                Check {
                    name: "safe",
                    severity: Severity::Blocker,
                    platforms: &[],
                },
                Status::Fail,
                "x",
                Some("read logs"),
            ),
        ];
        apply_conflict_policy(&mut r);
        assert_eq!(r[1].fix.as_deref(), Some(WARN_POINTER));
        assert_eq!(r[2].fix.as_deref(), Some("read logs"));
        for status in [Status::Ok, Status::Skip] {
            let mut rows = vec![
                make_result(
                    Check {
                        name: "supervisor_conflict",
                        severity: Severity::Blocker,
                        platforms: &[],
                    },
                    status,
                    "x",
                    None::<String>,
                ),
                make_result(
                    Check {
                        name: "x",
                        severity: Severity::Blocker,
                        platforms: &[],
                    },
                    Status::Fail,
                    "x",
                    Some("journal service start"),
                ),
            ];
            apply_conflict_policy(&mut rows);
            assert_eq!(rows[1].fix.as_deref(), Some("journal service start"));
        }
        let mut absent = vec![make_result(
            Check {
                name: "x",
                severity: Severity::Blocker,
                platforms: &[],
            },
            Status::Fail,
            "x",
            Some("journal service start"),
        )];
        apply_conflict_policy(&mut absent);
        assert_eq!(absent[0].fix.as_deref(), Some("journal service start"));
    }
    #[test]
    fn ac11_writability_battery_bindings_differ() {
        let c = context();
        let a = run_entry(
            registry::lookup(Battery::Journal, "journal_dir_writable").unwrap(),
            &c,
        )
        .unwrap();
        let b = run_entry(
            registry::lookup(Battery::JournalReadiness, "journal_dir_writable").unwrap(),
            &c,
        )
        .unwrap();
        assert_eq!(a.status, Status::Skip);
        assert_eq!(a.detail, "no local journal");
        assert_ne!(b.detail, "no local journal");
    }
    #[test]
    fn ac12_union_is_native_only_22_names() {
        assert_eq!(registry::union_names().len(), 22);
    }
    #[test]
    fn ac13_battery_is_read_only_for_missing_paths() {
        let c = context();
        let snapshot = |root: &std::path::Path| -> Vec<(String, Vec<u8>)> {
            fn walk(
                root: &std::path::Path,
                current: &std::path::Path,
                rows: &mut Vec<(String, Vec<u8>)>,
            ) {
                if let Ok(entries) = fs::read_dir(current) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let relative = path.strip_prefix(root).unwrap().display().to_string();
                        if path.is_dir() {
                            rows.push((format!("{relative}/"), Vec::new()));
                            walk(root, &path, rows);
                        } else {
                            rows.push((relative, fs::read(&path).unwrap_or_default()));
                        }
                    }
                }
            }
            let mut rows = Vec::new();
            walk(root, root, &mut rows);
            rows.sort();
            rows
        };
        fs::create_dir_all(&c.home_dir).unwrap();
        fs::create_dir_all(&c.journal_path).unwrap();
        fs::write(c.home_dir.join("kept"), b"home").unwrap();
        fs::write(c.journal_path.join("kept"), b"journal").unwrap();
        let before = (snapshot(&c.home_dir), snapshot(&c.journal_path));
        let _ = run(
            &args::DoctorArgs {
                verbose: false,
                json: false,
                jsonl: false,
                port: 5015,
                readiness: false,
            },
            &c,
        );
        let _ = run(
            &args::DoctorArgs {
                verbose: false,
                json: false,
                jsonl: false,
                port: 5015,
                readiness: true,
            },
            &c,
        );
        assert_eq!(before, (snapshot(&c.home_dir), snapshot(&c.journal_path)));
    }
    #[test]
    fn ac17_every_stub_names_its_wave() {
        let c = context();
        for battery in [Battery::Journal, Battery::JournalReadiness] {
            for entry in registry::entries(battery)
                .iter()
                .filter(|e| e.deferred.is_some())
            {
                let r = run_entry(entry, &c).unwrap();
                assert_eq!(r.status, Status::Skip);
                assert!(r.detail.contains(&entry.deferred.unwrap().to_string()));
            }
        }
    }
    #[test]
    fn ac15_real_checks_have_ok_and_non_ok_paths() {
        let mut c = context();
        let config = Check {
            name: "config_dir_readable",
            severity: Severity::Blocker,
            platforms: &[Platform::Linux],
        };
        assert_eq!(
            checks::config_dir_readable::run(&c, config).unwrap().status,
            Status::Fail
        );
        fs::create_dir_all(c.home_dir.join(".config")).unwrap();
        assert_eq!(
            checks::config_dir_readable::run(&c, config).unwrap().status,
            Status::Ok
        );
        let writable = Check {
            name: "journal_dir_writable",
            severity: Severity::Blocker,
            platforms: &[Platform::Linux],
        };
        fs::create_dir_all(&c.journal_path).unwrap();
        assert_eq!(
            checks::journal_dir_writable::shared(&c, writable)
                .unwrap()
                .status,
            Status::Ok
        );
        fs::remove_dir_all(&c.journal_path).unwrap();
        fs::write(&c.journal_path, b"file").unwrap();
        assert_eq!(
            checks::journal_dir_writable::shared(&c, writable)
                .unwrap()
                .status,
            Status::Fail
        );
        c.platform = Platform::Darwin;
        fs::create_dir_all(c.home_dir.join("Library/LaunchAgents")).unwrap();
        let conflict = Check {
            name: "supervisor_conflict",
            severity: Severity::Blocker,
            platforms: &[Platform::Darwin],
        };
        assert_eq!(
            checks::supervisor_conflict::run(&c, conflict)
                .unwrap()
                .status,
            Status::Ok
        );
        fs::write(c.home_dir.join("Library/LaunchAgents/foreign.plist"),"<plist><dict><key>Label</key><string>foreign</string><key>KeepAlive</key><true/><key>ProgramArguments</key><array><string>/Applications/solstone.app/x</string></array></dict></plist>").unwrap();
        assert_eq!(
            checks::supervisor_conflict::run(&c, conflict)
                .unwrap()
                .status,
            Status::Fail
        );
        fs::remove_file(c.home_dir.join("Library/LaunchAgents/foreign.plist")).unwrap();
        fs::write(c.home_dir.join("Library/LaunchAgents/org.solpbc.solstone.plist"),"<plist version=\"1.0\"><dict><key>ProgramArguments</key><array><string>/bin/sh</string></array></dict></plist>").unwrap();
        let plist = Check {
            name: "launchd_stale_plist",
            severity: Severity::Advisory,
            platforms: &[Platform::Darwin],
        };
        assert_eq!(
            checks::launchd_stale_plist::run(&c, plist).unwrap().status,
            Status::Ok
        );
        fs::write(
            c.home_dir
                .join("Library/LaunchAgents/org.solpbc.solstone.plist"),
            b"bad",
        )
        .unwrap();
        assert_eq!(
            checks::launchd_stale_plist::run(&c, plist).unwrap().status,
            Status::Fail
        );
    }
    #[test]
    fn supervisor_conflict_detects_binary_foreign_launcher_plist() {
        let mut c = context();
        c.platform = Platform::Darwin;
        let directory = c.home_dir.join("Library/LaunchAgents");
        fs::create_dir_all(&directory).unwrap();
        let mut data = plist::Dictionary::new();
        data.insert(
            "Label".into(),
            plist::Value::String("example.foreign".into()),
        );
        data.insert("KeepAlive".into(), plist::Value::Boolean(true));
        data.insert(
            "ProgramArguments".into(),
            plist::Value::Array(vec![plist::Value::String(
                "/Applications/solstone.app/Contents/MacOS/solstone".into(),
            )]),
        );
        plist::Value::Dictionary(data)
            .to_file_binary(directory.join("foreign.plist"))
            .unwrap();
        let result = checks::supervisor_conflict::run(
            &c,
            Check {
                name: "supervisor_conflict",
                severity: Severity::Blocker,
                platforms: &[Platform::Darwin],
            },
        )
        .unwrap();
        assert_eq!(result.status, Status::Fail);
    }
}
