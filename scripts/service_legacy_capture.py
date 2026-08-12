#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regenerate service evidence as one fail-closed full-tree transaction."""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Callable, TypeVar

from build_service_legacy_packaging_provenance import selected_rust_tools
from service_legacy_git import TAG_NAMESPACE, GitFacts, GitTransport
from service_legacy_integrity import (
    IntegrityError,
    audit_corpus,
    verify_non_owned_source_closure,
)

ROOT = Path(__file__).resolve().parents[1]
LIVE_EVIDENCE = ROOT / "core/fixtures/service_legacy_evidence"
ORACLE = ROOT / "scripts/fixtures/service_legacy_path_role_oracle.json"
STAGES = (
    "capture_service_legacy_commit_census.py",
    "capture_service_legacy_tag_census.py",
    "acquire_service_legacy_cpython.py",
    "capture_service_legacy_raw.py",
    "normalize_service_legacy_evidence.py",
    "generate_service_legacy_negative_twins.py",
    "derive_service_legacy_semantic_deltas.py",
    "build_service_legacy_packaging_provenance.py",
    "write_service_legacy_manifest.py",
)
POST_STAGE_GUARDS = ("historical-facts", "integrity-audit", "evidence-gate")
FAILURE_BOUNDARIES = (
    *STAGES[:8],
    POST_STAGE_GUARDS[0],
    STAGES[8],
    *POST_STAGE_GUARDS[1:],
)
T = TypeVar("T")


def write_json(path: Path, value: Any) -> None:
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def run(command: list[str], *, environment: dict[str, str], label: str) -> None:
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        text=True,
        check=False,
    )
    if result.returncode:
        raise IntegrityError(
            label, f"command exited {result.returncode}: {' '.join(command)}"
        )


def capture_environment(
    *, capture_input: str, evidence: Path, python_cache: Path, scratch: Path
) -> dict[str, str]:
    rustup = Path(shutil.which("rustup") or "").resolve()
    if not rustup.is_file():
        raise IntegrityError("rust-tool", "rustup provisioning tool is unavailable")
    return {
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "HOME": str(scratch / "home"),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "PATH": "/usr/bin:/bin",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONHASHSEED": "0",
        "PYTHONNOUSERSITE": "1",
        "SERVICE_LEGACY_CAPTURE_INPUT": capture_input,
        "SERVICE_LEGACY_EVIDENCE_ROOT": str(evidence),
        "SERVICE_LEGACY_FULL_TREE_STAGING": "1",
        "SERVICE_LEGACY_PYTHON_CACHE_ROOT": str(python_cache),
        "SERVICE_LEGACY_RUSTUP": str(rustup),
        "SERVICE_LEGACY_TAG_NAMESPACE": TAG_NAMESPACE,
        "SERVICE_LEGACY_WHEEL_SCRATCH_ROOT": str(scratch / "wheels"),
        "SOURCE_DATE_EPOCH": "0",
        "TMPDIR": str(scratch / "tmp"),
        "TZ": "UTC",
        "CARGO_HOME": os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")),
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
    }


def run_evidence_gate(environment: dict[str, str]) -> None:
    _channel, _rustup, cargo, rustc = selected_rust_tools()
    gate_environment = dict(environment)
    gate_environment.update(
        {
            "CARGO": str(cargo),
            "PATH": f"{cargo.parent}:/usr/bin:/bin",
            "RUSTC": str(rustc),
        }
    )
    run(
        ["/usr/bin/make", "check-service-legacy-evidence"],
        environment=gate_environment,
        label="evidence-gate",
    )


def authoritative_record(facts: GitFacts) -> dict[str, Any]:
    return {
        "capture_input": facts.capture_input,
        "git": {
            "executable": "/usr/bin/git",
            "executable_sha256": facts.git_sha256,
            "https_helper": "git-remote-https",
            "https_helper_path": facts.helper_path,
            "https_helper_sha256": facts.helper_sha256,
        },
        "remote": {
            "main": facts.main_object,
            "repository": "https://github.com/solpbc/solstone-journal.git",
            "tags": list(facts.tags),
        },
        "schema": "service-legacy-capture-input",
        "schema_version": 1,
    }


def verify_historical_facts(
    transport: GitTransport, facts: GitFacts, stage: Path
) -> None:
    follow = json.loads((stage / "follow-census.json").read_text(encoding="utf-8"))
    if len(follow.get("entries", [])) != 44:
        raise IntegrityError("follow-count", "follow census is not exactly 44 entries")
    for row in follow["entries"]:
        commit = row["commit"]
        if transport.run(
            ["merge-base", "--is-ancestor", commit, facts.capture_input],
            cwd=ROOT,
            check=False,
        ).returncode:
            raise IntegrityError("follow-reachability", f"{commit} is not an ancestor")
        blob = transport.run(
            ["rev-parse", f"{commit}:{row['path']}"], cwd=ROOT
        ).stdout.strip()
        if blob != row["blob"]:
            raise IntegrityError("follow-blob", f"{commit}:{row['path']} differs")
    tags = json.loads((stage / "tag-census.json").read_text(encoding="utf-8"))
    by_name = {row["ref"].removeprefix("refs/tags/"): row for row in facts.tags}
    if len(tags.get("tags", [])) != 66 or set(by_name) != {
        row["tag"] for row in tags["tags"]
    }:
        raise IntegrityError("tag-count", "tag census differs from authoritative refs")
    for row in tags["tags"]:
        fact = by_name[row["tag"]]
        private = f"{TAG_NAMESPACE}/{row['tag']}"
        peeled = transport.run(["rev-parse", private + "^{}"], cwd=ROOT).stdout.strip()
        if peeled != fact["peeled"]:
            raise IntegrityError("tag-peeled", f"{row['tag']} peeled target differs")
        if row["path"] is not None:
            blob = transport.run(
                ["rev-parse", f"{private}^{{}}:{row['path']}"], cwd=ROOT
            ).stdout.strip()
            if blob != row["blob"]:
                raise IntegrityError("tag-blob", f"{row['tag']} path/blob differs")


def atomic_exchange(
    first: Path, second: Path, *, force_unsupported: bool = False
) -> None:
    if first.parent.resolve() != second.parent.resolve():
        raise IntegrityError("exchange", "both trees must have the same parent")
    if force_unsupported:
        raise IntegrityError("exchange-unsupported", "injected unsupported exchange")
    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = getattr(libc, "renameat2", None)
    if renameat2 is None:
        raise IntegrityError("exchange-unsupported", "renameat2 is unavailable")
    renameat2.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    ]
    renameat2.restype = ctypes.c_int
    result = renameat2(
        -100,
        os.fsencode(first),
        -100,
        os.fsencode(second),
        2,
    )
    if result:
        error = ctypes.get_errno()
        raise IntegrityError(
            "exchange", f"renameat2(RENAME_EXCHANGE) failed: {os.strerror(error)}"
        )


def maybe_fail(label: str) -> None:
    if os.environ.get("SERVICE_LEGACY_FAIL_AFTER") == label:
        raise IntegrityError("injected-failure", label)


def run_stage_plan(steps: list[tuple[str, Callable[[], None]]]) -> None:
    """Run one ordered stage plan and expose a causal post-stage failure seam."""

    labels = tuple(label for label, _action in steps)
    if labels != FAILURE_BOUNDARIES:
        raise IntegrityError(
            "stage-plan",
            f"stage plan differs from the closed boundary order: {labels!r}",
        )
    for label, action in steps:
        action()
        maybe_fail(label)


def promote(
    stage: Path,
    *,
    live: Path = LIVE_EVIDENCE,
    inject_cleanup_failure: bool = False,
) -> dict[str, Any]:
    atomic_exchange(live, stage)
    try:
        if inject_cleanup_failure:
            raise OSError("injected cleanup failure")
        shutil.rmtree(stage)
    except OSError as exc:
        return {
            "residue": str(stage),
            "state": "committed_with_cleanup_warning",
            "warning": str(exc),
        }
    return {"state": "committed"}


def staged_transaction(
    live: Path,
    builder: Callable[[Path], T],
    *,
    promote_live: bool,
    inject_cleanup_failure: bool = False,
) -> tuple[T, dict[str, Any]]:
    """Build beside ``live`` and publish only the completely verified tree."""

    parent = live.parent.resolve()
    stage = Path(tempfile.mkdtemp(prefix=".service-legacy-staging.", dir=parent))
    committed = False
    try:
        built = builder(stage)
        if not promote_live:
            return built, {"staged": str(stage), "state": "staged"}
        transaction = promote(
            stage,
            live=live,
            inject_cleanup_failure=inject_cleanup_failure,
        )
        committed = True
        return built, transaction
    finally:
        if not committed and promote_live:
            shutil.rmtree(stage, ignore_errors=True)


def regenerate(capture_input: str, *, promote_live: bool = True) -> dict[str, Any]:
    if len(capture_input) != 40 or any(
        character not in "0123456789abcdef" for character in capture_input
    ):
        raise IntegrityError("capture-input", "capture input is not an exact commit id")
    scratch: Path | None = None
    try:
        with GitTransport() as transport:
            facts = transport.fetch_authority(ROOT)
            if facts.capture_input != capture_input:
                raise IntegrityError(
                    "capture-input",
                    f"HEAD {facts.capture_input} differs from required {capture_input}",
                )
            verify_non_owned_source_closure(capture_input)
            (ROOT / "scratch").mkdir(exist_ok=True)
            scratch = Path(
                tempfile.mkdtemp(prefix="service-legacy-run-", dir=ROOT / "scratch")
            )
            python_cache = scratch / "python"
            for directory in (scratch / "home", scratch / "tmp", scratch / "wheels"):
                directory.mkdir(parents=True, exist_ok=True)

            def build(stage: Path) -> dict[str, Any]:
                environment = capture_environment(
                    capture_input=capture_input,
                    evidence=stage,
                    python_cache=python_cache,
                    scratch=scratch,
                )

                def script_action(stage_name: str) -> Callable[[], None]:
                    command = [sys.executable, str(ROOT / "scripts" / stage_name)]
                    if stage_name == "capture_service_legacy_raw.py":
                        command.extend(
                            [
                                "--output-root",
                                str(stage / "raw"),
                                "--scratch-root",
                                str(scratch / "capture"),
                            ]
                        )
                    return lambda: run(
                        command, environment=environment, label=stage_name
                    )

                report: dict[str, Any] = {}

                def historical_action() -> None:
                    write_json(
                        stage / "capture-input.json", authoritative_record(facts)
                    )
                    verify_historical_facts(transport, facts, stage)

                def audit_action() -> None:
                    report.update(audit_corpus(stage, ORACLE))

                steps: list[tuple[str, Callable[[], None]]] = []
                steps.extend((name, script_action(name)) for name in STAGES[:8])
                steps.append(("historical-facts", historical_action))
                steps.append((STAGES[8], script_action(STAGES[8])))
                steps.append(("integrity-audit", audit_action))
                steps.append(
                    (
                        "evidence-gate",
                        lambda: run_evidence_gate(environment),
                    )
                )
                run_stage_plan(steps)
                return report

            report, transaction = staged_transaction(
                LIVE_EVIDENCE,
                build,
                promote_live=promote_live,
                inject_cleanup_failure=os.environ.get(
                    "SERVICE_LEGACY_INJECT_CLEANUP_FAILURE"
                )
                == "1",
            )
            return {
                "capture_input": capture_input,
                "integrity": report,
                "transaction": transaction,
            }
    finally:
        if scratch is not None:
            shutil.rmtree(scratch, ignore_errors=True)


def self_test() -> None:
    with tempfile.TemporaryDirectory(
        prefix="service-legacy-transaction-test-"
    ) as temporary:
        root = Path(temporary)
        live = root / "live"
        staged = root / "staged"
        live.mkdir()
        staged.mkdir()
        (live / "value").write_text("old", encoding="utf-8")
        (staged / "value").write_text("new", encoding="utf-8")
        try:
            atomic_exchange(live, staged, force_unsupported=True)
        except IntegrityError as exc:
            if exc.guard != "exchange-unsupported":
                raise
        else:
            raise AssertionError("unsupported exchange did not refuse")
        if (live / "value").read_text(encoding="utf-8") != "old":
            raise AssertionError("exchange refusal changed the live tree")
        result = promote(staged, live=live, inject_cleanup_failure=True)
        if result["state"] != "committed_with_cleanup_warning":
            raise AssertionError("cleanup failure was not reported as committed")
        if (live / "value").read_text(encoding="utf-8") != "new":
            raise AssertionError("atomic exchange did not publish complete new tree")
        if (staged / "value").read_text(encoding="utf-8") != "old":
            raise AssertionError("atomic exchange did not preserve complete old tree")
        shutil.rmtree(staged)

        failure_root = root / "failure-matrix"
        failure_root.mkdir()
        failure_live = failure_root / "live"
        failure_live.mkdir()
        (failure_live / "value").write_text("old", encoding="utf-8")
        for label in FAILURE_BOUNDARIES:
            previous = os.environ.get("SERVICE_LEGACY_FAIL_AFTER")
            os.environ["SERVICE_LEGACY_FAIL_AFTER"] = label
            try:
                visited: list[str] = []

                def build_failure(stage: Path) -> None:
                    steps: list[tuple[str, Callable[[], None]]] = []
                    for boundary in FAILURE_BOUNDARIES:

                        def action(
                            boundary: str = boundary, stage: Path = stage
                        ) -> None:
                            visited.append(boundary)
                            (stage / boundary).write_text(boundary, encoding="utf-8")

                        steps.append((boundary, action))
                    run_stage_plan(steps)

                try:
                    staged_transaction(
                        failure_live,
                        build_failure,
                        promote_live=True,
                    )
                except IntegrityError as exc:
                    if exc.guard != "injected-failure":
                        raise
                else:
                    raise AssertionError(f"stage poison {label} did not fail")
                expected = list(
                    FAILURE_BOUNDARIES[: FAILURE_BOUNDARIES.index(label) + 1]
                )
                if visited != expected:
                    raise AssertionError(
                        f"stage poison {label} visited {visited!r}, expected {expected!r}"
                    )
                if (failure_live / "value").read_text(encoding="utf-8") != "old":
                    raise AssertionError(f"stage poison {label} changed the live tree")
                residue = sorted(failure_root.glob(".service-legacy-staging.*"))
                if residue:
                    raise AssertionError(
                        f"stage poison {label} left staging residue: {residue!r}"
                    )
            finally:
                if previous is None:
                    os.environ.pop("SERVICE_LEGACY_FAIL_AFTER", None)
                else:
                    os.environ["SERVICE_LEGACY_FAIL_AFTER"] = previous


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture-input")
    parser.add_argument("--stage-only", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        print("service-legacy transaction self-test passed")
        return 0
    if args.capture_input is None:
        parser.error("--capture-input is required")
    print(
        json.dumps(
            regenerate(args.capture_input, promote_live=not args.stage_only),
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
