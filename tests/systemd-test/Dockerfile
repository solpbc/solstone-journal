# Debian-based image that runs systemd --user end-to-end so the solstone
# install-integration test can verify that `journal setup` actually starts the
# user service, not just that the unit file was written. See the playbook
# at operator runbook for trade-offs and limits.

FROM debian:bookworm

ENV container=docker
ENV LC_ALL=C
ENV DEBIAN_FRONTEND=noninteractive

# Full systemd (not systemd-sysv-only) + dbus-user-session is required for
# per-user systemd instances to run without a GUI login. python3 is the
# install target. curl + ca-certificates fetch uv. sudo is for any setup
# step that escalates (journal setup does not, but having it avoids surprises).
RUN apt-get update && apt-get install -y --no-install-recommends \
        systemd \
        systemd-sysv \
        dbus \
        dbus-user-session \
        libsystemd0 \
        python3 \
        python3-pip \
        python3-venv \
        curl \
        ca-certificates \
        sudo \
        procps \
        less \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Units that don't apply inside a container — masking keeps `systemctl
# is-system-running` from going into degraded state for irrelevant reasons.
RUN systemctl mask \
        sys-kernel-config.mount \
        sys-kernel-debug.mount \
        sys-kernel-tracing.mount \
        dev-hugepages.mount \
        dev-mqueue.mount \
        systemd-logind.service \
        getty.target \
        console-getty.service \
        systemd-firstboot.service \
        kmod-static-nodes.service \
        systemd-modules-load.service \
        systemd-udevd.service \
        systemd-udev-trigger.service

ARG TEST_USER=solstone
ARG TEST_UID=1000
RUN useradd -m -u ${TEST_UID} -s /bin/bash ${TEST_USER} \
    && echo "${TEST_USER} ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/${TEST_USER}

# loginctl enable-linger needs to run AFTER the user instance manager exists,
# which means after PID 1 boot. Pre-touching the linger marker file is the
# documented equivalent that survives a fresh `docker run`.
RUN mkdir -p /var/lib/systemd/linger \
    && touch /var/lib/systemd/linger/${TEST_USER}

# uv as the test user, so `uv tool install solstone-journal` is the same
# host-install path the install-integration tests are meant to exercise.
USER ${TEST_USER}
WORKDIR /home/${TEST_USER}
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV PATH="/home/solstone/.local/bin:${PATH}"

# /run/user/<uid> + XDG_RUNTIME_DIR are normally provisioned by pam_systemd
# on a real login. Inside the container the user-instance manager will set
# them up itself once linger is on, but the shell that we `docker exec` into
# does NOT inherit them automatically — we set XDG_RUNTIME_DIR in profile
# so `systemctl --user` works from any exec without re-deriving it.
RUN echo 'export XDG_RUNTIME_DIR="/run/user/$(id -u)"' >> /home/${TEST_USER}/.profile \
    && echo 'export XDG_RUNTIME_DIR="/run/user/$(id -u)"' >> /home/${TEST_USER}/.bashrc

USER root

# systemd's documented Docker stop signal.
STOPSIGNAL SIGRTMIN+3

CMD ["/sbin/init"]
