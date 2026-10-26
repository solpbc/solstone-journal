#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Package pinned appraisal resources, generate metadata, and sign the iOS app."""

import argparse
import hashlib
import json
import pathlib
import plistlib
import re
import shutil
import struct
import subprocess
import sys

import build_backup

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import package_mobile_pdfium
import package_mobile_media


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def macho_content_sha256(path: pathlib.Path) -> str:
    data = path.read_bytes()
    if len(data) < 32 or data[:4] != b"\xcf\xfa\xed\xfe":
        raise ValueError(f"helper is not a little-endian 64-bit Mach-O: {path}")
    ncmds, sizeofcmds = struct.unpack_from("<II", data, 16)
    if ncmds > 512 or sizeofcmds > 1024 * 1024 or 32 + sizeofcmds > len(data):
        raise ValueError("helper load-command bounds are invalid")
    cursor = 32
    signature = None
    for _ in range(ncmds):
        if cursor + 8 > 32 + sizeofcmds:
            raise ValueError("helper load command is truncated")
        command, command_size = struct.unpack_from("<II", data, cursor)
        if command_size < 8 or cursor + command_size > 32 + sizeofcmds:
            raise ValueError("helper load command size is invalid")
        if command == 0x1D:
            if command_size < 16 or signature is not None:
                raise ValueError("helper code-signature command is invalid")
            offset, size = struct.unpack_from("<II", data, cursor + 8)
            end = offset + size
            if offset < 32 + sizeofcmds or end > len(data):
                raise ValueError("helper code-signature range is invalid")
            signature = (offset, end)
        cursor += command_size
    if signature is None:
        raise ValueError("helper has no code-signature command")
    start, end = signature
    return hashlib.sha256(data[:start] + data[end:]).hexdigest()


def canonical_digest(value: dict) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def package_members(bundle: pathlib.Path) -> list[dict[str, object]]:
    members = []
    for path in sorted(bundle.rglob("*")):
        relative = path.relative_to(bundle).as_posix()
        if relative in ("gpu-appraisal-receipt.json", "_CodeSignature/CodeResources"):
            continue
        if path.is_symlink():
            raise ValueError(f"appraisal bundle contains symlink: {relative}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"appraisal bundle contains non-file member: {relative}")
        digest = (
            macho_content_sha256(path)
            if relative == "Appraisal"
            else sha256(path)
        )
        members.append(
            {
                "path": relative,
                "sha256": digest,
                "size_bytes": path.stat().st_size,
            }
        )
    return members


def prepare_rim_directory(rims: pathlib.Path, allowed_names: set[str]) -> None:
    if rims.is_symlink():
        raise ValueError("appraisal RIM directory is a symlink")
    rims.mkdir(exist_ok=True)
    for member in rims.iterdir():
        if member.is_symlink() or not member.is_file():
            raise ValueError(f"appraisal RIM directory contains non-file: {member.name}")
        if member.name not in allowed_names:
            raise ValueError(f"appraisal RIM directory contains unexpected member: {member.name}")


def needed_libraries(helper: pathlib.Path) -> list[str]:
    output = subprocess.check_output(["otool", "-L", helper], text=True)
    result = []
    for line in output.splitlines()[1:]:
        value = line.strip().split(" (", 1)[0]
        if value:
            result.append(value)
    if not result:
        raise ValueError("appraisal helper has no dynamic system closure")
    return result


def production_pcr_pins() -> list[str]:
    text = (
        ROOT / "core/crates/solstone-core-spp-attest/src/pins.rs"
    ).read_text()
    match = re.search(
        r"pub const PRODUCTION_PCR_SHA256_PINS:\s*&\[&str\]\s*=\s*&\[(.*?)\];",
        text,
        re.DOTALL,
    )
    if not match:
        raise ValueError("production PCR pins are unavailable")
    pins = re.findall(r'"([0-9a-fA-F]{64})"', match.group(1))
    if not pins:
        raise ValueError("production PCR pin set is empty")
    return pins


def regorus_pin() -> dict[str, str]:
    data = json.loads(
        (
            ROOT / "core/distribution/nvattest-windows-sources.json"
        ).read_text()
    )
    matches = [
        item
        for item in data.get("native_sources", [])
        if item.get("name") == "regorus-build-revision"
    ]
    if len(matches) != 1:
        raise ValueError("regorus build revision pin is unavailable")
    item = matches[0]
    return {
        "revision": item["revision"],
        "original_sha256": item["original_sha256"],
        "patched_sha256": item["sha256"],
    }


def validate_build_inputs(record: dict[str, object]) -> None:
    mobile = record.get("mobile_native_build")
    if not isinstance(mobile, dict):
        raise ValueError("appraisal build record has no mobile native build")
    expected = {
        "driver_sha256": sha256(ROOT / "scripts/mobile_native_build.py"),
        "inputs_sha256": sha256(ROOT / "core/distribution/mobile/inputs.json"),
    }
    for field, digest in expected.items():
        if mobile.get(field) != digest:
            raise ValueError(f"appraisal build record {field} differs from the source tree")
    patches = mobile.get("patches")
    if not isinstance(patches, list):
        raise ValueError("appraisal build record has no patch census")
    nvattest = [
        patch
        for patch in patches
        if isinstance(patch, dict) and patch.get("component") == "nvattest"
    ]
    current_patch = sha256(ROOT / "core/distribution/mobile/nvattest-mobile.patch")
    if len(nvattest) != 1 or nvattest[0].get("sha256") != current_patch:
        raise ValueError("appraisal build record nvattest patch differs from the source tree")


def build_receipt(
    helper: pathlib.Path,
    bundle: pathlib.Path,
    build_record_path: pathlib.Path,
    resources: pathlib.Path,
) -> dict[str, object]:
    record = json.loads(build_record_path.read_text())
    recorded_digest = record.pop("record_sha256", None)
    if recorded_digest != canonical_digest(record):
        raise ValueError("appraisal build record digest mismatch")
    record["record_sha256"] = recorded_digest
    if (
        record.get("schema") != "ios-gpu-appraisal-static-build-v1"
        or record.get("status") != "passed"
        or record.get("complete") is not True
        or record.get("platform") != "ios"
        or record.get("arch") != "arm64"
    ):
        raise ValueError("appraisal build record is incomplete")

    validate_build_inputs(record)

    mobile = record["mobile_native_build"]
    pins = mobile["inputs"]
    regorus = regorus_pin()
    rim_entries = [
        {"filename": path.name, "sha256": sha256(path)}
        for path in sorted(resources.iterdir())
        if path.suffix in (".xml", ".corim")
    ]
    return {
        "schema": "gpu-appraisal-receipt-v2",
        "status": "passed",
        "complete": True,
        "exit": 0,
        "platform": "ios",
        "arch": "arm64",
        "driver_sha256": sha256(ROOT / "scripts/mobile_native_build.py"),
        "inputs_sha256": sha256(ROOT / "core/distribution/mobile/inputs.json"),
        "patch_sha256": sha256(
            ROOT / "core/distribution/mobile/nvattest-mobile.patch"
        ),
        "nvattest_revision": pins["nvattest"]["revision"],
        "regorus_revision": regorus["revision"],
        "regorus_original_sha256": regorus["original_sha256"],
        "regorus_patched_sha256": regorus["patched_sha256"],
        "ca_bundle_sha256": sha256(resources / "ca-bundle.pem"),
        "pcr_sha256_pins": production_pcr_pins(),
        "rims": rim_entries,
        "needed": needed_libraries(helper),
        "helper_sha256": macho_content_sha256(helper),
        "build_record_sha256": sha256(build_record_path),
        "package_members": package_members(bundle),
        "steps": mobile["steps"],
        "link_boundary": (
            "compile, static closure, final helper linkage, packaged resources, "
            "extension metadata, and signing identity; device appraisal is separate"
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("build_root", type=pathlib.Path)
    parser.add_argument("--bundle-identifier", default="app.solstone.journal.runtime.dev",
                        choices=("app.solstone.journal.runtime.dev", "app.solstone.journal.owner.dev"))
    parser.add_argument("--nvattest-lib", type=pathlib.Path, required=True)
    parser.add_argument("--appraisal-build-record", type=pathlib.Path, required=True)
    parser.add_argument("--appraisal-resources", type=pathlib.Path, required=True)
    parser.add_argument("--backup-lib", type=pathlib.Path, required=True)
    parser.add_argument("--backup-work", type=pathlib.Path, required=True)
    parser.add_argument("--pdfium-dir", type=pathlib.Path, required=True)
    parser.add_argument(
        "--keychain",
        default=str(
            pathlib.Path.home() / "Library/Keychains/sol-signing.keychain-db"
        ),
    )
    parser.add_argument(
        "--identity", default="Apple Development: Created via API (SNP7CMKMZ5)"
    )
    args = parser.parse_args()

    build_record = json.loads(args.appraisal_build_record.read_text())
    if sha256(args.nvattest_lib) != build_record.get("aggregate", {}).get("sha256"):
        raise ValueError("linked nvattest archive differs from build record")

    products = args.build_root / "derived/Build/Products/Debug-iphoneos"
    intermediates = (
        args.build_root
        / "derived/Build/Intermediates.noindex/JournalRuntimeDev.build/Debug-iphoneos"
    )
    app = products / "JournalRuntimeDev.app"
    media_manifest = None
    payload_source = ROOT / "tools/ios-runtime/Resources/JournalPayload"
    if args.bundle_identifier == "app.solstone.journal.owner.dev" or (payload_source / "media-models.json").exists():
        media_manifest = package_mobile_media.verify_payload(ROOT, payload_source)
        for bundle in [app, app / "Extensions/JournalWorker.appex"]:
            package_mobile_media.verify_directory(bundle / "JournalPayload", media_manifest)
    backup_record, backup_archive = build_backup.validate_output(args.backup_work.resolve())
    if build_backup.read_regular(args.backup_lib, 256 * 1024 * 1024) != backup_archive:
        raise ValueError("linked backup archive differs from its measured recipe")
    for name, bundle in [("JournalRuntimeDev", app), ("JournalWorker", app / "Extensions/JournalWorker.appex")]:
        build_backup.confirm_resource(bundle / "BackupRuntime", backup_record)
        build_backup.check_symbols(bundle / name)
    appraisal = app / "Extensions/Appraisal.appex"
    rims = appraisal / "RIMs"
    prepare_rim_directory(
        rims,
        {source.name for source in args.appraisal_resources.iterdir()
         if source.suffix in (".xml", ".corim")},
    )
    for source in sorted(args.appraisal_resources.iterdir()):
        if not source.is_file() or source.is_symlink():
            raise ValueError(f"invalid appraisal resource: {source}")
        if source.name == "ca-bundle.pem":
            destination = appraisal / source.name
        elif source.suffix in (".xml", ".corim"):
            destination = rims / source.name
        else:
            raise ValueError(f"unexpected appraisal resource: {source}")
        shutil.copyfile(source, destination)
    shutil.copyfile(
        args.appraisal_build_record, appraisal / "gpu-appraisal-build.json"
    )

    sdk = subprocess.check_output(
        ["xcrun", "--sdk", "iphoneos", "--show-sdk-path"], text=True
    ).strip()
    xcode_version = subprocess.check_output(
        ["xcodebuild", "-version"], text=True
    ).split()[-1]
    for name, bundle in [
        ("JournalWorker", app / "Extensions/JournalWorker.appex"),
        ("Appraisal", appraisal),
        ("JournalRuntimeDev", app),
    ]:
        info_path = bundle / "Info.plist"
        info = plistlib.loads(info_path.read_bytes())
        source_name = (
            "Host"
            if name == "JournalRuntimeDev"
            else "Appraisal"
            if name == "Appraisal"
            else "Helper"
        )
        inputs = [
            intermediates
            / f"{name}.build/Objects-normal/arm64/{source_name}.swiftconstvalues"
        ]
        if not all(path.is_file() for path in inputs):
            raise ValueError(f"compiler metadata is missing for {name}")
        host = name == "JournalRuntimeDev"
        output = (
            bundle / "Extensions/JournalRuntimeDev.appexpt"
            if host
            else args.build_root / f"{name}-metadata.plist"
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            [
                "xcrun",
                "exutil",
                "extract-extension-points"
                if host
                else "generate-appextension-plist",
                "--module-name",
                name,
                "--sdk-root",
                sdk,
                "--xcode-version",
                xcode_version,
                "--platform-family",
                "iOS",
                "--deployment-target",
                "26.0",
                "--bundle-identifier",
                info["CFBundleIdentifier"],
                "--output",
                str(output),
                *map(str, inputs),
            ],
            check=True,
        )
        generated = plistlib.loads(output.read_bytes())
        if not host:
            if "EXAppExtensionAttributes" not in generated:
                raise ValueError(f"extension metadata missing for {name}")
            info.update(generated)
            info_path.write_bytes(plistlib.dumps(info))
        elif not generated:
            raise ValueError("host extension points are missing")

    info = plistlib.loads((appraisal / "Info.plist").read_bytes())
    if (
        info.get("CFBundleIdentifier") != args.bundle_identifier + ".gpu-appraisal"
        or info.get("CFBundleExecutable") != "Appraisal"
        or info.get("EXAppExtensionAttributes", {}).get(
            "EXExtensionPointIdentifier"
        )
        != args.bundle_identifier + ".gpu-appraisal"
    ):
        raise ValueError("generated appraisal extension identity drifted")

    receipt_path = appraisal / "gpu-appraisal-receipt.json"
    receipt = build_receipt(
        appraisal / "Appraisal",
        appraisal,
        args.appraisal_build_record,
        args.appraisal_resources,
    )
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")

    pdfium_record = package_mobile_pdfium.embed_ios(args.pdfium_dir, app)
    pdfium_framework = app / "Frameworks/PDFium.framework"
    subprocess.run(
        ["codesign", "--force", "--sign", args.identity, "--keychain", args.keychain,
         "--timestamp=none", str(pdfium_framework)], check=True,
    )
    subprocess.run(
        ["codesign", "--verify", "--strict", str(pdfium_framework)], check=True,
    )
    signed_pdfium_sha256 = sha256(pdfium_framework / "PDFium")
    expected_members = receipt["package_members"]
    for name, bundle in [
        ("JournalWorker", app / "Extensions/JournalWorker.appex"),
        ("Appraisal", appraisal),
        ("JournalRuntimeDev", app),
    ]:
        entitlement = intermediates / f"{name}.build/{bundle.name}.xcent"
        subprocess.run(
            [
                "codesign",
                "--force",
                "--sign",
                args.identity,
                "--keychain",
                args.keychain,
                "--entitlements",
                str(entitlement),
                "--timestamp=none",
                "--generate-entitlement-der",
                str(bundle),
            ],
            check=True,
        )
    if package_members(appraisal) != expected_members:
        raise ValueError("codesigning changed a receipt-bound appraisal member")
    for bundle in [app, app / "Extensions/JournalWorker.appex"]:
        build_backup.confirm_resource(bundle / "BackupRuntime", backup_record)
        if media_manifest:
            package_mobile_media.verify_directory(bundle / "JournalPayload", media_manifest)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    if sha256(pdfium_framework / "PDFium") != signed_pdfium_sha256:
        raise ValueError("containing app signing changed the PDFium framework")
    if media_manifest:
        (args.build_root / "media-package-receipt.json").write_text(json.dumps({
            "schema": "solstone.mobile-media-package.v1",
            "manifest": media_manifest,
            "bundles": ["JournalRuntimeDev.app/JournalPayload", "JournalRuntimeDev.app/Extensions/JournalWorker.appex/JournalPayload"],
        }, indent=2) + "\n")
    (args.build_root / "pdfium-package-receipt.json").write_text(json.dumps({
        "schema": "solstone.mobile-pdfium-package.v1",
        "staging_receipt_sha256": sha256(args.pdfium_dir / "receipt.json"),
        "archive_sha256": pdfium_record["pin"]["sha256"],
        "unsigned_library_sha256": next(row["sha256"] for row in pdfium_record["members"]
                                        if row["path"] == "PDFium.framework/PDFium"),
        "signed_library_sha256": signed_pdfium_sha256,
        "framework_members": [{"path": path.relative_to(app).as_posix(), "sha256": sha256(path)}
                              for path in sorted(pdfium_framework.rglob("*")) if path.is_file()],
        "notice_members": [{"path": path.relative_to(app).as_posix(), "sha256": sha256(path)}
                           for path in sorted((app / "PDFiumNotices").rglob("*")) if path.is_file()],
    }, indent=2) + "\n")
    print("Packaged appraisal closure and verified development signatures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
