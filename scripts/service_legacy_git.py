#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Hermetic Git/ref adapter for service-evidence capture."""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from service_legacy_integrity import CANONICAL_REPOSITORY, IntegrityError

GIT = Path("/usr/bin/git")
TAG_NAMESPACE = "refs/service-legacy/authoritative-tags"
MAIN_REF = "refs/service-legacy/authoritative-main"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_helper() -> Path:
    candidates = (
        Path("/usr/libexec/git-core/git-remote-https"),
        Path("/usr/lib/git-core/git-remote-https"),
    )
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise IntegrityError("git-tool", "pinned git-remote-https helper is unavailable")


@dataclass(frozen=True)
class GitFacts:
    capture_input: str
    git_sha256: str
    helper_path: str
    helper_sha256: str
    main_object: str
    tags: tuple[dict[str, str], ...]


class GitTransport:
    """Run Git without caller config, credentials, proxy state, or repo discovery."""

    def __init__(
        self,
        repository: str = CANONICAL_REPOSITORY,
        *,
        allow_file_for_control: bool = False,
    ) -> None:
        if repository != CANONICAL_REPOSITORY and not allow_file_for_control:
            raise IntegrityError("remote-url", "noncanonical authority is forbidden")
        if allow_file_for_control and not repository.startswith("file://"):
            raise IntegrityError("remote-url", "control authority must use file://")
        if not GIT.is_file():
            raise IntegrityError("git-tool", f"missing {GIT}")
        self._temporary = tempfile.TemporaryDirectory(prefix="service-legacy-git-")
        root = Path(self._temporary.name).resolve()
        self.home = root / "home"
        self.xdg = root / "xdg"
        self.template = root / "template"
        self.empty_cwd = root / "outside"
        for directory in (self.home, self.xdg, self.template, self.empty_cwd):
            directory.mkdir()
        self.helper = git_helper().resolve()
        self.repository = repository
        self.allow_file_for_control = allow_file_for_control

    def close(self) -> None:
        self._temporary.cleanup()

    def __enter__(self) -> GitTransport:
        return self

    def __exit__(self, *_args: object) -> None:
        self.close()

    def environment(self, *, discovery_ceiling: bool = False) -> dict[str, str]:
        environment = {
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_EXEC_PATH": str(self.helper.parent),
            "GIT_TEMPLATE_DIR": str(self.template),
            "HOME": str(self.home),
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "PATH": "/usr/bin:/bin",
            "TMPDIR": "/var/tmp",
            "XDG_CONFIG_HOME": str(self.xdg),
        }
        if discovery_ceiling:
            environment["GIT_CEILING_DIRECTORIES"] = str(self.empty_cwd)
        return environment

    def run(
        self,
        arguments: list[str],
        *,
        cwd: Path | None = None,
        check: bool = True,
        discovery_ceiling: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        command = [
            str(GIT),
            "-c",
            "protocol.allow=never",
            "-c",
            f"protocol.{'file' if self.allow_file_for_control else 'https'}.allow=always",
            "-c",
            "http.sslVerify=true",
            "-c",
            "http.followRedirects=false",
            *arguments,
        ]
        result = subprocess.run(
            command,
            cwd=cwd or self.empty_cwd,
            env=self.environment(discovery_ceiling=discovery_ceiling),
            text=True,
            capture_output=True,
            check=False,
        )
        if check and result.returncode:
            detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
            raise IntegrityError("git-command", f"{' '.join(arguments)}: {detail}")
        return result

    def prove_empty_cwd(self) -> None:
        if any(self.empty_cwd.iterdir()):
            raise IntegrityError("git-cwd", "authoritative Git cwd is not empty")
        result = self.run(
            ["rev-parse", "--absolute-git-dir"],
            check=False,
            discovery_ceiling=True,
        )
        if result.returncode == 0:
            raise IntegrityError(
                "git-cwd", "authoritative Git cwd discovers a repository"
            )
        exec_path = self.run(["--exec-path"], discovery_ceiling=True).stdout.strip()
        if Path(exec_path).resolve() != self.helper.parent:
            raise IntegrityError(
                "git-tool", "Git did not retain the pinned helper root"
            )

    def ls_remote(self) -> tuple[str, dict[str, tuple[str, str]]]:
        self.prove_empty_cwd()
        result = self.run(
            ["ls-remote", self.repository, "refs/heads/main", "refs/tags/v0.*"],
            discovery_ceiling=True,
        )
        rows: dict[str, str] = {}
        for line in result.stdout.splitlines():
            object_id, separator, reference = line.partition("\t")
            if not separator or not re.fullmatch(r"[0-9a-f]{40}", object_id):
                raise IntegrityError("remote-ref", f"invalid ls-remote row {line!r}")
            if reference in rows:
                raise IntegrityError("remote-ref", f"duplicate remote ref {reference}")
            rows[reference] = object_id
        try:
            main = rows.pop("refs/heads/main")
        except KeyError as exc:
            raise IntegrityError("remote-main", "canonical remote lacks main") from exc
        base_refs = sorted(
            reference
            for reference in rows
            if reference.startswith("refs/tags/v0.") and not reference.endswith("^{}")
        )
        if len(base_refs) != 66:
            raise IntegrityError(
                "remote-tag-count", f"expected 66, found {len(base_refs)}"
            )
        if set(rows) - set(base_refs) - {reference + "^{}" for reference in base_refs}:
            raise IntegrityError("remote-ref", "unexpected canonical tag reference")
        tags = {
            reference.removeprefix("refs/tags/"): (
                rows[reference],
                rows.get(reference + "^{}", rows[reference]),
            )
            for reference in base_refs
        }
        return main, tags

    def clone(self, destination: Path) -> None:
        self.prove_empty_cwd()
        if destination.exists():
            raise IntegrityError("clone-destination", f"already exists: {destination}")
        self.run(
            ["clone", "--no-tags", self.repository, str(destination)],
            discovery_ceiling=True,
        )

    def fetch_authority(self, clone: Path) -> GitFacts:
        validate_disposable_clone(self, clone)
        capture_before = self.run(["rev-parse", "HEAD"], cwd=clone).stdout.strip()
        private_before = self.run(
            ["for-each-ref", "--format=%(refname)", "refs/service-legacy"],
            cwd=clone,
        ).stdout.splitlines()
        if private_before:
            raise IntegrityError(
                "private-ref",
                f"private authority namespace is not empty: {private_before}",
            )
        remote_main, remote_tags = self.ls_remote()
        self.run(
            [
                "fetch",
                "--no-tags",
                self.repository,
                f"+refs/heads/main:{MAIN_REF}",
                f"+refs/tags/v0.*:{TAG_NAMESPACE}/v0.*",
            ],
            cwd=clone,
        )
        local_main = self.run(["rev-parse", MAIN_REF], cwd=clone).stdout.strip()
        if local_main != remote_main:
            raise IntegrityError("remote-main", "fetched main differs from ls-remote")
        facts: list[dict[str, str]] = []
        for tag, (object_id, peeled_id) in sorted(remote_tags.items()):
            private = f"{TAG_NAMESPACE}/{tag}"
            local_object = self.run(["rev-parse", private], cwd=clone).stdout.strip()
            local_peeled = self.run(
                ["rev-parse", private + "^{}"], cwd=clone
            ).stdout.strip()
            if (local_object, local_peeled) != (object_id, peeled_id):
                raise IntegrityError(
                    "remote-tag", f"fetched {tag} differs from ls-remote"
                )
            facts.append(
                {
                    "object": object_id,
                    "peeled": peeled_id,
                    "ref": f"refs/tags/{tag}",
                }
            )
        capture = self.run(["rev-parse", "HEAD"], cwd=clone).stdout.strip()
        if capture != capture_before:
            raise IntegrityError("capture-input", "HEAD changed during authority fetch")
        if self.run(
            ["merge-base", "--is-ancestor", capture, MAIN_REF], cwd=clone, check=False
        ).returncode:
            raise IntegrityError(
                "capture-input", "HEAD is not reachable from canonical main"
            )
        return GitFacts(
            capture_input=capture,
            git_sha256=sha256(GIT),
            helper_path=str(self.helper),
            helper_sha256=sha256(self.helper),
            main_object=remote_main,
            tags=tuple(facts),
        )


def _local_config(clone: Path) -> dict[str, list[str]]:
    config = clone / ".git/config"
    if not config.is_file() or (clone / ".git/config.worktree").exists():
        raise IntegrityError("git-layout", "clone config shape is not a full clone")
    result = subprocess.run(
        [
            str(GIT),
            "config",
            "--file",
            str(config),
            "--no-includes",
            "--null",
            "--list",
        ],
        env={
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
            "HOME": "/var/empty",
            "PATH": "/usr/bin:/bin",
        },
        capture_output=True,
        check=True,
    )
    values: dict[str, list[str]] = {}
    for entry in result.stdout.decode().split("\0"):
        if not entry:
            continue
        key, separator, value = entry.partition("\n")
        if not separator:
            raise IntegrityError("git-config", f"malformed config row {entry!r}")
        values.setdefault(key.lower(), []).append(value)
    return values


def validate_disposable_clone(transport: GitTransport, clone: Path) -> None:
    clone = clone.resolve()
    marker = clone / ".git"
    if not marker.is_dir() or marker.is_symlink():
        raise IntegrityError(
            "git-layout", "capture root is not an independent full clone"
        )
    config = _local_config(clone)
    allowed_singletons = {
        "branch.main.merge": "refs/heads/main",
        "branch.main.remote": "origin",
        "core.bare": "false",
        "core.logallrefupdates": "true",
        "core.repositoryformatversion": "0",
        "remote.origin.fetch": "+refs/heads/*:refs/remotes/origin/*",
        "remote.origin.url": transport.repository,
    }
    for key, expected in allowed_singletons.items():
        if config.get(key) != [expected]:
            raise IntegrityError("git-config", f"{key} is not exact")
    optional_exact = {"remote.origin.tagopt": ["--no-tags"]}
    for key, expected in optional_exact.items():
        if key in config and config[key] != expected:
            raise IntegrityError("git-config", f"{key} is not exact")
    allowed_keys = set(allowed_singletons) | set(optional_exact) | {"core.filemode"}
    unexpected = set(config) - allowed_keys
    if unexpected:
        raise IntegrityError(
            "git-config", f"unexpected local keys: {sorted(unexpected)}"
        )
    if config.get("core.filemode") not in (["true"], ["false"]):
        raise IntegrityError("git-config", "core.filemode is not one boolean")
    for key in config:
        if (
            key.startswith(("url.", "include.", "http.", "credential.", "remote."))
            and key not in allowed_keys
        ):
            raise IntegrityError("git-config", f"forbidden local config key: {key}")

    git_dir = Path(
        transport.run(["rev-parse", "--absolute-git-dir"], cwd=clone).stdout.strip()
    ).resolve()
    common = Path(
        transport.run(["rev-parse", "--git-common-dir"], cwd=clone).stdout.strip()
    )
    if not common.is_absolute():
        common = (clone / common).resolve()
    else:
        common = common.resolve()
    expected = marker.absolute()
    if git_dir != expected or common != expected:
        raise IntegrityError("git-layout", "Git/common directory escapes full clone")
    targets = (
        "index",
        "objects",
        "refs",
        "logs",
        "packed-refs",
    )
    for target in targets:
        path = Path(
            transport.run(["rev-parse", "--git-path", target], cwd=clone).stdout.strip()
        )
        if not path.is_absolute():
            path = clone / path
        resolved = path.resolve(strict=False)
        if not resolved.is_relative_to(expected):
            raise IntegrityError(
                "git-layout", f"mutable Git target escapes clone: {target}"
            )
    alternates = expected / "objects/info/alternates"
    if alternates.exists() and alternates.read_text(encoding="utf-8").strip():
        raise IntegrityError("git-layout", "object alternates are forbidden")
    if (expected / "worktrees").exists():
        raise IntegrityError("git-layout", "registered linked worktrees are forbidden")
    status = transport.run(
        ["status", "--porcelain=v1", "--untracked-files=all"], cwd=clone
    ).stdout
    if status:
        raise IntegrityError("capture-clean", "capture clone is not completely clean")


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="service-legacy-git-test-") as temporary:
        root = Path(temporary)
        clone = root / "capture"
        environment = {
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
            "HOME": str(root / "home"),
            "PATH": "/usr/bin:/bin",
        }
        Path(environment["HOME"]).mkdir()
        wrong_work = root / "wrong-work"
        wrong_remote = root / "wrong.git"
        subprocess.run(
            [str(GIT), "init", "--initial-branch=main", str(wrong_work)],
            env=environment,
            check=True,
            capture_output=True,
        )
        (wrong_work / "canary").write_text("wrong authority", encoding="utf-8")
        for arguments in (
            ["config", "user.name", "Service Legacy Test"],
            ["config", "user.email", "service-legacy@example.invalid"],
            ["add", "canary"],
            ["commit", "-m", "wrong authority"],
        ):
            subprocess.run(
                [str(GIT), *arguments],
                cwd=wrong_work,
                env=environment,
                check=True,
                capture_output=True,
            )
        wrong_head = subprocess.run(
            [str(GIT), "rev-parse", "HEAD"],
            cwd=wrong_work,
            env=environment,
            check=True,
            text=True,
            capture_output=True,
        ).stdout.strip()
        (wrong_work / "canary").write_text("second authority", encoding="utf-8")
        subprocess.run(
            [str(GIT), "commit", "-am", "second authority"],
            cwd=wrong_work,
            env=environment,
            check=True,
            capture_output=True,
        )
        wrong_main = subprocess.run(
            [str(GIT), "rev-parse", "HEAD"],
            cwd=wrong_work,
            env=environment,
            check=True,
            text=True,
            capture_output=True,
        ).stdout.strip()
        for index in range(66):
            subprocess.run(
                [str(GIT), "tag", f"v0.{index:03d}", wrong_head],
                cwd=wrong_work,
                env=environment,
                check=True,
                capture_output=True,
            )
        subprocess.run(
            [str(GIT), "clone", "--bare", str(wrong_work), str(wrong_remote)],
            env=environment,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            [str(GIT), "init", "--initial-branch=main", str(clone)],
            env=environment,
            check=True,
            capture_output=True,
        )
        for key, value in (
            ("remote.origin.url", CANONICAL_REPOSITORY),
            ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
            ("branch.main.remote", "origin"),
            ("branch.main.merge", "refs/heads/main"),
        ):
            subprocess.run(
                [str(GIT), "config", "--file", str(clone / ".git/config"), key, value],
                env=environment,
                check=True,
            )
        with GitTransport() as transport:
            validate_disposable_clone(transport, clone)
            safe = transport.environment(discovery_ceiling=True)
            forbidden = {
                key
                for key in safe
                if key.startswith(("HTTP_", "HTTPS_", "ALL_PROXY", "GIT_CONFIG_COUNT"))
            }
            if forbidden:
                raise AssertionError(f"hermetic environment admitted {forbidden}")
            transport.prove_empty_cwd()

            def positive_rewrite(
                poison_environment: dict[str, str], *, cwd: Path = clone
            ) -> None:
                control_environment = dict(environment)
                control_environment.update(poison_environment)
                output = subprocess.run(
                    [
                        str(GIT),
                        "-c",
                        "protocol.file.allow=always",
                        "ls-remote",
                        CANONICAL_REPOSITORY,
                        "refs/heads/main",
                    ],
                    cwd=cwd,
                    env=control_environment,
                    check=True,
                    text=True,
                    capture_output=True,
                ).stdout
                if not output.startswith(wrong_main + "\trefs/heads/main"):
                    raise AssertionError(
                        "unisolated URL-rewrite positive control missed"
                    )
                caller = dict(os.environ)
                previous_cwd = Path.cwd()
                try:
                    os.environ.clear()
                    os.environ.update(control_environment)
                    os.chdir(cwd)
                    result = transport.run(
                        ["config", "--get-regexp", r"^url\."],
                        check=False,
                        discovery_ceiling=True,
                    )
                finally:
                    os.chdir(previous_cwd)
                    os.environ.clear()
                    os.environ.update(caller)
                if result.returncode == 0 or result.stdout:
                    raise AssertionError("ambient URL rewrite entered Git transport")

            system_config = root / "system.gitconfig"
            global_config = root / "global.gitconfig"
            for config in (system_config, global_config):
                config.write_text(
                    f'[url "file://{wrong_remote}"]\n\tinsteadOf = {CANONICAL_REPOSITORY}\n',
                    encoding="utf-8",
                )
            positive_rewrite(
                {
                    "GIT_CONFIG_NOSYSTEM": "0",
                    "GIT_CONFIG_SYSTEM": str(system_config),
                }
            )
            positive_rewrite({"GIT_CONFIG_GLOBAL": str(global_config)})
            positive_rewrite(
                {
                    "GIT_CONFIG_COUNT": "1",
                    "GIT_CONFIG_KEY_0": f"url.file://{wrong_remote}.insteadOf",
                    "GIT_CONFIG_VALUE_0": CANONICAL_REPOSITORY,
                }
            )

            local_config = clone / ".git/config"
            local_original = local_config.read_bytes()
            subprocess.run(
                [
                    str(GIT),
                    "config",
                    "--file",
                    str(local_config),
                    f"url.file://{wrong_remote}.insteadOf",
                    CANONICAL_REPOSITORY,
                ],
                env=environment,
                check=True,
            )
            positive_rewrite({})
            local_config.write_bytes(local_original)

            subprocess.run(
                [
                    str(GIT),
                    "config",
                    "--file",
                    str(local_config),
                    "extensions.worktreeConfig",
                    "true",
                ],
                env=environment,
                check=True,
            )
            worktree_config = clone / ".git/config.worktree"
            worktree_config.write_text(
                f'[url "file://{wrong_remote}"]\n\tinsteadOf = {CANONICAL_REPOSITORY}\n',
                encoding="utf-8",
            )
            positive_rewrite({})
            worktree_config.unlink()
            local_config.write_bytes(local_original)

            poison_environment = {
                "ALL_PROXY": "socks5://poison.invalid",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES": "/poison/objects",
                "GIT_CONFIG_COUNT": "1",
                "GIT_CONFIG_KEY_0": f"url.file://{root}/wrong.insteadOf",
                "GIT_CONFIG_VALUE_0": CANONICAL_REPOSITORY,
                "GIT_DIR": str(clone / ".git"),
                "GIT_OBJECT_DIRECTORY": "/poison/objects",
                "HTTPS_PROXY": "http://poison.invalid",
            }
            caller = dict(os.environ)
            try:
                os.environ.update(poison_environment)
                if any(
                    os.environ.get(key) != value
                    for key, value in poison_environment.items()
                ):
                    raise AssertionError("Git poison positive control is inert")
                isolated = transport.environment(discovery_ceiling=True)
            finally:
                os.environ.clear()
                os.environ.update(caller)
            if set(isolated) & set(poison_environment):
                raise AssertionError("caller Git/proxy poison entered transport")

            subprocess.run(
                [
                    str(GIT),
                    "config",
                    "--file",
                    str(clone / ".git/config"),
                    "url.file:///tmp/wrong.insteadof",
                    CANONICAL_REPOSITORY,
                ],
                env=environment,
                check=True,
            )
            try:
                validate_disposable_clone(transport, clone)
            except IntegrityError as exc:
                if exc.guard != "git-config":
                    raise
            else:
                raise AssertionError("local insteadOf poison was accepted")

            subprocess.run(
                [
                    str(GIT),
                    "config",
                    "--file",
                    str(clone / ".git/config"),
                    "--unset-all",
                    "url.file:///tmp/wrong.insteadof",
                ],
                env=environment,
                check=True,
            )
            subprocess.run(
                [
                    str(GIT),
                    "config",
                    "--file",
                    str(clone / ".git/config"),
                    "remote.origin.url",
                    f"file://{wrong_remote}",
                ],
                env=environment,
                check=True,
            )
            try:
                validate_disposable_clone(transport, clone)
            except IntegrityError as exc:
                if exc.guard != "git-config":
                    raise
            else:
                raise AssertionError("wrong origin URL was accepted")

        controlled_url = f"file://{wrong_remote}"
        controlled_clone = root / "controlled-capture"
        with GitTransport(
            controlled_url, allow_file_for_control=True
        ) as controlled_transport:
            controlled_transport.clone(controlled_clone)
            facts = controlled_transport.fetch_authority(controlled_clone)
            if len(facts.tags) != 66:
                raise AssertionError("controlled authoritative tag denominator differs")
        for index in range(66):
            subprocess.run(
                [
                    str(GIT),
                    "tag",
                    "-f",
                    f"v0.{index:03d}",
                    facts.capture_input,
                ],
                cwd=controlled_clone,
                env=environment,
                check=True,
                capture_output=True,
            )
        # Local v0 tags do not alter the already captured private authority facts.
        local_tags = subprocess.run(
            [str(GIT), "for-each-ref", "--format=%(refname)", "refs/tags/v0.*"],
            cwd=controlled_clone,
            env=environment,
            check=True,
            text=True,
            capture_output=True,
        ).stdout.splitlines()
        if len(local_tags) != 66 or len(facts.tags) != 66:
            raise AssertionError(
                "local-tag poison control did not exercise both namespaces"
            )

        poisoned_clone = root / "private-ref-poison"
        with GitTransport(
            controlled_url, allow_file_for_control=True
        ) as controlled_transport:
            controlled_transport.clone(poisoned_clone)
            controlled_transport.run(
                ["update-ref", f"{TAG_NAMESPACE}/v0.000", facts.capture_input],
                cwd=poisoned_clone,
            )
            try:
                controlled_transport.fetch_authority(poisoned_clone)
            except IntegrityError as exc:
                if exc.guard != "private-ref":
                    raise
            else:
                raise AssertionError("preexisting private authority ref was accepted")

        head_poison_clone = root / "head-poison"

        class HeadMutatingTransport(GitTransport):
            def ls_remote(self) -> tuple[str, dict[str, tuple[str, str]]]:
                result = super().ls_remote()
                self.run(["update-ref", "HEAD", wrong_head], cwd=head_poison_clone)
                return result

        with HeadMutatingTransport(
            controlled_url, allow_file_for_control=True
        ) as controlled_transport:
            controlled_transport.clone(head_poison_clone)
            try:
                controlled_transport.fetch_authority(head_poison_clone)
            except IntegrityError as exc:
                if exc.guard != "capture-input":
                    raise
            else:
                raise AssertionError("mid-run HEAD mutation was accepted")

        subprocess.run(
            [str(GIT), "tag", "v0.066", wrong_head],
            cwd=wrong_work,
            env=environment,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            [str(GIT), "push", str(wrong_remote), "refs/tags/v0.066"],
            cwd=wrong_work,
            env=environment,
            check=True,
            capture_output=True,
        )
        with GitTransport(
            controlled_url, allow_file_for_control=True
        ) as controlled_transport:
            try:
                controlled_transport.ls_remote()
            except IntegrityError as exc:
                if exc.guard != "remote-tag-count":
                    raise
            else:
                raise AssertionError("extra authoritative remote tag was accepted")

        external = root / "external"
        subprocess.run(
            [str(GIT), "init", "--bare", str(external)],
            env=environment,
            check=True,
            capture_output=True,
        )
        linked = root / "linked"
        linked.mkdir()
        (linked / ".git").symlink_to(external, target_is_directory=True)
        with GitTransport() as transport:
            try:
                validate_disposable_clone(transport, linked)
            except IntegrityError as exc:
                if exc.guard != "git-layout":
                    raise
            else:
                raise AssertionError("external .git symlink was accepted")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", required=True)
    args = parser.parse_args()
    if args.self_test:
        self_test()
        print("service-legacy Git isolation self-test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
