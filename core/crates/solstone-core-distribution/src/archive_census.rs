// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Refuse undeclared or unsafe archive payloads in a staged tree.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::archive_taxonomy::{ContainerKind, classify_container};
use crate::inventory::{ArchiveSlot, Entry, Inventory, validate_archive_member_path};
use crate::macho::looks_like_macho;
use crate::record::FileRecord;
use crate::stage::staged_records;
use crate::tar::{gunzip_bytes, tar_records};

#[derive(Debug)]
pub struct ArchiveCensusError {
    message: String,
}

impl ArchiveCensusError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ArchiveCensusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ArchiveCensusError {}

#[derive(Debug, Clone)]
pub struct ValidatedArchiveSlot {
    pub staged_path: String,
    pub slot: ArchiveSlot,
    pub records: Vec<FileRecord>,
    pub native_members: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GzipMemberKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedGzipMember {
    pub path: String,
    pub kind: GzipMemberKind,
    pub mode: u32,
    pub bytes: Vec<u8>,
    pub macho: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedGzipArchive {
    pub members: Vec<ValidatedGzipMember>,
    pub records: Vec<FileRecord>,
    pub native_members: Vec<(String, Vec<u8>)>,
}

/// Census every staged file that declares a recognized archive signature.
///
/// A recognized container has no benign default: the staged destination must
/// name exactly one matching archive slot. This is intentionally independent
/// of filename extensions.
pub fn validate_staged_archives(
    stage: &Path,
    inventory: &Inventory,
    checkout: &Path,
) -> Result<Vec<ValidatedArchiveSlot>, ArchiveCensusError> {
    let mut validated = Vec::new();
    for record in staged_records(stage).map_err(|error| {
        ArchiveCensusError::new(format!("enumerate staged archive census: {error}"))
    })? {
        let bytes = fs::read(stage.join(&record.dest)).map_err(|error| {
            ArchiveCensusError::new(format!("read staged archive {}: {error}", record.dest))
        })?;
        let Some(observed) = classify_container(&bytes) else {
            continue;
        };
        let slots = archive_slots_at(inventory, &record.dest);
        let slot = match slots.as_slice() {
            [] => {
                return Err(ArchiveCensusError::new(format!(
                    "undeclared recognized archive {} ({observed:?})",
                    record.dest
                )));
            }
            [slot] => *slot,
            _ => {
                return Err(ArchiveCensusError::new(format!(
                    "duplicate slot for staged archive {}",
                    record.dest
                )));
            }
        };
        if slot.container != observed {
            return Err(ArchiveCensusError::new(format!(
                "unsupported declared encoding for {}: declared {:?}, observed {observed:?}",
                record.dest, slot.container
            )));
        }
        if slot.container != ContainerKind::GzipTar {
            return Err(ArchiveCensusError::new(format!(
                "unsupported declared encoding for {}: {:?}",
                record.dest, slot.container
            )));
        }
        let validated_archive = validate_gzip_archive(&record.dest, &bytes, slot)?;
        if slot.inspect_only {
            for executable in &slot.executables {
                let member = validated_archive
                    .members
                    .iter()
                    .find(|m| m.path == executable.path)
                    .ok_or_else(|| {
                        ArchiveCensusError::new(format!(
                            "missing declared executable in {}: {}",
                            record.dest, executable.path
                        ))
                    })?;
                let actual_digest = crate::digest::sha256_hex(&member.bytes);
                let source_path = checkout.join(&executable.digest_source);
                let source = fs::read_to_string(&source_path).map_err(|error| {
                    ArchiveCensusError::new(format!(
                        "read archive executable digest source {}: {error}",
                        source_path.display()
                    ))
                })?;
                let expected_digest =
                    crate::inventory::digest_const_hex(&source, &executable.digest_const)
                        .ok_or_else(|| {
                            ArchiveCensusError::new(format!(
                                "missing required:\n  digest {}",
                                executable.digest_const
                            ))
                        })?;
                if actual_digest != expected_digest {
                    return Err(ArchiveCensusError::new(format!(
                        "digest mismatch in {} for {}: expected {expected_digest}, got {actual_digest}",
                        record.dest, executable.path
                    )));
                }
            }
        }
        validated.push(ValidatedArchiveSlot {
            staged_path: record.dest,
            slot: slot.clone(),
            records: validated_archive.records,
            native_members: validated_archive.native_members,
        });
    }
    Ok(validated)
}

fn archive_slots_at<'a>(inventory: &'a Inventory, dest: &str) -> Vec<&'a ArchiveSlot> {
    inventory
        .entry
        .iter()
        .filter_map(|entry| match entry {
            Entry::ModelAsset {
                dest: entry_dest,
                archive_slot: Some(slot),
                ..
            } if entry_dest == dest => Some(slot),
            _ => None,
        })
        .collect()
}

pub(crate) fn validate_gzip_archive(
    staged_path: &str,
    compressed: &[u8],
    slot: &ArchiveSlot,
) -> Result<ValidatedGzipArchive, ArchiveCensusError> {
    let uncompressed = gunzip_bytes(compressed).map_err(|error| {
        ArchiveCensusError::new(format!("open gzip archive {staged_path}: {error}"))
    })?;
    let mut archive = tar::Archive::new(uncompressed.as_slice());
    let mut member_paths = BTreeSet::new();
    let mut member_collision_keys = BTreeSet::new();
    let mut macho_paths = BTreeSet::new();
    let mut elf_paths = BTreeSet::new();
    let mut members = Vec::new();
    for entry in archive.entries().map_err(|error| {
        ArchiveCensusError::new(format!("enumerate gzip archive {staged_path}: {error}"))
    })? {
        let mut entry = entry.map_err(|error| {
            ArchiveCensusError::new(format!("read gzip archive {staged_path}: {error}"))
        })?;
        let kind = entry.header().entry_type();
        let raw_path = entry.path().map_err(|error| {
            ArchiveCensusError::new(format!(
                "read archive member path in {staged_path}: {error}"
            ))
        })?;
        let raw_path = raw_path
            .to_str()
            .ok_or_else(|| {
                ArchiveCensusError::new(format!("non-UTF-8 archive member path in {staged_path}"))
            })?
            .to_owned();
        let member_path = if kind.is_dir() {
            raw_path.trim_end_matches('/')
        } else {
            &raw_path
        };
        crate::archive::refuse_escape(member_path).map_err(|error| {
            ArchiveCensusError::new(format!(
                "archive member path escape in {staged_path}: {}",
                error.as_str()
            ))
        })?;
        validate_archive_member_path(member_path).map_err(|reason| {
            ArchiveCensusError::new(format!(
                "non-canonical archive member path in {staged_path}: {member_path} ({reason})"
            ))
        })?;
        let mode = entry.header().mode().map_err(|error| {
            ArchiveCensusError::new(format!(
                "read archive member mode in {staged_path}: {error}"
            ))
        })?;

        let mut bytes = Vec::new();
        if kind.is_file() {
            entry.read_to_end(&mut bytes).map_err(|error| {
                ArchiveCensusError::new(format!(
                    "read archive member {member_path} in {staged_path}: {error}"
                ))
            })?;
        }
        let is_macho = kind.is_file() && looks_like_macho(&bytes);
        let is_elf = kind.is_file() && bytes.starts_with(b"\x7fELF");
        let is_executable = if slot.inspect_only { is_elf } else { is_macho };

        if !member_paths.insert(member_path.to_owned()) {
            let label = if is_executable {
                "duplicate executable name"
            } else {
                "duplicate archive member path"
            };
            return Err(ArchiveCensusError::new(format!(
                "{label} in {staged_path}: {member_path}"
            )));
        }
        let collision_key = member_path.to_lowercase();
        if !member_collision_keys.insert(collision_key) {
            let label = if is_executable {
                "duplicate executable name"
            } else {
                "case-or-Unicode-colliding archive member path"
            };
            return Err(ArchiveCensusError::new(format!(
                "{label} in {staged_path}: {member_path}"
            )));
        }

        if kind.is_dir() {
            members.push(ValidatedGzipMember {
                path: member_path.to_owned(),
                kind: GzipMemberKind::Directory,
                mode,
                bytes,
                macho: false,
            });
            continue;
        }
        if kind.is_symlink()
            || kind.is_hard_link()
            || kind.is_character_special()
            || kind.is_block_special()
        {
            return Err(ArchiveCensusError::new(format!(
                "symlink/hardlink/device member in {staged_path}: {member_path}"
            )));
        }
        if !kind.is_file() {
            return Err(ArchiveCensusError::new(format!(
                "unsupported archive member in {staged_path}: {member_path}"
            )));
        }
        if slot.inspect_only && is_macho {
            return Err(ArchiveCensusError::new(format!(
                "unexpected Mach-O in {staged_path}: {member_path}"
            )));
        }
        if is_macho && !macho_paths.insert(member_path.to_owned()) {
            return Err(ArchiveCensusError::new(format!(
                "duplicate executable name in {staged_path}: {member_path}"
            )));
        }
        if is_elf && !elf_paths.insert(member_path.to_owned()) {
            return Err(ArchiveCensusError::new(format!(
                "duplicate executable name in {staged_path}: {member_path}"
            )));
        }
        members.push(ValidatedGzipMember {
            path: member_path.to_owned(),
            kind: GzipMemberKind::File,
            mode,
            bytes,
            macho: is_macho,
        });
    }

    let expected = slot
        .executables
        .iter()
        .map(|executable| executable.path.as_str())
        .collect::<BTreeSet<_>>();

    let native_members = if slot.inspect_only {
        let observed = elf_paths
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let missing = expected.difference(&observed).copied().collect::<Vec<_>>();
        let unexpected = observed.difference(&expected).copied().collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(ArchiveCensusError::new(format!(
                "missing declared ELF in {staged_path}: {}",
                missing.join(", ")
            )));
        }
        if !unexpected.is_empty() {
            return Err(ArchiveCensusError::new(format!(
                "unexpected ELF in {staged_path}: {}",
                unexpected.join(", ")
            )));
        }

        members
            .iter()
            .filter(|m| elf_paths.contains(&m.path))
            .map(|m| (m.path.clone(), m.bytes.clone()))
            .collect()
    } else {
        let observed = macho_paths
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let missing = expected.difference(&observed).copied().collect::<Vec<_>>();
        let unexpected = observed.difference(&expected).copied().collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(ArchiveCensusError::new(format!(
                "missing declared executable in {staged_path}: {}",
                missing.join(", ")
            )));
        }
        if !unexpected.is_empty() {
            return Err(ArchiveCensusError::new(format!(
                "unexpected second Mach-O in {staged_path}: {}",
                unexpected.join(", ")
            )));
        }

        members
            .iter()
            .filter(|m| m.macho)
            .map(|m| (m.path.clone(), m.bytes.clone()))
            .collect()
    };

    let records = tar_records(compressed).map_err(|error| {
        ArchiveCensusError::new(format!("record gzip archive {staged_path}: {error}"))
    })?;
    Ok(ValidatedGzipArchive {
        members,
        records,
        native_members,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write};

    use tar::{Builder, EntryType, Header};
    use tempfile::TempDir;

    use super::validate_staged_archives;
    use crate::archive_taxonomy::ContainerKind;
    use crate::inventory::Inventory;
    use crate::macho::{FixtureSpec, fixture};
    use crate::stage::write_staged_file;
    use crate::tar::{deterministic_gzip, gunzip_bytes};

    const SLOT_ID: &str = "fixture-archive-slot";
    const ARCHIVE_DEST: &str = "lib/fixture/archive.tar.gz";
    const EXECUTABLE_PATH: &str = "fixture/bin/rfdetr-cli";

    fn fixture_inventory(entries: &str) -> Inventory {
        toml_edit::de::from_str(&format!(
            r#"
version = 1
product = "fixture"
payload = "payload.txt"
payload_dest_prefix = "share"
payload_src_root = "core/payload"
{empty_entry}
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
notary_profile = "profile"
keychain = "keychain"
codesign_path = "codesign"
xcode = "xcode"
notarytool = "notarytool"
{entries}
"#,
            empty_entry = if entries.trim().is_empty() {
                "entry = []"
            } else {
                ""
            }
        ))
        .expect("fixture inventory parses")
    }

    fn slot_entry(id: &str, dest: &str, container: ContainerKind, executables: &[&str]) -> String {
        let container = match container {
            ContainerKind::RawTar => "raw-tar",
            ContainerKind::GzipTar => "gzip-tar",
            ContainerKind::Bzip2Tar => "bzip2-tar",
            ContainerKind::XzTar => "xz-tar",
            ContainerKind::ZstdTar => "zstd-tar",
            ContainerKind::Zip => "zip",
        };
        let executables = executables
            .iter()
            .map(|path| {
                format!(
                    "{{ path = \"{path}\", digest_const = \"FIXTURE_BINARY_SHA256\", digest_source = \"fixtures.rs\" }}"
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"
[[entry]]
kind = "model-asset"
source = "fixture.tar.gz"
dest = "{dest}"
mode = 0o644
digest_const = "FIXTURE_ARCHIVE_SHA256"
digest_source = "fixtures.rs"
archive_slot = {{ id = "{id}", target = "macos-arm64", container = "{container}", executables = [{executables}] }}
targets = ["macos-arm64"]
"#
        )
    }

    fn gzip_tar(entries: &[(&str, EntryType, &[u8])]) -> Vec<u8> {
        let encoder = deterministic_gzip(Vec::new());
        let mut builder = Builder::new(encoder);
        for (path, kind, bytes) in entries {
            let mut header = Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_path(path).expect("fixture path");
            header.set_mode(0o755);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            if kind.is_file() {
                header.set_size(bytes.len() as u64);
                header.set_cksum();
                builder
                    .append(&header, *bytes)
                    .expect("append fixture file");
            } else {
                header.set_size(0);
                if kind.is_symlink() || kind.is_hard_link() {
                    header.set_link_name("target").expect("fixture link target");
                }
                header.set_cksum();
                builder
                    .append(&header, io::empty())
                    .expect("append fixture member");
            }
        }
        builder.finish().expect("finish fixture tar");
        builder
            .into_inner()
            .expect("fixture encoder")
            .finish()
            .expect("finish fixture gzip")
    }

    fn stage_archive(temporary: &TempDir, dest: &str, bytes: &[u8]) {
        write_staged_file(temporary.path(), dest, bytes).expect("stage archive");
    }

    fn macho() -> Vec<u8> {
        fixture(&FixtureSpec::default())
    }

    #[test]
    fn accepts_the_real_rfdetr_macos_archive() {
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let inventory_path = repository.join("core/distribution/inventory.toml");
        let inventory = crate::inventory::load_inventory(&inventory_path).expect("inventory");
        let (source, dest) = inventory
            .entry
            .iter()
            .find_map(|entry| match entry {
                crate::inventory::Entry::ModelAsset {
                    source,
                    dest,
                    archive_slot: Some(_),
                    ..
                } => Some((source, dest)),
                _ => None,
            })
            .expect("RF-DETR archive slot");
        let temporary = TempDir::new().expect("temporary stage");
        stage_archive(
            &temporary,
            dest,
            &fs::read(repository.join(source)).expect("RF-DETR"),
        );
        let validated = validate_staged_archives(temporary.path(), &inventory, &repository)
            .expect("valid archive");
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].slot.id, "rfdetr-macos-metal-arm64");
    }

    #[test]
    fn accepts_a_second_synthetic_archive_slot() {
        let synthetic_path = "synthetic/nested/second-cli";
        let inventory = fixture_inventory(&slot_entry(
            "synthetic-second-archive",
            "lib/fixture/second.tar.gz",
            ContainerKind::GzipTar,
            &[synthetic_path],
        ));
        let temporary = TempDir::new().expect("temporary stage");
        stage_archive(
            &temporary,
            "lib/fixture/second.tar.gz",
            &gzip_tar(&[(synthetic_path, EntryType::Regular, &macho())]),
        );
        let validated = validate_staged_archives(temporary.path(), &inventory, temporary.path())
            .expect("valid archive");
        assert_eq!(validated[0].slot.id, "synthetic-second-archive");
    }

    #[test]
    fn refuses_an_undeclared_recognized_archive() {
        let temporary = TempDir::new().expect("temporary stage");
        stage_archive(
            &temporary,
            ARCHIVE_DEST,
            &gzip_tar(&[(EXECUTABLE_PATH, EntryType::Regular, &macho())]),
        );
        let error =
            validate_staged_archives(temporary.path(), &fixture_inventory(""), temporary.path())
                .expect_err("undeclared archive is refused")
                .to_string();
        assert!(error.contains("undeclared recognized archive"), "{error}");
    }

    #[test]
    fn refuses_duplicate_slots_for_one_staged_path() {
        let inventory = fixture_inventory(&format!(
            "{}{}",
            slot_entry(
                SLOT_ID,
                ARCHIVE_DEST,
                ContainerKind::GzipTar,
                &[EXECUTABLE_PATH]
            ),
            slot_entry(
                "other-slot",
                ARCHIVE_DEST,
                ContainerKind::GzipTar,
                &[EXECUTABLE_PATH]
            )
        ));
        let temporary = TempDir::new().expect("temporary stage");
        stage_archive(
            &temporary,
            ARCHIVE_DEST,
            &gzip_tar(&[(EXECUTABLE_PATH, EntryType::Regular, &macho())]),
        );
        let error = validate_staged_archives(temporary.path(), &inventory, temporary.path())
            .expect_err("duplicate slots are refused")
            .to_string();
        assert!(error.contains("duplicate slot"), "{error}");
    }

    #[test]
    fn refuses_a_container_that_disagrees_with_its_slot() {
        let inventory = fixture_inventory(&slot_entry(
            SLOT_ID,
            ARCHIVE_DEST,
            ContainerKind::RawTar,
            &[EXECUTABLE_PATH],
        ));
        let temporary = TempDir::new().expect("temporary stage");
        stage_archive(
            &temporary,
            ARCHIVE_DEST,
            &gzip_tar(&[(EXECUTABLE_PATH, EntryType::Regular, &macho())]),
        );
        let error = validate_staged_archives(temporary.path(), &inventory, temporary.path())
            .expect_err("mismatched container is refused")
            .to_string();
        assert!(error.contains("unsupported declared encoding"), "{error}");
    }

    #[test]
    fn refuses_each_non_gzip_declared_container() {
        let fixtures = [
            (ContainerKind::RawTar, raw_tar_signature()),
            (ContainerKind::Bzip2Tar, b"BZh9".to_vec()),
            (
                ContainerKind::XzTar,
                vec![0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00],
            ),
            (ContainerKind::ZstdTar, vec![0x28, 0xb5, 0x2f, 0xfd]),
            (ContainerKind::Zip, b"PK\x03\x04".to_vec()),
        ];
        for (kind, bytes) in fixtures {
            let inventory = fixture_inventory(&slot_entry(SLOT_ID, ARCHIVE_DEST, kind, &[]));
            let temporary = TempDir::new().expect("temporary stage");
            stage_archive(&temporary, ARCHIVE_DEST, &bytes);
            let error = validate_staged_archives(temporary.path(), &inventory, temporary.path())
                .expect_err("non-gzip container is refused")
                .to_string();
            assert!(error.contains("unsupported declared encoding"), "{error}");
        }
    }

    #[test]
    fn refuses_missing_and_unexpected_macho_members() {
        let inventory = fixture_inventory(&slot_entry(
            SLOT_ID,
            ARCHIVE_DEST,
            ContainerKind::GzipTar,
            &[EXECUTABLE_PATH],
        ));
        let temporary = TempDir::new().expect("temporary stage");
        stage_archive(
            &temporary,
            ARCHIVE_DEST,
            &gzip_tar(&[("fixture/bin/other-cli", EntryType::Regular, &macho())]),
        );
        let error = validate_staged_archives(temporary.path(), &inventory, temporary.path())
            .expect_err("missing member is refused")
            .to_string();
        assert!(error.contains("missing declared executable"), "{error}");

        let temporary = TempDir::new().expect("temporary stage");
        stage_archive(
            &temporary,
            ARCHIVE_DEST,
            &gzip_tar(&[
                (EXECUTABLE_PATH, EntryType::Regular, &macho()),
                ("fixture/bin/second-cli", EntryType::Regular, &macho()),
            ]),
        );
        let error = validate_staged_archives(temporary.path(), &inventory, temporary.path())
            .expect_err("unexpected member is refused")
            .to_string();
        assert!(error.contains("unexpected second Mach-O"), "{error}");
    }

    #[test]
    fn refuses_duplicate_executable_names_links_and_path_escapes() {
        let inventory = fixture_inventory(&slot_entry(
            SLOT_ID,
            ARCHIVE_DEST,
            ContainerKind::GzipTar,
            &[EXECUTABLE_PATH],
        ));
        for (entries, expected) in [
            (
                vec![
                    (EXECUTABLE_PATH, EntryType::Regular, macho()),
                    (EXECUTABLE_PATH, EntryType::Regular, macho()),
                ],
                "duplicate executable name",
            ),
            (
                vec![
                    (EXECUTABLE_PATH, EntryType::Regular, macho()),
                    ("fixture/link", EntryType::Symlink, Vec::new()),
                ],
                "symlink/hardlink/device member",
            ),
        ] {
            let temporary = TempDir::new().expect("temporary stage");
            let borrowed = entries
                .iter()
                .map(|(path, kind, bytes)| (*path, *kind, bytes.as_slice()))
                .collect::<Vec<_>>();
            stage_archive(&temporary, ARCHIVE_DEST, &gzip_tar(&borrowed));
            let error = validate_staged_archives(temporary.path(), &inventory, temporary.path())
                .expect_err("unsafe member is refused")
                .to_string();
            assert!(error.contains(expected), "{error}");
        }

        let temporary = TempDir::new().expect("temporary stage");
        stage_archive(
            &temporary,
            ARCHIVE_DEST,
            &gzip_tar_with_path("../escape", &macho()),
        );
        let error = validate_staged_archives(temporary.path(), &inventory, temporary.path())
            .expect_err("path escape is refused")
            .to_string();
        assert!(error.contains("archive member path escape"), "{error}");
    }

    #[test]
    fn inspect_only_slot_reports_exact_elf_member_and_refuses_extra_and_digest_mismatch() {
        let temp = TempDir::new().expect("tempdir");
        let dest = "lib/fixture/archive.tar.gz";
        let member_path = "fixture/bin/tool";
        let elf_bytes = b"\x7fELFfixturebytes";
        let digest = crate::digest::sha256_hex(elf_bytes);

        // Write a digest source file
        let digest_src = "fixtures/consts.rs";
        let digest_src_path = temp.path().join(digest_src);
        fs::create_dir_all(digest_src_path.parent().unwrap()).unwrap();
        fs::write(
            &digest_src_path,
            format!("pub const TOOL_SHA256: &str = \"{digest}\";\n"),
        )
        .unwrap();

        let make_inv = |execs: &[&str], digest_name: &str| {
            let exec_entries = execs
                .iter()
                .map(|p| {
                    format!(
                        "{{ path = \"{p}\", digest_const = \"{digest_name}\", digest_source = \"{digest_src}\" }}"
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            fixture_inventory(&format!(
                r#"
[[entry]]
kind = "model-asset"
source = "fixture.tar.gz"
dest = "{dest}"
mode = 0o644
digest_const = "FIXTURE_ARCHIVE_SHA256"
digest_source = "{digest_src}"
archive_slot = {{ id = "elf-slot", target = "macos-arm64", container = "gzip-tar", inspect_only = true, executables = [{exec_entries}] }}
targets = ["macos-arm64"]
"#
            ))
        };

        // 1. Success case: reports exactly the declared ELF member
        let inv_ok = make_inv(&[member_path], "TOOL_SHA256");
        let archive_bytes = gzip_tar(&[(member_path, EntryType::Regular, elf_bytes)]);
        stage_archive(&temp, dest, &archive_bytes);
        let validated =
            validate_staged_archives(temp.path(), &inv_ok, temp.path()).expect("census succeeds");
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].native_members.len(), 1);
        assert_eq!(validated[0].native_members[0].0, member_path);
        assert_eq!(validated[0].native_members[0].1, elf_bytes);

        // 2. Extra ELF member fails, naming archive and member
        let extra_archive = gzip_tar(&[
            (member_path, EntryType::Regular, elf_bytes),
            ("fixture/bin/extra", EntryType::Regular, b"\x7fELFextra"),
        ]);
        stage_archive(&temp, dest, &extra_archive);
        let err_extra = validate_staged_archives(temp.path(), &inv_ok, temp.path())
            .expect_err("extra ELF member must fail")
            .to_string();
        assert!(err_extra.contains(dest), "{err_extra}");
        assert!(err_extra.contains("fixture/bin/extra"), "{err_extra}");
        assert!(err_extra.contains("ELF"), "{err_extra}");

        // Mach-O member in inspect-only slot fails, naming archive, member, and Mach-O
        let macho_bytes = macho();
        let macho_archive = gzip_tar(&[
            (member_path, EntryType::Regular, elf_bytes),
            ("fixture/bin/macho_tool", EntryType::Regular, &macho_bytes),
        ]);
        stage_archive(&temp, dest, &macho_archive);
        let err_macho = validate_staged_archives(temp.path(), &inv_ok, temp.path())
            .expect_err("Mach-O member in inspect-only must fail")
            .to_string();
        assert!(err_macho.contains(dest), "{err_macho}");
        assert!(err_macho.contains("fixture/bin/macho_tool"), "{err_macho}");
        assert!(err_macho.contains("Mach-O"), "{err_macho}");

        // 3. Digest mismatch fails, naming archive and member
        fs::write(
            &digest_src_path,
            "pub const TOOL_SHA256: &str = \"0000000000000000000000000000000000000000000000000000000000000000\";\n",
        )
        .unwrap();
        stage_archive(&temp, dest, &archive_bytes);
        let err_mismatch = validate_staged_archives(temp.path(), &inv_ok, temp.path())
            .expect_err("digest mismatch must fail")
            .to_string();
        assert!(err_mismatch.contains(dest), "{err_mismatch}");
        assert!(err_mismatch.contains(member_path), "{err_mismatch}");
    }

    #[test]
    fn real_rfdetr_linux_tarballs_admit_and_match_consts() {
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let inventory_path = repository.join("core/distribution/inventory.toml");
        let inventory = crate::inventory::load_inventory(&inventory_path).expect("inventory");

        let cases = [
            (
                "linux-x86_64",
                "core/models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz",
                "lib/solstone_journal_models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz",
                "rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64/rfdetr-cli",
                "RFDETR_ENGINE_LINUX_CPU_X64_TARBALL_SHA256",
                crate::elf::machine_x86_64(),
            ),
            (
                "linux-aarch64",
                "core/models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-arm64.tar.gz",
                "lib/solstone_journal_models/assets/rfdetr/rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-arm64.tar.gz",
                "rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-arm64/rfdetr-cli",
                "RFDETR_ENGINE_LINUX_CPU_ARM64_TARBALL_SHA256",
                crate::elf::machine_aarch64(),
            ),
        ];

        let install_rs = fs::read_to_string(
            repository.join("core/crates/solstone-core-local/src/install/rfdetr_install.rs"),
        )
        .expect("read rfdetr_install.rs");

        for (_target_id, source_rel, dest_rel, member_rel, tarball_const, expected_machine) in cases
        {
            let tarball_bytes = fs::read(repository.join(source_rel)).expect("read tarball");
            let actual_tarball_sha = crate::digest::sha256_hex(&tarball_bytes);
            let expected_tarball_sha =
                crate::inventory::digest_const_hex(&install_rs, tarball_const)
                    .expect("find tarball sha256 const");
            assert_eq!(
                actual_tarball_sha, expected_tarball_sha,
                "tarball sha256 must match {tarball_const}"
            );

            let temp = TempDir::new().expect("tempdir");
            stage_archive(&temp, dest_rel, &tarball_bytes);
            let validated = validate_staged_archives(temp.path(), &inventory, &repository)
                .expect("census real tarball");
            assert_eq!(validated.len(), 1);
            let slot = &validated[0];
            assert_eq!(slot.native_members.len(), 1);
            assert_eq!(slot.native_members[0].0, member_rel);

            let member_bytes = &slot.native_members[0].1;
            let pkg_rel_dir = std::path::Path::new(member_rel)
                .parent()
                .unwrap()
                .to_str()
                .unwrap();
            let staged_files: &[(&str, &[u8])] = &[(dest_rel, &tarball_bytes)];

            crate::elf::admit_elf(
                dest_rel,
                member_bytes,
                expected_machine,
                pkg_rel_dir,
                Some((dest_rel, member_rel)),
                staged_files,
            )
            .expect("admit_elf on nested member succeeds");
        }
    }

    fn gzip_tar_with_path(path: &str, bytes: &[u8]) -> Vec<u8> {
        let compressed = gzip_tar(&[("fixture/safe", EntryType::Regular, bytes)]);
        let mut raw = gunzip_bytes(&compressed).expect("fixture tar");
        raw[..100].fill(0);
        raw[..path.len()].copy_from_slice(path.as_bytes());
        raw[148..156].fill(b' ');
        let checksum = raw[..512]
            .iter()
            .fold(0u32, |sum, byte| sum + u32::from(*byte));
        let checksum = format!("{checksum:06o}\0 ");
        raw[148..156].copy_from_slice(checksum.as_bytes());
        let mut encoder = deterministic_gzip(Vec::new());
        encoder.write_all(&raw).expect("fixture gzip");
        encoder.finish().expect("finish fixture gzip")
    }

    fn raw_tar_signature() -> Vec<u8> {
        let mut bytes = vec![0; 262];
        bytes[257..262].copy_from_slice(b"ustar");
        bytes
    }
}
