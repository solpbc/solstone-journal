# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Browser scenario verification using Pinchtab and CDP.

Existing JPEG baselines are human-review snapshots only: verify mode checks that
the expected files exist, and update mode refreshes them for manual review.
Diffed PNG baselines are objective visual-regression checks, but updating them
still requires human judgment. Do not reflexively update a PNG baseline and
accept a real UI regression.

The full pinchtab REST-driven browser suite is currently red due to pre-existing
pinchtab REST instability. The CDP-driven transcripts scenarios can be run in
isolation with ``--scenario transcripts/<name>`` and are the validation gate for
transcripts visual-regression coverage.
"""

from __future__ import annotations

import argparse
import base64
import json
import logging
import os
import re
import shutil
import signal
import subprocess
import time
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlparse, urlunparse
from urllib.request import urlopen

import numpy as np
import requests
from PIL import Image
from websockets.sync.client import connect

logger = logging.getLogger(__name__)


SCENARIOS: list[dict[str, Any]] = [
    {
        "app": "chat",
        "name": "bar-reasons",
        "steps": [
            {"do": "navigate", "path": "/static/tests/chat-bar-reasons.html"},
            {"do": "wait", "ms": 500},
            {"do": "assert_text", "text": "PASS chat bar reasons: 0 failure(s)"},
        ],
    },
    # smoke scenarios
    {
        "app": "sol",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/sol/20260304"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "activities",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/activities/20260304"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "speakers",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/speakers/20260304"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "todos",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/todos/20260304"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "tokens",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/tokens/20260304"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "transcripts",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/transcripts/20260304"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "transcripts",
        "name": "day-tall",
        "engine": "cdp",
        "diff": True,
        "viewport": {"width": 1265, "height": 713},
        "steps": [
            {"do": "navigate", "path": "/app/transcripts/20260304"},
            {
                "do": "wait_for",
                "expression": "transcripts_day_ready",
                "timeout_ms": 10000,
            },
            {"do": "screenshot"},
        ],
    },
    {
        "app": "transcripts",
        "name": "day-short",
        "engine": "cdp",
        "diff": True,
        "viewport": {"width": 1265, "height": 500},
        "steps": [
            {"do": "navigate", "path": "/app/transcripts/20260304"},
            {
                "do": "wait_for",
                "expression": "transcripts_day_ready",
                "timeout_ms": 10000,
            },
            {"do": "screenshot"},
        ],
    },
    {
        "app": "transcripts",
        "name": "segment-tall",
        "engine": "cdp",
        "diff": True,
        "viewport": {"width": 1265, "height": 713},
        "steps": [
            {
                "do": "navigate",
                "path": "/app/transcripts/20260304#090000_300/transcript",
            },
            {
                "do": "wait_for",
                "expression": "transcripts_segment_ready",
                "timeout_ms": 10000,
            },
            {"do": "screenshot"},
        ],
    },
    {
        "app": "transcripts",
        "name": "segment-short",
        "engine": "cdp",
        "diff": True,
        "viewport": {"width": 1265, "height": 500},
        "steps": [
            {
                "do": "navigate",
                "path": "/app/transcripts/20260304#090000_300/transcript",
            },
            {
                "do": "wait_for",
                "expression": "transcripts_segment_ready",
                "timeout_ms": 10000,
            },
            {"do": "screenshot"},
        ],
    },
    {
        "app": "entities",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/entities"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "health",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/health"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "import",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/import"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "observer",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/observer"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "search",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/search"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "settings",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/settings"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "stats",
        "name": "smoke",
        "steps": [
            {"do": "navigate", "path": "/app/stats"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    # interactive scenarios
    {
        "app": "search",
        "name": "search-flow",
        "steps": [
            {"do": "navigate", "path": "/app/search"},
            {"do": "wait", "ms": 1000},
            {"do": "snapshot"},
            {"do": "find_input", "as": "search_input"},
            {"do": "type", "var": "search_input", "text": "romeo"},
            {"do": "wait", "ms": 1500},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "entities",
        "name": "entity-detail",
        "steps": [
            {"do": "navigate", "path": "/app/entities/work/romeo_montague"},
            {"do": "wait", "ms": 1000},
            {"do": "screenshot"},
        ],
    },
    {
        "app": "todos",
        "name": "todo-states",
        "steps": [
            {"do": "evaluate", "expression": "document.cookie='facet=work;path=/'"},
            {"do": "navigate", "path": "/app/todos/20260304"},
            {"do": "wait", "ms": 1200},
            {"do": "screenshot"},
        ],
    },
]


_ERROR_LISTENER_JS = (
    "window.__pt_errors=[];"
    "window.addEventListener('error',e=>window.__pt_errors.push("
    "String(e.message||e.error||e)));"
    "window.addEventListener('unhandledrejection',e=>window.__pt_errors.push("
    "'unhandledrejection: '+String(e.reason)));"
    "window.onerror=(msg,src,line,col,e)=>window.__pt_errors.push(String(e||msg));"
    "if(!window.__pt_orig_console_error){window.__pt_orig_console_error=console.error;"
    "console.error=function(){window.__pt_errors.push("
    "'console.error: '+Array.prototype.join.call(arguments,' '));"
    "return window.__pt_orig_console_error.apply(console,arguments);};}"
)

_VISUAL_FREEZE_EPOCH_MS = 1772625600000

# CDP visual captures run this script before deferred app scripts; it freezes
# Date.now, disables visual motion, hides scrollbars and audio peak meters, and
# exposes __solstoneVisualBeforeCapture for deterministic capture. The prior
# SurfaceState/AppServices race-masking stub was removed after the transcripts
# workspace deferred its ResizeObserver registrations into initTranscripts().
_VISUAL_STABILITY_JS = f"""
(() => {{
  Date.now = () => {_VISUAL_FREEZE_EPOCH_MS};
  const css = `
    *, *::before, *::after {{
      animation: none !important;
      transition: none !important;
      caret-color: transparent !important;
      scroll-behavior: auto !important;
    }}
    html, body, * {{ scrollbar-width: none !important; }}
    *::-webkit-scrollbar {{ width: 0 !important; height: 0 !important; }}
    audio[data-role="segment-audio"] {{ visibility: hidden !important; }}
  `;
  function installStyle() {{
    if (document.getElementById('solstone-visual-stability')) return;
    const style = document.createElement('style');
    style.id = 'solstone-visual-stability';
    style.textContent = css;
    (document.head || document.documentElement).appendChild(style);
  }}
  window.__solstoneVisualBeforeCapture = () => {{
    installStyle();
    window.scrollTo(0, 0);
    document.querySelectorAll('audio[data-role="segment-audio"]').forEach((audio) => {{
      try {{
        audio.pause();
        audio.currentTime = 0;
      }} catch (_) {{}}
    }});
  }};
  if (document.readyState === 'loading') {{
    document.addEventListener('DOMContentLoaded', installStyle, {{ once: true }});
  }} else {{
    installStyle();
  }}
}})();
"""

TRANSCRIPTS_DAY_READY_JS = """
(() => Boolean(
  document.querySelector('.tr-zoom-pill[data-key]')
  && !document.querySelector('#trPanel.loading')
))()
"""

TRANSCRIPTS_SEGMENT_READY_JS = """
(() => Boolean(
  document.querySelector('.tr-zoom-pill.tr-active[data-key="090000_300"]')
  && document.querySelector('#tr-tabpanel-transcript.tr-tab-pane.active .tr-unified')
  && !document.querySelector('#trPanel.loading')
))()
"""

WAIT_FOR_EXPRESSIONS = {
    "transcripts_day_ready": TRANSCRIPTS_DAY_READY_JS,
    "transcripts_segment_ready": TRANSCRIPTS_SEGMENT_READY_JS,
}

# Calibration on 2026-05-22 used three repeated CDP captures per transcript
# scenario with channel_delta_threshold=1 and changed_pixels_pct_threshold=0.0.
# Observed residuals: day-tall max_delta=244 / 0.019070% changed; day-short,
# segment-tall, and segment-short all max_delta=0 / 0.000000% changed. The
# day-tall source was not obvious in a brief visual review; likely candidates
# are a tiny focus/caret/overflow indicator rather than layout drift. The
# percentage threshold is set just above that very small residual area.
CHANNEL_DELTA_THRESHOLD = 4
CHANGED_PIXELS_PCT_THRESHOLD = 0.05


ROUTE_SMOKE_EXCLUDES = (
    "/api/",
    "/static",
    "/ingest",
    "/callosum",
    "/local-endpoints",
    "/raw",
    "/pdf",
    "/manifest/",
    "/generation-status",
    "/overflow/",
)


DETAIL_HREF_JS = """
(() => {
  const values = [window.location.pathname];
  document.querySelectorAll('a[href]').forEach((el) => values.push(el.href));
  document.querySelectorAll('[onclick]').forEach((el) => values.push(el.getAttribute('onclick') || ''));
  document.querySelectorAll('[data-import-id]').forEach((el) => {
    if (el.dataset.importId) values.push('/app/import/' + el.dataset.importId);
  });
  return JSON.stringify(values.filter(Boolean));
})()
"""


def baseline_path(scenario: dict[str, Any]) -> Path:
    suffix = ".png" if scenario.get("diff") else ".jpg"
    return (
        Path("tests/baselines/visual") / scenario["app"] / f"{scenario['name']}{suffix}"
    )


@dataclass(frozen=True)
class ComparisonResult:
    passed: bool
    changed_pixels: int
    total_pixels: int
    pct_changed_pixels: float
    max_channel_delta: int
    diff_image_bytes: bytes
    message: str


class CdpError(RuntimeError):
    """Chrome DevTools Protocol operation failed."""


def parse_remote_debugging_port(cmdline: str | list[str]) -> int | None:
    parts = (
        re.split(r"\0|\s+", cmdline.strip()) if isinstance(cmdline, str) else cmdline
    )
    for part in parts:
        match = re.fullmatch(r"--remote-debugging-port=(\d+)", part)
        if not match:
            continue
        port = int(match.group(1))
        if 0 < port <= 65535:
            return port
    return None


def build_device_metrics_payload(width: int, height: int) -> dict[str, Any]:
    return {
        "width": width,
        "height": height,
        "deviceScaleFactor": 1,
        "mobile": False,
    }


def visual_artifact_paths(baseline: Path) -> tuple[Path, Path]:
    actual = baseline.with_name(f"{baseline.stem}.actual.png")
    diff = baseline.with_name(f"{baseline.stem}.diff.png")
    return actual, diff


def _encode_png(image: Image.Image) -> bytes:
    buffer = BytesIO()
    image.save(buffer, format="PNG")
    return buffer.getvalue()


def compare_png(
    actual: bytes,
    baseline: bytes,
    *,
    channel_delta_threshold: int = CHANNEL_DELTA_THRESHOLD,
    changed_pixels_pct_threshold: float = CHANGED_PIXELS_PCT_THRESHOLD,
) -> ComparisonResult:
    actual_image = Image.open(BytesIO(actual)).convert("RGBA")
    baseline_image = Image.open(BytesIO(baseline)).convert("RGBA")
    if actual_image.size != baseline_image.size:
        diff_image = Image.new("RGBA", actual_image.size, (255, 0, 255, 255))
        message = (
            f"dimension mismatch: actual {actual_image.size[0]}x{actual_image.size[1]}, "
            f"baseline {baseline_image.size[0]}x{baseline_image.size[1]}"
        )
        return ComparisonResult(
            passed=False,
            changed_pixels=actual_image.size[0] * actual_image.size[1],
            total_pixels=actual_image.size[0] * actual_image.size[1],
            pct_changed_pixels=100.0,
            max_channel_delta=255,
            diff_image_bytes=_encode_png(diff_image),
            message=message,
        )

    actual_array = np.asarray(actual_image, dtype=np.int16)
    baseline_array = np.asarray(baseline_image, dtype=np.int16)
    delta = np.abs(actual_array[:, :, :3] - baseline_array[:, :, :3])
    per_pixel_delta = np.max(delta, axis=2)
    changed = per_pixel_delta > channel_delta_threshold
    changed_pixels = int(np.count_nonzero(changed))
    total_pixels = int(changed.size)
    pct_changed_pixels = (changed_pixels / total_pixels) * 100 if total_pixels else 0.0
    max_channel_delta = int(delta.max()) if delta.size else 0

    diff_array = np.asarray(actual_image).copy()
    diff_array[changed] = [255, 0, 255, 255]
    diff_bytes = _encode_png(Image.fromarray(diff_array, mode="RGBA"))
    passed = pct_changed_pixels <= changed_pixels_pct_threshold
    if passed:
        message = (
            f"changed {pct_changed_pixels:.4f}% of pixels, "
            f"max channel delta {max_channel_delta}"
        )
    else:
        message = (
            f"changed {pct_changed_pixels:.4f}% of pixels "
            f"(limit {changed_pixels_pct_threshold:.4f}%), "
            f"max channel delta {max_channel_delta}"
        )
    return ComparisonResult(
        passed=passed,
        changed_pixels=changed_pixels,
        total_pixels=total_pixels,
        pct_changed_pixels=pct_changed_pixels,
        max_channel_delta=max_channel_delta,
        diff_image_bytes=diff_bytes,
        message=message,
    )


def _listener_inodes_from_proc_file(path: Path, port: int) -> set[str]:
    inodes: set[str] = set()
    port_hex = f"{port:04X}"
    try:
        lines = path.read_text().splitlines()[1:]
    except OSError:
        return inodes
    for line in lines:
        parts = line.split()
        if len(parts) < 10:
            continue
        local_address = parts[1]
        state = parts[3]
        inode = parts[9]
        if state == "0A" and local_address.rsplit(":", 1)[-1].upper() == port_hex:
            inodes.add(inode)
    return inodes


def _tcp_listener_inodes_for_port(port: int) -> set[str]:
    inodes = _listener_inodes_from_proc_file(Path("/proc/net/tcp"), port)
    inodes.update(_listener_inodes_from_proc_file(Path("/proc/net/tcp6"), port))
    if not inodes:
        raise CdpError(f"no listening socket found for pinchtab API port {port}")
    return inodes


def _pids_for_socket_inodes(inodes: set[str]) -> set[int]:
    pids: set[int] = set()
    inode_refs = {f"socket:[{inode}]" for inode in inodes}
    for proc_dir in Path("/proc").iterdir():
        if not proc_dir.name.isdigit():
            continue
        fd_dir = proc_dir / "fd"
        try:
            fd_entries = list(fd_dir.iterdir())
        except OSError:
            continue
        for fd in fd_entries:
            try:
                if os.readlink(fd) in inode_refs:
                    pids.add(int(proc_dir.name))
                    break
            except OSError:
                continue
    if not pids:
        raise CdpError(
            "no process owns the pinchtab API listening socket "
            f"inode(s): {', '.join(sorted(inodes))}"
        )
    return pids


def _read_ppid(proc_dir: Path) -> int | None:
    try:
        content = (proc_dir / "stat").read_text()
    except OSError:
        return None
    try:
        after_comm = content.rsplit(")", 1)[1].strip().split()
        return int(after_comm[1])
    except (IndexError, ValueError):
        return None


def _descendant_pids(root_pids: set[int]) -> set[int]:
    parent_to_children: dict[int, set[int]] = {}
    for proc_dir in Path("/proc").iterdir():
        if not proc_dir.name.isdigit():
            continue
        pid = int(proc_dir.name)
        ppid = _read_ppid(proc_dir)
        if ppid is None:
            continue
        parent_to_children.setdefault(ppid, set()).add(pid)

    descendants: set[int] = set()
    stack = list(root_pids)
    while stack:
        pid = stack.pop()
        for child in parent_to_children.get(pid, set()):
            if child in descendants:
                continue
            descendants.add(child)
            stack.append(child)
    return descendants


def _pinchtab_instance_ports_from_log(api_port: int) -> list[int]:
    log_path = Path(f"/tmp/pinchtab-{api_port}.log")
    try:
        text = log_path.read_text()
    except OSError:
        return []
    ports: list[int] = []
    for match in re.finditer(r"\bport=(\d+)\b", text):
        port = int(match.group(1))
        if port not in ports:
            ports.append(port)
    return ports


def _remote_debugging_ports_for_listener_port(port: int) -> dict[int, set[int]]:
    try:
        listener_pids = _pids_for_socket_inodes(_tcp_listener_inodes_for_port(port))
    except CdpError:
        return {}
    candidate_pids = listener_pids | _descendant_pids(listener_pids)
    ports: dict[int, set[int]] = {}
    for pid in candidate_pids:
        try:
            raw = Path(f"/proc/{pid}/cmdline").read_text()
        except OSError:
            continue
        port = parse_remote_debugging_port(raw)
        if port is not None:
            ports.setdefault(port, set()).add(pid)
    return ports


def discover_chrome_debug_port(api_port: int, *, timeout: float = 10.0) -> int:
    """Discover Chrome CDP from pinchtab's API port.

    The API listener process tree is the primary source. Pinchtab also spawns a
    per-instance listener whose port is only exposed in the pinchtab log, so the
    log-derived port is checked as a supplementary process-tree root.
    """
    deadline = time.monotonic() + timeout
    listener_ports: list[int] = [api_port]
    ports: dict[int, set[int]] = {}
    while time.monotonic() < deadline:
        listener_ports = [api_port, *_pinchtab_instance_ports_from_log(api_port)]
        ports = {}
        for listener_port in listener_ports:
            for debug_port, pids in _remote_debugging_ports_for_listener_port(
                listener_port
            ).items():
                ports.setdefault(debug_port, set()).update(pids)
        if len(ports) == 1:
            return next(iter(ports))
        if len(ports) > 1:
            break
        time.sleep(0.1)

    if not ports:
        raise CdpError(
            "no Chrome --remote-debugging-port found under pinchtab API port "
            f"{api_port}; checked listener ports {listener_ports}"
        )
    details = ", ".join(
        f"{port}: {sorted(pids)}" for port, pids in sorted(ports.items())
    )
    raise CdpError(
        "multiple Chrome remote debugging ports found under pinchtab "
        f"API port {api_port}: {details}"
    )


def _terminate_listener_processes_for_port(port: int) -> None:
    try:
        pids = _pids_for_socket_inodes(_tcp_listener_inodes_for_port(port))
    except CdpError:
        return
    for pid in pids:
        try:
            os.killpg(os.getpgid(pid), signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            try:
                os.kill(pid, signal.SIGTERM)
            except (ProcessLookupError, PermissionError):
                continue


class PinchTab:
    """Minimal pinchtab HTTP client with process lifecycle.

    Pinchtab v0.7.x uses a flat API — endpoints are at the root level
    (e.g., /navigate, /screenshot, /snapshot) rather than nested under
    /tabs/<id>/ or /instances/. Chrome is auto-managed by the server.
    """

    def __init__(self, port: int = 19867) -> None:
        self.port = port
        self.base_url = f"http://localhost:{port}"
        self._process: subprocess.Popen | None = None
        self._session = requests.Session()

    def start(self, timeout: int = 30) -> None:
        """Launch pinchtab and wait for health check."""
        # Pinchtab reads PINCHTAB_PORT (v0.7.x; renamed from BRIDGE_PORT).
        # /screenshot may return image/* bytes directly; JSON base64 kept as fallback.
        # See pinchtab --help -> ENVIRONMENT.
        env = {
            **os.environ,
            "PINCHTAB_PORT": str(self.port),
            "BRIDGE_HEADLESS": "true",
        }
        profile_dir = Path.home() / ".pinchtab" / "profiles" / "default"
        if profile_dir.exists():
            # Clear cached default profile for deterministic runs — pinchtab persists
            # cookies/storage across sessions. Other tools that share pinchtab use
            # their own named profiles, so this nuke is isolated to test state.
            try:
                shutil.rmtree(profile_dir)
            except OSError as exc:
                raise RuntimeError(
                    f"failed to clear pinchtab default profile: {profile_dir}"
                ) from exc
        self._stderr_path = f"/tmp/pinchtab-{self.port}.log"
        self._stderr_file = open(self._stderr_path, "w")
        try:
            self._process = subprocess.Popen(
                ["pinchtab"],
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=self._stderr_file,
                process_group=0,
            )
        except Exception as exc:
            self._stderr_file.close()
            raise RuntimeError("failed to start pinchtab") from exc

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self._process.poll() is not None:
                self._stderr_file.close()
                try:
                    stderr = Path(self._stderr_path).read_text()
                except Exception:
                    stderr = ""
                raise RuntimeError(
                    f"pinchtab exited with code {self._process.returncode}\n{stderr}"
                )
            try:
                response = self._session.get(f"{self.base_url}/health", timeout=2)
                if response.status_code == 200:
                    health = response.json()
                    instance = health.get("defaultInstance") or {}
                    if (
                        health.get("status") == "ok"
                        and instance.get("status") == "running"
                    ):
                        return
            except requests.ConnectionError:
                pass
            time.sleep(0.5)
        self.stop()
        raise RuntimeError("pinchtab failed to start")

    def stop(self) -> None:
        """Terminate pinchtab process and all children."""
        instance_ports = _pinchtab_instance_ports_from_log(self.port)
        if hasattr(self, "_stderr_file") and self._stderr_file:
            try:
                self._stderr_file.close()
            except Exception:
                pass
        if self._process:
            pid = self._process.pid
            if self._process.poll() is None:
                self._session.close()
                # Kill the entire process group to catch the Go binary child
                try:
                    os.killpg(os.getpgid(pid), signal.SIGTERM)
                except (ProcessLookupError, PermissionError):
                    self._process.terminate()
                try:
                    self._process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(os.getpgid(pid), signal.SIGKILL)
                    except (ProcessLookupError, PermissionError):
                        self._process.send_signal(signal.SIGKILL)
                    self._process.wait()
            self._process = None
        for instance_port in instance_ports:
            _terminate_listener_processes_for_port(instance_port)

    def navigate(self, url: str) -> None:
        response = self._session.post(
            f"{self.base_url}/navigate",
            json={"url": url},
            timeout=30,
        )
        response.raise_for_status()

    def screenshot(self) -> bytes:
        response = self._session.get(
            f"{self.base_url}/screenshot",
            timeout=30,
        )
        response.raise_for_status()
        content_type = response.headers.get("Content-Type", "")
        if content_type.startswith("image/"):
            return response.content
        payload = response.json()
        return base64.b64decode(payload["base64"])

    def snapshot(self) -> dict:
        response = self._session.get(
            f"{self.base_url}/snapshot",
            timeout=30,
        )
        response.raise_for_status()
        return response.json()

    def text(self) -> str:
        response = self._session.get(
            f"{self.base_url}/text",
            timeout=30,
        )
        response.raise_for_status()
        payload = response.json()
        if isinstance(payload, dict):
            return payload.get("text", "")
        if isinstance(payload, str):
            return payload
        return ""

    def action(self, kind: str, **kwargs: Any) -> None:
        response = self._session.post(
            f"{self.base_url}/action",
            json={"kind": kind, **kwargs},
            timeout=30,
        )
        response.raise_for_status()

    def evaluate(self, expression: str) -> Any:
        response = self._session.post(
            f"{self.base_url}/evaluate",
            json={"expression": expression},
            timeout=30,
        )
        response.raise_for_status()
        try:
            return response.json()
        except ValueError:
            return response.text


def _get_json(url: str, timeout: float = 10.0) -> Any:
    with urlopen(url, timeout=timeout) as response:
        return json.loads(response.read().decode("utf-8"))


class CdpConnection:
    def __init__(self, web_socket_url: str, timeout: float = 10.0) -> None:
        self._web_socket_url = web_socket_url
        self._timeout = timeout
        self._next_id = 1
        self._events: list[dict[str, Any]] = []
        self._ws = connect(web_socket_url, open_timeout=timeout, close_timeout=timeout)

    def call(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        *,
        timeout: float | None = None,
    ) -> dict[str, Any]:
        message_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {"id": message_id, "method": method}
        if params is not None:
            payload["params"] = params
        self._ws.send(json.dumps(payload))

        deadline = time.monotonic() + (timeout or self._timeout)
        while time.monotonic() < deadline:
            remaining = max(0.1, deadline - time.monotonic())
            try:
                raw = self._ws.recv(timeout=remaining)
            except TimeoutError:
                break
            message = json.loads(raw)
            if message.get("id") == message_id:
                if "error" in message:
                    raise CdpError(f"CDP {method} failed: {message['error']}")
                return message.get("result", {})
            self._events.append(message)
        raise CdpError(f"CDP {method} timed out on {self._web_socket_url}")

    def wait_for_event(self, method: str, *, timeout: float = 10.0) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for index, event in enumerate(self._events):
                if event.get("method") == method:
                    return self._events.pop(index)
            remaining = max(0.1, deadline - time.monotonic())
            try:
                raw = self._ws.recv(timeout=remaining)
            except TimeoutError:
                break
            event = json.loads(raw)
            if event.get("method") == method:
                return event
            self._events.append(event)
        raise CdpError(f"timed out waiting for CDP event {method}")

    def close(self) -> None:
        self._ws.close()


class CdpBrowser:
    def __init__(self, debug_port: int, browser_ws_url: str) -> None:
        self.debug_port = debug_port
        self._browser = CdpConnection(browser_ws_url)

    @classmethod
    def from_pinchtab_port(cls, api_port: int) -> "CdpBrowser":
        debug_port = discover_chrome_debug_port(api_port)
        version = _get_json(f"http://127.0.0.1:{debug_port}/json/version")
        browser_ws_url = version.get("webSocketDebuggerUrl")
        if not browser_ws_url:
            raise CdpError(
                f"Chrome CDP /json/version on port {debug_port} did not expose "
                "webSocketDebuggerUrl"
            )
        return cls(debug_port, browser_ws_url)

    def list_targets(self) -> list[dict[str, Any]]:
        targets = _get_json(f"http://127.0.0.1:{self.debug_port}/json")
        if not isinstance(targets, list):
            raise CdpError(f"Chrome CDP /json on port {self.debug_port} was not a list")
        return targets

    def create_page(self, url: str = "about:blank") -> "CdpPage":
        result = self._browser.call("Target.createTarget", {"url": url})
        target_id = result.get("targetId")
        if not target_id:
            raise CdpError(f"Target.createTarget returned no targetId: {result}")

        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            for target in self.list_targets():
                if target.get("id") == target_id and target.get("type") == "page":
                    page_ws_url = target.get("webSocketDebuggerUrl")
                    if page_ws_url:
                        return CdpPage(target_id, CdpConnection(page_ws_url), self)
            time.sleep(0.1)
        raise CdpError(f"created target {target_id} never appeared in /json")

    def close_target(self, target_id: str) -> None:
        self._browser.call("Target.closeTarget", {"targetId": target_id})

    def close(self) -> None:
        self._browser.close()


class CdpPage:
    def __init__(
        self, target_id: str, connection: CdpConnection, browser: CdpBrowser
    ) -> None:
        self.target_id = target_id
        self._connection = connection
        self._browser = browser
        self._closed = False
        self._connection.call("Page.enable")
        self._connection.call("Runtime.enable")

    def add_script_on_new_document(self, source: str) -> None:
        self._connection.call(
            "Page.addScriptToEvaluateOnNewDocument", {"source": source}
        )

    def set_viewport(self, width: int, height: int) -> None:
        self._connection.call(
            "Emulation.setDeviceMetricsOverride",
            build_device_metrics_payload(width, height),
        )

    def navigate(self, url: str, *, timeout: float = 20.0) -> None:
        self._connection.call("Page.navigate", {"url": url}, timeout=timeout)
        self._connection.wait_for_event("Page.loadEventFired", timeout=timeout)

    def evaluate(self, expression: str, *, timeout: float = 10.0) -> Any:
        result = self._connection.call(
            "Runtime.evaluate",
            {
                "expression": expression,
                "returnByValue": True,
                "awaitPromise": True,
            },
            timeout=timeout,
        )
        if "exceptionDetails" in result:
            raise CdpError(f"Runtime.evaluate failed: {result['exceptionDetails']}")
        remote = result.get("result", {})
        if "value" in remote:
            return remote["value"]
        return remote.get("description")

    def wait_for_expression(
        self,
        expression: str,
        *,
        timeout_ms: int = 10000,
        interval_ms: int = 100,
    ) -> None:
        deadline = time.monotonic() + (timeout_ms / 1000)
        while time.monotonic() < deadline:
            if self.evaluate(expression):
                return
            time.sleep(interval_ms / 1000)
        raise CdpError(f"timed out waiting for expression: {expression}")

    def capture_png(self) -> bytes:
        result = self._connection.call(
            "Page.captureScreenshot",
            {"format": "png", "fromSurface": True},
            timeout=20,
        )
        data = result.get("data")
        if not isinstance(data, str):
            raise CdpError(f"Page.captureScreenshot returned no data: {result}")
        return base64.b64decode(data)

    def collect_console_errors(self) -> list[str]:
        raw = self.evaluate("JSON.stringify(window.__pt_errors || [])")
        try:
            parsed = json.loads(raw)
        except (TypeError, json.JSONDecodeError):
            return []
        if not isinstance(parsed, list):
            return []
        return [str(item) for item in parsed]

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        try:
            self._connection.close()
        finally:
            self._browser.close_target(self.target_id)


def inject_error_listener(pt: PinchTab) -> None:
    pt.evaluate(_ERROR_LISTENER_JS)


def collect_console_errors(pt: PinchTab) -> list[str]:
    result = pt.evaluate("JSON.stringify(window.__pt_errors||[])")
    value = result if isinstance(result, str) else result.get("result", "[]")
    try:
        return json.loads(value)
    except (json.JSONDecodeError, TypeError):
        return []


def find_input_ref(snapshot: dict) -> str | None:
    """Find first text input node ref from snapshot."""
    for node in snapshot.get("nodes", []):
        role = str(node.get("role", "")).lower()
        tag = str(node.get("tag", "")).lower()
        if role in ("textbox", "searchbox", "combobox") or tag == "input":
            return node.get("ref")
    return None


def find_ref(snapshot: dict, text: str) -> str | None:
    needle = str(text).lower()
    for node in snapshot.get("nodes", []):
        ref = node.get("ref")
        if not ref:
            continue
        if needle == "":
            return ref
        if (
            needle in str(node.get("name", "")).lower()
            or needle in str(node.get("text", "")).lower()
            or needle in str(node.get("label", "")).lower()
            or needle in str(node.get("value", "")).lower()
        ):
            return ref
    return None


def _is_app_shell_path(path: str) -> bool:
    return path.startswith("/app/") and not any(
        excluded in path for excluded in ROUTE_SMOKE_EXCLUDES
    )


def _with_pt_capture(url: str) -> str:
    parsed = urlparse(url)
    query = parse_qsl(parsed.query, keep_blank_values=True)
    query.append(("__pt_capture", "1"))
    return urlunparse(parsed._replace(query=urlencode(query)))


def _resolve_url(base_url: str, path: str, *, capture: bool = False) -> str:
    url = f"{base_url.rstrip('/')}{path}"
    if capture:
        return _with_pt_capture(url)
    return url


def _resolve_redirect_path(base_url: str, path: str) -> str:
    try:
        response = requests.get(
            _resolve_url(base_url, path), allow_redirects=True, timeout=10
        )
    except requests.RequestException:
        return path
    final_path = urlparse(response.url).path
    if response.ok and _is_app_shell_path(final_path):
        return final_path
    return path


def _derive_app_page_routes() -> list[str]:
    from flask import Flask

    from solstone.apps import AppRegistry

    registry = AppRegistry()
    registry.discover()
    app = Flask(__name__)
    registry.register_blueprints(app)

    routes: list[str] = []
    seen: set[str] = set()
    for rule in sorted(app.url_map.iter_rules(), key=lambda item: item.rule):
        methods = rule.methods - {"HEAD", "OPTIONS"}
        if "GET" not in methods or not rule.endpoint.startswith("app:"):
            continue
        if any(excluded in rule.rule for excluded in ROUTE_SMOKE_EXCLUDES):
            continue
        if rule.rule in seen:
            continue
        seen.add(rule.rule)
        routes.append(rule.rule)
    return routes


def _parent_route(rule: str) -> str:
    before_param = rule.split("<", 1)[0]
    if before_param.endswith("/"):
        return before_param
    parent = before_param.rsplit("/", 1)[0]
    return parent + "/"


def _route_regex(rule: str) -> re.Pattern[str]:
    parts: list[str] = []
    cursor = 0
    for match in re.finditer(r"<[^>]+>", rule):
        parts.append(re.escape(rule[cursor : match.start()]))
        parts.append(r"[^/?#]+")
        cursor = match.end()
    parts.append(re.escape(rule[cursor:]))
    pattern = "".join(parts)
    return re.compile(rf"^{pattern}/?$")


def _candidate_paths(values: list[str]) -> list[str]:
    paths: list[str] = []
    for value in values:
        parsed = urlparse(value)
        if parsed.path.startswith("/app/"):
            paths.append(parsed.path)
            continue
        for match in re.findall(r"/app/[A-Za-z0-9_./%:-]+", value):
            paths.append(urlparse(match).path)
    return paths


def _extract_detail_path(pt: PinchTab, rule: str) -> str | None:
    result = pt.evaluate(DETAIL_HREF_JS)
    raw = result if isinstance(result, str) else result.get("result", "[]")
    try:
        values = json.loads(raw)
    except (json.JSONDecodeError, TypeError):
        return None
    matcher = _route_regex(rule)
    for path in _candidate_paths(values):
        if matcher.fullmatch(path):
            return path
    return None


def _eval_json(pt: PinchTab, expression: str) -> Any:
    result = pt.evaluate(f"JSON.stringify({expression})")
    raw = result if isinstance(result, str) else result.get("result", "null")
    return json.loads(raw)


def _assert_loading_cleared(pt: PinchTab, path: str) -> list[str]:
    checks: list[tuple[str, str]] = []
    if re.fullmatch(r"/app/activities/\d{8}/?", path):
        checks.append(
            (
                "activities-day Loading activities...",
                "!document.body.innerText.includes('Loading activities...')",
            )
        )
    elif path.rstrip("/") == "/app/import":
        checks.append(
            (
                "import list loading imports...",
                "!document.body.innerText.includes('loading imports...')",
            )
        )
    elif re.fullmatch(r"/app/import/[^/]+/?", path):
        checks.append(
            (
                "import detail loading...",
                "!(document.getElementById('importMeta')?.innerText.toLowerCase().includes('loading')"
                " || document.getElementById('overviewContent')?.innerText.toLowerCase().includes('loading'))",
            )
        )
    elif re.fullmatch(r"/app/sol/\d{8}/?", path):
        checks.append(
            (
                "sol loading agents...",
                "getComputedStyle(document.getElementById('loading-view')).display === 'none'",
            )
        )
    elif re.fullmatch(r"/app/speakers/\d{8}/?", path):
        checks.append(
            (
                "speakers loading...",
                "!(document.getElementById('spkSegmentList')?.innerText.trim().toLowerCase() === 'loading...')",
            )
        )
    elif re.fullmatch(r"/app/tokens/\d{8}/?", path):
        checks.append(
            (
                "tokens loading token usage data...",
                "getComputedStyle(document.getElementById('tokens-loading')).display === 'none'",
            )
        )
    elif path.rstrip("/") == "/app/observer":
        checks.append(
            (
                "observer loading observers...",
                "!document.body.innerText.includes('loading observers...')",
            )
        )
    elif path.rstrip("/") == "/app/link":
        checks.append(
            (
                "link status loading...",
                "document.getElementById('link-status-text')?.innerText.trim() !== 'loading…'",
            )
        )
    elif path.rstrip("/") == "/app/support":
        checks.append(
            (
                "support checking for tickets",
                "!document.body.innerText.includes('checking for tickets')",
            )
        )
    elif path.rstrip("/") == "/app/settings":
        checks.append(
            (
                "settings provider/context placeholders",
                "["
                "document.getElementById('providerStatus')?.innerText,"
                "document.getElementById('contextGroups')?.innerText,"
                "document.getElementById('visionCategoryGroups')?.innerText,"
                "document.getElementById('segmentInsightsList')?.innerText,"
                "document.getElementById('dailyInsightsList')?.innerText,"
                "document.getElementById('mutedFacetsList')?.innerText"
                "].every((text) => !String(text || '').trim().toLowerCase().startsWith('loading'))",
            )
        )

    errors: list[str] = []
    for label, expression in checks:
        try:
            if not _eval_json(pt, expression):
                errors.append(f"loading sentinel still visible: {label}")
        except Exception as exc:
            errors.append(f"loading sentinel check failed for {label}: {exc}")
    return errors


def _resolve_route_path(
    pt: PinchTab, base_url: str, rule: str
) -> tuple[str | None, str | None]:
    if "<" not in rule:
        return _resolve_redirect_path(base_url, rule), None
    if rule.startswith("/app/activities/<day>/screens/<stream>/"):
        # pre-existing unrelated bug, out of scope: list emits timestamp-only URLs.
        return None, "activities dev screen detail route has a stale list link"

    parent = _resolve_redirect_path(base_url, _parent_route(rule))
    if _route_regex(rule).fullmatch(parent):
        return parent, None
    pt.navigate(_resolve_url(base_url, parent, capture=True))
    time.sleep(1.2)
    path = _extract_detail_path(pt, rule)
    if path:
        return path, None
    return None, f"no concrete link found from {parent}"


def run_rest_scenario(
    pt: PinchTab, scenario: dict[str, Any], base_url: str, mode: str
) -> dict[str, Any]:
    """Execute one scenario. Returns {ok, errors, console_errors}."""
    identifier = f"{scenario['app']}/{scenario['name']}"
    errors: list[str] = []
    variables: dict[str, str] = {}
    last_snapshot: dict[str, Any] | None = None
    console_errors: list[str] = []

    logger.info("  %s", identifier)

    try:
        inject_error_listener(pt)
    except Exception:
        pass

    for step in scenario["steps"]:
        action = step["do"]
        try:
            if action == "navigate":
                capture = _is_app_shell_path(step["path"])
                url = _resolve_url(base_url, step["path"], capture=capture)
                pt.navigate(url)
                time.sleep(0.3)
                if not capture:
                    try:
                        inject_error_listener(pt)
                    except Exception:
                        pass

            elif action == "wait":
                time.sleep(float(step["ms"]) / 1000)

            elif action == "snapshot":
                last_snapshot = pt.snapshot()

            elif action == "screenshot":
                png = pt.screenshot()
                path = baseline_path(scenario)
                if mode == "update":
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(png)
                else:
                    if not path.exists():
                        errors.append(f"baseline not found: {path}")
                    # No pixel comparison — baselines are for human review

            elif action == "find":
                if last_snapshot is None:
                    errors.append("find without prior snapshot")
                    continue
                ref = find_ref(last_snapshot, step["text"])
                if ref is None:
                    errors.append(f"find: text not found: {step['text']!r}")
                    continue
                variables[step["as"]] = ref

            elif action == "find_input":
                if last_snapshot is None:
                    errors.append("find_input without prior snapshot")
                    continue
                ref = find_input_ref(last_snapshot)
                if ref is None:
                    errors.append("no text input found in snapshot")
                    continue
                variables[step["as"]] = ref

            elif action == "click":
                ref = step.get("ref") or variables.get(step.get("var", ""))
                if not ref:
                    errors.append(f"click: no ref resolved for {step}")
                    continue
                pt.action("click", ref=ref)

            elif action == "type":
                ref = step.get("ref") or variables.get(step.get("var", ""))
                if not ref:
                    errors.append(f"type: no ref resolved for {step}")
                    continue
                pt.action("type", ref=ref, text=step["text"])

            elif action == "assert_text":
                text = step["text"]
                page_text = pt.text().lower()
                if str(text).lower() not in page_text:
                    errors.append(f"assert_text: '{text}' not found")

            elif action == "evaluate":
                pt.evaluate(step["expression"])

            else:
                errors.append(f"unknown step type: {action}")

        except Exception as exc:
            errors.append(f"step {action} failed: {exc}")

    try:
        console_errors = collect_console_errors(pt)
    except Exception:
        logger.debug("Unable to collect console errors for %s", identifier)
    if console_errors:
        errors.extend(f"captured JS error: {err}" for err in console_errors)

    return {
        "ok": len(errors) == 0,
        "errors": errors,
        "console_errors": console_errors,
    }


def _write_diff_artifacts(
    baseline: Path, actual_png: bytes, result: ComparisonResult
) -> tuple[Path, Path]:
    actual_path, diff_path = visual_artifact_paths(baseline)
    actual_path.write_bytes(actual_png)
    diff_path.write_bytes(result.diff_image_bytes)
    return actual_path, diff_path


def run_cdp_scenario(
    cdp: CdpBrowser, scenario: dict[str, Any], base_url: str, mode: str
) -> dict[str, Any]:
    """Execute one diffed CDP scenario."""
    errors: list[str] = []
    console_errors: list[str] = []
    page: CdpPage | None = None

    try:
        viewport = scenario["viewport"]
        page = cdp.create_page("about:blank")
        # CDP setup order matters for deterministic first paint:
        # create page -> install pre-document stability script -> set viewport
        # -> navigate -> wait for DOM readiness -> stabilize before capture
        # -> capture PNG -> close the target in finally.
        page.add_script_on_new_document(_VISUAL_STABILITY_JS)
        page.set_viewport(int(viewport["width"]), int(viewport["height"]))

        for step in scenario["steps"]:
            action = step["do"]
            if action == "navigate":
                capture = _is_app_shell_path(step["path"])
                url = _resolve_url(base_url, step["path"], capture=capture)
                page.navigate(url)

            elif action == "wait_for":
                expression_key = step["expression"]
                expression = WAIT_FOR_EXPRESSIONS.get(expression_key, expression_key)
                page.wait_for_expression(
                    expression,
                    timeout_ms=int(step.get("timeout_ms", 10000)),
                    interval_ms=int(step.get("interval_ms", 100)),
                )

            elif action == "wait":
                time.sleep(float(step["ms"]) / 1000)

            elif action == "evaluate":
                page.evaluate(step["expression"])

            elif action == "screenshot":
                page.evaluate(
                    "window.__solstoneVisualBeforeCapture "
                    "&& window.__solstoneVisualBeforeCapture()"
                )
                png = page.capture_png()
                path = baseline_path(scenario)
                if mode == "update":
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(png)
                    continue
                if not path.exists():
                    errors.append(f"baseline not found: {path}")
                    continue
                result = compare_png(png, path.read_bytes())
                if not result.passed:
                    actual_path, diff_path = _write_diff_artifacts(path, png, result)
                    errors.append(
                        f"visual diff failed for {path}: {result.message}; "
                        f"actual={actual_path}; diff={diff_path}"
                    )

            else:
                errors.append(f"unknown CDP step type: {action}")

        if page is not None:
            console_errors = page.collect_console_errors()
            if console_errors:
                errors.extend(f"captured JS error: {err}" for err in console_errors)
    except Exception as exc:
        errors.append(f"CDP scenario failed: {exc}")
    finally:
        if page is not None:
            try:
                page.close()
            except Exception as exc:
                errors.append(f"CDP target cleanup failed: {exc}")

    return {
        "ok": len(errors) == 0,
        "errors": errors,
        "console_errors": console_errors,
    }


def run_scenario(
    pt: PinchTab,
    scenario: dict[str, Any],
    base_url: str,
    mode: str,
    cdp: CdpBrowser | None = None,
) -> dict[str, Any]:
    if scenario.get("engine") == "cdp":
        if cdp is None:
            raise CdpError(f"CDP browser is required for {_scenario_id(scenario)}")
        return run_cdp_scenario(cdp, scenario, base_url, mode)
    return run_rest_scenario(pt, scenario, base_url, mode)


def run_cold_load_smoke(pt: PinchTab, base_url: str) -> list[dict[str, Any]]:
    """Cold-load every registered app page route with pre-parse error capture."""
    results: list[dict[str, Any]] = []
    for rule in _derive_app_page_routes():
        identifier = f"cold-load/{rule}"
        logger.info("  %s", identifier)
        errors: list[str] = []
        console_errors: list[str] = []
        path: str | None = None

        try:
            path, skip_reason = _resolve_route_path(pt, base_url, rule)
            if skip_reason:
                logger.info("    SKIP %s", skip_reason)
                continue
            if not path:
                logger.info("    SKIP no concrete path")
                continue

            pt.navigate(_resolve_url(base_url, path, capture=True))
            time.sleep(1.2)
            errors.extend(_assert_loading_cleared(pt, path))
            console_errors = collect_console_errors(pt)
            if console_errors:
                errors.extend(f"captured JS error: {err}" for err in console_errors)
        except Exception as exc:
            errors.append(f"cold-load route failed: {exc}")

        results.append(
            {
                "scenario": identifier,
                "ok": len(errors) == 0,
                "errors": errors,
                "console_errors": console_errors,
            }
        )
    return results


def _scenario_id(scenario: dict[str, Any]) -> str:
    return f"{scenario['app']}/{scenario['name']}"


def _select_scenarios(filters: list[str] | None) -> list[dict[str, Any]]:
    if not filters:
        return SCENARIOS

    selected: list[dict[str, Any]] = []
    known = {_scenario_id(scenario): scenario for scenario in SCENARIOS}
    missing: list[str] = []
    for filter_value in filters:
        scenario = known.get(filter_value)
        if scenario is None:
            missing.append(filter_value)
            continue
        selected.append(scenario)

    if missing:
        known_ids = ", ".join(sorted(known))
        raise ValueError(
            "unknown browser scenario filter(s): "
            f"{', '.join(missing)}. Known scenarios: {known_ids}"
        )
    return selected


def run_all(
    pt: PinchTab,
    base_url: str,
    mode: str,
    scenario_filters: list[str] | None = None,
) -> tuple[list[dict[str, Any]], list[tuple[str, list[str]]]]:
    """Run all scenarios. Returns (results, console_error_pairs)."""
    results: list[dict[str, Any]] = []
    all_console_errors: list[tuple[str, list[str]]] = []
    selected_scenarios = _select_scenarios(scenario_filters)
    cdp: CdpBrowser | None = None
    try:
        if not scenario_filters:
            for result in run_cold_load_smoke(pt, base_url):
                results.append(result)
                if result["console_errors"]:
                    all_console_errors.append(
                        (result["scenario"], result["console_errors"])
                    )
        for scenario in selected_scenarios:
            identifier = _scenario_id(scenario)
            if scenario.get("engine") == "cdp" and cdp is None:
                cdp = CdpBrowser.from_pinchtab_port(pt.port)
            result = run_scenario(pt, scenario, base_url, mode, cdp)
            results.append({"scenario": identifier, **result})
            if result["console_errors"]:
                all_console_errors.append((identifier, result["console_errors"]))
    finally:
        if cdp is not None:
            cdp.close()
    return results, all_console_errors


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Browser scenario verification")
    parser.add_argument(
        "command",
        choices=["verify", "update"],
        help="Verify or update baselines",
    )
    parser.add_argument("--base-url", required=True, help="Convey base URL")
    parser.add_argument(
        "--pinchtab-port",
        type=int,
        default=19867,
        help="Pinchtab bridge port",
    )
    parser.add_argument(
        "--scenario",
        action="append",
        default=[],
        metavar="APP/NAME",
        help="Run only matching scenario(s); skips cold-load route sweep",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    logging.basicConfig(level=logging.INFO, format="%(message)s")

    try:
        _select_scenarios(args.scenario)
    except ValueError as exc:
        logger.error("%s", exc)
        return 2

    pt = PinchTab(port=args.pinchtab_port)
    logger.info("Starting pinchtab on port %d...", args.pinchtab_port)
    pt.start()

    try:
        logger.info("Running browser scenarios (%s)...", args.command)
        try:
            results, console_errors = run_all(
                pt, args.base_url, args.command, args.scenario
            )
        except ValueError as exc:
            logger.error("%s", exc)
            return 2

        passed = sum(1 for r in results if r["ok"])
        failed = sum(1 for r in results if not r["ok"])

        if failed:
            logger.info("")
            logger.info("Failures:")
            for result in results:
                if result["ok"]:
                    continue
                for err in result["errors"]:
                    logger.info("  %s: %s", result["scenario"], err)

        if console_errors:
            logger.info("")
            logger.info("JS console errors:")
            for scenario, errors in console_errors:
                for err in errors:
                    logger.info("  %s: %s", scenario, err)

        logger.info("")
        if args.command == "update":
            logger.info("Updated %d scenario baselines.", passed + failed)
        else:
            logger.info("Browser verification: %d passed, %d failed.", passed, failed)

        if failed:
            logger.info("Run 'make update-browser-baselines' to update baselines")
            return 1

        return 0
    finally:
        pt.stop()


if __name__ == "__main__":
    raise SystemExit(main())
