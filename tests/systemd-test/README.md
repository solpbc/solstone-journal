# solstone-systemd-test

Docker image that runs `systemd --user` end-to-end so the solstone
install-integration suite can verify that `journal setup` actually starts the
user service, rather than only checking that the unit file got written to
`~/.config/systemd/user/solstone.service`.

For privilege requirements, host-kernel assumptions, and test limits, see
[why `--privileged`](#why---privileged) and [what this does NOT
model](#what-this-does-not-model) below.

## quick start

```bash
make build               # build the image
make smoke               # ~30s: verifies systemd --user works end-to-end
SOLSTONE_DIST_DIR=/var/tmp/<outdir>/linux-x86_64 make install # ~3-5min: install candidate .deb, then journal setup
make legacy-upgrade      # ~3-5min: install over a seeded legacy non-symlink wrapper
make release-crossover-v1022-deb # public v1.0.22 to candidate .deb crossover
make release-crossover-v1022-rpm # public v1.0.22 to candidate .rpm crossover
```

`smoke` installs a tiny `runner-smoke.service` and confirms systemd
`--user` accepts, enables, starts, and reports it active. Use it as a
fast pre-flight before chasing solstone-specific failures.

`install` installs the newest candidate `.deb` from `SOLSTONE_DIST_DIR`, then runs `journal setup -y --skip-models --skip-skills` and
verifies the resulting `solstone.service` reaches `active` plus `journal
service status` reports a running service and live callosum clients. The command
may still return nonzero for the model degradation this cell creates on
purpose. `--skip-models` / `--skip-skills` are passed by default because Parakeet
/ Claude-skill installation is orthogonal to the systemd question; use `make
full` to drop those flags.

Linked-device ingest is not a systemd-test mode. It requires a paired client certificate, so use `tools/journal_device_sim` with a disposable journal for protocol-v3 ingest and reconciliation proof.

For a pre-publish local tree gate, produce the Linux x86_64 package artifacts
and point the runner at that directory. The wheel path this test used to drive
is retired.

`legacy-upgrade` installs V2 over an exact V1 runtime shape: generated
`solstone` and `sol` console scripts, a running historical systemd unit, and a
schema-1 setup manifest. One `journal setup` invocation must stop the old PID
before replacing the unit, publish the V2 wrappers and service, retain the V1
environment as recovery material, and create durable wrapper backups under
`~/.local/share/solstone/setup-backups/`. The test inventories every
pre-existing journal artifact outside setup's closed write set and verifies
that the inventory is byte-for-byte identical afterward. It finishes with the
same live service and `service_identity` checks as the clean-install path.

The `release-crossover-v1022-*` targets are the native-package reference gate.
Set `SOLSTONE_DIST_DIR` to a produced `linux-x86_64` artifact directory before
running either target. Each target starts from its baseline package, installs the candidate native
package through the system package manager, and returns to the ordinary owner for setup and
lifecycle checks. Those checks cover failure/retry, preservation, service takeover, logout/reboot,
uninstall, downgrade, process cleanup, and teardown. The Debian target uses the `.deb`; the
Fedora target uses the `.rpm`.

## the worked example

```bash
docker build -t solstone-systemd-test .
SOLSTONE_DIST_DIR=/var/tmp/<outdir>/linux-x86_64 ./run-test.sh install
```

Internally that runs (as the non-root `solstone` user inside a
`--privileged` container that booted `/sbin/init` to PID 1):

```bash
# install the linux-x86_64 tree, then:
journal setup -y --skip-models --skip-skills
test -f ~/.config/systemd/user/solstone.service
systemctl --user is-active solstone        # → active
journal service status                      # → exit 0
```

## interactive debugging

```bash
make shell                                  # opens a user shell in the running container
# inside: systemctl --user status solstone, journalctl --user -u solstone, etc.
```

`KEEP=1 SOLSTONE_DIST_DIR=/var/tmp/<outdir>/linux-x86_64 ./run-test.sh install` runs the test then leaves the container
up. This is useful when an install step fails and you want to inspect it.

## environment knobs

| Variable     | Default                       | What it does                                                            |
|--------------|-------------------------------|-------------------------------------------------------------------------|
| `IMAGE`      | `solstone-systemd-test:latest`| Image tag.                                                              |
| `CONTAINER`  | `solstone-systemd-test-run`   | Container name (so parallel runs need distinct names).                  |
| `TEST_USER`  | `solstone`                    | Non-root user inside the image. Matches the Dockerfile `TEST_USER` arg. |
| `PRIVILEGED` | `1`                           | `0` switches to the less-privileged path (cgroup-v2 host namespace + `CAP_SYS_ADMIN` + apparmor=unconfined). |
| `KEEP`       | `0`                           | `1` leaves the container up on success for inspection.                  |
| `SOLSTONE_DIST_DIR` | unset                  | Host directory of produced `linux-x86_64` artifacts, mounted at `/artifacts`. Required for package-backed tests. Must contain the candidate `.deb` or `.rpm` selected by the target. |

## why `--privileged`

Booting `systemd` as PID 1 inside a container needs read-write access to
the cgroup hierarchy and a few capabilities (`CAP_SYS_ADMIN`, etc.) that
the default Docker profile denies. `--privileged` is the simplest, most
portable way. The reduced-privilege path uses `--cgroupns=host`,
`CAP_SYS_ADMIN`, and a bind-mount of `/sys/fs/cgroup`; that is what
`PRIVILEGED=0` uses. It is host-sensitive, so run that path on the target host
before relying on it and fall back to `--privileged` if it hits cgroup
write-permission errors.

## what this does NOT model

- No graphical login session (no `pam_systemd` running for real, no
  `user@<uid>.service` graph populated by GUI login)
- No real journald persistence across container restarts
- No NetworkManager, no resolved as the resolver
- No hardware-backed secure enclaves, no TPM
- No linked-device ingest client. This harness proves package installation and user-systemd readiness; protocol-v3 ingest requires a paired client certificate. Use `tools/journal_device_sim` with a disposable journal for ingest and reconciliation proof.
- 7657 (the mutual-TLS pairing/sync surface) is bound by the convey
  secure_listener (`solstone/solstone/convey/secure_listener/accept.py:41`)
  for device-to-device pairing/sync. Plain HTTP on 5015
  (`DEFAULT_SERVICE_PORT`) is the convey Flask app for login, `/init`,
  `/app/today`, and similar routes. It is reachable from the container, but there
  is no explicit `/health` route there. The authoritative readiness
  probe is `journal service status`, which talks to the callosum Unix
  socket at `<journal>/health/callosum.sock`. The runner uses that
  probe instead of `curl http://localhost:5015/health` (the request
  body's shorthand).

## file inventory

| File          | What it is                                                                |
|---------------|---------------------------------------------------------------------------|
| `Dockerfile`  | Debian 12 (bookworm) base, full systemd, dbus-user-session, pre-lingered non-root user, uv pre-installed. |
| `Dockerfile.fedora` | Fedora base for the native `.rpm` crossover. |
| `run-test.sh` | `smoke` / `install` / `legacy-upgrade` / `shell` modes. Drives the boot-wait, runs the package install, asserts readiness. |
| `run-release-crossover-v1022.sh` | Runs the public-v1 package crossover in a disposable Debian or Fedora container. |
| `Makefile`    | Build, install, crossover, debugging, and cleanup targets. |
| `README.md`   | This file.                                                                |
