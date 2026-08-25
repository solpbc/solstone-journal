// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use plist::Value;
use solstone_core_installation_identity::{
    Generation, GuardFields, IdentityError, InstallationId, JournalToken, NamespaceName, OwnerBase,
    PlatformTag, RootToken, load_installation_binding,
};
use solstone_core_service_unit::{
    build_service_environment, render_launchd_plist, render_systemd_unit,
};

mod support;

const HOME: &str = "/home/sol";
const PATH: &str = "/usr/bin:/bin";
const RUNTIME_DIR: &str = "/opt/sol/bin";
const LAUNCHER: &str = "/home/sol/.local/bin/journal";
const PORT: &str = "5015";
const JOURNAL: &str = "/srv/journal";

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/var/tmp/solstone-core-service-unit-identity-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create test root");
        fs::create_dir(root.join("home")).expect("create test home");
        Self(root)
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn guard() -> GuardFields {
    GuardFields {
        namespace: NamespaceName::parse(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
        id: InstallationId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        generation: Generation::new(1).unwrap(),
        journal_token: JournalToken::from_raw_absolute(b"/srv/journal".to_vec()).unwrap(),
    }
}

fn render(
    env: &BTreeMap<String, String>,
    launcher: &str,
    port: &str,
    journal: &str,
) -> (Value, support::ParsedUnit) {
    let plist = render_launchd_plist(env, launcher, port, journal).expect("valid render");
    let unit = render_systemd_unit(env, launcher, port, journal).expect("valid render");
    (support::parse_plist(&plist), support::parse_unit(&unit))
}

fn arguments(plist: &Value) -> Vec<String> {
    plist.as_dictionary().expect("plist dictionary")["ProgramArguments"]
        .as_array()
        .expect("arguments array")
        .iter()
        .map(|value| value.as_string().expect("argument string").to_owned())
        .collect()
}

fn environment(plist: &Value) -> BTreeMap<String, String> {
    plist.as_dictionary().expect("plist dictionary")["EnvironmentVariables"]
        .as_dictionary()
        .expect("environment dictionary")
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_string().expect("env string").to_owned(),
            )
        })
        .collect()
}

fn log_paths(plist: &Value) -> (String, String) {
    let dictionary = plist.as_dictionary().expect("plist dictionary");
    (
        dictionary["StandardOutPath"]
            .as_string()
            .expect("stdout path")
            .to_owned(),
        dictionary["StandardErrorPath"]
            .as_string()
            .expect("stderr path")
            .to_owned(),
    )
}

#[test]
fn one_input_at_a_time_changes_only_its_derived_field() {
    let baseline_env = build_service_environment(HOME, Some(PATH), RUNTIME_DIR, &guard());
    let (baseline_plist, baseline_unit) = render(&baseline_env, LAUNCHER, PORT, JOURNAL);

    let (plist, unit) = render(&baseline_env, "/home/sol/a b/$journal", PORT, JOURNAL);
    assert_eq!(arguments(&plist)[1..], arguments(&baseline_plist)[1..]);
    assert_eq!(unit.exec_start[0], "/home/sol/a b/$journal");
    assert_eq!(unit.exec_start[1..], baseline_unit.exec_start[1..]);
    assert_eq!(environment(&plist), environment(&baseline_plist));
    assert_eq!(log_paths(&plist), log_paths(&baseline_plist));
    assert_eq!(unit.environment, baseline_unit.environment);
    assert_eq!(unit.log_paths, baseline_unit.log_paths);

    let (plist, unit) = render(&baseline_env, LAUNCHER, "5 815${PORT}%", JOURNAL);
    assert_eq!(arguments(&plist)[0], arguments(&baseline_plist)[0]);
    assert_eq!(arguments(&plist)[1], "start");
    assert_eq!(arguments(&plist)[2], "5 815${PORT}%");
    assert_eq!(unit.exec_start[2], "5 815${PORT}%");
    assert_eq!(environment(&plist), environment(&baseline_plist));
    assert_eq!(log_paths(&plist), log_paths(&baseline_plist));
    assert_eq!(unit.environment, baseline_unit.environment);
    assert_eq!(unit.log_paths, baseline_unit.log_paths);

    let home_env = build_service_environment("/home/sol $ café", Some(PATH), RUNTIME_DIR, &guard());
    let (plist, unit) = render(&home_env, LAUNCHER, PORT, JOURNAL);
    assert_eq!(arguments(&plist), arguments(&baseline_plist));
    assert_eq!(environment(&plist)["HOME"], "/home/sol $ café");
    assert_eq!(unit.environment["HOME"], "/home/sol $ café");
    assert_eq!(
        environment(&plist)["PATH"],
        environment(&baseline_plist)["PATH"]
    );
    assert_eq!(log_paths(&plist), log_paths(&baseline_plist));
    assert_eq!(unit.exec_start, baseline_unit.exec_start);
    assert_eq!(unit.log_paths, baseline_unit.log_paths);

    let (plist, unit) = render(&baseline_env, LAUNCHER, PORT, "/srv/journal space%");
    assert_eq!(arguments(&plist), arguments(&baseline_plist));
    assert_eq!(environment(&plist), environment(&baseline_plist));
    assert_eq!(
        log_paths(&plist),
        (
            "/srv/journal space%/health/service.log".to_owned(),
            "/srv/journal space%/health/service.log".to_owned(),
        )
    );
    assert_eq!(unit.exec_start, baseline_unit.exec_start);
    assert_eq!(unit.environment, baseline_unit.environment);
    assert_eq!(
        unit.log_paths,
        (
            "/srv/journal space%/health/service.log".to_owned(),
            "/srv/journal space%/health/service.log".to_owned(),
        )
    );
}

#[test]
fn path_inputs_change_only_path_construction() {
    let baseline = build_service_environment(HOME, Some(PATH), RUNTIME_DIR, &guard());
    let (baseline_plist, baseline_unit) = render(&baseline, LAUNCHER, PORT, JOURNAL);
    let duplicate = build_service_environment(
        HOME,
        Some("/usr/bin:/opt/sol/bin:/usr/bin:/bin"),
        RUNTIME_DIR,
        &guard(),
    );
    let absent = build_service_environment(HOME, None, RUNTIME_DIR, &guard());
    assert_eq!(duplicate["HOME"], baseline["HOME"]);
    assert!(!duplicate.contains_key("PYTHONUNBUFFERED"));
    assert!(!baseline.contains_key("PYTHONUNBUFFERED"));
    assert_eq!(duplicate["PATH"], "/opt/sol/bin:/usr/bin:/bin");
    assert_eq!(absent["PATH"], "/opt/sol/bin:/usr/local/bin:/usr/bin:/bin");

    let alternate_runtime = build_service_environment(HOME, Some(PATH), "/runtime other", &guard());
    assert_eq!(alternate_runtime["HOME"], baseline["HOME"]);
    assert!(!alternate_runtime.contains_key("PYTHONUNBUFFERED"));
    assert_eq!(alternate_runtime["PATH"], "/runtime other:/usr/bin:/bin");

    for environment_input in [duplicate, absent, alternate_runtime] {
        let (plist, unit) = render(&environment_input, LAUNCHER, PORT, JOURNAL);
        assert_eq!(arguments(&plist), arguments(&baseline_plist));
        assert_eq!(log_paths(&plist), log_paths(&baseline_plist));
        assert_eq!(environment(&plist), environment_input);
        assert_eq!(unit.exec_start, baseline_unit.exec_start);
        assert_eq!(unit.environment, environment_input);
        assert_eq!(unit.log_paths, baseline_unit.log_paths);
    }
}

#[test]
fn default_unit_is_notify_and_does_not_export_pythonunbuffered() {
    let environment = build_service_environment(HOME, Some(PATH), RUNTIME_DIR, &guard());
    let unit = render_systemd_unit(&environment, LAUNCHER, PORT, JOURNAL).expect("valid render");
    assert!(unit.contains("Type=notify\n"));
    assert!(!unit.contains("PYTHONUNBUFFERED"));
}

#[test]
fn load_binding_is_callable_without_setup_admission() {
    let fixture = TempRoot::new();
    let owner = OwnerBase::at_home(fixture.home(), PlatformTag::current()).expect("owner base");
    let root =
        RootToken::from_raw_absolute(b"/service-unit/unadopted".to_vec()).expect("root token");

    let result = load_installation_binding(&owner, &root);

    assert!(matches!(
        result,
        Err(IdentityError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound
    ));
    assert!(!owner.path().exists());
}
