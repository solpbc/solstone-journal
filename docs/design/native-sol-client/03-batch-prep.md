# Native sol Client Batch Prep

This file ground-truths the remaining native-sol HTTP batch against the frozen
grammar oracle at `core/fixtures/native-sol/sol-call-grammar-v1.json:1`. The
oracle currently contains 178 `sol call` entries; the 14 groups below account
for 132 entries.

Reference context loaded first:

- Lead-slice findings and table format: `docs/design/native-sol-client/00-prep-findings.md:72`, `docs/design/native-sol-client/00-prep-findings.md:101`, `docs/design/native-sol-client/00-prep-findings.md:176`.
- Native authority/conformance design: `docs/design/native-sol-client/02-design.md:44`, `docs/design/native-sol-client/02-design.md:83`, `docs/design/native-sol-client/02-design.md:114`, `docs/design/native-sol-client/02-design.md:190`, `docs/design/native-sol-client/02-design.md:255`.
- Lead implementation pattern: `solstone/apps/activities/native/authority.toml:1`, `solstone/apps/activities/native/command.rs:1`.
- App discovery and network-to-link CLI name override: `solstone/think/call.py:22`, `solstone/think/call.py:73`, `solstone/think/call.py:120`.
- Shared Convey client: default `ConveyClient(require_service=True)` at `solstone/think/convey_client.py:87`; `urlencode(..., doseq=True)` repeated-query encoding at `solstone/think/convey_client.py:129`; JSON error decoding at `solstone/think/convey_client.py:194`; `@convey_cli` stderr/exit handling at `solstone/think/convey_client.py:273`.

All client calls in this batch use `get_client()` with the default
`require_service=True`. No batch leaf uses the support app's
`ConveyClient(require_service=False)` exception.

## 1. Per-leaf HTTP detail table

### awareness

Routes are owned by `solstone/apps/awareness/routes.py:44`. Commands are in
`solstone/apps/awareness/call.py:22`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `awareness imports` | `--record/-r VALUE`, `--declined`, `--nudge` | GET `/app/awareness/api/imports` when no flag; otherwise POST same route with body-JSON `record`, `declined`, or `nudge` (`solstone/apps/awareness/call.py:45`, `solstone/apps/awareness/routes.py:79`). | POST can emit `missing_request_body`, `invalid_json_request`, `invalid_request_value`, `awareness_busy` (`solstone/apps/awareness/routes.py:84`). `@convey_cli` prints server error and exits 1. | Always JSON pretty output; no consent or dry-run. |
| `awareness log` | `<kind>` required, optional `<message>`, `--key/-k VALUE`, `--data/-d JSON` | POST `/app/awareness/api/log`; body-JSON `kind`, `key`, `message`, `data` (`solstone/apps/awareness/call.py:117`, `solstone/apps/awareness/routes.py:136`). | Local invalid JSON: `Error: --data must be valid JSON`, exit 1 (`solstone/apps/awareness/call.py:130`). Server: `missing_request_body`, `invalid_json_request`, `missing_required_field`. | JSON pretty output; no consent or dry-run. |
| `awareness log-read` | optional `<day>`, `--kind/-k VALUE`, `--limit/-n INT` | Repeated GET `/app/awareness/api/log`; query `limit=100`, `offset`, optional `day`, `kind` (`solstone/apps/awareness/call.py:77`, `solstone/apps/awareness/routes.py:125`). | Pagination parser can emit input reason codes through `parse_pagination_params`; `@convey_cli` otherwise handles server errors. | Human rows as one-line JSON; empty prints `No entries found.`; client-side `--limit` keeps last N rows. |
| `awareness status` | optional `<section>` | GET `/app/awareness/api/state`; `section` is applied locally, not sent as query (`solstone/apps/awareness/call.py:23`, `solstone/apps/awareness/routes.py:65`). | Route can emit `awareness_section_not_found` only for query `section`, which CLI does not send (`solstone/apps/awareness/routes.py:65`). Local missing section prints `No '<section>' state.` and exits 0. | JSON pretty output of whole state or selected section; no consent or dry-run. |

### body

The native body authority and handler declare the retained three-command
surface in `solstone/apps/body/native/authority.toml` and
`solstone/apps/body/native/command.rs`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `body day` | `<day_value>` required, `--json` | GET `/app/body/api/day/{day_value}`; path `day_value` (native authority and handler). | Server: `invalid_day`. Local malformed payload exits 1. | `--json` pretty JSON; human prints day, entry count, glucose stats. |
| `body status` | `--json` | GET `/app/body/api/status` (native authority and handler). | No route-specific reason codes in the backing route. Local malformed payload exits 1. | `--json` pretty JSON; human prints imports/entries/coverage. |
| `body window` | `--from VALUE` required, `--to VALUE` required, `--json` | GET `/app/body/api/window`; query `from`, `to` (native authority and handler). | Server: `invalid_request_value` for malformed timestamps, end before start, or window too large. | `--json` pretty JSON; human prints window, entries, brief label. |

### chat

The single batch leaf is `sol call chat start`, distinct from top-level
`sol chat`. Its route is the core chat blueprint at `solstone/convey/chat.py:80`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `chat start` | `--summary VALUE` required, `--message VALUE`, `--category VALUE` required, `--dedupe VALUE` required, `--dedupe-window INT`, `--since-ts INT` required, `--trigger-talent VALUE` required | POST `/api/chat/start`; body-JSON `summary`, `message`, `category`, `dedupe`, `dedupe_window`, `since_ts`, `trigger_talent` (`solstone/apps/chat/call.py:17`, `solstone/convey/chat.py:221`). | Local validation exits 1 with exact strings: `Error: summary is required`, `Error: summary must be 80 characters or fewer`, `Error: message must be 500 characters or fewer`, `Error: dedupe is required`, `Error: trigger_talent is required`, `Error: since_ts must be positive` (`solstone/apps/chat/call.py:37`). Server `ValueError` becomes `invalid_request_value` (`solstone/convey/chat.py:233`), with field validation in the chat start helper. | Compact JSON result; no consent or dry-run. |

### entities

Routes are owned by `solstone/apps/entities/routes.py:124`; edge index routes
start at `solstone/apps/entities/routes.py:405`. Commands are in
`solstone/apps/entities/call.py:477`. `SOL_FACET` and `SOL_DAY` local defaults
use exact errors `Error: facet is required (pass as argument or set SOL_FACET).`
and `Error: day is required (pass as argument or set SOL_DAY).`
(`solstone/apps/entities/call.py:37`, `solstone/apps/entities/call.py:47`).
Repeated `--kinds` uses query `kinds=[...]` and the shared `doseq=True`
encoding.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `entities accept-merge-candidate` | `<source_slug>` required, `<target_slug>` required, `--facet/-f`, `--commit/--no-commit` default false | POST `/app/entities/api/accept-merge-candidate`; body-JSON `facet`, `source_slug`, `target_slug`, `commit`; `facet` from option or `SOL_FACET` required (`solstone/apps/entities/call.py:831`, `solstone/apps/entities/routes.py:899`). | Server: `missing_required_field`, `entity_busy`; domain result may return JSON `status=error`, which CLI prints as `Error: ...` exit 1 (`solstone/apps/entities/call.py:374`). | Default is preview; human prints merge preview fields. `--commit` persists and prints undo hint. |
| `entities aka` | `<entity>` required, `<aka_value>` required, `--facet/-f` | GET `/app/entities/api/{facet}/resolve?name=entity`, then POST `/app/entities/api/{facet}/aka`; path `facet`; body-JSON `entity_id`, `aka`, `exclude_name`, `entity` (`solstone/apps/entities/call.py:687`, `solstone/apps/entities/routes.py:492`, `solstone/apps/entities/routes.py:793`). | Resolve miss prints `Error: Entity '<entity>' not found...`; local first-word skip and existing-alias skip exit 0 (`solstone/apps/entities/call.py:341`, `solstone/apps/entities/call.py:700`). Server: `missing_required_field`, `entity_alias_conflict`, `entity_not_found`, `entity_busy`. | Human only; immediate mutation when POST is reached; no confirmation. |
| `entities ambiguities` | `--status VALUE`, `--json` | GET `/app/entities/api/ambiguities`; query `status` optional (`solstone/apps/entities/call.py:979`, `solstone/apps/entities/routes.py:1095`). | Local status validation: `Error: --status must be open or resolved.`, exit 1. Server: `invalid_request_value`, `entity_operation_failed`. | Human list or pretty JSON. |
| `entities attach` | `<type>` required, `<entity>` required, `<description>` required, `--facet/-f` | POST `/app/entities/api/{facet}/attach`; path `facet`; body-JSON `type`, `name`, `description` (`solstone/apps/entities/call.py:614`, `solstone/apps/entities/routes.py:602`). | Server: `missing_request_body`, `missing_required_field`, `invalid_entity_type`, `entity_already_exists`, `entity_blocked`, `entity_not_found`, `entity_busy`. CLI maps already-exists to `Entity '<entity>' already attached.` exit 0 (`solstone/apps/entities/call.py:635`). | Human only; immediate mutation. |
| `entities detect` | `<type>` required, `<entity>` required, `<description>` required, `--facet/-f`, `--day/-d` | POST `/app/entities/api/{facet}/detected`; path `facet`; body-JSON `day`, `type`, `entity`, `description`; `facet` and `day` can default from env and are required after resolution (`solstone/apps/entities/call.py:582`, `solstone/apps/entities/routes.py:529`). | Server: `missing_required_field`, `invalid_entity_type`, `entity_blocked`, `invalid_request_value`, `entity_busy`. | Human only; immediate mutation. |
| `entities dismiss-merge-candidate` | `<source_slug>` required, `<target_slug>` required, `--facet/-f` | POST `/app/entities/api/dismiss-merge-candidate`; body-JSON `facet`, `source_slug`, `target_slug` (`solstone/apps/entities/call.py:883`, `solstone/apps/entities/routes.py:938`). | Server: `missing_required_field`, `entity_busy`; domain `status=error` prints `Error: ...` exit 1. | Human status; immediate mutation. |
| `entities entity-history` | `<entity_id>` required, `--json` | GET `/app/entities/api/journal/entity/{entity_id}/history`; path `entity_id` (`solstone/apps/entities/call.py:1034`, `solstone/apps/entities/routes.py:1046`). | Server: `entity_not_found`, `entity_operation_failed`. | Human version rows or pretty JSON; read-only. |
| `entities history` | `<entity>` required, optional `<peer>`, repeated `--kinds`, `--facet/-f`, `--day-from`, `--day-to`, `--limit/-n` default 50, `--offset` default 0, `--json` | GET `/app/entities/api/history`; query `entity`, `peer`, repeated `kinds`, `facet`, `day_from`, `day_to`, `limit`, `offset` (`solstone/apps/entities/call.py:1134`, `solstone/apps/entities/routes.py:430`). | Server: `missing_required_field`, `invalid_request_value`, `edge_index_unavailable`; unresolved entity returns `{resolved:null, query, candidates}` and CLI exits 1 with suggestions (`solstone/apps/entities/call.py:153`). | Human evidence rows or pretty JSON; read-only; offset pagination. |
| `entities list` | optional `<facet>`, `--facet/-f`, `--day/-d` | If no day, GET `/app/entities/api/{facet}`; if `--day`, GET `/app/entities/api/{facet}/detected?day=...`; path `facet`; query `day` for detected (`solstone/apps/entities/call.py:477`, `solstone/apps/entities/routes.py:482`, `solstone/apps/entities/routes.py:520`). | `facet` required via arg/option/env. Server: base route `entity_operation_failed`; detected route `missing_required_field` if day omitted by a non-CLI caller. | Human list; read-only. |
| `entities merge` | `<source_slug>` required, `<target_slug>` required, `--commit/--no-commit` default false, `--keep-source-as-aka/--no-keep-source-as-aka` default true | POST `/app/entities/api/merge`; body-JSON `source_slug`, `target_slug`, `commit`, `keep_source_as_aka` (`solstone/apps/entities/call.py:919`, `solstone/apps/entities/routes.py:963`). | Server: `missing_required_field`, `entity_busy`, plus `_entity_operation_error`: `entity_operation_failed`, `operation_no_longer_available`, `entity_not_found`, `entity_blocked`, `invalid_request_value` (`solstone/apps/entities/routes.py:253`). CLI preserves JSON error shape to stderr (`solstone/apps/entities/call.py:420`). | Pretty JSON always; default dry-run/plan, `--commit` persists and returns undo descriptor. |
| `entities merge-candidates` | `--facet/-f`, `--status`, `--json` | GET `/app/entities/api/merge-candidates`; query `facet`, `status` (`solstone/apps/entities/call.py:795`, `solstone/apps/entities/routes.py:886`). | No route-specific reason codes. | Human rows or JSON item list; read-only. |
| `entities move` | `<entity>` required, `--from VALUE` required, `--to VALUE` required, `--merge`, `--consent` | GET source resolve, GET target resolve, then POST `/app/entities/api/move`; body-JSON `entity`, `from_facet`, `to_facet`, `merge`, `consent` (`solstone/apps/entities/call.py:517`, `solstone/apps/entities/routes.py:736`). | Local missing facet after resolver: `Error: Facet '<facet>' (--from) does not exist.` or `(--to) does not exist.` (`solstone/apps/entities/call.py:545`). Server: `missing_required_field`, `entity_operation_failed`, `entity_already_exists`; resolver can yield local not-found/blocked errors. | Human only; immediate mutation. `--consent` is an audit assertion, not a prompt. |
| `entities network` | `<entity>` required, repeated `--kinds`, `--facet/-f`, `--day-from`, `--day-to`, `--limit/-n` default 25, `--evidence-limit` default 5, `--include-principal`, `--json` | GET `/app/entities/api/network`; query `entity`, repeated `kinds`, `facet`, `day_from`, `day_to`, `limit`, `evidence_limit`, `include_principal` (`solstone/apps/entities/call.py:1084`, `solstone/apps/entities/routes.py:405`). | Server: `missing_required_field`, `invalid_request_value`, `edge_index_unavailable`; unresolved entity returns resolution payload and CLI exits 1 with suggestions. | Human network rows or pretty JSON; read-only; limit/evidence-limit truncation. |
| `entities observations` | `<entity>` required, `--facet/-f` | GET resolve, then GET `/app/entities/api/{facet}/observations?name=resolved_name` (`solstone/apps/entities/call.py:1211`, `solstone/apps/entities/routes.py:1168`). | Resolver local not-found/blocked errors. Server observations route: `missing_required_field`. | Human rows; read-only. |
| `entities observe` | `<entity>` required, `<content>` required, `--facet/-f`, `--source-day VALUE` | GET resolve, then POST `/app/entities/api/{facet}/observe`; body-JSON `name`, `content`, `source_day`, `entity` (`solstone/apps/entities/call.py:1241`, `solstone/apps/entities/routes.py:1177`). | Resolver local not-found/blocked errors. Server: `missing_required_field`, `invalid_request_value`, `entity_busy`. | Human only; immediate mutation. |
| `entities overview` | repeated `--kinds`, `--facet/-f`, `--day-from`, `--day-to`, `--limit/-n` default 25, `--json` | GET `/app/entities/api/overview`; query repeated `kinds`, `facet`, `day_from`, `day_to`, `limit` (`solstone/apps/entities/call.py:1179`, `solstone/apps/entities/routes.py:468`). | Server: `invalid_request_value`, `edge_index_unavailable`. | Human overview or pretty JSON; read-only; limit truncation. |
| `entities record-merge-candidate` | `<source>` required, `<target>` required, `--facet/-f`, `--day/-d`, `--evidence` required, `--basis` default `name-variant`, `--detections`, `--needs`, `--json` | POST `/app/entities/api/record-merge-candidate`; body-JSON `facet`, `day`, `source`, `target`, `evidence`, `basis`, `detections`, `needs`; `facet`/`day` can default from env and are required (`solstone/apps/entities/call.py:733`, `solstone/apps/entities/routes.py:835`). | Server: `missing_required_field`, `invalid_request_value` when source and target slug match, `entity_busy`. | Human created/updated line or JSON row; immediate mutation. |
| `entities resolve-ambiguity` | `<ambiguity_id>` required, `<entity_id>` required, `--yes`, `--json` | POST `/app/entities/api/ambiguities/{ambiguity_id}/resolve`; path `ambiguity_id`; body-JSON `entity_id` (`solstone/apps/entities/call.py:1006`, `solstone/apps/entities/routes.py:1113`). | Local refusal: `Refusing to resolve this ambiguity without --yes.`, exit 1 (`solstone/apps/entities/call.py:408`). Server: `missing_required_field`, `entity_not_found`, `entity_busy`, `invalid_request_value`. | Human success or pretty JSON. Confirmation is `--yes`; no prompt. |
| `entities restore-version` | `<entity_id>` required, `<version_id>` required, `--yes`, `--json` | POST `/app/entities/api/journal/entity/{entity_id}/restore`; path `entity_id`; body-JSON `version_id` (`solstone/apps/entities/call.py:1055`, `solstone/apps/entities/routes.py:1057`). | Local refusal: `Refusing to restore this identity version without --yes.`, exit 1. Server: `missing_required_field`, `entity_not_found`, `entity_busy`, `invalid_request_value`, `entity_operation_failed`. | Human success or pretty JSON. Confirmation is `--yes`; no prompt. |
| `entities search` | optional `<query_pos>`, `--query/-q`, `--type/-t`, `--facet/-f`, `--since`, `--limit/-n` default 20 | GET `/app/entities/api/search`; query `query`, `type`, `facet`, `since`, `limit` (`solstone/apps/entities/call.py:1270`, `solstone/apps/entities/routes.py:1207`). | Route coerces invalid `limit` to 20; no reason code for bad limit (`solstone/apps/entities/routes.py:1210`). | Human list; read-only; server-side limit. |
| `entities undo-merge` | `<merge_id>` required, `--yes`, `--json` | POST `/app/entities/api/merge/{merge_id}/undo`; path `merge_id`; body-JSON `{}` (`solstone/apps/entities/call.py:952`, `solstone/apps/entities/routes.py:994`). | Local refusal: `Refusing to undo this merge without --yes.`, exit 1. Server: `entity_busy`, plus `_entity_operation_error` reason set: `entity_operation_failed`, `operation_no_longer_available`, `entity_not_found`, `entity_blocked`, `invalid_request_value`. | Human success or pretty JSON. Confirmation is `--yes`; no prompt. |
| `entities update` | `<entity>` required, `<description>` required, `--facet/-f`, `--day/-d` | If no day: GET resolve then POST `/app/entities/api/{facet}/update-description`; body-JSON `entity_id`, `description`, `entity`, `name`. If `--day`: POST `/app/entities/api/{facet}/update-detected`; body-JSON `day`, `entity`, `description` (`solstone/apps/entities/call.py:643`, `solstone/apps/entities/routes.py:665`, `solstone/apps/entities/routes.py:703`). | Facet required via env. Server no-day: `missing_required_field`, `entity_not_found`, `entity_busy`; detected: `missing_required_field`, `invalid_request_value`, `entity_busy`. | Human only; immediate mutation. |

### facets

CLI commands live in `solstone/apps/facets/call.py:36`, but the HTTP backing
routes are curation routes at `solstone/apps/curation/routes.py:47`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `facets accept` | `<name_key>` required | POST `/app/curation/api/facet/accept`; body-JSON `name_key` (`solstone/apps/facets/call.py:66`, `solstone/apps/curation/routes.py:161`). | Server: `missing_required_field`, `entity_busy`; `_result_response` can return JSON status 400 without a reason envelope for domain `status=error` (`solstone/apps/curation/routes.py:109`). | Human status line; mutation. |
| `facets dismiss` | `<name_key>` required | POST `/app/curation/api/facet/dismiss`; body-JSON `name_key` (`solstone/apps/facets/call.py:78`, `solstone/apps/curation/routes.py:174`). | Server: `missing_required_field`, `entity_busy`; possible domain `status=error` 400 without reason envelope. | Human status line; mutation. |
| `facets list-candidates` | `--status`, `--json` | GET `/app/curation/api/facet/candidates`; `--status` is local filter, not query (`solstone/apps/facets/call.py:37`, `solstone/apps/curation/routes.py:156`). | No route-specific reason codes. | Human list or JSON rows; read-only. |

### import

Routes are owned by `solstone/apps/import/routes.py:80`. Commands are in
`solstone/apps/import/call.py:53`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `import list-staged` | `--source VALUE` required, `--area VALUE` | GET `/app/import/api/journal-sources/{source}/staged`; path `source`; query `area` optional (`solstone/apps/import/call.py:53`, `solstone/apps/import/routes.py:1386`). | Server: `journal_source_problem`, `invalid_request_value`. CLI maps invalid area to `Error: Area must be one of: entities, facets, config.` and missing source to `Error: Import source '<source>' not found...` (`solstone/apps/import/call.py:36`). | One JSON object per staged item; read-only. |
| `import resolve-config` | `<field>` required, `<action>` required, `--source VALUE` required | POST `/app/import/api/journal-sources/{source}/resolve-config`; path `source`; body-JSON `field`, `action` (`solstone/apps/import/call.py:132`, `solstone/apps/import/routes.py:1501`). | Server: `journal_source_problem`, `import_not_found`, `invalid_request_value`; CLI prints `Error: <detail>` for detail-bearing errors. | Human `Resolved config field ...`; mutation. |
| `import resolve-config-all` | `--source VALUE` required, `--category VALUE` required | POST `/app/import/api/journal-sources/{source}/resolve-config-all`; path `source`; body-JSON `category` (`solstone/apps/import/call.py:150`, `solstone/apps/import/routes.py:1522`). | Server: `journal_source_problem`, `import_not_found`, `invalid_request_value`. | Human `Applied N <category> config field(s).`; mutation. |
| `import resolve-entity` | `<source_id>` required, `<action>` required, `--source VALUE` required, `--target VALUE` | POST `/app/import/api/journal-sources/{source}/resolve-entity`; path `source`; body-JSON `source_id`, `action`, `target` (`solstone/apps/import/call.py:75`, `solstone/apps/import/routes.py:1454`). | Server: `journal_source_problem`, `import_not_found`, `invalid_request_value`. | Human action-specific line; mutation. |
| `import resolve-staged-facet` | `<staged_file>` required, `--apply`, `--skip`, `--source VALUE` required | POST `/app/import/api/journal-sources/{source}/resolve-facet`; path `source`; body-JSON `staged_file`, `mode` (`apply` or `skip`) (`solstone/apps/import/call.py:102`, `solstone/apps/import/routes.py:1480`). | Local validation if both/neither apply mode: `Error: Exactly one of --apply or --skip is required.`, exit 1 (`solstone/apps/import/call.py:110`). Server: `journal_source_problem`, `import_not_found`, `invalid_request_value`. | Human applied/skipped line; mutation. |

### ledger

Built-in CLI is `solstone/think/tools/ledger.py:96`; Flask routes are
`solstone/convey/ledger.py:39`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `ledger close` | `<item_id>` required, `--note VALUE` required, `--as VALUE` default `closed`, `--json` | POST `/api/ledger/{item_id}/close`; path `item_id`; body-JSON `note`, `as_state` (`solstone/think/tools/ledger.py:154`, `solstone/convey/ledger.py:142`). | Local BadParameter if `--as` not `closed`/`dropped` (`solstone/think/tools/ledger.py:161`). Server: `missing_request_body`, `invalid_json_request`, `missing_required_field`, `invalid_request_value`, `ledger_item_not_found`, `activities_busy`. | Human table or JSON item array; mutation, no confirmation. |
| `ledger decisions` | `--owner`, `--since`, `--involving`, `--top`, `--facets`, `--json` | Paginated GET `/api/ledger/decisions`; query `owner`, `since`, `involving`, `facets`, plus `limit/offset` from paginator (`solstone/think/tools/ledger.py:178`, `solstone/convey/ledger.py:114`). | Server: `invalid_day`. CLI maps `ledger_item_not_found`/`activities_busy` through `_handle_ledger_error` (`solstone/think/tools/ledger.py:28`). | Human table or JSON list; read-only; `--top` client cap. |
| `ledger get` | `<item_id>` required, `--json` | GET `/api/ledger/{item_id}`; path `item_id` (`solstone/think/tools/ledger.py:141`, `solstone/convey/ledger.py:132`). | Server: `ledger_item_not_found`; CLI prints `ledger item not found: <item_id>`, exit 1. | Human table or JSON item array; read-only. |
| `ledger list` | `--state` default `open`, `--owner`, `--counterparty`, `--age-days-gte`, `--closed-since`, `--top`, `--sort`, `--facets`, `--json` | Paginated GET `/api/ledger`; query `state`, `owner`, `counterparty`, `age_days_gte`, `closed_since`, `sort`, `facets`, plus `limit/offset` (`solstone/think/tools/ledger.py:97`, `solstone/convey/ledger.py:76`). | Local BadParameter if sort not `age_days_desc`, `opened_at_desc`, `closed_at_desc` (`solstone/think/tools/ledger.py:109`). Server: `invalid_request_value`, `invalid_day`. | Human table or JSON list; read-only; `--top` client cap. |

### link

The CLI group is the network app under the name override `link`
(`solstone/think/call.py:22`). Routes are owned by
`solstone/apps/network/routes.py:128`; Convey also registers an `/app/link`
legacy alias for the same blueprint at `solstone/convey/__init__.py:149`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `link authorized-clients` | no params | GET `/app/network/api/devices` (`solstone/apps/network/call.py:401`, `solstone/apps/network/routes.py:491`). | No route-specific reason codes. | Human flat client list; read-only. |
| `link list` | no params | GET `/app/network/api/devices` (`solstone/apps/network/call.py:363`, `solstone/apps/network/routes.py:491`). | No route-specific reason codes. | Human grouped device list; read-only. |
| `link observer-pause` | no params | No HTTP request; local placeholder only (`solstone/apps/network/call.py:417`). | Prints `observer-pause is not yet available.` and exits 0. | Local-only escalation; no render beyond message. |
| `link pair` | `--device-label VALUE`, `--as VALUE`, `--timeout INT` default 300, `--no-wait` | POST `/app/network/pair-start` with body-JSON `device_label`, `role`; unless `--no-wait`, then repeated GET `/app/network/api/devices` and GET `/app/network/api/pair/nonce-status?nonce=...` (`solstone/apps/network/call.py:274`, `solstone/apps/network/routes.py:674`, `solstone/apps/network/routes.py:668`). | Local invalid role: `invalid role; expected one of: phone, observer, peer`, exit 2 (`solstone/apps/network/call.py:297`). Server: `pairing_request_invalid`, `invalid_operation_for_state`, `pairing_relay_unavailable`. Timeout prints `Timed out. Pair code expired.`, exit 2. | Human pair link/join instructions; `--no-wait` avoids polling. |
| `link private-link disable` | no params | POST `/app/network/private-link/disable` (`solstone/apps/network/call.py:251`, `solstone/apps/network/routes.py:612`). | Server: `service_operation_failed`; client maps it to `couldn't turn off your private network right now. try again in a moment.`, exit 1 (`solstone/apps/network/call.py:257`). | Human success or repair-needed line; mutation. |
| `link private-link setup` | `--wait-seconds FLOAT` default 900.0, `--poll-interval FLOAT` default 1.0 | POST `/app/network/private-link/enable`, then repeated GET `/app/network/api/private-link` until terminal or timeout (`solstone/apps/network/call.py:227`, `solstone/apps/network/routes.py:590`, `solstone/apps/network/routes.py:585`). | Server: `invalid_operation_for_state`, `service_operation_failed`, `service_busy`; local terminal failures exit 1 (`solstone/apps/network/call.py:189`). | Prints `setting up your private network...`, possible `continue to approve → {portal_url}`, success/needs-subscription/error text. Consent is external portal approval. |
| `link private-link status` | no params | GET `/app/network/api/private-link` (`solstone/apps/network/call.py:219`, `solstone/apps/network/routes.py:585`). | No route-specific reason codes. | Human status; read-only. |
| `link status` | no params | GET `/app/network/api/status`, GET `/app/network/api/private-link`, GET `/app/network/api/devices` (`solstone/apps/network/call.py:449`, `solstone/apps/network/routes.py:499`, `solstone/apps/network/routes.py:585`, `solstone/apps/network/routes.py:491`). | No route-specific reason codes on these routes. | Human status summary; read-only; multi-request. |
| `link unpair` | `<target>` required | POST `/app/network/unpair`; body-JSON `fingerprint` if target starts `sha256:`, else `device_label` (`solstone/apps/network/call.py:423`, `solstone/apps/network/routes.py:976`). | Server: `missing_required_field`, `paired_device_not_found`. CLI maps not-found to target-specific `No paired device with ...`, exit 1 (`solstone/apps/network/call.py:438`). | Human `Unpaired.`; mutation, no confirmation. |

### profile

Built-in CLI is `solstone/think/tools/profile.py:144`; Flask routes are
`solstone/convey/profile.py:27`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `profile brief` | `<name>` required, `--json` | GET `/api/profile/{name}/brief`; path `name` URL-quoted (`solstone/think/tools/profile.py:169`, `solstone/convey/profile.py:56`). | Server: `entity_not_found`; CLI prints `profile not found: <name>`, exit 1 (`solstone/think/tools/profile.py:29`). | Compact JSON or human brief; read-only. |
| `profile cadence` | `<name>` required, `--include-mentions`, `--json` | GET `/api/profile/{name}/cadence`; path `name`; query `include_mentions=true` when flag set (`solstone/think/tools/profile.py:183`, `solstone/convey/profile.py:64`). | Server: `entity_not_found`. | Compact JSON or human cadence; read-only. |
| `profile full` | `<name>` required, `--facets CSV`, `--include-mentions`, `--json` | GET `/api/profile/{name}`; path `name`; query `facets`, `include_mentions=true` (`solstone/think/tools/profile.py:145`, `solstone/convey/profile.py:44`). | Server: `entity_not_found`. | Compact JSON or human full profile; read-only. |
| `profile list-active` | `--window-days INT` default 30, `--json` | Paginated GET `/api/profiles/active`; query `window_days`, plus `limit/offset` (`solstone/think/tools/profile.py:203`, `solstone/convey/profile.py:74`). | Server: `invalid_request_value` if `window_days` is not an integer. | Compact JSON list or human IDs; read-only; paginator cap optional. |

### settings

Routes are owned by `solstone/apps/settings/routes.py:86`. Commands are in
`solstone/apps/settings/call.py:31`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `settings convey status` | no params | GET `/app/settings/api/convey/status` (`solstone/apps/settings/call.py:156`, `solstone/apps/settings/routes.py:531`). | Server: `settings_operation_failed`. | Human `status_text`; read-only. |
| `settings identity set` | `--name`, `--preferred`, `--bio`, `--timezone`, `--pronouns JSON`, `--add-email`, `--remove-email`, `--add-alias`, `--remove-alias` | GET `/app/settings/api/config`, then POST `/app/settings/api/config`; body-JSON `section=identity`, `data={...}` (`solstone/apps/settings/call.py:274`, `solstone/apps/settings/routes.py:274`, `solstone/apps/settings/routes.py:293`). | Local invalid pronouns: `Invalid JSON in pronouns`, exit 1. Server: `missing_request_body`, `missing_required_field`, `invalid_config_value`, `config_busy`, `settings_operation_failed`. | JSON identity; mutation. |
| `settings identity show` | no params | GET `/app/settings/api/config` (`solstone/apps/settings/call.py:265`, `solstone/apps/settings/routes.py:274`). | Server: `settings_operation_failed`. | JSON identity; read-only. |
| `settings keys clear` | `<env_var>` required | POST `/app/settings/api/config`; body-JSON `section=env`, `key=env_var`, `value=""` (`solstone/apps/settings/call.py:209`, `solstone/apps/settings/routes.py:293`). | Local invalid env var: `Invalid env var: <env_var>. Must be one of: PLAUD_ACCESS_TOKEN`, exit 1 (`solstone/apps/settings/call.py:84`). Server settings reason set above. | JSON cleared status; mutation. |
| `settings keys set` | `<env_var>` required, `<value>` required | POST `/app/settings/api/config`; body-JSON `section=env`, `key=env_var`, `value` (`solstone/apps/settings/call.py:193`, `solstone/apps/settings/routes.py:293`). | Same local env-var validation; server settings reason set. | JSON set/validation status; mutation. |
| `settings keys show` | no params | GET `/app/settings/api/config` (`solstone/apps/settings/call.py:182`, `solstone/apps/settings/routes.py:274`). | Server: `settings_operation_failed`. | JSON key status; read-only. |
| `settings keys validate` | `--cache-result` | GET `/app/settings/api/validate-keys` by default; POST same route when caching (`solstone/apps/settings/call.py:221`, `solstone/apps/settings/routes.py:816`). | Server POST: `config_busy`, `settings_operation_failed`; GET: `settings_operation_failed`. | JSON validation. Default read-only; `--cache-result` mutates cached validation. |
| `settings observer set` | `--enabled/--no-enabled`, `--capture-interval INT` | GET `/app/settings/api/observe`, then POST `/app/settings/api/observe`; body-JSON `tmux.enabled`, `tmux.capture_interval` as supplied (`solstone/apps/settings/call.py:338`, `solstone/apps/settings/routes.py:1090`, `solstone/apps/settings/routes.py:1125`). | Local range error: `tmux.capture_interval must be an integer between <min> and <max>`, exit 1 (`solstone/apps/settings/call.py:354`). Server: `missing_request_body`, `invalid_config_value`, `config_busy`, `settings_operation_failed`. | JSON tmux config; mutation. |
| `settings observer show` | no params | GET `/app/settings/api/observe` (`solstone/apps/settings/call.py:329`, `solstone/apps/settings/routes.py:1090`). | Server: `settings_operation_failed`. | JSON observe config; read-only. |
| `settings processing set` | `--mode`, `--window-start`, `--window-end`, `--time-window/--no-time-window`, `--display-powersave/--no-display-powersave` | POST `/app/settings/api/config`; body-JSON `section=processing`, `data` populated only from provided options (`solstone/apps/settings/call.py:99`, `solstone/apps/settings/routes.py:293`). | Local no-op validation: `error: provide at least one of --mode/--window-start/--window-end/--time-window/--display-powersave`, exit 1 (`solstone/apps/settings/call.py:141`). Server settings reason set including `invalid_config_value`. | JSON processing config; mutation. |
| `settings processing show` | no params | GET `/app/settings/api/processing` (`solstone/apps/settings/call.py:91`, `solstone/apps/settings/routes.py:624`). | Server: `settings_operation_failed`. | JSON processing config; read-only. |
| `settings show` | no params | GET `/app/settings/api/config` (`solstone/apps/settings/call.py:165`, `solstone/apps/settings/routes.py:274`). | Server: `settings_operation_failed`. | JSON summary of identity/transcribe/observe/keys; read-only. |
| `settings transcribe set-backend` | `<backend>` required | GET `/app/settings/api/transcribe`, then POST `/app/settings/api/config`; body-JSON `section=transcribe`, `data.backend` (`solstone/apps/settings/call.py:250`, `solstone/apps/settings/routes.py:558`, `solstone/apps/settings/routes.py:293`). | Local invalid backend: `Invalid backend: <backend>. Must be one of: ...`, exit 1 (`solstone/apps/settings/call.py:255`). Server settings reason set. | JSON transcribe config; mutation. |
| `settings transcribe show` | no params | GET `/app/settings/api/transcribe` (`solstone/apps/settings/call.py:235`, `solstone/apps/settings/routes.py:558`). | Server: `settings_operation_failed`. | JSON transcribe config; read-only. |

### sol

Routes are owned by `solstone/apps/sol/routes.py:55`. Commands are in
`solstone/apps/sol/call.py:20`.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `sol reset` | no params | POST `/app/sol/api/reset` (`solstone/apps/sol/call.py:41`, `solstone/apps/sol/routes.py:811`). | Server: `identity_busy`. | JSON pretty result; mutation. |
| `sol set-name` | `<name>` required, `--status/-s` default `chosen` | POST `/app/sol/api/set-name`; body-JSON `name`, `status` (`solstone/apps/sol/call.py:21`, `solstone/apps/sol/routes.py:775`). | Server: `missing_request_body`, `invalid_json_request`, `missing_required_field`, `identity_busy`. | JSON pretty agent result; mutation. |
| `sol set-owner` | `<name>` required, `--bio/-b` | POST `/app/sol/api/set-owner`; body-JSON `name`, `bio` (`solstone/apps/sol/call.py:49`, `solstone/apps/sol/routes.py:837`). | Server: `missing_request_body`, `invalid_json_request`, `missing_required_field`, `identity_busy`. | JSON pretty result; mutation. |
| `sol sol-init` | no params | POST `/app/sol/api/sol-init` (`solstone/apps/sol/call.py:64`, `solstone/apps/sol/routes.py:868`). | Server: `identity_busy`. | JSON pretty result; mutation. |

### speakers

Routes are owned by `solstone/apps/speakers/routes.py:189`; CLI-specific routes
begin at `solstone/apps/speakers/routes.py:2674`. Commands are in
`solstone/apps/speakers/call.py:72`. Commands with `--commit` preview by
default and persist only when passed (`solstone/apps/speakers/call.py:10`).

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `speakers attribute-segment` | `<day>` required, `<stream>` required, `<segment>` required, `--commit`, `--save/--no-save` default true, `--accumulate/--no-accumulate` default true, `--json` | POST `/app/speakers/api/attribute-segment`; body-JSON `day`, `stream`, `segment`, `commit`, `save`, `accumulate` (`solstone/apps/speakers/call.py:285`, `solstone/apps/speakers/routes.py:2749`). | Server: `missing_required_field`, `invalid_day`, `invalid_segment_or_stream`, `speaker_owner_centroid_required`, `speaker_labels_busy`, `speaker_voiceprint_busy`. CLI maps centroid error to `Error: <detail>`, exit 1. | Default preview prints `REPORT ONLY — pass --commit to persist.`; `--json` suppresses that and emits JSON. |
| `speakers backfill` | `--commit`, `--reattribute`, `--json` | POST `/app/speakers/api/backfill`; body-JSON `commit`, `reattribute` (`solstone/apps/speakers/call.py:491`, `solstone/apps/speakers/routes.py:2725`). | No explicit route reason codes; underlying failures are reflected in returned `errors` list. | Default preview banner; `--commit` persists; human stats or JSON. |
| `speakers backfill-last-seen` | `--commit`, `--json` | POST `/app/speakers/api/backfill-last-seen`; body-JSON `commit` (`solstone/apps/speakers/call.py:556`, `solstone/apps/speakers/routes.py:2717`). | No explicit route reason codes. | Default preview banner; human stats or JSON. |
| `speakers bootstrap` | `--commit`, `--json` | POST `/app/speakers/api/bootstrap`; body-JSON `commit` (`solstone/apps/speakers/call.py:165`, `solstone/apps/speakers/routes.py:2681`). | Server: `speaker_owner_centroid_required`. | Default preview banner; human stats or JSON. |
| `speakers build-from-tags` | `--json` | POST `/app/speakers/api/owner/build-from-tags` (`solstone/apps/speakers/call.py:1040`, `solstone/apps/speakers/routes.py:2218`). | Server: `speaker_voiceprint_busy`, `entity_not_found`. | Human owner status guidance or JSON; immediate mutation if evidence passes. |
| `speakers confirm-owner` | `--backfill/--no-backfill` default true, `--json` | POST `/app/speakers/api/owner/confirm-cli`, then by default POST `/app/speakers/api/backfill` with `commit=true` (`solstone/apps/speakers/call.py:1229`, `solstone/apps/speakers/routes.py:3018`, `solstone/apps/speakers/routes.py:2725`). | First request: `speaker_voiceprint_busy`, `speaker_command_failed`. Second request has no explicit route reason codes. | Human confirmation plus backfill status, or JSON with optional `backfill`; `--no-backfill` skips second mutation. |
| `speakers correct` | `<day>` required, `<stream>` required, `<segment>` required, `<source>` required, `<sentence_id>` required, `<new_speaker>` required, `--json` | POST `/app/speakers/api/correct-attribution`; body-JSON `day`, `stream`, `segment_key`, `source`, `sentence_id`, `new_speaker` (`solstone/apps/speakers/call.py:372`, `solstone/apps/speakers/routes.py:1803`). | Server: `missing_request_body`, `missing_required_field`, `invalid_day`, `invalid_segment_or_stream`, `speaker_not_found`, `entity_blocked`, `speaker_review_unavailable`, `speaker_sentence_missing`, `speaker_owner_voice_too_close`, `speaker_voiceprint_busy`, `speaker_labels_busy`. | Immediate mutation; human correction and propagation offer, or JSON. |
| `speakers day-segments` | `<day>` required, `--limit/-n` default 20, `--json` | GET `/app/speakers/api/segments-cli/{day}`; path `day`; query `limit` (`solstone/apps/speakers/call.py:1203`, `solstone/apps/speakers/routes.py:1293`). | Server: `invalid_day`, `invalid_request_value`. | Human bounded list or JSON; client-visible truncation by `limit`. |
| `speakers detect` | `--force` | POST `/app/speakers/api/owner/detect`; body-JSON `force` (`solstone/apps/speakers/call.py:1022`, `solstone/apps/speakers/routes.py:2204`). | Server: `speaker_voiceprint_busy`. | JSON pretty result; mutation. |
| `speakers discover` | `--json` | POST `/app/speakers/api/discovery/scan` (`solstone/apps/speakers/native/command.rs:448`, `solstone/apps/speakers/routes.py:2366`). | Server: `speaker_discovery_failed` at 503/500. | Human clusters or JSON; degraded scans show warnings; successful scans may write discovery cache. |
| `speakers dismiss-cluster` | `<cluster_id>` required, `--disposition VALUE` required | POST `/app/speakers/api/discovery/dismiss`; body-JSON `cluster_id`, `disposition` (`solstone/apps/speakers/call.py:858`, `solstone/apps/speakers/routes.py:2927`). | Server: `missing_required_field`, `invalid_request_value`, `speaker_review_unavailable`, `speaker_command_failed`. CLI maps `speaker_command_failed` detail to stderr exit 1. | JSON pretty result; mutation. |
| `speakers dismissals` | no params | GET `/app/speakers/api/discovery/dismissals` (`solstone/apps/speakers/call.py:882`, `solstone/apps/speakers/routes.py:2972`). | No explicit route reason codes. | JSON pretty result; read-only. |
| `speakers identify` | `<cluster_id>` required, optional `<name>`, `--entity-id`, `--create`, `--entity-type` default `Person`, `--resolve-only`, `--request-id`, repeated `--reviewed-near-match-entity-id` | POST `/app/speakers/api/discovery/identify-cli`; body-JSON `cluster_id`, `name`, `entity_id`, `create_new`, `entity_type`, `resolve_only`, `request_id`, `reviewed_near_match_entity_ids` (`solstone/apps/speakers/call.py:756`, `solstone/apps/speakers/routes.py:2825`). | Local BadParameter: `name or --entity-id is required` (`solstone/apps/speakers/call.py:785`). Server: `missing_required_field`, `invalid_request_value`, `speaker_identify_recoverable`, `speaker_identify_repair_required`, `speaker_identify_conflict`, `speaker_identify_operation_not_found`, `speaker_not_found`, `invalid_entity_type`, `speaker_command_failed`, `speaker_labels_busy`, `speaker_voiceprint_busy` (`solstone/apps/speakers/routes.py:2526`). CLI prints retry/inspect guidance for identify failure codes (`solstone/apps/speakers/call.py:107`). | JSON pretty result. `--resolve-only` is dry-run resolution. |
| `speakers identify-operation` | `<operation_id>` required | GET `/app/speakers/api/discovery/identify/operations/{operation_id}` (`solstone/apps/speakers/call.py:840`, `solstone/apps/speakers/routes.py:2907`). | Server: `speaker_identify_operation_not_found`; CLI prints inspect guidance. | JSON pretty; read-only. |
| `speakers identify-operations` | no params | GET `/app/speakers/api/discovery/identify/operations` (`solstone/apps/speakers/call.py:832`, `solstone/apps/speakers/routes.py:2900`). | No explicit route reason codes. | JSON pretty; read-only. |
| `speakers identify-undo` | `<operation_id>` required | POST `/app/speakers/api/discovery/identify/undo`; body-JSON `operation_id` (`solstone/apps/speakers/call.py:811`, `solstone/apps/speakers/routes.py:2880`). | Server: `missing_required_field`, identify failure codes above, `speaker_labels_busy`, `speaker_voiceprint_busy`, `speaker_command_failed`. | JSON pretty result; mutation. |
| `speakers keep-separate-list` | no params | GET `/app/speakers/api/name-variants/keep-separate` (`solstone/apps/speakers/call.py:890`, `solstone/apps/speakers/routes.py:2979`). | No explicit route reason codes. | JSON pretty; read-only. |
| `speakers link-import` | `<name>` required, `--entity-id VALUE` required | POST `/app/speakers/api/link-import`; body-JSON `name`, `entity_id` (`solstone/apps/speakers/call.py:918`, `solstone/apps/speakers/routes.py:3002`). | Server: `speaker_command_failed` on domain error. | JSON pretty result; mutation. |
| `speakers merge-names` | `<alias>` required, `<canonical>` required | POST `/app/speakers/api/merge-names`; body-JSON `alias`, `canonical` (`solstone/apps/speakers/call.py:898`, `solstone/apps/speakers/routes.py:2986`). | Server: `speaker_command_failed` on domain error. | JSON pretty result; mutation. |
| `speakers owner-ready` | no params | POST `/app/speakers/api/owner/ready` (`solstone/apps/speakers/call.py:1285`, `solstone/apps/speakers/routes.py:3046`). | No explicit route reason codes. | JSON pretty result; read/cheap POST, no known state change. |
| `speakers presence` | `<cluster_id>` required, `--json` | GET `/app/speakers/api/discovery/cluster/{cluster_id}/presence`; path `cluster_id` (`solstone/apps/speakers/call.py:688`, `solstone/apps/speakers/routes.py:2376`). | Server: `speaker_review_unavailable`; CLI maps to `Cluster <id> was not found.` plus discover hint, exit 1. | Human evidence/candidates or JSON; read-only. |
| `speakers propagate-correction` | `<old_speaker>` required, `<new_speaker>` required, `--commit`, `--json` | POST `/app/speakers/api/propagate-correction`; body-JSON `old_speaker`, `new_speaker`, `commit` (`solstone/apps/speakers/call.py:430`, `solstone/apps/speakers/routes.py:1998`). | Server: `missing_required_field`, `invalid_request_value`, `speaker_not_found`, `entity_blocked`, `speaker_labels_busy`, `speaker_voiceprint_busy`. | Default preview banner; `--commit` persists and prints reversal command. |
| `speakers rebuild-owner` | `--override`, `--json` | POST `/app/speakers/api/owner/rebuild`; body-JSON `override` (`solstone/apps/speakers/call.py:1077`, `solstone/apps/speakers/routes.py:2242`). | Server: `speaker_voiceprint_busy`. | Human status/guidance or JSON; mutation only on `rebuilt`. |
| `speakers reject-owner` | no params | POST `/app/speakers/api/owner/reject-cli` (`solstone/apps/speakers/call.py:1277`, `solstone/apps/speakers/routes.py:3036`). | Server: `speaker_voiceprint_busy`. | JSON pretty result; mutation. |
| `speakers resolve-names` | `--commit`, `--json` | POST `/app/speakers/api/resolve-names`; body-JSON `commit` (`solstone/apps/speakers/call.py:228`, `solstone/apps/speakers/routes.py:2695`). | No explicit route reason codes. | Default preview banner; human stats or JSON; `--commit` persists. |
| `speakers seed-from-imports` | `--commit`, `--json` | POST `/app/speakers/api/seed-from-imports`; body-JSON `commit` (`solstone/apps/speakers/call.py:938`, `solstone/apps/speakers/routes.py:2703`). | Server: `speaker_owner_centroid_required`. | Default preview banner; human stats or JSON; `--commit` persists. |
| `speakers sentences` | `<day>` required, `<stream>` required, `<segment>` required, `<source>` required, `--json` | GET `/app/speakers/api/review-cli/{day}/{stream}/{segment}/{source}` (`solstone/apps/speakers/call.py:1180`, `solstone/apps/speakers/routes.py:1597`). | Server: `invalid_day`, `invalid_segment_or_stream`, `speaker_review_unavailable`. | Human sentence rows or JSON; read-only. |
| `speakers status` | optional `<section>` | GET `/app/speakers/api/status`; `section` selected locally (`solstone/apps/speakers/call.py:133`, `solstone/apps/speakers/routes.py:2675`). | Local unknown section returns JSON `{"error": "Unknown section '...'. Valid: ..."}` and exits 0. No route-specific reason codes. | JSON pretty; read-only. |
| `speakers suggest` | `--limit/-n INT` default 5, `--json` | GET `/app/speakers/api/suggest`; query `limit` (`solstone/apps/speakers/native/command.rs:791`, `solstone/apps/speakers/routes.py:2823`). | Server: `invalid_request_value` for invalid limit. | Markdown from server or complete server JSON body; read-only. |
| `speakers tag-owner` | `<day>` required, `<stream>` required, `<segment>` required, `<source>` required, `<sentence_id>` required, `--json` | POST `/app/speakers/api/owner/tag-cli`; body-JSON `day`, `stream`, `segment_key`, `source`, `sentence_id` (`solstone/apps/speakers/call.py:1134`, `solstone/apps/speakers/routes.py:2267`). | Server: `missing_request_body`, `speaker_owner_identity_required`, plus assign path `missing_required_field`, `invalid_day`, `invalid_segment_or_stream`, `speaker_sentence_missing`, `entity_blocked`, `speaker_not_found`, `speaker_owner_voice_too_close`, `speaker_voiceprint_busy`, `speaker_labels_busy`. CLI maps selected codes to stderr detail exit 1 (`solstone/apps/speakers/call.py:1157`). | Human tagged/already-assigned line or JSON; immediate mutation. |
| `speakers wipe` | `--commit`, `--json` | POST `/app/speakers/api/wipe`; body-JSON `commit` (`solstone/apps/speakers/call.py:598`, `solstone/apps/speakers/routes.py:2740`). | No explicit route reason codes. | Default preview banner; destructive mutation only with `--commit`; human counts or JSON. |

### thinking

Routes are owned by `solstone/apps/thinking/routes.py:91`. Commands are in
`solstone/apps/thinking/call.py:21`. The user direction listed nested
`keys/local/providers/scout`; the oracle also includes `thinking confidential`
with five leaves, and those are needed to reach the fixed count of 23.

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `thinking clear-local-endpoint` | no params | DELETE `/app/thinking/api/local/endpoint` (`solstone/apps/thinking/call.py:690`, `solstone/apps/thinking/routes.py:1184`). | Server: `invalid_operation_for_state`, `config_busy`, `settings_operation_failed`. | JSON result; mutation. |
| `thinking confidential disable` | no params | POST `/app/thinking/api/confidential/disable` (`solstone/apps/thinking/call.py:505`, `solstone/apps/thinking/routes.py:720`). | Server: `settings_operation_failed`. | JSON result; mutation. |
| `thinking confidential enable` | `--wait-seconds FLOAT` default 900.0, `--poll-interval FLOAT` default 1.0 | POST `/app/thinking/api/confidential/enable`, then repeated GET `/app/thinking/api/providers` until terminal (`solstone/apps/thinking/call.py:407`, `solstone/apps/thinking/routes.py:697`, `solstone/apps/thinking/routes.py:1235`). | Server: `invalid_operation_for_state`, `service_busy`, `settings_operation_failed`. Local timeout/swept-unconfigured paths print guidance and exit 1 (`solstone/apps/thinking/call.py:409`). | Human portal and terminal status; external browser consent via printed `continue in browser → ...`; mutation. |
| `thinking confidential recheck` | no params | POST `/app/thinking/api/confidential/recheck`, then GET `/app/thinking/api/providers` (`solstone/apps/thinking/call.py:475`, `solstone/apps/thinking/routes.py:740`). | Server: `invalid_operation_for_state`, `settings_operation_failed`. | JSON `{ok, attestation, error?}`; mutation/check request. |
| `thinking confidential status` | no params | GET `/app/thinking/api/providers`; CLI extracts active lane confidential state (`solstone/apps/thinking/call.py:352`, `solstone/apps/thinking/routes.py:1235`). | Server: `invalid_request_value` for unknown local model query, `settings_operation_failed`. CLI sends no local_model query. | JSON confidential subset; read-only. |
| `thinking keys clear` | `<env_var>` required | PUT `/app/thinking/api/keys`; body-JSON `env_var`, `value=""` (`solstone/apps/thinking/call.py:558`, `solstone/apps/thinking/routes.py:785`). | Local invalid env var: `Invalid env var: <env_var>. Must be one of: GOOGLE_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY`, exit 1 (`solstone/apps/thinking/call.py:154`). Server: `missing_request_body`, `invalid_config_value`, `invalid_request_value`, `config_busy`, `settings_operation_failed`. | JSON cleared status; mutation. |
| `thinking keys set` | `<env_var>` required, `<value>` required | PUT `/app/thinking/api/keys`; body-JSON `env_var`, `value` (`solstone/apps/thinking/call.py:529`, `solstone/apps/thinking/routes.py:785`). | Same env-var validation and server reason set. | JSON set/validation; mutation. |
| `thinking keys show` | no params | GET `/app/thinking/api/keys` (`solstone/apps/thinking/call.py:514`, `solstone/apps/thinking/routes.py:785`). | Server: `settings_operation_failed`. | JSON keys/env/key_validation; read-only. |
| `thinking keys validate` | `--cache-result` | GET `/app/thinking/api/validate-keys` by default; POST same route when caching (`solstone/apps/thinking/call.py:574`, `solstone/apps/thinking/routes.py:885`). | Server POST: `config_busy`, `settings_operation_failed`; GET: `settings_operation_failed`. | JSON key_validation. Default read-only; `--cache-result` mutates cache. |
| `thinking local availability` | `--model VALUE` | GET `/app/thinking/api/local/availability`; query `model` optional (`solstone/apps/thinking/call.py:715`, `solstone/apps/thinking/routes.py:985`). | Server: `invalid_request_value`, `settings_operation_failed`. | JSON availability; read-only. |
| `thinking local bootstrap` | `--model VALUE` | POST `/app/thinking/api/local/bootstrap`; query `model` optional (`solstone/apps/thinking/call.py:731`, `solstone/apps/thinking/routes.py:998`). | Server: `invalid_request_value`, `settings_operation_failed`. | JSON bootstrap payload; mutation/process start. |
| `thinking local bootstrap-status` | `--model VALUE` | GET `/app/thinking/api/local/bootstrap/status`; query `model` optional (`solstone/apps/thinking/call.py:747`, `solstone/apps/thinking/routes.py:1017`). | Server: `invalid_request_value`, `settings_operation_failed`. | JSON status; read-only. |
| `thinking local models` | no params | GET `/app/thinking/api/local/models` (`solstone/apps/thinking/call.py:763`, `solstone/apps/thinking/routes.py:1094`). | Server: `settings_operation_failed`. | JSON model list; read-only. |
| `thinking local readiness` | no params | GET `/app/thinking/api/providers/local/status` (`solstone/apps/thinking/call.py:699`, `solstone/apps/thinking/routes.py:1271`). | Server: `settings_operation_failed`. | JSON local provider status; read-only. |
| `thinking local status` | no params | GET `/app/thinking/api/providers/local/status` (`solstone/apps/thinking/call.py:707`, `solstone/apps/thinking/routes.py:1271`). | Server: `settings_operation_failed`. | JSON local provider status; read-only. |
| `thinking providers set-active` | `--provider VALUE` required, `--model VALUE` | Usually POST `/app/thinking/api/providers`; body-JSON `lane`, `provider`, `model`. If provider is `local`, CLI first GETs `/app/thinking/api/providers` to decide lane (`solstone/apps/thinking/call.py:622`, `solstone/apps/thinking/routes.py:1444`). | Local invalid provider: `Invalid provider: <provider>. Must be one of: anthropic, google, openai, local`; local `--model` with local provider: `--model is only valid for cloud providers.` (`solstone/apps/thinking/call.py:161`, `solstone/apps/thinking/call.py:628`). Server: `missing_request_body`, `missing_required_field`, `invalid_config_value`, `invalid_operation_for_state`, `config_busy`, `settings_operation_failed`. | Human `active: ...`; mutation. |
| `thinking providers show` | `--human` | GET `/app/thinking/api/providers` (`solstone/apps/thinking/call.py:588`, `solstone/apps/thinking/routes.py:1235`). | Server: `invalid_request_value`, `settings_operation_failed`. | Default JSON providers payload; `--human` prints active lane/provider statuses. |
| `thinking scout check` | no params | POST `/app/thinking/api/scout/check` (`solstone/apps/thinking/call.py:372`, `solstone/apps/thinking/routes.py:612`). | Server: `settings_operation_failed`. | JSON plus guidance; check request. |
| `thinking scout disable` | no params | POST `/app/thinking/api/scout/disable` (`solstone/apps/thinking/call.py:491`, `solstone/apps/thinking/routes.py:677`). | Server: `settings_operation_failed`. | JSON result/status; mutation. |
| `thinking scout enable` | `--wait-seconds FLOAT` default 900.0, `--poll-interval FLOAT` default 1.0 | POST `/app/thinking/api/scout/enable`, then repeated GET `/app/thinking/api/scout` until terminal (`solstone/apps/thinking/call.py:382`, `solstone/apps/thinking/routes.py:621`, `solstone/apps/thinking/routes.py:603`). | Server: `invalid_operation_for_state`, `service_busy`, `settings_operation_failed`. Local repair-needed exits 1. | Human portal/terminal status; external consent through approval URL; mutation. |
| `thinking scout refresh` | `--wait-seconds FLOAT` default 900.0, `--poll-interval FLOAT` default 1.0 | POST `/app/thinking/api/scout/refresh`, then repeated GET `/app/thinking/api/scout` (`solstone/apps/thinking/call.py:450`, `solstone/apps/thinking/routes.py:650`). | Server: `invalid_operation_for_state`, `service_busy`, `settings_operation_failed`. | Human terminal status; mutation. |
| `thinking scout status` | no params | GET `/app/thinking/api/scout` (`solstone/apps/thinking/call.py:342`, `solstone/apps/thinking/routes.py:603`). | Server: `settings_operation_failed`. | JSON plus guidance; read-only. |
| `thinking set-local-endpoint` | `--url VALUE` required, `--model VALUE` required, `--credential VALUE` | POST `/app/thinking/api/local/endpoint`; body-JSON `endpoint_url`, `served_model_id`, optional `credential` (`solstone/apps/thinking/call.py:663`, `solstone/apps/thinking/routes.py:1103`). | Server: `missing_request_body`, `missing_required_field`, `invalid_request_value`, `invalid_operation_for_state`, `config_busy`, `settings_operation_failed`. | JSON local endpoint; mutation. |

### transcripts

Routes are owned by `solstone/apps/transcripts/routes.py:107`. Commands are in
`solstone/apps/transcripts/call.py:119`. `scan`, `segments`, and `read` use
`SOL_DAY` with exact local error
`Error: day is required (pass as argument or set SOL_DAY).`
(`solstone/apps/transcripts/call.py:28`). `read` also uses `SOL_SEGMENT` and
`SOL_STREAM` defaults (`solstone/apps/transcripts/call.py:38`).

| Leaf | Argv grammar | Request | Reason-code / stderr / exit | Render / consent-dry-run |
|---|---|---|---|---|
| `transcripts read` | optional `<day>`, `--start`, `--length`, `--segment`, `--segments`, `--stream`, `--full`, `--raw`, `--transcripts`, hidden `--audio`, `--percepts`, hidden `--screen`, `--agents`, `--max` default 16384 | GET `/app/transcripts/api/read/{day}`; path `day`; query `transcripts`, `percepts`, `agents`; one selection mode among `start/end`, `segments`, or `segment`; optional `stream` (`solstone/apps/transcripts/call.py:238`, `solstone/apps/transcripts/routes.py:437`). Hidden aliases set `--audio` -> transcripts and `--screen` -> percepts (`solstone/apps/transcripts/call.py:264`). | Local conflicts: `Error: Cannot use --full and --raw together.`, `Error: Cannot mix --full/--raw with individual source flags.`, `Error: Cannot mix --segment, --segments, and --start/--length.`, exit 1 (`solstone/apps/transcripts/call.py:284`). Server: `invalid_day`, `invalid_segment_or_stream`. | Markdown output; client-side byte cap with stderr `[truncated: <bytes> bytes total, --max <max>]`; `--max 0` unlimited. |
| `transcripts scan` | optional `<day>` | GET `/app/transcripts/api/day/{day}`; path `day` (`solstone/apps/transcripts/call.py:119`, `solstone/apps/transcripts/routes.py:417`). | Local day env error above. Server: `invalid_day`. | Human transcript/percept ranges; read-only. |
| `transcripts segments` | optional `<day>` | GET `/app/transcripts/api/segments/{day}`; path `day` (`solstone/apps/transcripts/call.py:166`, `solstone/apps/transcripts/routes.py:402`). | Local day env error above. Server: `invalid_day`. | Human segment rows; empty prints `No segments.` |
| `transcripts speakers` | `<day>` required, `<stream>` required, `<segment>` required, `--json` | GET `/app/transcripts/api/segment/{day}/{stream}/{segment}`; path fields (`solstone/apps/transcripts/call.py:190`, `solstone/apps/transcripts/routes.py:878`). | Route reason codes from segment API include `invalid_day`, `invalid_segment_or_stream`, `file_not_found`, `file_read_failed` in the same module (`solstone/apps/transcripts/routes.py:878`). | Human speaker rows or JSON; read-only. |
| `transcripts stats` | `<month>` required | GET `/app/transcripts/api/stats/{month}`, then for each returned day GET `/app/transcripts/api/ranges/{day}` (`solstone/apps/transcripts/call.py:352`, `solstone/apps/transcripts/routes.py:486`, `solstone/apps/transcripts/routes.py:383`). | Stats route: `invalid_month`; range route: `invalid_day`. | Human per-day counts plus total; read-only; multi-request by returned days. |

## 2. ESCALATIONS: local-only (non-HTTP) leaves

One local-only leaf found:

| Leaf | Evidence | Escalation |
|---|---|---|
| `link observer-pause` | The command body only prints `observer-pause is not yet available.` and has no `get_client()` call (`solstone/apps/network/call.py:417`). | This has no Convey HTTP route today. Adding a server route is out of scope for this batch. |

All other 131 batch leaves invoke `get_client().request(...)`, a shared local
helper that does so, or `paginate_collection(get_client(), ...)`; the broad
source scan is consistent with the per-leaf rows above.

## 3. Applicability catalogs

### Upload leaves

None found in the 132-leaf batch. No batch command calls `ConveyClient.upload`;
the only upload-style native work remains outside this batch, for example the
lead support attachment route. Shared upload support lives at
`solstone/think/convey_client.py:151`.

### Multi-request leaves

| Leaf | Request sequence |
|---|---|
| `awareness log-read` | Repeated GET `/app/awareness/api/log` with `limit=100` and advancing `offset` until exhausted. |
| `entities aka` | GET facet resolve, then POST aka unless local alias skip applies. |
| `entities history` | Single HTTP request, but repeated-query `kinds`; pagination by `offset` is caller-controlled, not automatic. |
| `entities move` | GET source resolve, GET target resolve, POST move. |
| `entities observations` | GET resolve, then GET observations. |
| `entities observe` | GET resolve, then POST observe. |
| `entities update` | No-day path GET resolve then POST update-description; day path is one POST. |
| `ledger decisions` | `paginate_collection` can issue multiple GET `/api/ledger/decisions` pages (`solstone/think/convey_client.py:235`). |
| `ledger list` | `paginate_collection` can issue multiple GET `/api/ledger` pages. |
| `link pair` | POST pair-start, then unless `--no-wait`, repeated GET devices plus GET nonce-status. |
| `link private-link setup` | POST enable, then repeated GET private-link until terminal. |
| `link status` | GET status, GET private-link, GET devices. |
| `profile list-active` | `paginate_collection` can issue multiple GET `/api/profiles/active` pages. |
| `settings identity set` | GET config, then POST config. |
| `settings observer set` | GET observe, then POST observe. |
| `settings transcribe set-backend` | GET transcribe, then POST config. |
| `speakers confirm-owner` | POST owner confirm, then by default POST backfill with `commit=true`; `--no-backfill` makes it one request. |
| `thinking confidential enable` | POST confidential enable, then repeated GET providers. |
| `thinking confidential recheck` | POST confidential recheck, then GET providers. |
| `thinking providers set-active` | Provider `local` path GETs providers before POST providers; cloud paths are one POST. |
| `thinking scout enable` | POST scout enable, then repeated GET scout. |
| `thinking scout refresh` | POST scout refresh, then repeated GET scout. |
| `transcripts stats` | GET month stats, then GET ranges for each returned day. |

### Pagination leaves

| Leaf | Mechanism |
|---|---|
| `awareness log-read` | Client loop with query `limit=100`, `offset`; optional `--limit/-n` truncates locally to last N. |
| `entities history` | Server query `limit`, `offset`; caller controls offset. |
| `entities network` | Server query `limit` and `evidence_limit`; bounded result. |
| `entities overview` | Server query `limit`; bounded result. |
| `entities search` | Server query `limit`; invalid limit falls back to 20. |
| `ledger decisions` | Shared paginator injects `limit/offset`; optional `--top` client cap. |
| `ledger list` | Shared paginator injects `limit/offset`; optional `--top` client cap. |
| `profile list-active` | Shared paginator injects `limit/offset`; no CLI `--top`. |
| `speakers day-segments` | Server query `limit`; response includes returned/total. |
| `speakers suggest` | Server query `limit`; response is the complete server body with limit-bounded items. |
| `transcripts read` | Client-side output truncation by `--max` bytes; not server pagination. |

### Env-default leaves

| Leaf | Env default behavior |
|---|---|
| `entities accept-merge-candidate` | `--facet/-f` optional; missing falls back to `SOL_FACET`; if still missing, exact error above. |
| `entities aka` | Same `SOL_FACET` required resolver. |
| `entities attach` | Same `SOL_FACET` required resolver. |
| `entities detect` | `SOL_FACET` and `SOL_DAY` both required after option fallback. |
| `entities dismiss-merge-candidate` | Same `SOL_FACET` required resolver. |
| `entities list` | Positional facet or `--facet/-f` or `SOL_FACET`; required. `--day` omission is valid and does not use `SOL_DAY`. |
| `entities observations` | Same `SOL_FACET` required resolver. |
| `entities observe` | Same `SOL_FACET` required resolver; `--source-day` omission is valid and does not use `SOL_DAY`. |
| `entities record-merge-candidate` | `SOL_FACET` and `SOL_DAY` both required after option fallback. |
| `entities update` | `SOL_FACET` required; `--day` omission is valid and selects attached-entity update. |
| `transcripts read` | Positional day or `SOL_DAY` required; `SOL_SEGMENT` and `SOL_STREAM` are optional selection defaults. |
| `transcripts scan` | Positional day or `SOL_DAY` required. |
| `transcripts segments` | Positional day or `SOL_DAY` required. |

No other batch leaf uses `SOL_DAY` or `SOL_FACET`.

### Mutation leaves

| Leaves | Confirmation/dry-run posture |
|---|---|
| `awareness imports` when POST, `awareness log` | Immediate mutation; no confirmation or dry-run. |
| `chat start` | Creates a sol-initiated chat request; no confirmation or dry-run. |
| `entities attach`, `detect`, `dismiss-merge-candidate`, `move`, `observe`, `record-merge-candidate`, `resolve-ambiguity`, `restore-version`, `undo-merge`, `update` | Immediate mutation where applicable; `resolve-ambiguity`, `restore-version`, and `undo-merge` require `--yes`. `move --consent` is audit metadata only. |
| `entities accept-merge-candidate`, `entities merge` | Default dry-run/preview; `--commit` persists. |
| `facets accept`, `facets dismiss` | Immediate mutation. |
| `import resolve-config`, `resolve-config-all`, `resolve-entity`, `resolve-staged-facet` | Immediate mutation; `resolve-staged-facet` requires exactly one of `--apply`/`--skip`. |
| `ledger close` | Immediate mutation; no confirmation. |
| `link pair`, `private-link setup`, `private-link disable`, `unpair` | Pair/setup start service/link state; setup uses external approval flow; unpair/disable no prompt. |
| `settings identity set`, `keys set`, `keys clear`, `keys validate --cache-result`, `observer set`, `processing set`, `transcribe set-backend` | Mutate config/cache; no confirmation. |
| `sol reset`, `set-name`, `set-owner`, `sol-init` | Mutate identity/agent setup; no confirmation. |
| `speakers bootstrap`, `resolve-names`, `attribute-segment`, `propagate-correction`, `backfill`, `backfill-last-seen`, `wipe`, `seed-from-imports` | Default dry-run/preview; `--commit` persists. |
| `speakers correct`, `discover`, `dismiss-cluster`, `identify`, `identify-undo`, `merge-names`, `link-import`, `detect`, `build-from-tags`, `rebuild-owner`, `tag-owner`, `confirm-owner`, `reject-owner` | Mutating POSTs. `identify --resolve-only` dry-runs resolution. `confirm-owner --no-backfill` prevents the default follow-up backfill mutation. |
| `thinking keys set/clear`, `keys validate --cache-result`, `providers set-active`, `set-local-endpoint`, `clear-local-endpoint`, `local bootstrap`, `scout check/enable/disable/refresh`, `confidential enable/disable/recheck` | Mutating/config/process POST/PUT/DELETE; scout/confidential enable flows use external approval URLs. |

### Consent/dry-run leaves

| Leaf | Default direction |
|---|---|
| `entities accept-merge-candidate`, `entities merge` | Default preview (`--commit/--no-commit` default false). |
| `entities resolve-ambiguity`, `entities restore-version`, `entities undo-merge` | Refuse without `--yes`; no prompt. |
| `entities move` | `--consent` is optional audit assertion; no prompt. |
| `import resolve-staged-facet` | Requires explicit `--apply` or `--skip`. |
| `link private-link setup` | External approval URL may be printed; command polls until terminal. |
| `settings keys validate`, `thinking keys validate` | Default GET is read-only validation; `--cache-result` switches to POST/cache mutation. |
| `speakers bootstrap`, `resolve-names`, `attribute-segment`, `propagate-correction`, `backfill`, `backfill-last-seen`, `wipe`, `seed-from-imports` | Default report-only banner: `REPORT ONLY — pass --commit to persist.` |
| `speakers identify` | `--resolve-only` performs dry-run resolution. |
| `speakers confirm-owner` | Default `--backfill`; `--no-backfill` skips the follow-up POST. |
| `thinking scout enable`, `thinking confidential enable` | External approval URL and polling; no CLI prompt. |

## 4. Contract-fragment coverage catalog

Current `FRAGMENT_MODULES` includes network, observer, home, activities,
support, push, chat, health, root, voice, and import
(`solstone/convey/contract/assemble.py:18`).

Existing fragments overlapping this batch:

- `solstone.apps.network.contract` exists, but it covers only
  `POST /app/network/pair-start`, `POST /app/network/pair`, `POST /app/network/unpair`,
  `GET /app/network/local-endpoints`, and `GET /app/network/api/status`
  (`solstone/apps/network/contract.py:43`). Batch gaps: `GET /app/network/api/devices`,
  `GET /app/network/api/private-link`, `POST /app/network/private-link/enable`,
  `POST /app/network/private-link/disable`, `GET /app/network/api/pair/nonce-status`.
  `link observer-pause` has no HTTP route.
- `solstone.apps.import.contract` exists, but it covers ingest native routes
  `import.save`, `import.savePath`, `import.meta`, and `import.start`
  (`solstone/apps/import/contract.py:130`). Batch gaps: all five journal-source
  staging/resolve routes under `/app/import/api/journal-sources/{source}/...`.
- `solstone.convey.chat_contract` exists, but it does not cover
  `POST /api/chat/start`; existing operations are message/session/offer/draft
  operations (`solstone/convey/chat_contract.py:29`).

New fragments needed for the batch after checking the current tree:

`awareness`, `body`, `chat(group start)`, `entities`, `facets/curation`,
`ledger`, `profile`, `settings`, `sol`, `speakers`, `thinking`,
`transcripts`, plus fragment additions for existing `network` and `import`.

Route reason-code sets for the batch, grouped by owning module:

| Route set | Reason-code set |
|---|---|
| Awareness state/imports/log routes | `awareness_section_not_found`, `missing_request_body`, `invalid_json_request`, `invalid_request_value`, `missing_required_field`, `awareness_busy` (`solstone/apps/awareness/routes.py:65`). |
| Body routes | `invalid_day`, `invalid_request_value` (native body surface). |
| Chat start | `invalid_request_value` (`solstone/convey/chat.py:221`). |
| Entities edge routes | `missing_required_field`, `invalid_request_value`, `edge_index_unavailable` (`solstone/apps/entities/routes.py:405`). |
| Entities facet CRUD/call routes | `missing_request_body`, `missing_required_field`, `invalid_entity_type`, `entity_already_exists`, `entity_alias_conflict`, `entity_blocked`, `entity_not_found`, `entity_operation_failed`, `entity_busy`, `invalid_request_value` (`solstone/apps/entities/routes.py:482`). |
| Entities merge/history/ambiguity routes | `missing_required_field`, `entity_operation_failed`, `operation_no_longer_available`, `entity_not_found`, `entity_blocked`, `invalid_request_value`, `entity_busy` (`solstone/apps/entities/routes.py:963`). |
| Facets curation routes | `missing_required_field`, `entity_busy`; domain `status=error` may return 400 without reason envelope (`solstone/apps/curation/routes.py:109`). |
| Import journal-source routes | `journal_source_problem`, `import_not_found`, `invalid_request_value` (`solstone/apps/import/routes.py:1386`). |
| Ledger routes | `invalid_request_value`, `invalid_day`, `missing_request_body`, `invalid_json_request`, `missing_required_field`, `ledger_item_not_found`, `activities_busy` (`solstone/convey/ledger.py:76`). |
| Link routes | `pairing_request_invalid`, `invalid_operation_for_state`, `pairing_relay_unavailable`, `service_operation_failed`, `service_busy`, `missing_required_field`, `paired_device_not_found` (`solstone/apps/network/routes.py:590`). |
| Profile routes | `entity_not_found`, `invalid_request_value` (`solstone/convey/profile.py:44`). |
| Settings routes | `missing_request_body`, `missing_required_field`, `invalid_config_value`, `config_busy`, `settings_operation_failed` (`solstone/apps/settings/routes.py:293`). |
| Sol routes | `missing_request_body`, `invalid_json_request`, `missing_required_field`, `identity_busy` (`solstone/apps/sol/routes.py:775`). |
| Speakers read/review routes | `invalid_month`, `invalid_day`, `invalid_request_value`, `invalid_segment_or_stream`, `speaker_review_unavailable`, `speaker_sentence_missing` (`solstone/apps/speakers/routes.py:1211`). |
| Speakers attribution/owner/discovery mutation routes | `missing_request_body`, `missing_required_field`, `invalid_day`, `invalid_request_value`, `invalid_segment_or_stream`, `invalid_entity_type`, `entity_blocked`, `entity_not_found`, `speaker_owner_voice_too_close`, `speaker_review_unavailable`, `speaker_sentence_missing`, `speaker_attribution_state_invalid`, `speaker_not_found`, `speaker_owner_identity_required`, `speaker_voiceprint_busy`, `speaker_labels_busy`, `speaker_owner_centroid_required`, `speaker_command_failed`, `speaker_identify_recoverable`, `speaker_identify_repair_required`, `speaker_identify_conflict`, `speaker_identify_operation_not_found` (`solstone/apps/speakers/routes.py:1653`, `solstone/apps/speakers/routes.py:2526`). |
| Thinking routes | `missing_request_body`, `missing_required_field`, `invalid_config_value`, `invalid_request_value`, `invalid_operation_for_state`, `service_busy`, `config_busy`, `settings_operation_failed` (`solstone/apps/thinking/routes.py:755`, `solstone/apps/thinking/routes.py:985`, `solstone/apps/thinking/routes.py:1444`). |
| Transcripts routes | `invalid_day`, `invalid_month`, `invalid_segment_or_stream`, `file_not_found`, `file_read_failed`, `operation_no_longer_available`, `invalid_operation_for_state` within the module; CLI leaves use the read/day/segments/stats/segment subset (`solstone/apps/transcripts/routes.py:383`). |

Reason-code definitions are in `solstone/convey/reasons.py:49`,
`solstone/convey/reasons.py:185`, `solstone/convey/reasons.py:223`,
`solstone/convey/reasons.py:321`, `solstone/convey/reasons.py:364`, and
`solstone/convey/reasons.py:456`.

## 5. Flask blueprint registration for the gate collectors

Current conformance route collection registers activities/support/health/chat/root
only (`scripts/check_native_sol_conformance.py:383`). Current contract-route
collector registers activities/support/health only
(`scripts/check_native_sol_contract_routes.py:55`).

Additional registrations needed to cover this batch:

| Area | Import | Blueprint symbol |
|---|---|---|
| awareness | `from solstone.apps.awareness.routes import awareness_bp` | `awareness_bp` (`solstone/apps/awareness/routes.py:44`) |
| body | retired Python blueprint | retired Python blueprint |
| facets | `from solstone.apps.curation.routes import curation_bp` | `curation_bp` (`solstone/apps/curation/routes.py:47`) |
| entities | `from solstone.apps.entities.routes import entities_bp` | `entities_bp` (`solstone/apps/entities/routes.py:124`) |
| import | `from solstone.apps.import.routes import import_bp` | `import_bp` (`solstone/apps/import/routes.py:80`) |
| link | `from solstone.apps.network.routes import network_bp` | `network_bp` (`solstone/apps/network/routes.py:128`); register normal `/app/network` prefix for CLI routes; `/app/link` alias is a separate legacy production registration. |
| settings | `from solstone.apps.settings.routes import settings_bp` | `settings_bp` (`solstone/apps/settings/routes.py:86`) |
| sol | `from solstone.apps.sol.routes import sol_bp` | `sol_bp` (`solstone/apps/sol/routes.py:55`) |
| speakers | native convey-shell route surface | native speakers routes |
| thinking | `from solstone.apps.thinking.routes import thinking_bp` | `thinking_bp` (`solstone/apps/thinking/routes.py:91`) |
| transcripts | `from solstone.apps.transcripts.routes import transcripts_bp` | `transcripts_bp` (`solstone/apps/transcripts/routes.py:107`) |
| ledger | `from solstone.convey.ledger import bp as ledger_bp` | `ledger_bp` (`solstone/convey/ledger.py:39`) |
| profile full/brief/cadence | `from solstone.convey.profile import bp as profile_bp` | `profile_bp` (`solstone/convey/profile.py:27`) |
| profile list-active | `from solstone.convey.profile import profiles_bp` | `profiles_bp` (`solstone/convey/profile.py:28`) |

Already-present registrations that still matter:

- `from solstone.convey.chat import chat_bp` covers `POST /api/chat/start`
  (`solstone/convey/chat.py:80`, `solstone/convey/chat.py:221`).
- `from solstone.convey.health import bp as health_bp` remains needed for the
  lead health leaves (`solstone/convey/health.py:31`).
- `from solstone.convey.root import bp as root_bp` remains needed for top-level
  chat/root lead coverage (`solstone/convey/root.py:68`).

## 6. Grammar detail extraction (from the oracle)

The following compact enumeration was generated directly from
`core/fixtures/native-sol/sol-call-grammar-v1.json:1` by reading each entry's
`path`, `kind`, `help`, and `params` fields. Hidden aliases/flags are included.

### awareness

- `awareness imports`: `--record/-r`, `--declined` flag, `--nudge` flag.
- `awareness log`: `<kind>` required, `<message>`, `--key/-k`, `--data/-d`.
- `awareness log-read`: `<day>`, `--kind/-k`, `--limit/-n`.
- `awareness status`: `<section>`.

### body

- `body day`: `<day_value>` required, `--json` flag.
- `body status`: `--json` flag.
- `body window`: `--from` required, `--to` required, `--json` flag.

### chat

- `chat start`: `--summary` required, `--message`, `--category` required,
  `--dedupe` required, `--dedupe-window`, `--since-ts` required,
  `--trigger-talent` required.

### entities

- `entities accept-merge-candidate`: `<source_slug>` required, `<target_slug>` required, `--facet/-f`, `--commit/--no-commit` flag.
- `entities aka`: `<entity>` required, `<aka_value>` required, `--facet/-f`.
- `entities ambiguities`: `--status`, `--json` flag.
- `entities attach`: `<type>` required, `<entity>` required, `<description>` required, `--facet/-f`.
- `entities detect`: `<type>` required, `<entity>` required, `<description>` required, `--facet/-f`, `--day/-d`.
- `entities dismiss-merge-candidate`: `<source_slug>` required, `<target_slug>` required, `--facet/-f`.
- `entities entity-history`: `<entity_id>` required, `--json` flag.
- `entities history`: `<entity>` required, `<peer>`, `--kinds` multiple default `[]`, `--facet/-f`, `--day-from`, `--day-to`, `--limit/-n` default `50`, `--offset`, `--json` flag.
- `entities list`: `<facet>`, `--facet/-f`, `--day/-d`.
- `entities merge`: `<source_slug>` required, `<target_slug>` required, `--commit/--no-commit` flag, `--keep-source-as-aka/--no-keep-source-as-aka` flag default `true`.
- `entities merge-candidates`: `--facet/-f`, `--status`, `--json` flag.
- `entities move`: `<entity>` required, `--from` required, `--to` required, `--merge` flag, `--consent` flag.
- `entities network`: `<entity>` required, `--kinds` multiple default `[]`, `--facet/-f`, `--day-from`, `--day-to`, `--limit/-n` default `25`, `--evidence-limit` default `5`, `--include-principal` flag, `--json` flag.
- `entities observations`: `<entity>` required, `--facet/-f`.
- `entities observe`: `<entity>` required, `<content>` required, `--facet/-f`, `--source-day`.
- `entities overview`: `--kinds` multiple default `[]`, `--facet/-f`, `--day-from`, `--day-to`, `--limit/-n` default `25`, `--json` flag.
- `entities record-merge-candidate`: `<source>` required, `<target>` required, `--facet/-f`, `--day/-d`, `--evidence` required, `--basis` default `name-variant`, `--detections`, `--needs`, `--json` flag.
- `entities resolve-ambiguity`: `<ambiguity_id>` required, `<entity_id>` required, `--yes` flag, `--json` flag.
- `entities restore-version`: `<entity_id>` required, `<version_id>` required, `--yes` flag, `--json` flag.
- `entities search`: `<query_pos>`, `--query/-q`, `--type/-t`, `--facet/-f`, `--since`, `--limit/-n` default `20`.
- `entities undo-merge`: `<merge_id>` required, `--yes` flag, `--json` flag.
- `entities update`: `<entity>` required, `<description>` required, `--facet/-f`, `--day/-d`.

Unusual grammar: repeated `--kinds`; secondary flag names for merge/accept and
keep-source-as-aka.

### facets

- `facets accept`: `<name_key>` required.
- `facets dismiss`: `<name_key>` required.
- `facets list-candidates`: `--status`, `--json` flag.

### import

- `import list-staged`: `--source` required, `--area`.
- `import resolve-config`: `<field>` required, `<action>` required, `--source` required.
- `import resolve-config-all`: `--source` required, `--category` required.
- `import resolve-entity`: `<source_id>` required, `<action>` required, `--source` required, `--target`.
- `import resolve-staged-facet`: `<staged_file>` required, `--apply` flag, `--skip` flag, `--source` required.

Unusual grammar: mutually exclusive `--apply` and `--skip` is enforced locally,
not encoded as a Typer mutual-exclusion group.

### ledger

- `ledger close`: `<item_id>` required, `--note` required, `--as` default `closed`, `--json` flag.
- `ledger decisions`: `--owner`, `--since`, `--involving`, `--top`, `--facets`, `--json` flag.
- `ledger get`: `<item_id>` required, `--json` flag.
- `ledger list`: `--state` default `open`, `--owner`, `--counterparty`, `--age-days-gte`, `--closed-since`, `--top`, `--sort`, `--facets`, `--json` flag.

### link

- `link authorized-clients`: no params.
- `link list`: no params.
- `link observer-pause`: no params.
- `link pair`: `--device-label`, `--as`, `--timeout` default `300`, `--no-wait` flag.
- `link private-link disable`: no params.
- `link private-link setup`: `--wait-seconds` default `900.0`, `--poll-interval` default `1.0`.
- `link private-link status`: no params.
- `link status`: no params.
- `link unpair`: `<target>` required.

Unusual grammar: nested `private-link` subgroup; `observer-pause` is in the
oracle but is local-only today.

### profile

- `profile brief`: `<name>` required, `--json` flag.
- `profile cadence`: `<name>` required, `--include-mentions` flag, `--json` flag.
- `profile full`: `<name>` required, `--facets`, `--include-mentions` flag, `--json` flag.
- `profile list-active`: `--window-days` default `30`, `--json` flag.

### settings

- `settings convey status`: no params.
- `settings identity set`: `--name`, `--preferred`, `--bio`, `--timezone`, `--pronouns`, `--add-email`, `--remove-email`, `--add-alias`, `--remove-alias`.
- `settings identity show`: no params.
- `settings keys clear`: `<env_var>` required.
- `settings keys set`: `<env_var>` required, `<value>` required.
- `settings keys show`: no params.
- `settings keys validate`: `--cache-result` flag.
- `settings observer set`: `--enabled/--no-enabled` flag, `--capture-interval`.
- `settings observer show`: no params.
- `settings processing set`: `--mode`, `--window-start`, `--window-end`, `--time-window/--no-time-window` flag, `--display-powersave/--no-display-powersave` flag.
- `settings processing show`: no params.
- `settings show`: no params.
- `settings transcribe set-backend`: `<backend>` required.
- `settings transcribe show`: no params.

Unusual grammar: nested subgroups `convey`, `identity`, `keys`, `observer`,
`processing`, `transcribe`; secondary negative flags on `observer set` and
`processing set`.

### sol

- `sol reset`: no params.
- `sol set-name`: `<name>` required, `--status/-s` default `chosen`.
- `sol set-owner`: `<name>` required, `--bio/-b`.
- `sol sol-init`: no params.

### speakers

- `speakers attribute-segment`: `<day>` required, `<stream>` required, `<segment>` required, `--commit` flag, `--save/--no-save` flag default `true`, `--accumulate/--no-accumulate` flag default `true`, `--json` flag.
- `speakers backfill`: `--commit` flag, `--reattribute` flag, `--json` flag.
- `speakers backfill-last-seen`: `--commit` flag, `--json` flag.
- `speakers bootstrap`: `--commit` flag, `--json` flag.
- `speakers build-from-tags`: `--json` flag.
- `speakers confirm-owner`: `--backfill/--no-backfill` flag default `true`, `--json` flag.
- `speakers correct`: `<day>` required, `<stream>` required, `<segment>` required, `<source>` required, `<sentence_id>` required, `<new_speaker>` required, `--json` flag.
- `speakers day-segments`: `<day>` required, `--limit/-n` default `20`, `--json` flag.
- `speakers detect`: `--force` flag.
- `speakers discover`: `--json` flag.
- `speakers dismiss-cluster`: `<cluster_id>` required, `--disposition` required.
- `speakers dismissals`: no params.
- `speakers identify`: `<cluster_id>` required, `<name>`, `--entity-id`, `--create` flag, `--entity-type` default `Person`, `--resolve-only` flag, `--request-id`, `--reviewed-near-match-entity-id` multiple.
- `speakers identify-operation`: `<operation_id>` required.
- `speakers identify-operations`: no params.
- `speakers identify-undo`: `<operation_id>` required.
- `speakers keep-separate-list`: no params.
- `speakers link-import`: `<name>` required, `--entity-id` required.
- `speakers merge-names`: `<alias>` required, `<canonical>` required.
- `speakers owner-ready`: no params.
- `speakers presence`: `<cluster_id>` required, `--json` flag.
- `speakers propagate-correction`: `<old_speaker>` required, `<new_speaker>` required, `--commit` flag, `--json` flag.
- `speakers rebuild-owner`: `--override` flag, `--json` flag.
- `speakers reject-owner`: no params.
- `speakers resolve-names`: `--commit` flag, `--json` flag.
- `speakers seed-from-imports`: `--commit` flag, `--json` flag.
- `speakers sentences`: `<day>` required, `<stream>` required, `<segment>` required, `<source>` required, `--json` flag.
- `speakers status`: `<section>`.
- `speakers suggest`: `--limit/-n` default `5`, `--json` flag.
- `speakers tag-owner`: `<day>` required, `<stream>` required, `<segment>` required, `<source>` required, `<sentence_id>` required, `--json` flag.
- `speakers wipe`: `--commit` flag, `--json` flag.

Unusual grammar: many dry-run-by-default `--commit` flags; secondary
`--save/--no-save`, `--accumulate/--no-accumulate`, and
`--backfill/--no-backfill`; repeated `--reviewed-near-match-entity-id`.

### thinking

- `thinking clear-local-endpoint`: no params.
- `thinking confidential disable`: no params.
- `thinking confidential enable`: `--wait-seconds` default `900.0`, `--poll-interval` default `1.0`.
- `thinking confidential recheck`: no params.
- `thinking confidential status`: no params.
- `thinking keys clear`: `<env_var>` required.
- `thinking keys set`: `<env_var>` required, `<value>` required.
- `thinking keys show`: no params.
- `thinking keys validate`: `--cache-result` flag.
- `thinking local availability`: `--model`.
- `thinking local bootstrap`: `--model`.
- `thinking local bootstrap-status`: `--model`.
- `thinking local models`: no params.
- `thinking local readiness`: no params.
- `thinking local status`: no params.
- `thinking providers set-active`: `--provider` required, `--model`.
- `thinking providers show`: `--human` flag.
- `thinking scout check`: no params.
- `thinking scout disable`: no params.
- `thinking scout enable`: `--wait-seconds` default `900.0`, `--poll-interval` default `1.0`.
- `thinking scout refresh`: `--wait-seconds` default `900.0`, `--poll-interval` default `1.0`.
- `thinking scout status`: no params.
- `thinking set-local-endpoint`: `--url` required, `--model` required, `--credential`.

Unusual grammar: nested `confidential`, `keys`, `local`, `providers`, and
`scout` subgroups; long-poll options share float defaults across Scout and
confidential setup flows.

### transcripts

- `transcripts read`: `<day>`, `--start`, `--length`, `--segment`, `--segments`, `--stream`, `--full` flag, `--raw` flag, `--transcripts` flag, hidden `--audio` flag, `--percepts` flag, hidden `--screen` flag, `--agents` flag, `--max` default `16384`.
- `transcripts scan`: `<day>`.
- `transcripts segments`: `<day>`.
- `transcripts speakers`: `<day>` required, `<stream>` required, `<segment>` required, `--json` flag.
- `transcripts stats`: `<month>` required.

Unusual grammar: hidden compat aliases `read --audio` and `read --screen` are
present in the oracle and command code (`solstone/apps/transcripts/call.py:264`).

## 7. Clean test baseline (read-only, untouched tree)

All requested baseline commands passed on this tree.

| Command | Result | One-line tail |
|---|---|---|
| `make check-rust-test` | pass, exit 0 | `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` |
| `make check-native-sol-inventory` | pass, exit 0 | `core/crates/solstone-core-sol-client/src/generated/inventory.rs is current` |
| `make check-native-sol-conformance` | pass, exit 0 | `native sol conformance ok` |
| `make check-openapi` | pass, exit 0 | `observer-client-contract: pass for docs/openapi/observer-client-contract` |
| `make test-only TEST=tests/native_sol/` | pass, exit 0 | `104 passed in 2.02s` |

Post-baseline git status shows only this new doc as untracked; no tracked
product, oracle, or generated files changed.
