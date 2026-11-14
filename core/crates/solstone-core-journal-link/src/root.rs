// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The journal's one root secret, and the sub-keys taken from it.

use std::path::Path;

use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};
use solstone_core_journal_io::{AtomicWriteOptions, write_bytes_exclusive};

/// The journal's one root secret for what it mints and must still recognise
/// after a restore or a move: 32 random bytes as hex, readable only by the
/// owner, made the first time something is minted.
///
/// 🔑 It is under `link/`, which a backup, a restore and an export all carry.
/// The endpoint's own keys under `mcp-endpoint/` are left out of a backup, so
/// a secret there would change every name and close every link on a restore.
/// It never rotates.
///
/// ⛔ The root is never used as a key itself. Each use takes its own sub-key
/// by HKDF-SHA256 under its own label, so no two uses share key bytes.
pub const ROOT_KEY_PATH: &str = "link/root.key";
const ROOT_KEY_BYTES: usize = 32;

/// Why a sub-key could not be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootError {
    /// The journal has no root secret, and this caller may not make one.
    Absent,
    /// The file is there and can't be read, or doesn't hold a key.
    ///
    /// ⛔ Never answered by writing a new root: that would change every name
    /// and close every link the journal ever gave.
    Unusable,
}

/// Whether a caller may make the root when the journal has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenAbsent {
    /// Minting: the first thing minted makes the root.
    Create,
    /// Reading back what was minted: no root means nothing was minted, and
    /// the read writes nothing.
    Refuse,
}

/// Fill `out` with the sub-key for `label`: HKDF-SHA256 of the root, no salt,
/// the label as info.
///
/// ⛔ One label per use and per primitive. A label is a contract: changing it
/// changes every value made under it.
pub fn sub_key(
    journal_root: &Path,
    label: &[u8],
    when_absent: WhenAbsent,
    out: &mut [u8],
) -> Result<(), RootError> {
    let root = root_key(journal_root, when_absent)?;
    hkdf::Salt::new(hkdf::HKDF_SHA256, &[])
        .extract(&root)
        .expand(&[label], SubKeyLen(out.len()))
        .and_then(|okm| okm.fill(out))
        .map_err(|_| RootError::Unusable)
}

struct SubKeyLen(usize);

impl hkdf::KeyType for SubKeyLen {
    fn len(&self) -> usize {
        self.0
    }
}

fn root_key(
    journal_root: &Path,
    when_absent: WhenAbsent,
) -> Result<[u8; ROOT_KEY_BYTES], RootError> {
    let path = journal_root.join(ROOT_KEY_PATH);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_root_key(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if when_absent == WhenAbsent::Refuse {
                return Err(RootError::Absent);
            }
            let mut key = [0_u8; ROOT_KEY_BYTES];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| RootError::Unusable)?;
            let mut text = String::with_capacity(ROOT_KEY_BYTES * 2 + 1);
            for byte in key {
                text.push_str(&format!("{byte:02x}"));
            }
            text.push('\n');
            // Another process may make it first: whichever is published is
            // the journal's, so read back what is there.
            let _ = write_bytes_exclusive(
                &path,
                text.as_bytes(),
                AtomicWriteOptions { mode: Some(0o600) },
            );
            parse_root_key(&std::fs::read_to_string(&path).map_err(|_| RootError::Unusable)?)
        }
        Err(_) => Err(RootError::Unusable),
    }
}

fn parse_root_key(text: &str) -> Result<[u8; ROOT_KEY_BYTES], RootError> {
    let text = text.trim();
    if text.len() != ROOT_KEY_BYTES * 2 || !text.is_ascii() {
        return Err(RootError::Unusable);
    }
    let mut key = [0_u8; ROOT_KEY_BYTES];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| RootError::Unusable)?;
    }
    Ok(key)
}
