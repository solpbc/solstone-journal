# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import importlib
import json
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from solstone.convey import create_app
from solstone.think import journal_config
from tests.helpers.journal_config import seed_journal_config


class _FixedDateTime(datetime):
    @classmethod
    def now(cls, tz=None):
        return datetime(2026, 7, 19, 12, 0, tzinfo=tz or timezone.utc)


@pytest.fixture(autouse=True)
def use_built_core(monkeypatch: pytest.MonkeyPatch) -> None:
    helper = (
        Path(__file__).resolve().parents[1]
        / "core"
        / "target"
        / "debug"
        / "solstone-core"
    )
    monkeypatch.setattr(
        journal_config.core_handshake,
        "check_solstone_core_handshake",
        lambda: journal_config.core_handshake.CoreHandshakeResult("ok"),
    )
    monkeypatch.setattr(
        journal_config.core_handshake,
        "helper_path_for_executable",
        lambda: helper,
    )


def _base_config(**updates: Any) -> dict[str, Any]:
    config: dict[str, Any] = {
        "setup": {"completed_at": 1700000000000},
        "identity": {"name": "Before", "preferred": "Before"},
        "journal": {"name": "Before"},
        "env": {},
        "providers": {
            "active": {"provider": "google", "model": "gemini-3.5-flash"},
            "key_validation": {},
            "local": {},
        },
        "retention": {"raw_media": "keep", "raw_media_days": None},
    }
    config.update(updates)
    return config


def _spy_changed(monkeypatch: pytest.MonkeyPatch, module: Any) -> list[bool]:
    real_mutate = module.mutate_journal_config
    changes: list[bool] = []

    def wrapped(mutator, *args, **kwargs):
        result = real_mutate(mutator, *args, **kwargs)
        changes.append(result.changed)
        return result

    monkeypatch.setattr(module, "mutate_journal_config", wrapped)
    return changes


def _client(journal: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    monkeypatch.setenv("SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES", "1")
    app = create_app(str(journal))
    app.config["TESTING"] = True
    return app.test_client()


@dataclass(frozen=True)
class RouteCase:
    path_id: str
    module_name: str
    method: str
    path: str
    payload: dict[str, Any] | None
    changed_seed: dict[str, Any]
    noop_seed: dict[str, Any]
    patch: Callable[[pytest.MonkeyPatch, Any], None] | None = None


def _patch_noop_action(monkeypatch: pytest.MonkeyPatch, module: Any) -> None:
    if hasattr(module, "log_app_action"):
        monkeypatch.setattr(module, "log_app_action", lambda **_kwargs: None)




def _patch_thinking_key_validation(
    monkeypatch: pytest.MonkeyPatch, module: Any
) -> None:
    _patch_noop_action(monkeypatch, module)
    monkeypatch.setattr(module, "datetime", _FixedDateTime)
    monkeypatch.setattr(
        module,
        "validate_key",
        lambda provider, key: {"valid": True, "provider": provider, "key": key},
    )




def _patch_thinking_validate_all(monkeypatch: pytest.MonkeyPatch, module: Any) -> None:
    monkeypatch.setattr(
        module,
        "_compute_ai_key_validation",
        lambda _config: {"google": {"valid": True}},
    )


ROUTE_CASES: tuple[RouteCase, ...] = (
    RouteCase(
        "thinking.keys.set",
        "solstone.apps.thinking.routes",
        "put",
        "/app/thinking/api/keys",
        {"env_var": "GOOGLE_API_KEY", "value": "google-key"},
        _base_config(env={}),
        _base_config(
            env={"GOOGLE_API_KEY": "google-key"},
            providers={
                "active": {"provider": "google", "model": "gemini-3.5-flash"},
                "key_validation": {
                    "google": {
                        "valid": True,
                        "provider": "google",
                        "key": "google-key",
                        "timestamp": "2026-07-19T12:00:00+00:00",
                    }
                },
                "local": {},
            },
        ),
        _patch_thinking_key_validation,
    ),
    RouteCase(
        "thinking.keys.clear",
        "solstone.apps.thinking.routes",
        "put",
        "/app/thinking/api/keys",
        {"env_var": "GOOGLE_API_KEY", "value": ""},
        _base_config(
            env={"GOOGLE_API_KEY": "google-key"},
            providers={
                "active": {"provider": "google", "model": "gemini-3.5-flash"},
                "key_validation": {"google": {"valid": True}},
                "local": {},
            },
        ),
        _base_config(
            env={},
            providers={
                "active": {"provider": "google", "model": "gemini-3.5-flash"},
                "key_validation": {},
                "local": {},
            },
        ),
        _patch_thinking_key_validation,
    ),
    RouteCase(
        "thinking.validate_all_keys",
        "solstone.apps.thinking.routes",
        "post",
        "/app/thinking/api/validate-keys",
        None,
        _base_config(env={"GOOGLE_API_KEY": "google-key"}),
        _base_config(
            env={"GOOGLE_API_KEY": "google-key"},
            providers={
                "active": {"provider": "google", "model": "gemini-3.5-flash"},
                "key_validation": {"google": {"valid": True}},
                "local": {},
            },
        ),
        _patch_thinking_validate_all,
    ),
    RouteCase(
        "thinking.update_local_endpoint",
        "solstone.apps.thinking.routes",
        "post",
        "/app/thinking/api/local/endpoint",
        {
            "endpoint_url": "https://local.example/v1",
            "served_model_id": "model",
            "credential": "secret",
        },
        _base_config(),
        _base_config(
            providers={
                "active": {"provider": "google", "model": "gemini-3.5-flash"},
                "key_validation": {},
                "local": {
                    "endpoint_url": "https://local.example",
                    "served_model_id": "model",
                    "credential": "secret",
                },
            }
        ),
    ),
    RouteCase(
        "thinking.clear_local_endpoint",
        "solstone.apps.thinking.routes",
        "delete",
        "/app/thinking/api/local/endpoint",
        None,
        _base_config(
            providers={
                "active": {"provider": "google", "model": "gemini-3.5-flash"},
                "key_validation": {},
                "local": {
                    "endpoint_url": "https://local.example",
                    "served_model_id": "model",
                    "credential": "secret",
                },
            }
        ),
        _base_config(
            providers={
                "active": {"provider": "google", "model": "gemini-3.5-flash"},
                "key_validation": {},
                "local": {},
            }
        ),
    ),
    RouteCase(
        "thinking.update_providers",
        "solstone.apps.thinking.routes",
        "put",
        "/app/thinking/api/providers",
        {"lane": "byo", "provider": "openai"},
        _base_config(),
        _base_config(
            providers={
                "active": {"provider": "openai", "model": "gpt-4o"},
                "key_validation": {},
                "local": {},
            }
        ),
    ),
    RouteCase(
        "thinking.update_generators",
        "solstone.apps.thinking.routes",
        "put",
        "/app/thinking/api/generators",
        {"summary": {"disabled": True}},
        _base_config(),
        _base_config(talent_overrides={"talent.system.summary": {"disabled": True}}),
    ),
    RouteCase(
        "sol.api_set_name",
        "solstone.apps.sol.routes",
        "post",
        "/app/sol/api/set-name",
        {"name": "helper", "status": "chosen"},
        _base_config(
            agent={"name": "sol", "name_status": "default", "named_date": None}
        ),
        _base_config(
            agent={
                "name": "helper",
                "name_status": "chosen",
                "named_date": "2026-07-19",
            }
        ),
    ),
    RouteCase(
        "sol.api_reset",
        "solstone.apps.sol.routes",
        "post",
        "/app/sol/api/reset",
        None,
        _base_config(
            agent={
                "name": "helper",
                "name_status": "chosen",
                "named_date": "2026-07-19",
            }
        ),
        _base_config(
            agent={"name": "sol", "name_status": "default", "named_date": None}
        ),
    ),
    RouteCase(
        "sol.api_set_owner",
        "solstone.apps.sol.routes",
        "post",
        "/app/sol/api/set-owner",
        {"name": "Owner", "bio": "Bio"},
        _base_config(identity={"name": "Before"}),
        _base_config(identity={"name": "Owner", "bio": "Bio"}),
    ),
)


@pytest.mark.parametrize("case", ROUTE_CASES, ids=lambda case: case.path_id)
def test_route_mutation_paths_report_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, case: RouteCase
) -> None:
    module = importlib.import_module(case.module_name)
    _patch_noop_action(monkeypatch, module)
    if case.patch is not None:
        case.patch(monkeypatch, module)
    if case.module_name == "solstone.apps.sol.routes":
        monkeypatch.setattr(module, "datetime", _FixedDateTime)
    changes = _spy_changed(monkeypatch, module)
    client = _client(tmp_path, monkeypatch)

    seed_journal_config(case.changed_seed, tmp_path)
    response = getattr(client, case.method)(case.path, json=case.payload)
    assert response.status_code == 200
    assert changes[-1] is True

    seed_journal_config(case.noop_seed, tmp_path)
    response = getattr(client, case.method)(case.path, json=case.payload)
    assert response.status_code == 200
    assert changes[-1] is False


def test_tools_retention_mutation_paths_report_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = importlib.import_module("solstone.think.tools.call")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(module, "log_call_action", lambda **_kwargs: None)
    changes = _spy_changed(monkeypatch, module)

    cases = [
        (
            "clear",
            {
                "retention": {
                    "per_stream": {
                        "desktop": {"raw_media": "days", "raw_media_days": 7}
                    }
                }
            },
            {"retention": {}},
            lambda: module.config(mode=None, days=None, stream="desktop", clear=True),
        ),
        (
            "stream",
            {"retention": {}},
            {
                "retention": {
                    "per_stream": {
                        "desktop": {"raw_media": "days", "raw_media_days": 7}
                    }
                }
            },
            lambda: module.config(mode="days", days=7, stream="desktop", clear=False),
        ),
        (
            "default",
            {"retention": {"raw_media": "keep", "raw_media_days": None}},
            {"retention": {"raw_media": "processed", "raw_media_days": None}},
            lambda: module.config(
                mode="processed", days=None, stream=None, clear=False
            ),
        ),
    ]
    for _path_id, changed_seed, noop_seed, invoke in cases:
        seed_journal_config(changed_seed, tmp_path)
        invoke()
        assert changes[-1] is True
        seed_journal_config(noop_seed, tmp_path)
        invoke()
        assert changes[-1] is False


def test_backup_state_mutation_paths_report_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from solstone.think.backup import state
    from solstone.think.backup.destination import Destination

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(state, "generate_daily_key", lambda: "D" * 64)
    monkeypatch.setattr(state, "generate_recovery_key", lambda: "R" * 64)
    changes = _spy_changed(monkeypatch, state)
    destination = Destination("repo", "local", {"k": "v"})
    retention = {"hourly": 1, "daily": 2, "weekly": 3, "monthly": 4}
    offload = {"enabled": True, "budget_bytes": 10, "floor_bytes": 5}
    cases: list[Callable[[], None]] = [
        lambda: state.generate_and_store_keys(),
        lambda: state.set_destination(destination),
        lambda: state.set_enabled(True),
        lambda: state.set_mode("operated"),
        lambda: state.set_recovery_key_confirmed(True),
        lambda: state.set_retention(retention),
        lambda: state.set_offload(offload),
        lambda: state.set_recovery_key("B" * 64),
        lambda: state.clear_backup_config(),
        lambda: state.record_backup_result(status="ok", time=1, snapshot_id="s"),
        lambda: state.record_prune_result(status="ok", time=1),
        lambda: state.record_offload_result(
            status="ok",
            time=1,
            files_marked=1,
            bytes_marked=2,
            ran_out_of_markable_media=False,
        ),
        lambda: state.record_verification_result(
            status="ok", time=1, checked_subset="all"
        ),
        lambda: state.record_restore_result(
            status="ok",
            time=1,
            reason=None,
            scope="all",
            day=None,
            segments_selected=1,
            segments_restored=1,
            files_expected=1,
            files_restored=1,
            bytes_expected=1,
            bytes_restored=1,
        ),
    ]
    for invoke in cases:
        seed_journal_config({}, tmp_path)
        invoke()
        assert changes[-1] is True
        invoke()
        assert changes[-1] is False


def test_provider_install_migration_reports_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    install_state = importlib.import_module("solstone.think.providers.install_state")
    changes = _spy_changed(monkeypatch, install_state)
    seed_journal_config(
        {
            "providers": {
                "bundled": {
                    "local": {
                        "install_state": "installed",
                        "model_id": "local/model",
                        "vulkan_device_index": "1",
                    },
                    "parakeet": {"model_repo": "repo"},
                },
                "local": {},
            }
        },
        tmp_path,
    )

    install_state.migrate_legacy_provider_install_state(journal_path=tmp_path)
    assert changes[-1] is True
    install_state.migrate_legacy_provider_install_state(journal_path=tmp_path)
    assert changes[-1] is False


def test_pairing_and_sol_voice_paths_report_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    pairing = importlib.import_module("solstone.think.pairing.config")
    sol_voice = importlib.import_module(
        "solstone.convey." + "sol" + "_initiated.settings"
    )
    cases = [
        (pairing, {"pairing": {}}, lambda: pairing.set_home_address("https://home")),
        (
            pairing,
            {"pairing": {"home_address": "https://home"}},
            pairing.clear_home_address,
        ),
        (sol_voice, {}, lambda: sol_voice.save_settings({"daily_cap": 3})),
    ]
    for module, seed, invoke in cases:
        seed_journal_config(seed, tmp_path)
        changes = _spy_changed(monkeypatch, module)
        invoke()
        assert changes[-1] is True
        invoke()
        assert changes[-1] is False


def test_service_mutation_paths_report_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    spl = importlib.import_module("solstone.think.services.spl")
    spp = importlib.import_module("solstone.think.services.spp")
    monkeypatch.setattr(spp, "_now_iso", lambda: "2026-07-19T12:00:00+00:00")
    monkeypatch.setattr(
        spl,
        "LinkState",
        SimpleNamespace(
            load_or_create=lambda: SimpleNamespace(instance_id="i", home_label="h")
        ),
    )
    monkeypatch.setattr(
        spl,
        "load_or_generate_ca",
        lambda *_args, **_kwargs: SimpleNamespace(pubkey_spki_pem="pem"),
    )
    monkeypatch.setattr(spl, "enroll_home", lambda *_args, **_kwargs: "token")
    monkeypatch.setattr(spl, "save_service_token", lambda _token: None)
    from solstone.think.models import LOCAL_MODEL

    spp_payload = {
        "account_id": "acct",
        "endpoint_url": "https://local.example/v1",
        "served_model_id": "model",
        "credential": "credential",
        "created_at": "2026-07-01T00:00:00+00:00",
    }
    spp_noop = _base_config(
        providers={
            "active": {"provider": "local", "model": LOCAL_MODEL},
            "local": {
                "endpoint_url": "https://local.example",
                "served_model_id": "model",
                "credential": "credential",
            },
        },
        services={
            "confidential": {
                "enabled_at": "2026-07-19T12:00:00+00:00",
                "account_id": "acct",
                "endpoint_url": "https://local.example",
                "served_model_id": "model",
                "credential_created_at": "2026-07-01T00:00:00+00:00",
                spp.CREDENTIAL_FINGERPRINT_FIELD: spp._fingerprint_key("credential"),
                "prior_active": {"provider": "local", "model": LOCAL_MODEL},
                "prior_local_endpoint": {
                    "endpoint_url": "https://local.example",
                    "served_model_id": "model",
                    "credential": "credential",
                },
            }
        },
    )
    cases = [
        (
            spl,
            _base_config(link={"posture": "direct"}),
            _base_config(link={"posture": "spl"}),
            spl.enable_spl,
        ),
        (
            spl,
            _base_config(link={"posture": "spl"}),
            _base_config(link={"posture": "direct"}),
            spl.disable_spl,
        ),
        (
            spp,
            _base_config(),
            spp_noop,
            lambda: spp.provision_confidential_handoff(spp_payload),
        ),
        (spp, spp_noop, _base_config(), spp.disable_confidential),
    ]
    for module, changed_seed, noop_seed, invoke in cases:
        seed_journal_config(changed_seed, tmp_path)
        changes = _spy_changed(monkeypatch, module)
        invoke()
        assert changes[-1] is True
        seed_journal_config(noop_seed, tmp_path)
        invoke()
        assert changes[-1] is False


def test_root_finalize_reports_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = importlib.import_module("solstone.convey.root")
    utils = importlib.import_module("solstone.think.utils")
    establish = importlib.import_module("solstone.think.link.establish")
    monkeypatch.setattr(establish, "is_committed", lambda: True)
    monkeypatch.setattr(utils, "now_ms", lambda: 1700000000000)
    monkeypatch.setattr(
        root, "locked_modify_convey_config", lambda *_args, **_kwargs: None
    )
    monkeypatch.setattr(root, "start_secure_listener", lambda *_args, **_kwargs: None)
    changes = _spy_changed(monkeypatch, root)
    client = _client(tmp_path, monkeypatch)
    payload = {"name": "Owner", "preferred": "owner", "retention_mode": "keep"}

    seed_journal_config(_base_config(convey={"allow_network_access": True}), tmp_path)
    response = client.post("/init/finalize", json=payload)
    assert response.status_code == 200
    assert changes[-1] is True

    seed_journal_config(
        _base_config(
            identity={"name": "Owner", "preferred": "owner"},
            setup={"completed_at": 1700000000000},
            retention={"raw_media": "keep", "raw_media_days": None},
        ),
        tmp_path,
    )
    response = client.post("/init/finalize", json=payload)
    assert response.status_code == 200
    assert changes[-1] is False


def test_maintenance_mutation_paths_report_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    observer = importlib.import_module(
        "solstone.apps.observer.maint.000_migrate_remote_to_observer"
    )
    settings = importlib.import_module(
        "solstone.apps.settings.maint.008_migrate_pairing_home_address"
    )
    thinking = importlib.import_module(
        "solstone.apps.thinking.maint.000_unify_provider_config"
    )

    cases = [
        (
            "observer",
            observer,
            _base_config(observe={"remote": {"enabled": True}}),
            _base_config(observe={}),
            lambda: observer._migrate_config(tmp_path),
        ),
        (
            "settings",
            settings,
            _base_config(pairing={"host_url": "https://home.example"}),
            _base_config(pairing={"home_address": "home.example"}),
            settings.main,
        ),
        (
            "thinking",
            thinking,
            _base_config(
                providers={
                    "google_vertex": {"project_id": "p"},
                    "key_validation": {"google_vertex": {"valid": True}},
                }
            ),
            _base_config(
                providers={"active": {"provider": "local", "model": "local/qwen3.5-4b"}}
            ),
            thinking.main,
        ),
    ]
    for path_id, module, changed_seed, noop_seed, invoke in cases:
        changes = _spy_changed(monkeypatch, module)
        seed_journal_config(changed_seed, tmp_path)
        invoke()
        capsys.readouterr()
        assert changes[-1] is True, path_id
        seed_journal_config(noop_seed, tmp_path)
        invoke()
        capsys.readouterr()
        assert changes[-1] is False, path_id


def test_setup_brain_reports_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    setup = importlib.import_module("solstone.think.setup")
    journal_config = importlib.import_module("solstone.think.journal_config")
    real_mutate = journal_config.mutate_journal_config
    changes: list[bool] = []

    def wrapped(mutator, *args, **kwargs):
        result = real_mutate(mutator, *args, **kwargs)
        changes.append(result.changed)
        return result

    monkeypatch.setattr(journal_config, "mutate_journal_config", wrapped)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    ctx = setup.SetupContext(
        mode=setup.SetupMode.NON_INTERACTIVE,
        project_root=tmp_path,
        is_source_checkout=True,
        journal_path=tmp_path,
        journal_source="test",
        config_path=tmp_path / "config" / "journal.json",
        manifest_path=tmp_path / "setup.json",
        port=5015,
        port_source="test",
        port_supplied=False,
        step_timeout_seconds=1,
        variant="cpu",
        variant_source="test",
        yes=True,
        skip_models=True,
        skip_brain=True,
        skip_skills=True,
        skip_service=True,
        skip_wrapper=True,
        accept_existing_journal=True,
        force=False,
        stdin_is_tty=False,
        stdout_is_tty=False,
        args_resolved={},
        doctor_advisories=[],
    )

    seed_journal_config(_base_config(providers={}), tmp_path)
    setup.step_brain(ctx, 1)
    assert changes[-1] is True

    from solstone.think.models import LOCAL_MODEL

    seed_journal_config(
        _base_config(providers={"active": {"provider": "local", "model": LOCAL_MODEL}}),
        tmp_path,
    )
    setup.step_brain(ctx, 1)
    assert changes[-1] is False


def test_push_reach_reports_changed_and_noop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    reach = importlib.import_module("solstone.think.push.reach")
    now = int(datetime(2026, 6, 20, 11, 30, tzinfo=timezone.utc).timestamp())
    expires_at = "2026-06-20T12:00:00Z"
    expires_epoch = int(datetime(2026, 6, 20, 12, tzinfo=timezone.utc).timestamp())
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("SERVICES_PORTAL_URL", "https://portal.test")
    monkeypatch.setattr(reach.time, "time", lambda: now)
    monkeypatch.setattr(
        reach,
        "LinkState",
        SimpleNamespace(load=lambda: SimpleNamespace(instance_id="instance")),
    )
    monkeypatch.setattr(
        reach,
        "load_or_generate_ca",
        lambda *_args, **_kwargs: SimpleNamespace(pubkey_spki_pem="pem"),
    )
    monkeypatch.setattr(reach, "mint_reach_assertion", lambda *_args, **_kwargs: "jwt")
    state = {
        "token": "reach-token",
        "expires_at": expires_at,
        "expires_epoch": expires_epoch,
        "instance_id": "instance",
    }

    class Response:
        status = 200

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def read(self):
            return json.dumps(
                {
                    "token": state["token"],
                    "token_type": "Bearer",
                    "expires_at": expires_at,
                    "expires_in": 1800,
                    "instance_id": state["instance_id"],
                }
            ).encode("utf-8")

        def getcode(self):
            return 200

    monkeypatch.setattr(
        reach.urllib_request, "urlopen", lambda *_args, **_kwargs: Response()
    )
    changes = _spy_changed(monkeypatch, reach)

    seed_journal_config({}, tmp_path)
    assert reach.ensure_reach_token() == "reach-token"
    assert changes[-1] is True

    seed_journal_config({"services": {"push": {"reach_token": state}}}, tmp_path)
    assert reach.ensure_reach_token() == "reach-token"
    assert changes[-1] is False
