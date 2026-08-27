# Solstone install-integration test runner — see README.md for usage and
# operator runbook for the trade-off discussion.

IMAGE ?= solstone-systemd-test:latest

.PHONY: build smoke install observer-ingest full legacy-upgrade legacy-upgrade-v1022 shell clean rebuild

build:
	docker build -t $(IMAGE) .

smoke: build
	IMAGE=$(IMAGE) ./run-test.sh smoke

install: build
	IMAGE=$(IMAGE) ./run-test.sh install

observer-ingest: build
	IMAGE=$(IMAGE) ./run-test.sh observer-ingest

# Full path: also runs the model + Claude-skill installers. Slow.
full: build
	IMAGE=$(IMAGE) ./run-test.sh install full

# Upgrade-over-legacy-state path: seed a legacy non-symlink wrapper, then
# assert journal setup self-heals it through to a healthy service_identity.
legacy-upgrade: build
	IMAGE=$(IMAGE) ./run-test.sh legacy-upgrade

# Same crossover proof, pinned to the actual shipped v1.0.22 shape (native
# sol/solstone root-launchers, a separate journal console-script, a
# Type=notify unit) rather than the older shape `legacy-upgrade` covers.
# NOT wired into the top-level `check-systemd-test` gate: it currently and
# correctly fails, because it caught a real gap rather than a fixture bug --
# see its own precondition-check log line and the commit that added it.
legacy-upgrade-v1022: build
	IMAGE=$(IMAGE) ./run-test.sh legacy-upgrade-v1022

shell: build
	IMAGE=$(IMAGE) KEEP=1 ./run-test.sh shell

clean:
	-docker rm -f solstone-systemd-test-run 2>/dev/null
	-docker rmi $(IMAGE) 2>/dev/null

rebuild: clean build
