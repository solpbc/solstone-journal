# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native `solstone link` lifecycle for the device simulator."""

from __future__ import annotations

import os
import queue
import re
import shutil
import subprocess
import threading
import time
from collections import deque
from pathlib import Path

_STARTUP_RE = re.compile(r"forwarding 127\.0\.0\.1:([0-9]+) -> home .+ over pl")


class LinkProcessError(RuntimeError):
    """Pairing or bridge startup failed without exposing credential material."""


class LinkBridge:
    """Pair one isolated identity and own one native link-serve child."""

    def __init__(
        self,
        *,
        solstone_bin: str,
        pair_code: str,
        state_dir: Path,
        carrier: str,
        relay_url: str | None,
        startup_timeout: float,
    ) -> None:
        self._solstone_bin = solstone_bin
        self._pair_code = pair_code
        self._state_dir = state_dir
        self._carrier = carrier
        self._relay_url = relay_url
        self._startup_timeout = startup_timeout
        self._label = "journal-device-sim"
        self._xdg_dir = state_dir / "credentials"
        self._process: subprocess.Popen[str] | None = None
        self._reader: threading.Thread | None = None
        self._lines: queue.Queue[str] = queue.Queue(maxsize=256)
        self._log: deque[str] = deque(maxlen=128)
        self._startup_complete = threading.Event()
        self.base_url: str | None = None

    @property
    def credential_dir(self) -> Path:
        return self._xdg_dir

    def _env(self) -> dict[str, str]:
        env = dict(os.environ)
        env["XDG_CONFIG_HOME"] = str(self._xdg_dir)
        return env

    def _bundle_dir(self) -> Path:
        return self._xdg_dir / "solstone-observer" / "spl" / self._label

    def _pair_if_needed(self) -> None:
        if self._bundle_dir().is_dir():
            return
        self._xdg_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(self._xdg_dir, 0o700)
        command = [
            self._solstone_bin,
            "link",
            "join",
            "--code",
            self._pair_code,
            "--as",
            "observer",
            "--label",
            self._label,
        ]
        try:
            result = subprocess.run(
                command,
                env=self._env(),
                check=False,
                capture_output=True,
                text=True,
                timeout=self._startup_timeout,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise LinkProcessError(
                f"solstone link join could not run: {type(error).__name__}"
            ) from error
        if result.returncode != 0:
            raise LinkProcessError(
                f"solstone link join exited {result.returncode}; inspect the journal pairing window"
            )
        if not self._bundle_dir().is_dir():
            raise LinkProcessError(
                "solstone link join reported success without a credential bundle"
            )

    def _drain_output(self, stream: object) -> None:
        if not hasattr(stream, "readline"):
            return
        readline = stream.readline
        while True:
            line = readline()
            if not line:
                return
            line = line.rstrip("\r\n")
            self._log.append(line)
            if self._startup_complete.is_set():
                continue
            try:
                self._lines.put_nowait(line)
            except queue.Full:
                try:
                    self._lines.get_nowait()
                except queue.Empty:
                    pass
                self._lines.put_nowait(line)

    def start(self) -> str:
        self._pair_if_needed()
        command = [
            self._solstone_bin,
            "link",
            "serve",
            "--label",
            self._label,
            "--port",
            "0",
        ]
        if self._carrier == "direct":
            command.append("--direct")
        elif self._relay_url:
            command.extend(["--relay-url", self._relay_url])
        try:
            self._process = subprocess.Popen(
                command,
                env=self._env(),
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
            )
        except OSError as error:
            raise LinkProcessError(
                f"solstone link serve could not start: {type(error).__name__}"
            ) from error
        assert self._process.stdout is not None
        self._reader = threading.Thread(
            target=self._drain_output,
            args=(self._process.stdout,),
            daemon=True,
            name="journal-device-sim-link-output",
        )
        self._reader.start()
        deadline = time.monotonic() + self._startup_timeout
        while time.monotonic() < deadline:
            if self._process.poll() is not None:
                raise LinkProcessError(
                    f"solstone link serve exited {self._process.returncode} before binding; "
                    "the current CLI must support --port 0 and ingest protocol v3"
                )
            try:
                line = self._lines.get(timeout=0.1)
            except queue.Empty:
                continue
            match = _STARTUP_RE.fullmatch(line)
            if match:
                self._startup_complete.set()
                self.base_url = f"http://127.0.0.1:{int(match.group(1))}"
                return self.base_url
        raise LinkProcessError(
            "solstone link serve did not report its bound port in time"
        )

    def stop(self) -> None:
        process = self._process
        self._process = None
        self._startup_complete.set()
        if process is None:
            return
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if process.stdout is not None:
            process.stdout.close()
        if self._reader is not None:
            self._reader.join(timeout=1)
            self._reader = None

    def remove_credentials(self) -> None:
        resolved = self._xdg_dir.resolve()
        state_root = self._state_dir.resolve()
        if resolved.parent != state_root or resolved.name != "credentials":
            raise LinkProcessError(
                "refusing to remove a credential path outside simulator state"
            )
        if resolved.exists():
            shutil.rmtree(resolved)

    def __enter__(self) -> LinkBridge:
        self.start()
        return self

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self.stop()
