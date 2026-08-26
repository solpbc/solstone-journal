// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! # Convey HTTP substrate design
//!
//! This crate is the future Rust substrate for the journal-host `convey` HTTP
//! service. It is library-only in this wave: no `src/bin/` exists until the
//! separate `journal convey` integration wave owns port selection, config, and
//! journal-path loading. `solstone-core-convey-http` is the established
//! `solstone-core-<domain>` name and describes the narrow server substrate
//! without claiming ownership of the application routes.
//!
//! ## D1: iOS canary membership — exclude by product shape
//!
//! The crate is excluded from `check-rust-ios`, while remaining in the full
//! `cargo deny` graph for all configured targets. This follows the permanent
//! `solstone-core-sol-link` precedent: `convey` is a server run by the machine
//! hosting the journal, and phones are remote HTTP clients rather than server
//! hosts. Its current empty library compiling for iOS is incidental, not a
//! supported deployment promise; future TLS and native loopback-binding work
//! reinforces that distinction. `docs/PORTING.md` records this product-shape
//! decision beside the `sol-link` precedent.
//!
//! ## D2: crate name and shape — `solstone-core-convey-http`, library only
//!
//! Keep the prep name and use a library-only shape, like
//! `solstone-core-journal-io`. The crate is intentionally not yet wired to
//! `journal convey`, so an executable would prematurely decide configuration,
//! listening port, and journal resolution. A later integration wave may add a
//! binary only when it owns that process boundary.
//!
//! ## D3: identity injection — accept-time `Extension<AccessBasis>`
//!
//! `identity.rs` defines `AccessBasis::{Localhost, LinkedDevice { carrier, cid
//! }, PairingPeer { carrier }}` and `Carrier::{Direct, ViaSpl}`. `AccessBasis`
//! derives `Debug`, `Clone`, `PartialEq`, and `Eq`. The acceptor constructs this
//! value from connection provenance and passes it to `serve_connection`; no
//! request header, path, query, or body can construct or replace it. For each
//! connection,
//! `serve_connection` clones the supplied `Router` and layers
//! `axum::Extension(identity)` before adapting it with
//! `TowerToHyperService`. Axum 0.8.9's `Extension<T>` layer inserts a cloned
//! value into request extensions, and its extractor reads request parts, so
//! handlers place `Extension<AccessBasis>` before any body-consuming
//! extractor. `Router::layer` covers normal routes and fallbacks alike.
//!
//! `gate.rs` admits `Localhost` and `LinkedDevice`; `PairingPeer` is confined
//! to the pairing ceremony by the door. `Carrier` is observability-only:
//! `Direct` and `ViaSpl` have identical authorization behavior and may only
//! affect logging/metrics supplied by a future caller. This is structural rather
//! than convention: the only accepted identity type is an enum supplied by the
//! connection caller, never parsed from an HTTP request.
//!
//! ## D4: shared HTTP/1 serving — caller-configured builders
//!
//! `serve.rs` will expose one generic core,
//! `serve_connection<I>(io, router, identity, builder: &http1::Builder)`, for
//! Tokio `AsyncRead + AsyncWrite + Unpin + Send + 'static` I/O. It wraps `io`
//! in `hyper_util::rt::TokioIo`, applies the per-connection layers, adapts the
//! router with `TowerToHyperService`, and drives
//! `hyper::server::conn::http1::Builder::serve_connection`. TCP and mux-stream
//! callers are thin builder factories around that same function: the TCP
//! builder keeps the Hyper 1.11 default (`keep_alive(true)`), while the mux
//! builder explicitly calls `keep_alive(false)`. No `server-auto` builder is
//! used: it would enable HTTP/2 even though this server is HTTP/1 only.
//!
//! ## D5: request bounds — standard layer plus HTTP/1 parser limits
//!
//! Every per-connection router clone uses
//! `tower_http::limit::RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT)`,
//! where `REQUEST_BODY_LIMIT` is 4 GiB + 1 MiB. That layer is not path-aware:
//! it immediately returns 413 for an oversized `Content-Length` and wraps
//! streamed bodies with the same limit. It is the SAVE transport ceiling.
//! Narrower caps are enforced after dispatch: import-web’s router-level
//! `DefaultBodyLimit` (128 MiB) and 64 MiB per-part collector on every
//! import route except SAVE; SAVE’s MethodRouter `DefaultBodyLimit` matches
//! this connection ceiling and its counted `file`-field writer refuses at
//! 4 GiB; Support pins 128 MiB on its extractor layer and on draft
//! `to_bytes`. For a chunked body with no `Content-Length`, the consuming
//! handler must map the wrapped body's `LengthLimitError` to 413;
//! the fallback does not consume a body, so acceptance coverage uses the
//! immediate oversized-`Content-Length` path and implementation must not claim
//! that the layer alone emits a streamed-body response. The HTTP/1 builder will
//! call `max_headers(32)`
//! (Hyper's default is 100) and `max_buf_size(64 * 1024)`. Hyper has no
//! separate header-byte API: `max_buf_size` bounds the connection's HTTP/1
//! read/write buffers and therefore bounds an incomplete header/message, not
//! total streamed request-body bytes. Its default is 417,792 bytes and its
//! minimum is 8,192 bytes, so 64 KiB is a valid, materially stricter bound.
//!
//! ## D6: error envelope — minimal legacy-compatible fallback
//!
//! `envelope.rs` will define a serializable `ErrorEnvelope` with exactly
//! `error`, `reason_code`, and `detail` fields, and an `error_envelope` helper
//! returning `(StatusCode, axum::Json<ErrorEnvelope>)`. This matches the
//! existing Python `error_response()` envelope; field order is immaterial.
//! The minimal probe route is a fallback returning 404 with
//! `reason_code = "not_found"`, `error = "Not Found"`, and `detail` set to
//! the observed accept-time identity for the connection probes.
//! Porting the Python `Reason` registry is a deliberate non-goal: this wave
//! supplies only the envelope shape required by the transport substrate.
//!
//! ## Planned module layout
//!
//! - `lib.rs`: module exports and this design contract.
//! - `identity.rs`: the closed `AccessBasis` and observability-only `Carrier`
//!   enums.
//! - `gate.rs`: the request-side access gate that accepts the owner and linked
//!   device `AccessBasis` variants.
//! - `serve.rs`: HTTP/1 builder factories and shared generic
//!   `serve_connection` implementation.
//! - `listener.rs`: loopback-only TCP listener validation and accept helper.
//! - `envelope.rs`: JSON error type/helper and the minimal 404 fallback.
//!
//! ## Acceptance-test ownership
//!
//! 1. `serve.rs::tests::tcp_and_mux_stream_use_the_shared_http1_path`
//!    exercises both I/O kinds through `serve::serve_connection`, including
//!    the fallback and gate path with their distinct keep-alive builders.
//! 2. `listener.rs::tests::loopback_and_supplied_identities_round_trip`
//!    proves the observed `Direct` and `ViaSpl` values over duplex I/O.
//!    `tests/convey_http_loopback.rs` proves `Localhost` on a real loopback
//!    accept for each stack.
//! 3. `serve.rs::tests::request_data_cannot_replace_accept_time_access_basis`
//!    exercises `serve::serve_connection` plus `identity::AccessBasis`; request
//!    headers, path, query, and body must not alter the injected basis.
//! 4. `tests/convey_http_loopback.rs` exercises `listener::bind_loopback`
//!    on the exact IPv4 and IPv6 loopback addresses; the paired-device
//!    wildcard door is owned separately by `solstone-core-convey-shell::door`
//!    via `convey_shell::serve`.
//! 5. `serve.rs::tests::configured_body_header_and_buffer_bounds_are_enforced`
//!    exercises `serve::tcp_builder`, `serve::mux_builder`, and the
//!    `RequestBodyLimitLayer` path for `REQUEST_BODY_LIMIT` (4 GiB + 1 MiB)
//!    declared bodies, 32 headers, and the 64 KiB HTTP/1 buffer bound.
//!    Wire-level SAVE transport and extractor-ceiling proofs live in
//!    `solstone-core-import-web` `tests/import_web_transport.rs`.
//!    Wire-level Support extractor-ceiling proofs live in
//!    `solstone-core-support-web` `tests/support_web_transport.rs`.
//!    Entities HTML wire proofs remain in `solstone-core-convey-shell`
//!    `tests/boundary_tcp.rs`. The 4 GiB file-field ceiling is owned by
//!    `solstone-core-import-web::save_stream` unit tests, not this crate.
//! 6. `identity.rs::tests::access_basis_variants_remain_exhaustive` uses
//!    exhaustive matches over `AccessBasis` and `Carrier`, so adding a future
//!    `AccessBasis` variant is a compile-time test failure.
//!
//! `gate.rs::tests::access_gate_accepts_established_bases_and_refuses_pairing_peers`
//! remains a supporting test: it proves both `Carrier` variants authorize
//! identically, but does not replace the criterion-two round-trip test. Likewise,
//! `envelope.rs::tests::error_envelope_uses_the_legacy_json_shape` protects
//! D6's compatibility surface.
//!
//! Every acceptance criterion is subject to the required break-then-revert
//! discipline during implementation. In particular, criteria 3 and 4 must be
//! made red by temporarily deriving identity from request data and by making
//! one listener bind a wildcard address; criterion 6 is made red by a
//! temporary fourth identity variant, which produces the intended compile
//! failure in its exhaustive-match test. Criteria 1, 2, and 5 likewise require
//! a deliberate regression and observed red result before the correct code is
//! restored.

pub mod envelope;
pub mod gate;
pub mod identity;
pub mod listener;
pub mod refusal;
pub mod serve;
