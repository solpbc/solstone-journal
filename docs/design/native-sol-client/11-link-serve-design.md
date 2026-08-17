# Native Sol Link Serve Shipping Design

This records the shipping design for the native link-serve delivery, native `sol link serve`.
It supersedes the hold decision in
`docs/design/native-sol-client/09-link-serve-design.md` because SPL v0.3.0
closed the four D9 blockers and the native resident boundary has since shipped.

Evidence base:

- `docs/design/native-sol-client/09-link-serve-prep.md`
- `docs/design/native-sol-client/09-link-serve-design.md`
- `docs/design/native-sol-client/resident-command-design.md`
- `docs/design/native-sol-client/08-link-join-design.md`
- spl-rust v0.3.0 annotated tag
  `62f3c9b6d75f4230b5a13566040965da19fc1c08`, peeled commit
  `6986cf75514f67894469bc395d44801f6d8793ba`

No implementation lands in this record.

## D0. Recommendation

Decision: **ship the native `link.serve` authority entry against SPL v0.3.0**.
It is declared, compiled, inventoried, and parity-covered in this checkout, but
public `sol link` remains the Python compatibility-dispatched path until the
separate cutover checkout flips the command.

All four blockers from `09-link-serve-design.md` D9 are closed:

1. Relay boundary: SPL now exposes public relay enrollment at
   spl-rust v0.3.0 `crates/spl-transport/src/relay_pairing.rs:143-170`, and
   `Credential` carries `relay_origin`, `device_token`, and
   `device_token_expires_at` at
   `crates/spl-transport/src/credential.rs:55-63`.
2. Local status route: `BridgePolicy.local_response` can answer an authorized
   request without opening upstream at
   `crates/spl-transport/src/journal_bridge.rs:160-163` and
   `crates/spl-transport/src/journal_bridge.rs:511-517`.
3. Attribution: `BridgePolicy.attribution_headers` receives the unfiltered
   authorized request at
   `crates/spl-transport/src/journal_bridge.rs:164-175` and is merged before
   upstream at `crates/spl-transport/src/journal_bridge.rs:525-533`.
4. Resident startup: Solstone has `ResidentHandler` and `ResidentCommand` at
   `core/crates/solstone-core-sol-client/src/resident.rs:6-40`, and
   `run_resident_command` renders the `Err(CommandOutput)` arm as buffered
   output while printing `Ok(ResidentCommand)` startup before serving at
   `core/crates/solstone-core-sol/src/lib.rs:692-718`.

Relay token persistence is not a fifth gap. Python does not persist relay
tokens for serve. It calls `client.enroll_device(relay_url, identity)` during
dial at `solstone/think/link/dialer.py:127-134`, using the bundle's
`home_attestation.jwt` each run. Native serve must match that: enroll at serve
startup through SPL's public `enroll_device`, build an in-memory `Credential`
with `relay_origin` plus `device_token`, and never write those fields into the
join bundle. Persisting them would diverge from Python and is out of scope.

Constraints:

- New Rust files keep SPDX headers and `unsafe_code = "forbid"` remains
  unchanged (`core/Cargo.toml:24-25`).
- No second SPL adapter crate; extend `solstone-core-sol-link`.
- No second `check-rust-ios` exclusion. `solstone-core-sol-client` and
  `solstone/think/native/link/command.rs` stay free of SPL, Tokio, rustls, and
  host networking because they are in the iOS canary (`Makefile:206`).
- Do not reimplement SPL-owned mux, header allow-listing, hop-by-hop filtering,
  request parsing, response transforms, or proxy framing.
- Python `solstone/think/link/serve_cli.py` and the Python CLI boundary stay
  working and unchanged through this arc.
- Compat boundary files from the cutover scope stay untouched.

## D1. Resident Inventory Lane

Decision: use option (a), a generator-level resident boundary.

Add an authority entry for `link.serve` in
`core/native-sol/think/native/link/authority.toml` with the normal top-level link
shape plus one entry-local marker: `resident = true`. Missing or false
`resident` means the existing buffered boundary. The `link.serve` authority fields
are:

- `surface = "sol-link"`
- `path = ["link", "serve"]`
- `kind = "top-level"`
- `operation_id = "link.serve"`
- `entry_type = "top-level-link"`
- `handler = "link_serve"`
- `resident = true`

`scripts/build_native_sol_inventory.py` must parse `resident` as a boolean,
default false, reject non-boolean values, and route resident entries into a new
generated `RESIDENT_HANDLERS` table. It should reject `resident = true` on
`surface = "sol-call"` or `kind != "top-level"`; this boundary is for resident
process commands, not HTTP/app callbacks.

The generated inventory should still emit one `InventoryEntry` per authority
entry. Add `resident: bool` to `InventoryEntry` so accessors can map entries to
the correct handler table without relying on parallel arrays of equal length.
The generated `HANDLERS` table contains only non-resident entries, and
`RESIDENT_HANDLERS` contains only resident entries. Existing generated code emits
one `HANDLERS` binding per entry today at
`scripts/build_native_sol_inventory.py:330-346`.

`core/crates/solstone-core-sol-client/src/aggregate.rs` changes:

- Import `ResidentHandler`.
- Keep `pub type Handler = for<'a> fn(CommandContext<'a>) -> CommandOutput`
  unchanged at `aggregate.rs:23`.
- Add `resident_handler_bindings() -> &'static [ResidentHandler]`.
- Add `resident_handler_for(path) -> Option<(&'static InventoryEntry,
  ResidentHandler)>`.
- Update `handler_for(path)` to skip resident entries and index into the
  buffered table.

`core/crates/solstone-core-sol/tests/resident.rs:162-166` changes:

- The length assertion becomes buffered handler count plus resident handler
  count equals `aggregate::entries().len()`.
- Retain `assert_buffered_handler_slice(handlers)` exactly as the compile-time
  proof that the buffered boundary stays `&'static [Handler]`.
- Add a mirror helper taking `&'static [ResidentHandler]` and assert
  `aggregate::resident_handler_bindings()` passes it.

Rejected alternative: a separate buffered `link_serve` handler plus a resident
handler for the same command. `ResidentHandler` is
`fn(CommandContext) -> Result<ResidentCommand, CommandOutput>`, and
`run_resident_command` renders the `Err` arm through the ordinary buffered
path at `core/crates/solstone-core-sol/src/lib.rs:703-705`. Therefore `--help`,
argv/usage errors, port-range refusal, and bundle-resolution failures all use
the real resident handler and return buffered output before the serve loop can
exist. A second buffered entry point would duplicate command parsing and create
a cutover landmine where one path must refuse real invocations.

`dispatch_sol_link_with_seams` must not hardcode `["link", "join"]`. It should
derive `["link", "<verb>"]` from argv:

- Accept full argv `["link", "<verb>", ...]`.
- Accept parity-style argv `["<verb>", ...]`.
- Resolve the candidate path against generated inventory for `surface =
  "sol-link"`.
- Pass only the remaining option argv into `CommandContext.args`.

Its return type changes from `CommandOutput` to
`LinkDispatch { Buffered(CommandOutput), Resident { handler: ResidentHandler,
args: Vec<String> } }`. The dispatcher resolves the path and returns the
trimmed argv for resident commands. The `-cli` crate may resolve and return a
resident outcome for parity and seam callers, but it must not run the resident
loop or invoke the resident handler in production. Public `sol link` production
dispatch stays on the Python compatibility path in this checkout, matching
`link.join`; the cutover checkout owns wiring `solstone-core-sol` to build the
resident `CommandContext` and call `run_resident_command`.

`core/crates/solstone-core-sol-client-cli/src/bin/resolve_parity_leaves.rs:47`
must stop hardcoding `"sol-link" => ["link", "join"]`. It should use the same
argv-derived link-path helper as the link dispatcher so `link.join` and
`link.serve` resolve independently.

`FINAL_TOP_LEVEL_LINK_TOTAL` moves from `1` to `2` in
`scripts/build_native_sol_inventory.py:55`; the coverage recheck at
`scripts/check_native_sol_coverage.py:162-166` then expects both `link.join` and
`link.serve`.

Add `core/fixtures/native-sol/parity/link_serve.jsonl`, include it in
`core/crates/solstone-core-sol-client-cli/tests/parity.rs` beside
`LINK_JOIN_VECTORS` at `parity.rs:41`, and chain it in the test at
`parity.rs:72`.

Parity is structurally prevented from entering the serve loop this way:

- The parity harness resolves the real resident handler.
- For resident vectors it invokes the handler only to observe
  `Err(CommandOutput)`.
- If a vector ever returns `Ok(ResidentCommand)`, the parity harness fails the
  test instead of calling `ResidentCommand::serve`.
- Therefore `--help` and failure vectors exercise the real implementation, while
  successful resident duty cannot block the parity test process.

The success vector is `sol link serve --help`, exit 0, empty transport requests,
and the confirmed 638-byte stdout. At least one failure vector is required for
the success/failure coverage bucket; `scripts/check_native_sol_coverage.py`
accepts success at `:429-443` and failure at `:411-426`.

## D2. Serve Seam Types

Decision: add plain owned link-serve seam types to
`core/crates/solstone-core-sol-client/src/seam.rs`; keep SPL, Tokio, rustls, and
network handles in `solstone-core-sol-link`.

Client-seam data:

- `LinkServeEndpoint`: host string and port.
- `LinkServeBundle`: private key PEM, client cert PEM, CA chain PEM list, home
  attestation string, instance id, home label, and local endpoints.
- `LinkServeRequest`: label, requested port, direct flag, optional relay origin,
  and bundle.
- `LinkServeStatusSnapshot`: the nine status fields serialized by the local
  status route.
- `LinkServeFailure`: reason, detail, and timestamp for sanitized status
  reporting.
- `LinkServeErrorKind`: invalid bundle, invalid relay URL, relay enrollment
  failed, bind failed, runtime unavailable, bridge start capability failure,
  transport failure, shutdown failure, and unsupported platform.
- `LinkServeError`: kind plus optional status code or endpoint enum where needed;
  no raw peer body, nonce, certificate body, token, or pair-link fragment.

Runner seam:

- `LinkServeRunner: Send + Sync`.
- `LinkServeRunner::start(request)` binds and starts the bridge, then returns a
  `LinkServeSession`.
- `LinkServeSession` exposes `bound_port()` and a blocking `serve(self,
  shutdown: &dyn ShutdownSignal)` method that returns a `CommandOutput`-shaped
  result through `LinkServeError`.

This split is load-bearing. The resident handler must bind before building the
startup line, but the client crate must not know Tokio. The SPL adapter can
build a Tokio runtime inside `start`, call `journal_bridge::start`, retain the
handle/runtime in the returned session, and block in `serve` until
`ShutdownSignal::wait()` fires.

`CommandContext` gains `link_serve: Option<&dyn LinkServeRunner>`, mirroring
`link_pairing`. `LinkDispatchSeams` gains the same field. Parity/unit tests pass
`ScriptedLinkServeRunner`; the cutover checkout will wire production
`solstone-core-sol` to pass `SplLinkServeRunner`.

Scripted test surface:

- `ExpectedLinkServeCall` records an expected `LinkServeRequest` and either a
  started session or `LinkServeError`.
- `RecordedLinkServeCall` records the request.
- `ScriptedLinkServeRunner` has `new`, `assert_done`, and `recorded`, matching
  `ScriptedLinkJoinPairingSeam` at
  `core/crates/solstone-core-sol-client/src/seam.rs:265-313`.
- The scripted session returns a configured `serve_result`; refusal and parity
  tests fail before constructing a session, while resident-duty tests can drive
  `serve` with a fake `ShutdownSignal`.

Adapter responsibilities in `core/crates/solstone-core-sol-link`:

- Load SPL v0.3.0 through the existing adapter crate; update the workspace SPL
  pins from v0.1.0 at `core/Cargo.toml:52-53`.
- Convert `LinkServeBundle` into `spl_transport::credential::Credential`.
- Compute the first CA certificate fingerprint prefix as the join adapter does
  at `core/crates/solstone-core-sol-link/src/lib.rs:77-95`.
- For non-direct relay, call public `enroll_device(relay_origin, instance_id,
  home_attestation)` and place the returned token only in the in-memory
  credential.
- Construct `TransportClient` and a `CarrierOpener`.
- Configure and start `journal_bridge::start`.
- Own the Tokio runtime and bridge session lifetime.

## D3. Bind, Startup, And Signals

Decision: the seam returns a started session plus bound port before serving.

`journal_bridge::start` owns the listener and binds IPv4 loopback
unconditionally at spl-rust v0.3.0
`crates/spl-transport/src/journal_bridge.rs:370`. Its handle exposes `port()` but
not a full `SocketAddr` at `crates/spl-transport/src/journal_bridge.rs:381-384`
and `:244-248`.

The resident sequence is:

1. `run_resident_command` installs the SIGINT/SIGTERM mask.
2. The `link_serve` resident handler parses argv, resolves the bundle, resolves
   relay mode, and calls `LinkServeRunner::start`.
3. The runner starts SPL bridge and returns a session with the actual bound
   port.
4. The handler formats startup bytes and returns `Ok(ResidentCommand)`.
5. The runner prints and flushes startup.
6. The serve closure blocks on shutdown and drops the bridge session.

The exact native startup bytes are:

- `forwarding 127.0.0.1:{port} -> home {label} over pl\n`

Python writes the same message through `LOG.info` at
`solstone/think/link/serve_cli.py:122-127`. Native writes it to stdout through
the resident runner at `core/crates/solstone-core-sol/src/lib.rs:708-717`. This
is `expected-differs`: timing matches resident expectations, but stream and log
format differ.

Honor `resident-command-design.md` D4: the runtime must be built inside
the resident handler, never while constructing global process seams. Threads
inherit the creator thread's signal mask, and `run_resident_command` blocks
SIGINT/SIGTERM before calling the handler at
`core/crates/solstone-core-sol/src/lib.rs:692-705`. If a future cutover creates
the Tokio runtime or its worker threads before the resident handler runs, they
inherit the unblocked mask and can observe process signals outside the resident
shutdown contract.

Port handling:

- Product `--port` validates `1..=65535`, matching Python at
  `solstone/think/link/serve_cli.py:195-198`.
- `BridgePolicy.port` uses that resolved port and is never zero in product
  code. Port zero remains test-only in resident infrastructure.
- Bind failures map before startup output, through `Err(CommandOutput)`.

## D4. Status Payload

Decision: preserve the nine-key sorted JSON shape. Use SPL status facts where
available and one small consumer-side tracker where it materially helps owner
debugging. Every non-faithful field is `expected-differs`.

Python serializes the status body with `sort_keys=True` at
`solstone/think/link/serve_cli.py:378-386`, and emits these keys from
`TunnelClient._status_snapshot` at `solstone/think/link/dialer.py:680-724`.
SPL v0.3.0 exposes only `listener_active`, `contacted`, `carrier_live`, and
`active_requests` at
`crates/spl-transport/src/journal_bridge.rs:64-75`.

Status mapping:

| Key | Native source | Marker and rationale |
| --- | --- | --- |
| `health` | `healthy` when `listener_active && carrier_live`, else `unhealthy` | `expected-differs`; Python also requires manager task and live session at `dialer.py:686-701`. |
| `state` | `connected` if `carrier_live`; `disconnected` if listener active without carrier; `closed` if listener inactive | `expected-differs`; SPL has no connecting/degraded/dead-manager enum. |
| `manager_alive` | `listener_active` | `expected-differs`; native bridge has a listener fact, not Python's asyncio manager-task fact. |
| `connected_age_seconds` | derived from the shared consumer-side tracker's `last_connected_at` when `carrier_live` | `expected-differs`; close enough for owner age display, not Python's session timestamp. |
| `last_connected_at` | same consumer-side tracker | `expected-differs`; native timestamp is carrier-open time. |
| `last_failure` | consumer-side tracker around `CarrierOpener::dial_carrier` errors, sanitized through the serve error mapper | `expected-differs`; retained because it is load-bearing owner diagnostics. |
| `next_retry_at` | always `null` | `expected-differs`; SPL retry schedule is private. |
| `reconnect_count` | shared consumer-side tracker count of failed or replacement carrier opens | `expected-differs`; useful diagnostic, not Python's exact lifecycle counter. |
| `active_requests` | `JournalBridgeStatus.active_requests` | `expected-differs`; SPL counts accepted connection tasks, including the status request itself, at `journal_bridge.rs:128-142`, not Python's tunnel request counter. |

`local_response` receives `(&RequestHead, &JournalBridgeStatus)` and can return
status, content type, and body only:
`crates/spl-transport/src/journal_bridge.rs:53-62` and
`crates/spl-transport/src/journal_bridge.rs:160-163`. That is sufficient for
`Content-Type: application/json` and an exact `Content-Length`, because SPL
writes both at `crates/spl-transport/src/journal_bridge.rs:876-884`. It cannot
set arbitrary headers.

The status route is never forwarded upstream because the `local_response`
closure checks `RequestHead::path()` for `/_solstone/link/status` and returns
`Some(LocalResponse)` only for that path. SPL returns immediately after writing
the local response at `crates/spl-transport/src/journal_bridge.rs:511-517`;
every other path returns `None` and proceeds to upstream forwarding at
`crates/spl-transport/src/journal_bridge.rs:519-535`.

Clock and update rule: the adapter uses one small internal clock seam for the
status tracker. This is deliberately not `CommandContext.clock`: SPL's
`BridgePolicy` hooks are stored as `Arc` values on the bridge runtime, so
borrowing the command clock into the local status closure would complicate the
resident session lifetime for no product benefit. Production uses a system
clock; adapter tests inject a fake clock. The tracker is exactly one shared
struct updated only around the `CarrierOpener::dial_carrier` success/failure
point. `connected_age_seconds` is derived when rendering status; no other
adapter path may mutate those four consumer-side fields.

## D5. Relay URL Resolution

Decision: native serve uses command/env/default relay resolution and does not
read journal config.

Python precedence is `SOL_LINK_RELAY_URL` > journal config `link.relay_url` >
default `https://link.solstone.app` at `solstone/think/link/paths.py:76-98`,
with `--relay-url` overriding before that at
`solstone/think/link/serve_cli.py:183-186`.

Native precedence:

1. If `--direct` is set, relay origin is `None`; `--relay-url` and
   `SOL_LINK_RELAY_URL` are ignored, matching Python's direct branch at
   `solstone/think/link/serve_cli.py:111-113`.
2. If `--relay-url` is nonblank, strip and trim trailing slashes.
3. Else if `ctx.env["SOL_LINK_RELAY_URL"]` is nonblank, strip and trim trailing
   slashes.
4. Else use `https://link.solstone.app`.

The journal-config leg is intentionally omitted and marked `expected-differs`.
`sol link serve` is a satellite/observer command; requiring a local journal
config would couple relay selection to state a satellite may not have. The
existing `ctx.journal_root: Option<&Path>` remains available for link join peer
cases, but serve must not require it for relay resolution.

Relay enrollment:

- Non-direct native serve calls SPL `enroll_device` at serve startup, using the
  resolved relay origin, bundle `instance_id`, and bundle `home_attestation`.
- The returned device token is placed in memory only.
- `device_token_expires_at` remains `None` unless SPL exposes expiry in a later
  API; v0.3.0 `enroll_device` returns only the token at
  `crates/spl-transport/src/relay_pairing.rs:143-170`.
- If enrollment fails, the resident handler returns buffered failure output
  before startup. This differs from Python's background connection manager,
  which records failures and retries at `solstone/think/link/dialer.py:277-315`;
  marker `expected-differs`.

## D6. Direct Provability

Decision: prove `--direct` at the Solstone seam boundary and rely on SPL's
structural relay gate inside `TransportClient`.

SPL v0.3.0 enters relay fallback only when LAN is unreachable and
`relay_eligible()` is true at `crates/spl-transport/src/client.rs:179-190`.
`relay_eligible()` requires both `credential.relay_origin.is_some()` and
`device_token.is_some()` at `crates/spl-transport/src/client.rs:195-197`.
`dial_relay_carrier` remains `pub(crate)` at
`crates/spl-transport/src/relay.rs:445-455`, so there is no public SPL hook that
can count relay attempts directly.

AC 6 therefore has two required halves:

1. Command seam test: use `ScriptedLinkServeRunner` to capture
   `LinkServeRequest` for `--direct` with a poisoned `SOL_LINK_RELAY_URL` and a
   supplied `--relay-url`. Assert `relay_origin` is `None`, the direct flag is
   true, and the bundle carries no relay token or expiry.
2. Adapter enrollment test: construct a direct `LinkServeRequest` and run it
   through a test-only `SplLinkServeRunner` with a fake relay enrollment seam.
   Assert the enrollment recorder is empty. This proves direct mode never even
   asks for a relay token; a test that only checks a boolean flag is insufficient.

Together those tests make a relay attempt observable at the boundary where
Solstone still has control, without reimplementing or instrumenting SPL's
private relay dial.

## D7. Error Mapping

Decision: add serve-specific exhaustive mappings beside the existing join
mapping shape in `core/crates/solstone-core-sol-link/src/lib.rs:98-150` and its
tests at `core/crates/solstone-core-sol-link/src/lib.rs:386-506`.

Serve text differs from join text because join is a pairing ceremony and serve
is a resident proxy. Serve errors should tell the owner how to restore reachability
or re-pair; they must not mention pair-code parsing unless the SPL variant is
literally `PairLink`.

Transport mapping:

| SPL variant | Serve text |
| --- | --- |
| `Io` | `Link transport I/O failed while serving. Check that the journal is reachable on LAN/VPN or relay, then retry.` |
| `Tls` | `Secure link handshake failed. Re-run sol link join if the journal certificate or pairing changed.` |
| `Crypto` | `Link credential material is invalid. Re-run sol link join for this observer.` |
| `Mux` | `SPL stream framing failed while serving. Retry; re-pair if it continues.` |
| `Http` | `The journal response over the link could not be parsed. Update both peers or retry.` |
| `Json` | `Relay or bridge JSON could not be parsed. Check the relay URL and retry.` |
| `PairLink` | `Stored pairing data is invalid. Re-run sol link join for this observer.` |
| `Pairing` | `Link credential or relay enrollment failed. Re-run sol link join if retrying does not fix it.` |
| `Rejected { status, body }` | `The paired journal rejected the link request with HTTP {status}.` Drop `body`. |
| `Relay(HomeOffline)` | `The relay reports the home journal is offline. Start the journal or use --direct on LAN/VPN.` |
| `Relay(Unauthorized)` | `The relay rejected this observer token. Re-run sol link join for this observer.` |
| `Relay(Unpaid)` | `The relay account is not available. Check relay service/account status or use --direct.` |
| `Relay(UnknownInstance)` | `The relay does not know this journal instance. Re-run sol link join.` |
| `Relay(PairWindowClosed)` | `The relay pairing window is closed. Re-run sol link join from a fresh code.` |
| `Relay(Overflow)` | `The relay is temporarily overloaded. Retry or use --direct on LAN/VPN.` |
| `Relay(Abnormal)` | `The relay connection closed abnormally. Retry or use --direct on LAN/VPN.` |
| `Relay(UpgradeRejected)` | `The relay rejected the WebSocket upgrade. Check --relay-url and retry.` |
| `Relay(Stalled)` | `The relay connection stalled. Retry or use --direct on LAN/VPN.` |
| `RelayControlRejected { endpoint: EnrollDevice, status }` | `Relay enrollment was rejected with HTTP {status}. Re-run sol link join if the bundle attestation is stale.` |
| `RelayControlRejected { endpoint: TokenRefresh, status }` | `Relay token refresh was rejected with HTTP {status}. Re-run sol link join for this observer.` |
| `NoEndpoint` | `No journal endpoint is available. Re-run sol link join or pass --relay-url unless using --direct intentionally.` |
| `NotPaired` | `Link credentials are missing. Run sol link join before sol link serve.` |
| `LocalOffset` | `Local offset lookup failed. Check the system clock and retry.` |

The match must have no wildcard arm. The compile-time risk is intentionally
carried by the exhaustive `match`; tests enumerate every current variant so a
new SPL variant fails visibly either at compile time or by missing case coverage.

Bridge start mapping:

- `BridgeStartError::Capability`: should be unreachable because
  `BridgePolicy.capability_gate` is disabled. If it occurs, emit
  `Native link bridge setup failed before serving. Retry after reinstalling solstone-core.`
  with tempfail semantics.
- `BridgeStartError::Bind(error)` with `ErrorKind::AddrInUse`: emit Python's
  specific guidance, `cannot bind 127.0.0.1:{port}: address already in use.
  Another sol link serve or Convey may already be using that port.`, matching
  `solstone/think/link/serve_cli.py:201-207`.
- Other bind errors: `cannot bind 127.0.0.1:{port}: bind failed`; do not include
  path-like or OS strings if they can carry unexpected data.

## D8. Expected-Differs

Carry-forward decisions from `09-link-serve-design.md` D7, rejudged for
SPL v0.3.0:

1. Help: `matches-python`. Keep the 638-byte help oracle and 114-byte usage
   prefix.
2. Port validation: `matches-python`. Reject outside `1..=65535` before bind.
3. Bind address: `matches-python`. SPL binds `127.0.0.1`.
4. Startup line: `expected-differs`. Native timing matches resident semantics,
   but native emits stdout while Python uses logging.
5. Request method/path/body: `matches-python`. SPL bridge forwards these.
6. Request headers: `expected-differs`. `ForwardAll` preserves non-reserved
   headers, but SPL strips `Host`, `Content-Length`, caller auth, and hop-by-hop
   at `spl-core/src/bridge.rs:299-308`; Python forwards `Host` and sets
   `Content-Length` at `serve_cli.py:408-419`.
7. Response header allow-list: `expected-differs`. Do not reimplement SPL's
   response filtering to recover Python's broader pass-through behavior.
8. `Set-Cookie`: `expected-differs`. SPL cookie transforms remain SPL-owned.
9. Upstream `Host`: `expected-differs`. SPL strips it; Python tests assert it is
   forwarded today.
10. Connection model: `expected-differs`. SPL writes `Connection: close` in
    local responses at `journal_bridge.rs:876-884`; Python's server behavior is
    not identical.
11. `Host: localhost:5015`: `expected-differs`. Capability disabled still
    checks loopback host at `journal_bridge.rs:487-499`; SPL exact-host behavior
    is retained.
12. Chunked request body: `expected-differs`. Python rejects request
    `Transfer-Encoding` before tunnel at `serve_cli.py:311-316`; SPL request
    parsing owns this behavior.
13. Local status route: flipped to `matches-python` for routing. v0.3.0
    `local_response` can answer status locally and skip upstream.
14. Status payload: `expected-differs`. The nine-key shape is preserved, but
    several values are approximated or null because SPL exposes four facts.
15. Relay mode: flipped to `matches-python` for capability. v0.3.0 public
    enrollment lets native serve create the in-memory relay token at startup.
    Enrollment timing and retry semantics remain `expected-differs`.
16. Observer attribution: `expected-differs`. Native serve installs no
    `attribution_headers` hook. Keep real `BridgeNames.observer_header_name` and
    `protocol_version_header_name` lowercase as SPL requires at
    `spl-core/src/bridge.rs:21-30`. Do not decoy them to smuggle raw caller
    headers past reserved stripping. Client-supplied
    `X-Solstone-Observer` and `X-Solstone-Protocol-Version` do not survive SPL's
    reserved-header strip.
17. Streaming body delivery: `matches-python` for incremental body delivery when
    `stream_response` is true for all requests. Header framing remains
    `expected-differs`.
18. Gateway/lifecycle errors: `expected-differs`. Python has JSON lifecycle
    503s at `serve_cli.py:362-376`; SPL/adapter errors use serve-specific
    sanitized native text.

New differences introduced by this design:

19. Relay URL journal config leg: `expected-differs`. Native serve ignores
    journal config to keep satellite operation independent of a local journal.
20. Resident clean shutdown: `expected-differs`. Native SIGINT/SIGTERM exits
    cleanly through the resident boundary; Python catches `KeyboardInterrupt` only
    for foreground interruption at `serve_cli.py:128-136`.
21. Relay enrollment failure timing: `expected-differs`. Native enrollment
    failure happens before startup; Python's manager records/retries failures in
    the background.

Per-request attribution delivered by this design:

- Native installs no attribution hook in this checkout.
- Native does not deliver per-request client-supplied
  `X-Solstone-Observer` or `X-Solstone-Protocol-Version`; SPL strips those
  reserved names before upstream forwarding.
- Consumer authentication headers remain required and are injected through
  `CarrierOpener::proxy_headers`, which is separate from attribution.
- Richer attribution is an out-of-scope follow-up.

## D9. Test Plan

No validation commands run in this design stage. Implementation must add these
named tests.

AC 1, `make ci` green:

- Non-test gate requirement. Run `make ci` once on the settled tree after
  focused checks because this arc bumps a pinned dependency, refreshes the lock,
  regenerates native inventory, and moves gate constants.

AC 2, SPL pins at `tag = "v0.3.0"`:

- Non-test source assertion through review and lock inspection. `core/Cargo.toml`
  must pin both `spl-core` and `spl-transport` to `tag = "v0.3.0"` and
  `core/Cargo.lock` must resolve both to
  `6986cf75514f67894469bc395d44801f6d8793ba`.

AC 3, native implementation and no Python subprocess:

- `make check-native-sol-no-python-spawn`.
- `core/crates/solstone-core-sol-client-cli/src/lib.rs` unit test
  `sol_link_dispatch_resolves_join_and_serve_from_full_or_trimmed_argv`, proving
  generated native inventory resolves `link serve` for native parity/seam
  callers without invoking Python.
- Production `sol link` remains the Python compatibility-dispatched path until
  cutover; this checkout does not claim no-Python production dispatch for public
  `sol link serve`.

AC 4, compat boundary untouched:

- Non-test source constraint: do not modify `solstone/think/sol_compat_inventory.py`,
  `solstone/think/sol_compat_cli.py`, the `SOLSTONE_NATIVE_COMPAT_ACTIVE`
  sentinel, or `TOP_LEVEL_COMPAT_COMMANDS`.

AC 5, loopback bind assertion:

- `core/crates/solstone-core-sol-link/tests/sol_link_serving.rs::status_request_does_not_open_carrier_but_ordinary_request_does`
  binds the loopback listener on port `0`, connects to the OS-assigned
  `handle.port()` at `127.0.0.1`, and checks the peer socket fact that is
  observable from the connection.
- `core/crates/solstone-core-sol-link/src/serve.rs::solstone_adapter_adds_no_wildcard_bind_host_literal`
  asserts the new adapter code adds no wildcard or named-loopback bind host.
- Limit: in a single-interface container, connect-and-check alone can be weak;
  the paired source-shape assertion catches accidental introduction of a
  configurable or wildcard bind in Solstone code. SPL's own bind literal remains
  separately evidenced at `journal_bridge.rs:370`.

AC 6, `--direct` provability:

- `solstone/think/native/link/command.rs::serve_direct_omits_relay_even_with_poisoned_relay_inputs`.
- `core/crates/solstone-core-sol-link/src/serve.rs::direct_credentials_have_no_relay_fields_and_do_not_enroll`.

AC 7, status path local only:

- `core/crates/solstone-core-sol-link/src/serve.rs::bridge_policy_status_is_local_and_attribution_hook_is_empty`
  asserts the nine-key JSON status body and no attribution output.
- `core/crates/solstone-core-sol-link/tests/sol_link_serving.rs::status_request_does_not_open_carrier_but_ordinary_request_does`.
  Use a counting fake `CarrierOpener`; status request asserts zero opens and
  local JSON response headers.
- The same fixture drives a non-status request and asserts the carrier-open
  counter increments. This positive control prevents a miswired or lazily unused
  opener from making the zero-open assertion vacuous.

AC 8, bundle selection including ambiguous labels:

- `solstone/think/native/link/command.rs::serve_bundle_resolution_names_sorted_labels_and_supports_single_default`.
  Cover explicit label, single-bundle default, and multiple bundles.
- The multiple-bundle assertion must prove the error names the available labels
  in sorted order and instructs `--label`.

AC 9, streaming latch:

- `core/crates/solstone-core-sol-link/tests/sol_link_serving.rs::proxied_response_streams_before_upstream_completion`.
  Start a real in-process `journal_bridge::start` with a local SPL transport
  peer. The peer writes the response head and a first body chunk beginning with
  byte `A`, then blocks on a test-owned latch before writing `B` and closing.
  The client must read `A` over the loopback bridge before the latch is
  released, then read `B` after release.
- This fails against a buffering implementation because the first client body
  read cannot complete until the upstream producer emits `B` or closes.

AC 10, exhaustive transport mapping:

- `solstone/think/native/link/command.rs::serve_transport_error_text_covers_every_variant_without_secret_leaks`.
  Mirror the join tests at `core/crates/solstone-core-sol-link/src/lib.rs:386-506`.
  A new upstream variant should fail the exhaustive match at compile time; the
  runtime table also catches missing nested relay variants or leaked substrings.

AC 11, exit code parity:

- `solstone/think/native/link/command.rs::serve_argv_errors_exit_two_before_starting`
  asserts exit `2` for argv/usage errors, including unknown flag, missing value,
  and non-integer port.
- `solstone/think/native/link/command.rs::serve_non_argv_failures_exit_one`
  asserts exit `1` for non-argv failures:
  missing bundle, ambiguous bundle, bind failure, and relay enrollment failure.
- `solstone/think/native/link/command.rs::serve_help_is_python_byte_exact`
  asserts exit `0` for `--help`; `serve_bundle_resolution_names_sorted_labels_and_supports_single_default`
  drives a clean fake shutdown through `ResidentCommand::serve`.
- Existing resident fixture tests in `core/crates/solstone-core-sol/tests/resident.rs`
  keep SIGINT/SIGTERM resident shutdown exit `0`.

AC 12, `expected-differs` declared:

- Non-test documentation requirement. This record's D8 is the authoritative
  expected-differs list and must be updated with any implementation-time
  behavior change.

AC 13, SPDX and lints:

- Non-test source plus gate requirement. New Rust files have the two-line SPDX
  header, no `unsafe`, and workspace lints remain unchanged. `make
  check-rust-fmt`, `make check-rust-clippy`, and `make check-rust-ios` are the
  focused gates.

AC 14, help oracle:

- `solstone/think/native/link/command.rs::serve_help_is_python_byte_exact`
  mirrors the join help assertion at `command.rs:1095-1104`, asserting
  638-byte full help and 114-byte usage prefix.
- `core/fixtures/native-sol/parity/link_serve.jsonl` contains the same help
  stdout, exit 0, and empty transport requests.

Additional implementation tests:

- Resident boundary inventory:
  `tests/test_native_sol_inventory.py::test_resident_entries_generate_resident_handlers`
  asserts `resident = true` routes `link_serve` into `RESIDENT_HANDLERS`.
- Resident static slice proof:
  `core/crates/solstone-core-sol/tests/resident.rs::resident_fixture_is_absent_from_inventory_and_handlers_are_buffered`
  updated to assert buffered plus resident length equals entries and both static
  slice types compile.
- Relay enrollment at serve time:
  `core/crates/solstone-core-sol-link/src/serve.rs::relay_credentials_enroll_at_serve_time_in_memory`
  asserts relay origin, instance id, and home attestation are passed to the fake
  enrollment seam, and no token file is written.
- Bridge policy:

- `core/crates/solstone-core-sol-link/src/serve.rs::bridge_policy_status_is_local_and_attribution_hook_is_empty`.
  Assert `port` is requested port, streaming predicate true for representative
  requests, local response status hook installed, and no attribution output.
- Resident startup timing:

- Existing `core/crates/solstone-core-sol/tests/resident.rs` resident fixture
  tests assert startup stdout is visible before process completion and
  SIGINT/SIGTERM exit code is 0.
- Parity resolver and no-loop parity:

- `core/crates/solstone-core-sol-client-cli/src/lib.rs::sol_link_dispatch_resolves_join_and_serve_from_full_or_trimmed_argv`.
- `core/crates/solstone-core-sol-client-cli/tests/parity.rs` includes
  `LINK_SERVE_VECTORS` and fails if any link serve parity vector returns
  `Ok(ResidentCommand)`.

## D10. Implementation Sequence And File Manifest

Sequence:

1. Bump SPL pins in `core/Cargo.toml` and `core/Cargo.lock` to v0.3.0, with any
   `core/deny.toml` source-policy adjustment required by the lock update.
2. Extend the generated inventory model for `resident = true`, add
   `RESIDENT_HANDLERS`, aggregate accessors, and resident static-slice tests.
3. Add link-dispatch return enum, argv-derived link resolution, and parity
   resolver changes; do not route public `sol link serve` to native production
   dispatch in this checkout.
4. Add client seam types, `CommandContext.link_serve`, and scripted runner.
5. Add `link_serve` resident handler: argparse-compatible help/errors, port
   validation, bundle loading, relay resolution, direct enforcement, startup
   formatting, and `Err(CommandOutput)` refusal paths.
6. Add SPL serve adapter in `solstone-core-sol-link`: runtime/session split,
   relay enrollment, credential construction, bridge policy, status tracker,
   explicit no-attribution-hook policy, and error mapping.
7. Add parity vectors and focused tests.
8. Regenerate inventory and native fixtures through existing repository
   commands during implementation, not in this design stage.

Files to create or touch:

- `docs/design/native-sol-client/11-link-serve-design.md`
- `docs/PORTING.md`
- `core/native-sol/think/native/link/authority.toml`
- `solstone/think/native/link/command.rs`
- `scripts/build_native_sol_inventory.py`
- `core/Cargo.toml`
- `core/Cargo.lock`
- `core/crates/solstone-core-sol-client/src/aggregate.rs`
- `core/crates/solstone-core-sol-client/src/command.rs`
- `core/crates/solstone-core-sol-client/src/generated/inventory.rs`
- `core/crates/solstone-core-sol-client/src/seam.rs`
- `core/crates/solstone-core-sol-client-cli/src/lib.rs`
- `core/crates/solstone-core-sol-client-cli/src/bin/resolve_parity_leaves.rs`
- `core/crates/solstone-core-sol-client-cli/tests/parity.rs`
- `core/crates/solstone-core-sol/Cargo.toml`
- `core/crates/solstone-core-sol/src/lib.rs`
- `core/crates/solstone-core-sol/src/bin/solstone-resident-fixture.rs`
- `core/crates/solstone-core-sol/tests/resident.rs`
- `core/crates/solstone-core-sol-link/src/lib.rs`
- `core/crates/solstone-core-sol-link/src/serve.rs`
- `core/fixtures/native-sol/parity/link_serve.jsonl`
- `solstone/apps/activities/native/command.rs`
- `solstone/apps/support/native/command.rs`
- `solstone/think/native/chat/command.rs`
- `solstone/think/native/import/command.rs`
- `solstone/think/native/notify/command.rs`
- `solstone/think/tools/native/health/command.rs`
- `tests/test_native_sol_conformance.py`
- `tests/test_native_sol_inventory.py`

Files explicitly not touched:

- `solstone/think/link/serve_cli.py`
- Python CLI compatibility boundary files from the cutover scope
- Any join bundle schema/persistence file solely to store relay tokens

## D11. Risks And Open Questions

- SPL's attribution hook remains intentionally unused. Native serve must not
  claim per-request observer attribution; richer attribution needs a separate
  design with explicit consumer semantics.
- Ignoring journal config for relay URL is an intentional satellite-friendly
  difference. If operators depend on `config/journal.json` relay overrides for
  observer hosts, they must move that setting to `SOL_LINK_RELAY_URL` or
  `--relay-url`.
- Native relay enrollment before startup is simpler and keeps SPL credential
  construction coherent, but it can fail before opening the local listener where
  Python would start and expose degraded status.
- AC 5 cannot prove SPL internals beyond public behavior and source-shape
  guardrails. It can prove Solstone did not add a wildcard/configurable bind and
  that the running bridge accepts the expected loopback endpoint.
- The resident parity harness must never call `ResidentCommand::serve`.
  Accidentally allowing an `Ok(ResidentCommand)` vector to proceed would hang or
  require a fake shutdown path that weakens the parity proof.
