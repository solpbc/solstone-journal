# solstone-systemd-test

Docker image that runs `systemd --user` end-to-end so the solstone
install-integration suite can verify that `journal setup` actually starts the
user service — not just that the unit file got written to
`~/.config/systemd/user/solstone.service`.

For trade-offs (privilege requirements, host-kernel assumptions, what
this image does NOT model, and the CI-runner path forward) see the
operational playbook in the sol pbc org repo,
`operator runbook`. Read that first.

## quick start

```bash
make build               # build the image
make smoke               # ~30s — verifies systemd --user works end-to-end
make install             # ~3-5min — tree install, then journal setup
make observer-ingest     # retired v2 migration fixture; not a linked-device test
make legacy-upgrade      # ~3-5min — install over a seeded legacy non-symlink wrapper
```

`smoke` installs a tiny `runner-smoke.service` and confirms systemd
`--user` accepts, enables, starts, and reports it active. Use it as a
fast pre-flight before chasing solstone-specific failures.

`install` runs the actual journal install path (the relocatable tree from
[INSTALL.md](../../INSTALL.md), then `journal setup -y --skip-models --skip-skills`) and
verifies the resulting `solstone.service` reaches `active` plus `journal
service status` returns 0. `--skip-models / --skip-skills` are passed by default because
Parakeet / Claude-skill installation is orthogonal to
the systemd question; use `make full` to drop those flags.

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
up — useful when an install step fails and you want to poke at it.

## environment knobs

| Variable     | Default                       | What it does                                                            |
|--------------|-------------------------------|-------------------------------------------------------------------------|
| `IMAGE`      | `solstone-systemd-test:latest`| Image tag.                                                              |
| `CONTAINER`  | `solstone-systemd-test-run`   | Container name (so parallel runs need distinct names).                  |
| `TEST_USER`  | `solstone`                    | Non-root user inside the image. Matches the Dockerfile `TEST_USER` arg. |
| `PRIVILEGED` | `1`                           | `0` switches to the less-privileged path (cgroup-v2 host namespace + `CAP_SYS_ADMIN` + apparmor=unconfined). |
| `KEEP`       | `0`                           | `1` leaves the container up on success for inspection.                  |
| `SOLSTONE_DIST_DIR` | unset                  | Host directory of produced `linux-x86_64` artifacts, mounted at `/artifacts`. Required for `install`, `observer-ingest`, and `legacy-upgrade`. Must contain `solstone-journal-*-linux-x86_64.deb`. |

## why `--privileged`

Booting `systemd` as PID 1 inside a container needs read-write access to
the cgroup hierarchy and a few capabilities (`CAP_SYS_ADMIN`, etc.) that
the default Docker profile denies. `--privileged` is the simplest, most
portable way to grant that. On modern Docker (≥20.10) with a cgroup-v2
host (Fedora 31+, Debian 11+, Ubuntu 21.10+, Arch), the same setup works
with just `--cgroupns=host`, `CAP_SYS_ADMIN`, and a bind-mount of
`/sys/fs/cgroup` — that's what `PRIVILEGED=0` uses. Run the less-
privileged path first on a new host; fall back to `--privileged` if you
hit cgroup write-permission errors.

See `operator runbook` (org repo) § trade-offs for the full
discussion including the rootless-podman, distrobox, and lima
alternatives.

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
  (`DEFAULT_SERVICE_PORT`) is the convey Flask app — login, `/init`,
  `/app/today`, etc. — and is reachable from the container, but there
  is no explicit `/health` route there. The authoritative readiness
  probe is `journal service status`, which talks to the callosum Unix
  socket at `<journal>/health/callosum.sock`. The runner uses that
  probe instead of `curl http://localhost:5015/health` (the request
  body's shorthand).

## file inventory

| File          | What it is                                                                |
|---------------|---------------------------------------------------------------------------|
| `Dockerfile`  | Debian 12 (bookworm) base, full systemd, dbus-user-session, pre-lingered non-root user, uv pre-installed. |
| `run-test.sh` | `smoke` / `install` / `observer-ingest` / `legacy-upgrade` / `shell` modes. Drives the boot-wait, runs the install, asserts readiness. |
| `Makefile`    | `build` / `smoke` / `install` / `observer-ingest` / `full` / `legacy-upgrade` / `shell` / `clean` / `rebuild`. |
| `README.md`   | This file.                                                                |
