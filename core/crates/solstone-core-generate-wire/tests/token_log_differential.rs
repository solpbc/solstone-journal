// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use solstone_core_generate_wire::usage_for_log;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let venv = repository_root().join(".venv/bin/python3");
    assert!(venv.is_file(), "differential requires make install");
    venv
}

fn python_usage(usage: &Value) -> Value {
    let script = concat!(
        "import json, os, sys, tempfile, time\n",
        "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
        "from pathlib import Path\n",
        "from solstone.think.models import log_token_usage\n",
        "journal = tempfile.mkdtemp()\n",
        "os.environ['SOLSTONE_JOURNAL'] = journal\n",
        "log_token_usage('model', json.loads(sys.argv[1]), context='test.context', type='generate')\n",
        "path = Path(journal) / 'tokens' / (time.strftime('%Y%m%d') + '.jsonl')\n",
        "print(json.dumps(json.loads(path.read_text())['usage']))\n",
    );
    let output = Command::new(python())
        .args(["-c", script, &usage.to_string()])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .output()
        .expect("run Python token log normalization");
    assert!(
        output.status.success(),
        "Python stderr: {:?}",
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("Python normalized usage JSON")
}

#[test]
fn token_log_normalization_matches_python() {
    for usage in [
        json!({
            "input_tokens": 2,
            "output_tokens": 3,
            "total_tokens": 5,
            "cached_tokens": 1,
            "reasoning_tokens": 4,
            "cache_creation_tokens": 6,
            "requests": 1,
        }),
        json!({"input_tokens": 2, "output_tokens": 3}),
        json!({}),
        json!({"reasoning_tokens": 4}),
        json!({"input_tokens": 2, "cached_input_tokens": 1}),
    ] {
        assert_eq!(usage_for_log(&usage), python_usage(&usage), "usage={usage}");
    }
}
