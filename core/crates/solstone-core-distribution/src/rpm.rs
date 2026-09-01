// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::digest::sha256_hex;
use crate::record::FileRecord;
use crate::relocate::{from_system_path, to_system_path};
use crate::stage::staged_files;
use crate::tar::gzip_bytes;

const LEAD_LEN: usize = 96;
const HEADER_MAGIC: [u8; 8] = [0x8e, 0xad, 0xe8, 0x01, 0x00, 0x00, 0x00, 0x00];
const RPM_INT16: i32 = 3;
const RPM_INT32: i32 = 4;
const RPM_STRING: i32 = 6;
const RPM_BIN: i32 = 7;
const RPM_STRING_ARRAY: i32 = 8;
const RPM_I18NSTRING: i32 = 9;

const TAG_HEADER_I18N_TABLE: i32 = 100;
const TAG_HEADER_IMMUTABLE: i32 = 63;
const TAG_NAME: i32 = 1000;
const TAG_VERSION: i32 = 1001;
const TAG_RELEASE: i32 = 1002;
const TAG_SUMMARY: i32 = 1004;
const TAG_DESCRIPTION: i32 = 1005;
const TAG_BUILD_TIME: i32 = 1006;
const TAG_BUILD_HOST: i32 = 1007;
const TAG_SIZE: i32 = 1009;
const TAG_LICENSE: i32 = 1014;
const TAG_GROUP: i32 = 1016;
const TAG_OS: i32 = 1021;
const TAG_ARCH: i32 = 1022;
const TAG_FILE_SIZES: i32 = 1028;
const TAG_FILE_MODES: i32 = 1030;
const TAG_FILE_RDEVS: i32 = 1033;
const TAG_FILE_MTIMES: i32 = 1034;
const TAG_FILE_DIGESTS: i32 = 1035;
const TAG_FILE_LINKTOS: i32 = 1036;
const TAG_FILE_FLAGS: i32 = 1037;
const TAG_FILE_USERNAMES: i32 = 1039;
const TAG_FILE_GROUPNAMES: i32 = 1040;
const TAG_SOURCE_RPM: i32 = 1044;
const TAG_FILE_VERIFY_FLAGS: i32 = 1045;
const TAG_ARCHIVE_SIZE: i32 = 1046;
const TAG_PROVIDE_NAME: i32 = 1047;
const TAG_REQUIRE_FLAGS: i32 = 1048;
const TAG_REQUIRENAME: i32 = 1049;
const TAG_REQUIRE_VERSION: i32 = 1050;
const TAG_RPM_VERSION: i32 = 1064;
const TAG_FILE_DEVICES: i32 = 1095;
const TAG_FILE_INODES: i32 = 1096;
const TAG_FILE_LANGS: i32 = 1097;
const TAG_PROVIDE_FLAGS: i32 = 1112;
const TAG_PROVIDE_VERSION: i32 = 1113;
const TAG_DIR_INDEXES: i32 = 1116;
const TAG_BASENAMES: i32 = 1117;
const TAG_DIRNAMES: i32 = 1118;
const TAG_PAYLOAD_FORMAT: i32 = 1124;
const TAG_PAYLOAD_COMPRESSOR: i32 = 1125;
const TAG_PAYLOAD_FLAGS: i32 = 1126;
const TAG_PLATFORM: i32 = 1132;
const TAG_FILE_DIGEST_ALGO: i32 = 5011;
const TAG_ENCODING: i32 = 5062;
const TAG_PAYLOAD_DIGEST: i32 = 5092;
const TAG_PAYLOAD_DIGEST_ALGO: i32 = 5093;

const SIGTAG_HEADER_SIGNATURES: i32 = 62;
const SIGTAG_SHA256_HEADER: i32 = 273;
const SIGTAG_SIZE: i32 = 1000;

const SHA256_ALGORITHM: u32 = 8;
const RPM_SENSE_EQUAL: u32 = 8;
const RPM_SENSE_RPMLIB_LESS_EQUAL: u32 = 0x0100_000a;
const REGULAR_FILE_TYPE: u32 = 0o100000;

pub struct RpmMeta<'a> {
    pub version: &'a str,
    pub arch: &'a str,
}

struct PackageFile {
    archive: String,
    basename: String,
    dirname: String,
    dirname_index: u32,
    bytes: Vec<u8>,
    digest: String,
    mode: u16,
    inode: u32,
}

pub fn write_rpm(stage: &Path, dest: &Path, meta: RpmMeta<'_>) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let files = package_files(stage)?;
    let raw_payload = cpio_bytes(&files)?;
    let payload = gzip_bytes(&raw_payload)?;
    let header = main_header(&files, &raw_payload, &payload, &meta)?;
    let signature = signature_header(&header, &payload)?;

    let mut lead = [0_u8; LEAD_LEN];
    lead[0..4].copy_from_slice(&0xedabeedb_u32.to_be_bytes());
    lead[4] = 3;
    lead[5] = 0;
    let lead_name = format!("solstone-journal-{}-1", meta.version);
    let lead_name = &lead_name.as_bytes()[..lead_name.len().min(65)];
    lead[10..10 + lead_name.len()].copy_from_slice(lead_name);
    lead[76..78].copy_from_slice(&1_u16.to_be_bytes());
    lead[78..80].copy_from_slice(&5_u16.to_be_bytes());

    let mut out = Vec::new();
    out.extend_from_slice(&lead);
    out.extend_from_slice(&signature);
    while out.len() % 8 != 0 {
        out.push(0);
    }
    out.extend_from_slice(&header);
    out.extend_from_slice(&payload);
    fs::write(dest, out)
}

fn package_files(stage: &Path) -> io::Result<Vec<PackageFile>> {
    let mut prepared = Vec::new();
    let mut directories = BTreeMap::<String, u32>::new();
    for (index, dest) in staged_files(stage)?.into_iter().enumerate() {
        let archive = to_system_path(&dest).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unstaged dest {dest} has no system prefix"),
            )
        })?;
        let slash = archive.rfind('/').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("rpm member {archive} has no directory"),
            )
        })?;
        let dirname = format!("/{}/", &archive[..slash]);
        let next_index = directories.len() as u32;
        let dirname_index = *directories.entry(dirname.clone()).or_insert(next_index);
        let path = stage.join(&dest);
        let bytes = fs::read(&path)?;
        let mode = (REGULAR_FILE_TYPE | crate::stage::file_mode(&fs::metadata(&path)?)) as u16;
        prepared.push(PackageFile {
            archive,
            basename: dest.rsplit('/').next().unwrap_or(&dest).to_owned(),
            dirname,
            dirname_index,
            digest: sha256_hex(&bytes),
            bytes,
            mode,
            inode: (index + 1) as u32,
        });
    }
    Ok(prepared)
}

fn signature_header(header: &[u8], payload: &[u8]) -> io::Result<Vec<u8>> {
    let total_size = header
        .len()
        .checked_add(payload.len())
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "rpm exceeds 4 GiB"))?;
    let mut builder = HeaderBuilder::default();
    builder.add_string(SIGTAG_SHA256_HEADER, &sha256_hex(header));
    builder.add_u32(SIGTAG_SIZE, total_size);
    Ok(builder.finish(SIGTAG_HEADER_SIGNATURES))
}

fn main_header(
    files: &[PackageFile],
    raw_payload: &[u8],
    payload: &[u8],
    meta: &RpmMeta<'_>,
) -> io::Result<Vec<u8>> {
    let installed_size = files.iter().try_fold(0_u32, |sum, file| {
        u32::try_from(file.bytes.len())
            .ok()
            .and_then(|size| sum.checked_add(size))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "rpm files exceed 4 GiB"))
    })?;
    let archive_size = u32::try_from(raw_payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "rpm payload exceeds 4 GiB"))?;
    let counts = files.len();
    let zeros = vec![0_u32; counts];
    let zero_devices = vec![0_u16; counts];
    let roots = vec!["root"; counts];
    let empty = vec![""; counts];
    let sizes = files
        .iter()
        .map(|file| u32::try_from(file.bytes.len()).expect("file size checked above"))
        .collect::<Vec<_>>();
    let modes = files.iter().map(|file| file.mode).collect::<Vec<_>>();
    let digests = files
        .iter()
        .map(|file| file.digest.as_str())
        .collect::<Vec<_>>();
    let inodes = files.iter().map(|file| file.inode).collect::<Vec<_>>();
    let dir_indexes = files
        .iter()
        .map(|file| file.dirname_index)
        .collect::<Vec<_>>();
    let basenames = files
        .iter()
        .map(|file| file.basename.as_str())
        .collect::<Vec<_>>();
    let mut dirnames = files
        .iter()
        .map(|file| (file.dirname_index, file.dirname.as_str()))
        .collect::<Vec<_>>();
    dirnames.sort_unstable_by_key(|(index, _)| *index);
    dirnames.dedup_by_key(|(index, _)| *index);
    let dirnames = dirnames
        .into_iter()
        .map(|(_, path)| path)
        .collect::<Vec<_>>();
    let provide_version = format!("{}-1", meta.version);
    let require_names = [
        "libc.so.6()(64bit)",
        "libgcc_s.so.1()(64bit)",
        "libstdc++.so.6()(64bit)",
        "rpmlib(CompressedFileNames)",
        "rpmlib(FileDigests)",
        "rpmlib(PayloadFilesHavePrefix)",
    ];
    let require_versions = ["", "", "", "3.0.4-1", "4.6.0-1", "4.0-1"];
    let require_flags = [
        0,
        0,
        0,
        RPM_SENSE_RPMLIB_LESS_EQUAL,
        RPM_SENSE_RPMLIB_LESS_EQUAL,
        RPM_SENSE_RPMLIB_LESS_EQUAL,
    ];

    let mut builder = HeaderBuilder::default();
    builder.add_string_array(TAG_HEADER_I18N_TABLE, &["C"]);
    builder.add_string(TAG_NAME, "solstone-journal");
    builder.add_string(TAG_VERSION, meta.version);
    builder.add_string(TAG_RELEASE, "1");
    builder.add_i18n_string(TAG_SUMMARY, "solstone journal");
    builder.add_i18n_string(TAG_DESCRIPTION, "Private, local-first personal software");
    builder.add_u32(TAG_BUILD_TIME, 0);
    builder.add_string(TAG_BUILD_HOST, "solstone-cleanroom");
    builder.add_u32(TAG_SIZE, installed_size);
    builder.add_string(TAG_LICENSE, "AGPL-3.0-only");
    builder.add_i18n_string(TAG_GROUP, "Applications/Productivity");
    builder.add_string(TAG_OS, "linux");
    builder.add_string(TAG_ARCH, meta.arch);
    builder.add_u32_array(TAG_FILE_SIZES, &sizes);
    builder.add_u16_array(TAG_FILE_MODES, &modes);
    builder.add_u16_array(TAG_FILE_RDEVS, &zero_devices);
    builder.add_u32_array(TAG_FILE_MTIMES, &zeros);
    builder.add_string_array(TAG_FILE_DIGESTS, &digests);
    builder.add_string_array(TAG_FILE_LINKTOS, &empty);
    builder.add_u32_array(TAG_FILE_FLAGS, &zeros);
    builder.add_string_array(TAG_FILE_USERNAMES, &roots);
    builder.add_string_array(TAG_FILE_GROUPNAMES, &roots);
    builder.add_string(
        TAG_SOURCE_RPM,
        &format!("solstone-journal-{}-1.src.rpm", meta.version),
    );
    builder.add_u32_array(TAG_FILE_VERIFY_FLAGS, &vec![u32::MAX; counts]);
    builder.add_u32(TAG_ARCHIVE_SIZE, archive_size);
    builder.add_string_array(TAG_PROVIDE_NAME, &["solstone-journal"]);
    builder.add_u32_array(TAG_REQUIRE_FLAGS, &require_flags);
    builder.add_string_array(TAG_REQUIRENAME, &require_names);
    builder.add_string_array(TAG_REQUIRE_VERSION, &require_versions);
    builder.add_string(TAG_RPM_VERSION, "solstone-rust-writer-1");
    builder.add_u32_array(TAG_FILE_DEVICES, &vec![1_u32; counts]);
    builder.add_u32_array(TAG_FILE_INODES, &inodes);
    builder.add_string_array(TAG_FILE_LANGS, &empty);
    builder.add_u32_array(TAG_PROVIDE_FLAGS, &[RPM_SENSE_EQUAL]);
    builder.add_string_array(TAG_PROVIDE_VERSION, &[provide_version.as_str()]);
    builder.add_u32_array(TAG_DIR_INDEXES, &dir_indexes);
    builder.add_string_array(TAG_BASENAMES, &basenames);
    builder.add_string_array(TAG_DIRNAMES, &dirnames);
    builder.add_string(TAG_PAYLOAD_FORMAT, "cpio");
    builder.add_string(TAG_PAYLOAD_COMPRESSOR, "gzip");
    builder.add_string(TAG_PAYLOAD_FLAGS, "9");
    builder.add_string(TAG_PLATFORM, &format!("{}-unknown-linux", meta.arch));
    builder.add_u32(TAG_FILE_DIGEST_ALGO, SHA256_ALGORITHM);
    builder.add_string(TAG_ENCODING, "utf-8");
    builder.add_string_array(TAG_PAYLOAD_DIGEST, &[sha256_hex(payload).as_str()]);
    builder.add_u32(TAG_PAYLOAD_DIGEST_ALGO, SHA256_ALGORITHM);
    Ok(builder.finish(TAG_HEADER_IMMUTABLE))
}

#[derive(Default)]
struct HeaderBuilder {
    index: Vec<u8>,
    store: Vec<u8>,
    entries: usize,
}

impl HeaderBuilder {
    fn add_string(&mut self, tag: i32, value: &str) {
        let offset = self.store.len() as i32;
        self.store.extend_from_slice(value.as_bytes());
        self.store.push(0);
        self.push_index(tag, RPM_STRING, offset, 1);
    }

    fn add_i18n_string(&mut self, tag: i32, value: &str) {
        let offset = self.store.len() as i32;
        self.store.extend_from_slice(value.as_bytes());
        self.store.push(0);
        self.push_index(tag, RPM_I18NSTRING, offset, 1);
    }

    fn add_string_array(&mut self, tag: i32, values: &[&str]) {
        let offset = self.store.len() as i32;
        for value in values {
            self.store.extend_from_slice(value.as_bytes());
            self.store.push(0);
        }
        self.push_index(tag, RPM_STRING_ARRAY, offset, values.len() as i32);
    }

    fn add_u16_array(&mut self, tag: i32, values: &[u16]) {
        self.align_store(2);
        let offset = self.store.len() as i32;
        for value in values {
            self.store.extend_from_slice(&value.to_be_bytes());
        }
        self.push_index(tag, RPM_INT16, offset, values.len() as i32);
    }

    fn add_u32(&mut self, tag: i32, value: u32) {
        self.add_u32_array(tag, &[value]);
    }

    fn add_u32_array(&mut self, tag: i32, values: &[u32]) {
        self.align_store(4);
        let offset = self.store.len() as i32;
        for value in values {
            self.store.extend_from_slice(&value.to_be_bytes());
        }
        self.push_index(tag, RPM_INT32, offset, values.len() as i32);
    }

    fn align_store(&mut self, alignment: usize) {
        while !self.store.len().is_multiple_of(alignment) {
            self.store.push(0);
        }
    }

    fn push_index(&mut self, tag: i32, kind: i32, offset: i32, count: i32) {
        push_index(&mut self.index, tag, kind, offset, count);
        self.entries += 1;
    }

    fn finish(mut self, region_tag: i32) -> Vec<u8> {
        let entry_count = self.entries + 1;
        let trailer_offset = self.store.len() as i32;
        let mut region = Vec::with_capacity(16);
        push_index(&mut region, region_tag, RPM_BIN, trailer_offset, 16);
        self.store.extend_from_slice(&region_tag.to_be_bytes());
        self.store.extend_from_slice(&RPM_BIN.to_be_bytes());
        self.store
            .extend_from_slice(&(-(entry_count as i32 * 16)).to_be_bytes());
        self.store.extend_from_slice(&16_i32.to_be_bytes());

        let mut header = HEADER_MAGIC.to_vec();
        header.extend_from_slice(&(entry_count as u32).to_be_bytes());
        header.extend_from_slice(&(self.store.len() as u32).to_be_bytes());
        header.extend_from_slice(&region);
        header.extend_from_slice(&self.index);
        header.extend_from_slice(&self.store);
        header
    }
}

fn push_index(index: &mut Vec<u8>, tag: i32, kind: i32, offset: i32, count: i32) {
    index.extend_from_slice(&tag.to_be_bytes());
    index.extend_from_slice(&kind.to_be_bytes());
    index.extend_from_slice(&offset.to_be_bytes());
    index.extend_from_slice(&count.to_be_bytes());
}

fn cpio_bytes(files: &[PackageFile]) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    for file in files {
        write_cpio_member(
            &mut raw,
            file.inode,
            &format!("./{}", file.archive),
            &file.bytes,
            u32::from(file.mode),
        )?;
    }
    write_cpio_member(&mut raw, files.len() as u32 + 1, "TRAILER!!!", &[], 0)?;
    Ok(raw)
}

fn write_cpio_member(
    out: &mut Vec<u8>,
    ino: u32,
    name: &str,
    bytes: &[u8],
    mode: u32,
) -> io::Result<()> {
    let name_bytes = format!("{name}\0");
    let namesize = name_bytes.len() as u32;
    let filesize = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "cpio member exceeds 4 GiB"))?;
    let mut header = String::from("070701");
    header.push_str(&format!("{ino:08x}"));
    header.push_str(&format!("{mode:08x}"));
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000001");
    header.push_str("00000000");
    header.push_str(&format!("{filesize:08x}"));
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str("00000000");
    header.push_str(&format!("{namesize:08x}"));
    header.push_str("00000000");
    out.write_all(header.as_bytes())?;
    out.write_all(name_bytes.as_bytes())?;
    pad4(out, 6 + 13 * 8 + name_bytes.len())?;
    out.write_all(bytes)?;
    pad4(out, bytes.len())?;
    Ok(())
}

fn pad4(out: &mut Vec<u8>, used: usize) -> io::Result<()> {
    let pad = (4 - (used % 4)) % 4;
    if pad > 0 {
        out.write_all(&[0_u8; 4][..pad])?;
    }
    Ok(())
}

fn skip_headers(bytes: &[u8]) -> io::Result<usize> {
    if bytes.len() < LEAD_LEN + 16 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated rpm"));
    }
    let signature_end = skip_one_header(bytes, LEAD_LEN)?;
    let header_at = align8(signature_end);
    skip_one_header(bytes, header_at)
}

fn skip_one_header(bytes: &[u8], offset: usize) -> io::Result<usize> {
    if offset + 16 > bytes.len() || bytes[offset..offset + 8] != HEADER_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid rpm header magic",
        ));
    }
    let index = u32::from_be_bytes(bytes[offset + 8..offset + 12].try_into().unwrap()) as usize;
    let data = u32::from_be_bytes(bytes[offset + 12..offset + 16].try_into().unwrap()) as usize;
    Ok(offset + 16 + index * 16 + data)
}

pub fn rpm_records(path: &Path) -> io::Result<Vec<FileRecord>> {
    let bytes = fs::read(path)?;
    let start = skip_headers(&bytes)?;
    let raw = crate::tar::gunzip_bytes(&bytes[start..])?;
    read_cpio_records(&raw)
}

fn read_cpio_records(raw: &[u8]) -> io::Result<Vec<FileRecord>> {
    let mut offset = 0;
    let mut records = Vec::new();
    while offset + 110 <= raw.len() {
        if &raw[offset..offset + 6] != b"070701" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid cpio magic",
            ));
        }
        let mode = parse_hex(&raw[offset + 14..offset + 22])?;
        let filesize = parse_hex(&raw[offset + 54..offset + 62])?;
        let namesize = parse_hex(&raw[offset + 94..offset + 102])?;
        let name_start = offset + 110;
        let name_end = name_start + namesize;
        if name_end > raw.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated cpio name",
            ));
        }
        let name = std::str::from_utf8(&raw[name_start..name_end - 1])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let after_name = align4(name_end);
        let data_end = after_name + filesize;
        if name == "TRAILER!!!" {
            break;
        }
        if data_end > raw.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated cpio file",
            ));
        }
        let name = name.strip_prefix("./").unwrap_or(name);
        let dest = from_system_path(name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("rpm member {name} is outside the system prefix"),
            )
        })?;
        let digest = sha256_hex(&raw[after_name..data_end]);
        records.push(FileRecord::file(dest, mode as u32 & 0o7777, digest));
        offset = align4(data_end);
    }
    records.sort();
    Ok(records)
}

fn parse_hex(bytes: &[u8]) -> io::Result<usize> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    usize::from_str_radix(text, 16)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn align8(value: usize) -> usize {
    (value + 7) & !7
}

pub fn rpm_requires(path: &Path) -> io::Result<Vec<String>> {
    let bytes = fs::read(path)?;
    let sig_end = align8(skip_one_header(&bytes, LEAD_LEN)?);
    read_require_names(&bytes[sig_end..])
}

pub fn rpm_arch(path: &Path) -> io::Result<String> {
    let bytes = fs::read(path)?;
    let sig_end = align8(skip_one_header(&bytes, LEAD_LEN)?);
    read_string_tag(&bytes[sig_end..], TAG_ARCH)
}

fn read_string_tag(header: &[u8], wanted: i32) -> io::Result<String> {
    if header.len() < 16 || header[..8] != HEADER_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid rpm header",
        ));
    }
    let index = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
    let data = u32::from_be_bytes(header[12..16].try_into().unwrap()) as usize;
    let store_at = 16 + index * 16;
    if store_at + data > header.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated rpm header store",
        ));
    }
    let store = &header[store_at..store_at + data];
    for slot in 0..index {
        let base = 16 + slot * 16;
        let tag = i32::from_be_bytes(header[base..base + 4].try_into().unwrap());
        if tag != wanted {
            continue;
        }
        let offset = i32::from_be_bytes(header[base + 8..base + 12].try_into().unwrap()) as usize;
        let end = store[offset..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unterminated rpm string"))?;
        return std::str::from_utf8(&store[offset..offset + end])
            .map(str::to_owned)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("rpm tag {wanted} missing"),
    ))
}

fn read_require_names(header: &[u8]) -> io::Result<Vec<String>> {
    if header.len() < 16 || header[..8] != HEADER_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid rpm header",
        ));
    }
    let index = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
    let data = u32::from_be_bytes(header[12..16].try_into().unwrap()) as usize;
    let store_at = 16 + index * 16;
    if store_at + data > header.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated rpm header store",
        ));
    }
    let store = &header[store_at..store_at + data];
    for slot in 0..index {
        let base = 16 + slot * 16;
        let tag = i32::from_be_bytes(header[base..base + 4].try_into().unwrap());
        if tag != TAG_REQUIRENAME {
            continue;
        }
        let offset = i32::from_be_bytes(header[base + 8..base + 12].try_into().unwrap()) as usize;
        let count = i32::from_be_bytes(header[base + 12..base + 16].try_into().unwrap()) as usize;
        let mut names = Vec::new();
        let mut cursor = offset;
        for _ in 0..count {
            let end = store[cursor..]
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "unterminated require")
                })?;
            names.push(
                std::str::from_utf8(&store[cursor..cursor + end])
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                    .to_owned(),
            );
            cursor += end + 1;
        }
        return Ok(names);
    }
    Ok(Vec::new())
}
