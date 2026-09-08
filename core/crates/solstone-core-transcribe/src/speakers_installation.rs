// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native speakers-analyze installation validation and generation borrowing.
//!
//! Root acquisition roles are [`SpeakersAnalyzeOwnerRole`]. Cortex talent
//! workers and whole-day `segment_repair` are not roles and never consult this
//! generation.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::{
    JsonWriteOptions, LeaseOptions, MalformedPolicy, acquire_file_lease, read_json, write_json,
};
use solstone_core_system::process::{
    InspectResult, InstanceVerdict, ProcessInstance, ProcessInstanceSource,
    SystemProcessInstanceSource,
};

use crate::args::{CliError, installation_error};
use crate::model_assets::resolve_model_asset_path;
use crate::resolve_model_asset;

const HELPER_BINARY_NAME: &str = "solstone-core-speakers-analyze";
const HELPER_BINARY_ENV: &str = "SOLSTONE_SPEAKERS_ANALYZE_BINARY";
const GENERATION_ENV_KEY: &str = "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID";
const GENERATION_FD_ENV_KEY: &str = "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_FD";
const GENERATION_TOKEN_ENV_KEY: &str = "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_TOKEN";
const INSTALL_GENERATION_SCHEMA: &str = "solstone.speakers_analyze.install_generation.v1";
const OWNER_SCHEMA: &str = "solstone.speakers_analyze.owner.v1";
const PROOF_KEY_SCHEMA: &str = "solstone.speakers_analyze.install_proof_key.v1";
const WESPEAKER_ASSET: &str = "wespeaker-resnet34-256.onnx";
const PYANNOTE_ASSET: &str = "pyannote-segmentation-3.0.onnx";
const OWNER_WRITE_OPTIONS: JsonWriteOptions = JsonWriteOptions {
    mode: Some(0o600),
    indent: Some(2),
    sort_keys: true,
};

/// Closed set of process-tree roots that may acquire a speakers-analyze generation.
///
/// Authorization never derives this from argv, executable name, or process title.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpeakersAnalyzeOwnerRole {
    Supervisor,
    Sense,
    Think,
    Transcribe,
    #[cfg(windows)]
    Convey,
    #[cfg(windows)]
    Maintenance,
}

impl SpeakersAnalyzeOwnerRole {
    /// Stable snake_case label stored in `owner.json`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supervisor => "supervisor",
            Self::Sense => "sense",
            Self::Think => "think",
            Self::Transcribe => "transcribe",
            #[cfg(windows)]
            Self::Convey => "convey",
            #[cfg(windows)]
            Self::Maintenance => "maintenance",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "supervisor" => Some(Self::Supervisor),
            "sense" => Some(Self::Sense),
            "think" => Some(Self::Think),
            "transcribe" => Some(Self::Transcribe),
            #[cfg(windows)]
            "convey" => Some(Self::Convey),
            #[cfg(windows)]
            "maintenance" => Some(Self::Maintenance),
            _ => None,
        }
    }
}

/// Observability-only view of the current generation owner.
///
/// Never consulted by [`enter_speakers_analyze_generation`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpeakersAnalyzeOwnerView {
    Available {
        role: SpeakersAnalyzeOwnerRole,
        pid: u32,
        started_at: String,
        install_generation_id: String,
    },
    Unavailable,
}

impl SpeakersAnalyzeOwnerView {
    /// Age of an available owner record relative to `now`.
    ///
    /// Missing, unparsable, or future `started_at` values yield `None`.
    pub fn age(&self, now: DateTime<Utc>) -> Option<chrono::TimeDelta> {
        let Self::Available { started_at, .. } = self else {
            return None;
        };
        let started = DateTime::parse_from_rfc3339(started_at)
            .ok()?
            .with_timezone(&Utc);
        let age = now.signed_duration_since(started);
        (age >= chrono::TimeDelta::zero()).then_some(age)
    }
}

/// A held or inherited speakers-analyze installation generation.
///
/// Unix roots retain the advisory lease and descendants retain their inherited
/// descriptor. Windows roots and borrowers retain the same exclusive file object
/// through reduced-rights capabilities; the last holder releases the generation.
#[derive(Debug)]
pub struct SpeakersAnalyzeGeneration {
    _lease: Option<solstone_core_journal_io::FileLease>,
    inherited_fd: Option<i32>,
    environment: BTreeMap<OsString, OsString>,
    #[cfg(windows)]
    read_grant: solstone_core_system::process::ReadFileGrant,
}

impl SpeakersAnalyzeGeneration {
    /// Environment passed only to native child commands that can borrow this generation.
    #[cfg(not(windows))]
    pub fn inheritance_environment(&self) -> BTreeMap<OsString, OsString> {
        self.environment.clone()
    }
    /// Metadata and typed capabilities for the next authenticated journal child.
    pub fn child_launch_context(&self) -> solstone_core_system::process::ChildLaunchContext {
        solstone_core_system::process::ChildLaunchContext {
            environment: self.environment.clone(),
            #[cfg(windows)]
            read_file_grants: vec![self.read_grant.clone()],
        }
    }
}

impl Drop for SpeakersAnalyzeGeneration {
    fn drop(&mut self) {
        if let Some(fd) = self.inherited_fd.take() {
            close_inherited_fd(fd);
        }
    }
}

/// Enter a validated installation generation for this journal process tree.
///
/// `role` is a static call-site declaration of the acquiring root. Borrowers
/// still pass the role of their own process; it is recorded only when this
/// process actually acquires.
#[cfg(not(windows))]
pub fn enter_speakers_analyze_generation(
    journal: &Path,
    role: SpeakersAnalyzeOwnerRole,
) -> Result<SpeakersAnalyzeGeneration, CliError> {
    enter_impl(
        journal,
        role,
        installation_proof()?,
        true,
        inherited_from_env(),
    )
}

/// Test seam: drive acquire/borrow with a caller-supplied proof and no asset digest check.
#[cfg(all(test, not(windows)))]
pub(crate) fn enter_with_proof(
    journal: &Path,
    role: SpeakersAnalyzeOwnerRole,
    proof: Value,
) -> Result<SpeakersAnalyzeGeneration, CliError> {
    enter_impl(journal, role, proof, false, inherited_from_env())
}

#[cfg(not(windows))]
fn enter_impl(
    journal: &Path,
    role: SpeakersAnalyzeOwnerRole,
    proof: Value,
    validate_assets: bool,
    inherited: Option<(String, String, String)>,
) -> Result<SpeakersAnalyzeGeneration, CliError> {
    let generation_path = generation_path(journal);
    let lease_path = generation_lock_path(journal);

    if role != SpeakersAnalyzeOwnerRole::Supervisor
        && let Some((id, fd, token)) = inherited.as_ref()
        && borrowed_generation_matches(&generation_path, &lease_path, &proof, id, fd, token)
    {
        return Ok(SpeakersAnalyzeGeneration {
            _lease: None,
            inherited_fd: None,
            environment: inheritance_map(id, fd, token),
        });
    }

    let lease = acquire_file_lease(
        &lease_path,
        LeaseOptions {
            attempts: 1,
            retry_max: Duration::ZERO,
            ..LeaseOptions::default()
        },
    )
    .map_err(|error| installation_error(format!("generation-lease: {error}")))?
    .ok_or_else(|| generation_contention_error(journal))?;

    // A lease holder always validates the pinned bytes before publishing a
    // record that descendants may borrow.
    if validate_assets {
        validate_model_assets()?;
    }
    let id = random_hex()?;
    let token = random_hex()?;
    fs::write(lease.path(), &token)
        .map_err(|error| installation_error(format!("generation-token: {error}")))?;
    let fd = duplicate_for_inheritance(&lease)?;
    let record = json!({
        "schema": INSTALL_GENERATION_SCHEMA,
        "id": id,
        "token": token,
        "proof": proof,
    });
    if let Err(error) = write_json(&generation_path, &record, OWNER_WRITE_OPTIONS) {
        close_inherited_fd(fd);
        return Err(installation_error(format!("generation-record: {error}")));
    }
    let _ = write_owner_record(journal, role, &id);
    Ok(SpeakersAnalyzeGeneration {
        _lease: Some(lease),
        inherited_fd: Some(fd),
        environment: inheritance_map(&id, &fd.to_string(), &token),
    })
}

/// Enter a Windows generation using only an authenticated launch capability
/// for borrowing. Root acquisition owns one exclusive-open file object.
#[cfg(windows)]
pub fn enter_speakers_analyze_generation(
    journal: &Path,
    role: SpeakersAnalyzeOwnerRole,
    admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> Result<SpeakersAnalyzeGeneration, CliError> {
    use solstone_core_system::process::{ReadFileGrant, ReadFileGrantKind};
    use std::io::Write;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    if env::var_os(GENERATION_FD_ENV_KEY).is_some() {
        return Err(installation_error(
            "obsolete Windows generation descriptor marker",
        ));
    }
    let proof = installation_proof()?;
    let metadata = windows_inherited_generation()?;
    if let Some(admitted) = admitted {
        if admitted.journal() != journal || role == SpeakersAnalyzeOwnerRole::Supervisor {
            return Err(installation_error(
                "generation launch provenance does not match this root",
            ));
        }
        let [grant] = admitted.read_file_grants() else {
            return Err(installation_error(
                "authenticated generation grant is missing or duplicated",
            ));
        };
        let (id, token) =
            metadata.ok_or_else(|| installation_error("generation metadata is missing"))?;
        if grant.kind() != ReadFileGrantKind::SpeakersAnalyzeGeneration {
            return Err(installation_error("unexpected generation grant kind"));
        }
        validate_windows_borrow(journal, &proof, &id, &token, grant)?;
        return Ok(SpeakersAnalyzeGeneration {
            _lease: None,
            inherited_fd: None,
            environment: windows_generation_environment(&id, &token),
            read_grant: grant.clone(),
        });
    }
    if metadata.is_some() {
        return Err(installation_error(
            "generation metadata has no authenticated launch capability",
        ));
    }
    let lease_path = generation_lock_path(journal);
    fs::create_dir_all(lease_path.parent().expect("generation lock has parent"))
        .map_err(|error| installation_error(format!("generation-directory: {error}")))?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&lease_path)
        .map_err(|error| {
            if error.raw_os_error() == Some(32) {
                generation_contention_error(journal)
            } else {
                installation_error(format!("generation-open: {error}"))
            }
        })?;
    let attributes = file
        .metadata()
        .map_err(|error| installation_error(error.to_string()))?;
    if !attributes.is_file() || attributes.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(installation_error(
            "generation lock is not an ordinary file",
        ));
    }
    // Package proof already verified the signed Windows helper/model bytes.
    // Write through this exclusive file object; never reopen its pathname.
    let id = random_hex()?;
    let token = random_hex()?;
    file.set_len(0)
        .and_then(|_| file.write_all(token.as_bytes()))
        .and_then(|_| file.sync_all())
        .map_err(|error| installation_error(format!("generation-token: {error}")))?;
    let grant =
        ReadFileGrant::try_clone_read_only(ReadFileGrantKind::SpeakersAnalyzeGeneration, &file)
            .map_err(|error| installation_error(format!("generation-capability: {error}")))?;
    write_json(
        generation_path(journal),
        &json!({
            "schema": INSTALL_GENERATION_SCHEMA, "id": id, "token": token, "proof": proof,
        }),
        OWNER_WRITE_OPTIONS,
    )
    .map_err(|error| installation_error(format!("generation-record: {error}")))?;
    // The reduced-rights duplicate retains the same exclusive file object after
    // this local writable handle closes; there is no explicit unlock on Drop.
    drop(file);
    Ok(SpeakersAnalyzeGeneration {
        _lease: None,
        inherited_fd: None,
        environment: windows_generation_environment(&id, &token),
        read_grant: grant,
    })
}

#[cfg(windows)]
fn windows_generation_environment(id: &str, token: &str) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([
        (GENERATION_ENV_KEY.into(), id.into()),
        (GENERATION_TOKEN_ENV_KEY.into(), token.into()),
    ])
}

#[cfg(windows)]
fn windows_inherited_generation() -> Result<Option<(String, String)>, CliError> {
    let id = env::var_os(GENERATION_ENV_KEY);
    let token = env::var_os(GENERATION_TOKEN_ENV_KEY);
    match (id, token) {
        (None, None) => Ok(None),
        (Some(id), Some(token)) => {
            let id = id
                .into_string()
                .map_err(|_| installation_error("non-Unicode generation ID"))?;
            let token = token
                .into_string()
                .map_err(|_| installation_error("non-Unicode generation token"))?;
            if [&id, &token].iter().any(|value| {
                value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) {
                return Err(installation_error("malformed generation metadata"));
            }
            Ok(Some((id, token)))
        }
        _ => Err(installation_error("partial generation metadata")),
    }
}

#[cfg(windows)]
fn validate_windows_borrow(
    journal: &Path,
    proof: &Value,
    id: &str,
    token: &str,
    grant: &solstone_core_system::process::ReadFileGrant,
) -> Result<(), CliError> {
    use solstone_core_journal_io::windows_file_identity;
    use std::os::windows::fs::{FileExt, MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let fail =
        || installation_error("generation capability does not match the current installation");
    let path = generation_lock_path(journal);
    // Metadata-only open is compatible with the exclusive data open; it cannot
    // read the token or borrow the generation by itself.
    let named = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(|_| fail())?;
    let metadata = named.metadata().map_err(|_| fail())?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || windows_file_identity(&named).map_err(|_| fail())?
            != windows_file_identity(grant.file()).map_err(|_| fail())?
    {
        return Err(fail());
    }
    // Observe exclusive data-open contention. A metadata handle alone cannot
    // produce a successful token read from the authenticated grant below.
    match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
    {
        Err(error) if error.raw_os_error() == Some(32) => {}
        _ => return Err(fail()),
    }
    if grant.file().metadata().map_err(|_| fail())?.len() != 32 {
        return Err(fail());
    }
    let mut bytes = [0u8; 32];
    let mut consumed = 0;
    while consumed < bytes.len() {
        // Every short-read retry supplies its own absolute offset. Duplicated
        // file handles share a cursor, and seek_read also updates that cursor.
        let count = grant
            .file()
            .seek_read(&mut bytes[consumed..], consumed as u64)
            .map_err(|_| fail())?;
        if count == 0 {
            return Err(fail());
        }
        consumed += count;
    }
    if bytes != token.as_bytes() {
        return Err(fail());
    }
    let record = read_json(
        generation_path(journal),
        Value::Null,
        MalformedPolicy::Raise,
    )
    .map_err(|_| fail())?;
    if record.get("schema").and_then(Value::as_str) != Some(INSTALL_GENERATION_SCHEMA)
        || record.get("id").and_then(Value::as_str) != Some(id)
        || record.get("token").and_then(Value::as_str) != Some(token)
        || record.get("proof") != Some(proof)
    {
        return Err(fail());
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn validate_speakers_analyze_runtime() -> Result<ValidatedInstallation, CliError> {
    let package = solstone_core_local::install::onnx_readiness::verified_windows_onnx_package()
        .map_err(installation_error)?;
    Ok(ValidatedInstallation {
        helper: package.speakers_worker,
        wespeaker_model: package.wespeaker_model,
        pyannote_model: package.pyannote_model,
    })
}

#[cfg(windows)]
fn installation_proof() -> Result<Value, CliError> {
    let paths = validate_speakers_analyze_runtime()?;
    Ok(json!({
        "schema": PROOF_KEY_SCHEMA, "platform": runtime_platform(),
        "helper": file_stamp(&paths.helper)?,
        "assets": [
            { "name": WESPEAKER_ASSET, "file": file_stamp(&paths.wespeaker_model)? },
            { "name": PYANNOTE_ASSET, "file": file_stamp(&paths.pyannote_model)? },
        ],
    }))
}

/// Read owner diagnostics without affecting acquire/borrow authorization.
pub fn read_speakers_analyze_owner(journal: &Path) -> SpeakersAnalyzeOwnerView {
    read_speakers_analyze_owner_inner(journal).unwrap_or(SpeakersAnalyzeOwnerView::Unavailable)
}

fn read_speakers_analyze_owner_inner(journal: &Path) -> Option<SpeakersAnalyzeOwnerView> {
    if !generation_lock_is_held(journal) {
        return None;
    }
    let record = read_json(generation_path(journal), Value::Null, MalformedPolicy::Skip).ok()?;
    let current_id = record.get("id").and_then(Value::as_str)?;
    if record.get("schema").and_then(Value::as_str) != Some(INSTALL_GENERATION_SCHEMA) {
        return None;
    }
    let current_token = fs::read_to_string(generation_lock_path(journal)).ok()?;
    if record.get("token").and_then(Value::as_str) != Some(current_token.as_str()) {
        return None;
    }
    let owner = read_json(owner_path(journal), Value::Null, MalformedPolicy::Skip).ok()?;
    if owner.get("schema").and_then(Value::as_str) != Some(OWNER_SCHEMA) {
        return None;
    }
    let role = owner
        .get("role")
        .and_then(Value::as_str)
        .and_then(SpeakersAnalyzeOwnerRole::parse)?;
    let pid = owner
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok().filter(|pid| *pid != 0))?;
    let process_instance =
        serde_json::from_value::<ProcessInstance>(owner.get("process_instance")?.clone()).ok()?;
    if process_instance.pid != pid
        || !matches!(
            SystemProcessInstanceSource.observe(&process_instance),
            InstanceVerdict::SameLive { .. }
        )
    {
        return None;
    }
    let started_at = owner.get("started_at").and_then(Value::as_str)?;
    if started_at != process_started_at(&process_instance)? {
        return None;
    }
    let install_generation_id = owner.get("install_generation_id").and_then(Value::as_str)?;
    if install_generation_id != current_id {
        return None;
    }
    Some(SpeakersAnalyzeOwnerView::Available {
        role,
        pid,
        started_at: started_at.to_owned(),
        install_generation_id: install_generation_id.to_owned(),
    })
}

/// Fully validate the helper and both pinned model assets for an invocation.
#[cfg(not(windows))]
pub(crate) fn validate_speakers_analyze_runtime() -> Result<ValidatedInstallation, CliError> {
    let proof = installation_proof()?;
    let wespeaker_model = resolve_model_asset(WESPEAKER_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    let pyannote_model = resolve_model_asset(PYANNOTE_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    let helper = PathBuf::from(
        proof
            .get("helper")
            .and_then(|helper| helper.get("path"))
            .and_then(Value::as_str)
            .expect("installation proof always contains helper path"),
    );
    Ok(ValidatedInstallation {
        helper,
        wespeaker_model,
        pyannote_model,
    })
}

/// Paths used for one validated helper invocation.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct ValidatedInstallation {
    pub(crate) helper: PathBuf,
    pub(crate) wespeaker_model: PathBuf,
    pub(crate) pyannote_model: PathBuf,
}

#[cfg(not(windows))]
fn installation_proof() -> Result<Value, CliError> {
    check_platform_coverage()?;
    let helper = helper_path()?;
    let wespeaker = resolve_model_asset_path(WESPEAKER_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    let pyannote = resolve_model_asset_path(PYANNOTE_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    Ok(json!({
        "schema": PROOF_KEY_SCHEMA,
        "platform": runtime_platform(),
        "helper": file_stamp(&helper)?,
        "assets": [
            { "name": WESPEAKER_ASSET, "file": file_stamp(&wespeaker)? },
            { "name": PYANNOTE_ASSET, "file": file_stamp(&pyannote)? },
        ],
    }))
}

fn check_platform_coverage() -> Result<(), CliError> {
    let (platform, architecture) = runtime_platform();
    let covered = matches!(
        (platform, architecture),
        ("linux", "x86_64" | "aarch64") | ("darwin", "arm64")
    );
    covered.then_some(()).ok_or_else(|| {
        installation_error(format!("platform-unsupported: {platform}/{architecture}"))
    })
}

fn runtime_platform() -> (&'static str, &'static str) {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => ("darwin", "arm64"),
        (platform, architecture) => (platform, architecture),
    }
}

fn helper_path() -> Result<PathBuf, CliError> {
    let executable =
        env::current_exe().map_err(|error| installation_error(format!("helper-path: {error}")))?;
    let directory = executable.parent().ok_or_else(|| {
        installation_error("helper-path: current executable has no parent directory")
    })?;
    let candidate = match env::var(HELPER_BINARY_ENV) {
        Ok(path) if !path.is_empty() => PathBuf::from(path),
        _ => directory.join(HELPER_BINARY_NAME),
    };
    if !is_executable(&candidate) {
        return Err(installation_error(format!(
            "helper-not-executable: {}",
            candidate.display()
        )));
    }
    Ok(candidate)
}

fn validate_model_assets() -> Result<(), CliError> {
    resolve_model_asset(WESPEAKER_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    resolve_model_asset(PYANNOTE_ASSET)
        .map_err(|error| installation_error(format!("asset-missing: {error}")))?;
    Ok(())
}

fn file_stamp(path: &Path) -> Result<Value, CliError> {
    let metadata = fs::metadata(path).map_err(|error| {
        installation_error(format!("proof-metadata: {}: {error}", path.display()))
    })?;
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().to_string());
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    };
    #[cfg(not(unix))]
    let mode = 0;
    Ok(json!({
        "path": path.to_string_lossy(),
        "size": metadata.len(),
        "modified_ns": modified_ns,
        "mode": mode,
    }))
}

fn borrowed_generation_matches(
    generation_path: &Path,
    lease_path: &Path,
    proof: &Value,
    id: &str,
    fd: &str,
    token: &str,
) -> bool {
    if !generation_lock_path_is_held(lease_path)
        || !inherited_generation_ofd_holds_lock(fd)
        || fs::read_to_string(lease_path).ok().as_deref() != Some(token)
    {
        return false;
    }
    let Ok(record) = read_json(generation_path, Value::Null, MalformedPolicy::Skip) else {
        return false;
    };
    record.get("schema").and_then(Value::as_str) == Some(INSTALL_GENERATION_SCHEMA)
        && record.get("id").and_then(Value::as_str) == Some(id)
        && record.get("token").and_then(Value::as_str) == Some(token)
        && record.get("proof") == Some(proof)
}

#[cfg(unix)]
fn duplicate_for_inheritance(lease: &solstone_core_journal_io::FileLease) -> Result<i32, CliError> {
    lease
        .duplicate_for_inheritance()
        .map_err(|error| installation_error(format!("generation-fd: {error}")))
}

#[cfg(not(unix))]
fn duplicate_for_inheritance(_: &solstone_core_journal_io::FileLease) -> Result<i32, CliError> {
    Err(installation_error("generation-fd: unsupported platform"))
}

fn close_inherited_fd(fd: i32) {
    #[cfg(unix)]
    {
        let _ = nix::unistd::close(fd);
    }
    #[cfg(not(unix))]
    {
        let _ = fd;
    }
}

#[cfg(unix)]
fn inherited_generation_ofd_holds_lock(fd: &str) -> bool {
    let Ok(fd) = fd.parse::<i32>() else {
        return false;
    };
    if !Path::new("/dev/fd").join(fd.to_string()).exists() {
        return false;
    }
    solstone_core_journal_io::probe_exclusive_flock_no_release(fd).unwrap_or(false)
}

#[cfg(not(unix))]
fn inherited_generation_ofd_holds_lock(_: &str) -> bool {
    false
}

fn random_hex() -> Result<String, CliError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| installation_error(format!("generation-random: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn inherited_from_env() -> Option<(String, String, String)> {
    match (
        env::var(GENERATION_ENV_KEY),
        env::var(GENERATION_FD_ENV_KEY),
        env::var(GENERATION_TOKEN_ENV_KEY),
    ) {
        (Ok(id), Ok(fd), Ok(token)) => Some((id, fd, token)),
        _ => None,
    }
}

fn inheritance_map(id: &str, fd: &str, token: &str) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([
        (OsString::from(GENERATION_ENV_KEY), OsString::from(id)),
        (OsString::from(GENERATION_FD_ENV_KEY), OsString::from(fd)),
        (
            OsString::from(GENERATION_TOKEN_ENV_KEY),
            OsString::from(token),
        ),
    ])
}

fn write_owner_record(journal: &Path, role: SpeakersAnalyzeOwnerRole, id: &str) -> Result<(), ()> {
    let pid = std::process::id();
    let InspectResult::Present {
        instance: process_instance,
        ..
    } = SystemProcessInstanceSource.inspect(pid)
    else {
        return Err(());
    };
    let started_at = process_started_at(&process_instance).ok_or(())?;
    let record = json!({
        "schema": OWNER_SCHEMA,
        "role": role.as_str(),
        "pid": pid,
        "process_instance": process_instance,
        "started_at": started_at,
        "install_generation_id": id,
    });
    write_json(owner_path(journal), &record, OWNER_WRITE_OPTIONS).map_err(|_| ())
}

fn process_started_at(instance: &ProcessInstance) -> Option<String> {
    let epoch_seconds = instance.birth.epoch_seconds()?;
    if !epoch_seconds.is_finite() || epoch_seconds < 0.0 {
        return None;
    }
    let mut seconds = epoch_seconds.floor() as i64;
    let mut nanoseconds = ((epoch_seconds - seconds as f64) * 1_000_000_000.0).round() as u32;
    if nanoseconds == 1_000_000_000 {
        seconds = seconds.checked_add(1)?;
        nanoseconds = 0;
    }
    DateTime::<Utc>::from_timestamp(seconds, nanoseconds)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

fn generation_contention_error(journal: &Path) -> CliError {
    let message = match read_speakers_analyze_owner(journal) {
        SpeakersAnalyzeOwnerView::Available {
            role,
            pid,
            started_at,
            install_generation_id,
        } => {
            let age = SpeakersAnalyzeOwnerView::Available {
                role,
                pid,
                started_at: started_at.clone(),
                install_generation_id: install_generation_id.clone(),
            }
            .age(Utc::now())
            .map_or_else(
                || "unavailable".to_owned(),
                |age| age.num_seconds().to_string(),
            );
            format!(
                "generation-lease-contended: owner_role={} owner_pid={pid} owner_started_at={started_at} install_generation_id={install_generation_id} owner_age_seconds={age}; use the supervised path or stop that process cleanly",
                role.as_str()
            )
        }
        SpeakersAnalyzeOwnerView::Unavailable => "generation-lease-contended: owner_details=unavailable; use the supervised path or stop the current process cleanly".to_owned(),
    };
    CliError::SpeakersInstallation { message }
}

fn generation_lock_is_held(journal: &Path) -> bool {
    generation_lock_path_is_held(&generation_lock_path(journal))
}

fn generation_lock_path_is_held(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;

        let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
            return false;
        };
        matches!(
            solstone_core_journal_io::lease::probe_file_lease(&file),
            solstone_core_journal_io::lease::LeaseProbe::Active
        )
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn generation_path(journal: &Path) -> PathBuf {
    journal.join("health/speakers-analyze/install-generation.json")
}

fn generation_lock_path(journal: &Path) -> PathBuf {
    journal.join("health/speakers-analyze/install-generation.lock")
}

fn owner_path(journal: &Path) -> PathBuf {
    journal.join("health/speakers-analyze/owner.json")
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration as StdDuration;

    use serde_json::json;
    use tempfile::TempDir;

    fn proof() -> Value {
        json!({
            "schema": PROOF_KEY_SCHEMA,
            "marker": "synthetic-proof",
        })
    }

    fn inherited_from(generation: &SpeakersAnalyzeGeneration) -> (String, String, String) {
        let environment = generation.inheritance_environment();
        (
            environment
                .get(OsStr::new(GENERATION_ENV_KEY))
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            environment
                .get(OsStr::new(GENERATION_FD_ENV_KEY))
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            environment
                .get(OsStr::new(GENERATION_TOKEN_ENV_KEY))
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn owner_json(journal: &Path) -> Value {
        serde_json::from_slice(&fs::read(owner_path(journal)).unwrap()).unwrap()
    }

    fn rewrite_owner(journal: &Path, mutate: impl FnOnce(&mut Value)) {
        let mut record = owner_json(journal);
        mutate(&mut record);
        write_json(owner_path(journal), &record, OWNER_WRITE_OPTIONS).unwrap();
    }

    fn contended_detail(error: &CliError) -> String {
        error.message().unwrap_or_default().to_owned()
    }

    #[test]
    fn runtime_platform_matches_the_running_host() {
        let (platform, architecture) = runtime_platform();
        if std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64" {
            assert_eq!((platform, architecture), ("darwin", "arm64"));
        } else {
            assert_eq!(
                (platform, architecture),
                (std::env::consts::OS, std::env::consts::ARCH)
            );
        }
    }

    #[test]
    fn current_target_has_covered_speakers_analyze_runtime() {
        check_platform_coverage().unwrap();
    }

    #[test]
    fn runtime_platform_is_a_known_native_target_shape() {
        let (platform, architecture) = runtime_platform();
        assert!(matches!(
            (platform, architecture),
            ("linux", "x86_64" | "aarch64") | ("darwin", "arm64")
        ));
    }

    #[test]
    fn owner_role_as_str_is_closed_snake_case() {
        assert_eq!(SpeakersAnalyzeOwnerRole::Supervisor.as_str(), "supervisor");
        assert_eq!(SpeakersAnalyzeOwnerRole::Sense.as_str(), "sense");
        assert_eq!(SpeakersAnalyzeOwnerRole::Think.as_str(), "think");
        assert_eq!(SpeakersAnalyzeOwnerRole::Transcribe.as_str(), "transcribe");
        assert_eq!(SpeakersAnalyzeOwnerRole::parse("cortex"), None);
        assert_eq!(SpeakersAnalyzeOwnerRole::parse("segment_repair"), None);
    }

    #[cfg(unix)]
    #[test]
    fn owner_publish_writes_closed_owner_json_mode_0600() {
        let journal = TempDir::new().unwrap();
        let generation =
            enter_with_proof(journal.path(), SpeakersAnalyzeOwnerRole::Think, proof()).unwrap();
        let record = owner_json(journal.path());
        assert_eq!(record["schema"], OWNER_SCHEMA);
        assert_eq!(record["role"], "think");
        assert_eq!(record["pid"], std::process::id());
        assert!(DateTime::parse_from_rfc3339(record["started_at"].as_str().unwrap()).is_ok());
        assert_eq!(record["process_instance"]["pid"], std::process::id());
        let (id, _, _) = inherited_from(&generation);
        assert_eq!(record["install_generation_id"], id);
        assert_eq!(record.as_object().map(|object| object.len()), Some(6));
        let mode = fs::metadata(owner_path(journal.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        match read_speakers_analyze_owner(journal.path()) {
            SpeakersAnalyzeOwnerView::Available {
                role,
                pid,
                install_generation_id,
                ..
            } => {
                assert_eq!(role, SpeakersAnalyzeOwnerRole::Think);
                assert_eq!(pid, std::process::id());
                assert_eq!(install_generation_id, id);
            }
            SpeakersAnalyzeOwnerView::Unavailable => panic!("owner should be available"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn matching_inherited_ofd_borrows_without_taking_the_lease() {
        let journal = TempDir::new().unwrap();
        let owner = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Supervisor,
            proof(),
        )
        .unwrap();
        let inherited = inherited_from(&owner);
        let borrowed = enter_impl(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            proof(),
            false,
            Some(inherited.clone()),
        )
        .unwrap();
        assert!(borrowed._lease.is_none());
        assert!(borrowed.inherited_fd.is_none());
        assert_eq!(
            borrowed.inheritance_environment(),
            owner.inheritance_environment()
        );
        drop(borrowed);
        assert!(
            acquire_file_lease(
                generation_lock_path(journal.path()),
                LeaseOptions {
                    attempts: 1,
                    retry_max: Duration::ZERO,
                    ..LeaseOptions::default()
                },
            )
            .unwrap()
            .is_none(),
            "borrow must not release the owner's generation lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_never_borrows_an_inherited_generation() {
        let journal = TempDir::new().unwrap();
        let owner = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            proof(),
        )
        .unwrap();
        let error = enter_impl(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Supervisor,
            proof(),
            false,
            Some(inherited_from(&owner)),
        )
        .unwrap_err();
        assert_eq!(error.exit_code(), 78);
        assert!(contended_detail(&error).contains("generation-lease-contended"));
        assert_eq!(owner_json(journal.path())["role"], "transcribe");
    }

    #[cfg(unix)]
    #[test]
    fn independent_open_fd_does_not_borrow_and_contends() {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        let journal = TempDir::new().unwrap();
        let owner =
            enter_with_proof(journal.path(), SpeakersAnalyzeOwnerRole::Sense, proof()).unwrap();
        let (id, _, token) = inherited_from(&owner);
        let independent = OpenOptions::new()
            .read(true)
            .write(true)
            .open(generation_lock_path(journal.path()))
            .unwrap();
        let error = enter_impl(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            proof(),
            false,
            Some((id, independent.as_raw_fd().to_string(), token)),
        )
        .unwrap_err();
        let message = contended_detail(&error);
        assert!(message.contains("generation-lease-contended"), "{message}");
        assert_eq!(error.exit_code(), 78);
    }

    #[cfg(unix)]
    #[test]
    fn stale_unlocked_reopen_cannot_authenticate_a_borrow() {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        let journal = TempDir::new().unwrap();
        let owner =
            enter_with_proof(journal.path(), SpeakersAnalyzeOwnerRole::Sense, proof()).unwrap();
        let (id, _, token) = inherited_from(&owner);
        drop(owner);
        let independent = OpenOptions::new()
            .read(true)
            .write(true)
            .open(generation_lock_path(journal.path()))
            .unwrap();
        let replacement = enter_impl(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            proof(),
            false,
            Some((id, independent.as_raw_fd().to_string(), token)),
        )
        .unwrap();
        assert!(
            replacement._lease.is_some(),
            "a reopened descriptor on an unlocked stale file must acquire and republish as a new root, not borrow"
        );
        assert_eq!(owner_json(journal.path())["role"], "transcribe");
    }

    #[cfg(unix)]
    #[test]
    fn corrupt_owner_json_does_not_reject_a_valid_borrow() {
        let journal = TempDir::new().unwrap();
        let owner = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Supervisor,
            proof(),
        )
        .unwrap();
        fs::write(owner_path(journal.path()), b"{not-json").unwrap();
        let borrowed = enter_impl(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            proof(),
            false,
            Some(inherited_from(&owner)),
        );
        assert!(borrowed.is_ok(), "diagnostics must not gate borrow");
        assert_eq!(
            read_speakers_analyze_owner(journal.path()),
            SpeakersAnalyzeOwnerView::Unavailable
        );
    }

    #[cfg(unix)]
    #[test]
    fn well_formed_owner_json_does_not_admit_an_invalid_borrow() {
        let journal = TempDir::new().unwrap();
        let owner = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Supervisor,
            proof(),
        )
        .unwrap();
        match read_speakers_analyze_owner(journal.path()) {
            SpeakersAnalyzeOwnerView::Available { .. } => {}
            SpeakersAnalyzeOwnerView::Unavailable => panic!("owner metadata should be available"),
        }
        let (id, fd, _) = inherited_from(&owner);
        let error = enter_impl(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            proof(),
            false,
            Some((id, fd, "not-the-token".to_owned())),
        )
        .unwrap_err();
        assert!(contended_detail(&error).contains("generation-lease-contended"));
    }

    #[cfg(unix)]
    #[test]
    fn contention_diagnostics_are_unavailable_for_stale_or_dead_owner_records() {
        let journal = TempDir::new().unwrap();
        let owner = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Supervisor,
            proof(),
        )
        .unwrap();
        let (id, _, _) = inherited_from(&owner);
        let message_before = contended_detail(
            &enter_with_proof(
                journal.path(),
                SpeakersAnalyzeOwnerRole::Transcribe,
                proof(),
            )
            .unwrap_err(),
        );
        assert!(message_before.contains("generation-lease-contended"));
        assert!(message_before.contains("owner_role=supervisor"));
        assert!(message_before.contains(&format!("owner_pid={}", std::process::id())));
        assert!(message_before.contains("owner_started_at="));
        assert!(message_before.contains(&format!("install_generation_id={id}")));
        assert!(message_before.contains("owner_age_seconds="));
        assert!(message_before.contains("use the supervised path or stop that process cleanly"));

        rewrite_owner(journal.path(), |record| {
            record["install_generation_id"] = json!("stale-generation-id");
        });
        assert_eq!(
            read_speakers_analyze_owner(journal.path()),
            SpeakersAnalyzeOwnerView::Unavailable
        );
        let message_stale = contended_detail(
            &enter_with_proof(
                journal.path(),
                SpeakersAnalyzeOwnerRole::Transcribe,
                proof(),
            )
            .unwrap_err(),
        );
        assert!(message_stale.contains("generation-lease-contended"));
        assert!(message_stale.contains("owner_details=unavailable"));
        assert!(!message_stale.contains(&id));

        rewrite_owner(journal.path(), |record| {
            record["install_generation_id"] = json!(id);
            record["pid"] = json!(i32::MAX as u32);
        });
        assert_eq!(
            read_speakers_analyze_owner(journal.path()),
            SpeakersAnalyzeOwnerView::Unavailable
        );

        fs::write(owner_path(journal.path()), b"").unwrap();
        assert_eq!(
            read_speakers_analyze_owner(journal.path()),
            SpeakersAnalyzeOwnerView::Unavailable
        );

        drop(owner);
        assert_eq!(
            read_speakers_analyze_owner(journal.path()),
            SpeakersAnalyzeOwnerView::Unavailable
        );
    }

    #[cfg(unix)]
    #[test]
    fn owner_diagnostics_require_the_generation_records_current_token() {
        let journal = TempDir::new().unwrap();
        let _generation = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Supervisor,
            proof(),
        )
        .unwrap();
        fs::write(generation_lock_path(journal.path()), "replacement-token").unwrap();
        assert_eq!(
            read_speakers_analyze_owner(journal.path()),
            SpeakersAnalyzeOwnerView::Unavailable
        );
        let message = contended_detail(
            &enter_with_proof(
                journal.path(),
                SpeakersAnalyzeOwnerRole::Transcribe,
                proof(),
            )
            .unwrap_err(),
        );
        assert!(message.contains("generation-lease-contended"));
        assert!(message.contains("owner_details=unavailable"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pid_reuse_shaped_owner_identity_is_unavailable() {
        let journal = TempDir::new().unwrap();
        let _generation = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Supervisor,
            proof(),
        )
        .unwrap();
        rewrite_owner(journal.path(), |record| {
            let start_ticks = record["process_instance"]["birth"]["start_ticks"]
                .as_u64()
                .expect("Linux process birth has start_ticks");
            record["process_instance"]["birth"]["start_ticks"] = json!(start_ticks + 1);
        });
        assert_eq!(
            read_speakers_analyze_owner(journal.path()),
            SpeakersAnalyzeOwnerView::Unavailable
        );
    }

    #[cfg(unix)]
    #[test]
    fn owner_age_advances_and_rejects_invalid_started_at() {
        let journal = TempDir::new().unwrap();
        let _generation = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            proof(),
        )
        .unwrap();
        let view = read_speakers_analyze_owner(journal.path());
        let earlier = Utc::now();
        std::thread::sleep(StdDuration::from_millis(5));
        let later = Utc::now();
        let first = view.age(earlier).expect("age at first observation");
        let second = view.age(later).expect("age at second observation");
        assert!(second > first, "{second:?} should exceed {first:?}");

        rewrite_owner(journal.path(), |record| {
            record["started_at"] = json!("not-a-timestamp");
        });
        assert_eq!(
            read_speakers_analyze_owner(journal.path()).age(Utc::now()),
            None
        );

        let journal = TempDir::new().unwrap();
        let _generation = enter_with_proof(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            proof(),
        )
        .unwrap();
        rewrite_owner(journal.path(), |record| {
            record["started_at"] =
                json!((Utc::now() + chrono::TimeDelta::seconds(60)).to_rfc3339());
        });
        let view = read_speakers_analyze_owner(journal.path());
        assert_eq!(view, SpeakersAnalyzeOwnerView::Unavailable);
        assert_eq!(view.age(Utc::now()), None);
        assert_eq!(SpeakersAnalyzeOwnerView::Unavailable.age(Utc::now()), None);
    }

    #[cfg(unix)]
    #[test]
    fn owner_json_and_contention_text_are_closed_against_sentinels() {
        const SENTINEL: &str = "SENTINEL_ARGV_EXE_LEAK_PATH";
        let journal = TempDir::new().unwrap();
        let poisoned_proof = json!({
            "schema": PROOF_KEY_SCHEMA,
            "helper": { "path": format!("/tmp/{SENTINEL}") },
            "assets": [SENTINEL],
        });
        let owner = enter_impl(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Supervisor,
            poisoned_proof.clone(),
            false,
            Some((
                SENTINEL.to_owned(),
                SENTINEL.to_owned(),
                SENTINEL.to_owned(),
            )),
        )
        .unwrap();
        let owner_bytes = fs::read_to_string(owner_path(journal.path())).unwrap();
        assert!(
            !owner_bytes.contains(SENTINEL),
            "owner.json must not serialize proof, env, or argv-shaped inputs: {owner_bytes}"
        );
        let error = enter_impl(
            journal.path(),
            SpeakersAnalyzeOwnerRole::Transcribe,
            poisoned_proof,
            false,
            Some((
                SENTINEL.to_owned(),
                SENTINEL.to_owned(),
                SENTINEL.to_owned(),
            )),
        )
        .unwrap_err();
        let message = contended_detail(&error);
        assert!(message.contains("generation-lease-contended"));
        assert!(
            !message.contains(SENTINEL),
            "contention text must stay closed: {message}"
        );
        drop(owner);
    }

    #[cfg(unix)]
    #[test]
    fn failed_generation_record_publish_does_not_write_owner_json() {
        let journal = TempDir::new().unwrap();
        let speakers = journal.path().join("health/speakers-analyze");
        fs::create_dir_all(&speakers).unwrap();
        fs::create_dir(speakers.join("install-generation.json")).unwrap();
        let error =
            enter_with_proof(journal.path(), SpeakersAnalyzeOwnerRole::Sense, proof()).unwrap_err();
        assert!(
            contended_detail(&error).contains("generation-record"),
            "{}",
            contended_detail(&error)
        );
        assert!(!owner_path(journal.path()).exists());
        fs::remove_dir(generation_path(journal.path())).unwrap();
        let reacquired = enter_with_proof(journal.path(), SpeakersAnalyzeOwnerRole::Sense, proof());
        assert!(
            reacquired.is_ok(),
            "failed publication must not leak the duplicated lease descriptor: {reacquired:?}"
        );
    }
}

#[cfg(all(test, windows, feature = "test-hooks"))]
#[path = "speakers_generation_windows_tests.rs"]
mod windows_generation_tests;
