// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::CallosumEnvelope;
use crate::local_inference::{
    FrameAccum, FrameClass, LocalInferencePrivateRequest, PRIVATE_FRAME_CAP, PRIVATE_HEADER_LEN,
    PRIVATE_KIND_CREDENTIAL, PRIVATE_KIND_UNAVAILABLE, PRIVATE_MAGIC, PRIVATE_OP_READ_SNAPSHOT,
    PRIVATE_VERSION, decode_request_frame,
};

use super::READ_BUFFER_CAPACITY;

/// Result of decoding one Callosum wire frame.
pub(crate) enum ReadFrame {
    Envelope(CallosumEnvelope),
    Whitespace,
    Malformed,
    InvalidUtf8,
    PrivateRequest(LocalInferencePrivateRequest),
    PrivateResponse(#[allow(dead_code)] Vec<u8>),
    PrivateRejected,
    Eof,
}

/// Read one frame, retaining partial bytes and classification if an enclosing select cancels the read.
pub(crate) async fn read_frame<R>(
    reader: &mut BufReader<R>,
    accum: &mut FrameAccum,
) -> io::Result<ReadFrame>
where
    R: AsyncRead + Unpin,
{
    loop {
        if accum.check_ambiguous() {
            accum.clear();
            return Ok(ReadFrame::PrivateRejected);
        }

        match accum.class() {
            FrameClass::Undecided => {
                let available = reader.fill_buf().await?;
                if available.is_empty() {
                    if accum.is_empty() {
                        return Ok(ReadFrame::Eof);
                    }
                    let frame = decode_frame(accum.bytes());
                    accum.clear();
                    return Ok(frame);
                }
                let byte = available[0];
                reader.consume(1);
                accum.push_byte(byte);
            }
            FrameClass::Ordinary => {
                let initial_len = accum.len();
                let bytes_read = reader.read_until(b'\n', accum.bytes_mut()).await?;
                if bytes_read == 0 && accum.len() == initial_len {
                    if accum.is_empty() {
                        return Ok(ReadFrame::Eof);
                    }
                    let frame = decode_frame(accum.bytes());
                    accum.clear();
                    return Ok(frame);
                }
                if accum.bytes().last() == Some(&b'\n') {
                    accum.bytes_mut().pop();
                    let frame = decode_frame(accum.bytes());
                    accum.clear();
                    return Ok(frame);
                }
            }
            FrameClass::Private => {
                if accum.len() < PRIVATE_HEADER_LEN {
                    let needed = PRIVATE_HEADER_LEN - accum.len();
                    let available = reader.fill_buf().await?;
                    if available.is_empty() {
                        accum.clear();
                        return Ok(ReadFrame::PrivateRejected);
                    }
                    let to_consume = available.len().min(needed);
                    accum.extend_from_slice(&available[..to_consume]);
                    reader.consume(to_consume);
                    if accum.check_ambiguous() {
                        accum.clear();
                        return Ok(ReadFrame::PrivateRejected);
                    }
                    if accum.len() < PRIVATE_HEADER_LEN {
                        continue;
                    }
                }

                let header = accum.bytes();
                if header[..4] != PRIVATE_MAGIC || header[4] != PRIVATE_VERSION {
                    accum.set_class(FrameClass::PrivateRejected);
                    continue;
                }
                let discriminant = header[5];
                let body_len = u32::from_le_bytes(header[6..10].try_into().unwrap()) as usize;
                if PRIVATE_HEADER_LEN + body_len > PRIVATE_FRAME_CAP {
                    accum.set_class(FrameClass::PrivateRejected);
                    continue;
                }

                let total_len = PRIVATE_HEADER_LEN + body_len;
                if accum.len() < total_len {
                    let needed = total_len - accum.len();
                    let available = reader.fill_buf().await?;
                    if available.is_empty() {
                        accum.clear();
                        return Ok(ReadFrame::PrivateRejected);
                    }
                    let to_consume = available.len().min(needed);
                    accum.extend_from_slice(&available[..to_consume]);
                    reader.consume(to_consume);
                    if accum.len() < total_len {
                        continue;
                    }
                }

                let frame_bytes = accum.bytes();
                let frame = if discriminant == PRIVATE_OP_READ_SNAPSHOT {
                    match decode_request_frame(frame_bytes) {
                        Some(request) => ReadFrame::PrivateRequest(request),
                        None => ReadFrame::PrivateRejected,
                    }
                } else if discriminant == PRIVATE_KIND_CREDENTIAL
                    || discriminant == PRIVATE_KIND_UNAVAILABLE
                {
                    ReadFrame::PrivateResponse(frame_bytes.to_vec())
                } else {
                    ReadFrame::PrivateRejected
                };
                accum.clear();
                return Ok(frame);
            }
            FrameClass::PrivateRejected => {
                let available = reader.fill_buf().await?;
                if available.is_empty() {
                    accum.clear();
                    return Ok(ReadFrame::PrivateRejected);
                }
                if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                    reader.consume(pos + 1);
                    accum.clear();
                    return Ok(ReadFrame::PrivateRejected);
                }
                let remaining_to_cap = PRIVATE_FRAME_CAP.saturating_sub(accum.total_len());
                let to_consume = available.len().min(remaining_to_cap);
                if to_consume > 0 {
                    reader.consume(to_consume);
                    accum.add_skipped(to_consume);
                }
                if accum.total_len() >= PRIVATE_FRAME_CAP {
                    accum.clear();
                    return Ok(ReadFrame::PrivateRejected);
                }
            }
        }
    }
}

fn decode_frame(buffer: &[u8]) -> ReadFrame {
    let line = match std::str::from_utf8(buffer) {
        Ok(line) => line,
        Err(_) => return ReadFrame::InvalidUtf8,
    };
    if line.trim().is_empty() {
        return ReadFrame::Whitespace;
    }
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return ReadFrame::Malformed,
    };
    let Some(object) = value.as_object() else {
        return ReadFrame::Malformed;
    };
    if !object.contains_key("tract") || !object.contains_key("event") {
        return ReadFrame::Malformed;
    }
    match serde_json::from_value(value) {
        Ok(envelope) => ReadFrame::Envelope(envelope),
        Err(_) => ReadFrame::Malformed,
    }
}

pub(crate) fn reader<R>(stream: R) -> BufReader<R>
where
    R: AsyncRead + Unpin,
{
    BufReader::with_capacity(READ_BUFFER_CAPACITY, stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::poll_fn;
    use std::task::Poll;
    use tokio::io::AsyncWriteExt;

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_frame_read_retains_each_fragment_until_decode() {
        let (mut writer, read_half) = tokio::io::duplex(256);
        let mut frame_reader = reader(read_half);
        let mut accum = FrameAccum::new();
        for fragment in [br#"{"tract":"cortex","#.as_slice(), br#""event":"error"}"#] {
            writer.write_all(fragment).await.unwrap();
            let mut pending = Box::pin(read_frame(&mut frame_reader, &mut accum));
            assert!(
                poll_fn(|cx| Poll::Ready(pending.as_mut().poll(cx)))
                    .await
                    .is_pending()
            );
            drop(pending);
        }
        writer
            .write_all(b"\n{\"tract\":\"next\",\"event\":\"complete\"}\n")
            .await
            .unwrap();
        assert!(
            matches!(read_frame(&mut frame_reader, &mut accum).await.unwrap(), ReadFrame::Envelope(message) if message.tract == "cortex" && message.event == "error")
        );
        assert!(accum.is_empty());
        assert!(
            matches!(read_frame(&mut frame_reader, &mut accum).await.unwrap(), ReadFrame::Envelope(message) if message.tract == "next")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_frame_read_decodes_retained_final_bytes_at_eof() {
        let (mut writer, read_half) = tokio::io::duplex(256);
        let mut frame_reader = reader(read_half);
        let mut accum = FrameAccum::new();
        writer
            .write_all(br#"{"tract":"cortex","event":"error"}"#)
            .await
            .unwrap();
        let mut pending = Box::pin(read_frame(&mut frame_reader, &mut accum));
        assert!(
            poll_fn(|cx| Poll::Ready(pending.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        drop(pending);
        writer.shutdown().await.unwrap();
        assert!(
            matches!(read_frame(&mut frame_reader, &mut accum).await.unwrap(), ReadFrame::Envelope(message) if message.event == "error")
        );
        assert!(accum.is_empty());
        assert!(matches!(
            read_frame(&mut frame_reader, &mut accum).await.unwrap(),
            ReadFrame::Eof
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_private_fragment_retains_private_class_and_is_not_envelope() {
        let (mut writer, read_half) = tokio::io::duplex(256);
        let mut frame_reader = reader(read_half);
        let mut accum = FrameAccum::new();

        let req = crate::local_inference::encode_request_frame(42, &[9_u8; 32]);
        writer.write_all(&req[..15]).await.unwrap();
        let mut pending = Box::pin(read_frame(&mut frame_reader, &mut accum));
        assert!(
            poll_fn(|cx| Poll::Ready(pending.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        drop(pending);

        writer.write_all(&req[15..]).await.unwrap();
        let frame = read_frame(&mut frame_reader, &mut accum).await.unwrap();
        match frame {
            ReadFrame::PrivateRequest(decoded) => {
                assert_eq!(decoded.correlation, 42);
                assert_eq!(decoded.nonce, [9_u8; 32]);
            }
            _ => panic!("expected PrivateRequest"),
        }
    }
}
