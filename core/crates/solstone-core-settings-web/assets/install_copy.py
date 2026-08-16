# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

STT_LOCAL_REQUIREMENTS_TEMPLATE = (
    "local transcription needs about {ram_gb} GB of free memory for the on-device "
    "model (transcription, speaker labels, and overlap detection)."
)
STT_LOCAL_UNSUPPORTED = "local transcription is not available on this platform."
STT_DETECTED_MEMORY_TEMPLATE = (
    "{available_gb} GB of free memory detected on this machine."
)
STT_DETECTED_MEMORY_UNKNOWN = "free memory on this machine could not be detected."
STT_NO_LOCAL_STT_RECOVERY = (
    "free up memory on this machine or use a supported platform to transcribe locally. "
    "with confidential processing enabled, transcription runs on the service instead."
)
STT_EXPLICIT_LOCAL_LOW_TEMPLATE = (
    "free memory is below {ram_gb} GB. local transcription can still run, but this "
    "machine may be slow or unstable while it does."
)


__all__ = [
    "STT_LOCAL_REQUIREMENTS_TEMPLATE",
    "STT_LOCAL_UNSUPPORTED",
    "STT_DETECTED_MEMORY_TEMPLATE",
    "STT_DETECTED_MEMORY_UNKNOWN",
    "STT_NO_LOCAL_STT_RECOVERY",
    "STT_EXPLICIT_LOCAL_LOW_TEMPLATE",
]
