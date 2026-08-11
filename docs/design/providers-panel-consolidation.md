# Providers Panel Consolidation

Historical design note, July 2026: this document predates the current one-brain
provider architecture in `docs/PROVIDERS.md` and the retired OpenHands install
card. Treat body references to a standalone `mlx` provider, `openhands`, or the
old Settings provider-panel structure as historical implementation notes, not
current routing guidance.

Settings -> Providers will move from three install renderers (`bundledProviders`, `mlxBootstrapRegion`, `localBootstrapRegion`) to one install panel that renders `anthropic`, `openai`, `openhands`, `local`, and `mlx` from the shipped install-state contract. The contract producers stay unchanged: bundled provider state remains in `solstone/think/providers/bundled.py`, local state remains in `solstone/apps/settings/local_bootstrap.py`, mlx state remains in `solstone/apps/settings/mlx_bootstrap.py`, and copy remains in `solstone/apps/settings/install_copy.py`.

## D1. `/api/providers` Payload Extension Shape

Decision: keep the existing `bundled` map and add peer top-level `local` and extended `mlx` card-state dicts. Do not fold all five into a new server-side `install` or `providers_install` dict.

Rationale: before the cogitate-baseline change, `routes.py:872-895` returned bundled provider state for `anthropic`, `openai`, and `openhands`, and `workspace.html:5138-5139` consumed that field. Keeping `bundled` avoided changing the then-locked bundled contract shape while allowing the unified renderer to build one client-side map:

- `anthropic`, `openai`, `openhands` from `data.bundled`.
- `local` from `data.local`.
- `mlx` from `data.mlx`.

Server changes in `solstone/apps/settings/routes.py`:

- Add optional local model selection for `GET /api/providers`: read `request.args.get("local_model")`, validate against `LOCAL_MODEL_SPECS` the same way `_local_model_from_request()` validates `model` at `routes.py:638-642`, and fall back to `LOCAL_FLASH`.
- Add `"local": local_bootstrap.get_state(local_model_id)` to the response. The state shape is the 7-field `InstallStatus` payload returned by `local_bootstrap.get_state()` (`local_bootstrap.py:173-177`, `_payload_for_status` at `local_bootstrap.py:138-157`).
- Extend the existing `"mlx"` dict. Keep `"active_model"` for existing picker code (`routes.py:876-893`, `workspace.html:5347`, `workspace.html:5684-5686`) and merge in `mlx_bootstrap.get_state(mlx_active_model)`, whose payload is the same 7 install fields (`mlx_bootstrap.py:220-223`, `_payload_for_status` at `mlx_bootstrap.py:185-204`).

Model-state rule:

- Local: state is for the current local picker model when the browser supplies `local_model=<id>`; otherwise `LOCAL_FLASH`. This mirrors today: `getSelectedLocalModel()` returns the picker value or `LOCAL_FLASH` (`workspace.html:5689-5691`), and local status/bootstrap calls are made with that model (`workspace.html:6014-6015`, `workspace.html:6038-6039`).
- MLX: state is for the persisted `mlx.active_model`, defaulting to `QWEN_35_9B` (`routes.py:876-879`). The picker persists through `PUT /api/providers` (`routes.py:1269-1309`, `workspace.html:6439-6453`).

Renderer-read keys:

| Provider | Server-supplied keys | Client-derived/defaulted keys |
|---|---|---|
| `anthropic`, `openai` | Bundled card payload: `name`, `label`, `install_state`, `key_status`, `disabled`, progress fields, `install_error`, SDK/binary/auth/key fields, `issues`, `actions` (`bundled.py:605-647`) | Provider kind = `cloud`; overflow/action labels; key pill text/tone. |
| `openhands` | Runtime bundled payload: same bundled keys plus `runtime`, with `key_status = "not-applicable"` (`bundled.py:667-710`) | Provider kind = `openhands`; no key pill. |
| `local` | `InstallStatus`: `name`, `install_state`, `last_transition_at`, `last_progress_at`, `progress_bytes_received`, `progress_bytes_total`, `install_error` (`install_state.py:22-29`) | Label, no key pill, no bundled `actions`/`issues`, action set, model id from picker/default. |
| `mlx` | Existing `active_model` plus `InstallStatus` for that model (`routes.py:876-893`, `mlx_bootstrap.py:220-223`) | Label, no key pill, no bundled `actions`/`issues`, action set. |

Important renderer rule: use the map key (`local`, `mlx`) as the provider id. `mlx_bootstrap.get_state(model)` returns `name = model_id` (`mlx_bootstrap.py:127-128`), so action dispatch must not use `state.name` for local/mlx endpoint selection.

## D2. Card Component Contract

Decision: replace `bundledProviderMeta(state)` (`workspace.html:5365-5472`) with `providerCardMeta(state, kind)`, where `kind` is one of `cloud`, `openhands`, or `local-mlx`.

Inputs:

| Kind | State fields consumed |
|---|---|
| `cloud` | `install_state`, `install_error`, `progress_bytes_received`, `progress_bytes_total`, `key_status`, `key_configured`, `key_valid`, `disabled`, `binary_path`, `issues` from `bundled.py:605-647`. |
| `openhands` | `install_state`, `install_error`, progress fields, `disabled`, `runtime`, `binary_path`, `issues`; `key_status` is ignored except for `not-applicable` (`bundled.py:667-710`). |
| `local-mlx` | `install_state`, `install_error`, `progress_bytes_received`, `progress_bytes_total`; no `key_status`, no `disabled`, no bundled `actions`, no bundled `issues` (`local_bootstrap.py:138-177`, `mlx_bootstrap.py:185-223`). |

Outputs:

- `badgeLabel`
- `badgeTone`
- `primaryLabel`
- `primaryAction`
- `primaryDisabled`
- `overflowActions`
- `showKeyPill`
- `keyPillLabel`
- `keyPillTone`
- `showByteCounter`
- `showInstallError`

State matrix:

| install_state | key_status | Badge | Primary button | Overflow actions |
|---|---|---|---|---|
| `idle` | `key-needed` | `INSTALL_PHASE_IDLE` | `INSTALL_BUTTON_INSTALL` | none |
| `idle` | `valid` | `INSTALL_PHASE_IDLE` | `INSTALL_BUTTON_INSTALL` | none |
| `resolving` | any | `INSTALL_PHASE_RESOLVING` | `INSTALL_BUTTON_INSTALLING`, disabled | none |
| `downloading` | any | `INSTALL_PHASE_DOWNLOADING`; append bytes for local/mlx when progress fields are non-null | `INSTALL_BUTTON_INSTALLING`, disabled | none |
| `verifying` | any | `INSTALL_PHASE_VERIFYING`; append bytes for local/mlx when progress fields are non-null | `INSTALL_BUTTON_INSTALLING`, disabled | none |
| `installing` | any | `INSTALL_PHASE_INSTALLING` | `INSTALL_BUTTON_INSTALLING`, disabled | none |
| `installed` | `not-applicable` | `INSTALL_PHASE_INSTALLED` | none | bundled: `uninstall`, `disable` or `enable`; local/mlx: none |
| `installed` | `valid` | `INSTALL_PHASE_INSTALLED` | `validate-key` when key is configured; otherwise none | `uninstall`, `disable` or `enable` |
| `installed` | `validating` | `INSTALL_PHASE_INSTALLED` | disabled key validation indicator | `uninstall`, `disable` or `enable` |
| `installed` | `invalid` | `INSTALL_PHASE_INSTALLED` | client action to focus/re-enter key, then `validate-key` after key edit | `uninstall`, `disable` or `enable` |
| `installed` | `key-needed` | `INSTALL_PHASE_INSTALLED` | client action to focus/add key | `uninstall`, `disable` or `enable` |
| `failed` | any | `INSTALL_PHASE_FAILED_PREFIX + reason` | `INSTALL_BUTTON_RETRY` | none |

Failed-reason lookup:

| State value | Rendered detail |
|---|---|
| `install_error` is null/empty | `INSTALL_FAILED_FALLBACK` |
| `install_error === INSTALL_FAILED_UV_MISSING` | that constant value |
| other non-empty string | the server-provided string |

The renderer must source the prefix and fallback values from `INSTALL_COPY` (`workspace.html:3485`, `install_copy.py:6-22`) and must not retype phase strings because `test_workspace_does_not_duplicate_install_copy_strings` checks those values (`test_workspace_install_integrity.py`).

Disabled flag:

- Applies to bundled providers only (`bundled.py:610`, `bundled.py:672`).
- Preserve current behavior: disabled overrides the primary badge with `Disabled`, primary action becomes `enable`, and `uninstall` remains available when the provider is not actively installing (`workspace.html:5369-5376`, `bundled.py:560-564`).

Provider action sets:

- Cloud (`anthropic`, `openai`): install/retry, validate-key, disable, enable, uninstall. Key focus actions are client-only affordances for `key-needed`/`invalid`; endpoint dispatch remains `validate-key` after a key exists.
- `openhands`: install/retry, disable, enable, uninstall. No key pill; `key_status` is `not-applicable` (`bundled.py:662`).
- `local`/`mlx`: install/retry only. Existing local/mlx regions expose retry but no uninstall or disable controls (`workspace.html:5885-5893`, `workspace.html:6096-6104`).

## D3. Action Dispatch Map

Decision: one `runProviderAction(providerId, action)` dispatches by provider kind.

| Provider kind | Action | Method + endpoint | Body |
|---|---|---|---|
| cloud/openhands | `install` | `POST /api/providers/<name>/install` (`routes.py:962-967`) | none |
| cloud/openhands | `uninstall` | `POST /api/providers/<name>/uninstall` (`routes.py:969-973`) | none |
| cloud/openhands | `disable` | `POST /api/providers/<name>/disable` (`routes.py:976-980`) | none |
| cloud/openhands | `enable` | `POST /api/providers/<name>/enable` (`routes.py:983-987`) | none |
| cloud | `validate-key` | `POST /api/providers/<name>/validate-key` (`routes.py:990-994`) | none |
| local | `install` / `retry` | `POST /api/local/bootstrap?model=<id>` (`routes.py:721-729`) | none |
| mlx | `install` / `retry` | `POST /api/mlx/bootstrap?model=<id>` (`routes.py:658-666`) | none |

Model ids:

- MLX: `<id>` comes from `providersData.mlx.active_model` or `QWEN_35_9B` (`workspace.html:5684-5686`, `routes.py:876-879`).
- Local: `<id>` comes from `getSelectedLocalModel()` when the picker exists and has a value, otherwise `LOCAL_FLASH` (`workspace.html:5689-5691`). This also covers AC8 pre-install: before local is selected anywhere, the card installs `LOCAL_FLASH`.

After any action, update the one card optimistically only enough to show a pending state, then start the panel poll. The authoritative state comes from the next `/api/providers` poll.

## D4. Poll Cycle

Decision: one `pollProvidersPanel()` owns install/key polling.

| Concern | Decision |
|---|---|
| Interval | 1000ms. This matches current local/mlx polling (`workspace.html:5820-5824`, `workspace.html:6030-6034`) and is faster than bundled's current 2000ms (`workspace.html:5633-5635`). |
| Request | `GET /api/providers?local_model=<getSelectedLocalModel()>`. The response includes bundled, local, and active MLX install state per D1. |
| Start condition | Any card has `install_state` in `IN_FLIGHT_INSTALL_STATES` (`workspace.html:3486`), any bundled card has `key_status === "validating"` (`workspace.html:5619-5625`), or a just-fired action is awaiting its first reflected state. |
| Stop condition | All install states are terminal (`idle`, `installed`, `failed`) and no card has `key_status === "validating"`. |
| Render trigger | Every successful poll calls `renderProvidersPanel(data)`. |
| Error handling | Stop the interval and surface `notifyError("Provider status failed", err.message)`, matching current bundled poll behavior (`workspace.html:5645-5657`). |

Retired functions:

- `pollBundledProviders`, `startBundledProviderPolling`, `clearBundledProviderPollTimer`, `syncBundledProviderPolling` retire (`workspace.html:5619-5658`).
- `pollMlxBootstrap`, `startMlxBootstrapPolling`, `clearMlxBootstrapPollTimer` retire as polling functions (`workspace.html:5772-5777`, `workspace.html:5820-5839`).
- `pollLocalBootstrap`, `startLocalBootstrapPolling`, `clearLocalBootstrapPollTimer` retire as polling functions (`workspace.html:5982-5987`, `workspace.html:6030-6049`).
- `startMlxBootstrap` and `startLocalBootstrap` survive as action helpers or are folded into `runProviderAction` without losing their endpoints (`workspace.html:5802-5818`, `workspace.html:6012-6028`).

No behavior loss:

- MLX/local byte progress remains at 1000ms because the unified poll uses the same interval as current `pollMlxBootstrap` and `pollLocalBootstrap`.
- `/api/providers` must call `local_bootstrap.get_state(...)` and `mlx_bootstrap.get_state(...)`, not only read persisted config, because `_payload_for_status` overlays in-memory byte progress (`local_bootstrap.py:141-157`, `mlx_bootstrap.py:188-204`).

## D5. Model-Picker Relocation

Decision: delete the bootstrap-region wrappers and progress shells, but keep single shared model pickers near their current DOM locations.

Local:

- Move `#field-local-active-model` out of `#localBootstrapRegion` (`workspace.html:2480-2498`).
- Keep one shared local-model row between the generate provider row and the cogitate subsection, near today's position.
- Show it when either generate or cogitate provider is `local`, preserving `isLocalProviderSelected()` (`workspace.html:5935-5938`).
- Keep `localModelIdentifier` and `localScopeCopy` with the picker; remove install state, progress, and retry controls from that row.

MLX:

- Move `#field-mlx-active-model` out of `#mlxBootstrapRegion` (`workspace.html:2461-2479`).
- Place it immediately after the generate provider row (`workspace.html:2436-2460`) because MLX is generate-only and cogitate selection is disabled with an explanatory title (`workspace.html:5664-5677`).
- Keep `mlxModelIdentifier` and `mlxScopeCopy` with the picker; remove install state, progress, and retry controls from that row.

Insertion point: after the closing `</div>` for the generate provider row at `workspace.html:2460`, before generate key warning and before the cogitate subsection at `workspace.html:2505`.

## D6. Pre-Install Model Choice (AC8)

Decision:

- MLX pre-install uses `mlx.active_model`, defaulting to `QWEN_35_9B` (`routes.py:876-879`, `workspace.html:5684-5686`).
- Local pre-install uses `LOCAL_FLASH`. If the local picker is visible and the owner has selected another model, use that picker value; before any local selection, the picker is not visible and `getSelectedLocalModel()` falls back to `LOCAL_FLASH` (`workspace.html:5689-5691`).

Do not add persisted local active-model state in this change. That would be a selection-model change, while the spec says renderer consolidation only.

## D7. Test Coverage Matrix

| Test | File | Assertions |
|---|---|---|
| Structural rewrite | `solstone/apps/settings/tests/test_workspace_html.py` | Assert `#providersPanel` exists; assert `#bundledProviders`, `#mlxBootstrapRegion`, `#localBootstrapRegion`, `bundled-provider-grid`, `mlx-bootstrap-region`, `local-bootstrap-region`, `mlx-progress-shell`, and `local-progress-shell` are absent; assert `startMlxBootstrap` and `startLocalBootstrap` or their endpoint dispatch equivalents remain; assert `renderProvidersPanel`, `providerCardMeta`, `runProviderAction`, and `pollProvidersPanel` exist. Current assertions to replace are at `test_workspace_html.py:96-194`. |
| Endpoint payload | New `solstone/apps/settings/tests/test_providers_payload_extended.py` | Use the `settings_client` pattern from `test_workspace_install_copy_template.py:15-26`; `GET /api/providers`; assert top-level `local` and `mlx` dicts exist; each includes the 7 `InstallStatus` fields and `install_state` is in the canonical vocabulary from `InstallState` (`install_state.py:11-19`). |
| Visual smoke | New `solstone/apps/settings/tests/test_providers_panel_visual.py` | Mirror the werkzeug + pytest-playwright pattern in `test_workspace_qr_size.py:22-52`; navigate to `/app/settings/`; assert one panel renders five cards for `anthropic`, `openai`, `openhands`, `local`, `mlx`; assert each card badge text is from `INSTALL_COPY`; assert retired DOM ids are absent. Mark `@pytest.mark.integration`. |

Visual smoke data source: use a clean temporary settings journal with `setup.completed_at`, following `settings_client` (`test_workspace_install_copy_template.py:15-26`) rather than monkeypatching provider state. Clean fixture is preferred because it exercises the real route and template path.

Playwright precondition: `pytest-playwright` is already in dev dependencies (`pyproject.toml:181-189`), and `make install` installs Chromium (`Makefile:54-58`). Running the visual smoke outside the installed dev env requires the same `playwright install chromium` precondition.

Retained integrity coverage:

- `test_workspace_does_not_duplicate_install_copy_strings` remains in `test_workspace_install_integrity.py`; keep it aligned with the unified renderer's copy source.

Makefile:

- Add the visual smoke test path to `smoke-install-providers` (`Makefile:565-571`) with `-m integration`.

## D8. Touched-Files Manifest

| File | Change |
|---|---|
| `docs/design/providers-panel-consolidation.md` | New design gate document for this change. Markdown does not need SPDX per `AGENTS.md` section 8/9. |
| `solstone/apps/settings/routes.py` | Extend `GET /api/providers` (`routes.py:772-901`) with `local_model` handling plus top-level `local` and extended `mlx` install-state dicts. Do not touch contract-layer producers. |
| `solstone/apps/settings/workspace.html` | Replace three renderer regions: CSS around `workspace.html:947-1075`, markup around `workspace.html:2424-2503`, bundled renderer/action/poll around `workspace.html:5365-5660`, local/mlx poll/progress renderers around `workspace.html:5708-6125`, and dropdown handlers around `workspace.html:6406-6479`. |
| `solstone/apps/settings/tests/test_workspace_html.py` | Rewrite structural assertions that currently require `mlxBootstrapRegion`/`localBootstrapRegion` and old bootstrap calls (`test_workspace_html.py:96-194`). |
| `solstone/apps/settings/tests/test_workspace_install_integrity.py` | Preserve the install-state and install-copy integrity checks. |
| `solstone/apps/settings/tests/test_providers_payload_extended.py` | New endpoint payload test. Add SPDX header because this is Python source. |
| `solstone/apps/settings/tests/test_providers_panel_visual.py` | New pytest-playwright visual smoke. Add SPDX header because this is Python source. |
| `Makefile` | Add visual smoke test to `smoke-install-providers` (`Makefile:565-571`). |

No file deletions are planned. No changes to `install_copy.py`, `install_state.py`, `bundled.py`, `local_bootstrap.py`, or `mlx_bootstrap.py`.

## D9. Commit Plan

Use the requested three-commit split:

1. Backend payload extension: `routes.py` only. Add `local` and extended `mlx` payloads to `/api/providers`; include optional `local_model` request handling. This commit is informed by the frontend plan because the poll request needs the local picker value.
2. Frontend markup + JS: `workspace.html`. Add unified panel, relocate model pickers, replace renderer/action/poll functions, retire old region DOM ids/classes.
3. Tests + Makefile: structural test rewrite, integrity updates, endpoint payload test, visual smoke, and `smoke-install-providers` target update.

This split keeps the locked contract untouched and avoids compatibility shims or deprecated aliases.

## D10. Open Questions for the Operator

- Should the visual smoke cover only static render/retired DOM ids, or also exercise auto-fire by selecting `local`/`mlx` and observing the card enter an in-flight state?
- Should unified cards keep the current bundled metadata line that says `CLI: installed/not installed` (`workspace.html:5500-5508`), especially for `openhands`, or should the card limit itself to the spec fields: label, install badge, optional bytes, key pill, primary action, overflow?
- For local/mlx host-blocked cases, current UI synthesizes a failed/disabled retry state from availability (`workspace.html:5748-5756`, `workspace.html:5955-5963`). Should the unified card preserve that blocked visual state, or is action-time error reporting enough for this consolidation change?
