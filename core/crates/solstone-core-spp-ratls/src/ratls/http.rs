// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{ErrorKind, Read, Write};

pub const MAX_PROOF_RESPONSE_HEADERS: usize = 16 * 1024;
pub const MAX_PROOF_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedHttpResponse {
    pub status_line: Vec<u8>,
    pub headers: Vec<(Vec<u8>, Vec<u8>)>,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub enum BoundedHttpError {
    Transport(std::io::Error),
    Protocol(&'static str),
}

pub fn retry_interrupted<T>(
    mut operation: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

pub fn write_all_retry_interrupted<W: Write + ?Sized>(
    stream: &mut W,
    mut bytes: &[u8],
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let count = retry_interrupted(|| stream.write(bytes))?;
        if count == 0 {
            return Err(std::io::Error::from(ErrorKind::WriteZero));
        }
        bytes = &bytes[count..];
    }
    retry_interrupted(|| stream.flush())
}

pub fn response_status(status_line: &[u8]) -> Result<u16, &'static str> {
    let mut parts = status_line.split(|byte| *byte == b' ');
    let Some(version) = parts.next() else {
        return Err("response_status_invalid");
    };
    let Some(status) = parts.next() else {
        return Err("response_status_invalid");
    };
    if !version.starts_with(b"HTTP/") || status.len() != 3 {
        return Err("response_status_invalid");
    }
    let code: u16 = parse_ascii_digits(status).ok_or("response_status_invalid")?;
    if (100..200).contains(&code) {
        return Err("response_status_interim");
    }
    if code == 204 {
        return Err("response_status_no_content");
    }
    if code == 304 {
        return Err("response_status_not_modified");
    }
    Ok(code)
}

pub fn http_header_lines(head: &[u8]) -> Result<Vec<&[u8]>, &'static str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < head.len() {
        match head[index] {
            b'\r' if head.get(index + 1) == Some(&b'\n') => {
                lines.push(&head[start..index]);
                index += 2;
                start = index;
            }
            b'\r' | b'\n' => return Err("response_header_not_crlf"),
            _ => index += 1,
        }
    }
    lines.push(&head[start..]);
    Ok(lines)
}

pub fn recv_bounded_http_response<R: Read + ?Sized>(
    stream: &mut R,
    max_headers: usize,
    max_body: usize,
) -> Result<BoundedHttpResponse, BoundedHttpError> {
    let mut data = Vec::new();
    let marker = b"\r\n\r\n";
    while !data.windows(marker.len()).any(|window| window == marker) {
        if data.len() >= max_headers {
            return Err(BoundedHttpError::Protocol("response_headers_too_large"));
        }
        let mut buffer = [0u8; 4096];
        let read_len = buffer.len().min(max_headers - data.len());
        let count = retry_interrupted(|| stream.read(&mut buffer[..read_len]))
            .map_err(BoundedHttpError::Transport)?;
        if count == 0 {
            return Err(BoundedHttpError::Protocol("response_eof"));
        }
        data.extend_from_slice(&buffer[..count]);
    }
    let split = data
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("marker present");
    let (head, remainder) = data.split_at(split);
    let mut body = remainder[marker.len()..].to_vec();
    let lines = http_header_lines(head).map_err(BoundedHttpError::Protocol)?;
    let status_line = lines
        .first()
        .ok_or(BoundedHttpError::Protocol("response_status_invalid"))?
        .to_vec();
    let _ = response_status(&status_line).map_err(BoundedHttpError::Protocol)?;
    let mut headers = Vec::new();
    let mut content_length = None;
    for line in &lines[1..] {
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(BoundedHttpError::Protocol("response_header_invalid"));
        };
        let (name, value_with_colon) = line.split_at(colon);
        let value = &value_with_colon[1..];
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            return Err(BoundedHttpError::Protocol("response_transfer_encoding"));
        }
        if name.eq_ignore_ascii_case(b"content-length") {
            if content_length.is_some() {
                return Err(BoundedHttpError::Protocol(
                    "response_content_length_duplicate",
                ));
            }
            content_length = parse_ascii_digits(trim_optional_whitespace(value));
            if content_length.is_none() {
                return Err(BoundedHttpError::Protocol(
                    "response_content_length_invalid",
                ));
            }
        }
        headers.push((name.to_vec(), value.to_vec()));
    }
    let length =
        content_length
            .filter(|length| *length <= max_body)
            .ok_or(BoundedHttpError::Protocol(
                "response_content_length_invalid",
            ))?;
    if body.len() > length {
        return Err(BoundedHttpError::Protocol("response_surplus"));
    }
    while body.len() < length {
        let mut buffer = [0u8; 65536];
        let remaining = (length - body.len()).min(buffer.len());
        let count = retry_interrupted(|| stream.read(&mut buffer[..remaining]))
            .map_err(BoundedHttpError::Transport)?;
        if count == 0 {
            return Err(BoundedHttpError::Protocol("response_body_eof"));
        }
        body.extend_from_slice(&buffer[..count]);
    }
    Ok(BoundedHttpResponse {
        status_line,
        headers,
        body,
    })
}

/// Header field values may carry optional spaces or tabs around them; nothing else.
fn trim_optional_whitespace(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(start, |index| index + 1);
    &value[start..end]
}

/// Parses a non-empty run of ASCII digits; a sign or any other byte is refused.
fn parse_ascii_digits<T: std::str::FromStr>(value: &[u8]) -> Option<T> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn read(bytes: &[u8]) -> Result<BoundedHttpResponse, BoundedHttpError> {
        recv_bounded_http_response(&mut Cursor::new(bytes.to_vec()), 4096, 4096)
    }

    #[test]
    fn numeric_fields_accept_only_ascii_digits() {
        assert!(read(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").is_ok());
        assert!(read(b"HTTP/1.1 200 OK\r\nContent-Length: \t 2 \r\n\r\n{}").is_ok());
        for refused in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: +2\r\n\r\n{}"[..],
            b"HTTP/1.1 200 OK\r\nContent-Length: \x0b2\r\n\r\n{}",
            b"HTTP/1.1 200 OK\r\nContent-Length: 2x\r\n\r\n{}",
            b"HTTP/1.1 200 OK\r\nContent-Length: \r\n\r\n{}",
            b"HTTP/1.1 +20 OK\r\nContent-Length: 2\r\n\r\n{}",
        ] {
            assert!(
                read(refused).is_err(),
                "{:?}",
                String::from_utf8_lossy(refused)
            );
        }
    }
}
