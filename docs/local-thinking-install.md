# Local thinking installation check

Run this on demand on an Apple Silicon Mac to check portal installation from a
fresh journal and a journal with an older MLX install record. It starts the real
portal, requests installation over HTTP, selects local thinking, and asks the
installed model for an answer. It is separate from ordinary CI and release gates.

Use a source checkout matching the candidate binaries, Python 3, and the Rust
build prerequisites in [CONTRIBUTING.md](../CONTRIBUTING.md). Allow disk space
for the build, fixture copies, and two model downloads. Each fixture downloads the
pinned model and projector, about 3.4 GB, plus the runtime.

From the checkout root, prepare the candidate:

```bash
cargo build --manifest-path core/Cargo.toml --locked \
  -p solstone-core -p solstone-core-journal-bin -p solstone-core-sol-bin
make check-rust-onnx-stage
make build-sandbox-processing
```

Keep the processing helpers and staged ONNX library with the candidate. The
harness also copies this checkout's `core/payload` and `core/models/assets`.

Choose an absolute run directory that does not already exist:

```bash
python3 -m tools.local_thinking_install \
  --candidate-dir /absolute/path/to/checkout/core/target/debug \
  --run-dir /var/tmp/local-thinking-run
```

The harness creates its own journals and installation-identity namespaces under
the current account. It does not run `journal setup` or change the account's
journal configuration. Use only a disposable run directory.

An exit code of 0 requires both fixtures to complete installation and return a
nonempty answer from the bundled model. On macOS, the check also requires the
managed runtime's GPU arguments and Metal initialization and offload evidence.
The fresh fixture must report advancing model download bytes against the full
model-plus-projector total. The older MLX record must be recognized as non-native
before installation and replaced by a native installed record.

An unfixed candidate that refuses installation exits 1 with `bootstrap_refused`.
Missing prerequisites, failed admission, and portal startup failures are separate
failures; they do not count as that baseline result. A comparison run uses the
same harness source against each candidate. The receipt records binary, helper,
and harness hashes.

Inspect `RECEIPT.txt` and `receipt.json`, then the per-case HTTP, progress,
inference, process, and cleanup records. The harness attempts to stop its
processes and removes only namespaces it successfully admitted. Any cleanup error
makes the run fail; inspect the reported processes and namespace before cleanup. Journals, downloaded
files, staged binaries, and logs remain in the run directory; remove that
disposable tree after preserving the evidence you need and confirming cleanup.

The default startup timeout is 60 seconds and the install/runtime timeout is one
hour per wait. Use `--timeout-seconds` and `--install-timeout-seconds` to change
those bounds. The harness's own controls can be run separately:

```bash
python3 -m unittest discover -s tools/local_thinking_install/tests -v
```

This does not exercise rendered buttons. Check cancellation, retry after an
interrupted download, portal restart, keyboard focus, and narrow-screen layouts
in a browser when changing the portal lifecycle.
