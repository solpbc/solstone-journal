// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use solstone_core_local::install::{
    lease,
    rfdetr_install::{
        RFDETR_MODEL_SHA256, RfdetrInstallRecord, check_rfdetr_model, install_rfdetr,
    },
    test_hooks,
};
use tar::{Builder, EntryType, Header};

const RECEIVE_TIMEOUT: Duration = Duration::from_secs(2);
const RFDETR_RECEIVE_TIMEOUT: Duration = Duration::from_secs(30);
const PARAKEET_TEST_KEY: &str = "x86_64-unknown-linux-gnu";
const COMPILED_EXPECTATION_ENV: &str = "SOLSTONE_RFDETR_COMPILED_EXPECTATION_RS";
const RFDETR_ARCHIVE_NAME: &str = "rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz";
const RFDETR_MODEL_NAME: &str = "rfdetr-nano-f16.gguf";
const RFDETR_SLOT_ID: &str = "rfdetr-macos-metal-arm64";
const RFDETR_ASSET_DEST: &str = "lib/solstone_journal_models/assets/rfdetr";
const RFDETR_PROBE_JOURNAL_ENV: &str = "SOLSTONE_RFDETR_PROBE_JOURNAL";
const RFDETR_PROBE_SOCKET_ENV: &str = "SOLSTONE_RFDETR_PROBE_SOCKET";

struct LeaseCase {
    socket: UnixDatagram,
    root: tempfile::TempDir,
}

impl LeaseCase {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("solstone-local-installer-process-")
            .tempdir()
            .expect("create lease journal root");
        let socket = UnixDatagram::bind(root.path().join("lease-probe.sock"))
            .expect("bind case-bound lease probe socket");
        socket
            .set_read_timeout(Some(RECEIVE_TIMEOUT))
            .expect("set lease probe receive timeout");
        Self { socket, root }
    }

    fn run_child(&self) -> ReapedChild {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "--ignored", "lease_child_probe"])
            .env("SOLSTONE_LOCAL_LEASE_HELPER_ROOT", self.root.path())
            .env(
                "SOLSTONE_LOCAL_LEASE_HELPER_SOCKET",
                self.root.path().join("lease-probe.sock"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run lease child probe");
        ReapedChild(Some(child))
    }

    fn receive(&self) -> String {
        let mut message = [0_u8; 64];
        let length = self
            .socket
            .recv(&mut message)
            .expect("receive case-bound lease probe result");
        std::str::from_utf8(&message[..length])
            .expect("lease probe result is utf-8")
            .to_owned()
    }

    fn assert_no_extra_message(&self) {
        self.socket
            .set_nonblocking(true)
            .expect("make lease probe socket nonblocking");
        let mut message = [0_u8; 64];
        match self.socket.recv(&mut message) {
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Ok(_) => panic!("lease probe sent more than one result"),
            Err(error) => panic!("check lease probe result count: {error}"),
        }
        self.socket
            .set_nonblocking(false)
            .expect("restore lease probe socket blocking mode");
    }
}

struct ReapedChild(Option<Child>);

impl ReapedChild {
    fn reap(mut self) -> ExitStatus {
        self.0
            .take()
            .expect("child is present")
            .wait()
            .expect("reap lease probe child")
    }
}

struct ArchiveFixture {
    bytes: Vec<u8>,
    sha256: String,
    executable_sha256: String,
    member_path: String,
}

impl ArchiveFixture {
    fn new(member_path: &str, executable: &[u8]) -> Self {
        let parent = Path::new(member_path)
            .parent()
            .and_then(Path::to_str)
            .expect("fixture member has a UTF-8 parent");
        let encoder = GzEncoder::new(Vec::new(), Compression::none());
        let mut archive = Builder::new(encoder);

        let mut directory = Header::new_gnu();
        directory.set_entry_type(EntryType::Directory);
        directory.set_path(parent).expect("fixture directory path");
        directory.set_size(0);
        directory.set_mode(0o755);
        directory.set_uid(0);
        directory.set_gid(0);
        directory.set_mtime(0);
        directory.set_cksum();
        archive
            .append(&directory, io::empty())
            .expect("append fixture directory");

        let mut binary = Header::new_gnu();
        binary.set_entry_type(EntryType::Regular);
        binary
            .set_path(member_path)
            .expect("fixture executable path");
        binary.set_size(executable.len() as u64);
        binary.set_mode(0o755);
        binary.set_uid(0);
        binary.set_gid(0);
        binary.set_mtime(0);
        binary.set_cksum();
        archive
            .append(&binary, executable)
            .expect("append fixture executable");
        let bytes = archive
            .into_inner()
            .expect("finish fixture tar")
            .finish()
            .expect("finish fixture gzip");
        Self {
            sha256: sha256_hex(&bytes),
            executable_sha256: sha256_hex(executable),
            bytes,
            member_path: member_path.to_owned(),
        }
    }
}

struct RfdetrCase {
    socket: UnixDatagram,
    root: tempfile::TempDir,
    journal_index: u32,
}

impl RfdetrCase {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("solstone-rfdetr-package-")
            .tempdir_in("/var/tmp")
            .expect("create fixture package root");
        let socket = UnixDatagram::bind(root.path().join("rfdetr-probe.sock"))
            .expect("bind fixture package probe socket");
        assert_eq!(
            sha256_file(&checked_in_rfdetr_model()),
            RFDETR_MODEL_SHA256,
            "checked-in model pin"
        );
        socket
            .set_read_timeout(Some(RFDETR_RECEIVE_TIMEOUT))
            .expect("set fixture package probe timeout");
        Self {
            socket,
            root,
            journal_index: 0,
        }
    }

    fn expectation_path(
        &self,
        name: &str,
        archive: &ArchiveFixture,
        executable_sha256: &str,
        executable_member_path: &str,
    ) -> PathBuf {
        let path = self.root.path().join(format!("{name}-expectation.rs"));
        let value = format!(
            "pub const MACOS_DELIVERY_CONTRACT: Option<CompiledDeliveryContract> = Some(CompiledDeliveryContract {{\n    delivery_contract_sha256: {:?},\n    slot_id: {:?},\n    archive_sha256: {:?},\n    archive_size: {},\n    executable_member_path: {:?},\n    executable_sha256: {:?},\n}});\n",
            sha256_hex(b"fixture-delivery-contract"),
            RFDETR_SLOT_ID,
            archive.sha256,
            archive.bytes.len(),
            executable_member_path,
            executable_sha256,
        );
        fs::write(&path, value).expect("write fixture compiled expectation");
        path
    }

    fn stage_assets(&self, archive: &ArchiveFixture) {
        let assets = self.root.path().join(RFDETR_ASSET_DEST);
        fs::create_dir_all(&assets).expect("create fixture package assets");
        fs::write(assets.join(RFDETR_ARCHIVE_NAME), &archive.bytes)
            .expect("stage fixture rfdetr archive");
        let staged_model = assets.join(RFDETR_MODEL_NAME);
        if !staged_model.exists() {
            let model = checked_in_rfdetr_model();
            if fs::hard_link(&model, &staged_model).is_err() {
                fs::copy(&model, &staged_model).expect("copy checked-in model fixture");
            }
        }
    }

    fn install_probe(&self, built: &Path) -> PathBuf {
        let bin = self.root.path().join("bin");
        fs::create_dir_all(&bin).expect("create fixture package bin");
        let probe = bin.join("rfdetr-install-probe");
        fs::copy(built, &probe).expect("copy freshly compiled probe binary");
        fs::set_permissions(&probe, fs::Permissions::from_mode(0o755))
            .expect("make fixture probe executable");
        probe
    }

    fn run_probe(&mut self, built: &Path) -> (String, PathBuf) {
        let probe = self.install_probe(built);
        let journal = self
            .root
            .path()
            .join(format!("journal-{}", self.journal_index));
        self.journal_index += 1;
        fs::create_dir_all(&journal).expect("create fixture journal");
        let child = Command::new(probe)
            .args(["--exact", "--ignored", "rfdetr_install_probe"])
            .current_dir(self.root.path())
            .env(RFDETR_PROBE_JOURNAL_ENV, &journal)
            .env(
                RFDETR_PROBE_SOCKET_ENV,
                self.root.path().join("rfdetr-probe.sock"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run freshly compiled rfdetr probe");
        let child = ReapedChild(Some(child));
        let mut message = [0_u8; 128];
        let length = self
            .socket
            .recv(&mut message)
            .expect("receive rfdetr probe result");
        assert!(child.reap().success(), "rfdetr probe exits cleanly");
        (
            std::str::from_utf8(&message[..length])
                .expect("rfdetr probe result is utf-8")
                .to_owned(),
            journal,
        )
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crate is nested below repository root")
        .to_path_buf()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> String {
    sha256_hex(&fs::read(path).expect("read fixture file"))
}

fn checked_in_rfdetr_model() -> PathBuf {
    repository_root()
        .join("core/models/assets/rfdetr")
        .join(RFDETR_MODEL_NAME)
}

fn build_fresh_rfdetr_probe(expectation: Option<&Path>) -> PathBuf {
    let core = repository_root().join("core");
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .args([
            "test",
            "--manifest-path",
            core.join("Cargo.toml")
                .to_str()
                .expect("UTF-8 manifest path"),
            "-p",
            "solstone-core-local",
            "--features",
            "test-hooks",
            "--test",
            "local_installer_process",
            "--no-run",
            "--message-format=json",
        ])
        .current_dir(repository_root());
    match expectation {
        Some(path) => {
            command.env(COMPILED_EXPECTATION_ENV, path);
        }
        None => {
            command.env_remove(COMPILED_EXPECTATION_ENV);
        }
    }
    let output = command.output().expect("build fresh rfdetr probe");
    assert!(
        output.status.success(),
        "fresh rfdetr probe build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .find_map(|message| {
            (message["reason"] == "compiler-artifact"
                && message["target"]["name"] == "local_installer_process")
                .then(|| message["executable"].as_str().map(PathBuf::from))
                .flatten()
        })
        .expect("fresh local_installer_process compiler artifact")
}

fn assert_refusal(result: String, journal: &Path, reason: &str) {
    assert_eq!(result, format!("error:{reason}"));
    assert!(
        !contains_regular_file(&journal.join("cache/providers/rfdetr")),
        "refused probe must not leave a rf-detr artifact or sidecar"
    );
}

fn contains_regular_file(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    fs::read_dir(path)
        .expect("read rf-detr cache")
        .flatten()
        .any(|entry| {
            entry
                .file_type()
                .expect("inspect rf-detr cache entry")
                .is_file()
                || (entry.path().is_dir() && contains_regular_file(&entry.path()))
        })
}

impl Drop for ReapedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

fn child_result(root: &Path) -> Result<&'static str, String> {
    match lease::acquire(root, "local") {
        Ok(None) => Ok("contended"),
        Ok(Some(_lease)) => Ok("acquired-and-released"),
        Err(error) => Err(format!("lease acquire failed: {error}")),
    }
}

#[test]
fn two_real_processes_cannot_hold_the_same_lease() {
    let case = LeaseCase::new();
    let held = lease::acquire(case.root.path(), "local").unwrap().unwrap();
    let child = case.run_child();
    assert_eq!(case.receive(), "contended");
    assert!(child.reap().success(), "contended child must exit cleanly");
    case.assert_no_extra_message();
    drop(held);
    let child = case.run_child();
    assert_eq!(case.receive(), "acquired-and-released");
    assert!(
        child.reap().success(),
        "acquired-and-released child must exit cleanly"
    );
    case.assert_no_extra_message();
}

#[test]
fn inspect_parakeet_reports_ready_per_artifact_proofs() {
    let root = tempfile::Builder::new()
        .prefix("solstone-local-parakeet-ready-")
        .tempdir()
        .expect("create ready Parakeet journal");
    let _fixture = test_hooks::stage_ready_parakeet(root.path(), PARAKEET_TEST_KEY, true);
    let result = test_hooks::inspect_parakeet(root.path(), PARAKEET_TEST_KEY);

    assert_eq!(result["provider"], "parakeet");
    assert_eq!(result["target"]["artifact_key"], PARAKEET_TEST_KEY);
    assert_eq!(result["status"], "ready");
    assert_eq!(result["reason_code"], "ready");
    assert_eq!(result["ready"], true);
    assert_eq!(result["in_flight"], false);
    assert_eq!(result["artifacts"]["binary_installed"], true);
    assert_eq!(result["artifacts"]["binary_runnable"], true);
    for name in ["binary", "binary_cpu", "binary_vulkan", "model"] {
        assert_eq!(result["proof"][name]["status"], "ready", "{name}");
        assert!(result["proof"][name].get("cache_hit").is_none(), "{name}");
    }
    assert!(result["install"].is_object());
}

#[test]
fn inspect_parakeet_reports_held_lease_without_creating_a_lease() {
    let root = tempfile::Builder::new()
        .prefix("solstone-local-parakeet-lease-")
        .tempdir()
        .expect("create lease Parakeet journal");
    let _fixture = test_hooks::stage_ready_parakeet(root.path(), PARAKEET_TEST_KEY, true);
    let lease_path = lease::lease_path(root.path(), "parakeet");
    assert!(!lease_path.exists());
    let unlocked = test_hooks::inspect_parakeet(root.path(), PARAKEET_TEST_KEY);
    assert_eq!(unlocked["in_flight"], false);
    assert!(!lease_path.exists());

    let held = lease::acquire(root.path(), "parakeet")
        .expect("acquire Parakeet lease")
        .expect("lease is available");
    let locked = test_hooks::inspect_parakeet(root.path(), PARAKEET_TEST_KEY);
    assert_eq!(locked["in_flight"], true);
    assert_eq!(locked["ready"], true);
    assert_eq!(locked["status"], "ready");
    drop(held);
}

#[test]
fn inspect_parakeet_reports_unrunnable_cpu_binary() {
    let root = tempfile::Builder::new()
        .prefix("solstone-local-parakeet-unrunnable-")
        .tempdir()
        .expect("create unrunnable Parakeet journal");
    let _fixture = test_hooks::stage_ready_parakeet(root.path(), PARAKEET_TEST_KEY, false);
    let result = test_hooks::inspect_parakeet(root.path(), PARAKEET_TEST_KEY);

    assert_eq!(result["status"], "host-ineligible");
    assert_eq!(result["reason_code"], "binary_unavailable");
    assert_eq!(result["artifacts"]["binary_runnable"], false);
    assert_eq!(
        result["host"]["binary_runtime"]["reason_code"],
        "binary_unavailable"
    );
    for name in ["binary", "binary_cpu", "binary_vulkan", "model"] {
        assert_eq!(result["proof"][name]["status"], "ready", "{name}");
    }
}

#[test]
fn rfdetr_installer_uses_a_freshly_compiled_delivery_contract() {
    let mut case = RfdetrCase::new();
    let expected_member = "rfdetr-fixture/rfdetr-cli";
    let signed = ArchiveFixture::new(expected_member, &[b'S'; 64]);
    let unsigned = ArchiveFixture::new(expected_member, &[b'U'; 64]);
    let different = ArchiveFixture::new(expected_member, &[b'D'; 64]);
    let altered = ArchiveFixture::new(expected_member, &[b'A'; 64]);
    let moved = ArchiveFixture::new("moved/rfdetr-cli", &[b'S'; 64]);

    let signed_expectation = case.expectation_path(
        "signed",
        &signed,
        &signed.executable_sha256,
        &signed.member_path,
    );
    let signed_probe = build_fresh_rfdetr_probe(Some(&signed_expectation));

    // The same compiled contract first refuses the unsigned source stand-in,
    // then accepts the exact staged derivative it names.
    case.stage_assets(&unsigned);
    let (result, journal) = case.run_probe(&signed_probe);
    assert_refusal(result, &journal, "sha256_mismatch");

    case.stage_assets(&signed);
    let (result, journal) = case.run_probe(&signed_probe);
    assert_eq!(result, "healthy");
    assert!(journal.join("cache/providers/rfdetr").is_dir());

    case.stage_assets(&different);
    let (result, journal) = case.run_probe(&signed_probe);
    assert_refusal(result, &journal, "sha256_mismatch");

    let altered_expectation = case.expectation_path(
        "altered-executable",
        &altered,
        &signed.executable_sha256,
        &signed.member_path,
    );
    let altered_probe = build_fresh_rfdetr_probe(Some(&altered_expectation));
    case.stage_assets(&altered);
    let (result, journal) = case.run_probe(&altered_probe);
    assert_refusal(result, &journal, "sha256_mismatch");

    let moved_expectation = case.expectation_path(
        "moved-member",
        &moved,
        &moved.executable_sha256,
        &signed.member_path,
    );
    let moved_probe = build_fresh_rfdetr_probe(Some(&moved_expectation));
    case.stage_assets(&moved);
    let (result, journal) = case.run_probe(&moved_probe);
    assert_refusal(result, &journal, "member_path_mismatch");

    let missing_contract_probe = build_fresh_rfdetr_probe(None);
    case.stage_assets(&signed);
    let (result, journal) = case.run_probe(&missing_contract_probe);
    assert_refusal(result, &journal, "compiled_delivery_contract_missing");
}

#[test]
#[ignore]
fn lease_child_probe() {
    let root = std::env::var("SOLSTONE_LOCAL_LEASE_HELPER_ROOT")
        .map_err(|_| "SOLSTONE_LOCAL_LEASE_HELPER_ROOT must name the lease journal")
        .map(std::path::PathBuf::from)
        .unwrap();
    let socket_path = std::env::var("SOLSTONE_LOCAL_LEASE_HELPER_SOCKET")
        .map_err(|_| "SOLSTONE_LOCAL_LEASE_HELPER_SOCKET must name the case socket")
        .unwrap();
    let result = child_result(&root).unwrap();
    let sender = UnixDatagram::unbound().unwrap();
    sender
        .send_to(result.as_bytes(), socket_path)
        .map(|_| ())
        .unwrap_or_else(|error: io::Error| panic!("send lease probe result: {error}"));
}

#[test]
#[ignore]
fn rfdetr_install_probe() {
    let journal = std::env::var(RFDETR_PROBE_JOURNAL_ENV)
        .map(PathBuf::from)
        .expect("SOLSTONE_RFDETR_PROBE_JOURNAL must name the probe journal");
    let socket_path = std::env::var(RFDETR_PROBE_SOCKET_ENV)
        .expect("SOLSTONE_RFDETR_PROBE_SOCKET must name the probe socket");
    let result = match install_rfdetr(&journal, "darwin", "arm64", false) {
        Ok(RfdetrInstallRecord::Installed) => {
            match check_rfdetr_model(&journal, "darwin", "arm64") {
                Ok(RfdetrInstallRecord::Installed) => "healthy".to_owned(),
                Ok(RfdetrInstallRecord::PlatformUnavailable) => {
                    "error:platform_unavailable".to_owned()
                }
                Err(error) => format!("error:{}", error.reason_code),
            }
        }
        Ok(RfdetrInstallRecord::PlatformUnavailable) => "error:platform_unavailable".to_owned(),
        Err(error) => format!("error:{}", error.reason_code),
    };
    let sender = UnixDatagram::unbound().expect("create rfdetr probe socket");
    sender
        .send_to(result.as_bytes(), socket_path)
        .map(|_| ())
        .unwrap_or_else(|error| panic!("send rfdetr probe result: {error}"));
}
