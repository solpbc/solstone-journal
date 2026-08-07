# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""OpenAPI fragment for the native-client link routes."""

from __future__ import annotations

from solstone.convey.contract import (
    FieldSpec,
    OperationSpec,
    ParamSpec,
    RequestSpec,
    ResponseSpec,
)


def _json_error(
    status: int,
    reason_codes: tuple[str, ...],
    description: str,
) -> ResponseSpec:
    return ResponseSpec(
        status=status,
        description=description,
        reason_codes=reason_codes,
    )


_LOCAL_ENDPOINT_SCHEMA = {
    "type": "array",
    "items": {
        "type": "object",
        "additionalProperties": True,
        "properties": {
            "ip": {"type": "string"},
            "port": {"type": "integer"},
            "scope": {"type": "string"},
        },
        "required": ["ip", "port", "scope"],
    },
}

_REVOKED_OBSERVER_SCHEMA = {
    "type": "array",
    "items": {
        "type": "object",
        "additionalProperties": False,
        "properties": {
            "name": {"type": "string"},
            "prefix": {"type": "string"},
        },
        "required": ["name", "prefix"],
    },
}

_DEVICE_SCHEMA = {
    "type": "array",
    "items": {
        "type": "object",
        "additionalProperties": True,
        "properties": {
            "fingerprint": {"type": "string"},
            "fingerprint_short": {"type": "string"},
            "device_label": {"type": "string"},
            "display_label": {"type": "string"},
            "client_label": {"type": "string"},
            "paired_at": {"type": "string"},
            "last_seen_at": {"type": ["string", "null"]},
            "role": {"type": "string"},
            "network": {"type": ["string", "null"]},
            "kind": {"type": "string"},
            "observer_handle": {"type": ["string", "null"]},
        },
        "required": [
            "fingerprint",
            "fingerprint_short",
            "device_label",
            "display_label",
            "client_label",
            "paired_at",
            "last_seen_at",
            "role",
            "network",
            "kind",
            "observer_handle",
        ],
    },
}

OPERATIONS: list[OperationSpec] = [
    OperationSpec(
        operation_id="link.pairStart",
        method="POST",
        rule="/app/network/pair-start",
        summary="Start link pairing",
        description=(
            "Create a short-lived pairing nonce and return the link payload a "
            "native client can scan or open."
        ),
        request=RequestSpec(
            fields=(
                FieldSpec("device_label", "string"),
                FieldSpec("role", "string"),
                FieldSpec("same_machine", "boolean"),
            ),
            example={
                "device_label": "Jer iPhone",
                "role": "phone",
                "same_machine": False,
            },
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="Pairing nonce and link payload.",
                named_fields=(
                    FieldSpec("nonce", "string", required=True),
                    FieldSpec("pair_link", "string", required=True),
                    FieldSpec("expires_in", "integer", required=True),
                    FieldSpec("device_label", "string", required=True),
                    FieldSpec("ca_fingerprint", "string", required=True),
                ),
                example={
                    "nonce": "5f0d8c8b9f1e48b0a5f80b98f3d5e9b0",
                    "pair_link": "https://solstone.link/pair#0ABCD...",
                    "expires_in": 300,
                    "device_label": "Jer iPhone",
                    "ca_fingerprint": "9c5f2e0c8e6a42f0a32e55e5cf7f5b4a",
                },
            ),
            _json_error(
                400,
                ("invalid_operation_for_state", "pairing_request_invalid"),
                "Pair-start request rejected by handler validation.",
            ),
            _json_error(
                403,
                ("local_request_only", "pl_revoked"),
                "Access gate rejected the pair-start request.",
            ),
            _json_error(
                503,
                ("pairing_relay_unavailable",),
                "Pair-window could not be opened with the relay.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.devices",
        method="GET",
        rule="/app/network/api/devices",
        summary="List paired link devices",
        description="Return paired devices for native link list commands.",
        responses=(
            ResponseSpec(
                status=200,
                description="Paired devices.",
                named_fields=(
                    FieldSpec(
                        "devices",
                        "array",
                        required=True,
                        raw_schema=_DEVICE_SCHEMA,
                    ),
                ),
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.rename",
        method="POST",
        rule="/app/network/rename",
        summary="Rename a paired link device",
        description="Persist a home-assigned display label for a paired device.",
        request=RequestSpec(
            fields=(
                FieldSpec("fingerprint", "string", required=True),
                FieldSpec("label", "string", required=True),
            ),
            example={
                "fingerprint": "sha256:4bf5122f344554c53bde2ebb8cd2b7e3...",
                "label": "Jer iPhone",
            },
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="Persisted paired-device labels.",
                named_fields=(
                    FieldSpec("fingerprint", "string", required=True),
                    FieldSpec("device_label", "string", required=True),
                    FieldSpec("display_label", "string", required=True),
                ),
                example={
                    "fingerprint": "sha256:4bf5122f344554c53bde2ebb8cd2b7e3...",
                    "device_label": "Jer iPhone",
                    "display_label": "Jer iPhone",
                },
            ),
            _json_error(
                400,
                ("invalid_request_value", "missing_required_field"),
                "Rename request validation failed.",
            ),
            _json_error(
                404,
                ("paired_device_not_found",),
                "Fingerprint is not currently paired.",
            ),
            _json_error(
                500,
                ("convey_operation_failed",),
                "Rename label persistence failed.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.private-link.status",
        method="GET",
        rule="/app/network/api/private-link",
        summary="Read private network status",
        description="Return private network posture and operation state.",
        responses=(
            ResponseSpec(
                status=200,
                description="Private network status.",
                named_fields=(
                    FieldSpec("success", "boolean", required=True),
                    FieldSpec("service", "string", required=True),
                    FieldSpec("state", "string", required=True),
                    FieldSpec("posture", "string", required=True),
                    FieldSpec("enrolled", "boolean", required=True),
                    FieldSpec("relay_url", "string", required=True),
                    FieldSpec("actions", "object", required=True),
                    FieldSpec("operation", "object", required=True),
                ),
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.private-link.setup",
        method="POST",
        rule="/app/network/private-link/enable",
        summary="Start private network setup",
        description="Begin SPL private-link enrollment and return operation state.",
        responses=(
            ResponseSpec(
                status=202,
                description="Private network setup operation started.",
                named_fields=(
                    FieldSpec("success", "boolean", required=True),
                    FieldSpec("service", "string", required=True),
                    FieldSpec("operation", "object", required=True),
                ),
            ),
            _json_error(
                400,
                ("invalid_operation_for_state",),
                "The private network is already enabled or cannot be enabled.",
            ),
            _json_error(
                500,
                ("service_operation_failed",),
                "The consent link could not be prepared.",
            ),
            _json_error(
                503,
                ("service_busy",),
                "A service operation is already running.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.private-link.disable",
        method="POST",
        rule="/app/network/private-link/disable",
        summary="Disable private network",
        description="Turn off SPL private-link posture.",
        responses=(
            ResponseSpec(
                status=200,
                description="Private network disable result.",
                named_fields=(
                    FieldSpec("success", "boolean", required=True),
                    FieldSpec("service", "string", required=True),
                    FieldSpec("result", "object", required=True),
                    FieldSpec("status", "object", required=True),
                ),
            ),
            _json_error(
                500,
                ("service_operation_failed",),
                "The private network could not be disabled.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.pairNonceStatus",
        method="GET",
        rule="/app/network/api/pair/nonce-status",
        summary="Read pair nonce status",
        description="Return whether a pairing nonce is present and used.",
        parameters=(ParamSpec("nonce", "query"),),
        responses=(
            ResponseSpec(
                status=200,
                description="Pair nonce status.",
                named_fields=(
                    FieldSpec("present", "boolean", required=True),
                    FieldSpec("used", "boolean", required=True),
                ),
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.pair",
        method="POST",
        rule="/app/network/pair",
        summary="Complete link pairing",
        description=(
            "Accept a client CSR plus nonce, then return a signed certificate "
            "and home attestation."
        ),
        parameters=(
            ParamSpec(
                "token",
                "query",
                required=False,
                description="Pairing nonce; the body nonce can be used instead.",
            ),
        ),
        request=RequestSpec(
            fields=(
                FieldSpec("csr", "string", required=True),
                FieldSpec("nonce", "string"),
                FieldSpec("device_label", "string"),
                FieldSpec("sender_instance_id", "string"),
            ),
            example={
                "csr": "-----BEGIN CERTIFICATE REQUEST-----\n...\n-----END CERTIFICATE REQUEST-----\n",
                "nonce": "5f0d8c8b9f1e48b0a5f80b98f3d5e9b0",
                "device_label": "Jer iPhone",
                "sender_instance_id": "ios-01",
            },
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="Signed link material for the paired client.",
                named_fields=(
                    FieldSpec("client_cert", "string", required=True),
                    FieldSpec(
                        "ca_chain",
                        "array",
                        required=True,
                        item_type="string",
                    ),
                    FieldSpec("instance_id", "string", required=True),
                    FieldSpec("home_label", "string", required=True),
                    FieldSpec("home_attestation", "string", required=True),
                    FieldSpec("fingerprint", "string", required=True),
                    FieldSpec(
                        "local_endpoints", "array", raw_schema=_LOCAL_ENDPOINT_SCHEMA
                    ),
                ),
                example={
                    "client_cert": "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----\n",
                    "ca_chain": [
                        "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----\n"
                    ],
                    "instance_id": "4d1f3d57-4f39-4930-b8f8-5e6f2a84d51a",
                    "home_label": "home",
                    "home_attestation": "eyJhbGciOi...",
                    "fingerprint": "sha256:abc123",
                    "local_endpoints": [
                        {"ip": "192.168.1.10", "port": 7657, "scope": "lan"}
                    ],
                },
            ),
            _json_error(
                400,
                (
                    "missing_required_field",
                    "pairing_key_invalid",
                    "pairing_request_invalid",
                ),
                "Pair request rejected by handler validation.",
            ),
            _json_error(
                403,
                ("pl_revoked",),
                "Access gate rejected a revoked paired-link identity.",
            ),
            _json_error(
                410,
                ("operation_no_longer_available",),
                "Nonce expired or was already used.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.unpair",
        method="POST",
        rule="/app/network/unpair",
        summary="Unpair a device",
        description="Remove a paired client by fingerprint or device label.",
        request=RequestSpec(
            fields=(
                FieldSpec("fingerprint", "string"),
                FieldSpec("device_label", "string"),
            ),
            example={"fingerprint": "sha256:abc123"},
        ),
        responses=(
            ResponseSpec(
                status=200,
                description="The revoked fingerprint and any observer records revoked with it.",
                named_fields=(
                    FieldSpec("unpaired", "string", required=True),
                    FieldSpec(
                        "revoked_observers",
                        "array",
                        required=True,
                        raw_schema=_REVOKED_OBSERVER_SCHEMA,
                    ),
                ),
                example={
                    "unpaired": "sha256:abc123",
                    "revoked_observers": [
                        {"name": "phone-a", "prefix": "phone-a-"},
                    ],
                },
            ),
            _json_error(
                400,
                ("invalid_operation_for_state", "missing_required_field"),
                "The unpair request was incomplete or ambiguous.",
            ),
            _json_error(
                403,
                ("pl_revoked",),
                "Access gate rejected a revoked paired-link identity.",
            ),
            _json_error(
                404,
                ("paired_device_not_found",),
                "No paired device matched the request.",
            ),
            _json_error(
                500,
                ("internal_error",),
                "Observer revoke cascade failed after the device was unpaired.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.localEndpoints",
        method="GET",
        rule="/app/network/local-endpoints",
        summary="List local link endpoints",
        description="Return loopback-only LAN endpoint hints for link clients.",
        responses=(
            ResponseSpec(
                status=200,
                description="Current local endpoint advertisement.",
                named_fields=(
                    FieldSpec("v", "integer", required=True),
                    FieldSpec(
                        "endpoints",
                        "array",
                        required=True,
                        raw_schema=_LOCAL_ENDPOINT_SCHEMA,
                    ),
                    FieldSpec("ttl_s", "integer", required=True),
                    FieldSpec("generated_at", "string", required=True),
                ),
                example={
                    "v": 1,
                    "endpoints": [{"ip": "192.168.1.10", "port": 7657, "scope": "lan"}],
                    "ttl_s": 3600,
                    "generated_at": "2026-06-18T12:00:00Z",
                },
            ),
            _json_error(
                403,
                ("pl_revoked",),
                "Access gate rejected a revoked paired-link identity.",
            ),
            ResponseSpec(
                status=404,
                description="Non-loopback request; bare Flask abort, no reason body.",
            ),
        ),
    ),
    OperationSpec(
        operation_id="link.status",
        method="GET",
        rule="/app/network/api/status",
        summary="Read link status",
        description="Return the current link service posture and reachability view.",
        responses=(
            ResponseSpec(
                status=200,
                description="Link status snapshot.",
                named_fields=(
                    FieldSpec(
                        "instance_id",
                        "string",
                        required=True,
                        raw_schema={"type": ["string", "null"]},
                    ),
                    FieldSpec(
                        "home_label",
                        "string",
                        required=True,
                        raw_schema={"type": ["string", "null"]},
                    ),
                    FieldSpec("enrolled", "boolean", required=True),
                    FieldSpec("relay_url", "string", required=True),
                    FieldSpec(
                        "ca_fingerprint",
                        "string",
                        required=True,
                        raw_schema={"type": ["string", "null"]},
                    ),
                    FieldSpec("lan_accessible", "boolean", required=True),
                    FieldSpec("posture", "string", required=True),
                    FieldSpec("reachability", "string", required=True),
                    FieldSpec("relay_state", "string", required=True),
                    FieldSpec(
                        "last_link_event_at",
                        "integer",
                        required=True,
                        raw_schema={"type": ["integer", "null"]},
                    ),
                    FieldSpec(
                        "relay_listen_generation",
                        "integer",
                        required=True,
                        raw_schema={"type": ["integer", "null"]},
                    ),
                    FieldSpec(
                        "last_successful_relay_tunnel_at",
                        "integer",
                        required=True,
                        raw_schema={"type": ["integer", "null"]},
                    ),
                    FieldSpec(
                        "last_relay_tunnel_error",
                        "string",
                        required=True,
                        raw_schema={"type": ["string", "null"]},
                    ),
                    FieldSpec(
                        "last_relay_tunnel_error_at",
                        "integer",
                        required=True,
                        raw_schema={"type": ["integer", "null"]},
                    ),
                    FieldSpec(
                        "last_relay_listener_ack_at",
                        "integer",
                        required=True,
                        raw_schema={"type": ["integer", "null"]},
                    ),
                    FieldSpec(
                        "last_relay_listener_ack_generation",
                        "integer",
                        required=True,
                        raw_schema={"type": ["integer", "null"]},
                    ),
                    FieldSpec(
                        "home_address",
                        "string",
                        required=True,
                        raw_schema={"type": ["string", "null"]},
                    ),
                    FieldSpec(
                        "vpn",
                        "object",
                        required=True,
                        raw_schema={
                            "type": "object",
                            "additionalProperties": True,
                            "properties": {
                                "active": {"type": ["string", "null"]},
                                "candidates": {
                                    "type": "array",
                                    "items": {"type": "object"},
                                },
                            },
                        },
                    ),
                    FieldSpec(
                        "home_candidates",
                        "array",
                        required=True,
                        raw_schema={
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": False,
                                "properties": {
                                    "address": {"type": "string"},
                                    "selected": {"type": "boolean"},
                                    "source": {
                                        "type": "string",
                                        "enum": ["detected", "override"],
                                    },
                                },
                                "required": ["address", "selected", "source"],
                            },
                        },
                    ),
                    FieldSpec(
                        "home_candidates_state",
                        "string",
                        required=True,
                        raw_schema={
                            "type": "string",
                            "enum": ["ready", "unavailable"],
                        },
                    ),
                    FieldSpec(
                        "home_candidates_error",
                        "string",
                        required=True,
                        raw_schema={"type": ["string", "null"]},
                    ),
                ),
                example={
                    "instance_id": "4d1f3d57-4f39-4930-b8f8-5e6f2a84d51a",
                    "home_label": "home",
                    "enrolled": True,
                    "relay_url": "https://relay.solstone.local",
                    "ca_fingerprint": "9c5f2e0c8e6a42f0a32e55e5cf7f5b4a",
                    "lan_accessible": True,
                    "posture": "lan",
                    "reachability": "local",
                    "relay_state": "not_configured",
                    "last_link_event_at": None,
                    "relay_listen_generation": None,
                    "last_successful_relay_tunnel_at": None,
                    "last_relay_tunnel_error": None,
                    "last_relay_tunnel_error_at": None,
                    "last_relay_listener_ack_at": None,
                    "last_relay_listener_ack_generation": None,
                    "home_address": None,
                    "vpn": {"active": None, "candidates": []},
                    "home_candidates": [],
                    "home_candidates_state": "ready",
                    "home_candidates_error": None,
                },
            ),
            _json_error(
                403,
                ("pl_revoked",),
                "Access gate rejected a revoked paired-link identity.",
            ),
        ),
    ),
]

__all__ = ["OPERATIONS"]
