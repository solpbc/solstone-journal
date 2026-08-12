// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Executes the Python installer specs and compares their resolved identities.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use solstone_core_local::install::{ced_install, rerank_install, rfdetr_install};

#[test]
fn downloading_installer_specs_match_python() {
    let root = repository_root();
    let journal = tempfile::tempdir().expect("fixed scratch journal");
    let python = root.join(".venv/bin/python3");
    assert!(
        python.is_file(),
        "make check-differentials must provision .venv"
    );
    let output = Command::new(python)
        .args([
            "-c",
            PYTHON_SNAPSHOT,
            journal.path().to_str().expect("UTF-8 journal"),
        ])
        .current_dir(&root)
        .output()
        .expect("start Python installer oracle");
    assert!(
        output.status.success(),
        "Python installer oracle failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python: Value = serde_json::from_slice(&output.stdout).expect("Python oracle JSON");
    let rust = serde_json::json!({
        "rerank": rerank_install::differential_snapshot(journal.path()),
        "ced": ced_install::differential_snapshot(journal.path()),
        "rfdetr": rfdetr_install::differential_snapshot(journal.path()),
    });
    assert_eq!(rust, python);
    assert!(python["ced"]["expected_file_keys"]
        .as_array()
        .expect("CED expected-file keys")
        .iter()
        .any(|value| value == "models/mudler/ced-gguf/b5e9a4aad6438763c8da16079d77563fbed35c65/ced-tiny-q8_0.gguf"));
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

const PYTHON_SNAPSHOT: &str = r#"
import json
import sys
from pathlib import Path
from solstone.think.providers import rerank_install as rerank
from solstone.think.providers import ced_install as ced
from solstone.think.providers import rfdetr_install as rfdetr

journal = Path(sys.argv[1])
rerank_spec = rerank.RERANK_MODEL_SPEC
ced_engine = ced.CED_ENGINE_SPEC
ced_model = ced.CED_MODEL_SPEC
rfdetr_spec = rfdetr.RFDETR_SPEC
keys = ["linux-cpu-x64", "linux-cpu-arm64", "macos-metal-arm64"]
snapshot = {
    "rerank": {
        "repo": rerank_spec.repo,
        "revision": rerank_spec.revision,
        "files": [{
            "filename": file.path,
            "sha256": file.sha256,
            "size_bytes": file.size_bytes,
            "destination": str(rerank.asset_path(file, journal_path=journal)),
        } for file in rerank_spec.files],
        "cache_root": str(rerank.cache_root(journal)),
        "sidecar": str(rerank.sidecar_path(journal_path=journal)),
        "sidecar_file_keys": sorted(rerank._expected_files(rerank_spec)),
    },
    "ced": {
        "engine_version": ced_engine.version,
        "engines": [{
            "artifact_key": key,
            "filename": ced_engine.artifacts[key].path,
            "sha256": ced_engine.artifacts[key].sha256,
            "size_bytes": ced_engine.artifacts[key].size_bytes,
            "engine_dir": str(ced.engine_dir(key, journal_path=journal)),
        } for key in keys],
        "model": {
            "repo": ced_model.repo,
            "revision": ced_model.revision,
            "filename": ced_model.file,
            "sha256": ced_model.sha256,
            "size_bytes": ced_model.size_bytes,
        },
        "cache_root": str(ced.cache_root(journal)),
        "model_dir": str(ced.model_dir(journal_path=journal)),
        "sidecar": str(ced.sidecar_path(journal_path=journal)),
        "expected_file_keys": sorted(
            key for artifact_key in keys
            for key in ced._expected_files(ced_engine, ced_model, artifact_key)
        ),
    },
    "rfdetr": {
        "engine_ref": rfdetr_spec.engine.ref,
        "release_tag": rfdetr_spec.engine.release_tag,
        "tarball": {
            "filename": rfdetr_spec.engine.tarball_name,
            "sha256": rfdetr_spec.engine.tarball_sha256,
            "extracted_binary_sha256": rfdetr_spec.engine.binary_sha256,
        },
        "model": {
            "repo": rfdetr_spec.model.repo,
            "revision": rfdetr_spec.model.revision,
            "filename": rfdetr_spec.model.filename,
            "sha256": rfdetr_spec.model.sha256,
            "size_bytes": rfdetr_spec.model.size_bytes,
        },
        "cache_root": str(rfdetr.cache_root(journal)),
        "engine_binary": str(rfdetr.binary_path(journal_path=journal)),
        "model_path": str(rfdetr.model_path(journal_path=journal)),
        "sidecar": str(rfdetr.sidecar_path(journal_path=journal)),
    },
}
print(json.dumps(snapshot, sort_keys=True))
"#;
