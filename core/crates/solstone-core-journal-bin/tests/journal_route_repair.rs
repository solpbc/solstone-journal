// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(target_os = "linux")]

//! Process-boundary coverage for hidden, identity-guarded route repair.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, Generation, GuardFields, InstallationId, LegacyManifestEvidence,
    OwnerBase, PlatformTag, SetupAdmissionRequest, admit_setup, journal_token_from_path,
    root_token_from_path,
};
use solstone_core_service_unit::{build_service_environment, render_systemd_unit};
use solstone_core_setup::wrapper::{WrapperCommand, render_wrapper};

use support::locate_workspace_binary;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct Binaries {
    dispatcher: PathBuf,
    core: PathBuf,
}

fn binaries() -> &'static Binaries {
    static BINARIES: OnceLock<Binaries> = OnceLock::new();
    BINARIES.get_or_init(|| Binaries {
        dispatcher: locate_workspace_binary("solstone-core-journal-bin", "solstone-core-journal"),
        core: locate_workspace_binary("solstone-core", "solstone-core"),
    })
}

struct Fixture {
    root: PathBuf,
    prefix: PathBuf,
    home: PathBuf,
    journal: PathBuf,
    first_version: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = PathBuf::from("/var/tmp").join(format!(
            "solstone-journal-route-repair-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create isolated fixture root");
        let prefix = root.join("prefix");
        fs::create_dir(&prefix).expect("create prefix");
        let first_version = make_version(&prefix, "1.0.0-aaaaaaaaaaaa");
        symlink("versions/1.0.0-aaaaaaaaaaaa", prefix.join("current")).expect("select build");
        let home = root.join("home");
        let journal = root.join("journal");
        fs::create_dir(&home).expect("create fixture home");
        fs::create_dir(&journal).expect("create fixture journal");
        Self {
            root,
            prefix,
            home,
            journal,
            first_version,
        }
    }

    fn dispatcher(&self) -> PathBuf {
        self.prefix.join("current/bin/journal")
    }

    fn wrappers(&self) -> (PathBuf, PathBuf) {
        (
            self.home.join(".local/bin/journal"),
            self.home.join(".local/bin/solstone"),
        )
    }

    fn service_path(&self) -> PathBuf {
        self.home.join(".config/systemd/user/solstone.service")
    }

    fn install_owned_tuple(&self) -> GuardFields {
        fs::create_dir_all(self.home.join(".local/bin")).expect("create wrapper directory");
        let root_token = root_token_from_path(&self.prefix).expect("root token");
        let journal_token = journal_token_from_path(&self.journal).expect("journal token");
        let owner = OwnerBase::at_home(self.home.clone(), PlatformTag::current()).expect("owner");
        let admission = admit_setup(SetupAdmissionRequest {
            owner,
            root_token,
            journal_token,
            journal_is_explicit: true,
            legacy_manifest: LegacyManifestEvidence::Absent,
            artifacts: ArtifactBindingEvidence::Fresh,
        })
        .expect("admit fixture identity");
        let guard = GuardFields::from_binding(admission.binding());
        drop(admission);
        self.write_wrappers(&guard, &self.first_version);
        self.write_service(&guard);
        guard
    }

    fn write_wrappers(&self, guard: &GuardFields, version: &Path) {
        let (journal, solstone) = self.wrappers();
        fs::write(
            &journal,
            render_wrapper(
                WrapperCommand::Journal,
                &self.journal,
                &version.join("bin/journal"),
                guard,
            )
            .unwrap(),
        )
        .expect("write journal wrapper");
        fs::write(
            &solstone,
            render_wrapper(
                WrapperCommand::Solstone,
                &self.journal,
                &version.join("bin/solstone"),
                guard,
            )
            .unwrap(),
        )
        .expect("write solstone wrapper");
        for wrapper in [journal, solstone] {
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))
                .expect("make wrapper executable");
        }
    }

    fn write_service(&self, guard: &GuardFields) {
        self.write_service_for(guard, &self.prefix.join("current/bin"));
    }

    fn write_service_for(&self, guard: &GuardFields, runtime: &Path) {
        let path = self.service_path();
        fs::create_dir_all(path.parent().expect("service parent")).expect("create service parent");
        let environment = build_service_environment(
            self.home.to_str().expect("utf8 home"),
            Some("/usr/bin:/bin"),
            runtime.to_str().expect("utf8 runtime"),
            guard,
        );
        fs::write(
            path,
            render_systemd_unit(
                &environment,
                self.home
                    .join(".local/bin/journal")
                    .to_str()
                    .expect("utf8 launcher"),
                "5015",
            ),
        )
        .expect("write service");
    }

    fn acquire_route_lock(&self, token: &str) {
        let directory = self.prefix.join(".solstone-route.lock");
        fs::create_dir(&directory).expect("create route lock");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("lock mode");
        let owner = directory.join("owner");
        fs::write(&owner, format!("solstone-route-lock-v1\n{token}\n")).expect("write owner");
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o600)).expect("owner mode");
    }

    fn respring_current(&self) -> PathBuf {
        let version = make_version(&self.prefix, "1.0.0-bbbbbbbbbbbb");
        fs::remove_file(self.prefix.join("current")).expect("replace current");
        symlink("versions/1.0.0-bbbbbbbbbbbb", self.prefix.join("current"))
            .expect("select respun build");
        version
    }

    fn run_repair(&self, token: &str) -> Output {
        self.run_repair_from(&self.dispatcher(), token)
    }

    fn run_repair_from(&self, dispatcher: &Path, token: &str) -> Output {
        Command::new(dispatcher)
            .args(["__journal-route-repair", "--route-lock-owner", token])
            .env("HOME", &self.home)
            .env("SOLSTONE_JOURNAL", &self.journal)
            .output()
            .expect("run route repair")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn make_version(prefix: &Path, name: &str) -> PathBuf {
    let version = prefix.join("versions").join(name);
    let bin = version.join("bin");
    fs::create_dir_all(&bin).expect("create version bin");
    for (source, target) in [
        (&binaries().dispatcher, "journal"),
        (&binaries().dispatcher, "solstone"),
        (&binaries().core, "solstone-core"),
    ] {
        let destination = bin.join(target);
        fs::copy(source, &destination).expect("copy binary");
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
            .expect("make binary executable");
    }
    version
}

fn parse_record(output: Output, code: i32) -> BTreeMap<String, String> {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("ASCII record");
    assert!(stdout.ends_with('\n'));
    assert!(!stdout.ends_with("\n\n"));
    let fields = stdout[..stdout.len() - 1]
        .lines()
        .map(|line| {
            let (key, value) = line.split_once('=').expect("key=value");
            (key.to_owned(), value.to_owned())
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(fields.len(), 40, "complete repair record");
    assert_eq!(fields["command"], "repair");
    fields
}

fn assert_repair_refusal_preserves_artifacts(fixture: &Fixture, refusal: &str) {
    let (journal, solstone) = fixture.wrappers();
    let before = [
        fs::read(&journal).expect("journal wrapper bytes"),
        fs::read(&solstone).expect("solstone wrapper bytes"),
        fs::read(fixture.service_path()).expect("service unit bytes"),
    ];
    let fields = parse_record(fixture.run_repair(TOKEN), 2);
    assert_eq!(fields["outcome"], "refused");
    assert_eq!(fields["refusal"], refusal);
    assert_eq!(
        fs::read(&journal).expect("journal wrapper unchanged"),
        before[0]
    );
    assert_eq!(
        fs::read(&solstone).expect("solstone wrapper unchanged"),
        before[1]
    );
    assert_eq!(
        fs::read(fixture.service_path()).expect("service unit unchanged"),
        before[2]
    );
}

#[test]
fn repair_repoints_owned_drift_and_is_idempotent() {
    let fixture = Fixture::new("drift");
    fixture.install_owned_tuple();
    let selected = fixture.respring_current();
    fixture.acquire_route_lock(TOKEN);

    let first = fixture.run_repair(TOKEN);
    let first_code = first.status.code().expect("repair exit code");
    assert!(matches!(first_code, 0 | 3));
    let fields = parse_record(first, first_code);
    if fields["service_state"] == "runtime-drifted" {
        assert_eq!(first_code, 3, "{fields:?}");
        assert_eq!(fields["outcome"], "partial-failure", "{fields:?}");
    } else {
        assert_eq!(first_code, 0, "{fields:?}");
        assert_eq!(fields["outcome"], "success", "{fields:?}");
    }
    assert_eq!(fields["repair_wrapper"], "rewritten");
    assert_eq!(fields["repair_service"], "rewritten");
    assert_eq!(fields["terminal_identity_state"], "matched");
    let (journal, solstone) = fixture.wrappers();
    assert!(
        fs::read_to_string(&journal)
            .expect("journal wrapper")
            .contains(&format!(
                "SOL_BIN='{}'",
                selected.join("bin/journal").display()
            ))
    );
    assert!(
        fs::read_to_string(&solstone)
            .expect("solstone wrapper")
            .contains(&format!(
                "SOL_BIN='{}'",
                selected.join("bin/solstone").display()
            ))
    );
    let service = fs::read_to_string(fixture.service_path()).expect("service");
    assert!(service.contains(&format!("PATH={}/current/bin", fixture.prefix.display())));
    assert!(service.contains(&format!("ExecStart={} start 5015", journal.display())));

    let second = fixture.run_repair(TOKEN);
    let second_code = second.status.code().expect("repair exit code");
    let fields = parse_record(second, second_code);
    assert_eq!(fields["repair_wrapper"], "unchanged");
    if fields["service_state"] == "runtime-drifted" {
        assert_eq!(second_code, 3, "{fields:?}");
        assert_eq!(fields["repair_service"], "rewritten");
    } else {
        assert_eq!(second_code, 0, "{fields:?}");
        assert_eq!(fields["repair_service"], "unchanged");
    }
}

#[test]
fn repair_refuses_conflicting_guard_generations_without_mutating() {
    let fixture = Fixture::new("static-drift");
    let guard = fixture.install_owned_tuple();
    let mut stale_guard = guard.clone();
    stale_guard.generation = Generation::new(2).expect("stale nonzero generation");
    fixture.write_service_for(&stale_guard, &fixture.prefix.join("current/bin"));
    fixture.acquire_route_lock(TOKEN);

    assert_repair_refusal_preserves_artifacts(&fixture, "artifact-ambiguous");
}

#[test]
fn repair_refuses_missing_or_invalid_preconditions_without_writing() {
    let missing_lock = Fixture::new("missing-lock");
    missing_lock.install_owned_tuple();
    let wrapper = missing_lock.wrappers().0;
    let before = fs::read(&wrapper).expect("wrapper bytes");
    let fields = parse_record(missing_lock.run_repair(TOKEN), 2);
    assert_eq!(fields["refusal"], "lock-missing");
    assert_eq!(fs::read(&wrapper).expect("wrapper unchanged"), before);

    let mismatch = Fixture::new("mismatch");
    mismatch.install_owned_tuple();
    mismatch.acquire_route_lock("fedcba9876543210fedcba9876543210");
    let fields = parse_record(mismatch.run_repair(TOKEN), 2);
    assert_eq!(fields["refusal"], "lock-owner-mismatch");

    let malformed_lock = Fixture::new("malformed-lock");
    malformed_lock.install_owned_tuple();
    malformed_lock.acquire_route_lock(TOKEN);
    fs::set_permissions(
        malformed_lock.prefix.join(".solstone-route.lock"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("corrupt lock mode");
    let fields = parse_record(malformed_lock.run_repair(TOKEN), 2);
    assert_eq!(fields["refusal"], "lock-invalid");

    let absent = Fixture::new("absent");
    absent.install_owned_tuple();
    let (journal, solstone) = absent.wrappers();
    fs::remove_file(journal).expect("remove journal");
    fs::remove_file(solstone).expect("remove solstone");
    fs::remove_file(absent.service_path()).expect("remove service");
    absent.acquire_route_lock(TOKEN);
    let fields = parse_record(absent.run_repair(TOKEN), 0);
    assert_eq!(fields["tuple_state"], "not-applicable");
    assert_eq!(fields["repair_wrapper"], "not-run");
    assert_eq!(fields["repair_service"], "not-run");

    let no_identity = Fixture::new("no-identity");
    no_identity.acquire_route_lock(TOKEN);
    let fields = parse_record(no_identity.run_repair(TOKEN), 2);
    assert_eq!(fields["refusal"], "missing-identity");
}

#[test]
fn repair_handles_the_named_partial_route_shapes_without_creating_artifacts() {
    let wrappers_only = Fixture::new("wrappers-only");
    wrappers_only.install_owned_tuple();
    let selected = wrappers_only.respring_current();
    fs::remove_file(wrappers_only.service_path()).expect("remove service");
    wrappers_only.acquire_route_lock(TOKEN);
    let fields = parse_record(wrappers_only.run_repair(TOKEN), 0);
    assert_eq!(fields["repair_wrapper"], "rewritten");
    assert_eq!(fields["repair_service"], "not-run");
    assert!(!wrappers_only.service_path().exists());
    assert!(
        fs::read_to_string(wrappers_only.wrappers().0)
            .expect("rewritten wrapper")
            .contains(&format!(
                "SOL_BIN='{}'",
                selected.join("bin/journal").display()
            ))
    );

    let dangling = Fixture::new("dangling");
    dangling.install_owned_tuple();
    let (journal, solstone) = dangling.wrappers();
    fs::remove_file(&journal).expect("remove journal wrapper");
    fs::remove_file(&solstone).expect("remove solstone wrapper");
    dangling.acquire_route_lock(TOKEN);
    let fields = parse_record(dangling.run_repair(TOKEN), 2);
    assert_eq!(fields["refusal"], "tuple-not-repair-eligible");
    assert!(!journal.exists());
    assert!(!solstone.exists());

    let not_current = Fixture::new("not-current");
    not_current.install_owned_tuple();
    not_current.respring_current();
    not_current.acquire_route_lock(TOKEN);
    let wrapper = not_current.wrappers().0;
    let before = fs::read(&wrapper).expect("wrapper bytes");
    let fields = parse_record(
        not_current.run_repair_from(&not_current.first_version.join("bin/journal"), TOKEN),
        2,
    );
    assert_eq!(fields["refusal"], "not-current");
    assert_eq!(fs::read(&wrapper).expect("wrapper unchanged"), before);
}

#[test]
fn repair_refuses_foreign_malformed_unguarded_ambiguous_and_exact_v1_without_mutating() {
    let foreign = Fixture::new("foreign");
    let guard = foreign.install_owned_tuple();
    let foreign_guard = GuardFields {
        id: InstallationId::parse("ffeeddccbbaa99887766554433221100").expect("foreign id"),
        ..guard
    };
    foreign.write_wrappers(&foreign_guard, &foreign.first_version);
    foreign.write_service(&foreign_guard);
    foreign.acquire_route_lock(TOKEN);
    assert_repair_refusal_preserves_artifacts(&foreign, "artifact-foreign");

    let malformed = Fixture::new("malformed");
    malformed.install_owned_tuple();
    let malformed_journal = malformed.wrappers().0;
    fs::write(
        &malformed_journal,
        format!(
            "{}# solstone-installation-id: duplicate\n",
            fs::read_to_string(&malformed_journal).expect("read wrapper")
        ),
    )
    .expect("corrupt wrapper guard");
    malformed.acquire_route_lock(TOKEN);
    assert_repair_refusal_preserves_artifacts(&malformed, "artifact-malformed");

    let unguarded = Fixture::new("unguarded");
    let guard = unguarded.install_owned_tuple();
    let unguarded_journal = unguarded.wrappers().0;
    let wrapper = render_wrapper(
        WrapperCommand::Journal,
        &unguarded.journal,
        &unguarded.first_version.join("bin/journal"),
        &guard,
    )
    .unwrap()
    .lines()
    .filter(|line| !line.starts_with("# solstone-installation-"))
    .collect::<Vec<_>>()
    .join("\n");
    fs::write(&unguarded_journal, format!("{wrapper}\n")).expect("write unguarded wrapper");
    fs::set_permissions(&unguarded_journal, fs::Permissions::from_mode(0o755))
        .expect("make wrapper executable");
    unguarded.acquire_route_lock(TOKEN);
    assert_repair_refusal_preserves_artifacts(&unguarded, "artifact-unguarded");

    let ambiguous = Fixture::new("ambiguous");
    let guard = ambiguous.install_owned_tuple();
    let ambiguous_guard = GuardFields {
        id: InstallationId::parse("ffeeddccbbaa99887766554433221100").expect("ambiguous id"),
        ..guard
    };
    ambiguous.write_service(&ambiguous_guard);
    ambiguous.acquire_route_lock(TOKEN);
    assert_repair_refusal_preserves_artifacts(&ambiguous, "artifact-ambiguous");

    let exact_v1 = Fixture::new("exact-v1");
    exact_v1.install_owned_tuple();
    let legacy_bin = exact_v1.home.join(".local/share/uv/tools/solstone/bin");
    fs::create_dir_all(&legacy_bin).expect("create legacy launcher directory");
    let legacy = legacy_bin.join("solstone");
    fs::write(
        &legacy,
        concat!(
            "#!/usr/bin/python3\n",
            "# -*- coding: utf-8 -*-\n",
            "import sys\n",
            "from solstone.think.sol_cli import main\n",
            "if __name__ == '__main__':\n",
            "    if sys.argv[0].endswith('-script.pyw'):\n",
            "        sys.argv[0] = sys.argv[0][:-11]\n",
            "    elif sys.argv[0].endswith('.exe'):\n",
            "        sys.argv[0] = sys.argv[0][:-4]\n",
            "    sys.exit(main())\n"
        ),
    )
    .expect("write exact v1 launcher");
    fs::set_permissions(&legacy, fs::Permissions::from_mode(0o755))
        .expect("make legacy launcher executable");
    let solstone = exact_v1.wrappers().1;
    fs::remove_file(&solstone).expect("replace public wrapper");
    symlink(&legacy, &solstone).expect("link legacy launcher");
    exact_v1.acquire_route_lock(TOKEN);
    assert_repair_refusal_preserves_artifacts(&exact_v1, "artifact-exact-v1");
}
