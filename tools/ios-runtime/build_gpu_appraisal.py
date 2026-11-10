#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build the pinned iOS nvattest static closure and emit its build record."""

import argparse
import hashlib
import json
import pathlib
import re
import shlex
import shutil
import subprocess
import sys
import time
from typing import Any

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

from mobile_native_build import Build, sha256


NVAT_PRODUCTION_SYMBOLS = (
    "nvat_str_length",
    "nvat_str_get_data",
    "nvat_str_free",
    "nvat_sdk_opts_create",
    "nvat_sdk_opts_free",
    "nvat_sdk_init",
    "nvat_ocsp_client_create_raw_proofs",
    "nvat_ocsp_client_free",
    "nvat_rim_store_create_filesystem",
    "nvat_rim_store_free",
    "nvat_nonce_from_bytes",
    "nvat_nonce_free",
    "nvat_gpu_evidence_source_from_json_string",
    "nvat_gpu_evidence_source_free",
    "nvat_gpu_evidence_collect",
    "nvat_gpu_evidence_array_free",
    "nvat_evidence_policy_create_default",
    "nvat_evidence_policy_set_gpu_claims_version",
    "nvat_evidence_policy_free",
    "nvat_gpu_local_verifier_create",
    "nvat_gpu_local_verifier_upcast",
    "nvat_gpu_verifier_free",
    "nvat_verify_gpu_evidence",
    "nvat_claims_collection_serialize_json",
    "nvat_claims_collection_free",
)


def _configure_argv(build: Build) -> list[str]:
    matches = [
        step["argv"]
        for step in build.summary["steps"]
        if step.get("name") == "nvattest-configure"
    ]
    if len(matches) != 1:
        raise ValueError(f"expected one nvattest configure command, got {len(matches)}")
    argv = [arg for arg in matches[0] if not arg.startswith("-DBUILD_SHARED_LIBS=")]
    return [*argv, "-DBUILD_SHARED_LIBS=OFF", "-DCMAKE_SKIP_INSTALL_RULES=ON"]


def _static_dependencies(link_command: str, build_dir: pathlib.Path) -> list[pathlib.Path]:
    dependencies: list[pathlib.Path] = []
    for token in shlex.split(link_command):
        if not token.endswith(".a"):
            continue
        path = pathlib.Path(token)
        if not path.is_absolute():
            path = build_dir / path
        path = path.resolve()
        if path not in dependencies:
            dependencies.append(path)
    if not dependencies:
        raise ValueError("nvattest dynamic link command supplied no static dependencies")
    return dependencies


def _record_digest(record: dict[str, Any]) -> str:
    canonical = json.dumps(record, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(canonical).hexdigest()


def _prepare_host_build_profile(build: Build) -> None:
    configuration = build.work / ".cargo/config.toml"
    configuration.parent.mkdir()
    configuration.write_text('[profile.release.build-override]\nstrip = "none"\n')
    build.summary["inputs"]["appraisal-host-build-profile"] = {
        "path": ".cargo/config.toml",
        "sha256": sha256(configuration),
        "release_build_override": {"strip": "none"},
    }
    build.save()


def _read_required_symbols(build: Build, aggregate: pathlib.Path) -> str:
    """Read Darwin's partial table while rejecting any unexpected nm failure."""
    argv = ["nm", "-gU", aggregate]
    started = time.time()
    result = subprocess.run(
        list(map(str, argv)),
        cwd=REPO_ROOT,
        env=build.env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    log = build.receipts / "nvattest-static-symbols.log"
    log.write_bytes(result.stdout + result.stderr)
    unexpected = [
        line
        for line in result.stderr.decode(errors="replace").splitlines()
        if line
        and "Unknown attribute kind (105)" not in line
        and not line.endswith(": no symbols")
    ]
    accepted = result.returncode == 0 or (
        result.returncode == 1 and not unexpected and bool(result.stdout)
    )
    build.summary["steps"].append(
        {
            "name": "nvattest-static-symbols",
            "argv": list(map(str, argv)),
            "cwd": str(REPO_ROOT),
            "started_epoch": started,
            "seconds": round(time.time() - started, 3),
            "exit": result.returncode,
            "accepted": accepted,
            "accepted_nonzero_reason": (
                "Apple nm cannot decode LLVM 22 attributes in Rust compiler_builtins; "
                "the required C symbols remain readable and final helper linkage is authoritative"
                if result.returncode == 1 and accepted
                else ""
            ),
            "log": str(log),
            "log_sha256": sha256(log),
        }
    )
    build.save()
    if not accepted:
        raise ValueError(
            f"nm failed with exit {result.returncode}; unexpected diagnostics: {unexpected[:5]}"
        )
    return result.stdout.decode(errors="replace")


def build_static(args: argparse.Namespace) -> None:
    mobile_args = argparse.Namespace(
        platform="ios",
        work_dir=args.work_dir,
        cache_dir=args.cache_dir,
        jobs=args.jobs,
        components=["nvattest"],
        full_run=False,
    )
    build = Build(mobile_args)
    try:
        _prepare_host_build_profile(build)
        build.setup()
        build.nvattest()

        nvattest_dir = build.work / "build/nvattest"
        dynamic_link = (nvattest_dir / "CMakeFiles/nvat.dir/link.txt").read_text()
        dependencies = _static_dependencies(dynamic_link, nvattest_dir)
        build.run("nvattest-static-configure", _configure_argv(build))
        build.run(
            "nvattest-static-build",
            ["cmake", "--build", nvattest_dir, f"-j{args.jobs}", "--target", "nvat"],
        )
        primary = nvattest_dir / "libnvat.a"
        archives = [primary, *dependencies]
        missing = [str(path) for path in archives if not path.is_file()]
        if missing:
            raise ValueError(f"static closure member missing after rebuild: {missing}")

        args.output_lib.parent.mkdir(parents=True, exist_ok=False)
        aggregate = args.output_lib
        build.run(
            "nvattest-static-aggregate",
            ["xcrun", "libtool", "-static", "-o", aggregate, *archives],
        )
        architecture = build.run("nvattest-static-architecture", ["lipo", "-archs", aggregate])
        if architecture.read_text().strip() != "arm64":
            raise ValueError("aggregate nvattest archive is not arm64")
        symbols = _read_required_symbols(build, aggregate)
        missing_symbols = [
            name
            for name in NVAT_PRODUCTION_SYMBOLS
            if not re.search(rf"\b_?{re.escape(name)}$", symbols, re.MULTILINE)
        ]
        if missing_symbols:
            raise ValueError(
                f"aggregate nvattest archive lacks production symbols: {missing_symbols}"
            )

        args.resource_dir.mkdir(parents=True, exist_ok=False)
        ca_source = build.work / "artifacts/nvattest/ca-bundle.pem"
        shutil.copyfile(ca_source, args.resource_dir / "ca-bundle.pem")
        rims_dir = REPO_ROOT / "core/crates/solstone-core-spp-attest/rims"
        rims = []
        for source in sorted(rims_dir.iterdir()):
            if source.suffix not in (".xml", ".corim"):
                continue
            destination = args.resource_dir / source.name
            shutil.copyfile(source, destination)
            rims.append({"filename": source.name, "sha256": sha256(destination)})

        record = {
            "schema": "ios-gpu-appraisal-static-build-v1",
            "complete": True,
            "status": "passed",
            "platform": "ios",
            "arch": "arm64",
            "mobile_native_build": build.summary,
            "aggregate": {
                "sha256": sha256(aggregate),
                "size_bytes": aggregate.stat().st_size,
            },
            "static_archives": [
                {
                    "name": path.name,
                    "sha256": sha256(path),
                    "size_bytes": path.stat().st_size,
                }
                for path in archives
            ],
            "production_symbols": list(NVAT_PRODUCTION_SYMBOLS),
            "resources": {
                "ca_bundle_sha256": sha256(args.resource_dir / "ca-bundle.pem"),
                "rims": rims,
            },
        }
        record["record_sha256"] = _record_digest(record)
        args.build_record.parent.mkdir(parents=True, exist_ok=True)
        args.build_record.write_text(json.dumps(record, indent=2) + "\n")
        build.summary["complete"] = True
        build.save(0)
    except Exception as error:
        build.summary["error"] = str(error)
        build.save(getattr(error, "returncode", 1))
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=pathlib.Path, required=True)
    parser.add_argument("--cache-dir", type=pathlib.Path, required=True)
    parser.add_argument("--output-lib", type=pathlib.Path, required=True)
    parser.add_argument("--resource-dir", type=pathlib.Path, required=True)
    parser.add_argument("--build-record", type=pathlib.Path, required=True)
    parser.add_argument("--jobs", type=int, default=6)
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    try:
        build_static(args)
    except Exception as error:
        print(f"iOS appraisal native build failed: {error}", file=sys.stderr)
        return getattr(error, "returncode", 1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
