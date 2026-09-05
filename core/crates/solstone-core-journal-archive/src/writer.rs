// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::io::{Read, Seek, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::encode::DayWindow;
use crate::manifest::Manifest;
use crate::{ArchiveError, ArchiveMemberName, ArchiveSource};

const EXPORT_MANIFEST: &str = "_export.json";
const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// Failure while writing a portable archive's successful source format.
#[derive(Debug)]
pub(crate) enum ArchiveEncodingError {
    Source {
        member: Option<ArchiveMemberName>,
        source: ArchiveError,
    },
    Write {
        member: Option<ArchiveMemberName>,
        source: WriteFailure,
    },
}

/// The underlying failure from the ZIP writer or its byte sink.
#[derive(Debug)]
pub(crate) enum WriteFailure {
    Zip(zip::result::ZipError),
    Io(std::io::Error),
}

impl fmt::Display for ArchiveEncodingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source {
                member: Some(member),
                source,
            } => write!(formatter, "archive source {}: {source}", member.as_str()),
            Self::Source {
                member: None,
                source,
            } => write!(formatter, "archive source: {source}"),
            Self::Write {
                member: Some(member),
                source,
            } => write!(formatter, "archive write {}: {source}", member.as_str()),
            Self::Write {
                member: None,
                source,
            } => write!(formatter, "archive write: {source}"),
        }
    }
}

impl Error for ArchiveEncodingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
        }
    }
}

impl fmt::Display for WriteFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zip(source) => source.fmt(formatter),
            Self::Io(source) => source.fmt(formatter),
        }
    }
}

impl Error for WriteFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Zip(source) => Some(source),
            Self::Io(source) => Some(source),
        }
    }
}

/// Write one frozen source into a caller-owned portable archive writer.
pub(crate) fn write_archive<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    source: &ArchiveSource,
    manifest: &Manifest,
    day_window: Option<&DayWindow>,
) -> Result<(), ArchiveEncodingError> {
    let sliced_chronicle = day_window.is_some_and(|window| {
        source
            .inventory()
            .entries()
            .iter()
            .any(|entry| window.contains_member(entry.member_name().as_str()))
    });
    let included: Vec<&str> = match day_window {
        Some(_) if sliced_chronicle => vec!["chronicle"],
        Some(_) => Vec::new(),
        None => source
            .inventory()
            .included_root_names()
            .iter()
            .map(|name| name.as_str())
            .collect(),
    };
    if included.is_empty() {
        #[cfg(any(test, feature = "test-hooks"))]
        crate::encode::test_set_boundary(crate::encode::TestBoundary::RootDirectory);
    }
    for root in &included {
        #[cfg(any(test, feature = "test-hooks"))]
        crate::encode::test_set_boundary(crate::encode::TestBoundary::RootDirectory);
        zip.add_directory(*root, directory_options(manifest.timestamp))
            .map_err(|source| zip_error(None, source))?;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    let mut test_first_entry = true;
    for entry in source.inventory().entries() {
        if day_window.is_some_and(|window| !window.contains_member(entry.member_name().as_str())) {
            continue;
        }
        let member = entry.member_name();
        #[cfg(any(test, feature = "test-hooks"))]
        crate::encode::test_before_source_open(member);
        let opened = source
            .open_file(entry)
            .map_err(|source| source_error(Some(member), source))?;
        let expected_size = opened.inventoried_size();
        #[cfg(unix)]
        let mut file = opened.into_file();
        #[cfg(windows)]
        let mut file = std::io::Cursor::new(opened.into_bytes());
        #[cfg(any(test, feature = "test-hooks"))]
        crate::encode::test_set_boundary(if test_first_entry {
            crate::encode::TestBoundary::SourceStart
        } else {
            crate::encode::TestBoundary::MemberTransition
        });
        #[cfg(any(test, feature = "test-hooks"))]
        {
            test_first_entry = false;
        }
        zip.start_file(member.as_str(), file_options(manifest.timestamp))
            .map_err(|source| zip_error(Some(member), source))?;
        copy_inventoried(&mut file, zip, expected_size, member)?;
        #[cfg(any(test, feature = "test-hooks"))]
        if crate::encode::test_take_body_write_failure(member) {
            return Err(io_error(
                Some(member),
                std::io::Error::other("injected body write failure"),
            ));
        }
    }

    let manifest_member = ArchiveMemberName::new(EXPORT_MANIFEST.to_owned());
    #[cfg(any(test, feature = "test-hooks"))]
    crate::encode::test_set_boundary(crate::encode::TestBoundary::ManifestStart);
    zip.start_file(manifest_member.as_str(), file_options(manifest.timestamp))
        .map_err(|source| zip_error(Some(&manifest_member), source))?;
    #[cfg(any(test, feature = "test-hooks"))]
    crate::encode::test_set_boundary(crate::encode::TestBoundary::ManifestPayload);
    zip.write_all(&manifest.json)
        .map_err(|source| io_error(Some(&manifest_member), source))?;
    Ok(())
}

fn directory_options(timestamp: DateTime) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .last_modified_time(timestamp)
        .unix_permissions(0o700)
}

fn file_options(timestamp: DateTime) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(timestamp)
        .large_file(true)
        .unix_permissions(0o600)
}

fn copy_inventoried<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    expected_size: u64,
    member: &ArchiveMemberName,
) -> Result<(), ArchiveEncodingError> {
    let mut copied = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_SIZE];
    while copied < expected_size {
        #[cfg(any(test, feature = "test-hooks"))]
        crate::encode::test_before_source_read(member, copied);
        let remaining = expected_size - copied;
        let request = if remaining > COPY_BUFFER_SIZE as u64 {
            COPY_BUFFER_SIZE
        } else {
            remaining as usize
        };
        let count = reader
            .read(&mut buffer[..request])
            .map_err(|source| read_error(member, source))?;
        if count == 0 {
            return Err(changed_error(member));
        }
        #[cfg(any(test, feature = "test-hooks"))]
        crate::encode::test_set_boundary(crate::encode::TestBoundary::SourcePayload);
        writer
            .write_all(&buffer[..count])
            .map_err(|source| io_error(Some(member), source))?;
        copied += count as u64;
    }

    let mut probe = [0_u8; 1];
    #[cfg(any(test, feature = "test-hooks"))]
    crate::encode::test_before_source_read(member, copied);
    let count = reader
        .read(&mut probe)
        .map_err(|source| read_error(member, source))?;
    if count != 0 {
        return Err(changed_error(member));
    }
    Ok(())
}

fn source_error(member: Option<&ArchiveMemberName>, source: ArchiveError) -> ArchiveEncodingError {
    ArchiveEncodingError::Source {
        member: member.cloned(),
        source,
    }
}

fn read_error(member: &ArchiveMemberName, source: std::io::Error) -> ArchiveEncodingError {
    source_error(
        Some(member),
        ArchiveError::SourceIo {
            operation: "read inventoried file",
            member: Some(member.clone()),
            source,
        },
    )
}

fn changed_error(member: &ArchiveMemberName) -> ArchiveEncodingError {
    source_error(
        Some(member),
        ArchiveError::SourceChanged {
            member: Some(member.clone()),
        },
    )
}

fn zip_error(
    member: Option<&ArchiveMemberName>,
    source: zip::result::ZipError,
) -> ArchiveEncodingError {
    ArchiveEncodingError::Write {
        member: member.cloned(),
        source: WriteFailure::Zip(source),
    }
}

fn io_error(member: Option<&ArchiveMemberName>, source: std::io::Error) -> ArchiveEncodingError {
    ArchiveEncodingError::Write {
        member: member.cloned(),
        source: WriteFailure::Io(source),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::io::{self, Cursor, Read};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use zip::ZipArchive;

    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-journal-archive-writer-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create temporary directory");
            Self { path }
        }

        fn journal(&self) -> PathBuf {
            let root = self.path.join("journal");
            fs::create_dir(&root).expect("create journal");
            root
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write(root: &Path, member: &str, bytes: &[u8]) {
        let path = root.join(member);
        fs::create_dir_all(path.parent().expect("member parent")).expect("create parents");
        fs::write(path, bytes).expect("write member");
    }

    fn archive_bytes_with(
        source: &ArchiveSource,
        solstone_version: &str,
        exported_at: &str,
    ) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let manifest = manifest_for(source, solstone_version, exported_at);
        write_archive(&mut writer, source, &manifest, None).expect("write archive");
        writer.finish().expect("finish archive").into_inner()
    }

    fn archive_bytes(source: &ArchiveSource) -> Vec<u8> {
        archive_bytes_with(source, "0.9.0", "2026-08-07T21:22:23Z")
    }

    fn expected_manifest_with(
        source: &ArchiveSource,
        solstone_version: &str,
        exported_at: &str,
    ) -> Vec<u8> {
        manifest_for(source, solstone_version, exported_at).json
    }

    fn manifest_for(source: &ArchiveSource, solstone_version: &str, exported_at: &str) -> Manifest {
        crate::manifest::build(crate::manifest::ManifestFields {
            solstone_version,
            exported_at,
            source_journal: source
                .canonical_source()
                .to_str()
                .expect("UTF-8 canonical source"),
            day_count: source.inventory().day_count(),
            entity_count: source.inventory().entity_count(),
            facet_count: source.inventory().facet_count(),
        })
        .expect("build manifest")
    }

    fn expected_manifest(source: &ArchiveSource) -> Vec<u8> {
        expected_manifest_with(source, "0.9.0", "2026-08-07T21:22:23Z")
    }

    #[test]
    fn writes_fixed_order_and_format() {
        let temporary = TempDir::new("format");
        let root = temporary.journal();
        write(&root, "chronicle/20260101/z.txt", b"z");
        write(&root, "chronicle/20260101/a.txt", b"a");
        write(&root, "entities/alice/entity.json", b"alice");
        write(&root, "entities/bob/entity.json", b"bob");
        write(&root, "facets/blue/facet.json", b"blue");
        write(&root, "facets/green/facet.json", b"green");
        write(&root, "facets/red/facet.json", b"red");
        write(&root, "imports/import-1/source.bin", b"source");
        let source = ArchiveSource::open(&root).expect("open source");
        let bytes = archive_bytes(&source);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("open archive");
        let expected = [
            "chronicle/",
            "entities/",
            "facets/",
            "imports/",
            "chronicle/20260101/a.txt",
            "chronicle/20260101/z.txt",
            "entities/alice/entity.json",
            "entities/bob/entity.json",
            "facets/blue/facet.json",
            "facets/green/facet.json",
            "facets/red/facet.json",
            "imports/import-1/source.bin",
            "_export.json",
        ];
        let names: Vec<String> = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .expect("archive member")
                    .name()
                    .to_owned()
            })
            .collect();
        assert_eq!(names, expected);

        for index in 0..4 {
            let member = archive.by_index(index).expect("directory member");
            assert!(member.is_dir());
            assert_eq!(member.compression(), CompressionMethod::Stored);
            assert_eq!(member.unix_mode(), Some(0o40700));
            let timestamp = member.last_modified().expect("directory timestamp");
            assert_eq!(
                (
                    timestamp.year(),
                    timestamp.month(),
                    timestamp.day(),
                    timestamp.hour(),
                    timestamp.minute(),
                    timestamp.second(),
                ),
                (2026, 8, 7, 21, 22, 22)
            );
        }
        let expected_manifest = expected_manifest(&source);
        for index in 4..archive.len() {
            let mut member = archive.by_index(index).expect("file member");
            assert!(!member.is_dir());
            assert_eq!(member.compression(), CompressionMethod::Deflated);
            assert_eq!(member.unix_mode(), Some(0o100600));
            let timestamp = member.last_modified().expect("file timestamp");
            assert_eq!(
                (
                    timestamp.year(),
                    timestamp.month(),
                    timestamp.day(),
                    timestamp.hour(),
                    timestamp.minute(),
                    timestamp.second(),
                ),
                (2026, 8, 7, 21, 22, 22)
            );
            let name = member.name().to_owned();
            let mut contents = Vec::new();
            member.read_to_end(&mut contents).expect("read file member");
            let expected = match name.as_str() {
                "chronicle/20260101/a.txt" => b"a".as_slice(),
                "chronicle/20260101/z.txt" => b"z".as_slice(),
                "entities/alice/entity.json" => b"alice".as_slice(),
                "entities/bob/entity.json" => b"bob".as_slice(),
                "facets/blue/facet.json" => b"blue".as_slice(),
                "facets/green/facet.json" => b"green".as_slice(),
                "facets/red/facet.json" => b"red".as_slice(),
                "imports/import-1/source.bin" => b"source".as_slice(),
                "_export.json" => expected_manifest.as_slice(),
                _ => panic!("unexpected archive file {name}"),
            };
            assert_eq!(contents, expected, "wrong contents for {name}");
        }
    }

    #[test]
    fn empty_journal_contains_only_the_export_manifest() {
        let temporary = TempDir::new("empty-format");
        let root = temporary.journal();
        let source = ArchiveSource::open(&root).expect("open empty source");
        let expected_manifest = expected_manifest(&source);
        let bytes = archive_bytes(&source);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("open archive");

        assert_eq!(archive.len(), 1);
        let mut manifest = archive.by_index(0).expect("empty manifest");
        assert_eq!(manifest.name(), EXPORT_MANIFEST);
        let mut contents = Vec::new();
        manifest
            .read_to_end(&mut contents)
            .expect("read empty manifest");
        assert_eq!(contents, expected_manifest);
    }

    #[test]
    fn forwards_selected_version_and_timestamp_into_archive_metadata() {
        let temporary = TempDir::new("alternate-metadata");
        let root = temporary.journal();
        let source = ArchiveSource::open(&root).expect("open empty source");
        let bytes = archive_bytes_with(&source, "2.3.4", "2040-01-02T03:04:59Z");
        let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("open archive");

        for index in 0..archive.len() {
            let member = archive.by_index(index).expect("archive member");
            let timestamp = member.last_modified().expect("member timestamp");
            assert_eq!(
                (
                    timestamp.year(),
                    timestamp.month(),
                    timestamp.day(),
                    timestamp.hour(),
                    timestamp.minute(),
                    timestamp.second(),
                ),
                (2040, 1, 2, 3, 4, 58),
                "wrong timestamp for {}",
                member.name()
            );
        }

        let mut manifest = archive.by_name(EXPORT_MANIFEST).expect("manifest member");
        let mut contents = Vec::new();
        manifest
            .read_to_end(&mut contents)
            .expect("read alternate manifest");
        let source_journal = source
            .canonical_source()
            .to_str()
            .expect("UTF-8 canonical source");
        assert!(!source_journal.contains('"'));
        assert!(!source_journal.contains('\\'));
        let expected = format!(
            "{{\n  \"solstone_version\": \"2.3.4\",\n  \"exported_at\": \"2040-01-02T03:04:59Z\",\n  \"source_journal\": \"{source_journal}\",\n  \"day_count\": 0,\n  \"entity_count\": 0,\n  \"facet_count\": 0\n}}"
        );
        assert_eq!(contents, expected.as_bytes());
    }

    #[test]
    fn reserves_zip64_fields_with_safe_zip_writer_use() {
        const WRITER_SOURCE: &str = include_str!("writer.rs");
        let forbidden = ["un", "safe"].concat();
        assert!(!WRITER_SOURCE.contains(&forbidden));

        let temporary = TempDir::new("zip64");
        let root = temporary.journal();
        write(&root, "imports/import-1/source.bin", b"source");
        let source = ArchiveSource::open(&root).expect("open source");
        let bytes = archive_bytes(&source);
        assert!(has_zip64_extra(
            &bytes,
            b"imports/import-1/source.bin",
            b"PK\x03\x04"
        ));
        assert!(has_zip64_extra(&bytes, b"_export.json", b"PK\x03\x04"));
        assert!(has_zip64_extra(
            &bytes,
            b"imports/import-1/source.bin",
            b"PK\x01\x02"
        ));
        assert!(has_zip64_extra(&bytes, b"_export.json", b"PK\x01\x02"));
        assert!(!has_zip64_extra(&bytes, b"imports/", b"PK\x03\x04"));
        assert!(!has_zip64_extra(&bytes, b"imports/", b"PK\x01\x02"));

        let expected_manifest = expected_manifest(&source);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("open archive");
        let mut source_member = archive
            .by_name("imports/import-1/source.bin")
            .expect("source member");
        let mut source_bytes = Vec::new();
        source_member
            .read_to_end(&mut source_bytes)
            .expect("read source member");
        assert_eq!(source_bytes, b"source");
        drop(source_member);
        let mut manifest_member = archive.by_name("_export.json").expect("manifest member");
        let mut manifest_bytes = Vec::new();
        manifest_member
            .read_to_end(&mut manifest_bytes)
            .expect("read manifest member");
        assert_eq!(manifest_bytes, expected_manifest);
    }

    #[test]
    fn stale_file_open_preserves_member_provenance() {
        let temporary = TempDir::new("stale-open");
        let root = temporary.journal();
        let member = "imports/import-1/source.bin";
        write(&root, member, b"source");
        let source = ArchiveSource::open(&root).expect("open source");
        fs::remove_file(root.join(member)).expect("remove inventoried member");
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let manifest = manifest_for(&source, "0.9.0", "2026-08-07T21:22:22Z");
        let error = write_archive(&mut writer, &source, &manifest, None)
            .expect_err("stale source must fail");
        assert!(matches!(
            error,
            ArchiveEncodingError::Source {
                member: Some(actual),
                source: ArchiveError::SourceChanged { .. },
            } if actual.as_str() == member
        ));
    }

    #[test]
    fn truncated_file_after_inventory_preserves_member_provenance() {
        let temporary = TempDir::new("truncated");
        let root = temporary.journal();
        let member = "imports/import-1/source.bin";
        write(&root, member, b"source");
        let source = ArchiveSource::open(&root).expect("open source");
        fs::write(root.join(member), b"x").expect("truncate inventoried member");
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let manifest = manifest_for(&source, "0.9.0", "2026-08-07T21:22:22Z");
        let error = write_archive(&mut writer, &source, &manifest, None)
            .expect_err("truncated source must fail");
        assert!(matches!(
            error,
            ArchiveEncodingError::Source {
                member: Some(actual),
                source: ArchiveError::SourceChanged { .. },
            } if actual.as_str() == member
        ));
    }

    #[test]
    fn copy_size_mismatches_preserve_member_provenance() {
        let member = ArchiveMemberName::new("imports/import-1/source.bin".to_owned());
        for (bytes, expected_size) in [(b"short".as_slice(), 6), (b"long".as_slice(), 3)] {
            let mut reader = Cursor::new(bytes);
            let mut output = Vec::new();
            let error = copy_inventoried(&mut reader, &mut output, expected_size, &member)
                .expect_err("size mismatch must fail");
            assert!(matches!(
                error,
                ArchiveEncodingError::Source {
                    member: Some(actual),
                    source: ArchiveError::SourceChanged { .. },
                } if actual == member
            ));
        }
    }

    #[test]
    fn copy_read_failure_preserves_member_provenance() {
        let member = ArchiveMemberName::new("imports/import-1/source.bin".to_owned());
        let mut reader = FailingReader {
            inner: Cursor::new(b"source".to_vec()),
            succeeded: false,
        };
        let mut output = Vec::new();
        let error = copy_inventoried(&mut reader, &mut output, 6, &member)
            .expect_err("reader failure must fail");
        assert!(matches!(
            error,
            ArchiveEncodingError::Source {
                member: Some(actual),
                source: ArchiveError::SourceIo { .. },
            } if actual == member
        ));
    }

    #[test]
    fn copy_requests_at_most_sixty_four_kib() {
        let member = ArchiveMemberName::new("imports/import-1/source.bin".to_owned());
        let bytes = vec![7_u8; COPY_BUFFER_SIZE * 2 + 3];
        let mut reader = RecordingReader {
            inner: Cursor::new(bytes.clone()),
            requests: Vec::new(),
        };
        let mut output = Vec::new();
        copy_inventoried(&mut reader, &mut output, bytes.len() as u64, &member)
            .expect("copy bounded input");
        assert_eq!(output, bytes);
        assert!(
            reader
                .requests
                .iter()
                .all(|request| *request <= COPY_BUFFER_SIZE)
        );
        assert!(reader.requests.len() > 2);
    }

    #[test]
    fn streams_multi_mebibyte_source_end_to_end() {
        let temporary = TempDir::new("large-stream");
        let root = temporary.journal();
        let mut expected = vec![0_u8; 3 * 1024 * 1024 + 17];
        let mut state = 0x1234_5678_9abc_def0_u64;
        for byte in &mut expected {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = (state >> 56) as u8;
        }
        write(&root, "imports/import-1/large.bin", &expected);
        let source = ArchiveSource::open(&root).expect("open source");
        let bytes = archive_bytes(&source);
        let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("open archive");
        let mut member = archive
            .by_name("imports/import-1/large.bin")
            .expect("large member");
        let mut actual = Vec::new();
        member.read_to_end(&mut actual).expect("read large member");
        assert_eq!(actual, expected);
    }

    struct RecordingReader {
        inner: Cursor<Vec<u8>>,
        requests: Vec<usize>,
    }

    impl Read for RecordingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.requests.push(buffer.len());
            self.inner.read(buffer)
        }
    }

    struct FailingReader {
        inner: Cursor<Vec<u8>>,
        succeeded: bool,
    }

    impl Read for FailingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.succeeded {
                return Err(io::Error::other("injected read failure"));
            }
            self.succeeded = true;
            self.inner.read(&mut buffer[..1])
        }
    }

    fn has_zip64_extra(bytes: &[u8], name: &[u8], signature: &[u8; 4]) -> bool {
        let header_size = if signature == b"PK\x03\x04" { 30 } else { 46 };
        let name_offset = if header_size == 30 { 26 } else { 28 };
        let extra_offset = name_offset + 2;
        bytes.windows(4).enumerate().any(|(offset, window)| {
            if window != signature || offset + header_size > bytes.len() {
                return false;
            }
            let name_length = usize::from(read_u16(bytes, offset + name_offset));
            let extra_length = usize::from(read_u16(bytes, offset + extra_offset));
            let name_start = offset + header_size;
            let name_end = name_start + name_length;
            let extra_end = name_end + extra_length;
            name_end <= bytes.len()
                && extra_end <= bytes.len()
                && &bytes[name_start..name_end] == name
                && bytes[name_end..extra_end]
                    .windows(4)
                    .any(|field| field == [1, 0, 16, 0])
        })
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }
}
