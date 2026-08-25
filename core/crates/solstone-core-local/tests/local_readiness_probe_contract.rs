// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(target_os = "linux")]
mod linux {
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Mutex;

    use serde_json::{Map, Value};
    use solstone_core_local::install::readiness::inspect_local;

    static PATH_LOCK: Mutex<()> = Mutex::new(());

    struct ProbePathGuard {
        path: Option<OsString>,
        receipt: Option<OsString>,
    }

    impl ProbePathGuard {
        fn install(directory: &Path, receipt: &Path) -> Self {
            let path = env::var_os("PATH");
            let mut paths = vec![directory.to_path_buf()];
            if let Some(current) = &path {
                paths.extend(env::split_paths(current));
            }
            let receipt_value = env::var_os("SOLSTONE_TEST_NVIDIA_RECEIPT");
            // SAFETY: PATH is process-global; this dedicated target has one
            // PATH-mutating test guarded for its full lifetime by PATH_LOCK.
            unsafe {
                env::set_var("PATH", env::join_paths(paths).expect("PATH joins"));
                env::set_var("SOLSTONE_TEST_NVIDIA_RECEIPT", receipt);
            }
            Self {
                path,
                receipt: receipt_value,
            }
        }
    }

    impl Drop for ProbePathGuard {
        fn drop(&mut self) {
            // SAFETY: the matching install call holds PATH_LOCK until this
            // guard drops, so no test in this target can race restoration.
            unsafe {
                match self.path.take() {
                    Some(value) => env::set_var("PATH", value),
                    None => env::remove_var("PATH"),
                }
                match self.receipt.take() {
                    Some(value) => env::set_var("SOLSTONE_TEST_NVIDIA_RECEIPT", value),
                    None => env::remove_var("SOLSTONE_TEST_NVIDIA_RECEIPT"),
                }
            }
        }
    }

    #[test]
    fn inspect_local_reaches_the_real_nvidia_probe_subprocess() {
        let _path_lock = PATH_LOCK.lock().expect("PATH lock");
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
        let _path = ProbePathGuard::install(&bin, &receipt);
        let journal = root.path().join("journal");
        fs::create_dir(&journal).expect("journal creates");

        let _ = inspect_local(Map::from_iter([(
            "journal".to_owned(),
            Value::String(journal.display().to_string()),
        )]));

        assert!(receipt.exists(), "inspect_local did not reach nvidia-smi");
    }
}
