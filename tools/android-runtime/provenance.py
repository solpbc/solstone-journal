# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Hash local inputs, including uncommitted development app sources."""
import hashlib
import pathlib
import subprocess


def source_manifest(repo):
    repo = pathlib.Path(repo)
    scope = ['core', 'tools/android-runtime', 'tools/mobile-serving', 'tools/mobile-audio', 'rust-toolchain.toml', '.cargo', 'scripts/package_mobile_pdfium.py', 'scripts/android_cross_build.py', 'scripts/mobile_native_build.py']
    def git(*args):
        return subprocess.check_output(['git', *args], cwd=repo)
    names = git('ls-files', '-z', '--cached', '--others', '--exclude-standard', '--', *scope)
    files = {}
    for name in sorted(set(names.decode().split('\0')) - {''}):
        file = repo / name
        if file.is_file():
            files[name] = hashlib.sha256(file.read_bytes()).hexdigest()
    return dict(head=git('rev-parse', 'HEAD').decode().strip(),
                tracked_diff_sha256=hashlib.sha256(git('diff', '--binary', 'HEAD', '--', *scope)).hexdigest(),
                files=files)
