#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Package retained Android native tools through the maintained manifest producer."""
import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native-build', required=True, type=pathlib.Path,
                        help='retained Android build directory with artifacts/ and receipts/')
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parent
    repo = root.parents[1]
    receipt_path = args.native_build / 'receipts/mobile-native-build.json'
    receipt_bytes = receipt_path.read_bytes()
    receipt = json.loads(receipt_bytes)
    if receipt.get('platform') != 'android' or receipt.get('exit') != 0:
        raise ValueError('successful Android native build receipt required')
    artifacts = receipt['artifacts']
    roles = [
        ('restic-cli', 'artifacts/restic/restic', 'librestic.so'),
        ('rclone-cli', 'artifacts/rclone/rclone', 'librclone.so'),
        ('nvattest-cli', 'build/nvattest/nvattest', 'libnvattest.so'),
        ('nvattest-library', 'build/nvattest/nv-attestation-sdk-build/libnvat.so', 'libnvat.so'),
        ('nvattest-ca-bundle', 'artifacts/nvattest/ca-bundle.pem', 'ca-bundle.pem'),
    ]
    sources = []
    for component, relative, filename in roles:
        source = args.native_build / relative
        entries = [entry for entry in artifacts if entry.get('component') == component]
        if len(entries) != 1 or source.is_symlink() or not source.is_file():
            raise ValueError('one regular retained artifact required: ' + component)
        entry = entries[0]
        if source.stat().st_size != entry['size_bytes'] or digest(source) != entry['sha256']:
            raise ValueError('retained artifact differs from build receipt: ' + component)
        sources.append((source, filename))

    native = root / 'lib/arm64-v8a'
    assets = root / 'assets/service-tools'
    resources = assets / 'resources'
    native.mkdir(parents=True, exist_ok=True)
    resources.mkdir(parents=True, exist_ok=True)
    for source, filename in sources:
        destination = (resources if filename == 'ca-bundle.pem' else native) / filename
        shutil.copyfile(source, destination)
        destination.chmod(0o644 if filename in ('ca-bundle.pem', 'libnvat.so') else 0o755)
    packaged_receipt = assets / 'mobile-native-build.json'
    packaged_receipt.write_bytes(receipt_bytes)
    manifest = assets / 'manifest.json'
    # The Rust producer validates current immutable driver/native-input/patch
    # identities, exact artifact bytes, pinned CA/RIMs and the ELF load closure.
    subprocess.run([
        'cargo', 'run', '--locked', '--offline', '--manifest-path', str(repo / 'core/Cargo.toml'),
        '-p', 'solstone-core-spp-attest', '--example', 'android_service_tools_manifest', '--',
        str(native.resolve()), str(resources.resolve()), str(packaged_receipt), str(manifest),
    ], check=True)
    print(json.dumps({'manifest_sha256': digest(manifest), 'receipt_sha256': digest(packaged_receipt),
                      'native': str(native.resolve()), 'resources': str(resources.resolve())}))


if __name__ == '__main__':
    main()
