#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyyaml>=6,<7"]
# ///
"""Prepare the Windows journal's winget manifests and check its live channel."""

import argparse
import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import urllib.parse
import urllib.request

import yaml

PACKAGE = "solpbc.Journal"
PACK_ID = "SolstoneJournal"
SCHEMA = "1.12.0"
UPSTREAM = "microsoft/winget-pkgs"
ORIGIN = "https://updates.solstone.app/solstone-journal/release/windows"
DIRECTORY = Path(__file__).resolve().parents[1] / "packaging" / "winget"
TARGETS = {
    "x64": ("releases.win.json", "windows-x86_64"),
    "arm64": ("releases.win-arm64.json", "windows-aarch64"),
}
ARM_MINIMUM_OS = "10.0.22000.0"
MAX_FEED = 1024 * 1024
MAX_INSTALLER = 3 * 1024**3


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


HTTP = urllib.request.build_opener(NoRedirect())


def request(url):
    return urllib.request.Request(url, headers={"User-Agent": "solstone-journal-release/1.0"})


def published_version(architecture="x64"):
    feed_name, _ = TARGETS[architecture]
    with HTTP.open(request(f"{ORIGIN}/{feed_name}"), timeout=60) as response:
        raw = response.read(MAX_FEED + 1)
    if len(raw) > MAX_FEED:
        raise ValueError("Windows feed exceeds the size bound")
    feed = json.loads(raw)
    versions = {asset["Version"] for asset in feed["Assets"]
                if asset["PackageId"] == PACK_ID and asset["Type"] == "Full"}
    if not versions or any(not re.fullmatch(r"\d+\.\d+\.\d+", value) for value in versions):
        raise ValueError(f"Windows {architecture} feed has no valid full release")
    return max(versions, key=lambda version: tuple(map(int, version.split("."))))


def installer_url(version, architecture="x64"):
    _, target = TARGETS[architecture]
    return f"{ORIGIN}/solstone-journal-{version}-{target}-setup.exe"


def download_digest(url):
    digest = hashlib.sha256()
    size = 0
    with HTTP.open(request(url), timeout=300) as response:
        expected_size = response.headers.get("Content-Length")
        if expected_size is not None and not 0 < int(expected_size) <= MAX_INSTALLER:
            raise ValueError("installer length exceeds the size bound")
        while block := response.read(1024 * 1024):
            digest.update(block)
            size += len(block)
            if size > MAX_INSTALLER:
                raise ValueError("installer exceeds the size bound")
    if not size:
        raise ValueError(f"empty installer: {url}")
    if expected_size is not None and size != int(expected_size):
        raise ValueError(f"incomplete installer: received {size} of {expected_size} bytes")
    return digest.hexdigest().upper(), size


def github_json(path, allow_missing=False):
    result = subprocess.run(["gh", "api", "--method", "GET", path],
                            capture_output=True, text=True, timeout=120, check=False)
    if result.returncode:
        try:
            status = str(json.loads(result.stdout).get("status"))
        except (ValueError, AttributeError):
            status = None
        if allow_missing and status == "404":
            return None
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    return json.loads(result.stdout)


def write_manifest(suffix, data, kind):
    data.update(ManifestType=kind, ManifestVersion=SCHEMA)
    header = f"# yaml-language-server: $schema=https://aka.ms/winget-manifest.{kind}.{SCHEMA}.schema.json\n\n"
    (DIRECTORY / f"{PACKAGE}{suffix}.yaml").write_text(
        header + yaml.safe_dump(data, sort_keys=False, allow_unicode=True), encoding="utf-8")


def prepare(args):
    if not re.fullmatch(r"\d+\.\d+\.\d+", args.version):
        raise ValueError("version must be a released major.minor.patch")
    datetime.date.fromisoformat(args.release_date)
    architectures = ("x64", "arm64") if args.with_arm64 else ("x64",)
    for architecture in architectures:
        current = published_version(architecture)
        if current != args.version:
            raise ValueError(f"Windows {architecture} feed carries {current}, requested {args.version}")
    current = args.version
    installers = []
    sizes = {}
    for architecture in architectures:
        url = installer_url(current, architecture)
        digest, size = download_digest(url)
        row = dict(Architecture=architecture, InstallerUrl=url, InstallerSha256=digest)
        if architecture == "arm64":
            row["MinimumOSVersion"] = ARM_MINIMUM_OS
        installers.append(row)
        sizes[architecture] = size
    common = dict(PackageIdentifier=PACKAGE, PackageVersion=current)
    DIRECTORY.mkdir(parents=True, exist_ok=True)
    write_manifest(".installer", dict(
        **common, MinimumOSVersion="10.0.19045.0", Scope="user", InstallerType="exe",
        InstallModes=["silent", "silentWithProgress"],
        InstallerSwitches=dict(Silent="--silent", SilentWithProgress="--silent",
                               InstallLocation='--installto "<INSTALLPATH>"',
                               Log='--log "<LOGPATH>"'),
        UpgradeBehavior="install", ProductCode=PACK_ID, ReleaseDate=args.release_date,
        Dependencies=dict(PackageDependencies=[dict(PackageIdentifier="Microsoft.EdgeWebView2Runtime")]),
        AppsAndFeaturesEntries=[dict(ProductCode=PACK_ID)],
        InstallationMetadata=dict(DefaultInstallLocation=r"%LocalAppData%\SolstoneJournal"),
        Installers=installers
    ), "installer")
    write_manifest(".locale.en-US", dict(
        **common, PackageLocale="en-US", Publisher="sol pbc", PublisherUrl="https://solpbc.org",
        PublisherSupportUrl="https://support.solstone.app", PackageName="journal",
        PackageUrl="https://solstone.app", License="AGPL-3.0-only",
        LicenseUrl="https://github.com/solpbc/solstone-journal/blob/main/LICENSE",
        ShortDescription="your journal on windows", Moniker="solstone-journal",
        Tags=["journal", "open-source", "solstone"]
    ), "defaultLocale")
    write_manifest("", dict(**common, DefaultLocale="en-US"), "version")
    result = dict(package=PACKAGE, version=current,
                  installer_sha256=installers[0]["InstallerSha256"], installer_bytes=sizes["x64"])
    if args.with_arm64:
        result["installers"] = [dict(architecture=row["Architecture"],
                                     sha256=row["InstallerSha256"],
                                     bytes=sizes[row["Architecture"]]) for row in installers]
    print(json.dumps(result))


def check(args):
    manifests = [yaml.safe_load((DIRECTORY / f"{PACKAGE}{suffix}.yaml").read_text(encoding="utf-8"))
                 for suffix in ("", ".installer", ".locale.en-US")]
    version = manifests[0]["PackageVersion"]
    if any(doc["PackageIdentifier"] != PACKAGE or doc["PackageVersion"] != version for doc in manifests):
        raise ValueError("the three committed manifests disagree on package or version")
    architectures = ("x64", "arm64") if args.with_arm64 else ("x64",)
    installers = manifests[1]["Installers"]
    if (len(installers) != len(architectures)
            or {row["Architecture"] for row in installers} != set(architectures)):
        raise ValueError("installer architectures differ from the explicit selection; use --with-arm64 for both")
    checked = {}
    for installer in installers:
        architecture = installer["Architecture"]
        current = published_version(architecture)
        if version != current:
            raise ValueError(f"manifest carries {version}; Windows {architecture} origin carries {current}")
        if installer["InstallerUrl"] != installer_url(version, architecture):
            raise ValueError(f"manifest does not point at the versioned {architecture} journal Setup")
        if architecture == "arm64" and installer.get("MinimumOSVersion") != ARM_MINIMUM_OS:
            raise ValueError("ARM installer must require Windows 11")
        digest, size = download_digest(installer["InstallerUrl"])
        if digest != installer["InstallerSha256"].upper():
            raise ValueError(f"{architecture} installer digest mismatch: downloaded {digest}")
        checked[architecture] = dict(sha256=digest, bytes=size)
    digest, size = checked["x64"]["sha256"], checked["x64"]["bytes"]
    path = f"repos/{UPSTREAM}/contents/manifests/s/solpbc/Journal/{version}/{PACKAGE}.installer.yaml"
    merged = github_json(path, allow_missing=True)
    if merged is not None:
        import base64
        upstream = yaml.safe_load(base64.b64decode(merged["content"]))
        if (upstream["PackageIdentifier"] != PACKAGE or upstream["PackageVersion"] != version
                or upstream["Installers"] != manifests[1]["Installers"]):
            raise ValueError("merged winget installer differs from the committed manifest")
        result = dict(state="CURRENT", version=version, installer_sha256=digest, installer_bytes=size)
        code = 0
    else:
        query = urllib.parse.urlencode(dict(q=f'repo:{UPSTREAM} is:pr is:open "{PACKAGE}" "{version}" in:title'))
        search = github_json(f"search/issues?{query}")
        if search.get("incomplete_results"):
            raise ValueError("GitHub PR search returned incomplete results")
        exact = re.compile(rf"(?:^|\s){re.escape(PACKAGE)} version {re.escape(version)}(?:$|\s)")
        prs = [item for item in search["items"] if exact.search(item["title"])]
        result = dict(state="PENDING" if prs else "MISSING", version=version,
                      installer_sha256=digest, installer_bytes=size,
                      pull_requests=[item["html_url"] for item in prs])
        code = 0 if prs and args.allow_pending else 1
    if args.with_arm64:
        result["installers"] = checked
    print(json.dumps(result))
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("prepare", help="hash the published Setup and write the three manifests")
    command.add_argument("version")
    command.add_argument("--release-date", required=True)
    command.add_argument("--with-arm64", action="store_true", help="require same-version ARM64 and x64 installers")
    command = commands.add_parser("check", help="check origin version, complete Setup bytes and upstream winget state")
    command.add_argument("--allow-pending", action="store_true", help="allow an open PR while still reporting PENDING")
    command.add_argument("--with-arm64", action="store_true", help="require same-version ARM64 and x64 installers")
    args = parser.parse_args()
    try:
        return prepare(args) if args.command == "prepare" else check(args)
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"winget: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
