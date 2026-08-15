# Native Sol Notify and Contract Retirement Design

This records the this design design for two coupled changes:

- Port top-level `sol notify` from Python compatibility to native Rust.
- Retire `sol contract` from the public `sol` surface while leaving
  `journal contract` and the contract tooling module intact.

No frozen oracle blob is changed. Generated artifacts are regenerated from the
updated generators during implementation.

## D0. Authority Entry Type

The existing `local` entry type is not the right semantic fit for notify.
Today it is the `sol-call` placeholder entry
`solstone/apps/network/native/authority.toml:31-38`, path
`["link", "observer-pause"]`, operation `link.observer-pause`, handler
`observer_pause`. Its command implementation returns a local "not yet
available" message and has no top-level routing role.

Decision:

- Add a new surface `sol-notify`.
- Add a new entry type `top-level-notify`.
- Do not reuse `local`.

Reason: notify is a first-class top-level command, like chat and import. The
inventory partition is keyed by `(surface, entry_type)`, so a new surface is
needed either way. `top-level-notify` keeps the top-level native surfaces
consistent and avoids overloading `local` with a second meaning.

Authority sketch:

- File: `solstone/think/native/notify/authority.toml`
- Source: `command.rs`
- `surface = "sol-notify"`
- `path = ["notify"]`
- `kind = "top-level"`
- `help = "Send a notification via callosum"`
- `operation_id = "notify.top_level"`
- `entry_type = "top-level-notify"`
- `handler = "notify"`
- No `method`, `route`, `contract_operation_id`, or
  `backing_contract_operation_ids`
- Params: variadic positional `message`; string options `--title`, `--icon`,
  `--event`, `--action`, `--facet`, `--app`, `--badge`; integer
  `--auto-dismiss`; flag `--no-dismiss`; flags `-v/--verbose` and `-d/--debug`

Conformance routes `top-level-notify` through `check_non_http_entry`, not
`check_top_level_backing_contracts`. That is satisfiable because the authority
must have no `method`, no `route`, no `contract_operation_id`, and no OpenAPI
backing contract.

## D1. Notification Send Seam

Add a notification-specific seam in
`core/crates/solstone-core-sol-client/src/seam.rs`:

- Trait: `NotificationSink`
- Method: `send_line(&self, line: &str) -> Result<(), NotificationSinkError>`
- Error: a small typed `NotificationSinkError` whose public meaning is
  "unavailable"; detailed I/O causes are deliberately not rendered

The seam takes the fully built JSON line, including its trailing newline. This
keeps JSON byte construction in the native notify command and lets unit and
parity tests pin the exact wire string.

Real implementation:

- File: `core/crates/solstone-core-sol/src/lib.rs`
- Type: `UnixNotificationSink`
- State: `socket_path: PathBuf`
- Construction: in `run_dispatched`, after `resolve_process_journal_path()`,
  using `journal.path.join("health").join("callosum.sock")`
- Send behavior: connect a `std::os::unix::net::UnixStream`, set write and read
  timeouts to two seconds, then `write_all(line.as_bytes())`

Timeout honesty: Python's `socket.settimeout(2.0)` covers connect and send.
Rust `UnixStream::connect` has no connect-timeout API. The native version will
set `set_write_timeout(Some(2s))` and `set_read_timeout(Some(2s))` after
connect. This bounds writes, but AF_UNIX connect can still block if the listener
backlog is saturated. The design records that as a narrow implementation
semantic difference. There is no retry, ack, read protocol, or second send
attempt.

Fake implementation:

- File: `core/crates/solstone-core-sol-client/src/seam.rs`
- Type: `RecordingNotificationSink`
- Behavior: records each line verbatim in insertion order; can be configured to
  fail with `NotificationSinkError`

Blast radius decision: put the sink on `CommandContext` as
`notification_sink: Option<&dyn NotificationSink>`, and thread it through
`DispatchSeams`. This touches the nine existing `CommandContext` construction
sites, but it preserves the generated handler contract
`for<'a> fn(CommandContext<'a>) -> CommandOutput` from
`core/crates/solstone-core-sol-client/src/aggregate.rs:23`. Keeping notify in
the generated inventory without `CommandContext` would require a second dispatch
path or an unused dummy handler.

## D2. CLI Crate Threading

In `core/crates/solstone-core-sol-client-cli/src/lib.rs`:

- Add `Outcome::Notify`.
- Extend `evaluate_args` with a `notify` arm that resolves the `sol-notify`
  authority.
- Add `dispatch_sol_notify_with_seams`, mirroring chat/import and constructing
  a `CommandContext` with `notification_sink`.

In `core/crates/solstone-core-sol/src/lib.rs`:

- Add `[command, rest @ ..] if command == OsStr::new("notify")` to the
  top-level argv match.
- Place it with `chat` and `import`, before the leading-dash arm and before the
  compatibility arm.
- Add `notify` to `run_top_level_native` help resolution so
  `sol notify --help` uses native help.
- In `run_dispatched`, build `UnixNotificationSink` from the already resolved
  journal path and pass it through `DispatchSeams`.

Argv convention: notify matches chat, not import. `run_top_level_native` passes
`command_args` to `run_dispatched`, so the notify dispatcher receives args
after `notify` only. Parity should therefore call `dispatch_sol_notify_with_seams`
with vector `argv` as args-only, with no `skip(1)`.

## D3. Argument Parsing and Emission

The native parser is hand-rolled like the existing native commands and must
match the Python argparse surface.

Accepted syntax:

- Positional `message`: one or more values, joined with single spaces.
- String options: `--title`, `--icon`, `--event`, `--action`, `--facet`,
  `--app`, `--badge`.
- Value options support both `--name value` and `--name=value`, matching
  argparse.
- `--auto-dismiss N`: parses as integer and emits JSON key `autoDismiss` as a
  number.
- `--no-dismiss`: emits `dismissible: false`; native never emits
  `dismissible: true`.
- `-v/--verbose` and `-d/--debug`: accepted and ignored. Python
  `setup_cli()` only changes logging/setup behavior for this command; notify has
  no observable verbose/debug output to preserve.
- `-h/--help`: stdout is the byte-exact P1 help text, exit 0, stderr empty.

Absent options produce absent JSON keys, not nulls.

Emission key order must reproduce Python's insertion order from
`solstone/think/notify_cli.py`:

1. `tract`
2. `event`
3. `message`
4. `title`
5. `icon`
6. `action`
7. `facet`
8. `app`
9. `badge`
10. `autoDismiss`
11. `dismissible`

`--event` defaults to `show` and is emitted as the second key. It is not part of
the optional fields map.

JSON formatting must reproduce Python `json.dumps` defaults used by
`callosum_send`: comma-space separators, colon-space separators, ASCII escaping,
and one trailing newline. A compact `serde_json::to_string` output is not byte
compatible.

Malformed args:

- Missing message: exit 2.
- Unknown flag: exit 2.
- Non-integer `--auto-dismiss`: exit 2.

The error shape follows native chat: stdout empty, stderr is HELP followed by
`sol notify: error: ...\n`. The message text should match argparse wording for
the pinned malformed vectors.

## D4. Failure Collapse and CommandOutput

Every sink send failure collapses to the same user-visible result:

- stdout: empty
- stderr: `Failed to send notification (is callosum running?)\n`
- exit: 1

This includes no socket file, connection refused, timeout, partial write, and
any platform/path I/O failure. The real sink accepts a `PathBuf`, so non-UTF-8
journal paths are not inherently errors on Unix. The sink itself does not
resolve the journal; journal resolution remains in `run_dispatched`.

Successful send:

- stdout: empty
- stderr: `Notification sent\n`
- exit: 0

`CommandOutput` supports this, but not via its helpers. The struct has explicit
`stdout`, `stderr`, and `exit` fields. `CommandOutput::success(stdout)` always
sets empty stderr and exit 0, while `CommandOutput::failure(stderr, exit)` sets
empty stdout. Notify success must therefore construct `CommandOutput` directly.

## D5. `require_solstone()` Divergence

Native notify deliberately does not reproduce Python's convey-port TCP probe.
This is a narrow behavior change.

| State | Python today | Native after port |
| --- | --- | --- |
| Success | stdout empty; stderr `Notification sent\n`; exit 0 | same |
| Down stack before send | stdout empty; stderr `sol: solstone isn't running. Start it with 'journal up' and retry.\n`; exit 1 | stdout empty; stderr `Failed to send notification (is callosum running?)\n`; exit 1 |
| Supervisor-spawned down stack | stdout empty; stderr empty; exit 75 | stdout empty; stderr `Failed to send notification (is callosum running?)\n`; exit 1 if journal resolves and socket send fails |
| Unconfigured journal | Python setup/journal failure shape before probe | existing `run_dispatched` shape: stdout empty; stderr `native sol journal resolution failed: {error}\n`; exit 75 |

This avoids a second availability check and treats callosum socket delivery as
the only runtime dependency for notify.

## D6. Parity Coverage Strategy

Use option (a): extend the parity harness. Coverage will add notify to
`required_dispatch`, so a non-HTTP parity path is required.

Changes:

- Add `sol-notify` branching to
  `core/crates/solstone-core-sol-client-cli/tests/parity.rs`.
- Add `sol-notify => ["notify"]` mapping to
  `core/crates/solstone-core-sol-client-cli/src/bin/resolve_parity_leaves.rs`.
- Add `core/fixtures/native-sol/parity/notify.jsonl`.
- Add `required_top_level_notify` to `scripts/check_native_sol_coverage.py`.

Vector schema addition:

- New optional key under `expected`: `notifications`.
- Shape: array of strings.
- Each string is one exact line passed to `NotificationSink::send_line`,
  including the trailing `\n`.
- Existing vectors that omit `expected.notifications` mean an expected empty
  notification list. The harness should not require editing all old vectors.
- If a vector omits `expected.notifications` but the fake records any lines,
  the vector fails.

Notify vectors to include:

- Help vector: proves exact P1 help on stdout, exit 0, no send.
- Minimal success vector: proves stdout empty, stderr `Notification sent\n`,
  exit 0, and one notification line with `tract = "notification"`,
  `event = "show"`, message joined from positional words, one JSON object, and a
  single trailing newline.
- Full-options success vector: proves optional key order, `autoDismiss` as a
  number, and `dismissible` only as false.
- Failure vector: fake sink failure proves the collapsed stderr/exit.
- Malformed-args vector: proves exit 2 argparse-style errors.

Coverage should treat notify as non-HTTP. It cannot use `requests` for
request-binding. Instead, notify coverage requires parity success/failure and at
least one non-empty `expected.notifications` binding for `notify.top_level`.

## D7. Root-Contract Retirement Mechanism

`scripts/build_native_sol_root_contract.py` needs the smallest explicit filter.

Add module-level `RETIRED_ACCESS_COMMANDS = frozenset({"contract"})`.

Add filtering after AST extraction of `ACCESS_HELP_GROUPS`, before
`render_stdout()` is used:

- `access_groups()` still extracts the frozen oracle exactly as today.
- A new filter checks that every retired command is present in the extracted
  oracle before filtering.
- If a retired command is absent, error. This prevents the filter from silently
  rotting after the frozen oracle changes.
- Remove retired commands from their groups.
- If filtering empties a group, error rather than dropping the group. An empty
  group would mean the oracle structure changed enough that the retire list
  needs review.

Slot in `build()`: after `groups = access_groups(tree, names)` and before
`apps = call_groups()` / `render_stdout(...)`.

Do not touch:

- Frozen commit/path/blob constants.
- Blob hash verification.
- The frozen blob itself.
- `core/fixtures/native-sol/root-contract-v1.json` by hand.

After filtering `contract`, the Tools group still contains sibling-owned
`skills` and `link`, so the group survives. This remains a sibling-contention
area if the skills/link sessions land nearby changes.

## D8. Contract Provenance String

Use `python -m solstone.think.contract_cli build` as the replacement. It matches
`Makefile:826` and works from a packaged wheel without a repository root.

Exact replacements:

| File | Before | After |
| --- | --- | --- |
| `solstone/think/contract/journal.py:130` | `sol contract build` | `python -m solstone.think.contract_cli build` |
| `solstone/think/contract/journal.py:133` | ``regenerate with `sol contract build`.`` | ``regenerate with `python -m solstone.think.contract_cli build`.`` |
| `solstone/think/contract/journal.py:157` | stale bundle message ending ``run `sol contract build``` | same message ending ``run `python -m solstone.think.contract_cli build``` |
| `solstone/think/contract_cli.py:35` | missing bundle message ending ``run `sol contract build``` | same message ending ``run `python -m solstone.think.contract_cli build``` |

`solstone/talent/journal/contract/bundle.json` is generated and should change
through `make contract`, not by hand. The historical design reference in
`docs/design/native-sol-client/06-cutover-design.md:287` stays unchanged.

Do not touch the OpenAPI contract-route strings in `Makefile:561` or
`scripts/check_native_sol_contract_routes.py:140,145`; they are unrelated to the
retired `sol contract` command.

## D9. Python Notify Deletion and Test Repointing

Delete `solstone/think/notify_cli.py`.

Remove notify and contract from the finite top-level compatibility sets:

- `solstone/think/sol_compat_inventory.py`
- `core/crates/solstone-core-sol/src/lib.rs::TOP_LEVEL_COMPAT_COMMANDS`

Do not add a third compat-set expression, and do not touch
`sol_compat_cli.py`, the sentinel, the recursion guard, or
`solstone/think/sol_cli.py`.

Repoint tests to `check` as the surviving compatibility exemplar. Avoid
`skills` and `link` because sibling sessions own those areas.

Exact repoints:

- `core/crates/solstone-core/tests/version.rs:393`: change compat invocation
  from `notify message` to `check message`.
- `core/crates/solstone-core/tests/version.rs:411`: update asserted argv from
  `<notify><message>` to `<check><message>`.
- `scripts/check_access_imports_clean.py`: use
  `solstone.think.check` and `sol check --help [solstone.think.check]`.
- `tests/test_sol_compat_cli.py:183`: use bare command string `check`.
- `tests/test_sol.py:286,290`: use existing module `solstone.think.check` for
  the patched `import_module` failure path.

Access-import cleanliness:

- Add `("sol notify --help", ["sol", "notify", "--help"])` to
  `NATIVE_CASES` in `scripts/check_access_imports_clean.py:68`.
- Delete the now-dead `contract` exclusion comment and filter at
  `scripts/check_access_imports_clean.py:93-97`.

`solstone.egg-info/SOURCES.txt` is not tracked in this worktree, so deleting
`notify_cli.py` has no tracked egg-info edit.

## D10. Test Placement

Native notify command tests:

- File: `solstone/think/native/notify/command.rs`
- Assertions: parser behavior, exact help output, JSON key order and formatting,
  absent optional keys, `autoDismiss` numeric emission, `dismissible: false`,
  ignored verbose/debug flags, malformed arg exit 2, failing sink collapse, and
  success stderr with exit 0.

Parity tests:

- File: `core/crates/solstone-core-sol-client-cli/tests/parity.rs`
- Fixtures: `core/fixtures/native-sol/parity/notify.jsonl`
- Assertions: help text, wire bytes, success/failure stdout/stderr/exit, and
  malformed args.

Real Unix socket tests:

- File: `core/crates/solstone-core-sol/src/lib.rs`, next to
  `UnixNotificationSink`.
- Cases: no socket file and accept-then-close/reject with a local
  `UnixListener` under a temp directory.
- Use `#[cfg(unix)]` for the real socket tests.
- Use `std::thread::spawn(...)` if concurrency is needed for accept-then-close;
  avoid `.spawn(` and avoid a helper named `output()`.

These tests do not violate the no-Python-spawn patterns: notify command source
contains no process spawning, the real sink test is in the core sol shell crate,
and `std::thread::spawn(...)` is not the forbidden `.spawn(` process pattern.
They also fit the unit-test rail: AF_UNIX sockets under a temp dir are local IPC,
not live network or service dependencies.

## D11. Implementation Order

1. Add native notify authority and command skeleton, plus the
   `NotificationSink` seam, `CommandContext` field, and dispatch threading.
   Update all nine `CommandContext` construction sites with
   `notification_sink: None` unless notify tests pass a fake.

2. Extend native inventory gates for `sol-notify` / `top-level-notify`:
   `ENTRY_TYPES`, allowed surfaces, final totals, top-level partition map,
   conformance entry-type dispatch, architecture surface recognition if needed,
   and coverage's required top-level notify set.

3. Run `make build-native-sol-inventory` during implementation. Generated
   artifact: `core/crates/solstone-core-sol-client/src/generated/inventory.rs`.
   This is the point where inventory/conformance/architecture can be made
   structurally green again.

4. Wire native top-level `notify` in `core/crates/solstone-core-sol/src/lib.rs`
   before the leading-dash and compatibility arms. Build the real sink from the
   resolved journal path in `run_dispatched`.

5. Extend parity for non-HTTP notification side effects: harness branch,
   resolver mapping, `expected.notifications` schema, and
   `core/fixtures/native-sol/parity/notify.jsonl`. This is where coverage for
   `notify.top_level` re-greens.

6. Add the root-contract retire filter and remove `contract` from the public
   compat sets. Run `make build-native-sol-root-contract`. Generated artifact:
   `core/fixtures/native-sol/root-contract-v1.json`. This re-greens the
   root-contract oracle after `contract` disappears from bare `sol` output.

7. Delete `solstone/think/notify_cli.py`, remove `notify` from the compat sets,
   repoint the five test sites to `check`, add the native `sol notify --help`
   access-import case, and remove the dead contract exclusion in
   `scripts/check_access_imports_clean.py`.

8. Replace the four contract provenance command strings with
   `python -m solstone.think.contract_cli build`.

9. Run `make contract`. Generated artifact:
   `solstone/talent/journal/contract/bundle.json`.

10. Final implementation validation should use the requested narrow gates,
    including the native inventory, coverage, conformance, root-contract,
    compat, no-python-spawn, architecture, contract, and access-import checks.

## Risks and Open Questions

- JSON byte parity is the highest-risk detail. Python `json.dumps` defaults are
  not the same as `serde_json::to_string`; native notify needs a deliberate
  serializer or formatting path.
- Rust AF_UNIX connect timeout cannot exactly match Python's socket timeout.
  The documented native behavior bounds writes, not connect.
- Adding `notification_sink` to `CommandContext` is a nine-site mechanical edit,
  but it keeps the generated inventory handler model intact.
- The root-contract Tools group still contains sibling-owned `skills` and
  `link`; rebase and regenerate if either sibling session lands nearby
  inventory/root-contract changes first.

## C1 Implementation Note

Argparse was checked with `prog = "sol notify"` and `COLUMNS=80` for malformed
arguments. Python emits the usage block only, not full help:

- Missing message: stdout empty, stderr is the usage block followed by
  `sol notify: error: the following arguments are required: message\n`, exit 2.
- Unknown flag `--bogus`: stdout empty, stderr is the usage block followed by
  `sol notify: error: unrecognized arguments: --bogus\n`, exit 2.
- Bad `--auto-dismiss nope`: stdout empty, stderr is the usage block followed
  by `sol notify: error: argument --auto-dismiss: invalid int value: 'nope'\n`,
  exit 2.

Native `sol chat` and `sol import` intentionally diverge from argparse here:
their `argparse_error()` helpers emit the full command `HELP` text followed by
`sol <command>: error: ...\n`. Native notify follows that existing native
surface convention, and the parity malformed-argument vector pins the full-help
shape rather than Python's usage-only shape.
