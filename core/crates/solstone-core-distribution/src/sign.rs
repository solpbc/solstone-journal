// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};

use minisign::{PublicKey, PublicKeyBox, SecretKey, SecretKeyBox, SignatureBox};
use serde::Deserialize;

use crate::digest::sha256_hex;

const PRODUCT_PIN: &str = include_str!("../../../../packaging/keys/solstone-journal-release.pub");
const KEY_ENV: &str = "SOLSTONE_JOURNAL_MINISIGN_KEY";
#[cfg(feature = "test-fixture-pin")]
const PIN_OVERRIDE_ENV: &str = "SOLSTONE_JOURNAL_MINISIGN_PIN";
const UNTRUSTED_COMMENT: &str = "signature from solstone-journal release key";
const MANIFEST_SUFFIX: &str = ".manifest.json";
const MINISIG_SUFFIX: &str = ".minisig";
const PARTIAL_SUFFIX: &str = ".partial";
const ARCHIVE_SUFFIXES: &[&str] = &[".tar.gz", ".deb", ".rpm", ".pkg"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignRefusal {
    MissingKeyEnv,
    EmptyKeyPath,
    RelativeKeyPath,
    UnreadableKey,
    MissingPassphrase,
    WrongPassphrase,
    MissingManifest,
    AmbiguousManifest,
    UnparseableManifest,
    ListedArchiveAbsent,
    ListedDigestMismatch,
    ExtraUnlistedArchive,
    VerifyAfterSign,
}

impl SignRefusal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingKeyEnv => "missing-key-env",
            Self::EmptyKeyPath => "empty-key-path",
            Self::RelativeKeyPath => "relative-key-path",
            Self::UnreadableKey => "unreadable-key",
            Self::MissingPassphrase => "missing-passphrase",
            Self::WrongPassphrase => "wrong-passphrase",
            Self::MissingManifest => "missing-manifest",
            Self::AmbiguousManifest => "ambiguous-manifest",
            Self::UnparseableManifest => "unparseable-manifest",
            Self::ListedArchiveAbsent => "listed-archive-absent",
            Self::ListedDigestMismatch => "listed-digest-mismatch",
            Self::ExtraUnlistedArchive => "extra-unlisted-archive",
            Self::VerifyAfterSign => "verify-after-sign",
        }
    }
}

#[derive(Debug)]
pub struct SignError {
    kind: SignRefusal,
    detail: String,
}

impl SignError {
    fn new(kind: SignRefusal, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for SignError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}\n  {}", self.kind.as_str(), self.detail)
    }
}

impl std::error::Error for SignError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[allow(dead_code)]
    product: String,
    #[allow(dead_code)]
    version: String,
    #[allow(dead_code)]
    target: String,
    files: BTreeMap<String, String>,
}

struct TempSig {
    path: PathBuf,
    persist: bool,
}

impl Drop for TempSig {
    fn drop(&mut self) {
        if !self.persist {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn run(dir: &Path) -> Result<PathBuf, SignError> {
    let basename = discover_basename(dir)?;
    let manifest_name = format!("{basename}{MANIFEST_SUFFIX}");
    let manifest_path = dir.join(&manifest_name);
    let manifest_bytes = fs::read(&manifest_path).map_err(|error| {
        SignError::new(
            SignRefusal::UnparseableManifest,
            format!("could not read {}: {error}", manifest_path.display()),
        )
    })?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        SignError::new(
            SignRefusal::UnparseableManifest,
            format!("could not parse {}: {error}", manifest_path.display()),
        )
    })?;
    check_completeness(dir, &manifest)?;

    let key_path = load_key_path()?;
    let passphrase = load_passphrase(&key_path)?;
    let secret = load_secret_key(&key_path, passphrase)?;

    let dest = dir.join(format!("{basename}{MANIFEST_SUFFIX}{MINISIG_SUFFIX}"));
    let temp = dir.join(format!(
        "{basename}{MANIFEST_SUFFIX}{MINISIG_SUFFIX}{PARTIAL_SUFFIX}"
    ));
    let mut guard = TempSig {
        path: temp,
        persist: false,
    };

    let public = PublicKey::from_secret_key(&secret).ok();
    let signature = minisign::sign(
        public.as_ref(),
        &secret,
        Cursor::new(manifest_bytes.as_slice()),
        None,
        Some(UNTRUSTED_COMMENT),
    )
    .map_err(|error| {
        SignError::new(
            SignRefusal::UnreadableKey,
            format!("could not produce a signature: {error}"),
        )
    })?;
    fs::write(&guard.path, signature.into_string()).map_err(|error| {
        SignError::new(
            SignRefusal::VerifyAfterSign,
            format!("could not write {}: {error}", guard.path.display()),
        )
    })?;

    let pin = resolve_pin()?;
    let signature_box = SignatureBox::from_file(&guard.path).map_err(|error| {
        SignError::new(
            SignRefusal::VerifyAfterSign,
            format!("could not read {}: {error}", guard.path.display()),
        )
    })?;
    minisign::verify(
        &pin,
        &signature_box,
        Cursor::new(manifest_bytes.as_slice()),
        true,
        false,
        false,
    )
    .map_err(|error| {
        SignError::new(
            SignRefusal::VerifyAfterSign,
            format!(
                "signature at {} did not verify: {error}",
                guard.path.display()
            ),
        )
    })?;

    fs::rename(&guard.path, &dest).map_err(|error| {
        SignError::new(
            SignRefusal::VerifyAfterSign,
            format!("could not replace {}: {error}", dest.display()),
        )
    })?;
    guard.persist = true;
    Ok(dest)
}

fn discover_basename(dir: &Path) -> Result<String, SignError> {
    let entries = fs::read_dir(dir).map_err(|error| {
        SignError::new(
            SignRefusal::MissingManifest,
            format!("could not read {}: {error}", dir.display()),
        )
    })?;
    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            SignError::new(
                SignRefusal::MissingManifest,
                format!("could not read {}: {error}", dir.display()),
            )
        })?;
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(MANIFEST_SUFFIX) {
            manifests.push(name);
        }
    }
    match manifests.as_slice() {
        [] => Err(SignError::new(
            SignRefusal::MissingManifest,
            format!("no *{MANIFEST_SUFFIX} in {}", dir.display()),
        )),
        [name] => Ok(name
            .strip_suffix(MANIFEST_SUFFIX)
            .expect("suffix checked")
            .to_owned()),
        _ => {
            manifests.sort();
            Err(SignError::new(
                SignRefusal::AmbiguousManifest,
                format!(
                    "multiple *{MANIFEST_SUFFIX} in {}: {}",
                    dir.display(),
                    manifests.join(", ")
                ),
            ))
        }
    }
}

fn check_completeness(dir: &Path, manifest: &Manifest) -> Result<(), SignError> {
    for (name, expected) in &manifest.files {
        if !is_safe_archive_name(name) {
            return Err(SignError::new(
                SignRefusal::UnparseableManifest,
                "listed archive name is not a single filename".to_owned(),
            ));
        }
        let path = dir.join(name);
        if !path.is_file() {
            return Err(SignError::new(
                SignRefusal::ListedArchiveAbsent,
                name.clone(),
            ));
        }
        let actual = sha256_hex(&fs::read(&path).map_err(|error| {
            SignError::new(SignRefusal::ListedArchiveAbsent, format!("{name}: {error}"))
        })?);
        if actual != *expected {
            return Err(SignError::new(
                SignRefusal::ListedDigestMismatch,
                name.clone(),
            ));
        }
    }

    let mut extras = Vec::new();
    let entries = fs::read_dir(dir).map_err(|error| {
        SignError::new(
            SignRefusal::ExtraUnlistedArchive,
            format!("could not read {}: {error}", dir.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            SignError::new(
                SignRefusal::ExtraUnlistedArchive,
                format!("could not read {}: {error}", dir.display()),
            )
        })?;
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_archive_name(&name) && !manifest.files.contains_key(&name) {
            extras.push(name);
        }
    }
    extras.sort();
    if !extras.is_empty() {
        return Err(SignError::new(
            SignRefusal::ExtraUnlistedArchive,
            extras.join(", "),
        ));
    }
    Ok(())
}

fn is_safe_archive_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

fn is_archive_name(name: &str) -> bool {
    ARCHIVE_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

fn load_key_path() -> Result<PathBuf, SignError> {
    let value = match std::env::var(KEY_ENV) {
        Ok(value) => value,
        Err(_) => {
            return Err(SignError::new(
                SignRefusal::MissingKeyEnv,
                format!("{KEY_ENV} is unset"),
            ));
        }
    };
    if value.is_empty() {
        return Err(SignError::new(
            SignRefusal::EmptyKeyPath,
            format!("{KEY_ENV} is empty"),
        ));
    }
    let path = PathBuf::from(&value);
    if !path.is_absolute() {
        return Err(SignError::new(SignRefusal::RelativeKeyPath, value));
    }
    Ok(path)
}

fn load_passphrase(key_path: &Path) -> Result<String, SignError> {
    let mut stdin_bytes = Vec::new();
    io::stdin().read_to_end(&mut stdin_bytes).map_err(|error| {
        SignError::new(
            SignRefusal::MissingPassphrase,
            format!("could not read stdin: {error}"),
        )
    })?;
    if !stdin_bytes.is_empty() {
        return decode_passphrase(stdin_bytes);
    }
    let pass_path = pass_path(key_path);
    match fs::read(&pass_path) {
        Ok(bytes) => decode_passphrase(bytes),
        Err(_) => Err(SignError::new(
            SignRefusal::MissingPassphrase,
            "stdin was empty and .pass was absent or unreadable".to_owned(),
        )),
    }
}

fn pass_path(key_path: &Path) -> PathBuf {
    let mut path = key_path.as_os_str().to_os_string();
    path.push(".pass");
    PathBuf::from(path)
}

fn decode_passphrase(bytes: Vec<u8>) -> Result<String, SignError> {
    String::from_utf8(bytes).map_err(|_| {
        SignError::new(
            SignRefusal::MissingPassphrase,
            "passphrase is not valid UTF-8".to_owned(),
        )
    })
}

fn load_secret_key(key_path: &Path, passphrase: String) -> Result<SecretKey, SignError> {
    let text = fs::read_to_string(key_path).map_err(|error| {
        SignError::new(
            SignRefusal::UnreadableKey,
            format!("{}: {error}", key_path.display()),
        )
    })?;
    let boxed = SecretKeyBox::from_string(&text).map_err(|error| {
        SignError::new(
            SignRefusal::UnreadableKey,
            format!("{}: {error}", key_path.display()),
        )
    })?;
    boxed.into_secret_key(Some(passphrase)).map_err(|error| {
        let message = error.to_string();
        if message.contains("Wrong password") {
            SignError::new(
                SignRefusal::WrongPassphrase,
                format!("{}", key_path.display()),
            )
        } else {
            SignError::new(
                SignRefusal::UnreadableKey,
                format!("{}: {error}", key_path.display()),
            )
        }
    })
}

fn parse_pin(text: &str) -> Result<PublicKey, SignError> {
    let boxed = PublicKeyBox::from_string(text).map_err(|error| {
        SignError::new(
            SignRefusal::VerifyAfterSign,
            format!("could not parse public pin: {error}"),
        )
    })?;
    PublicKey::from_box(boxed).map_err(|error| {
        SignError::new(
            SignRefusal::VerifyAfterSign,
            format!("could not parse public pin: {error}"),
        )
    })
}

#[cfg(not(feature = "test-fixture-pin"))]
fn resolve_pin() -> Result<PublicKey, SignError> {
    parse_pin(PRODUCT_PIN)
}

#[cfg(feature = "test-fixture-pin")]
fn resolve_pin() -> Result<PublicKey, SignError> {
    let text = match std::env::var(PIN_OVERRIDE_ENV) {
        Ok(path) if !path.is_empty() => fs::read_to_string(&path).map_err(|error| {
            SignError::new(
                SignRefusal::VerifyAfterSign,
                format!("could not read {PIN_OVERRIDE_ENV} {path}: {error}"),
            )
        })?,
        _ => PRODUCT_PIN.to_owned(),
    };
    parse_pin(&text)
}

#[cfg(test)]
mod pin_source {
    const SOURCE: &str = include_str!("sign.rs");
    const OVERRIDE: &str = concat!("SOLSTONE_JOURNAL_MINISIGN", "_PIN");
    const FEATURE_ON: &str = "#[cfg(feature = \"test-fixture-pin\")]";
    const FEATURE_OFF: &str = "#[cfg(not(feature = \"test-fixture-pin\"))]";

    #[test]
    fn pin_override_env_lives_only_in_the_fixture_pin_feature_sibling() {
        assert_eq!(SOURCE.matches(OVERRIDE).count(), 1);
        let mut off_body = String::new();
        let mut in_off = false;
        for line in SOURCE.lines() {
            let trimmed = line.trim();
            if trimmed == FEATURE_OFF
                || trimmed.starts_with("#[cfg(all(test, not(feature = \"test-fixture-pin\")))]")
            {
                in_off = true;
                continue;
            }
            if trimmed == FEATURE_ON {
                in_off = false;
                continue;
            }
            if in_off {
                off_body.push_str(line);
                off_body.push('\n');
            }
        }
        assert!(!off_body.contains(OVERRIDE));
    }
}

#[cfg(all(test, not(feature = "test-fixture-pin")))]
mod tests {
    use super::{PRODUCT_PIN, parse_pin, resolve_pin};

    #[test]
    fn default_resolve_pin_returns_the_compiled_product_pin() {
        let resolved = resolve_pin().expect("compiled pin parses");
        let expected = parse_pin(PRODUCT_PIN).expect("product pin parses");
        assert_eq!(resolved, expected);
    }
}
