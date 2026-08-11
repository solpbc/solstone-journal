# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import subprocess
import venv
from pathlib import Path

import numpy as np
import pytest

from solstone.think.speakers_analyze_installation import (
    runtime_has_speakers_analyze_wheel_coverage,
)

ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.integration
@pytest.mark.timeout(600)
def test_cpu_leaf_install_reaches_speakers_analyze_helper(tmp_path: Path) -> None:
    if not runtime_has_speakers_analyze_wheel_coverage():
        pytest.skip("host is not covered by solstone-core-speakers-analyze wheels")

    dist_dir = tmp_path / "dist"
    build = subprocess.run(
        [
            "uv",
            "build",
            "--package",
            "solstone-journal",
            "--wheel",
            "--out-dir",
            str(dist_dir),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    assert build.returncode == 0, build.stderr or build.stdout
    wheels = sorted(dist_dir.glob("solstone_journal-*.whl"))
    assert len(wheels) == 1

    env_root = tmp_path / "venv"
    venv.EnvBuilder(with_pip=True, symlinks=False).create(env_root)
    python = env_root / "bin" / "python"
    install = subprocess.run(
        [str(python), "-m", "pip", "install", str(wheels[0])],
        capture_output=True,
        text=True,
        check=False,
        timeout=600,
    )
    assert install.returncode == 0, install.stderr or install.stdout

    invariant = subprocess.run(
        [
            str(python),
            "-c",
            "\n".join(
                [
                    "from solstone.think.speakers_analyze_installation import check_speakers_analyze_installation",
                    "result = check_speakers_analyze_installation()",
                    'print(f"installation status={result.status!r} message={result.message!r}")',
                    "raise SystemExit(0 if result.status == 'ok' else 1)",
                ]
            ),
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    print(invariant.stdout, end="")
    assert invariant.returncode == 0, invariant.stderr or invariant.stdout


@pytest.mark.integration
@pytest.mark.timeout(900)
def test_clean_leaf_install_without_sklearn_runs_discovery_cluster(
    tmp_path: Path,
) -> None:
    if not runtime_has_speakers_analyze_wheel_coverage():
        pytest.skip("host is not covered by solstone-core-speakers-analyze wheels")

    dist_dir = tmp_path / "dist"
    for package in ("solstone", "solstone-journal"):
        build = subprocess.run(
            [
                "uv",
                "build",
                "--package",
                package,
                "--wheel",
                "--out-dir",
                str(dist_dir),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            timeout=180,
        )
        assert build.returncode == 0, build.stderr or build.stdout

    root_wheels = sorted(
        path
        for path in dist_dir.glob("solstone-*.whl")
        if not path.name.startswith("solstone_journal")
    )
    leaf_wheels = sorted(dist_dir.glob("solstone_journal-*.whl"))
    assert len(root_wheels) == 1
    assert len(leaf_wheels) == 1
    root_requirement = f"solstone[journal-host] @ {root_wheels[0].resolve().as_uri()}"
    leaf_requirement = f"solstone-journal @ {leaf_wheels[0].resolve().as_uri()}"

    env_root = tmp_path / "venv"
    venv.EnvBuilder(with_pip=True, symlinks=False).create(env_root)
    python = env_root / "bin" / "python"
    install = subprocess.run(
        [
            str(python),
            "-m",
            "pip",
            "install",
            "--find-links",
            str(dist_dir),
            root_requirement,
            leaf_requirement,
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=600,
    )
    assert install.returncode == 0, install.stderr or install.stdout

    payload_path = tmp_path / "embeddings.f32le"
    matrix = np.eye(5, 256, dtype=np.float32)
    matrix.astype("<f4", copy=False).tofile(payload_path)
    request = {
        "schema": "solstone-speaker-discovery-cluster-request-v1",
        "embeddings_f32le_path": str(payload_path),
        "payload_format": "raw-f32le-row-major-v1",
        "dtype": "float32-le",
        "shape": [5, 256],
        "min_cluster_size": 5,
        "min_samples": 3,
    }
    invariant = subprocess.run(
        [
            str(python),
            "-c",
            "\n".join(
                [
                    "import importlib.metadata as md",
                    "import json",
                    "import os",
                    "import subprocess",
                    "import sys",
                    "from solstone.think import warm",
                    "from solstone.think.speakers_analyze_installation import speakers_analyze_path_for_executable",
                    "try:",
                    "    md.distribution('scikit-learn')",
                    "except md.PackageNotFoundError:",
                    "    pass",
                    "else:",
                    "    raise SystemExit('scikit-learn unexpectedly installed')",
                    "raise_code = warm.warm()",
                    "if raise_code != 0:",
                    "    raise SystemExit(f'warm failed: {raise_code}')",
                    "helper = speakers_analyze_path_for_executable()",
                    "if not os.access(helper, os.X_OK):",
                    "    raise SystemExit(f'helper is not executable: {helper}')",
                    "completed = subprocess.run([str(helper), 'discovery-cluster'], input=sys.stdin.read(), text=True, capture_output=True, check=False, timeout=120)",
                    "if completed.returncode != 0:",
                    "    raise SystemExit(completed.stderr or completed.stdout)",
                    "response = json.loads(completed.stdout)",
                    "assert response['schema'] == 'solstone-speaker-discovery-cluster-response-v1'",
                    "assert len(response['labels']) == 5",
                    "assert response['parameters'] == {'min_cluster_size': 5, 'min_samples': 3}",
                    "assert response['algorithm'] == 'hdbscan-eom-euclidean-f64-prim-mst'",
                ]
            ),
        ],
        input=json.dumps(request),
        capture_output=True,
        text=True,
        check=False,
        timeout=240,
    )
    assert invariant.returncode == 0, invariant.stderr or invariant.stdout
