# Native Sol Link Serve Design

## Status

- Decision: **HOLD**. No implementation landed on this arc.
- `sol link serve` remains the Python path at `solstone/think/link/serve_cli.py`;
  routing, SPL pins, and gate constants did not change.
- The blockers are three spl-rust gaps, relay enrollment, local-route hook, and
  per-request attribution, plus one Solstone gap: buffered `CommandOutput` does
  not fit a resident command. See D9/D10.
- Before reopening this arc, first re-check D10's upstream issue list against
  the then-current spl-rust release.

This records the design-stage decision for arc native link-serve, native
`sol link serve`.

Evidence base:

- `docs/design/native-sol-client/09-link-serve-prep.md`
- `spl-rust v0.2.0`, an annotated tag where bare `git rev-parse v0.2.0`
  yields tag object `22dd02eb151f8a4e5c8ce48101f58c23a040205a`, while the
  line-number-pinned commit is peeled `v0.2.0^{}`
  `05bca1c4a4b530ee824c172c57cae7c20a8bb049`

No runtime behavior changes are made by this record.

## D0. Recommendation

Decision: **B. Hold the implementation**. Do not land a native `sol link serve`
implementation on SPL v0.2.0.

Rationale: a reduced LAN-only native serve would not be a usable substitute for
Python's resident serve. It would lack relay mode, lack the local status route,
break the observer attribution contract, and diverge on important HTTP header
and connection behavior. The scope also forbids re-implementing SPL-owned mux,
proxy transforms, header allow-lists, or hop-by-hop behavior, so the blocked
criteria cannot be recovered inside Solstone without new SPL public API.

Behavior marker: `expected-differs`; current SPL v0.2.0 cannot meet Python
functional parity for this command.

## D1. Corrected Observer Attribution

Decision: confirm the prep note's decoy `BridgeNames` values are wrong and must
not be adopted.

The SPL v0.2.0 path is:

- spl-rust v0.2.0 `crates/spl-core/src/bridge.rs:270-296` runs
  `upstream_request_headers`.
- That function strips reserved headers before any opener sees them.
- spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge_carrier.rs:63`
  calls `self.opener.proxy_headers(upstream_headers)` on the already-filtered
  set.
- spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:90-102`
  says opener implementations must add the consumer's complete
  authentication-header set.

That model is deliberately anti-spoofing: caller-supplied loopback attribution
is stripped, and the opener injects authoritative headers. Setting
`observer_header_name` or `protocol_version_header_name` to decoy names would
defeat that protection for the real `X-Solstone-*` names. Setting them to the
real names preserves SPL's intended protection, but then per-request
client-supplied `X-Solstone-Observer` and `X-Solstone-Protocol-Version` cannot
survive. The opener never receives the original unfiltered request head, so it
cannot recover the stripped values.

Conclusion: per-request, client-supplied observer attribution cannot survive
the v0.2.0 bridge. This is a third gap against the contract in
`solstone/observe/protocol.py:19-23`.

Exact missing public API: either a per-request opener hook that receives the
unfiltered `RequestHead` and can re-inject selected authoritative headers, or a
bridge policy that exempts named headers from reserved stripping while retaining
explicit anti-spoofing semantics.

Behavior marker: `expected-differs`.

## D2. Authority and Dispatch

Decision: the native authority, if this implementation becomes unblocked, is a second
top-level link entry in `core/native-sol/think/native/link/authority.toml`.

Authority shape:

- `surface = "sol-link"`
- `path = ["link", "serve"]`
- `kind = "top-level"`
- `operation_id = "link.serve"`
- `entry_type = "top-level-link"`
- `handler = "link_serve"`
- Params from `solstone/think/link/serve_cli.py:82-97`:
  `label` optional text option `--label`;
  `port` optional integer option `--port` with default `5015`;
  `relay_url` optional text option `--relay-url`;
  `direct` boolean flag `--direct`.

Help output remains byte-for-byte Python-compatible at 80 columns. Prep
confirmed the oracle is 638 bytes.

Dispatch changes:

- `core/crates/solstone-core-sol-client-cli/src/lib.rs:180-189` currently
  hardcodes `["link", "join"]` for `dispatch_sol_link_with_seams`.
  It must derive the surface lookup path from argv. The dispatcher should
  accept both full `["link", "<verb>", ...]` and parity-style
  `["<verb>", ...]`, resolve `["link", "join"]` or `["link", "serve"]`
  through generated inventory, and pass the remaining option argv to the
  matched handler.
- `core/crates/solstone-core-sol-client-cli/src/bin/resolve_parity_leaves.rs:47`
  currently hardcodes `["link", "join"]`. It must use each vector's argv for
  `surface == "sol-link"` so `link.join` and `link.serve` resolve independently.
- Shipping top-level native routing also needs an `Outcome::Link` or equivalent
  in `solstone-core-sol-client-cli` and `solstone-core-sol`, because
  `evaluate_args` currently recognizes `chat`, `import`, and `notify`, but not
  `link`.

Behavior marker: `matches-python` for argv shape and help bytes; no runtime
serve behavior is implemented while the implementation is held.

## D3. Serve Seam and Runtime Boundary

Decision: keep SPL types out of `solstone-core-sol-client`. All serve seam
types in `core/crates/solstone-core-sol-client/src/seam.rs` must be plain owned
data.

Plain data types:

- `LinkServeEndpoint { host: String, port: u16 }`
- `LinkServeBundle { private_key_pem: String, client_cert_pem: String,
  ca_chain_pem: String, home_attestation: String, instance_id: String,
  home_label: String, local_endpoints: Vec<LinkServeEndpoint> }`
- `LinkServeRequest { label: String, port: u16, direct: bool,
  relay_origin: Option<String>, bundle: LinkServeBundle }`
- `LinkServeErrorKind` with plain variants for invalid bundle, bind failure,
  runtime unavailable, transport failure, interrupted shutdown, and unsupported
  relay.
- `LinkServeRunner: Send + Sync` with one resident blocking `serve` method.

The adapter in `core/crates/solstone-core-sol-link` owns all SPL imports and:

- converts `LinkServeBundle` into `spl_transport::credential::Credential`;
- computes `ca_fp_prefix`;
- constructs `spl_transport::client::TransportClient`;
- implements `spl_transport::journal_bridge::CarrierOpener`;
- constructs `JournalBridgeConfig`;
- starts `journal_bridge::start`;
- owns the Tokio runtime and resident blocking loop.

Blocking output decision: do not pretend `CommandOutput` can match Python's
startup log. Python logs `forwarding 127.0.0.1:PORT -> home LABEL over pl` at
startup and then blocks (`serve_cli.py:122-128`). Native `CommandOutput` is
buffered (`core/crates/solstone-core-sol-client/src/command.rs:29-53`), and
`solstone-core-sol` prints it only after the handler returns
(`core/crates/solstone-core-sol/src/lib.rs:691-695`). A real resident command
therefore needs a streaming output/signal-aware command interface, not the
current buffered handler shape. Returning the startup line only after shutdown
would be observably wrong and should not be accepted as parity.

Behavior marker: `matches-python` for SPL isolation and fakeable seam shape;
`expected-differs` for startup output under the current buffered command
interface.

## D4. Bridge Configuration

Decision: if a reduced LAN-only implementation is ever forced, use SPL's bridge
rather than re-implementing proxy transforms. The following values are the
closest viable configuration, with known divergences recorded.

`BridgePolicy` values:

1. `port`: parsed `--port`, default `5015`.
   Behavior marker: `matches-python`.
2. `capability_gate`: `Disabled`, because Python has no bootstrap capability
   cookie requirement.
   Behavior marker: mixed; no capability token `matches-python`, but SPL still
   enforces exact `Host`.
3. `stream_response`: true for every request, not only `GET /sse/events`, so
   response body bytes are delivered incrementally where SPL allows streaming.
   Behavior marker: `matches-python` for incremental delivery.
4. `request_headers`: `RequestHeaderPolicy::ForwardAll`, so non-reserved
   request headers are forwarded.
   Behavior marker: mixed; arbitrary non-reserved headers `matches-python`,
   but reserved names are still stripped.
5. `max_request_body_bytes`: SPL default 8 MiB.
   Behavior marker: `expected-differs`; Python has no equivalent explicit
   bridge-level body ceiling.

`BridgeNames` values:

1. `capability_cookie_name = "__solstone_link_cap"`
2. `upstream_cookie_prefix = ""`
3. `observer_header_name = "x-solstone-observer"`
4. `protocol_version_header_name = "x-solstone-protocol-version"`

The real observer/protocol header names are deliberately reserved so local
callers cannot spoof them. This means caller-supplied per-request attribution
is stripped and cannot be recovered by v0.2.0.

Behavior marker: `expected-differs` for observer attribution; `matches-python`
for unprefixed upstream cookie names, before SPL's cookie attribute rewrites.

## D5. Direct and Relay

Decision: `--direct` must be enforced structurally by credential construction.

For `--direct`, the adapter must construct a credential with no `relay_origin`,
no `device_token`, and no `device_token_expires_at`, regardless of
`--relay-url`. That makes relay fallback impossible in
`TransportClient::dial_carrier`, not merely skipped by convention.

For non-direct serve on SPL v0.2.0, relay is still impossible from the current
Solstone bundle. The join bundle persists only `private.pem`, `cert.pem`,
`chain.pem`, `home_attestation.jwt`, and `peer.json`
(`solstone/think/native/link/command.rs:28-34`), while SPL relay carrier
eligibility requires both `relay_origin` and `device_token`
(spl-rust v0.2.0 `crates/spl-transport/src/client.rs:146-147`).
Supplying `--relay-url` can name an origin, but it cannot create the missing
device token because first enrollment is private to `pair_over_relay`.

Therefore relay is impossible on both paths for native serve on v0.2.0 with
the current bundle: direct by construction, non-direct by missing public
enrollment/token state.

Behavior marker: `matches-python` for `--direct` intent; `expected-differs` for
non-direct `--relay-url`.

## D6. Bundle to Credential

Decision: native serve should load the same observer bundle shape as Python.

Bundle resolution:

- Use `XDG_CONFIG_HOME/solstone-observer/spl/<label>` or
  `~/.config/solstone-observer/spl/<label>`, matching
  `observer_paths.py:12-19`.
- With `--label`, validate that one bundle.
- Without `--label`, scan valid observer bundles; fail on none or multiple,
  matching `serve_cli.py:140-164`.
- Required files match `bundle.py:12-18`.

Credential construction:

- Read `private.pem`, `cert.pem`, `chain.pem`, `home_attestation.jwt`, and
  `peer.json`, matching `bundle.py:40-63`.
- `instance_id`, `home_label`, and `local_endpoints` come from `peer.json`.
- Endpoints use `ip` or `host` as host and default port `7657` when port is
  absent, matching `dialer.py:106-123`.
- `ca_fp_prefix` comes from the first certificate in `chain.pem`. Python join
  derives the CA fingerprint from `chain_pem` at `join_cli.py:213-216` and
  `join_cli.py:880-884`; the existing Rust link adapter does the same at
  `core/crates/solstone-core-sol-link/src/lib.rs:77-88`. Native serve should
  compute the first 16 bytes of that SHA-256 digest in the SPL adapter and pass
  them into `Credential.ca_fp_prefix`.

Behavior marker: `matches-python` for file layout and label selection;
`matches-python` for CA fingerprint source.

## D7. Behavior Decisions

1. `help` - `matches-python`. Use the confirmed 638-byte Python help oracle.
2. `port validation` - `matches-python`. Reject ports outside `1..=65535`
   before attempting bind, matching `serve_cli.py:195-198`.
3. `bind address` - `matches-python`. SPL binds `127.0.0.1` internally at
   spl-rust v0.2.0 `crates/spl-transport/src/journal_bridge.rs:241`.
4. `startup line` - `expected-differs` unless native CLI gains streaming output.
   Current `CommandOutput` cannot emit before a resident handler returns.
5. `request method/path/body` - `matches-python` in the SPL bridge: the request
   method, target, and body are forwarded to `open_stream`.
6. `request headers` - `expected-differs`. `ForwardAll` preserves non-reserved
   headers, but SPL strips `Authorization`, configured observer/protocol
   headers, `Host`, `Content-Length`, and hop-by-hop headers before the opener.
7. `response header allow-list` - `expected-differs`. SPL drops headers Python
   forwards, including `X-Solstone-Request-Id`, `Content-Disposition`,
   `X-Accel-Buffering`, arbitrary X headers, upstream `Transfer-Encoding`, and
   upstream `Content-Length`.
8. `Set-Cookie` - `expected-differs`. SPL prefixes names by
   `upstream_cookie_prefix` and drops `Domain` and `Secure`; Python passes
   cookies through.
9. `Host upstream` - `expected-differs`. SPL strips upstream `Host`; Python
   forwards it and has a test asserting that.
10. `connection model` - `expected-differs`. SPL handles one request per TCP
    connection and writes `connection: close`; Python uses `ThreadingHTTPServer`
    behavior and does not add close on normal proxied responses.
11. `Host: localhost:5015` under disabled capability gate - `expected-differs`.
    SPL accepts only exact `127.0.0.1:<port>` and returns local 403 for
    `localhost:<port>`.
12. `chunked request body` - `expected-differs`. Python explicitly returns 400
    before the tunnel when request `Transfer-Encoding` is present; SPL reads
    absent `Content-Length` as zero and does not have that explicit rejection.
13. `local status route` - `expected-differs`. SPL answers only bootstrap
    locally and provides no status hook.
14. `status payload` - `expected-differs`. Even with a route hook, SPL does not
    expose faithful carrier health/state.
15. `relay mode` - `expected-differs`. `--relay-url` cannot enroll or produce a
    resident relay carrier on v0.2.0 from the current bundle.
16. `observer attribution` - `expected-differs`. Per-request
    `X-Solstone-Observer` cannot survive reserved stripping.
17. `streaming body delivery` - `matches-python` only for the byte-delivery
    property if `stream_response` is true for all requests. Header framing still
    differs because SPL drops upstream `Transfer-Encoding`.
18. `gateway/lifecycle errors` - `expected-differs`. Python has JSON lifecycle
    responses such as retryable 503; SPL local upstream-open failures are
    bridge-owned responses.

## D8. Test Plan

No validation runs in this design stage. If upstream APIs close the gaps and
implementation proceeds, tests remain in-process/faked with no external relay,
browser, or live journal.

Acceptance-oriented plan:

- AC 1, authority/help: add `core/fixtures/native-sol/parity/link_serve.jsonl`
  vectors for `link serve --help`, invalid port, missing bundle, multiple
  bundles, and bind error through a fake serve seam. Assert exact 638-byte help.
- AC 2, argv dispatch: unit-test `dispatch_sol_link_with_seams` with
  `["link", "join"]`, `["join"]`, `["link", "serve"]`, and `["serve"]`.
  Unit-test `resolve_parity_leaves` resolves operation ids from each vector's
  argv instead of hardcoded join.
- AC 3, bundle loading: fake `FileProvider` tests for label selection, missing
  required files, invalid `peer.json`, non-list `local_endpoints`, and endpoint
  coercion from `ip`/`host` plus default port.
- AC 4, credential construction: fake `LinkServeRunner` records
  `LinkServeRequest`; assert `ca_fp_prefix` source is first cert in `chain.pem`,
  and direct requests carry no relay fields.
- AC 5, bridge policy/header behavior: in-process SPL bridge tests with a fake
  `CarrierOpener` capture the upstream headers and record the expected SPL
  divergences. Do not reimplement header filtering in Solstone tests.
- AC 6, `--direct` and relay: fake adapter tests assert `--direct` structurally
  clears relay fields. Non-direct v0.2.0 test asserts `--relay-url` alone is
  rejected or reported unsupported rather than silently claiming relay parity.
- AC 7, status route: blocked on SPL. Once unblocked, test a local-route hook
  with a fake status snapshot and assert no upstream stream opens. Separately
  test faithful payload fields only if SPL exposes a real bridge/carrier status
  snapshot.
- AC 8, gates/exhaustiveness: host-crate tests cover exhaustive error mapping
  from SPL serve errors into plain `LinkServeErrorKind`; no wildcard arms. iOS
  check remains a dependency-boundary canary, not the SPL adapter proof.
- AC 9, streaming: use a fake carrier that sends response head plus byte `A`,
  then blocks on a test-controlled latch before sending byte `B`. The client
  reads one byte and asserts it receives `A` while the producer has not released
  `B`; then releases the latch and reads `B`. This fails against a buffering
  implementation because the first read cannot complete before the second chunk
  or stream end.

## D9. Gap Assessment

### Relay Lane

Missing public API: public relay device enrollment or re-enrollment equivalent
to private `enroll_device(relay_origin, instance_id, home_attestation)`, and/or
a public persistent relay carrier constructor that can produce `DialedCarrier`
from a relay origin plus valid device token.

Blocks: AC 6 and functional parity for `--relay-url`.

Can Solstone meet it in scope without re-implementing SPL-owned pieces? No.
Using public one-shot relay helpers would require Solstone to build its own
resident proxy/mux behavior around them, which is out of scope. The current
bundle also lacks persisted relay origin and device token.

### Local Status Route

Missing public API: a consumer local-route hook in `journal_bridge` before
upstream forwarding.

Blocks: AC 7.

Can Solstone meet it in scope without re-implementing SPL-owned pieces? No.
Running a second proxy or wrapping SPL's listener would reimplement routing and
proxy transforms the scope assigns to SPL.

Status payload: even if SPL added a route hook, the payload would still not be
faithful on v0.2.0. Prep classifies `health` as not producible from public API,
and the remaining fields as consumer-side approximations rather than
`JournalBridgeHandle` facts. SPL also keeps carrier liveness, stream maps, and
keepalive state private.

Additional missing public API for faithful payload: a bridge/carrier status
snapshot exposing health, state, manager/accept-loop liveness, connected
timestamp, last failure, next retry, reconnect count, and active request or
active stream count.

### Observer Attribution

Missing public API: a per-request opener hook receiving unfiltered request
headers, or a reserved-header exemption policy with explicit anti-spoofing
semantics.

Blocks: the observer protocol contract at `solstone/observe/protocol.py:19-23`;
client-supplied `X-Solstone-Observer` cannot survive the v0.2.0 bridge when
configured safely.

Can Solstone meet it in scope without re-implementing SPL-owned pieces? No.
Decoy `BridgeNames` preserve the raw header but defeat SPL's designated
anti-spoofing model. Static opener re-injection could add one authoritative
observer value, but it cannot preserve per-request client-supplied attribution.

### Resident Startup Output

Missing public API: a native command interface with streaming stdout/stderr or
an event sink for resident commands.

Blocks: Python-equivalent startup observability for `sol link serve`.

Can Solstone meet it in scope without reworking CLI plumbing? Not faithfully.
The current `CommandOutput` is buffered until return, so a blocking resident
handler cannot emit the startup line at startup.

## D10. Upstream Issue List

Candidate SPL issues:

1. Expose relay first-enrollment or re-enrollment for an already paired
   credential: public API taking relay origin, instance id, and home attestation
   and returning device token plus expiry.
2. Expose a public persistent relay carrier construction path, or make
   `TransportClient` able to acquire missing relay tokens through a public
   consumer-provided enrollment hook.
3. Add `journal_bridge` local-route hook before upstream forwarding.
4. Add `journal_bridge`/`MuxCarrier` status snapshot API with the fields needed
   for Python status parity.
5. Add a per-request opener/auth hook that receives unfiltered request headers,
   or a policy to exempt selected headers from reserved stripping with explicit
   anti-spoofing controls.

Candidate Solstone-native issue if SPL closes its gaps:

1. Add a streaming/resident command output interface for native top-level
   commands, or route resident commands through a process entrypoint that can
   print startup output before blocking.

## D11. Coverage and Inventory Plan

For implementation after unblocking:

- `scripts/build_native_sol_inventory.py`: change
  `FINAL_TOP_LEVEL_LINK_TOTAL` from `1` to `2`; leave
  `FINAL_ORACLE_TOTAL`, `FINAL_HTTP_TOTAL`,
  `FINAL_JOURNAL_PYTHON_COMPAT_TOTAL`, `FINAL_STUB_COUNTS`, and
  `FINAL_HTTP_GROUP_COUNTS` unchanged.
- `scripts/check_native_sol_coverage.py`: no rule change beyond the moved
  imported total; add success/failure parity buckets for `link.serve`.
- `scripts/check_native_sol_conformance.py`: no rule change for a second
  `top-level-link`; existing line 139 routes top-level link entries through
  `check_non_http_entry`.
- Generated inventory moves after authority regeneration.
- `core/fixtures/native-sol/parity/link_join.jsonl` continues satisfying
  `link.join`; add a separate `link_serve.jsonl`.

Behavior marker: `matches-python` for authority partition intent; no product
behavior until implementation.

## D12. Baseline Correction

Prep corrected one baseline claim:

- `tests/sandbox_profile/test_marker.py::test_marker_refusal_matrix_zero_side_effects`
  passes on the untouched tree with exit 0.
- Only
  `tests/test_core_sdist_compile_inputs_integration.py::test_core_sdist_compile_inputs_are_required_by_real_wheel_build`
  fails, with exit 2 from Cargo refusing to update `core/Cargo.lock` under
  `--locked` inside the sdist control-wheel build.
- `cargo metadata --manifest-path core/Cargo.toml --locked` succeeds, so the
  workspace lock is not drifted. The refusal is in the sdist build path.

Assessment: a future SPL v0.2.0 pin bump plausibly interacts with that failing
integration because it changes git dependency lock entries and the sdist must
carry a lock consistent with its rendered manifest. The known failure is not
evidence that the workspace lock is currently stale, but any pin bump must be
checked against the sdist packaging path once implementation is authorized.

## D13. File Manifest

Design-stage edits:

- `docs/design/native-sol-client/09-link-serve-design.md`

Implementation-stage edits if the implementation is later unblocked:

- `core/native-sol/think/native/link/authority.toml`
- `solstone/think/native/link/command.rs`
- `scripts/build_native_sol_inventory.py`
- `scripts/check_native_sol_coverage.py`
- `core/Cargo.toml`
- `core/Cargo.lock`
- `core/deny.toml`
- `core/crates/solstone-core-sol-client/src/command.rs`
- `core/crates/solstone-core-sol-client/src/seam.rs`
- `core/crates/solstone-core-sol-client/src/generated/inventory.rs`
- `core/crates/solstone-core-sol-client-cli/src/lib.rs`
- `core/crates/solstone-core-sol-client-cli/src/bin/resolve_parity_leaves.rs`
- `core/crates/solstone-core-sol-client-cli/tests/parity.rs`
- `core/crates/solstone-core-sol/src/lib.rs`
- `core/crates/solstone-core-sol-link/Cargo.toml`
- `core/crates/solstone-core-sol-link/src/lib.rs`
- `core/crates/solstone-core-sol-link/src/serve.rs`
- `core/fixtures/native-sol/parity/link_serve.jsonl`
- focused Rust test modules under the touched crates
- `docs/PORTING.md` only if native resident-command or SPL adapter boundaries
  need new doctrine

No Python product deletion is part of this arc. Python `serve_cli.py` remains
the functional implementation until SPL and native CLI gaps are closed.

## D14. Risks and Open Questions

- SPL may choose a different attribution API than the two shapes named here;
  the Solstone design should follow SPL's anti-spoofing model rather than
  preserving raw loopback headers.
- A future relay-capable design also needs bundle persistence for relay origin,
  device token, and token expiry; current native join captures token fields in
  the seam but does not persist them.
- The existing response-header allow-list may be intentional SPL hardening. If
  Solstone needs `Content-Disposition`, request ids, or SSE buffering headers
  over serve, that should be negotiated upstream rather than bypassed locally.
- The startup-output issue is independent of SPL. Even after SPL closes its
  gaps, native resident commands need streaming output or a non-buffered
  process path.

## D15. Python Serve Follow-Ups

These defects are unfixed, ride the live Python `sol link serve` path today,
and are out of scope for this held native arc:

1. `serve_cli.py:286-288` post-head `chunks.get()` has no timeout, while the
   response-head wait has the 30-second timeout.
2. `future.cancel()` is a no-op on an already-running future, so
   `_proxy_to_queue` can keep producing while `_put_queue_item`
   (`dialer.py:42-62`) retries forever on `queue.Full` with no stop token.
3. `RESPONSE_HOP_BY_HOP` (`serve_cli.py:48-56`) omits `transfer-encoding`,
   while `REQUEST_HOP_BY_HOP` includes it.
