# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import json
import logging
import os
import platform
import queue
import socket
import threading
import time
from pathlib import Path
from typing import Any, Callable, NamedTuple
from urllib.parse import quote

import requests
from urllib3.filepost import encode_multipart_formdata

from solstone.apps.observer.routes import OBSERVER_CALLOSUM_SSE_ROUTE
from solstone.think.link.ca import cert_fingerprint
from solstone.think.link.client import (
    Client,
    ClientIdentity,
    EnrolledDevice,
    StreamResetError,
    TlsError,
    TunnelSession,
)
from solstone.think.utils import get_config, get_journal, read_service_port

logger = logging.getLogger(__name__)
HOST = socket.gethostname()
PLATFORM = platform.system().lower()
RETRY_BACKOFF = [1, 5, 15]
MAX_RETRIES = 3
UPLOAD_TIMEOUT = 300
EVENT_TIMEOUT = 30
CALLOSUM_RECONNECT_BACKOFF = [1, 2, 4, 8, 16, 30]
PL_BUNDLE_FILES = {
    "private.pem",
    "cert.pem",
    "chain.pem",
    "home_attestation.jwt",
    "peer.json",
}


class UploadResult(NamedTuple):
    success: bool
    duplicate: bool = False


class PlRequestResult(NamedTuple):
    status: int
    headers: dict[str, str]
    body: bytes


def _spl_bundle_dir(label: str) -> Path:
    config_home = os.environ.get("XDG_CONFIG_HOME")
    root = Path(config_home) if config_home else Path.home() / ".config"
    return root / "solstone-observer" / "spl" / label


def _load_pl_identity(label: str) -> ClientIdentity:
    bundle_dir = _spl_bundle_dir(label)
    if not bundle_dir.is_dir():
        raise ValueError(f"observe.observer.spl_label bundle not found: {bundle_dir}")

    missing = sorted(
        name for name in PL_BUNDLE_FILES if not (bundle_dir / name).exists()
    )
    if missing:
        raise ValueError(
            "observe.observer.spl_label bundle missing required file(s): "
            + ", ".join(missing)
        )

    private_key_pem = (bundle_dir / "private.pem").read_text(encoding="utf-8")
    client_cert_pem = (bundle_dir / "cert.pem").read_text(encoding="utf-8")
    ca_chain_pem = (bundle_dir / "chain.pem").read_text(encoding="utf-8")
    home_attestation = (bundle_dir / "home_attestation.jwt").read_text(encoding="utf-8")
    try:
        peer = json.loads((bundle_dir / "peer.json").read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise ValueError(f"invalid peer.json in {bundle_dir}: {exc}") from exc

    local_endpoints = peer.get("local_endpoints") or ()
    if not isinstance(local_endpoints, list):
        raise ValueError("peer.json local_endpoints must be a list")

    return ClientIdentity(
        private_key_pem=private_key_pem,
        client_cert_pem=client_cert_pem,
        ca_chain_pem=ca_chain_pem,
        fingerprint=cert_fingerprint(client_cert_pem),
        home_instance_id=str(peer.get("instance_id") or ""),
        home_label=str(peer.get("home_label") or ""),
        home_attestation=home_attestation,
        local_endpoints=tuple(local_endpoints),
    )


def _endpoint_label(endpoint: dict[str, object]) -> str:
    host = str(endpoint.get("ip") or endpoint.get("host") or "?")
    port = endpoint.get("port") or 7657
    return f"lan-direct {host}:{port}"


def cleanup_draft(draft_dir: str) -> None:
    """Remove all files in a draft directory and delete the directory."""
    try:
        for name in os.listdir(draft_dir):
            fp = os.path.join(draft_dir, name)
            if os.path.isfile(fp):
                os.remove(fp)
        os.rmdir(draft_dir)
    except OSError:
        pass


def finalize_draft(draft_dir: str, segment_key: str) -> str | None:
    """Rename a draft directory to its final segment name.

    Preserves captured data locally when observer upload fails, so the
    think pipeline can process it later.

    Args:
        draft_dir: Path to the draft directory (e.g. .../HHMMSS_draft/)
        segment_key: Final segment name (e.g. "091551_300")

    Returns:
        Path to the finalized directory, or None on failure.
    """
    final_dir = os.path.join(os.path.dirname(draft_dir), segment_key)
    try:
        os.rename(draft_dir, final_dir)
        logger.info(f"Finalized draft locally: {final_dir}")
        return final_dir
    except OSError as e:
        logger.error(f"Failed to finalize draft {draft_dir} -> {final_dir}: {e}")
        return None


class ObserverClient:
    """HTTP client for uploading observer segments to the ingest server."""

    def __init__(
        self,
        stream: str,
        host: str = HOST,
        platform_name: str = PLATFORM,
    ):
        config = get_config()
        observer_cfg = config.get("observe", {}).get("observer", {})
        self._pair_mode = observer_cfg.get("pair_mode", "dl")
        if self._pair_mode not in {"dl", "pl"}:
            raise ValueError("observe.observer.pair_mode must be 'dl' or 'pl'")
        self._url = observer_cfg.get("url", "").rstrip("/")
        if not self._url:
            # Discover local convey port from health directory
            port = read_service_port("convey")
            if port:
                self._url = f"http://localhost:{port}"
                logger.info(f"Discovered convey at port {port}")
            else:
                logger.warning("No convey port found in health directory")
                self._url = ""
        self._key = observer_cfg.get("key")
        self._auto_register = observer_cfg.get("auto_register", True)
        self._name = observer_cfg.get("name") or stream
        self._stream = stream
        self._host = host
        self._platform = platform_name
        self._revoked = False
        self._session = requests.Session()
        self._callosum_thread: threading.Thread | None = None
        self._callosum_stop = threading.Event()
        self._callosum_response: requests.Response | None = None
        self._callosum_error: Exception | None = None
        self._pl_loop: asyncio.AbstractEventLoop | None = None
        self._pl_loop_thread: threading.Thread | None = None
        self._pl_session: TunnelSession | None = None
        self._pl_session_lock: asyncio.Lock | None = None
        self._pl_identity: ClientIdentity | None = None
        self._pl_relay_url: str | None = None
        self._pl_enrolled: EnrolledDevice | None = None
        self._pl_fingerprint_prefix: str | None = None

        if self._pair_mode == "pl":
            if self._key:
                raise ValueError(
                    "observe.observer.pair_mode=pl cannot be combined with "
                    "observe.observer.key"
                )
            spl_label = str(observer_cfg.get("spl_label") or "").strip()
            if not spl_label:
                raise ValueError(
                    "observe.observer.spl_label is required when pair_mode=pl"
                )
            spl_relay_url = str(observer_cfg.get("spl_relay_url") or "").strip()
            if not spl_relay_url:
                raise ValueError(
                    "observe.observer.spl_relay_url is required when pair_mode=pl"
                )
            self._pl_identity = _load_pl_identity(spl_label)
            self._pl_relay_url = spl_relay_url.rstrip("/")
            self._pl_fingerprint_prefix = self._pl_identity.fingerprint.replace(
                "sha256:", ""
            )[:16]
            self._auto_register = False

    def _persist_key(self, key: str) -> None:
        journal = get_journal()
        config_path = Path(journal) / "config" / "journal.json"
        config_path.parent.mkdir(parents=True, exist_ok=True)

        config: dict[str, Any] = {}
        if config_path.exists():
            try:
                with open(config_path, encoding="utf-8") as f:
                    config = json.load(f)
            except (json.JSONDecodeError, OSError) as e:
                logger.error(
                    f"Cannot read {config_path}: {e} — skipping key persistence"
                )
                return

        config.setdefault("observe", {}).setdefault("observer", {})["key"] = key

        with open(config_path, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2)
            f.write("\n")
        os.chmod(config_path, 0o600)

        logger.info(f"Persisted observer key to {config_path}")

    def _ensure_registered(self) -> None:
        if self._pair_mode == "pl":
            return
        if self._key:
            return
        if not self._url:
            return
        if not self._auto_register:
            logger.error(
                "No observer key configured and auto_register disabled. "
                "Set observe.observer.key in journal config or enable auto_register."
            )
            return

        url = f"{self._url}/app/observer/api/create"
        for attempt, delay in enumerate(RETRY_BACKOFF):
            try:
                resp = self._session.post(
                    url,
                    json={"name": self._name},
                    timeout=EVENT_TIMEOUT,
                )
                if resp.status_code == 200:
                    data = resp.json()
                    self._key = data["key"]
                    self._persist_key(self._key)
                    logger.info(
                        f"Auto-registered as '{self._name}' (key: {self._key[:8]}...)"
                    )
                    return
                elif resp.status_code == 403:
                    self._revoked = True
                    logger.error("Registration rejected (403)")
                    return
                else:
                    logger.warning(
                        f"Registration attempt {attempt + 1} failed: {resp.status_code}"
                    )
            except requests.RequestException as e:
                logger.warning(f"Registration attempt {attempt + 1} failed: {e}")
            if attempt < len(RETRY_BACKOFF) - 1:
                time.sleep(delay)
        logger.error(f"Registration failed after {MAX_RETRIES} attempts")

    def _ensure_pl_loop(self) -> asyncio.AbstractEventLoop:
        if self._pl_loop is not None and self._pl_loop.is_running():
            return self._pl_loop

        loop = asyncio.new_event_loop()
        ready = threading.Event()

        def run_loop() -> None:
            asyncio.set_event_loop(loop)
            self._pl_session_lock = asyncio.Lock()
            ready.set()
            loop.run_forever()

        thread = threading.Thread(
            target=run_loop,
            name=f"observer-pl-{self._name}",
            daemon=True,
        )
        thread.start()
        ready.wait()
        self._pl_loop = loop
        self._pl_loop_thread = thread
        return loop

    def _run_pl(self, coro):
        loop = self._ensure_pl_loop()
        future = asyncio.run_coroutine_threadsafe(coro, loop)
        return future.result()

    async def _get_pl_session(self) -> TunnelSession:
        if self._pl_identity is None:
            raise TlsError("PL identity not loaded")
        if self._pl_session_lock is None:
            self._pl_session_lock = asyncio.Lock()
        async with self._pl_session_lock:
            if self._pl_session is not None:
                return self._pl_session
            self._pl_session = await self._open_tunnel()
            return self._pl_session

    async def _close_pl_session(self) -> None:
        session = self._pl_session
        self._pl_session = None
        if session is not None:
            await session.close()

    async def _pl_request(
        self,
        method: str,
        path: str,
        *,
        headers: dict[str, str] | None = None,
        body: bytes = b"",
    ) -> PlRequestResult:
        session = await self._get_pl_session()
        try:
            status, response_headers, response_body = await session.request(
                method,
                path,
                headers=headers,
                body=body,
            )
            return PlRequestResult(status, response_headers, response_body)
        except (ConnectionError, OSError, StreamResetError):
            await self._close_pl_session()
            raise

    async def _open_tunnel(self) -> TunnelSession:
        if self._pl_identity is None:
            raise TlsError("PL identity not loaded")

        attempts: list[tuple[str, Any]] = []
        for endpoint in self._pl_identity.local_endpoints:
            label = _endpoint_label(endpoint)
            attempts.append((label, self._dial_direct_endpoint(endpoint)))
        if self._pl_relay_url:
            attempts.append(("spl-relay", self._dial_relay()))
        if not attempts:
            raise TlsError("no PL dial attempts configured")

        tasks = {asyncio.create_task(coro): label for label, coro in attempts}
        pending = set(tasks)
        failures: dict[str, BaseException] = {}

        while pending:
            done, pending = await asyncio.wait(
                pending,
                return_when=asyncio.FIRST_COMPLETED,
            )
            for task in done:
                label = tasks[task]
                try:
                    session = task.result()
                except BaseException as exc:
                    failures[label] = exc
                    continue
                for loser in pending:
                    loser.cancel()
                if pending:
                    await asyncio.gather(*pending, return_exceptions=True)
                return session

        detail = "; ".join(
            f"{label}: {type(exc).__name__}: {exc}" for label, exc in failures.items()
        )
        raise TlsError(f"all PL dial attempts failed: {detail}")

    async def _dial_direct_endpoint(self, endpoint: dict[str, object]) -> TunnelSession:
        if self._pl_identity is None:
            raise TlsError("PL identity not loaded")
        host = str(endpoint.get("ip") or endpoint.get("host") or "").strip()
        if not host:
            raise TlsError("LAN endpoint missing ip")
        port_value = endpoint.get("port") or 7657
        try:
            port = int(port_value)
        except (TypeError, ValueError) as exc:
            raise TlsError(f"LAN endpoint has invalid port: {port_value!r}") from exc
        enrolled = EnrolledDevice(device_token="", identity=self._pl_identity)
        return await Client.dial_direct(host, enrolled, port=port)

    async def _dial_relay(self) -> TunnelSession:
        if self._pl_identity is None or self._pl_relay_url is None:
            raise TlsError("PL relay is not configured")
        if self._pl_enrolled is None:
            self._pl_enrolled = Client.enroll_device(
                self._pl_relay_url,
                self._pl_identity,
            )
        return await Client.dial(self._pl_relay_url, self._pl_enrolled)

    def upload_segment(
        self,
        day: str,
        segment: str,
        files: list[Path],
        meta: dict[str, Any] | None = None,
    ) -> UploadResult:
        if self._revoked:
            logger.warning("Client revoked, skipping upload")
            return UploadResult(False)

        if self._pair_mode == "pl":
            return self._upload_segment_pl(day, segment, files, meta)

        self._ensure_registered()
        if not self._key:
            return UploadResult(False)

        url = f"{self._url}/app/observer/ingest"
        for attempt, delay in enumerate(RETRY_BACKOFF):
            file_handles = []
            files_data = []
            try:
                for path in files:
                    if not path.exists():
                        logger.warning(f"File not found, skipping: {path}")
                        continue
                    fh = open(path, "rb")
                    file_handles.append(fh)
                    files_data.append(
                        ("files", (path.name, fh, "application/octet-stream"))
                    )

                if not files_data:
                    logger.error("No valid files to upload")
                    return UploadResult(False)

                data: dict[str, Any] = {
                    "day": day,
                    "segment": segment,
                }
                if not meta or "host" not in meta:
                    data["host"] = self._host
                if not meta or "platform" not in meta:
                    data["platform"] = self._platform
                if meta:
                    data["meta"] = json.dumps(meta)

                headers = {}
                if self._key:
                    headers["Authorization"] = f"Bearer {self._key}"
                    logger.debug(
                        f"Sending Authorization header: Bearer {self._key[:8]}..."
                    )

                response = self._session.post(
                    url,
                    data=data,
                    files=files_data,
                    headers=headers,
                    timeout=UPLOAD_TIMEOUT,
                )

                if response.status_code == 200:
                    resp_data = response.json()
                    is_duplicate = resp_data.get("status") == "duplicate"
                    return UploadResult(True, duplicate=is_duplicate)
                if response.status_code == 403:
                    self._revoked = True
                    logger.error("Upload rejected (403)")
                    return UploadResult(False)

                logger.warning(
                    f"Upload attempt {attempt + 1} failed: "
                    f"{response.status_code} {response.text}"
                )
            except requests.RequestException as e:
                logger.warning(f"Upload attempt {attempt + 1} failed: {e}")
            finally:
                for fh in file_handles:
                    try:
                        fh.close()
                    except Exception:
                        pass

            if attempt < len(RETRY_BACKOFF) - 1:
                time.sleep(delay)

        logger.error(f"Upload failed after {MAX_RETRIES} attempts: {day}/{segment}")
        return UploadResult(False)

    def _upload_segment_pl(
        self,
        day: str,
        segment: str,
        files: list[Path],
        meta: dict[str, Any] | None,
    ) -> UploadResult:
        for attempt, delay in enumerate(RETRY_BACKOFF):
            try:
                fields: list[tuple[str, Any]] = [
                    ("day", day),
                    ("segment", segment),
                ]
                if not meta or "host" not in meta:
                    fields.append(("host", self._host))
                if not meta or "platform" not in meta:
                    fields.append(("platform", self._platform))
                if meta:
                    fields.append(("meta", json.dumps(meta)))

                for path in files:
                    if not path.exists():
                        logger.warning(f"File not found, skipping: {path}")
                        continue
                    fields.append(
                        (
                            "files",
                            (
                                path.name,
                                path.read_bytes(),
                                "application/octet-stream",
                            ),
                        )
                    )

                if not any(field[0] == "files" for field in fields):
                    logger.error("No valid files to upload")
                    return UploadResult(False)

                body, content_type = encode_multipart_formdata(fields)
                result = self._run_pl(
                    self._pl_request(
                        "POST",
                        "/app/observer/ingest",
                        headers={"Content-Type": content_type},
                        body=body,
                    )
                )

                if result.status == 200:
                    resp_data = json.loads(result.body.decode("utf-8") or "{}")
                    is_duplicate = resp_data.get("status") == "duplicate"
                    return UploadResult(True, duplicate=is_duplicate)
                if result.status == 403:
                    self._revoked = True
                    logger.error("Upload rejected (403)")
                    return UploadResult(False)

                logger.warning(
                    "PL upload attempt %s failed: %s %s",
                    attempt + 1,
                    result.status,
                    result.body.decode("utf-8", errors="replace"),
                )
            except (ConnectionError, OSError, StreamResetError, TlsError) as exc:
                logger.warning("PL upload attempt %s failed: %s", attempt + 1, exc)
            if attempt < len(RETRY_BACKOFF) - 1:
                time.sleep(delay)

        logger.error(f"PL upload failed after {MAX_RETRIES} attempts: {day}/{segment}")
        return UploadResult(False)

    def relay_event(self, tract: str, event: str, **fields: Any) -> bool:
        if self._revoked:
            return False

        if self._pair_mode == "pl":
            return self._relay_event_pl(tract, event, **fields)

        self._ensure_registered()
        if not self._key:
            return False

        url = f"{self._url}/app/observer/ingest/{self._key}/event"
        payload = {"tract": tract, "event": event, **fields}
        try:
            resp = self._session.post(url, json=payload, timeout=EVENT_TIMEOUT)
            if resp.status_code == 200:
                return True
            if resp.status_code == 403:
                self._revoked = True
                logger.error("Event relay rejected (403)")
                return False
            logger.warning(f"Event relay failed: {resp.status_code} {resp.text}")
            return False
        except requests.RequestException as e:
            logger.debug(f"Event relay failed: {e}")
            return False

    def _relay_event_pl(self, tract: str, event: str, **fields: Any) -> bool:
        payload = {"tract": tract, "event": event, **fields}
        body = json.dumps(payload).encode("utf-8")
        try:
            result = self._run_pl(
                self._pl_request(
                    "POST",
                    "/app/observer/ingest/event",
                    headers={"Content-Type": "application/json"},
                    body=body,
                )
            )
            if result.status == 200:
                return True
            if result.status == 403:
                self._revoked = True
                logger.error("Event relay rejected (403)")
                return False
            logger.warning(
                "PL event relay failed: %s %s",
                result.status,
                result.body.decode("utf-8", errors="replace"),
            )
            return False
        except (ConnectionError, OSError, StreamResetError, TlsError) as exc:
            logger.debug("PL event relay failed: %s", exc)
            return False

    def subscribe_callosum(self, callback: Callable[[dict], None]) -> None:
        if self._callosum_thread is not None and self._callosum_thread.is_alive():
            raise RuntimeError("subscribe_callosum already active")

        self._callosum_stop.clear()
        self._callosum_error = None
        self._callosum_thread = threading.Thread(
            target=self._callosum_loop,
            args=(callback,),
            daemon=True,
        )
        self._callosum_thread.start()

    def _callosum_loop(self, callback: Callable[[dict], None]) -> None:
        if self._pair_mode == "pl":
            self._callosum_loop_pl(callback)
            return

        if self._revoked:
            return

        self._ensure_registered()
        if not self._key or not self._url:
            return

        path = OBSERVER_CALLOSUM_SSE_ROUTE.replace("<key>", quote(self._key, safe=""))
        url = f"{self._url}{path}"
        headers = {"Authorization": f"Bearer {self._key}"}
        backoff_index = 0

        while not self._callosum_stop.is_set():
            response: requests.Response | None = None
            try:
                response = self._session.get(
                    url,
                    headers=headers,
                    stream=True,
                    timeout=(EVENT_TIMEOUT, None),
                )
                self._callosum_response = response

                if response.status_code == 200:
                    backoff_index = 0
                    self._consume_callosum_response(response, callback)
                elif response.status_code in {401, 403}:
                    self._revoked = True
                    self._callosum_error = RuntimeError(
                        f"Callosum subscription rejected ({response.status_code})"
                    )
                    logger.warning(
                        "Callosum subscription rejected (%s)", response.status_code
                    )
                    return
                else:
                    self._callosum_error = RuntimeError(
                        f"Callosum subscription failed ({response.status_code})"
                    )
                    logger.debug(
                        "Callosum subscription failed: %s %s",
                        response.status_code,
                        response.text,
                    )
            except requests.RequestException as e:
                self._callosum_error = e
                logger.debug(f"Callosum subscription transport failed: {e}")
            except Exception as e:
                self._callosum_error = e
                if self._callosum_stop.is_set():
                    logger.debug(f"Callosum subscription stopped: {e}")
                else:
                    logger.debug(f"Callosum subscription failed: {e}", exc_info=True)
            finally:
                if self._callosum_response is response:
                    self._callosum_response = None
                if response is not None:
                    self._close_callosum_response(response)

            if self._callosum_stop.is_set():
                return

            delay = CALLOSUM_RECONNECT_BACKOFF[
                min(backoff_index, len(CALLOSUM_RECONNECT_BACKOFF) - 1)
            ]
            if self._callosum_stop.wait(delay):
                return
            if backoff_index < len(CALLOSUM_RECONNECT_BACKOFF) - 1:
                backoff_index += 1

    def _callosum_loop_pl(self, callback: Callable[[dict], None]) -> None:
        if self._revoked or not self._pl_fingerprint_prefix:
            return

        path = OBSERVER_CALLOSUM_SSE_ROUTE.replace(
            "<key>",
            quote(self._pl_fingerprint_prefix, safe=""),
        )
        backoff_index = 0

        while not self._callosum_stop.is_set():
            chunks: queue.Queue[bytes | Exception | None] = queue.Queue()
            loop = self._ensure_pl_loop()
            future = asyncio.run_coroutine_threadsafe(
                self._pl_callosum_reader(path, chunks),
                loop,
            )
            data_lines: list[str] = []
            text_buffer = ""
            while not self._callosum_stop.is_set():
                try:
                    item = chunks.get(timeout=0.1)
                except queue.Empty:
                    if future.done():
                        break
                    continue

                if item is None:
                    break
                if isinstance(item, Exception):
                    self._callosum_error = item
                    if self._revoked:
                        return
                    break
                text_buffer = self._consume_callosum_text(
                    text_buffer,
                    item,
                    data_lines,
                    callback,
                )

            if self._callosum_stop.is_set():
                future.cancel()
                return

            if text_buffer:
                self._dispatch_callosum_frame(data_lines, callback)

            delay = CALLOSUM_RECONNECT_BACKOFF[
                min(backoff_index, len(CALLOSUM_RECONNECT_BACKOFF) - 1)
            ]
            if self._callosum_stop.wait(delay):
                return
            if backoff_index < len(CALLOSUM_RECONNECT_BACKOFF) - 1:
                backoff_index += 1

    async def _pl_callosum_reader(
        self,
        path: str,
        chunks: queue.Queue[bytes | Exception | None],
    ) -> None:
        try:
            session = await self._get_pl_session()
            status, _headers, initial_body, stream = await session.stream_request(
                "GET",
                path,
                headers={"Accept": "text/event-stream"},
            )
            if status == 200:
                if initial_body:
                    chunks.put(initial_body)
                async for chunk in stream.read():
                    chunks.put(chunk)
                return
            if status in {401, 403}:
                self._revoked = True
                chunks.put(RuntimeError(f"Callosum subscription rejected ({status})"))
                return
            chunks.put(RuntimeError(f"Callosum subscription failed ({status})"))
        except (ConnectionError, OSError, StreamResetError, TlsError) as exc:
            await self._close_pl_session()
            chunks.put(exc)
        except Exception as exc:
            chunks.put(exc)
        finally:
            chunks.put(None)

    def _consume_callosum_response(
        self,
        response: requests.Response,
        callback: Callable[[dict], None],
    ) -> None:
        data_lines: list[str] = []
        for raw_line in response.iter_lines(decode_unicode=True):
            if self._callosum_stop.is_set():
                return
            line = raw_line.decode("utf-8") if isinstance(raw_line, bytes) else raw_line
            if line == "":
                self._dispatch_callosum_frame(data_lines, callback)
                data_lines = []
            elif line.startswith(":"):
                continue
            elif line.startswith("data:"):
                data = line[5:]
                if data.startswith(" "):
                    data = data[1:]
                data_lines.append(data)

        self._dispatch_callosum_frame(data_lines, callback)

    def _consume_callosum_text(
        self,
        buffer: str,
        chunk: bytes,
        data_lines: list[str],
        callback: Callable[[dict], None],
    ) -> str:
        text = buffer + chunk.decode("utf-8", errors="replace")
        while "\n" in text:
            line, text = text.split("\n", 1)
            line = line.rstrip("\r")
            if line == "":
                self._dispatch_callosum_frame(data_lines, callback)
                data_lines.clear()
            elif line.startswith(":"):
                continue
            elif line.startswith("data:"):
                data = line[5:]
                if data.startswith(" "):
                    data = data[1:]
                data_lines.append(data)
        return text

    def _dispatch_callosum_frame(
        self,
        data_lines: list[str],
        callback: Callable[[dict], None],
    ) -> None:
        if not data_lines:
            return

        try:
            payload = json.loads("\n".join(data_lines))
        except json.JSONDecodeError as e:
            logger.warning(f"Invalid callosum SSE payload: {e}")
            return

        try:
            callback(payload)
        except Exception:
            logger.exception("Callosum subscription callback failed")

    def _close_callosum_response(self, response: requests.Response) -> None:
        self._shutdown_callosum_response_socket(response)
        try:
            response.close()
        except Exception as e:
            logger.debug(f"Callosum response close failed: {e}")

    def _shutdown_callosum_response_socket(self, response: requests.Response) -> None:
        try:
            raw = getattr(response, "raw", None)
            fp = getattr(raw, "_fp", None)
            socket_fp = getattr(fp, "fp", None)
            socket_raw = getattr(socket_fp, "raw", None)
            sock = getattr(socket_raw, "_sock", None)
            if sock is not None:
                sock.shutdown(socket.SHUT_RDWR)
        except Exception:
            pass

    def stop(self) -> None:
        self._callosum_stop.set()
        if self._callosum_response is not None:
            self._close_callosum_response(self._callosum_response)
        if (
            self._callosum_thread is not None
            and self._callosum_thread.is_alive()
            and self._callosum_thread is not threading.current_thread()
        ):
            self._callosum_thread.join(timeout=5.0)
        if self._pl_loop is not None:
            try:
                self._run_pl(self._close_pl_session())
            except Exception as exc:
                logger.debug("PL session close failed: %s", exc)
            self._pl_loop.call_soon_threadsafe(self._pl_loop.stop)
            if self._pl_loop_thread is not None and self._pl_loop_thread.is_alive():
                self._pl_loop_thread.join(timeout=5.0)
        self._session.close()
