// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{fs, io::Write};

static COPY: OnceLock<PathBuf> = OnceLock::new();

/// Stable copy of the Cargo-built fixture. Parallel Cargo jobs may relink the
/// original executable while another integration test is starting it, which
/// produces ETXTBSY on Unix.
pub fn path() -> PathBuf {
    COPY.get_or_init(copy_fixture).clone()
}

pub fn string() -> String {
    path().to_string_lossy().into_owned()
}

fn copy_fixture() -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "solstone-system-test-child-{}-{}",
        std::process::id(),
        module_path!().replace("::", "-")
    ));
    fs::create_dir_all(&directory).expect("fixture copy directory");
    let destination = directory.join("solstone-system-test-child");
    let source = Path::new(env!("CARGO_BIN_EXE_solstone-system-test-child"));
    let bytes = fs::read(source).expect("read Cargo fixture");
    let mut destination_file = fs::File::create(&destination).expect("create stable fixture copy");
    destination_file
        .write_all(&bytes)
        .expect("write stable fixture copy");
    destination_file
        .sync_all()
        .expect("sync stable fixture copy");
    drop(destination_file);
    fs::set_permissions(
        &destination,
        fs::metadata(source)
            .expect("fixture metadata")
            .permissions(),
    )
    .expect("fixture permissions");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match std::process::Command::new(&destination)
            .arg("lines")
            .env_remove("SOLSTONE_JOURNAL")
            .output()
        {
            Ok(output) if output.status.success() => break,
            Err(error)
                if error.raw_os_error() == Some(26) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(output) => panic!("fixture copy probe exited with {}", output.status),
            Err(error) => panic!("fixture copy probe failed: {error}"),
        }
    }
    destination
}

#[cfg(unix)]
#[allow(dead_code)]
pub struct TestGeneration {
    _coordinator: solstone_core_system::lifecycle::ParentLossCoordinator,
}

#[cfg(unix)]
#[allow(dead_code)]
impl TestGeneration {
    pub fn admit(journal: &Path) -> Self {
        use solstone_core_system::lifecycle::{
            CoordinatorBootstrap, DeclaredParent, ParentLossCoordinator,
        };

        let parent = DeclaredParent::capture_current().expect("test parent identity");
        let (coordinator, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: journal.to_path_buf(),
            supervisor: parent.instance(),
            enabled: Vec::new(),
            supervisor_heartbeat_filename: format!(
                "solstone-v2-system-test-{}.check",
                std::process::id()
            ),
            capability: format!(
                "system-test-capability-{}-{}",
                std::process::id(),
                journal.display()
            )
            .into_bytes(),
        })
        .expect("admitting test generation");
        Self {
            _coordinator: coordinator,
        }
    }
}
