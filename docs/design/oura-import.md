# Oura API Lane — Design

> OAuth/sync/rebuild shipped in `solstone-core-body-ingest`. File-import save and webhooks are still open. Python paths in this file are dead.


- **Date:** 2026-07-05 (overnight lane, owner-authorized)
- **Repo:** `solstone`, branch `health-imports-phase1`
- **Companion skeleton:** `solstone/think/importers/oura.py` + `tests/test_oura_importer.py` + synthetic fixtures under `tests/fixtures/importers/health/oura_synthetic/` (landed with this doc; see §9)
- **Hard rules honored:** no network code anywhere (a test greps the module for network-capable imports), no OAuth against real Oura, no live-journal writes, no credentials or token files, synthetic fixtures only. The first live OAuth authorization is **OWNER-PRESENT-ONLY** (§8, phase O2).
- **Medical claims:** Oura's numbers render as attributed facts — "Readiness 82 · Oura's score" — never our gloss, never medical interpretation.

> **Historical design record.** Sections 1–10 preserve the July 2026 product
> and data-shape decisions; they no longer define production code ownership or
> the current runbook. See [`../health_imports.md`](../health_imports.md) for
> the current boundary.

Status note, 2026-08-09: Oura connect, HTTPS transport, token refresh,
pagination, backfill, cursor state, normalization, native-bundle publication,
dedupe replay, and restore rebuild are now Rust-owned in
`solstone-core-body-ingest` and the body-source/store/rebuild crates. Python's
`oura.py` is a read-only differential oracle and `body_native.py` is process
transport. The Python sync registry, OAuth/token code, network client, cursor
writer, bundle writer, and dedupe writer are gone. The owner commands remain
`journal importer --connect oura`, `journal importer --sync oura`, and
`journal importer --sync oura --save --confirm-body-save`. Oura file-import
save and webhooks remain deferred.

The native sync has aggregate run budgets in addition to its per-response
limit: 128 MiB of response bodies, 5,000 pages, and 100,000 source items. A
save may rotate an expired refresh token because it persists the replacement
before retrying; catalog mode refuses that 401 rather than invalidate the
stored grant. Retained API pages are covered by a canonical raw inventory and
are verified during restore rebuild.

---

## 1. Where this fits in the existing architecture

The original stack recorded below is historical. Its current ownership is:

| Piece | File | Role |
|---|---|---|
| Native body ingress | `core/crates/solstone-core-body-ingest/` | Oura OAuth, API sync, cursor, normalization, immutable native-bundle publication |
| Native body contracts | `core/crates/solstone-core-body-source/` | Row/hash, manifest, envelope, ledger, and validation contracts |
| Native body store | `core/crates/solstone-core-body-store/` + `solstone-core-body-rebuild/` | Ordered replay and atomic `imports/health-dedupe.sqlite` reconstruction |
| Python Oura reader | `solstone/think/importers/oura.py` | Preview and independent parse/normalization differential oracle only |
| Process adapter | `solstone/think/body_native.py` | Version-matched native helper dispatch and result validation only |
| Native body surface | `core/crates/solstone-core-convey-shell/` | Read-only archive and day presentation over normalized shards and the dedupe DB |

Oura is the second body source family. Import preserves its source attribution;
presentation may reconcile overlap with Apple Health later.

---

## 2. (a) What the Oura API v2 adds beyond the Apple Health mirror

The owner's ring already reaches the journal indirectly: the Oura app mirrors some series into Apple Health, and those arrive with `source_family="apple_health"` and a ring `source_name`. What the mirror carries (empirically: sleep stages and some vitals) versus what it **cannot** carry (Oura's computed scores and contributors) is the reason this lane exists.

### Endpoints worth importing (Oura API v2, `https://api.ouraring.com/v2/usercollection/...`)

Endpoint names below are from model knowledge of the v2 API; **verify each against live docs at phase O2 when network work is authorized**. Confidence flags: ✅ high, ◑ moderate, ⚠ verify.

| Endpoint | Payload (key fields) | Beyond the AH mirror? | Confidence |
|---|---|---|---|
| `daily_sleep` | `score`, `contributors` (deep_sleep, efficiency, latency, rem_sleep, restfulness, timing, total_sleep), `day` | **Yes** — the sleep score and its contributor breakdown never cross into HealthKit | ✅ |
| `daily_readiness` | `score`, `contributors` (activity_balance, body_temperature, hrv_balance, previous_day_activity, previous_night, recovery_index, resting_heart_rate, sleep_balance), `temperature_deviation`, `temperature_trend_deviation` | **Yes** — readiness score and °C temperature deviation are Oura-only | ✅ |
| `daily_resilience` | `level` (limited/adequate/solid/strong/exceptional), `contributors` (sleep_recovery, daytime_recovery, stress) | **Yes** — resilience is Oura-only; endpoint added ~mid-2024 | ◑ (field names ⚠) |
| `daily_stress` | `stress_high` (s), `recovery_high` (s), `day_summary` (restored/normal/stressful) | **Yes** — daytime stress minutes are Oura-only | ◑ |
| `daily_spo2` | `spo2_percentage.average`, `breathing_disturbance_index` | **Yes** — nightly SpO2 average + BDI; AH mirror may carry raw SpO2 samples but not Oura's nightly average/BDI | ◑ |
| `sleep` | per-period: `bedtime_start/end`, `type` (long_sleep/late_nap/…), stage durations (`deep_sleep_duration`, `rem_sleep_duration`, `light_sleep_duration`, `awake_time`), `efficiency`, `latency`, `average_heart_rate`, `lowest_heart_rate`, `average_hrv`, `average_breath`, `sleep_phase_5_min` hypnogram (1=deep, 2=light, 3=REM, 4=awake) | **Partly** — stages also mirror into AH as `HKCategoryValueSleepAnalysis*` intervals, but Oura-native durations, efficiency/latency, per-period HRV/HR aggregates, and the 5-minute hypnogram string are richer and carry Oura's own period identity (`id`, `day` attribution) | ✅ |
| `heartrate`, `daily_activity`, `workout`, `session`, `enhanced_tag`, `sleep_time`, `ring_configuration`, `vO2max` / `daily_cardiovascular_age` | series + activity + tags + device | Mostly **duplicates the AH mirror** (HR, steps, workouts) or is metadata; excluded from the first import scope to avoid double-counting in presentation | ⚠ names |

Other API facts to verify live at O2: OAuth2 endpoints (`cloud.ouraring.com/oauth/authorize`, `api.ouraring.com/oauth/token` ⚠), scopes (`daily heartrate workout tag session spo2 stress heart_health metabolic`), rate limit (historically 5000 requests / 5 min ⚠), the no-auth sandbox (`/v2/sandbox/usercollection/*` ⚠), personal-access-token deprecation status ⚠, webhook subscription API ⚠.

**Original skeleton scope:** `daily_sleep`, `daily_readiness` (+ split-out `temperature_deviation` rows), `daily_resilience`, `daily_stress`, `daily_spo2`, `sleep`. The shipped sync scope is broader; see §5 for the current OAuth scope set and §9a for the later granted-scope endpoint additions.

### Do we still need the pending Oura export?

**Keep the request open, but nothing waits on it.**

- The API serves full account history for every endpoint above (paged by `start_date`/`end_date` + `next_token`), so backfill does not need the export.
- The export still earns its keep as: (1) an offline raw archive independent of API availability and dev-app approval; (2) possibly the only carrier of legacy/older-generation or full-resolution data the v2 API doesn't expose (⚠ unknown until inspected); (3) a zero-network import path that could run under today's gate before OAuth is ever authorized.
- When it arrives: inspect read-only; if its shape matches API documents, the existing parse layer covers it; if not, it gets its own parser under the **reserved** `SOURCE_OURA = "oura"` family. Records from export and API deliberately do **not** collapse at import (per `docs/health_imports.md` — cross-source reconciliation happens at query/presentation time, and document `id`s should match if the export carries them).

---

## 3. (b) Presentation — day pages, new card, overview, window API

Oura values render as attributed facts with no medical interpretation. The native body surface presents them.

### Presentation rules

| Do | Don't |
|---|---|
| `Readiness 82 · Oura's score` | "You're well recovered" |
| `Sleep score 88 · Oura's score` | "Great sleep!" |
| `Resilience solid · Oura's level` | "Your resilience is strong — keep it up" |
| `Deep 1h 31m · REM 1h 48m · Light 4h 00m · Awake 32m — Oura's staging` | Any re-derived or re-weighted stage math presented as ours |
| `Temperature deviation +0.34 °C · Oura's measurement` | "Possible fever" / any clinical reading |
| `Day stress summary normal · Oura's label` | Color-coding stress as good/bad beyond quoting Oura's own label, attributed |

If Oura's own qualitative band is shown (e.g., "optimal"), it renders quoted and attributed ("Oura calls this optimal"), never as our judgment.

### Existing cards that absorb Oura data

- **Sleep card (day page).** `oura.sleep` rows join the day's sleep interval pool. The primary-source rule in `health_schema.pick_day_sleep` is unchanged — Oura becomes one more source in `intervals_by_source`, and when it wins coverage the card renders Oura's stage breakdown from period metadata (`deep/rem/light/awake` durations) instead of interval-derived math, labeled "— Oura's staging". The day's `oura.daily_sleep` score renders as one attributed line on the same card: `Sleep score 88 · Oura's score`. Oura's `day` attribution (night belongs to the day it ended) already matches the journal's cross-midnight canon, so no re-attribution.
- **Coverage families (`_FAMILY_RULES` in `body/routes.py`).** Fragment additions:
  - `Sleep` gains `("oura.daily_sleep", "oura.sleep")`
  - `Heart` gains `("oura.daily_spo2",)` (consistent with `OxygenSaturation` living in Heart)
  - New family **`Recovery`** (ordered after Glucose, before Activity in `_FAMILY_ORDER`) claims `("oura.daily_readiness", "oura.daily_resilience", "oura.daily_stress", "oura.temperature_deviation")`
- **Sources / audit surfaces.** Source label for `source_family="oura_api"` renders as "Oura API". The archive day-grid needs no change — Oura rows enter `health-dedupe.sqlite` and count per day like any other family.
- **Friendly names** already landed in `health_schema.FRIENDLY_TYPE_NAMES` (`oura.daily_readiness` → "Readiness", etc.), so any generic signal list renders cleanly today.

### New day-page card: "How recovered am I?"

Renders only when the day has at least one of readiness / resilience / stress / temperature-deviation / SpO2 rows (same "cards appear only with data" rule as the rest of the day page).

```
How recovered am I?
Readiness 82 · Oura's score
Resilience solid · Oura's level
Temperature deviation −0.21 °C · Oura's measurement
Nightly blood oxygen 97.4% · Oura's average (breathing disturbance index 3)
Daytime stress 2h 0m high · 5h 40m recovery · Oura's day summary: normal
▸ Oura's contributors  (disclosure: raw contributor numbers, verbatim, attributed)
```

The skeleton's `render_day_summary()` in `oura.py` is the copy reference implementation — the card and the (optional, later) `import.oura` day-summary transcript must agree with it line-for-line in register.

- **Day lede** gains at most one clause when present: `…, readiness 82 (Oura's score)`.
- **Day prompts** may gain: "What did my day look like around the readiness dip on {date}?" — phrased as a question about the journal, never advice.

### Overview / coverage additions

- Archive coverage chips pick up the `Recovery` family automatically once `_FAMILY_RULES` lands.
- The overview's sources snapshot lists "Oura API" with last-seen day (from dedupe rows), marking staleness with the existing `STALE_SOURCE_DAYS` rule — factual ("last brought in N days ago"), not alarming.

### Window API additions (`/api/window`)

Oura's daily documents are day-granularity; they don't belong inside intra-day windows as samples. Two additions:

1. `events`: `oura.sleep` periods are true intervals (`bedtime_start`/`bedtime_end`) — include them in the window's events list like workouts (they already fit `_row_interval`).
2. New `day_context` block: for each calendar day the window overlaps, the day's Oura score rows as attributed facts — `{"day": "20260102", "facts": ["Readiness 82 · Oura's score", ...]}`. Windows never interpolate a daily score across hours.

---

## 4. (c) Storage

Mirrors `apple_health` exactly; all writes live under `imports/**` plus (optionally, save phase) declared day-summary transcript files — L7-clean.

```
imports/<import_id>/
  raw/oura/<endpoint>.jsonl            # verbatim API pages when raw_retention=retain_parsed
  normalized/<YYYY-MM>.jsonl           # monthly shards, schema solstone.health.oura.v1
  manifest.json                        # shared.write_manifest, source_type "oura"
  content_manifest.jsonl               # shared.write_content_manifest
  fetch_windows.json                   # fetched window evidence for the chunker
imports/health-dedupe.sqlite           # shared dedupe DB (existing)
imports/oura.json                      # sync cursor (phase O3; never tokens)
chronicle/<day>/import.oura/000000_300/day_summary_transcript.md   # optional, save phase
```

Importer-owned files under `imports/` are private (`0600`) and importer-owned directories under `imports/` are created or repaired as `0700`. Oura API sync applies the validated `raw_retention.decision`: `retain_parsed` keeps raw API pages, while `discard` writes no raw page JSONL and stores no new `raw_ref` values.

**Normalized row** (implemented in the skeleton):

```json
{
  "schema": "solstone.health.oura.v1",
  "source_family": "oura_api",
  "kind": "daily_summary" | "sleep_period",
  "record_type": "oura.daily_readiness" | "oura.daily_sleep" | "oura.daily_resilience"
               | "oura.daily_stress" | "oura.daily_spo2" | "oura.temperature_deviation"
               | "oura.sleep",
  "dedupe_key": "sha256:…",
  "day": "20260102",
  "start_date": "...", "end_date": "... (sleep periods only)",
  "source_record_id": "<oura document id>",
  "value": 82, "unit": "score|degC|%|s|null",
  "metadata": { "contributors": {...}, "stage durations": "…", "…": "…" },
  "raw_ref": "imports/<id>/raw/oura#<endpoint>-<n>"
}
```

**Source family: `oura_api`** (new constant `SOURCE_OURA_API` in `health_schema.py`, added to `KNOWN_SOURCE_FAMILIES`). Three Oura-adjacent families now exist by design and never collapse at import: `apple_health` (mirror rows), `oura_api` (this lane), `oura` (reserved for the pending account export).

**Dedupe keys** go through `health_schema.health_record_dedupe_key`. Every Oura v2 document carries a stable `id`, so keys take the `source-id` path (`source_family` + `record_type` + `source_record_id`). Consequences, both wanted:

- Oura *revises* recent documents (scores settle for a day or two). Same `id` → same key → the dedupe upsert updates in place (`value_hash` records that the payload changed) instead of duplicating. Re-fetching a trailing window is idempotent (L9).
- The temperature-deviation row splits out of the readiness document with `source_record_id = "<readiness id>/temperature_deviation"`, keeping its identity distinct and stable.

---

## 5. (d) Sync design

**Backend.** `OuraSyncBackend` is registered in `SYNCABLE_REGISTRY["oura"]` and implements save-mode sync. Save runs validate the pre-save gate before taking the per-journal import lock, recheck the gate inside the lock, then fetch and persist only after the gate and lock both hold. Cursor-only quiet runs intentionally advance `imports/oura.json` without creating an import bundle.

**Cursor state** at `imports/oura.json` via `sync.load_sync_state`/`save_sync_state`:

```json
{
  "schema": "solstone.import_sync.oura.v1",
  "last_sync_at": "2026-07-10T06:00:00Z",
  "endpoints": { "daily_sleep": {"high_water_day": "2026-07-09"}, "…": {} },
  "backfill": { "complete": false, "oldest_fetched_day": "2026-03-01" },
  "last_result": { "pages": 4, "rows": 61, "inserted": 58, "updated": 3 }
}
```

Never tokens, never client credentials, never raw values in the cursor. Catalog (dry-run) sync writes **nothing**, including the cursor; the cursor advances only on gated save runs.

**Poll cadence.** The ring reaches Oura's cloud only when the phone app syncs, so aggressive polling buys nothing. Default: every 6 hours via the existing scheduler, plus manual `journal importer --sync oura` (catalog by default, `--save` for the gated write path). Each save run re-fetches a trailing 7-day window to pick up Oura's document revisions — idempotent by document-id keys.

**Backfill.** The API serves full history: page each endpoint in 30-day `start_date`/`end_date` chunks, following `next_token`, walking back from today until pages come back empty (or from the `personal_info` registration date if exposed). Resumable via `backfill.oldest_fetched_day`; runs inside the same gate + rate-limit budget (limit figure ⚠ verify at O2). Backfill is just repeated save-mode sync — no special write path.

**OAuth (design only; phase O2, OWNER-PRESENT-ONLY).**

- Authorization-code flow; **PKCE preferred if Oura supports it** (⚠ unverified — Oura's documented flow historically uses a client secret; if PKCE isn't supported, the confidential-client secret lives behind the same token boundary below, and nothing else changes).
- Redirect: loopback `http://localhost:<ephemeral>/callback` on the journal host, opened in the owner's browser with the owner at the keyboard. No headless, no automated retry, no unattended re-auth ever; if tokens die, sync degrades to a factual "authorization needed" status until the owner runs the step again.
- **Token boundary: journal configuration, never the repo.** Client id, (secret if applicable), access + refresh tokens live in the journal's config domain under the reserved key `oura` (`OAUTH_CONFIG_KEY` in the skeleton), written exclusively through the config owner `solstone/think/journal_config.py` (L2). Never in this repository, never in env vars, never in logs, never in `imports/oura.json`, never through Oracle/Claude prompts. Refresh-token rotation writes through the same owner.
- **Dev-account cap noted:** Oura developer apps are limited to roughly 10 users before requiring Oura's partnership review — irrelevant for a single owner, but it means client credentials must never be shared or committed, and a future multi-owner story needs Oura's blessing first.
- Scopes: future owner-present authorization requests ask for `daily`, `heartrate`, `workout`, `tag`, `session`, `spo2`, `stress`, `heart_health`, and `metabolic`. `email` and `personal` are no longer requested.

| Scope | Endpoint family authorized | Polled? | Notes |
|---|---|---|---|
| `daily` | daily sleep/readiness/activity documents | Yes | Core daily scores and activity documents. |
| `heartrate` | `heartrate` series | Yes | Oura-native high-frequency series; normalized with owner-local day attribution. |
| `workout` | `workout` documents | Yes | Activity interval documents. |
| `tag` | `enhanced_tag` documents | Yes | Owner-entered tag metadata. |
| `session` | `session` documents | Yes | Meditation/breathing/rest sessions. |
| `spo2` | `daily_spo2` documents | Yes | Nightly oxygen summary documents. |
| `stress` | `daily_resilience`, `daily_stress` | Yes | Live Oura scope system maps this to resilience/stress families. |
| `heart_health` | `daily_cardiovascular_age`, `vO2_max` | Yes | Live Oura scope system maps this to cardiovascular age and VO2 max. |
| `metabolic` | `blood_glucose` only | No | Kept deliberately, but `blood_glucose` remains partner-gated in `_PARTNER_GATED_ENDPOINTS` and is never in `SYNC_ENDPOINTS`. |

Removing `email` and `personal` changes only what future authorization requests ask for. It does not retroactively revoke scopes already granted on an already-issued token. Narrowing an existing token's granted scopes requires owner-present re-consent and/or revoking the old token; this implementation does not perform that operator action.

---

## 6. (e) Gate

Landed in the skeleton:

- `SENSITIVE_IMPORTERS` is an explicit importer/backend-name gate set: `{"apple_health", "oura"}`. It intentionally does not derive from source-family registries because `oura_api` and `dexcom_clarity` are source families, not approval-artifact importer names.
- Same approval artifact (`imports/_approvals/health_import_preflight.json`, same `APPROVAL_SCHEMA`/`CHECKLIST_VERSION`): `approved_importers` must include `"oura"`; all five replication-destination decisions and the raw-retention decision apply unchanged to Oura data.
- Same per-run `--confirm-health-save` requirement; `OuraImporter.process()` enforces the gate itself in save mode (defense in depth alongside the CLI's pre-`process` enforcement), **before** any parse or write, then stops at the phase-O1 seam.
- Tests prove: missing artifact blocks; artifact without `"oura"` in `approved_importers` blocks (`importer_not_approved`); missing per-run confirmation blocks; a fully approved run still writes nothing (seam); failure payloads leak no fixture paths or values.

Phase O3 extends the health gate to sync with a separate `oura_sync_preflight` artifact: any save-mode `sync()` calls `enforce_oura_sync_gate(...)` before its first journal write, with the confirm flag passed explicitly from the CLI/scheduler invocation. Scheduled runs require `scheduled_sync.approved: true`, a cadence, and an unexpired timezone-aware `scheduled_sync.valid_until`; a scheduled job never self-confirms implicitly.

The save-mode sync lock is `hold_lock(journal_root / "imports" / "oura.json", mode=0o600)`, which creates the private sidecar `imports/oura.json.lock`. The first gate runs before the lock so an initially invalid artifact creates nothing. The authoritative gate then runs again inside the lock before cursor read, client construction, token refresh, fetch, import-id allocation, bundle writes, or cursor advance. The lock is held through all of those steps, closing the import-id TOCTOU. If token refresh happens while the health lock is held, the ordering is health lock -> config lock; config writes do not acquire the health lock, so there is no inverse path.

Oura sync applies the validated raw-retention decision from `PreSaveGateDecision`: `retain_parsed` writes raw API page JSONL under `imports/<id>/raw/oura/`; `discard` writes no raw pages and no new `raw_ref` values; `retain_complete` is rejected by the gate as source-incompatible. Scheduled consent is valid only while `now < scheduled_sync.valid_until`; missing, malformed, naive, or expired values block before any network or journal write. Consent expiring after the authoritative in-lock gate passes does not abort the running transaction.

---

## 7. (f) Phased rollout

| Phase | Contents | Guardrail |
|---|---|---|
| **O0 — landed tonight** | Design doc; `oura.py` parse/normalize/dedupe skeleton; synthetic fixtures; `"oura"` in `SENSITIVE_IMPORTERS`; file-importer registry entry with only detect/preview/dry-run live; sync + OAuth seams raise `NotImplementedError` | No network code (test-enforced); no journal writes; no sync registry entry |
| **O1 — file-import save path (synthetic only)** | Raw install under `imports/<id>/raw/oura/`; normalized shards; dedupe upserts; optional `--with-day-summaries` writing `import.oura` transcripts from `render_day_summary`; L2 table + hygiene-script owner entries extended to `oura.py`; body app absorbs record types (family rules, sleep card, "How recovered am I?" card, window `day_context`) | Gate enforced; synthetic fixtures and temp journals only until the owner's separate live approval |
| **O2 — first OAuth. OWNER-PRESENT-ONLY.** | Register Oura dev app; verify PKCE vs confidential, scopes, rate limits, sandbox against live docs; interactive `sol import oura auth` with the owner at the keyboard; tokens land in journal config via `journal_config.py`; single `personal_info` verification call; nothing unattended | **Owner physically present for every step**; no credentials anywhere but journal config |
| **O3 — sync** | Real `sync()` (catalog default, gated save); `SYNCABLE_REGISTRY` entry + flip the phase-guard test; cursor state `imports/oura.json`; trailing-7-day revision window; 30-day backfill chunks; double-run idempotence verified | Gate before first write; catalog mode writes nothing |
| **O4 — steady state** | 6-hourly schedule (opt-in per §6); backfill completion; pending-export reconciliation when it arrives (read-only inspection first); webhooks study (deferred — needs a public endpoint) | Scheduled runs only after explicit opt-in recorded in the approval artifact |

---

## 8. Open questions for the owner (morning)

1. **PKCE vs client secret** — can't verify Oura's current OAuth support offline; decides whether a secret enters journal config at O2.
2. **Family naming** — keeping `oura_api` vs `oura` split assumes the account export may still arrive with a different shape. If you cancel the export request, we could collapse to one `oura` family before any live data exists (cheapest moment to rename).
3. **Day-summary transcripts** — should Oura write optional `import.oura` day summaries like Apple Health does (`--with-day-summaries`), or should day pages read normalized rows only? Skeleton renders the copy either way.
4. **`daily_activity` / `heartrate` endpoints** — excluded to avoid double-counting the AH mirror. Confirm, or pick a precedence rule for presentation.
5. **Canonical home for this doc** — copy into `docs/design/` (repo) as the durable reference?
6. **Oura sandbox API** — a no-auth synthetic endpoint (⚠ verify) could de-risk O3 before real OAuth; still network, so it needs your explicit go-ahead like any other network step.

---

## 9. Amendments — 2026-07-07 (token relocation + glucose/cardio lane)

Upstream ruling (project owner, 2026-07-07): *"the journal is the one
trusted store, so device OAuth tokens live there alongside everything else
rather than machine-local. no carve-out for device tokens."* This restores
§5's original token boundary and lands with the following changes, in one
commit (the hourly sync lane runs against repo HEAD):

- **Token storage (hard cut, no shims).** `OuraTokens` load/save moved from
  `~/Library/Application Support/Solstone/secrets/oura/` into journal config
  under `oura.tokens.{access_token, refresh_token, expires_at, token_type}`,
  and the confidential-client secret into `oura.client_secret` — both read
  and written exclusively through the config owner
  (`solstone/think/journal_config.py`, writes under `hold_config_lock`).
  Refresh rotation persists through the same path.
  `solstone/think/importers/local_secrets.py` and the fingerprint-keyed
  machine-local scheme are **deleted** (the old files on disk remain
  untouched as a safety copy; they are simply no longer read).
- **Scopes.** The connect flow (`journal importer --connect oura`) now
  requests an explicit scope set and prints it for the owner:
  `daily heartrate workout tag session spo2 stress heart_health metabolic`.
  `email` and `personal` were removed from future authorization requests;
  existing tokens keep whatever scopes they were already granted until the
  owner re-consents or revokes them. The first six are Oura's documented
  health-data set;
  `stress` / `heart_health` / `metabolic` are live but undocumented
  (evidence: tidepool-org/platform's Oura partner integration maps
  `extapi:stress` → daily_resilience, `extapi:heart_health` →
  daily_cardiovascular_age + vo2_max, `extapi:metabolic` → blood_glucose;
  Oura's authorize front door verifiably rewrites plain `scope=X` to
  `extapi:X`). Empirical driver: our no-scope default grant reads
  resilience and cardiovascular age but 401s on blood_glucose.
- **Two new sync endpoints.**
  - `daily_cardiovascular_age` — documented (openapi-1.35): day-paged
    documents `{id, day, pulse_wave_velocity (m/s, nullable),
    vascular_age (years, nullable)}`; journal day is Oura's `day`
    verbatim; normalizes to `oura.daily_cardiovascular_age`
    (value=vascular_age, unit=years).
  - `blood_glucose` — live but absent from the published spec (route
    exists: unauthenticated GET returns missing-token 400 where bogus
    routes 404; 401 with our current token = scope gap, not 404).
    **Pinned assumptions** (comment + tests; the first post-reauth sync
    falsifies or confirms): heartrate-shaped series rows
    `{timestamp, glucose}`, UTC instants converted to owner-local for
    day/month (raw timestamp kept in `source_record_id`), mg/dL,
    datetime-paged with a 31-day chunk cap.
- **Cursor upgrade / backfill.** Per-endpoint cursor state gains
  `backfill_complete`. Endpoints missing from an existing cursor are
  fetched from `BACKFILL_HORIZON_DAY` (2015-01-01, pre-dating any Oura
  data) on the first post-upgrade save — full history within chunk
  limits, not just the trailing window. Endpoints that complete a fetch
  with no data are marked backfilled and poll a 30-day trailing window
  thereafter (no horizon re-walks). Fresh installs keep the deliberate
  30-day first-sync window.
- **Scope degradation (deploy safety).** A 401 that survives one good
  token refresh raises `OuraEndpointUnauthorized`; the sync engine skips
  that endpoint, reports it in `errors`, and keeps every other endpoint
  syncing — so the hourly lane stays alive between this commit landing
  and the owner's reauthorization, and the skipped endpoint backfills
  from the horizon on the first post-reauth save.

Historical operator steps after that amendment: `journal importer --connect oura`
(browser reauth with the printed scopes), then
`journal importer --sync oura --save --confirm-body-save` (the new
endpoints backfill from the horizon automatically).

---

## 9a. Amendments — 2026-07-07 (granted-scope endpoints + blood_glucose partner-gating)

The owner reauthorized with the full printed scope set and directed that
every newly granted endpoint "make it into solstone". All shapes below
were verified two ways on 2026-07-07: against openapi-1.35 (fetched from
Oura's docs) and by read-only live GET probes with the freshly granted
token (no `--save` run, no journal writes).

### Four endpoints join `SYNC_ENDPOINTS`

| Endpoint | Live rows (probe) | Shape (openapi-1.35, live-confirmed) | Normalization |
|---|---|---|---|
| `workout` | 93 since Jun 1; 390 since 2024-01-01 | required `id, activity, day, start_datetime, end_datetime, intensity (easy/moderate/hard), source (manual/autodetected/confirmed/workout_heart_rate)`; nullable `calories` (kcal), `distance` (m), `label` | `oura.workout`, kind=`workout` (mirrors the AH workout event shape); no scalar value; activity/intensity/source/label/calories/distance in metadata |
| `session` | 1 row | required `id, day, start_datetime, end_datetime, type (breathing/meditation/nap/relaxation/rest/body_status)`; nullable `mood (bad/worse/same/good/great)` and `heart_rate`/`heart_rate_variability`/`motion_count` sample blocks `{interval, items[], timestamp}` | `oura.session`, kind=`session`; no scalar value; metadata carries `type`/`mood` only — sample blocks stay in the raw page (raw_ref), never in normalized rows |
| `enhanced_tag` | 2 rows | required `id, start_time, start_day` — **the one document endpoint with no `day` field**; nullable `tag_type_code, end_time, end_day, comment, custom_name` | `oura.enhanced_tag`, kind=`tag`; journal day = `start_day` verbatim (`_DOCUMENT_DAY_FIELDS`); tag text fields are metadata, never a value |
| `vO2_max` | 0 rows (endpoint valid: 200 empty page; lowercase `vo2_max` 404s — the route casing is exact) | required `id, day, timestamp, vo2_max (integer)` (PublicVO2Max) | `oura.vo2_max`, kind=`daily_summary`, value=`vo2_max`, unit `mL/kg/min` (by definition; the spec carries no unit field); friendly name "VO2 max" |

**Timestamp finding (load-bearing).** Workout/session datetimes and tag
times are wearer-local offset instants (`LocalizedDateTime` /
`LocalDateTime`; live workout rows carry `-04:00`…`-07:00` across
travel/DST), **not** UTC-Z — so unlike heartrate there is no
UTC→owner-local conversion: datetimes pass through verbatim and the
journal day is Oura's day field verbatim (a 23:12 local workout whose
UTC instant crosses midnight stays on its local day; timezone-pinned in
tests).

**Window limits.** All four are day-paged (`start_date`/`end_date`).
Live probes accepted a 2.5-year window on each (200), so the default
364-day chunking is comfortably safe; no per-endpoint cap entries were
added. Horizon backfill from 2015-01-01 is ~12 chunks per endpoint.

**Cursor upgrade.** Exactly the §9 semantics: the four endpoints are
missing from existing cursors, so the first post-upgrade save walks each
from `BACKFILL_HORIZON_DAY`; endpoints that come back empty (vO2_max,
likely) are marked `backfill_complete` and poll a 30-day trailing window
thereafter.

### blood_glucose is partner-gated — demoted from the poll set

Portal finding (owner, 2026-07): the developer portal shows **all**
grantable scopes already enabled and **no `metabolic` option** —
blood_glucose is available only to Oura partner integrations
(Tidepool-class). Every poll would 401 forever, and the hourly lane was
reporting that error each cycle. Resolution:

- `_PARTNER_GATED_ENDPOINTS = ("blood_glucose",)` in `oura.py`;
  `SYNC_ENDPOINTS` no longer contains it, so syncs make zero
  blood_glucose requests and report zero errors (test-pinned).
- The parse/normalize/dedupe machinery, fixture, and §9 pinned
  assumptions all stay wired for a future partner grant or file import.
- The cursor never carries the endpoint (stale entries from the 2026-07
  cursor generation are dropped on the next save rewrite), so it is
  never marked `backfill_complete` — re-enabling is one line (move the
  name back into `SYNC_ENDPOINTS`) and still backfills from the horizon.

### Display-pass notes (native body surface)

The same ring's workouts arrive through both pipes: `oura.workout` rows
(this lane) and AH-mirror `HKWorkoutActivityType*` rows sourced "Oura".
Day-level aggregation must keep one canonical pipe (O-5C). The retired Python
mapping is not a source for future presentation changes:

```python
OURA_WORKOUT_TYPE: ("WorkoutActivityType",),   # OURA_WORKOUT_TYPE = "oura.workout"
```

(The fragment must NOT be spelled with the `HK` prefix: `HK`-prefixed
fragments match exactly one identifier, while the bare fragment
substring-matches every `HKWorkoutActivityType*` activity. The
`_is_oura_named_mirror_row` guard already restricts the drop to
Oura-sourced mirror rows, so Watch/iPhone workouts are untouched.)

Also for the display pass: `oura.workout` rows carry Oura's field names
in metadata (`calories` kcal, `distance` m, `activity`), not the AH
`totalEnergyBurned`/`totalDistance` keys `_workout_metrics` reads — the
day-card metrics line needs a small mapping (or the generic fallback)
if Oura workout calories/distance should render; duration already works
(computed from `start_date`/`end_date`). The card name currently
renders the friendly type ("Workout"); `metadata["activity"]` has the
specific activity if wanted.

Operator step after this lands (owner-side backfill):
`journal importer --sync oura --save --confirm-body-save`, with the four
new endpoints backfill from the horizon automatically; no
reauthorization needed (the scopes were granted 2026-07-07).

---

## 10. What landed with this doc (historical phase-O0 inventory)

- `solstone/think/importers/oura.py` — parse layer (`parse_oura_bundle`, `parse_endpoint_document`, `parse_oura_day`), normalizer (`normalize_bundle` → rows + `HealthDedupeRecord`s via `health_schema`), factual rendering (`render_day_summary`), `OuraImporter` (detect/preview/dry-run live; save gated then seamed), `OuraSyncBackend` + OAuth seams. Network egress follows a lazy-import discipline: no module-level network imports, with live egress confined to the allowlisted transport path enforced by tests.
- `solstone/think/importers/health_schema.py` — `SOURCE_OURA_API`, `KNOWN_SOURCE_FAMILIES` entry, friendly names for the seven `oura.*` record types.
- `solstone/think/importers/pre_save_gate.py` — `"oura"` joins `SENSITIVE_IMPORTERS`.
- `solstone/think/importers/file_importer.py` — registry entry (preview/dry-run-only paths active).
- `tests/fixtures/importers/health/oura_synthetic/` — six endpoint documents, API-page-shaped, arithmetic-consistent, fully synthetic.
- `tests/test_oura_importer.py` — 30 tests: registration/gate membership, parse validation, normalization + dedupe-key stability and cross-family non-collision, JSONL round-trip, detect/preview/dry-run, gate enforcement (blocks before any write; approved runs still write nothing past the seam), attributed factual rendering, seam errors, no-network guard.
