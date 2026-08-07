// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs, path::Path, path::PathBuf};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    env::temp_dir().join(format!("solstone-core-{name}-{stamp}"))
}

fn identity_arg(public_argv0: &str) -> String {
    format!("__solstone_identity={public_argv0}")
}

/// Run `sol root` through a `solstone-core` binary this test just copied into
/// place.
///
/// `fs::copy` closes its own descriptors, but the test harness is
/// multi-threaded: any thread that forks while the copy's write descriptor is
/// still open hands the child an inherited copy of it, and the kernel refuses
/// to exec the file with `ETXTBSY` until that child execs or exits. The window
/// is short and the condition always clears, so retry past it rather than
/// failing a test that is not about process spawning.
fn sol_root_output(program: &Path, cwd: &Path, public_argv0: &str) -> Output {
    for _ in 0..100 {
        match Command::new(program)
            .arg(identity_arg(public_argv0))
            .arg("root")
            .current_dir(cwd)
            .output()
        {
            Ok(output) => return output,
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("fake solstone-core should execute: {error:?}"),
        }
    }
    panic!(
        "fake solstone-core stayed busy after retries: {}",
        program.display()
    )
}

#[test]
fn version_writes_stdout_and_exits_zero() {
    let output = Command::new(bin())
        .arg("--version")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        format!("solstone-core {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
}

#[test]
fn usage_error_writes_stderr_and_exits_64() {
    let output = Command::new(bin())
        .arg("--unknown")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(64));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        ""
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        solstone_core_cli::USAGE
    );
}

#[test]
fn journal_path_override_prints_cli_label_without_creating() {
    let target = temp_path("override-no-create");
    let output = Command::new(bin())
        .arg("journal-path")
        .arg("--journal")
        .arg(&target)
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        format!("cli\t{}\n", target.display())
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
    assert!(!target.exists());
}

#[test]
fn journal_path_override_create_creates_directory() {
    let target = temp_path("override-create");
    let output = Command::new(bin())
        .arg("journal-path")
        .arg("--journal")
        .arg(&target)
        .arg("--create")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        format!("cli\t{}\n", target.display())
    );
    assert!(target.is_dir());
    fs::remove_dir_all(target).expect("cleanup created journal");
}

#[test]
fn journal_path_empty_override_prints_but_create_errors() {
    let output = Command::new(bin())
        .arg("journal-path")
        .arg("--journal")
        .arg("")
        .output()
        .expect("solstone-core should execute");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        "cli\t\n"
    );

    let create_output = Command::new(bin())
        .arg("journal-path")
        .arg("--journal")
        .arg("")
        .arg("--create")
        .output()
        .expect("solstone-core should execute");
    assert_eq!(create_output.status.code(), Some(75));
    assert_eq!(
        String::from_utf8(create_output.stdout).expect("stdout should be utf-8"),
        ""
    );
    assert!(
        String::from_utf8(create_output.stderr)
            .expect("stderr should be utf-8")
            .starts_with("could not create journal directory (cli): ")
    );
}

#[test]
fn journal_path_env_spaces_are_unstripped() {
    let output = Command::new(bin())
        .arg("journal-path")
        .env("SOLSTONE_JOURNAL", "   ")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        "env\t   \n"
    );
}

#[test]
fn journal_path_config_tilde_is_literal() {
    let home = temp_path("config-tilde-home");
    let config_dir = home.join(".config").join("solstone");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(config_dir.join("config.toml"), "journal = \"~/journal\"\n").expect("write config");

    let output = Command::new(bin())
        .arg("journal-path")
        .env_remove("SOLSTONE_JOURNAL")
        .env("HOME", &home)
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        "config\t~/journal\n"
    );
    fs::remove_dir_all(home).expect("cleanup config home");
}

#[test]
fn sol_and_solstone_identities_report_the_same_native_version() {
    let sol = Command::new(bin())
        .arg(identity_arg("sol"))
        .arg("--version")
        .output()
        .expect("sol identity should execute");
    let solstone = Command::new(bin())
        .arg(identity_arg("solstone"))
        .arg("--version")
        .output()
        .expect("solstone identity should execute");

    assert_eq!(sol.status.code(), Some(0));
    assert_eq!(solstone.status.code(), Some(0));
    assert_eq!(sol.stdout, solstone.stdout);
    assert_eq!(sol.stderr, solstone.stderr);
    assert_eq!(
        String::from_utf8(sol.stdout).expect("stdout should be utf-8"),
        format!("sol (solstone) {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        String::from_utf8(solstone.stderr).expect("stderr should be utf-8"),
        ""
    );
}

#[test]
fn sol_identity_marker_must_be_exact_first_arg() {
    for args in [
        vec!["__solstone_identity="],
        vec!["__solstone_identity=bogus"],
        vec!["journal-path", "__solstone_identity=sol"],
    ] {
        let output = Command::new(bin())
            .args(args)
            .output()
            .expect("solstone-core should execute");

        assert_eq!(output.status.code(), Some(64));
        assert_eq!(
            String::from_utf8(output.stdout).expect("stdout should be utf-8"),
            ""
        );
        assert_eq!(
            String::from_utf8(output.stderr).expect("stderr should be utf-8"),
            solstone_core_cli::USAGE
        );
    }
}

#[test]
fn sol_root_installed_layout_is_independent_of_cwd() {
    let env_root = temp_path("sol-root-installed-layout");
    let bin_dir = env_root.join("bin");
    let site_packages = env_root
        .join("lib")
        .join("python3.13")
        .join("site-packages");
    fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    fs::create_dir_all(site_packages.join("solstone")).expect("create fake package dir");
    fs::write(site_packages.join("solstone").join("__init__.py"), "").expect("write init");
    let fake_solstone_core = bin_dir.join("solstone-core");
    fs::copy(bin(), &fake_solstone_core)
        .expect("copy solstone-core binary into fake install layout");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&fake_solstone_core)
            .expect("fake solstone-core metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_solstone_core, permissions)
            .expect("make fake solstone-core executable");
    }

    let unrelated = env_root.join("unrelated");
    fs::create_dir_all(&unrelated).expect("create unrelated cwd");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source_checkout = manifest
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .expect("workspace checkout root");

    for cwd in [&unrelated, source_checkout] {
        let output = sol_root_output(&fake_solstone_core, cwd, "sol");
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8(output.stdout).expect("stdout should be utf-8"),
            format!("{}\n", site_packages.display())
        );
        assert_eq!(
            String::from_utf8(output.stderr).expect("stderr should be utf-8"),
            ""
        );
    }
    fs::remove_dir_all(env_root).expect("cleanup fake install layout");
}

#[cfg(unix)]
#[test]
fn sol_root_installed_layout_canonicalizes_lib64_alias_independent_of_cwd() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let env_root = temp_path("sol-root-installed-lib64-layout");
    let bin_dir = env_root.join("bin");
    let site_packages = env_root
        .join("lib")
        .join("python3.13")
        .join("site-packages");
    fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    fs::create_dir_all(site_packages.join("solstone")).expect("create fake package dir");
    fs::write(site_packages.join("solstone").join("__init__.py"), "").expect("write init");
    symlink("lib", env_root.join("lib64")).expect("create lib64 symlink");
    let canonical_site_packages =
        fs::canonicalize(&site_packages).expect("canonical fake site-packages");
    let fake_solstone_core = bin_dir.join("solstone-core");
    fs::copy(bin(), &fake_solstone_core)
        .expect("copy solstone-core binary into fake install layout");
    let mut permissions = fs::metadata(&fake_solstone_core)
        .expect("fake solstone-core metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_solstone_core, permissions)
        .expect("make fake solstone-core executable");

    let unrelated = env_root.join("unrelated");
    fs::create_dir_all(&unrelated).expect("create unrelated cwd");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source_checkout = manifest
        .parent()
        .and_then(|path| path.parent())
        .and_then(|path| path.parent())
        .expect("workspace checkout root");

    for cwd in [&unrelated, source_checkout] {
        let output = sol_root_output(&fake_solstone_core, cwd, "sol");
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8(output.stdout).expect("stdout should be utf-8"),
            format!("{}\n", canonical_site_packages.display())
        );
        assert_eq!(
            String::from_utf8(output.stderr).expect("stderr should be utf-8"),
            ""
        );
    }
    fs::remove_dir_all(env_root).expect("cleanup fake install layout");
}
