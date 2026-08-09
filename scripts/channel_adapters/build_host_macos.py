#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""External macOS build-host adapter for the release rail."""

from __future__ import annotations

import re
import shlex
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.channel_adapters.adapter_common import (  # noqa: E402
    LaneConfig,
    die,
    load_config,
    read_json,
    require_success_token,
    scp_from,
    scp_to,
    sha256_size,
    ssh_run,
    verify_retrieved_file,
    write_json,
)
from scripts.check_release_preflight import (  # noqa: E402
    expected_presign_lane_tool_evidence,
)
from scripts.release_public_evidence import validate_public_evidence_tree  # noqa: E402
from scripts.release_target_policy import TARGET_ENV_KEYS  # noqa: E402
from scripts.release_tool_pins import (  # noqa: E402
    HOST_VARIANT_TOOL_KEYS,
    parse_host_variant_tool_banner,
    tool_value_matches_pin,
)

TOOLCHAIN_TOKEN = "TOOLCHAIN_OK"
CHECKOUT_TOKEN = "CHECKOUT_OK"
DIST_TOKEN = "DIST_OK"
ARTIFACT_TOKEN = "ARTIFACT"
IPV4_SHAPE_RE = re.compile(r"^\d{1,3}(?:\.\d{1,3}){3}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def _remote_work(lane: LaneConfig, cohort_id: str) -> str:
    return f"{lane.remote_work_prefix}-build-{cohort_id}"


def _failure_detail(failures: object) -> str:
    lines: list[str] = []
    for failure in failures:  # type: ignore[union-attr]
        lines.append(
            f"{failure.error}; expected {failure.expected}; "
            f"actual {failure.actual}; repair {failure.repair}"
        )
    return "\n".join(lines)


def _stream_detail(stderr: str, stdout: str) -> str:
    sections: list[str] = []
    normalized_stderr = stderr.strip()
    normalized_stdout = stdout.strip()
    if normalized_stderr:
        sections.append(f"stderr:\n{normalized_stderr}")
    if normalized_stdout:
        sections.append(f"stdout:\n{normalized_stdout}")
    return "\n".join(sections)


def _parse_observed_tool_lines(stdout: str) -> dict[str, str]:
    observed: dict[str, str] = {}
    for line in stdout.splitlines():
        if "\t" not in line:
            continue
        key, value = line.split("\t", 1)
        observed[key] = value
    return observed


def _parse_artifact_listing(
    stdout: str,
    expected_files: list[str],
) -> dict[str, tuple[str, int]]:
    expected = set(expected_files)
    artifacts: dict[str, tuple[str, int]] = {}
    for line in stdout.splitlines():
        if not line.startswith(f"{ARTIFACT_TOKEN}\t"):
            continue
        parts = line.split("\t")
        if len(parts) != 4:
            die(
                "macOS artifact listing reported malformed artifact digest line",
                detail=line,
            )
        _token, name, sha256, bytes_text = parts
        if name in artifacts:
            die(
                "macOS artifact listing reported duplicate artifact digest", detail=name
            )
        if name not in expected:
            die("macOS artifact listing reported unexpected artifact", detail=name)
        try:
            byte_count = int(bytes_text)
        except ValueError:
            die(
                f"macOS artifact listing reported invalid digest/size for {name}",
                detail=bytes_text,
            )
        if not SHA256_RE.fullmatch(sha256) or byte_count < 0:
            die(
                f"macOS artifact listing reported invalid digest/size for {name}",
                detail=f"{sha256}/{bytes_text}",
            )
        artifacts[name] = (sha256, byte_count)
    for name in expected_files:
        if name not in artifacts:
            die(f"macOS artifact listing did not report digest/size for {name}")
    return artifacts


def _validate_host_variant_public_shape(tool: str, banner: str) -> None:
    if parse_host_variant_tool_banner(tool, banner) is None:
        die(f"{tool} host-variant banner does not match the rail parser")
    if "/" in banner or "\\" in banner or "@" in banner or ".local" in banner:
        die(f"{tool} host-variant banner contains private host/path syntax")
    match = re.search(r"\((?P<inner>[^()]*)\)", banner)
    if match and IPV4_SHAPE_RE.fullmatch(match["inner"]):
        die(f"{tool} host-variant banner contains an IP-shaped host component")


def _derive_tool_evidence(build_lane: LaneConfig) -> dict[str, str]:
    script = r"""
set -euo pipefail
source ~/.cargo/env 2>/dev/null || true
emit() { printf '%s\t%s\n' "$1" "$2"; }
python_out="$(python3.14 --version 2>&1)"
emit python "${python_out#Python }"
emit rustc "$(rustc --version 2>&1)"
emit cargo "$(cargo --version 2>&1)"
emit uv "$(uv --version 2>&1)"
emit maturin "$(maturin --version 2>&1)"
emit cargo-deny "$(cargo-deny --version 2>&1)"
xcodebuild_out="$(xcodebuild -version 2>&1)"
xcode_version="$(awk '/^Xcode / { print $2 }' <<<"$xcodebuild_out")"
xcode_build="$(awk '/^Build version / { print $3 }' <<<"$xcodebuild_out")"
emit xcode "xcode ${xcode_version} build ${xcode_build}"
swift_out="$(swift --version 2>&1)"
swift_banner="$(printf '%s' "$swift_out" | tr '\n' ' ' | sed 's/[[:space:]]*$//')"
emit swift "$swift_banner"
[ -x /usr/bin/codesign ]
emit codesign "codesign pinned-path verified"
emit notarytool "$(xcrun notarytool --version 2>&1)"
echo TOOLCHAIN_OK
"""
    result = ssh_run(build_lane, script, check=False)
    require_success_token(result, TOOLCHAIN_TOKEN, "macOS toolchain verification")
    observed = _parse_observed_tool_lines(result.stdout or "")
    expected = expected_presign_lane_tool_evidence("macos-arm64")
    missing = sorted(set(expected) - set(observed))
    if missing:
        die("macOS toolchain evidence is incomplete", detail=", ".join(missing))
    evidence: dict[str, str] = {}
    for key, expected_value in expected.items():
        observed_value = observed[key]
        if not tool_value_matches_pin(key, expected_value, observed_value):
            die(
                f"macOS tool {key} does not match release pin",
                detail=f"observed {observed_value!r}",
            )
        if key in HOST_VARIANT_TOOL_KEYS:
            _validate_host_variant_public_shape(key, observed_value)
            evidence[key] = observed_value
        else:
            evidence[key] = expected_value
    public_failures = validate_public_evidence_tree(
        "build_host.tool_evidence",
        evidence,
    )
    if public_failures:
        die(
            "macOS tool evidence contains non-public text",
            detail=_failure_detail(public_failures),
        )
    return evidence


def build_macos(build_lane: LaneConfig, request_path: Path) -> None:
    req = read_json(request_path)
    cohort = req["cohort_id"]
    commit = req["expected_commit"]
    sb = req["source_bundle"]
    outputs = req["expected_outputs"]
    macos_wheels = outputs["macos_wheels"]
    native_records = outputs["native_records"]
    if not isinstance(macos_wheels, list) or not all(
        isinstance(name, str) for name in macos_wheels
    ):
        die("macOS expected wheel inventory is invalid")
    if not isinstance(native_records, list) or not all(
        isinstance(name, str) for name in native_records
    ):
        die("macOS expected native record inventory is invalid")

    bundle = Path(sb["path"])
    out_dir = Path(req["paths"]["output_dir"])
    resp_path = Path(req["paths"]["response"])

    local_sha, local_bytes = sha256_size(bundle)
    if local_sha != sb["sha256"] or local_bytes != sb["bytes"]:
        die(
            "local source bundle digest/size mismatch",
            detail=(
                f"expected {sb['sha256']}/{sb['bytes']} got {local_sha}/{local_bytes}"
            ),
        )

    work = _remote_work(build_lane, cohort)
    src = f"{work}/src"
    quoted_work = shlex.quote(work)
    quoted_src = shlex.quote(src)
    quoted_commit = shlex.quote(commit)

    ssh_run(build_lane, f"set -e; rm -rf {quoted_work}; mkdir -p {quoted_work}")
    scp_to(build_lane, bundle, f"{work}/source.bundle")
    checkout = ssh_run(
        build_lane,
        f"""
set -euo pipefail
git clone --quiet {quoted_work}/source.bundle {quoted_src}
cd {quoted_src}
git checkout --quiet {quoted_commit}
HEAD=$(git rev-parse HEAD)
[ "$HEAD" = {quoted_commit} ] || {{ echo "HEAD $HEAD != {commit}" >&2; exit 4; }}
[ -z "$(git status --porcelain)" ] || {{ echo "tree not clean" >&2; exit 5; }}
echo CHECKOUT_OK
""",
        check=False,
    )
    require_success_token(checkout, CHECKOUT_TOKEN, "macOS checkout")

    tool_evidence = _derive_tool_evidence(build_lane)

    for key in ("remote_run_wrapper", "tmux_window", "unlock_workdir"):
        if getattr(build_lane, key) is None:
            die(f"build lane signing-session config is missing {key}")
    assert build_lane.remote_run_wrapper is not None
    assert build_lane.tmux_window is not None
    assert build_lane.unlock_workdir is not None
    quoted_remote_run_wrapper = shlex.quote(build_lane.remote_run_wrapper)
    unlock = ssh_run(
        build_lane,
        f"{quoted_remote_run_wrapper} "
        f"{shlex.quote(build_lane.tmux_window)} "
        f"{shlex.quote(build_lane.unlock_workdir)} "
        "'make unlock-signing'",
        check=False,
    )
    if unlock.returncode != 0:
        die(
            "make unlock-signing failed on macOS build host",
            detail=_stream_detail(unlock.stderr, unlock.stdout),
        )

    build = ssh_run(
        build_lane,
        f"{quoted_remote_run_wrapper} "
        f"{shlex.quote(build_lane.tmux_window)} "
        f"{quoted_src} "
        f"'set -e; mkdir -p {shlex.quote(work)}/pyshim; "
        f'ln -sf "$(command -v python3.14)" {shlex.quote(work)}/pyshim/python3; '
        f"PATH={shlex.quote(work)}/pyshim:$PATH PYTHONPATH={quoted_src} "
        "make wheel-macos'",
        check=False,
    )
    if build.returncode != 0:
        die(
            "make wheel-macos failed on macOS build host",
            detail=_stream_detail(build.stderr, build.stdout),
        )

    expected_files = [*macos_wheels, *native_records]
    quoted_expected_files = " ".join(shlex.quote(name) for name in expected_files)
    listing = ssh_run(
        build_lane,
        f"""
set -euo pipefail
cd {quoted_src}/dist
for f in {quoted_expected_files}; do
  [ -f "$f" ] || {{ echo "missing $f" >&2; exit 6; }}
  sha256="$(shasum -a 256 "$f" | awk '{{print $1}}')"
  bytes="$(stat -f%z "$f")"
  printf 'ARTIFACT\\t%s\\t%s\\t%s\\n' "$f" "$sha256" "$bytes"
done
echo DIST_OK
""",
        check=False,
    )
    require_success_token(listing, DIST_TOKEN, "macOS artifact listing")
    artifact_listing = _parse_artifact_listing(listing.stdout or "", expected_files)

    out_dir.mkdir(parents=True, exist_ok=True)
    for name in expected_files:
        scp_from(build_lane, f"{src}/dist/{name}", out_dir / name)
        expected_sha256, expected_bytes = artifact_listing[name]
        verify_retrieved_file(
            out_dir / name,
            expected_sha256=expected_sha256,
            expected_bytes=expected_bytes,
            label=name,
        )

    response = {
        "schema_version": 1,
        "cohort_id": cohort,
        "attestation": {
            "source_commit": commit,
            "clean_tree": True,
            "bundle_sha256": sb["sha256"],
            "bundle_bytes": sb["bytes"],
        },
        "tool_evidence": tool_evidence,
        "macos_wheels": macos_wheels,
        "native_records": native_records,
    }
    write_json(resp_path, response)


def cleanup(build_lane: LaneConfig, cohort_id: str, _bundle_sha256: str) -> None:
    ssh_run(
        build_lane,
        f"rm -rf {shlex.quote(_remote_work(build_lane, cohort_id))}",
        check=False,
    )


def main(argv: list[str]) -> int:
    build_lane, _proof_lanes = load_config(proof_targets=tuple(TARGET_ENV_KEYS))
    if not argv:
        die("no subcommand (expected build-macos|cleanup)")
    sub = argv[0]
    if sub == "build-macos":
        if len(argv) != 2:
            die("build-macos requires a request file path")
        build_macos(build_lane, Path(argv[1]))
        return 0
    if sub == "cleanup":
        if len(argv) != 3:
            die("cleanup requires <cohort_id> <bundle_sha256>")
        cleanup(build_lane, argv[1], argv[2])
        return 0
    die(f"unknown subcommand: {sub}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
