# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Test harness orchestration, classification, and receipt generation."""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from .fixtures import setup_case_journal
from .portal import HttpResponse, PortalProcess, PortalStartupError, find_free_port, send_http_request

SPAWN_UNAVAILABLE_SNIPPET = "local install can't be started from this build yet"
MODEL_TOTAL_BYTES = 2_740_937_888 + 672_423_616  # 3,413,361,504


class PrerequisiteError(Exception):
    """Raised when candidate directory fails prerequisite checks."""


@dataclass
class CaseResult:
    name: str
    convey_port: int | None
    direct_port: int | None
    http_status: int
    reason_code: str | None
    detail: str | None
    classification: str
    note: str
    post_admit_outcome: str | None = None
    prior_mlx_discriminator: dict[str, Any] | None = None
    cleanup_error: str | None = None
    generate_answer: str | None = None
    metal_evidence: str | None = None


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


def find_or_build_helper() -> Path:
    """Locate or build the tools-only Cargo helper binary."""
    helper_dir = Path(__file__).resolve().parent / "helper"
    target_bin = helper_dir / "target" / "debug" / "solstone-local-thinking-install-helper"

    env = dict(os.environ)
    if "BINDGEN_EXTRA_CLANG_ARGS" not in env:
        for p in ("/usr/lib/clang", "/usr/lib64/clang"):
            if os.path.exists(p):
                for root, dirs, _ in os.walk(p):
                    if "include" in dirs and os.path.exists(os.path.join(root, "include", "limits.h")):
                        env["BINDGEN_EXTRA_CLANG_ARGS"] = f"-I{os.path.join(root, 'include')}"
                        break
                if "BINDGEN_EXTRA_CLANG_ARGS" in env:
                    break

    cmd = ["cargo", "build", "--locked", "--manifest-path", str(helper_dir / "Cargo.toml")]
    proc = subprocess.run(cmd, env=env, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise RuntimeError(f"Failed to build local thinking install helper: {proc.stderr}")
    if not target_bin.exists():
        raise RuntimeError(f"Helper binary not found after build at {target_bin}")
    return target_bin


def helper_admit(helper_bin: Path, root_dir: Path, journal_dir: Path) -> dict[str, Any]:
    cmd = [
        str(helper_bin),
        "admit",
        "--root",
        str(root_dir.resolve()),
        "--journal",
        str(journal_dir.resolve()),
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False, timeout=30)
    try:
        data = json.loads(proc.stdout)
        if proc.returncode == 2:
            return {"ok": False, "error": "namespace_exists", "data": data}
        if proc.returncode != 0:
            return {"ok": False, "error": f"Helper exit {proc.returncode}: {data}"}
        return data
    except Exception:
        return {"ok": False, "error": f"Helper failed (exit {proc.returncode}): {proc.stderr}"}


def helper_inspect_status(helper_bin: Path, journal_dir: Path) -> dict[str, Any]:
    cmd = [
        str(helper_bin),
        "inspect-status",
        "--journal",
        str(journal_dir.resolve()),
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False, timeout=30)
    try:
        data = json.loads(proc.stdout)
        if proc.returncode != 0:
            return {"ok": False, "error": f"Helper exit {proc.returncode}: {data}"}
        return data
    except Exception:
        return {"ok": False, "error": f"Helper failed (exit {proc.returncode}): {proc.stderr}"}


def helper_namespace_path(helper_bin: Path, root_dir: Path) -> dict[str, Any]:
    cmd = [
        str(helper_bin),
        "namespace-path",
        "--root",
        str(root_dir.resolve()),
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False, timeout=30)
    try:
        data = json.loads(proc.stdout)
        if proc.returncode != 0:
            return {"ok": False, "error": f"Helper exit {proc.returncode}: {data}"}
        return data
    except Exception:
        return {"ok": False, "error": f"Helper failed (exit {proc.returncode}): {proc.stderr}"}


def validate_candidate_dir(candidate_dir: Path) -> None:
    if not candidate_dir.exists() or not candidate_dir.is_dir():
        raise PrerequisiteError(f"Candidate directory does not exist: {candidate_dir}")

    missing: list[str] = []
    req_bins = (
        "solstone-core-journal",
        "solstone-core",
        "solstone-core-sol",
        "solstone-core-speakers-analyze",
        "solstone-core-vad-analyze",
    )
    for name in req_bins:
        bin_path = candidate_dir / name
        if not bin_path.exists():
            missing.append(name)
        elif not os.access(bin_path, os.X_OK):
            missing.append(f"{name} (not executable)")

    if missing:
        if any("solstone-core-speakers-analyze" in m for m in missing):
            raise PrerequisiteError(
                f"Missing candidate binaries ({', '.join(missing)}). "
                "Run `make build` and `make build-sandbox-processing` "
                "(ONNX payload under `core/target/lib/solstone-core-speakers-analyze`; "
                "transcription models already in `core/models/assets`)."
            )
        raise PrerequisiteError(f"Missing candidate binaries: {', '.join(missing)}")


def setup_disposable_source_root(
    case_dir: Path,
    candidate_dir: Path,
    repo_root: Path,
) -> Path:
    source_root = case_dir / "source-root"
    if source_root.exists():
        raise FileExistsError(f"Disposable source-root already exists: {source_root}")
    source_root.mkdir(parents=True, exist_ok=False)

    (source_root / "pyproject.toml").write_text(
        '[project]\nname = "local-thinking-install-harness"\nversion = "0.1.0"\n',
        encoding="utf-8",
    )
    (source_root / ".git").mkdir(parents=True, exist_ok=True)

    for relative in ("core/payload", "core/models/assets"):
        src = repo_root / relative
        if not src.is_dir():
            raise FileNotFoundError(f"Missing complete candidate payload: {src}")
        shutil.copytree(src, source_root / relative)

    debug_dir = source_root / "core" / "target" / "debug"
    debug_dir.mkdir(parents=True, exist_ok=True)

    req_bins = [
        "solstone-core-journal",
        "solstone-core",
        "solstone-core-sol",
        "solstone-core-speakers-analyze",
        "solstone-core-vad-analyze",
    ]
    for b in req_bins:
        src_b = candidate_dir / b
        dest_b = debug_dir / b
        shutil.copy2(src_b, dest_b)
        os.chmod(dest_b, 0o755)

    if (candidate_dir / "solstone").exists():
        shutil.copy2(candidate_dir / "solstone", debug_dir / "solstone")
        os.chmod(debug_dir / "solstone", 0o755)

    candidate_lib = candidate_dir.parent / "lib" / "solstone-core-speakers-analyze"
    if not candidate_lib.is_dir():
        raise FileNotFoundError(f"Missing staged ONNX library: {candidate_lib}")
    if candidate_lib.exists():
        dest_lib = source_root / "core" / "target" / "lib" / "solstone-core-speakers-analyze"
        dest_lib.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(candidate_lib, dest_lib, dirs_exist_ok=True)

    return source_root


def cleanup_namespace(admission: dict[str, Any], source_root: Path) -> str | None:
    """Remove only the namespace this invocation successfully admitted."""
    try:
        if admission.get("ok") is not True or admission.get("root") != str(source_root.resolve()):
            return "Refusing cleanup without matching successful admission"
        ns_path = Path(admission["namespace_path"])
        ns_hex = admission["namespace_hex"]
        if (len(ns_hex) != 64 or any(c not in "0123456789abcdef" for c in ns_hex)
                or ns_path.name != ns_hex or ns_path.parent.name != "namespaces"
                or not ns_path.is_absolute() or ns_path.is_symlink()):
            return f"Refusing unsafe namespace path: {ns_path}"
        if ns_path.exists():
            backup = source_root.parent / "identity-before-cleanup"
            shutil.copytree(ns_path, backup)
            shutil.rmtree(ns_path)
        return None
    except Exception as err:
        return f"Cleanup failed: {err}"


def collect_provenance(candidate_dir: Path) -> dict[str, Any]:
    journal_bin = candidate_dir / "solstone-core-journal"
    core_bin = candidate_dir / "solstone-core"

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
    if resp.status == 500:
        detail_str = ""
        if isinstance(resp.json_data, dict):
            detail_str = str(resp.json_data.get("detail") or resp.json_data.get("error") or "")
        if (isinstance(resp.json_data, dict)
                and resp.json_data.get("reason_code") == "settings_operation_failed"
                and SPAWN_UNAVAILABLE_SNIPPET in detail_str):
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


def is_leftover_mlx(helper_bin: Path, journal_dir: Path) -> bool:
    inspect_status = helper_inspect_status(helper_bin, journal_dir)
    if not inspect_status.get("ok"):
        return True
    sha = inspect_status.get("target_fingerprint_sha256")
    fp_json = inspect_status.get("target_fingerprint_json") or ""
    return bool(sha == "legacy-mlx" or '"runtime":"mlx"' in fp_json)


def wait_until_mlx_replaced(
    helper_bin: Path,
    journal_dir: Path,
    timeout_seconds: float = 15.0,
    poll_interval: float = 0.2,
) -> bool:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        inspect = helper_inspect_status(helper_bin, journal_dir)
        if inspect.get("ok"):
            sha = inspect.get("target_fingerprint_sha256")
            fp_json = inspect.get("target_fingerprint_json") or ""
            is_mlx = (sha == "legacy-mlx" or '"runtime":"mlx"' in fp_json)
            attempt_id = inspect.get("attempt_id")
            if not is_mlx and attempt_id != "018e4f1a2b3c4d5e0000000000000001":
                if sys.platform == "darwin":
                    if inspect.get("native"):
                        return True
                else:
                    return True
        time.sleep(poll_interval)
    return False


def run_generate_proof(
    staged_core_bin: Path,
    journal_dir: Path,
    case_dir: Path,
) -> tuple[bool, str, str]:
    request_payload = {
        "schema": "solstone-generate-request-v2",
        "id": "portal-bootstrap-proof",
        "context": "portal.bootstrap.validation",
        "contents": [
            {
                "type": "text",
                "text": "What is two plus two? Answer with the number only.",
            }
        ],
        "max_output_tokens": 128,
        "timeout_s": 120,
        "enforce_responsiveness": False,
        "temperature": 0.3,
        "json_output": False,
        "json_schema": None,
        "attempt_index": 0,
        "exclusive_admission": False,
        "system_instruction": None,
        "thinking_budget": None,
        "transport_retries": None,
    }
    input_bytes = json.dumps(request_payload).encode("utf-8")
    env = dict(os.environ)
    env["SOLSTONE_JOURNAL"] = str(journal_dir.resolve())
    env["PATH"] = f"{staged_core_bin.parent}:{os.environ.get('PATH', '')}"

    try:
        proc = subprocess.run(
            [str(staged_core_bin), "generate", "--one-shot"],
            input=input_bytes,
            capture_output=True,
            env=env,
            timeout=130.0,
            check=False,
        )
    except Exception as err:
        return False, "", f"Failed to execute solstone-core generate: {err}"

    stdout_text = proc.stdout.decode("utf-8", errors="replace")
    stderr_text = proc.stderr.decode("utf-8", errors="replace")

    (case_dir / "generate_stdout.json").write_text(stdout_text, encoding="utf-8")
    (case_dir / "generate_stderr.log").write_text(stderr_text, encoding="utf-8")

    if proc.returncode != 0:
        return False, "", f"solstone-core generate exited {proc.returncode}: {stderr_text[:200]}"

    try:
        resp_data = json.loads(stdout_text)
        if (resp_data.get("schema") != "solstone-generate-response-v2"
                or resp_data.get("id") != "portal-bootstrap-proof"):
            return False, "", "Wrong generate response identity"
        if resp_data.get("outcome") == "generated":
            if resp_data.get("model") != "local/qwen3.5-4b" or not resp_data.get("inference"):
                return False, "", "Response lacks bundled local model/inference evidence"
            text = (resp_data.get("text") or "").strip()
            if text:
                return True, text, ""
            return False, "", "Response outcome was generated but text was empty"
        return False, "", f"Response outcome was {resp_data.get('outcome')}"
    except Exception as err:
        return False, "", f"Failed to parse generate response JSON: {err}"


def check_metal_evidence_darwin(journal_dir: Path) -> tuple[bool, str]:
    runtime_status_file = journal_dir / "health" / "providers" / "runtime" / "local.json"
    if not runtime_status_file.exists():
        return False, "health/providers/runtime/local.json not found"

    try:
        runtime_data = json.loads(runtime_status_file.read_text(encoding="utf-8"))
        process_info = runtime_data.get("process") or {}
        pid = process_info.get("pid")
        if not pid:
            return False, "No PID in runtime health record"
    except Exception as err:
        return False, f"Failed to read runtime health record: {err}"

    # Get cmdline tokens
    if sys.platform == "darwin":
        ps_proc = subprocess.run(
            ["ps", "-p", str(pid), "-www", "-o", "args="],
            capture_output=True,
            text=True,
            check=False,
        )
        if ps_proc.returncode != 0 or not ps_proc.stdout.strip():
            return False, f"Process {pid} is not running"
        tokens = shlex.split(ps_proc.stdout.strip())
    else:
        cmdline_file = Path(f"/proc/{pid}/cmdline")
        if not cmdline_file.exists():
            return False, f"Process {pid} /proc cmdline not found"
        try:
            cmdline_bytes = cmdline_file.read_bytes()
            tokens = [t.decode("utf-8", errors="replace") for t in cmdline_bytes.split(b"\x00") if t]
        except Exception as err:
            return False, f"Failed to read /proc/{pid}/cmdline: {err}"

    (journal_dir.parent / "runtime-process.json").write_text(json.dumps({"health": runtime_data, "argv": tokens}, indent=2))
    if not tokens or not Path(tokens[0]).resolve().is_relative_to(journal_dir.resolve()) or "llama-server" not in Path(tokens[0]).name:
        return False, f"Process {pid} tokens do not contain llama-server: {tokens[:5]}"

    has_gpu_layers_999 = any(
        tokens[i] == "--n-gpu-layers" and i + 1 < len(tokens) and tokens[i + 1] == "999"
        for i in range(len(tokens))
    )
    if not has_gpu_layers_999:
        return False, f"Runtime argv tokens missing adjacent --n-gpu-layers 999: {tokens}"

    if "--kv-unified" not in tokens:
        return False, f"Runtime argv tokens missing --kv-unified: {tokens}"
    if "--mmproj" not in tokens:
        return False, f"Runtime argv tokens missing --mmproj: {tokens}"

    chronicle_dir = journal_dir / "chronicle"
    oplog_files = list(chronicle_dir.glob("*/health/oplog--*"))
    evidence = []
    for oplog in oplog_files:
        content = oplog.read_text(encoding="utf-8", errors="replace")
        evidence.extend(f"{oplog}: {line.strip()}" for line in content.splitlines()
                        if "ggml_metal" in line or "offloaded" in line)
    excerpt = "\n".join(evidence)
    (journal_dir.parent / "metal-evidence.log").write_text(excerpt)
    if "ggml_metal" not in excerpt or "offloaded" not in excerpt:
        return False, "Missing Metal initialization or layer-offload evidence in managed oplogs"
    return True, excerpt


def run_post_admit_checks(
    port: int,
    staged_core_bin: Path,
    journal_dir: Path,
    case_name: str,
    case_dir: Path,
    helper_bin: Path,
    install_timeout_seconds: float = 3600.0,
) -> tuple[str, str | None, str | None]:
    poll_url = f"http://127.0.0.1:{port}/app/thinking/api/local/bootstrap/status?model=local%2Fqwen3.5-4b"
    deadline = time.monotonic() + install_timeout_seconds
    terminal_state = None

    progress_file = case_dir / "progress.jsonl"
    progress_samples: list[dict[str, Any]] = []

    while time.monotonic() < deadline:
        resp = send_http_request(poll_url, method="GET")
        if resp.status == 200 and isinstance(resp.json_data, dict):
            state = resp.json_data.get("install_state")
            bytes_rx = resp.json_data.get("progress_bytes_received")
            bytes_tot = resp.json_data.get("progress_bytes_total")

            sample = {
                "t": time.time(),
                "install_state": state,
                "progress_bytes_received": bytes_rx,
                "progress_bytes_total": bytes_tot,
            }
            progress_samples.append(sample)
            with open(progress_file, "a", encoding="utf-8") as pf:
                pf.write(json.dumps(sample) + "\n")
            if state in ("installed", "failed"):
                terminal_state = state
                break
        time.sleep(1.0)

    if terminal_state != "installed":
        return f"terminal_state_{terminal_state or 'timeout'}", None, None

    # Validate progress samples for green fresh case
    if case_name == "fresh":
        dl_samples = [s for s in progress_samples if s["install_state"] == "downloading"
                      and s.get("progress_bytes_total") == MODEL_TOTAL_BYTES]
        if not dl_samples:
            return "fresh_missing_download_progress", None, None

        has_model_total = any(s.get("progress_bytes_total") == MODEL_TOTAL_BYTES for s in dl_samples)
        if not has_model_total:
            return "fresh_missing_expected_bytes_total", None, None

        rx_values = [s["progress_bytes_received"] for s in dl_samples if s.get("progress_bytes_received") is not None]
        if not rx_values:
            return "fresh_missing_bytes_received", None, None

        if any(v < 0 or v > MODEL_TOTAL_BYTES for v in rx_values):
            return "fresh_progress_out_of_bounds", None, None
        if any(b < a for a, b in zip(rx_values, rx_values[1:])):
            return "fresh_progress_decreased", None, None
        if len(set(rx_values)) < 2:
            return "fresh_progress_did_not_advance", None, None

    # Switch lane to local via PUT /app/thinking/api/providers
    activate_url = f"http://127.0.0.1:{port}/app/thinking/api/providers"
    activate_resp = send_http_request(
        activate_url,
        method="PUT",
        data={"lane": "local"},
    )
    if activate_resp.status not in (200, 202):
        return f"activate_failed_{activate_resp.status}", None, None

    # Poll /app/thinking/api/local/runtime until phase == "ready"
    runtime_url = f"http://127.0.0.1:{port}/app/thinking/api/local/runtime"
    runtime_ready = False
    runtime_deadline = time.monotonic() + install_timeout_seconds
    while time.monotonic() < runtime_deadline:
        r_resp = send_http_request(runtime_url, method="GET")
        if r_resp.status == 200 and isinstance(r_resp.json_data, dict):
            if r_resp.json_data.get("phase") == "ready":
                runtime_ready = True
                break
        time.sleep(1.0)

    if not runtime_ready:
        return "runtime_not_ready", None, None

    # Run generate proof
    gen_ok, gen_text, gen_err = run_generate_proof(staged_core_bin, journal_dir, case_dir)
    if not gen_ok:
        return f"generate_failed: {gen_err}", None, None

    # Check Metal evidence on Darwin
    metal_evidence_str = "skipped_not_darwin"
    if sys.platform == "darwin":
        metal_ok, metal_detail = check_metal_evidence_darwin(journal_dir)
        if not metal_ok:
            return f"metal_evidence_failed: {metal_detail}", gen_text, None
        metal_evidence_str = metal_detail

    # Terminal inspect status
    final_inspect = helper_inspect_status(helper_bin, journal_dir)
    (case_dir / "final-install-status.json").write_text(json.dumps(final_inspect, indent=2))
    if not final_inspect.get("ok"):
        return f"final_status_unreadable: {final_inspect.get('error')}", gen_text, metal_evidence_str

    if sys.platform == "darwin" and final_inspect.get("native") is not True:
        return "final_status_not_native", gen_text, metal_evidence_str
    if final_inspect.get("install_state") != "installed":
        return "final_status_not_installed", gen_text, metal_evidence_str

    if case_name == "prior_mlx":
        if is_leftover_mlx(helper_bin, journal_dir):
            return "prior_mlx_still_leftover_at_finish", gen_text, metal_evidence_str

    return "passed", gen_text, metal_evidence_str


def run_scenario(
    candidate_dir: Path,
    case_dir: Path,
    case_name: str,
    repo_root: Path,
    helper_bin: Path,
    portal_timeout_seconds: float = 60.0,
    install_timeout_seconds: float = 3600.0,
) -> CaseResult:
    if case_dir.exists():
        return CaseResult(
            name=case_name,
            convey_port=None,
            direct_port=None,
            http_status=0,
            reason_code="case_dir_exists",
            detail=f"Case directory already exists: {case_dir}. Exclusive case dirs required.",
            classification="harness_infra",
            note=f"Case directory already exists: {case_dir}",
        )

    case_dir.mkdir(parents=True, exist_ok=False)
    journal_dir = setup_case_journal(case_dir, case_name)
    source_root = setup_disposable_source_root(case_dir, candidate_dir, repo_root)
    log_file = case_dir / "supervisor.log"

    prior_mlx_discriminator = None
    if case_name == "prior_mlx":
        initial_inspect = helper_inspect_status(helper_bin, journal_dir)
        prior_mlx_discriminator = initial_inspect
        if not initial_inspect.get("ok") or initial_inspect.get("native") is not False:
            return CaseResult(
                name=case_name,
                convey_port=None,
                direct_port=None,
                http_status=0,
                reason_code="fixture_error",
                detail=f"Prior MLX initial status verification failed: {initial_inspect}",
                classification="harness_infra",
                note="Prior MLX initial inspection did not return {ok: true, native: false}",
                prior_mlx_discriminator=prior_mlx_discriminator,
            )

    admit_result = helper_admit(helper_bin, source_root, journal_dir)
    if not admit_result.get("ok"):
        return CaseResult(
            name=case_name,
            convey_port=None,
            direct_port=None,
            http_status=0,
            reason_code="admission_failed",
            detail=str(admit_result.get("error")),
            classification="harness_infra",
            note=f"Setup admission refused: {admit_result.get('error')}",
            prior_mlx_discriminator=prior_mlx_discriminator,
        )

    (case_dir / "admission.json").write_text(json.dumps(admit_result, indent=2))
    direct_port = find_free_port()
    staged_journal_bin = source_root / "core" / "target" / "debug" / "solstone-core-journal"
    staged_core_bin = source_root / "core" / "target" / "debug" / "solstone-core"
    portal = PortalProcess(
        staged_journal_bin=staged_journal_bin,
        journal_dir=journal_dir,
        log_file=log_file,
        direct_port=direct_port,
        case_dir=case_dir,
    )

    convey_port: int | None = None
    try:
        convey_port = portal.start(timeout_seconds=portal_timeout_seconds)
    except BaseException as err:
        portal_stop_err = portal.stop()
        ns_cleanup_err = cleanup_namespace(admit_result, source_root) if not portal_stop_err else "Namespace retained because process cleanup failed"
        cleanup_errors = [e for e in (portal_stop_err, ns_cleanup_err) if e]
        return CaseResult(
            name=case_name,
            convey_port=None,
            direct_port=direct_port,
            http_status=0,
            reason_code="startup_error",
            detail=str(err),
            classification="harness_infra",
            note=f"Portal failed to start: {err}",
            prior_mlx_discriminator=prior_mlx_discriminator,
            cleanup_error="; ".join(cleanup_errors) if cleanup_errors else None,
        )

    res: CaseResult | None = None
    try:
        bootstrap_url = (
            f"http://127.0.0.1:{convey_port}/app/thinking/api/local/bootstrap?model=local%2Fqwen3.5-4b"
        )
        resp = send_http_request(bootstrap_url, method="POST", timeout=10.0)

        (case_dir / "bootstrap.json").write_text(
            json.dumps(
                {
                    "url": bootstrap_url,
                    "status": resp.status,
                    "reason_code": resp.json_data.get("reason_code") if isinstance(resp.json_data, dict) else None,
                    "detail": resp.error_detail,
                    "body": resp.body,
                },
                indent=2,
            ),
            encoding="utf-8",
        )

        reason_code = None
        detail = resp.error_detail
        if isinstance(resp.json_data, dict):
            reason_code = resp.json_data.get("reason_code")

        classification, note = classify_bootstrap_response(resp, case_name)
        post_admit_outcome = None
        gen_answer = None
        metal_ev = None

        if classification == "admitted" and case_name == "prior_mlx":
            replaced = wait_until_mlx_replaced(helper_bin, journal_dir, timeout_seconds=15.0, poll_interval=0.2)
            if not replaced:
                classification = "false_in_flight"
                note = "Portal echoed stale MLX in-flight record instead of admitting a native spawn"

        if classification == "admitted":
            post_admit_outcome, gen_answer, metal_ev = run_post_admit_checks(
                port=convey_port,
                staged_core_bin=staged_core_bin,
                journal_dir=journal_dir,
                case_name=case_name,
                case_dir=case_dir,
                helper_bin=helper_bin,
                install_timeout_seconds=install_timeout_seconds,
            )

        res = CaseResult(
            name=case_name,
            convey_port=convey_port,
            direct_port=direct_port,
            http_status=resp.status,
            reason_code=reason_code,
            detail=detail,
            classification=classification,
            note=note,
            post_admit_outcome=post_admit_outcome,
            prior_mlx_discriminator=prior_mlx_discriminator,
            generate_answer=gen_answer,
            metal_evidence=metal_ev,
        )
        return res
    finally:
        portal_stop_err = portal.stop()
        ns_cleanup_err = cleanup_namespace(admit_result, source_root) if not portal_stop_err else "Namespace retained because process cleanup failed"
        cleanup_errors = [e for e in (portal_stop_err, ns_cleanup_err) if e]
        if cleanup_errors and res is not None:
            res.cleanup_error = "; ".join(cleanup_errors)


def run_harness(
    candidate_dir: Path,
    run_dir: Path,
    portal_timeout_seconds: float = 60.0,
    install_timeout_seconds: float = 3600.0,
) -> HarnessReport:
    candidate_dir = candidate_dir.resolve()
    run_dir = run_dir.resolve()
    run_dir.mkdir(parents=True, exist_ok=False)
    repo_root = Path(__file__).resolve().parent.parent.parent

    try:
        validate_candidate_dir(candidate_dir)
    except PrerequisiteError as err:
        provenance: dict[str, Any] = {"candidate_dir": str(candidate_dir), "error": str(err)}
        run_dir.mkdir(parents=True, exist_ok=True)
        report = HarnessReport(
            provenance=provenance,
            cases=[],
            overall_outcome="harness_infra",
            timestamp=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            all_passed=False,
        )
        receipt_json = run_dir / "receipt.json"
        receipt_json.write_text(json.dumps(asdict(report), indent=2), encoding="utf-8")
        receipt_txt = run_dir / "RECEIPT.txt"
        receipt_txt.write_text(
            f"OVERALL OUTCOME: HARNESS_INFRA\n\nPrerequisite error: {err}\n", encoding="utf-8"
        )
        return report

    helper_bin = find_or_build_helper()

    run_dir.mkdir(parents=True, exist_ok=True)
    provenance = collect_provenance(candidate_dir)
    provenance["helper_sha256"] = compute_sha256(helper_bin)
    harness_root = Path(__file__).resolve().parent
    provenance["harness_files"] = {
        str(p.relative_to(harness_root)): compute_sha256(p)
        for p in sorted(harness_root.rglob("*")) if p.is_file()
        and "target" not in p.parts and "__pycache__" not in p.parts
    }
    provenance_path = run_dir / "provenance.json"
    provenance_path.write_text(json.dumps(provenance, indent=2), encoding="utf-8")

    cases_dir = run_dir / "cases"
    cases_dir.mkdir(parents=True, exist_ok=True)

    results: list[CaseResult] = []
    for case_name in ("fresh", "prior_mlx"):
        case_dir = cases_dir / case_name
        result = run_scenario(
            candidate_dir=candidate_dir,
            case_dir=case_dir,
            case_name=case_name,
            repo_root=repo_root,
            helper_bin=helper_bin,
            portal_timeout_seconds=portal_timeout_seconds,
            install_timeout_seconds=install_timeout_seconds,
        )
        results.append(result)

    all_admitted = all(r.classification == "admitted" for r in results)
    all_refused = all(r.classification == "bootstrap_refused" for r in results)
    has_false_in_flight = any(r.classification == "false_in_flight" for r in results)
    has_infra_error = any(r.classification == "harness_infra" or r.cleanup_error for r in results)

    if has_infra_error:
        overall_outcome = "harness_infra"
    elif has_false_in_flight:
        overall_outcome = "false_in_flight"
    elif all_refused:
        overall_outcome = "bootstrap_refused"
    elif all_admitted:
        if all(r.post_admit_outcome == "passed" for r in results):
            overall_outcome = "passed"
        elif any(r.post_admit_outcome and "metal" in r.post_admit_outcome for r in results):
            overall_outcome = "inference_not_proven"
        elif any(r.post_admit_outcome and "generate" in r.post_admit_outcome for r in results):
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
        f"Candidate Dir:    {provenance.get('candidate_dir')}",
        f"Journal Binary:   {provenance.get('solstone_core_journal_path')}",
        f"Journal SHA256:   {str(provenance.get('solstone_core_journal_sha256', ''))[:16]}...",
        f"Core SHA256:      {str(provenance.get('solstone_core_sha256', ''))[:16]}...",
        "------------------------------------------------------------",
        "CASE RESULTS:",
    ]
    for c in report.cases:
        receipt_txt_lines.extend([
            f"  Case:           {c.name}",
            f"  Convey Port:    {c.convey_port}",
            f"  Direct Port:    {c.direct_port}",
            f"  HTTP Status:    {c.http_status}",
            f"  Reason Code:    {c.reason_code or 'none'}",
            f"  Classification: {c.classification}",
            f"  Note:           {c.note}",
        ])
        if c.post_admit_outcome:
            receipt_txt_lines.append(f"  Post-Admit:     {c.post_admit_outcome}")
        if c.generate_answer:
            receipt_txt_lines.append(f"  Generate Ans:   {c.generate_answer}")
        if c.metal_evidence:
            receipt_txt_lines.append(f"  Metal Evidence: {c.metal_evidence}")
        if c.prior_mlx_discriminator:
            receipt_txt_lines.append(f"  Discriminator:  {json.dumps(c.prior_mlx_discriminator)}")
        if c.cleanup_error:
            receipt_txt_lines.append(f"  Cleanup Error:  {c.cleanup_error}")
        receipt_txt_lines.append("------------------------------------------------------------")

    receipt_txt_lines.append(
        "RESULT: " + ("PASS (All admitted and verified)" if report.all_passed else f"FAIL ({report.overall_outcome})")
    )
    receipt_txt_lines.append("============================================================")

    receipt_txt_path.write_text("\n".join(receipt_txt_lines) + "\n", encoding="utf-8")

    return report
