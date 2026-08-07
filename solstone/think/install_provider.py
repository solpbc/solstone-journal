# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Top-level `journal install-provider <name>` — install a provider runtime.

Local-system-only: only meaningful on the host that stores the journal. Moved
here from the old journal-access provider-install surface.
"""

from __future__ import annotations

import argparse
import json
import logging
import sys
from typing import Any

from solstone.think.providers import local_install, mlx_install, parakeet_install
from solstone.think.providers.fit_report import FitReport
from solstone.think.providers.install_lease import acquire_install_lease, probe_install_lease_free
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    begin_or_replace_install_attempt,
    canonical_fingerprint,
    fingerprint_sha256,
    observe_install_attempt,
    read_install_status,
)
from solstone.think.utils import require_solstone

PARAKEET_DOWNLOAD_DISCLOSURE = (
    "parakeet-cpp fetches two external artifacts into this journal's provider "
    "cache before it can run: the parakeet.cpp server binary from github.com "
    "(MIT) and the speech model from huggingface.co (CC-BY-4.0)."
)

LOCAL_DOWNLOAD_DISCLOSURE = (
    "local model assets: downloading the llama.cpp runtime from "
    "updates.solstone.app (MIT; the CUDA build also carries NVIDIA-licensed "
    "runtime components; see THIRD_PARTY_NOTICES.md) or github.com (MIT), "
    "and the model from huggingface.co (Apache-2.0)."
)
LOG = logging.getLogger(__name__)


def _render_fit_report(report: FitReport) -> None:
    from solstone.think.providers import fit_report

    print(fit_report.render_fit_report(report), file=sys.stderr)


def _target_sha(fingerprint: dict) -> str:
    return fingerprint_sha256(canonical_fingerprint(fingerprint))


def _progress_line(status: dict) -> None:
    received = status.get("progress_bytes_received")
    total = status.get("progress_bytes_total")
    progress = ""
    if received is not None:
        progress = f" {received}"
        if total is not None:
            progress += f"/{total}"
    print(
        f"observing {status['provider']} install: {status['install_state']}{progress}",
        file=sys.stderr,
    )


def _observe_same_target(provider: str, target_sha: str) -> int:
    status = read_install_status(name=provider)
    if (
        status["install_state"] not in IN_FLIGHT_STATES
        or status["target_fingerprint_sha256"] != target_sha
    ):
        print(
            f"{provider} install already running for a different target",
            file=sys.stderr,
        )
        return 1
    final = observe_install_attempt(
        provider,
        target_fingerprint_sha256=target_sha,
        timeout_s=60.0 * 60.0,
        progress=_progress_line,
    )
    if final is None:
        print(f"timed out observing {provider} install", file=sys.stderr)
        return 1
    print(json.dumps(final, indent=2))
    return 0 if final["install_state"] == "installed" else 1


def _status_exit_code(status: dict[str, Any]) -> int:
    return 1 if status.get("install_state") == "failed" else 0


def _handle_install_failure(provider: str, exc: Exception) -> int:
    print(str(exc), file=sys.stderr)
    try:
        status = read_install_status(name=provider)
    except Exception as status_exc:
        print(
            f"could not read persisted {provider} install status: {status_exc}",
            file=sys.stderr,
        )
        LOG.warning(
            "could not read persisted %s install status after failure",
            provider,
            exc_info=True,
        )
        return 1
    print(json.dumps(status, indent=2))
    return 1


def _is_mlx_backend() -> bool:
    return mlx_install.is_mlx_platform_supported()


def _install_mlx_local() -> int:
    spec = mlx_install.resolve_model_spec()
    readiness = mlx_install.inspect_readiness(spec.name)
    if readiness.ready:
        print("local already installed", file=sys.stderr)
        print(json.dumps(read_install_status(name="local"), indent=2))
        return 0
    fingerprint = mlx_install.target_fingerprint(spec.name)
    target_sha = _target_sha(fingerprint)
    if not probe_install_lease_free("local"):
        return _observe_same_target("local", target_sha)
    try:
        from solstone.think.providers import fit_report

        _render_fit_report(fit_report.build_mlx_fit_report(spec.name))
        status = mlx_install.install_local_mlx(spec.name, owner={"entry": "install_provider"})
    except mlx_install.MLXInstallBusyError:
        return _observe_same_target("local", target_sha)
    except Exception as exc:
        return _handle_install_failure("local", exc)
    print(json.dumps(status, indent=2))
    return _status_exit_code(status)


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="journal install-provider",
        description="Install or retry a provider runtime.",
    )
    parser.add_argument("name", help="Provider to install: 'local' or 'parakeet'.")
    args = parser.parse_args()

    require_solstone()

    if args.name not in {"local", "parakeet"}:
        print(
            f"unsupported provider {args.name!r}; supported: local, parakeet",
            file=sys.stderr,
        )
        return 2

    if args.name == "parakeet":
        print(PARAKEET_DOWNLOAD_DISCLOSURE, file=sys.stderr)
        readiness = parakeet_install.inspect_readiness()
        if readiness.ready:
            print("parakeet already installed", file=sys.stderr)
            print(json.dumps(read_install_status(name="parakeet"), indent=2))
            return 0
        fingerprint = parakeet_install.target_fingerprint()
        target_sha = _target_sha(fingerprint)
        lease = acquire_install_lease("parakeet")
        if lease is None:
            return _observe_same_target("parakeet", target_sha)
        try:
            from solstone.think.providers import fit_report

            _render_fit_report(fit_report.build_parakeet_fit_report())
            attempt_status = begin_or_replace_install_attempt(
                "parakeet",
                fingerprint,
                initial_state="resolving",
                owner={"entry": "install_provider"},
            )
            status = parakeet_install.install_parakeet(
                lease=lease,
                attempt_status=attempt_status,
            )
        except Exception as exc:
            return _handle_install_failure("parakeet", exc)
        finally:
            lease.release()
        print(json.dumps(status, indent=2))
        return _status_exit_code(status)

    if _is_mlx_backend():
        return _install_mlx_local()

    print(LOCAL_DOWNLOAD_DISCLOSURE, file=sys.stderr)
    readiness = local_install.inspect_readiness()
    if readiness.ready:
        print("local already installed", file=sys.stderr)
        print(json.dumps(read_install_status(name="local"), indent=2))
        return 0
    fingerprint = local_install.target_fingerprint()
    target_sha = _target_sha(fingerprint)
    if not probe_install_lease_free("local"):
        return _observe_same_target("local", target_sha)
    try:
        from solstone.think.providers import fit_report

        _render_fit_report(fit_report.build_local_fit_report(local_install.LOCAL_MODEL))
        status = local_install.install_local(owner={"entry": "install_provider"})
    except local_install.LocalInstallBusyError:
        return _observe_same_target("local", target_sha)
    except Exception as exc:
        return _handle_install_failure("local", exc)
    print(json.dumps(status, indent=2))
    return _status_exit_code(status)


if __name__ == "__main__":
    raise SystemExit(main())
