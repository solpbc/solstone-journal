#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Host regression for the real endpoint/caller; supply a JVM org.json jar.

No Android runtime, JNI or device is used. The Android platform doubles model
Message recycling before deferred work, while org.json supplies actual JSON.
The existing lifecycle-only endpoint substitute is deliberately excluded.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--java-home', type=Path, required=True)
    parser.add_argument('--json-jar', type=Path, required=True)
    args = parser.parse_args()
    jar = args.json_jar.resolve(strict=True)
    java = args.java_home.resolve(strict=True) / 'bin'
    root = Path(__file__).resolve().parent
    owner = root.parent / 'src/app/solstone/development/journal'
    excluded = {'CreatorRecoveryEndpoint.java', 'OwnerCreatorRecoveryLifecycleTest.java',
                'IBinder.java', 'AlertDialog.java'}
    sources = [p for p in (root / 'creator_recovery_lifecycle').rglob('*.java')
               if 'org/json' not in p.as_posix() and p.name not in excluded]
    sources += list((root / 'creator_recovery_delivery').rglob('*.java'))
    sources += [owner / (name + '.java') for name in
                ('CreatorRecoveryState', 'CreatorRecoveryEndpoint', 'OwnerCreatorRecovery')]
    with tempfile.TemporaryDirectory(prefix='recovery-delivery-') as directory:
        subprocess.run([str(java / 'javac'), '-cp', str(jar), '-d', directory,
                        *map(str, sorted(sources))], check=True, timeout=120)
        subprocess.run([str(java / 'java'), '-cp', directory + ':' + str(jar),
                        'app.solstone.development.journal.CreatorRecoveryDeliveryTest'],
                       check=True, timeout=30)


if __name__ == '__main__':
    main()
