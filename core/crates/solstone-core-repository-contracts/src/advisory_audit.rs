// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Authenticated, mirror-bound, offline cargo-deny execution.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const PRODUCT: &str = "solstone-journal";
const SOURCE_COHORT: &str = "sol-controlled-rustsec-mirror-v1";
const CARGO_DENY_VERSION: &str = "cargo-deny 0.20.2";
const PUBLIC_KEY_SHA256: &str = "c9fb713fe57791afbdebddde7b334e950ce1efcc167d49daf4cc1cbd930bb122";
const PUBLIC_KEY_ID: &str = "5FCC81CD3DE12315";
const RECEIPT_MAX_AGE: u64 = 86_400;
const FUTURE_TOLERANCE: u64 = 300;
const CACHE_HASH_SEED: u64 = 0xca80de71;

#[derive(Clone, Debug)]
pub struct AdvisoryAuditRequest {
    pub bundle: PathBuf,
    pub receipt: PathBuf,
    pub public_key: PathBuf,
    pub locator: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FreshnessReceipt {
    max_age: u64,
    synced_commit: String,
    utc: String,
}

#[derive(Clone, Debug, Serialize)]
struct AdvisoryPackage {
    name: String,
    version: String,
}

#[derive(Clone, Debug, Serialize)]
struct AdvisoryFinding {
    id: String,
    packages: Vec<AdvisoryPackage>,
}

#[derive(Clone, Debug, Serialize)]
struct AdvisoryWitness {
    schema: &'static str,
    product: &'static str,
    source_cohort: &'static str,
    synced_commit: String,
    receipt_utc: String,
    receipt_sha256: String,
    receipt_signature_sha256: String,
    bundle_sha256: String,
    public_key_sha256: &'static str,
    public_key_id: &'static str,
    max_age: u64,
    checked_at: String,
    cargo_lock_sha256: String,
    cargo_deny_version: &'static str,
    advisories: Vec<AdvisoryFinding>,
    verdict: &'static str,
}

struct Programs {
    git: PathBuf,
    minisign: PathBuf,
    cargo_deny: PathBuf,
}

struct PacketFiles {
    receipt: Vec<u8>,
    signature: Vec<u8>,
    public_key: Vec<u8>,
    bundle: Vec<u8>,
}

pub fn run_advisory_audit(repo: &Path, request: &AdvisoryAuditRequest) -> Result<String, String> {
    let receipt_path = safe_absolute_file(&request.receipt, "freshness receipt")?;
    let signature_path = PathBuf::from(format!("{}.minisig", receipt_path.display()));
    let signature_path = safe_absolute_file(&signature_path, "freshness receipt signature")?;
    let public_key_path = safe_absolute_file(&request.public_key, "mirror public key")?;
    let bundle_path = safe_absolute_file(&request.bundle, "advisory mirror bundle")?;
    let database_name = validate_locator(&request.locator)?;
    let packet = PacketFiles {
        receipt: fs::read(&receipt_path)
            .map_err(|error| format!("read freshness receipt: {error}"))?,
        signature: fs::read(&signature_path)
            .map_err(|error| format!("read freshness receipt signature: {error}"))?,
        public_key: fs::read(&public_key_path)
            .map_err(|error| format!("read mirror public key: {error}"))?,
        bundle: fs::read(&bundle_path)
            .map_err(|error| format!("read advisory mirror bundle: {error}"))?,
    };
    if sha256_hex(&packet.public_key) != PUBLIC_KEY_SHA256 {
        return Err("mirror public key does not match the pinned identity".to_owned());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("read host clock: {error}"))?
        .as_secs();
    let receipt = validate_receipt(&packet.receipt, &packet.signature, now)?;
    let programs = Programs {
        git: find_program("git")?,
        minisign: find_program("minisign")?,
        cargo_deny: find_program("cargo-deny")?,
    };
    let run_root = create_run_root()?;
    let result = run_in_scratch(
        repo,
        request,
        &programs,
        &receipt,
        &packet,
        &database_name,
        &run_root,
        now,
    );
    let cleanup = fs::remove_dir_all(&run_root);
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(format!("remove advisory audit scratch: {error}")),
        (Ok(witness), Ok(())) => Ok(witness),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_in_scratch(
    repo: &Path,
    request: &AdvisoryAuditRequest,
    programs: &Programs,
    receipt: &FreshnessReceipt,
    packet: &PacketFiles,
    database_name: &str,
    run_root: &Path,
    now: u64,
) -> Result<String, String> {
    let packet_root = run_root.join("packet");
    let database_root = run_root.join("advisory-dbs");
    let verify_root = run_root.join("bundle-verify");
    let template_root = run_root.join("git-template");
    fs::create_dir(&packet_root).map_err(|error| format!("create packet copy root: {error}"))?;
    fs::create_dir(&database_root).map_err(|error| format!("create database root: {error}"))?;
    fs::create_dir(&verify_root)
        .map_err(|error| format!("create bundle verification root: {error}"))?;
    fs::create_dir(&template_root).map_err(|error| format!("create Git template root: {error}"))?;
    let gitconfig = run_root.join("gitconfig");
    write_new(&gitconfig, b"")?;

    let receipt_path = packet_root.join("freshness.json");
    let signature_path = packet_root.join("freshness.json.minisig");
    let public_key_path = packet_root.join("mirror.pub");
    let bundle_path = packet_root.join("advisory-db.bundle");
    write_new(&receipt_path, &packet.receipt)?;
    write_new(&signature_path, &packet.signature)?;
    write_new(&public_key_path, &packet.public_key)?;
    write_new(&bundle_path, &packet.bundle)?;
    verify_signature(
        &programs.minisign,
        &receipt_path,
        &signature_path,
        &public_key_path,
    )?;

    run_git(
        &programs.git,
        &verify_root,
        &["init", "--initial-branch=main"],
        &gitconfig,
        &template_root,
        "initialize bundle verification repository",
    )?;
    run_git(
        &programs.git,
        &verify_root,
        &["bundle", "verify", path_text(&bundle_path)?],
        &gitconfig,
        &template_root,
        "verify advisory bundle",
    )?;
    let heads = run_git(
        &programs.git,
        &verify_root,
        &["bundle", "list-heads", path_text(&bundle_path)?],
        &gitconfig,
        &template_root,
        "inspect advisory bundle heads",
    )?;
    verify_bundle_heads(&heads.stdout, &receipt.synced_commit)?;

    let checkout = database_root.join(database_name);
    run_git(
        &programs.git,
        run_root,
        &[
            "clone",
            "--no-checkout",
            "--no-tags",
            path_text(&bundle_path)?,
            path_text(&checkout)?,
        ],
        &gitconfig,
        &template_root,
        "materialize advisory bundle",
    )?;
    run_git(
        &programs.git,
        &checkout,
        &["checkout", "--detach", "--force", &receipt.synced_commit],
        &gitconfig,
        &template_root,
        "checkout signed advisory commit",
    )?;
    let head = run_git(
        &programs.git,
        &checkout,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        &gitconfig,
        &template_root,
        "read advisory checkout head",
    )?;
    if strict_line(&head.stdout) != Some(receipt.synced_commit.as_str()) {
        return Err("advisory checkout does not match the signed commit".to_owned());
    }
    let shallow = run_git(
        &programs.git,
        &checkout,
        &["rev-parse", "--is-shallow-repository"],
        &gitconfig,
        &template_root,
        "inspect advisory checkout depth",
    )?;
    if strict_line(&shallow.stdout) != Some("false") {
        return Err("advisory checkout is shallow".to_owned());
    }
    let status = run_git(
        &programs.git,
        &checkout,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        &gitconfig,
        &template_root,
        "inspect advisory checkout status",
    )?;
    if !status.stdout.is_empty() {
        return Err("advisory checkout is dirty".to_owned());
    }

    let cargo_deny_version = run_clean(&programs.cargo_deny, repo, &["--version"], &[])?;
    if !cargo_deny_version.status.success()
        || strict_line(&cargo_deny_version.stdout) != Some(CARGO_DENY_VERSION)
    {
        return Err(format!("audit requires exactly {CARGO_DENY_VERSION}"));
    }

    let config_path = run_root.join("deny.toml");
    materialize_config(repo, &config_path, &database_root, &request.locator)?;
    let cargo_lock_path = repo.join("core/Cargo.lock");
    let lock_before = fs::read(&cargo_lock_path)
        .map_err(|error| format!("read core/Cargo.lock before audit: {error}"))?;
    let mut advisory_args = vec![
        "--format".to_owned(),
        "json".to_owned(),
        "--manifest-path".to_owned(),
        path_text(&repo.join("core/Cargo.toml"))?.to_owned(),
        "--locked".to_owned(),
        "--offline".to_owned(),
        "--config".to_owned(),
        path_text(&config_path)?.to_owned(),
    ];
    advisory_args.extend([
        "check".to_owned(),
        "advisories".to_owned(),
        "--audit-compatible-output".to_owned(),
    ]);
    let advisory_output = run_clean_owned(
        &programs.cargo_deny,
        repo,
        &advisory_args,
        &[("CARGO_NET_OFFLINE", "true")],
    )?;
    if !advisory_output.status.success() {
        return Err(format!(
            "cargo-deny rejected the authenticated advisory packet: {}",
            compact_stderr(&advisory_output.stderr)
        ));
    }
    let advisories = parse_advisories(&advisory_output.stdout)?;
    let lock_after = fs::read(&cargo_lock_path)
        .map_err(|error| format!("read core/Cargo.lock after audit: {error}"))?;
    if lock_after != lock_before {
        return Err("core/Cargo.lock changed during advisory audit".to_owned());
    }
    verify_packet_unchanged(request, packet)?;
    let witness = AdvisoryWitness {
        schema: "solstone.advisory-audit.v1",
        product: PRODUCT,
        source_cohort: SOURCE_COHORT,
        synced_commit: receipt.synced_commit.clone(),
        receipt_utc: receipt.utc.clone(),
        receipt_sha256: sha256_hex(&packet.receipt),
        receipt_signature_sha256: sha256_hex(&packet.signature),
        bundle_sha256: sha256_hex(&packet.bundle),
        public_key_sha256: PUBLIC_KEY_SHA256,
        public_key_id: PUBLIC_KEY_ID,
        max_age: receipt.max_age,
        checked_at: format_utc(now),
        cargo_lock_sha256: sha256_hex(&lock_before),
        cargo_deny_version: CARGO_DENY_VERSION,
        advisories,
        verdict: "pass",
    };
    serde_json::to_string(&witness).map_err(|error| format!("serialize advisory witness: {error}"))
}

fn safe_absolute_file(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} path must be absolute"));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("read {label} metadata {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{label} is not one regular non-link file"));
    }
    fs::canonicalize(path).map_err(|error| format!("resolve {label}: {error}"))
}

fn validate_locator(locator: &str) -> Result<String, String> {
    if locator.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err("advisory mirror locator contains whitespace".to_owned());
    }
    let lower = locator.to_ascii_lowercase();
    if lower.contains("github.com/rustsec/advisory-db") {
        return Err("public RustSec GitHub locator is forbidden".to_owned());
    }
    let rest = lower
        .strip_prefix("ssh://")
        .or_else(|| lower.strip_prefix("https://"))
        .ok_or_else(|| "advisory mirror locator must use ssh:// or https://".to_owned())?;
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| "advisory mirror locator has no repository path".to_owned())?;
    if authority.is_empty() || path.is_empty() || path.contains('?') || path.contains('#') {
        return Err("advisory mirror locator is malformed".to_owned());
    }
    let terminal = path
        .split('/')
        .next_back()
        .ok_or_else(|| "advisory mirror locator has no terminal name".to_owned())?;
    if !matches!(terminal, "advisory-db" | "rustsec-advisory-db.git") {
        return Err(
            "advisory mirror locator must end in advisory-db or rustsec-advisory-db.git".to_owned(),
        );
    }
    let hash = xxh64(lower.as_bytes(), CACHE_HASH_SEED);
    Ok(format!("{terminal}-{hash:016x}"))
}

fn validate_receipt(bytes: &[u8], signature: &[u8], now: u64) -> Result<FreshnessReceipt, String> {
    let receipt: FreshnessReceipt = serde_json::from_slice(bytes)
        .map_err(|error| format!("freshness receipt is malformed: {error}"))?;
    if receipt.max_age != RECEIPT_MAX_AGE {
        return Err(format!(
            "freshness receipt max_age must be {RECEIPT_MAX_AGE}"
        ));
    }
    if !is_lower_hex(&receipt.synced_commit, 40) {
        return Err("freshness receipt commit must be one full lowercase Git object id".to_owned());
    }
    let receipt_time = parse_utc(&receipt.utc)
        .ok_or_else(|| "freshness receipt utc must be canonical UTC seconds".to_owned())?;
    if receipt_time > now.saturating_add(FUTURE_TOLERANCE) {
        return Err("freshness receipt is too far in the future".to_owned());
    }
    if now.saturating_sub(receipt_time) > RECEIPT_MAX_AGE {
        return Err("freshness receipt is stale".to_owned());
    }
    let mut canonical = serde_json::to_vec(&receipt)
        .map_err(|error| format!("serialize freshness receipt: {error}"))?;
    canonical.push(b'\n');
    if bytes != canonical {
        return Err("freshness receipt body is not canonical JSON".to_owned());
    }
    let trusted = format!(
        "trusted comment: solpbc-advisory-mirror-v1 synced_commit={} utc={} max_age={}",
        receipt.synced_commit, receipt.utc, receipt.max_age
    );
    let signature_text = std::str::from_utf8(signature)
        .map_err(|_| "freshness receipt signature is not UTF-8".to_owned())?;
    let comments: Vec<&str> = signature_text
        .lines()
        .filter(|line| line.starts_with("trusted comment:"))
        .collect();
    if comments != [trusted.as_str()] {
        return Err("freshness receipt trusted comment is not canonical".to_owned());
    }
    Ok(receipt)
}

fn verify_signature(
    minisign: &Path,
    receipt: &Path,
    signature: &Path,
    public_key: &Path,
) -> Result<(), String> {
    let output = run_clean(
        minisign,
        Path::new("/var/tmp"),
        &[
            "-Vm",
            path_text(receipt)?,
            "-x",
            path_text(signature)?,
            "-p",
            path_text(public_key)?,
        ],
        &[],
    )?;
    if !output.status.success() {
        return Err(format!(
            "freshness receipt signature is invalid: {}",
            compact_stderr(&output.stderr)
        ));
    }
    Ok(())
}

fn verify_bundle_heads(bytes: &[u8], commit: &str) -> Result<(), String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "advisory bundle heads are not UTF-8".to_owned())?;
    let mut heads = BTreeSet::new();
    for line in text.lines() {
        let (oid, name) = line
            .split_once(' ')
            .ok_or_else(|| "advisory bundle head row is malformed".to_owned())?;
        if oid != commit || name.contains(char::is_whitespace) || !heads.insert(name) {
            return Err("advisory bundle heads do not match the signed commit".to_owned());
        }
    }
    if heads != BTreeSet::from(["HEAD", "refs/heads/main"]) {
        return Err(
            "advisory bundle must advertise exactly signed HEAD and refs/heads/main".to_owned(),
        );
    }
    Ok(())
}

fn materialize_config(
    repo: &Path,
    destination: &Path,
    database_root: &Path,
    locator: &str,
) -> Result<(), String> {
    let source = fs::read_to_string(repo.join("core/deny.toml"))
        .map_err(|error| format!("read core/deny.toml: {error}"))?;
    let header = "[advisories]\n";
    let insertion = format!(
        "{header}db-path = {}\ndb-urls = [{}]\n",
        serde_json::to_string(path_text(database_root)?)
            .map_err(|error| format!("encode advisory database path: {error}"))?,
        serde_json::to_string(locator)
            .map_err(|error| format!("encode advisory mirror locator: {error}"))?
    );
    let rendered = source.replacen(header, &insertion, 1);
    if rendered == source {
        return Err("core/deny.toml has no [advisories] table".to_owned());
    }
    write_new(destination, rendered.as_bytes())
}

fn parse_advisories(bytes: &[u8]) -> Result<Vec<AdvisoryFinding>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "cargo-deny advisory output is not UTF-8".to_owned())?;
    let mut findings: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut parsed = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| format!("parse cargo-deny advisory JSON: {error}"))?;
        collect_advisories(&value, &mut findings);
        parsed += 1;
    }
    if parsed == 0 {
        return Err("cargo-deny advisory output contained no JSON records".to_owned());
    }
    Ok(findings
        .into_iter()
        .map(|(id, packages)| AdvisoryFinding {
            id,
            packages: packages
                .into_iter()
                .map(|(name, version)| AdvisoryPackage { name, version })
                .collect(),
        })
        .collect())
}

fn collect_advisories(value: &Value, findings: &mut BTreeMap<String, BTreeSet<(String, String)>>) {
    match value {
        Value::Object(object) => {
            if let (Some(advisory), Some(package)) = (object.get("advisory"), object.get("package"))
                && let (Some(id), Some(name), Some(version)) = (
                    advisory.get("id").and_then(Value::as_str),
                    package.get("name").and_then(Value::as_str),
                    package.get("version").and_then(Value::as_str),
                )
            {
                findings
                    .entry(id.to_owned())
                    .or_default()
                    .insert((name.to_owned(), version.to_owned()));
            }
            for child in object.values() {
                collect_advisories(child, findings);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect_advisories(child, findings);
            }
        }
        _ => {}
    }
}

fn run_git(
    program: &Path,
    cwd: &Path,
    args: &[&str],
    gitconfig: &Path,
    template_root: &Path,
    label: &str,
) -> Result<Output, String> {
    let gitconfig_text = path_text(gitconfig)?;
    let template_text = path_text(template_root)?;
    let output = run_clean(
        program,
        cwd,
        args,
        &[
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", gitconfig_text),
            ("GIT_CONFIG_COUNT", "0"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_ALLOW_PROTOCOL", "file"),
            ("GIT_PROTOCOL_FROM_USER", "0"),
            ("GIT_TEMPLATE_DIR", template_text),
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "{label} failed: {}",
            compact_stderr(&output.stderr)
        ));
    }
    Ok(output)
}

fn run_clean(
    program: &Path,
    cwd: &Path,
    args: &[&str],
    extra_environment: &[(&str, &str)],
) -> Result<Output, String> {
    let owned: Vec<String> = args.iter().map(|value| (*value).to_owned()).collect();
    run_clean_owned(program, cwd, &owned, extra_environment)
}

fn run_clean_owned(
    program: &Path,
    cwd: &Path,
    args: &[String],
    extra_environment: &[(&str, &str)],
) -> Result<Output, String> {
    if !program.is_absolute() {
        return Err("audit child program is not absolute".to_owned());
    }
    let mut command = Command::new(program);
    command
        .env_clear()
        .current_dir(cwd)
        .args(args)
        .env("LC_ALL", "C");
    for name in ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME"] {
        if let Some(value) = env::var_os(name) {
            command.env(name, value);
        }
    }
    for (name, value) in extra_environment {
        command.env(name, value);
    }
    command
        .output()
        .map_err(|error| format!("launch {}: {error}", program.display()))
}

fn find_program(name: &str) -> Result<PathBuf, String> {
    let path = env::var_os("PATH").ok_or_else(|| format!("PATH is unavailable for {name}"))?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return fs::canonicalize(&candidate)
                .map_err(|error| format!("resolve {name} program: {error}"));
        }
    }
    Err(format!("required audit program is unavailable: {name}"))
}

fn create_run_root() -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("read host clock: {error}"))?
        .as_nanos();
    let root = PathBuf::from(format!(
        "/var/tmp/solstone-journal-advisory-audit-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&root).map_err(|error| format!("create advisory audit scratch: {error}"))?;
    Ok(root)
}

fn verify_packet_unchanged(
    request: &AdvisoryAuditRequest,
    before: &PacketFiles,
) -> Result<(), String> {
    let signature_path = PathBuf::from(format!("{}.minisig", request.receipt.display()));
    let after = PacketFiles {
        receipt: fs::read(&request.receipt)
            .map_err(|error| format!("re-read freshness receipt: {error}"))?,
        signature: fs::read(&signature_path)
            .map_err(|error| format!("re-read freshness receipt signature: {error}"))?,
        public_key: fs::read(&request.public_key)
            .map_err(|error| format!("re-read mirror public key: {error}"))?,
        bundle: fs::read(&request.bundle)
            .map_err(|error| format!("re-read advisory mirror bundle: {error}"))?,
    };
    if after.receipt != before.receipt
        || after.signature != before.signature
        || after.public_key != before.public_key
        || after.bundle != before.bundle
    {
        return Err("advisory packet changed during audit".to_owned());
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn strict_line(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    let line = text.strip_suffix('\n')?;
    if line.is_empty() || line.contains(['\n', '\r']) {
        None
    } else {
        Some(line)
    }
}

fn compact_stderr(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .split_whitespace()
        .take(40)
        .collect::<Vec<_>>()
        .join(" ")
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not UTF-8: {}", path.display()))
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_utc(value: &str) -> Option<u64> {
    if value.len() != 20 {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[index].is_ascii_digit() {
            return None;
        }
    }
    let number = |start: usize, end: usize| value[start..end].parse::<i64>().ok();
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1..=12).contains(&month) || hour >= 24 || minute >= 60 || second >= 60 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day < 1 || day > days_in_month {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

fn format_utc(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remainder / 3_600,
        remainder % 3_600 / 60,
        remainder % 60
    )
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = yoe * 365 + yoe / 4 - yoe / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn xxh64(input: &[u8], seed: u64) -> u64 {
    const P1: u64 = 11_400_714_785_074_694_791;
    const P2: u64 = 14_029_467_366_897_019_727;
    const P3: u64 = 1_609_587_929_392_839_161;
    const P4: u64 = 9_650_029_242_287_828_579;
    const P5: u64 = 2_870_177_450_012_600_261;
    let round = |acc: u64, lane: u64| {
        acc.wrapping_add(lane.wrapping_mul(P2))
            .rotate_left(31)
            .wrapping_mul(P1)
    };
    let merge = |acc: u64, lane: u64| (acc ^ round(0, lane)).wrapping_mul(P1).wrapping_add(P4);
    let mut offset = 0;
    let mut hash = if input.len() >= 32 {
        let mut v1 = seed.wrapping_add(P1).wrapping_add(P2);
        let mut v2 = seed.wrapping_add(P2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(P1);
        while offset <= input.len() - 32 {
            v1 = round(v1, read_u64(&input[offset..]));
            v2 = round(v2, read_u64(&input[offset + 8..]));
            v3 = round(v3, read_u64(&input[offset + 16..]));
            v4 = round(v4, read_u64(&input[offset + 24..]));
            offset += 32;
        }
        let hash = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
        merge(merge(merge(merge(hash, v1), v2), v3), v4)
    } else {
        seed.wrapping_add(P5)
    };
    hash = hash.wrapping_add(input.len() as u64);
    while offset + 8 <= input.len() {
        let lane = round(0, read_u64(&input[offset..]));
        hash ^= lane;
        hash = hash.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        offset += 8;
    }
    if offset + 4 <= input.len() {
        hash ^= u64::from(read_u32(&input[offset..])).wrapping_mul(P1);
        hash = hash.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
        offset += 4;
    }
    while offset < input.len() {
        hash ^= u64::from(input[offset]).wrapping_mul(P5);
        hash = hash.rotate_left(11).wrapping_mul(P1);
        offset += 1;
    }
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(P2);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(P3);
    hash ^ (hash >> 32)
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes[..8].try_into().expect("eight-byte xxhash lane"))
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().expect("four-byte xxhash lane"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::{
        FreshnessReceipt, format_utc, materialize_config, parse_advisories, parse_utc,
        validate_locator, validate_receipt, verify_bundle_heads, xxh64,
    };
    use std::fs;

    const NOW: u64 = 1_777_593_600;

    fn receipt(utc: &str) -> (Vec<u8>, Vec<u8>) {
        let value = FreshnessReceipt {
            max_age: 86_400,
            synced_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            utc: utc.to_owned(),
        };
        let mut body = serde_json::to_vec(&value).expect("serialize receipt");
        body.push(b'\n');
        let signature = format!(
            "untrusted comment: synthetic\nAAAA\ntrusted comment: solpbc-advisory-mirror-v1 synced_commit={} utc={} max_age={}\nBBBB\n",
            value.synced_commit, value.utc, value.max_age
        )
        .into_bytes();
        (body, signature)
    }

    #[test]
    fn receipt_controls_vary_each_rejected_coordinate() {
        let now = parse_utc("2026-05-01T00:00:00Z").expect("now");
        let (body, signature) = receipt("2026-04-30T23:59:59Z");
        assert!(validate_receipt(&body, &signature, now).is_ok());

        let mut malformed = body.clone();
        malformed[0] = b'[';
        assert!(validate_receipt(&malformed, &signature, now).is_err());
        let (stale, stale_signature) = receipt("2026-04-29T23:59:59Z");
        assert!(validate_receipt(&stale, &stale_signature, now).is_err());
        let (future, future_signature) = receipt("2026-05-01T00:05:01Z");
        assert!(validate_receipt(&future, &future_signature, now).is_err());
        let mut wrong_comment = signature.clone();
        let index = wrong_comment
            .windows(14)
            .position(|window| window == b"synced_commit=")
            .expect("comment commit");
        wrong_comment[index + 14] = b'f';
        assert!(validate_receipt(&body, &wrong_comment, now).is_err());
    }

    #[test]
    fn locator_controls_reject_public_or_wrong_identity() {
        let locator =
            "ssh://jer@fedora.local/data/git/advisory-mirrors/rustsec/rustsec-advisory-db.git";
        let name = validate_locator(locator).expect("private locator");
        assert!(name.starts_with("rustsec-advisory-db.git-"));
        for invalid in [
            "git@fedora.local:advisory-db",
            "https://github.com/RustSec/advisory-db",
            "ssh://fedora.local/data/git/not-rustsec.git",
            "ssh://fedora.local/data/git/advisory-db?ref=main",
        ] {
            assert!(validate_locator(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn bundle_heads_are_exactly_bound_to_signed_commit() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let good = format!("{commit} HEAD\n{commit} refs/heads/main\n");
        assert!(verify_bundle_heads(good.as_bytes(), commit).is_ok());
        let bad = format!("{commit} HEAD\n{commit} refs/heads/main\n{commit} refs/heads/extra\n");
        assert!(verify_bundle_heads(bad.as_bytes(), commit).is_err());
        let other = "f123456789abcdef0123456789abcdef01234567";
        let mismatch = format!("{commit} HEAD\n{other} refs/heads/main\n");
        assert!(verify_bundle_heads(mismatch.as_bytes(), commit).is_err());
    }

    #[test]
    fn utc_and_xxhash_primitives_match_stable_controls() {
        assert_eq!(format_utc(NOW), "2026-05-01T00:00:00Z");
        assert_eq!(parse_utc("2026-05-01T00:00:00Z"), Some(NOW));
        assert_eq!(xxh64(b"", 0), 0xef46_db37_51d8_e999);
        assert_eq!(xxh64(b"hello", 0), 0x26c7_827d_889f_6da3);
    }

    #[test]
    fn materialized_policy_uses_only_the_explicit_database_root_and_locator() {
        let root = tempfile::tempdir().expect("temporary policy root");
        fs::create_dir(root.path().join("core")).expect("core directory");
        fs::write(
            root.path().join("core/deny.toml"),
            "[advisories]\nignore = []\n\n[licenses]\nallow = []\n",
        )
        .expect("deny policy");
        let database = root.path().join("explicit-advisory-dbs");
        fs::create_dir(&database).expect("database root");
        let destination = root.path().join("materialized.toml");
        let locator = "ssh://no-route.invalid/mirror/rustsec-advisory-db.git";
        materialize_config(root.path(), &destination, &database, locator)
            .expect("materialize advisory policy");
        let rendered = fs::read_to_string(destination).expect("rendered policy");
        assert!(rendered.contains(&format!("db-path = {:?}", database)));
        assert!(rendered.contains(&format!("db-urls = [\"{locator}\"]")));
        assert!(!rendered.contains(".cargo/advisory-db"));
    }

    #[test]
    fn advisory_witness_projection_is_sorted_and_deduplicated() {
        let output = br#"{"vulnerabilities":{"list":[{"advisory":{"id":"RUSTSEC-2099-0002"},"package":{"name":"zeta","version":"2.0.0"}},{"advisory":{"id":"RUSTSEC-2099-0001"},"package":{"name":"alpha","version":"1.0.0"}},{"advisory":{"id":"RUSTSEC-2099-0001"},"package":{"name":"alpha","version":"1.0.0"}}]}}
"#;
        let findings = parse_advisories(output).expect("parse cargo-deny audit JSON");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].id, "RUSTSEC-2099-0001");
        assert_eq!(findings[0].packages.len(), 1);
        assert_eq!(findings[1].id, "RUSTSEC-2099-0002");
    }
}
