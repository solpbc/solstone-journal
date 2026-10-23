// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, io, os::unix::fs::FileTypeExt, path::Path};

/// Called only after retirement settles and before a new serving owner exists.
/// Keep the journal and its identity in place; only transient IPC uses the
/// shorter app-private files directory, as on the other mobile embedding.
pub(crate) fn prepare(root: &Path) -> Result<(), String> {
    let files = root.parent().ok_or("app files directory missing")?;
    let target = files.join("callosum.sock");
    if !target.is_absolute() || target.as_os_str().len() >= 104 {
        return Err("app-private Callosum target exceeds Unix socket path bound".into());
    }
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_socket() => {}
        Ok(_) => return Err("app-private Callosum target has another binding".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let health = root.join("health");
    match fs::symlink_metadata(&health) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err("journal health directory has another binding".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&health).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    let alias = health.join("callosum.sock");
    match fs::symlink_metadata(&alias) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(&alias).map_err(|error| error.to_string())? != target {
                return Err("Callosum endpoint already has another binding".into());
            }
        }
        Ok(_) => return Err("Callosum endpoint already has another binding".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(&target, &alias).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "android-owner-endpoint-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn journal(&self) -> std::path::PathBuf {
            let root = self.0.join("journal".repeat(12));
            fs::create_dir(&root).unwrap();
            root
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn long_journal_keeps_its_files_and_reaches_the_short_private_endpoint() {
        let directory = Directory::new();
        let root = directory.journal();
        fs::write(root.join("original"), b"preserved").unwrap();
        prepare(&root).unwrap();
        let alias = root.join("health/callosum.sock");
        assert!(alias.as_os_str().len() >= 104);
        let target = fs::read_link(&alias).unwrap();
        let listener = UnixListener::bind(&target).unwrap();
        let _client = UnixStream::connect(&target).unwrap();
        let _peer = listener.accept().unwrap();
        prepare(&root).unwrap();
        assert_eq!(fs::read(root.join("original")).unwrap(), b"preserved");
        assert_eq!(fs::read_link(&alias).unwrap(), target);
    }

    #[test]
    fn conflicting_endpoint_or_target_is_preserved() {
        for conflict in ["endpoint", "target"] {
            let directory = Directory::new();
            let root = directory.journal();
            fs::create_dir(root.join("health")).unwrap();
            let path = if conflict == "endpoint" {
                root.join("health/callosum.sock")
            } else {
                directory.0.join("callosum.sock")
            };
            fs::write(&path, b"keep").unwrap();
            assert!(prepare(&root).is_err());
            assert_eq!(fs::read(path).unwrap(), b"keep");
        }
    }

    #[test]
    fn unexpected_alias_and_health_link_are_preserved() {
        let directory = Directory::new();
        let root = directory.journal();
        fs::create_dir(root.join("health")).unwrap();
        let alias = root.join("health/callosum.sock");
        std::os::unix::fs::symlink("another", &alias).unwrap();
        assert!(prepare(&root).is_err());
        assert_eq!(fs::read_link(&alias).unwrap(), Path::new("another"));
        fs::remove_file(alias).unwrap();
        fs::remove_dir(root.join("health")).unwrap();
        std::os::unix::fs::symlink(&directory.0, root.join("health")).unwrap();
        assert!(prepare(&root).is_err());
        assert!(
            fs::symlink_metadata(root.join("health"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
