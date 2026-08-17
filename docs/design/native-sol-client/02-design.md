# Native `sol` Client Spine + Reviewer-Pinned Lead Slice Design

This design builds on:

- `docs/design/native-sol-client/00-prep-findings.md`
- `docs/design/native-sol-client/01-oracle-repro.md`
- `docs/PORTING.md`

No installed `sol` entry point changes in this change. The native surface is built and verified, but Python remains the owner default.

## 1. Crate Topology, App Ownership, Generated Aggregate

### Decision

Use one shared native client library, one small CLI adapter library, and one non-installed process shell.

| Crate / area | Owns | Does not own |
|---|---|---|
| `solstone-core-sol-client` | Shared transport, journal port reader, decode/error taxonomy, request/response structs, fakeable seams, and generated aggregation mechanics under `src/generated/`. | App command vocabularies, grammar/defaults, request paths, renderers, process env, stdout/stderr, exit codes, installed Python entry points. |
| `solstone-core-sol-client-cli` | Argv-to-typed invocation adapter for the native client. It consumes the generated aggregate and returns typed outcomes such as migrated invocation, top-level chat invocation, moved stub, unsupported. | Env reads, printing, exits, HTTP, journal resolution. |
| `solstone-core-sol` process shell | Non-installed binary for test/dev execution. Reads argv/env/stdin, resolves journal with `solstone-core-journal`, reads the Convey port, builds real seams, writes stdout/stderr, returns exit codes. | App vocabularies, parsing rules, request construction, rendering logic. |
| Existing `solstone-core-journal` | Reused journal path resolver: `resolve_journal_path`, `Source`, `ResolvedJournal`. | Convey port reading; that belongs to the new client library. |

The new native port reader lives in `solstone-core-sol-client`, takes a resolved journal path, reads `<journal>/health/convey.port`, and returns 5015 for missing, unreadable, empty, or malformed content. The process shell obtains the journal path through `solstone-core-journal`; no native layer sets `SOLSTONE_JOURNAL`.

Do not create per-app crates for this lead slice. They add package churn without changing ownership. App-local ownership is enforced by placing native command source and declarative authority beside the real Python owners:

| App module | Owns |
|---|---|
| `solstone/apps/activities/native/` | Activities grammar/defaults, stdin JSON validation, request construction, reason-specific stderr mapping, markdown/JSON rendering. |
| `solstone/apps/support/native/` | Support grammar, enabled precheck flow, dry-run/consent behavior, multipart metadata, support-specific unreachable text, local diagnose fallback rendering. |
| `solstone/think/tools/native/health/` | Health grammar, date selection, health rendering, local mutual-exclusion error, HTTP operation bindings including pipeline. |
| `solstone/think/native/moved/` | `identity` and `navigate` moved-stub grammar and exact exit-2 stderr, beside the `sol call` owner. |
| `solstone/think/native/chat/` | Top-level `sol chat` argparse-equivalent grammar, chat state machine, progress rendering, terminal handling, beside `chat_cli.py`. |

Generated Rust aggregation in the shared client crate `#[path]`-includes these real-adjacent sources. Shared code may expose generic building blocks only: transport, ordered JSON value handling, query/multipart encoding, decode, errors, clock/build-identity/process-spawn traits, root dispatch mechanics, and aggregate mechanics.

### Generated Aggregate

Use code generation, not runtime registration.

| Decision | Detail |
|---|---|
| App-owned authority file | Each real-adjacent native directory contains an `authority.toml` beside its Rust module. It declares command path, kind, help, params, operation id, route method/path, contract operation id, handler symbol, and whether the entry is HTTP, moved-stub, local-only, or top-level chat. |
| Generated source | `scripts/build_native_sol_inventory.py` scans `core/native-sol/**/native/authority.toml` in lexicographic app/path order and emits `core/crates/solstone-core-sol-client/src/generated/inventory.rs`. |
| No hand-maintained central list | `lib.rs` includes only shared modules and `generated`; generated source declares path-based app modules and handler bindings. Adding a test app authority requires no shared module edit; stale generated output fails the `--check` target. |
| Loud failure | Malformed authority, duplicate path, duplicate operation id, missing handler symbol, noncanonical route path, or unsupported param shape fails generation. Missing handler symbols also fail Rust compilation because generated bindings call them directly. |
| Why not `inventory`/`linkme` | Runtime/life-before-main registration adds dependency and initialization behavior risk, conflicts with `unsafe_code=forbid` expectations, complicates iOS eligibility, and makes staleness checks weaker. Codegen matches the repo's existing build/check generated-artifact idiom. |
| God-registry lint | A new architecture lint rejects app command vocabularies, route paths, mirrored `core/crates/*/src/apps/` trees, and app switchboards in shared modules outside generated aggregate output and approved test fixtures. It uses the allowlist self-check pattern: new hits fail, stale allowlist entries fail. |

The compatibility dispatcher is generated from the same aggregate. It may classify unported inputs as unsupported, but it must never spawn Python and must not be installed as `sol`.

## 2. HTTP Client Dependency

### Decision

Use `ureq` 3.3.0 with `default-features = false` as the mature maintained synchronous HTTP client/parser for loopback HTTP. It passed the required implementation diligence gates with the new client crate still included in the iOS library graph:

- `make check-rust-deny`: pass (`bans ok, licenses ok, sources ok`; license-allowance warnings pre-existing style).
- `make check-rust-msrv`: pass on Rust 1.95.0.
- `make check-rust-ios`: pass with `solstone-core-sol-client` included.
- `make check-rust-fmt`: pass.
- `make check-rust-clippy`: pass.

### Justification

| Requirement | HTTP library decision |
|---|---|
| Localhost-only, no TLS | `ureq = { version = "3.3.0", default-features = false }`; reject non-loopback URLs at the native client boundary. |
| Dependency footprint | `ureq` passed first; no fallback candidate was evaluated. With default features disabled, TLS and compression features are not enabled for the native client crate. |
| MSRV / iOS / deny | `ureq` 3.3.0 declares Rust 1.85 and passes this workspace's Rust 1.95 MSRV, deny/license/source policy, and iOS `--lib` gate. |
| Timeout observability | Transport records timeout policy and phase: `api`, `upload`, `chat-post`, `sse-open` x `connect`, `read`, `total`. `ureq::Agent::config_builder()` exposes `timeout_connect`, `timeout_recv_response`, `timeout_recv_body`, and `timeout_global`; rendering still maps to Python's single timeout owner message where parity requires it. |
| API timeout | Connect timeout 2s, read timeout 20s, wall-clock total 30s. |
| Upload timeout | Connect timeout 2s, read timeout 120s, wall-clock total 180s. |
| Chat POST timeout | Single 10s policy matching Python's `_TimeoutSession`; exposed as the chat-post policy. |
| SSE timeout | Connect timeout 10s; no read timeout after headers by setting receive/body read timeouts to `None`. The SSE stream reader has no socket read deadline; chat's fake clock controls degraded progress. |
| Response body support | Framing, connection semantics, `Content-Length`, chunked, and connection-close body handling come from `ureq`/`ureq-proto`; `ureq::Body` documents those body-length modes. |
| Multipart | Multipart body construction remains ours so request shape is deterministic and parity-testable; `RequestBuilder::send` accepts a custom body and caller-supplied `Content-Type`. |
| SSE | The pure SSE event parser remains ours. `ureq::Body::as_reader` exposes a readable response body stream for SSE mode. |

## 3. Grammar Oracle, Native Inventory, Four-Way Join

### Frozen Grammar Oracle

| Item | Decision |
|---|---|
| Fixture path | Commit the exact 120632-byte oracle at `core/fixtures/native-sol/sol-call-grammar-v1.json`. |
| Fixture guard | `scripts/check_native_sol_grammar_oracle.py` asserts schema, source hash, entries count `174`, byte length `120632`, and SHA-256 `cfa8c95c25e14937e5f616027bd2f15d610c0fa86d542b7adc4eb5da39409ce2`. |
| Production generator | Move the scratch generator logic into `scripts/build_native_sol_grammar_oracle.py`. It can reproduce the fixture from Python source for maintainers, but CI's hard guard is the frozen fixture self-check. |
| Regeneration rule | The fixture is independent evidence and is never generated from Rust. A fixture update requires explicit senior approval and a new pinned hash. The old `c61078f0` / 113106-byte pin is dead. |

### Native Lead Inventory

The native lead inventory is generated from app-local authority files and must equal the pinned lead subset:

- 21 `sol call` leaves: activities 6, support 11, health 4.
- 2 moved stubs: `identity`, `navigate`.
- Top-level `sol chat` is tracked separately because it is not in the `sol call` oracle.

| Inventory field | Purpose |
|---|---|
| `surface` | `sol-call` or `sol-chat`. |
| `path` | Command-name path, root excluded for `sol-call`; `["chat"]` for top-level chat. |
| `kind` | `command`, `callback`, or `top-level`. |
| `help` | Must match oracle for `sol-call` paths. |
| `params` | Click-compatible param objects for oracle join: name, kind, type, required, nargs, multiple, default, options, secondary, hidden, is_flag, count, flag_value. |
| `handler` | Rust handler symbol in the app module. |
| `operation_id` | Native operation id, stable across route/contract/corpus joins. |
| `http` | Method, route template, query/body/multipart model, or `null` for moved/local-only. |
| `rendering` | Named renderer owned by the app module. |

The oracle join compares, for the 23 `sol call` entries, `path`, `kind`, `help`, and every `params` field. Chat grammar parity is enforced by the parity corpus, not the `sol call` oracle.

### Four-Way Conformance Join

Use a lead-slice manifest for operation-level joins. Keep the 20-file digest guard separate because the established 20-file Python manifest intentionally excludes `health.py`, `think/call.py`, and `pipeline_health.py`.

| Join side | Key |
|---|---|
| Lead-slice manifest | `operation_id` plus `surface` and `path`. |
| App-local authority | Same `operation_id`; declares handler, grammar, route, and contract operation. |
| Server route | Canonical `(method, path_template)` plus Flask endpoint name. |
| Contract fragment | OpenAPI `operation_id`, method, path, reason codes, request/response shape. |

Failure conditions:

- Lead manifest entry has no app-local authority.
- App-local migrated authority is not covered by the current native conformance
  join.
- HTTP authority has no matching server route.
- HTTP authority has no matching contract fragment.
- Contract fragment marked native-client lead has no matching route or authority.
- Server route marked native-client lead has no matching contract fragment or authority.
- Method/path/operation id/reason-code sets disagree across authority, route, and contract.
- App-local authority names a Python manifest file not covered by the separate 20-file digest guard, except for explicitly tagged health-route additions needed for the pipeline delta.

## 4. Parity Corpus Format + Dual Execution

### Corpus Location and Schema

Commit vectors under `core/fixtures/native-sol/parity/`, grouped by app. Use JSONL for diffable append-only vectors.

| Vector field | Meaning |
|---|---|
| `id` | Stable vector id, e.g. `activities.list.default_today.json`. |
| `surface` | `sol-call` or `sol-chat`. |
| `argv` | Full user-facing argv, including `sol`, `call` where applicable. |
| `env` | Explicit env map; missing means unset. Includes `SOL_DAY`, `SOL_FACET`, support URL env, and flags needed by the Python side. |
| `stdin` | UTF-8 text or structured file reference for stdin JSON payload mode. |
| `files` | Fixture files visible to attach flows: path, bytes or fixture ref, size, filename. |
| `clock` | Fixed wall date/time, monotonic script, and timezone where relevant. |
| `transport` | Ordered script of expected requests and canned responses/faults. |
| `expected` | stdout, stderr, exit status, and captured request sequence. |
| `normalizations` | Explicit per-vector permitted differences. Empty list means byte-exact old behavior wins. |

Transport request shape:

| Field | Meaning |
|---|---|
| `method` | HTTP method. |
| `path` | Path without query. |
| `query` | Ordered list of `[key, value]` pairs so repeated keys and `doseq` cases are exact. |
| `headers` | Asserted subset only; incidental headers are ignored unless listed. |
| `json` | Expected JSON body as a value. |
| `multipart` | Field names, filenames, content types, byte lengths, and form fields. Boundary bytes are normalized unless a vector pins them. |
| `timeout_policy` | `api`, `upload`, `chat-post`, or `sse-open`. |
| `response` | Status, headers, body, SSE chunks, or named transport fault. |

### Dual Execution

| Side | Runner design |
|---|---|
| Python | `tests/native_sol/run_python_parity.py` runs the current c3eb-equivalent Python tree. It monkeypatches Convey client construction, chat `requests.get`, clock/date seams, support build identity, and subprocess spawn. It captures stdout/stderr/exit and request shapes. |
| Native | Rust tests in `solstone-core-sol-client` load the same vectors, run through `solstone-core-sol-client-cli` plus fake seams, and capture stdout/stderr/exit/request shapes. |

Both sides assert the same vector expectations. Python is the byte-for-byte oracle unless a vector lists a permitted normalization.

### Determinism

| Source | Seam |
|---|---|
| Activities default today | App date provider from the fake clock. |
| Health pipeline default today/yesterday | App date provider from the fake clock. |
| Chat `POLL_SECONDS` and `IDLE_CEILING_SECONDS` | Fake monotonic clock and fake wait primitive; no real sleeping in tests. |
| Chat SSE stream | Fake transport supplies byte chunks and EOF/interruption points. |
| Support diagnose build identity | Fake build-identity provider; no real `git rev-parse` in parity tests. |
| Support attach files | Fixture file provider controls existence, filename, byte length, and bytes. |
| Multipart boundary | Fake transport can force a stable boundary or normalize boundary bytes explicitly. |
| Process spawning | Spawn-fail seam is installed in all parity tests. Any spawn attempt fails the vector. |

## 5. Seam Inventory

| Seam | Rust shape | Fake injection |
|---|---|---|
| HTTP transport | `HttpTransport` trait with `send(request, policy)` and `open_sse(request, policy)`. | Scripted transport validates method/path/query/body/multipart/headers and returns canned responses or faults. |
| Clock | `Clock` trait with wall date/time, monotonic time, and wait/advance operations. | Fixed date and scripted monotonic clock; chat tests advance without sleeping. |
| Process spawn | `ProcessSpawner` trait. | Default test seam fails every spawn; migrated handlers receive no real spawner. Unsupported commands return explicit unsupported outcomes without spawning. |
| Build identity | `BuildIdentityProvider` trait. | Deterministic version/revision/platform provider for support diagnose. Real provider may read package metadata and git only in process shell context. |
| File provider | `FileProvider` equivalent for local attach paths. | Fixture-backed file existence, size, name, and byte source. |
| SSE parser | Pure parser/iterator over byte chunks into JSON object events. | Unit vectors cover comments/heartbeats, `event:` plus `data:`, multiline data joined by `\n`, one leading-space strip after `data:`, blank-line flush, clean EOF, interruption, malformed JSON ignored, non-object frames ignored. |
| Ordered JSON | Ordered value abstraction for server responses that must preserve insertion order. | Corpus asserts health pipeline pretty output after parse/re-dump. |
| Output sink | Process shell concern only. Library returns typed output buffers. | Tests capture buffers directly. |

### Native Error Taxonomy

| Native error | Python target | Owner-facing message |
|---|---|---|
| `Unreachable` | `ConveyUnreachableError` | Base `message()` is `I couldn't reach the journal over HTTP.`; owner-facing rendering is app-specific — see note. |
| `Timeout` | `ConveyTimeoutError` | `The journal didn't answer in time.` |
| `MalformedSuccess` | malformed 2xx path | `I couldn't read the journal response.` |
| `UnreadableServerError` | non-2xx unreadable error path | `The journal returned an unreadable error.` |
| `ReasonRejected` | non-2xx reason-coded JSON | Server `error`, `reason_code`, `detail`, `status`, payload. |

App modules render app-specific wrappers over these errors where Python does. On `Unreachable`: support renders its two-line portal fallback and chat renders the service-down message (both `require_service=False`); `require_service=True` apps such as activities and health render the shared service-down message (`sol: solstone isn't running. Start it with 'journal up' and retry.`), because their Python path exits via `require_solstone()` before the handler runs. Raw OS/socket/ureq text stays internal diagnostic detail only.

## 6. Health Pipeline Server Route

### Decision

Add exactly one server behavior: `GET /api/health/pipeline?day=YYYYMMDD`.

| Requirement | Design |
|---|---|
| Explicit day | Server requires `day`. It does not compute today, yesterday, or read CLI flags. |
| Date validation | Missing or noncanonical `day` reuses existing health/convey reason-code behavior; the prior `INVALID_DAY` idea is retired for this route. |
| Calculation | Route delegates to `summarize_pipeline_day(day)`. |
| Calculation failure | Preserve owner-visible bytes/status/exit from the current Python pipeline path. Do not add new clean `ConveyClientError` handling around pipeline failures. |
| Client behavior | Python and native clients keep `--day` / `--yesterday` / default-today selection and exact mutual-exclusion stderr `--day and --yesterday are mutually exclusive`. |

### Key-Order Landmine

`sol call health pipeline` currently prints `json.dumps(summary, indent=2, sort_keys=False)` from the local `summarize_pipeline_day()` dict. The HTTP cutover must not reorder keys.

Design:

- The Flask route must not use `jsonify` for pipeline.
- It returns an `application/json` response built from Python `json.dumps(summary, ensure_ascii=False, separators=(",", ":"))` so the wire object order is `summarize_pipeline_day()` insertion order.
- Python client parses with `json.loads`, which preserves object insertion order, then prints `json.dumps(summary, indent=2, sort_keys=False)`.
- Native client parses into an ordered JSON value and pretty-prints in insertion order with Python-compatible two-space JSON formatting for this command.
- Contract and parity fixtures pin a representative pipeline summary to prove route wire order, Python post-cutover output, native output, and pre-cutover output remain byte-identical.

Required fixtures:

| Fixture | Proves |
|---|---|
| `health.pipeline.day` | Explicit `--day` request shape and byte-identical pretty output. |
| `health.pipeline.default_today` | Fixed-clock default date selection stays client-owned. |
| `health.pipeline.yesterday` | Fixed-clock yesterday selection stays client-owned. |
| `health.pipeline.mutual_exclusion` | Exact stderr and exit 1 without HTTP. |
| `health.pipeline.server_missing_day` | Direct route returns `missing_required_field`. |
| `health.pipeline.server_invalid_day` | Direct route returns `invalid_day`. |
| `health.pipeline.failure` | Direct route returns `health_report_failed`. |

This is the only permitted Python behavior delta in this design.

## 7. Static Checks + `install-checks` Wiring

### New Checks

| Check | Asserts | Make target |
|---|---|---|
| Grammar oracle self-check | Fixture schema/source/count/bytes/SHA match the corrected pin. | `check-native-sol-grammar-oracle` |
| 20-file digest guard | `git ls-tree HEAD -- <20 files> | LC_ALL=C sort | sha256sum` equals `1d14f01a819f2f44bfe229603aa38861cda3460ff1ca66b9593a33b6172a772d`. This runs against HEAD. | `check-native-sol-python-manifest` |
| Native lead inventory staleness | Generated inventory source and JSON report match app-owned authorities; inventory equals pinned lead subset; test app fixture proves no central edit is needed. | `check-native-sol-inventory` |
| Oracle subset join | Native `sol call` lead entries match frozen oracle `path`, `kind`, `help`, and `params`; chat excluded and covered by parity. | part of `check-native-sol-inventory` |
| Four-way conformance join | Lead manifest, app-local authority, server route, and contract fragment agree; omissions from any side fail. | `check-native-sol-conformance` |
| Contract-route coverage | Activities/support/health fragments are present and their route method/path/reason codes match Flask routes. | `check-native-sol-contract-routes` and extended `check-openapi` |
| Architecture lint | No app vocab/paths/renderers in shared modules; no god registry; no app switchboard outside generated files; migrated handlers do not depend on compatibility dispatch or process spawn. Allowlist fails on new and stale entries. | `check-native-sol-architecture` |
| No Python spawn | Static lint plus spawn-fail seam tests prove migrated commands never spawn Python; unsupported commands return explicit unsupported outcomes. | `check-native-sol-no-python-spawn` |
| Parity corpus | Python and native runners execute the same vectors and match stdout/stderr/exit/request shapes modulo declared normalizations. | `check-native-sol-parity` |

### Wiring Order

Wire into `install-checks` before the existing Rust gates and before `check-openapi` where relevant:

1. `check-native-sol-grammar-oracle`
2. `check-native-sol-python-manifest`
3. `check-native-sol-inventory`
4. `check-native-sol-conformance`
5. `check-native-sol-contract-routes`
6. `check-native-sol-architecture`
7. `check-native-sol-no-python-spawn`
8. `check-native-sol-parity`
9. Existing `check-openapi`, extended to include activities/support/health fragments and regenerated `docs/openapi/convey-clients.json` in implementation.
10. Existing Rust format/MSRV/clippy/test/deny/iOS gates.

No `.github/workflows` changes are needed; repo CI flows through `make ci`, and `make ci` flows through `install-checks` plus tests.

## Implementation Order

1. Commit frozen fixtures and self-checks: grammar oracle fixture, 20-file manifest file, check scripts, and Makefile targets.
2. Add app-owned authority format and generator with a tiny test authority to prove zero central-list edits.
3. Add Rust crate scaffolding with SPDX headers and empty shared/app modules.
4. Add `ureq`-backed transport, port reader, decode/errors, ordered JSON, and SSE parser behind seams.
5. Add activities/support/health/moved/chat app modules in the lead-slice order, each with parity vectors before handler completion.
6. Add health pipeline server route and OpenAPI fragments for activities/support/health.
7. Add four-way conformance, architecture lint, no-spawn check, parity runners, and install-checks wiring.
8. Run the narrow and then full requested gates in implementation, not in this design stage.

## Risks And Open Questions

| Risk / question | Design response |
|---|---|
| Ordered JSON for health pipeline can drift in Rust. | Use an ordered value representation and parity fixtures; do not rely on default `serde_json::Map` ordering unless configured to preserve order. |
| HTTP transfer semantics may drift from Python. | Use `ureq`/`ureq-proto` for Content-Length, connection-close, and chunked body support; keep multipart construction and the SSE parser parity-tested in native code. |
| Authority TOML could become a second registry. | It is app-owned, per-app, and generated into aggregate source; central shared hand lists are forbidden by architecture lint. |
| 20-file digest does not cover health code. | Treat the 20-file digest as source-baseline evidence only. Operation coverage comes from the lead-slice manifest and four-way join, including health. |
| Support consent flows are easy to over-abstract. | Keep all support-specific copy, dry-run, draft capture, and diagnose fallback in `apps/support`; shared transport only returns generic errors. |
| Compatibility dispatcher could become a fallback. | It only classifies unsupported inputs and returns an explicit unsupported outcome. No Python spawn path exists in native migrated handlers. |

## Closing Confirmation

- The 20-file digest guard runs against HEAD and is designed to stay green.
- The installed Python `sol` default, packaging, entry points, and managed wrappers remain untouched.
- No Python fallback exists for migrated native commands.
- No second hand-maintained command registry is introduced.
- No native code sets `SOLSTONE_JOURNAL`.
- Every new Rust/Python/generated source file gets the repository SPDX header.
- No plugin system, runtime-loaded modules, network contract fetching, or telemetry is introduced.
- `health pipeline` is the only permitted Python behavior delta, and only to move its calculation behind `GET /api/health/pipeline?day=YYYYMMDD` while preserving CLI grammar, date selection, rendering, errors, and exit behavior.
