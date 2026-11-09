#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Verify and stage the maintained mobile Qwen image for app assembly."""

import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import stat
import struct

import mobile_native_build
import package_mobile_pdfium

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_LICENSES = ("llama-LICENSE.txt", "openssl-LICENSE.txt")
THIRD_PARTY_NOTICES = "qwen-THIRD_PARTY_NOTICES.txt"
LICENSES = (*PACKAGE_LICENSES, THIRD_PARTY_NOTICES)
IMAGES = {
    "android": "libjournal_mobile_qwen.so",
    "ios": "JournalMobileQwen.framework/JournalMobileQwen",
}


def inspect_android(image_bytes):
    result = package_mobile_pdfium.inspect_android(image_bytes)
    offset = struct.unpack_from("<Q", image_bytes, 32)[0]
    size, count = struct.unpack_from("<HH", image_bytes, 54)
    relro = []
    for index in range(count):
        tag, _, _, address, _, _, memory_size, _ = struct.unpack_from(
            "<IIQQQQQQ", image_bytes, offset + index * size)
        if tag == 0x6474E552:
            relro.append(address + memory_size)
    if len(relro) != 1 or relro[0] % 16384:
        raise ValueError("Qwen protected read-only region does not support 16 KiB pages")
    return result


def read_regular(path, limit):
    path = Path(path)
    if path.is_symlink():
        raise ValueError("Qwen package member is a symbolic link")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    with os.fdopen(os.open(path, flags), "rb") as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > limit:
            raise ValueError("Qwen package member is not a bounded regular file")
        data = source.read(limit + 1)
        if len(data) > limit:
            raise ValueError("Qwen package member exceeds its size bound")
        return data


def license_bytes(package, name):
    if name == THIRD_PARTY_NOTICES:
        return read_regular(ROOT / "core/distribution/mobile" / name, 1024 * 1024)
    if name not in PACKAGE_LICENSES:
        raise ValueError("unknown Qwen license resource")
    return read_regular(Path(package) / name, 1024 * 1024)


def copy_licenses(package, destination):
    for name in LICENSES:
        path = Path(destination) / name
        if path.is_symlink():
            raise ValueError("Qwen license destination is a symbolic link")
        path.write_bytes(license_bytes(package, name))


def verify_licenses(package, destination):
    for name in LICENSES:
        if read_regular(Path(destination) / name, 1024 * 1024) != license_bytes(package, name):
            raise ValueError("Qwen license differs from its admitted source")


def validate_record(record, platform):
    if platform not in IMAGES:
        raise ValueError("unsupported native Qwen platform")
    binding = record.get("binding", {})
    pins = json.loads((ROOT / "core/distribution/mobile/inputs.json").read_text())["inputs"]
    patched = binding.get("patched_source_sha256", "")
    if len(patched) != 64 or any(char not in "0123456789abcdef" for char in patched):
        raise ValueError("Qwen package has no patched-source identity")
    diagnostics = binding.get("diagnostics", {})
    metal_no_residency = (diagnostics == {"metal_no_residency": True}
                          and diagnostics["metal_no_residency"] is True)
    if diagnostics and (not metal_no_residency or platform != "ios"):
        raise ValueError("unsupported Qwen package diagnostic")
    patch_paths = ["core/distribution/mobile/llama-embedded-lifecycle.patch"]
    if platform == "android":
        patch_paths.append("core/distribution/mobile/llama-android-dotprod-repack.patch")
    if metal_no_residency:
        patch_paths.append("core/distribution/mobile/llama-metal-residency-diagnostic.patch")
    patches = [{"path": name, "sha256": mobile_native_build.sha256(ROOT / name)} for name in patch_paths]
    build_inputs = dict(binding.get("build_inputs", {}))
    build_inputs.update({
        "driver_sha256": mobile_native_build.sha256(ROOT / "scripts/mobile_native_build.py"),
        "inputs_sha256": mobile_native_build.sha256(ROOT / "core/distribution/mobile/inputs.json"),
        "support_driver_sha256": mobile_native_build.sha256(ROOT / "scripts/android_cross_build.py"),
        "openssl": pins["openssl"],
    })
    expected, identity = mobile_native_build.qwen_runtime_binding(
        platform, pins["llama"], patched, patches, ROOT / "tools/mobile-qwen-engine", build_inputs,
        metal_no_residency=metal_no_residency,
    )
    if (record.get("schema") != "solstone.mobile-qwen-package.v1"
            or record.get("signed") is not False
            or record.get("image") != IMAGES[platform]
            or binding != expected
            or record.get("runtime_identity") != identity
            or record.get("exports") != sorted(mobile_native_build.QWEN_EXPORTS)):
        raise ValueError("Qwen package does not match the maintained source, platform or execution policy")


def load_package(directory, platform):
    """Return the source-bound unsigned package record and its verified image."""
    directory = Path(directory)
    if directory.is_symlink() or not directory.is_dir() or platform not in IMAGES:
        raise ValueError("a native Qwen package directory and supported platform are required")
    record = json.loads(read_regular(directory / "manifest.json", 256 * 1024))
    validate_record(record, platform)
    image = directory / IMAGES[platform]
    image_bytes = read_regular(image, 256 * 1024 * 1024)
    if (hashlib.sha256(image_bytes).hexdigest() != record.get("image_sha256")
            or len(image_bytes) != record.get("image_size_bytes")):
        raise ValueError("Qwen package image differs from its native receipt")
    if platform == "ios":
        package_mobile_pdfium.inspect_ios(image_bytes, "@rpath/JournalMobileQwen.framework/JournalMobileQwen")
        info = plistlib.loads(read_regular(image.parent / "Info.plist", 256 * 1024))
        if (info.get("CFBundleExecutable") != "JournalMobileQwen"
                or info.get("CFBundlePackageType") != "FMWK"):
            raise ValueError("Qwen framework metadata differs from its image")
    else:
        # This existing parser checks generic AArch64 shared-library load
        # geometry, including the required 16 KiB page alignment.
        inspect_android(image_bytes)
    for name in LICENSES:
        license_bytes(directory, name)
    return record, image


def stage_android(directory, runtime_root):
    record, image = load_package(directory, "android")
    runtime_root = Path(runtime_root)
    native = runtime_root / "lib/arm64-v8a"
    assets = runtime_root / "assets/qwen-runtime"
    native.mkdir(parents=True, exist_ok=True)
    assets.mkdir(parents=True, exist_ok=True)
    shutil.copy2(image, native / image.name)
    copy_licenses(directory, assets)
    verify_licenses(directory, assets)
    (assets / "manifest.json").write_text(json.dumps(record, indent=2) + "\n")
    return record


def verify_android_archive(archive, runtime_root):
    assets = Path(runtime_root) / "assets/qwen-runtime"
    record_bytes = read_regular(assets / "manifest.json", 256 * 1024)
    record = json.loads(record_bytes)
    validate_record(record, "android")
    if archive.read("assets/qwen-runtime/manifest.json") != record_bytes:
        raise ValueError("APK Qwen package record differs from the admitted package")
    image = archive.read("lib/arm64-v8a/libjournal_mobile_qwen.so")
    if (hashlib.sha256(image).hexdigest() != record["image_sha256"]
            or len(image) != record["image_size_bytes"]):
        raise ValueError("APK Qwen image differs from the admitted package")
    inspect_android(image)
    for name in LICENSES:
        if archive.read("assets/qwen-runtime/" + name) != license_bytes(assets, name):
            raise ValueError("APK Qwen license differs from the admitted package")
    verify_licenses(assets, assets)
    return record
