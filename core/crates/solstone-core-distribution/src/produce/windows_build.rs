// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Live Windows Cargo production. No caller-supplied build log is an admission API.

use solstone_core_installed_payload::WindowsArchitecture;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::digest::sha256_hex;
use crate::inventory::{Entry, Inventory};
use crate::provenance::Provenance;
use crate::select::ArtifactId;

const LOG_LIMIT: u64 = 128 * 1024 * 1024;
const PE_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Debug, serde::Serialize)]
pub struct WindowsCargoEvidence {
    pub source: Provenance,
    pub target: String,
    pub argv: Vec<String>,
    pub actual_exit_code: i32,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub ffmpeg_run_id: String,
    pub ffmpeg_source_sha256: String,
    pub files: Vec<WindowsCargoOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_ocr_helper: Option<ScreenOcrCargoEvidence>,
}

#[derive(Debug, serde::Serialize)]
pub struct ScreenOcrCargoEvidence {
    pub target: String,
    pub manifest_sha256: String,
    pub lock_sha256: String,
    pub recipe_sha256: String,
    pub oar_revision: String,
    pub argv: Vec<String>,
    pub actual_exit_code: i32,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct WindowsCargoOutput {
    pub package: String,
    pub bin: String,
    pub sha256: String,
    pub bytes: u64,
}

/// Constructed only after this module runs Cargo and checks its actual output.
/// Original logs and target directory remain on disk on every failure; this
/// type makes no claim that unrelated or unknown build descendants are gone.
pub struct BuiltWindowsProduct {
    evidence: WindowsCargoEvidence,
    evidence_bytes: Vec<u8>,
    artifacts: BTreeMap<ArtifactId, PathBuf>,
    bytes: BTreeMap<ArtifactId, Vec<u8>>,
}

impl BuiltWindowsProduct {
    pub fn evidence(&self) -> &WindowsCargoEvidence {
        &self.evidence
    }
    pub fn evidence_bytes(&self) -> &[u8] {
        &self.evidence_bytes
    }
    pub fn artifacts(&self) -> &BTreeMap<ArtifactId, PathBuf> {
        &self.artifacts
    }
    pub fn bytes(&self) -> &BTreeMap<ArtifactId, Vec<u8>> {
        &self.bytes
    }
}

/// Build in a clean checkout whose core/target does not yet exist. The operator
/// supplies its existing fenced, network-denied native build environment.
/// Build tools stay Unowned; this code creates no Job or replacement manager.
/// The outer reviewed driver owns the deadline and host cleanup disposition.
pub fn build_windows_product(
    checkout: &Path,
    inventory: &Inventory,
    log_directory: &Path,
    ffmpeg_archive: &Path,
) -> Result<BuiltWindowsProduct, String> {
    build_windows_product_for_architecture(
        checkout,
        inventory,
        log_directory,
        ffmpeg_archive,
        WindowsArchitecture::X86_64,
    )
}

pub fn build_windows_product_for_architecture(
    checkout: &Path,
    inventory: &Inventory,
    log_directory: &Path,
    ffmpeg_archive: &Path,
    architecture: WindowsArchitecture,
) -> Result<BuiltWindowsProduct, String> {
    let triple = architecture.rust_triple();
    if !cfg!(windows) {
        return Err("Windows product compilation requires its native MSVC host".into());
    }
    let source = capture_source(checkout)?;
    let target_dir = checkout.join("core").join("target");
    fs::create_dir(&target_dir).map_err(|e| format!("fresh core/target required: {e}"))?;
    let target_root = solstone_core_journal_io::JournalRoot::open(&target_dir)
        .map_err(|e| format!("retain fresh target root: {e}"))?;
    let epoch = super::git_stdout(checkout, &["show", "-s", "--format=%ct", "HEAD"])
        .map_err(|e| e.to_string())?;
    fs::create_dir(log_directory)
        .map_err(|e| format!("fresh Cargo log directory required: {e}"))?;
    // The shared selector returns a verbatim canonical path on Windows. Keep
    // the supplied ordinary drive spelling for the bound reader and child.
    super::select_ffmpeg_input(checkout, Some(ffmpeg_archive.as_os_str()))
        .map_err(|e| e.to_string())?;
    let ffmpeg_source_sha256 = sha256_hex(&read_bounded(ffmpeg_archive, 256 * 1024 * 1024)?);
    let ffmpeg_run_id = super::ffmpeg_build_run_id();
    let argv = cargo_argv(inventory, architecture)?;
    let stdout_path = log_directory.join("cargo.stdout");
    let stderr_path = log_directory.join("cargo.stderr");
    let mut command = Command::new("cargo");
    command
        .current_dir(checkout)
        .args(&argv)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("SOURCE_DATE_EPOCH", &epoch)
        .env("ZERO_AR_DATE", "1")
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_ENCODED_RUSTFLAGS", "-Ctarget-feature=+crt-static")
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("FFMPEG_DIR")
        .env("FFMPEG_MARCH", "")
        .env("FFMPEG_MTUNE", "")
        .env("SOLSTONE_FFMPEG_SOURCE_ARCHIVE", ffmpeg_archive)
        .env(super::OFFLINE, "1")
        .env(
            solstone_core_ffmpeg_build_support::BUILD_RUN_ID_ENV,
            &ffmpeg_run_id,
        )
        .stdin(Stdio::null());
    let status = capture_build_command(
        &mut command,
        &stdout_path,
        &stderr_path,
        &log_directory.join("cargo.exit.json"),
    )?;
    if !status.success() {
        return Err(format!(
            "Cargo failed with actual exit {:?}; original logs retained at {}",
            status.code(),
            log_directory.display()
        ));
    }
    target_root
        .revalidate_canonical_binding()
        .map_err(|e| format!("fresh Cargo target binding changed: {e}"))?;
    if capture_source(checkout)? != source {
        return Err("product source or Cargo.lock changed during compilation".into());
    }
    let stdout = read_bounded(&stdout_path, LOG_LIMIT)?;
    let stderr = read_bounded(&stderr_path, LOG_LIMIT)?;
    let text = std::str::from_utf8(&stdout).map_err(|e| format!("Cargo JSON is not UTF-8: {e}"))?;
    require_finished_cargo(text)?;
    let mut artifacts = super::bind_cargo_json(text, triple).map_err(|e| e.to_string())?;
    let screen_ocr = if screen_ocr_helper_selected(inventory, architecture) {
        let helper =
            build_screen_ocr_helper(checkout, &target_dir, log_directory, &epoch, architecture)?;
        artifacts.insert(
            ArtifactId {
                package: crate::screen_ocr::HELPER_PACKAGE.into(),
                bin: crate::screen_ocr::HELPER_BIN.into(),
                triple: triple.into(),
            },
            helper.path.clone(),
        );
        if capture_source(checkout)? != source {
            return Err(
                "product source or Cargo.lock changed during independent helper compilation".into(),
            );
        }
        Some(helper)
    } else {
        None
    };
    crate::select::refuse_wrong_triple(inventory, architecture.target_id(), &artifacts)
        .map_err(|e| e.to_string())?;
    crate::select::refuse_extra(inventory, architecture.target_id(), &artifacts)
        .map_err(|e| e.to_string())?;
    let selection =
        crate::select::select_artifacts(inventory, architecture.target_id(), &artifacts)
            .map_err(|e| e.to_string())?;
    let release_root = retain_release_root(&target_root, architecture)?;
    let expected_parent = release_root
        .canonical_path()
        .canonicalize()
        .map_err(|e| format!("resolve current release output directory: {e}"))?;
    let mut bytes = BTreeMap::new();
    let mut files = Vec::new();
    for bin in &selection.bins {
        let path = bin
            .path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", bin.path.display()))?;
        let expected_bin_parent = if bin.package == crate::screen_ocr::HELPER_PACKAGE {
            target_dir
                .join("screen-ocr-helper")
                .join(triple)
                .join("release")
                .canonicalize()
                .map_err(|e| format!("resolve independent screen OCR output directory: {e}"))?
        } else {
            expected_parent.clone()
        };
        if path.parent() != Some(expected_bin_parent.as_path())
            || path.file_name().and_then(|s| s.to_str())
                != Some(format!("{}.exe", bin.bin).as_str())
        {
            return Err(format!(
                "Cargo artifact is outside its fresh target output: {}",
                path.display()
            ));
        }
        let data = read_bounded(&path, PE_LIMIT)?;
        if bin.package == crate::screen_ocr::HELPER_PACKAGE
            && screen_ocr
                .as_ref()
                .map(|helper| &helper.evidence.artifact_sha256)
                != Some(&sha256_hex(&data))
        {
            return Err("independent screen OCR helper changed after its build receipt".into());
        }
        let info = crate::pe_dependencies::inspect_dependencies(&data, architecture)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if info.is_dll {
            return Err(format!("Cargo command is a DLL: {}", bin.bin));
        }
        files.push(WindowsCargoOutput {
            package: bin.package.clone(),
            bin: bin.bin.clone(),
            sha256: sha256_hex(&data),
            bytes: data.len() as u64,
        });
        bytes.insert(
            ArtifactId {
                package: bin.package.clone(),
                bin: bin.bin.clone(),
                triple: bin.triple.clone(),
            },
            data,
        );
    }
    let ffmpeg_out = super::require_single_ffmpeg_out_dir(
        super::bind_ffmpeg_build_script_out_dirs(text),
        triple,
    )
    .map_err(|e| e.to_string())?;
    let canonical_ffmpeg = ffmpeg_out
        .canonicalize()
        .map_err(|e| format!("FFmpeg output directory: {e}"))?;
    if !canonical_ffmpeg.starts_with(expected_parent.join("build")) {
        return Err("FFmpeg build evidence is outside this fresh Cargo target".into());
    }
    super::validate_ffmpeg_evidence(
        &canonical_ffmpeg.join(super::EVIDENCE_DIR),
        &ffmpeg_run_id,
        triple,
        "release",
    )
    .map_err(|e| e.to_string())?;
    let record = solstone_core_ffmpeg_build_support::read_current_run_record(
        &canonical_ffmpeg.join(super::EVIDENCE_DIR),
    )
    .map_err(|e| e.to_string())?;
    if record.source_sha256 != ffmpeg_source_sha256 {
        return Err(
            "fresh FFmpeg configuration is not bound to the admitted source archive".into(),
        );
    }
    release_root
        .revalidate_canonical_binding()
        .map_err(|e| format!("release output binding changed: {e}"))?;
    let evidence = WindowsCargoEvidence {
        source,
        target: triple.into(),
        argv,
        actual_exit_code: status
            .code()
            .ok_or("Cargo success has no native exit code")?,
        stdout_sha256: sha256_hex(&stdout),
        stderr_sha256: sha256_hex(&stderr),
        ffmpeg_run_id,
        ffmpeg_source_sha256,
        files,
        screen_ocr_helper: screen_ocr.map(|helper| helper.evidence),
    };
    let evidence_bytes = serde_json::to_vec_pretty(&evidence).map_err(|e| e.to_string())?;
    create_log(&log_directory.join("cargo-evidence.json"))?
        .write_all(&evidence_bytes)
        .map_err(|e| e.to_string())?;
    Ok(BuiltWindowsProduct {
        evidence,
        evidence_bytes,
        artifacts,
        bytes,
    })
}

/// Whether the inventory ships the independent screen OCR helper on Windows.
/// The helper has its own Cargo lock, so it is compiled only when selected.
fn screen_ocr_helper_selected(inventory: &Inventory, architecture: WindowsArchitecture) -> bool {
    inventory.entry.iter().any(|entry| {
        matches!(
            entry,
            Entry::Bin { package, targets, .. }
                if package == crate::screen_ocr::HELPER_PACKAGE
                    && targets
                        .iter()
                        .any(|t| t == architecture.target_id())
        )
    })
}

fn build_screen_ocr_helper(
    checkout: &Path,
    target_dir: &Path,
    log_directory: &Path,
    epoch: &str,
    architecture: WindowsArchitecture,
) -> Result<BuiltScreenOcrHelper, String> {
    let triple = architecture.rust_triple();
    let [manifest_path, lock_path, recipe_path] = screen_ocr_input_paths(checkout);
    let manifest = read_bounded(&manifest_path, 4 * 1024 * 1024)?;
    let lock = read_bounded(&lock_path, 64 * 1024 * 1024)?;
    let recipe_bytes = read_bounded(&recipe_path, 1024 * 1024)?;
    crate::screen_ocr::verify_oar_revision(&recipe_bytes, &lock)?;
    let recipe = crate::screen_ocr::Recipe::load(&recipe_bytes)?;
    let lock_sha256 = sha256_hex(&lock);

    let helper_target = target_dir.join("screen-ocr-helper");
    fs::create_dir(&helper_target)
        .map_err(|error| format!("fresh independent screen OCR target required: {error}"))?;
    let argv = screen_ocr_cargo_argv(architecture);
    let stdout_path = log_directory.join("screen-ocr-cargo.stdout");
    let stderr_path = log_directory.join("screen-ocr-cargo.stderr");
    let mut command = Command::new("cargo");
    command
        .current_dir(checkout)
        .args(&argv)
        .env("CARGO_TARGET_DIR", &helper_target)
        .env("CARGO_INCREMENTAL", "0")
        .env("SOURCE_DATE_EPOCH", epoch)
        .env("ZERO_AR_DATE", "1")
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_ENCODED_RUSTFLAGS", "-Ctarget-feature=+crt-static")
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env(super::OFFLINE, "1")
        .stdin(Stdio::null());
    let status = capture_build_command(
        &mut command,
        &stdout_path,
        &stderr_path,
        &log_directory.join("screen-ocr-cargo.exit.json"),
    )?;
    if !status.success() {
        return Err(format!(
            "independent screen OCR Cargo failed with actual exit {:?}; original logs retained at {}",
            status.code(),
            log_directory.display()
        ));
    }
    let stdout = read_bounded(&stdout_path, LOG_LIMIT)?;
    let stderr = read_bounded(&stderr_path, LOG_LIMIT)?;
    let text = std::str::from_utf8(&stdout)
        .map_err(|error| format!("screen OCR Cargo JSON is not UTF-8: {error}"))?;
    require_finished_cargo(text)?;
    let artifacts = super::bind_cargo_json(text, triple).map_err(|error| error.to_string())?;
    let expected_id = ArtifactId {
        package: crate::screen_ocr::HELPER_PACKAGE.into(),
        bin: crate::screen_ocr::HELPER_BIN.into(),
        triple: triple.into(),
    };
    if artifacts.len() != 1 {
        return Err("independent screen OCR build emitted an unexpected artifact set".into());
    }
    let path = artifacts
        .get(&expected_id)
        .ok_or("independent screen OCR helper artifact is missing or wrong-triple")?
        .canonicalize()
        .map_err(|error| format!("resolve independent screen OCR helper: {error}"))?;
    let expected_parent = helper_target
        .join(triple)
        .join("release")
        .canonicalize()
        .map_err(|error| format!("resolve independent screen OCR output directory: {error}"))?;
    if path.parent() != Some(expected_parent.as_path())
        || path.file_name().and_then(|name| name.to_str())
            != Some(format!("{}.exe", crate::screen_ocr::HELPER_BIN).as_str())
    {
        return Err("independent screen OCR helper escaped its fresh target output".into());
    }
    let artifact = read_bounded(&path, PE_LIMIT)?;
    if artifact.is_empty()
        || sha256_hex(&read_bounded(&lock_path, 64 * 1024 * 1024)?) != lock_sha256
    {
        return Err("independent screen OCR artifact or lock changed during build".into());
    }
    Ok(BuiltScreenOcrHelper {
        path,
        evidence: ScreenOcrCargoEvidence {
            target: triple.into(),
            manifest_sha256: sha256_hex(&manifest),
            lock_sha256,
            recipe_sha256: sha256_hex(&recipe_bytes),
            oar_revision: recipe.source.oar_revision,
            argv,
            actual_exit_code: status
                .code()
                .ok_or("screen OCR Cargo success has no native exit code")?,
            stdout_sha256: sha256_hex(&stdout),
            stderr_sha256: sha256_hex(&stderr),
            artifact_sha256: sha256_hex(&artifact),
            artifact_bytes: artifact.len() as u64,
        },
    })
}

/// The helper's manifest, lock and recipe under `checkout`, joined one
/// component at a time: a bounded read opens the parent as a root, and Windows
/// refuses a root spelled with a forward slash.
fn screen_ocr_input_paths(checkout: &Path) -> [PathBuf; 3] {
    [
        crate::screen_ocr::HELPER_MANIFEST,
        crate::screen_ocr::HELPER_LOCK,
        crate::screen_ocr::RECIPE,
    ]
    .map(|relative| super::windows_stage::join_components(checkout, relative))
}

struct BuiltScreenOcrHelper {
    path: PathBuf,
    evidence: ScreenOcrCargoEvidence,
}

fn screen_ocr_cargo_argv(architecture: WindowsArchitecture) -> Vec<String> {
    [
        "build",
        "--manifest-path",
        crate::screen_ocr::HELPER_MANIFEST,
        "--locked",
        "--offline",
        "--release",
        "--target",
        architecture.rust_triple(),
        "--message-format=json",
        "--jobs",
        "2",
    ]
    .map(str::to_owned)
    .to_vec()
}

fn cargo_argv(
    inventory: &Inventory,
    architecture: WindowsArchitecture,
) -> Result<Vec<String>, String> {
    let mut args = [
        "build",
        "--manifest-path",
        "core/Cargo.toml",
        "--locked",
        "--offline",
        "--release",
        "--target",
        architecture.rust_triple(),
        "--message-format=json",
        "--jobs",
        "2",
    ]
    .map(str::to_owned)
    .to_vec();
    let mut count = 0;
    let mut journal_core = false;
    for entry in &inventory.entry {
        if let Entry::Bin {
            package,
            bin,
            targets,
            ..
        } = entry
            && targets.iter().any(|t| t == architecture.target_id())
            && package != crate::screen_ocr::HELPER_PACKAGE
        {
            args.extend(["-p".into(), package.clone(), "--bin".into(), bin.clone()]);
            if package == "solstone-core" && bin == "solstone-core" {
                journal_core = true;
            }
            count += 1;
        }
    }
    if count == 0 {
        return Err("Windows inventory declares no Cargo commands".into());
    }
    if journal_core {
        args.extend(super::shipped_core_feature_args());
    }
    Ok(args)
}

pub(crate) fn capture_source(checkout: &Path) -> Result<Provenance, String> {
    let status = super::git_stdout(
        checkout,
        &["status", "--porcelain", "--untracked-files=all"],
    )
    .map_err(|e| e.to_string())?;
    crate::provenance::require_clean(!status.is_empty()).map_err(|e| e.to_string())?;
    Ok(Provenance {
        commit: super::git_stdout(checkout, &["rev-parse", "HEAD"]).map_err(|e| e.to_string())?,
        lock_sha256: crate::provenance::lock_digest(&checkout.join("core/Cargo.lock"))
            .map_err(|e| e.to_string())?,
    })
}

fn create_log(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))
}

// File workers are created before native launch, so partial worker setup cannot
// abandon an already running build. This is byte capture, not a process owner.
// The original Child stays alive until both workers have reached EOF and closed
// their files. A held pipe may block this function; the reviewed outer driver
// supplies the finite deadline and retains the host fence on incomplete capture.
fn capture_build_command(
    command: &mut Command,
    stdout_path: &Path,
    stderr_path: &Path,
    exit_path: &Path,
) -> Result<std::process::ExitStatus, String> {
    let stdout_file = create_log(stdout_path)?;
    let stderr_file = create_log(stderr_path)?;
    let (out_sender, out_receiver) = std::sync::mpsc::channel();
    let (err_sender, err_receiver) = std::sync::mpsc::channel();
    let out_worker = std::thread::Builder::new()
        .name("cargo-stdout".into())
        .spawn(move || drain_build_stream::<std::process::ChildStdout>(out_receiver, stdout_file))
        .map_err(|e| format!("prepare Cargo stdout capture before launch: {e}"))?;
    let err_worker = match std::thread::Builder::new()
        .name("cargo-stderr".into())
        .spawn(move || drain_build_stream::<std::process::ChildStderr>(err_receiver, stderr_file))
    {
        Ok(worker) => worker,
        Err(e) => {
            drop(out_sender);
            let settled = out_worker.join();
            return Err(format!(
                "prepare Cargo stderr capture before launch: {e}; stdout setup settlement: {settled:?}"
            ));
        }
    };
    let launched = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match launched {
        Ok(child) => child,
        Err(e) => {
            drop(out_sender);
            drop(err_sender);
            let out = out_worker.join();
            let err = err_worker.join();
            return Err(format!(
                "Cargo launch failed: {e}; capture setup settlement: {out:?}, {err:?}"
            ));
        }
    };
    // Both streams are configured here; absence would be a std::process invariant
    // failure. Still run the root wait and both joins on every delivery result.
    let out_sent = child
        .stdout
        .take()
        .ok_or("missing piped Cargo stdout")
        .and_then(|pipe| {
            out_sender
                .send(pipe)
                .map_err(|_| "Cargo stdout worker ended before delivery")
        });
    let err_sent = child
        .stderr
        .take()
        .ok_or("missing piped Cargo stderr")
        .and_then(|pipe| {
            err_sender
                .send(pipe)
                .map_err(|_| "Cargo stderr worker ended before delivery")
        });
    drop(out_sender);
    drop(err_sender);
    let status = child
        .wait()
        .map_err(|e| format!("Cargo wait failed; retain build for reconciliation: {e}"));
    // Persist the actual root result before waiting on inherited writers. No log
    // digest or successful build evidence is produced until both joins succeed.
    let persisted = match &status {
        Ok(status) => (|| {
            let mut file = create_log(exit_path)?;
            file.write_all(&serde_json::to_vec(&status.code()).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())
        })(),
        Err(_) => Ok(()),
    };
    let out = out_worker.join();
    let err = err_worker.join();
    // Explicitly retain the original process handle through stream settlement.
    drop(child);
    let mut failures = Vec::new();
    for sent in [out_sent, err_sent] {
        if let Err(error) = sent {
            failures.push(error.to_owned());
        }
    }
    for (name, result) in [("stdout", out), ("stderr", err)] {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(format!("{name}: {error}")),
            Err(_) => failures.push(format!("{name} worker panicked; capture incomplete")),
        }
    }
    if let Err(error) = persisted {
        failures.push(format!("persist actual Cargo exit: {error}"));
    }
    if !failures.is_empty() {
        return Err(format!(
            "Cargo root exit {:?}; capture refused: {}",
            status.as_ref().map(|value| value.code()),
            failures.join("; ")
        ));
    }
    status
}

fn drain_build_stream<R: Read>(
    receiver: std::sync::mpsc::Receiver<R>,
    mut file: File,
) -> Result<(), String> {
    let Ok(stream) = receiver.recv() else {
        return Ok(());
    };
    let size = std::io::copy(&mut stream.take(LOG_LIMIT + 1), &mut file)
        .map_err(|e| format!("Cargo raw stream capture failed: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("Cargo raw stream flush failed: {e}"))?;
    if size > LOG_LIMIT {
        return Err("Cargo raw stream exceeds byte limit; retained log is incomplete".into());
    }
    Ok(())
}

fn retain_release_root(
    target: &solstone_core_journal_io::JournalRoot,
    architecture: WindowsArchitecture,
) -> Result<solstone_core_journal_io::JournalRoot, String> {
    target
        .revalidate_canonical_binding()
        .map_err(|e| format!("fresh target binding changed: {e}"))?;
    solstone_core_journal_io::JournalRoot::open(
        &target
            .canonical_path()
            .join(architecture.rust_triple())
            .join("release"),
    )
    .map_err(|e| format!("retain fresh release output directory: {e}"))
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let parent = path.parent().ok_or("input has no parent")?;
    let leaf = path.file_name().ok_or("input has no filename")?;
    let root = solstone_core_journal_io::JournalRoot::open(parent).map_err(|e| e.to_string())?;
    root.revalidate_canonical_binding()
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let bytes = solstone_core_journal_io::read_observed_root_file_bounded(
        &root,
        leaf,
        usize::try_from(limit).map_err(|_| "input limit exceeds address space")?,
    )
    .map_err(|e| e.to_string())?
    .ok_or("input is missing")?
    .bytes;
    #[cfg(windows)]
    let bytes = {
        use std::io::Read;
        let file = solstone_core_journal_io::open_windows_regular_file_from_bound_parent(
            &root,
            leaf,
            root.canonical_path(),
        )
        .map_err(|e| e.to_string())?
        .ok_or("input is missing")?;
        let before = file.metadata().map_err(|e| e.to_string())?;
        if before.len() > limit {
            return Err("input exceeds byte limit".into());
        }
        let mut bytes = Vec::new();
        (&file)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let after = file.metadata().map_err(|e| e.to_string())?;
        if bytes.len() as u64 != before.len()
            || after.len() != before.len()
            || after.modified().map_err(|e| e.to_string())?
                != before.modified().map_err(|e| e.to_string())?
        {
            return Err("input changed during bounded read".into());
        }
        bytes
    };
    root.revalidate_canonical_binding()
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn require_finished_cargo(text: &str) -> Result<(), String> {
    let mut finished = 0;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let message: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("invalid Cargo JSON: {e}"))?;
        if message["reason"] == "build-finished" {
            if message["success"] != true || finished != 0 {
                return Err("Cargo did not emit one successful build-finished record".into());
            }
            finished += 1;
        } else if finished != 0 {
            return Err("Cargo emitted output after build-finished".into());
        }
        if message["reason"] == "compiler-artifact" && message["fresh"] != false {
            return Err("fresh Windows target unexpectedly reused a Cargo artifact".into());
        }
    }
    if finished != 1 {
        return Err("Cargo successful build-finished record is missing".into());
    }
    Ok(())
}

/// Integration-only access to the actual production capture boundary.
#[cfg(feature = "test-hooks")]
pub fn capture_build_command_for_test(
    command: &mut Command,
    stdout: &Path,
    stderr: &Path,
    exit: &Path,
) -> Result<std::process::ExitStatus, String> {
    capture_build_command(command, stdout, stderr, exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn producer_component_paths_admit_nested_bounded_inputs() {
        let temporary = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let root = temporary.path().to_path_buf();
        #[cfg(not(windows))]
        let root = temporary.path().canonicalize().unwrap();
        let source =
            super::super::windows_stage::join_components(&root, "core/distribution/notice.txt");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"original notice").unwrap();
        assert_eq!(read_bounded(&source, 15).unwrap(), b"original notice");
        assert!(read_bounded(&source, 3).is_err());
        #[cfg(windows)]
        {
            // A verbatim-prefix root can normalize push(), masking this bug.
            assert!(!root.as_os_str().to_string_lossy().starts_with(r"\\?\"));
            assert!(read_bounded(&root.join("core/distribution/notice.txt"), 15).is_err());
        }
        let binary = super::super::windows_stage::join_components(&root, "bin/worker.exe");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"controlled bytes").unwrap();
        assert_eq!(read_bounded(&binary, 16).unwrap(), b"controlled bytes");
        for architecture in WindowsArchitecture::ALL {
            let target = solstone_core_journal_io::JournalRoot::open(&root).unwrap();
            let release = root.join(architecture.rust_triple()).join("release");
            fs::create_dir_all(&release).unwrap();
            let retained = retain_release_root(&target, architecture).unwrap();
            let command = retained.canonical_path().join("worker.exe");
            fs::write(&command, b"current output").unwrap();
            assert_eq!(
                command.canonicalize().unwrap().parent(),
                Some(retained.canonical_path().canonicalize().unwrap().as_path()),
            );
            assert_eq!(read_bounded(&command, 14).unwrap(), b"current output");
            retained.revalidate_canonical_binding().unwrap();
        }
    }

    #[test]
    fn screen_ocr_helper_inputs_are_read_through_component_paths() {
        let temporary = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let root = temporary.path().to_path_buf();
        #[cfg(not(windows))]
        let root = temporary.path().canonicalize().unwrap();
        for (path, relative) in screen_ocr_input_paths(&root).iter().zip([
            crate::screen_ocr::HELPER_MANIFEST,
            crate::screen_ocr::HELPER_LOCK,
            crate::screen_ocr::RECIPE,
        ]) {
            assert_eq!(
                path,
                &super::super::windows_stage::join_components(&root, relative)
            );
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, relative.as_bytes()).unwrap();
            assert_eq!(read_bounded(path, 64).unwrap(), relative.as_bytes());
            #[cfg(windows)]
            assert!(read_bounded(&root.join(relative), 64).is_err());
        }
    }

    #[test]
    fn cargo_completion_and_freshness_are_required_together() {
        let artifact = r#"{"reason":"compiler-artifact","fresh":false}"#;
        let finish = r#"{"reason":"build-finished","success":true}"#;
        require_finished_cargo(&format!("{artifact}\n{finish}\n")).unwrap();
        for text in [
            artifact.to_owned(),
            format!("{finish}\n{artifact}"),
            format!("{finish}\n{finish}"),
            format!("{}\n{finish}", artifact.replace("false", "true")),
            finish.replace("true", "false"),
            "invalid".into(),
        ] {
            assert!(require_finished_cargo(&text).is_err(), "{text}");
        }
    }

    #[test]
    fn cargo_commands_come_from_the_windows_inventory() {
        let inventory_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../distribution/inventory.toml")
            .canonicalize()
            .unwrap();
        for architecture in WindowsArchitecture::ALL {
            let mut inventory = crate::inventory::load_inventory(&inventory_path).unwrap();
            if architecture == WindowsArchitecture::Aarch64
                && !inventory.entry.iter().any(|entry| matches!(entry, Entry::Bin { targets, .. } if targets.iter().any(|target| target == architecture.target_id())))
            {
                assert!(cargo_argv(&inventory, architecture).is_err());
                // The production ARM inventory is admitted with its native closure.
                // Exercise this build API using explicitly declared ARM commands.
                for entry in &mut inventory.entry {
                    if let Entry::Bin { targets, .. } = entry
                        && targets
                            .iter()
                            .any(|target| target == WindowsArchitecture::X86_64.target_id())
                    {
                        targets.push(architecture.target_id().into());
                    }
                }
            }
            let argv = cargo_argv(&inventory, architecture).unwrap();
            for command in ["solstone-core-sol", "solstone-core-ced-analyze"] {
                assert!(argv.windows(2).any(|pair| pair == ["--bin", command]));
            }
            assert!(
                !argv
                    .windows(2)
                    .any(|pair| pair == ["--bin", "solstone-core-journal"]),
                "Windows cargo argv builds solstone.exe, not a journal.exe binary"
            );
            assert!(!argv.iter().any(|s| s == "solstone-core-llama"));
            assert!(
                argv.windows(2)
                    .any(|pair| pair == ["--target", architecture.rust_triple()])
            );
            assert_eq!(
                argv.windows(2)
                    .filter(|pair| *pair == ["--features", "solstone-core/journal-mcp-endpoint"])
                    .count(),
                1,
                "the agent connector is compiled into the Windows journal exactly once"
            );
            assert!(
                !argv
                    .iter()
                    .any(|argument| argument == crate::screen_ocr::HELPER_PACKAGE)
            );
            let helper = screen_ocr_cargo_argv(architecture);
            assert!(
                helper.windows(2).any(|pair| {
                    pair == ["--manifest-path", crate::screen_ocr::HELPER_MANIFEST]
                })
            );
            assert!(helper.iter().any(|argument| argument == "--locked"));
            assert!(
                helper
                    .windows(2)
                    .any(|pair| pair == ["--target", architecture.rust_triple()])
            );
            assert!(!helper.iter().any(|argument| argument == "core/Cargo.toml"));
        }
    }

    #[test]
    fn windows_builds_the_screen_ocr_helper_only_for_its_selected_architecture() {
        let inventory_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../distribution/inventory.toml")
            .canonicalize()
            .unwrap();
        for selected in WindowsArchitecture::ALL {
            let mut inventory = crate::inventory::load_inventory(&inventory_path).unwrap();
            for entry in &mut inventory.entry {
                if let Entry::Bin {
                    package, targets, ..
                } = entry
                    && package == crate::screen_ocr::HELPER_PACKAGE
                {
                    targets.retain(|target| WindowsArchitecture::from_target_id(target).is_none());
                }
            }
            for architecture in WindowsArchitecture::ALL {
                assert!(!screen_ocr_helper_selected(&inventory, architecture));
            }
            for entry in &mut inventory.entry {
                if let Entry::Bin {
                    package, targets, ..
                } = entry
                    && package == crate::screen_ocr::HELPER_PACKAGE
                {
                    targets.push(selected.target_id().into());
                }
            }
            for architecture in WindowsArchitecture::ALL {
                assert_eq!(
                    screen_ocr_helper_selected(&inventory, architecture),
                    architecture == selected,
                );
            }
        }
    }
}
