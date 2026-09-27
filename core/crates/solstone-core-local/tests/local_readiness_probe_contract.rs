// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(target_os = "linux")]
mod linux {
    use std::env;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use serde_json::{Map, Value};
    use solstone_core_local::install::readiness::inspect_local;

    #[test]
    fn probe_child() {
        let Some(journal) = env::var_os("SOLSTONE_TEST_NVIDIA_JOURNAL") else {
            return;
        };
        let _ = inspect_local(Map::from_iter([(
            "journal".to_owned(),
            Value::String(journal.to_string_lossy().into_owned()),
        )]));
    }

    #[test]
    fn inspect_local_reaches_the_real_nvidia_probe_subprocess() {
        let root = tempfile::Builder::new()
            .prefix("solstone-local-readiness-probe-")
            .tempdir_in("/var/tmp")
            .expect("temporary root");
        let bin = root.path().join("bin");
        let receipt = root.path().join("nvidia-smi-receipt");
        fs::create_dir(&bin).expect("fake bin creates");
        let shim = bin.join("nvidia-smi");
        fs::write(
            &shim,
            "#!/bin/sh\nprintf '%s\\n' \"$$\" >> \"$SOLSTONE_TEST_NVIDIA_RECEIPT\"\nexit 0\n",
        )
        .expect("fake nvidia-smi writes");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("fake nvidia-smi is executable");
        let journal = root.path().join("journal");
        fs::create_dir(&journal).expect("journal creates");

        let mut paths = vec![bin];
        if let Some(current) = env::var_os("PATH") {
            paths.extend(env::split_paths(&current));
        }
        let child = Command::new(env::current_exe().expect("test executable"))
            .args(["--exact", "linux::probe_child"])
            .env("PATH", env::join_paths(paths).expect("PATH joins"))
            .env("SOLSTONE_TEST_NVIDIA_RECEIPT", &receipt)
            .env("SOLSTONE_TEST_NVIDIA_JOURNAL", &journal)
            .output()
            .expect("probe child runs");
        assert!(
            child.status.success(),
            "probe child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );

        assert!(receipt.exists(), "inspect_local did not reach nvidia-smi");
    }
}
