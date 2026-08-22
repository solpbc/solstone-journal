// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared policy for admitting and configuring FFmpeg source builds.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

pub const BUILD_RUN_ID_ENV: &str = "SOLSTONE_FFMPEG_BUILD_RUN_ID";
pub const EVIDENCE_DIR: &str = "solstone-ffmpeg-evidence";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    Developer,
    Release,
}

pub fn parse_build_profile(value: &str) -> Result<BuildProfile, String> {
    match value {
        "debug" => Ok(BuildProfile::Developer),
        "release" => Ok(BuildProfile::Release),
        _ => Err(format!("unexpected:\n  PROFILE value {value}")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureMode {
    Debug,
    Release,
}

pub fn parse_debug_env(value: &str) -> Result<ConfigureMode, String> {
    match value {
        "true" => Ok(ConfigureMode::Debug),
        "false" => Ok(ConfigureMode::Release),
        _ => Err(format!("unexpected:\n  DEBUG value {value}")),
    }
}

pub fn configure_mode_args(mode: ConfigureMode, windows: bool) -> Vec<String> {
    match mode {
        ConfigureMode::Debug => vec!["--enable-debug".into(), "--disable-stripping".into()],
        ConfigureMode::Release => {
            let mut args = vec![
                "--disable-debug".into(),
                "--enable-stripping".into(),
                "--extra-cflags=-O3 -ffast-math -funroll-loops".into(),
            ];
            if !windows {
                args.push("--extra-ldflags=-flto".into());
            }
            args
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAdmission {
    UseArchive,
    Fetch,
}

pub fn select_source_admission(
    profile: BuildProfile,
    archive_present: bool,
    offline_present: bool,
) -> Result<SourceAdmission, String> {
    match (profile, archive_present, offline_present) {
        (BuildProfile::Release, false, _) => {
            Err("missing required:\n  SOLSTONE_FFMPEG_SOURCE_ARCHIVE for release profile".into())
        }
        (_, true, _) => Ok(SourceAdmission::UseArchive),
        (BuildProfile::Developer, false, true) => {
            Err("missing required:\n  SOLSTONE_FFMPEG_SOURCE_ARCHIVE while offline".into())
        }
        (BuildProfile::Developer, false, false) => Ok(SourceAdmission::Fetch),
    }
}

pub fn run_fetch_if_selected(
    admission: SourceAdmission,
    mut fetch: impl FnMut() -> Result<(), String>,
) -> Result<SourceAdmission, String> {
    if admission == SourceAdmission::Fetch {
        fetch()?;
    }
    Ok(admission)
}

pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("unexpected:\n  SHA-256 input is empty".into());
    }

    let actual = sha256_hex(bytes);
    if actual == expected_hex {
        Ok(())
    } else {
        Err(format!("unexpected:\n  SHA-256 digest {actual}"))
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn configure_fingerprint(
    target: &str,
    profile: &str,
    source_sha256: &str,
    program: &str,
    args: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"solstone-ffmpeg-configure-fingerprint-v1");
    fingerprint_field(&mut hasher, target);
    fingerprint_field(&mut hasher, profile);
    fingerprint_field(&mut hasher, source_sha256);
    fingerprint_field(&mut hasher, program);
    hasher.update((args.len() as u64).to_be_bytes());
    for arg in args {
        fingerprint_field(&mut hasher, arg);
    }
    format!("{:x}", hasher.finalize())
}

fn fingerprint_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureReceipt {
    pub target: String,
    pub profile: String,
    pub source_sha256: String,
    pub program: String,
    pub args: Vec<String>,
    pub fingerprint: String,
}

impl ConfigureReceipt {
    pub fn new(
        target: &str,
        profile: &str,
        source_sha256: &str,
        program: &str,
        args: &[String],
    ) -> Self {
        Self {
            target: target.to_owned(),
            profile: profile.to_owned(),
            source_sha256: source_sha256.to_owned(),
            program: program.to_owned(),
            args: args.to_vec(),
            fingerprint: configure_fingerprint(target, profile, source_sha256, program, args),
        }
    }

    pub fn filename(&self) -> Result<String, String> {
        configure_receipt_filename(&self.target, &self.fingerprint)
    }

    pub fn serialize(&self) -> Result<String, String> {
        self.validate()?;
        let mut text = String::from("version=1\n");
        append_line(&mut text, "target", &self.target)?;
        append_line(&mut text, "profile", &self.profile)?;
        append_line(&mut text, "source_sha256", &self.source_sha256)?;
        append_line(&mut text, "program", &self.program)?;
        for arg in &self.args {
            append_line(&mut text, "arg", arg)?;
        }
        append_line(&mut text, "fingerprint", &self.fingerprint)?;
        Ok(text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut version = None;
        let mut target = None;
        let mut profile = None;
        let mut source_sha256 = None;
        let mut program = None;
        let mut args = Vec::new();
        let mut fingerprint = None;
        for line in text.split_terminator('\n') {
            let (key, value) = parse_line(line)?;
            match key {
                "version" => set_once(&mut version, key, value)?,
                "target" => set_once(&mut target, key, value)?,
                "profile" => set_once(&mut profile, key, value)?,
                "source_sha256" => set_once(&mut source_sha256, key, value)?,
                "program" => set_once(&mut program, key, value)?,
                "arg" => args.push(value.to_owned()),
                "fingerprint" => set_once(&mut fingerprint, key, value)?,
                _ => return Err(format!("unexpected:\n  configure receipt key {key}")),
            }
        }
        if version.as_deref() != Some("1") {
            return Err("unexpected:\n  configure receipt version".into());
        }
        let receipt = Self {
            target: required_record_field(target, "target")?,
            profile: required_record_field(profile, "profile")?,
            source_sha256: required_record_field(source_sha256, "source_sha256")?,
            program: required_record_field(program, "program")?,
            args,
            fingerprint: required_record_field(fingerprint, "fingerprint")?,
        };
        receipt.validate()?;
        if receipt.serialize()? != text {
            return Err("unexpected:\n  non-canonical configure receipt".into());
        }
        Ok(receipt)
    }

    fn validate(&self) -> Result<(), String> {
        for (key, value) in [
            ("target", &self.target),
            ("profile", &self.profile),
            ("source_sha256", &self.source_sha256),
            ("program", &self.program),
            ("fingerprint", &self.fingerprint),
        ] {
            validate_line_value(key, value)?;
        }
        for arg in &self.args {
            validate_line_value("arg", arg)?;
        }
        let expected = configure_fingerprint(
            &self.target,
            &self.profile,
            &self.source_sha256,
            &self.program,
            &self.args,
        );
        if self.fingerprint != expected {
            return Err("unexpected:\n  configure receipt fingerprint".into());
        }
        self.filename()?;
        Ok(())
    }
}

pub fn configure_receipt_filename(target: &str, fingerprint: &str) -> Result<String, String> {
    if target.is_empty()
        || !target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!("unexpected:\n  configure receipt target {target}"));
    }
    if fingerprint.len() != 64
        || !fingerprint
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("unexpected:\n  configure receipt fingerprint filename".into());
    }
    Ok(format!("configure-{target}-{fingerprint}.v1"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConfigureReceipt {
    pub receipt: ConfigureReceipt,
    pub content_sha256: String,
}

pub fn write_configure_receipt(
    evidence_dir: &Path,
    receipt: &ConfigureReceipt,
) -> Result<StoredConfigureReceipt, String> {
    let filename = receipt.filename()?;
    let content = receipt.serialize()?;
    let expected_sha256 = sha256_hex(content.as_bytes());
    fs::create_dir_all(evidence_dir).map_err(|error| error.to_string())?;
    let path = evidence_dir.join(&filename);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => file
            .write_all(content.as_bytes())
            .map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let stored = read_configure_receipt(evidence_dir, &filename)?;
            if stored.receipt != *receipt || stored.content_sha256 != expected_sha256 {
                return Err("unexpected:\n  conflicting configure receipt".into());
            }
            return Ok(stored);
        }
        Err(error) => return Err(error.to_string()),
    }
    read_configure_receipt(evidence_dir, &filename)
}

pub fn read_configure_receipt(
    evidence_dir: &Path,
    filename: &str,
) -> Result<StoredConfigureReceipt, String> {
    validate_filename(filename)?;
    let content =
        fs::read_to_string(evidence_dir.join(filename)).map_err(|error| error.to_string())?;
    let receipt = ConfigureReceipt::parse(&content)?;
    if receipt.filename()? != filename {
        return Err("unexpected:\n  configure receipt filename".into());
    }
    Ok(StoredConfigureReceipt {
        receipt,
        content_sha256: sha256_hex(content.as_bytes()),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigureRunRecord {
    pub run_id: String,
    pub target: String,
    pub profile: String,
    pub source_sha256: String,
    pub fingerprint: String,
    pub configure_executed: bool,
    pub receipt_filename: String,
    pub receipt_sha256: String,
}

impl ConfigureRunRecord {
    pub fn new(
        run_id: &str,
        receipt: &StoredConfigureReceipt,
        configure_executed: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            run_id: run_id.to_owned(),
            target: receipt.receipt.target.clone(),
            profile: receipt.receipt.profile.clone(),
            source_sha256: receipt.receipt.source_sha256.clone(),
            fingerprint: receipt.receipt.fingerprint.clone(),
            configure_executed,
            receipt_filename: receipt.receipt.filename()?,
            receipt_sha256: receipt.content_sha256.clone(),
        })
    }

    pub fn serialize(&self) -> Result<String, String> {
        self.validate()?;
        let mut text = String::from("version=1\n");
        append_line(&mut text, "run_id", &self.run_id)?;
        append_line(&mut text, "target", &self.target)?;
        append_line(&mut text, "profile", &self.profile)?;
        append_line(&mut text, "source_sha256", &self.source_sha256)?;
        append_line(&mut text, "fingerprint", &self.fingerprint)?;
        append_line(
            &mut text,
            "configure_executed",
            if self.configure_executed {
                "true"
            } else {
                "false"
            },
        )?;
        append_line(&mut text, "receipt_filename", &self.receipt_filename)?;
        append_line(&mut text, "receipt_sha256", &self.receipt_sha256)?;
        Ok(text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut version = None;
        let mut run_id = None;
        let mut target = None;
        let mut profile = None;
        let mut source_sha256 = None;
        let mut fingerprint = None;
        let mut configure_executed = None;
        let mut receipt_filename = None;
        let mut receipt_sha256 = None;
        for line in text.split_terminator('\n') {
            let (key, value) = parse_line(line)?;
            match key {
                "version" => set_once(&mut version, key, value)?,
                "run_id" => set_once(&mut run_id, key, value)?,
                "target" => set_once(&mut target, key, value)?,
                "profile" => set_once(&mut profile, key, value)?,
                "source_sha256" => set_once(&mut source_sha256, key, value)?,
                "fingerprint" => set_once(&mut fingerprint, key, value)?,
                "configure_executed" => set_once(&mut configure_executed, key, value)?,
                "receipt_filename" => set_once(&mut receipt_filename, key, value)?,
                "receipt_sha256" => set_once(&mut receipt_sha256, key, value)?,
                _ => return Err(format!("unexpected:\n  configure run record key {key}")),
            }
        }
        if version.as_deref() != Some("1") {
            return Err("unexpected:\n  configure run record version".into());
        }
        let configure_executed =
            match required_record_field(configure_executed, "configure_executed")?.as_str() {
                "true" => true,
                "false" => false,
                value => return Err(format!("unexpected:\n  configure_executed {value}")),
            };
        let record = Self {
            run_id: required_record_field(run_id, "run_id")?,
            target: required_record_field(target, "target")?,
            profile: required_record_field(profile, "profile")?,
            source_sha256: required_record_field(source_sha256, "source_sha256")?,
            fingerprint: required_record_field(fingerprint, "fingerprint")?,
            configure_executed,
            receipt_filename: required_record_field(receipt_filename, "receipt_filename")?,
            receipt_sha256: required_record_field(receipt_sha256, "receipt_sha256")?,
        };
        record.validate()?;
        if record.serialize()? != text {
            return Err("unexpected:\n  non-canonical configure run record".into());
        }
        Ok(record)
    }

    fn validate(&self) -> Result<(), String> {
        for (key, value) in [
            ("run_id", &self.run_id),
            ("target", &self.target),
            ("profile", &self.profile),
            ("source_sha256", &self.source_sha256),
            ("fingerprint", &self.fingerprint),
            ("receipt_filename", &self.receipt_filename),
            ("receipt_sha256", &self.receipt_sha256),
        ] {
            validate_line_value(key, value)?;
        }
        if configure_receipt_filename(&self.target, &self.fingerprint)? != self.receipt_filename {
            return Err("unexpected:\n  configure run record receipt filename".into());
        }
        Ok(())
    }
}

pub fn write_current_run_record(
    evidence_dir: &Path,
    record: &ConfigureRunRecord,
) -> Result<PathBuf, String> {
    fs::create_dir_all(evidence_dir).map_err(|error| error.to_string())?;
    let path = evidence_dir.join("current-run.v1");
    fs::write(&path, record.serialize()?).map_err(|error| error.to_string())?;
    Ok(path)
}

pub fn read_current_run_record(evidence_dir: &Path) -> Result<ConfigureRunRecord, String> {
    let text = fs::read_to_string(evidence_dir.join("current-run.v1"))
        .map_err(|error| error.to_string())?;
    ConfigureRunRecord::parse(&text)
}

fn append_line(text: &mut String, key: &str, value: &str) -> Result<(), String> {
    validate_line_value(key, value)?;
    text.push_str(key);
    text.push('=');
    text.push_str(value);
    text.push('\n');
    Ok(())
}

fn parse_line(line: &str) -> Result<(&str, &str), String> {
    if line.is_empty() || line.contains('\r') {
        return Err("unexpected:\n  configure evidence line".into());
    }
    line.split_once('=')
        .ok_or_else(|| "unexpected:\n  configure evidence line".into())
}

fn set_once(slot: &mut Option<String>, key: &str, value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!(
            "unexpected:\n  duplicate configure evidence key {key}"
        ));
    }
    *slot = Some(value.to_owned());
    Ok(())
}

fn required_record_field(value: Option<String>, key: &str) -> Result<String, String> {
    value.ok_or_else(|| format!("missing required:\n  configure evidence {key}"))
}

fn validate_line_value(key: &str, value: &str) -> Result<(), String> {
    if value.contains(['\n', '\r']) {
        return Err(format!(
            "unexpected:\n  configure evidence {key} contains newline"
        ));
    }
    Ok(())
}

fn validate_filename(filename: &str) -> Result<(), String> {
    let mut components = Path::new(filename).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err("unexpected:\n  configure receipt filename".into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegPin {
    pub commit: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

impl FfmpegPin {
    pub fn archive_root(&self) -> String {
        format!("FFmpeg-{}", self.commit)
    }
}

pub fn parse_ffmpeg_pin(builder_inputs_toml_text: &str) -> Result<FfmpegPin, String> {
    let mut in_ffmpeg = false;
    let mut ffmpeg_seen = false;
    let mut commit = None;
    let mut filename = None;
    let mut url = None;
    let mut sha256 = None;
    let mut size = None;

    for raw_line in builder_inputs_toml_text.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            if line == "[ffmpeg]" {
                if ffmpeg_seen {
                    return Err("unexpected:\n  duplicate [ffmpeg] table".into());
                }
                ffmpeg_seen = true;
                in_ffmpeg = true;
            } else {
                in_ffmpeg = false;
            }
            continue;
        }
        if !in_ffmpeg || line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, raw_value)) = line.split_once('=') else {
            return Err(format!("unexpected:\n  ffmpeg table entry {line}"));
        };
        let key = key.trim();
        let raw_value = raw_value.trim();
        match key {
            "commit" => set_string_value(&mut commit, "commit", raw_value)?,
            "filename" => set_string_value(&mut filename, "filename", raw_value)?,
            "url" => set_string_value(&mut url, "url", raw_value)?,
            "sha256" => set_string_value(&mut sha256, "sha256", raw_value)?,
            "size" => set_size_value(&mut size, raw_value)?,
            _ => {}
        }
    }

    if !ffmpeg_seen {
        return Err("missing required:\n  [ffmpeg] table".into());
    }

    Ok(FfmpegPin {
        commit: required_field(commit, "commit")?,
        filename: required_field(filename, "filename")?,
        url: required_field(url, "url")?,
        sha256: required_field(sha256, "sha256")?,
        size: size.ok_or_else(|| "missing required:\n  ffmpeg.size".to_owned())?,
    })
}

fn set_string_value(slot: &mut Option<String>, key: &str, raw_value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("unexpected:\n  duplicate ffmpeg.{key}"));
    }
    let Some(value) = raw_value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return Err(format!(
            "unexpected:\n  ffmpeg.{key} must be a quoted string"
        ));
    };
    *slot = Some(value.to_owned());
    Ok(())
}

fn set_size_value(slot: &mut Option<u64>, raw_value: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err("unexpected:\n  duplicate ffmpeg.size".into());
    }
    *slot = Some(
        raw_value
            .parse()
            .map_err(|_| format!("unexpected:\n  ffmpeg.size value {raw_value}"))?,
    );
    Ok(())
}

fn required_field(value: Option<String>, key: &str) -> Result<String, String> {
    value.ok_or_else(|| format!("missing required:\n  ffmpeg.{key}"))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const BUILDER_INPUTS: &str = include_str!("../../../distribution/builder-inputs.toml");

    #[test]
    fn parses_cargo_profile_values_strictly() {
        assert_eq!(parse_build_profile("debug"), Ok(BuildProfile::Developer));
        assert_eq!(parse_build_profile("release"), Ok(BuildProfile::Release));

        let error = parse_build_profile("custom").unwrap_err();
        assert!(error.contains("PROFILE"));
        assert!(error.contains("custom"));
    }

    #[test]
    fn parses_cargo_debug_values_strictly() {
        assert_eq!(parse_debug_env("true"), Ok(ConfigureMode::Debug));
        assert_eq!(parse_debug_env("false"), Ok(ConfigureMode::Release));

        let error = parse_debug_env("sometimes").unwrap_err();
        assert!(error.contains("DEBUG"));
        assert!(error.contains("sometimes"));
    }

    #[test]
    fn configure_mode_arguments_match_the_selected_mode() {
        let mistyped_optimization = ["-", "0", "3"].concat();
        let debug_args = configure_mode_args(ConfigureMode::Debug, false);
        assert!(debug_args.contains(&"--enable-debug".into()));
        assert!(debug_args.contains(&"--disable-stripping".into()));
        assert!(
            !debug_args
                .iter()
                .any(|arg| arg.contains(&mistyped_optimization))
        );

        let release_args = configure_mode_args(ConfigureMode::Release, false);
        assert!(release_args.contains(&"--disable-debug".into()));
        assert!(release_args.contains(&"--enable-stripping".into()));
        assert!(release_args.iter().any(|arg| arg.contains("-O3")));
        assert!(
            !release_args
                .iter()
                .any(|arg| arg.contains(&mistyped_optimization))
        );
        assert!(release_args.contains(&"--extra-ldflags=-flto".into()));
        assert!(
            !configure_mode_args(ConfigureMode::Release, true)
                .iter()
                .any(|arg| arg == "--extra-ldflags=-flto")
        );
    }

    #[test]
    fn source_admission_is_independent_of_configure_mode() {
        assert_eq!(
            select_source_admission(BuildProfile::Release, true, false),
            Ok(SourceAdmission::UseArchive)
        );
        assert!(select_source_admission(BuildProfile::Release, false, false).is_err());
        assert!(select_source_admission(BuildProfile::Release, false, true).is_err());
        assert_eq!(
            select_source_admission(BuildProfile::Developer, true, false),
            Ok(SourceAdmission::UseArchive)
        );
        assert!(select_source_admission(BuildProfile::Developer, false, true).is_err());
        assert_eq!(
            select_source_admission(BuildProfile::Developer, false, false),
            Ok(SourceAdmission::Fetch)
        );
    }

    #[test]
    fn fetch_seam_only_runs_for_selected_fetches() {
        let mut called = false;
        let release_result =
            select_source_admission(BuildProfile::Release, false, false).and_then(|admission| {
                run_fetch_if_selected(admission, || {
                    called = true;
                    Ok(())
                })
            });
        assert!(release_result.is_err());
        assert!(!called);

        let offline_result = select_source_admission(BuildProfile::Developer, false, true)
            .and_then(|admission| {
                run_fetch_if_selected(admission, || {
                    called = true;
                    Ok(())
                })
            });
        assert!(offline_result.is_err());
        assert!(!called);

        let fetch_result =
            select_source_admission(BuildProfile::Developer, false, false).and_then(|admission| {
                run_fetch_if_selected(admission, || {
                    called = true;
                    Ok(())
                })
            });
        assert_eq!(fetch_result, Ok(SourceAdmission::Fetch));
        assert!(called);
    }

    #[test]
    fn sha256_verification_refuses_empty_and_mismatched_inputs() {
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(verify_sha256(b"abc", expected), Ok(()));

        let mismatch = verify_sha256(b"abc", "0").unwrap_err();
        assert!(mismatch.contains("SHA-256 digest"));

        let empty = verify_sha256(b"", expected).unwrap_err();
        assert!(empty.contains("empty"));
    }

    #[test]
    fn parses_the_checked_in_ffmpeg_pin_table() {
        let pin = parse_ffmpeg_pin(BUILDER_INPUTS).unwrap();
        assert!(!pin.commit.is_empty());
        assert_eq!(pin.filename, format!("FFmpeg-{}.tar.gz", pin.commit));
        assert_eq!(
            pin.url,
            format!(
                "https://github.com/FFmpeg/FFmpeg/archive/{}.tar.gz",
                pin.commit
            )
        );
        assert_eq!(pin.sha256.len(), 64);
        assert_eq!(pin.size, 17_322_302);
        assert_eq!(pin.archive_root(), format!("FFmpeg-{}", pin.commit));
    }

    #[test]
    fn rejects_ffmpeg_pin_missing_a_required_key() {
        let missing_size = r#"
[ffmpeg]
commit = "commit"
filename = "archive.tar.gz"
url = "https://example.invalid/archive.tar.gz"
sha256 = "digest"
"#;
        let error = parse_ffmpeg_pin(missing_size).unwrap_err();
        assert!(error.contains("ffmpeg.size"));
    }

    fn receipt(args: &[&str]) -> ConfigureReceipt {
        ConfigureReceipt::new(
            "x86_64-unknown-linux-gnu",
            "release",
            &"a".repeat(64),
            "/source/configure",
            &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
        )
    }

    fn evidence_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        PathBuf::from("/var/tmp").join(format!(
            "solstone-ffmpeg-build-support-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn configure_fingerprint_binds_every_field_without_boundary_collisions() {
        let args = vec!["a".to_owned(), "bc".to_owned()];
        let fingerprint = configure_fingerprint("target", "release", "source", "configure", &args);
        assert_ne!(
            fingerprint,
            configure_fingerprint("target", "debug", "source", "configure", &args)
        );
        assert_ne!(
            fingerprint,
            configure_fingerprint("other-target", "release", "source", "configure", &args)
        );
        assert_ne!(
            fingerprint,
            configure_fingerprint("target", "release", "other-source", "configure", &args)
        );
        assert_ne!(
            fingerprint,
            configure_fingerprint("target", "release", "source", "other-configure", &args)
        );
        assert_ne!(
            fingerprint,
            configure_fingerprint(
                "target",
                "release",
                "source",
                "configure",
                &["ab".to_owned(), "c".to_owned()]
            )
        );
        assert_ne!(
            fingerprint,
            configure_fingerprint(
                "target",
                "release",
                "source",
                "configure",
                &["a".to_owned(), "bd".to_owned()]
            )
        );
    }

    #[test]
    fn configure_receipts_round_trip_and_use_content_addressed_filenames() {
        let first = receipt(&["--disable-debug"]);
        let second = receipt(&["--enable-debug"]);
        assert_ne!(first.fingerprint, second.fingerprint);
        assert_ne!(first.filename().unwrap(), second.filename().unwrap());

        let dir = evidence_dir("receipt");
        let stored = write_configure_receipt(&dir, &first).unwrap();
        assert_eq!(stored.receipt, first);
        let reread = read_configure_receipt(&dir, &stored.receipt.filename().unwrap()).unwrap();
        assert_eq!(reread, stored);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn configure_receipt_rejects_a_tampered_fingerprint_binding() {
        let receipt = receipt(&["--disable-debug"]);
        let tampered = receipt
            .serialize()
            .unwrap()
            .replace("arg=--disable-debug", "arg=--enable-debug");
        let dir = evidence_dir("tampered-receipt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(receipt.filename().unwrap()), tampered).unwrap();
        let error = read_configure_receipt(&dir, &receipt.filename().unwrap()).unwrap_err();
        assert!(error.contains("fingerprint"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn configure_run_records_round_trip_with_receipt_digest() {
        let dir = evidence_dir("run-record");
        let stored = write_configure_receipt(&dir, &receipt(&["--disable-debug"])).unwrap();
        let record = ConfigureRunRecord::new("run-1", &stored, false).unwrap();
        write_current_run_record(&dir, &record).unwrap();
        assert_eq!(read_current_run_record(&dir).unwrap(), record);
        let _ = fs::remove_dir_all(dir);
    }
}
