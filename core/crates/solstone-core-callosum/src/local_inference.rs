// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)]

use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::num::NonZeroU16;
use std::time::Instant;

use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const LOCAL_INFERENCE_TOKEN_MAX: usize = 256;
pub(crate) const PRIVATE_FRAME_CAP: usize = 512;
pub(crate) const PRIVATE_MAGIC: [u8; 4] = *b"SCLP";
pub(crate) const PRIVATE_VERSION: u8 = 1;
pub(crate) const PRIVATE_HEADER_LEN: usize = 10;
pub(crate) const PRIVATE_REQUEST_BODY_LEN: usize = 40;
pub(crate) const PRIVATE_OP_READ_SNAPSHOT: u8 = 1;
pub(crate) const PRIVATE_KIND_CREDENTIAL: u8 = 0x81;
pub(crate) const PRIVATE_KIND_UNAVAILABLE: u8 = 0x82;
pub(crate) const NONCE_LEN: usize = 32;
pub(crate) const MAC_LEN: usize = 32;

const MAC_DOMAIN: &[u8] = b"solstone-callosum-windows-local-inference-reply-v1\0";

/// In-memory opaque credential token for Windows local-inference endpoints.
#[derive(Clone, Eq, PartialEq)]
pub struct LocalInferenceToken(Vec<u8>);

impl LocalInferenceToken {
    /// Return the raw token bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for LocalInferenceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "LocalInferenceToken(len={})", self.0.len())
    }
}

impl fmt::Display for LocalInferenceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "LocalInferenceToken(len={})", self.0.len())
    }
}

/// A verified, active local-inference service endpoint snapshot.
#[derive(Clone, Eq, PartialEq)]
pub struct LocalInferenceSnapshot {
    generation: u64,
    port: NonZeroU16,
    token: LocalInferenceToken,
}

impl LocalInferenceSnapshot {
    /// Construct a verified snapshot from generation, port, and token bytes.
    ///
    /// Refuses port 0, empty tokens, and tokens exceeding [`LOCAL_INFERENCE_TOKEN_MAX`].
    #[must_use]
    pub fn try_new(generation: u64, port: u16, token: &[u8]) -> Option<Self> {
        let port = NonZeroU16::new(port)?;
        if token.is_empty() || token.len() > LOCAL_INFERENCE_TOKEN_MAX {
            return None;
        }
        Some(Self {
            generation,
            port,
            token: LocalInferenceToken(token.to_vec()),
        })
    }

    /// The lifecycle generation of the active inference service.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The TCP loopback port of the active inference service.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port.get()
    }

    /// The raw credential token bytes.
    #[must_use]
    pub fn token(&self) -> &[u8] {
        self.token.as_bytes()
    }
}

impl fmt::Debug for LocalInferenceSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalInferenceSnapshot")
            .field("generation", &self.generation)
            .field("port", &self.port.get())
            .field("token", &self.token)
            .finish()
    }
}

/// The response offer returned by the server's snapshot provider closure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalInferenceSnapshotOffer {
    Unavailable,
    Ready(LocalInferenceSnapshot),
}

/// Failures encountered when requesting a local-inference snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalInferenceReadError {
    /// Local inference service is not currently running or available.
    Unavailable,
    /// Local inference credential handoff is unsupported on this platform or transport.
    Unsupported,
    /// Received a malformed or invalid response frame.
    Malformed,
    /// Cryptographic verification or admission handshake failed.
    Authentication,
    /// Transport I/O failure or premature socket closure.
    Transport,
    /// The request or handshake did not complete before the deadline.
    Timeout,
    /// A synchronous call was attempted from inside an active async runtime.
    RuntimeEntered,
}

impl fmt::Display for LocalInferenceReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("local inference service is unavailable"),
            Self::Unsupported => {
                formatter.write_str("local inference handoff is unsupported on this platform")
            }
            Self::Malformed => formatter.write_str("malformed local inference response"),
            Self::Authentication => {
                formatter.write_str("local inference authentication or verification failed")
            }
            Self::Transport => formatter.write_str("local inference transport error"),
            Self::Timeout => formatter.write_str("local inference request timed out"),
            Self::RuntimeEntered => formatter
                .write_str("synchronous local inference request called within async runtime"),
        }
    }
}

impl Error for LocalInferenceReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Decoded private request payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalInferencePrivateRequest {
    pub correlation: u64,
    pub nonce: [u8; NONCE_LEN],
}

/// The state classification of accumulating bytes in a frame buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameClass {
    Undecided,
    Private,
    Ordinary,
    PrivateRejected,
}

/// Accumulator holding partial frame bytes and its classification state.
#[derive(Clone)]
pub(crate) struct FrameAccum {
    bytes: Vec<u8>,
    class: FrameClass,
    skipped_bytes: usize,
}

impl FrameAccum {
    pub(crate) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            class: FrameClass::Undecided,
            skipped_bytes: 0,
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn bytes_mut(&mut self) -> &mut Vec<u8> {
        &mut self.bytes
    }

    pub(crate) fn class(&self) -> FrameClass {
        self.class
    }

    pub(crate) fn set_class(&mut self, class: FrameClass) {
        self.class = class;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn total_len(&self) -> usize {
        self.bytes.len() + self.skipped_bytes
    }

    pub(crate) fn add_skipped(&mut self, count: usize) {
        self.skipped_bytes += count;
    }

    pub(crate) fn clear(&mut self) {
        self.bytes.clear();
        self.class = FrameClass::Undecided;
        self.skipped_bytes = 0;
    }

    pub(crate) fn push_byte(&mut self, byte: u8) {
        match self.class {
            FrameClass::Undecided => {
                self.bytes.push(byte);
                self.update_undecided_class();
            }
            FrameClass::Private => {
                if self.bytes.len() < PRIVATE_FRAME_CAP {
                    self.bytes.push(byte);
                } else {
                    self.class = FrameClass::PrivateRejected;
                }
            }
            FrameClass::Ordinary => {
                self.bytes.push(byte);
            }
            FrameClass::PrivateRejected => {
                // In rejected mode, do not store beyond cap.
                if self.bytes.len() < PRIVATE_FRAME_CAP {
                    self.bytes.push(byte);
                }
            }
        }
    }

    pub(crate) fn extend_from_slice(&mut self, slice: &[u8]) {
        for &byte in slice {
            self.push_byte(byte);
        }
    }

    fn update_undecided_class(&mut self) {
        let len = self.bytes.len();
        if len <= PRIVATE_MAGIC.len() {
            if self.bytes[..len] == PRIVATE_MAGIC[..len] {
                self.class = FrameClass::Private;
            } else {
                self.class = FrameClass::Ordinary;
            }
        } else if self.bytes.starts_with(&PRIVATE_MAGIC) {
            self.class = FrameClass::Private;
        } else {
            self.class = FrameClass::Ordinary;
        }
    }

    pub(crate) fn check_ambiguous(&mut self) -> bool {
        if !self.bytes.starts_with(&PRIVATE_MAGIC) {
            return false;
        }
        // If full buffer or suffix after magic parses as JSON with tract & event, mark PrivateRejected.
        if is_json_envelope(&self.bytes) || is_json_envelope(&self.bytes[PRIVATE_MAGIC.len()..]) {
            self.class = FrameClass::PrivateRejected;
            return true;
        }
        false
    }
}

fn is_json_envelope(slice: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(slice) else {
        return false;
    };
    let trimmed = text.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return false;
    };
    let Some(obj) = value.as_object() else {
        return false;
    };
    obj.contains_key("tract") && obj.contains_key("event")
}

/// Compute reply HMAC-SHA256 over length-prefixed transcript fields.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub(crate) fn compute_reply_mac(
    secret: &[u8; MAC_LEN],
    version: u8,
    kind: u8,
    correlation: u64,
    nonce: &[u8; NONCE_LEN],
    generation: u64,
    port: u16,
    token: &[u8],
) -> [u8; MAC_LEN] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC-Sha256 accepts 32-byte key");
    mac.update(MAC_DOMAIN);

    // magic
    mac.update(&(PRIVATE_MAGIC.len() as u64).to_le_bytes());
    mac.update(&PRIVATE_MAGIC);

    // version
    mac.update(&1_u64.to_le_bytes());
    mac.update(&[version]);

    // kind
    mac.update(&1_u64.to_le_bytes());
    mac.update(&[kind]);

    // correlation
    mac.update(&8_u64.to_le_bytes());
    mac.update(&correlation.to_le_bytes());

    // nonce
    mac.update(&(nonce.len() as u64).to_le_bytes());
    mac.update(nonce);

    // generation
    mac.update(&8_u64.to_le_bytes());
    mac.update(&generation.to_le_bytes());

    // port
    mac.update(&2_u64.to_le_bytes());
    mac.update(&port.to_le_bytes());

    // token
    mac.update(&(token.len() as u64).to_le_bytes());
    mac.update(token);

    mac.finalize().into_bytes().into()
}

/// Verify reply HMAC-SHA256.
#[allow(dead_code, clippy::too_many_arguments)]
#[must_use]
pub(crate) fn verify_reply_mac(
    secret: &[u8; MAC_LEN],
    version: u8,
    kind: u8,
    correlation: u64,
    nonce: &[u8; NONCE_LEN],
    generation: u64,
    port: u16,
    token: &[u8],
    expected_mac: &[u8; MAC_LEN],
) -> bool {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(MAC_DOMAIN);

    mac.update(&(PRIVATE_MAGIC.len() as u64).to_le_bytes());
    mac.update(&PRIVATE_MAGIC);

    mac.update(&1_u64.to_le_bytes());
    mac.update(&[version]);

    mac.update(&1_u64.to_le_bytes());
    mac.update(&[kind]);

    mac.update(&8_u64.to_le_bytes());
    mac.update(&correlation.to_le_bytes());

    mac.update(&(nonce.len() as u64).to_le_bytes());
    mac.update(nonce);

    mac.update(&8_u64.to_le_bytes());
    mac.update(&generation.to_le_bytes());

    mac.update(&2_u64.to_le_bytes());
    mac.update(&port.to_le_bytes());

    mac.update(&(token.len() as u64).to_le_bytes());
    mac.update(token);

    mac.verify_slice(expected_mac).is_ok()
}

/// Encode a private read-snapshot request frame (exactly 50 bytes).
#[allow(dead_code)]
#[must_use]
pub(crate) fn encode_request_frame(correlation: u64, nonce: &[u8; NONCE_LEN]) -> [u8; 50] {
    let mut frame = [0_u8; 50];
    frame[..4].copy_from_slice(&PRIVATE_MAGIC);
    frame[4] = PRIVATE_VERSION;
    frame[5] = PRIVATE_OP_READ_SNAPSHOT;
    frame[6..10].copy_from_slice(&(PRIVATE_REQUEST_BODY_LEN as u32).to_le_bytes());
    frame[10..18].copy_from_slice(&correlation.to_le_bytes());
    frame[18..50].copy_from_slice(nonce);
    frame
}

/// Encode a private response frame.
#[must_use]
pub(crate) fn encode_response_frame(
    secret: &[u8; MAC_LEN],
    kind: u8,
    correlation: u64,
    nonce: &[u8; NONCE_LEN],
    generation: u64,
    port: u16,
    token: &[u8],
) -> Vec<u8> {
    let token_bytes = if kind == PRIVATE_KIND_CREDENTIAL {
        token
    } else {
        &[]
    };
    let (gen_val, port_val) = if kind == PRIVATE_KIND_CREDENTIAL {
        (generation, port)
    } else {
        (0, 0)
    };
    let mac = compute_reply_mac(
        secret,
        PRIVATE_VERSION,
        kind,
        correlation,
        nonce,
        gen_val,
        port_val,
        token_bytes,
    );
    let body_len = 8 + 32 + 8 + 2 + 4 + token_bytes.len() + 32;
    let mut frame = Vec::with_capacity(PRIVATE_HEADER_LEN + body_len);
    frame.extend_from_slice(&PRIVATE_MAGIC);
    frame.push(PRIVATE_VERSION);
    frame.push(kind);
    frame.extend_from_slice(&(body_len as u32).to_le_bytes());
    frame.extend_from_slice(&correlation.to_le_bytes());
    frame.extend_from_slice(nonce);
    frame.extend_from_slice(&gen_val.to_le_bytes());
    frame.extend_from_slice(&port_val.to_le_bytes());
    frame.extend_from_slice(&(token_bytes.len() as u32).to_le_bytes());
    frame.extend_from_slice(token_bytes);
    frame.extend_from_slice(&mac);
    frame
}

/// Decode a private request header and body.
pub(crate) fn decode_request_frame(buffer: &[u8]) -> Option<LocalInferencePrivateRequest> {
    if buffer.len() != PRIVATE_HEADER_LEN + PRIVATE_REQUEST_BODY_LEN {
        return None;
    }
    if buffer[..4] != PRIVATE_MAGIC
        || buffer[4] != PRIVATE_VERSION
        || buffer[5] != PRIVATE_OP_READ_SNAPSHOT
    {
        return None;
    }
    let body_len = u32::from_le_bytes(buffer[6..10].try_into().ok()?);
    if body_len as usize != PRIVATE_REQUEST_BODY_LEN {
        return None;
    }
    let correlation = u64::from_le_bytes(buffer[10..18].try_into().ok()?);
    let mut nonce = [0_u8; NONCE_LEN];
    nonce.copy_from_slice(&buffer[18..50]);
    Some(LocalInferencePrivateRequest { correlation, nonce })
}

/// Session managing a single private credential exchange.
#[allow(dead_code)]
pub(crate) struct ExchangeSession {
    secret: [u8; MAC_LEN],
    correlation: u64,
    nonce: [u8; NONCE_LEN],
    deadline: Instant,
    request_bytes: [u8; 50],
    written_bytes: usize,
    inbound: Vec<u8>,
    in_ordinary_line: bool,
}

#[allow(dead_code)]
impl ExchangeSession {
    pub(crate) fn new(
        secret: [u8; MAC_LEN],
        correlation: u64,
        nonce: [u8; NONCE_LEN],
        deadline: Instant,
    ) -> Self {
        let request_bytes = encode_request_frame(correlation, &nonce);
        Self {
            secret,
            correlation,
            nonce,
            deadline,
            request_bytes,
            written_bytes: 0,
            inbound: Vec::with_capacity(PRIVATE_FRAME_CAP),
            in_ordinary_line: false,
        }
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(crate) fn request_to_write(&self) -> &[u8] {
        if self.written_bytes < self.request_bytes.len() {
            &self.request_bytes[self.written_bytes..]
        } else {
            &[]
        }
    }

    pub(crate) fn mark_written(&mut self, count: usize) {
        self.written_bytes = (self.written_bytes + count).min(self.request_bytes.len());
    }

    pub(crate) fn is_request_written(&self) -> bool {
        self.written_bytes >= self.request_bytes.len()
    }

    pub(crate) fn append_inbound_byte(&mut self, byte: u8) -> Result<(), LocalInferenceReadError> {
        if self.in_ordinary_line {
            if byte == b'\n' {
                self.in_ordinary_line = false;
            }
            return Ok(());
        }

        if self.inbound.is_empty() {
            if byte == b'S' {
                self.inbound.push(byte);
            } else if byte != b'\n' {
                self.in_ordinary_line = true;
            }
            return Ok(());
        }

        if self.inbound.len() < PRIVATE_MAGIC.len() {
            self.inbound.push(byte);
            if self.inbound[..] == PRIVATE_MAGIC[..self.inbound.len()] {
                Ok(())
            } else {
                self.inbound.clear();
                if byte != b'\n' {
                    self.in_ordinary_line = true;
                }
                Ok(())
            }
        } else {
            if self.inbound.len() >= PRIVATE_FRAME_CAP {
                return Err(LocalInferenceReadError::Malformed);
            }
            self.inbound.push(byte);
            Ok(())
        }
    }

    pub(crate) fn append_inbound_slice(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), LocalInferenceReadError> {
        for &byte in bytes {
            self.append_inbound_byte(byte)?;
        }
        Ok(())
    }

    pub(crate) fn clear_non_matching_inbound_line(&mut self) {
        self.inbound.clear();
        self.in_ordinary_line = false;
    }

    /// Advance the exchange state, enforcing deadline and executing response verification.
    pub(crate) fn advance(
        &mut self,
        now: Instant,
    ) -> Result<Option<LocalInferenceSnapshot>, LocalInferenceReadError> {
        if now >= self.deadline {
            return Err(LocalInferenceReadError::Timeout);
        }
        if !self.is_request_written() {
            return Ok(None);
        }
        if self.inbound.len() < PRIVATE_HEADER_LEN {
            return Ok(None);
        }
        // Inspect magic: if divergent, discard completed newline line.
        if !self.inbound.starts_with(&PRIVATE_MAGIC) {
            if let Some(pos) = self.inbound.iter().position(|&b| b == b'\n') {
                self.inbound.drain(..=pos);
            }
            return Ok(None);
        }
        let body_len = u32::from_le_bytes(
            self.inbound[6..10]
                .try_into()
                .map_err(|_| LocalInferenceReadError::Malformed)?,
        ) as usize;
        if PRIVATE_HEADER_LEN + body_len > PRIVATE_FRAME_CAP {
            return Err(LocalInferenceReadError::Malformed);
        }
        let total_len = PRIVATE_HEADER_LEN + body_len;
        if self.inbound.len() < total_len {
            return Ok(None);
        }

        let raw_frame = &self.inbound[..total_len];
        let snapshot = Self::verify_and_decode_response(
            &self.secret,
            &self.nonce,
            self.correlation,
            raw_frame,
        )?;
        Ok(Some(snapshot))
    }

    /// Verify and decode a response frame.
    ///
    /// Policy order:
    /// 1. MAC (Authentication on failure)
    /// 2. nonce equality (Authentication)
    /// 3. version == 1 (Malformed)
    /// 4. kind is credential or unavailable (Malformed)
    /// 5. correlation equality (Malformed)
    /// 6. unavailable -> Err(Unavailable); credential -> try_new (port 0 or empty token -> Malformed)
    pub(crate) fn verify_and_decode_response(
        secret: &[u8; MAC_LEN],
        expected_nonce: &[u8; NONCE_LEN],
        expected_correlation: u64,
        frame: &[u8],
    ) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
        if frame.len() < PRIVATE_HEADER_LEN + 8 + 32 + 8 + 2 + 4 + 32 {
            return Err(LocalInferenceReadError::Malformed);
        }
        let version = frame[4];
        let kind = frame[5];
        let correlation = u64::from_le_bytes(frame[10..18].try_into().unwrap());
        let mut nonce = [0_u8; NONCE_LEN];
        nonce.copy_from_slice(&frame[18..50]);
        let generation = u64::from_le_bytes(frame[50..58].try_into().unwrap());
        let port = u16::from_le_bytes(frame[58..60].try_into().unwrap());
        let token_len = u32::from_le_bytes(frame[60..64].try_into().unwrap()) as usize;
        let token_end = 64 + token_len;
        if frame.len() != token_end + MAC_LEN {
            return Err(LocalInferenceReadError::Malformed);
        }
        let token_bytes = &frame[64..token_end];
        let mut received_mac = [0_u8; MAC_LEN];
        received_mac.copy_from_slice(&frame[token_end..token_end + MAC_LEN]);

        // 1. MAC
        if !verify_reply_mac(
            secret,
            version,
            kind,
            correlation,
            &nonce,
            generation,
            port,
            token_bytes,
            &received_mac,
        ) {
            return Err(LocalInferenceReadError::Authentication);
        }

        // 2. Nonce
        if &nonce != expected_nonce {
            return Err(LocalInferenceReadError::Authentication);
        }

        // 3. Version
        if version != PRIVATE_VERSION {
            return Err(LocalInferenceReadError::Malformed);
        }

        // 4. Kind
        if kind != PRIVATE_KIND_CREDENTIAL && kind != PRIVATE_KIND_UNAVAILABLE {
            return Err(LocalInferenceReadError::Malformed);
        }

        // 5. Correlation
        if correlation != expected_correlation {
            return Err(LocalInferenceReadError::Malformed);
        }

        // 6. Outcome
        if kind == PRIVATE_KIND_UNAVAILABLE {
            return Err(LocalInferenceReadError::Unavailable);
        }

        LocalInferenceSnapshot::try_new(generation, port, token_bytes)
            .ok_or(LocalInferenceReadError::Malformed)
    }
}

/// Drive the blocking exchange pump over generic Read/Write.
#[allow(dead_code)]
pub(crate) fn drive_exchange_blocking<R: Read, W: Write, Now: FnMut() -> Instant>(
    reader: &mut R,
    writer: &mut W,
    session: &mut ExchangeSession,
    mut now: Now,
) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
    let mut chunk = [0_u8; 128];
    loop {
        if let Some(snapshot) = session.advance(now())? {
            return Ok(snapshot);
        }

        if !session.is_request_written() {
            let to_write = session.request_to_write();
            match writer.write(to_write) {
                Ok(n) if n > 0 => {
                    session.mark_written(n);
                    if let Some(snapshot) = session.advance(now())? {
                        return Ok(snapshot);
                    }
                    continue;
                }
                Ok(_) => {}
                Err(err) if err.kind() == ErrorKind::WouldBlock => {}
                Err(err) if err.kind() == ErrorKind::TimedOut => {
                    return Err(LocalInferenceReadError::Timeout);
                }
                Err(_) => return Err(LocalInferenceReadError::Transport),
            }
            if now() >= session.deadline {
                return Err(LocalInferenceReadError::Timeout);
            }
        }

        match reader.read(&mut chunk) {
            Ok(0) => {
                if now() >= session.deadline {
                    return Err(LocalInferenceReadError::Timeout);
                }
                return Err(LocalInferenceReadError::Transport);
            }
            Ok(n) => {
                session.append_inbound_slice(&chunk[..n])?;
                if let Some(snapshot) = session.advance(now())? {
                    return Ok(snapshot);
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                if now() >= session.deadline {
                    return Err(LocalInferenceReadError::Timeout);
                }
            }
            Err(err) if err.kind() == ErrorKind::TimedOut => {
                return Err(LocalInferenceReadError::Timeout);
            }
            Err(_) => return Err(LocalInferenceReadError::Transport),
        }
    }
}

/// Perform Windows handshake over blocking I/O, then drive exchange.
#[cfg(any(test, windows))]
pub(crate) fn exchange_after_admission<R: Read, W: Write, Now: FnMut() -> Instant>(
    reader: &mut R,
    writer: &mut W,
    secret: &[u8; MAC_LEN],
    correlation: u64,
    nonce: [u8; NONCE_LEN],
    deadline: Instant,
    mut now: Now,
) -> Result<LocalInferenceSnapshot, LocalInferenceReadError> {
    let mut greeting = [0_u8; crate::windows::PIPE_HANDSHAKE_LEN];
    let mut greeting_read = 0;
    while greeting_read < greeting.len() {
        if now() >= deadline {
            return Err(LocalInferenceReadError::Timeout);
        }
        match reader.read(&mut greeting[greeting_read..]) {
            Ok(0) => return Err(LocalInferenceReadError::Authentication),
            Ok(n) => {
                greeting_read += n;
                if now() >= deadline {
                    return Err(LocalInferenceReadError::Timeout);
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {}
            Err(err) if err.kind() == ErrorKind::TimedOut => {
                return Err(LocalInferenceReadError::Timeout);
            }
            Err(_) => return Err(LocalInferenceReadError::Authentication),
        }
    }

    let proof = crate::windows::client_proof(secret, &greeting)
        .map_err(|_| LocalInferenceReadError::Authentication)?;
    let mut proof_written = 0;
    while proof_written < proof.len() {
        if now() >= deadline {
            return Err(LocalInferenceReadError::Timeout);
        }
        match writer.write(&proof[proof_written..]) {
            Ok(0) => {}
            Ok(n) => {
                proof_written += n;
                if now() >= deadline {
                    return Err(LocalInferenceReadError::Timeout);
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {}
            Err(err) if err.kind() == ErrorKind::TimedOut => {
                return Err(LocalInferenceReadError::Timeout);
            }
            Err(_) => return Err(LocalInferenceReadError::Authentication),
        }
    }

    let mut session = ExchangeSession::new(*secret, correlation, nonce, deadline);
    drive_exchange_blocking(reader, writer, &mut session, now)
}

/// Gating check for synchronous invocation.
pub(crate) fn gate_sync(
    runtime_entered: bool,
    platform_supported: bool,
    fill: impl FnOnce() -> Result<[u8; NONCE_LEN], ()>,
) -> Result<[u8; NONCE_LEN], LocalInferenceReadError> {
    if runtime_entered {
        return Err(LocalInferenceReadError::RuntimeEntered);
    }
    if !platform_supported {
        return Err(LocalInferenceReadError::Unsupported);
    }
    fill().map_err(|_| LocalInferenceReadError::Authentication)
}

/// Gating check for asynchronous invocation.
pub(crate) fn gate_async(
    platform_supported: bool,
    fill: impl FnOnce() -> Result<[u8; NONCE_LEN], ()>,
) -> Result<[u8; NONCE_LEN], LocalInferenceReadError> {
    if !platform_supported {
        return Err(LocalInferenceReadError::Unsupported);
    }
    fill().map_err(|_| LocalInferenceReadError::Authentication)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io;
    use std::time::Duration;

    #[test]
    fn try_new_refusals_and_valid_construction() {
        assert!(LocalInferenceSnapshot::try_new(1, 0, b"token").is_none());
        assert!(LocalInferenceSnapshot::try_new(1, 8080, b"").is_none());
        let oversized = [b'a'; 257];
        assert!(LocalInferenceSnapshot::try_new(1, 8080, &oversized).is_none());

        let valid = LocalInferenceSnapshot::try_new(0, 1, b"snapshot-token-9f3a\xff").unwrap();
        assert_eq!(valid.generation(), 0);
        assert_eq!(valid.port(), 1);
        assert_eq!(valid.token(), b"snapshot-token-9f3a\xff");
    }

    #[test]
    fn token_and_snapshot_debug_and_display_redact_token_bytes() {
        let raw_token = b"snapshot-token-9f3a\xff";
        let token = LocalInferenceToken(raw_token.to_vec());
        let token_debug = format!("{token:?}");
        let token_display = format!("{token}");
        assert_eq!(token_debug, "LocalInferenceToken(len=20)");
        assert_eq!(token_display, "LocalInferenceToken(len=20)");
        assert!(!token_debug.contains("snapshot-token"));
        assert!(!token_display.contains("snapshot-token"));

        let snapshot = LocalInferenceSnapshot::try_new(42, 8080, raw_token).unwrap();
        let snapshot_debug = format!("{snapshot:?}");
        assert!(snapshot_debug.contains("generation: 42"));
        assert!(snapshot_debug.contains("port: 8080"));
        assert!(snapshot_debug.contains("LocalInferenceToken(len=20)"));
        assert!(!snapshot_debug.contains("snapshot-token"));
    }

    #[test]
    fn error_display_debug_and_source_never_contain_token_bytes() {
        for err in [
            LocalInferenceReadError::Unavailable,
            LocalInferenceReadError::Unsupported,
            LocalInferenceReadError::Malformed,
            LocalInferenceReadError::Authentication,
            LocalInferenceReadError::Transport,
            LocalInferenceReadError::Timeout,
            LocalInferenceReadError::RuntimeEntered,
        ] {
            let debug_text = format!("{err:?}");
            let display_text = format!("{err}");
            assert!(!debug_text.contains("snapshot-token"));
            assert!(!display_text.contains("snapshot-token"));
            assert!(err.source().is_none());
        }
    }

    #[test]
    fn reply_mac_success_and_wrong_secret() {
        let secret = [7_u8; MAC_LEN];
        let wrong_secret = [8_u8; MAC_LEN];
        let nonce = [9_u8; NONCE_LEN];
        let mac = compute_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            100,
            &nonce,
            1,
            8080,
            b"tok",
        );
        assert!(verify_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            100,
            &nonce,
            1,
            8080,
            b"tok",
            &mac
        ));
        assert!(!verify_reply_mac(
            &wrong_secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            100,
            &nonce,
            1,
            8080,
            b"tok",
            &mac
        ));
    }

    #[test]
    fn reply_mac_rejects_mutated_transcript_fields() {
        let secret = [7_u8; MAC_LEN];
        let nonce = [9_u8; NONCE_LEN];
        let mac = compute_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            100,
            &nonce,
            1,
            8080,
            b"tok",
        );

        // Mutated kind
        assert!(!verify_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_UNAVAILABLE,
            100,
            &nonce,
            1,
            8080,
            b"tok",
            &mac
        ));
        // Mutated correlation
        assert!(!verify_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            101,
            &nonce,
            1,
            8080,
            b"tok",
            &mac
        ));
        // Mutated nonce
        let mut wrong_nonce = nonce;
        wrong_nonce[0] ^= 1;
        assert!(!verify_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            100,
            &wrong_nonce,
            1,
            8080,
            b"tok",
            &mac
        ));
        // Mutated generation
        assert!(!verify_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            100,
            &nonce,
            2,
            8080,
            b"tok",
            &mac
        ));
        // Mutated port
        assert!(!verify_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            100,
            &nonce,
            1,
            8081,
            b"tok",
            &mac
        ));
        // Mutated token
        assert!(!verify_reply_mac(
            &secret,
            PRIVATE_VERSION,
            PRIVATE_KIND_CREDENTIAL,
            100,
            &nonce,
            1,
            8080,
            b"tok2",
            &mac
        ));
    }

    #[test]
    fn policy_order_resigned_version_2_is_malformed_and_broken_mac_version_2_is_authentication() {
        let secret = [7_u8; MAC_LEN];
        let nonce = [9_u8; NONCE_LEN];
        let correlation = 100_u64;

        // Correctly MAC'd version 2 frame:
        let mut frame_v2 = encode_response_frame(
            &secret,
            PRIVATE_KIND_CREDENTIAL,
            correlation,
            &nonce,
            1,
            8080,
            b"token",
        );
        frame_v2[4] = 2; // change version in header to 2
        // Re-calculate MAC over version 2
        let mac_v2 = compute_reply_mac(
            &secret,
            2,
            PRIVATE_KIND_CREDENTIAL,
            correlation,
            &nonce,
            1,
            8080,
            b"token",
        );
        let len = frame_v2.len();
        frame_v2[len - MAC_LEN..].copy_from_slice(&mac_v2);

        // Correct MAC on version 2 -> Malformed
        let result =
            ExchangeSession::verify_and_decode_response(&secret, &nonce, correlation, &frame_v2);
        assert_eq!(result, Err(LocalInferenceReadError::Malformed));

        // Broken MAC on version 2 -> Authentication
        frame_v2[len - 1] ^= 1;
        let result =
            ExchangeSession::verify_and_decode_response(&secret, &nonce, correlation, &frame_v2);
        assert_eq!(result, Err(LocalInferenceReadError::Authentication));
    }

    #[test]
    fn advance_rejects_nonce_mismatch_and_replay() {
        let secret = [7_u8; MAC_LEN];
        let nonce_a = [1_u8; NONCE_LEN];
        let nonce_b = [2_u8; NONCE_LEN];
        let correlation = 42;

        let frame_b = encode_response_frame(
            &secret,
            PRIVATE_KIND_CREDENTIAL,
            correlation,
            &nonce_b,
            1,
            8080,
            b"tok",
        );

        // Verification expecting nonce_a against replay of frame_b
        let result =
            ExchangeSession::verify_and_decode_response(&secret, &nonce_a, correlation, &frame_b);
        assert_eq!(result, Err(LocalInferenceReadError::Authentication));
    }

    #[test]
    fn unavailable_body_has_no_token_bytes_and_returns_unavailable() {
        let secret = [7_u8; MAC_LEN];
        let nonce = [1_u8; NONCE_LEN];
        let correlation = 42;

        let frame = encode_response_frame(
            &secret,
            PRIVATE_KIND_UNAVAILABLE,
            correlation,
            &nonce,
            10,
            9000,
            b"ignored",
        );
        let result =
            ExchangeSession::verify_and_decode_response(&secret, &nonce, correlation, &frame);
        assert_eq!(result, Err(LocalInferenceReadError::Unavailable));
    }

    #[test]
    fn ambiguous_sclp_json_is_rejected_without_envelope_decode() {
        let mut accum = FrameAccum::new();
        accum.extend_from_slice(b"SCLP{\"tract\":\"observe\",\"event\":\"tick\"}\n");
        assert!(accum.check_ambiguous());
        assert_eq!(accum.class(), FrameClass::PrivateRejected);
    }

    #[test]
    fn oversized_body_len_never_stores_past_512() {
        let mut accum = FrameAccum::new();
        accum.extend_from_slice(b"SCLP");
        accum.push_byte(1); // version
        accum.push_byte(1); // op
        accum.extend_from_slice(&1000_u32.to_le_bytes()); // declared body 1000 > 512
        for _ in 0..1000 {
            accum.push_byte(b'x');
        }
        assert_eq!(accum.len(), PRIVATE_FRAME_CAP);
        assert_eq!(accum.class(), FrameClass::PrivateRejected);
    }

    struct ScriptedReader {
        reads: VecDeque<io::Result<Vec<u8>>>,
    }

    impl ScriptedReader {
        fn new(reads: Vec<io::Result<Vec<u8>>>) -> Self {
            Self {
                reads: reads.into(),
            }
        }
    }

    impl Read for ScriptedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(item) = self.reads.front_mut() else {
                return Ok(0);
            };
            match item {
                Ok(bytes) => {
                    let to_read = buf.len().min(bytes.len());
                    buf[..to_read].copy_from_slice(&bytes[..to_read]);
                    bytes.drain(..to_read);
                    if bytes.is_empty() {
                        self.reads.pop_front();
                    }
                    Ok(to_read)
                }
                Err(_) => {
                    let err = self.reads.pop_front().unwrap().unwrap_err();
                    Err(err)
                }
            }
        }
    }

    #[derive(Default)]
    struct ScriptedWriter {
        writes: Vec<u8>,
    }

    impl Write for ScriptedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn drive_exchange_blocking_scripted_success_and_unavailable() {
        let secret = [3_u8; MAC_LEN];
        let nonce = [4_u8; NONCE_LEN];
        let correlation = 10;
        let start = Instant::now();
        let deadline = start + Duration::from_secs(5);

        let response = encode_response_frame(
            &secret,
            PRIVATE_KIND_CREDENTIAL,
            correlation,
            &nonce,
            5,
            8080,
            b"test-token",
        );

        let mut reader = ScriptedReader::new(vec![
            Ok(response[..20].to_vec()),
            Ok(response[20..].to_vec()),
        ]);
        let mut writer = ScriptedWriter::default();
        let mut session = ExchangeSession::new(secret, correlation, nonce, deadline);
        let snapshot =
            drive_exchange_blocking(&mut reader, &mut writer, &mut session, || start).unwrap();
        assert_eq!(snapshot.generation(), 5);
        assert_eq!(snapshot.port(), 8080);
        assert_eq!(snapshot.token(), b"test-token");
        assert!(writer.writes.starts_with(&PRIVATE_MAGIC));

        // Test unavailable response
        let unavail_response = encode_response_frame(
            &secret,
            PRIVATE_KIND_UNAVAILABLE,
            correlation,
            &nonce,
            0,
            0,
            &[],
        );
        let mut reader_u = ScriptedReader::new(vec![Ok(unavail_response)]);
        let mut writer_u = ScriptedWriter::default();
        let mut session_u = ExchangeSession::new(secret, correlation, nonce, deadline);
        let err = drive_exchange_blocking(&mut reader_u, &mut writer_u, &mut session_u, || start)
            .unwrap_err();
        assert_eq!(err, LocalInferenceReadError::Unavailable);
    }

    #[test]
    fn drive_exchange_blocking_long_ordinary_line_discarded_before_private_reply() {
        let secret = [3_u8; MAC_LEN];
        let nonce = [4_u8; NONCE_LEN];
        let correlation = 10;
        let start = Instant::now();
        let deadline = start + Duration::from_secs(5);

        let response = encode_response_frame(
            &secret,
            PRIVATE_KIND_CREDENTIAL,
            correlation,
            &nonce,
            5,
            8080,
            b"test-token",
        );

        // Long ordinary line well over 512 bytes:
        let mut long_line = vec![b'{'; 600];
        long_line.extend_from_slice(b"\"tract\":\"observe\",\"event\":\"tick\"}\n");

        let mut reader = ScriptedReader::new(vec![Ok(long_line), Ok(response)]);
        let mut writer = ScriptedWriter::default();
        let mut session = ExchangeSession::new(secret, correlation, nonce, deadline);
        let snapshot =
            drive_exchange_blocking(&mut reader, &mut writer, &mut session, || start).unwrap();
        assert_eq!(snapshot.generation(), 5);
        assert_eq!(snapshot.port(), 8080);
        assert_eq!(snapshot.token(), b"test-token");
    }

    #[test]
    fn drive_exchange_blocking_expired_deadline_fails_without_write() {
        let secret = [3_u8; MAC_LEN];
        let nonce = [4_u8; NONCE_LEN];
        let start = Instant::now();
        let deadline = start - Duration::from_secs(1);

        let mut reader = ScriptedReader::new(vec![]);
        let mut writer = ScriptedWriter::default();
        let mut session = ExchangeSession::new(secret, 1, nonce, deadline);
        let err =
            drive_exchange_blocking(&mut reader, &mut writer, &mut session, || start).unwrap_err();
        assert_eq!(err, LocalInferenceReadError::Timeout);
        assert!(writer.writes.is_empty());
    }

    struct TimedReader<'a> {
        inner: ScriptedReader,
        clock: &'a std::cell::Cell<Instant>,
        step: Duration,
        calls: usize,
    }
    impl Read for TimedReader<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            let result = self.inner.read(bytes);
            self.clock.set(self.clock.get() + self.step);
            result
        }
    }
    struct PartialWriter<'a> {
        written: Vec<u8>,
        clock: &'a std::cell::Cell<Instant>,
    }
    impl Write for PartialWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.written.push(bytes[0]);
            self.clock.set(self.clock.get() + Duration::from_secs(1));
            Ok(1)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn continuing_partial_writes_cannot_outlive_deadline() {
        let start = Instant::now();
        let clock = std::cell::Cell::new(start);
        let mut writer = PartialWriter {
            written: Vec::new(),
            clock: &clock,
        };
        let mut reader = ScriptedReader::new(vec![]);
        let mut session = ExchangeSession::new([3; 32], 1, [4; 32], start + Duration::from_secs(2));
        let result =
            drive_exchange_blocking(&mut reader, &mut writer, &mut session, || clock.get());
        assert_eq!(result.unwrap_err(), LocalInferenceReadError::Timeout);
        assert_eq!(writer.written.len(), 2);
        assert!(!session.is_request_written());
    }

    #[test]
    fn partial_response_and_unrelated_frames_do_not_extend_deadline() {
        let response = encode_response_frame(
            &[3; 32],
            PRIVATE_KIND_CREDENTIAL,
            1,
            &[4; 32],
            1,
            8080,
            b"tok",
        );
        for first_read in [
            response[..10].to_vec(),
            b"{\"tract\":\"observe\",\"event\":\"tick\"}\n".to_vec(),
        ] {
            let start = Instant::now();
            let clock = std::cell::Cell::new(start);
            let mut reader = TimedReader {
                inner: ScriptedReader::new(vec![Ok(first_read), Ok(response.clone())]),
                clock: &clock,
                step: Duration::from_secs(2),
                calls: 0,
            };
            let mut writer = ScriptedWriter::default();
            let mut session =
                ExchangeSession::new([3; 32], 1, [4; 32], start + Duration::from_secs(2));
            let result =
                drive_exchange_blocking(&mut reader, &mut writer, &mut session, || clock.get());
            assert_eq!(result.unwrap_err(), LocalInferenceReadError::Timeout);
            assert!(session.is_request_written());
            assert_eq!(reader.calls, 1);
            assert_eq!(reader.inner.reads.len(), 1);
        }
    }

    #[test]
    fn drive_exchange_blocking_incomplete_disconnect_and_forged_mac() {
        let secret = [3_u8; MAC_LEN];
        let nonce = [4_u8; NONCE_LEN];
        let correlation = 1;
        let start = Instant::now();
        let deadline = start + Duration::from_secs(5);

        // Disconnect before complete reply
        let mut reader_disc = ScriptedReader::new(vec![Ok(b"SCLP\x01\x01".to_vec()), Ok(vec![])]);
        let mut writer_disc = ScriptedWriter::default();
        let mut session_disc = ExchangeSession::new(secret, correlation, nonce, deadline);
        let err = drive_exchange_blocking(
            &mut reader_disc,
            &mut writer_disc,
            &mut session_disc,
            || start,
        )
        .unwrap_err();
        assert_eq!(err, LocalInferenceReadError::Transport);

        // Forged MAC
        let mut resp = encode_response_frame(
            &secret,
            PRIVATE_KIND_CREDENTIAL,
            correlation,
            &nonce,
            1,
            8080,
            b"tok",
        );
        let len = resp.len();
        resp[len - 1] ^= 0xff; // corrupt MAC
        let mut reader_forge = ScriptedReader::new(vec![Ok(resp)]);
        let mut writer_forge = ScriptedWriter::default();
        let mut session_forge = ExchangeSession::new(secret, correlation, nonce, deadline);
        let err = drive_exchange_blocking(
            &mut reader_forge,
            &mut writer_forge,
            &mut session_forge,
            || start,
        )
        .unwrap_err();
        assert_eq!(err, LocalInferenceReadError::Authentication);

        // Wrong correlation (with valid MAC for that wrong correlation)
        let resp_wrong_corr = encode_response_frame(
            &secret,
            PRIVATE_KIND_CREDENTIAL,
            999,
            &nonce,
            1,
            8080,
            b"tok",
        );
        let mut reader_corr = ScriptedReader::new(vec![Ok(resp_wrong_corr)]);
        let mut writer_corr = ScriptedWriter::default();
        let mut session_corr = ExchangeSession::new(secret, correlation, nonce, deadline);
        let err = drive_exchange_blocking(
            &mut reader_corr,
            &mut writer_corr,
            &mut session_corr,
            || start,
        )
        .unwrap_err();
        assert_eq!(err, LocalInferenceReadError::Malformed);
    }

    #[test]
    fn exchange_after_admission_tests() {
        let secret = [5_u8; MAC_LEN];
        let nonce = [6_u8; NONCE_LEN];
        let correlation = 1;
        let start = Instant::now();
        let deadline = start + Duration::from_secs(5);

        // Missing/broken greeting -> Authentication and writes no SCLP
        let mut reader_bad = ScriptedReader::new(vec![Ok(b"broken-greeting".to_vec())]);
        let mut writer_bad = ScriptedWriter::default();
        let err = exchange_after_admission(
            &mut reader_bad,
            &mut writer_bad,
            &secret,
            correlation,
            nonce,
            deadline,
            || start,
        )
        .unwrap_err();
        assert_eq!(err, LocalInferenceReadError::Authentication);
        assert!(!writer_bad.writes.starts_with(&PRIVATE_MAGIC));

        // Valid greeting then EOF -> Transport
        let greeting = crate::windows::server_greeting([9_u8; crate::windows::PIPE_CHALLENGE_LEN]);
        let mut reader_eof = ScriptedReader::new(vec![Ok(greeting.to_vec()), Ok(vec![])]);
        let mut writer_eof = ScriptedWriter::default();
        let err = exchange_after_admission(
            &mut reader_eof,
            &mut writer_eof,
            &secret,
            correlation,
            nonce,
            deadline,
            || start,
        )
        .unwrap_err();
        assert_eq!(err, LocalInferenceReadError::Transport);
    }

    #[test]
    fn gate_sync_and_async_checks() {
        assert_eq!(
            gate_sync(true, false, || Ok([0; 32])),
            Err(LocalInferenceReadError::RuntimeEntered)
        );
        assert_eq!(
            gate_sync(false, false, || Ok([0; 32])),
            Err(LocalInferenceReadError::Unsupported)
        );
        assert_eq!(
            gate_sync(false, true, || Err(())),
            Err(LocalInferenceReadError::Authentication)
        );
        assert_eq!(gate_sync(false, true, || Ok([5; 32])), Ok([5; 32]));

        assert_eq!(
            gate_async(false, || Ok([0; 32])),
            Err(LocalInferenceReadError::Unsupported)
        );
        assert_eq!(
            gate_async(true, || Err(())),
            Err(LocalInferenceReadError::Authentication)
        );
        assert_eq!(gate_async(true, || Ok([6; 32])), Ok([6; 32]));
    }
}
