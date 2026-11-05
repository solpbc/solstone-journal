// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Join-the-next-run request coalescing middleware.
//!
//! - GET requests share a run only when path, raw query (`None` distinct from empty,
//!   order-sensitive), `AccessBasis`, and the full value sequences of `If-None-Match`,
//!   `If-Modified-Since`, and `Range` match. No `AccessBasis` means the request runs
//!   alone and is never stored under a missing-extension key. HEAD and every non-GET
//!   run alone, including two HEADs with each other. `Accept-Encoding` is not a key.
//! - Join the next run only. A request that arrives while a run is in flight does not
//!   take that run's result. The pending set is taken under the mutex before the next
//!   handler runs, so an arrival racing the start cannot slip into the run that just
//!   began. At most one in-flight run plus one pending set. A waiter waits at most the
//!   remainder of the in-flight run plus one full run.
//! - `immediate`: this request arrived when nothing was in flight and started that run.
//!   `queued`: it waited, including the first registrant of the next run. Each waiter
//!   appends `Server-Timing` via `HeaderMap::append` (`coalesced;desc="immediate"` or
//!   `coalesced;desc="queued"`). Do not replace a `Server-Timing` value the handler set.
//!   A shared `dur`, when the handler set one, is the run's duration, not a queued
//!   waiter's wait.
//! - The run executes the opener request only. A handler that reads another header, or
//!   an extension that can differ between requests with the same key, has to add it to
//!   the key. The four production handlers do not read request headers. Speakers reads
//!   the shell journal extension, which the router sets the same way for every request.
//! - Buffer the body. Over 64 MiB, or a body read error, fails the whole run with
//!   `error_envelope` (not an empty 500, not a truncated 200). Reason codes:
//!   `coalesced_response_too_large`, `coalesced_response_unreadable`,
//!   `coalesced_handler_panicked`. Status 500. The 64 MiB cap sits next to the measured
//!   fixture maximum of 2708 bytes.
//! - The execution task holds no `TargetUse`. Each waiter's `run_admitted` spawn holds
//!   its own until that waiter is answered and the response body is dropped. A
//!   whole-journal restore still drains every in-flight run's waiters. Client loss does
//!   not drop a waiter: `run_admitted` spawns, and dropping the `JoinHandle` detaches
//!   rather than cancels.
//! - A client deadline below 2T on these routes can starve its own retries. Callers
//!   treat them as long reads (deadline at least 2T, or no retry while a request is
//!   outstanding).
//! - After waiters are answered the slot drops the body. Nothing is kept for the next
//!   request.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};
use std::task::{Context, Poll};

use axum::body::{Body, Bytes, HttpBody};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use tower::{Layer, Service};

const SERVER_TIMING: HeaderName = HeaderName::from_static("server-timing");

use crate::identity::AccessBasis;

pub const COALESCED_RESPONSE_BODY_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Immediate,
    Queued,
}

impl Disposition {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Queued => "queued",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinEvent {
    Registered(Disposition),
    Answered,
}

#[derive(Clone)]
pub struct JoinNotice(Arc<dyn Fn(JoinEvent) + Send + Sync>);

impl JoinNotice {
    pub fn new(notify: impl Fn(JoinEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(notify))
    }

    pub fn notify(&self, event: JoinEvent) {
        (self.0)(event);
    }
}

#[derive(Copy, Clone, Default)]
pub struct CoalesceLayer;

pub fn layer() -> CoalesceLayer {
    CoalesceLayer
}

impl<S> Layer<S> for CoalesceLayer {
    type Service = CoalesceService<S>;

    fn layer(&self, inner: S) -> CoalesceService<S> {
        CoalesceService { inner }
    }
}

#[derive(Clone)]
pub struct CoalesceService<S> {
    inner: S,
}

#[derive(Clone, PartialEq, Eq)]
struct CoalesceKey {
    path: String,
    query: Option<String>,
    basis: AccessBasis,
    if_none_match: Vec<Vec<u8>>,
    if_modified_since: Vec<Vec<u8>>,
    range: Vec<Vec<u8>>,
}

impl Hash for CoalesceKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.query.hash(state);
        hash_basis(&self.basis, state);
        self.if_none_match.hash(state);
        self.if_modified_since.hash(state);
        self.range.hash(state);
    }
}

fn hash_basis<H: Hasher>(basis: &AccessBasis, state: &mut H) {
    std::mem::discriminant(basis).hash(state);
    match basis {
        AccessBasis::Localhost => {}
        AccessBasis::LinkedDevice {
            carrier,
            cid,
            leaf_spki,
        } => {
            (*carrier as u8).hash(state);
            cid.as_str().hash(state);
            leaf_spki.hash(state);
        }
        AccessBasis::PairingPeer { carrier } => {
            (*carrier as u8).hash(state);
        }
    }
}

#[derive(Clone)]
struct SharedOutcome {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

struct Waiter {
    tx: tokio::sync::oneshot::Sender<Response<Body>>,
    notice: Option<JoinNotice>,
    disposition: Disposition,
}

struct PendingCohort {
    opener: Request<Body>,
    waiters: Vec<Waiter>,
}

struct Slot {
    pending: Option<PendingCohort>,
}

static REGISTRY: LazyLock<Mutex<HashMap<CoalesceKey, Slot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn extract_header_values(headers: &HeaderMap, name: HeaderName) -> Vec<Vec<u8>> {
    headers
        .get_all(name)
        .iter()
        .map(|v| v.as_bytes().to_vec())
        .collect()
}

fn error_outcome(reason_code: &'static str, message: &'static str) -> SharedOutcome {
    let envelope = crate::envelope::ErrorEnvelope {
        error: message.to_string(),
        reason_code: reason_code.to_string(),
        detail: String::new(),
    };
    let body_bytes = Bytes::from(serde_json::to_vec(&envelope).unwrap_or_default());
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    SharedOutcome {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        headers,
        body: body_bytes,
    }
}

fn panic_outcome() -> SharedOutcome {
    error_outcome("coalesced_handler_panicked", "coalesced handler panicked")
}

fn answer_waiters(waiters: &mut Vec<Waiter>, outcome: &SharedOutcome) {
    for waiter in waiters.drain(..) {
        if let Some(notice) = &waiter.notice {
            notice.notify(JoinEvent::Answered);
        }
        let mut headers = outcome.headers.clone();
        let timing_val = if waiter.disposition == Disposition::Immediate {
            HeaderValue::from_static("coalesced;desc=\"immediate\"")
        } else {
            HeaderValue::from_static("coalesced;desc=\"queued\"")
        };
        headers.append(SERVER_TIMING, timing_val);

        let mut response = Response::new(Body::from(outcome.body.clone()));
        *response.status_mut() = outcome.status;
        *response.headers_mut() = headers;

        let _ = waiter.tx.send(response);
    }
}

fn handoff<S>(key: &CoalesceKey, service: &S)
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: std::fmt::Display + Send + 'static,
{
    let next_cohort = {
        let mut reg = REGISTRY.lock().expect("registry lock");
        if let Some(slot) = reg.get_mut(key) {
            if let Some(cohort) = slot.pending.take() {
                Some(cohort)
            } else {
                reg.remove(key);
                None
            }
        } else {
            None
        }
    };
    if let Some(cohort) = next_cohort {
        spawn_execution(key.clone(), service.clone(), cohort.opener, cohort.waiters);
    }
}

struct TaskGuard<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: std::fmt::Display + Send + 'static,
{
    key: CoalesceKey,
    inner_service: S,
    waiters: Vec<Waiter>,
    disarmed: bool,
}

impl<S> Drop for TaskGuard<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: std::fmt::Display + Send + 'static,
{
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        let outcome = panic_outcome();
        answer_waiters(&mut self.waiters, &outcome);
        handoff(&self.key, &self.inner_service);
    }
}

async fn collect_response(response: Response<Body>) -> SharedOutcome {
    let (parts, mut body) = response.into_parts();
    let mut collected = Vec::new();

    loop {
        let frame_result = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;

        match frame_result {
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    if collected.len().saturating_add(data.len()) > COALESCED_RESPONSE_BODY_LIMIT {
                        drop(collected);
                        return error_outcome(
                            "coalesced_response_too_large",
                            "coalesced response too large",
                        );
                    }
                    collected.extend_from_slice(&data);
                }
            }
            Some(Err(_err)) => {
                drop(collected);
                return error_outcome(
                    "coalesced_response_unreadable",
                    "coalesced response unreadable",
                );
            }
            None => {
                break;
            }
        }
    }

    SharedOutcome {
        status: parts.status,
        headers: parts.headers,
        body: Bytes::from(collected),
    }
}

fn spawn_execution<S>(
    key: CoalesceKey,
    mut service: S,
    opener_req: Request<Body>,
    waiters: Vec<Waiter>,
) where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: std::fmt::Display + Send + 'static,
{
    tokio::spawn(async move {
        let mut guard = TaskGuard {
            key: key.clone(),
            inner_service: service.clone(),
            waiters,
            disarmed: false,
        };

        let outcome = match service.call(opener_req).await {
            Ok(response) => collect_response(response).await,
            Err(_) => panic_outcome(),
        };

        answer_waiters(&mut guard.waiters, &outcome);
        guard.disarmed = true;
        handoff(&key, &service);
    });
}

impl<S> Service<Request<Body>> for CoalesceService<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: std::fmt::Display + Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(
        &mut self,
        request: Request<Body>,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Response, S::Error>> + Send>> {
        let method = request.method().clone();
        let basis = request.extensions().get::<AccessBasis>().cloned();
        let notice = request.extensions().get::<JoinNotice>().cloned();

        // Bypass: non-GET or no AccessBasis
        if method != Method::GET || basis.is_none() {
            let mut inner = self.inner.clone();
            return Box::pin(async move {
                if let Some(n) = &notice {
                    n.notify(JoinEvent::Registered(Disposition::Immediate));
                }
                let mut response = inner.call(request).await?;
                response.headers_mut().append(
                    SERVER_TIMING,
                    HeaderValue::from_static("coalesced;desc=\"immediate\""),
                );
                if let Some(n) = &notice {
                    n.notify(JoinEvent::Answered);
                }
                Ok(response)
            });
        }

        let basis = basis.expect("checked is_some");
        let path = request.uri().path().to_string();
        let query = request.uri().query().map(|q| q.to_string());
        let if_none_match = extract_header_values(request.headers(), header::IF_NONE_MATCH);
        let if_modified_since = extract_header_values(request.headers(), header::IF_MODIFIED_SINCE);
        let range = extract_header_values(request.headers(), header::RANGE);

        let key = CoalesceKey {
            path,
            query,
            basis,
            if_none_match,
            if_modified_since,
            range,
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        let inner = self.inner.clone();

        let (disposition, opener) = {
            let mut reg = REGISTRY.lock().expect("registry lock");
            if let Some(slot) = reg.get_mut(&key) {
                let waiter = Waiter {
                    tx,
                    notice: notice.clone(),
                    disposition: Disposition::Queued,
                };
                if let Some(cohort) = &mut slot.pending {
                    cohort.waiters.push(waiter);
                } else {
                    slot.pending = Some(PendingCohort {
                        opener: request,
                        waiters: vec![waiter],
                    });
                }
                (Disposition::Queued, None)
            } else {
                let waiter = Waiter {
                    tx,
                    notice: notice.clone(),
                    disposition: Disposition::Immediate,
                };
                reg.insert(key.clone(), Slot { pending: None });
                (Disposition::Immediate, Some((waiter, request)))
            }
        };

        // Notice called only after mutex is released
        if let Some(n) = &notice {
            n.notify(JoinEvent::Registered(disposition));
        }

        if let Some((waiter, opener_req)) = opener {
            spawn_execution(key, inner, opener_req, vec![waiter]);
        }

        Box::pin(async move {
            match rx.await {
                Ok(response) => Ok(response),
                Err(_) => {
                    let outcome = panic_outcome();
                    let mut headers = outcome.headers.clone();
                    headers.append(
                        SERVER_TIMING,
                        HeaderValue::from_static("coalesced;desc=\"queued\""),
                    );
                    let mut response = Response::new(Body::from(outcome.body));
                    *response.status_mut() = outcome.status;
                    *response.headers_mut() = headers;
                    Ok(response)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::body::{Body, Bytes, to_bytes};
    use axum::extract::Query;
    use axum::http::header;
    use axum::http::{Method, Request, StatusCode};
    use axum::routing::get;
    use serde_json::Value;
    use tokio::sync::{Notify, oneshot};
    use tower::ServiceExt;

    use super::*;
    use crate::identity::{AccessBasis, Carrier};

    // --- Minimal RFC 1952 / RFC 1951 Gzip / Deflate decompressor for test 6 ---

    struct BitReader<'a> {
        data: &'a [u8],
        byte_pos: usize,
        bit_buf: u64,
        bit_cnt: u8,
    }

    impl<'a> BitReader<'a> {
        fn new(data: &'a [u8]) -> Self {
            Self {
                data,
                byte_pos: 0,
                bit_buf: 0,
                bit_cnt: 0,
            }
        }

        fn read_bits(&mut self, n: u8) -> u32 {
            while self.bit_cnt < n {
                let b = if self.byte_pos < self.data.len() {
                    let v = self.data[self.byte_pos];
                    self.byte_pos += 1;
                    v as u64
                } else {
                    0
                };
                self.bit_buf |= b << self.bit_cnt;
                self.bit_cnt += 8;
            }
            let mask = if n == 32 {
                u32::MAX as u64
            } else {
                (1u64 << n) - 1
            };
            let res = (self.bit_buf & mask) as u32;
            self.bit_buf >>= n;
            self.bit_cnt -= n;
            res
        }

        fn align_byte(&mut self) {
            let rem = self.bit_cnt % 8;
            if rem > 0 {
                self.read_bits(rem);
            }
            self.bit_buf = 0;
            self.bit_cnt = 0;
        }

        fn read_u16_le(&mut self) -> u16 {
            self.align_byte();
            let b0 = self.data[self.byte_pos] as u16;
            let b1 = self.data[self.byte_pos + 1] as u16;
            self.byte_pos += 2;
            b0 | (b1 << 8)
        }

        fn read_bytes(&mut self, n: usize) -> Vec<u8> {
            self.align_byte();
            let slice = &self.data[self.byte_pos..self.byte_pos + n];
            self.byte_pos += n;
            slice.to_vec()
        }
    }

    struct HuffmanTree {
        nodes: Vec<[i16; 2]>,
    }

    fn build_huffman_tree(lengths: &[u8]) -> HuffmanTree {
        let mut tree = HuffmanTree {
            nodes: vec![[-1, -1]],
        };
        let mut max_len = 0;
        for &l in lengths {
            if l > max_len {
                max_len = l;
            }
        }
        if max_len == 0 {
            return tree;
        }
        let mut bl_count = vec![0u16; (max_len + 1) as usize];
        for &l in lengths {
            if l > 0 {
                bl_count[l as usize] += 1;
            }
        }
        let mut next_code = vec![0u16; (max_len + 1) as usize];
        let mut code = 0u16;
        for bits in 1..=max_len as usize {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }
        for (sym, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let c = next_code[len as usize];
            next_code[len as usize] += 1;
            let mut curr = 0;
            for bit_idx in (0..len).rev() {
                let bit = ((c >> bit_idx) & 1) as usize;
                if bit_idx == 0 {
                    tree.nodes[curr][bit] = -(sym as i16 + 1);
                } else {
                    let next = tree.nodes[curr][bit];
                    if next == -1 {
                        let new_node = tree.nodes.len() as i16;
                        tree.nodes.push([-1, -1]);
                        tree.nodes[curr][bit] = new_node;
                        curr = new_node as usize;
                    } else {
                        curr = next as usize;
                    }
                }
            }
        }
        tree
    }

    fn decode_symbol(reader: &mut BitReader<'_>, tree: &HuffmanTree) -> u16 {
        let mut curr = 0;
        loop {
            let bit = reader.read_bits(1) as usize;
            let next = tree.nodes[curr][bit];
            if next < 0 {
                return (-next - 1) as u16;
            }
            curr = next as usize;
        }
    }

    fn fixed_lit_tree() -> HuffmanTree {
        let mut lens = vec![0u8; 288];
        lens[..144].fill(8);
        lens[144..256].fill(9);
        lens[256..280].fill(7);
        lens[280..288].fill(8);
        build_huffman_tree(&lens)
    }

    fn fixed_dist_tree() -> HuffmanTree {
        let lens = vec![5u8; 32];
        build_huffman_tree(&lens)
    }

    const LEN_BASE: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const LEN_EXTRA: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    const DIST_BASE: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const DIST_EXTRA: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];
    const CL_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];

    fn decompress_gzip(input: &[u8]) -> Vec<u8> {
        assert!(input.len() >= 18, "gzip payload too short");
        assert_eq!(input[0], 0x1f);
        assert_eq!(input[1], 0x8b);
        assert_eq!(input[2], 8);
        let flg = input[3];
        let mut pos = 10;
        if flg & 4 != 0 {
            let xlen = u16::from_le_bytes([input[pos], input[pos + 1]]) as usize;
            pos += 2 + xlen;
        }
        if flg & 8 != 0 {
            while pos < input.len() && input[pos] != 0 {
                pos += 1;
            }
            pos += 1;
        }
        if flg & 16 != 0 {
            while pos < input.len() && input[pos] != 0 {
                pos += 1;
            }
            pos += 1;
        }
        if flg & 2 != 0 {
            pos += 2;
        }
        let deflate_bytes = &input[pos..input.len() - 8];
        let mut reader = BitReader::new(deflate_bytes);
        let mut output = Vec::new();

        loop {
            let bfinal = reader.read_bits(1);
            let btype = reader.read_bits(2);
            match btype {
                0 => {
                    let len = reader.read_u16_le() as usize;
                    let _nlen = reader.read_u16_le();
                    output.extend_from_slice(&reader.read_bytes(len));
                }
                1 => {
                    let lit_tree = fixed_lit_tree();
                    let dist_tree = fixed_dist_tree();
                    loop {
                        let sym = decode_symbol(&mut reader, &lit_tree);
                        if sym < 256 {
                            output.push(sym as u8);
                        } else if sym == 256 {
                            break;
                        } else {
                            let idx = (sym - 257) as usize;
                            let len = LEN_BASE[idx] + reader.read_bits(LEN_EXTRA[idx]) as u16;
                            let dist_sym = decode_symbol(&mut reader, &dist_tree) as usize;
                            let dist =
                                DIST_BASE[dist_sym] + reader.read_bits(DIST_EXTRA[dist_sym]) as u16;
                            for _ in 0..len {
                                let b = output[output.len() - dist as usize];
                                output.push(b);
                            }
                        }
                    }
                }
                2 => {
                    let hlit = reader.read_bits(5) + 257;
                    let hdist = reader.read_bits(5) + 1;
                    let hclen = reader.read_bits(4) + 4;
                    let mut cl_lens = [0u8; 19];
                    for i in 0..hclen {
                        cl_lens[CL_ORDER[i as usize]] = reader.read_bits(3) as u8;
                    }
                    let cl_tree = build_huffman_tree(&cl_lens);
                    let mut all_lens = Vec::with_capacity((hlit + hdist) as usize);
                    while all_lens.len() < (hlit + hdist) as usize {
                        let sym = decode_symbol(&mut reader, &cl_tree);
                        match sym {
                            0..=15 => all_lens.push(sym as u8),
                            16 => {
                                let rep = reader.read_bits(2) + 3;
                                let last = *all_lens.last().expect("last symbol");
                                for _ in 0..rep {
                                    all_lens.push(last);
                                }
                            }
                            17 => {
                                let rep = reader.read_bits(3) + 3;
                                all_lens.resize(all_lens.len() + rep as usize, 0);
                            }
                            18 => {
                                let rep = reader.read_bits(7) + 11;
                                all_lens.resize(all_lens.len() + rep as usize, 0);
                            }
                            _ => unreachable!(),
                        }
                    }
                    let lit_tree = build_huffman_tree(&all_lens[..hlit as usize]);
                    let dist_tree = build_huffman_tree(&all_lens[hlit as usize..]);
                    loop {
                        let sym = decode_symbol(&mut reader, &lit_tree);
                        if sym < 256 {
                            output.push(sym as u8);
                        } else if sym == 256 {
                            break;
                        } else {
                            let idx = (sym - 257) as usize;
                            let len = LEN_BASE[idx] + reader.read_bits(LEN_EXTRA[idx]) as u16;
                            let dist_sym = decode_symbol(&mut reader, &dist_tree) as usize;
                            let dist =
                                DIST_BASE[dist_sym] + reader.read_bits(DIST_EXTRA[dist_sym]) as u16;
                            for _ in 0..len {
                                let b = output[output.len() - dist as usize];
                                output.push(b);
                            }
                        }
                    }
                }
                _ => panic!("invalid deflate block type: {btype}"),
            }
            if bfinal == 1 {
                break;
            }
        }
        output
    }

    // --- Tests 1..=8 ---

    #[tokio::test]
    async fn coalescing_two_executions_divides_burst_into_immediate_and_queued() {
        let executions = Arc::new(AtomicUsize::new(0));
        let exec_clone = executions.clone();
        let (run1_entered_tx, run1_entered_rx) = oneshot::channel();
        let run1_entered_tx = Arc::new(Mutex::new(Some(run1_entered_tx)));
        let (run1_release_tx, run1_release_rx) = oneshot::channel();
        let run1_release_rx = Arc::new(tokio::sync::Mutex::new(Some(run1_release_rx)));

        let app = axum::Router::new().route(
            "/test/burst",
            get(move || {
                let executions = exec_clone.clone();
                let run1_entered_tx = run1_entered_tx.clone();
                let run1_release_rx = run1_release_rx.clone();
                async move {
                    let id = executions.fetch_add(1, Ordering::SeqCst) + 1;
                    if id == 1 {
                        if let Some(tx) = run1_entered_tx.lock().unwrap().take() {
                            let _ = tx.send(());
                        }
                        let rx = run1_release_rx.lock().await.take().unwrap();
                        let _ = rx.await;
                    }
                    format!("body-{id}")
                }
            })
            .layer(layer()),
        );

        // Request 1 arrives and holds execution 1
        let (req1_reg_tx, req1_reg_rx) = oneshot::channel();
        let req1_reg_tx = Arc::new(Mutex::new(Some(req1_reg_tx)));
        let app1 = app.clone();
        let task1 = tokio::spawn(async move {
            let req = Request::get("/test/burst")
                .extension(AccessBasis::Localhost)
                .extension(JoinNotice::new(move |event| {
                    if let JoinEvent::Registered(d) = event
                        && let Some(tx) = req1_reg_tx.lock().unwrap().take()
                    {
                        let _ = tx.send(d);
                    }
                }))
                .body(Body::empty())
                .unwrap();
            app1.oneshot(req).await.unwrap()
        });

        // Wait until run 1 has registered and entered handler
        assert_eq!(req1_reg_rx.await.unwrap(), Disposition::Immediate);
        run1_entered_rx.await.unwrap();

        // 9 requests arrive while run 1 is held
        let queued_registrations = Arc::new(AtomicUsize::new(0));
        let all_queued_notify = Arc::new(Notify::new());
        let mut tasks = Vec::new();

        for _ in 0..9 {
            let app_c = app.clone();
            let q_reg = queued_registrations.clone();
            let q_not = all_queued_notify.clone();
            let task = tokio::spawn(async move {
                let req = Request::get("/test/burst")
                    .extension(AccessBasis::Localhost)
                    .extension(JoinNotice::new(move |event| {
                        if let JoinEvent::Registered(Disposition::Queued) = event
                            && q_reg.fetch_add(1, Ordering::SeqCst) + 1 == 9
                        {
                            q_not.notify_one();
                        }
                    }))
                    .body(Body::empty())
                    .unwrap();
                app_c.oneshot(req).await.unwrap()
            });
            tasks.push(task);
        }

        // Wait until all 9 are registered as queued
        all_queued_notify.notified().await;

        // Release run 1
        run1_release_tx.send(()).unwrap();

        let resp1 = task1.await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        assert_eq!(
            resp1.headers()[SERVER_TIMING],
            "coalesced;desc=\"immediate\""
        );
        let body1 = to_bytes(resp1.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body1, "body-1");

        for task in tasks {
            let resp = task.await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(resp.headers()[SERVER_TIMING], "coalesced;desc=\"queued\"");
            let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            assert_eq!(body, "body-2");
        }

        assert_eq!(executions.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn coalescing_second_generation_does_not_leak_into_started_run() {
        let generation = Arc::new(AtomicUsize::new(1));
        let gen_clone = generation.clone();
        let e2_entered = Arc::new(Notify::new());
        let e2_release = Arc::new(Notify::new());
        let run_count = Arc::new(AtomicUsize::new(0));

        let e2_entered_c = e2_entered.clone();
        let e2_release_c = e2_release.clone();

        let (e1_notified_tx, e1_notified_rx) = oneshot::channel();
        let e1_notified_tx = Arc::new(Mutex::new(Some(e1_notified_tx)));
        let (e1_unblock_tx, e1_unblock_rx) = oneshot::channel();
        let e1_unblock_rx = Arc::new(tokio::sync::Mutex::new(Some(e1_unblock_rx)));

        let app_with_gate = axum::Router::new().route(
            "/test/generation_gated",
            get(move || {
                let e1_notified_tx = e1_notified_tx.clone();
                let e1_unblock_rx = e1_unblock_rx.clone();
                async move {
                    if let Some(tx) = e1_notified_tx.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    if let Some(rx) = e1_unblock_rx.lock().await.take() {
                        let _ = rx.await;
                    }
                    "e1-done"
                }
            })
            .layer(layer()),
        );

        // R1 starts E1 and is held
        let task1 = tokio::spawn({
            let app = app_with_gate.clone();
            async move {
                app.oneshot(
                    Request::get("/test/generation_gated")
                        .extension(AccessBasis::Localhost)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        e1_notified_rx.await.unwrap();

        // R2 arrives and queues behind E1
        let (r2_reg_tx, r2_reg_rx) = oneshot::channel();
        let r2_reg_tx = Arc::new(Mutex::new(Some(r2_reg_tx)));
        let task2 = tokio::spawn({
            let app = app_with_gate.clone();
            async move {
                app.oneshot(
                    Request::get("/test/generation_gated")
                        .extension(AccessBasis::Localhost)
                        .extension(JoinNotice::new(move |event| {
                            if let JoinEvent::Registered(Disposition::Queued) = event
                                && let Some(tx) = r2_reg_tx.lock().unwrap().take()
                            {
                                let _ = tx.send(());
                            }
                        }))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        r2_reg_rx.await.unwrap();

        // Release E1
        e1_unblock_tx.send(()).unwrap();
        task1.await.unwrap();
        task2.await.unwrap();

        // Now test the generation leak on `/test/generation`
        // We'll use two requests to hold E2 after it reads generation
        let (hold_e1_tx, hold_e1_rx) = oneshot::channel();
        let hold_e1_tx = Arc::new(Mutex::new(Some(hold_e1_tx)));
        let (unblock_e1_tx, unblock_e1_rx) = oneshot::channel();
        let unblock_e1_rx = Arc::new(tokio::sync::Mutex::new(Some(unblock_e1_rx)));

        let app_gen = axum::Router::new().route(
            "/test/gen_leak",
            get(move || {
                let generation_var = generation.clone();
                let run_count = run_count.clone();
                let hold_e1_tx = hold_e1_tx.clone();
                let unblock_e1_rx = unblock_e1_rx.clone();
                let e2_entered = e2_entered.clone();
                let e2_release = e2_release.clone();
                async move {
                    let run = run_count.fetch_add(1, Ordering::SeqCst) + 1;
                    let g = generation_var.load(Ordering::SeqCst);
                    if run == 1 {
                        if let Some(tx) = hold_e1_tx.lock().unwrap().take() {
                            let _ = tx.send(());
                        }
                        if let Some(rx) = unblock_e1_rx.lock().await.take() {
                            let _ = rx.await;
                        }
                    } else if run == 2 {
                        e2_entered.notify_one();
                        e2_release.notified().await;
                    }
                    format!("gen-{g}")
                }
            })
            .layer(layer()),
        );

        // R1 starts E1 and holds it
        let task_r1 = tokio::spawn({
            let app = app_gen.clone();
            async move {
                app.oneshot(
                    Request::get("/test/gen_leak")
                        .extension(AccessBasis::Localhost)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        hold_e1_rx.await.unwrap();

        // R2 queues behind E1
        let (r2_queued_tx, r2_queued_rx) = oneshot::channel();
        let r2_queued_tx = Arc::new(Mutex::new(Some(r2_queued_tx)));
        let task_r2 = tokio::spawn({
            let app = app_gen.clone();
            async move {
                app.oneshot(
                    Request::get("/test/gen_leak")
                        .extension(AccessBasis::Localhost)
                        .extension(JoinNotice::new(move |event| {
                            if let JoinEvent::Registered(Disposition::Queued) = event
                                && let Some(tx) = r2_queued_tx.lock().unwrap().take()
                            {
                                let _ = tx.send(());
                            }
                        }))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        r2_queued_rx.await.unwrap();

        // Release E1; E2 will start and read generation 1, then notify e2_entered
        unblock_e1_tx.send(()).unwrap();
        task_r1.await.unwrap();

        e2_entered_c.notified().await;

        // Bump generation to 2
        gen_clone.store(2, Ordering::SeqCst);

        // Issue R3 after E2 has read generation 1 and is held
        let (r3_queued_tx, r3_queued_rx) = oneshot::channel();
        let r3_queued_tx = Arc::new(Mutex::new(Some(r3_queued_tx)));
        let task_r3 = tokio::spawn({
            let app = app_gen.clone();
            async move {
                app.oneshot(
                    Request::get("/test/gen_leak")
                        .extension(AccessBasis::Localhost)
                        .extension(JoinNotice::new(move |event| {
                            if let JoinEvent::Registered(Disposition::Queued) = event
                                && let Some(tx) = r3_queued_tx.lock().unwrap().take()
                            {
                                let _ = tx.send(());
                            }
                        }))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        r3_queued_rx.await.unwrap();

        // Release E2
        e2_release_c.notify_one();

        let resp_r2 = task_r2.await.unwrap();
        let body_r2 = to_bytes(resp_r2.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body_r2, "gen-1");

        let resp_r3 = task_r3.await.unwrap();
        let body_r3 = to_bytes(resp_r3.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body_r3, "gen-2");
    }

    #[tokio::test]
    async fn coalescing_panicking_handler_clears_slot_and_answers_waiters_with_envelope() {
        let executions = Arc::new(AtomicUsize::new(0));
        let exec_c = executions.clone();
        let run1_entered = Arc::new(Notify::new());
        let run1_release = Arc::new(Notify::new());

        let r1_ent_c = run1_entered.clone();
        let r1_rel_c = run1_release.clone();

        let app = axum::Router::new().route(
            "/test/panic",
            get(move || {
                let exec = exec_c.clone();
                let run1_entered = r1_ent_c.clone();
                let run1_release = r1_rel_c.clone();
                async move {
                    let id = exec.fetch_add(1, Ordering::SeqCst) + 1;
                    if id == 1 {
                        run1_entered.notify_one();
                        run1_release.notified().await;
                        format!("success-{id}")
                    } else if id == 2 {
                        panic!("simulated handler panic");
                    } else {
                        format!("success-{id}")
                    }
                }
            })
            .layer(layer()),
        );

        // Run 1 starts and is held
        let task1 = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get("/test/panic")
                        .extension(AccessBasis::Localhost)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        run1_entered.notified().await;

        // While run 1 is held, two waiters queue behind it to form run 2 with >1 waiter
        let (w2_queued_tx, w2_queued_rx) = oneshot::channel();
        let w2_queued_tx = Arc::new(Mutex::new(Some(w2_queued_tx)));
        let task2 = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get("/test/panic")
                        .extension(AccessBasis::Localhost)
                        .extension(JoinNotice::new(move |event| {
                            if let JoinEvent::Registered(Disposition::Queued) = event
                                && let Some(tx) = w2_queued_tx.lock().unwrap().take()
                            {
                                let _ = tx.send(());
                            }
                        }))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        w2_queued_rx.await.unwrap();

        let (w3_queued_tx, w3_queued_rx) = oneshot::channel();
        let w3_queued_tx = Arc::new(Mutex::new(Some(w3_queued_tx)));
        let task3 = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get("/test/panic")
                        .extension(AccessBasis::Localhost)
                        .extension(JoinNotice::new(move |event| {
                            if let JoinEvent::Registered(Disposition::Queued) = event
                                && let Some(tx) = w3_queued_tx.lock().unwrap().take()
                            {
                                let _ = tx.send(());
                            }
                        }))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        w3_queued_rx.await.unwrap();

        // Release run 1; run 2 (with task2 and task3) will execute and panic
        run1_release.notify_one();

        let resp1 = task1.await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);

        let resp2 = task2.await.unwrap();
        let resp3 = task3.await.unwrap();

        assert_eq!(resp2.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(resp3.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body2 = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
        let body3 = to_bytes(resp3.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body2, body3);

        let json2: Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["reason_code"], "coalesced_handler_panicked");

        // Next identical request is immediate and new execution
        let resp4 = app
            .oneshot(
                Request::get("/test/panic")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp4.status(), StatusCode::OK);
        assert_eq!(
            resp4.headers()[SERVER_TIMING],
            "coalesced;desc=\"immediate\""
        );
        let body4 = to_bytes(resp4.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body4, "success-3");
    }

    #[tokio::test]
    async fn coalescing_dropped_opener_still_answers_the_cohort() {
        let executions = Arc::new(AtomicUsize::new(0));
        let exec_c = executions.clone();
        let e1_entered = Arc::new(Notify::new());
        let e1_release = Arc::new(Notify::new());
        let e2_entered = Arc::new(Notify::new());
        let e2_release = Arc::new(Notify::new());

        let e1_ent_c = e1_entered.clone();
        let e1_rel_c = e1_release.clone();
        let e2_ent_c = e2_entered.clone();
        let e2_rel_c = e2_release.clone();

        let app = axum::Router::new().route(
            "/test/drop_opener",
            get(move || {
                let exec = exec_c.clone();
                let e1_ent = e1_ent_c.clone();
                let e1_rel = e1_rel_c.clone();
                let e2_ent = e2_ent_c.clone();
                let e2_rel = e2_rel_c.clone();
                async move {
                    let id = exec.fetch_add(1, Ordering::SeqCst) + 1;
                    if id == 1 {
                        e1_ent.notify_one();
                        e1_rel.notified().await;
                    } else if id == 2 {
                        e2_ent.notify_one();
                        e2_rel.notified().await;
                    }
                    format!("success-{id}")
                }
            })
            .layer(layer()),
        );

        // Run 1 starts and is held
        let task_e1 = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get("/test/drop_opener")
                        .extension(AccessBasis::Localhost)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        e1_entered.notified().await;

        // E2's cohort: opener (r2_opener) and waiters (r2_waiter1, r2_waiter2)
        let (r2_opener_reg_tx, r2_opener_reg_rx) = oneshot::channel();
        let r2_opener_reg_tx = Arc::new(Mutex::new(Some(r2_opener_reg_tx)));
        let task_r2_opener = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get("/test/drop_opener")
                        .extension(AccessBasis::Localhost)
                        .extension(JoinNotice::new(move |event| {
                            if let JoinEvent::Registered(Disposition::Queued) = event
                                && let Some(tx) = r2_opener_reg_tx.lock().unwrap().take()
                            {
                                let _ = tx.send(());
                            }
                        }))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        r2_opener_reg_rx.await.unwrap();

        let (r2_w1_reg_tx, r2_w1_reg_rx) = oneshot::channel();
        let r2_w1_reg_tx = Arc::new(Mutex::new(Some(r2_w1_reg_tx)));
        let task_r2_w1 = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get("/test/drop_opener")
                        .extension(AccessBasis::Localhost)
                        .extension(JoinNotice::new(move |event| {
                            if let JoinEvent::Registered(Disposition::Queued) = event
                                && let Some(tx) = r2_w1_reg_tx.lock().unwrap().take()
                            {
                                let _ = tx.send(());
                            }
                        }))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        });
        r2_w1_reg_rx.await.unwrap();

        // Release E1; E2 will start and enter handler
        e1_release.notify_one();
        task_e1.await.unwrap();

        e2_entered.notified().await;

        // Abort / drop the opener's client future while E2 is held
        task_r2_opener.abort();

        // Release E2
        e2_release.notify_one();

        // The other waiter still receives shared success body
        let resp_w1 = task_r2_w1.await.unwrap();
        assert_eq!(resp_w1.status(), StatusCode::OK);
        let body_w1 = to_bytes(resp_w1.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body_w1, "success-2");

        // Next identical request is immediate and new execution
        let resp3 = app
            .oneshot(
                Request::get("/test/drop_opener")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp3.status(), StatusCode::OK);
        assert_eq!(
            resp3.headers()[SERVER_TIMING],
            "coalesced;desc=\"immediate\""
        );
        let body3 = to_bytes(resp3.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body3, "success-3");
    }

    #[tokio::test]
    async fn coalescing_key_distinguishes_query_basis_headers_and_method() {
        let executions = Arc::new(AtomicUsize::new(0));
        let exec_c = executions.clone();

        let app = axum::Router::new().route(
            "/test/key",
            get({
                let exec = exec_c.clone();
                move |Query(params): Query<HashMap<String, String>>, req: Request<Body>| {
                    let exec = exec.clone();
                    async move {
                        exec.fetch_add(1, Ordering::SeqCst);
                        let if_none_match = req
                            .headers()
                            .get(header::IF_NONE_MATCH)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("none")
                            .to_string();
                        let q = params.get("q").cloned().unwrap_or_default();
                        format!("q={q};inm={if_none_match}")
                    }
                }
            })
            .head({
                let exec = exec_c.clone();
                move || {
                    let exec = exec.clone();
                    async move {
                        exec.fetch_add(1, Ordering::SeqCst);
                        "head"
                    }
                }
            })
            .layer(layer()),
        );

        // 1. Differing query
        let r1 = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=a")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let r2 = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=b")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            to_bytes(r1.into_body(), usize::MAX).await.unwrap(),
            to_bytes(r2.into_body(), usize::MAX).await.unwrap()
        );

        // 2. Differing basis
        let r_lh = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=basis")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let r_peer = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=basis")
                    .extension(AccessBasis::PairingPeer {
                        carrier: Carrier::Direct,
                    })
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r_lh.status(), StatusCode::OK);
        assert_eq!(r_peer.status(), StatusCode::OK);

        // 3. Differing If-None-Match vs matching If-None-Match
        let r_inm1 = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=tag")
                    .header(header::IF_NONE_MATCH, "\"tag1\"")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let r_inm2 = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=tag")
                    .header(header::IF_NONE_MATCH, "\"tag2\"")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            to_bytes(r_inm1.into_body(), usize::MAX).await.unwrap(),
            to_bytes(r_inm2.into_body(), usize::MAX).await.unwrap()
        );

        // 4. Differing Range vs If-Modified-Since
        let r_range = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=headers")
                    .header(header::RANGE, "bytes=0-100")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let r_ims = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=headers")
                    .header(header::IF_MODIFIED_SINCE, "Wed, 21 Oct 2026 07:28:00 GMT")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r_range.status(), StatusCode::OK);
        assert_eq!(r_ims.status(), StatusCode::OK);

        // 5. Two requests with no AccessBasis are two executions
        let count_before = executions.load(Ordering::SeqCst);
        let _ = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=nobasis")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let _ = app
            .clone()
            .oneshot(
                Request::get("/test/key?q=nobasis")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), count_before + 2);

        // 6. HEAD does not share with GET or with another HEAD
        let count_before = executions.load(Ordering::SeqCst);
        let head1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/test/key?q=head")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let head2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/test/key?q=head")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), count_before + 2);
        assert_eq!(
            head1.headers()[SERVER_TIMING],
            "coalesced;desc=\"immediate\""
        );
        assert_eq!(
            head2.headers()[SERVER_TIMING],
            "coalesced;desc=\"immediate\""
        );
    }

    #[tokio::test]
    async fn coalescing_accept_encoding_coexists_with_outer_compression() {
        let large_payload = serde_json::json!({
            "items": (0..150).map(|i| format!("item-payload-string-padding-{i}")).collect::<Vec<_>>()
        });
        let payload_str = serde_json::to_string(&large_payload).unwrap();
        assert!(payload_str.len() > 1024);

        let executions = Arc::new(AtomicUsize::new(0));
        let exec_c = executions.clone();
        let run1_held = Arc::new(Notify::new());
        let run1_release = Arc::new(Notify::new());
        let r1_held_c = run1_held.clone();
        let r1_rel_c = run1_release.clone();
        let p_str = payload_str.clone();

        let inner_router = axum::Router::new().route(
            "/test/compression",
            get(move || {
                let exec = exec_c.clone();
                let r1_held = r1_held_c.clone();
                let r1_rel = r1_rel_c.clone();
                let p_str = p_str.clone();
                async move {
                    let id = exec.fetch_add(1, Ordering::SeqCst) + 1;
                    if id == 1 {
                        r1_held.notify_one();
                        r1_rel.notified().await;
                    }
                    ([(header::CONTENT_TYPE, "application/json")], p_str)
                }
            })
            .layer(layer()),
        );

        // CompressionLayer outside the coalescer
        let app = inner_router.layer(crate::serve::compression_layer());

        // Request 0 starts run 1 and is held
        let app0 = app.clone();
        let task0 = tokio::spawn(async move {
            let req = Request::get("/test/compression")
                .extension(AccessBasis::Localhost)
                .body(Body::empty())
                .unwrap();
            app0.oneshot(req).await.unwrap()
        });

        run1_held.notified().await;

        // While run 1 is held:
        // Request 1 with Accept-Encoding: gzip (queued in run 2)
        let (q1_reg_tx, q1_reg_rx) = oneshot::channel();
        let q1_reg_tx = Arc::new(Mutex::new(Some(q1_reg_tx)));
        let app1 = app.clone();
        let task1 = tokio::spawn(async move {
            let req = Request::get("/test/compression")
                .header(header::ACCEPT_ENCODING, "gzip")
                .extension(AccessBasis::Localhost)
                .extension(JoinNotice::new(move |event| {
                    if let JoinEvent::Registered(Disposition::Queued) = event
                        && let Some(tx) = q1_reg_tx.lock().unwrap().take()
                    {
                        let _ = tx.send(());
                    }
                }))
                .body(Body::empty())
                .unwrap();
            app1.oneshot(req).await.unwrap()
        });
        q1_reg_rx.await.unwrap();

        // Request 2: identity request (queued in run 2, same coalesced group)
        let (q2_reg_tx, q2_reg_rx) = oneshot::channel();
        let q2_reg_tx = Arc::new(Mutex::new(Some(q2_reg_tx)));
        let app2 = app.clone();
        let task2 = tokio::spawn(async move {
            let req = Request::get("/test/compression")
                .extension(AccessBasis::Localhost)
                .extension(JoinNotice::new(move |event| {
                    if let JoinEvent::Registered(Disposition::Queued) = event
                        && let Some(tx) = q2_reg_tx.lock().unwrap().take()
                    {
                        let _ = tx.send(());
                    }
                }))
                .body(Body::empty())
                .unwrap();
            app2.oneshot(req).await.unwrap()
        });
        q2_reg_rx.await.unwrap();

        // Release run 1. Both task1 and task2 execute together in run 2
        run1_release.notify_one();

        let _ = task0.await.unwrap();
        let resp1 = task1.await.unwrap();
        let resp2 = task2.await.unwrap();

        assert_eq!(resp1.status(), StatusCode::OK);
        assert_eq!(resp2.status(), StatusCode::OK);

        // Gzip response has Content-Encoding: gzip
        assert_eq!(resp1.headers()[header::CONTENT_ENCODING], "gzip");
        // Identity response has no Content-Encoding
        assert!(!resp2.headers().contains_key(header::CONTENT_ENCODING));

        let body1_raw = to_bytes(resp1.into_body(), usize::MAX).await.unwrap();
        let body2_raw = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();

        // Decoded bodies byte-identical
        let decompressed1 = decompress_gzip(&body1_raw);
        assert_eq!(decompressed1, body2_raw.as_ref());
        assert_eq!(body2_raw, payload_str.as_bytes());
        assert_eq!(executions.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn coalescing_server_timing_coexists_with_handler_header() {
        let app = axum::Router::new().route(
            "/test/timing",
            get(|| async { ([(SERVER_TIMING, "dur=42")], "ok") }).layer(layer()),
        );

        let resp = app
            .oneshot(
                Request::get("/test/timing")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let timing_values: Vec<_> = resp
            .headers()
            .get_all(SERVER_TIMING)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();

        assert!(timing_values.iter().any(|v| v.contains("dur=42")));
        assert!(
            timing_values
                .iter()
                .any(|v| v.contains("coalesced;desc=\"immediate\""))
        );
    }

    struct OversizedBody {
        chunk: Bytes,
        count: usize,
    }

    impl HttpBody for OversizedBody {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
            if self.count < 65 {
                self.count += 1;
                Poll::Ready(Some(Ok(hyper::body::Frame::data(self.chunk.clone()))))
            } else {
                Poll::Ready(None)
            }
        }
    }

    #[tokio::test]
    async fn coalescing_response_cap_rejects_oversized_body_with_envelope() {
        let executions = Arc::new(AtomicUsize::new(0));
        let exec_c = executions.clone();
        let run1_held = Arc::new(Notify::new());
        let run1_release = Arc::new(Notify::new());
        let r1_held_c = run1_held.clone();
        let r1_rel_c = run1_release.clone();

        let app = axum::Router::new().route(
            "/test/oversized",
            get(move || {
                let exec = exec_c.clone();
                let run1_held = r1_held_c.clone();
                let run1_release = r1_rel_c.clone();
                async move {
                    let id = exec.fetch_add(1, Ordering::SeqCst) + 1;
                    if id == 1 {
                        run1_held.notify_one();
                        run1_release.notified().await;
                        Body::from("initial-run")
                    } else {
                        // Produce a streaming body of chunks totaling > 64 MiB (65 chunks of 1 MiB)
                        let chunk = Bytes::from(vec![b'A'; 1024 * 1024]);
                        Body::new(OversizedBody { chunk, count: 0 })
                    }
                }
            })
            .layer(layer()),
        );

        // Opener task 0 starts run 1 and is held
        let app0 = app.clone();
        let task0 = tokio::spawn(async move {
            app0.oneshot(
                Request::get("/test/oversized")
                    .extension(AccessBasis::Localhost)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        });

        run1_held.notified().await;

        // While run 1 is held, waiter 1 and waiter 2 queue in run 2's cohort (>= 2 waiters)
        let (q1_reg_tx, q1_reg_rx) = oneshot::channel();
        let q1_reg_tx = Arc::new(Mutex::new(Some(q1_reg_tx)));
        let app1 = app.clone();
        let task1 = tokio::spawn(async move {
            app1.oneshot(
                Request::get("/test/oversized")
                    .extension(AccessBasis::Localhost)
                    .extension(JoinNotice::new(move |event| {
                        if let JoinEvent::Registered(Disposition::Queued) = event
                            && let Some(tx) = q1_reg_tx.lock().unwrap().take()
                        {
                            let _ = tx.send(());
                        }
                    }))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        });
        q1_reg_rx.await.unwrap();

        let (q2_reg_tx, q2_reg_rx) = oneshot::channel();
        let q2_reg_tx = Arc::new(Mutex::new(Some(q2_reg_tx)));
        let app2 = app.clone();
        let task2 = tokio::spawn(async move {
            app2.oneshot(
                Request::get("/test/oversized")
                    .extension(AccessBasis::Localhost)
                    .extension(JoinNotice::new(move |event| {
                        if let JoinEvent::Registered(Disposition::Queued) = event
                            && let Some(tx) = q2_reg_tx.lock().unwrap().take()
                        {
                            let _ = tx.send(());
                        }
                    }))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        });
        q2_reg_rx.await.unwrap();

        // Release run 1 to trigger run 2 with oversized body
        run1_release.notify_one();

        let resp0 = task0.await.unwrap();
        assert_eq!(resp0.status(), StatusCode::OK);

        let resp1 = task1.await.unwrap();
        let resp2 = task2.await.unwrap();

        assert_eq!(resp1.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(resp2.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body1 = to_bytes(resp1.into_body(), usize::MAX).await.unwrap();
        let body2 = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body1, body2);

        let json: Value = serde_json::from_slice(&body1).unwrap();
        assert_eq!(json["reason_code"], "coalesced_response_too_large");
        assert!(!body1.starts_with(b"AAAA"));
    }
}
