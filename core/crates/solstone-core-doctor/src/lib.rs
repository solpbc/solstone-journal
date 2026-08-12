// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
pub mod args;
pub mod checks;
pub mod context;
pub mod features;
pub mod output;
pub mod registry;
pub mod vocabulary;
use context::CheckContext;
use registry::{Battery, RegistryEntry};
use vocabulary::*;
pub fn run(args: &args::DoctorArgs, context: &CheckContext) -> Vec<CheckResult> {
    if let Some(feature) = &args.feature {
        return run_entry(
            registry::lookup(Battery::Journal, &format!("feature:{feature}"))
                .expect("validated feature"),
            context,
        )
        .into_iter()
        .collect();
    }
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
mod w3c_tests;
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
    if let Some(wave) = entry.deferred {
        return Some(make_result(
            entry.check,
            Status::Skip,
            format!("deferred to wave {wave}"),
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
        os::unix::fs::{PermissionsExt, symlink},
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(0);
    const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s\n' "$0" > "$POISON_MARKER"
exit 97
"#;

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
            machine_id: Some("test-machine".into()),
            checkout_root: None,
            python_env_root: None,
            port: 5015,
            service_status_timeout: Duration::from_millis(10),
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
        }
    }
    #[test]
    fn registry_ground_truth() {
        assert_eq!(registry::union_names().len(), 31);
        assert_eq!(
            registry::union_names()
                .iter()
                .filter(|name| registry::lookup(Battery::Journal, name)
                    .or_else(|| registry::lookup(Battery::JournalReadiness, name))
                    .is_some())
                .count(),
            31
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
    fn ac1_feature_grammar_and_override() {
        assert!(
            args::parse_doctor_args(&[
                "--feature".into(),
                "pdf-import".into(),
                "--readiness".into()
            ])
            .is_ok()
        );
        assert!(
            args::parse_doctor_args(&["--feature".into(), "nope".into()])
                .unwrap_err()
                .0
                .contains("pdf-export, pdf-import")
        );
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
        output::emit_jsonl_to(&mut bytes, &results, "2026-01-01T00:00:00Z", 7, 5015, None);
        let lines = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0]["event"], "doctor.started");
        for key in ["event", "ts", "started_at", "version", "port", "feature"] {
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
                feature: None,
            },
            &c,
        )
        .unwrap();
        assert_eq!(r.status, Status::Skip);
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
    fn ac12_union_is_native_only_31_names() {
        assert_eq!(registry::union_names().len(), 31);
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
                feature: None,
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
                feature: None,
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
    fn ac15_service_running_receives_ok_and_crash_status_over_callosum() {
        use serde_json::{Map, Value};
        use solstone_core_callosum::{CallosumEnvelope, CallosumSocketServer};
        let mut c = context();
        fs::create_dir_all(c.home_dir.join(".config/systemd/user")).unwrap();
        fs::write(
            c.home_dir.join(".config/systemd/user/solstone.service"),
            b"x",
        )
        .unwrap();
        fs::create_dir_all(c.callosum_socket_path.parent().unwrap()).unwrap();
        c.service_status_timeout = Duration::from_millis(250);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(CallosumSocketServer::bind(&c.callosum_socket_path))
            .unwrap();
        let check = Check {
            name: "service_running",
            severity: Severity::Blocker,
            platforms: &[Platform::Linux],
        };
        let first = c.clone();
        let handle =
            std::thread::spawn(move || checks::service_running::run(&first, check).unwrap());
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_millis(100), async {
                while server.client_count() == 0 {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
        });
        let envelope = CallosumEnvelope {
            tract: "supervisor".into(),
            event: "status".into(),
            ts: None,
            extra: Map::from_iter([("crashed".into(), Value::Array(vec![]))]),
        };
        for _ in 0..20 {
            assert!(server.broadcast(envelope.clone()));
            runtime.block_on(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
            });
            if handle.is_finished() {
                break;
            }
        }
        let ok = handle.join().unwrap();
        assert_eq!(ok.status, Status::Ok);
        assert_eq!(ok.detail, "journal service is running");
        let second = c.clone();
        let handle =
            std::thread::spawn(move || checks::service_running::run(&second, check).unwrap());
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_millis(100), async {
                while server.client_count() == 0 {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
        });
        let envelope = CallosumEnvelope {
            tract: "supervisor".into(),
            event: "status".into(),
            ts: None,
            extra: Map::from_iter([(
                "crashed".into(),
                Value::Array(vec![serde_json::json!({"name":"foo","restart_attempts":3})]),
            )]),
        };
        for _ in 0..20 {
            assert!(server.broadcast(envelope.clone()));
            runtime.block_on(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
            });
            if handle.is_finished() {
                break;
            }
        }
        let crash = handle.join().unwrap();
        assert_eq!(crash.status, Status::Fail);
        assert!(
            crash
                .detail
                .contains("crash-loop: foo (3 restart attempts)")
        );
        assert_eq!(crash.fix.as_deref(), Some("run journal service logs"));
        runtime.block_on(server.stop());
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
    #[test]
    fn ac18_silent_present_socket_warns_without_execution_error() {
        use std::os::unix::net::UnixListener;
        let mut c = context();
        fs::create_dir_all(c.home_dir.join(".config/systemd/user")).unwrap();
        fs::write(
            c.home_dir.join(".config/systemd/user/solstone.service"),
            b"x",
        )
        .unwrap();
        fs::create_dir_all(c.callosum_socket_path.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&c.callosum_socket_path).unwrap();
        c.service_status_timeout = Duration::from_millis(50);
        let handle = std::thread::spawn(move || {
            let _ = listener.accept();
            std::thread::sleep(Duration::from_millis(100));
        });
        let r = checks::service_running::run(
            &c,
            Check {
                name: "service_running",
                severity: Severity::Blocker,
                platforms: &[Platform::Linux],
            },
        )
        .unwrap();
        handle.join().unwrap();
        assert_eq!(r.status, Status::Warn);
        assert_eq!(r.detail, "service installed but not running");
        assert!(r.execution_error.is_none());
        assert!(!results_failed(&[r]));
    }
    #[test]
    fn ac18_silent_present_socket_with_failed_service_command_fails() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let mut c = context();
        fs::create_dir_all(c.home_dir.join(".config/systemd/user")).unwrap();
        fs::write(
            c.home_dir.join(".config/systemd/user/solstone.service"),
            b"x",
        )
        .unwrap();
        fs::create_dir_all(c.callosum_socket_path.parent().unwrap()).unwrap();
        let script = c.home_dir.parent().unwrap().join("failed-service.sh");
        fs::write(&script, "#!/bin/sh\necho failed\nexit 0\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        c.service_status_command_override = Some((script, Vec::new()));
        c.service_status_timeout = Duration::from_millis(50);
        let listener = UnixListener::bind(&c.callosum_socket_path).unwrap();
        let handle = std::thread::spawn(move || {
            let _ = listener.accept();
            std::thread::sleep(Duration::from_millis(100));
        });
        let r = checks::service_running::run(
            &c,
            Check {
                name: "service_running",
                severity: Severity::Blocker,
                platforms: &[Platform::Linux],
            },
        )
        .unwrap();
        handle.join().unwrap();
        assert_eq!(r.status, Status::Fail);
        assert_eq!(r.detail, "journal service unit is failed");
        assert_eq!(
            r.fix.as_deref(),
            Some("run journal service restart; if it persists, run journal service logs")
        );
        assert!(r.execution_error.is_none());
    }

    #[test]
    fn ac9_full_batteries_never_invoke_poisoned_interpreters() {
        if let Some(root) = std::env::var_os("SOLSTONE_DOCTOR_AC9_ROOT") {
            run_ac9_child(std::path::PathBuf::from(root));
            return;
        }

        let staged = checks::test_support::context();
        let root = staged
            .install_bin_dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("staged root")
            .to_path_buf();
        let poison_dir = root.join("poison");
        let marker = root.join("poison-marker");
        fs::create_dir_all(&poison_dir).expect("create poison directory");
        for name in ["python", "python3", "pip", "uv"] {
            let shim = poison_dir.join(name);
            fs::write(&shim, POISON_INTERPRETER).expect("write poison PATH shim");
            fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
                .expect("make poison PATH shim executable");
        }
        stage_ac9_batteries(&staged.context);

        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "tests::ac9_full_batteries_never_invoke_poisoned_interpreters",
            ])
            .env("SOLSTONE_DOCTOR_AC9_ROOT", &root)
            .env("POISON_MARKER", &marker)
            .env("PATH", &poison_dir)
            .output()
            .expect("run isolated poison-interpreter child");
        assert!(
            output.status.success(),
            "child test failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !marker.exists(),
            "native doctor invoked a poison interpreter: {}",
            fs::read_to_string(&marker).unwrap_or_default()
        );
    }

    fn run_ac9_child(root: std::path::PathBuf) {
        let context = CheckContext {
            home_dir: root.join("home"),
            install_bin_dir: root.join("install/bin"),
            journal_path: root.join("journal"),
            callosum_socket_path: root.join("journal/health/callosum.sock"),
            platform: Platform::Linux,
            now: chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            host_arch: "x86_64".into(),
            hostname: "test-host".into(),
            machine_id: Some("test-machine".into()),
            checkout_root: None,
            python_env_root: None,
            port: 5015,
            service_status_timeout: Duration::from_millis(10),
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
        };
        for readiness in [false, true] {
            let results = run(
                &args::DoctorArgs {
                    verbose: false,
                    json: false,
                    jsonl: false,
                    port: 5015,
                    feature: None,
                    readiness,
                },
                &context,
            );
            assert_eq!(
                results.len(),
                registry::entries(if readiness {
                    Battery::JournalReadiness
                } else {
                    Battery::Journal
                })
                .len(),
                "entire battery must run"
            );
            assert!(
                results
                    .iter()
                    .all(|result| result.execution_error.is_none()),
                "a check failed before the battery completed: {results:?}"
            );
        }
    }

    fn stage_ac9_batteries(context: &CheckContext) {
        fs::create_dir_all(&context.home_dir).expect("create staged home");
        fs::create_dir_all(&context.journal_path).expect("create staged journal");
        fs::create_dir_all(&context.install_bin_dir).expect("create staged install bin");
        let site_packages = checks::test_support::site_packages(context, "python3.12");
        checks::test_support::metadata(
            &site_packages,
            "solstone-1.2.3.dist-info",
            "solstone",
            "1.2.3",
            Some(">=3.12"),
        );
        checks::test_support::metadata(
            &site_packages,
            "solstone_journal-1.2.3.dist-info",
            "solstone-journal",
            "1.2.3",
            None,
        );
        for module in ["frontmatter", "flask", "onnxruntime"] {
            fs::create_dir(site_packages.join(module)).expect("create host dependency module");
        }
        fs::write(
            context
                .install_bin_dir
                .parent()
                .expect("install prefix")
                .join("pyvenv.cfg"),
            "version = 3.12.0\n",
        )
        .expect("write staged pyvenv config");
        for binary in ["sol", "journal", "python"] {
            let path = context.install_bin_dir.join(binary);
            fs::write(&path, POISON_INTERPRETER).expect("write staged executable");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("make staged executable");
        }
        let aliases = context.home_dir.join(".local/bin");
        fs::create_dir_all(&aliases).expect("create staged aliases");
        symlink(context.install_bin_dir.join("sol"), aliases.join("sol")).expect("link staged sol");
        symlink(
            context.install_bin_dir.join("journal"),
            aliases.join("journal"),
        )
        .expect("link staged journal");
        let unit = context
            .home_dir
            .join(".config/systemd/user/solstone.service");
        fs::create_dir_all(unit.parent().expect("unit parent")).expect("create unit parent");
        fs::write(
            unit,
            format!(
                "ExecStart={} start 5015\n",
                context.install_bin_dir.join("journal").display()
            ),
        )
        .expect("write staged service unit");
    }
}
