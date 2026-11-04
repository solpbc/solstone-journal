// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared media mechanics; callers own path admission, scheduling and errors.

use std::future::Future;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes, HttpBody};
use axum::http::{StatusCode, header};
use axum::response::Response;
use hyper::body::Frame;

#[derive(Debug, PartialEq, Eq)]
pub enum ParsedRange {
    Valid { start: usize, end: usize },
    Unsatisfiable,
    Ignore,
}

/// Parse the legacy single-range grammar for a nonempty file.
pub fn parse_range(value: &str, total: usize) -> ParsedRange {
    let Some(spec) = value.strip_prefix("bytes=") else {
        return ParsedRange::Ignore;
    };
    if spec.contains(',') {
        return ParsedRange::Ignore;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return ParsedRange::Ignore;
    };
    if start.is_empty() {
        let Ok(length) = end.parse::<usize>() else {
            return ParsedRange::Ignore;
        };
        if length == 0 {
            return ParsedRange::Unsatisfiable;
        }
        let start = total.saturating_sub(length);
        return ParsedRange::Valid {
            start,
            end: total - 1,
        };
    }
    let Ok(start) = start.parse::<usize>() else {
        return ParsedRange::Ignore;
    };
    if start >= total {
        return ParsedRange::Unsatisfiable;
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        let Ok(end) = end.parse::<usize>() else {
            return ParsedRange::Ignore;
        };
        end.min(total - 1)
    };
    if end < start {
        ParsedRange::Unsatisfiable
    } else {
        ParsedRange::Valid { start, end }
    }
}

/// Personal media, the owner's own recordings, is never kept by any cache, and
/// every answer advertises ranges so a player can seek from the first response.
pub const MEDIA_CACHE_CONTROL: &str = "no-store";

pub fn media_response(
    status: StatusCode,
    mimetype: &str,
    path: &Path,
    body: MediaBody,
    content_range: Option<String>,
) -> Response {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mimetype)
        .header(header::CONTENT_LENGTH, body.length)
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename={filename}"),
        )
        .header(header::CACHE_CONTROL, MEDIA_CACHE_CONTROL)
        .header(header::ACCEPT_RANGES, "bytes");
    if let Some(content_range) = content_range {
        response = response.header(header::CONTENT_RANGE, content_range);
    }
    response
        .body(Body::new(body))
        .expect("media response builds")
}

/// A selected interval of an open file, sent as it is read.
///
/// The first chunk is read when the body is made, on the caller's blocking
/// thread, so an open, seek or read failure still becomes an error response
/// before any header is sent. Each later chunk is read on the blocking pool,
/// never on a Convey async worker, so a slow client holds no thread between
/// chunks. A file that ends inside the interval ends the body with an error,
/// which aborts the response short of its `Content-Length` rather than passing
/// partial media off as complete.
pub struct MediaBody {
    length: u64,
    state: MediaBodyState,
}

type MediaReader = Box<dyn ReadSeek + Send>;
type ChunkRead = tokio::task::JoinHandle<std::io::Result<(MediaReader, Vec<u8>)>>;

enum MediaBodyState {
    Ready {
        reader: Option<MediaReader>,
        pending: Option<Vec<u8>>,
        remaining: u64,
    },
    Reading {
        task: ChunkRead,
        remaining: u64,
    },
    Done,
}

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// Bytes read per chunk.
pub const MEDIA_CHUNK_BYTES: usize = 64 * 1024;

impl MediaBody {
    pub fn empty() -> Self {
        Self {
            length: 0,
            state: MediaBodyState::Done,
        }
    }

    /// Seek to `start` and read the first chunk of `length` bytes now.
    pub fn from_reader(
        mut reader: impl Read + Seek + Send + 'static,
        start: usize,
        length: usize,
    ) -> std::io::Result<Self> {
        if length == 0 {
            return Ok(Self::empty());
        }
        reader.seek(SeekFrom::Start(start as u64))?;
        let length = length as u64;
        let first = read_chunk(&mut reader, length)?;
        let remaining = length - first.len() as u64;
        Ok(Self {
            length,
            state: MediaBodyState::Ready {
                reader: (remaining > 0).then(|| Box::new(reader) as MediaReader),
                pending: Some(first),
                remaining,
            },
        })
    }
}

fn read_chunk(reader: &mut (impl Read + ?Sized), remaining: u64) -> std::io::Result<Vec<u8>> {
    let size =
        usize::try_from(remaining.min(MEDIA_CHUNK_BYTES as u64)).unwrap_or(MEDIA_CHUNK_BYTES);
    let mut chunk = vec![0; size];
    reader.read_exact(&mut chunk)?;
    Ok(chunk)
}

impl HttpBody for MediaBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        loop {
            match &mut self.state {
                MediaBodyState::Done => return Poll::Ready(None),
                MediaBodyState::Ready {
                    reader,
                    pending,
                    remaining,
                } => {
                    if let Some(chunk) = pending.take() {
                        return Poll::Ready(Some(Ok(Frame::data(Bytes::from(chunk)))));
                    }
                    let remaining = *remaining;
                    let Some(mut reader) = reader.take().filter(|_| remaining > 0) else {
                        self.state = MediaBodyState::Done;
                        return Poll::Ready(None);
                    };
                    let task = tokio::task::spawn_blocking(move || {
                        let chunk = read_chunk(&mut reader, remaining)?;
                        Ok((reader, chunk))
                    });
                    self.state = MediaBodyState::Reading { task, remaining };
                }
                MediaBodyState::Reading { task, remaining } => {
                    let read = match Pin::new(task).poll(context) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(read) => read,
                    };
                    let remaining = *remaining;
                    match read {
                        Ok(Ok((reader, chunk))) => {
                            let left = remaining - chunk.len() as u64;
                            self.state = MediaBodyState::Ready {
                                reader: Some(reader),
                                pending: Some(chunk),
                                remaining: left,
                            };
                        }
                        Ok(Err(error)) => {
                            self.state = MediaBodyState::Done;
                            return Poll::Ready(Some(Err(error)));
                        }
                        Err(join) => {
                            self.state = MediaBodyState::Done;
                            return Poll::Ready(Some(Err(std::io::Error::other(join))));
                        }
                    }
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self.state, MediaBodyState::Done)
    }
}

pub fn mime_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "flac" => Some("audio/flac"),
        "opus" => Some("audio/opus"),
        "ogg" => Some("audio/ogg"),
        "m4a" => Some("audio/mp4"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "webm" => Some("video/webm"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "heic" => Some("image/heic"),
        "heif" => Some("image/heif"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "tiff" => Some("image/tiff"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind};

    #[test]
    fn range_grammar_preserves_suffix_open_invalid_and_multiple_ranges() {
        use ParsedRange::{Ignore, Unsatisfiable, Valid};
        for (value, expected) in [
            ("bytes=1-2", Valid { start: 1, end: 2 }),
            ("bytes=1-99", Valid { start: 1, end: 4 }),
            ("bytes=3-", Valid { start: 3, end: 4 }),
            ("bytes=-2", Valid { start: 3, end: 4 }),
            ("bytes=-99", Valid { start: 0, end: 4 }),
            ("bytes=-0", Unsatisfiable),
            ("bytes=5-", Unsatisfiable),
            ("bytes=5-invalid", Unsatisfiable),
            ("bytes=3-1", Unsatisfiable),
            ("bytes=0-1,3-4", Ignore),
            ("Bytes=0-1", Ignore),
            ("items=0-1", Ignore),
            ("bytes= 0-1", Ignore),
            ("bytes=0-invalid", Ignore),
            ("bytes=-", Ignore),
            ("bytes=garbage", Ignore),
            ("bytes=18446744073709551616-", Ignore),
            ("bytes=-18446744073709551616", Ignore),
            ("bytes=0-18446744073709551616", Ignore),
        ] {
            assert_eq!(parse_range(value, 5), expected, "{value}");
        }
    }

    async fn collect(mut body: MediaBody) -> (Vec<u8>, usize, Option<std::io::Error>) {
        let mut bytes = Vec::new();
        let mut frames = 0;
        while let Some(frame) =
            std::future::poll_fn(|context| Pin::new(&mut body).poll_frame(context)).await
        {
            match frame {
                Ok(frame) => {
                    frames += 1;
                    bytes.extend_from_slice(frame.data_ref().unwrap());
                }
                Err(error) => return (bytes, frames, Some(error)),
            }
        }
        (bytes, frames, None)
    }

    struct Counted {
        file: Cursor<Vec<u8>>,
        read: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Read for Counted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let length = self.file.read(buf)?;
            self.read
                .fetch_add(length, std::sync::atomic::Ordering::SeqCst);
            Ok(length)
        }
    }
    impl Seek for Counted {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.file.seek(position)
        }
    }

    #[tokio::test]
    async fn streams_only_the_selected_interval_in_chunks() {
        let source: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
        let read = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let body = MediaBody::from_reader(
            Counted {
                file: Cursor::new(source.clone()),
                read: read.clone(),
            },
            1_000,
            200_000,
        )
        .unwrap();
        // Only the first chunk is read before the response starts.
        assert_eq!(
            read.load(std::sync::atomic::Ordering::SeqCst),
            MEDIA_CHUNK_BYTES
        );
        let (bytes, frames, error) = collect(body).await;
        assert!(error.is_none());
        assert_eq!(bytes, source[1_000..201_000]);
        assert_eq!(frames, 200_000usize.div_ceil(MEDIA_CHUNK_BYTES));
        assert_eq!(read.load(std::sync::atomic::Ordering::SeqCst), 200_000);
    }

    #[tokio::test]
    async fn a_short_interval_refuses_before_headers_or_aborts_the_body() {
        // Short within the first chunk: no response is made.
        let error = MediaBody::from_reader(Cursor::new(vec![7; 4096]), 4095, 2)
            .err()
            .unwrap();
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
        // Short after the first chunk: the body ends in an error, never short and clean.
        let body = MediaBody::from_reader(
            Cursor::new(vec![7; MEDIA_CHUNK_BYTES + 10]),
            0,
            MEDIA_CHUNK_BYTES + 20,
        )
        .unwrap();
        let (bytes, _, error) = collect(body).await;
        assert_eq!(bytes.len(), MEDIA_CHUNK_BYTES);
        assert_eq!(error.unwrap().kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn every_media_answer_is_uncacheable_and_advertises_ranges() {
        for (status, range) in [
            (StatusCode::OK, None),
            (StatusCode::PARTIAL_CONTENT, Some("bytes 0-1/5".to_owned())),
        ] {
            let response = media_response(
                status,
                "audio/flac",
                Path::new("mic.flac"),
                MediaBody::from_reader(Cursor::new(b"abcde".to_vec()), 0, 2).unwrap(),
                range.clone(),
            );
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
            assert_eq!(response.headers()[header::CONTENT_LENGTH], "2");
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_RANGE)
                    .map(|value| value.to_str().unwrap().to_owned()),
                range
            );
        }
    }

    #[test]
    fn seek_and_read_failures_propagate() {
        struct Failing(bool);
        impl Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(ErrorKind::PermissionDenied))
            }
        }
        impl Seek for Failing {
            fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
                if self.0 {
                    Ok(0)
                } else {
                    Err(std::io::Error::from(ErrorKind::Unsupported))
                }
            }
        }
        assert_eq!(
            MediaBody::from_reader(Failing(false), 0, 1)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::Unsupported
        );
        assert_eq!(
            MediaBody::from_reader(Failing(true), 0, 1)
                .err()
                .unwrap()
                .kind(),
            ErrorKind::PermissionDenied
        );
    }
}
