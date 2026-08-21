# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native `solstone link` lifecycle for the device simulator."""

from __future__ import annotations

import base64
import binascii
import hashlib
import json
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

_STARTUP_RE = re.compile(
    r"forwarding 127\.0\.0\.1:([0-9]+) -> home .+ "
    r"(via direct connection|via direct or relay|via relay only)"
)
_BUNDLE_FILES = (
    "private.pem",
    "cert.pem",
    "chain.pem",
    "home_attestation.jwt",
    "peer.json",
)
_DEFAULT_CONVEY_PORT = 5015
_MAX_CERT_PEM_BYTES = 64 * 1024
_MAX_VERSION_OUTPUT = 512
_MAX_PEER_JSON_BYTES = 64 * 1024
_CERTIFICATE_BEGIN = b"-----BEGIN CERTIFICATE-----"
_CERTIFICATE_END = b"-----END CERTIFICATE-----"
_CERTIFICATE_BODY_RE = re.compile(rb"[A-Za-z0-9+/]+={0,2}")


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
        if carrier not in {"direct", "relay"}:
            raise LinkProcessError("carrier must be direct or relay")
        self._carrier = carrier
        self._relay_url = relay_url
        self._convey_port, self._convey_port_source = self._resolve_convey_port(
            convey_port
        )
        self._startup_timeout = startup_timeout
        self._label = "journal-device-sim"
        self._xdg_dir = state_dir / "credentials"
        self._process: subprocess.Popen[str] | None = None
        self._reader: threading.Thread | None = None
        self._lines: queue.Queue[str] = queue.Queue(maxsize=256)
        self._log: deque[str] = deque(maxlen=128)
        self._startup_complete = threading.Event()
        self._resolved_solstone_bin: Path | None = None
        self._native_provenance: dict[str, str | None] | None = None
        self._credential_provenance: dict[str, object] | None = None
        self.base_url: str | None = None

    @property
    def credential_dir(self) -> Path:
        return self._xdg_dir

    @property
    def provenance(self) -> dict[str, object]:
        """Return JSON-ready, non-secret provenance for run evidence."""

        native = None
        if self._native_provenance is not None:
            native = dict(self._native_provenance)
        credentials = None
        if self._credential_provenance is not None:
            credentials = {
                "cert_pem_sha256": self._credential_provenance[
                    "cert_pem_sha256"
                ],
                "client_cid": self._credential_provenance["client_cid"],
                "peer": dict(self._credential_provenance["peer"]),
            }
        return {
            "native_executable": native,
            "convey": {
                "port": self._convey_port,
                "source": self._convey_port_source,
            },
            "credentials": credentials,
        }

    @staticmethod
    def _resolve_convey_port(explicit: int | None) -> tuple[int, str]:
        if explicit is not None:
            if isinstance(explicit, bool) or not isinstance(explicit, int):
                raise LinkProcessError(
                    "Convey port must be an integer from 1 to 65535"
                )
            if not 1 <= explicit <= 65535:
                raise LinkProcessError(
                    "Convey port must be an integer from 1 to 65535"
                )
            return explicit, "explicit"
        ambient = os.environ.get("SOLSTONE_CONVEY_PORT")
        if ambient is None:
            return _DEFAULT_CONVEY_PORT, "default"
        if re.fullmatch(r"[0-9]+", ambient) is None:
            raise LinkProcessError(
                "SOLSTONE_CONVEY_PORT must be an integer from 1 to 65535"
            )
        try:
            port = int(ambient)
        except ValueError as error:
            raise LinkProcessError(
                "SOLSTONE_CONVEY_PORT must be an integer from 1 to 65535"
            ) from error
        if not 1 <= port <= 65535:
            raise LinkProcessError(
                "SOLSTONE_CONVEY_PORT must be an integer from 1 to 65535"
            )
        return port, "ambient"

    def _env(self) -> dict[str, str]:
        env = dict(os.environ)
        env["XDG_CONFIG_HOME"] = str(self._xdg_dir)
        env["SOLSTONE_CONVEY_PORT"] = str(self._convey_port)
        return env

    @staticmethod
    def _sha256(path: Path) -> str:
        digest = hashlib.sha256()
        try:
            with path.open("rb") as handle:
                while chunk := handle.read(1024 * 1024):
                    digest.update(chunk)
        except OSError as error:
            raise LinkProcessError(
                f"provenance file could not be read: {type(error).__name__}"
            ) from error
        return digest.hexdigest()

    @staticmethod
    def _certificate_der(path: Path) -> bytes:
        """Decode one bounded certificate PEM to its exact DER bytes."""

        try:
            metadata = path.stat()
            if metadata.st_size > _MAX_CERT_PEM_BYTES:
                raise LinkProcessError("credential cert.pem is too large")
            pem = path.read_bytes()
        except LinkProcessError:
            raise
        except OSError as error:
            raise LinkProcessError(
                f"credential cert.pem could not be read: {type(error).__name__}"
            ) from error
        lines = pem.splitlines()
        if (
            len(lines) < 3
            or lines[0] != _CERTIFICATE_BEGIN
            or lines[-1] != _CERTIFICATE_END
        ):
            raise LinkProcessError("credential cert.pem is invalid")
        body_lines = lines[1:-1]
        if any(
            len(line) > 64 or _CERTIFICATE_BODY_RE.fullmatch(line) is None
            for line in body_lines
        ):
            raise LinkProcessError("credential cert.pem is invalid")
        if any(b"=" in line for line in body_lines[:-1]):
            raise LinkProcessError("credential cert.pem is invalid")
        encoded = b"".join(body_lines)
        if len(encoded) % 4 != 0:
            raise LinkProcessError("credential cert.pem is invalid")
        try:
            certificate_der = base64.b64decode(encoded, validate=True)
        except (binascii.Error, ValueError) as error:
            raise LinkProcessError("credential cert.pem is invalid") from error
        if not LinkBridge._is_single_der_sequence(certificate_der):
            raise LinkProcessError("credential cert.pem is invalid")
        return certificate_der

    @staticmethod
    def _is_single_der_sequence(value: bytes) -> bool:
        if len(value) < 2 or value[0] != 0x30:
            return False
        first_length = value[1]
        if first_length < 0x80:
            header_length = 2
            content_length = first_length
        else:
            length_octets = first_length & 0x7F
            if (
                length_octets == 0
                or length_octets > 4
                or len(value) < 2 + length_octets
            ):
                return False
            encoded_length = value[2 : 2 + length_octets]
            if encoded_length[0] == 0:
                return False
            content_length = int.from_bytes(encoded_length, "big")
            if content_length < 0x80:
                return False
            header_length = 2 + length_octets
        return header_length + content_length == len(value)

    @classmethod
    def _client_cid(cls, path: Path) -> str:
        certificate_der = cls._certificate_der(path)
        return f"sha256:{hashlib.sha256(certificate_der).hexdigest()}"

    def _version_output(self, executable: Path) -> str | None:
        try:
            result = subprocess.run(
                [str(executable), "--version"],
                env=self._env(),
                check=False,
                capture_output=True,
                text=True,
                timeout=min(max(self._startup_timeout, 0.1), 5.0),
            )
        except (OSError, subprocess.TimeoutExpired, UnicodeError):
            return None
        if result.returncode != 0:
            return None
        output = result.stdout.strip()
        if not output:
            return None
        return output[:_MAX_VERSION_OUTPUT]

    def _native_executable(self) -> str:
        if self._resolved_solstone_bin is None:
            found = shutil.which(self._solstone_bin)
            if found is None:
                raise LinkProcessError("solstone executable could not be resolved")
            try:
                resolved = Path(found).resolve(strict=True)
                metadata = resolved.stat()
            except OSError as error:
                raise LinkProcessError(
                    f"solstone executable could not be resolved: {type(error).__name__}"
                ) from error
            if not stat.S_ISREG(metadata.st_mode):
                raise LinkProcessError("solstone executable is not a regular file")
            digest = self._sha256(resolved)
            version = self._version_output(resolved)
            if self._sha256(resolved) != digest:
                raise LinkProcessError(
                    "solstone executable changed while provenance was captured"
                )
            self._resolved_solstone_bin = resolved
            self._native_provenance = {
                "path": str(resolved),
                "sha256": digest,
                "version": version,
            }
            return str(resolved)
        digest = self._sha256(self._resolved_solstone_bin)
        assert self._native_provenance is not None
        if digest != self._native_provenance["sha256"]:
            raise LinkProcessError(
                "solstone executable changed after provenance was captured"
            )
        return str(self._resolved_solstone_bin)

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
        peer_path = self._bundle_dir() / "peer.json"
        try:
            if peer_path.stat().st_size > _MAX_PEER_JSON_BYTES:
                raise LinkProcessError("credential peer.json is too large")
            peer = json.loads(peer_path.read_text(encoding="utf-8"))
        except LinkProcessError:
            raise
        except (OSError, UnicodeError, json.JSONDecodeError) as error:
            raise LinkProcessError("credential peer.json is invalid") from error
        if not isinstance(peer, dict):
            raise LinkProcessError("credential peer.json is invalid")
        instance_id = peer.get("instance_id")
        home_label = peer.get("home_label")
        if not isinstance(instance_id, str) or not instance_id:
            raise LinkProcessError("credential peer.json instance_id is missing")
        if not isinstance(home_label, str):
            raise LinkProcessError("credential peer.json home_label is invalid")
        self._credential_provenance = {
            "cert_pem_sha256": self._sha256(self._bundle_dir() / "cert.pem"),
            "client_cid": self._client_cid(self._bundle_dir() / "cert.pem"),
            "peer": {
                "instance_id": instance_id,
                "home_label": home_label,
            },
        }

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
            self._native_executable(),
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
            self._native_executable(),
            "link",
            "serve",
            "--label",
            self._label,
            "--port",
            "0",
        ]
        if self._carrier == "direct":
            command.append("--direct")
            expected_policy = "via direct connection"
        else:
            command.append("--relay-only")
            expected_policy = "via relay only"
            if self._relay_url:
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
                if match.group(2) != expected_policy:
                    raise LinkProcessError(
                        "solstone link serve reported an unexpected carrier policy"
                    )
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
