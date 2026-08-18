// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::{Path, PathBuf};

use axum::extract::multipart::{Field, MultipartError};
use axum::http::StatusCode;
use tempfile::{NamedTempFile, TempPath};
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub(crate) enum SaveStreamError {
    CeilingExceeded,
    LengthLimited,
    Read,
    Io(io::Error),
}

#[derive(Debug)]
pub(crate) struct CountedTemp {
    bytes: u64,
    temp: TempPath,
}

impl CountedTemp {
    pub(crate) fn path(&self) -> &Path {
        &self.temp
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn keep(self) -> io::Result<PathBuf> {
        self.temp.keep().map_err(io::Error::from)
    }
}

pub(crate) fn is_length_limit(error: &MultipartError) -> bool {
    error.status() == StatusCode::PAYLOAD_TOO_LARGE
}

pub(crate) async fn stream_field(
    mut field: Field<'_>,
    directory: &Path,
    ceiling: u64,
) -> Result<CountedTemp, SaveStreamError> {
    let temporary = NamedTempFile::new_in(directory).map_err(SaveStreamError::Io)?;
    let (std_file, temp) = temporary.into_parts();
    let mut file = tokio::fs::File::from_std(std_file);
    let mut written = 0_u64;
    let result = async {
        while let Some(chunk) = field.chunk().await.map_err(read_error)? {
            let added = u64::try_from(chunk.len()).map_err(|_| SaveStreamError::CeilingExceeded)?;
            if written.saturating_add(added) > ceiling {
                return Err(SaveStreamError::CeilingExceeded);
            }
            file.write_all(&chunk).await.map_err(SaveStreamError::Io)?;
            written += added;
        }
        file.flush().await.map_err(SaveStreamError::Io)?;
        file.sync_all().await.map_err(SaveStreamError::Io)?;
        Ok(written)
    }
    .await;
    match result {
        Ok(bytes) => {
            drop(file);
            Ok(CountedTemp { bytes, temp })
        }
        Err(error) => {
            drop(file);
            drop(temp);
            Err(error)
        }
    }
}

fn read_error(error: MultipartError) -> SaveStreamError {
    if is_length_limit(&error) {
        SaveStreamError::LengthLimited
    } else {
        SaveStreamError::Read
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Request};
    use tempfile::TempDir;

    use super::{SaveStreamError, stream_field};

    fn multipart_body(payload: &[u8]) -> (String, Vec<u8>) {
        let boundary = "save-stream-test";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(payload);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    async fn multipart_from(payload: &[u8]) -> Multipart {
        let (content_type, body) = multipart_body(payload);
        let request = Request::builder()
            .header("content-type", content_type)
            .body(Body::from(body))
            .expect("request");
        Multipart::from_request(request, &())
            .await
            .expect("multipart")
    }

    #[tokio::test]
    async fn exact_fit_succeeds() {
        let directory = TempDir::new_in("/var/tmp").expect("temp");
        let mut multipart = multipart_from(b"12345678").await;
        let field = multipart
            .next_field()
            .await
            .expect("field")
            .expect("present");
        let counted = stream_field(field, directory.path(), 8)
            .await
            .expect("stream");
        assert_eq!(counted.bytes(), 8);
        assert_eq!(std::fs::read(counted.path()).expect("read"), b"12345678");
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("dir")
                .flatten()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn one_byte_over_refuses_and_removes_temp() {
        let directory = TempDir::new_in("/var/tmp").expect("temp");
        let mut multipart = multipart_from(b"123456789").await;
        let field = multipart
            .next_field()
            .await
            .expect("field")
            .expect("present");
        let error = stream_field(field, directory.path(), 8)
            .await
            .expect_err("over");
        assert!(matches!(error, SaveStreamError::CeilingExceeded));
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("dir")
                .flatten()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn chunked_length_limit_maps_to_length_limited() {
        let (content_type, body) = multipart_body(&[b'x'; 80]);
        let mut request = Request::builder()
            .header("content-type", content_type)
            .body(Body::from(body))
            .expect("request");
        DefaultBodyLimit::max(16).apply(&mut request);
        let mut multipart = Multipart::from_request(request, &())
            .await
            .expect("multipart");
        let error = multipart
            .next_field()
            .await
            .expect_err("extractor length limit");
        assert!(super::is_length_limit(&error));
        assert!(matches!(
            super::read_error(error),
            SaveStreamError::LengthLimited
        ));
    }
}
