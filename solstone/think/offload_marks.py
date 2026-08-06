# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Read-only view of unresolved raw-media offload release marks."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from solstone.think import retention_executor


@dataclass(frozen=True)
class OffloadMark:
    id: str
    day: str
    stream: str
    segment_dir: str
    names: tuple[str, ...]
    bytes: int


@dataclass(frozen=True)
class OffloadMarkIndex:
    _marks_by_target: dict[tuple[str, str, str], tuple[OffloadMark, ...]]

    def marks_for(self, day: str, stream: str, segment_dir: str) -> list[OffloadMark]:
        return list(self._marks_by_target.get((day, stream, segment_dir), ()))

    def matches(
        self,
        day: str,
        stream: str,
        segment_dir: str,
        sorted_names: tuple[str, ...],
    ) -> OffloadMark | None:
        return next(
            (
                mark
                for mark in self.marks_for(day, stream, segment_dir)
                if mark.names == sorted_names
            ),
            None,
        )

    @property
    def total_bytes(self) -> int:
        return sum(
            mark.bytes for marks in self._marks_by_target.values() for mark in marks
        )

    @property
    def total_files(self) -> int:
        return sum(
            len(mark.names)
            for marks in self._marks_by_target.values()
            for mark in marks
        )


def load_offload_mark_index(journal: str) -> OffloadMarkIndex:
    """Read the register once and index its unresolved offload release marks."""
    return mark_index_from_receipt(retention_executor.marks(journal))


def mark_index_from_receipt(receipt: dict[str, Any]) -> OffloadMarkIndex:
    """Build an offload-mark index from a retention executor receipt."""
    register = receipt.get("marks")
    if not isinstance(register, dict):
        return OffloadMarkIndex({})
    raw_marks = register.get("marks")
    if not isinstance(raw_marks, dict):
        return OffloadMarkIndex({})

    by_target: dict[tuple[str, str, str], list[OffloadMark]] = {}
    for raw_mark in raw_marks.values():
        if not isinstance(raw_mark, dict):
            continue
        if (
            raw_mark.get("class") != "offload_raw_release"
            or raw_mark.get("state") != "marked"
        ):
            continue
        target = raw_mark.get("target")
        proposal = raw_mark.get("proposal")
        if not isinstance(target, dict) or not isinstance(proposal, dict):
            continue
        mark_id = raw_mark.get("id")
        day = target.get("day")
        stream = target.get("stream")
        segment_dir = target.get("dir")
        names = proposal.get("names")
        bytes_count = proposal.get("bytes")
        if (
            not all(
                isinstance(value, str) for value in (mark_id, day, stream, segment_dir)
            )
            or not isinstance(names, list)
            or not all(isinstance(name, str) for name in names)
            or type(bytes_count) is not int
        ):
            continue
        mark = OffloadMark(
            id=mark_id,
            day=day,
            stream=stream,
            segment_dir=segment_dir,
            names=tuple(names),
            bytes=bytes_count,
        )
        by_target.setdefault((day, stream, segment_dir), []).append(mark)

    return OffloadMarkIndex(
        {target: tuple(marks) for target, marks in by_target.items()}
    )


__all__ = [
    "OffloadMark",
    "OffloadMarkIndex",
    "load_offload_mark_index",
    "mark_index_from_receipt",
]
