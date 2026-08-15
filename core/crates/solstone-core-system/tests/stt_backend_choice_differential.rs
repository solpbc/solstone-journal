// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Proves `solstone_core_system::stt_backend_choice::resolve_stt_backend_choice`
//! agrees with the Python original it replaces
//! (`solstone.observe.transcribe.resource.resolve_stt_backend_choice`) across
//! the same decision table the Rust unit test exercises. This is the
//! "option (a)" proof for the backend-selection architectural fork: two in-scope
//! supervisor call sites depend on this choice at runtime, so porting it
//! without a differential would silently duplicate an already-shared
//! decision a fourth way.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use solstone_core_system::stt_backend_choice::{decision_table, resolve_stt_backend_choice};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let repository = repository_root();
    let venv = repository.join(".venv/bin/python3");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

/// Runs the real Python `resolve_stt_backend_choice` over `rows` (each row:
/// `[explicit_backend, available_bytes, floor_bytes, local_backend,
/// confidential_lane_active, confidential_audio_enabled]`) and returns its
/// choices in the same order.
///
/// Loads `resource.py` directly by file path with `importlib`, rather than
/// `from solstone.observe.transcribe.resource import ...`, so this oracle
/// depends on nothing beyond the stdlib `resource.py` itself needs
/// (`resolve_stt_backend_choice`'s own docstring guarantees it reads no
/// config/env/machine state). A plain package import would run
/// `solstone/observe/transcribe/__init__.py`'s full transitive import graph
/// -- unrelated third-party dependencies this pure function does not use --
/// purely as a side effect of package layout.
fn python_choices(rows: &Value) -> Vec<String> {
    let script = concat!(
        "import importlib.util, json, os, sys\n",
        "path = os.path.join(\n",
        "    os.environ['SOLSTONE_REPO_ROOT'],\n",
        "    'solstone/observe/transcribe/resource.py',\n",
        ")\n",
        "spec = importlib.util.spec_from_file_location('stt_resource_oracle', path)\n",
        "resource = importlib.util.module_from_spec(spec)\n",
        "spec.loader.exec_module(resource)\n",
        "rows = json.load(sys.stdin)\n",
        "results = []\n",
        "for explicit, available, floor, local, lane_active, audio_enabled in rows:\n",
        "    results.append(resource.resolve_stt_backend_choice(\n",
        "        explicit,\n",
        "        available,\n",
        "        floor_bytes=floor,\n",
        "        local_backend=local,\n",
        "        confidential_lane_active=lane_active,\n",
        "        confidential_audio_enabled=audio_enabled,\n",
        "    ))\n",
        "json.dump(results, sys.stdout)\n",
    );
    let mut child = Command::new(python())
        .args(["-c", script])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start Python oracle");
    child
        .stdin
        .take()
        .expect("Python stdin")
        .write_all(rows.to_string().as_bytes())
        .expect("write decision table to Python stdin");
    let output = child.wait_with_output().expect("Python oracle exit");
    assert!(
        output.status.success(),
        "Python oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python oracle JSON output")
}

#[test]
fn rust_and_python_agree_across_the_decision_table() {
    let table = decision_table();
    let rows: Vec<Value> = table
        .iter()
        .map(
            |(_name, explicit, available, floor, local, lane_active, audio_enabled, _expected)| {
                json!([
                    explicit,
                    available,
                    floor,
                    local,
                    lane_active,
                    audio_enabled
                ])
            },
        )
        .collect();
    let python_results = python_choices(&Value::Array(rows));
    assert_eq!(
        python_results.len(),
        table.len(),
        "Python returned a different row count than the decision table"
    );

    for (
        (name, explicit, available, floor, local, lane_active, audio_enabled, expected),
        python_choice,
    ) in table.iter().zip(python_results.iter())
    {
        let rust_choice = resolve_stt_backend_choice(
            *explicit,
            *available,
            *floor,
            *local,
            *lane_active,
            *audio_enabled,
        );
        assert_eq!(
            &rust_choice, python_choice,
            "{name}: Rust vs Python diverge"
        );
        assert_eq!(
            &rust_choice, expected,
            "{name}: Rust vs decision table diverge"
        );
    }
}
