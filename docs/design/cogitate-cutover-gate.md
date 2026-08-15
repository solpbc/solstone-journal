# Cogitate cutover cogitate cutover: design gate

## 1. Resolve the two incorrect premises

### 1.1 `_execute_with_tools` is in scope, but the rest of talent lifecycle is not

**Decision:** interpret the out-of-scope "talent lifecycle" exclusion narrowly.  It
excludes configuration loading, scheduling, retries, hook declaration validation,
and the `generate` path.  It does **not** exclude the cogitate transport adapter in
`talents.py`: retain and, where necessary, edit `_execute_with_tools` and its direct
`_run_talent` call site.

**Evidence and rationale:** Cortex starts `python -m solstone.think.talents`
(`solstone/think/cortex.py:957-966`).  `_run_talent` selects `_execute_with_tools`
only for a cogitate talent (`solstone/think/talents.py:2040-2054`), while the
adapter dispatches cloud runs straight to `cogitate_client.run_cogitate` and local
runs through the local admission wrapper (`solstone/think/talents.py:1613-1621`).
The AC7--10 behaviors are already in that adapter, not in the thin client:
post-hooks/no-output handling (`:1549-1574`), output/provenance (`:1579-1599`),
terminal fields and `start` suppression (`:1601-1611`), and quota conversion
(`:1625-1646`).  Leaving the whole file untouched would make AC7--10 unverifiable
and would leave the adapter importing a deleted quota class.  This is a small
transport-owned exception, not a reopening of the 2,000-line lifecycle module.

The implementation should make no behavioral rewrite here: preserve the existing
wrapper and only update imports/call seams and tests.  Cortex continues to log
cogitate terminal usage after it consumes this event stream
(`solstone/think/cortex.py:1240-1284`).

### 1.2 OpenHands is not fully dead until the cloud registry is repointed

**Decision:** point all three cloud entries (`google`, `openai`, `anthropic`) in
`PROVIDER_REGISTRY` directly at `solstone.think.cogitate_client`; leave `local`
pointing at `providers.local`.  No brain-CLI wrapper or alternate cloud provider
module is needed.

**Evidence and rationale:** the diagnostic component constructs a normal cogitate
config (including `diagnostic`, provider, model, and limits), gets the registered
module, awaits `module.run_cogitate(config=config, on_event=None)`, and passes only
for a nonblank string (`solstone/think/brain_cli.py:558-596`).  The thin client has
that compatible signature and return type, with only an optional
`context_window` keyword (`solstone/think/cogitate_client.py:157-163`), so it
already satisfies this diagnostic call as well as the cloud talent call.  In
contrast, `providers.local.run_cogitate` performs endpoint resolution, slot
admission, telemetry, and local error translation around the client
(`solstone/think/providers/local.py:170-310`), so it remains local-only and must
not become the cloud target.

This fixes the only live registry consumer of the deleted module.  It also makes
the generic validation dispatch meaningful: `providers.validate_key` and
`providers.validate_model` resolve the registered module and call its methods
(`solstone/think/providers/__init__.py:184-235`); the Thinking routes import those
generic functions rather than importing a provider implementation
(`solstone/apps/thinking/routes.py:59-63,378-393,633-688,763-776,821-856`).

## 2. Decisions for the five open items

### 2.1 Validation functions live in `cogitate_client.py`

**Decision:** move `_validation_reason`, `_probe`, `validate_key`, and
`validate_model` from `providers/openhands.py` into `cogitate_client.py`, alongside
the three generate-child environment override constants they use.  Retain the
generic wrappers in `providers/__init__.py` unchanged and leave local's distinct
`validate_key(provider="local", api_key="")` in `providers/local.py` unchanged;
it has no `validate_model` counterpart (`solstone/think/providers/local.py:328-346`).

**Justification:** these functions use the native generate client, not the SDK:
`_probe` calls `generate_client.generate_with_result` (`providers/openhands.py:1707-1733`),
and the key-validity carve-out/model probe live immediately after it
(`:1736-1760`).  With the registry decision above, locating them in the cloud
native client preserves every routes call without a second cloud facade.  Add
focused tests for generic cloud registry dispatch and retain local validation
coverage; do not move local validation into the cloud client.

### 2.2 Remove the obsolete exception-name classifier branch

**Decision:** delete the `MaxTurnsExhausted`/`MaxIterationsReached` branch from
`providers/shared.py`, including its stale comments.

**Justification:** that branch is an SDK-exception rescue
(`solstone/think/providers/shared.py:318-329`).  The Python class disappears with
the policy/SDK runtime, whereas the native wire emits the already-canonical
`max_turns_exhausted` reason code (`core/crates/solstone-core-cogitate-runtime/src/outcome.rs:110-130`).
Keeping a comment or compatibility arm would preserve a state that cannot occur.
Keep the reason-code vocabulary itself; remove only exception-type matching.

### 2.3 Freeze `cogitate_contract.json`; do not regenerate it from Python

**Decision:** make `core/fixtures/cogitate_contract.json` static committed
conformance data.  Remove its Python builder, artifact-path entry, and
Python-contract imports from `scripts/build_core_fixtures.py`, then adjust
`tests/test_core_fixtures.py` so fixture freshness covers only managed fixtures.
Give the JSON fixture an explicit frozen/provenance field documenting the final
Python-reference commit and the native divergence ledger location.

**Justification:** both Rust crates include this file directly
(`core/crates/solstone-core-cogitate/src/oracle.rs:11` and
`core/crates/solstone-core-cogitate-tools/src/oracle.rs:11`).  Its two direct test
function consumers are the cogitate preamble check
(`core/crates/solstone-core-cogitate/src/preambles.rs:46-48`) and the tools
finish-description conformance check
(`core/crates/solstone-core-cogitate-tools/src/runtime_conformance.rs:81-86`).
The current builder (`scripts/build_core_fixtures.py:231-264`) does not emit the
checked-in `finish_description` key (`core/fixtures/cogitate_contract.json:48-53`),
so regenerating it would silently break the latter test.  There is no surviving
Python contract source after this cut.  Static data therefore is safer than a
misleading generator and keeps `make core-fixtures` meaningful for every other
managed fixture.

Do not confuse this fixture with `cogitate_oracle.json`: the latter has the
broader frozen reference vectors and native divergence reconciliation.  Preserve
the static contract file as Rust-owned conformance input, not as a Python-derived
artifact.

### 2.4 Retire both Python oracle/prompt scripts; preserve native frozen data

**Decision:** delete `scripts/check_cogitate_prompts.py`, remove its Makefile
target/wiring, and delete `scripts/cogitate_oracle_corpus.py`.  Preserve the
committed frozen oracle fixture, but add a final-reference-commit and
native-divergence-ledger pointer in its header/provenance.

**Justification:** the prompt checker imports deleted contract symbols
(`scripts/check_cogitate_prompts.py:39-49`), so it cannot survive as written.
The detector's forbidden definitions cover the actual Python preambles, tiers,
capabilities, and finalization contract (`scripts/check_cogitate_cutover.py:45-57`);
the few remaining helper names in the file are not a viable partial contract.
Native prompt/policy tests now own this contract
(`core/crates/solstone-core-cogitate/src/prompt.rs:16-50,65-120`).  Remove the
`check-cogitate-prompts` call from `install-checks` (`Makefile:800`) and its target
(`Makefile:1082-1083`), rather than retaining a Python linter without an
authoritative Python source.

The corpus generator calls the removed read tools, contract, policy, OpenHands
module, and prompt helpers (`scripts/cogitate_oracle_corpus.py:44-69,1263,1431-1482,1643-1696`).
It already describes the output as a frozen record (`:5-24`), but that wording
does not make an un-runnable generator valuable.  Delete it after recording the
final provenance in the fixture; the native divergence mechanism is the living
owner of intentional differences.

### 2.5 Record two completed conversion-retirement waves

**Decision:** add two `status="done"` waves to `conversion-retirements.toml`, not
one.  The checker accepts exactly one `distribution` per wave
(`scripts/check_conversion_retirements.py:284-353`), while this cut removes two
top-level distributions.  Both entries use an empty
`test_only_dependency_locations` list.

| id | distribution | python_roots | import_roots |
| --- | --- | --- | --- |
| `Cogitate cutover-cogitate-openhands-sdk` | `openhands-sdk` | `solstone/think/providers/openhands.py`, `solstone/think/providers/emit_final_tool.py`, `solstone/think/providers/read_tools.py`, `solstone/think/cogitate_read_tools.py` | `openhands` |
| `Cogitate cutover-cogitate-litellm` | `litellm` | none | `litellm` |

The existing manifest's field shape is `id`, `status`, `distribution`,
`python_roots`, `import_roots`, and `test_only_dependency_locations`
(`conversion-retirements.toml:12-31`).  Before marking the waves done, remove
SDK/package spellings from code, Makefile comments, and relevant maintenance
scripts: the retirement checker scans `Makefile`, `scripts`, and `solstone` for
every alias (`scripts/check_conversion_retirements.py:167-225,365-367`).  In
particular, `scripts/check_extras_consistency.py`,
`solstone/think/providers/shared.py`, `local_endpoint.py`, `local_server.py`,
and `solstone/log_policy.py` currently contain such spellings.

**Clarification:** the new wave entries in `conversion-retirements.toml` are not
their own false positive; that manifest is outside the checker's `content_roots`
(`conversion-retirements.toml:6-16`,
`scripts/check_conversion_retirements.py:204-208`).  The actual unavoidable text
match is the frozen `scripts/check_cogitate_cutover.py` detector's own forbidden
root vocabulary (`:39-57`), because `scripts` *is* a content root.  Do not edit
that detector.  Add its exact path to the retirement manifest's
`content_exclusions`, whose entries are explicitly exact paths
(`scripts/check_conversion_retirements.py:80-97,200-201`), so the retirement
scanner does not mistake enforcement text for a runtime dependency.

## 3. Failure-cap rename and the remaining native-contract consumers

### 3.1 Rename the surviving policy half to `solstone.think.deterministic_failure_caps`

**Decision:** create `solstone/think/deterministic_failure_caps.py` with only
`DETERMINISTIC_FAILURE_REASON_CODES`, `DETERMINISTIC_FAILURE_CAPS`, and
`failure_capped`.  Delete `cogitate_policy.py`; do not retain a forwarding module.
The client-owned request defaults (`MAX_TURNS`, `DEFAULT_RUN_COST_CAP_USD`, and
`DEFAULT_READ_CALL_BUDGET`) move directly into `cogitate_client.py`, where they
are serialized into the native request (`solstone/think/cogitate_client.py:44-85`).

Update production imports in:

- `solstone/think/thinking.py:43,1287-1317`;
- `solstone/think/pipeline_health.py:21-23,1415-1421`;
- `solstone/convey/chat.py:81,1956`;
- `core/crates/solstone-core-system-health/tests/support/python_pipeline_health_oracle.py:9,173`.

Update the corresponding cap tests (`tests/test_think_daily_idempotency.py`,
`tests/test_pipeline_health.py`, and `tests/test_chat_closer.py`) and any retained
script importer.  Delete the command-policy-only `tests/test_cogitate_policy.py`.
This avoids carrying the SDK-era `CogitatePolicy`/`MaxTurnsExhausted` command gate
while preserving the independent daily-completion failure rule.

### 3.2 Replace all live Python-contract consumers with native ownership

`talent.py` validates `access_tier` from the Python contract
(`solstone/think/talent.py:33,90-105,383-393,649-659`), while `talent_cli.py`
uses the same contract for inventory/tool display and `assemble_prompt` for
`show --prompt` (`solstone/think/talent_cli.py:41-48,406-445,500-559`).

Reuse the existing native contract and one-shot boundary instead of copying these
values back into Python or adding a new CLI selector:

- cache and parse `solstone-core cogitate --talent-contract` in
  `cogitate_client.py`; use its `tiers`/`talent_facing` data for `talent.py`
  validation and talent inventory.  This is real production code today:
  `talent_contract()` builds tier/tool JSON
  (`core/crates/solstone-core/src/talent_contract.rs:13-67`), and the existing
  `TalentContract` command dispatches it for exactly
  `solstone-core cogitate --talent-contract`
  (`core/crates/solstone-core/src/main.rs:1844-1850`,
  `core/crates/solstone-core-cli/src/lib.rs:577-582,1969-1975`).
- add one `rendered_prompt` object to the existing terminal `dry_run` event.  Its
  two members are the request's `initial_prompt` and the native-composed
  `system_instruction` from `CogitateRequest::to_run_input()`
  (`core/crates/solstone-core-cogitate-wire/src/request.rs:97-125`).  The current
  dry-run event has no prompt data—only envelope fields, `dry_run`, and `terminal`
  (`event.rs:181-192`; the checked-in wire contract permits no optional dry-run
  fields at `core/fixtures/cogitate_wire_contract.json:121-126`).  Update that
  existing wire contract and its native conformance tests for this one field.
  This reuses `solstone-core cogitate --one-shot`: there is no separate CLI
  `--dry-run` argument, only the JSON request's `dry_run` field
  (`request.rs:36-38,90-93`), which the existing one-shot handler recognizes
  before endpoint resolution (`main.rs:1860-1888`).
- add a small `cogitate_client.py` helper that builds the normal request with
  `dry_run=true`, invokes that existing one-shot command, and extracts
  `rendered_prompt`.  Have `talent_cli.show_effective_prompt` use it.  The command
  remains necessary because it promises an *effective* prompt and prints both
  system instruction and instruction today (`talent_cli.py:422-445`); raw markdown
  plus a tier would omit the conditional native preamble, `sol`-tool hint, and
  read-scope suffix composed by `compose_system_instruction`
  (`core/crates/solstone-core-cogitate/src/prompt.rs:16-50`).  Update
  `tests/test_talent.py` and `tests/test_talent_cli.py` to stub the native
  contract/dry-run helper, not Python contract constants or `assemble_prompt`.

`talent.py` needs the cached native query for its one membership check: it uses
only `TALENT_ACCESS_TIERS` (`talent.py:33,90-100`) and has no other Python or
native-call helper for it.  The current Rust equivalents—`TALENT_ACCESS_TIERS`,
`FUTURE_ACCESS_TIERS`, `AccessCapabilities`, and
`capabilities_for_access_tier`—exist only in the native crate
(`core/crates/solstone-core-cogitate/src/access_tiers.rs:22-87`) and are surfaced
to Python only through the already-existing `--talent-contract` JSON.  The helper
must cache that query; it should not duplicate tier literals in Python.  Neither
`FUTURE_ACCESS_TIERS` nor `AccessCapabilities` is otherwise read by `talent.py`.

`providers/cli.py` has no remaining production caller for its SDK-only
`CLIRunner` or `ThinkingAggregator`; only `QuotaExhaustedError` is still used by
the thin client and talent adapter (`solstone/think/cogitate_client.py:25`,
`solstone/think/talents.py:46`).  Move that exception to `providers/shared.py`,
delete `providers/cli.py`, and retire/move its unit tests accordingly.  Do not
preserve the OpenHands-only `ProviderKeyMissingError`.

Remove the SDK runtime files together: `providers/openhands.py`,
`providers/emit_final_tool.py`, `providers/read_tools.py`, and
`cogitate_read_tools.py`.  Update or retire every test that imports, patches, or
installs their SDK types: the SDK-specific suites are
`test_openhands_provider.py`, `test_openhands_errors.py`,
`test_openhands_sdk_shape.py`, `test_openhands_read_tools.py`,
`test_openhands_sol_tool.py`, `test_cogitate_context_headroom.py`, and
`test_cogitate_local_condenser.py`; generic tests with stale OpenHands/LiteLLM
fixtures include `test_brain_cli.py`, `test_talents_ndjson.py`,
`test_talent_provenance.py`, `test_no_implicit_cloud.py`, `test_local.py`,
`test_lane_failure_honesty.py`, `test_provider_error_classification.py`,
`test_generate_full.py`, `test_pipeline_health.py`, `test_log_policy.py`, and
`test_providers.py`.  Retain only assertions that still describe native behavior,
using client fakes or native event fixtures rather than SDK exception classes.

Finally, remove the two dependencies and their explanatory pin comments from
`pyproject.toml:115-127`, regenerate `uv.lock`, and update active documentation
that still names the former transport: `docs/PROVIDERS.md:61-100`,
`docs/CORTEX.md:95-96,317-320`, `docs/THINK.md:192-198,265-273`, and the stale
OpenHands design note if it remains presented as current guidance.

## 4. AC7--12 test-location and gate plan

The current coverage is useful but not sufficient.  `tests/test_talents.py`
already tests the wrapper's post-hook/provenance/start filtering and terminal
fields (`:180-230`), no-output paths (`:233-300`), local lease lifetime
(`:303-380`), native quota reconstruction (`:383-460`), and usage retention
(`:463-489`).  `tests/test_cortex.py` proves finish-event token logging
(`:2681-2727`) and terminal-error usage handling (`:2828-2868`).  There is no
dedicated `test_cogitate_client.py`; the existing native-event helper in
`test_talents.py:99-117` only covers a controlled successful stream.

| AC | Test location and assertion |
| --- | --- |
| AC7 | Keep and strengthen `tests/test_talents.py::test_native_cogitate_finish_runs_post_hooks_and_persists_clean_output` so its finish comes through a fake native executable/client boundary; assert post-hook output, provenance, and persisted output.  Keep the sidecar cases in `tests/test_talent_provenance.py`, updating their stale registry monkeypatches to `cogitate_client.run_cogitate`. |
| AC8 | Extend that same `test_talents.py` case to assert that native `start` is absent and `cache_hit=False`, `output_changed`, and integer `completed_at_ms` survive on `finish`; retain the blank/nonblank no-output cases. |
| AC9 | Keep `test_native_cogitate_finish_preserves_usage_to_talent_stdout_ndjson` in `tests/test_talents.py` and the Cortex finish/terminal logging tests above.  Add one focused handoff case if needed so a native finish usage object reaches `log_token_usage`, not merely each side independently. |
| AC10 | Retain the existing no-output, quota, and nonquota terminal tests in `tests/test_talents.py`; they cover the wrapper's terminal behavior without reviving an SDK provider. |
| AC11 | Retain both local admission tests in `tests/test_talents.py:303-380`; they already assert acquire, native call, then release on success and failure. |
| AC12 | Add `tests/test_cogitate_client.py`.  Use a fake handshaken executable/process to assert visible terminal errors for handshake/missing-binary, spawn or write failure, and EOF/nonzero mid-stream death without a terminal event.  Assert the exact native `reason_code` and `_evented` behavior. |

Add a focused `check-cogitate-cutover-tests` Make target, dependent on `.installed`,
that runs these named pytest nodes plus the registry/validation, talent-contract,
talent-CLI dry-run-helper, and brain-diagnostic tests.  Invoke it from
`install-checks` next to a new `check-cogitate-cutover` target wrapping
`scripts/check_cogitate_cutover.py`.  This is necessary because the current
`install-checks` chain runs static gates such as prompt checking
(`Makefile:727-800`) but does not run pytest.  The target is the wired gate for
all new Python tests; normal development can still run the same node list via
pytest directly.

## 5. Ordered implementation sequence

1. **Establish native replacement seams first.** Add the cached native talent
   contract reader and the one `rendered_prompt` object on the existing one-shot
   dry-run event, with wire-contract/conformance tests.  Add the client failure
   tests and dry-run-helper test seam.  This gives `talent.py` and `talent_cli.py`
   a source of truth before their Python contract disappears, without a new CLI
   selector or dispatch path.
2. **Make cloud routing deletion-safe.** Move validation helpers into
   `cogitate_client.py`, repoint the three cloud registry values, and update the
   brain diagnostic/route tests.  Confirm the registry's required
   `run_cogitate`, `validate_key`, and `validate_model` surface before deleting
   OpenHands.
3. **Preserve the live transport wrapper.** Make only the required
   `_execute_with_tools` import/seam updates in `talents.py`; preserve its
   post-hook, provenance, event filtering, and local branch.  Move
   `QuotaExhaustedError` to `providers/shared.py` before deleting `providers/cli.py`.
   Strengthen AC7--12 tests at this point.
4. **Move independent policy data.** Create
   `deterministic_failure_caps.py`, repoint all four production consumers and
   their tests/oracle importer, move request-default ownership to the client,
   then delete `cogitate_policy.py` and the dead shared classifier branch.
5. **Migrate contract-facing UX and fixtures.** Change `talent.py` and
   `talent_cli.py` to native contract/one-shot dry-run rendering; update their
   tests.  Freeze
   `cogitate_contract.json`, remove its builder/freshness wiring, and amend
   frozen oracle provenance before deleting the Python corpus generator.
6. **Delete the SDK runtime as one coherent removal.** Delete the contract,
   OpenHands runtime, read/final tools, SDK prompt scaffold, old prompt checker,
   and all SDK-only tests.  Rewrite generic tests that patch provider modules or
   import LiteLLM exception types; remove no-longer-live code/comments rather
   than adding compatibility shims.
7. **Remove packages and record retirement.** Remove `openhands-sdk` and
   `litellm` from `pyproject.toml`, regenerate the lock, clean the extras
   consistency script and source/Makefile spellings, then add both completed
   retirement waves and the detector exclusion required for its own literal
   forbidden-root vocabulary.
8. **Update docs and static gates.** Rewrite active provider/Cortex/Think docs
   to describe native cogitate and local admission accurately; remove stale
   current-design notes or mark them historical.  Remove the prompt-check
   target, add the cutover detector and focused pytest target to
   `install-checks`, and update the cutover detector only for intentionally
   surviving native/client paths.
9. **Run the settled gates in implementation stage.** Use the focused cutover
   pytest target, the cutover detector, conversion-retirement and core-fixture
   checks, then the explicitly required full preflight.  Do not treat a passing
   static detector as a substitute for the transport/error-path tests.

### Risks to hold at the gate

- The existing `cogitate_contract.json` is falsely labelled generated and
  lacks builder parity; deleting its builder without changing freshness tests is
  required to avoid a future accidental destructive regeneration.
- The native `--talent-contract` command provides metadata but not prompt text,
  and the current one-shot dry-run event carries only terminal status.  Silently
  rebuilding prompts in Python would recreate the deleted ownership split; the
  smallest complete fix is one `rendered_prompt` object on that existing dry-run
  event.
- The scope's "all of `talents.py` is out" reading is incompatible with where
  AC7--10 actually live.  This plan confines edits to the cogitate dispatch
  adapter and explicitly leaves lifecycle/generate machinery alone.
- The conversion checker scans text, not just imports.  Completed waves will
  fail until operational comments/scripts are cleaned or a narrowly justified
  detector exclusion is present.
