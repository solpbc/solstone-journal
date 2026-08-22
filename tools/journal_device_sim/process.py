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
import selectors
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
_MAX_HELP_OUTPUT_BYTES = 64 * 1024
_HELP_PREFLIGHT_TIMEOUT_S = 5.0
_NATIVE_ROOT_HEADER = b"solstone - journal access CLI"
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
        solstone_bin: str | None,
        pair_code: str | None,
        state_dir: Path,
        carrier: str,
        relay_url: str | None,
        convey_port: int | None,
        startup_timeout: float,
    ) -> None:
        if solstone_bin is None:
            self._solstone_bin = str(
                Path(__file__).resolve().parent.parents[1]
                / "core"
                / "target"
                / "debug"
                / "solstone"
            )
            self._native_selection_mode = "source-build-default"
            self._expected_solstone_bin = self._solstone_bin
        elif os.path.isabs(solstone_bin):
            self._solstone_bin = solstone_bin
            self._native_selection_mode = "override"
            self._expected_solstone_bin = None
        else:
            raise LinkProcessError(
                "solstone_bin must be an absolute path; omit it for the source-built default"
            )
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

    @staticmethod
    def _reap_native_probe(process: subprocess.Popen[bytes]) -> None:
        if process.poll() is not None:
            try:
                process.wait()
            except OSError:
                pass
            return
        try:
            process.terminate()
        except (OSError, ProcessLookupError):
            pass
        try:
            process.wait(timeout=0.2)
            return
        except (OSError, subprocess.TimeoutExpired):
            pass
        try:
            process.kill()
        except (OSError, ProcessLookupError):
            pass
        try:
            process.wait(timeout=0.2)
        except (OSError, subprocess.TimeoutExpired):
            pass

    def _run_bounded_native_probe(
        self,
        executable: Path,
        args: list[str],
        *,
        stdout_limit: int,
        stderr_limit: int,
        timeout: float,
    ) -> tuple[int | None, bytes, bytes, str | None]:
        process: subprocess.Popen[bytes] | None = None
        selector = selectors.DefaultSelector()
        stdout = bytearray()
        stderr = bytearray()
        buffers = {"stdout": stdout, "stderr": stderr}
        limits = {"stdout": stdout_limit, "stderr": stderr_limit}
        streams: dict[int, tuple[str, object]] = {}
        try:
            try:
                process = subprocess.Popen(
                    [str(executable), *args],
                    env=self._env(),
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                )
            except OSError as error:
                return None, b"", b"", f"probe-spawn-error:{type(error).__name__}"
            assert process.stdout is not None
            assert process.stderr is not None
            for name, stream in (("stdout", process.stdout), ("stderr", process.stderr)):
                descriptor = stream.fileno()
                os.set_blocking(descriptor, False)
                selector.register(descriptor, selectors.EVENT_READ, name)
                streams[descriptor] = (name, stream)

            deadline = time.monotonic() + timeout
            while streams:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return None, bytes(stdout), bytes(stderr), "probe-timeout"
                try:
                    ready = selector.select(remaining)
                except OSError as error:
                    return (
                        None,
                        bytes(stdout),
                        bytes(stderr),
                        f"probe-read-error:{type(error).__name__}",
                    )
                for key, _events in ready:
                    name = key.data
                    buffer = buffers[name]
                    limit = limits[name]
                    try:
                        chunk = os.read(key.fd, min(8192, limit + 1 - len(buffer)))
                    except BlockingIOError:
                        continue
                    except OSError as error:
                        return (
                            None,
                            bytes(stdout),
                            bytes(stderr),
                            f"probe-read-error:{type(error).__name__}",
                        )
                    if not chunk:
                        selector.unregister(key.fd)
                        streams.pop(key.fd, None)
                        continue
                    buffer.extend(chunk)
                    if len(buffer) > limit:
                        return (
                            None,
                            bytes(stdout),
                            bytes(stderr),
                            f"{name}-overflow",
                        )

            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None, bytes(stdout), bytes(stderr), "probe-timeout"
            try:
                return process.wait(timeout=remaining), bytes(stdout), bytes(stderr), None
            except subprocess.TimeoutExpired:
                return None, bytes(stdout), bytes(stderr), "probe-timeout"
            except OSError as error:
                return (
                    None,
                    bytes(stdout),
                    bytes(stderr),
                    f"probe-wait-error:{type(error).__name__}",
                )
        finally:
            selector.close()
            if process is not None:
                self._reap_native_probe(process)
                for stream in (process.stdout, process.stderr):
                    if stream is not None:
                        try:
                            stream.close()
                        except OSError:
                            pass

    @staticmethod
    def _source_revision() -> str:
        repo_root = Path(__file__).resolve().parent.parents[1]
        try:
            result = subprocess.run(
                ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
                check=False,
                capture_output=True,
                text=True,
                timeout=5,
            )
        except (OSError, subprocess.TimeoutExpired, UnicodeError):
            return "unknown"
        if result.returncode != 0:
            return "unknown"
        return result.stdout.strip() or "unknown"

    def _native_auth_error(
        self,
        condition: str,
        *,
        resolved: Path | None = None,
        digest: str | None = None,
        version: str | None = None,
    ) -> LinkProcessError:
        selected = str(resolved) if resolved is not None else self._solstone_bin
        expected = self._expected_solstone_bin or "not-applicable"
        return LinkProcessError(
            "solstone native launcher authentication failed: "
            f"selection_mode={self._native_selection_mode}; "
            f"resolved_path={selected}; "
            f"expected_path={expected}; "
            f"simulator_revision={self._source_revision()}; "
            f"sha256={digest or 'unavailable'}; "
            f"version={version or 'unavailable'}; "
            f"condition={condition}; "
            "recovery: run `make build` or pass "
            "`--solstone-bin /abs/path/to/solstone`"
        )

    def _authenticate_native_header(self, executable: Path, digest: str) -> bytes:
        status, stdout, _stderr, condition = self._run_bounded_native_probe(
            executable,
            ["--help"],
            stdout_limit=_MAX_HELP_OUTPUT_BYTES,
            stderr_limit=_MAX_HELP_OUTPUT_BYTES,
            timeout=_HELP_PREFLIGHT_TIMEOUT_S,
        )
        if condition is not None:
            raise self._native_auth_error(condition, resolved=executable, digest=digest)
        if status != 0:
            raise self._native_auth_error(
                f"nonzero-help-exit:{status}", resolved=executable, digest=digest
            )
        if not stdout.startswith(_NATIVE_ROOT_HEADER):
            raise self._native_auth_error(
                "wrong-header", resolved=executable, digest=digest
            )
        try:
            stdout.decode("utf-8")
        except UnicodeDecodeError:
            raise self._native_auth_error(
                "help-decode-error", resolved=executable, digest=digest
            ) from None
        return stdout

    def _version_output(self, executable: Path) -> str | None:
        status, stdout, _stderr, condition = self._run_bounded_native_probe(
            executable,
            ["--version"],
            stdout_limit=_MAX_HELP_OUTPUT_BYTES,
            stderr_limit=_MAX_HELP_OUTPUT_BYTES,
            timeout=min(max(self._startup_timeout, 0.1), _HELP_PREFLIGHT_TIMEOUT_S),
        )
        if condition is not None or status != 0:
            return None
        try:
            output = stdout.decode("utf-8").strip()
        except UnicodeDecodeError:
            return None
        if not output:
            return None
        return output[:_MAX_VERSION_OUTPUT]

    def _native_executable(self) -> str:
        if self._resolved_solstone_bin is None:
            try:
                resolved = Path(self._solstone_bin).resolve(strict=True)
                metadata = resolved.stat()
            except FileNotFoundError:
                raise self._native_auth_error("missing") from None
            except (OSError, RuntimeError) as error:
                raise self._native_auth_error(
                    f"unreadable:{type(error).__name__}"
                ) from error
            if not stat.S_ISREG(metadata.st_mode):
                raise self._native_auth_error("non-regular", resolved=resolved)
            if metadata.st_mode & 0o111 == 0:
                raise self._native_auth_error("not-executable", resolved=resolved)
            try:
                digest = self._sha256(resolved)
            except LinkProcessError as error:
                raise self._native_auth_error(
                    f"unreadable:{type(error).__name__}", resolved=resolved
                ) from error
            self._authenticate_native_header(resolved, digest)
            version = self._version_output(resolved)
            try:
                current_digest = self._sha256(resolved)
            except LinkProcessError as error:
                raise self._native_auth_error(
                    f"changed-candidate:current-digest-unavailable:{type(error).__name__}",
                    resolved=resolved,
                    digest=digest,
                    version=version,
                ) from error
            if current_digest != digest:
                raise self._native_auth_error(
                    f"changed-candidate:recorded={digest}:current={current_digest}",
                    resolved=resolved,
                    digest=digest,
                    version=version,
                )
            self._resolved_solstone_bin = resolved
            self._native_provenance = {
                "path": str(resolved),
                "sha256": digest,
                "version": version,
                "selection_mode": self._native_selection_mode,
            }
            return str(resolved)
        assert self._native_provenance is not None
        recorded_digest = self._native_provenance["sha256"]
        assert isinstance(recorded_digest, str)
        try:
            digest = self._sha256(self._resolved_solstone_bin)
        except LinkProcessError as error:
            raise self._native_auth_error(
                f"changed-candidate:current-digest-unavailable:{type(error).__name__}",
                resolved=self._resolved_solstone_bin,
                digest=recorded_digest,
                version=self._native_provenance["version"],
            ) from error
        if digest != recorded_digest:
            raise self._native_auth_error(
                f"changed-candidate:recorded={recorded_digest}:current={digest}",
                resolved=self._resolved_solstone_bin,
                digest=recorded_digest,
                version=self._native_provenance["version"],
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
        executable = self._native_executable()
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
            executable,
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
