# Logging

How the journal decides what to emit, at what level, and through which mechanism. Read this before
adding a new diagnostic, before choosing a level for one, or before reaching for `println!`/`eprintln!`.

## The one question that decides everything else

**Is this a program's output, or a diagnostic?**

- **Program output** — the thing the command was invoked to produce. A `--json` body, a human-readable
  status line, a `--help` banner, a structured line one process sends another over stdin/stdout as an
  RPC/IPC contract. This is unconditional and belongs on stdout/stderr via `println!`/`eprintln!` (or
  `print!`/`eprint!` for output built without a trailing newline). **Never route it through `log`** — a
  level filter would let an owner or a calling process silently lose the thing they ran the command to
  get, and RUST_LOG is not part of any command's documented contract.
- **Diagnostic** — an observation *about* the run, useful to someone debugging or operating the system,
  that no caller depends on parsing. Belongs in the `log` crate at a considered level (see below).

Getting this wrong in either direction is a real defect, not a style nit:
- Converting output to `log` breaks the command for every caller that reads stdout/stderr today —
  including a repository-contracts fixture or `--contract` vector. **A failing contract fixture after a
  logging change is not the fixture's fault — it is telling you the call site is output, not a
  diagnostic. Do not edit the fixture to match; put the call site back.**
- Leaving a diagnostic as a raw print makes it permanently unfilterable and unlevelled — it shows up
  identically whether the journal is calm or on fire, and it can never be turned down without deleting
  the line.

### Signals that a call site is output, not a diagnostic

- The caller is another process, not a human glancing at a terminal: a child binary's stdout/stderr is
  read by its parent as an RPC contract (request/response line, structured error line, exit-code
  discriminator). Several native worker binaries (`solstone-core-speakers-analyze`,
  `solstone-core-vad-analyze`, `solstone-core-depict`) speak exactly this shape — every line on stdout
  or stderr is part of the protocol the supervisor parses, not a log line, even though it looks like a
  plain `eprintln!` in the source.
- The message is behind an explicit CLI flag the person invoking the command controls directly (a
  `--verbose`/`--debug` banner gated on the command's own argument, not on `RUST_LOG`). Its presence is
  part of what that flag promises; routing it through `log` decouples it from the flag the operator
  actually set.
- It is emitted from inside a panic/unwind handler. Crash-time code should not depend on logger state —
  keep it as a raw `eprintln!` right up to `resume_unwind`.
- A build script's `println!("cargo:...")` — this is the Cargo build-script protocol, not logging, ever.

## Levels

One sentence each, aimed at *this* codebase, not a generic rubric:

- **`error!`** — the operation the caller asked for did not happen and nothing downstream can route
  around it. Reserved for the boundary that actually gives up, not every layer an error bubbles through
  on its way there. Don't log the same failure at every level of its propagation.
- **`warn!`** — something degraded, was skipped, or took a fallback path, but the operation as a whole
  still completed or is still safe to continue. A `warn!` should be a genuine "something is worth an
  operator's attention," not routine control flow that happens to be the less-common branch.
- **`info!`** — a notable, low-frequency lifecycle event: a service started or stopped, a
  configuration was resolved, a batch job began or finished with a one-line summary. Should read like a
  short narration of what changed, sparse enough that an operator can scan a day of `info!` output at a
  glance. If it would fire more than a handful of times a minute in steady state, it's `debug!`, not
  `info!`.
- **`debug!`** — the detail a person actually debugging this component would want: an intermediate
  decision, a retry attempt, a per-item outcome in a batch, a resolved path or endpoint. **`debug!` runs
  in production right now** (see below) — write it as a permanent citizen of the log stream, not a
  temporary trace you'll delete later.
- **`trace!`** — effectively unused in this codebase today (see Audit below). Per-iteration/hot-path
  detail that would be actively harmful at `debug!` volume. Reach for it before making `debug!` do that
  job; don't invent a new `debug!` firing thousands of times a second.

## Two invariants that hold at every level

- **Never log raw owner content.** Transcript text, screen text, audio, and anything derived closely
  enough from them to reconstruct owner activity does not appear in a log line at any level, including
  `debug!`. When a diagnostic must say *something* about content it can't print, mask it structurally —
  the pattern used to debug the SPP tool-call format (`shape=<xxxxxxxx=xxxx_xxxxx>`, every alphanumeric
  replaced so punctuation/structure survives and content cannot) is the model: enough shape to diagnose,
  nothing an owner would not want logged.
- **A diagnostic is not a substitute for a `Result`.** Don't log-and-continue where the correct contract
  is to return an error the caller must handle; don't swallow an error into a bare `warn!`/`info!` with
  no detail when the underlying error type is available and would cost nothing extra to include.

## Mechanism: `log`, not `tracing`

This codebase has one active logging framework: **`log`**, wired to `env_logger` via `install_logger()`
in `solstone-core`'s entry point (`Builder::from_env(Env::default().default_filter_or("warn"))`).
`RUST_LOG` is the control; `warn` is the compiled-in default when it is unset.

`tracing` appeared in exactly one crate (`solstone-core-support-portal`) with no `tracing_subscriber` or
`log`-bridge (`tracing-log`/`LogTracer`) installed anywhere in the workspace. **Those calls emitted
nothing at all, at any `RUST_LOG` setting** — not a wrong level, a silent void. This was not a style
inconsistency; it was a production defect the "two frameworks" framing had been treating as a
preference question. Resolved by converting the three call sites to `log` and dropping the crate's
`tracing` dependency. Unless a future need is concrete enough to justify installing a real `tracing`
subscriber (structured spans, async-aware context) workspace-wide, new code uses `log`.

### A binary must install its own logger — nothing does it for you

`log::error!`/`warn!`/`info!`/`debug!` are no-ops until something calls a logger initializer in that
*process's* `main()`. Linking a crate that happens to depend on `log` is not enough. Before converting a
call site inside a binary crate (one with its own `fn main()`, not a library folded into
`solstone-core`), check whether that binary's `main()` calls `env_logger::Builder::from_env(...)` (or
equivalent) — several native worker binaries do not, today, and a mechanical `eprintln!` → `log::warn!`
swap in one of them would not move the message to a different place, it would delete it. The fix, where
it's warranted, is the three-line pattern already used by `solstone-core`, `solstone-core-describe`, and
`solstone-core-journal-bin`:

```rust
fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}
```

`RUST_LOG` propagates correctly to every supervisor-spawned child today (verified directly against
`/proc/<pid>/environ` for the full live process tree) — a worker binary that installs this gets the same
level control as everything else for free.

A library crate (no `[[bin]]` of its own) never needs this — its `log::` calls run inside whichever
binary's process actually links it, and inherit that binary's installed logger. Most of this workspace's
`solstone-core-*` crates are libraries consumed by the single `solstone-core` executable (the supervisor
re-executes itself under different roles — `cortex`, `sense`, `spl`, etc. — rather than shipping separate
binaries for each), so `log::` calls in most of the workspace are already covered by `solstone-core`'s one
`install_logger()` call. Confirm with `grep '\[\[bin\]\]'` in the crate's `Cargo.toml` before assuming
either way.

## Volume is a design input, not an afterthought

`debug!` runs in production on the founder's own machine right now (a systemd drop-in sets
`RUST_LOG=debug` because `journal setup` regenerates the unit on every deploy). The log files this
produces live inside the journal itself. A `debug!` added per-iteration of a hot loop, or one that fires
on every routine request, degrades the exact thing this document exists to protect — a diagnosable
steady-state stream — for the sake of one investigation. Ask before adding a `debug!`: would this still
be tolerable printed continuously, forever, in production? If not, it needs to be rarer (summarize a
batch instead of a per-item line) or it's genuinely `trace!`-shaped and doesn't belong at `debug!` yet.

**Known noisy third-party crates in this dependency graph, not yet quieted:** `hyper`, `rustls`
(compiled with its own `"logging"` feature enabled), `tower`, and `ureq` are all verbose at `debug!`.
`install_logger`'s `default_filter_or` only applies when `RUST_LOG` is unset, so it does not touch the
live drop-in's blanket `RUST_LOG=debug` at all — quieting these under a blanket debug session needs an
explicit per-module directive in the value itself, e.g. `RUST_LOG=debug,hyper=warn,rustls=warn,tower=warn,ureq=warn`.
This is an operational (drop-in file) change, not a code change; see the audit below for what was and
was not applied.

## Audit — what this pass found and did

Measured against `origin/main` at the start of this work (grep methodology: `\b(log::)?debug!\(`, never
a fixed-string match on `println!`, which also matches inside `eprintln!`):

| call sites | count |
|---|---|
| `log::debug!` | 20, in 3 crates |
| `log::warn!` | 59 |
| `log::error!` | 8 |
| `log::info!` | 1 |
| `tracing::info!`/`tracing::warn!` | 3, in 1 crate — **emitted nothing**, no subscriber installed anywhere |
| `eprintln!` | 380 |
| `println!` | 214 |

**~594 raw prints against 88 call sites that actually reach a level filter** (the `log::` row above;
the 3 `tracing::` sites reached nothing). A large share of the raw-print total is
correctly a print: Cargo build-script protocol lines, CLI/IPC contract output (`journal doctor`,
`journal check --json`, every native worker's request/response protocol), and panic-path cleanup text.

**Coordination constraint that shaped this pass's scope:** the unmerged `vpe/w8-deploy` branch (44
commits ahead of `origin/main` at measurement time, still receiving commits — an active burn-in) touches
47 of this workspace's crates, including the two highest-volume raw-print crates
(`solstone-core` itself at 374 sites, `solstone-core-distribution` at 34) and most of the
clearly-diagnostic-heavy ones (`solstone-core-system`, `solstone-core-sense`, `solstone-core-setup`,
`solstone-core-describe`, `solstone-core-cortex`, `solstone-core-convey-shell`, `solstone-core-spl`,
`solstone-core-journal-io`). **This pass deliberately scoped to crates that branch does not touch**,
rather than editing a moving target and creating merge risk for live burn-in work. The highest-value
remaining conversions are in those excluded crates and are follow-up work once `vpe/w8-deploy` merges;
`solstone-core-repository-contracts` (30 sites) is additionally excluded on its own merits — a crate
named for contract fixtures is exactly the class this document says never to convert.

**Converted this pass** (all confirmed either genuinely diagnostic with a live logger downstream, or
given one):
- `solstone-core-callosum`: four `eprintln!` (`connection.rs`, `server.rs` ×3) describing a client-drain
  timeout, broadcast-queue saturation (×2, client and server paths), and a stalled-client eviction — all
  reachable only from code gated behind the crate's `wire` feature, which also gates its `log` dependency,
  and which the only production consumer (`solstone-core`) always enables. Levelled `warn!`, matching the
  existing Windows-only sibling (`record_unauthenticated_peer`) that already used `log::warn!` right next
  to the two that hadn't been converted.
- `solstone-core-transfer`: one `eprintln!` (best-effort indexer-rescan notification, Callosum socket
  unavailable) → `warn!`. Library-only crate; runs inside `solstone-core`.
- `solstone-core-support-portal`: the three silent `tracing::` calls → `log::`, plus the swallowed error
  in the "acknowledgement failed" arm now carries the actual error instead of discarding it.
- `solstone-core-depict`: added `install_logger()` (matching the three-crate precedent above) and
  converted the one genuine diagnostic — a swallowed `SEGMENT_META` JSON parse failure that falls back to
  continuing without it — to `warn!`. The other `depict` call sites are its request/response protocol
  and stay as `print!`/`eprintln!`.
- `solstone-core-speakers-analyze`: added `install_logger()` and converted two pre-protocol startup
  failures (missing `SOLSTONE_JOURNAL`, hosted-admission failure) to `error!` — both exit the process
  immediately after. Every other call site in this crate is the request/response protocol over
  stdin/stdout and stays untouched.

**Deliberately not converted, and why:**
- `solstone-core-vad-analyze` — every call site is protocol output; no diagnostic candidate exists, so no
  logger was added (nothing would use it).
- `solstone-core-top::production.rs` — the `verbose`/`debug`-flag banner is gated on the command's own
  flag, not `RUST_LOG`; the panic-cleanup lines run inside an unwind handler. Both stay raw per the
  signals above.
- `solstone-core-transcribe-cli`, `solstone-core-brain-cli`, `solstone-core-retention-cli`,
  `solstone-core-sol-client-cli` (`resolve_parity_leaves`), `solstone-core-vulkan-probe`,
  `solstone-core-pdf` — CLI/tool contract output (JSON bodies, usage banners, `--version`). Left alone.
- `solstone-core-speakers::filterbank.rs` — the one `eprintln!` here is inside a `#[test]` function, not
  caught by a `/tests/`-path or `_tests.rs`-suffix filter; test-debug output stays a plain print.
- Every `build.rs` in the safe crate set — Cargo build-script protocol, never logging.

## What's next

- Re-run this same audit against the excluded crates once `vpe/w8-deploy` merges — `solstone-core`
  itself is the single biggest opportunity (374 raw-print sites) and the one place `install_logger` and
  the third-party noisy-crate filter directive above actually live.
- Apply the noisy-crate `RUST_LOG` directive to the live drop-in once someone is deliberately touching
  it, rather than as a drive-by edit to a systemd unit outside this pass's own change set.
