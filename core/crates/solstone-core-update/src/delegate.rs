// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::ownership::{RecordedComponent, ServicePolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinMode {
    Closed,
    Inherit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait InstallerRunner: Send + Sync {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        stdin: StdinMode,
    ) -> std::io::Result<InstallerOutput>;
}

pub struct RealInstallerRunner;

impl InstallerRunner for RealInstallerRunner {
    fn run(
        &self,
        program: &Path,
        args: &[OsString],
        stdin: StdinMode,
    ) -> std::io::Result<InstallerOutput> {
        // Catalogue checks have a deadline. Approved installations remain in
        // the foreground until their owner exits; timing out a privileged
        // package manager could return while it is still changing the system.
        let limit = if args.iter().any(|a| a == OsStr::new("--list")) {
            // latest, catalogue and signature each allow three 120-second
            // stalled attempts plus retry delays. Leave room for that normal
            // retry path; redirects and a trickling server remain bounded.
            Some(Duration::from_secs(20 * 60))
        } else {
            None
        };
        run_installer(program, args, stdin, limit)
    }
}

fn drain_output(mut input: impl Read, echo: bool) -> std::io::Result<Vec<u8>> {
    const LIMIT: usize = 1024 * 1024;
    let mut captured = Vec::new();
    let mut buf = [0; 8192];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if echo {
            let mut stderr = std::io::stderr().lock();
            stderr.write_all(&buf[..n])?;
            stderr.flush()?;
        }
        let keep = n.min(LIMIT.saturating_sub(captured.len()));
        captured.extend_from_slice(&buf[..keep]);
    }
    Ok(captured)
}

// Keep terminal job control intact for consent, and terminate only descendants
// of the child we started if its deadline expires.
#[cfg(target_os = "linux")]
fn terminate_descendants(pid: u32) {
    // Stop each parent before killing its descendants so a failed child cannot
    // release the shell to run the next command while teardown is in progress.
    let _ = Command::new("/bin/kill")
        .args(["-STOP", "--", &pid.to_string()])
        .status();
    let children =
        std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).unwrap_or_default();
    for child in children
        .split_whitespace()
        .filter_map(|v| v.parse::<u32>().ok())
    {
        terminate_descendants(child);
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &child.to_string()])
            .status();
    }
}

fn run_installer(
    program: &Path,
    args: &[OsString],
    stdin: StdinMode,
    timeout: Option<Duration>,
) -> std::io::Result<InstallerOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(match stdin {
            StdinMode::Closed => Stdio::null(),
            StdinMode::Inherit => Stdio::inherit(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let output_thread = thread::spawn(move || drain_output(stdout, false));
    // Prompts and the exact package command must reach the terminal before the
    // installer waits for approval. Structured stdout remains captured.
    let error_thread = thread::spawn(move || drain_output(stderr, true));
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            #[cfg(target_os = "linux")]
            terminate_descendants(child.id());
            child.kill()?;
            break (child.wait()?, true);
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = output_thread
        .join()
        .map_err(|_| std::io::Error::other("installer output reader failed"))??;
    let stderr = error_thread
        .join()
        .map_err(|_| std::io::Error::other("installer diagnostic reader failed"))??;
    if timed_out {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "installer catalogue deadline exceeded",
        ));
    }
    Ok(InstallerOutput {
        success: status.success() && !timed_out,
        exit_code: status.code(),
        stdout,
        stderr,
    })
}

pub fn validate_safe_arg(arg: &str) -> Result<(), String> {
    if arg.contains('\n') || arg.contains('\r') || arg.contains('\0') {
        return Err(format!(
            "argument contains invalid newline or null: {arg:?}"
        ));
    }
    if arg.contains('`')
        || arg.contains('$')
        || arg.contains(';')
        || arg.contains('|')
        || arg.contains('&')
    {
        return Err(format!("argument contains shell metacharacters: {arg:?}"));
    }
    Ok(())
}

pub fn build_list_argv(
    script: &Path,
    lane: &str,
    version: Option<&str>,
) -> Result<Vec<OsString>, String> {
    validate_safe_arg(lane)?;
    if let Some(v) = version {
        validate_safe_arg(v)?;
    }
    let script_str = script
        .to_str()
        .ok_or_else(|| "script path is not valid utf-8".to_owned())?;
    validate_safe_arg(script_str)?;

    let mut args = vec![
        OsString::from(script),
        OsString::from("--list"),
        OsString::from("--json"),
        OsString::from("--lane"),
        OsString::from(lane),
    ];
    if let Some(v) = version {
        args.push(OsString::from("--version"));
        args.push(OsString::from(v));
    }
    args.push(OsString::from("--non-interactive"));
    Ok(args)
}

pub fn build_check_argv(script: &Path, lane: &str) -> Result<Vec<OsString>, String> {
    build_list_argv(script, lane, None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformInvocation {
    pub args: Vec<OsString>,
    pub stdin_mode: StdinMode,
    pub components: Vec<String>,
}

pub fn build_platform_mutation_invocations(
    script: &Path,
    components: &[RecordedComponent],
    prefix: Option<&Path>,
    verified_platform_version: Option<&str>,
    lane: &str,
    yes: bool,
) -> Result<Vec<PlatformInvocation>, String> {
    validate_safe_arg(lane)?;
    if let Some(ver) = verified_platform_version {
        validate_safe_arg(ver)?;
    }

    let script_str = script
        .to_str()
        .ok_or_else(|| "script path is not valid utf-8".to_owned())?;
    validate_safe_arg(script_str)?;

    if let Some(p) = prefix {
        let p_str = p
            .to_str()
            .ok_or_else(|| "prefix path is not valid utf-8".to_owned())?;
        validate_safe_arg(p_str)?;
    }

    // Split components into Start and NoStart groups
    let mut start_comps = Vec::new();
    let mut nostart_comps = Vec::new();

    for comp in components {
        validate_safe_arg(&comp.name)?;
        match comp.policy {
            ServicePolicy::Start => start_comps.push(comp),
            ServicePolicy::NoStart => nostart_comps.push(comp),
        }
    }

    let mut invocations = Vec::new();

    for (group_comps, is_nostart) in [(&start_comps, false), (&nostart_comps, true)] {
        if group_comps.is_empty() {
            continue;
        }

        let comp_names: Vec<String> = group_comps.iter().map(|c| c.name.clone()).collect();
        let comps_arg = comp_names.join(",");

        let mut argv = vec![
            OsString::from(script),
            OsString::from("--upgrade"),
            OsString::from("--json"),
            OsString::from("--no-path"),
            OsString::from("--confirm-package-command"),
            OsString::from("--lane"),
            OsString::from(lane),
            OsString::from("--components"),
            OsString::from(comps_arg),
        ];

        let has_tree = group_comps.iter().any(|c| c.route == "tree");
        if has_tree && let Some(p) = prefix {
            argv.push(OsString::from("--prefix"));
            argv.push(OsString::from(p));
        }

        if let Some(ver) = verified_platform_version {
            argv.push(OsString::from("--version"));
            argv.push(OsString::from(ver));
        }

        if is_nostart {
            argv.push(OsString::from("--no-start"));
        }

        let stdin_mode = if yes {
            argv.push(OsString::from("--yes"));
            StdinMode::Closed
        } else {
            StdinMode::Inherit
        };

        invocations.push(PlatformInvocation {
            args: argv,
            stdin_mode,
            components: comp_names,
        });
    }

    Ok(invocations)
}

pub fn build_native_mutation_argv(
    script: &Path,
    prefix: &Path,
    version: &str,
    lane: &str,
    role: &str,
    policy: ServicePolicy,
) -> Result<Vec<OsString>, String> {
    validate_safe_arg(lane)?;
    validate_safe_arg(role)?;
    validate_safe_arg(version)?;

    if version.contains("-r") {
        return Err(
            "native bootstrap version must be MAJOR.MINOR.PATCH, never a platform revision"
                .to_owned(),
        );
    }

    let script_str = script
        .to_str()
        .ok_or_else(|| "script path is not valid utf-8".to_owned())?;
    validate_safe_arg(script_str)?;

    let p_str = prefix
        .to_str()
        .ok_or_else(|| "prefix path is not valid utf-8".to_owned())?;
    validate_safe_arg(p_str)?;

    let mut argv = vec![
        OsString::from(script),
        OsString::from("--upgrade"),
        OsString::from("--prefix"),
        OsString::from(prefix),
        OsString::from("--version"),
        OsString::from(version),
        OsString::from("--lane"),
        OsString::from(lane),
        OsString::from("--no-path"),
        OsString::from("--role"),
        OsString::from(role),
    ];

    if policy == ServicePolicy::NoStart {
        argv.push(OsString::from("--no-start"));
    }

    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_builder_rejects_shell_metacharacters_and_newlines() {
        let script = Path::new("/var/tmp/install.sh");
        let prefix = Path::new("/var/tmp/prefix\nwith\nnewline");
        let comps = vec![RecordedComponent {
            name: "journal".to_owned(),
            route: "tree".to_owned(),
            policy: ServicePolicy::Start,
            version: Some("2.0.48".to_owned()),
        }];

        let res = build_platform_mutation_invocations(
            script,
            &comps,
            Some(prefix),
            Some("2.0.48-r1"),
            "release",
            true,
        );
        assert!(res.is_err(), "expected newline in prefix to be rejected");

        let bad_prefix = Path::new("/var/tmp/prefix;rm -rf /");
        let res2 = build_platform_mutation_invocations(
            script,
            &comps,
            Some(bad_prefix),
            Some("2.0.48-r1"),
            "release",
            true,
        );
        assert!(res2.is_err(), "expected semicolon in prefix to be rejected");
    }

    #[test]
    fn argv_builder_splits_differing_service_policies_into_separate_invocations() {
        let script = Path::new("/var/tmp/install.sh");
        let prefix = Path::new("/var/tmp/prefix");
        let comps = vec![
            RecordedComponent {
                name: "journal".to_owned(),
                route: "tree".to_owned(),
                policy: ServicePolicy::Start,
                version: Some("2.0.48".to_owned()),
            },
            RecordedComponent {
                name: "desktop".to_owned(),
                route: "tree".to_owned(),
                policy: ServicePolicy::NoStart,
                version: Some("2.0.48".to_owned()),
            },
        ];

        let invocations = build_platform_mutation_invocations(
            script,
            &comps,
            Some(prefix),
            Some("2.0.48-r2"),
            "release",
            true,
        )
        .unwrap();

        assert_eq!(invocations.len(), 2);

        // Group 1: start
        let inv1 = &invocations[0];
        assert_eq!(inv1.components, vec!["journal"]);
        assert!(
            inv1.args
                .contains(&OsString::from("--confirm-package-command"))
        );
        assert!(!inv1.args.contains(&OsString::from("--no-start")));
        assert!(inv1.args.contains(&OsString::from("--yes")));

        // Group 2: nostart
        let inv2 = &invocations[1];
        assert_eq!(inv2.components, vec!["desktop"]);
        assert!(
            inv2.args
                .contains(&OsString::from("--confirm-package-command"))
        );
        assert!(inv2.args.contains(&OsString::from("--no-start")));
        assert!(inv2.args.contains(&OsString::from("--yes")));
    }

    #[test]
    fn argv_builder_appends_confirm_package_command_to_all_platform_mutations() {
        let script = Path::new("/var/tmp/install.sh");
        let comps = vec![RecordedComponent {
            name: "journal".to_owned(),
            route: "deb".to_owned(),
            policy: ServicePolicy::Start,
            version: Some("2.0.48".to_owned()),
        }];

        let invocations = build_platform_mutation_invocations(
            script,
            &comps,
            None,
            Some("2.0.48-r1"),
            "release",
            false,
        )
        .unwrap();

        assert_eq!(invocations.len(), 1);
        assert!(
            invocations[0]
                .args
                .contains(&OsString::from("--confirm-package-command"))
        );
        // package only -> no --prefix
        assert!(!invocations[0].args.contains(&OsString::from("--prefix")));
    }

    #[test]
    fn argv_builder_omits_skip_signature_and_origin_flags() {
        let script = Path::new("/var/tmp/install.sh");
        let comps = vec![RecordedComponent {
            name: "journal".to_owned(),
            route: "tree".to_owned(),
            policy: ServicePolicy::Start,
            version: Some("2.0.48".to_owned()),
        }];

        let invocations = build_platform_mutation_invocations(
            script,
            &comps,
            Some(Path::new("/var/tmp/prefix")),
            Some("2.0.48-r1"),
            "release",
            true,
        )
        .unwrap();

        for arg in &invocations[0].args {
            let s = arg.to_string_lossy();
            assert_ne!(s, "--skip-signature");
            assert_ne!(s, "--origin");
            assert_ne!(s, "--dry-run");
            assert_ne!(s, "--route");
        }
    }

    #[test]
    fn argv_builder_sets_yes_only_when_explicitly_requested() {
        let script = Path::new("/var/tmp/install.sh");
        let comps = vec![RecordedComponent {
            name: "journal".to_owned(),
            route: "deb".to_owned(),
            policy: ServicePolicy::Start,
            version: Some("2.0.48".to_owned()),
        }];

        // yes = false
        let inv_no_yes = build_platform_mutation_invocations(
            script,
            &comps,
            None,
            Some("2.0.48-r1"),
            "release",
            false,
        )
        .unwrap();
        assert!(!inv_no_yes[0].args.contains(&OsString::from("--yes")));
        assert_eq!(inv_no_yes[0].stdin_mode, StdinMode::Inherit);

        // yes = true
        let inv_yes = build_platform_mutation_invocations(
            script,
            &comps,
            None,
            Some("2.0.48-r1"),
            "release",
            true,
        )
        .unwrap();
        assert!(inv_yes[0].args.contains(&OsString::from("--yes")));
        assert_eq!(inv_yes[0].stdin_mode, StdinMode::Closed);
    }

    #[test]
    fn native_version_is_the_journal_component_version_never_platform_revision() {
        let script = Path::new("/var/tmp/install.sh");
        let prefix = Path::new("/var/tmp/prefix");

        let err = build_native_mutation_argv(
            script,
            prefix,
            "2.0.48-r1",
            "release",
            "journal",
            ServicePolicy::Start,
        )
        .unwrap_err();
        assert!(err.contains("never a platform revision"));

        let ok = build_native_mutation_argv(
            script,
            prefix,
            "2.0.48",
            "release",
            "journal",
            ServicePolicy::Start,
        )
        .unwrap();
        assert!(ok.contains(&OsString::from("2.0.48")));
        let ver_idx = ok.iter().position(|a| a == "--version").unwrap();
        assert_eq!(ok[ver_idx + 1], "2.0.48");
        assert!(!ok[ver_idx + 1].to_string_lossy().contains("-r"));
    }

    #[test]
    fn newline_in_a_prefix_is_not_spawned() {
        let bad_prefix = Path::new("/tmp/test\nnewline");
        let err = build_native_mutation_argv(
            Path::new("/tmp/install.sh"),
            bad_prefix,
            "2.0.48",
            "release",
            "journal",
            ServicePolicy::Start,
        )
        .unwrap_err();
        assert!(err.contains("newline"));
    }
}

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod process_tests {
    use super::*;
    #[test]
    fn catalogue_can_finish_after_the_old_process_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("installer.sh");
        std::fs::write(
            &script,
            "sleep 31\nprintf '{\"platform_version\":\"2.0.48-r1\"}\\n'\n",
        )
        .unwrap();
        let args = build_list_argv(&script, "release", None).unwrap();
        let output = RealInstallerRunner
            .run(Path::new("/bin/sh"), &args, StdinMode::Closed)
            .unwrap();
        assert!(output.success, "{output:?}");
        assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok());
    }
    #[test]
    fn real_shell_receives_installer_flags_and_captures_one_json_result() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("installer.sh");
        std::fs::write(
            &script,
            r#"case "$1" in --list) printf '{"platform_version":"2.0.48-r1"}\n';; *) exit 17;; esac
"#,
        )
        .unwrap();
        let args = build_list_argv(&script, "release", None).unwrap();
        let output = RealInstallerRunner
            .run(Path::new("/bin/sh"), &args, StdinMode::Closed)
            .unwrap();
        assert!(output.success, "{output:?}");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["platform_version"],
            "2.0.48-r1"
        );
    }
    #[test]
    fn timeout_reaps_child_and_stops_its_descendant_before_returning() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("installer.sh");
        let marker = dir.path().join("late-write");
        std::fs::write(&script, format!("sleep 1; touch '{}'\n", marker.display())).unwrap();
        let args = vec![script.into_os_string()];
        let started = Instant::now();
        let error = run_installer(
            Path::new("/bin/sh"),
            &args,
            StdinMode::Closed,
            Some(Duration::from_millis(50)),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(800));
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!marker.exists());
    }
}
