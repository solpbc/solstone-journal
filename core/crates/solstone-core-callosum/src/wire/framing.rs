// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::CallosumEnvelope;

use super::READ_BUFFER_CAPACITY;

/// Result of decoding one newline-delimited Callosum frame.
pub(crate) enum ReadFrame {
    Envelope(CallosumEnvelope),
    Whitespace,
    Malformed,
    InvalidUtf8,
    Eof,
}

/// Read one frame, retaining partial bytes if an enclosing select cancels the read.
pub(crate) async fn read_frame<R>(
    reader: &mut BufReader<R>,
    buffer: &mut Vec<u8>,
) -> io::Result<ReadFrame>
where
    R: AsyncRead + Unpin,
{
    reader.read_until(b'\n', buffer).await?;
    if buffer.is_empty() {
        return Ok(ReadFrame::Eof);
    }
    if buffer.last() == Some(&b'\n') {
        buffer.pop();
    }
    let frame = decode_frame(buffer);
    buffer.clear();
    Ok(frame)
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
    use std::future::{Future, poll_fn};
    use std::task::Poll;
    use tokio::io::AsyncWriteExt;

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_frame_read_retains_each_fragment_until_decode() {
        let (mut writer, read_half) = tokio::io::duplex(256);
        let mut frame_reader = reader(read_half);
        let mut buffer = Vec::new();
        for fragment in [br#"{"tract":"cortex","#.as_slice(), br#""event":"error"}"#] {
            writer.write_all(fragment).await.unwrap();
            let mut pending = Box::pin(read_frame(&mut frame_reader, &mut buffer));
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
            matches!(read_frame(&mut frame_reader, &mut buffer).await.unwrap(), ReadFrame::Envelope(message) if message.tract == "cortex" && message.event == "error")
        );
        assert!(buffer.is_empty());
        assert!(
            matches!(read_frame(&mut frame_reader, &mut buffer).await.unwrap(), ReadFrame::Envelope(message) if message.tract == "next")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_frame_read_decodes_retained_final_bytes_at_eof() {
        let (mut writer, read_half) = tokio::io::duplex(256);
        let mut frame_reader = reader(read_half);
        let mut buffer = Vec::new();
        writer
            .write_all(br#"{"tract":"cortex","event":"error"}"#)
            .await
            .unwrap();
        let mut pending = Box::pin(read_frame(&mut frame_reader, &mut buffer));
        assert!(
            poll_fn(|cx| Poll::Ready(pending.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        drop(pending);
        writer.shutdown().await.unwrap();
        assert!(
            matches!(read_frame(&mut frame_reader, &mut buffer).await.unwrap(), ReadFrame::Envelope(message) if message.event == "error")
        );
        assert!(buffer.is_empty());
        assert!(matches!(
            read_frame(&mut frame_reader, &mut buffer).await.unwrap(),
            ReadFrame::Eof
        ));
    }
}
