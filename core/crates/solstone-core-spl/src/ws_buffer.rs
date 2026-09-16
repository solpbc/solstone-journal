// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use thiserror::Error;
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WsClosed;

/// The narrow WebSocket read seam used by relay tunnel forwarding.
pub trait WsByteSource {
    fn next_message(&mut self) -> impl Future<Output = Result<Option<Bytes>, WsClosed>> + Send;
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum WsBufferError {
    #[error("websocket read exceeded its absolute deadline")]
    ReadTimeout,
    #[error("websocket closed before the requested bytes arrived")]
    Closed,
}

/// An unbounded byte reader with a bounded prefix peek for tunnel dispatch.
pub struct BufferedWsReader<S: WsByteSource> {
    source: S,
    buffer: BytesMut,
}

impl<S: WsByteSource> BufferedWsReader<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            buffer: BytesMut::new(),
        }
    }

    /// Return the next `n` bytes without consuming them.
    pub async fn peek(&mut self, n: usize) -> Result<Bytes, WsBufferError> {
        self.fill(n).await?;
        let bytes = self.buffer.get(..n).ok_or(WsBufferError::Closed)?;
        Ok(Bytes::copy_from_slice(bytes))
    }

    /// Return a dispatch prefix within one absolute deadline without consuming it.
    pub async fn peek_bounded(
        &mut self,
        n: usize,
        deadline: Duration,
    ) -> Result<Bytes, WsBufferError> {
        self.fill_bounded(n, deadline).await?;
        let bytes = self.buffer.get(..n).ok_or(WsBufferError::Closed)?;
        Ok(Bytes::copy_from_slice(bytes))
    }

    /// Consume and return exactly `n` bytes, spanning binary frames as needed.
    pub async fn read_exactly(&mut self, n: usize) -> Result<Bytes, WsBufferError> {
        self.fill(n).await?;
        Ok(self.buffer.split_to(n).freeze())
    }

    /// Return all residue already received from the WebSocket and clear it.
    pub fn drain_buffer(&mut self) -> Bytes {
        self.buffer.split().freeze()
    }

    /// Read and discard frames until the peer closes, for at most `deadline`.
    ///
    /// Call this after closing the write half. Dropping a socket that still has
    /// unread incoming data aborts it, and an abort can lose the last frames
    /// this side sent, such as a journal's refusal the dialer has not read yet.
    pub async fn drain_until_closed(&mut self, deadline: Duration) {
        self.buffer.clear();
        let _ = tokio::time::timeout(deadline, async {
            while let Ok(Some(_)) = self.source.next_message().await {}
        })
        .await;
    }

    async fn fill(&mut self, n: usize) -> Result<(), WsBufferError> {
        while self.buffer.len() < n {
            self.receive().await?;
        }
        Ok(())
    }

    async fn fill_bounded(&mut self, n: usize, deadline: Duration) -> Result<(), WsBufferError> {
        let deadline = Instant::now().checked_add(deadline);
        while self.buffer.len() < n {
            let frame = match wait_for_frame(&mut self.source, deadline).await {
                Ok(Ok(Some(frame))) => frame,
                Ok(Ok(None)) | Ok(Err(_)) => return Err(WsBufferError::Closed),
                Err(_) => return Err(WsBufferError::ReadTimeout),
            };
            self.buffer.extend_from_slice(&frame);
        }
        Ok(())
    }

    async fn receive(&mut self) -> Result<(), WsBufferError> {
        match self.source.next_message().await {
            Ok(Some(frame)) => {
                let is_empty = frame.is_empty();
                self.buffer.extend_from_slice(&frame);
                if is_empty {
                    tokio::task::yield_now().await;
                }
                Ok(())
            }
            Ok(None) | Err(_) => Err(WsBufferError::Closed),
        }
    }
}

async fn wait_for_frame<S: WsByteSource>(
    source: &mut S,
    deadline: Option<Instant>,
) -> Result<Result<Option<Bytes>, WsClosed>, tokio::time::error::Elapsed> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, source.next_message()).await,
        None => Ok(source.next_message().await),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct Frames(VecDeque<Bytes>);

    impl Frames {
        fn new(frames: &[&[u8]]) -> Self {
            Self(
                frames
                    .iter()
                    .map(|frame| Bytes::copy_from_slice(frame))
                    .collect(),
            )
        }
    }

    impl WsByteSource for Frames {
        fn next_message(&mut self) -> impl Future<Output = Result<Option<Bytes>, WsClosed>> + Send {
            std::future::ready(Ok(self.0.pop_front()))
        }
    }

    #[tokio::test]
    async fn split_tls_prefix_peeks_then_replays_the_same_wire_bytes() {
        const PREFIX: &[u8] = b"\x16\x03\x01\x00";
        let mut reader = BufferedWsReader::new(Frames::new(&[b"\x16\x03", b"\x01\x00body"]));

        assert_eq!(
            reader.peek(PREFIX.len()).await,
            Ok(Bytes::from_static(PREFIX))
        );
        assert_eq!(
            reader.drain_buffer(),
            Bytes::from_static(b"\x16\x03\x01\x00body")
        );
    }

    #[tokio::test]
    async fn closed_stream_is_not_a_short_success() {
        let mut reader = BufferedWsReader::new(Frames::new(&[b"ab"]));
        assert_eq!(reader.read_exactly(3).await, Err(WsBufferError::Closed));
    }

    #[tokio::test]
    async fn draining_reads_to_the_peers_close() {
        let mut reader = BufferedWsReader::new(Frames::new(&[b"upload", b"more upload"]));
        reader.drain_until_closed(Duration::from_secs(5)).await;
        assert!(reader.source.0.is_empty());
        assert!(reader.drain_buffer().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn draining_gives_up_at_its_deadline() {
        struct Stalled;
        impl WsByteSource for Stalled {
            fn next_message(
                &mut self,
            ) -> impl Future<Output = Result<Option<Bytes>, WsClosed>> + Send {
                std::future::pending()
            }
        }
        let mut reader = BufferedWsReader::new(Stalled);
        let started = Instant::now();
        reader.drain_until_closed(Duration::from_secs(2)).await;
        assert_eq!(started.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn bounded_prefix_times_out_when_no_frame_arrives() {
        struct Stalled;
        impl WsByteSource for Stalled {
            fn next_message(
                &mut self,
            ) -> impl Future<Output = Result<Option<Bytes>, WsClosed>> + Send {
                std::future::pending()
            }
        }
        let mut reader = BufferedWsReader::new(Stalled);
        assert_eq!(
            reader.peek_bounded(1, Duration::ZERO).await,
            Err(WsBufferError::ReadTimeout)
        );
    }
}
