# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Portal HTTP dispatch for chat-request pushes and dedup silent pushes via scout-enabled worker."""

from __future__ import annotations

import json
import logging
import socket
from urllib import request as urllib_request
from urllib.error import HTTPError, URLError

from solstone.think.services.portal_client import portal_base_url, request_headers
from solstone.think.services.scout import scout_provenance

logger = logging.getLogger(__name__)

_TIMEOUT_SECONDS = 10


def dispatch_via_portal(*, request_id: str, summary: str, category: str) -> dict | None:
    scout = scout_provenance()
    if not scout:
        return None
    dispatch_token = scout.get("dispatch_token")
    if not dispatch_token:
        return None

    body = json.dumps(
        {"summary": summary, "category": category, "request_id": request_id}
    ).encode("utf-8")
    headers = request_headers("push")
    headers.update(
        {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {dispatch_token}",
        }
    )
    request = urllib_request.Request(
        f"{portal_base_url()}/push/dispatch",
        data=body,
        headers=headers,
        method="POST",
    )

    try:
        with urllib_request.urlopen(request, timeout=_TIMEOUT_SECONDS) as response:
            status = int(getattr(response, "status", response.getcode()))
            raw_body = response.read()
    except HTTPError as exc:
        status = int(exc.code)
        if 400 <= status < 500:
            logger.warning(
                "portal dispatch rejected: status=%s request_id=%s",
                status,
                request_id,
            )
        else:
            logger.warning(
                "portal dispatch server error: status=%s request_id=%s",
                status,
                request_id,
            )
        return None
    except (URLError, socket.timeout, TimeoutError) as exc:
        logger.warning(
            "portal dispatch transport failure: request_id=%s error=%s",
            request_id,
            type(exc).__name__,
        )
        return None
    except Exception as exc:
        logger.warning(
            "portal dispatch transport failure: request_id=%s error=%s",
            request_id,
            type(exc).__name__,
        )
        return None

    if not 200 <= status < 300:
        return None
    if not raw_body:
        return {"ok": True}
    try:
        payload = json.loads(raw_body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {"ok": True}
    return payload if isinstance(payload, dict) else {"ok": True}


def dispatch_dedup_via_portal(*, request_id: str, action: str) -> dict | None:
    scout = scout_provenance()
    if not scout:
        return None
    dispatch_token = scout.get("dispatch_token")
    if not dispatch_token:
        return None

    body = json.dumps({"request_id": request_id, "action": action}).encode("utf-8")
    headers = request_headers("push")
    headers.update(
        {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {dispatch_token}",
        }
    )
    request = urllib_request.Request(
        f"{portal_base_url()}/push/dedup",
        data=body,
        headers=headers,
        method="POST",
    )

    try:
        with urllib_request.urlopen(request, timeout=_TIMEOUT_SECONDS) as response:
            status = int(getattr(response, "status", response.getcode()))
            raw_body = response.read()
    except HTTPError as exc:
        status = int(exc.code)
        if 400 <= status < 500:
            logger.warning(
                "portal dedup rejected: status=%s request_id=%s",
                status,
                request_id,
            )
        else:
            logger.warning(
                "portal dedup server error: status=%s request_id=%s",
                status,
                request_id,
            )
        return None
    except (URLError, socket.timeout, TimeoutError) as exc:
        logger.warning(
            "portal dedup transport failure: request_id=%s error=%s",
            request_id,
            type(exc).__name__,
        )
        return None
    except Exception as exc:
        logger.warning(
            "portal dedup transport failure: request_id=%s error=%s",
            request_id,
            type(exc).__name__,
        )
        return None

    if not 200 <= status < 300:
        return None
    if not raw_body:
        return {"ok": True}
    try:
        payload = json.loads(raw_body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return {"ok": True}
    return payload if isinstance(payload, dict) else {"ok": True}
