// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::fs;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};

use minisign::{PublicKey, SecretKey, SecretKeyBox};

use crate::manifest_verify::{
    ManifestVerifyError, SignatureSource, capture_signature, discover_manifest,
    validate_release_set, verify_manifest_signature,
};

const KEY_ENV: &str = "SOLSTONE_JOURNAL_MINISIGN_KEY";
const UNTRUSTED_COMMENT: &str = "signature from solstone-journal release key";
const MINISIG_SUFFIX: &str = ".minisig";
const PARTIAL_SUFFIX: &str = ".partial";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignRefusal {
    MissingKeyEnv,
    EmptyKeyPath,
    RelativeKeyPath,
    UnreadableKey,
    MissingPassphrase,
    WrongPassphrase,
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
            Self::VerifyAfterSign => "verify-after-sign",
        }
    }
}

#[derive(Debug)]
pub enum SignError {
    Refusal { kind: SignRefusal, detail: String },
    Manifest(ManifestVerifyError),
}

impl SignError {
    fn new(kind: SignRefusal, detail: impl Into<String>) -> Self {
        Self::Refusal {
            kind,
            detail: detail.into(),
        }
    }
}

impl From<ManifestVerifyError> for SignError {
    fn from(error: ManifestVerifyError) -> Self {
        Self::Manifest(error)
    }
}

impl fmt::Display for SignError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refusal { kind, detail } => write!(formatter, "{}\n  {detail}", kind.as_str()),
            Self::Manifest(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SignError {}

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
    let manifest = discover_manifest(dir)?;
    let existing_signature = capture_signature(dir, &manifest, false)?;
    validate_release_set(dir, &manifest, existing_signature.as_ref())?;

    let key_path = load_key_path()?;
    let passphrase = load_passphrase(&key_path)?;
    let secret = load_secret_key(&key_path, passphrase)?;

    let dest = dir.join(format!("{}{}", manifest.name, MINISIG_SUFFIX));
    let temp = dir.join(format!(
        "{}{}{}",
        manifest.name, MINISIG_SUFFIX, PARTIAL_SUFFIX
    ));
    let mut guard = TempSig {
        path: temp,
        persist: false,
    };

    let public = PublicKey::from_secret_key(&secret).ok();
    let signature = minisign::sign(
        public.as_ref(),
        &secret,
        Cursor::new(manifest.bytes.as_slice()),
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

    let bytes = fs::read(&guard.path).map_err(|error| {
        SignError::new(
            SignRefusal::VerifyAfterSign,
            format!("could not re-read {}: {error}", guard.path.display()),
        )
    })?;
    let signature = SignatureSource::captured(
        guard.path.clone(),
        guard
            .path
            .file_name()
            .expect("temporary signature has a filename")
            .to_string_lossy()
            .into_owned(),
        bytes,
    );
    verify_manifest_signature(&manifest, &signature).map_err(|error| {
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
    key_path.with_extension("pass")
}

fn decode_passphrase(bytes: Vec<u8>) -> Result<String, SignError> {
    let text = String::from_utf8(bytes).map_err(|_| {
        SignError::new(
            SignRefusal::MissingPassphrase,
            "passphrase is not valid UTF-8".to_owned(),
        )
    })?;
    Ok(text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text.as_str())
        .to_owned())
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
            SignError::new(SignRefusal::WrongPassphrase, key_path.display().to_string())
        } else {
            SignError::new(
                SignRefusal::UnreadableKey,
                format!("{}: {error}", key_path.display()),
            )
        }
    })
}
