# Native Sol Resident Command Lane Design

## Status

- Decision: add a resident command lane for native `sol` without changing the
  buffered `CommandOutput` lane.
- Scope: L5.8 resident-lane infrastructure and a trivial resident fixture only.
  This does not design `sol link serve`.
- Routing: this design ships no production resident arm. `run_dispatched` and the
  five `solstone-core-sol-client-cli` dispatchers remain unchanged; the fixture
  bin calls the runner directly.
- Baseline: the corrected untouched-tree baseline is green for `make ci`;
  `make check-rust-{fmt,msrv,clippy,test,ios,deny}` all pass. The only observed
  failing test is
  `tests/test_core_sdist_compile_inputs_integration.py::test_core_sdist_compile_inputs_are_required_by_real_wheel_build`,
  marked integration-only at
  `tests/test_core_sdist_compile_inputs_integration.py:47` and excluded from
  `make test` by `Makefile:452` and `Makefile:461`.

Evidence base:

- Buffered context/output:
  `core/crates/solstone-core-sol-client/src/command.rs:12-53`.
- Current generated handler type:
  `core/crates/solstone-core-sol-client/src/aggregate.rs:23` and
  `core/crates/solstone-core-sol-client/src/generated/inventory.rs:2280`.
- Current dispatcher and render path:
  `core/crates/solstone-core-sol/src/lib.rs:444-554` and
  `core/crates/solstone-core-sol/src/lib.rs:691-696`.
- Native-sol gates:
  `scripts/check_native_sol_no_python_spawn.py:20-47`,
  `scripts/build_native_sol_inventory.py:39-56`,
  `scripts/build_native_sol_inventory.py:257-268`,
  `scripts/build_native_sol_inventory.py:485-505`,
  `scripts/check_native_sol_grammar_oracle.py:52-90`,
  `scripts/check_native_sol_architecture.py:81-88`,
  `scripts/core_compile_inputs.py:174-223`.

No runtime behavior changes are made by this record.

## D0. Lane Shape

Decision: add a separate resident handler shape in
`solstone-core-sol-client`, parallel to the existing buffered `Handler`:

`pub type ResidentHandler = for<'a> fn(CommandContext<'a>) -> Result<ResidentCommand<'a>, CommandOutput>;`

`ResidentCommand<'a>` has private fields:

- `startup: String`
- `serve: Box<dyn FnOnce(&dyn ShutdownSignal) -> CommandOutput + 'a>`

It has one constructor requiring both pieces, no `Default`, and no
`From<CommandOutput>` or `Into<CommandOutput>` path. `Err(CommandOutput)` means
"refused to go on duty" - bad args, bind failure, missing resources - and is
rendered through the existing buffered path unchanged.

Rationale: `CommandOutput` is frozen as the buffered contract, and `Handler` is
generated across the existing native inventory. Widening `Handler` would force
all buffered leaves to pay for resident behavior. A `Result` is the smallest new
surface: `Ok` means resident duty started, `Err` means ordinary refusal output.
A bespoke enum adds names but no extra state or safety.

Behavior marker: `matches-python` for preserving all existing buffered output;
new resident behavior is internal infrastructure with no current Python command
counterpart.

## D1. Borrowing And Runner Lifetime

Decision: resident execution must happen inside `run_dispatched`'s frame when a
production resident command is eventually routed. This implementation adds
no such arm, so the fixture invokes the runner directly with its own context.

`run_dispatched` builds `today`, argv strings, journal path, HTTP transport,
environment map, stdin, clock, file provider, build identity, client id provider,
chat event source, and notification sink as stack locals before dispatching
(`core/crates/solstone-core-sol/src/lib.rs:458-489`). `CommandContext<'a>` is
`Copy` and borrows those locals (`core/crates/solstone-core-sol-client/src/command.rs:12-27`).
A `ResidentCommand<'a>` may therefore borrow seams safely only while the
`run_dispatched` stack frame remains alive.

The runner must not return a resident handle to a caller beyond that frame. If a
future design wants detached resident handles, those handles must be `'static`
and cannot borrow from `CommandContext<'a>`.

Behavior marker: `matches-python` for existing command lifetime; this is a Rust
ownership constraint, not a user-visible change.

## D2. Runner Home

Decision: place the resident runner in `solstone-core-sol`, beside
`render_output`. Invoke it from inside `run_dispatched` only when a future
production resident arm exists; L5.8 leaves dispatch untouched.

Rationale: `solstone-core-sol` already owns native `sol` process behavior:
`run_dispatched` constructs real host seams at
`core/crates/solstone-core-sol/src/lib.rs:466-489`, and `render_output` writes
stdout/stderr at `core/crates/solstone-core-sol/src/lib.rs:691-696`.
`docs/PORTING.md:184-195` says `solstone-core` is only the process shell while
subsystem libraries take typed parameters and own no process-global state. The
runner needs process-global stdout and Unix signal handling, so it belongs in
the native `sol` host crate, not the thin `solstone-core` binary and not the
iOS-visible client crates.

Rejected shapes:

- Put the runner in `solstone-core`: works mechanically, but violates the
  existing split where the thin binary only selects the public identity and calls
  `solstone_core_sol::run` (`core/crates/solstone-core/src/main.rs:37-42`).
- Put the runner in `solstone-core-sol-client` or
  `solstone-core-sol-client-cli`: those crates are in the iOS canary and should
  not own host signal behavior.

Behavior marker: `matches-python` for buffered output; resident output is a new
native-only lane.

## D3. Startup Line And Port Binding

Decision: the resident handler binds first, then constructs the startup line,
then moves the listener into the serve closure.

The startup line must carry the actual bound port. Tests and fixtures should use
port `0` so the OS chooses a free port, then read `TcpListener::local_addr()` to
format the startup line. Bind failure returns `Err(CommandOutput::failure(...))`
before startup output is printed.

The startup line must include its trailing newline. `ResidentCommand::new()`
does not require or append it; the fixture supplies it at
`core/crates/solstone-core-sol/src/bin/solstone-resident-fixture.rs:49`, and
the test asserts it at
`core/crates/solstone-core-sol/tests/resident.rs:50-54`. A future resident
command author must preserve that convention because `BufReader::read_line`
waits for newline or EOF.

This shape works with the borrowing rule in D1 because the listener is owned by
the resident command's serve closure, while borrowed seams stay valid until the
runner finishes inside `run_dispatched`.

Behavior marker: `expected-differs` for the test fixture versus any buffered
fixture attempt: startup output must be observable before command completion.

## D4. Signal Wiring

Decision: add a `ShutdownSignal` seam with a blocking `wait()` method in
`solstone-core-sol-client`; the real implementation lives in `solstone-core-sol`
and uses nix `SigSet` block plus `wait`.

Use nix 0.30.1's safe APIs only:

- `SigSet::thread_set_mask` is safe at
  `/opt/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/nix-0.30.1/src/sys/signal.rs:565`.
- `SigSet::thread_block` is safe at
  `/opt/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/nix-0.30.1/src/sys/signal.rs:570`.
- `SigSet::wait` is safe at
  `/opt/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/nix-0.30.1/src/sys/signal.rs:589`.
- `sigaction` and `signal` are `pub unsafe fn` at
  `/opt/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/nix-0.30.1/src/sys/signal.rs:880`
  and
  `/opt/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/nix-0.30.1/src/sys/signal.rs:942`;
  do not use them under `core/Cargo.toml:24-25` `unsafe_code = "forbid"`.

Ordering rule: the runner installs the signal mask before invoking the resident
handler. Threads inherit the creator's mask, so resident handlers must not spawn
threads before the runner has installed the mask. The trivial fixture should not
spawn threads at all.

Dependency placement: add `nix` with feature `signal` as a non-dev dependency of
`solstone-core-sol` (`core/crates/solstone-core-sol/Cargo.toml:11`). Before
this design, the client crate had `nix` only as a dev-dependency
(`core/crates/solstone-core-sol-client/Cargo.toml:16-17`), and
`solstone-core-sol` had no `nix` in its dependency table. Keeping the real
signal impl in `solstone-core-sol` avoids adding host signal behavior to the
iOS-visible client surface.

Behavior marker: `expected-differs` for handled SIGINT/SIGTERM versus default
process death. The resident lane exits cleanly with code 0 instead of dying by
signal.

## D5. No Async

Decision: do not introduce async, Tokio signal handling, or a runtime.

The serve closure blocks on `shutdown.wait()`, returns a `CommandOutput`, and
lets RAII drop the listener. This satisfies the fixture and keeps the existing
buffered leaves paying zero runtime cost. Tokio's signal feature would add
`signal-hook-registry`; `signal-hook` is not in the lock. Neither is necessary
for a single blocking resident command.

Stop condition: if an implementation cannot express the fixture as bind, print,
wait, drop, and return, stop and redesign instead of making every command async.

Behavior marker: `matches-python` for not perturbing buffered commands.

## D6. Clean Shutdown Semantics

Decision: define clean shutdown by this order:

1. The runner prints the startup line and explicitly flushes stdout.
2. The serve closure blocks in `shutdown.wait()`.
3. On SIGINT or SIGTERM, the closure drops the listener before returning.
4. The runner renders the tail `CommandOutput`.
5. The process exits with code 0.

The flush is part of the resident contract even though Rust's current `Stdout`
uses `LineWriter` (`std/src/io/stdio.rs:609` and `std/src/io/stdio.rs:717`) and
newline output reaches a pipe on this toolchain. The AC-3 discriminator is not
buffer flushing. It is structural: the buffered lane only prints after a handler
returns (`core/crates/solstone-core-sol/src/lib.rs:554` and
`core/crates/solstone-core-sol/src/lib.rs:691-696`), while a resident command is
still running after startup.

Port release proves only that no live listener owns the port. Rust sets
`SO_REUSEADDR` before Unix bind (`std/src/sys/net/connection/socket/mod.rs:551-560`),
but a probe showed a second bind over a live listener still fails with
`EADDRINUSE`. A successful re-bind after child exit therefore proves the port is
genuinely free. It does not prove graceful shutdown, because the kernel closes
descriptors on any process exit, including default signal death or SIGKILL.

Graceful shutdown is proven by exit status. On Unix,
`ExitStatus::code()` returns `None` for signal termination
(`std/src/process.rs:1949-1958`), and `ExitStatusExt::signal()` returns the
terminating signal (`std/src/os/unix/process.rs:308-312`). Tests must assert
`code() == Some(0)` and `signal() == None`.

Behavior marker: `expected-differs` against default signal disposition.

## D7. AC-6 Structural Boundary

Decision: use the type system to make the resident lane explicit and
uninhabitable by ordinary buffered handlers.

Both directions:

- A buffered handler cannot be installed into the resident lane because
  `Handler` and `ResidentHandler` are distinct function-pointer types.
  Generated `HANDLERS: &[Handler]` cannot hold a `ResidentHandler`
  (`core/crates/solstone-core-sol-client/src/generated/inventory.rs:2280`), and
  `aggregate::handler_for()` returns `Handler`
  (`core/crates/solstone-core-sol-client/src/aggregate.rs:36-40`).
- `ResidentCommand` is uninhabitable from `CommandOutput`: private fields, no
  `Default`, no `From`/`Into`, and a single constructor requiring both a startup
  line and a serve closure.

What someone would have to write to break it: deliberately change a command's
declared handler type to `Result<ResidentCommand<'_>, CommandOutput>` and call
`ResidentCommand::new(startup, closure)`. That is an explicit resident command,
not an accidental inventory migration.

What the type system does not prevent: a buffered `Handler` can still block
forever internally before returning `CommandOutput`. AC-6 does not claim to
prevent all bad blocking behavior; it prevents accidental installation of a
buffered command into the resident runner and accidental construction of a
resident command from buffered output.

Behavior marker: `matches-python` for inventory behavior; this is a structural
Rust proof.

## D8. Fixture Home

Decision: put the trivial resident fixture in a new bin target under
`solstone-core-sol`, for example
`core/crates/solstone-core-sol/src/bin/solstone-resident-fixture.rs`, with tests
in `core/crates/solstone-core-sol/tests/resident.rs` using
`env!("CARGO_BIN_EXE_solstone-resident-fixture")`.

Gate check by source shape:

- `check-native-sol-inventory`: clears. Inventory discovery scans
  `solstone/**/native/authority.toml`, skipping private app authorities
  (`scripts/build_native_sol_inventory.py:257-268`), and top-level counts are
  pinned to known surfaces (`scripts/build_native_sol_inventory.py:485-505`).
  A Cargo bin under `core/crates/solstone-core-sol` is not an authority.
- `check-native-sol-root-contract`: clears. The root contract pins bare `sol`
  stdout in `core/fixtures/native-sol/root-contract-v1.json:55`; a hidden test
  bin has no help listing and no inventory partition.
- `check-native-sol-grammar-oracle`: clears. The grammar projection includes
  only discovered entries with `surface == "sol-call"`
  (`scripts/check_native_sol_grammar_oracle.py:52-60`) and reconciles against
  frozen oracle paths (`scripts/check_native_sol_grammar_oracle.py:69-90`).
- `check-native-sol-architecture`: clears. The architecture check looks for
  mirrored app trees, shared-client vocabulary, authority adjacency, native HTTP
  ownership, and packaging excludes
  (`scripts/check_native_sol_architecture.py:81-88`). A `solstone-core-sol` test
  bin is none of those.
- `check-rust-release-manifest`: clears by script shape. It validates the
  expected `solstone_core` source and wheel artifacts from package names, not
  Cargo bin target inventory (`scripts/check_rust_release_manifest.py:376-409`).
- `check-core-sdist-compile-inputs`: clears. The compile-input closure starts
  from the root package's bin targets, then dependency crates' lib targets only
  (`scripts/core_compile_inputs.py:174-183`). Dependency crate bins are not
  walked; `_root_bin_targets` applies to `solstone-core`, not
  `solstone-core-sol` (`scripts/core_compile_inputs.py:202-216`).
- Packaging render: clears by script shape. `scripts/render_packaging.py` rewrites
  workspace package versions and lockfile member blocks
  (`scripts/render_packaging.py:220-330`); it does not enumerate Cargo bin
  targets.

Rejected homes:

- `solstone/apps/*/native/command.rs`: authority-local command code is compiled
  into the iOS-visible client crate and, if declared, enters inventory and the
  root/grammar contracts.
- `solstone-core-sol-client` or `solstone-core-sol-client-cli`: scanned by the
  no-Python-spawn gate and iOS-visible; not the place for process fixtures or
  real signal handling.
- `solstone-core-sol/src/` library-only fixture code: acceptable for the runner,
  but putting fixture-only behavior in the library public surface is unnecessary
  when a bin can exercise it.
- `solstone-core/src/main.rs` hidden argv marker: possible fallback because
  `main.rs` already recognizes `__solstone_identity=sol` and
  `__solstone_identity=solstone` before normal parsing
  (`core/crates/solstone-core/src/main.rs:37-42` and
  `core/crates/solstone-core/src/main.rs:66-72`), but worse because it ships
  fixture-only dispatch in the public process shell.
- `core/crates/solstone-core/tests/`: has the right spawn precedent, including
  ETXTBSY retry (`core/crates/solstone-core/tests/version.rs:35-54` and
  `core/crates/solstone-core/tests/version.rs:56-78`), but the fixture bin
  belongs to the `solstone-core-sol` package so its integration test should live
  there.

Spawn gate note: the forbidden spawn pattern is method-call `.spawn\s*\(`
(`scripts/check_native_sol_no_python_spawn.py:30-47`). `std::thread::spawn(...)`
does not match that regex, while `Builder::new().spawn(...)` does. This is
irrelevant for the chosen test home because `core/crates/solstone-core-sol/tests`
is outside the scan set, but it should stay visible to the implementer.

Behavior marker: no user-visible behavior change.

## D9. Test Plan

AC 2 - buffered path unchanged:

- Because L5.8 ships no production resident arm and leaves dispatch untouched,
  AC 2 is pinned with
  `buffered_usage_error_output_stays_byte_identical_without_resident_arm` at
  `core/crates/solstone-core-sol/src/lib.rs:1186-1190`. That test asserts the
  exact `stdout`, `stderr`, and `exit` for `usage_error_output()`, which is the
  representative buffered output adjacent to the new runner and proves the added
  resident lane did not perturb buffered output construction.

AC 3 - startup before completion:

- Test name should state the discriminator, for example
  `resident_startup_line_arrives_before_child_exit_unlike_buffered_output`.
- Spawn the fixture with `stdout(Stdio::piped())`; `Child.stdout` is only present
  when captured (`std/src/process.rs:234-244`), and `Stdio::piped()` requests the
  pipe (`std/src/process.rs:1550-1551`).
- Read one startup line from `ChildStdout` with `BufReader::read_line`; it reads
  until newline or EOF and blocks otherwise (`std/src/io/mod.rs:2561-2578`).
- Immediately assert `child.try_wait() == Ok(None)`, because `try_wait` is
  non-blocking and returns `Ok(None)` while the child is still running
  (`std/src/process.rs:2367-2402`).
- Then signal the child.

AC 4 - SIGINT:

- After reading the startup line and port, send SIGINT.
- Assert `status.code() == Some(0)` and `status.signal() == None`; this proves a
  handled graceful exit rather than default SIGINT death.
- After `wait()`, bind the same host/port again; this proves no live listener owns
  the port now, not that shutdown was graceful.

AC 5 - SIGTERM:

- Same as AC 4, using SIGTERM. Default death would be `signal() == Some(15)`;
  graceful handling is `code() == Some(0)` and `signal() == None`.

AC 6 - structural proof:

- `resident_fixture_is_absent_from_inventory_and_handlers_are_buffered` at
  `core/crates/solstone-core-sol/tests/resident.rs:149-166` asserts
  `aggregate::handler_for(...)` is `None` for plausible fixture paths, asserts no
  inventory entry path contains the fixture name, and passes
  `aggregate::handler_bindings()` into `assert_buffered_handler_slice`.
- The helper's `&'static [Handler]` parameter is the compile-time assertion: the
  generated binding slice must stay on the buffered `Handler` lane, so a
  resident handler binding would fail to type-check there.

## D10. Expected-Differs

Declared expected differences:

1. Resident fixture startup output is visible before process completion. This
   intentionally differs from buffered `CommandOutput`, which is rendered only
   after a handler returns.
2. Resident fixture SIGINT/SIGTERM handling exits with code 0 and no signal. This
   intentionally differs from default Unix signal death.

No expected user-visible difference is declared for existing shipped `sol`
commands. Buffered handlers, generated inventory, root help, grammar oracle,
Python compatibility sentinel, and SPL pins remain unchanged.
