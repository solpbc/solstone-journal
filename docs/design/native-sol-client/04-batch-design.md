# Native Sol Batch Gate/Manifest/Coverage Design

This design extends the lead-slice architecture in `02-design.md`; it does not
replace the local-authority pattern, frozen grammar oracle, generated Rust
inventory, or four-way HTTP conformance join.

Fixed partition for the sol-call oracle:

| Class | Count | Definition |
|---|---:|---|
| Native HTTP | 152 | `surface="sol-call"` authority entries with `entry_type="http"` |
| Journal Python-compat | 23 | Oracle entries not covered by any sol-call authority; all must be group `journal` |
| Native stubs | 3 | `identity`, `navigate`, and `link observer-pause` |

The per-group HTTP counts are exact: activities 6, awareness 4, body 3, chat 1,
entities 22, facets 3, health 4, import 5, ledger 4, link 8, profile 4,
settings 14, sol 4, speakers 31, support 11, thinking 23, transcripts 5. These
sum to 152. Existing top-level `sol chat` remains outside this 178-path sol-call
partition.

## D1 - Full-Inventory Classification Gate

Decision: extend `scripts/build_native_sol_inventory.py` and keep the Make target
`check-native-sol-inventory`. The script already owns authority discovery,
grammar-oracle subset comparison, `entry_type="local"`, and PUT/DELETE method
validation, so the full partition check belongs beside `check_oracle_subset`
rather than in a second parser.

The enhanced check loads the 178-entry grammar oracle and all non-private
`surface="sol-call"` authorities. It fails on duplicate oracle paths, duplicate
authority paths, duplicate operation IDs, extra authority paths, and any grammar
drift in kind/help/params. It then computes `uncovered = oracle_paths -
authority_paths`; this set must contain exactly 23 paths, every path must start
with `journal`, and no journal path may have an authority. This avoids a frozen
per-leaf journal list while still proving journal remains Python-compat.

Authority classification is exact: 152 `http`, 2 `moved-stub`, and 1 `local`.
The two moved stubs are `identity` and `navigate`, both stderr
`Moved to \`journal <name>\` — run that instead.` and exit 2. The one local stub
is `link observer-pause`, owned by `solstone/apps/network/native/`, with stdout
`observer-pause is not yet available.`, exit 0, and no method/route/contract
fields.

## D2 - Lead-Manifest Scaling / Conformance

Decision: eliminate the hand-maintained lead manifest from conformance. A
155-entry hand manifest would duplicate authority data; a generated projection is
acceptable but unnecessary if the gates derive their expected sets from
authorities and contracts.

`scripts/check_native_sol_conformance.py` should read authorities, Flask routes,
and the assembled OpenAPI document directly. Authorities contribute
`operation_id`, path, entry type, method, route, and `contract_operation_id`.
Flask routes contribute actual method/path presence plus server reason codes via
the existing AST route scanner. Contract fragments contribute operation IDs,
method/path bindings, and `x-reason-codes`; this becomes the natural home for
reason-code truth. The generated Rust inventory remains joined through the
preceding `check-native-sol-inventory` target, which proves the authority-derived
inventory is current before conformance runs.

For each HTTP authority, conformance asserts: Flask has the method+route,
contract has `contract_operation_id`, contract method/path matches the authority,
and contract reason-code set equals the server route reason-code set. For
`moved-stub`, `local`, and `top-level-chat` entries, conformance asserts no
regular HTTP contract is required unless the existing top-level chat authority
declares backing contract operations.

## D3 - Applicability Manifest

Decision: add a committed frozen fixture at
`core/fixtures/native-sol/applicability.json`, keyed by leaf ID, where leaf ID is
the authority `operation_id`. The manifest covers exactly the 152 native HTTP
leaves: the 21 lead-slice HTTP leaves (activities 6, support 11, health 4) plus
the 131 batch HTTP leaves. Stubs and journal Python-compat entries are excluded;
the 132nd batch path, `link observer-pause`, is a local stub and has no
applicability entry.

Schema:

| Field | Shape | Meaning |
|---|---|---|
| `schema` | string | `native-sol-applicability-v1` |
| `http_count` | integer | Must be 152 |
| `entries` | object | Keys are HTTP authority operation IDs |
| `entries.*.path` | string array | Oracle argv path for diagnostics |
| `entries.*.group` | string | First argv segment, using `link` for network override |
| `entries.*.pagination` | object | `enabled`, `kind` (`none`, `server`, `client-loop`, `client-truncate`), `params`, `client_cap` |
| `entries.*.mutation` | object | `enabled`, `methods`, `state_change` |
| `entries.*.upload` | object | `enabled`, `fields`, `file_count` (`none`, `single`, `multi`), `per_file_failure` (`n/a`, `abort`, `continue`) |
| `entries.*.env_default` | object | `vars`, each with `name` from `SOL_DAY`/`SOL_FACET`/`SOL_SEGMENT`/`SOL_STREAM`, `mode` (`required`, `optional-selector`, `valid-absent`), and required negative cases |
| `entries.*.confirmation` | object | `enabled`, `prompts`, `bypass_flags`, refusal exit behavior |
| `entries.*.consent` | object | `enabled`, flags such as `--yes`/`--anonymous`, and default direction |
| `entries.*.dry_run` | object | `enabled`, flags such as `--commit`, `--submit`, `--no-submit`, and default direction |
| `entries.*.multi_request` | object | `enabled`, `min_requests`, `max_requests`, and ordered `boundaries` |

For the 131 new batch HTTP leaves, `upload.enabled=false`; `link
observer-pause` is outside the HTTP manifest. Lead support upload leaves are
recorded as `upload.enabled=true`, including `support.attach` and any
support-create draft-attachment path represented by the lead fixtures. The
coverage gate consumes this manifest as requirements, not documentation:
`multi_request` requires boundary-failure vectors; `dry_run` requires preview
and commit/submit vectors; `env_default` requires explicit, absent, and invalid
vectors; `pagination` requires its advertised paging/truncation case; `upload`
requires file-shape and file-failure vectors.

Dispatches must fill `case_ids` from the command behavior and acceptance
criteria first, then add or bless parity vectors to satisfy those requirements.
Do not seed applicability from the vectors that happen to exist: every
multi-request later-boundary failure, upload missing/unreadable/later-file
boundary plus rejection-before-mutation, env explicit/absent/invalid or
valid-absent case, and dry-run preview/commit case is required evidence even if
the current fixture set would otherwise pass without it.

## D4 - Coverage-Equality Gate

Decision: add `scripts/check_native_sol_coverage.py` with Make target
`check-native-sol-coverage`. This is a fixture coverage gate, not a behavior
test; it reads authorities, `applicability.json`, and frozen native-sol parity
vectors.

The required set is the 152 HTTP operation IDs from authority discovery, and the
D1 classification gate has already proven those authority paths equal the frozen
grammar-oracle HTTP paths. This makes the required-set anchor independent
frozen-oracle evidence, not a circular native-only inventory. AC#5 means the
required set must never be built from the vectors themselves; it does not forbid
mapping a vector to the leaf it exercises.

Each parity vector maps to its leaf by resolving `surface` and `argv` through the
same production longest-path dispatcher used by `solstone-core-sol-client-cli`.
The coverage gate must call or share that resolver rather than maintaining
hand-rolled prefix matching. This works for frozen fixtures such as
`core/fixtures/native-sol/parity/health.jsonl`, whose vectors have `id` and
`argv` but no `leaf_id`. New fixtures may carry optional clarity tags, but if a
tag is present the gate asserts it agrees with production argv-to-leaf dispatch;
the tag is never required for frozen vectors.

The request-binding, success, and failure buckets are classified from observable
fields already present in every parity vector. Request-binding means
`expected.requests` pins the outgoing shape: method, route, query, body,
headers, and timeout policy. Success means `expected.exit == 0` and the scripted
transport responses for the exercised HTTP calls are non-fault 2xx responses.
Failure means `expected.exit != 0`, or any scripted transport request injects a
fault or non-2xx response.

The existing `tests/native_sol/test_parity_coverage.py` should keep option and
positional token checks, but the new gate adds per-leaf equality and
applicability-driven adversarial requirements. A leaf with no request-binding
vector, no success vector, or no failure vector fails even if its option tokens
are covered elsewhere. Explicit adversarial identification is required only for
manifest-driven subcases such as multi-request boundaries, env-default
explicit/absent/invalid cases, dry-run preview+commit, and pagination. Those
requirements apply to editable fixtures for leaves that actually have those
dimensions; health's four frozen leaves have no multi-request, env-default,
dry-run, upload, or pagination dimensions, so the new gate never requires a
health fixture edit.

## D5 - Contract-Route Gate Disposition

Decision: derive `MIGRATED_ROUTES` in place inside
`scripts/check_native_sol_contract_routes.py` from HTTP authorities and remove
the hardcoded dict. Keeping this gate separate preserves its single
responsibility: contract routes agree with Flask routes for migrated native HTTP
operations.

The derived check builds expected entries from every `surface="sol-call"`,
`entry_type="http"` authority: `(method, route) -> contract_operation_id`. For
each expected route, Flask must expose the same method+OpenAPI path, and the
assembled contract must expose the same method+path with the same operation ID.
The reverse check remains scoped to expected native operation IDs, so unrelated
contract operations do not become part of this gate by accident.

## D6 - Repo-Wide Static Ownership Proof

Decision: extend the existing static gates instead of adding a third one.
`check_native_sol_no_python_spawn.py` should own process/Python fallback checks;
`check_native_sol_architecture.py` should own source placement, shared-client
vocabulary, and native HTTP ownership checks.

Both scripts should enumerate native command sources from authority discovery,
not broad globs alone, so new groups are covered automatically and stale native
files can be separately reported. The no-spawn check forbids `std::process`,
`tokio::process`, `Command::new`, `.spawn()`, `.output()`, `exec*` calls,
`python`/`python3` fallback strings, PyO3/CPython references, and compatibility
dispatch symbols outside the process seam. The architecture check forbids native
HTTP command sources from importing or referencing Python server/domain modules,
journal paths, direct journal environment resolution, or filesystem mutation.

Filesystem access is allowed only through explicit shared seams where already
required, such as port discovery, fixture file providers, and support upload file
reading. App-local `command.rs` files for HTTP leaves must not call `std::fs`,
`File`, `OpenOptions`, path deletion/creation APIs, or journal/domain writers;
they must build HTTP requests through `solstone-core-sol-client` transport.

## D7 - Shared-vs-App-Local Extraction Policy

Decision: shared crate code is allowed only for app-neutral primitives used by
more than one app. Anything with app vocabulary, route names, terminal messages,
or domain-specific terminal states stays app-local beside the Python owner.

Pagination should become a shared iterator primitive because ledger, profile,
and awareness all need limit/offset client loops. The primitive should accept an
initial offset, page limit, optional client cap, and a closure that maps
`offset/limit` to one HTTP request; rendering and item interpretation stay
app-local.

PUT and DELETE do not need a new primitive: `HttpMethod` already includes
`Delete` and `Put`, and `UreqHttpTransport` sends both. The batch should add
app-local uses for thinking keys/local endpoints against the existing shared
transport.

Long-poll flows stay app-local. Link pair/private-link, thinking scout, and
thinking confidential have different terminal states and user-facing messages;
they should reuse the existing `Clock` seam for deterministic waiting. Ordered
JSON pretty-printing is already shared from the health lead and should be reused
where human parity requires stable object order.

## D8 - Group Porting Order

Decision: stage the work so each new gate is proven on small surfaces before the
large domain ports.

1. Body (3) first: new fragment, new `body_bp` registration, HTTP authority,
   applicability entries, request/success/failure parity, and static ownership
   with minimal command logic.
2. Sol (4), then facets (3): sol exercises simple identity mutations; facets
   exercises CLI app `facets` routed through `curation_bp` and the 400
   non-envelope domain error case.
3. Awareness (4): first batch pagination/client-loop group and GET/POST dual
   route shape.
4. Ledger (4) and profile (4): built-in tool ownership under `solstone/convey/`
   and shared pagination reuse.
5. Import (5), transcripts (5), and link (8 plus local stub): import extends an
   existing fragment, transcripts exercises hidden aliases and env selectors,
   link exercises network name override, private-link subgroup, long-poll, and
   the `observer-pause` local stub. The stub gets a parity vector in the
   editable moved/stub fixture `core/fixtures/native-sol/parity/moved.jsonl`
   asserting stdout
   `observer-pause is not yet available.` and exit 0, but remains outside the
   152-leaf applicability manifest and HTTP coverage-equality set.
6. Settings (14): nested subgroups and config mutation breadth.
7. Entities (22), thinking (23), speakers (31): large groups last, with thinking
   covering PUT/DELETE and long-poll provider flows, and speakers covering the
   widest reason-code and multi-request surface.

## Additional Required Decisions

### AC#8 Owner Confirm Then Backfill

`speakers confirm-owner` is the canonical multi-request boundary-failure case.
The default path is POST confirm, then POST backfill with `commit=true`; the
`--no-backfill` path is one request. The applicability entry marks
`multi_request.enabled=true`, `min_requests=1`, `max_requests=2`, and one
boundary `confirm -> backfill`. Parity must include a vector where confirm
succeeds and backfill fails; output and exit must preserve that confirm already
committed and must not imply rollback.

### Contract Fragments

Add new fragment modules to `FRAGMENT_MODULES` for:

| Area | Fragment module |
|---|---|
| awareness | `solstone.apps.awareness.contract` |
| body | `solstone.apps.body.contract` |
| entities | `solstone.apps.entities.contract` |
| facets/curation | `solstone.apps.curation.contract` |
| ledger | `solstone.convey.ledger_contract` |
| profile | `solstone.convey.profile_contract` |
| settings | `solstone.apps.settings.contract` |
| sol | `solstone.apps.sol.contract` |
| speakers | native convey-shell contract surface |
| thinking | `solstone.apps.thinking.contract` |
| transcripts | `solstone.apps.transcripts.contract` |

Add operations to existing fragments:

| Area | Existing fragment |
|---|---|
| link/network | `solstone.apps.network.contract` |
| import journal-source routes | `solstone.apps.import.contract` |
| chat start | `solstone.convey.chat_contract` |

`chat start` should not create a second `/api/chat/*` fragment unless the operator wants
contract ownership to follow CLI ownership instead of route ownership; the
current contract module already owns the `/api/chat` route family.

### Blueprint Collectors

Both conformance and contract-route collectors should use one shared blueprint
registration helper and register the route modules below. This covers the 152
sol-call HTTP leaves plus the existing top-level chat HTTP surface.

| Area | Import | Symbol |
|---|---|---|
| activities | `solstone.apps.activities.routes` | `activities_bp` |
| support | `solstone.apps.support.routes` | `support_bp` |
| health | `solstone.convey.health` | `bp as health_bp` |
| chat start/top-level chat | `solstone.convey.chat` | `chat_bp` |
| root/top-level chat backing | `solstone.convey.root` | `bp as root_bp` |
| awareness | `solstone.apps.awareness.routes` | `awareness_bp` |
| body | `solstone.apps.body.routes` | `body_bp` |
| facets | `solstone.apps.curation.routes` | `curation_bp` |
| entities | `solstone.apps.entities.routes` | `entities_bp` |
| import | `solstone.apps.import.routes` | `import_bp` |
| link | `solstone.apps.network.routes` | `network_bp` |
| settings | `solstone.apps.settings.routes` | `settings_bp` |
| sol | `solstone.apps.sol.routes` | `sol_bp` |
| speakers | native convey-shell route surface | native speakers routes |
| thinking | `solstone.apps.thinking.routes` | `thinking_bp` |
| transcripts | `solstone.apps.transcripts.routes` | `transcripts_bp` |
| ledger | `solstone.convey.ledger` | `bp as ledger_bp` |
| profile full/brief/cadence | `solstone.convey.profile` | `bp as profile_bp` |
| profile list-active | `solstone.convey.profile` | `profiles_bp` |

Register network with its normal `/app/network` prefix; the `/app/link` alias is
legacy production registration, not the native CLI contract route.

### Make Wiring

Keep the native gate order in `install-checks` anchored where it is today:
grammar oracle, Python manifest, inventory, architecture, contract-route,
conformance, no-python-spawn, then OpenAPI. The inventory target now includes D1
classification. Add `check-native-sol-coverage` after conformance and before
no-python-spawn so the fixture coverage equality runs after the route/contract
join has established the HTTP operation set.

### Frozen Guardrails

The 20-file Python digest gate stays frozen and unchanged. The health four-file
native set, health fixture bytes, and grammar-oracle pin stay untouched except
through their existing approved generator/check flows. The journal group remains
Python-compat and receives no authorities. No new server routes are part of this
batch; `link observer-pause` is a local native stub and requires no Flask route
or contract operation.

## Risks / Open Questions

- Route reason-code extraction is AST-based today. If a batch route emits reason
  codes through dynamic variables the conformance gate should either reject that
  pattern or require a small explicit route-side annotation; it should not weaken
  the contract reason-code equality.

## Frozen-Fixture Confirmation

The new gates classify `core/fixtures/native-sol/parity/health.jsonl` strictly
read-only through production argv-to-leaf dispatch and observable expected
request/exit/transport fields. No new gate edits the frozen health fixture, adds
fields to it, or edits the 20-file Python manifest or grammar oracle.
