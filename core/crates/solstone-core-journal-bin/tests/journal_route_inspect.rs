// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(target_os = "linux")]

//! Process-boundary coverage for the hidden, read-only journal route inspection.

mod support;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_installation_identity::{
    ArtifactBindingEvidence, GuardFields, InstallationId, JournalToken, LegacyManifestEvidence,
    OwnerBase, PlatformTag, SetupAdmissionRequest, admit_setup, journal_token_from_path,
    root_token_from_path,
};
use solstone_core_service_unit::{build_service_environment, render_systemd_unit};
use solstone_core_setup::wrapper::{WrapperCommand, render_wrapper};

use support::locate_workspace_binary;

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
            "solstone-journal-route-inspect-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create isolated fixture root");
        let prefix = root.join("prefix");
        fs::create_dir_all(&prefix).expect("create prefix");
        let first_version = make_version(&prefix, "1.0.0-aaaaaaaaaaaa");
        symlink("versions/1.0.0-aaaaaaaaaaaa", prefix.join("current")).expect("select first build");
        let home = root.join("home");
        let journal = root.join("journal");
        fs::create_dir_all(&home).expect("create fixture home");
        fs::create_dir_all(&journal).expect("create fixture journal");
        Self {
            root,
            prefix,
            home,
            journal,
            first_version,
        }
    }

    fn journal_dispatcher(&self) -> PathBuf {
        self.prefix.join("current/bin/journal")
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
        let bin = self.home.join(".local/bin");
        for (command, wrapper_command) in [
            ("journal", WrapperCommand::Journal),
            ("solstone", WrapperCommand::Solstone),
        ] {
            fs::write(
                bin.join(command),
                render_wrapper(
                    wrapper_command,
                    &self.journal,
                    &version.join("bin").join(command),
                    guard,
                )
                .unwrap(),
            )
            .expect("write managed wrapper");
        }
    }

    fn write_service(&self, guard: &GuardFields) {
        let path = self.service_path();
        fs::create_dir_all(path.parent().expect("service parent")).expect("create service parent");
        let launcher = self.home.join(".local/bin/journal");
        let environment = build_service_environment(
            self.home.to_str().expect("utf8 home"),
            Some("/usr/bin:/bin"),
            self.prefix
                .join("current/bin")
                .to_str()
                .expect("utf8 runtime"),
            guard,
        );
        fs::write(
            path,
            render_systemd_unit(
                &environment,
                launcher.to_str().expect("utf8 launcher"),
                "5015",
            ),
        )
        .expect("write managed service");
    }

    fn run(&self) -> Output {
        self.run_from(&self.journal_dispatcher(), None)
    }

    fn run_from(&self, dispatcher: &Path, path: Option<&str>) -> Output {
        let mut command = Command::new(dispatcher);
        command
            .arg("__journal-route-inspect")
            .env("HOME", &self.home)
            .env("SOLSTONE_JOURNAL", &self.journal);
        if let Some(path) = path {
            command.env("PATH", path);
        }
        command
            .output()
            .expect("run hidden journal route inspection")
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
    fs::create_dir_all(&bin).expect("create version binary directory");
    for (source, target) in [
        (&binaries().dispatcher, "journal"),
        (&binaries().dispatcher, "solstone"),
        (&binaries().core, "solstone-core"),
    ] {
        let destination = bin.join(target);
        fs::copy(source, &destination).expect("copy native fixture binary");
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
            .expect("make native fixture binary executable");
    }
    version
}

fn parse_record(output: &Output) -> BTreeMap<String, String> {
    assert!(
        output.stderr.is_empty(),
        "inspection diagnostics must not share stdout: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("ASCII record");
    assert!(stdout.ends_with('\n'));
    assert!(!stdout.ends_with("\n\n"));
    let mut fields = BTreeMap::new();
    for line in stdout[..stdout.len() - 1].split('\n') {
        let (key, value) = line.split_once('=').expect("key=value record line");
        assert!(
            fields.insert(key.to_owned(), value.to_owned()).is_none(),
            "duplicate {key}"
        );
    }
    assert_eq!(fields.len(), 36, "one complete record field group");
    assert_eq!(fields["record_version"], "1");
    assert_eq!(fields["command"], "inspect");
    fields
}

fn assert_success(output: Output) -> BTreeMap<String, String> {
    assert_eq!(output.status.code(), Some(0));
    let fields = parse_record(&output);
    assert_eq!(fields["outcome"], "success");
    assert_eq!(fields["refusal"], "none");
    fields
}

fn decode_path_hex(value: &str) -> PathBuf {
    assert_eq!(value.len() % 2, 0, "hex values have complete bytes");
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                .expect("valid hex byte")
        })
        .collect();
    PathBuf::from(OsString::from_vec(bytes))
}

#[test]
fn inspection_reports_each_route_artifact_independently() {
    let absent = Fixture::new("absent");
    let fields = assert_success(absent.run());
    assert_eq!(fields["tuple_state"], "missing-identity");

    let aligned = Fixture::new("aligned");
    aligned.install_owned_tuple();
    let fields = assert_success(aligned.run());
    assert_eq!(fields["journal_wrapper_state"], "aligned", "{fields:?}");
    assert_eq!(fields["solstone_wrapper_state"], "aligned", "{fields:?}");
    // The static unit and guard are aligned. `observe_runtime` deliberately
    // runs the real, absolute systemctl and cannot be redirected to this
    // /var/tmp fixture, so an unrelated user manager is runtime-drifted;
    // a real manager that reports the fixture unit absent is aligned.
    assert!(
        matches!(
            fields["service_state"].as_str(),
            "aligned" | "runtime-drifted"
        ),
        "{fields:?}"
    );
    assert_eq!(
        decode_path_hex(&fields["service_launcher_hex"]),
        aligned.home.join(".local/bin/journal")
    );

    let drifted = Fixture::new("drifted");
    drifted.install_owned_tuple();
    make_version(&drifted.prefix, "1.0.0-bbbbbbbbbbbb");
    fs::remove_file(drifted.prefix.join("current")).expect("replace current link");
    symlink(
        "versions/1.0.0-bbbbbbbbbbbb",
        drifted.prefix.join("current"),
    )
    .expect("select respun build");
    let fields = assert_success(drifted.run());
    assert_eq!(fields["journal_wrapper_state"], "drifted");
    assert_eq!(fields["tuple_state"], "drifted");

    let foreign = Fixture::new("foreign");
    let guard = foreign.install_owned_tuple();
    let foreign_guard = GuardFields {
        id: InstallationId::parse("ffeeddccbbaa99887766554433221100").expect("foreign id"),
        ..guard
    };
    foreign.write_wrappers(&foreign_guard, &foreign.first_version);
    foreign.write_service(&foreign_guard);
    let fields = assert_success(foreign.run());
    assert_eq!(fields["journal_wrapper_state"], "foreign");

    let malformed = Fixture::new("malformed");
    malformed.install_owned_tuple();
    let wrapper = malformed.home.join(".local/bin/journal");
    fs::write(
        &wrapper,
        format!(
            "{}# solstone-installation-id: duplicate\n",
            fs::read_to_string(&wrapper).expect("read wrapper")
        ),
    )
    .expect("corrupt wrapper guard");
    let fields = assert_success(malformed.run());
    assert_eq!(fields["journal_wrapper_state"], "malformed");

    let exact_v1 = Fixture::new("exact-v1");
    let legacy_bin = exact_v1.home.join(".local/share/uv/tools/solstone/bin");
    let public_bin = exact_v1.home.join(".local/bin");
    fs::create_dir_all(&legacy_bin).expect("create legacy launcher directory");
    fs::create_dir_all(&public_bin).expect("create public wrapper directory");
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
        .expect("make launcher executable");
    symlink(&legacy, public_bin.join("solstone")).expect("link legacy launcher");
    let fields = assert_success(exact_v1.run());
    assert_eq!(fields["solstone_wrapper_state"], "exact-v1");

    let unguarded = Fixture::new("unguarded");
    let guard = unguarded.install_owned_tuple();
    let wrapper = unguarded.home.join(".local/bin/journal");
    let unguarded_wrapper = render_wrapper(
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
    fs::write(&wrapper, format!("{unguarded_wrapper}\n")).expect("write unguarded wrapper");
    let fields = assert_success(unguarded.run());
    assert_eq!(fields["journal_wrapper_state"], "unguarded");

    let mixed = Fixture::new("mixed");
    let guard = mixed.install_owned_tuple();
    fs::remove_file(mixed.home.join(".local/bin/solstone")).expect("remove one wrapper");
    let drifted_guard = GuardFields {
        journal_token: JournalToken::from_raw_absolute(b"/different-journal".to_vec())
            .expect("different journal token"),
        ..guard
    };
    mixed.write_service(&drifted_guard);
    let fields = assert_success(mixed.run());
    assert_eq!(fields["journal_wrapper_state"], "aligned");
    assert_eq!(fields["solstone_wrapper_state"], "missing");
    assert_eq!(fields["service_state"], "drifted");

    let dangling = Fixture::new("dangling");
    dangling.install_owned_tuple();
    fs::remove_file(dangling.home.join(".local/bin/journal"))
        .expect("remove service launcher wrapper");
    let fields = assert_success(dangling.run());
    assert_eq!(fields["journal_wrapper_state"], "missing");
    assert_eq!(fields["service_state"], "dangling");
}

#[test]
fn inspection_uses_the_invoked_binary_not_path_or_argv_alias() {
    let fixture = Fixture::new("current-exe");
    fixture.install_owned_tuple();
    let baseline = assert_success(fixture.run());

    let poisoned = assert_success(fixture.run_from(
        &fixture.journal_dispatcher(),
        Some("/nonexistent:/also-nonexistent"),
    ));

    let alias_dir = fixture.root.join("argv-alias");
    fs::create_dir_all(&alias_dir).expect("create alias directory");
    let alias = alias_dir.join("journal-route-alias");
    symlink(fixture.journal_dispatcher(), &alias).expect("create dispatcher argv alias");
    let aliased = assert_success(fixture.run_from(&alias, None));

    for key in ["prefix_hex", "current_bin_hex", "current_state"] {
        assert_eq!(poisoned[key], baseline[key], "PATH changed {key}");
        assert_eq!(aliased[key], baseline[key], "argv alias changed {key}");
    }
}
