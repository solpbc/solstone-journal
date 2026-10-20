// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_HEADER_BYTES: usize = 4096;
pub const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_REGIONS: usize = 3000;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_GEOMETRY_POINTS: usize = 16;
pub const MAX_REQUEST_ID_BYTES: usize = 128;
pub const MAX_DIAGNOSTIC_BYTES: usize = 512;
const MAX_READY_BODY_BYTES: usize = 16 * 1024;

pub(crate) fn dimensions_within_pixel_limit(width: u32, height: u32, max_pixels: u64) -> bool {
    width != 0
        && height != 0
        && u64::from(width)
            .checked_mul(u64::from(height))
            .is_some_and(|pixels| pixels <= max_pixels)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Header {
    Ready {
        version: u16,
    },
    Request {
        version: u16,
        request_id: String,
        png_sha256: String,
    },
    Response {
        version: u16,
        request_id: String,
        png_sha256: String,
    },
    Error {
        version: u16,
        request_id: String,
        png_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyIdentity {
    pub protocol_version: u16,
    pub recipe_sha256: String,
    pub helper_sha256: String,
    pub detector_sha256: String,
    pub recognizer_sha256: String,
    pub dictionary_sha256: String,
    pub runtime_sha256: String,
    pub os: String,
    pub arch: String,
    pub settings: crate::recipe::Settings,
    pub limits: crate::recipe::Limits,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseBody {
    pub regions: Vec<Region>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub text: String,
    pub confidence: Option<f32>,
    pub points: Vec<Point>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidPng,
    InferenceFailed,
    ResponseLimit,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub code: ErrorCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Io,
    Truncated,
    InvalidLength,
    HeaderTooLarge,
    BodyTooLarge,
    InvalidHeader,
    InvalidRequestId,
    InvalidDigest,
    WrongMessageKind,
    RequestHashMismatch,
    ResponseBindingMismatch,
    InvalidResponseBody,
    ReadyIdentityMismatch,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Io => "protocol_io",
            Self::Truncated => "truncated_frame",
            Self::InvalidLength => "invalid_length",
            Self::HeaderTooLarge => "header_too_large",
            Self::BodyTooLarge => "body_too_large",
            Self::InvalidHeader => "invalid_header",
            Self::InvalidRequestId => "invalid_request_id",
            Self::InvalidDigest => "invalid_digest",
            Self::WrongMessageKind => "wrong_message_kind",
            Self::RequestHashMismatch => "request_hash_mismatch",
            Self::ResponseBindingMismatch => "response_binding_mismatch",
            Self::InvalidResponseBody => "invalid_response_body",
            Self::ReadyIdentityMismatch => "ready_identity_mismatch",
        };
        f.write_str(message)
    }
}

impl std::error::Error for ProtocolError {}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

pub fn read_message<R: Read>(reader: &mut R) -> Result<Option<(Header, Vec<u8>)>, ProtocolError> {
    let mut header_len_bytes = [0; 4];
    match reader.read(&mut header_len_bytes[..1]) {
        Ok(0) => return Ok(None),
        Ok(1) => {}
        Ok(_) => unreachable!("one-byte read cannot return more than one byte"),
        Err(_) => return Err(ProtocolError::Io),
    }
    reader
        .read_exact(&mut header_len_bytes[1..])
        .map_err(|_| ProtocolError::Truncated)?;

    let header_len = u32::from_be_bytes(header_len_bytes) as usize;
    validate_header_length(header_len)?;

    let mut header_bytes = vec![0; header_len];
    reader
        .read_exact(&mut header_bytes)
        .map_err(|_| ProtocolError::Truncated)?;
    let header: Header =
        serde_json::from_slice(&header_bytes).map_err(|_| ProtocolError::InvalidHeader)?;
    validate_header(&header)?;

    let mut body_len_bytes = [0; 4];
    reader
        .read_exact(&mut body_len_bytes)
        .map_err(|_| ProtocolError::Truncated)?;
    let body_len = u32::from_be_bytes(body_len_bytes) as usize;
    let max_body = body_limit(&header);
    validate_body_length(body_len, max_body)?;
    if matches!(header, Header::Request { .. }) && body_len == 0 {
        return Err(ProtocolError::InvalidLength);
    }

    let mut body = vec![0; body_len];
    reader
        .read_exact(&mut body)
        .map_err(|_| ProtocolError::Truncated)?;
    Ok(Some((header, body)))
}

pub fn write_message<W: Write>(
    writer: &mut W,
    header: &Header,
    body: &[u8],
) -> Result<(), ProtocolError> {
    validate_header(header)?;
    let header_bytes = serde_json::to_vec(header).map_err(|_| ProtocolError::InvalidHeader)?;
    validate_header_length(header_bytes.len())?;
    validate_body_length(body.len(), body_limit(header))?;
    if matches!(header, Header::Request { .. }) && body.is_empty() {
        return Err(ProtocolError::InvalidLength);
    }

    writer
        .write_all(&(header_bytes.len() as u32).to_be_bytes())
        .and_then(|()| writer.write_all(&header_bytes))
        .and_then(|()| writer.write_all(&(body.len() as u32).to_be_bytes()))
        .and_then(|()| writer.write_all(body))
        .map_err(|_| ProtocolError::Io)
}

pub fn validate_png_binding(header: &Header, png: &[u8]) -> Result<(), ProtocolError> {
    let Header::Request { png_sha256, .. } = header else {
        return Err(ProtocolError::WrongMessageKind);
    };
    validate_body_length(png.len(), MAX_PNG_BYTES)?;
    if png.is_empty() {
        return Err(ProtocolError::InvalidLength);
    }
    if sha256_hex(png) != *png_sha256 {
        return Err(ProtocolError::RequestHashMismatch);
    }
    Ok(())
}

pub fn validate_response_binding(
    header: &Header,
    request_id: &str,
    png_sha256: &str,
) -> Result<(), ProtocolError> {
    match header {
        Header::Response {
            version,
            request_id: response_id,
            png_sha256: response_hash,
        }
        | Header::Error {
            version,
            request_id: response_id,
            png_sha256: response_hash,
        } if *version == PROTOCOL_VERSION
            && response_id == request_id
            && response_hash == png_sha256 =>
        {
            Ok(())
        }
        Header::Response { .. } | Header::Error { .. } => {
            Err(ProtocolError::ResponseBindingMismatch)
        }
        _ => Err(ProtocolError::WrongMessageKind),
    }
}

pub fn validate_ready_identity(
    actual: &ReadyIdentity,
    expected: &ReadyIdentity,
) -> Result<(), ProtocolError> {
    validate_ready_fields(actual)?;
    if actual == expected {
        Ok(())
    } else {
        Err(ProtocolError::ReadyIdentityMismatch)
    }
}

pub fn decode_ready_identity(header: &Header, body: &[u8]) -> Result<ReadyIdentity, ProtocolError> {
    let Header::Ready { version } = header else {
        return Err(ProtocolError::WrongMessageKind);
    };
    if *version != PROTOCOL_VERSION || body.len() > MAX_READY_BODY_BYTES {
        return Err(ProtocolError::ReadyIdentityMismatch);
    }
    let identity: ReadyIdentity =
        serde_json::from_slice(body).map_err(|_| ProtocolError::ReadyIdentityMismatch)?;
    if identity.protocol_version != *version {
        return Err(ProtocolError::ReadyIdentityMismatch);
    }
    validate_ready_fields(&identity)?;
    Ok(identity)
}

pub fn decode_response_body(
    body: &[u8],
    width: u32,
    height: u32,
) -> Result<ResponseBody, ProtocolError> {
    validate_body_length(body.len(), MAX_RESPONSE_BYTES)?;
    let response: ResponseBody =
        serde_json::from_slice(body).map_err(|_| ProtocolError::InvalidResponseBody)?;
    validate_response_body(&response, width, height)?;
    Ok(response)
}

pub fn validate_response_body(
    response: &ResponseBody,
    width: u32,
    height: u32,
) -> Result<(), ProtocolError> {
    if response.regions.len() > MAX_REGIONS || width == 0 || height == 0 {
        return Err(ProtocolError::InvalidResponseBody);
    }
    for region in &response.regions {
        if region.text.len() > MAX_TEXT_BYTES
            || region.points.is_empty()
            || region.points.len() > MAX_GEOMETRY_POINTS
            || region
                .confidence
                .is_some_and(|score| !score.is_finite() || !(0.0..=1.0).contains(&score))
            || region.points.iter().any(|point| {
                !point.x.is_finite()
                    || !point.y.is_finite()
                    || point.x < 0.0
                    || point.y < 0.0
                    || point.x > width as f32
                    || point.y > height as f32
            })
        {
            return Err(ProtocolError::InvalidResponseBody);
        }
    }
    Ok(())
}

pub fn validate_error_body(body: &[u8]) -> Result<ErrorBody, ProtocolError> {
    if body.len() > MAX_DIAGNOSTIC_BYTES {
        return Err(ProtocolError::BodyTooLarge);
    }
    serde_json::from_slice(body).map_err(|_| ProtocolError::InvalidResponseBody)
}

fn body_limit(header: &Header) -> usize {
    match header {
        Header::Ready { .. } => MAX_READY_BODY_BYTES,
        Header::Request { .. } => MAX_PNG_BYTES,
        Header::Response { .. } => MAX_RESPONSE_BYTES,
        Header::Error { .. } => MAX_DIAGNOSTIC_BYTES,
    }
}

fn validate_header_length(length: usize) -> Result<(), ProtocolError> {
    if length == 0 {
        Err(ProtocolError::InvalidLength)
    } else if length > MAX_HEADER_BYTES {
        Err(ProtocolError::HeaderTooLarge)
    } else {
        Ok(())
    }
}

fn validate_body_length(length: usize, limit: usize) -> Result<(), ProtocolError> {
    if length > limit {
        Err(ProtocolError::BodyTooLarge)
    } else {
        Ok(())
    }
}

fn validate_header(header: &Header) -> Result<(), ProtocolError> {
    let version = match header {
        Header::Ready { version }
        | Header::Request { version, .. }
        | Header::Response { version, .. }
        | Header::Error { version, .. } => version,
    };
    if *version != PROTOCOL_VERSION {
        return Err(ProtocolError::InvalidHeader);
    }
    if let Header::Request {
        request_id,
        png_sha256,
        ..
    }
    | Header::Response {
        request_id,
        png_sha256,
        ..
    }
    | Header::Error {
        request_id,
        png_sha256,
        ..
    } = header
    {
        validate_request_id(request_id)?;
        validate_digest(png_sha256)?;
    }
    Ok(())
}

fn validate_request_id(request_id: &str) -> Result<(), ProtocolError> {
    if request_id.is_empty()
        || request_id.len() > MAX_REQUEST_ID_BYTES
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ProtocolError::InvalidRequestId);
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<(), ProtocolError> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ProtocolError::InvalidDigest);
    }
    Ok(())
}

fn validate_ready_fields(identity: &ReadyIdentity) -> Result<(), ProtocolError> {
    for digest in [
        identity.recipe_sha256.as_str(),
        identity.helper_sha256.as_str(),
        identity.detector_sha256.as_str(),
        identity.recognizer_sha256.as_str(),
        identity.dictionary_sha256.as_str(),
        identity.runtime_sha256.as_str(),
    ] {
        validate_digest(digest).map_err(|_| ProtocolError::ReadyIdentityMismatch)?;
    }
    if identity.protocol_version != PROTOCOL_VERSION
        || identity.os.is_empty()
        || identity.arch.is_empty()
    {
        return Err(ProtocolError::ReadyIdentityMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn request(png: &[u8]) -> (Header, Vec<u8>) {
        (
            Header::Request {
                version: PROTOCOL_VERSION,
                request_id: "frame_17".to_string(),
                png_sha256: sha256_hex(png),
            },
            png.to_vec(),
        )
    }

    #[test]
    fn framed_request_round_trips_and_hash_binds_actual_png_bytes() {
        let png = b"fake png bytes";
        let (header, body) = request(png);
        let mut bytes = Vec::new();
        write_message(&mut bytes, &header, &body).unwrap();
        let (read_header, read_body) = read_message(&mut Cursor::new(bytes)).unwrap().unwrap();
        assert_eq!(read_header, header);
        assert_eq!(read_body, png);
        validate_png_binding(&read_header, &read_body).unwrap();

        assert_eq!(
            validate_png_binding(&read_header, b"different"),
            Err(ProtocolError::RequestHashMismatch)
        );
    }

    #[test]
    fn framing_rejects_oversized_headers_bodies_and_truncation_before_reading_payloads() {
        let mut oversized_header = Vec::new();
        oversized_header.extend_from_slice(&((MAX_HEADER_BYTES as u32) + 1).to_be_bytes());
        assert_eq!(
            read_message(&mut Cursor::new(oversized_header)),
            Err(ProtocolError::HeaderTooLarge)
        );

        let (header, _) = request(b"png");
        let header_bytes = serde_json::to_vec(&header).unwrap();
        let mut oversized_body = Vec::new();
        oversized_body.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
        oversized_body.extend_from_slice(&header_bytes);
        oversized_body.extend_from_slice(&((MAX_PNG_BYTES as u32) + 1).to_be_bytes());
        assert_eq!(
            read_message(&mut Cursor::new(oversized_body)),
            Err(ProtocolError::BodyTooLarge)
        );

        assert_eq!(
            read_message(&mut Cursor::new([0, 0])),
            Err(ProtocolError::Truncated)
        );
    }

    #[test]
    fn header_rejects_unmodeled_path_and_network_fields() {
        let json = br#"{"type":"request","version":1,"request_id":"x","png_sha256":"0000000000000000000000000000000000000000000000000000000000000000","path":"/private/file","url":"https://example.invalid"}"#;
        assert!(serde_json::from_slice::<Header>(json).is_err());
    }

    #[test]
    fn response_binding_and_shape_are_bounded_and_validated() {
        let digest = "ab".repeat(32);
        let header = Header::Response {
            version: PROTOCOL_VERSION,
            request_id: "frame_17".to_string(),
            png_sha256: digest.clone(),
        };
        validate_response_binding(&header, "frame_17", &digest).unwrap();
        assert_eq!(
            validate_response_binding(&header, "frame_18", &digest),
            Err(ProtocolError::ResponseBindingMismatch)
        );

        let body = ResponseBody {
            regions: vec![Region {
                text: "hello 世界".to_string(),
                confidence: Some(0.9),
                points: vec![
                    Point { x: 1.0, y: 1.0 },
                    Point { x: 5.0, y: 1.0 },
                    Point { x: 5.0, y: 4.0 },
                    Point { x: 1.0, y: 4.0 },
                ],
            }],
        };
        let encoded = serde_json::to_vec(&body).unwrap();
        assert_eq!(decode_response_body(&encoded, 8, 8).unwrap(), body);

        let too_many_points = ResponseBody {
            regions: vec![Region {
                text: "x".to_string(),
                confidence: None,
                points: vec![Point { x: 0.0, y: 0.0 }; MAX_GEOMETRY_POINTS + 1],
            }],
        };
        assert_eq!(
            validate_response_body(&too_many_points, 8, 8),
            Err(ProtocolError::InvalidResponseBody)
        );
    }

    #[test]
    fn ready_identity_comparison_binds_all_runtime_recipe_and_settings_fields() {
        let identity = ReadyIdentity {
            protocol_version: PROTOCOL_VERSION,
            recipe_sha256: "a".repeat(64),
            helper_sha256: "b".repeat(64),
            detector_sha256: "c".repeat(64),
            recognizer_sha256: "d".repeat(64),
            dictionary_sha256: "e".repeat(64),
            runtime_sha256: "f".repeat(64),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            settings: crate::recipe::Recipe::load().unwrap().settings,
            limits: crate::recipe::Recipe::load().unwrap().limits,
        };
        validate_ready_identity(&identity, &identity).unwrap();

        let ready_header = Header::Ready {
            version: PROTOCOL_VERSION,
        };
        let ready_body = serde_json::to_vec(&identity).unwrap();
        assert_eq!(
            decode_ready_identity(&ready_header, &ready_body).unwrap(),
            identity
        );

        let mut other_runtime = identity.clone();
        other_runtime.runtime_sha256 = "0".repeat(64);
        assert_eq!(
            validate_ready_identity(&identity, &other_runtime),
            Err(ProtocolError::ReadyIdentityMismatch)
        );
    }

    #[test]
    fn clean_eof_is_distinct_from_a_truncated_frame() {
        assert_eq!(read_message(&mut Cursor::new([])), Ok(None));
        assert_eq!(
            read_message(&mut Cursor::new([0, 0, 0])),
            Err(ProtocolError::Truncated)
        );
    }

    #[test]
    fn protocol_size_limits_accept_the_boundary_and_reject_one_over() {
        assert_eq!(validate_header_length(MAX_HEADER_BYTES), Ok(()));
        assert_eq!(
            validate_header_length(MAX_HEADER_BYTES + 1),
            Err(ProtocolError::HeaderTooLarge)
        );
        assert_eq!(validate_body_length(MAX_PNG_BYTES, MAX_PNG_BYTES), Ok(()));
        assert_eq!(
            validate_body_length(MAX_PNG_BYTES + 1, MAX_PNG_BYTES),
            Err(ProtocolError::BodyTooLarge)
        );
        assert_eq!(
            validate_body_length(MAX_RESPONSE_BYTES, MAX_RESPONSE_BYTES),
            Ok(())
        );
        assert_eq!(
            validate_body_length(MAX_RESPONSE_BYTES + 1, MAX_RESPONSE_BYTES),
            Err(ProtocolError::BodyTooLarge)
        );
        assert_eq!(
            validate_body_length(MAX_DIAGNOSTIC_BYTES, MAX_DIAGNOSTIC_BYTES),
            Ok(())
        );
        assert_eq!(
            validate_body_length(MAX_DIAGNOSTIC_BYTES + 1, MAX_DIAGNOSTIC_BYTES),
            Err(ProtocolError::BodyTooLarge)
        );
        assert_eq!(
            validate_body_length(MAX_READY_BODY_BYTES, MAX_READY_BODY_BYTES),
            Ok(())
        );
        assert_eq!(
            validate_body_length(MAX_READY_BODY_BYTES + 1, MAX_READY_BODY_BYTES),
            Err(ProtocolError::BodyTooLarge)
        );
    }

    #[test]
    fn decoded_pixel_limit_accepts_exact_boundary_and_rejects_invalid_dimensions() {
        assert!(dimensions_within_pixel_limit(40_000_000, 1, 40_000_000));
        assert!(!dimensions_within_pixel_limit(40_000_001, 1, 40_000_000));
        assert!(!dimensions_within_pixel_limit(0, 1, 40_000_000));
        assert!(!dimensions_within_pixel_limit(1, 0, 40_000_000));
        assert!(!dimensions_within_pixel_limit(
            u32::MAX,
            u32::MAX,
            40_000_000
        ));
    }

    #[test]
    fn request_and_response_headers_reject_identity_mismatches_and_bad_fields() {
        let digest = "ab".repeat(32);
        let (request_header, png) = request(b"image bytes");
        assert_eq!(validate_png_binding(&request_header, &png), Ok(()));
        assert_eq!(
            validate_png_binding(&request_header, b"different image"),
            Err(ProtocolError::RequestHashMismatch)
        );
        assert_eq!(
            validate_png_binding(&request_header, &[]),
            Err(ProtocolError::InvalidLength)
        );

        let response = Header::Response {
            version: PROTOCOL_VERSION,
            request_id: "frame_17".into(),
            png_sha256: digest.clone(),
        };
        assert_eq!(
            validate_response_binding(&response, "frame_17", &digest),
            Ok(())
        );
        assert_eq!(
            validate_response_binding(&response, "frame_18", &digest),
            Err(ProtocolError::ResponseBindingMismatch)
        );
        assert_eq!(
            validate_response_binding(&response, "frame_17", &"cd".repeat(32)),
            Err(ProtocolError::ResponseBindingMismatch)
        );
        assert_eq!(
            validate_response_binding(
                &Header::Response {
                    version: PROTOCOL_VERSION + 1,
                    request_id: "frame_17".into(),
                    png_sha256: digest.clone(),
                },
                "frame_17",
                &digest,
            ),
            Err(ProtocolError::ResponseBindingMismatch)
        );
        assert_eq!(
            validate_request_id(&"x".repeat(MAX_REQUEST_ID_BYTES)),
            Ok(())
        );
        assert_eq!(
            validate_request_id(&"x".repeat(MAX_REQUEST_ID_BYTES + 1)),
            Err(ProtocolError::InvalidRequestId)
        );
        assert_eq!(validate_digest(&"a".repeat(64)), Ok(()));
        assert_eq!(
            validate_digest(&"g".repeat(64)),
            Err(ProtocolError::InvalidDigest)
        );
    }

    #[test]
    fn response_region_text_and_geometry_limits_accept_boundary_and_reject_overflow() {
        let boundary_region = Region {
            text: "x".to_string(),
            confidence: Some(1.0),
            points: vec![Point { x: 0.0, y: 0.0 }; MAX_GEOMETRY_POINTS],
        };
        let mut response = ResponseBody {
            regions: vec![boundary_region.clone(); MAX_REGIONS],
        };
        assert_eq!(validate_response_body(&response, 8, 8), Ok(()));

        response.regions.push(boundary_region.clone());
        assert_eq!(
            validate_response_body(&response, 8, 8),
            Err(ProtocolError::InvalidResponseBody)
        );

        let max_text = Region {
            text: "x".repeat(MAX_TEXT_BYTES),
            confidence: Some(1.0),
            points: vec![Point { x: 0.0, y: 0.0 }],
        };
        assert_eq!(
            validate_response_body(
                &ResponseBody {
                    regions: vec![max_text],
                },
                8,
                8,
            ),
            Ok(())
        );

        let too_long_text = Region {
            text: "x".repeat(MAX_TEXT_BYTES + 1),
            confidence: Some(1.0),
            points: vec![Point { x: 0.0, y: 0.0 }],
        };
        assert_eq!(
            validate_response_body(
                &ResponseBody {
                    regions: vec![too_long_text],
                },
                8,
                8,
            ),
            Err(ProtocolError::InvalidResponseBody)
        );

        let mut too_many_points = boundary_region;
        too_many_points.points.push(Point { x: 0.0, y: 0.0 });
        assert_eq!(
            validate_response_body(
                &ResponseBody {
                    regions: vec![too_many_points],
                },
                8,
                8,
            ),
            Err(ProtocolError::InvalidResponseBody)
        );

        for point in [
            Point { x: 9.0, y: 0.0 },
            Point {
                x: f32::NAN,
                y: 0.0,
            },
        ] {
            assert_eq!(
                validate_response_body(
                    &ResponseBody {
                        regions: vec![Region {
                            text: "x".into(),
                            confidence: Some(0.5),
                            points: vec![point],
                        }],
                    },
                    8,
                    8,
                ),
                Err(ProtocolError::InvalidResponseBody)
            );
        }
    }
}
