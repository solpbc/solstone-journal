# observer-over-pl

## Summary

This lode adds a paired-link (`pl`) transport path to the observer client while
keeping the existing bearer-key HTTP (`dl`) path unchanged. The server-side
observer routes will resolve identity from `g.identity.fingerprint` for PL
requests and from bearer/url keys for DL requests. Observer record writes stay
owned by the observer app, even when pairing is initiated by the link app.

The design keeps DL as the default and requires explicit PL config. PL observer
records are keyed by 16 hex chars from the client certificate fingerprint;
existing DL records remain keyed by the first 8 chars of the bearer key.

## Decisions

### 1. Module lift layout

Chosen: lift `tests/link/client.py` to `solstone/think/link/client.py` and keep
`tests/link/client.py` as a thin re-export shim.

Rationale: all current imports are named imports from `tests.link.client`:
`tests/link/test_privacy_scan.py:13`, `tests/link/test_pl_sse.py:15`,
`tests/link/test_lan_direct.py:16`, `tests/link/test_integration.py:13`, and
`tests/link/test_identity_stamp.py:14`. A shim preserves those tests unchanged.
The lifted module should define `__all__` so the shim's star import includes the
private test helper `_http_request_bytes`. Keep `Client.pair` on the lifted
class because existing integration tests call it directly, for example
`tests/link/test_identity_stamp.py:37` and `tests/link/test_integration.py:42`.

The public exported names are:

- `Client`
- `ClientIdentity`
- `EncryptedTransport`
- `EnrolledDevice`
- `StreamResetError`
- `TlsError`
- `TunnelSession`
- `_http_request_bytes`

Add the requested top-of-module contract docstring in
`solstone/think/link/client.py`: `TlsError` means TLS handshake failure,
`ConnectionError`/`OSError` mean transport failure, and `OSError` can also mean
pre-dial credential-file errors.

### 2. Cross-app write routing

Chosen: `solstone/apps/link/routes.py:_complete_pairing()` will call
`solstone.apps.observer.utils.mint_pl_observer_record()` only when
`consumed.role == "observer"`.

Rationale: the current static linter will not catch a direct write from
`link/routes.py` into `journal/apps/observer/...`, because it only scans infra
scopes and read-verb app `call.py` handlers (`scripts/check_layer_hygiene.py:37-43`,
`scripts/check_layer_hygiene.py:123-135`, `scripts/check_layer_hygiene.py:203-214`)
and only targets entities/facets/observations (`scripts/check_layer_hygiene.py:61-70`).
The layer rule still applies: observer records are observer-app state, so the
observer app owns the writer.

The writer signature:

- `mint_pl_observer_record(fingerprint: str, device_label: str, paired_at: str) -> Path`

Record shape:

- `fingerprint`: full `sha256:<hex>` client certificate fingerprint.
- `mode`: `"pl"`.
- `name`: `device_label`.
- `paired_at`: ISO timestamp from `_complete_pairing()`.
- `created_at`: current `now_ms()` for compatibility with observer list sorting.
- `last_seen`: `None`.
- `last_segment`: `None`.
- `enabled`: `True`.
- `stats`: same counters as DL create, starting at zero.

The filename is `<fingerprint-prefix-16>.json`, where the prefix is the first
16 hex chars after stripping `sha256:`. The writer should refuse an existing
target path so rollback cannot delete a pre-existing record.

### 3. Pair-completion atomicity

Chosen: generate all cryptographic response material first, then perform the
two durable writes in order: observer record first, authorized client second.

Rationale: `/pair` consumes its nonce before `_complete_pairing()` at
`solstone/apps/link/routes.py:384-398`; `/by-code` does the same at
`solstone/apps/link/routes.py:424-437`. `NonceStore.consume()` writes the used
state immediately (`solstone/think/link/nonces.py:72-93`), so pairing is
single-use regardless of downstream failures.

Failure state machine:

| failure point | state after failure |
|---|---|
| bad CSR in `sign_csr()` | nonce consumed; no observer record; no AuthorizedClients entry. |
| response material generation fails before writes | nonce consumed; no observer record; no AuthorizedClients entry. |
| observer writer fails | nonce consumed; no AuthorizedClients entry. |
| `AuthorizedClients.add()` fails after observer write | delete the newly written observer record, then re-raise. |
| response serialization fails after both writes | pairing is durable; no rollback. |

`AuthorizedClients.add()` currently owns the paired-device ledger write at
`solstone/think/link/auth.py:84-107`. `_complete_pairing()` currently adds before
minting the attestation (`solstone/apps/link/routes.py:311-321`); move in-memory
attestation/response construction before durable writes so no expected code path
fails after the ledger write.

### 4. Identity-resolve helper

Chosen: add `resolve_observer_identity(url_key: str | None = None)` in
`solstone/apps/observer/utils.py`, returning:

- `record_or_none`
- `filename_prefix_or_none`
- `error_response_or_none`

Rationale: this is the smallest per-route diff while preserving exact error
reasons. A `record | None` helper would leave revoked/disabled checks duplicated
across seven routes. A result-sum type is clean but forces seven switch blocks.
The tuple result lets each route do one early-return check and then use the
prefix for history, stats, and SSE registration.

The helper reads `g.identity` directly. PL identity is already stamped by
Convey: `install_identity_stamper()` sets DL identity defaults at
`solstone/convey/__init__.py:95-108`, while the secure listener stamps
`request.environ["pl.identity"]` at `solstone/convey/secure_listener/wsgi.py:213-239`.
The identity shape is `mode`, `fingerprint`, `device_label`, `paired_at`, and
`session_id` (`solstone/convey/secure_listener/identity.py:12-18`).

DL branch:

- Extract bearer key from `Authorization: Bearer` first, then `url_key`, matching
  current `_get_key()` behavior at `solstone/apps/observer/routes.py:105-112`.
- Missing key returns `error_response(AUTH_REQUIRED, detail="Authorization required")`.
- Unknown/wrong key returns `error_response(AUTH_KEY_INVALID, detail="Invalid key")`.
- Revoked record returns `error_response(PL_REVOKED, detail="Observer revoked")`.
- Disabled record returns `error_response(FEATURE_UNAVAILABLE, detail="Observer disabled")`.
- Success returns the observer record plus its derived filename prefix.

PL branch:

- Applies when `g.identity.mode in {"pl-direct", "pl-via-spl"}`.
- Ignore `url_key`; route path keys are routing tokens only in PL mode.
- Missing fingerprint returns `AUTH_REQUIRED`.
- No observer record for the fingerprint returns `AUTH_REQUIRED`, not
  `AUTH_KEY_INVALID`. This preserves the implicit-role behavior: a phone-paired
  client is authorized as a paired device but not as an observer.
- Revoked and disabled use the same 403 responses as DL.
- Success returns the observer record plus the 16-char fingerprint filename prefix.

`callosum_sse()` also needs a way to re-check identity inside its generator.
The helper should either return a `reload` callable or expose a prefix/fingerprint
that the route can pass to `ObserverRegistry.by_prefix()` or `by_fingerprint()`.

### 5. Observer registry and lookup cache

Chosen: add a unified `ObserverRegistry` singleton in
`solstone/apps/observer/utils.py`. Keep `load_observer(key)` as a thin wrapper
for existing callers.

Rationale: `load_observer()` currently derives a filename from `key[:8]` and
verifies full-key equality (`solstone/apps/observer/utils.py:43-64`). That cannot
load PL records. A registry scan can validate both DL and PL records and expose
lookup by key, fingerprint, prefix, and name while keeping old callers stable.

Registry behavior:

- Track the observers directory mtime, mirroring the reload pattern used by
  `AuthorizedClients.reload_if_stale()` (`solstone/think/link/auth.py:63-77`).
- On reload, scan `get_observers_dir().glob("*.json")`.
- Validate each record as exactly one of:
  - DL: has non-empty `key`, has no `fingerprint`.
  - PL: has non-empty `fingerprint`, has no `key`.
- Warn-log and skip invalid records.
- Return shallow copies augmented with derived `filename_prefix` and `mode`.
- Invalidate/reload after `save_observer()`, `mint_pl_observer_record()`, and
  `increment_stat()`.

Accessors:

- `by_key(key: str) -> dict | None`
- `by_fingerprint(fingerprint: str) -> dict | None`
- `by_prefix(prefix: str) -> dict | None`
- `by_name(name: str) -> dict | None`
- `all() -> list[dict]`

Existing production callers of `load_observer()` are
`solstone/apps/observer/routes.py:207`, `:273`, `:297`, `:759`, `:885`, `:971`,
`:1006`, `:1060`, and `:1116`. Tests also call it directly in
`solstone/apps/observer/tests/test_utils.py` and observer SSE tests, so keeping
the wrapper reduces churn.

### 6. Width handling in observer infrastructure

Chosen: derive prefixes from records, not from local auth strings.

Rationale: history and stats helpers are already width-agnostic when passed a
prefix (`solstone/apps/observer/utils.py:28-40`, `:123-148`, `:164-190`). The
breakage is in filename derivation and route-local `auth_key[:8]`.

Required changes:

- Add `observer_filename_prefix(record)` and `_observer_filename(record)`.
- `save_observer(data)` uses `_observer_filename(data)`: DL returns
  `<key[:8]>.json`, PL returns `<fingerprint-prefix-16>.json`.
- `load_observer(key)` delegates to `ObserverRegistry.by_key(key)`.
- Add `load_observer_by_fingerprint(fingerprint)` for PL helper use.
- Route-local `key_prefix = auth_key[:8]` sites are replaced with the resolved
  prefix: `solstone/apps/observer/routes.py:283`, `:793`, `:819`, `:936`, `:981`,
  and `:1131`.
- `_serialize_observer()` derives prefix from the record at
  `solstone/apps/observer/routes.py:187-199`.
- Event handlers stop using `observer.get("key", "")[:8]` at
  `solstone/apps/observer/events.py:42` and `:87`.

### 7. Per-route PL URL synthesis

Chosen: PL clients use keyless URLs where they already exist for upload/event,
and use the resolved filename prefix in route URLs where the server has only a
keyed or routing-token form. In PL mode, the server ignores the route key for
identity and trusts `g.identity.fingerprint`.

Route confirmation:

- Upload has keyless and keyed forms at `solstone/apps/observer/routes.py:732-734`.
- Event has keyless and keyed forms at `solstone/apps/observer/routes.py:1044-1046`.
- SSE has only the keyed form via `OBSERVER_CALLOSUM_SSE_ROUTE` at
  `solstone/apps/observer/routes.py:78-80` and `:266-267`.
- Transfer has only keyed form at `solstone/apps/observer/routes.py:878-879`.
- Manifest has only keyed form at `solstone/apps/observer/routes.py:964-965`.
- Manifest-day has only keyed form at `solstone/apps/observer/routes.py:999-1000`.
- Segments has both keyless and keyed forms at `solstone/apps/observer/routes.py:1096-1098`.
  Even though a keyless segments route exists, PL should use the keyed
  `<fp-prefix-16>` URL for consistency with transfer/manifest and route logging.

PL paths:

| operation | PL client path |
|---|---|
| upload | `POST /app/observer/ingest` |
| event | `POST /app/observer/ingest/event` |
| SSE | `GET /app/observer/<fp-prefix-16>/callosum` |
| transfer | `POST /app/observer/ingest/<fp-prefix-16>/transfer` |
| manifest | `GET /app/observer/ingest/<fp-prefix-16>/manifest` |
| manifest-day | `GET /app/observer/ingest/<fp-prefix-16>/manifest/<day>` |
| segments | `GET /app/observer/ingest/<fp-prefix-16>/segments/<day>` |

### 8. Config schema

Chosen: PL mode requires `observe.observer.spl_relay_url`. There is no fallback
to a hardcoded relay URL.

Rationale: the scope says the relay URL must come from `peer.json.relay_url` or
`observe.observer.spl_relay_url`, but Lode A intentionally omitted `relay_url`
from `peer.json`. The actual bundle writer records only `label`, `paired_at`,
`instance_id`, `home_label`, `fingerprint`, `local_endpoints`, and `role`
(`solstone/think/link/join_cli.py:120-134`). Therefore the only valid relay URL
source in this lode is config. The hardcoded default in
`solstone/think/link/paths.py:33-36` is for the home link service, not observer
client PL dialing.

PL config example:

```yaml
observe:
  observer:
    pair_mode: pl
    spl_label: laptop
    spl_relay_url: https://spl-relay.example
```

Startup validation in `ObserverClient.__init__()`:

- `pair_mode` defaults to `"dl"`.
- Value must be `"dl"` or `"pl"`; otherwise raise `ValueError`.
- `pair_mode=pl` plus `key` set raises a dual-config mutex error naming both
  `observe.observer.key` and `observe.observer.pair_mode`.
- `pair_mode=pl` plus missing `spl_label` raises.
- `pair_mode=pl` plus missing `spl_relay_url` raises.
- `pair_mode=pl` plus missing bundle directory raises.
- `pair_mode=pl` plus any missing required file raises.
- `pair_mode=dl` keeps today’s behavior unchanged.

Required bundle files are the Lode A set: `private.pem`, `cert.pem`, `chain.pem`,
`home_attestation.jwt`, and `peer.json` (`solstone/think/link/bundle.py`).
The bundle path is `$XDG_CONFIG_HOME/solstone-observer/spl/<label>/` or
`~/.config/solstone-observer/spl/<label>/`
(`solstone/think/link/observer_paths.py`).

Implementation deviation: Lode A's `peer.json.fingerprint` is the CA fingerprint,
not the client certificate fingerprint that Convey stamps into `g.identity`.
`ObserverClient` therefore derives `ClientIdentity.fingerprint` from `cert.pem`
using `cert_fingerprint()` instead of trusting `peer.json.fingerprint`.

### 9. PL dial race and session lifecycle

Chosen: keep `ObserverClient`'s public methods synchronous and run PL tunnel
work on a private asyncio loop thread owned by the client instance.

Rationale: `ObserverClient.upload_segment()`, `relay_event()`, and
`subscribe_callosum()` are synchronous today (`solstone/observe/observer_client.py:173-285`).
The lifted link `TunnelSession` is async (`tests/link/client.py:375-443`).
A private loop thread preserves the public API while allowing a cached session.

ObserverClient PL internals:

- `_load_pl_bundle()` reads private key, cert, CA chain, attestation, and
  `peer.json` and builds a `ClientIdentity`.
- `_open_tunnel()` races LAN-direct and spl-relay attempts and returns a
  `TunnelSession`.
- LAN-direct attempts use `peer.json.local_endpoints`, whose canonical shape is
  `ip`, `port`, `scope` (`solstone/think/link/local_endpoints.py:11-31`).
- The spl-relay attempt uses `spl_relay_url`; it enrolls with the relay using
  the bundled home attestation via `Client.enroll_device()` before dialing.
- Use `asyncio.wait(..., return_when=FIRST_COMPLETED)`.
- First successful TLS handshake wins.
- Cancel loser tasks after a winner.
- If all attempts fail, raise one `TlsError` listing each attempt and its failure.
- Cache the winning `TunnelSession` per `ObserverClient` instance.
- On `StreamResetError`, `ConnectionError`, `OSError`, or `TlsError` during a
  request, close/drop the cached session and lazily reopen on the next request.

HTTP over PL:

- Use prepared request machinery to build method, path, headers, and body for
  multipart upload and JSON event requests, then send through `TunnelSession`.
- For upload, preserve DL retry semantics: transport failures retry according to
  `RETRY_BACKOFF`; final failure returns `UploadResult(False)`.
- Do not call `finalize_draft()` internally. Current DL client returns
  `UploadResult(False)` on failure and has no production finalize call site.
- `subscribe_callosum()` needs a streaming PL request using `_http_request_bytes`
  and the mux stream reader, not `TunnelSession.request()`, because SSE does not
  complete like a normal response.

### 10. Test locations

Chosen:

- `solstone/apps/observer/tests/` for observer record schema, identity helper,
  route branching, pair-completion observer record effects, observer-list mode
  column, implicit-role behavior, and re-pair tombstones.
- `tests/link/test_dialer_unit.py` for tunnel race unit tests.
- `tests/integration/test_observer_over_pl_e2e.py` for an authored integration
  test; do not require it in fast gates.

Rationale: observer record and route behavior belongs with the observer app’s
existing tests. Dial race mechanics belong with existing link tunnel tests under
`tests/link/`.

### 11. `journal observer list` and `status` mode column

Chosen: add a `Mode` column to human output and a `mode` field to JSON output.
For PL rows, `Prefix` shows the 16-hex fingerprint prefix.

Current list output starts at `solstone/observe/observer_cli.py:222`; the human
header currently has Name, Prefix, Status, Last Seen, Segments, Bytes at
`solstone/observe/observer_cli.py:247-263`. `_status_single()` starts at
`solstone/observe/observer_cli.py:361` and prints Prefix at `:390`.

Sketch:

- Human `list`: `Name`, `Mode`, `Prefix`, `Status`, `Last Seen`, `Segments`, `Bytes`.
- JSON `list`: add `"mode": "dl" | "pl"` and make `"prefix"` use
  `filename_prefix`.
- Human `status`: print `Mode` next to Prefix.
- JSON `status`: add `"mode"` and use `filename_prefix`.
- DL create/install output remains DL-specific.

### 12. Implicit role behavior

Chosen: only `role == "observer"` mints an observer record. Default phone role
does not.

Rationale: `pair_start()` defaults role to `"phone"` and accepts `phone`,
`observer`, or `peer` (`solstone/apps/link/routes.py:266-289`). `_complete_pairing`
receives the consumed nonce with role at `solstone/apps/link/routes.py:303-319`.
Phone-paired clients may be valid paired devices, but they are not observers.

Expected route result: a phone-paired client POSTing to `/app/observer/ingest`
over PL has a valid `g.identity.fingerprint` but no observer record, so
`resolve_observer_identity()` returns 401 `AUTH_REQUIRED`.

### 13. Re-pair tombstone

Chosen: re-pairing a label creates a new PL observer record for the new
fingerprint. The old observer record stays on disk until explicit revoke.

Rationale: the filename is fingerprint-derived, not label-derived. A re-pair
with a new cert produces a new fingerprint and therefore a new file. This avoids
silent mutation of old history/stat paths and matches the existing soft-delete
model for DL observers.

## Module Layout

Net-new:

| file | purpose |
|---|---|
| `solstone/think/link/client.py` | Lifted PL client/tunnel helpers from `tests/link/client.py`. |
| `docs/design/lodes/observer-over-pl.md` | This design. |
| `tests/link/test_dialer_unit.py` | Unit tests for LAN/relay race and failure aggregation. |
| `tests/integration/test_observer_over_pl_e2e.py` | Authored PL observer end-to-end integration test. |

Modified:

| file | purpose |
|---|---|
| `tests/link/client.py` | Thin compatibility shim for existing tests. |
| `solstone/apps/observer/utils.py` | ObserverRegistry, PL writer, identity resolver, prefix helpers. |
| `solstone/apps/observer/routes.py` | Replace per-route auth/load/check blocks with identity resolver. |
| `solstone/apps/observer/events.py` | Use derived prefix for PL-safe history/stats. |
| `solstone/apps/link/routes.py` | Mint observer records for observer-role pair completion. |
| `solstone/observe/observer_client.py` | Config validation, PL bundle loading, PL tunnel requests. |
| `solstone/observe/observer_cli.py` | Mode column/field and PL-safe prefix display. |
| `solstone/apps/observer/tests/*.py` | Schema, helper, route, pair, list/status tests. |

## Public Surface

Lifted link client exports:

- `Client`
- `ClientIdentity`
- `EncryptedTransport`
- `EnrolledDevice`
- `StreamResetError`
- `TlsError`
- `TunnelSession`
- `_http_request_bytes`

Observer app helper signatures:

- `observer_filename_prefix(record: dict) -> str`
- `load_observer(key: str) -> dict | None`
- `load_observer_by_fingerprint(fingerprint: str) -> dict | None`
- `mint_pl_observer_record(fingerprint: str, device_label: str, paired_at: str) -> Path`
- `resolve_observer_identity(url_key: str | None = None) -> tuple[dict | None, str | None, tuple[Response, int] | None]`

ObserverRegistry accessors:

- `ObserverRegistry.singleton() -> ObserverRegistry`
- `ObserverRegistry.by_key(key: str) -> dict | None`
- `ObserverRegistry.by_fingerprint(fingerprint: str) -> dict | None`
- `ObserverRegistry.by_prefix(prefix: str) -> dict | None`
- `ObserverRegistry.by_name(name: str) -> dict | None`
- `ObserverRegistry.all() -> list[dict]`

## Per-Route Branching Matrix

| route | DL behavior | PL behavior |
|---|---|---|
| `GET /app/observer/<key>/callosum` | Bearer/url key resolves via `by_key`; subscriber prefix is DL filename prefix. | URL uses `<fp-prefix-16>` for routing; identity resolves via `g.identity.fingerprint`; subscriber prefix is PL filename prefix. |
| `POST /app/observer/ingest` | Bearer key required; current keyless route remains. | Keyless route; identity resolves via `g.identity.fingerprint`; no bearer header. |
| `POST /app/observer/ingest/<key>/transfer` | Bearer/url key resolves via `by_key`. | URL key is `<fp-prefix-16>` but ignored for identity; helper uses fingerprint. |
| `GET /app/observer/ingest/<key>/manifest` | Bearer/url key resolves via `by_key`; reads DL history prefix. | URL key is `<fp-prefix-16>`; reads PL history prefix. |
| `GET /app/observer/ingest/<key>/manifest/<day>` | Bearer/url key resolves via `by_key`; auth only after that. | URL key is `<fp-prefix-16>`; auth by fingerprint; no prefix needed after auth. |
| `POST /app/observer/ingest/event` | Bearer key required; current keyless route remains. | Keyless route; identity resolves by fingerprint; status events update observer record as today. |
| `GET /app/observer/ingest/<key>/segments/<day>` | Bearer/url key resolves via `by_key`; reads DL history prefix. | URL key is `<fp-prefix-16>`; reads PL history prefix. |

All branches preserve:

- 401 `AUTH_REQUIRED` for no usable auth or PL paired-but-not-observer.
- 401 `AUTH_KEY_INVALID` for invalid DL bearer/url key.
- 403 `PL_REVOKED` for revoked observer records.
- 403 `FEATURE_UNAVAILABLE` for disabled observer records.

## Pair-Completion Atomicity

State sequence for observer role:

1. Nonce is consumed by `/pair` or `/by-code`.
2. CSR is signed and response material is built in memory.
3. `mint_pl_observer_record()` writes `<fp-prefix-16>.json`.
4. `AuthorizedClients.add()` writes `journal/link/authorized_clients.json`.
5. If step 4 fails, delete the observer record from step 3 and re-raise.
6. Emit `link.pair_complete` only after both durable writes succeed.

State sequence for non-observer role:

1. Nonce is consumed.
2. CSR is signed and response material is built.
3. `AuthorizedClients.add()` writes the paired-device ledger.
4. No observer record is written.

No cleanup runs for old observer records during re-pair. Explicit revoke remains
the only cleanup/tombstone path.

## Config Schema

DL default remains valid:

```yaml
observe:
  observer:
    pair_mode: dl
    key: "<bearer key>"
    url: "https://home.example"
```

PL config:

```yaml
observe:
  observer:
    pair_mode: pl
    spl_label: laptop
    spl_relay_url: https://spl-relay.example
    name: laptop
```

Validation failures are startup errors from `ObserverClient.__init__()`, not
silent fallbacks. There is no hardcoded relay fallback and no `peer.json.relay_url`
fallback in this lode.

## Implementation Phasing

1. Lift `tests/link/client.py` to `solstone/think/link/client.py`, add `__all__`,
   and replace `tests/link/client.py` with the re-export shim. Run link tests
   that import the shim.
2. Add observer prefix helpers and `ObserverRegistry` in
   `solstone/apps/observer/utils.py`. Keep `load_observer()` as a wrapper.
3. Update `save_observer()`, `list_observers()`, `find_observer_by_name()`, and
   `increment_stat()` to use registry/prefix helpers and invalidate after writes.
4. Add `mint_pl_observer_record()` and tests for DL record validation, PL record
   validation, invalid xor records, and existing-file refusal.
5. Add `resolve_observer_identity()` and route-helper tests for DL success,
   DL missing/invalid, PL observer success, PL phone/no-record, revoked, and disabled.
6. Replace the seven observer route auth blocks with the helper. Update prefix
   usages in `routes.py` and SSE heartbeat reload logic.
7. Update `solstone/apps/observer/events.py` to derive history/stat prefix from
   returned observer records.
8. Wire `solstone/apps/link/routes.py:_complete_pairing()` to mint observer
   records for observer-role nonces with rollback on `AuthorizedClients.add()`
   failure. Add observer-role, phone-role, rollback, and re-pair tombstone tests.
9. Update `solstone/observe/observer_cli.py` list/status output to include mode
   and derived prefixes. Add output tests.
10. Add PL config validation and bundle loading to `ObserverClient.__init__()`.
    Keep DL behavior unchanged.
11. Add PL tunnel loop/session/race support in `ObserverClient`, backed by the
    lifted `solstone.think.link.client` module. Add `tests/link/test_dialer_unit.py`.
12. Add PL request paths for upload, event, SSE, transfer, manifest,
    manifest-day, and segments. Preserve DL retry/failure behavior and do not
    call `finalize_draft()` inside the client.
13. Add `tests/integration/test_observer_over_pl_e2e.py` as authored coverage
    for pair-as-observer, PL upload, server route identity, and status/history.
14. Run targeted observer app tests, link tests, and `make check-layer-hygiene`.
    Run broader `make test` if time allows.

This order front-loads the module lift and observer registry because every later
route/client change depends on stable PL-safe identity and prefix handling.

## Test Plan

| test file | proves |
|---|---|
| `tests/link/test_privacy_scan.py` and existing link imports | Re-export shim keeps existing named imports working. |
| `tests/link/test_dialer_unit.py` | LAN/relay race picks first successful handshake, cancels losers, aggregates all-fail errors, drops cached session after reset. |
| `solstone/apps/observer/tests/test_utils.py` | Registry validates DL/PL records, derives filename prefixes, loads by key/fingerprint/prefix, skips invalid records. |
| `solstone/apps/observer/tests/test_routes.py` | Seven routes return correct 401/403 distinctions in DL and PL modes and use PL history prefixes. |
| `solstone/apps/observer/tests/test_callosum_sse.py` | PL SSE registers/unregisters under 16-char prefix and stops on revoke/disable. |
| `solstone/apps/observer/tests/test_pl_pairing.py` | Observer-role pairing writes observer record, phone-role pairing does not, rollback deletes new observer record on ledger write failure. |
| `solstone/apps/observer/tests/test_routes.py` | Phone-paired PL ingest returns 401 `AUTH_REQUIRED`; observer-paired PL ingest succeeds. |
| `solstone/apps/observer/tests/test_routes.py` | Re-pair with same label/new fingerprint leaves old record and creates new record. |
| `tests/test_observer_cli.py` | `journal observer list/status` include mode and show 16-char PL prefix. |
| `tests/integration/test_observer_over_pl_e2e.py` | Authored end-to-end: pair as observer, start PL client, upload segment, verify history/status. |

## Out Of Scope

- No `--pair-mode` CLI flag.
- No deprecation of DL.
- No admin endpoint branching.
- No dashboard role grouping.
- No iOS/Android changes.
- No `relay_url` field on `peer.json`.
- No per-request `last_seen` write.
