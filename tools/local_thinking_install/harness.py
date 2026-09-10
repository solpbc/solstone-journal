# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Test harness orchestration, classification, and receipt generation."""

from __future__ import annotations

import hashlib
import json
import subprocess
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from .fixtures import setup_case_journal
from .portal import HttpResponse, PortalProcess, PortalStartupError, send_http_request

SPAWN_UNAVAILABLE_SNIPPET = "local install can't be started from this build yet"


@dataclass
class CaseResult:
    name: str
    convey_port: int | None
    http_status: int
    reason_code: str | None
    detail: str | None
    classification: str
    note: str
    post_admit_outcome: str | None = None
    prior_mlx_discriminator: str | None = None


@dataclass
class HarnessReport:
    provenance: dict[str, Any]
    cases: list[CaseResult]
    overall_outcome: str
    timestamp: str
    all_passed: bool


def compute_sha256(path: Path) -> str:
    hasher = hashlib.sha256()
    with open(path, "rb") as fp:
        while chunk := fp.read(65536):
            hasher.update(chunk)
    return hasher.hexdigest()


def collect_provenance(candidate_dir: Path) -> dict[str, Any]:
    journal_bin = candidate_dir / "solstone-core-journal"
    if not journal_bin.exists() and (candidate_dir / "solstone-core-journal.exe").exists():
        journal_bin = candidate_dir / "solstone-core-journal.exe"

    core_bin = candidate_dir / "solstone-core"
    if not core_bin.exists() and (candidate_dir / "solstone-core.exe").exists():
        core_bin = candidate_dir / "solstone-core.exe"

    if not journal_bin.exists():
        raise FileNotFoundError(f"Candidate solstone-core-journal missing at {journal_bin}")
    if not core_bin.exists():
        raise FileNotFoundError(f"Candidate solstone-core missing at {core_bin}")

    journal_sha = compute_sha256(journal_bin)
    core_sha = compute_sha256(core_bin)

    version_header = ""
    try:
        proc = subprocess.run(
            [str(journal_bin), "--help"],
            capture_output=True,
            text=True,
            timeout=5.0,
            check=False,
        )
        lines = (proc.stdout or proc.stderr).strip().splitlines()
        version_header = "\n".join(lines[:10])
    except Exception:
        version_header = "unavailable"

    return {
        "candidate_dir": str(candidate_dir),
        "solstone_core_journal_path": str(journal_bin),
        "solstone_core_journal_sha256": journal_sha,
        "solstone_core_path": str(core_bin),
        "solstone_core_sha256": core_sha,
        "version_header": version_header[:500],
    }


def classify_bootstrap_response(
    resp: HttpResponse,
    case_name: str,
) -> tuple[str, str]:
    """Classify the HTTP POST response from /app/thinking/api/local/bootstrap."""
    if resp.status == 500:
        detail_str = ""
        if isinstance(resp.json_data, dict):
            detail_str = str(resp.json_data.get("detail") or resp.json_data.get("error") or "")
        if SPAWN_UNAVAILABLE_SNIPPET in detail_str or SPAWN_UNAVAILABLE_SNIPPET in resp.body:
            return (
                "bootstrap_refused",
                "Spawn unavailable in production build (baseline refusal)",
            )
        return (
            "unexpected",
            f"HTTP 500 without expected refusal detail: {resp.body[:200]}",
        )

    if resp.status == 302:
        return (
            "harness_infra",
            f"HTTP 302 redirect encountered (session gate not established): {resp.headers.get('location')}",
        )

    if resp.status in (200, 202):
        if isinstance(resp.json_data, dict):
            install_state = resp.json_data.get("install_state")
            if install_state in ("resolving", "downloading", "verifying", "installing", "installed"):
                return (
                    "admitted",
                    f"Install admitted with in-flight state: {install_state}",
                )
            return (
                "unexpected",
                f"HTTP {resp.status} with unexpected install_state: {install_state}",
            )
        return (
            "unexpected",
            f"HTTP {resp.status} with non-dict JSON body: {resp.body[:200]}",
        )

    if resp.status == 0:
        return (
            "harness_infra",
            f"Connection failure to portal: {resp.error_detail}",
        )

    return (
        "unexpected",
        f"HTTP {resp.status}: {resp.body[:200]}",
    )


def is_leftover_mlx(journal_dir: Path) -> bool:
    """Check if journal local provider status still holds the legacy MLX record."""
    status_file = journal_dir / "health" / "providers" / "local.json"
    if not status_file.exists():
        return False
    try:
        data = json.loads(status_file.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            return False
        if data.get("target_fingerprint_sha256") == "legacy-mlx":
            return True
        target_fp = data.get("target_fingerprint_json")
        if target_fp and '"runtime":"mlx"' in target_fp:
            return True
    except Exception:
        pass
    return False


def wait_until_mlx_replaced(
    journal_dir: Path,
    timeout_seconds: float = 15.0,
    poll_interval: float = 0.2,
) -> bool:
    """Poll health/providers/local.json until the legacy MLX fingerprint is replaced."""
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if not is_leftover_mlx(journal_dir):
            return True
        time.sleep(poll_interval)
    return not is_leftover_mlx(journal_dir)


def check_metal_evidence(journal_dir: Path, log_file: Path) -> bool:
    """Verify Metal execution evidence in runtime argv, health records, or supervisor log."""
    if log_file.exists():
        try:
            log_content = log_file.read_text(encoding="utf-8", errors="replace")
            if "--n-gpu-layers 999" in log_content and "--mmproj" in log_content:
                return True
        except Exception:
            pass

    health_dir = journal_dir / "health"
    if health_dir.exists():
        for json_file in health_dir.rglob("*.json"):
            try:
                content = json_file.read_text(encoding="utf-8", errors="replace")
                if "--n-gpu-layers" in content and "--mmproj" in content:
                    return True
            except Exception:
                pass

    return False


def run_post_admit_checks(
    port: int,
    journal_dir: Path,
    log_file: Path,
    install_timeout_seconds: float = 3600.0,
) -> str:
    """Execute fixed-candidate path after admission (status poll -> activate -> verify)."""
    poll_url = f"http://127.0.0.1:{port}/app/thinking/api/local/bootstrap/status?model=local%2Fqwen3.5-4b"
    deadline = time.monotonic() + install_timeout_seconds
    terminal_state = None

    while time.monotonic() < deadline:
        resp = send_http_request(poll_url, method="GET")
        if resp.status == 200 and isinstance(resp.json_data, dict):
            state = resp.json_data.get("install_state")
            if state in ("installed", "failed"):
                terminal_state = state
                break
        time.sleep(1.0)

    if terminal_state != "installed":
        return f"terminal_state_{terminal_state or 'timeout'}"

    # Activate local provider lane via convey thinking update_providers
    activate_url = f"http://127.0.0.1:{port}/app/thinking/api/providers"
    activate_payload = {"lane": "local", "model": "local/qwen3.5-4b"}
    activate_resp = send_http_request(
        activate_url,
        method="PUT",
        data=activate_payload,
    )
    if activate_resp.status not in (200, 202):
        activate_resp = send_http_request(
            activate_url,
            method="POST",
            data=activate_payload,
        )
    if activate_resp.status not in (200, 202):
        return f"activate_failed_{activate_resp.status}"

    # Verify Metal execution evidence
    if check_metal_evidence(journal_dir, log_file):
        return "passed"

    return "inference_not_proven"


def run_scenario(
    candidate_dir: Path,
    case_dir: Path,
    case_name: str,
    portal_timeout_seconds: float = 60.0,
    install_timeout_seconds: float = 3600.0,
) -> CaseResult:
    """Run an isolated portal test case for the given scenario."""
    journal_dir = setup_case_journal(case_dir, case_name)
    log_file = case_dir / "supervisor.log"
    portal = PortalProcess(candidate_dir, journal_dir, log_file)

    prior_mlx_note = None
    if case_name == "prior_mlx":
        prior_mlx_note = (
            "Prior MLX record loaded in health/providers/local.json with runtime:mlx. "
            "Native target discrimination expects status_targets_native to treat this as non-native."
        )

    try:
        port = portal.start(timeout_seconds=portal_timeout_seconds)
    except PortalStartupError as err:
        return CaseResult(
            name=case_name,
            convey_port=None,
            http_status=0,
            reason_code="startup_error",
            detail=str(err),
            classification="harness_infra",
            note=f"Portal failed to start: {err}",
            prior_mlx_discriminator=prior_mlx_note,
        )

    try:
        bootstrap_url = (
            f"http://127.0.0.1:{port}/app/thinking/api/local/bootstrap?model=local%2Fqwen3.5-4b"
        )
        resp = send_http_request(bootstrap_url, method="POST", timeout=10.0)

        reason_code = None
        detail = resp.error_detail
        if isinstance(resp.json_data, dict):
            reason_code = resp.json_data.get("reason_code")

        classification, note = classify_bootstrap_response(resp, case_name)
        post_admit_outcome = None

        # Check for false in-flight if prior_mlx still retains legacy MLX status
        if classification == "admitted" and case_name == "prior_mlx":
            replaced = wait_until_mlx_replaced(
                journal_dir, timeout_seconds=15.0, poll_interval=0.2
            )
            if not replaced:
                classification = "false_in_flight"
                note = "Portal echoed stale MLX in-flight record instead of admitting a native spawn"

        if classification == "admitted":
            post_admit_outcome = run_post_admit_checks(
                port,
                journal_dir=journal_dir,
                log_file=log_file,
                install_timeout_seconds=install_timeout_seconds,
            )

        return CaseResult(
            name=case_name,
            convey_port=port,
            http_status=resp.status,
            reason_code=reason_code,
            detail=detail,
            classification=classification,
            note=note,
            post_admit_outcome=post_admit_outcome,
            prior_mlx_discriminator=prior_mlx_note,
        )
    finally:
        portal.stop()


def run_harness(
    candidate_dir: Path,
    run_dir: Path,
    portal_timeout_seconds: float = 60.0,
    install_timeout_seconds: float = 3600.0,
) -> HarnessReport:
    """Execute the full local thinking install harness across isolated cases."""
    candidate_dir = candidate_dir.resolve()
    run_dir = run_dir.resolve()
    run_dir.mkdir(parents=True, exist_ok=True)

    provenance = collect_provenance(candidate_dir)
    provenance_path = run_dir / "provenance.json"
    provenance_path.write_text(json.dumps(provenance, indent=2), encoding="utf-8")

    cases_dir = run_dir / "cases"
    cases_dir.mkdir(parents=True, exist_ok=True)

    results: list[CaseResult] = []
    for case_name in ("fresh", "prior_mlx"):
        case_dir = cases_dir / case_name
        case_dir.mkdir(parents=True, exist_ok=True)
        result = run_scenario(
            candidate_dir=candidate_dir,
            case_dir=case_dir,
            case_name=case_name,
            portal_timeout_seconds=portal_timeout_seconds,
            install_timeout_seconds=install_timeout_seconds,
        )
        results.append(result)

    all_admitted = all(r.classification == "admitted" for r in results)
    all_refused = all(r.classification == "bootstrap_refused" for r in results)
    has_false_in_flight = any(r.classification == "false_in_flight" for r in results)
    has_infra_error = any(r.classification == "harness_infra" for r in results)

    if has_infra_error:
        overall_outcome = "harness_infra"
    elif has_false_in_flight:
        overall_outcome = "false_in_flight"
    elif all_refused:
        overall_outcome = "bootstrap_refused"
    elif all_admitted:
        if all(r.post_admit_outcome == "passed" for r in results):
            overall_outcome = "passed"
        elif any(r.post_admit_outcome == "inference_not_proven" for r in results):
            overall_outcome = "inference_not_proven"
        elif any(r.post_admit_outcome and r.post_admit_outcome.startswith("terminal_state_failed") for r in results):
            overall_outcome = "install_failed"
        elif any(r.post_admit_outcome and r.post_admit_outcome.startswith("terminal_state_timeout") for r in results):
            overall_outcome = "install_timeout"
        else:
            overall_outcome = "unexpected"
    else:
        overall_outcome = "unexpected"

    report = HarnessReport(
        provenance=provenance,
        cases=results,
        overall_outcome=overall_outcome,
        timestamp=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        all_passed=(overall_outcome == "passed"),
    )

    # Write receipt files
    receipt_json_path = run_dir / "receipt.json"
    receipt_json_path.write_text(
        json.dumps(asdict(report), indent=2), encoding="utf-8"
    )

    receipt_txt_path = run_dir / "RECEIPT.txt"
    receipt_txt_lines = [
        "============================================================",
        "LOCAL THINKING INSTALL HARNESS RECEIPT",
        "============================================================",
        f"Timestamp:        {report.timestamp}",
        f"Overall Outcome:  {report.overall_outcome.upper()}",
        f"Candidate Dir:    {provenance['candidate_dir']}",
        f"Journal Binary:   {provenance['solstone_core_journal_path']}",
        f"Journal SHA256:   {provenance['solstone_core_journal_sha256'][:16]}...",
        f"Core SHA256:      {provenance['solstone_core_sha256'][:16]}...",
        "------------------------------------------------------------",
        "CASE RESULTS:",
    ]
    for c in report.cases:
        receipt_txt_lines.extend([
            f"  Case:           {c.name}",
            f"  Convey Port:    {c.convey_port}",
            f"  HTTP Status:    {c.http_status}",
            f"  Reason Code:    {c.reason_code or 'none'}",
            f"  Classification: {c.classification}",
            f"  Note:           {c.note}",
        ])
        if c.post_admit_outcome:
            receipt_txt_lines.append(f"  Post-Admit:     {c.post_admit_outcome}")
        if c.prior_mlx_discriminator:
            receipt_txt_lines.append(f"  Discriminator:  {c.prior_mlx_discriminator}")
        receipt_txt_lines.append("------------------------------------------------------------")

    receipt_txt_lines.append(
        "RESULT: " + ("PASS (All admitted and verified)" if report.all_passed else f"FAIL ({report.overall_outcome})")
    )
    receipt_txt_lines.append("============================================================")

    receipt_txt_path.write_text("\n".join(receipt_txt_lines) + "\n", encoding="utf-8")

    return report
