// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows-only Callosum endpoint derivation and authentication support.

use std::io;
use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub(crate) const PIPE_SECRET_FILE: &str = "callosum.pipe-secret";
pub(crate) const PIPE_MAGIC: [u8; 4] = *b"SCNP";
pub(crate) const PIPE_VERSION: u8 = 1;
pub(crate) const PIPE_CHALLENGE_LEN: usize = 32;
pub(crate) const PIPE_HANDSHAKE_LEN: usize = 4 + 1 + PIPE_CHALLENGE_LEN;
const PIPE_NAME_PREFIX: &str = r"\\.\pipe\solstone-callosum-v1-";
const DERIVATION_DOMAIN: &[u8] = b"solstone-callosum-windows-pipe-name-v1\0";
const AUTH_DOMAIN: &[u8] = b"solstone-callosum-windows-client-proof-v1\0";

/// Keeps the Windows namespace input explicit and unit-testable without importing installation
/// identity. The fixed value narrows Round 19's cloned-image guarantee only: the canonical
/// socket path remains the primary same-machine differentiator for distinct journal roots.
pub(crate) trait WindowsPipeNamespace {
    fn namespace_bytes(&self) -> &'static [u8];
}

pub(crate) struct FixedWindowsPipeNamespace;

impl WindowsPipeNamespace for FixedWindowsPipeNamespace {
    fn namespace_bytes(&self) -> &'static [u8] {
        b"solstone-callosum-windows-placeholder-v1"
    }
}

pub(crate) fn secret_path(socket_path: &Path) -> io::Result<PathBuf> {
    let parent = socket_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Callosum socket path has no parent",
            )
        })?;
    Ok(parent.join(PIPE_SECRET_FILE))
}

pub(crate) fn pipe_name_from_utf16(
    canonical_path: &[u16],
    namespace: &impl WindowsPipeNamespace,
) -> io::Result<String> {
    if canonical_path.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Callosum socket path cannot be empty",
        ));
    }
    let mut input = Vec::with_capacity(
        DERIVATION_DOMAIN.len()
            + 16
            + canonical_path.len().saturating_mul(2)
            + namespace.namespace_bytes().len(),
    );
    input.extend_from_slice(DERIVATION_DOMAIN);
    input.extend_from_slice(&(canonical_path.len() as u64).to_le_bytes());
    for unit in canonical_path {
        input.extend_from_slice(&unit.to_le_bytes());
    }
    input.extend_from_slice(&(namespace.namespace_bytes().len() as u64).to_le_bytes());
    input.extend_from_slice(namespace.namespace_bytes());
    let digest = Sha256::digest(input);
    let pipe_name = format!("{PIPE_NAME_PREFIX}{digest:x}");
    if pipe_name.len() > 256 || !pipe_name.is_ascii() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "derived Callosum pipe name is invalid",
        ));
    }
    Ok(pipe_name)
}

pub(crate) fn server_greeting(challenge: [u8; PIPE_CHALLENGE_LEN]) -> [u8; PIPE_HANDSHAKE_LEN] {
    let mut greeting = [0_u8; PIPE_HANDSHAKE_LEN];
    greeting[..4].copy_from_slice(&PIPE_MAGIC);
    greeting[4] = PIPE_VERSION;
    greeting[5..].copy_from_slice(&challenge);
    greeting
}

pub(crate) fn client_proof(
    secret: &[u8; PIPE_CHALLENGE_LEN],
    greeting: &[u8; PIPE_HANDSHAKE_LEN],
) -> io::Result<[u8; PIPE_HANDSHAKE_LEN]> {
    if greeting[..4] != PIPE_MAGIC || greeting[4] != PIPE_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid Callosum handshake",
        ));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid Callosum secret"))?;
    mac.update(AUTH_DOMAIN);
    mac.update(greeting);
    let mut proof = [0_u8; PIPE_HANDSHAKE_LEN];
    proof[..4].copy_from_slice(&PIPE_MAGIC);
    proof[4] = PIPE_VERSION;
    proof[5..].copy_from_slice(&mac.finalize().into_bytes());
    Ok(proof)
}

pub(crate) fn verify_client_proof(
    secret: &[u8; PIPE_CHALLENGE_LEN],
    greeting: &[u8; PIPE_HANDSHAKE_LEN],
    proof: &[u8; PIPE_HANDSHAKE_LEN],
) -> bool {
    if proof[..4] != PIPE_MAGIC || proof[4] != PIPE_VERSION {
        return false;
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(AUTH_DOMAIN);
    mac.update(greeting);
    mac.verify_slice(&proof[5..]).is_ok()
}

#[cfg(windows)]
pub(crate) fn pipe_name(socket_path: &Path) -> io::Result<String> {
    use std::os::windows::ffi::OsStrExt;

    let parent = socket_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Callosum socket path has no parent",
            )
        })?;
    let filename = socket_path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Callosum socket path has no filename",
            )
        })?;
    let canonical_parent = std::fs::canonicalize(parent)?;
    let canonical_path = canonical_parent.join(filename);
    let units: Vec<u16> = canonical_path.as_os_str().encode_wide().collect();
    pipe_name_from_utf16(&units, &FixedWindowsPipeNamespace)
}

#[cfg(windows)]
pub(crate) fn read_secret(socket_path: &Path) -> io::Result<[u8; PIPE_CHALLENGE_LEN]> {
    let bytes = std::fs::read(secret_path(socket_path)?)?;
    bytes.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Callosum pipe secret has invalid length",
        )
    })
}

#[cfg(windows)]
pub(crate) fn create_or_read_secret(socket_path: &Path) -> io::Result<[u8; PIPE_CHALLENGE_LEN]> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let path = secret_path(socket_path)?;
    match OpenOptions::new().create_new(true).write(true).open(&path) {
        Ok(mut file) => {
            let mut secret = [0_u8; PIPE_CHALLENGE_LEN];
            getrandom::fill(&mut secret)
                .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))?;
            // Intentional: the ordinary inherited file descriptor protects this secret. Round 21's
            // custom DACL carve-out applies to the named-pipe endpoint only, never this file.
            file.write_all(&secret)?;
            file.sync_all()?;
            Ok(secret)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => read_secret(socket_path),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
pub(crate) mod sid {
    #![allow(unsafe_code)]

    use std::io;
    use std::mem::size_of;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::NonNull;

    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct LocalWideString(NonNull<u16>);

    impl LocalWideString {
        fn as_ptr(&self) -> *const u16 {
            self.0.as_ptr()
        }
    }

    impl Drop for LocalWideString {
        fn drop(&mut self) {
            // SAFETY: ConvertSidToStringSidW allocated this pointer with LocalAlloc, whose
            // documented matching release is LocalFree exactly once.
            unsafe {
                LocalFree(self.0.as_ptr().cast());
            }
        }
    }

    pub(crate) fn current_user_sid() -> io::Result<String> {
        let mut token = std::ptr::null_mut();
        // SAFETY: GetCurrentProcess is a valid pseudo-handle and token points to writable storage.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0
            || token.is_null()
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OpenProcessToken returned a non-null owned handle, transferred to OwnedHandle.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut required = 0_u32;
        // SAFETY: this is the documented sizing call with a valid token and null output buffer.
        let first = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut required,
            )
        };
        // SAFETY: retrieves the error produced by the preceding sizing call.
        if first != 0
            || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
            || required < size_of::<TOKEN_USER>() as u32
        {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = vec![0_u8; required as usize];
        // SAFETY: bytes has the exact capacity reported by the successful sizing call.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                bytes.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation initialized TOKEN_USER at the start of the returned buffer.
        let user = unsafe { bytes.as_ptr().cast::<TOKEN_USER>().read_unaligned() };
        if user.User.Sid.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "TokenUser returned a null SID",
            ));
        }
        let mut text = std::ptr::null_mut();
        // SAFETY: the SID points into the still-live token-information buffer and text is writable.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 || text.is_null() {
            return Err(io::Error::last_os_error());
        }
        let text = LocalWideString(NonNull::new(text).expect("null checked above"));
        // SAFETY: ConvertSidToStringSidW returned a NUL-terminated UTF-16 LocalAlloc buffer.
        let length = unsafe {
            (0..)
                .take_while(|index| *text.as_ptr().add(*index) != 0)
                .count()
        };
        // SAFETY: length was bounded by the allocation's NUL terminator, and LocalWideString keeps
        // that allocation alive for this conversion and releases it on every return path.
        unsafe { String::from_utf16(std::slice::from_raw_parts(text.as_ptr(), length)) }
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Windows SID is not UTF-16"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FixedWindowsPipeNamespace, PIPE_CHALLENGE_LEN, PIPE_VERSION, WindowsPipeNamespace,
        client_proof, pipe_name_from_utf16, secret_path, server_greeting, verify_client_proof,
    };
    use std::path::Path;

    #[test]
    fn pipe_name_derivation_has_a_stable_golden_value() {
        let name = pipe_name_from_utf16(
            &[
                67, 58, 92, 106, 92, 104, 92, 99, 97, 108, 108, 111, 115, 117, 109, 46, 115, 111,
                99, 107,
            ],
            &FixedWindowsPipeNamespace,
        )
        .unwrap();
        assert_eq!(
            name,
            r"\\.\pipe\solstone-callosum-v1-a4a4990315f382ac70e7c49249ffbf6ba523e2e952417e48ad328c652e117858"
        );
        assert!(name.is_ascii());
    }

    #[test]
    fn pipe_name_derivation_rejects_an_empty_canonical_path() {
        assert!(pipe_name_from_utf16(&[], &FixedWindowsPipeNamespace).is_err());
    }

    #[test]
    fn fixed_windows_namespace_seam_is_explicit_and_stable() {
        assert_eq!(
            FixedWindowsPipeNamespace.namespace_bytes(),
            b"solstone-callosum-windows-placeholder-v1"
        );
    }

    #[test]
    fn handshake_hmac_uses_the_versioned_domain_separated_transcript() {
        let secret = [7_u8; PIPE_CHALLENGE_LEN];
        let greeting = server_greeting([9_u8; PIPE_CHALLENGE_LEN]);
        let proof = client_proof(&secret, &greeting).unwrap();
        assert_eq!(
            &proof[5..],
            [
                0x81, 0x4d, 0xaf, 0x32, 0x2c, 0x33, 0xfd, 0x40, 0xfe, 0x28, 0x23, 0xa7, 0x56, 0x9a,
                0x57, 0x49, 0x37, 0x2c, 0xf0, 0x13, 0x97, 0x60, 0x32, 0xab, 0x89, 0xba, 0x05, 0xae,
                0x33, 0x61, 0xef, 0x26,
            ]
        );
        assert!(verify_client_proof(&secret, &greeting, &proof));
        let mut wrong = proof;
        wrong[5] ^= 1;
        assert!(!verify_client_proof(&secret, &greeting, &wrong));
        let mut wrong_version = greeting;
        wrong_version[4] = PIPE_VERSION + 1;
        assert!(client_proof(&secret, &wrong_version).is_err());
    }

    #[test]
    fn secret_path_is_a_health_sibling() {
        assert_eq!(
            secret_path(Path::new("journal/health/callosum.sock")).unwrap(),
            Path::new("journal/health/callosum.pipe-secret")
        );
    }
}
