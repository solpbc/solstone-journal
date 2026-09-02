// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pre-build signing and deterministic repacking of declared macOS archives.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tar::{Builder, EntryType, Header};

use crate::apple::{AppleError, ArchiveMemberSigner};
use crate::archive_census::{GzipMemberKind, ValidatedGzipMember, validate_gzip_archive};
use crate::digest::sha256_hex;
use crate::inventory::{ArchiveSlot, Entry, Inventory, digest_const_hex};
use crate::tar::deterministic_gzip;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedArchive {
    pub slot_id: String,
    pub staged_dest: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub size: u64,
    pub source_sha256: String,
    pub signed_executables: Vec<SealedExecutable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedExecutable {
    pub member_path: String,
    pub source_sha256: String,
    pub signed_sha256: String,
    pub mode: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SealedArchiveSet {
    pub archives: Vec<SealedArchive>,
}

impl SealedArchiveSet {
    #[must_use]
    pub fn by_slot_id(&self, slot_id: &str) -> Option<&SealedArchive> {
        self.archives
            .iter()
            .find(|archive| archive.slot_id == slot_id)
    }
}

#[derive(Debug)]
pub struct ArchiveSealError {
    message: String,
}

impl ArchiveSealError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ArchiveSealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ArchiveSealError {}

impl From<io::Error> for ArchiveSealError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<AppleError> for ArchiveSealError {
    fn from(error: AppleError) -> Self {
        Self::new(error.to_string())
    }
}

pub(crate) fn seal_declared_archives(
    checkout: &Path,
    inventory: &Inventory,
    target_id: &str,
    signer: &dyn ArchiveMemberSigner,
) -> Result<SealedArchiveSet, ArchiveSealError> {
    seal_declared_archives_inner(checkout, inventory, target_id, signer, None)
}

fn seal_declared_archives_inner(
    checkout: &Path,
    inventory: &Inventory,
    target_id: &str,
    signer: &dyn ArchiveMemberSigner,
    mut before_revalidate: Option<&mut dyn FnMut(&Path)>,
) -> Result<SealedArchiveSet, ArchiveSealError> {
    let scratch_root = checkout
        .parent()
        .ok_or_else(|| ArchiveSealError::new("missing required:\n  checkout parent"))?
        .join("archive-seal");
    fs::create_dir_all(&scratch_root)?;
    let mut archives = Vec::new();

    for (index, entry) in inventory.entry.iter().enumerate() {
        let Entry::ModelAsset {
            source,
            dest,
            digest_const,
            digest_source,
            archive_slot: Some(slot),
            ..
        } = entry
        else {
            continue;
        };
        if slot.target != target_id {
            continue;
        }
        let source_path = checkout.join(source);
        let source_bytes = read_verified_source(
            &source_path,
            &checkout.join(digest_source),
            digest_const,
            dest,
        )?;
        let source_sha256 = sha256_hex(&source_bytes);
        let source_archive = validate_gzip_archive(dest, &source_bytes, slot)
            .map_err(|error| ArchiveSealError::new(error.to_string()))?;

        if let Some(hook) = before_revalidate.as_deref_mut() {
            hook(&source_path);
        }
        let revalidated = read_verified_source(
            &source_path,
            &checkout.join(digest_source),
            digest_const,
            dest,
        )?;
        if revalidated != source_bytes {
            return Err(ArchiveSealError::new(format!(
                "TOCTOU archive mutation before signing: {dest}"
            )));
        }
        validate_declared_executable_digests(checkout, slot, &source_archive.members)?;

        let scratch = scratch_root.join(index.to_string());
        if scratch.exists() {
            fs::remove_dir_all(&scratch)?;
        }
        fs::create_dir_all(&scratch)?;
        let (repacked, signed_executables) =
            sign_and_repack(&scratch, slot, &source_archive.members, signer)?;
        verify_repacked_archive(dest, slot, &source_archive.members, &repacked)?;
        archives.push(SealedArchive {
            slot_id: slot.id.clone(),
            staged_dest: dest.clone(),
            sha256: sha256_hex(&repacked),
            size: repacked.len() as u64,
            bytes: repacked,
            source_sha256,
            signed_executables,
        });
    }
    archives.sort_by(|left, right| left.slot_id.cmp(&right.slot_id));
    Ok(SealedArchiveSet { archives })
}

fn read_verified_source(
    source_path: &Path,
    digest_source_path: &Path,
    digest_const: &str,
    dest: &str,
) -> Result<Vec<u8>, ArchiveSealError> {
    let bytes = fs::read(source_path)?;
    let digest_source = fs::read_to_string(digest_source_path)?;
    let expected = digest_const_hex(&digest_source, digest_const).ok_or_else(|| {
        ArchiveSealError::new(format!("missing required:\n  digest {digest_const}"))
    })?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(ArchiveSealError::new(format!(
            "unexpected:\n  {dest} digest {actual}"
        )));
    }
    Ok(bytes)
}

fn validate_declared_executable_digests(
    checkout: &Path,
    slot: &ArchiveSlot,
    source_members: &[ValidatedGzipMember],
) -> Result<(), ArchiveSealError> {
    for executable in &slot.executables {
        let member = source_members
            .iter()
            .find(|member| member.path == executable.path)
            .ok_or_else(|| {
                ArchiveSealError::new(format!(
                    "archive slot {} missing declared executable {}",
                    slot.id, executable.path
                ))
            })?;
        let digest_source_path = checkout.join(&executable.digest_source);
        let digest_source = fs::read_to_string(&digest_source_path).map_err(|error| {
            ArchiveSealError::new(format!(
                "read archive executable digest source {}: {error}",
                digest_source_path.display()
            ))
        })?;
        let expected =
            digest_const_hex(&digest_source, &executable.digest_const).ok_or_else(|| {
                ArchiveSealError::new(format!(
                    "missing required:\n  digest {}",
                    executable.digest_const
                ))
            })?;
        let actual = sha256_hex(&member.bytes);
        if actual != expected {
            return Err(ArchiveSealError::new(format!(
                "archive slot {} executable {} digest mismatch: expected {expected}, observed {actual}",
                slot.id, executable.path
            )));
        }
    }
    Ok(())
}

fn sign_and_repack(
    scratch: &Path,
    slot: &ArchiveSlot,
    source_members: &[ValidatedGzipMember],
    signer: &dyn ArchiveMemberSigner,
) -> Result<(Vec<u8>, Vec<SealedExecutable>), ArchiveSealError> {
    let executable_paths = slot
        .executables
        .iter()
        .map(|executable| executable.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut emitted = Vec::with_capacity(source_members.len());
    let mut directories = Vec::new();
    let mut signed_executables = Vec::new();
    for member in source_members {
        let extracted = extract_member(scratch, member)?;
        if member.kind == GzipMemberKind::Directory {
            directories.push((extracted, member.mode));
            emitted.push(member.clone());
            continue;
        }
        let mut emitted_member = member.clone();
        if executable_paths.contains(member.path.as_str()) {
            let signed = signer.sign_executable(&extracted, &member.path)?;
            let mode = crate::stage::file_mode(&fs::metadata(&extracted)?);
            if mode != member.mode {
                return Err(ArchiveSealError::new(format!(
                    "mode mutation while signing {}: {:04o} -> {:04o}",
                    member.path, member.mode, mode
                )));
            }
            let signed_bytes = fs::read(&extracted)?;
            let signed_sha256 = sha256_hex(&signed_bytes);
            if signed.sha256 != signed_sha256 {
                return Err(ArchiveSealError::new(format!(
                    "signer digest mismatch for {}",
                    member.path
                )));
            }
            emitted_member.bytes = signed_bytes;
            signed_executables.push(SealedExecutable {
                member_path: member.path.clone(),
                source_sha256: sha256_hex(&member.bytes),
                signed_sha256,
                mode: member.mode,
            });
        }
        emitted.push(emitted_member);
    }
    for (directory, mode) in directories.into_iter().rev() {
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&directory)?.permissions();
            permissions.set_mode(mode);
            fs::set_permissions(directory, permissions)?;
        }
        #[cfg(not(unix))]
        let _ = (directory, mode);
    }
    let bytes = repack_members(&emitted)?;
    Ok((bytes, signed_executables))
}

fn extract_member(
    scratch: &Path,
    member: &ValidatedGzipMember,
) -> Result<PathBuf, ArchiveSealError> {
    let path = scratch.join(&member.path);
    match member.kind {
        GzipMemberKind::Directory => {
            fs::create_dir_all(&path)?;
        }
        GzipMemberKind::File => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, &member.bytes)?;
            #[cfg(unix)]
            {
                let mut permissions = fs::metadata(&path)?.permissions();
                permissions.set_mode(member.mode);
                fs::set_permissions(&path, permissions)?;
            }
        }
    }
    Ok(path)
}

fn repack_members(members: &[ValidatedGzipMember]) -> Result<Vec<u8>, ArchiveSealError> {
    let encoder = deterministic_gzip(Vec::new());
    let mut builder = Builder::new(encoder);
    for member in members {
        let mut header = Header::new_gnu();
        header.set_entry_type(match member.kind {
            GzipMemberKind::File => EntryType::Regular,
            GzipMemberKind::Directory => EntryType::Directory,
        });
        header.set_path(&member.path)?;
        header.set_size(match member.kind {
            GzipMemberKind::File => member.bytes.len() as u64,
            GzipMemberKind::Directory => 0,
        });
        header.set_mode(member.mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_username("")?;
        header.set_groupname("")?;
        header.set_mtime(0);
        header.set_cksum();
        match member.kind {
            GzipMemberKind::File => builder.append(&header, member.bytes.as_slice())?,
            GzipMemberKind::Directory => builder.append(&header, io::empty())?,
        }
    }
    builder.finish()?;
    Ok(builder.into_inner()?.finish()?)
}

fn verify_repacked_archive(
    staged_dest: &str,
    slot: &ArchiveSlot,
    source_members: &[ValidatedGzipMember],
    repacked: &[u8],
) -> Result<(), ArchiveSealError> {
    let reopened = validate_gzip_archive(staged_dest, repacked, slot)
        .map_err(|error| ArchiveSealError::new(error.to_string()))?;
    if reopened.members.len() != source_members.len() {
        return Err(ArchiveSealError::new(format!(
            "repacked archive member multiplicity changed for {staged_dest}"
        )));
    }
    for (source, rebuilt) in source_members.iter().zip(&reopened.members) {
        if source.path != rebuilt.path || source.kind != rebuilt.kind || source.mode != rebuilt.mode
        {
            return Err(ArchiveSealError::new(format!(
                "repacked archive member metadata changed for {}",
                source.path
            )));
        }
        if !source.macho && source.bytes != rebuilt.bytes {
            return Err(ArchiveSealError::new(format!(
                "repacked non-executable member changed for {}",
                source.path
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn seal_declared_archives_with_hook(
    checkout: &Path,
    inventory: &Inventory,
    target_id: &str,
    signer: &dyn ArchiveMemberSigner,
    before_revalidate: &mut dyn FnMut(&Path),
) -> Result<SealedArchiveSet, ArchiveSealError> {
    seal_declared_archives_inner(
        checkout,
        inventory,
        target_id,
        signer,
        Some(before_revalidate),
    )
}

#[cfg(test)]
mod tests {
    use std::io;

    use tar::{Builder, EntryType, Header};
    use tempfile::TempDir;

    use super::*;
    use crate::apple::{AppleError, FakeArchiveMemberSigner, SignedMember};
    use crate::archive_census::{validate_gzip_archive, validate_staged_archives};
    use crate::archive_contract::{DeliveryContract, PrebuildInputIdentity};
    use crate::macho::{FixtureSpec, fixture};
    use crate::stage::write_staged_file;

    const SYNTHETIC_DEST: &str = "lib/fixture/synthetic.tar.gz";
    const SYNTHETIC_EXECUTABLE: &str = "synthetic/bin/second-cli";

    struct PanicArchiveMemberSigner;

    impl ArchiveMemberSigner for PanicArchiveMemberSigner {
        fn sign_executable(
            &self,
            _path: &Path,
            _relative_member_path: &str,
        ) -> Result<SignedMember, AppleError> {
            panic!("declared executable digests must be validated before signing")
        }
    }

    fn gzip_tar(entries: &[(&str, EntryType, u32, &[u8])]) -> Vec<u8> {
        let encoder = deterministic_gzip(Vec::new());
        let mut builder = Builder::new(encoder);
        for (path, kind, mode, bytes) in entries {
            let mut header = Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_path(path).expect("fixture path");
            header.set_size(if kind.is_file() {
                bytes.len() as u64
            } else {
                0
            });
            header.set_mode(*mode);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            if kind.is_file() {
                builder.append(&header, *bytes).expect("fixture file");
            } else {
                builder
                    .append(&header, io::empty())
                    .expect("fixture directory");
            }
        }
        builder.finish().expect("finish fixture tar");
        builder
            .into_inner()
            .expect("fixture encoder")
            .finish()
            .expect("finish fixture gzip")
    }

    fn synthetic_inventory() -> Inventory {
        toml_edit::de::from_str(&format!(
            r#"
version = 1
product = "fixture"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "core/payload"
deny = []
[artifact]
basename = "fixture-{{version}}-{{os}}-{{arch}}"
[[target]]
id = "macos-arm64"
os = "macos"
arch = "arm64"
lane = "apple-native"
triple_apple = "aarch64-apple-darwin"
min_macos = "14.0"
[apple]
team_id = "team"
app_identity = "app"
installer_identity = "installer"
notary_profile = "profile"
keychain = "keychain"
pkg_identifier = "app.fixture"
install_location = "/usr/local"
codesign_path = "codesign"
xcode = "xcode"
notarytool = "notarytool"
[[entry]]
kind = "model-asset"
source = "assets/synthetic.tar.gz"
dest = "{SYNTHETIC_DEST}"
mode = 0o644
digest_const = "SYNTHETIC_ARCHIVE_SHA256"
digest_source = "fixtures.rs"
archive_slot = {{ id = "synthetic-second-archive", target = "macos-arm64", container = "gzip-tar", executables = [
  {{ path = "{SYNTHETIC_EXECUTABLE}", digest_const = "SYNTHETIC_BINARY_SHA256", digest_source = "fixtures.rs" }},
] }}
targets = ["macos-arm64"]
"#,
        ))
        .expect("synthetic inventory")
    }

    fn synthetic_checkout() -> (TempDir, PathBuf, Inventory, Vec<u8>) {
        let temporary = TempDir::new().expect("temporary checkout");
        let checkout = temporary.path().join("checkout");
        let binary = fixture(&FixtureSpec::default());
        let archive = gzip_tar(&[
            ("synthetic", EntryType::Directory, 0o755, b""),
            ("synthetic/LICENSE", EntryType::Regular, 0o644, b"AGPL"),
            ("synthetic/bin", EntryType::Directory, 0o755, b""),
            (SYNTHETIC_EXECUTABLE, EntryType::Regular, 0o755, &binary),
        ]);
        fs::create_dir_all(checkout.join("assets")).expect("asset directory");
        fs::write(checkout.join("assets/synthetic.tar.gz"), &archive).expect("archive");
        fs::write(
            checkout.join("fixtures.rs"),
            format!(
                "pub const SYNTHETIC_ARCHIVE_SHA256: &str = \"{}\";\npub const SYNTHETIC_BINARY_SHA256: &str = \"{}\";\n",
                sha256_hex(&archive),
                sha256_hex(&binary)
            ),
        )
        .expect("digest source");
        let inventory = synthetic_inventory();
        (temporary, checkout, inventory, archive)
    }

    fn copied_rfdetr_checkout() -> (TempDir, PathBuf, Inventory, Vec<u8>) {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let inventory_path = repository.join("core/distribution/inventory.toml");
        let inventory = crate::inventory::load_inventory(&inventory_path).expect("inventory");
        let (source, digest_source) = inventory
            .entry
            .iter()
            .find_map(|entry| match entry {
                Entry::ModelAsset {
                    source,
                    digest_source,
                    archive_slot: Some(_),
                    ..
                } => Some((source, digest_source)),
                _ => None,
            })
            .expect("RF-DETR slot");
        let temporary = TempDir::new().expect("temporary checkout");
        let checkout = temporary.path().join("checkout");
        let source_dest = checkout.join(source);
        let digest_dest = checkout.join(digest_source);
        fs::create_dir_all(source_dest.parent().expect("source parent")).expect("source parent");
        fs::create_dir_all(digest_dest.parent().expect("digest parent")).expect("digest parent");
        let archive = fs::read(repository.join(source)).expect("RF-DETR source");
        fs::write(&source_dest, &archive).expect("copy archive");
        fs::copy(repository.join(digest_source), &digest_dest).expect("copy digest source");
        (temporary, checkout, inventory, archive)
    }

    fn archive_slot(inventory: &Inventory) -> &ArchiveSlot {
        inventory
            .entry
            .iter()
            .find_map(|entry| match entry {
                Entry::ModelAsset {
                    archive_slot: Some(slot),
                    ..
                } => Some(slot),
                _ => None,
            })
            .expect("archive slot")
    }

    #[test]
    fn seals_the_real_rfdetr_archive_and_binds_its_delivery_contract() {
        let (_temporary, checkout, inventory, original) = copied_rfdetr_checkout();
        let signer = FakeArchiveMemberSigner::new("prepared-one");
        let sealed = seal_declared_archives(&checkout, &inventory, "macos-arm64", &signer)
            .expect("seal RF-DETR");
        assert_eq!(sealed.archives.len(), 1);
        let archive = &sealed.archives[0];
        assert_ne!(archive.bytes, original);

        let slot = archive_slot(&inventory);
        let original =
            validate_gzip_archive(&archive.staged_dest, &original, slot).expect("source");
        let rebuilt =
            validate_gzip_archive(&archive.staged_dest, &archive.bytes, slot).expect("sealed");
        assert_eq!(original.members.len(), rebuilt.members.len());
        for (source, signed) in original.members.iter().zip(&rebuilt.members) {
            assert_eq!(source.path, signed.path);
            assert_eq!(source.kind, signed.kind);
            assert_eq!(source.mode, signed.mode);
            if !source.macho {
                assert_eq!(source.bytes, signed.bytes);
            }
        }
        let source_cli = original
            .members
            .iter()
            .find(|member| member.path.ends_with("/rfdetr-cli"))
            .expect("source cli");
        let signed_cli = rebuilt
            .members
            .iter()
            .find(|member| member.path == source_cli.path)
            .expect("signed cli");
        assert_eq!(source_cli.mode, 0o755);
        assert_eq!(signed_cli.mode, source_cli.mode);
        assert_ne!(source_cli.bytes, signed_cli.bytes);

        let staged = TempDir::new().expect("temporary stage");
        write_staged_file(staged.path(), &archive.staged_dest, &archive.bytes)
            .expect("stage sealed");
        validate_staged_archives(staged.path(), &inventory).expect("sealed archive census");

        let prebuild = PrebuildInputIdentity::from_sealed_archives(
            "macos-arm64",
            "commit",
            &"a".repeat(64),
            b"inventory",
            &sealed,
        );
        let delivery = DeliveryContract::from_sealed_archives(&prebuild, &sealed);
        assert_eq!(delivery.prebuild_input_sha256, prebuild.digest());
    }

    #[test]
    fn synthetic_second_slot_seals_and_mode_mutation_refuses() {
        let (_temporary, checkout, inventory, original) = synthetic_checkout();
        let signer = FakeArchiveMemberSigner::new("synthetic");
        let sealed = seal_declared_archives(&checkout, &inventory, "macos-arm64", &signer)
            .expect("synthetic slot seals");
        assert_ne!(sealed.archives[0].bytes, original);

        let (_temporary, checkout, inventory, _) = synthetic_checkout();
        let broken = FakeArchiveMemberSigner::with_mode_mutation("mode-break");
        let error = seal_declared_archives(&checkout, &inventory, "macos-arm64", &broken)
            .expect_err("mode mutation refuses");
        assert!(error.to_string().contains("mode mutation"));
    }

    #[test]
    fn declared_executable_digest_refuses_before_signing_then_corrected_fixture_seals() {
        let (_temporary, checkout, inventory, archive) = synthetic_checkout();
        let executable = validate_gzip_archive(SYNTHETIC_DEST, &archive, archive_slot(&inventory))
            .expect("synthetic source archive")
            .members
            .into_iter()
            .find(|member| member.path == SYNTHETIC_EXECUTABLE)
            .expect("synthetic executable");

        fs::write(
            checkout.join("fixtures.rs"),
            format!(
                "pub const SYNTHETIC_ARCHIVE_SHA256: &str = \"{}\";\npub const SYNTHETIC_BINARY_SHA256: &str = \"{}\";\n",
                sha256_hex(&archive),
                "0".repeat(64),
            ),
        )
        .expect("write mismatched executable digest");
        let error = seal_declared_archives(
            &checkout,
            &inventory,
            "macos-arm64",
            &PanicArchiveMemberSigner,
        )
        .expect_err("mismatched declared executable digest refuses before signing");
        assert!(error.to_string().contains("synthetic-second-archive"));
        assert!(error.to_string().contains(SYNTHETIC_EXECUTABLE));
        assert!(error.to_string().contains("expected"));
        assert!(error.to_string().contains("observed"));

        fs::write(
            checkout.join("fixtures.rs"),
            format!(
                "pub const SYNTHETIC_ARCHIVE_SHA256: &str = \"{}\";\npub const SYNTHETIC_BINARY_SHA256: &str = \"{}\";\n",
                sha256_hex(&archive),
                sha256_hex(&executable.bytes),
            ),
        )
        .expect("write matching executable digest");
        seal_declared_archives(
            &checkout,
            &inventory,
            "macos-arm64",
            &FakeArchiveMemberSigner::new("corrected-digest"),
        )
        .expect("corrected declared executable digest seals");
    }

    /// This reuses one checkout to model separate producer invocations. A
    /// `produce::run`-level proof would require a detached checkout, build,
    /// and real Apple signing credentials, which are not hermetic here.
    #[test]
    fn separate_seal_invocations_do_not_leak_signed_derivatives() {
        let (_temporary, checkout, inventory, _) = synthetic_checkout();
        let first = seal_declared_archives(
            &checkout,
            &inventory,
            "macos-arm64",
            &FakeArchiveMemberSigner::new("invocation-one"),
        )
        .expect("first invocation seals");
        let first_archive = first.archives.first().expect("first sealed archive");
        let first_bytes = first_archive.bytes.clone();
        let first_sha256 = first_archive.sha256.clone();

        let second = seal_declared_archives(
            &checkout,
            &inventory,
            "macos-arm64",
            &FakeArchiveMemberSigner::new("invocation-two"),
        )
        .expect("second invocation seals");
        let second_archive = second.archives.first().expect("second sealed archive");

        assert_eq!(first.archives[0].bytes, first_bytes);
        assert_eq!(first.archives[0].sha256, first_sha256);
        assert_ne!(second_archive.bytes, first_bytes);
        assert_ne!(second_archive.sha256, first_sha256);

        let slot = archive_slot(&inventory);
        let sealed =
            validate_gzip_archive(&second_archive.staged_dest, &second_archive.bytes, slot)
                .expect("second sealed archive reopens");
        let executable = sealed
            .members
            .iter()
            .find(|member| member.path == SYNTHETIC_EXECUTABLE)
            .expect("second signed executable");
        assert!(
            executable.bytes.ends_with(
                format!("\nSOLSTONE-FAKE-ARCHIVE-SIGNATURE:invocation-two:{SYNTHETIC_EXECUTABLE}")
                    .as_bytes()
            )
        );
        assert!(
            !executable
                .bytes
                .windows(b"SOLSTONE-FAKE-ARCHIVE-SIGNATURE:invocation-one".len())
                .any(|bytes| bytes == b"SOLSTONE-FAKE-ARCHIVE-SIGNATURE:invocation-one")
        );
    }

    #[test]
    fn source_swap_between_validation_and_signing_refuses_then_restoring_source_seals() {
        let (_temporary, checkout, inventory, original) = synthetic_checkout();
        let signer = FakeArchiveMemberSigner::new("swap");
        let mut swap = |source: &Path| fs::write(source, b"swapped archive").expect("swap source");
        let result = seal_declared_archives_with_hook(
            &checkout,
            &inventory,
            "macos-arm64",
            &signer,
            &mut swap,
        );
        assert!(result.is_err());
        assert!(
            result
                .expect_err("swapped archive is refused")
                .to_string()
                .contains("digest")
        );

        fs::write(checkout.join("assets/synthetic.tar.gz"), original)
            .expect("restore unsigned source");
        seal_declared_archives(&checkout, &inventory, "macos-arm64", &signer)
            .expect("restored source seals");
    }
}
