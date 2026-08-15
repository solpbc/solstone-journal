# Solstone install-integration test runner — see README.md for usage and
# operator runbook for the trade-off discussion.

IMAGE ?= solstone-systemd-test:latest

.PHONY: build smoke install observer-ingest full legacy-upgrade shell clean rebuild

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

shell: build
	IMAGE=$(IMAGE) KEEP=1 ./run-test.sh shell

clean:
	-docker rm -f solstone-systemd-test-run 2>/dev/null
	-docker rmi $(IMAGE) 2>/dev/null

rebuild: clean build
