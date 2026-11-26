// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use solstone_core_distribution::cleanroom::bind_loopback;

const CONNECT_DEADLINE: Duration = Duration::from_secs(2);

#[test]
fn returned_loopback_port_is_owned_and_reachable_without_rebinding() {
    let (listener, reported_port) = bind_loopback().expect("bind cleanroom loopback listener");
    let owned = listener
        .local_addr()
        .expect("inspect owned listener address");
    assert_eq!(owned.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(owned.port(), 0);
    assert_eq!(reported_port, owned.port());

    let reported = SocketAddr::from((Ipv4Addr::LOCALHOST, reported_port));
    let client = TcpStream::connect_timeout(&reported, CONNECT_DEADLINE)
        .expect("connect to the exact reported address");
    assert_eq!(client.peer_addr().expect("client peer address"), owned);

    listener.set_nonblocking(true).expect("make accept bounded");
    // A completed connect can precede delivery to Darwin's accept queue.
    let deadline = Instant::now() + CONNECT_DEADLINE;
    let (accepted, peer) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "accept exact client: {error}");
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("accept exact client: {error}"),
        }
    };
    assert_eq!(
        accepted.local_addr().expect("accepted local address"),
        owned
    );
    assert_eq!(accepted.peer_addr().expect("accepted peer address"), peer);
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

#[cfg(target_os = "linux")]
#[test]
fn acquire_builder_rootfs_normalization_is_independent_of_caller_umask() {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("distribution/acquire-builder.sh");
    let temp = tempfile::tempdir().expect("tempdir");

    // Synthetic exported rootfs containing group- and world-writable members,
    // mirroring a Debian container export where member bits exceed 0755/0644.
    let src = temp.path().join("src");
    std::fs::create_dir_all(src.join("usr/local/bin")).expect("mkdir");
    std::fs::set_permissions(
        src.join("usr/local"),
        std::fs::Permissions::from_mode(0o775),
    )
    .expect("chmod dir");
    std::fs::write(src.join("usr/local/bin/tool"), b"#!/bin/sh\n").expect("write script");
    std::fs::set_permissions(
        src.join("usr/local/bin/tool"),
        std::fs::Permissions::from_mode(0o775),
    )
    .expect("chmod script");
    std::fs::write(src.join("etc-sample.conf"), b"value=1\n").expect("write file");
    std::fs::set_permissions(
        src.join("etc-sample.conf"),
        std::fs::Permissions::from_mode(0o664),
    )
    .expect("chmod file");
    let raw_tar = temp.path().join("raw.tar");
    let tar_status = Command::new("tar")
        .args([
            "-C",
            src.to_str().unwrap(),
            "-cf",
            raw_tar.to_str().unwrap(),
            ".",
        ])
        .status()
        .expect("create synthetic raw.tar");
    assert!(tar_status.success(), "tar create failed");

    let fake_dist = temp.path().join("fake-dist");
    std::fs::write(&fake_dist, "#!/bin/sh\nexit 0\n").expect("write fake-dist");
    std::fs::set_permissions(&fake_dist, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake-dist");

    let fake_docker = temp.path().join("fake-docker");
    std::fs::write(
        &fake_docker,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  build) exit 0 ;;\n  create) echo fake-cid ;;\n  export) cat '{}' ;;\n  rm) exit 0 ;;\n  *) exit 1 ;;\nesac\n",
            raw_tar.display()
        ),
    )
    .expect("write fake-docker");
    std::fs::set_permissions(&fake_docker, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake-docker");

    let mut outputs = Vec::new();
    for mask in ["0002", "0022", "0077"] {
        let work = temp.path().join(format!("work-{mask}"));
        let output = Command::new("sh")
            .args([
                "-c",
                &format!("umask {mask} && exec sh '{}'", script.display()),
            ])
            .env("SOLSTONE_CONTAINER_RUNTIME", &fake_docker)
            .env("SOLSTONE_DISTRIBUTION_BIN", &fake_dist)
            .env("SOLSTONE_BUILDER_WORKDIR", &work)
            .output()
            .expect("run acquire-builder.sh");
        // Synthetic tar will fail the real pin check (exit 2) after writing builder-rootfs.tar.
        assert_eq!(
            output.status.code(),
            Some(2),
            "expected pin mismatch exit 2 for umask {mask}, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rootfs = std::fs::read(work.join("builder-rootfs.tar"))
            .expect("builder-rootfs.tar written before pin check");
        outputs.push((mask, rootfs));
    }

    assert_eq!(
        outputs[0].1, outputs[1].1,
        "umask 0002 and 0022 must produce byte-identical builder-rootfs.tar"
    );
    assert_eq!(
        outputs[1].1, outputs[2].1,
        "umask 0022 and 0077 must produce byte-identical builder-rootfs.tar"
    );
}
