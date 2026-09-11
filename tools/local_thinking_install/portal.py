# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Portal process lifecycle and HTTP interaction helpers."""

from __future__ import annotations

import json
import os
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


class PortalStartupError(Exception):
    """Raised when the supervisor portal fails to start or become ready."""


class NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    """HTTP handler that prevents following redirects so 302s can be inspected."""

    def redirect_request(
        self,
        req: urllib.request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> urllib.request.Request | None:
        return None


@dataclass
class HttpResponse:
    status: int
    headers: dict[str, str]
    body: str
    json_data: dict[str, Any] | None
    error_detail: str | None


def find_free_port() -> int:
    """Find an unused TCP port by binding to 127.0.0.1:0."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class PortalProcess:
    """Manages the lifecycle of a real supervisor portal process tree."""

    def __init__(
        self,
        staged_journal_bin: Path,
        journal_dir: Path,
        log_file: Path,
        direct_port: int,
        case_dir: Path | None = None,
    ) -> None:
        self.staged_journal_bin = staged_journal_bin.resolve()
        self.journal_dir = journal_dir.resolve()
        self.log_file = log_file.resolve()
        self.direct_port = direct_port
        self.case_dir = case_dir.resolve() if case_dir else None
        self.process: subprocess.Popen[bytes] | None = None
        self.convey_port: int | None = None

    def start(self, timeout_seconds: float = 60.0) -> int:
        """Start supervisor process with --direct-port and positional 0 convey port.

        Waits for journal/health/convey.port and probes convey readiness.
        Returns the discovered convey port.
        """
        if not self.staged_journal_bin.exists():
            raise PortalStartupError(f"Journal binary missing: {self.staged_journal_bin}")
        if not os.access(self.staged_journal_bin, os.X_OK):
            raise PortalStartupError(f"Journal binary not executable: {self.staged_journal_bin}")

        staged_bin_dir = self.staged_journal_bin.parent
        env = dict(os.environ)
        env["SOLSTONE_JOURNAL"] = str(self.journal_dir)
        env["PATH"] = f"{staged_bin_dir}:{os.environ.get('PATH', '')}"
        # The pinned server hides Metal initialization at its default info level.
        # Capture backend/offload evidence without changing the production plan.
        env["LLAMA_ARG_LOG_VERBOSITY"] = "5"
        if "TMPDIR" not in env:
            env["TMPDIR"] = "/var/tmp"

        self.log_file.parent.mkdir(parents=True, exist_ok=True)
        log_fp = open(self.log_file, "wb")

        cmd = [
            str(self.staged_journal_bin),
            "supervisor",
            "0",
            "--no-daily",
            "--no-schedule",
            "--no-spl",
            "--direct-port",
            str(self.direct_port),
            "--journal",
            str(self.journal_dir),
        ]
        try:
            self.process = subprocess.Popen(
                cmd,
                stdout=log_fp,
                stderr=subprocess.STDOUT,
                env=env,
                cwd=str(self.journal_dir),
                start_new_session=True,
            )
        except Exception as error:
            log_fp.close()
            raise PortalStartupError(f"Failed to spawn supervisor: {error}") from error
        finally:
            log_fp.close()

        deadline = time.monotonic() + timeout_seconds
        convey_port_file = self.journal_dir / "health" / "convey.port"

        try:
            while time.monotonic() < deadline:
                if self.process.poll() is not None:
                    raise PortalStartupError(
                        f"Supervisor exited prematurely with code {self.process.returncode}. See log: {self.log_file}"
                    )

                if convey_port_file.exists():
                    try:
                        raw_port = convey_port_file.read_text(encoding="utf-8").strip()
                        if raw_port:
                            self.convey_port = int(raw_port)
                    except (ValueError, OSError):
                        self.convey_port = None

                if self.convey_port is not None:
                    probe_ready, last_probe_resp = self._probe_ready(self.convey_port)
                    if probe_ready:
                        if self.case_dir:
                            ports_path = self.case_dir / "ports.json"
                            ports_path.write_text(
                                json.dumps(
                                    {
                                        "convey_port": self.convey_port,
                                        "direct_port": self.direct_port,
                                    },
                                    indent=2,
                                ),
                                encoding="utf-8",
                            )
                            if last_probe_resp:
                                probe_path = self.case_dir / "probe_readiness.json"
                                probe_path.write_text(
                                    json.dumps(
                                        {
                                            "status": last_probe_resp.status,
                                            "body": last_probe_resp.body,
                                            "headers": last_probe_resp.headers,
                                        },
                                        indent=2,
                                    ),
                                    encoding="utf-8",
                                )
                        return self.convey_port

                time.sleep(0.1)

            raise PortalStartupError(
                f"Timed out after {timeout_seconds}s waiting for health/convey.port and convey readiness"
            )
        except Exception:
            self.stop()
            raise

    def _probe_ready(self, port: int) -> tuple[bool, HttpResponse | None]:
        url = f"http://127.0.0.1:{port}/app/thinking/api/local/availability"
        resp = send_http_request(url, method="GET", timeout=1.0)
        if resp.status == 200:
            return True, resp
        if resp.status == 302:
            location = resp.headers.get("location", "")
            if "/init" in location:
                raise PortalStartupError(
                    f"Session gate redirected to /init (location: {location}); journal is not established"
                )

        state_url = f"http://127.0.0.1:{port}/app/thinking/api/state"
        state_resp = send_http_request(state_url, method="GET", timeout=1.0)
        if state_resp.status == 200:
            return True, state_resp
        if state_resp.status == 302:
            location = state_resp.headers.get("location", "")
            if "/init" in location:
                raise PortalStartupError(
                    f"Session gate redirected to /init (location: {location}); journal is not established"
                )

        return False, resp

    def stop(self, timeout_seconds: float = 15.0) -> str | None:
        """Stop the owned process group and verify no fixture processes remain."""
        if self.process is None:
            return None
        process = self.process
        pid = process.pid
        try:
            # start_new_session makes this PID the group ID, even after its exit.
            try:
                os.killpg(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=timeout_seconds)
            except subprocess.TimeoutExpired:
                os.killpg(pid, signal.SIGKILL)
                process.wait(timeout=5)
            inventory = subprocess.run(
                ["ps", "-axo", "pid=,ppid=,command="], capture_output=True,
                text=True, check=True, timeout=10,
            )
            markers = (str(self.journal_dir), str(self.staged_journal_bin.parent))
            remaining = [line for line in inventory.stdout.splitlines()
                         if any(marker in line for marker in markers)]
            receipt = {"supervisor_pid": pid, "exit_code": process.returncode,
                       "remaining_fixture_processes": remaining}
            if self.case_dir:
                (self.case_dir / "process-cleanup.json").write_text(json.dumps(receipt, indent=2))
            if remaining:
                return f"Fixture processes remain: {remaining}"
            self.process = None
            return None
        except Exception as error:
            return f"Error stopping supervisor process {pid}: {error}"


def send_http_request(
    url: str,
    method: str = "GET",
    data: dict[str, Any] | None = None,
    timeout: float = 10.0,
) -> HttpResponse:
    """Send HTTP request without following redirects, capturing full status and payload."""
    opener = urllib.request.build_opener(NoRedirectHandler)
    body_bytes = None
    headers = {"Accept": "application/json"}

    if data is not None:
        body_bytes = json.dumps(data).encode("utf-8")
        headers["Content-Type"] = "application/json"

    req = urllib.request.Request(
        url,
        data=body_bytes,
        headers=headers,
        method=method,
    )

    try:
        with opener.open(req, timeout=timeout) as response:
            status = response.status
            resp_headers = {k.lower(): v for k, v in response.headers.items()}
            raw_body = response.read().decode("utf-8", errors="replace")
            json_data = None
            try:
                json_data = json.loads(raw_body)
            except Exception:
                pass
            return HttpResponse(
                status=status,
                headers=resp_headers,
                body=raw_body,
                json_data=json_data,
                error_detail=None,
            )
    except urllib.error.HTTPError as err:
        status = err.code
        resp_headers = {k.lower(): v for k, v in err.headers.items()}
        raw_body = err.read().decode("utf-8", errors="replace")
        json_data = None
        detail = None
        try:
            json_data = json.loads(raw_body)
            if isinstance(json_data, dict):
                detail = json_data.get("detail") or json_data.get("error")
        except Exception:
            pass
        return HttpResponse(
            status=status,
            headers=resp_headers,
            body=raw_body,
            json_data=json_data,
            error_detail=detail,
        )
    except urllib.error.URLError as err:
        return HttpResponse(
            status=0,
            headers={},
            body=str(err),
            json_data=None,
            error_detail=str(err),
        )
