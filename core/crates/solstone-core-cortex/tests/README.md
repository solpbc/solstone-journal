# Cortex tests

Cortex tests are split by what they need from the host.

- **Library tests** (`src/`, run by `cargo test -p solstone-core-cortex --lib`)
  cover state, renewal, service startup, storage and command construction.
  They never start a process: a spawn is checked by building the worker
  `Command` and inspecting its program, arguments, working directory and
  environment.
- **`cortex_child_supervisor`** (this directory) covers behavior that needs a
  real child, such as process groups, graceful and forced stop, working
  directories, stdin failure, and draining or stopping a running talent. It
  runs on Unix only and needs the `test-hooks` feature:

  ```sh
  cargo test -p solstone-core-cortex --test cortex_child_supervisor --features test-hooks
  ```

## Test hooks

`src/test_hooks.rs` exists only under `cfg(test)` or the `test-hooks`
feature, and is hidden from the documentation. It re-exports the crate-private
spawn, stop and state surfaces the integration tests drive. A default build
does not compile it, and the crate's documented API is the same with or
without it.

## Fixture binaries

`tests/bin/` holds two binaries, also built only with `test-hooks`:

- `solstone-cortex-controller` starts one talent worker through `spawn_one`
  in a test journal, so a test can see which directory the worker inherits.
- `solstone-cortex-worker` stands in for a talent worker. `CORTEX_WORKER_MODE`
  picks its behavior, for example writing a finish record, reporting its
  working directory, leaving a grandchild behind in its process group, or
  ignoring SIGTERM.
