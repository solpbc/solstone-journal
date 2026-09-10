# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Portal process lifecycle and HTTP interaction helpers."""

from __future__ import annotations

import json
import os
import signal
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


class PortalProcess:
    """Manages the lifecycle of a real supervisor portal process tree."""

    def __init__(
        self,
        candidate_dir: Path,
        journal_dir: Path,
        log_file: Path,
    ) -> None:
        self.candidate_dir = candidate_dir.resolve()
        self.journal_dir = journal_dir.resolve()
        self.log_file = log_file.resolve()
        self.process: subprocess.Popen[bytes] | None = None
        self.port: int | None = None

    def start(self, timeout_seconds: float = 60.0) -> int:
        """Start supervisor process and wait until convey HTTP port answers."""
        journal_bin = self.candidate_dir / "solstone-core-journal"
        if not journal_bin.exists() and (self.candidate_dir / "solstone-core-journal.exe").exists():
            journal_bin = self.candidate_dir / "solstone-core-journal.exe"

        if not journal_bin.exists():
            raise PortalStartupError(f"Journal binary missing: {journal_bin}")
        if not os.access(journal_bin, os.X_OK):
            raise PortalStartupError(f"Journal binary not executable: {journal_bin}")

        env = dict(os.environ)
        env["SOLSTONE_JOURNAL"] = str(self.journal_dir)
        env["PATH"] = f"{self.candidate_dir}:{os.environ.get('PATH', '')}"
        if "TMPDIR" not in env:
            env["TMPDIR"] = "/var/tmp"

        self.log_file.parent.mkdir(parents=True, exist_ok=True)
        log_fp = open(self.log_file, "wb")

        cmd = [str(journal_bin), "supervisor", "0", "--no-daily"]
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
        port_file = self.journal_dir / "health" / "convey.port"

        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise PortalStartupError(
                    f"Supervisor exited prematurely with code {self.process.returncode}. See log: {self.log_file}"
                )

            if port_file.exists():
                try:
                    port_raw = port_file.read_text(encoding="utf-8").strip()
                    if port_raw.isdigit():
                        candidate_port = int(port_raw)
                        if self._probe_ready(candidate_port):
                            self.port = candidate_port
                            return candidate_port
                except Exception:
                    pass

            time.sleep(0.2)

        self.stop()
        raise PortalStartupError(
            f"Timed out after {timeout_seconds}s waiting for convey readiness on {port_file}"
        )

    def _probe_ready(self, port: int) -> bool:
        url = f"http://127.0.0.1:{port}/app/thinking/api/local/availability"
        opener = urllib.request.build_opener(NoRedirectHandler)
        req = urllib.request.Request(url, method="GET")
        try:
            with opener.open(req, timeout=1.0) as resp:
                if resp.status == 200:
                    return True
                if resp.status == 302:
                    location = resp.headers.get("Location", "")
                    if "/init" in location:
                        raise PortalStartupError(
                            "Session gate redirected to /init; journal is not established"
                        )
        except urllib.error.HTTPError as err:
            if err.code == 302 and "/init" in err.headers.get("Location", ""):
                raise PortalStartupError(
                    "Session gate redirected to /init; journal is not established"
                ) from err
            if err.code in (200, 404, 500):
                return True
        except Exception:
            return False
        return False

    def stop(self, timeout_seconds: float = 5.0) -> None:
        """Terminate supervisor process tree."""
        if self.process is None:
            return
        pid = self.process.pid
        try:
            if hasattr(os, "killpg"):
                try:
                    pgid = os.getpgid(pid)
                    os.killpg(pgid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            else:
                self.process.terminate()

            deadline = time.monotonic() + timeout_seconds
            while time.monotonic() < deadline:
                if self.process.poll() is not None:
                    break
                time.sleep(0.1)

            if self.process.poll() is None:
                if hasattr(os, "killpg"):
                    try:
                        pgid = os.getpgid(pid)
                        os.killpg(pgid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                else:
                    self.process.kill()
                self.process.wait()
        except Exception:
            pass
        finally:
            self.process = None


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
