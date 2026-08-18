# Convey Native-Client OpenAPI Contract

> `docs/openapi/convey-clients.json` is the source. Hand-edit it. Pinned
> operation IDs still bind. The Python assembler and `make openapi` are gone.
> This document is the historical design, not a generation plan.


## 1. Fragment And Ownership Shape

The contract framework will live under `solstone/convey/contract/`:

| Path | Role |
|---|---|
| `solstone/convey/contract/spec.py` | Frozen dataclasses for the small contract DSL: `OperationSpec`, `RequestSpec`, `ResponseSpec`, `FieldSpec`, and `ParamSpec`. |
| `solstone/convey/contract/assemble.py` | Assembles ordered `OperationSpec` values into an OpenAPI 3.1 document, introspects `solstone.convey.reasons`, and owns `CALLOSUM_REGISTRY`. |

The code-adjacent opt-in fragments are:

| Path | Ownership evidence |
|---|---|
| `solstone/apps/network/contract.py` | Link routes are owned by the link app blueprint at `solstone/apps/network/routes.py:124-128`. |
| `solstone/apps/observer/contract.py` | Observer routes are owned by the observer app blueprint at `solstone/apps/observer/routes.py:79-86`. |
| `solstone/convey/push_contract.py` | Push is a root Convey blueprint beside `solstone/convey/push.py`, whose `push_bp` is defined at `solstone/convey/push.py:30`. |

Each fragment exposes `OPERATIONS: list[OperationSpec]`. The generator uses only
this explicit fragment list:

- `solstone.apps.network.contract`
- `solstone.apps.observer.contract`
- `solstone.convey.push_contract`

The generator never scrapes `app.url_map`; membership is exactly what the
fragments declare. Route drift is checked in pytest, not during generation.

`OperationSpec` should store the Flask/Werkzeug rule as the route identity
because Flask rules use `<day>` params (`solstone/apps/observer/routes.py:1029`,
`solstone/apps/observer/routes.py:1132`). Assembly converts that rule to the
OpenAPI path form (`{day}`), with an optional explicit OpenAPI path override if
a future rule cannot be converted mechanically.

The dataclasses are feasible if they are not over-normalized. `ResponseSpec`
and `RequestSpec` need to allow a raw schema mapping or component reference in
addition to named fields. That keeps frozen dataclasses workable for:

- Free-form maps such as `days` and `segments`.
- Top-level array responses.
- `oneOf` for `observer.ingestSegments`.
- `text/event-stream` responses with an event payload component.

Object schemas remain open (`additionalProperties: true`). Named fields are
still declared so conformance can reject accidental top-level response drift.

## 2. Operation IDs

These operation IDs are pinned and renames are breaking:

- `link.pairStart`
- `link.pair`
- `link.unpair`
- `link.localEndpoints`
- `link.status`
- `observer.register`
- `observer.ingestUpload`
- `observer.ingestEvent`
- `observer.ingestManifest`
- `observer.ingestManifestDay`
- `observer.ingestSegments`
- `observer.callosumStream`
- `push.register`
- `push.unregister`

They are 1:1 with the handler set in `solstone/apps/network/routes.py:401-861`,
`solstone/apps/observer/routes.py:256-1250`, and
`solstone/convey/push.py:73-112`.

## 3. Per-Operation Table

Paths below are OpenAPI paths. Flask conformance uses the corresponding
Werkzeug rules, including `<day>`.

| Method | Path | operationId | Named request fields | Named success response fields | `x-reason-codes` |
|---|---|---|---|---|---|
| POST | `/app/network/pair-start` | `link.pairStart` | body: `device_label?`, `role?` (`solstone/apps/network/routes.py:530-535`) | `nonce`, `pair_link`, `expires_in`, `device_label`, `ca_fingerprint` (`routes.py:287-294`, `routes.py:594-602`) | `invalid_operation_for_state`, `pairing_request_invalid`, `pl_revoked` |
| POST | `/app/network/pair` | `link.pair` | query: `token?`; body: `csr`, `nonce?`, `device_label?`, `sender_instance_id?` (`routes.py:704-724`) | `client_cert`, `ca_chain`, `instance_id`, `home_label`, `home_attestation`, `fingerprint`, `local_endpoints?` (`routes.py:622-633`) | `missing_required_field`, `operation_no_longer_available`, `pairing_key_invalid`, `pairing_request_invalid`, `pl_revoked` |
| POST | `/app/network/unpair` | `link.unpair` | body: `fingerprint?`, `device_label?`; one is required (`routes.py:797-818`) | `unpaired` (`routes.py:861`) | `missing_required_field`, `paired_device_not_found`, `pl_revoked` |
| GET | `/app/network/local-endpoints` | `link.localEndpoints` | none | `v`, `endpoints`, `ttl_s`, `generated_at` (`routes.py:507-513`, `solstone/think/link/local_endpoints.py:34-40`) | `pl_revoked` |
| GET | `/app/network/api/status` | `link.status` | none | `instance_id`, `home_label`, `enrolled`, `relay_url`, `ca_fingerprint`, `lan_accessible`, `posture`, `reachability`, `relay_state`, `home_address`, `vpn` (`routes.py:421-435`) | `pl_revoked` |
| POST | `/app/observer/register` | `observer.register` | body: `platform`, `hostname`, `stream_type`, `version`, `label?` (`solstone/apps/observer/routes.py:419-452`) | `key`, `prefix`, `name`, `ingest_url`, `protocol_version` (`routes.py:467-475`) | `invalid_segment_or_stream`, `local_request_only`, `missing_required_field`, `settings_operation_failed` |
| POST | `/app/observer/ingest` | `observer.ingestUpload` | auth: `Authorization` bearer or `X-Solstone-Observer`; multipart: `segment`, `day`, `files`, `host?`, `platform?`, `meta?` (`routes.py:890-942`) | normal/collision: `status`, `segment`, `files`, `bytes`; duplicate: `status`, `existing_segment`, `message` (`routes.py:740-747`, `routes.py:859-865`) | `auth_key_invalid`, `auth_required`, `feature_unavailable`, `ingest_no_files`, `ingest_storage_failed`, `invalid_day`, `invalid_segment_or_stream`, `missing_required_field`, `pl_revoked` |
| POST | `/app/observer/ingest/event` | `observer.ingestEvent` | auth; body: `tract`, `event`, plus open event fields (`routes.py:1073-1093`) | `status` (`routes.py:1100`) | `auth_key_invalid`, `auth_required`, `feature_unavailable`, `missing_required_field`, `pl_revoked` |
| POST | `/app/observer/health` | `observer.health` | auth; body: diagnostics-only beacon fields `name`, `stream_type`, `version`, `uptime`, `last_successful_sync`, `pending_queue_depth`, `recent_error_count`, `last_error_reason`; unexpected fields ignored | `status` | `auth_key_invalid`, `auth_required`, `feature_unavailable`, `pl_revoked` |
| GET | `/app/observer/ingest/manifest` | `observer.ingestManifest` | auth | `days` free-form map (`routes.py:1012-1026`) | `auth_key_invalid`, `auth_required`, `feature_unavailable`, `pl_revoked` |
| GET | `/app/observer/ingest/manifest/{day}` | `observer.ingestManifestDay` | auth; path: `day` (`routes.py:1029-1038`) | `version`, `day`, `created_at`, `host`, `segments` free-form map (`routes.py:1039-1061`) | `auth_key_invalid`, `auth_required`, `feature_unavailable`, `invalid_day`, `pl_revoked` |
| GET | `/app/observer/ingest/segments/{day}` | `observer.ingestSegments` | auth; path: `day`; query: `stream?`; header: `X-Solstone-Protocol-Version?` (`routes.py:1103-1112`, `routes.py:1132-1175`) | v2 envelope: `items`, `total`, `protocol_version`; legacy: top-level array of segment items (`routes.py:1115-1129`, `routes.py:1209-1249`) | `auth_key_invalid`, `auth_required`, `feature_unavailable`, `invalid_day`, `pl_revoked` |
| GET | `/app/observer/callosum` | `observer.callosumStream` | auth | `text/event-stream`; data payload names `tract`, `event`, `ts`, plus passthrough fields (`routes.py:240-255`, `routes.py:302-308`) | `auth_key_invalid`, `auth_required`, `feature_unavailable`, `pl_revoked` |
| POST | `/api/push/register` | `push.register` | body: `device_token`, `bundle_id`, `environment`, `platform` (`solstone/convey/push.py:78-96`) | `registered`, `device_count` (`push.py:96-103`) | `invalid_json_request`, `pl_revoked`, `push_request_invalid` |
| DELETE | `/api/push/register` | `push.unregister` | none | `removed`, `device_count` (`push.py:106-112`) | `pl_revoked`, `push_request_invalid` |

The `pl_revoked` entries on link and push operations come from the root access
gate, not the handlers. The non-exempt set is established by
`solstone/convey/root.py:75-97`; the legacy `reason` body is emitted at
`solstone/convey/root.py:110-118`. `observer.callosumStream` is also non-exempt,
but it additionally has handler-local observer auth errors through
`resolve_observer_identity()`.

`link.localEndpoints` also has a bare non-reason-coded Flask 404 for non-loopback
requests at `solstone/apps/network/routes.py:503-506`. That response is documented
separately from `x-reason-codes`.

## 4. Reason-Code Two-Tier Model

The global enum is generated from module-level `Reason` instances in
`solstone.convey.reasons`. `Reason` is a frozen dataclass with
`code`, `message`, and `status` at `solstone/convey/reasons.py:7-11`. The module
currently has 84 such instances and no `__all__`. Assembly will sort all
`.code` values into:

- `components.schemas.Error.properties.reason_code.enum`

The shared Error schema is:

- `error`: string
- `reason_code`: string, enum of all global codes
- `detail`: string
- `additionalProperties: true`

`additionalProperties: true` absorbs the access-gate legacy `reason` key from
`error_response_with_reason()` (`solstone/convey/utils.py:300-318`), while normal
handler errors use `error_response()` (`solstone/convey/utils.py:269-297`).

The per-operation referenced set is emitted as `x-reason-codes` on each
contracted error response. Removing a referenced code is breaking. Adding a new
global code that is not referenced by any operation is staleness only.

Provider-readiness/runtime reason registries are separate string registries:
`READINESS_REASON_CODES` and `REASON_CODES` live at
`solstone/think/providers/state.py:33-47`, runtime codes live at
`solstone/think/providers/shared.py:250-269`, and readiness presentation lives
at `solstone/convey/provider_readiness.py:83-299`. None of the 14 fragments
should import those registries.

## 5. Segments Version Handling

`observer.ingestSegments` has an optional integer header parameter
`X-Solstone-Protocol-Version`. The header name and current version come from
`solstone/observe/protocol.py:12-16`.

The handler defaults absent or unparsable headers to protocol version 1 at
`solstone/apps/observer/routes.py:1103-1112`. It returns the v2 collection
envelope when `client_pv >= OBSERVER_PROTOCOL_VERSION`, currently 2, at
`solstone/apps/observer/routes.py:1124-1128`; otherwise it returns a bare array
at `solstone/apps/observer/routes.py:1129`.

The 200 response schema is `oneOf`:

- `SegmentsEnvelope`: object with `items`, `total`, `protocol_version`.
- `SegmentsArray`: array of `SegmentItem`.

`SegmentItem` names `key`, `observed`, `files`, and optional `original_key`.
Each file object names `name`, `size`, `sha256`, `status`, and optional
`submitted_name` at `solstone/apps/observer/routes.py:1344-1448`.

This does not introduce a new negotiation system. `protocol_version` is only the
existing v2 response-envelope field.

## 6. SSE Envelope And Registry

`observer.callosumStream` returns a 200 `text/event-stream` response. OpenAPI
3.1 can represent this as a normal response media type, with the schema
documenting the JSON payload carried in each `data:` frame.

`components.schemas.CallosumEvent` is:

- required `tract`: string
- required `event`: string
- required `ts`: integer
- `additionalProperties: true`

The response description must preserve the actual frame formats:

- Data frame: `data: {json}\n\n` (`solstone/apps/observer/routes.py:298`)
- Heartbeat: `: heartbeat\n\n` (`routes.py:270`, `routes.py:293`)
- Error frame: `event: error\ndata: {Error}\n\n` (`routes.py:111-112`)

The SSE response will also carry an `x-sse-error-frame` extension containing:

- schema reference: `#/components/schemas/Error`
- `x-reason-codes`: `auth_required`, `pl_revoked`, `feature_unavailable`

The non-exhaustive registry is emitted as `x-callosum-registry` from a single
Python constant, `CALLOSUM_REGISTRY`, in `solstone/convey/contract/assemble.py`.
The same constant is rendered into `docs/CONVEY.md` inside this marker block:

- `<!-- BEGIN GENERATED callosum-registry -->`
- `<!-- END GENERATED callosum-registry -->`

The registry should include the documented tracts from `docs/CALLOSUM.md:33-148`:

- `cortex`: `request`, `start`, `thinking`, `tool_start`, `tool_end`, `finish`, `error`, `talent_updated`, `info`, `status`
- `supervisor`: `started`, `stopped`, `restarting`, `status`, `queue`
- `logs`: `exec`, `line`, `exit`
- `observe`: `status`, `observing`, `detected`, `described`, `transcribed`, `observed`
- `importer`: `started`, `status`, `completed`, `error`
- `think`: `started`, `status`, `group_started`, `group_completed`, `talent_started`, `talent_completed`, `completed`, `segments_started`, `segments_completed`
- `activity`: `live`, `recorded`
- `sync`: `status`
- `notification`: `*`
- `navigate`: `request`

It should also include the implemented `chat` tract from
`solstone/convey/chat_stream.py:35-74` and `solstone/convey/chat_stream.py:349-366`:

- `chat`: `owner_message`, `sol_message`, `talent_spawned`, `talent_finished`, `talent_errored`, `reflection_ready`, `chat_queue_depth`, `chat_error`, `sol_chat_request`, `sol_chat_request_superseded`, `owner_chat_open`, `owner_chat_dismissed`, `support_draft`, `result`, `support_submit_claim`

Implementation should also fix the stale non-generated prose in
`docs/CONVEY.md`: the route is currently wrong at `docs/CONVEY.md:131-132`, and
the `api/list` sentence is stale at `docs/CONVEY.md:149-151`.

## 7. Artifact Paths And Headers

The generated OpenAPI authority outputs are:

- `docs/openapi/convey-clients.json`
- `docs/openapi/observer-client-contract/manifest.json`
- `docs/openapi/observer-client-contract/projection.openapi.json`
- `docs/openapi/observer-client-contract/vectors.json`
- `docs/openapi/observer-client-contract/fixtures/wire-behavior.json`
- `docs/openapi/observer-client-contract/consumer-audit.json`

`docs/openapi/` is an existing generated-contract directory. The observer
bundle is a generated subdirectory beside the full Convey client artifact, and
the generator must not list files inside that bundle as generator inputs.

Generated JSON is pretty-printed with stable key order and a trailing newline.
The full artifact header fields are:

- `openapi`: `3.1.0`
- `info.title`: `Solstone Convey Native-Client Contract`
- `info.version`: `1.0.0`
- `info.x-generated-by`: `make openapi (scripts/build_openapi_contract.py)`
- `info.x-generated`: `true`
- `info.description`: generated-file notice, regenerate command, and do-not-hand-edit warning

`info.version` is the static document version. It is not observer protocol
negotiation.

The observer bundle projection is an OpenAPI 3.1 document over the frozen
observer-client operation set. The bundle manifest records the bundle SemVer,
OpenAPI document version, observer on-wire protocol version, supported response
variants, source-input digests, payload file digests, vocabulary inventory, and
consumer audit metadata.

## 8. Checks, Make Targets, And Messages

There are three check surfaces.

### Generator Staleness

`scripts/build_openapi_contract.py` is thin. It imports assembly from
`solstone.convey.contract`, renders the full OpenAPI artifact, renders the
observer-client bundle, and updates the generated Callosum registry block in
`docs/CONVEY.md`.

`--check` mode regenerates in memory and compares:

- `docs/openapi/convey-clients.json`
- the marker-delimited generated block in `docs/CONVEY.md`
- every file under `docs/openapi/observer-client-contract/`

On diff it exits 1 and prints exactly:

```text
OpenAPI generated outputs are stale: {paths}. Run: make openapi
```

### Breaking Tripwire

`scripts/check_openapi_contract.py` regenerates the current spec, loads the
committed artifact, and classifies differences.

Breaking:

- removed or renamed `operationId`
- removed endpoint
- removed or renamed named response field
- removed request field
- new required request field
- removed parameter
- removed per-operation referenced reason code

Additive and allowed:

- new operation
- new optional field
- new optional parameter
- global reason enum addition
- registry addition

Removal of a global reason code that is not referenced by any operation is
staleness, not breaking.

On breaking diff it exits 1 and prints exactly:

```text
OpenAPI contract breaking changes detected: {items}. If intentional, run `make openapi` to re-pin and notify native-client owners; otherwise revert.
```

### Observer Client Bundle Gates

`scripts/check_observer_client_contract_bundle.py` checks the generated observer
bundle after the existing full-OpenAPI tripwire and staleness check. It reports
these gates independently and accumulates failure state:

- bundle staleness;
- manifest/file inventory verification, including path safety and source-input
  digests;
- Git-history compatibility and bundle SemVer enforcement;
- Windows/Linux consumer-audit coverage from `consumer-audit.json`.

Each failure exits nonzero and prints a recovery action, for example regenerate
with `make openapi`, repair the manifest, or apply the required bundle SemVer
bump.

To export a verified committed bundle to a new directory inside the repository,
run:

```text
.venv/bin/python scripts/export_observer_client_contract_bundle.py <new-destination>
```

The export command refuses an existing destination, stale generated output,
digest mismatch, unsafe paths, symlinks/non-regular files, and unlisted payload
files. It stages into a fresh sibling directory, verifies the staged bytes, then
uses one final rename into the still-absent destination.

### Conformance Tests

`tests/test_openapi_contract.py` is a normal pytest module. `make test` and
`make test-cov` already collect `tests/` and `solstone/apps/` at
`Makefile:307-317`.

The test creates a Flask app with `create_app(journal=...)`
(`solstone/convey/__init__.py:54-55`) and marks setup complete using
`tests/_baseline_harness.py:150-160`.

Conformance checks:

- Each fragment method/rule resolves in `app.url_map`; this catches
  fragment-vs-reality drift without making the generator journal-aware.
- Observer auth works through both Bearer and `X-Solstone-Observer`, matching
  `solstone/apps/observer/utils.py:296-333`.
- `observer.ingestSegments` returns the v2 envelope or legacy array based on
  `X-Solstone-Protocol-Version`.
- JSON and multipart parsing are exercised for at least push/register and
  observer/ingest.
- At least one malformed or unauthorized request returns structured
  `{error, reason_code, detail}`.
- Each reachable named response has no undeclared top-level fields unless the
  response is explicitly free-form.
- In-memory classifier tests cover the reviewer scenarios for operation
  removal, named-field removal, required request-field addition, per-operation
  reason-code removal, and global-enum staleness.

On undeclared top-level response fields, the test fails with exactly:

```text
OpenAPI contract conformance failed: {operationId} returned undeclared top-level field(s): {fields}. Declare it optional in {fragment_path}, then run make openapi, or fix the handler.
```

### Make Targets

Those targets were never rebuilt after the assembler was deleted. Edit
`docs/openapi/convey-clients.json` directly. The list below is historical.

- `make openapi`: regenerate `docs/openapi/convey-clients.json`, the observer
  bundle, and the `docs/CONVEY.md` generated block.
- `make check-openapi`: run the breaking tripwire first, then the full generated
  staleness check, then `make check-openapi-observer-client-contract`.
- `make check-openapi-observer-client-contract`: run the accumulated observer
  bundle staleness, manifest, compatibility, and Windows/Linux coverage gates.

The order is intentional. If staleness ran first and exited on any diff, the
breaking classifier would never report breaking diffs. The observer bundle gate
runs after the existing tripwire-then-staleness sequence.

Wire `check-openapi` into `install-checks`, next to the existing generated
reference checks. The current pattern is `check-* : .installed` targets at
`Makefile:479-524`, `install-checks` chaining at `Makefile:383-427`, and `ci`
depending on `install-checks` at `Makefile:428-430`.

## 9. Feasibility Findings

Most decisions validate cleanly against the current code.

Amendment required: `make check-openapi` must run
`scripts/check_openapi_contract.py` before `scripts/build_openapi_contract.py
--check`. The proposed order in the decision text had staleness first, but
staleness exits 1 on any diff. That would prevent breaking diffs from reaching
the breaking classifier.

Amendment required: `OperationSpec` needs to preserve the Flask/Werkzeug rule
or an equivalent route identity in addition to the OpenAPI path. Flask rules use
`<day>` (`solstone/apps/observer/routes.py:1029`,
`solstone/apps/observer/routes.py:1132`); OpenAPI paths use `{day}`. A small
rule-to-OpenAPI conversion in assembly is sufficient.

Amendment required: `ResponseSpec` and `RequestSpec` need a raw schema mapping
or component-reference escape hatch. Field-only dataclasses are too rigid for
`oneOf`, top-level arrays, and `text/event-stream`, but frozen dataclasses remain
feasible with that minimal extension.

Documentation mismatch: the assignment context says `docs/CALLOSUM.md:33-148`
contains `chat`, but that range does not. The implemented chat tract is emitted
from `solstone/convey/chat_stream.py:349-366`, with valid event names defined at
`solstone/convey/chat_stream.py:35-74`. The generated registry should include
chat from source and can become the source of truth for the CONVEY.md block.

Contract assumption to state: non-exempt routes can redirect to setup before
reaching handlers when setup is incomplete (`solstone/convey/root.py:120-122`).
The native-client contract and conformance tests assume a setup-complete journal,
matching the baseline harness setup path.
