// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-time RSA generation and the retained signing/public-key views.

use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use ring::signature::RsaKeyPair;
use rsa::RsaPrivateKey;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};

use crate::errors::PortalClientError;
use crate::jwk::{Jwk, thumbprint};

pub(crate) struct Keypair {
    pub(crate) signer: RsaKeyPair,
    pub(crate) jwk: Jwk,
    pub(crate) thumbprint: String,
}

impl Keypair {
    pub(crate) fn from_pem(pem: &[u8]) -> Result<Self, PortalClientError> {
        let text = std::str::from_utf8(pem).map_err(|error| PortalClientError::KeypairInvalid {
            message: error.to_string(),
        })?;
        let private = RsaPrivateKey::from_pkcs8_pem(text).map_err(|error| {
            PortalClientError::KeypairInvalid {
                message: error.to_string(),
            }
        })?;
        let der = private
            .to_pkcs8_der()
            .map_err(|error| PortalClientError::KeypairInvalid {
                message: error.to_string(),
            })?;
        let signer = RsaKeyPair::from_pkcs8(der.as_bytes()).map_err(|error| {
            PortalClientError::KeypairInvalid {
                message: error.to_string(),
            }
        })?;
        let jwk = Jwk::public_key(&private.to_public_key());
        let thumbprint = thumbprint(&jwk);
        Ok(Self {
            signer,
            jwk,
            thumbprint,
        })
    }

    pub(crate) fn generate() -> Result<(Self, Vec<u8>), PortalClientError> {
        let private = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 4096).map_err(|error| {
            PortalClientError::KeypairInvalid {
                message: error.to_string(),
            }
        })?;
        let pem = private
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|error| PortalClientError::KeypairInvalid {
                message: error.to_string(),
            })?
            .to_string()
            .into_bytes();
        let keypair = Self::from_pem(&pem)?;
        Ok((keypair, pem))
    }
}

pub(crate) fn save_keypair(path: &Path, pem: &[u8]) -> Result<(), PortalClientError> {
    // The reference writes, then chmods. Creating at 0600 plus chmod after write has
    // the same observable result for fresh and pre-existing files without a window.
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    use std::io::Write;
    options
        .open(path)
        .and_then(|mut file| file.write_all(pem))
        .map_err(|error| PortalClientError::Storage {
            message: error.to_string(),
        })?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        PortalClientError::Storage {
            message: error.to_string(),
        }
    })?;
    Ok(())
}
