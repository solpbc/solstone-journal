# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import dataclasses
import importlib.util

import pytest

from solstone.think import features as features_module
from solstone.think.features import (
    FEATURES,
    Feature,
    MissingExtraError,
    install_hint,
    is_available,
    require_extra,
)


def test_feature_is_frozen():
    feature = Feature(
        name="test",
        summary="x",
        pip_modules=("sys",),
        apt_packages=(),
        brew_packages=(),
        usage="x",
    )

    with pytest.raises(dataclasses.FrozenInstanceError):
        feature.name = "changed"


def test_features_registry_contents():
    assert "pdf" in FEATURES
    assert "whisper" in FEATURES

    for feature in FEATURES.values():
        assert isinstance(feature.name, str)
        assert isinstance(feature.summary, str)
        assert isinstance(feature.pip_modules, tuple)
        assert isinstance(feature.apt_packages, tuple)
        assert isinstance(feature.brew_packages, tuple)
        assert isinstance(feature.usage, str)
        assert feature.pip_modules


def test_is_available_true_for_pdf():
    assert is_available("pdf") is True


def test_is_available_reflects_whisper_installation():
    assert is_available("whisper") is (
        importlib.util.find_spec("faster_whisper") is not None
    )


def test_is_available_false_for_missing_module(monkeypatch):
    monkeypatch.setattr(
        features_module,
        "FEATURES",
        {
            "fake": Feature(
                name="fake",
                summary="x",
                pip_modules=("definitely_not_installed_xyz",),
                apt_packages=(),
                brew_packages=(),
                usage="x",
            )
        },
    )

    assert is_available("fake") is False


def test_is_available_unknown_raises_keyerror():
    with pytest.raises(KeyError):
        is_available("nonexistent")


def test_install_hint_pdf_linux():
    assert (
        install_hint("pdf", "linux")
        == "pip install 'solstone[pdf]' and apt install libpango-1.0-0 libpangoft2-1.0-0 poppler-utils"
    )


def test_install_hint_pdf_darwin():
    assert (
        install_hint("pdf", "darwin")
        == "pip install 'solstone[pdf]' and brew install pango poppler"
    )


def test_install_hint_whisper_linux():
    assert install_hint("whisper", "linux") == "pip install 'solstone[whisper]'"


def test_install_hint_whisper_darwin():
    assert install_hint("whisper", "darwin") == "pip install 'solstone[whisper]'"


def test_install_hint_unknown_raises_keyerror():
    with pytest.raises(KeyError):
        install_hint("nonexistent", "linux")


def test_missing_extra_error_message():
    err = MissingExtraError("pdf", "linux")

    assert (
        err.args[0]
        == "feature 'pdf' requires the [pdf] extra: pip install 'solstone[pdf]' and apt install libpango-1.0-0 libpangoft2-1.0-0 poppler-utils"
    )


def test_missing_extra_error_name_attribute():
    assert MissingExtraError("pdf", "linux").name == "pdf"


def test_require_extra_succeeds_when_available():
    require_extra("pdf")


def test_require_extra_raises_when_missing(monkeypatch):
    monkeypatch.setattr(
        features_module,
        "FEATURES",
        {
            "fake": Feature(
                name="fake",
                summary="x",
                pip_modules=("definitely_not_installed_xyz",),
                apt_packages=(),
                brew_packages=(),
                usage="x",
            )
        },
    )

    with pytest.raises(MissingExtraError) as error:
        require_extra("fake")

    assert "pip install 'solstone[fake]'" in error.value.args[0]
