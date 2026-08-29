# Solstone install-integration test runner — see README.md for usage and
# operator runbook for the trade-off discussion.

IMAGE ?= solstone-systemd-test:latest
FEDORA_IMAGE ?= solstone-systemd-test:fedora

.PHONY: build build-fedora smoke install full legacy-upgrade legacy-upgrade-v1022 release-crossover-v1022-deb release-crossover-v1022-rpm shell clean rebuild

build:
	docker build -t $(IMAGE) .

build-fedora:
	docker build -f Dockerfile.fedora -t $(FEDORA_IMAGE) .

smoke: build
	IMAGE=$(IMAGE) ./run-test.sh smoke

install: build
	IMAGE=$(IMAGE) ./run-test.sh install


# Full path: also runs the model + Claude-skill installers. Slow.
full: build
	IMAGE=$(IMAGE) ./run-test.sh install full

# Upgrade-over-legacy-state path: seed a legacy non-symlink wrapper, then
# assert journal setup self-heals it through to a healthy service_identity.
legacy-upgrade: build
	IMAGE=$(IMAGE) ./run-test.sh legacy-upgrade

# Deterministic shaped-fixture proof of stop-before-publish ordering.
legacy-upgrade-v1022: build
	IMAGE=$(IMAGE) ./run-test.sh legacy-upgrade-v1022

# Public v1.0.22 wheels, native candidate package, real systemd service,
# failure/retry, lifecycle, downgrade, and teardown.
release-crossover-v1022-deb: build
	IMAGE=$(IMAGE) PACKAGE_FORMAT=deb ./run-release-crossover-v1022.sh

release-crossover-v1022-rpm: build-fedora
	IMAGE=$(FEDORA_IMAGE) PACKAGE_FORMAT=rpm ./run-release-crossover-v1022.sh

shell: build
	IMAGE=$(IMAGE) KEEP=1 ./run-test.sh shell

clean:
	-docker rm -f solstone-systemd-test-run 2>/dev/null
	-docker rm -f solstone-systemd-v1022 2>/dev/null
	-docker rmi $(IMAGE) 2>/dev/null
	-docker rmi $(FEDORA_IMAGE) 2>/dev/null

rebuild: clean build
