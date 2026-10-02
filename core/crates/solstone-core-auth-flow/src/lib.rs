// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared OAuth PKCE, encoding, token, and browser-launch primitives.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use getrandom::fill as fill_random;
use sha2::{Digest, Sha256};

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Failure generating cryptographically secure random bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandomError;

impl fmt::Display for RandomError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("failed to generate random bytes")
    }
}

impl std::error::Error for RandomError {}

/// Failure decoding a percent-encoded string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError;

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid percent-encoded string")
    }
}

impl std::error::Error for DecodeError {}

/// Failure planning a browser invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    UnsupportedOs,
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedOs => {
                formatter.write_str("unsupported operating system for browser launch")
            }
        }
    }
}

impl std::error::Error for PlanError {}

/// Target operating system for browser process planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    Linux,
    Macos,
    Windows,
    Other,
}

/// Detect the target operating system compiled for this executable.
pub const fn current_target_os() -> TargetOs {
    #[cfg(target_os = "linux")]
    return TargetOs::Linux;
    #[cfg(target_os = "macos")]
    return TargetOs::Macos;
    #[cfg(target_os = "windows")]
    return TargetOs::Windows;
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return TargetOs::Other;
}

/// Stdio disposition carried by [`BrowserInvocation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserStdio {
    Null,
}

/// Planned browser process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserInvocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub stdin: BrowserStdio,
    pub stdout: BrowserStdio,
    pub stderr: BrowserStdio,
}

/// Encode bytes into an unpadded URL-safe Base64 string.
pub fn base64_url_no_pad(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(BASE64URL_ALPHABET[(first >> 2) as usize]));
        output.push(char::from(
            BASE64URL_ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize],
        ));
        if chunk.len() > 1 {
            output.push(char::from(
                BASE64URL_ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize],
            ));
        }
        if chunk.len() > 2 {
            output.push(char::from(BASE64URL_ALPHABET[(third & 0x3f) as usize]));
        }
    }
    output
}

/// Generate a cryptographically secure Base64URL-encoded random token.
pub fn random_token(bytes: usize) -> Result<String, RandomError> {
    let mut raw = vec![0_u8; bytes];
    fill_random(&mut raw).map_err(|_| RandomError)?;
    Ok(base64_url_no_pad(&raw))
}

/// Percent-encode a string value using uppercase hex digits.
pub fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Percent-decode a string value, converting `+` to space.
pub fn percent_decode(value: &str) -> Result<String, DecodeError> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                decoded.push((hex(bytes[index + 1])? << 4) | hex(bytes[index + 2])?);
                index += 3;
            }
            b'%' => return Err(DecodeError),
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| DecodeError)
}

fn hex(byte: u8) -> Result<u8, DecodeError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(DecodeError),
    }
}

/// Derive the PKCE S256 code challenge for a verifier string.
pub fn code_challenge_s256(verifier: &str) -> String {
    base64_url_no_pad(&Sha256::digest(verifier.as_bytes()))
}

/// Plan the platform-specific browser invocation for a URL.
pub fn plan_browser(os: TargetOs, url: &str) -> Result<BrowserInvocation, PlanError> {
    let (program, args) = match os {
        TargetOs::Linux => ("xdg-open", vec![url.to_owned()]),
        TargetOs::Macos => ("open", vec![url.to_owned()]),
        TargetOs::Windows => (
            "rundll32",
            vec!["url.dll,FileProtocolHandler".to_owned(), url.to_owned()],
        ),
        TargetOs::Other => return Err(PlanError::UnsupportedOs),
    };
    Ok(BrowserInvocation {
        program: PathBuf::from(program),
        args,
        cwd: None,
        env: BTreeMap::new(),
        stdin: BrowserStdio::Null,
        stdout: BrowserStdio::Null,
        stderr: BrowserStdio::Null,
    })
}

/// Spawn the planned browser process.
pub fn execute_browser_invocation(invocation: &BrowserInvocation) -> std::io::Result<()> {
    let mut command = Command::new(&invocation.program);
    command.args(&invocation.args);
    if let Some(cwd) = &invocation.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &invocation.env {
        command.env(key, value);
    }
    command
        .stdin(match invocation.stdin {
            BrowserStdio::Null => Stdio::null(),
        })
        .stdout(match invocation.stdout {
            BrowserStdio::Null => Stdio::null(),
        })
        .stderr(match invocation.stderr {
            BrowserStdio::Null => Stdio::null(),
        })
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_url_encoding_matches_vectors() {
        assert_eq!(base64_url_no_pad(b""), "");
        assert_eq!(base64_url_no_pad(b"f"), "Zg");
        assert_eq!(base64_url_no_pad(b"fo"), "Zm8");
        assert_eq!(base64_url_no_pad(b"foo"), "Zm9v");
        assert_eq!(
            code_challenge_s256("synthetic-verifier"),
            "SwisrY-odM5E0NhbqyXlh9EaF96vyb-VtU1Zb4xw37I"
        );
    }

    #[test]
    fn percent_encoding_and_decoding_round_trips() {
        let input = "hello world / & ? = +";
        let encoded = percent_encode(input);
        assert_eq!(encoded, "hello%20world%20%2F%20%26%20%3F%20%3D%20%2B");
        assert_eq!(percent_decode(&encoded).unwrap(), input);
        assert_eq!(percent_decode("hello+world").unwrap(), "hello world");
        assert!(percent_decode("invalid%GG").is_err());
        assert!(percent_decode("trailing%").is_err());
    }

    #[test]
    fn random_tokens_generate_requested_entropy() {
        let token = random_token(32).unwrap();
        assert!(!token.is_empty());
        assert_eq!(random_token(0).unwrap(), "");
    }

    #[test]
    fn plan_browser_handles_all_targets() {
        let url = "https://example.test/auth?a=1&b=2&c=3";
        let linux = plan_browser(TargetOs::Linux, url).unwrap();
        assert_eq!(linux.program, PathBuf::from("xdg-open"));
        assert_eq!(linux.args, vec![url]);

        let macos = plan_browser(TargetOs::Macos, url).unwrap();
        assert_eq!(macos.program, PathBuf::from("open"));
        assert_eq!(macos.args, vec![url]);

        let windows = plan_browser(TargetOs::Windows, url).unwrap();
        assert_eq!(windows.program, PathBuf::from("rundll32"));
        assert_eq!(
            windows.args,
            vec!["url.dll,FileProtocolHandler".to_owned(), url.to_owned()]
        );

        assert_eq!(
            plan_browser(TargetOs::Other, url),
            Err(PlanError::UnsupportedOs)
        );
    }
}
