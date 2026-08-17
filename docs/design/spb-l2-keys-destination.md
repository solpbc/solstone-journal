# SPB L2 keys and destination

## Gate verdict

The L2 design is technically feasible with no blocking issue found. The only
gate decision points to carry forward are:

- Recovery-key confirmation deliberately accepts Crockford lookalikes
  (`O -> 0`, `I/L -> 1`) and ignores non-alphabet grouping/noise. This is a
  usability expansion beyond literal case/whitespace matching.
- `restic cat config` on 0.19.0 does not surface normal shared repository locks:
  it returned 0 while a `backup --stdin` process held a lock. Mapping returncode
  11 to `locked` is still correct and harmless because restic emits 11 for
  commands that contend for exclusive locks, but tests should not require local
  `cat config` to produce 11.
- Native B2 is supported by restic and is feasible, but the restic 0.19.0 docs
  recommend B2's S3-compatible API for better behavior. Keeping native `b2` is a
  product choice, not a technical blocker.

External references used for destination validation:

- Restic 0.19.0 repository/password automation, S3, MinIO, S3-compatible, and
  B2 examples: https://restic.readthedocs.io/en/stable/030_preparing_a_new_repo.html
- Cloudflare R2 S3 endpoint and access-key model:
  https://developers.cloudflare.com/r2/get-started/s3/

## Module layout

Add four new modules under `solstone/think/backup/`:

- `keys.py`: pure crypto/string helpers. No restic, no journal config imports.
- `destination.py`: destination model, backend env assembly, sanitized read-only
  reachability probe.
- `repo.py`: repository initialization and recovery-key installation via restic.
- `state.py`: config-section accessors using only `solstone.think.journal_config`
  helpers for reads/writes/locking.

Also make these minimal edits:

- `runner.py`: add optional `pass_fds` support and thread it to
  `subprocess.run`.
- `journal_default.json`: add the top-level `backup` schema.

No CLI, UI, scheduler, execution loop, backup command, or sol-pbc service contact
is in this change.

## Durable contracts

### Recovery keys

Canonical recovery key:

- Exactly 64 uppercase Crockford characters.
- Alphabet source: `from solstone.apps.network.crockford32 import ALPHABET`.
- Generated as `secrets.choice(ALPHABET)` per character.
- 64 characters * 5 bits per Crockford character = 320 bits.
- Persist the canonical form.

Display recovery key:

- 16 groups of 4 canonical characters.
- Groups are joined by one space.
- `format_recovery_key_display(canonical) -> str` is the only formatter.

Normalization and confirmation:

- `normalize_recovery_key(value)` uppercases, folds `I` and `L` to `1`, folds
  `O` to `0`, then drops characters not in `ALPHABET`.
- `confirm_recovery_key(candidate, canonical)` compares normalized strings for
  exact equality.
- Do not use constant-time comparison. The key is owner-entered local setup
  material, not an online oracle.
- Do not import `crockford32._normalize_char`; keep the recovery-key normalizer
  self-contained.

False-accept proof for lookalike folding:

- Canonical keys are drawn only from `ALPHABET`, which excludes `I`, `L`, `O`,
  and `U`.
- Folding `I/L` can only produce `1`; folding `O` can only produce `0`.
- Because canonical keys never contain the folded source characters, two
  different canonical keys cannot collapse to the same normalized form through
  these folds.
- A candidate with a folded character is accepted only when it normalizes to the
  exact canonical key. If it differs at any real canonical position, equality
  fails.

Alphabet import validation:

- `solstone/apps/network/crockford32.py` imports no solstone modules. It defines
  `ALPHABET` at line 8 and depends only on local constants/functions, so importing
  the constant from `keys.py` cannot create a cycle.
- The same think-to-apps edge already exists in
  `solstone/think/link/join_cli.py:49`.
- Factoring one constant into a new think module would broaden this change.

### Backup config

Add this top-level `backup` section to `core/fixtures/journal_default.json`:

- `enabled`: `false`
- `mode`: `"byo"`
- `destination`: `repository: null`, `backend: null`, `credentials: {}`
- `daily_key`: `null`
- `recovery_key`: `null`
- `confirmed_recovery_key`: `false`
- `retention`: `hourly: 24`, `daily: 7`, `weekly: 4`, `monthly: 12`
- `schedule`: `every: "daily"`, `enabled: false`
- `last_backup`: `time: null`, `snapshot_id: null`, `status: null`,
  `error_reason: null`

`last_backup` is schema-only in L2. Do not add an unused update setter.

Existing journals do not get default merges. `state.py` readers must default
per field when reading an existing partial `journal.json`.

Power-user raw-password escape:

- `generate_and_store_keys()` is get-or-create.
- If `backup.daily_key` is non-null, respect it exactly and do not regenerate.
- If `backup.recovery_key` is non-null, respect it exactly and do not regenerate.
- If missing, generate `daily_key` with `secrets.token_urlsafe(32)` and
  `recovery_key` with the canonical Crockford generator.
- Detection rule is simple non-null presence in config.

### Destination config

`Destination(repository: str, backend: str, credentials: dict[str, str])`
contains:

- `repository`: credential-free restic repository string, passed directly as
  `RESTIC_REPOSITORY`.
- `backend`: discriminator, one of `"s3"` or `"b2"`.
- `credentials`: backend-specific secrets, passed only through environment.

Credential env assembly:

- `s3`: `AWS_ACCESS_KEY_ID` from `access_key_id`, `AWS_SECRET_ACCESS_KEY` from
  `secret_access_key`.
- `b2`: `B2_ACCOUNT_ID` from `account_id`, `B2_ACCOUNT_KEY` from `account_key`.
- Unknown backend or missing required credential fields raises a clear boundary
  error.

Canonical credential-free repository examples:

- AWS S3: `s3:s3.us-east-1.amazonaws.com/bucket_name/path/to/repo`
- Cloudflare R2: `s3:https://<ACCOUNT_ID>.r2.cloudflarestorage.com/bucket_name/path`
- MinIO or S3-compatible: `s3:http://localhost:9000/bucket_name/path`
- Native Backblaze B2: `b2:bucketname:path/to/repo`

Do not embed access keys, secret keys, session tokens, or presigned URLs in the
repository string.

## Restic invocation model

`run_restic` remains the only restic invocation path in source code.

Runner extension:

- Add `pass_fds: tuple[int, ...] = ()` to `run_restic`.
- Thread it to `subprocess.run(..., pass_fds=pass_fds)`.
- Default empty tuple preserves all existing callers.
- Python subprocess supports `pass_fds` with `close_fds=True`; supplying
  `pass_fds` forces `close_fds` behavior for every other descriptor. This matches
  the desired pipe-only inheritance model.

Two-key install:

- Initialize with the daily key as `RESTIC_PASSWORD` through `run_restic`.
- Add the recovery key with `restic key add --new-password-file /dev/fd/<N>`.
- Existing repo unlock for `key add` still uses daily key in `RESTIC_PASSWORD`.
- The recovery key is delivered via an `os.pipe()` read end passed with
  `pass_fds`.

Pipe protocol:

- Write `recovery_key + "\n"` to the pipe write end.
- The payload is about 65 bytes, far below Linux pipe capacity and POSIX
  `PIPE_BUF`, so the pre-spawn write is atomic and cannot fill the pipe.
- Close the write end before spawning restic.
- Spawn `run_restic(..., pass_fds=(read_fd,))`.
- Restic opens `/dev/fd/<read_fd>`, reads the buffered bytes, sees EOF, and
  exits.
- Parent closes all pipe fds in `finally`.
- No writer thread is needed.

Guard/scrub interaction:

- The recovery key never appears in argv. Argv contains only `/dev/fd/<N>`.
- The recovery key is not the `password` argument and is not in `backend_env`, so
  current scrub logic will not scrub it.
- Empirical 0.19.0 `key add` does not echo the new password.
- Do not add an extra scrub-secrets parameter in L2. If future restic output ever
  echoes the new password, adding extra scrub secrets is a small runner extension.

## Repository init state machine

The repo itself is the source of truth. There is no persisted initialized flag.

`init_repository(destination, daily_key, recovery_key, restic_path, timeout=None)`
does:

1. `validate_destination(destination, daily_key)`.
2. If `repo_missing`, run `restic init` with the daily key, add recovery key, then
   verify recovery unlocks.
3. If `repo_exists`, probe with the recovery key.
4. If recovery also returns `repo_exists`, no-op.
5. If recovery returns `auth_failed`, add the recovery key with daily unlock, then
   verify recovery unlocks.
6. If daily returns `auth_failed`, raise conflict: repo exists but our daily key
   does not unlock it. Do not init or overwrite.
7. If status is `locked`, `timeout`, or `unreachable`, raise or return a clear
   setup error. Do not init.

Partial-failure coverage:

- Init succeeds, recovery add fails: rerun sees daily unlock and recovery
  `auth_failed`, then adds recovery.
- Recovery add succeeds, final verify is interrupted: rerun sees both keys unlock
  and no-ops.
- Destination existed before setup with another password: daily probe returns
  `auth_failed`; code raises conflict and never reinitializes.
- Re-init is avoided because restic returns 1 on an already initialized repo.

## Sanitized destination validation

`validate_destination` runs `restic cat config` through `run_restic` and never
returns or logs raw stdout/stderr.

Return dataclass:

- `DestinationStatus(reachable: bool, repo_exists: bool, reason_code: str, message: str)`

Reason-code mapping:

- `0`: `repo_exists`, reachable true, repo_exists true,
  message `backup repository is reachable`
- `10`: `repo_missing`, reachable true, repo_exists false,
  message `backup destination is reachable and needs setup`
- `12`: `auth_failed`, reachable true, repo_exists true,
  message `repository password was rejected`
- `11`: `locked`, reachable true, repo_exists true,
  message `repository is locked; try again shortly`
- `124`: `timeout`, reachable false, repo_exists false,
  message `could not reach the backup destination`
- Any other nonzero: `unreachable`, reachable false, repo_exists false,
  message `could not reach the backup destination`

Logging, if any, is limited to returncode and reason code.

## Function and signature inventory

### `keys.py`

- `generate_daily_key() -> str`
  Pure secret generator. No journal mutation.
- `generate_recovery_key() -> str`
  Pure canonical recovery-key generator. No journal mutation.
- `format_recovery_key_display(canonical: str) -> str`
  Read/format helper. Validates canonical length/alphabet.
- `normalize_recovery_key(value: str) -> str`
  Read/parse helper. Applies folding and grouping/noise removal.
- `confirm_recovery_key(candidate: str, canonical: str) -> bool`
  Read/compare helper.

### `destination.py`

- `Destination(repository: str, backend: str, credentials: dict[str, str])`
- `DestinationStatus(reachable: bool, repo_exists: bool, reason_code: str, message: str)`
- `assemble_backend_env(destination: Destination) -> dict[str, str]`
  Read/assembly helper. No journal mutation.
- `validate_destination(destination: Destination, password: str, *, restic_path: Path, timeout: float | None = None) -> DestinationStatus`
  Read-only restic probe. Sanitizes all output.

### `repo.py`

- `init_repository(destination: Destination, *, daily_key: str, recovery_key: str, restic_path: Path, timeout: float | None = None) -> None`
  Write verb. May initialize remote repo and add restic key.
- `_add_recovery_key(destination: Destination, *, daily_key: str, recovery_key: str, restic_path: Path, timeout: float | None = None) -> None`
  Internal write helper for `restic key add` via pipe-FD.
- `_verify_recovery_key(destination: Destination, *, recovery_key: str, restic_path: Path, timeout: float | None = None) -> None`
  Internal read/guard helper. Raises on failed verification.

### `state.py`

- `BackupKeys(daily_key: str, recovery_key: str, recovery_key_display: str)`
- `get_backup_config() -> dict[str, Any]`
  Read accessor with per-field defaults.
- `get_destination() -> Destination | None`
  Read accessor. Returns `None` until repository/backend are set.
- `get_keys() -> BackupKeys | None`
  Read accessor. Returns `None` until both keys are present.
- `generate_and_store_keys() -> BackupKeys`
  Write accessor. Uses `hold_config_lock`, preserves non-null existing keys.
- `set_destination(destination: Destination) -> None`
  Write accessor. Uses `hold_config_lock`.
- `set_recovery_key_confirmed(confirmed: bool = True) -> None`
  Write accessor. Uses `hold_config_lock`.
- `status_view() -> dict[str, Any]`
  Read accessor. Redacted owner-safe state only.

### `runner.py`

- `run_restic(..., pass_fds: tuple[int, ...] = ()) -> ResticResult`
  Existing behavior with optional FD inheritance for pipe-backed password files.

Read/write naming:

- Read/no journal mutation: `get_`, `validate_`, `assemble_`, `format_`,
  `normalize_`, `confirm_`.
- Write/mutating: `generate_and_store_`, `set_`, `init_`, `_add_`.

## Redacted status view

`status_view()` returns only:

- `enabled`
- `mode`
- `destination`: `repository`, `backend`, `credentials_set`
- `daily_key_set`
- `recovery_key_set`
- `recovery_key_confirmed`
- `retention`
- `schedule`
- `last_backup`

No `daily_key`, `recovery_key`, or credential values are present. Credentials
collapse to a boolean. Repository is shown because it is owner destination
identity, not the credential channel.

## Test plan

Add focused unit tests:

- `tests/test_backup_keys.py`
  - canonical key length/alphabet.
  - display grouping is 16 groups of 4.
  - normalization accepts spaces, hyphens, case, and lookalikes.
  - lookalike folds do not collapse different canonical keys.
  - invalid canonical formatting raises loudly.
- `tests/test_backup_destination.py`
  - s3 env assembly.
  - b2 env assembly.
  - unknown backend and missing credentials fail loudly.
  - `validate_destination` maps returncodes 0, 10, 11, 12, 124, and unknown.
  - raw restic stderr/stdout never appears in returned status or logs.
- `tests/test_backup_state.py`
  - default backup section materializes by accessor when absent.
  - existing partial `journal.json` gets per-field defaults on read.
  - setters use `write_journal_config` under config lock.
  - `generate_and_store_keys` preserves hand-set daily key.
  - status view contains no key or credential values.
- `tests/test_backup_runner.py`
  - pass_fds default is `()`.
  - pass_fds is threaded to monkeypatched `subprocess.run`.
  - existing scrub/argv/env tests remain unchanged.
- Optional local integration test, skipped unless `shutil.which("restic")` exists:
  - initialize a local repo.
  - add recovery key through pipe-FD.
  - prove daily and recovery keys independently unlock.
  - No network required.

The existing host-restic integration idiom can be mirrored, but all behavior
that depends on 0.19.0 semantics should use the vendored path in implementation
tests where practical.

## Hygiene checklist

- SPDX headers and `from __future__ import annotations` on new Python files.
- `state.py` imports `journal_config` helpers only; no `journal_io` primitives.
- Config writes go through `write_journal_config`.
- Read-modify-write setters hold `hold_config_lock`.
- No `os.replace`, temp `.replace`, or custom atomic write code in backup state.
- No layer-hygiene allowlist edits expected.
- No sol-pbc endpoint, support endpoint, portal endpoint, or relay endpoint.
- Restic raw stderr/stdout is never owner-visible from destination validation.
- Recovery key and backend credentials never appear in argv.

## Implementation sequence

1. Extend `runner.py` with `pass_fds` and tests.
2. Add `keys.py` and key tests.
3. Add `destination.py` and sanitized mapping tests.
4. Add `state.py`, default schema, and redaction/config tests.
5. Add `repo.py` and idempotency tests using monkeypatched
   `validate_destination`/`run_restic`.
6. Add optional local restic integration test.
7. Run focused tests, then `make ci` before commit.

## Irreversible owner-facing commitments

Once an owner records recovery keys or initializes a repository:

- The recovery key is a durable master key. Losing both daily and recovery keys
  makes the backup data irrecoverable.
- The canonical recovery key written to paper must continue to unlock the repo.
  Silent regeneration is forbidden.
- Repository initialization writes real restic config/key material into the
  owner-selected destination. Reinitializing the same path is not a repair path.
- The daily key in config is part of the repo key set. Hand-editing it later does
  not change the repository key; it only changes what solstone will try.
- The selected repository string points at external owner storage. Deleting or
  reusing that path outside solstone can destroy or conflict with backup state.
- The repo format/chunker parameters are fixed by restic at init and become part
  of future compatibility expectations.
