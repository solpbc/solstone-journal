// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Streaming SHA-256 through Windows CNG. Every hashing error refuses admission.

use std::io;
use std::ptr;

use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider,
    BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptHashData,
    BCryptOpenAlgorithmProvider, MS_PRIMITIVE_PROVIDER,
};

struct Algorithm(BCRYPT_ALG_HANDLE);

impl Drop for Algorithm {
    fn drop(&mut self) {
        // SAFETY: this handle came from a successful open and outlives every hash.
        unsafe { BCryptCloseAlgorithmProvider(self.0, 0) };
    }
}

pub(super) struct Sha256 {
    handle: BCRYPT_HASH_HANDLE,
    // The provider drops after Sha256::drop destroys its dependent hash object.
    _algorithm: Algorithm,
}

impl Sha256 {
    pub(super) fn new() -> io::Result<Self> {
        let mut algorithm = ptr::null_mut();
        // SAFETY: constants are terminated UTF-16 strings; output is writable.
        check("BCryptOpenAlgorithmProvider", unsafe {
            BCryptOpenAlgorithmProvider(
                &mut algorithm,
                BCRYPT_SHA256_ALGORITHM,
                MS_PRIMITIVE_PROVIDER,
                0,
            )
        })?;
        let algorithm = Algorithm(algorithm);
        let mut handle = ptr::null_mut();
        // SAFETY: the provider is live. CNG owns the null/zero hash-object buffer
        // until BCryptDestroyHash; null/zero secret selects an ordinary SHA hash.
        check("BCryptCreateHash", unsafe {
            BCryptCreateHash(
                algorithm.0,
                &mut handle,
                ptr::null_mut(),
                0,
                ptr::null(),
                0,
                0,
            )
        })?;
        Ok(Self {
            handle,
            _algorithm: algorithm,
        })
    }

    pub(super) fn update(&mut self, bytes: &[u8]) -> io::Result<()> {
        let length = u32::try_from(bytes.len())
            .map_err(|_| io::Error::other("CNG hash input exceeds a 32-bit buffer length"))?;
        // SAFETY: this exclusively borrowed hash is live and the input spans length.
        check("BCryptHashData", unsafe {
            BCryptHashData(self.handle, bytes.as_ptr(), length, 0)
        })
    }

    pub(super) fn finish(self) -> io::Result<[u8; 32]> {
        let mut digest = [0; 32];
        // SAFETY: the live SHA-256 hash writes exactly 32 bytes to this output.
        check("BCryptFinishHash", unsafe {
            BCryptFinishHash(self.handle, digest.as_mut_ptr(), digest.len() as u32, 0)
        })?;
        Ok(digest)
    }
}

impl Drop for Sha256 {
    fn drop(&mut self) {
        // SAFETY: this handle came from successful creation, has one owner, and
        // its provider is still live. Destruction also frees CNG's object buffer.
        unsafe { BCryptDestroyHash(self.handle) };
    }
}

fn check(operation: &str, status: i32) -> io::Result<()> {
    if status < 0 {
        Err(io::Error::other(format!(
            "{operation} failed with NTSTATUS 0x{:08x}",
            status as u32
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Sha256;

    fn digest(chunks: &[&[u8]]) -> String {
        let mut hash = Sha256::new().unwrap();
        for chunk in chunks {
            hash.update(chunk).unwrap();
        }
        hash.finish()
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn independent_sha256_vectors_and_streaming() {
        assert_eq!(
            digest(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest(&[b"a", b"", b"bc"]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let block = [b'a'; 1000];
        let chunks = vec![block.as_slice(); 1000];
        assert_eq!(
            digest(&chunks),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn failed_native_status_is_an_error() {
        assert!(super::check("hash", 0).is_ok());
        let error = super::check("hash", 0xc0000001_u32 as i32).unwrap_err();
        assert!(error.to_string().contains("0xc0000001"));
    }
}
