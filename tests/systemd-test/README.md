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
make install             # ~3-5min: tree install, then journal setup
make observer-ingest     # retired v2 migration fixture; not a linked-device test
make legacy-upgrade      # ~3-5min: install over a seeded legacy non-symlink wrapper
make release-crossover-v1022-deb # public v1.0.22 to candidate .deb crossover
make release-crossover-v1022-rpm # public v1.0.22 to candidate .rpm crossover
```

`smoke` installs a tiny `runner-smoke.service` and confirms systemd
`--user` accepts, enables, starts, and reports it active. Use it as a
fast pre-flight before chasing solstone-specific failures.

`install` runs the actual journal install path (the relocatable tree from
[INSTALL.md](../../INSTALL.md), then `journal setup -y --skip-models --skip-skills`) and
verifies the resulting `solstone.service` reaches `active` plus `journal
service status` reports a running service and live callosum clients. The command
may still return nonzero for the model degradation this cell creates on
purpose. `--skip-models / --skip-skills` are passed by default because Parakeet
/ Claude-skill installation is orthogonal to the systemd question; use `make
full` to drop those flags.

`observer-ingest` is a retired v2 migration fixture. It invokes
`/app/observer/register` and `/app/observer/ingest`, neither of which is a
supported linked-device upload route. Do not use it as a product or release
gate. Supported clients pair first and use the protocol-v3 linked-device
contract with mTLS.
The fixture remains only as a record of the package-data check that needs a
linked-device replacement.

For a pre-publish local tree gate, produce the linux-x86_64 artifacts with
`solstone-distribution` and point the runner at that directory. The wheel
path this test used to drive is retired; see [INSTALL.md](../../INSTALL.md).

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
running either target. Each target installs the pinned public v1.0.22 wheel,
starts its real user service, installs the candidate native package through the
system package manager, and returns to the ordinary owner for setup and
lifecycle checks. Those checks cover failure/retry, preservation, service
takeover, logout/reboot, uninstall, downgrade, process cleanup, and teardown.
The Debian target uses the `.deb`; the Fedora target uses the `.rpm`.

## the worked example

```bash
docker build -t solstone-systemd-test .
./run-test.sh install
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

`KEEP=1 ./run-test.sh install` runs the test then leaves the container
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
portable way to grant that. On modern Docker (≥20.10) with a cgroup-v2
host (Fedora 31+, Debian 11+, Ubuntu 21.10+, Arch), the same setup works
with just `--cgroupns=host`, `CAP_SYS_ADMIN`, and a bind-mount of
`/sys/fs/cgroup`; that is what `PRIVILEGED=0` uses. Run the less-
privileged path first on a new host; fall back to `--privileged` if you
hit cgroup write-permission errors.

## what this does NOT model

- No graphical login session (no `pam_systemd` running for real, no
  `user@<uid>.service` graph populated by GUI login)
- No real journald persistence across container restarts
- No NetworkManager, no resolved as the resolver
- No hardware-backed secure enclaves, no TPM
- No real PL-networked observer client or tunnel. `observer-ingest` uses
  the real register and ingest HTTP routes, but the client is a loopback
  `curl` payload inside the container.
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
| `run-test.sh` | `smoke` / `install` / `observer-ingest` / `legacy-upgrade` / `shell` modes. Drives the boot-wait, runs the install, asserts readiness. |
| `run-release-crossover-v1022.sh` | Runs the public-v1 package crossover in a disposable Debian or Fedora container. |
| `Makefile`    | Build, install, crossover, debugging, and cleanup targets. |
| `README.md`   | This file.                                                                |
