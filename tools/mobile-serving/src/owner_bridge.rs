// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bridge copies live only while a content URI or security scope cannot outlast
//! the transfer. They are not admitted originals and they are not save temps.
//!
//! Cleanup runs only before owner admission opens. It deletes a numeric child
//! when a sibling receipt or a bounded sibling claim proves this process wrote
//! it, the id is not live, and the import metadata owner resolves no file path
//! to that directory. A claim with no directory is that same owned partial: the
//! copy was claimed and the process died before the directory or receipt
//! existed. Unreadable metadata, an uncertain claim or receipt, or an unknown
//! child deletes nothing.
//!
//! A folder copy does not reuse an id from a restarted counter. `allocate_copy`
//! creates the claim file and the directory exclusively, and `release_copy`
//! deletes only that claim's directory and a receipt this copy created.

use serde_json::Value;
use solstone_core_import::read_import_metadata;
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub const DIRECTORY_NAME: &str = "owner-bridge";
const COPY_BUFFER: usize = 64 * 1024;

pub fn directory(cache: &Path) -> PathBuf {
    cache.join(DIRECTORY_NAME)
}

pub fn receipt_path(directory: &Path) -> Option<PathBuf> {
    let name = directory.file_name()?.to_str()?;
    name.parse::<u64>().ok()?;
    Some(directory.with_file_name(format!("{name}.receipt")))
}

pub fn receipt_body(id: u64) -> String {
    format!("{{\"id\":{id}}}\n")
}

/// Record that `directory` is a bridge copy this owner wrote.
pub fn write_receipt(directory: &Path) -> io::Result<()> {
    let id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse::<u64>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bridge id required"))?;
    let path = receipt_path(directory)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bridge id required"))?;
    fs::write(path, receipt_body(id))
}

/// Delete abandoned children of `cache/owner-bridge`. Admitted originals and
/// live transfers stay. This never walks `imports/` as a deletion root.
/// A failure deletes nothing.
pub fn reclaim(cache: &Path, journal: &Path, live: &[u64]) -> Result<(), String> {
    let root = directory(cache);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let admitted = admitted_paths(journal)?;
    let mut doomed = Vec::new();
    let mut claims = Vec::new();
    let mut directories = Vec::new();
    let mut receipts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err("bridge name is uncertain".into());
        };
        if let Some(id) = numeric_sibling(name, ".receipt") {
            receipts.push(id);
            continue;
        }
        if name.ends_with(".receipt") {
            continue;
        }
        if let Some(id) = numeric_sibling(name, ".claim") {
            if matches!(inspect_claim(&entry.path())?, ClaimProof::Proven) {
                claims.push((id, entry.path()));
            }
            continue;
        }
        if name.ends_with(".claim") {
            continue;
        }
        if let Ok(id) = name.parse::<u64>() {
            directories.push(id);
        }
        let proven = proven_copy(&root, name, &entry.path())?;
        if let Some(id) = proven
            && !live.contains(&id)
            && !admitted.iter().any(|kept| covers(kept, &entry.path()))
        {
            doomed.push(entry.path());
        }
    }
    let mut orphan_claims = Vec::new();
    for (id, claim) in claims {
        if directories.contains(&id) || receipts.contains(&id) || live.contains(&id) {
            continue;
        }
        let dir = root.join(id.to_string());
        if admitted.iter().any(|kept| covers(kept, &dir)) {
            continue;
        }
        orphan_claims.push(claim);
    }
    for path in doomed {
        fs::remove_dir_all(&path).map_err(|error| error.to_string())?;
        if let Some(receipt) = receipt_path(&path) {
            remove_sibling_file(&receipt)?;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            remove_sibling_file(&path.with_file_name(format!("{name}.claim")))?;
        }
    }
    for claim in orphan_claims {
        remove_proven_claim(&claim)?;
    }
    Ok(())
}

fn remove_sibling_file(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
            fs::remove_file(path).map_err(|error| error.to_string())
        }
        Ok(_) => Err("bridge claim is uncertain".into()),
    }
}

enum ClaimProof {
    Absent,
    Proven,
}

fn numeric_sibling(name: &str, suffix: &str) -> Option<u64> {
    name.strip_suffix(suffix)?.parse().ok()
}

/// A regular claim of at most 128 bytes is provenance for the partial copy
/// `allocate_copy` creates before any receipt. A symlink, directory, overlong
/// file, or unreadable claim fails the sweep before any deletion.
fn inspect_claim(path: &Path) -> Result<ClaimProof, String> {
    let metadata = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ClaimProof::Absent),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 128 {
        return Err("bridge claim is uncertain".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() > 128 {
        return Err("bridge claim is uncertain".into());
    }
    Ok(ClaimProof::Proven)
}

fn remove_proven_claim(path: &Path) -> Result<(), String> {
    match inspect_claim(path)? {
        ClaimProof::Proven => fs::remove_file(path).map_err(|error| error.to_string()),
        ClaimProof::Absent => Ok(()),
    }
}

fn proven_copy(root: &Path, name: &str, path: &Path) -> Result<Option<u64>, String> {
    let Ok(id) = name.parse::<u64>() else {
        return Ok(None);
    };
    let kind = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if kind.file_type().is_symlink() || !kind.is_dir() {
        return Err("bridge entry is uncertain".into());
    }
    let receipt = root.join(format!("{id}.receipt"));
    let receipt_proves = match fs::symlink_metadata(&receipt) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
        Ok(kind) if kind.file_type().is_symlink() || !kind.is_file() => {
            return Err("bridge receipt is uncertain".into());
        }
        Ok(_) => {
            let bytes = fs::read(&receipt).map_err(|error| error.to_string())?;
            let proves = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| value.get("id").and_then(Value::as_u64))
                == Some(id);
            Some(proves)
        }
    };
    // A bad claim fails closed even when this directory is kept.
    let claim = inspect_claim(&root.join(format!("{id}.claim")))?;
    if receipt_proves == Some(false) {
        return Ok(None);
    }
    if receipt_proves == Some(true) || matches!(claim, ClaimProof::Proven) {
        return Ok(Some(id));
    }
    Ok(None)
}

fn covers(reference: &Path, bridge: &Path) -> bool {
    let bridge_forms = path_forms(bridge);
    let reference_forms = path_forms(reference);
    bridge_forms.iter().any(|bridge| {
        reference_forms.iter().any(|reference| {
            reference == bridge || reference.starts_with(bridge) || bridge.starts_with(reference)
        })
    })
}

fn path_forms(path: &Path) -> Vec<PathBuf> {
    let mut forms = vec![lexical(path)];
    if let Ok(canonical) = fs::canonicalize(path) {
        forms.push(canonical);
    }
    forms
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn admitted_paths(journal: &Path) -> Result<Vec<PathBuf>, String> {
    let imports = journal.join("imports");
    let entries = match fs::read_dir(&imports) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "import name is uncertain".to_owned())?;
        let kind = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if kind.file_type().is_symlink() {
            return Err("import listing is uncertain".into());
        }
        if !kind.is_dir() {
            continue;
        }
        let metadata_path = entry.path().join("import.json");
        let metadata_kind = match fs::symlink_metadata(&metadata_path) {
            Ok(kind) => kind,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if metadata_kind.file_type().is_symlink() || !metadata_kind.is_file() {
            return Err("import metadata is uncertain".into());
        }
        if solstone_core_import::staging::import_directory(journal, &name).is_err() {
            return Err("import metadata is uncertain".into());
        }
        let metadata = read_import_metadata(journal, &name).map_err(|error| error.to_string())?;
        let Some(raw) = metadata.get("file_path") else {
            continue;
        };
        let Some(raw) = raw.as_str() else {
            return Err("import path is uncertain".into());
        };
        if raw.is_empty() {
            continue;
        }
        paths.extend(reference_candidates(journal, &entry.path(), raw)?);
    }
    Ok(paths)
}

fn reference_candidates(
    journal: &Path,
    import_dir: &Path,
    raw: &str,
) -> Result<Vec<PathBuf>, String> {
    let path = Path::new(raw);
    let mut bases = Vec::new();
    if path.is_absolute() {
        bases.push(path.to_path_buf());
    } else {
        bases.push(journal.join(path));
        bases.push(import_dir.join(path));
    }
    let mut out = Vec::new();
    for candidate in bases {
        if candidate.try_exists().map_err(|error| error.to_string())? {
            out.push(fs::canonicalize(&candidate).map_err(|error| error.to_string())?);
        } else {
            out.push(lexical(&candidate));
        }
    }
    Ok(out)
}

pub fn copy_reader(mut reader: impl io::Read, mut writer: impl io::Write) -> io::Result<u64> {
    let mut buffer = [0u8; COPY_BUFFER];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(total);
        }
        writer.write_all(&buffer[..read])?;
        total += u64::try_from(read).unwrap_or(u64::MAX);
    }
}

/// One folder copy this process created. The token matches the sibling claim.
/// A restarted process does not have the token, and an admitted copy has none.
pub struct OwnedCopy {
    cache: PathBuf,
    id: u64,
    token: String,
    receipt_created: bool,
}

impl OwnedCopy {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn receipt_created(&self) -> bool {
        self.receipt_created
    }

    pub fn directory(&self) -> PathBuf {
        directory(&self.cache).join(self.id.to_string())
    }
}

/// Rebuild a copy from the cache, id, and claim token. The token is the
/// capability; a wrong token cannot release another id.
pub fn adopt_copy(cache: &Path, id: u64, token: &str, receipt_created: bool) -> OwnedCopy {
    OwnedCopy {
        cache: cache.to_path_buf(),
        id,
        token: token.to_owned(),
        receipt_created,
    }
}

/// Create a new `cache/owner-bridge/<id>` that no directory, receipt, or claim
/// already occupies. The id scan starts at 1, so a counter that restarted at
/// zero cannot land on an admitted copy.
pub fn allocate_copy(cache: &Path) -> io::Result<OwnedCopy> {
    let root = directory(cache);
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "couldn't be read",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(&root)?,
        Err(error) => return Err(error),
    }
    for id in 1..u64::MAX {
        let dir = root.join(id.to_string());
        let receipt = root.join(format!("{id}.receipt"));
        let claim = root.join(format!("{id}.claim"));
        if occupied(&dir)? || occupied(&receipt)? || occupied(&claim)? {
            continue;
        }
        let token = fresh_token()?;
        match create_exclusive(&claim, token.as_bytes()) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        match fs::create_dir(&dir) {
            Ok(()) => {
                return Ok(OwnedCopy {
                    cache: cache.to_path_buf(),
                    id,
                    token,
                    receipt_created: false,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let _ = remove_matching_claim(&claim, &token);
                continue;
            }
            Err(error) => {
                let _ = remove_matching_claim(&claim, &token);
                return Err(error);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "couldn't be read",
    ))
}

/// Write this copy's receipt. An existing receipt is left untouched.
pub fn commit_copy(owned: &mut OwnedCopy) -> io::Result<()> {
    let claim = claim_file(&owned.cache, owned.id);
    if !claim_matches(&claim, &owned.token)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "couldn't be read",
        ));
    }
    let Some(receipt) = receipt_path(&owned.directory()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "couldn't be read",
        ));
    };
    create_exclusive(&receipt, receipt_body(owned.id).as_bytes())?;
    owned.receipt_created = true;
    Ok(())
}

/// Delete the directory and, only when this copy wrote it, the receipt.
/// A claim that does not match deletes nothing.
pub fn release_copy(owned: &OwnedCopy) -> io::Result<()> {
    let claim = claim_file(&owned.cache, owned.id);
    if !claim_matches(&claim, &owned.token)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "couldn't be read",
        ));
    }
    let dir = owned.directory();
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "couldn't be read",
            ));
        }
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&dir)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "couldn't be read",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if owned.receipt_created {
        let receipt = receipt_path(&dir)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "couldn't be read"))?;
        match fs::symlink_metadata(&receipt) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "couldn't be read",
                ));
            }
            Ok(_) => fs::remove_file(&receipt)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    remove_matching_claim(&claim, &owned.token)
}

/// Drop the claim after the path is published. The directory and receipt stay.
pub fn disarm_copy(owned: &OwnedCopy) -> io::Result<()> {
    let claim = claim_file(&owned.cache, owned.id);
    if !claim_matches(&claim, &owned.token)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "couldn't be read",
        ));
    }
    remove_matching_claim(&claim, &owned.token)
}

fn claim_file(cache: &Path, id: u64) -> PathBuf {
    directory(cache).join(format!("{id}.claim"))
}

fn occupied(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn fresh_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn create_exclusive(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

fn claim_matches(path: &Path, token: &str) -> io::Result<bool> {
    if token.is_empty() || token.len() > 128 {
        return Ok(false);
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 128 {
        return Ok(false);
    }
    Ok(fs::read(path)? == token.as_bytes())
}

fn remove_matching_claim(path: &Path, token: &str) -> io::Result<()> {
    if !claim_matches(path, token)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "couldn't be read",
        ));
    }
    fs::remove_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailAfter {
        left: usize,
    }

    impl io::Write for FailAfter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.left == 0 {
                return Err(io::Error::new(io::ErrorKind::StorageFull, "no space"));
            }
            let taken = buf.len().min(self.left);
            self.left -= taken;
            Ok(taken)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn prove(path: &Path) {
        fs::create_dir_all(path).unwrap();
        write_receipt(path).unwrap();
    }

    #[test]
    fn failed_copy_discards_only_the_directory_it_created() {
        let tmp = tempfile::tempdir().unwrap();
        let owned = allocate_copy(tmp.path()).unwrap();
        let dir = owned.directory();
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::write(dir.join("notes").join("a.md"), b"hello").unwrap();
        let error = copy_reader(&b"vault-bytes"[..], FailAfter { left: 4 }).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        release_copy(&owned).unwrap();
        assert!(!dir.exists());
        assert!(!receipt_path(&dir).unwrap().exists());
        assert!(!claim_file(tmp.path(), owned.id()).exists());
    }

    #[test]
    fn successful_copy_keeps_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let mut owned = allocate_copy(tmp.path()).unwrap();
        let dir = owned.directory();
        let file = fs::File::create(dir.join("note.md")).unwrap();
        copy_reader(&b"kept"[..], file).unwrap();
        commit_copy(&mut owned).unwrap();
        assert_eq!(fs::read(dir.join("note.md")).unwrap(), b"kept");
        assert_eq!(
            fs::read(receipt_path(&dir).unwrap()).unwrap(),
            receipt_body(owned.id()).into_bytes()
        );
    }

    #[test]
    fn restarted_allocation_failure_leaves_admitted_copy_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let journal = tmp.path().join("journal");
        let admitted = directory(&cache).join("1");
        fs::create_dir_all(admitted.join("notes")).unwrap();
        let body = b"keep\0\xff";
        fs::write(admitted.join("notes").join("a.md"), body).unwrap();
        write_receipt(&admitted).unwrap();
        let receipt = fs::read(receipt_path(&admitted).unwrap()).unwrap();
        let record = journal.join("imports").join("20260101_010101");
        fs::create_dir_all(&record).unwrap();
        let metadata = serde_json::json!({"file_path": &admitted}).to_string();
        fs::write(record.join("import.json"), &metadata).unwrap();

        let owned = allocate_copy(&cache).unwrap();
        assert_ne!(owned.id(), 1, "a restarted counter must not reuse id 1");
        fs::write(owned.directory().join("new.md"), b"temp").unwrap();
        release_copy(&owned).unwrap();

        assert_eq!(fs::read(admitted.join("notes").join("a.md")).unwrap(), body);
        assert_eq!(fs::read(receipt_path(&admitted).unwrap()).unwrap(), receipt);
        assert_eq!(
            fs::read(record.join("import.json")).unwrap(),
            metadata.as_bytes()
        );
        assert!(!owned.directory().exists());
        assert!(!claim_file(&cache, owned.id()).exists());

        let forged = adopt_copy(&cache, 1, "not-the-owner", true);
        assert!(release_copy(&forged).is_err());
        assert!(commit_copy(&mut adopt_copy(&cache, 1, "not-the-owner", false)).is_err());
        assert_eq!(fs::read(admitted.join("notes").join("a.md")).unwrap(), body);
        assert_eq!(fs::read(receipt_path(&admitted).unwrap()).unwrap(), receipt);
        assert_eq!(
            fs::read(record.join("import.json")).unwrap(),
            metadata.as_bytes()
        );
    }

    #[test]
    fn reclaim_keeps_admitted_originals_live_transfers_and_save_temps() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let journal = tmp.path().join("journal");
        let bridge = directory(&cache);
        prove(&bridge.join("7"));
        prove(&bridge.join("8"));
        prove(&bridge.join("9"));
        fs::create_dir_all(bridge.join("notes")).unwrap();
        let admitted = bridge.join("8");
        let imports = journal.join("imports");
        fs::create_dir_all(imports.join("20260101_010101")).unwrap();
        fs::create_dir_all(imports.join(".save-tmp")).unwrap();
        fs::write(imports.join(".save-tmp").join("partial"), b"keep").unwrap();
        let metadata = serde_json::json!({"file_path": admitted});
        fs::write(
            imports.join("20260101_010101").join("import.json"),
            metadata.to_string(),
        )
        .unwrap();
        let save_tmp = imports.join(".save-tmp");
        reclaim(&cache, &journal, &[7]).unwrap();
        assert!(bridge.join("7").exists(), "live transfer stays");
        assert!(bridge.join("8").exists(), "admitted original stays");
        assert!(
            !bridge.join("9").exists(),
            "abandoned bridge copy is removed"
        );
        assert!(!receipt_path(&bridge.join("9")).unwrap().exists());
        assert!(bridge.join("notes").exists(), "unknown child stays");
        assert!(save_tmp.join("partial").exists());
        assert!(imports.join("20260101_010101").join("import.json").exists());
    }

    #[test]
    fn reclaim_resolves_relative_and_dot_segment_references() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let journal = tmp.path().join("journal");
        let bridge = directory(&cache);
        let relative_target = bridge.join("8");
        let dotted = bridge.join("4");
        prove(&relative_target);
        prove(&dotted);
        let imports = journal.join("imports");
        fs::create_dir_all(imports.join("20260101_010101")).unwrap();
        fs::create_dir_all(imports.join("20260101_010102")).unwrap();
        let relative = Path::new("..")
            .join(cache.file_name().unwrap())
            .join(DIRECTORY_NAME)
            .join("8");
        fs::write(
            imports.join("20260101_010101").join("import.json"),
            serde_json::json!({"file_path": relative}).to_string(),
        )
        .unwrap();
        let dotted_path = dotted.join("..").join("4").join(".");
        fs::write(
            imports.join("20260101_010102").join("import.json"),
            serde_json::json!({"file_path": dotted_path}).to_string(),
        )
        .unwrap();
        reclaim(&cache, &journal, &[]).unwrap();
        assert!(relative_target.exists());
        assert!(dotted.exists());
    }

    #[test]
    fn corrupt_or_unreadable_metadata_and_unknown_children_delete_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let journal = tmp.path().join("journal");
        let bridge = directory(&cache);
        prove(&bridge.join("9"));
        fs::create_dir_all(bridge.join("stray")).unwrap();
        fs::write(bridge.join("9.receipt-lookalike"), b"no").unwrap();
        let imports = journal.join("imports");
        let record = imports.join("20260101_010101");
        fs::create_dir_all(&record).unwrap();
        fs::write(record.join("import.json"), b"not json").unwrap();
        let error = reclaim(&cache, &journal, &[]).unwrap_err();
        assert!(!error.is_empty());
        assert!(bridge.join("9").exists());
        assert!(bridge.join("stray").exists());

        fs::remove_file(record.join("import.json")).unwrap();
        fs::create_dir(record.join("import.json")).unwrap();
        assert!(reclaim(&cache, &journal, &[]).is_err());
        assert!(bridge.join("9").exists());
    }

    #[test]
    fn mismatched_receipt_is_not_abandoned_and_a_live_id_interleaves_safely() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let journal = tmp.path().join("journal");
        let bridge = directory(&cache);
        prove(&bridge.join("3"));
        fs::write(receipt_path(&bridge.join("3")).unwrap(), b"{\"id\":9}\n").unwrap();
        prove(&bridge.join("5"));
        prove(&bridge.join("6"));
        reclaim(&cache, &journal, &[5]).unwrap();
        assert!(bridge.join("3").exists(), "wrong receipt stays");
        assert!(bridge.join("5").exists(), "live id stays");
        assert!(!bridge.join("6").exists(), "other proven copy is removed");
    }

    fn write_claim(bridge: &Path, id: u64) {
        fs::write(bridge.join(format!("{id}.claim")), b"claim-token").unwrap();
    }

    fn partial_copy(bridge: &Path, id: u64) -> PathBuf {
        let dir = bridge.join(id.to_string());
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::write(dir.join("notes").join("a.md"), b"partial\0\xff").unwrap();
        write_claim(bridge, id);
        dir
    }

    fn admit(journal: &Path, name: &str, path: &Path) {
        let record = journal.join("imports").join(name);
        fs::create_dir_all(&record).unwrap();
        fs::write(
            record.join("import.json"),
            serde_json::json!({"file_path": path}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn reclaim_removes_claim_partials_and_keeps_admitted_references() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let journal = tmp.path().join("journal");
        let bridge = directory(&cache);
        fs::create_dir_all(&bridge).unwrap();
        write_claim(&bridge, 11);
        write_claim(&bridge, 4);
        let during = partial_copy(&bridge, 5);
        let admitted = partial_copy(&bridge, 6);
        let missing = bridge.join("16");
        write_claim(&bridge, 16);
        let live = partial_copy(&bridge, 7);
        prove(&bridge.join("9"));
        let mismatched = bridge.join("3");
        prove(&mismatched);
        fs::write(receipt_path(&mismatched).unwrap(), b"{\"id\":9}\n").unwrap();
        write_claim(&bridge, 3);
        fs::create_dir_all(bridge.join("notes")).unwrap();
        admit(&journal, "20260101_010101", &admitted);
        admit(&journal, "20260101_010102", &missing);

        reclaim(&cache, &journal, &[4, 7]).unwrap();

        assert!(
            !bridge.join("11.claim").exists(),
            "claim created before its directory is removed"
        );
        assert!(bridge.join("4.claim").exists(), "live claim stays");
        assert!(!during.exists(), "partial copy with no receipt is removed");
        assert!(!bridge.join("5.claim").exists());
        assert_eq!(
            fs::read(admitted.join("notes").join("a.md")).unwrap(),
            b"partial\0\xff",
            "admitted claim copy stays"
        );
        assert!(bridge.join("6.claim").exists());
        assert!(
            !receipt_path(&admitted).unwrap().exists(),
            "the admitted control has no receipt"
        );
        assert!(
            bridge.join("16.claim").exists(),
            "a claim named by an admitted path stays"
        );
        assert_eq!(
            fs::read(live.join("notes").join("a.md")).unwrap(),
            b"partial\0\xff"
        );
        assert!(bridge.join("7.claim").exists());
        assert!(!bridge.join("9").exists(), "receipt copy is still removed");
        assert!(!receipt_path(&bridge.join("9")).unwrap().exists());
        assert!(mismatched.exists(), "a mismatched receipt stays");
        assert!(bridge.join("3.claim").exists());
        assert!(bridge.join("notes").exists(), "unknown child stays");
    }

    #[test]
    fn uncertain_claim_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let journal = tmp.path().join("journal");
        let bridge = directory(&cache);
        prove(&bridge.join("9"));
        let partial = bridge.join("5");
        fs::create_dir_all(&partial).unwrap();
        fs::write(partial.join("a.md"), b"keep").unwrap();
        fs::create_dir(bridge.join("5.claim")).unwrap();
        assert!(reclaim(&cache, &journal, &[]).is_err());
        assert!(bridge.join("9").exists());
        assert_eq!(fs::read(partial.join("a.md")).unwrap(), b"keep");

        fs::remove_dir(bridge.join("5.claim")).unwrap();
        fs::write(bridge.join("5.claim"), vec![b'x'; 129]).unwrap();
        assert!(reclaim(&cache, &journal, &[]).is_err());
        assert!(bridge.join("9").exists());
        assert_eq!(fs::read(partial.join("a.md")).unwrap(), b"keep");
        fs::remove_file(bridge.join("5.claim")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("elsewhere", bridge.join("5.claim")).unwrap();
            assert!(reclaim(&cache, &journal, &[]).is_err());
            assert!(bridge.join("9").exists());
            assert_eq!(fs::read(partial.join("a.md")).unwrap(), b"keep");
        }
    }
}
