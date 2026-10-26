// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private source snapshots bind a preview to the bytes and dates the producer reads.

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// Includes dates because image, document and note placement can depend on them.
/// Refuses links and special files; directory traversal has one stable order.
pub fn fingerprint(source: &Path) -> io::Result<String> {
    let mut hash = Sha256::new();
    hash_tree(source, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn children(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut children = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    children.sort();
    Ok(children)
}

fn hash_tree(path: &Path, hash: &mut Sha256) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !(meta.is_file() || meta.is_dir()) {
        return Err(io::Error::other(
            "import sources must contain only regular files and folders",
        ));
    }
    if meta.is_dir() {
        hash.update(b"directory");
        for child in children(path)? {
            let name = child
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| io::Error::other("an import filename is not UTF-8"))?;
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            hash_tree(&child, hash)?;
        }
    } else {
        hash.update(b"file");
        hash.update(meta.len().to_le_bytes());
        let modified = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        hash.update(modified.as_secs().to_le_bytes());
        hash.update(modified.subsec_nanos().to_le_bytes());
        let mut file = fs::File::open(path)?;
        let mut buffer = [0; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
    }
    Ok(())
}

/// Snapshot under a staged import, leaving no journal recording or spending permission.
/// A new copy is removed unless its preview record is committed successfully.
pub struct Snapshot {
    pub relative: PathBuf,
    pub digest: String,
    temporary: Option<tempfile::TempDir>,
}

// Metadata carries logical slash paths; native separators are not wire syntax.
fn portable_relative(path: &Path) -> Result<PathBuf, String> {
    let parts = path
        .components()
        .map(|part| match part {
            std::path::Component::Normal(name) => name.to_str().ok_or("invalid snapshot path"),
            _ => Err("invalid snapshot path"),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PathBuf::from(parts.join("/")))
}

impl Snapshot {
    pub fn persist(mut self) {
        if let Some(temporary) = self.temporary.take() {
            let _ = temporary.keep();
        }
    }

    /// Delete a fresh temporary directory on this thread.
    ///
    /// A reused snapshot has no temporary directory. This does not use [`Drop`].
    pub fn delete_unkept(mut self) {
        if let Some(temporary) = self.temporary.take() {
            drop(temporary);
        }
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Some(temporary) = self.temporary.take() {
            // A cancelled HTTP preview may own a large tree. Never delete it on
            // the executor that serves supervisor and owner requests.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn_blocking(move || drop(temporary));
            } else {
                drop(temporary);
            }
        }
    }
}

pub fn create(journal: &Path, import_id: &str, source: &Path) -> Result<Snapshot, String> {
    let parent = staged_parent(journal, import_id)?;
    refuse_source_inside_staging(&parent, source)?;
    let before = fingerprint(source).map_err(|error| error.to_string())?;
    let metadata =
        crate::read_import_metadata(journal, import_id).map_err(|error| error.to_string())?;
    if let Some(record) = metadata.get("preview")
        && record["source"].as_str() == source.to_str()
        && record["digest"].as_str() == Some(before.as_str())
        && let Some(relative) = record["relative"].as_str()
        && let Ok(existing) = solstone_core_journal_io::contained_path(&parent, relative)
        && fingerprint(&existing).is_ok_and(|digest| digest == before)
    {
        return Ok(Snapshot {
            relative: PathBuf::from(relative),
            digest: before,
            temporary: None,
        });
    }
    let temporary = tempfile::Builder::new()
        .prefix("preview-")
        .tempdir_in(&parent)
        .map_err(|error| error.to_string())?;
    finish_copy(&parent, source, before, temporary)
}

fn staged_parent(journal: &Path, import_id: &str) -> Result<PathBuf, String> {
    crate::validate_timestamp(import_id).map_err(|error| error.to_string())?;
    solstone_core_journal_io::contained_path(journal, &format!("imports/{import_id}"))
        .map_err(|error| error.to_string())
}

fn refuse_source_inside_staging(parent: &Path, source: &Path) -> Result<(), String> {
    let canonical_source = source.canonicalize().map_err(|error| error.to_string())?;
    if parent
        .canonicalize()
        .map_err(|error| error.to_string())?
        .starts_with(&canonical_source)
    {
        return Err("choose a source outside its import staging folder".to_owned());
    }
    Ok(())
}

fn finish_copy(
    parent: &Path,
    source: &Path,
    before: String,
    temporary: tempfile::TempDir,
) -> Result<Snapshot, String> {
    let destination = temporary
        .path()
        .join(source.file_name().ok_or("source has no filename")?);
    copy_tree(source, &destination).map_err(|error| error.to_string())?;
    let after = fingerprint(source).map_err(|error| error.to_string())?;
    if before != after || before != fingerprint(&destination).map_err(|error| error.to_string())? {
        return Err("the source changed while preparing its preview; try again".to_owned());
    }
    let relative = portable_relative(
        destination
            .strip_prefix(parent)
            .map_err(|error| error.to_string())?,
    )?;
    Ok(Snapshot {
        relative,
        digest: before,
        temporary: Some(temporary),
    })
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() || !(meta.is_file() || meta.is_dir()) {
        return Err(io::Error::other(
            "import sources must contain only regular files and folders",
        ));
    }
    if meta.is_dir() {
        solstone_core_journal_io::create_directory_with_mode(destination, 0o700)
            .map_err(io::Error::other)?;
        for child in children(source)? {
            copy_tree(
                &child,
                &destination.join(child.file_name().expect("directory child")),
            )?;
        }
    } else {
        let mut input = fs::File::open(source)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(destination)?;
        io::copy(&mut input, &mut output)?;
        output.set_times(fs::FileTimes::new().set_modified(meta.modified()?))?;
    }
    Ok(())
}

pub fn verify(source: &Path, expected: &str) -> Result<(), String> {
    if fingerprint(source).map_err(|error| error.to_string())? != expected {
        return Err(
            "the source changed after preview; preview it again before importing".to_owned(),
        );
    }
    Ok(())
}

/// Recheck the captured options at the consuming boundary as well as HTTP start.
/// The queued command must not silently adopt a new owner time zone or new settings.
pub fn verify_preview(
    journal: &Path,
    source: &Path,
    token: &str,
    timestamp: &str,
    force: bool,
    setting: Option<&str>,
    source_hint: Option<&str>,
) -> Result<(), String> {
    let lexical_imports = journal.join("imports");
    let canonical_imports = lexical_imports
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let relative = source
        .strip_prefix(&lexical_imports)
        .or_else(|_| source.strip_prefix(&canonical_imports))
        .map_err(|error| error.to_string())?;
    let id = relative
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
        .ok_or("invalid preview source")?;
    crate::validate_timestamp(id).map_err(|error| error.to_string())?;
    let metadata = crate::read_import_metadata(journal, id).map_err(|error| error.to_string())?;
    let record = metadata
        .get("preview")
        .ok_or("the saved preview is missing")?;
    let options = &record["options"];
    let preview_relative = relative
        .strip_prefix(id)
        .ok()
        .and_then(|path| portable_relative(path).ok());
    if record["token"].as_str() != Some(token)
        || options["timestamp"].as_str() != Some(timestamp)
        || options["force"].as_bool() != Some(force)
        || options["zone"].as_str()
            != Some(solstone_core_journal_config::owner_zone(journal).name())
        || options["setting"]
            .as_str()
            .filter(|value| !value.is_empty())
            != setting.filter(|value| !value.is_empty())
        || options["setting"]
            .as_str()
            .filter(|value| !value.is_empty())
            != metadata
                .get("setting")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
        || options["source"].as_str()
            != metadata
                .get("source_hint")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != "null")
        || options["source"].as_str() != source_hint
        || preview_relative.as_ref().and_then(|path| path.to_str()) != record["relative"].as_str()
    {
        return Err(
            "the source or settings changed after preview; preview again before importing"
                .to_owned(),
        );
    }
    verify(
        source,
        record["digest"]
            .as_str()
            .ok_or("the saved preview is unreadable")?,
    )
}

/// Strava's download is never retained, including copies made for an earlier preview.
/// Externally owned files are left alone.
pub fn discard_download(journal: &Path, import_id: &str) -> Result<(), String> {
    crate::validate_timestamp(import_id).map_err(|error| error.to_string())?;
    let parent = solstone_core_journal_io::contained_path(journal, &format!("imports/{import_id}"))
        .map_err(|error| error.to_string())?;
    let metadata =
        crate::read_import_metadata(journal, import_id).map_err(|error| error.to_string())?;
    if let Some(path) = metadata
        .get("file_path")
        .and_then(serde_json::Value::as_str)
    {
        let path = Path::new(path);
        let lexical_parent = journal.join("imports").join(import_id);
        if let Ok(relative) = path
            .strip_prefix(&parent)
            .or_else(|_| path.strip_prefix(&lexical_parent))
        {
            let relative = portable_relative(relative)?;
            let safe = solstone_core_journal_io::contained_path(
                &parent,
                relative.to_str().ok_or("invalid download path")?,
            )
            .map_err(|error| error.to_string())?;
            if safe.is_file() {
                fs::remove_file(safe).map_err(|error| error.to_string())?;
            }
        }
    }
    for path in children(&parent).map_err(|error| error.to_string())? {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("preview-"))
            && fs::symlink_metadata(&path)
                .map_err(|error| error.to_string())?
                .is_dir()
        {
            fs::remove_dir_all(path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "20261011_120000";
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(format!("imports/{ID}"))).unwrap();
        let source = root.path().join("notes.txt");
        fs::write(&source, b"one recording").unwrap();
        crate::write_import_metadata(root.path(), ID, &serde_json::Map::new()).unwrap();
        (root, source)
    }
    fn save_preview(root: &Path, source: &Path, relative: &Path, digest: &str) {
        let metadata = json!({"setting":"office", "source_hint":"obsidian", "preview": {"token":"token", "source":source, "relative":relative, "digest":digest,
            "options":{"timestamp":ID,"force":false,"zone":solstone_core_journal_config::owner_zone(root).name(),"setting":"office","source":"obsidian"}}});
        crate::write_import_metadata(root, ID, metadata.as_object().unwrap()).unwrap();
    }
    #[test]
    fn snapshots_keep_bytes_dates_and_do_not_create_recordings_or_consent() {
        let (root, source) = fixture();
        let prepared = create(root.path(), ID, &source).unwrap();
        let (relative, digest) = (prepared.relative.clone(), prepared.digest.clone());
        assert!(!relative.as_os_str().to_str().unwrap().contains('\\'));
        prepared.persist();
        let snapshot = root.path().join("imports").join(ID).join(&relative);
        assert_eq!(
            fingerprint(&source).unwrap(),
            fingerprint(&snapshot).unwrap()
        );
        assert!(!root.path().join("chronicle").exists());
        assert!(!root.path().join("health").exists());
        save_preview(root.path(), &source, &relative, &digest);
        let reused = create(root.path(), ID, &source).unwrap();
        assert_eq!(reused.relative, relative);
        assert_eq!(reused.digest, digest);
        drop(reused);
        assert!(snapshot.exists());
        fs::write(&source, b"changed after start acceptance").unwrap();
        assert!(verify(&source, &digest).is_err());
        assert!(verify(&snapshot, &digest).is_ok());
    }
    #[test]
    fn consuming_boundary_refuses_changed_options_zone_token_and_snapshot() {
        let (root, source) = fixture();
        let prepared = create(root.path(), ID, &source).unwrap();
        let (relative, digest) = (prepared.relative.clone(), prepared.digest.clone());
        prepared.persist();
        let snapshot = root.path().join("imports").join(ID).join(&relative);
        save_preview(root.path(), &source, &relative, &digest);
        let check = |token, force, setting| {
            verify_preview(
                root.path(),
                &snapshot,
                token,
                ID,
                force,
                Some(setting),
                Some("obsidian"),
            )
        };
        assert!(check("token", false, "office").is_ok());
        assert!(check("other", false, "office").is_err());
        assert!(check("token", true, "office").is_err());
        assert!(check("token", false, "home").is_err());
        let mut metadata = crate::read_import_metadata(root.path(), ID).unwrap();
        metadata.get_mut("preview").unwrap()["relative"] = json!(format!(
            "{}/./{}",
            relative.parent().unwrap().to_str().unwrap(),
            relative.file_name().unwrap().to_str().unwrap(),
        ));
        crate::write_import_metadata(root.path(), ID, &metadata).unwrap();
        assert!(check("token", false, "office").is_err());
        save_preview(root.path(), &source, &relative, &digest);
        fs::write(&snapshot, b"tampered").unwrap();
        assert!(check("token", false, "office").is_err());
    }
    #[test]
    fn failed_previews_remove_new_snapshots() {
        let (root, source) = fixture();
        let prepared = create(root.path(), ID, &source).unwrap();
        let path = root
            .path()
            .join("imports")
            .join(ID)
            .join(&prepared.relative);
        assert!(path.exists());
        drop(prepared);
        assert!(!path.exists());
    }
    #[test]
    fn metadata_changes_invalidate_the_consuming_token() {
        let (root, source) = fixture();
        let prepared = create(root.path(), ID, &source).unwrap();
        let path = root
            .path()
            .join("imports")
            .join(ID)
            .join(&prepared.relative);
        save_preview(root.path(), &source, &prepared.relative, &prepared.digest);
        let mut metadata = crate::read_import_metadata(root.path(), ID).unwrap();
        metadata.insert("setting".into(), json!("changed"));
        crate::write_import_metadata(root.path(), ID, &metadata).unwrap();
        assert!(
            verify_preview(
                root.path(),
                &path,
                "token",
                ID,
                false,
                Some("office"),
                Some("obsidian")
            )
            .is_err()
        );
    }
    #[test]
    fn a_source_cannot_include_its_own_snapshot_destination() {
        let (root, _) = fixture();
        assert!(create(root.path(), ID, root.path()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn source_links_are_refused() {
        let (root, source) = fixture();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(source, &link).unwrap();
        assert!(create(root.path(), ID, &link).is_err());
    }
    #[test]
    fn strava_cleanup_removes_preview_copies_and_keeps_external_sources() {
        let (root, source) = fixture();
        let prepared = create(root.path(), ID, &source).unwrap();
        let relative = prepared.relative.clone();
        prepared.persist();
        crate::write_import_metadata(
            root.path(),
            ID,
            json!({"file_path":source}).as_object().unwrap(),
        )
        .unwrap();
        discard_download(root.path(), ID).unwrap();
        assert!(source.exists());
        assert!(!root.path().join("imports").join(ID).join(relative).exists());
    }
    #[test]
    fn strava_cleanup_refuses_parent_traversal() {
        let (root, source) = fixture();
        let escaped = root.path().join(format!("imports/{ID}/../../notes.txt"));
        crate::write_import_metadata(
            root.path(),
            ID,
            json!({"file_path":escaped}).as_object().unwrap(),
        )
        .unwrap();
        assert!(discard_download(root.path(), ID).is_err());
        assert!(source.exists());
    }
    #[test]
    fn dropping_a_snapshot_removes_its_tree_before_drop_returns_outside_a_runtime() {
        let (root, source) = fixture();
        let prepared = create(root.path(), ID, &source).unwrap();
        let path = root
            .path()
            .join("imports")
            .join(ID)
            .join(&prepared.relative);
        assert!(path.exists());
        drop(prepared);
        assert!(!path.exists());
    }

    #[test]
    fn dropping_a_snapshot_on_a_runtime_returns_without_removing_the_tree_first() {
        let (root, source) = fixture();
        let path = {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let prepared = create(root.path(), ID, &source).unwrap();
                let path = root
                    .path()
                    .join("imports")
                    .join(ID)
                    .join(&prepared.relative);
                assert!(path.exists());
                drop(prepared);
                path
            })
        };
        assert!(!path.exists());
    }
}
