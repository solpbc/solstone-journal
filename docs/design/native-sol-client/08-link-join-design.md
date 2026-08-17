# Native Sol Link Join Design

This records the design for porting top-level `sol link join` to native Rust.
Generated artifacts are regenerated during implementation; this record does not
change runtime behavior.

## D0. Topology

Decision: use Option A, a new host-only lib crate named
`solstone-core-sol-link`, and exclude it from `make check-rust-ios`.

Rationale: native authority `command.rs` files are compiled into
`solstone-core-sol-client`, which is iOS-visible, so they cannot name
`spl-transport` types. A host-only lib crate is the existing, greppable
mechanism used by `solstone-core-indexer-store` and
`solstone-core-speakers-onnx` for non-iOS dependency islands, while keeping fmt,
clippy, and `check-rust-test` coverage. Putting the adapter in
`solstone-core/src/main.rs` would work mechanically, but it hides substantial
logic in the thin binary and contradicts the library-first native-port shape in
`docs/PORTING.md`.

Rejected shapes:

- Feature-gating `spl-transport` behind a non-default feature: invisible and
  fragile under Cargo feature unification.
- `[workspace] exclude`: escapes workspace fmt, clippy, and test coverage.

Behavior marker: `matches-python`; this is an implementation topology, not a
user-visible behavior change.

## D1. Authority and Dispatch

Authority:

- File: `core/native-sol/think/native/link/authority.toml`
- Source: `command.rs`
- `surface = "sol-link"`
- `path = ["link", "join"]`
- `kind = "top-level"`
- `operation_id = "link.join"`
- `entry_type = "top-level-link"`
- `handler = "link_join"`
- Params:
  `home` option, text, optional, `options = ["--home"]`;
  `code` option, text, required, `options = ["--code"]`;
  `as_role` option, text, optional, `options = ["--as"]`;
  `label` option, text, optional, `options = ["--label"]`

`link.join` is the right operation id because this is a subcommand under a
top-level domain. Chat and notify use `<name>.top_level` because each owns a
single-verb top-level command; `link` has multiple verbs, and `link_join` avoids
a generic generated handler name.

Dispatch shape:

- `core/crates/solstone-core-sol-client-cli/src/lib.rs` adds
  `LinkDispatchSeams` and `dispatch_sol_link_with_seams`.
- This change does not flip top-level `sol link` routing. `sol link join --help`
  still reaches Python compatibility until a later cutover connects the native
  dispatcher.
- The native command receives `journal_root` as an explicit dispatch parameter.
  It must not resolve `SOLSTONE_JOURNAL` internally.

## D2. Consumer-Side Seam

All seam types live in
`core/crates/solstone-core-sol-client/src/seam.rs`. They must not name any
`spl_*` type.

Trait:

- `LinkJoinPairingSeam: Send + Sync`
- `pair_direct(&self, request: LinkJoinDirectRequest) ->
  Result<LinkJoinCredential, LinkJoinPairingError>`
- `pair_relay(&self, request: LinkJoinRelayRequest) ->
  Result<LinkJoinCredential, LinkJoinPairingError>`

Owned request data:

- `LinkJoinPairTarget { host: String, port: u16 }`
- `LinkJoinDirectRequest { targets: Vec<LinkJoinPairTarget>, nonce_hex: String,
  ca_fp_prefix: Vec<u8>, device_label: String, additional_fields:
  serde_json::Map<String, serde_json::Value> }`
- `LinkJoinRelayRequest { relay_origin: String, secret: Vec<u8>, ca_fp_spki:
  Vec<u8>, device_label: String, additional_fields:
  serde_json::Map<String, serde_json::Value> }`

Owned response data:

- `LinkJoinCredential { client_key_pem: String, client_cert_pem: String,
  ca_chain_pem: Vec<String>, ca_fingerprint: String, instance_id: String,
  home_label: String, home_attestation: Option<String>, local_endpoints:
  serde_json::Value, relay_device_token: Option<String>,
  relay_device_token_expires_at: Option<i64> }`

The seam returns `ca_chain_pem: Vec<String>`, not a pre-joined chain. The native
command owns the Python-compatible `join_chain` formatting because it is bundle
presentation, not transport. The seam also returns the precomputed
`ca_fingerprint` string because `spl_transport::tls::parse_certs` is the only
public PEM-to-DER path at SPL v0.1.0.

This seam makes both paths fakeable in-process with zero sockets: native command
tests provide a recording/failing `LinkJoinPairingSeam`, and the real
`spl-transport` adapter exists only in the excluded host crate.

Behavior marker: `matches-python`.

## D3. SPL Adapter Boundaries

The real adapter in `solstone-core-sol-link` owns all `spl-transport` imports
and the async runtime boundary.

Direct pairing:

- Parse the already validated plain direct request into SPL-compatible values.
- Call SPL's existing direct path. `spl_core::PAIR_PATH` is
  `/app/network/pair`, and `spl_transport::pairing::pair_with_seam` sends
  `format!("{PAIR_PATH}?token={nonce_hex}")`, byte-identical to Python's
  `_direct_pair_path`.
- Keep `_framed_target` validation in the native command for `--home`: missing
  host yields `Pair-link target missing host.`, and missing explicit port yields
  `Pair-link target missing explicit port.`. The seam target only needs host and
  port.

Relay pairing:

- Convert the plain relay request to `spl_core::pairlink::RelayPairLink`.
- Call `spl_transport::relay_pairing::pair_over_relay`.
- Return relay device-token fields only from this path.

The adapter computes `ca_fingerprint` from the first PEM certificate in the
returned chain via `spl_transport::tls::parse_certs` and `spl_core::ca`.

## D4. Behavior Decisions

1. `topology` - `matches-python`. Option A is a build-boundary decision; CLI
   behavior remains the Python contract.
2. `seam shape` - `matches-python`. Two plain-data methods make direct and
   relay fakeable without sockets and keep SPL types outside iOS-visible code.
3. `--as peer` state - `expected-differs`. Native reads `link/state.json`
   read-only and fails before pairing if it is missing, unreadable, or lacks a
   valid `instance_id`. Error text names the creator command:
   `Peer join requires an initialized link identity. Run 'sol call link pair'
   on this journal first, then retry.`
4. Pair-link prefix guard - `matches-python`. Keep the
   `https://go.solstone.app/p#` check above `spl_core::pairlink::parse`, so
   bare fragments and alternate hosts fail with the Python-facing message.
5. `--home` override - `expected-differs`. Native requires host plus explicit
   port before dialing, matching Python's validation, but ignores a base-path or
   query prefix and uses SPL's fixed pair path. Python would honor the prefix;
   it is meaningless for this raw pairing socket.
6. `local_endpoints` - mixed. Absent or JSON null becomes `[]`
   (`matches-python`). A present array passes through verbatim with nested key
   order preserved (`matches-python`). A present non-array fails before any
   write (`expected-differs`; Python coerces to `[]`). A serialized value over
   16 KiB fails before any write (`expected-differs`; Python has no ceiling).
7. Existing path ordering - `expected-differs`. Python prechecks only the
   direct-observer path. Native prechecks both observer paths because the label
   determines their destination before dialing; both peer paths still check
   after the response because the directory name is the receiver `instance_id`.
   Post-burn failures use
   `Credentials path already exists: {path}. The pairing code is now spent; generate a new one and rerun after removing it.`
   Pre-dial observer failures keep Python's wording.
8. `peer.json` bytes - `matches-python`. Use injected UTC clock with
   `%Y-%m-%dT%H:%M:%SZ`, exact key order `label`, `paired_at`, `instance_id`,
   `home_label`, `fingerprint`, `local_endpoints`, `role`, two-space indent,
   `": "` separators, trailing newline, and Python `ensure_ascii=True`
   semantics. `serde_json` does not escape non-ASCII by default, so the command
   needs an explicit Python-compatible JSON writer. Nested object order is
   preserved through the workspace `serde_json` `preserve_order` feature.
9. `home_attestation` - `matches-python`. `home_attestation.jwt` is mandatory;
   reject missing or empty `home_attestation` before writing any bundle file
   with `Pair response missing home_attestation`.
10. Relay returned-certificate validation - `expected-differs`. This is a
    security improvement: Python's relay path binds the returned private key but
    never validates the returned client certificate, while
    `spl_transport::relay_pairing::pair_over_relay` verifies the live peer leaf
    against the pinned CA.
11. `home_label` - `expected-differs`. SPL's `PairResponse.home_label` is a
    required string with no serde default, so native fails deserialization where
    Python would coerce missing or non-string to `""`. Do not weaken SPL's
    schema in the native command.
12. Missing `--code` - `matches-python`. The native argv parser must produce
    exit 2 before pair-link parsing or seam access, with an argparse-shaped
    required-argument error for `--code`.

## D5. Bundle Writes

The native command owns credential layout and byte formatting:

- Observer path: `observer_bundle_dir(label)` equivalent under
  `$XDG_CONFIG_HOME/solstone-observer/spl/<label>/`, falling back to
  `~/.config/solstone-observer/spl/<label>/`.
- Peer path: `<journal_root>/peers/<remote_instance_id>/`.
- Bundle files: `private.pem`, `cert.pem`, `chain.pem`,
  `home_attestation.jwt`, and `peer.json`.
- Atomic publish: create parent, refuse if destination exists, write staging
  directory mode `0700`, file mode `0600`, fsync files and staging directory,
  `std::fs::rename`, fsync parent directory, and remove staging on failure.

`std::fs::rename` can replace an existing empty directory on Unix, so the
explicit destination precheck remains load-bearing. A race after the precheck is
the same known Python limitation; no cross-process lock is added in this change.

## D6. Gate Plumbing

Inventory:

- `scripts/build_native_sol_inventory.py`: add `"top-level-link"` to
  `ENTRY_TYPES`; add `FINAL_TOP_LEVEL_LINK_TOTAL = 1`; add
  `("sol-link", "top-level-link"): FINAL_TOP_LEVEL_LINK_TOTAL` to
  `check_top_level_partition()`.
- Leave `FINAL_ORACLE_TOTAL`, `FINAL_HTTP_TOTAL`,
  `FINAL_JOURNAL_PYTHON_COMPAT_TOTAL`, and `FINAL_STUB_COUNTS` unchanged. This
  adds one non-HTTP top-level entry and does not change sol-call grammar,
  HTTP authorities, journal Python compatibility count, or stub inventory.
- Accept `sol-link` as a valid surface in discovery.

Conformance:

- `scripts/check_native_sol_conformance.py`: route `top-level-link` through
  `check_non_http_entry`, matching notify/import non-HTTP authority rules.

Coverage:

- `scripts/check_native_sol_coverage.py`: add
  `FINAL_TOP_LEVEL_LINK_TOTAL`, compute `required_top_level_link`, fold it into
  `required_dispatch`, and require link `success` and `failure` buckets.
- Do not require request or notification binding for link, because the
  executable parity harness uses recorded HTTP transport while pairing uses TLS
  sockets and relay WebSockets.

Parity:

- Add `core/fixtures/native-sol/parity/link_join.jsonl`; do not reuse
  `link.jsonl`, which already covers the `sol-call` link surface.
- Author only pre-network vectors: `--help`, invalid `--as`, invalid `--label`,
  invalid pair link with `--home`, and missing `--code`.
- `core/crates/solstone-core-sol-client-cli/src/bin/resolve_parity_leaves.rs`:
  add `"sol-link" => vec!["link".into(), "join".into()]`.
- `core/crates/solstone-core-sol-client-cli/tests/parity.rs`: include the new
  fixture and dispatch `surface == "sol-link"` through
  `dispatch_sol_link_with_seams`.

The parity harness compares the native command's actual `stdout`, `stderr`,
`exit`, and side effects to the vector's `expected` block. It does not rederive
expected output from Python. The native help text is still byte-identical to
Python for an invisible later cutover.

Deny:

- `core/deny.toml`: add `CDLA-Permissive-2.0` to `[licenses].allow` for
  `webpki-roots`.
- `core/deny.toml`: with `unknown-git = "deny"`, add the cargo-deny source
  allowance as `allow-git = ["https://github.com/solpbc/spl-rust"]` under
  `[sources]`.
- Prep found no other missing license from `spl-transport`'s transitive graph.
  The raw `Unlicense` occurrence is represented by `Unlicense OR MIT`, already
  covered through `MIT`.
- `[graph].targets` already includes `aarch64-apple-ios`; cargo-deny will still
  evaluate the workspace dependency graph for that target, so the git allowance
  and license allow-list must cover the new reachable crate.

Access/import cleanliness:

- Do not edit `scripts/check_access_imports_clean.py` unless
  `make check-access-imports-clean` actually fails. Routing is not flipped, so
  `sol link join --help` still reaches Python.

## D7. Byte Oracles

Python-produced `peer.json` fixtures are byte oracles, not executable parity
vectors. Put them under `core/fixtures/native-sol/link-join/` and generate them
from `scripts/build_core_fixtures.py`, the existing Rust-facing Python fixture
builder. Because tests include them from `#[cfg(test)]` modules, they do not
create sdist compile-input obligations.

Required AC 5a cases:

1. Role-less observer, ASCII label/home label, empty `local_endpoints`, fixed
   clock.
2. Peer role serializer case with non-ASCII `label`, `home_label`, and endpoint
   strings to prove Python `ensure_ascii=True` `\u` escaping. This is a byte
   oracle for the formatter; real argv validation still rejects non-ASCII
   `--label`.
3. Nested `local_endpoints` array containing ordered objects to prove recursive
   key-order preservation and two-space pretty printing.

## D8. Error Exhaustiveness

`solstone-core-sol-link` owns the exhaustive conversion from
`spl_transport::TransportError` into a `solstone-core-sol-client` owned
`LinkJoinPairingErrorKind`. That match names all 14 SPL v0.1.0 variants:
`Io`, `Tls`, `Crypto`, `Mux`, `Http`, `Json`, `PairLink`, `Pairing`,
`Rejected`, `Relay`, `RelayControlRejected`, `NoEndpoint`, `NotPaired`, and
`LocalOffset`. The nested `RelayError` match names `HomeOffline`,
`Unauthorized`, `Unpaid`, `UnknownInstance`, `PairWindowClosed`, `Overflow`,
`Abnormal`, `UpgradeRejected`, and `Stalled`; `RelayControlEndpoint` is matched
by named endpoint as well. No `_` arm is allowed.

`solstone/think/native/link/command.rs` owns user-facing messages by matching
all `LinkJoinPairingErrorKind` variants exhaustively, again with no `_` arm.
The command never sees `spl_transport::TransportError`.

If SPL adds a new error variant, the excluded adapter crate fails to compile on
host workspace gates. The iOS canary skips the adapter by design, so AC 8 is
enforced by host fmt/clippy/test/deny coverage, not by `check-rust-ios`.

## D9. No-Retry Assertion

Add a `#[cfg(test)]` Rust test in `solstone-core-sol-link`. The small module
that calls SPL pairing has a doc comment stating the no-loop invariant and why:
SPL owns the one-request commit rule, and this adapter must not wrap it. The
test `include_str!`s that module and asserts `pair_with_seam(` appears exactly
once, `pair_over_relay(` appears exactly once, and the source contains no
`for `, `while `, or `loop ` token.

## D10. Release Proof

This change does not connect the new crate to the shipping `solstone-core` binary,
but cutover will make it part of the desktop `sol link` path. The `ring`
dependency therefore creates a real native dependency release-proof obligation
for the cutover change across the three desktop release targets:

- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `aarch64-apple-darwin`

This proof belongs to the cutover change; it is not waived by the iOS exclusion.

## D11. File Manifest

Design-stage edits:

- `docs/design/native-sol-client/08-link-join-design.md`
- `docs/PORTING.md`

Implementation-stage edits:

- `core/native-sol/think/native/link/authority.toml`
- `solstone/think/native/link/command.rs`
- `scripts/build_native_sol_inventory.py`
- `scripts/check_native_sol_conformance.py`
- `scripts/check_native_sol_coverage.py`
- `scripts/build_core_fixtures.py`
- `Makefile`
- `core/Cargo.toml`
- `core/Cargo.lock`
- `core/deny.toml`
- `core/crates/solstone-core-sol-link/Cargo.toml`
- `core/crates/solstone-core-sol-link/src/lib.rs`
- `core/crates/solstone-core-sol-link/src/direct_seam.rs`
- `core/crates/solstone-core-sol-link/src/pairing_entry.rs`
- `core/crates/solstone-core-sol-client/Cargo.toml`
- `core/crates/solstone-core-sol-client/src/command.rs`
- `core/crates/solstone-core-sol-client/src/seam.rs`
- `core/crates/solstone-core-sol-client/src/generated/inventory.rs`
- `core/crates/solstone-core-sol-client-cli/src/lib.rs`
- `core/crates/solstone-core-sol-client-cli/src/bin/resolve_parity_leaves.rs`
- `core/crates/solstone-core-sol-client-cli/tests/parity.rs`
- `core/fixtures/native-sol/parity/link_join.jsonl`
- `core/fixtures/native-sol/link-join/**`
- `docs/PORTING.md` Mobile Readiness rationale for the new excluded crate
- Existing native command test modules that construct `CommandContext` directly
  receive `link_pairing: None` and `journal_root: None`.

No Python product deletion is part of this change. `join_cli.py` remains while
top-level `sol link` help and non-joined link verbs continue through
compatibility.

## D12. Risks and Open Questions

- The later cutover must construct and inject the real seam from a desktop
  runtime boundary without moving `spl-transport` into an iOS-visible crate.
