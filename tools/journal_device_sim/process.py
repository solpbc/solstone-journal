# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native `solstone link` lifecycle for the device simulator."""

from __future__ import annotations

import os
import queue
import re
import shutil
import stat
import subprocess
import threading
import time
from collections import deque
from pathlib import Path

_STARTUP_RE = re.compile(r"forwarding 127\.0\.0\.1:([0-9]+) -> home .+ over pl")
_BUNDLE_FILES = (
    "private.pem",
    "cert.pem",
    "chain.pem",
    "home_attestation.jwt",
    "peer.json",
)


class LinkProcessError(RuntimeError):
    """Pairing or bridge startup failed without exposing credential material."""


class LinkBridge:
    """Pair one isolated identity and own one native link-serve child."""

    def __init__(
        self,
        *,
        solstone_bin: str,
        pair_code: str | None,
        state_dir: Path,
        carrier: str,
        relay_url: str | None,
        convey_port: int | None,
        startup_timeout: float,
    ) -> None:
        self._solstone_bin = solstone_bin
        self._pair_code = pair_code
        self._state_dir = state_dir
        self._carrier = carrier
        self._relay_url = relay_url
        self._convey_port = convey_port
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
        if self._convey_port is not None:
            env["SOLSTONE_CONVEY_PORT"] = str(self._convey_port)
        return env

    def _bundle_dir(self) -> Path:
        return self._xdg_dir / "solstone-observer" / "spl" / self._label

    @staticmethod
    def _path_exists(path: Path) -> bool:
        return os.path.lexists(path)

    @staticmethod
    def _require_plain_directory(path: Path) -> None:
        try:
            metadata = path.lstat()
        except FileNotFoundError as error:
            raise LinkProcessError(
                "credential bundle is incomplete; use a fresh simulator state directory"
            ) from error
        except OSError as error:
            raise LinkProcessError(
                f"credential state could not be inspected: {type(error).__name__}"
            ) from error
        if not stat.S_ISDIR(metadata.st_mode):
            raise LinkProcessError(
                "credential state must contain only plain directories"
            )

    def _validate_bundle(self) -> None:
        try:
            state_root = self._state_dir.resolve()
        except OSError as error:
            raise LinkProcessError(
                f"credential state could not be resolved: {type(error).__name__}"
            ) from error
        directories = (
            self._state_dir,
            self._xdg_dir,
            self._xdg_dir / "solstone-observer",
            self._xdg_dir / "solstone-observer" / "spl",
            self._bundle_dir(),
        )
        for directory in directories:
            self._require_plain_directory(directory)
        try:
            self._bundle_dir().resolve().relative_to(state_root)
        except ValueError as error:
            raise LinkProcessError(
                "credential bundle resolves outside simulator state"
            ) from error
        for name in _BUNDLE_FILES:
            path = self._bundle_dir() / name
            try:
                metadata = path.lstat()
            except FileNotFoundError as error:
                raise LinkProcessError(
                    "credential bundle is incomplete; use a fresh simulator state directory"
                ) from error
            except OSError as error:
                raise LinkProcessError(
                    f"credential bundle could not be inspected: {type(error).__name__}"
                ) from error
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_size == 0:
                raise LinkProcessError(
                    "credential bundle files must be non-empty regular files"
                )

    def _prepare_credential_root(self) -> None:
        try:
            if self._path_exists(self._state_dir):
                self._require_plain_directory(self._state_dir)
            else:
                self._state_dir.mkdir(mode=0o700, parents=True)
            directories = (
                self._xdg_dir,
                self._xdg_dir / "solstone-observer",
                self._xdg_dir / "solstone-observer" / "spl",
            )
            for directory in directories:
                if self._path_exists(directory):
                    self._require_plain_directory(directory)
                else:
                    directory.mkdir(mode=0o700)
            os.chmod(self._state_dir, 0o700)
            os.chmod(self._xdg_dir, 0o700)
        except LinkProcessError:
            raise
        except OSError as error:
            raise LinkProcessError(
                f"credential state could not be prepared: {type(error).__name__}"
            ) from error

    def ensure_paired(self) -> None:
        if self._path_exists(self._bundle_dir()):
            self._validate_bundle()
            if self._pair_code is not None:
                raise LinkProcessError(
                    "credential bundle already exists; use --paired or a fresh state directory"
                )
            return
        if self._pair_code is None:
            raise LinkProcessError(
                "pre-paired credential bundle is missing from simulator state"
            )
        self._prepare_credential_root()
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
        self._validate_bundle()

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
        self.ensure_paired()
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
        self._startup_complete.set()
        if process is None:
            return
        errors: list[OSError] = []
        exit_confirmed = process.poll() is not None
        if not exit_confirmed:
            try:
                process.terminate()
            except ProcessLookupError:
                pass
            except OSError as error:
                errors.append(error)
            try:
                process.wait(timeout=5)
                exit_confirmed = True
            except subprocess.TimeoutExpired:
                try:
                    process.kill()
                except ProcessLookupError:
                    pass
                except OSError as error:
                    errors.append(error)
                try:
                    process.wait(timeout=5)
                    exit_confirmed = True
                except (OSError, subprocess.TimeoutExpired) as error:
                    if isinstance(error, OSError):
                        errors.append(error)
                    else:
                        errors.append(OSError("native link child did not stop"))
            except OSError as error:
                errors.append(error)
        if exit_confirmed:
            self._process = None
        else:
            errors.append(OSError("native link child did not stop"))
        if exit_confirmed and process.stdout is not None:
            try:
                process.stdout.close()
            except OSError as error:
                errors.append(error)
        if exit_confirmed and self._reader is not None:
            self._reader.join(timeout=1)
            self._reader = None
        if errors:
            raise LinkProcessError(
                f"solstone link serve cleanup failed: {type(errors[0]).__name__}"
            ) from errors[0]

    def remove_credentials(self) -> None:
        try:
            resolved = self._xdg_dir.resolve()
            state_root = self._state_dir.resolve()
        except OSError as error:
            raise LinkProcessError(
                f"credential cleanup path failed: {type(error).__name__}"
            ) from error
        if resolved.parent != state_root or resolved.name != "credentials":
            raise LinkProcessError(
                "refusing to remove a credential path outside simulator state"
            )
        try:
            resolved.lstat()
        except FileNotFoundError:
            return
        except OSError as error:
            raise LinkProcessError(
                f"credential cleanup path failed: {type(error).__name__}"
            ) from error
        try:
            shutil.rmtree(resolved)
        except OSError as error:
            raise LinkProcessError(
                f"credential cleanup failed: {type(error).__name__}"
            ) from error

    def finish(self, *, remove_credentials: bool) -> None:
        errors: list[LinkProcessError] = []
        try:
            self.stop()
        except LinkProcessError as error:
            errors.append(error)
        if remove_credentials and self._process is None:
            try:
                self.remove_credentials()
            except LinkProcessError as error:
                errors.append(error)
        if errors:
            raise LinkProcessError(
                f"native bridge finalization failed: {errors[0]}"
            ) from errors[0]

    def __enter__(self) -> LinkBridge:
        self.start()
        return self

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self.stop()
